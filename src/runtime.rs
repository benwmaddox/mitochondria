//! A running program: one reservation for the whole layout, pages committed only where bytes are live.
use crate::layout::{up, Program, Scalar, Ty};
use crate::os::{self, PAGE};

pub struct Instance {
    pub prog: Program,
    base: *mut u8,
    reserved: usize,
    committed: Vec<bool>, // one flag per page
}

#[derive(Debug)]
pub enum Error { Full(String), Bad(String) }
fn bad<T>(s: impl Into<String>) -> Result<T, Error> { Err(Error::Bad(s.into())) }

impl Instance {
    /// Reserve the whole layout, refusing to start if it is over `budget` bytes.
    pub fn start(prog: Program, budget: usize) -> Result<Instance, String> {
        if prog.total > budget { return Err(format!("over budget: layout needs {} bytes, budget is {}", prog.total, budget)); }
        let reserved = up(prog.total.max(1), PAGE);
        let mut inst = Instance { base: os::reserve(reserved), reserved, committed: vec![false; reserved / PAGE], prog };
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
            Ty::Arr(e, n) => { let (es, _) = p.size_align(e); for i in 0..*n { self.ranges(e, off + i * es, fresh, out); } }
            Ty::Struct(i) => for f in &p.structs[*i].fields { self.ranges(&f.1, off + f.2, fresh, out); },
            _ => {}
        }
    }

    pub fn live_of(&self, ty: &Ty, off: usize) -> usize { let mut r = Vec::new(); self.ranges(ty, off, false, &mut r); r.iter().map(|(a, b)| b - a).sum() }
    pub fn live_total(&self) -> usize { self.prog.globals.iter().map(|(_, t, o)| self.live_of(t, *o)).sum() }

    /// Return pages that hold no live byte to the OS. Returns bytes released.
    pub fn trim(&mut self) -> usize {
        let mut r = Vec::new();
        for (_, ty, off) in &self.prog.globals { self.ranges(ty, *off, false, &mut r); }
        let mut live = vec![false; self.committed.len()];
        for (a, b) in r { for p in a / PAGE..=(b.max(a + 1) - 1) / PAGE { live[p] = true; } }
        let (mut freed, mut p) = (0, 0);
        while p < live.len() {
            if self.committed[p] && !live[p] {
                let s = p;
                while p < live.len() && self.committed[p] && !live[p] { p += 1; }
                os::decommit(unsafe { self.base.add(s * PAGE) }, (p - s) * PAGE);
                for q in s..p { self.committed[q] = false; }
                freed += (p - s) * PAGE;
            } else { p += 1; }
        }
        freed
    }

    // ---- paths: world.items[3].name ----
    fn resolve(&self, path: &str) -> Result<(usize, Ty), Error> {
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
        let (gname, gidx) = &segs[0];
        let (_, gty, goff) = self.prog.globals.iter().find(|g| &g.0 == gname).ok_or_else(|| Error::Bad(format!("no global `{}`", gname)))?;
        let (mut off, mut ty) = (*goff, gty.clone());
        let mut first = true;
        for (name, idx) in &segs {
            if !first {
                let Ty::Struct(i) = ty else { return bad(format!("`{}` is not a struct", name)) };
                let f = self.prog.structs[i].fields.iter().find(|f| &f.0 == name).ok_or_else(|| Error::Bad(format!("no field `{}`", name)))?;
                off += f.2; ty = f.1.clone();
            }
            first = false;
            for &k in idx {
                match ty.clone() {
                    Ty::Arr(e, n) => { if k >= n { return bad(format!("index {} out of range (fixed size {})", k, n)); } off += k * self.prog.size_align(&e).0; ty = *e; }
                    Ty::List(e, _) => {
                        let len = self.rd(off, 4) as usize;
                        if k >= len { return bad(format!("index {} out of range (live length {})", k, len)); }
                        off += self.prog.list_header(&e) + k * self.prog.size_align(&e).0; ty = *e;
                    }
                    _ => return bad("cannot index here"),
                }
            }
        }
        let _ = gidx;
        Ok((off, ty))
    }

    pub fn push(&mut self, path: &str) -> Result<usize, Error> {
        let (off, ty) = self.resolve(path)?;
        let Ty::List(e, cap) = ty else { return bad("not a list") };
        let len = self.rd(off, 4) as usize;
        if len == cap { return Err(Error::Full(format!("{} is full ({} of {})", path, len, cap))); }
        let eoff = off + self.prog.list_header(&e) + len * self.prog.size_align(&e).0;
        let mut r = Vec::new(); self.ranges(&e, eoff, true, &mut r);
        for (a, b) in r { self.ensure(a, b); unsafe { std::ptr::write_bytes(self.base.add(a), 0, b - a) }; } // fresh element: zeroed, empty inside
        self.wr(off, 4, len as u64 + 1);
        Ok(len)
    }
    pub fn pop(&mut self, path: &str) -> Result<(), Error> {
        let (off, ty) = self.resolve(path)?;
        if !matches!(ty, Ty::List(..)) { return bad("not a list"); }
        let len = self.rd(off, 4);
        if len == 0 { return bad("list is empty"); }
        self.wr(off, 4, len - 1); Ok(())
    }
    pub fn clear(&mut self, path: &str) -> Result<(), Error> {
        let (off, ty) = self.resolve(path)?;
        if !matches!(ty, Ty::List(..) | Ty::Str(_)) { return bad("not a list or string"); }
        self.wr(off, 4, 0); Ok(())
    }
    pub fn set(&mut self, path: &str, val: &str) -> Result<(), Error> {
        let (off, ty) = self.resolve(path)?;
        match ty {
            Ty::Str(cap) => {
                let s = val.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(val);
                if s.len() > cap { return Err(Error::Full(format!("string of {} bytes does not fit string<{}>", s.len(), cap))); }
                self.ensure(off + 4, off + 4 + s.len());
                unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), self.base.add(off + 4), s.len()) };
                self.wr(off, 4, s.len() as u64); Ok(())
            }
            Ty::Scalar(sc) => {
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
        let (off, ty) = self.resolve(path)?;
        Ok(match ty {
            Ty::Str(_) => { let n = self.rd(off, 4) as usize; format!("{:?}", String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(self.base.add(off + 4), n) })) }
            Ty::List(..) => format!("len {}", self.rd(off, 4)),
            Ty::Scalar(Scalar::F32) => f32::from_bits(self.rd(off, 4) as u32).to_string(),
            Ty::Scalar(Scalar::F64) => f64::from_bits(self.rd(off, 8)).to_string(),
            Ty::Scalar(s) => { let v = self.rd(off, s.size()); let sh = 64 - 8 * s.size() as u32; if matches!(s, Scalar::I8 | Scalar::I16 | Scalar::I32 | Scalar::I64) { (((v << sh) as i64) >> sh).to_string() } else { v.to_string() } }
            _ => return bad("can only get scalars, strings and list lengths"),
        })
    }

    /// Rows of (path, type, offset, reserved, live) for every list, string and fixed array reachable through structs.
    pub fn inspect(&self) -> Vec<(String, String, usize, usize, usize)> {
        let mut rows = Vec::new();
        for (n, ty, off) in &self.prog.globals { self.rows(n, ty, *off, &mut rows); }
        rows
    }
    fn rows(&self, path: &str, ty: &Ty, off: usize, out: &mut Vec<(String, String, usize, usize, usize)>) {
        let p = &self.prog;
        let row = |out: &mut Vec<_>| out.push((path.to_string(), p.show(ty), off, p.size_align(ty).0, self.live_of(ty, off)));
        match ty {
            Ty::Struct(i) => { row(out); for f in &p.structs[*i].fields { if !matches!(f.1, Ty::Scalar(_)) { self.rows(&format!("{}.{}", path, f.0), &f.1, off + f.2, out); } } }
            Ty::Scalar(_) => {}
            _ => row(out),
        }
    }
}

impl Drop for Instance { fn drop(&mut self) { os::release(self.base); } }
