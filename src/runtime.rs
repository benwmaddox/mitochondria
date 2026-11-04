//! A running program: one reservation for the whole layout, pages committed only where bytes are live.
use crate::layout::{up, CKind, Program, Scalar, TCol, Ty};
use crate::os::{self, PAGE};

pub struct Instance {
    pub prog: Program,
    base: *mut u8,
    reserved: usize,
    committed: Vec<bool>, // one flag per page
    pub last_trim: TrimStats,
    gcols: Vec<Option<(Vec<TCol>, u8)>>, // columns of each table/pool global, computed once
}

/// What the last `trim` cost: live ranges scanned, OS release calls made, time in each phase.
#[derive(Clone, Copy, Default, Debug)]
pub struct TrimStats { pub ranges: usize, pub os_calls: usize, pub scan_ns: u128, pub os_ns: u128 }

/// Where a path points. Strings and lists inside table/pool rows keep their length in a separate column.
enum Loc { Mem(usize, Ty), Str { len: usize, bytes: usize, cap: usize }, List { len: usize, data: usize, cap: usize, elem: Scalar } }

#[derive(Debug)]
pub enum Error { Full(String), TooLong(String), Bad(String) }
fn bad<T>(s: impl Into<String>) -> Result<T, Error> { Err(Error::Bad(s.into())) }

type Rows = Vec<(String, String, usize, usize, usize)>;
type Idx = Vec<(usize, Option<u32>)>; // [k] or [k@generation]

// Pool header: high-water mark, live count, lowest-free hint, ever-allocated mark.
const HW: usize = 0;
const COUNT: usize = 4;
const HINT: usize = 8;
const EVER: usize = 12;

impl Instance {
    /// Reserve the whole layout, refusing to start if it is over `budget` bytes.
    pub fn start(prog: Program, budget: usize) -> Result<Instance, String> {
        if prog.total > budget { return Err(format!("over budget: layout needs {} bytes, budget is {}", prog.total, budget)); }
        let reserved = up(prog.total.max(1), PAGE);
        let mut inst = Instance { base: os::reserve(reserved), reserved, committed: vec![false; reserved / PAGE], prog, last_trim: TrimStats::default(), gcols: Vec::new() };
        inst.gcols = inst.prog.globals.iter().map(|g| inst.columnar(&g.1).map(|(_, c, k)| (c, k))).collect();
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
    fn zero(&mut self, a: usize, n: usize) { self.ensure(a, a + n); unsafe { std::ptr::write_bytes(self.base.add(a), 0, n) }; }
    fn cols(&self, row: &Ty, n: usize) -> Vec<TCol> { self.prog.table_cols(row, n).unwrap().0 }
    /// Columns of a table or pool type: (capacity, columns, kind) where kind is 0 table, 1 pool, 2 checked pool.
    fn columnar(&self, ty: &Ty) -> Option<(usize, Vec<TCol>, u8)> {
        match ty {
            Ty::Table(r, n) => Some((*n, self.prog.table_cols(r, *n).unwrap().0, 0)),
            Ty::Pool(r, n, c) => Some((*n, self.prog.pool_cols(r, *n, *c).unwrap().0, 1 + *c as u8)),
            _ => None,
        }
    }
    /// Is slot `k` live? Only valid for k below the pool's high-water mark.
    fn bit(&self, live_off: usize, k: usize) -> bool { (self.rd(live_off + (k / 64) * 8, 8) >> (k % 64)) & 1 == 1 }

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
            Ty::Table(..) | Ty::Pool(..) => self.columnar_ranges(ty, off, fresh, None, out),
            Ty::Arr(e, n) => { let (es, _) = p.size_align(e); for i in 0..*n { self.ranges(e, off + i * es, fresh, out); } }
            Ty::Struct(i) => for f in &p.structs[*i].fields { self.ranges(&f.1, off + f.2, fresh, out); },
            _ => {}
        }
    }

    /// Ranges in use for a table or pool; `only` restricts to one column path (for `inspect`).
    fn columnar_ranges(&self, ty: &Ty, off: usize, fresh: bool, only: Option<&str>, out: &mut Vec<(usize, usize)>) {
        let Some((_, cols, kind)) = self.columnar(ty) else { return };
        let pool = kind > 0;
        let (hw, ever) = if fresh { (0, 0) } else { (self.rd(off + HW, 4) as usize, if pool { self.rd(off + EVER, 4) as usize } else { 0 }) };
        if only.is_none() { out.push((off, off + if pool { 16 } else { 4 })); }
        let live_off = off + cols[0].off;
        for (ci, c) in cols.iter().enumerate() {
            if only.map_or(false, |o| o != c.path) { continue; }
            let a = off + c.off;
            let dead = |r: usize| pool && !self.bit(live_off, r);
            match c.kind {
                CKind::Live => out.push((a, a + hw.div_ceil(64) * 8)),
                CKind::Gen => out.push((a, a + ever * 4)),
                CKind::Scalar(s) => out.push((a, a + hw * s.size())),
                CKind::Arr(s, k) => out.push((a, a + hw * k * s.size())),
                CKind::StrLen | CKind::ListLen => out.push((a, a + hw * 4)),
                CKind::StrBytes(m) => { let lc = off + cols[ci - 1].off; for r in 0..hw { if dead(r) { continue; } let l = self.rd(lc + 4 * r, 4) as usize; if l > 0 { out.push((a + r * m, a + r * m + l)); } } }
                CKind::ListData(s, m) => { let lc = off + cols[ci - 1].off; for r in 0..hw { if dead(r) { continue; } let l = self.rd(lc + 4 * r, 4) as usize; if l > 0 { out.push((a + r * m * s.size(), a + (r * m + l) * s.size())); } } }
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
        let Loc::Mem(off, ty) = self.resolve(path)? else { return bad("trim a list, table, pool, struct or array") };
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

    // ---- paths: world.items[3].name, world.units[7@2].hp ----
    fn resolve(&self, path: &str) -> Result<Loc, Error> {
        let mut segs: Vec<(String, Idx)> = Vec::new();
        for part in path.split('.') {
            let (name, mut rest) = match part.find('[') { Some(i) => (&part[..i], &part[i..]), None => (part, "") };
            let mut idx: Idx = Vec::new();
            while let Some(r) = rest.strip_prefix('[') {
                let e = r.find(']').ok_or_else(|| Error::Bad("missing `]`".into()))?;
                let (k, g) = match r[..e].split_once('@') { Some((k, g)) => (k, Some(g)), None => (&r[..e], None) };
                let bad_idx = || Error::Bad("bad index".into());
                idx.push((k.parse().map_err(|_| bad_idx())?, match g { Some(g) => Some(g.parse().map_err(|_| bad_idx())?), None => None }));
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
            while let Some((&(k, g), more)) = idx.split_first() {
                idx = more;
                if g.is_some() && !matches!(ty, Ty::Pool(_, _, true)) { return bad("`[k@generation]` needs a `checked` pool"); }
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
                        return self.columnar_field(off, &self.cols(&row, n), k, &segs[si + 1..], idx);
                    }
                    Ty::Pool(row, n, checked) => {
                        let cols = self.prog.pool_cols(&row, n, checked).unwrap().0;
                        let hw = self.rd(off + HW, 4) as usize;
                        if k >= hw || !self.bit(off + cols[0].off, k) { return bad(format!("slot {} is not live", k)); }
                        if let Some(g) = g {
                            let cur = self.rd(off + cols[1].off + 4 * k, 4) as u32;
                            if cur != g { return bad(format!("stale handle: slot {} is now generation {}, not {}", k, cur, g)); }
                        }
                        return self.columnar_field(off, &cols, k, &segs[si + 1..], idx);
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

    /// Locate field `rest` (e.g. `pos.x` or `name`) of row `k` of a table or pool with columns `cols`.
    fn columnar_field(&self, toff: usize, cols: &[TCol], k: usize, rest: &[(String, Idx)], idx: &[(usize, Option<u32>)]) -> Result<Loc, Error> {
        if !idx.is_empty() || rest.is_empty() { return bad("name a field of the row, e.g. items[3].hp"); }
        if rest[..rest.len() - 1].iter().any(|r| !r.1.is_empty()) { return bad("cannot index inside a row"); }
        let path = rest.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(".");
        let last_idx: Vec<usize> = rest[rest.len() - 1].1.iter().map(|x| x.0).collect();
        let ci = cols.iter().position(|c| c.path == path).ok_or_else(|| Error::Bad(format!("row has no field `{}`", path)))?;
        let c = &cols[ci];
        let a = toff + c.off;
        match c.kind {
            CKind::Scalar(s) => if last_idx.is_empty() { Ok(Loc::Mem(a + k * s.size(), Ty::Scalar(s))) } else { bad("not an array") },
            CKind::Arr(s, kk) => match last_idx.as_slice() {
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
                match last_idx.as_slice() {
                    [] => Ok(Loc::List { len: len_off, data, cap: m, elem: s }),
                    [j] => { let l = self.rd(len_off, 4) as usize; if *j >= l { return bad(format!("index {} out of range (live length {})", j, l)); } Ok(Loc::Mem(data + j * s.size(), Ty::Scalar(s))) }
                    _ => bad("too many indexes"),
                }
            }
            _ => bad("internal column"),
        }
    }

    /// Append to a list/table, or take the lowest free slot of a pool. Returns (index, generation for checked pools).
    pub fn push(&mut self, path: &str) -> Result<(usize, Option<u32>), Error> {
        match self.resolve(path)? {
            Loc::Mem(off, Ty::List(e, cap)) => {
                let len = self.rd(off, 4) as usize;
                if len == cap { return Err(Error::Full(format!("{} is full ({} of {})", path, len, cap))); }
                let eoff = off + self.prog.list_header(&e) + len * self.prog.size_align(&e).0;
                let mut r = Vec::new(); self.ranges(&e, eoff, true, &mut r);
                for (a, b) in r { self.zero(a, b - a); } // fresh element: zeroed, empty inside
                self.wr(off, 4, len as u64 + 1);
                Ok((len, None))
            }
            Loc::Mem(off, Ty::Table(row, cap)) => {
                let len = self.rd(off, 4) as usize;
                if len == cap { return Err(Error::Full(format!("{} is full ({} of {})", path, len, cap))); }
                self.zero_row(off, &self.cols(&row, cap), len);
                self.wr(off, 4, len as u64 + 1);
                Ok((len, None))
            }
            Loc::Mem(off, Ty::Pool(row, cap, ch)) => self.pool_alloc(path, off, &row, cap, ch),
            Loc::List { len, data, cap, elem } => {
                let l = self.rd(len, 4) as usize;
                if l == cap { return Err(Error::Full(format!("{} is full ({} of {})", path, l, cap))); }
                self.zero(data + l * elem.size(), elem.size());
                self.wr(len, 4, l as u64 + 1);
                Ok((l, None))
            }
            _ => bad("not a list, table or pool"),
        }
    }

    /// Zero the fixed columns of row `r` (scalars, arrays, string/list lengths). Byte columns stay as they are.
    fn zero_row(&mut self, off: usize, cols: &[TCol], r: usize) {
        for c in cols {
            let a = off + c.off;
            let (at, n) = match c.kind {
                CKind::Scalar(s) => (a + r * s.size(), s.size()),
                CKind::Arr(s, k) => (a + r * k * s.size(), k * s.size()),
                CKind::StrLen | CKind::ListLen => (a + r * 4, 4),
                _ => continue,
            };
            self.zero(at, n);
        }
    }

    fn pool_alloc(&mut self, path: &str, off: usize, row: &Ty, n: usize, ch: bool) -> Result<(usize, Option<u32>), Error> {
        let cols = self.prog.pool_cols(row, n, ch).unwrap().0;
        let (hw, count, hint) = (self.rd(off + HW, 4) as usize, self.rd(off + COUNT, 4) as usize, self.rd(off + HINT, 4) as usize);
        let live_off = off + cols[0].off;
        // lowest free slot at or after the hint, below the high-water mark; otherwise the high-water mark itself
        let mut slot = None;
        let mut w = hint / 64;
        while w * 64 < hw {
            let mut free = !self.rd(live_off + w * 8, 8);
            if (w + 1) * 64 > hw { free &= (1u64 << (hw - w * 64)) - 1; }
            if w == hint / 64 { free &= !0u64 << (hint % 64); }
            if free != 0 { slot = Some(w * 64 + free.trailing_zeros() as usize); break; }
            w += 1;
        }
        let s = match slot { Some(s) => s, None => { if hw == n { return Err(Error::Full(format!("{} is full ({} of {})", path, count, n))); } hw } };
        let waddr = live_off + (s / 64) * 8;
        let word = if (s / 64) * 64 < hw { self.rd(waddr, 8) } else { 0 }; // words wholly above hw may be uncommitted; bits there are zero
        self.wr(waddr, 8, word | (1u64 << (s % 64)));
        self.zero_row(off, &cols, s);
        let gen = if ch { let ga = off + cols[1].off + 4 * s; self.ensure(ga, ga + 4); Some(self.rd(ga, 4) as u32) } else { None };
        let new_hw = hw.max(s + 1);
        let ever = (self.rd(off + EVER, 4) as usize).max(new_hw);
        self.wr(off + HW, 4, new_hw as u64); self.wr(off + COUNT, 4, count as u64 + 1); self.wr(off + HINT, 4, s as u64 + 1); self.wr(off + EVER, 4, ever as u64);
        Ok((s, gen))
    }

    fn pool_free(&mut self, off: usize, row: &Ty, n: usize, ch: bool, k: usize, gen: Option<u32>) -> Result<(), Error> {
        let cols = self.prog.pool_cols(row, n, ch).unwrap().0;
        let hw = self.rd(off + HW, 4) as usize;
        let live_off = off + cols[0].off;
        if k >= hw || !self.bit(live_off, k) { return bad(format!("slot {} is not live", k)); }
        if let Some(g) = gen {
            if !ch { return bad("`[k@generation]` needs a `checked` pool"); }
            let cur = self.rd(off + cols[1].off + 4 * k, 4) as u32;
            if cur != g { return bad(format!("stale handle: slot {} is now generation {}, not {}", k, cur, g)); }
        }
        let waddr = live_off + (k / 64) * 8;
        let word = self.rd(waddr, 8);
        self.wr(waddr, 8, word & !(1u64 << (k % 64)));
        if ch { let ga = off + cols[1].off + 4 * k; let g = self.rd(ga, 4); self.wr(ga, 4, g + 1); }
        let (count, hint) = (self.rd(off + COUNT, 4), self.rd(off + HINT, 4));
        self.wr(off + COUNT, 4, count - 1); self.wr(off + HINT, 4, hint.min(k as u64));
        if k + 1 == hw { // the top slot went: lower the high-water mark to the highest live slot + 1
            let mut h = k;
            while h > 0 {
                let w = (h - 1) / 64;
                let mut word = self.rd(live_off + w * 8, 8);
                let valid = h - w * 64;
                if valid < 64 { word &= (1u64 << valid) - 1; }
                if word == 0 { h = w * 64; } else { h = w * 64 + (64 - word.leading_zeros() as usize); break; }
            }
            self.wr(off + HW, 4, h as u64);
        }
        Ok(())
    }

    fn pool_clear(&mut self, off: usize, row: &Ty, n: usize, ch: bool) {
        let cols = self.prog.pool_cols(row, n, ch).unwrap().0;
        let hw = self.rd(off + HW, 4) as usize;
        let live_off = off + cols[0].off;
        if ch { for k in 0..hw { if self.bit(live_off, k) { let ga = off + cols[1].off + 4 * k; let g = self.rd(ga, 4); self.wr(ga, 4, g + 1); } } }
        self.zero(live_off, hw.div_ceil(64) * 8);
        for o in [HW, COUNT, HINT] { self.wr(off + o, 4, 0); }
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
    pub fn clear(&mut self, path: &str) -> Result<(), Error> {
        if let Loc::Mem(off, Ty::Pool(row, n, ch)) = self.resolve(path)? { self.pool_clear(off, &row, n, ch); return Ok(()); }
        let at = self.len_slot(path)?; self.wr(at, 4, 0); Ok(())
    }

    /// Remove an item: swap-remove from a table (`remove t 5`), or free a pool slot (`remove p 5` or `remove p 5@2`).
    pub fn remove(&mut self, path: &str, spec: &str) -> Result<(), Error> {
        let (k, g) = match spec.split_once('@') { Some((k, g)) => (k, g.parse::<u32>().ok()), None => (spec, None) };
        let k: usize = k.trim().parse().map_err(|_| Error::Bad("expected a row or slot number".into()))?;
        match self.resolve(path)? {
            Loc::Mem(off, Ty::Pool(row, n, ch)) => self.pool_free(off, &row, n, ch, k, g),
            Loc::Mem(_, Ty::Table(..)) => self.swap_remove(path, k),
            _ => bad("not a table or pool"),
        }
    }

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
                CKind::Live | CKind::Gen => {}
            }
        }
        self.wr(off, 4, last as u64); Ok(())
    }

    pub fn set(&mut self, path: &str, val: &str) -> Result<(), Error> {
        match self.resolve(path)? {
            Loc::Str { len, bytes, cap } => {
                let s = val.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(val);
                if s.len() > cap { return Err(Error::TooLong(format!("string of {} bytes does not fit string<{}>", s.len(), cap))); }
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
            Loc::Mem(off, Ty::Pool(..)) => format!("live {}, high-water {}", self.rd(off + COUNT, 4), self.rd(off + HW, 4)),
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

    /// Sum a scalar field over every live row with a native loop. Returns (sum, live rows, ns per slot scanned).
    /// Works on `list<Struct>` (strided), `table<Struct>` (contiguous column) and `pool<Struct>` (column plus a live-bit test per slot).
    pub fn sum(&self, path: &str, field: &str, reps: usize) -> Result<(f64, usize, f64), Error> {
        let Loc::Mem(off, ty) = self.resolve(path)? else { return bad("not a list, table or pool") };
        let find = |cols: &[TCol]| -> Result<(usize, Scalar), Error> {
            let c = cols.iter().find(|c| c.path == field).ok_or_else(|| Error::Bad(format!("no field `{}`", field)))?;
            let CKind::Scalar(s) = c.kind else { return bad("sum needs a scalar field") };
            Ok((c.off, s))
        };
        let (base, stride, len, sc, live, count) = match &ty {
            Ty::Table(row, n) => { let (co, s) = find(&self.cols(row, *n))?; let l = self.rd(off, 4) as usize; (off + co, s.size(), l, s, None, l) }
            Ty::Pool(row, n, ch) => {
                let cols = self.prog.pool_cols(row, *n, *ch).unwrap().0;
                let (co, s) = find(&cols)?;
                (off + co, s.size(), self.rd(off + HW, 4) as usize, s, Some(off + cols[0].off), self.rd(off + COUNT, 4) as usize)
            }
            Ty::List(e, _) => {
                let Ty::Struct(i) = &**e else { return bad("list of structs needed") };
                let f = self.prog.structs[*i].fields.iter().find(|f| f.0 == field).ok_or_else(|| Error::Bad(format!("no field `{}`", field)))?;
                let Ty::Scalar(s) = f.1 else { return bad("sum needs a scalar field") };
                let l = self.rd(off, 4) as usize;
                (off + self.prog.list_header(e) + f.2, self.prog.size_align(e).0, l, s, None, l)
            }
            _ => return bad("not a list, table or pool"),
        };
        if !matches!(sc, Scalar::F32 | Scalar::I32 | Scalar::U32) { return bad("sum supports f32, i32, u32"); }
        let p = unsafe { self.base.add(base) };
        let read = |r: usize| unsafe { let q = p.add(r * stride); match sc { Scalar::F32 => (q as *const f32).read_unaligned() as f64, Scalar::I32 => (q as *const i32).read_unaligned() as f64, _ => (q as *const u32).read_unaligned() as f64 } };
        let t = std::time::Instant::now();
        let mut total = 0.0;
        for _ in 0..reps {
            let mut acc = 0.0f64;
            match live {
                None => for r in 0..len { acc += read(r); },
                Some(lo) => for r in 0..len { if self.bit(lo, r) { acc += read(r); } },
            }
            total = std::hint::black_box(acc);
        }
        let ns = t.elapsed().as_nanos() as f64 / (reps * len.max(1)) as f64;
        Ok((total, count, ns))
    }

    /// Rows of (path, type, offset, reserved, live) for every list, table, pool, string and fixed array reachable through structs.
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
            Ty::Table(..) | Ty::Pool(..) => {
                row(out);
                let (n, cols, _) = self.columnar(ty).unwrap();
                let mut seen: Vec<&str> = Vec::new();
                for c in &cols {
                    if seen.contains(&c.path.as_str()) { continue; }
                    seen.push(&c.path);
                    let mut live = Vec::new();
                    self.columnar_ranges(ty, off, false, Some(&c.path), &mut live);
                    let res: usize = cols.iter().filter(|x| x.path == c.path).map(|x| match x.kind {
                        CKind::Scalar(s) => n * s.size(), CKind::Arr(s, k) => n * k * s.size(), CKind::StrLen | CKind::ListLen => n * 4,
                        CKind::StrBytes(m) => n * m, CKind::ListData(s, m) => n * m * s.size(), CKind::Live => up(n, 64) / 8, CKind::Gen => n * 4 }).sum();
                    let kind = match c.kind { CKind::Scalar(s) => format!("{:?}", s).to_lowercase(), CKind::Arr(s, k) => format!("{:?}[{}]", s, k).to_lowercase(), CKind::StrLen => "string".into(), CKind::Live => "live bits".into(), CKind::Gen => "generations".into(), _ => "list".into() };
                    out.push((format!("{}.{}", path, c.path), format!("  column {}", kind), off + c.off, res, live.iter().map(|(a, b)| b - a).sum()));
                }
            }
            _ => row(out),
        }
    }
}

/// A scalar or string read from or written to state.
#[derive(Clone, Debug, PartialEq)]
pub enum Val { Int(i64), Float(f64), Bool(bool), Str(String) }

/// Typed access used by the endpoint interpreter: columns are cached per global, so a field access is a column lookup
/// plus offset arithmetic, not a path parse.
impl Instance {
    pub fn gindex(&self, name: &str) -> Option<usize> { self.prog.globals.iter().position(|g| g.0 == name) }
    /// (kind, checked): kind 0 list, 1 table, 2 pool.
    pub fn coll_kind(&self, g: usize) -> Option<(u8, bool)> {
        match &self.prog.globals[g].1 { Ty::List(..) => Some((0, false)), Ty::Table(..) => Some((1, false)), Ty::Pool(_, _, c) => Some((2, *c)), _ => None }
    }
    pub fn count(&self, g: usize) -> usize {
        let off = self.prog.globals[g].2;
        match self.coll_kind(g) { Some((2, _)) => self.rd(off + COUNT, 4) as usize, _ => self.rd(off, 4) as usize }
    }
    pub fn capacity(&self, g: usize) -> usize {
        match &self.prog.globals[g].1 { Ty::List(_, n) | Ty::Table(_, n) | Ty::Pool(_, n, _) => *n, _ => 0 }
    }
    /// Live slots of a table or pool, in slot order.
    pub fn slots(&self, g: usize) -> Vec<usize> {
        let off = self.prog.globals[g].2;
        match (self.coll_kind(g), &self.gcols[g]) {
            (Some((2, _)), Some((cols, _))) => { let hw = self.rd(off + HW, 4) as usize; let lo = off + cols[0].off; (0..hw).filter(|&k| self.bit(lo, k)).collect() }
            (Some((1, _)), _) => (0..self.rd(off, 4) as usize).collect(),
            _ => vec![],
        }
    }
    pub fn row_live(&self, g: usize, slot: usize) -> bool {
        let off = self.prog.globals[g].2;
        match (self.coll_kind(g), &self.gcols[g]) {
            (Some((2, _)), Some((cols, _))) => slot < self.rd(off + HW, 4) as usize && self.bit(off + cols[0].off, slot),
            (Some((1, _)), _) => slot < self.rd(off, 4) as usize,
            _ => false,
        }
    }
    pub fn row_gen(&self, g: usize, slot: usize) -> Option<u32> {
        match (self.coll_kind(g), &self.gcols[g]) {
            (Some((2, true)), Some((cols, _))) => Some(self.rd(self.prog.globals[g].2 + cols[1].off + 4 * slot, 4) as u32),
            _ => None,
        }
    }
    fn row_loc(&self, g: usize, slot: usize, field: &str) -> Result<Loc, Error> {
        let Some((cols, _)) = &self.gcols[g] else { return bad("not a table or pool") };
        if !self.row_live(g, slot) { return bad(format!("slot {} is not live", slot)); }
        self.columnar_field(self.prog.globals[g].2, cols, slot, &[(field.to_string(), Vec::new())], &[])
    }
    fn loc_val(&self, loc: Loc) -> Result<Val, Error> {
        match loc {
            Loc::Str { len, bytes, .. } => { let n = self.rd(len, 4) as usize; Ok(Val::Str(String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(self.base.add(bytes), n) }).into_owned())) }
            Loc::Mem(off, Ty::Scalar(s)) => Ok(match s {
                Scalar::F32 => Val::Float(f32::from_bits(self.rd(off, 4) as u32) as f64),
                Scalar::F64 => Val::Float(f64::from_bits(self.rd(off, 8))),
                Scalar::Bool => Val::Bool(self.rd(off, 1) != 0),
                Scalar::I8 | Scalar::I16 | Scalar::I32 | Scalar::I64 => { let sh = 64 - 8 * s.size() as u32; Val::Int(((self.rd(off, s.size()) << sh) as i64) >> sh) }
                _ => Val::Int(self.rd(off, s.size()) as i64),
            }),
            _ => bad("not a scalar or string"),
        }
    }
    fn loc_store(&mut self, loc: Loc, v: &Val) -> Result<(), Error> {
        match (loc, v) {
            (Loc::Str { len, bytes, cap }, Val::Str(s)) => {
                if s.len() > cap { return Err(Error::TooLong(format!("string of {} bytes does not fit string<{}>", s.len(), cap))); }
                self.ensure(bytes, bytes + s.len());
                unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), self.base.add(bytes), s.len()) };
                self.wr(len, 4, s.len() as u64); Ok(())
            }
            (Loc::Mem(off, Ty::Scalar(sc)), v) => {
                let bits = match (sc, v) {
                    (Scalar::F32, Val::Float(f)) => (*f as f32).to_bits() as u64,
                    (Scalar::F32, Val::Int(i)) => (*i as f32).to_bits() as u64,
                    (Scalar::F64, Val::Float(f)) => f.to_bits(),
                    (Scalar::F64, Val::Int(i)) => (*i as f64).to_bits(),
                    (Scalar::Bool, Val::Bool(b)) => *b as u64,
                    (Scalar::F32 | Scalar::F64 | Scalar::Bool, _) => return bad("wrong type for this field"),
                    (_, Val::Int(i)) => *i as u64,
                    (_, Val::Float(f)) => *f as i64 as u64,
                    (_, Val::Bool(b)) => *b as u64,
                    _ => return bad("wrong type for this field"),
                };
                self.wr(off, sc.size(), bits); Ok(())
            }
            _ => bad("wrong type for this field"),
        }
    }
    /// Field read by cached column: no path parse and no allocation for scalars. Strings allocate their result only.
    pub fn row_get(&self, g: usize, slot: usize, field: &str) -> Result<Val, Error> {
        if let Some((cols, _)) = &self.gcols[g] {
            if let Some(ci) = cols.iter().position(|c| c.path == field) {
                if self.row_live(g, slot) {
                    let off = self.prog.globals[g].2;
                    let a = off + cols[ci].off;
                    match cols[ci].kind {
                        CKind::Scalar(s) => return self.loc_val(Loc::Mem(a + slot * s.size(), Ty::Scalar(s))),
                        CKind::StrLen => if let CKind::StrBytes(m) = cols[ci + 1].kind { return self.loc_val(Loc::Str { len: a + 4 * slot, bytes: off + cols[ci + 1].off + slot * m, cap: m }); },
                        _ => {}
                    }
                }
            }
        }
        let l = self.row_loc(g, slot, field)?; self.loc_val(l)
    }
    pub fn row_set(&mut self, g: usize, slot: usize, field: &str, v: &Val) -> Result<(), Error> { let l = self.row_loc(g, slot, field)?; self.loc_store(l, v) }
    pub fn path_get(&self, path: &str) -> Result<Val, Error> { let l = self.resolve(path)?; self.loc_val(l) }
    pub fn path_set(&mut self, path: &str, v: &Val) -> Result<(), Error> { let l = self.resolve(path)?; self.loc_store(l, v) }
    pub fn alloc_row(&mut self, g: usize) -> Result<(usize, Option<u32>), Error> { let n = self.prog.globals[g].0.clone(); self.push(&n) }
    pub fn free_row(&mut self, g: usize, slot: usize) -> Result<(), Error> { let n = self.prog.globals[g].0.clone(); self.remove(&n, &slot.to_string()) }
}

impl Drop for Instance { fn drop(&mut self) { os::release(self.base); } }

#[cfg(test)]
mod tests {
    use super::*;
    const SRC: &str = "struct V { x: f32; y: f32; } struct I { id: i32; p: V; name: string<8>; tags: list<i32, 4>; } global w: table<I, 1000>; global l: list<I, 1000>;";
    fn inst() -> Instance { Instance::start(Program::parse(SRC).unwrap(), 1 << 30).unwrap() }
    const PSRC: &str = "struct U { hp: i32; name: string<8>; } global p: pool<U, 200>; global c: pool<U, 200> checked;";
    fn pinst() -> Instance { Instance::start(Program::parse(PSRC).unwrap(), 1 << 30).unwrap() }

    #[test]
    fn table_and_list_hold_the_same_data() {
        let mut i = inst();
        for p in ["w", "l"] { i.push(p).unwrap(); i.push(p).unwrap(); }
        i.set("l[1].id", "9").unwrap();
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
        assert!(matches!(i.set("w[0].name", "123456789"), Err(Error::TooLong(_))));
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

    #[test]
    fn pool_reuses_the_lowest_free_slot_and_the_live_bit_guards_access() {
        let mut i = pinst();
        for k in 0..5 { assert_eq!(i.push("p").unwrap().0, k); i.set(&format!("p[{}].hp", k), &(k * 10).to_string()).unwrap(); }
        i.remove("p", "1").unwrap(); i.remove("p", "3").unwrap();
        assert!(matches!(i.get("p[1].hp"), Err(Error::Bad(_)))); // live bit says no
        assert_eq!(i.get("p").unwrap(), "live 3, high-water 5");
        assert_eq!(i.push("p").unwrap().0, 1); // lowest free first
        assert_eq!(i.push("p").unwrap().0, 3);
        assert_eq!(i.push("p").unwrap().0, 5);
        assert_eq!(i.get("p[1].hp").unwrap(), "0"); // new item starts zeroed
        assert_eq!(i.get("p[4].hp").unwrap(), "40"); // stable slots: others did not move
    }

    #[test]
    fn unchecked_pool_accepts_the_stale_index_but_checked_rejects_it() {
        let mut i = pinst();
        i.push("p").unwrap(); i.set("p[0].hp", "111").unwrap();
        i.remove("p", "0").unwrap(); i.push("p").unwrap(); i.set("p[0].hp", "222").unwrap();
        assert_eq!(i.get("p[0].hp").unwrap(), "222"); // old index now means the new item (accepted)
        let (s, g) = i.push("c").unwrap(); let g = g.unwrap();
        i.remove("c", &format!("{}@{}", s, g)).unwrap();
        let (s2, g2) = i.push("c").unwrap();
        assert_eq!((s2, g2), (s, Some(g + 1))); // same slot, new generation
        assert!(matches!(i.get(&format!("c[{}@{}].hp", s, g)), Err(Error::Bad(m)) if m.contains("stale")));
        assert!(i.get(&format!("c[{}@{}].hp", s2, g + 1)).is_ok());
        assert!(matches!(i.remove("c", &format!("{}@{}", s, g)), Err(Error::Bad(_))));
    }

    #[test]
    fn pool_full_high_water_and_trim() {
        let mut i = pinst();
        for _ in 0..200 { i.push("p").unwrap(); }
        assert!(matches!(i.push("p"), Err(Error::Full(_))));
        i.remove("p", "50").unwrap();
        assert_eq!(i.push("p").unwrap().0, 50); // a free slot below the mark is found even when the pool looks full by high-water
        for k in (100..200).rev() { i.remove("p", &k.to_string()).unwrap(); }
        assert_eq!(i.get("p").unwrap(), "live 100, high-water 100"); // mark fell with the top slots
        i.set("p[7].name", "keep").unwrap();
        i.clear("c").unwrap();
        i.trim();
        assert_eq!(i.get("p[7].name").unwrap(), "\"keep\"");
        i.clear("p").unwrap();
        assert_eq!(i.get("p").unwrap(), "live 0, high-water 0");
        assert_eq!(i.push("p").unwrap().0, 0);
    }
}
