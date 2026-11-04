//! A running program: one reservation for the whole layout, pages committed only where bytes are live.
use crate::layout::{up, CKind, Program, Scalar, TCol, Ty};
use crate::os::{self, PAGE};

pub struct Instance {
    pub prog: Program,
    base: *mut u8,
    reserved: usize,
    committed: Vec<bool>, // one flag per page
    pub last_trim: TrimStats,
}

/// What the last `trim` cost: live ranges scanned, OS release calls made, time in each phase.
#[derive(Clone, Copy, Default, Debug)]
pub struct TrimStats { pub ranges: usize, pub os_calls: usize, pub scan_ns: u128, pub os_ns: u128 }

/// Where a path points. Strings and lists inside table rows keep their length in a separate column.
enum Loc { Mem(usize, Ty), Str { len: usize, bytes: usize, cap: usize }, List { len: usize, data: usize, cap: usize, elem: Scalar } }

#[derive(Debug)]
pub enum Error { Full(String), Bad(String) }
fn bad<T>(s: impl Into<String>) -> Result<T, Error> { Err(Error::Bad(s.into())) }

type Rows = Vec<(String, String, usize, usize, usize)>;

impl Instance {
    /// Reserve the whole layout, refusing to start if it is over `budget` bytes.
    pub fn start(prog: Program, budget: usize) -> Result<Instance, String> {
        if prog.total > budget { return Err(format!("over budget: layout needs {} bytes, budget is {}", prog.total, budget)); }
        let reserved = up(prog.total.max(1), PAGE);
        let mut inst = Instance { base: os::reserve(reserved), reserved, committed: vec![false; reserved / PAGE], prog, last_trim: TrimStats::default() };
        let mut r = Vec::new();
        for (_, ty, off) in &inst.prog.globals { inst.ranges(ty, *off, true, &mut r); }
        for (a, b) in r { inst.ensure(a, b); }
        Ok(inst)
    }

    pub fn reserved(&self) -> usize { self.reserved }
    pub fn committed(&self) -> usize { self.committed.iter().filter(|c| **c).count() * PAGE }

    /// Commit every page touching bytes a..b.
    fn ensure(&mut self, a: usize, b: usize) {
        if b <= a { return; }
        let (mut p, last) = (a / PAGE, (b - 1) / PAGE);
        while p <= last {
            if !self.committed[p] {
                let s = p;
                while p <= last && !self.committed[p] { p += 1; }
                assert!(os::commit(unsafe { self.base.add(s * PAGE) }, (p - s) * PAGE), "commit failed");
                for q in s..p { self.committed[q] = true; }
            } else { p += 1; }
        }
    }

    fn rd(&self, off: usize, n: usize) -> u64 { let mut v = 0u64; unsafe { std::ptr::copy_nonoverlapping(self.base.add(off), &mut v as *mut u64 as *mut u8, n) }; v }
    fn wr(&mut self, off: usize, n: usize, v: u64) { self.ensure(off, off + n); unsafe { std::ptr::copy_nonoverlapping(&v as *const u64 as *const u8, self.base.add(off), n) }; }
    fn cols(&self, row: &Ty, n: usize) -> Vec<TCol> { self.prog.table_cols(row, n).unwrap().0 }

    /// Collect the byte ranges in use for `ty` placed at `off`. `fresh` treats every list/string as empty.
    fn ranges(&self, ty: &Ty, off: usize, fresh: bool, out: &mut Vec<(usize, usize)>) {
        let p = &self.prog;
        if p.all_live(ty) { let (s, _) = p.size_align(ty); out.push((off, off + s)); return; }
        match ty {
            Ty::Str(_) => { let len = if fresh { 0 } else { self.rd(off, 4) as usize }; out.push((off, off + 4 + len)); }
            Ty::List(e, _) => {
                let len = if fresh { 0 } else { self.rd(off, 4) as usize };
                let (es, _) = p.size_align(e); let h = p.list_header(e);
                out.push((off, off + 4));
                for i in 0..len { self.ranges(e, off + h + i * es, fresh, out); }
            }
            Ty::Table(row, n) => self.table_ranges(row, *n, off, fresh, None, out),
            Ty::Arr(e, n) => { let (es, _) = p.size_align(e); for i in 0..*n { self.ranges(e, off + i * es, fresh, out); } }
            Ty::Struct(i) => for f in &p.structs[*i].fields { self.ranges(&f.1, off + f.2, fresh, out); },
            _ => {}
        }
    }

    /// Ranges in use for a table; `only` restricts to one field path (for `inspect`).
    fn table_ranges(&self, row: &Ty, n: usize, off: usize, fresh: bool, only: Option<&str>, out: &mut Vec<(usize, usize)>) {
        let len = if fresh { 0 } else { self.rd(off, 4) as usize };
        if only.is_none() { out.push((off, off + 4)); }
        let cols = self.cols(row, n);
        for (ci, c) in cols.iter().enumerate() {
            if only.map_or(false, |o| o != c.path) { continue; }
            let a = off + c.off;
            match c.kind {
                CKind::Scalar(s) => out.push((a, a + len * s.size())),
                CKind::Arr(s, k) => out.push((a, a + len * k * s.size())),
                CKind::StrLen | CKind::ListLen => out.push((a, a + len * 4)),
                CKind::StrBytes(m) => { let lc = off + cols[ci - 1].off; for r in 0..len { let l = self.rd(lc + 4 * r, 4) as usize; if l > 0 { out.push((a + r * m, a + r * m + l)); } } }
                CKind::ListData(s, m) => { let lc = off + cols[ci - 1].off; for r in 0..len { let l = self.rd(lc + 4 * r, 4) as usize; if l > 0 { out.push((a + r * m * s.size(), a + (r * m + l) * s.size())); } } }
            }
        }
    }

    pub fn live_of(&self, ty: &Ty, off: usize) -> usize { let mut r = Vec::new(); self.ranges(ty, off, false, &mut r); r.iter().map(|(a, b)| b - a).sum() }
    pub fn live_total(&self) -> usize { self.prog.globals.iter().map(|(_, t, o)| self.live_of(t, *o)).sum() }

    /// Return pages that hold no live byte to the OS. Returns bytes released.
    pub fn trim(&mut self) -> usize {
        let t0 = std::time::Instant::now();
        let mut r = Vec::new();
        for (_, ty, off) in &self.prog.globals { self.ranges(ty, *off, false, &mut r); }
        let pages = self.committed.len();
        self.release_dead(r, 0, pages, t0)
    }

    /// Trim only the pages wholly inside `path` (a global or a field of one). Cost follows the size of that part,
    /// not the whole program. Pages shared with neighbours are left alone.
    pub fn trim_path(&mut self, path: &str) -> Result<usize, Error> {
        let t0 = std::time::Instant::now();
        let Loc::Mem(off, ty) = self.resolve(path)? else { return bad("trim a list, table, struct or array") };
        let size = self.prog.size_align(&ty).0;
        let mut r = Vec::new();
        self.ranges(&ty, off, false, &mut r);
        Ok(self.release_dead(r, up(off, PAGE) / PAGE, (off + size) / PAGE, t0))
    }

    /// Decommit committed pages in `lo..hi` that no live range touches.
    fn release_dead(&mut self, ranges: Vec<(usize, usize)>, lo: usize, hi: usize, t0: std::time::Instant) -> usize {
        let mut live = vec![false; self.committed.len()];
        let mut st = TrimStats { ranges: ranges.len(), ..Default::default() };
        for (a, b) in ranges { if b > a { for p in a / PAGE..=(b - 1) / PAGE { live[p] = true; } } }
        st.scan_ns = t0.elapsed().as_nanos();
        let t1 = std::time::Instant::now();
        let (mut freed, mut p) = (0, lo);
        while p < hi {
            if self.committed[p] && !live[p] {
                let s = p;
                while p < hi && self.committed[p] && !live[p] { p += 1; }
                os::decommit(unsafe { self.base.add(s * PAGE) }, (p - s) * PAGE);
                st.os_calls += 1;
                for q in s..p { self.committed[q] = false; }
                freed += (p - s) * PAGE;
            } else { p += 1; }
        }
        st.os_ns = t1.elapsed().as_nanos();
        self.last_trim = st;
        freed
    }

    // ---- paths: world.items[3].name ----
    fn resolve(&self, path: &str) -> Result<Loc, Error> {
        let mut segs: Vec<(String, Vec<usize>)> = Vec::new();
        for part in path.split('.') {
            let (name, mut rest) = match part.find('[') { Some(i) => (&part[..i], &part[i..]), None => (part, "") };
            let mut idx = Vec::new();
            while let Some(r) = rest.strip_prefix('[') {
                let e = r.find(']').ok_or_else(|| Error::Bad("missing `]`".into()))?;
                idx.push(r[..e].parse().map_err(|_| Error::Bad("bad index".into()))?);
                rest = &r[e + 1..];
            }
            segs.push((name.to_string(), idx));
        }
        let (_, gty, goff) = self.prog.globals.iter().find(|g| g.0 == segs[0].0).ok_or_else(|| Error::Bad(format!("no global `{}`", segs[0].0)))?;
        let (mut off, mut ty) = (*goff, gty.clone());
        let mut si = 0;
        while si < segs.len() {
            let (name, idx) = &segs[si];
            if si > 0 {
                let Ty::Struct(i) = ty else { return bad(format!("`{}` is not a struct", name)) };
                let f = self.prog.structs[i].fields.iter().find(|f| &f.0 == name).ok_or_else(|| Error::Bad(format!("no field `{}`", name)))?;
                off += f.2; ty = f.1.clone();
            }
            let mut idx = idx.as_slice();
            while let Some((&k, more)) = idx.split_first() {
                idx = more;
                match ty.clone() {
                    Ty::Arr(e, n) => { if k >= n { return bad(format!("index {} out of range (fixed size {})", k, n)); } off += k * self.prog.size_align(&e).0; ty = *e; }
                    Ty::List(e, _) => {
                        let len = self.rd(off, 4) as usize;
                        if k >= len { return bad(format!("index {} out of range (live length {})", k, len)); }
                        off += self.prog.list_header(&e) + k * self.prog.size_align(&e).0; ty = *e;
                    }
                    Ty::Table(row, n) => {
                        let len = self.rd(off, 4) as usize;
                        if k >= len { return bad(format!("row {} out of range (live rows {})", k, len)); }
                        return self.table_field(off, &row, n, k, &segs[si + 1..], idx);
                    }
                    _ => return bad("cannot index here"),
                }
            }
            si += 1;
        }
        Ok(match ty {
            Ty::Str(cap) => Loc::Str { len: off, bytes: off + 4, cap },
            t => Loc::Mem(off, t),
        })
    }

    /// Locate field `rest` (e.g. `pos.x` or `name`) of row `k`.
    fn table_field(&self, toff: usize, row: &Ty, n: usize, k: usize, rest: &[(String, Vec<usize>)], idx: &[usize]) -> Result<Loc, Error> {
        if !idx.is_empty() || rest.is_empty() { return bad("name a field of the row, e.g. items[3].hp"); }
        if rest[..rest.len() - 1].iter().any(|r| !r.1.is_empty()) { return bad("cannot index inside a row"); }
        let path = rest.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(".");
        let last_idx = rest[rest.len() - 1].1.as_slice();
        let cols = self.cols(row, n);
        let ci = cols.iter().position(|c| c.path == path).ok_or_else(|| Error::Bad(format!("table row has no field `{}`", path)))?;
        let c = &cols[ci];
        let a = toff + c.off;
        match c.kind {
            CKind::Scalar(s) => if last_idx.is_empty() { Ok(Loc::Mem(a + k * s.size(), Ty::Scalar(s))) } else { bad("not an array") },
            CKind::Arr(s, kk) => match last_idx {
                [] => Ok(Loc::Mem(a + k * kk * s.size(), Ty::Arr(Box::new(Ty::Scalar(s)), kk))),
                [j] => { if *j >= kk { return bad(format!("index {} out of range (fixed size {})", j, kk)); } Ok(Loc::Mem(a + (k * kk + j) * s.size(), Ty::Scalar(s))) }
                _ => bad("too many indexes"),
            },
            CKind::StrLen => {
                let CKind::StrBytes(m) = cols[ci + 1].kind else { unreachable!() };
                Ok(Loc::Str { len: a + 4 * k, bytes: toff + cols[ci + 1].off + k * m, cap: m })
            }
            CKind::ListLen => {
                let CKind::ListData(s, m) = cols[ci + 1].kind else { unreachable!() };
                let (len_off, data) = (a + 4 * k, toff + cols[ci + 1].off + k * m * s.size());
                match last_idx {
                    [] => Ok(Loc::List { len: len_off, data, cap: m, elem: s }),
                    [j] => { let l = self.rd(len_off, 4) as usize; if *j >= l { return bad(format!("index {} out of range (live length {})", j, l)); } Ok(Loc::Mem(data + j * s.size(), Ty::Scalar(s))) }
                    _ => bad("too many indexes"),
                }
            }
            _ => bad("internal column"),
        }
    }

    pub fn push(&mut self, path: &str) -> Result<usize, Error> {
        match self.resolve(path)? {
            Loc::Mem(off, Ty::List(e, cap)) => {
                let len = self.rd(off, 4) as usize;
                if len == cap { return Err(Error::Full(format!("{} is full ({} of {})", path, len, cap))); }
                let eoff = off + self.prog.list_header(&e) + len * self.prog.size_align(&e).0;
                let mut r = Vec::new(); self.ranges(&e, eoff, true, &mut r);
                for (a, b) in r { self.ensure(a, b); unsafe { std::ptr::write_bytes(self.base.add(a), 0, b - a) }; } // fresh element: zeroed, empty inside
                self.wr(off, 4, len as u64 + 1);
                Ok(len)
            }
            Loc::Mem(off, Ty::Table(row, cap)) => {
                let len = self.rd(off, 4) as usize;
                if len == cap { return Err(Error::Full(format!("{} is full ({} of {})", path, len, cap))); }
                for c in self.cols(&row, cap) {
                    let a = off + c.off;
                    let (at, n) = match c.kind {
                        CKind::Scalar(s) => (a + len * s.size(), s.size()),
                        CKind::Arr(s, k) => (a + len * k * s.size(), k * s.size()),
                        CKind::StrLen | CKind::ListLen => (a + len * 4, 4),
                        _ => continue,
                    };
                    self.ensure(at, at + n); unsafe { std::ptr::write_bytes(self.base.add(at), 0, n) };
                }
                self.wr(off, 4, len as u64 + 1);
                Ok(len)
            }
            Loc::List { len, data, cap, elem } => {
                let l = self.rd(len, 4) as usize;
                if l == cap { return Err(Error::Full(format!("{} is full ({} of {})", path, l, cap))); }
                let at = data + l * elem.size();
                self.ensure(at, at + elem.size()); unsafe { std::ptr::write_bytes(self.base.add(at), 0, elem.size()) };
                self.wr(len, 4, l as u64 + 1);
                Ok(l)
            }
            _ => bad("not a list or table"),
        }
    }
    fn len_slot(&self, path: &str) -> Result<usize, Error> {
        match self.resolve(path)? {
            Loc::Mem(off, Ty::List(..)) | Loc::Mem(off, Ty::Table(..)) | Loc::Mem(off, Ty::Str(_)) => Ok(off),
            Loc::List { len, .. } | Loc::Str { len, .. } => Ok(len),
            _ => bad("not a list, table or string"),
        }
    }
    pub fn pop(&mut self, path: &str) -> Result<(), Error> {
        let at = self.len_slot(path)?;
        let len = self.rd(at, 4);
        if len == 0 { return bad("already empty"); }
        self.wr(at, 4, len - 1); Ok(())
    }
    pub fn clear(&mut self, path: &str) -> Result<(), Error> { let at = self.len_slot(path)?; self.wr(at, 4, 0); Ok(()) }

    /// Remove row `k` of a table by moving the last row into its place (rows stay dense; order changes).
    pub fn swap_remove(&mut self, path: &str, k: usize) -> Result<(), Error> {
        let Loc::Mem(off, Ty::Table(row, cap)) = self.resolve(path)? else { return bad("not a table") };
        let len = self.rd(off, 4) as usize;
        if k >= len { return bad(format!("row {} out of range (live rows {})", k, len)); }
        let last = len - 1;
        let cols = self.cols(&row, cap);
        for (ci, c) in cols.iter().enumerate() {
            let a = off + c.off;
            let copy = |me: &mut Self, from: usize, to: usize, n: usize| { me.ensure(to, to + n); unsafe { std::ptr::copy(me.base.add(from), me.base.add(to), n) } };
            match c.kind {
                CKind::Scalar(s) => copy(self, a + last * s.size(), a + k * s.size(), s.size()),
                CKind::Arr(s, kk) => copy(self, a + last * kk * s.size(), a + k * kk * s.size(), kk * s.size()),
                CKind::StrLen | CKind::ListLen => copy(self, a + last * 4, a + k * 4, 4),
                CKind::StrBytes(m) => { let l = self.rd(off + cols[ci - 1].off + 4 * last, 4) as usize; copy(self, a + last * m, a + k * m, l) }
                CKind::ListData(s, m) => { let l = self.rd(off + cols[ci - 1].off + 4 * last, 4) as usize; copy(self, a + last * m * s.size(), a + k * m * s.size(), l * s.size()) }
            }
        }
        self.wr(off, 4, last as u64); Ok(())
    }

    pub fn set(&mut self, path: &str, val: &str) -> Result<(), Error> {
        match self.resolve(path)? {
            Loc::Str { len, bytes, cap } => {
                let s = val.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(val);
                if s.len() > cap { return Err(Error::Full(format!("string of {} bytes does not fit string<{}>", s.len(), cap))); }
                self.ensure(bytes, bytes + s.len());
                unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), self.base.add(bytes), s.len()) };
                self.wr(len, 4, s.len() as u64); Ok(())
            }
            Loc::Mem(off, Ty::Scalar(sc)) => {
                let bits = match sc {
                    Scalar::F32 => val.parse::<f32>().map(|f| f.to_bits() as u64).map_err(|e| e.to_string()),
                    Scalar::F64 => val.parse::<f64>().map(|f| f.to_bits()).map_err(|e| e.to_string()),
                    Scalar::Bool => match val { "true" | "1" => Ok(1), "false" | "0" => Ok(0), _ => Err("expected true or false".into()) },
                    _ => val.parse::<i64>().map(|v| v as u64).map_err(|e| e.to_string()),
                };
                match bits { Ok(b) => { self.wr(off, sc.size(), b); Ok(()) } Err(e) => bad(e) }
            }
            _ => bad("can only set scalars and strings"),
        }
    }
    pub fn get(&self, path: &str) -> Result<String, Error> {
        Ok(match self.resolve(path)? {
            Loc::Str { len, bytes, .. } => { let n = self.rd(len, 4) as usize; format!("{:?}", String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(self.base.add(bytes), n) })) }
            Loc::List { len, .. } => format!("len {}", self.rd(len, 4)),
            Loc::Mem(off, Ty::List(..)) | Loc::Mem(off, Ty::Table(..)) => format!("len {}", self.rd(off, 4)),
            Loc::Mem(off, Ty::Scalar(s)) => self.show_scalar(off, s),
            _ => return bad("can only get scalars, strings and list lengths"),
        })
    }
    fn show_scalar(&self, off: usize, s: Scalar) -> String {
        match s {
            Scalar::F32 => f32::from_bits(self.rd(off, 4) as u32).to_string(),
            Scalar::F64 => f64::from_bits(self.rd(off, 8)).to_string(),
            _ => { let v = self.rd(off, s.size()); let sh = 64 - 8 * s.size() as u32; if matches!(s, Scalar::I8 | Scalar::I16 | Scalar::I32 | Scalar::I64) { (((v << sh) as i64) >> sh).to_string() } else { v.to_string() } }
        }
    }

    /// Sum a scalar field over every live row with a native loop. Returns (sum, rows, ns per row).
    /// Works on `list<Struct>` (strided) and `table<Struct>` (contiguous column) so the layouts can be compared.
    pub fn sum(&self, path: &str, field: &str, reps: usize) -> Result<(f64, usize, f64), Error> {
        let Loc::Mem(off, ty) = self.resolve(path)? else { return bad("not a list or table") };
        let (base, stride, len, sc) = match &ty {
            Ty::Table(row, n) => {
                let cols = self.cols(row, *n);
                let c = cols.iter().find(|c| c.path == field).ok_or_else(|| Error::Bad(format!("no field `{}`", field)))?;
                let CKind::Scalar(s) = c.kind else { return bad("sum needs a scalar field") };
                (off + c.off, s.size(), self.rd(off, 4) as usize, s)
            }
            Ty::List(e, _) => {
                let Ty::Struct(i) = &**e else { return bad("list of structs needed") };
                let f = self.prog.structs[*i].fields.iter().find(|f| f.0 == field).ok_or_else(|| Error::Bad(format!("no field `{}`", field)))?;
                let Ty::Scalar(s) = f.1 else { return bad("sum needs a scalar field") };
                (off + self.prog.list_header(e) + f.2, self.prog.size_align(e).0, self.rd(off, 4) as usize, s)
            }
            _ => return bad("not a list or table"),
        };
        if !matches!(sc, Scalar::F32 | Scalar::I32 | Scalar::U32) { return bad("sum supports f32, i32, u32"); }
        let p = unsafe { self.base.add(base) };
        let t = std::time::Instant::now();
        let mut total = 0.0;
        for _ in 0..reps {
            let mut acc = 0.0f64;
            for r in 0..len {
                let q = unsafe { p.add(r * stride) };
                acc += unsafe { match sc { Scalar::F32 => (q as *const f32).read_unaligned() as f64, Scalar::I32 => (q as *const i32).read_unaligned() as f64, _ => (q as *const u32).read_unaligned() as f64 } };
            }
            total = std::hint::black_box(acc);
        }
        let ns = t.elapsed().as_nanos() as f64 / (reps * len.max(1)) as f64;
        Ok((total, len, ns))
    }

    /// Rows of (path, type, offset, reserved, live) for every list, table, string and fixed array reachable through structs.
    pub fn inspect(&self) -> Rows {
        let mut rows = Vec::new();
        for (n, ty, off) in &self.prog.globals { self.rows(n, ty, *off, &mut rows); }
        rows
    }
    fn rows(&self, path: &str, ty: &Ty, off: usize, out: &mut Rows) {
        let p = &self.prog;
        let row = |out: &mut Rows| out.push((path.to_string(), p.show(ty), off, p.size_align(ty).0, self.live_of(ty, off)));
        match ty {
            Ty::Struct(i) => { row(out); for f in &p.structs[*i].fields { if !matches!(f.1, Ty::Scalar(_)) { self.rows(&format!("{}.{}", path, f.0), &f.1, off + f.2, out); } } }
            Ty::Scalar(_) => {}
            Ty::Table(r, n) => {
                row(out);
                let cols = self.cols(r, *n);
                let mut seen: Vec<&str> = Vec::new();
                for c in &cols {
                    if seen.contains(&c.path.as_str()) { continue; }
                    seen.push(&c.path);
                    let mut live = Vec::new();
                    self.table_ranges(r, *n, off, false, Some(&c.path), &mut live);
                    let res: usize = cols.iter().filter(|x| x.path == c.path).map(|x| match x.kind {
                        CKind::Scalar(s) => n * s.size(), CKind::Arr(s, k) => n * k * s.size(), CKind::StrLen | CKind::ListLen => n * 4,
                        CKind::StrBytes(m) => n * m, CKind::ListData(s, m) => n * m * s.size() }).sum();
                    let kind = match c.kind { CKind::Scalar(s) => format!("{:?}", s).to_lowercase(), CKind::Arr(s, k) => format!("{:?}[{}]", s, k).to_lowercase(), CKind::StrLen => "string".into(), _ => "list".into() };
                    out.push((format!("{}.{}", path, c.path), format!("  column {}", kind), off + c.off, res, live.iter().map(|(a, b)| b - a).sum()));
                }
            }
            _ => row(out),
        }
    }
}

impl Drop for Instance { fn drop(&mut self) { os::release(self.base); } }

#[cfg(test)]
mod tests {
    use super::*;
    const SRC: &str = "struct V { x: f32; y: f32; } struct I { id: i32; p: V; name: string<8>; tags: list<i32, 4>; } global w: table<I, 1000>; global l: list<I, 1000>;";
    fn inst() -> Instance { Instance::start(Program::parse(SRC).unwrap(), 1 << 30).unwrap() }

    #[test]
    fn table_and_list_hold_the_same_data() {
        let mut i = inst();
        for p in ["w", "l"] {
            i.push(p).unwrap(); i.push(p).unwrap();
            i.set(&format!("{}[1].id", p), "9").ok(); // list: ok; table: ok
        }
        i.set("w[1].id", "9").unwrap(); i.set("w[1].p.y", "2.5").unwrap(); i.set("w[1].name", "abc").unwrap();
        i.push("w[1].tags").unwrap(); i.set("w[1].tags[0]", "7").unwrap();
        assert_eq!(i.get("w[1].id").unwrap(), "9");
        assert_eq!(i.get("w[1].p.y").unwrap(), "2.5");
        assert_eq!(i.get("w[1].name").unwrap(), "\"abc\"");
        assert_eq!(i.get("w[1].tags[0]").unwrap(), "7");
        assert_eq!(i.get("l[1].id").unwrap(), "9");
    }

    #[test]
    fn limits_and_liveness_are_typed_errors() {
        let mut i = inst();
        i.push("w").unwrap();
        assert!(matches!(i.set("w[0].name", "123456789"), Err(Error::Full(_))));
        assert!(matches!(i.get("w[1].id"), Err(Error::Bad(_)))); // row 1 is not live
        for _ in 0..4 { i.push("w[0].tags").unwrap(); }
        assert!(matches!(i.push("w[0].tags"), Err(Error::Full(_))));
        assert!(matches!(i.get("w[0].tags[4]"), Err(Error::Bad(_))));
    }

    #[test]
    fn swap_remove_keeps_rows_dense_and_moves_strings() {
        let mut i = inst();
        for k in 0..3 { i.push("w").unwrap(); i.set(&format!("w[{}].id", k), &k.to_string()).unwrap(); i.set(&format!("w[{}].name", k), &format!("n{}", k)).unwrap(); }
        i.swap_remove("w", 0).unwrap();
        assert_eq!(i.get("w").unwrap(), "len 2");
        assert_eq!(i.get("w[0].id").unwrap(), "2");
        assert_eq!(i.get("w[0].name").unwrap(), "\"n2\"");
        assert_eq!(i.get("w[1].name").unwrap(), "\"n1\"");
    }

    #[test]
    fn committed_tracks_live_after_trim() {
        let mut i = inst();
        for _ in 0..1000 { i.push("w").unwrap(); }
        i.set("w[3].name", "x").unwrap();
        let before = i.committed();
        i.clear("w").unwrap();
        assert!(i.trim() > 0 && i.committed() < before);
        i.push("w").unwrap();
        i.set("w[0].name", "again").unwrap(); // re-commits what trim released
        assert_eq!(i.get("w[0].name").unwrap(), "\"again\"");
    }
}
