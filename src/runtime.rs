//! A running program: one reservation for the whole layout, pages committed only where bytes are live.
//! Every write goes through `wr`/`put`/`zero`, which can record the old bytes, so a request can be rolled back exactly.
use crate::layout::{up, CKind, Full, Kind, Program, Scalar, Ty, PAGE};
use crate::os;

pub struct Instance {
    pub prog: Program,
    base: *mut u8,
    reserved: usize,
    committed: Vec<bool>, // one flag per page
    pub last_trim: TrimStats,
    journal: Journal,
    shrinks: Vec<u64>, // per top-level collection: bumped whenever something in it is freed, popped or cleared
    committed_pages: usize,
    ranges_buf: Vec<(usize, usize)>, // reused by trims
    live_buf: Vec<bool>,
}

/// Old bytes of every write made while `on`, replayed backwards by `rollback`. Buffers are kept between requests.
#[derive(Default)]
struct Journal { on: bool, entries: Vec<(usize, usize, usize)>, bytes: Vec<u8> }

/// What the last `trim` cost: live ranges scanned, OS release calls made, time in each phase.
#[derive(Clone, Copy, Default, Debug)]
pub struct TrimStats { pub ranges: usize, pub os_calls: usize, pub scan_ns: u128, pub os_ns: u128 }

#[derive(Debug)]
pub enum Error { Full(String), TooLong(String), NotLive(String), Stale(String), Bad(String) }
fn bad<T>(s: impl Into<String>) -> Result<T, Error> { Err(Error::Bad(s.into())) }

/// A scalar, string or reference read from or written to state.
#[derive(Clone, Debug, PartialEq)]
pub enum Val { Int(i64), Float(f64), Bool(bool), Str(String), Ref(Option<(usize, u32)>) }

/// Where a path points: a plain value, a collection instance, an item (with a partial field path), a column field, or an array element of one.
#[derive(Clone, Debug)]
pub enum Loc { Plain(usize, Ty), Coll(usize, usize), Item(usize, usize, String), Field(usize, usize, usize), Elem(usize, usize, usize, usize) }

type Rows = Vec<(String, String, usize, usize, usize)>;

// header fields
const LEN: usize = 0;
const HEAD: usize = 1;
const HW: usize = 0;
const COUNT: usize = 1;
const HINT: usize = 2;
const EVER: usize = 3;

impl Instance {
    /// Reserve the whole layout, refusing to start if it is over `budget` bytes. Commits the plain values and collection headers.
    pub fn start(prog: Program, budget: usize) -> Result<Instance, String> {
        if prog.total > budget { return Err(format!("over budget: layout needs {} bytes, budget is {}", prog.total, budget)); }
        let reserved = up(prog.total.max(1), PAGE);
        let mut inst = Instance { base: os::reserve(reserved), reserved, committed: vec![false; reserved / PAGE], prog, last_trim: TrimStats::default(), journal: Journal { on: false, entries: Vec::with_capacity(1024), bytes: Vec::with_capacity(64 * 1024) }, shrinks: Vec::new(), committed_pages: 0, ranges_buf: Vec::new(), live_buf: Vec::new() }; // sized at startup
        inst.shrinks = vec![0; inst.prog.colls.len()];
        let mut r = vec![(0, inst.prog.plain_end)];
        for c in inst.prog.colls.iter().filter(|c| c.parent.is_none()) { r.push((c.hdr, c.hdr + c.hdr_fields() * 4)); } // nested headers commit with their parent item
        for (a, b) in r { inst.ensure(a, b); }
        Ok(inst)
    }

    pub fn reserved(&self) -> usize { self.reserved }
    /// Start of the reservation; compiled code adds the layout's constant offsets to it.
    pub fn base_ptr(&self) -> *mut u8 { self.base }
    pub fn committed(&self) -> usize { self.committed_pages * PAGE }

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
                self.committed_pages += p - s;
            } else { p += 1; }
        }
    }

    fn rd(&self, off: usize, n: usize) -> u64 { let mut v = 0u64; unsafe { std::ptr::copy_nonoverlapping(self.base.add(off), &mut v as *mut u64 as *mut u8, n) }; v }
    fn bytes(&self, off: usize, n: usize) -> &[u8] { unsafe { std::slice::from_raw_parts(self.base.add(off), n) } }
    fn save(&mut self, a: usize, n: usize) {
        if !self.journal.on || n == 0 { return; }
        let old = unsafe { std::slice::from_raw_parts(self.base.add(a), n) };
        let pos = self.journal.bytes.len();
        self.journal.bytes.extend_from_slice(old);
        self.journal.entries.push((a, n, pos));
    }
    fn wr(&mut self, off: usize, n: usize, v: u64) { self.ensure(off, off + n); self.save(off, n); unsafe { std::ptr::copy_nonoverlapping(&v as *const u64 as *const u8, self.base.add(off), n) }; }
    fn put(&mut self, off: usize, b: &[u8]) { self.ensure(off, off + b.len()); self.save(off, b.len()); unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), self.base.add(off), b.len()) }; }
    fn zero(&mut self, off: usize, n: usize) { if n == 0 { return; } self.ensure(off, off + n); self.save(off, n); unsafe { std::ptr::write_bytes(self.base.add(off), 0, n) }; }

    /// Start recording old bytes; `commit_changes` keeps the writes, `rollback` restores every recorded byte.
    pub fn begin(&mut self) { self.journal.on = true; self.journal.entries.clear(); self.journal.bytes.clear(); }
    pub fn commit_changes(&mut self) { self.journal.on = false; self.journal.entries.clear(); self.journal.bytes.clear(); }
    pub fn rollback(&mut self) {
        let j = std::mem::take(&mut self.journal);
        for &(a, n, pos) in j.entries.iter().rev() { unsafe { std::ptr::copy_nonoverlapping(j.bytes[pos..].as_ptr(), self.base.add(a), n) }; }
        self.journal = j;
        self.commit_changes();
    }
    pub fn journal_bytes(&self) -> usize { self.journal.bytes.len() }

    // ---- collection headers and live bits ----
    fn haddr(&self, cid: usize, f: usize, inst: usize) -> usize { let c = &self.prog.colls[cid]; c.hdr + 4 * (f * c.mult + inst) }
    fn hget(&self, cid: usize, f: usize, inst: usize) -> usize { self.rd(self.haddr(cid, f, inst), 4) as usize }
    fn hset(&mut self, cid: usize, f: usize, inst: usize, v: usize) { let a = self.haddr(cid, f, inst); self.wr(a, 4, v as u64); }
    fn waddr(&self, cid: usize, inst: usize, w: usize) -> usize { let c = &self.prog.colls[cid]; c.live + 8 * (inst * c.words() + w) }
    fn bit(&self, cid: usize, inst: usize, s: usize) -> bool { (self.rd(self.waddr(cid, inst, s / 64), 8) >> (s % 64)) & 1 == 1 }
    fn gaddr(&self, cid: usize, idx: usize) -> usize { self.prog.colls[cid].gen + 4 * idx }

    pub fn cap(&self, cid: usize) -> usize { self.prog.colls[cid].cap }
    pub fn is_list(&self, cid: usize) -> bool { self.prog.colls[cid].is_list() }
    pub fn count(&self, cid: usize, inst: usize) -> usize { if self.is_list(cid) { self.hget(cid, LEN, inst) } else { self.hget(cid, COUNT, inst) } }
    pub fn gen(&self, cid: usize, idx: usize) -> Option<u32> { if self.prog.colls[cid].checked() { Some(self.rd(self.gaddr(cid, idx), 4) as u32) } else { None } }

    pub fn item_live(&self, cid: usize, idx: usize) -> bool {
        let cap = self.cap(cid);
        let (inst, s) = (idx / cap, idx % cap);
        if inst >= self.prog.colls[cid].mult { return false; }
        if self.is_list(cid) { (s + cap - self.hget(cid, HEAD, inst)) % cap < self.hget(cid, LEN, inst) }
        else { s < self.hget(cid, HW, inst) && self.bit(cid, inst, s) }
    }
    /// Live items of one instance as item indexes: lists oldest to newest, pools in slot order.
    pub fn items(&self, cid: usize, inst: usize) -> Vec<usize> { let mut v = Vec::new(); self.items_into(cid, inst, &mut v); v }
    /// Same as `items`, into a buffer the caller reuses (no allocation once it is large enough).
    pub fn items_into(&self, cid: usize, inst: usize, out: &mut Vec<usize>) {
        out.clear();
        let cap = self.cap(cid);
        if self.is_list(cid) {
            let (len, head) = (self.hget(cid, LEN, inst), self.hget(cid, HEAD, inst));
            out.extend((0..len).map(|p| inst * cap + (head + p) % cap));
        } else {
            out.extend((0..self.hget(cid, HW, inst)).filter(|&s| self.bit(cid, inst, s)).map(|s| inst * cap + s));
        }
    }
    /// Item at position `pos` of a list instance (0 = oldest).
    pub fn list_at(&self, cid: usize, inst: usize, pos: usize) -> Option<usize> {
        let cap = self.cap(cid);
        if pos < self.hget(cid, LEN, inst) { Some(inst * cap + (self.hget(cid, HEAD, inst) + pos) % cap) } else { None }
    }

    /// Append to a list (dropping the oldest if declared) or take the lowest free slot of a pool. Returns the new item.
    pub fn push_item(&mut self, cid: usize, inst: usize) -> Result<usize, Error> {
        let (cap, kind) = { let c = &self.prog.colls[cid]; (c.cap, c.kind) };
        let full = |me: &Self, n: usize| Error::Full(format!("{} is full ({} of {})", me.prog.colls[cid].name, n, cap)); // built only on failure
        match kind {
            Kind::List(policy) => {
                let (len, head) = (self.hget(cid, LEN, inst), self.hget(cid, HEAD, inst));
                if len < cap {
                    let idx = inst * cap + (head + len) % cap;
                    self.zero_item(cid, idx); self.hset(cid, LEN, inst, len + 1); Ok(idx)
                } else if policy == Full::DropOldest {
                    let idx = inst * cap + head;
                    self.zero_item(cid, idx); self.hset(cid, HEAD, inst, (head + 1) % cap); Ok(idx)
                } else { Err(full(self, len)) }
            }
            Kind::Pool(checked) => {
                let (hw, count, hint) = (self.hget(cid, HW, inst), self.hget(cid, COUNT, inst), self.hget(cid, HINT, inst));
                let mut slot = None;
                let mut w = hint / 64;
                while w * 64 < hw {
                    let mut free = !self.rd(self.waddr(cid, inst, w), 8);
                    if (w + 1) * 64 > hw { free &= (1u64 << (hw - w * 64)) - 1; }
                    if w == hint / 64 { free &= !0u64 << (hint % 64); }
                    if free != 0 { slot = Some(w * 64 + free.trailing_zeros() as usize); break; }
                    w += 1;
                }
                let s = match slot { Some(s) => s, None => { if hw == cap { return Err(full(self, count)); } hw } };
                let wa = self.waddr(cid, inst, s / 64);
                let word = if (s / 64) * 64 < hw { self.rd(wa, 8) } else { 0 }; // words wholly above hw may be uncommitted; their bits are zero
                self.wr(wa, 8, word | (1u64 << (s % 64)));
                let idx = inst * cap + s;
                self.zero_item(cid, idx);
                if checked { let ga = self.gaddr(cid, idx); self.ensure(ga, ga + 4); }
                let nhw = hw.max(s + 1);
                let ever = self.hget(cid, EVER, inst).max(nhw);
                self.hset(cid, HW, inst, nhw); self.hset(cid, COUNT, inst, count + 1); self.hset(cid, HINT, inst, s + 1); self.hset(cid, EVER, inst, ever);
                Ok(idx)
            }
        }
    }

    /// Zero an item's fixed columns and empty its nested collections. String bytes are left; their length is now 0.
    fn zero_item(&mut self, cid: usize, idx: usize) {
        for k in 0..self.prog.colls[cid].cols.len() {
            let (a, n, bytes) = { let c = &self.prog.colls[cid].cols[k]; (c.off + idx * c.kind.size(), c.kind.size(), matches!(c.kind, CKind::StrBytes(_))) };
            if !bytes { self.zero(a, n); }
        }
        self.reset_children(cid, idx);
    }
    fn reset_children(&mut self, cid: usize, idx: usize) {
        for k in 0..self.prog.colls[cid].children.len() { let child = self.prog.colls[cid].children[k].1; self.reset_instance(child, idx); }
    }
    /// Empty one instance of a collection, including what its items hold.
    fn root(&self, mut cid: usize) -> usize { while let Some(p) = self.prog.colls[cid].parent { cid = p; } cid }
    fn shrank(&mut self, cid: usize) { let r = self.root(cid); self.shrinks[r] += 1; }
    fn reset_instance(&mut self, cid: usize, inst: usize) {
        self.shrank(cid);
        for f in 0..self.prog.colls[cid].hdr_fields() { let h = self.haddr(cid, f, inst); self.ensure(h, h + 4); } // a new parent's header may not be committed yet
        if !self.prog.colls[cid].children.is_empty() { for it in self.items(cid, inst) { self.reset_children(cid, it); } }
        if self.is_list(cid) { self.hset(cid, LEN, inst, 0); self.hset(cid, HEAD, inst, 0); return; }
        let hw = self.hget(cid, HW, inst);
        if self.prog.colls[cid].checked() { for it in self.items(cid, inst) { let ga = self.gaddr(cid, it); let g = self.rd(ga, 4); self.wr(ga, 4, g + 1); } }
        let wa = self.waddr(cid, inst, 0);
        self.zero(wa, hw.div_ceil(64) * 8);
        for f in [HW, COUNT, HINT] { self.hset(cid, f, inst, 0); }
    }

    /// Remove the newest item of a list.
    pub fn pop_item(&mut self, cid: usize, inst: usize) -> Result<(), Error> {
        let len = self.hget(cid, LEN, inst);
        if len == 0 { return bad("list is already empty"); }
        let idx = self.list_at(cid, inst, len - 1).unwrap();
        self.shrank(cid);
        self.reset_children(cid, idx);
        self.hset(cid, LEN, inst, len - 1); Ok(())
    }
    pub fn clear_items(&mut self, cid: usize, inst: usize) { self.reset_instance(cid, inst); }

    /// Free a pool slot. Other slots do not move; the high-water mark falls if this was the top slot.
    pub fn free_item(&mut self, cid: usize, idx: usize) -> Result<(), Error> {
        if self.is_list(cid) { return bad("lists remove only their newest item (pop) or drop the oldest when full"); }
        if !self.item_live(cid, idx) { return Err(Error::NotLive(format!("slot {} is not live", idx % self.cap(cid)))); }
        let cap = self.cap(cid);
        let (inst, s) = (idx / cap, idx % cap);
        self.shrank(cid);
        self.reset_children(cid, idx);
        let wa = self.waddr(cid, inst, s / 64);
        let word = self.rd(wa, 8);
        self.wr(wa, 8, word & !(1u64 << (s % 64)));
        if self.prog.colls[cid].checked() { let ga = self.gaddr(cid, idx); let g = self.rd(ga, 4); self.wr(ga, 4, g + 1); }
        let (count, hint, hw) = (self.hget(cid, COUNT, inst), self.hget(cid, HINT, inst), self.hget(cid, HW, inst));
        self.hset(cid, COUNT, inst, count - 1); self.hset(cid, HINT, inst, hint.min(s));
        if s + 1 == hw {
            let mut h = s;
            while h > 0 {
                let w = (h - 1) / 64;
                let mut word = self.rd(self.waddr(cid, inst, w), 8);
                let valid = h - w * 64;
                if valid < 64 { word &= (1u64 << valid) - 1; }
                if word == 0 { h = w * 64; } else { h = w * 64 + (64 - word.leading_zeros() as usize); break; }
            }
            self.hset(cid, HW, inst, h);
        }
        Ok(())
    }

    // ---- fields ----
    pub fn col(&self, cid: usize, path: &str) -> Option<usize> { self.prog.colls[cid].cols.iter().position(|c| c.path == path && !matches!(c.kind, CKind::StrBytes(_))) }
    pub fn child(&self, cid: usize, path: &str) -> Option<usize> { self.prog.colls[cid].children.iter().find(|k| k.0 == path).map(|k| k.1) }
    pub fn col_kind(&self, cid: usize, ci: usize) -> &CKind { &self.prog.colls[cid].cols[ci].kind }
    fn caddr(&self, cid: usize, idx: usize, ci: usize) -> usize { let c = &self.prog.colls[cid].cols[ci]; c.off + idx * c.kind.size() }

    fn scalar_val(&self, a: usize, s: Scalar) -> Val {
        match s {
            Scalar::F32 => Val::Float(f32::from_bits(self.rd(a, 4) as u32) as f64),
            Scalar::F64 => Val::Float(f64::from_bits(self.rd(a, 8))),
            Scalar::Bool => Val::Bool(self.rd(a, 1) != 0),
            s if s.signed() => { let sh = 64 - 8 * s.size() as u32; Val::Int(((self.rd(a, s.size()) << sh) as i64) >> sh) }
            s => Val::Int(self.rd(a, s.size()) as i64),
        }
    }
    fn store_scalar(&mut self, a: usize, s: Scalar, v: &Val) -> Result<(), Error> {
        let bits = match (s, v) {
            (Scalar::F32, Val::Float(f)) => (*f as f32).to_bits() as u64,
            (Scalar::F32, Val::Int(i)) => (*i as f32).to_bits() as u64,
            (Scalar::F64, Val::Float(f)) => f.to_bits(),
            (Scalar::F64, Val::Int(i)) => (*i as f64).to_bits(),
            (Scalar::Bool, Val::Bool(b)) => *b as u64,
            (Scalar::F32 | Scalar::F64 | Scalar::Bool, _) => return bad("wrong type for this field"),
            (_, Val::Int(i)) => {
                // a number that does not fit the declared width is refused, like a string that does not fit its maximum
                let (lo, hi): (i64, i64) = match s { Scalar::I8 => (-128, 127), Scalar::I16 => (-32768, 32767), Scalar::I32 => (i32::MIN as i64, i32::MAX as i64),
                    Scalar::U8 => (0, 255), Scalar::U16 => (0, 65535), Scalar::U32 => (0, u32::MAX as i64), Scalar::U64 => (0, i64::MAX), _ => (i64::MIN, i64::MAX) };
                if *i < lo || *i > hi { return Err(Error::TooLong(format!("{} does not fit {}", i, format!("{:?}", s).to_lowercase()))); }
                *i as u64
            }
            (_, Val::Bool(b)) => *b as u64,
            _ => return bad("wrong type for this field"),
        };
        self.wr(a, s.size(), bits); Ok(())
    }
    fn store_str(&mut self, len_at: usize, bytes_at: usize, cap: usize, v: &Val) -> Result<(), Error> {
        let Val::Str(s) = v else { return bad("expected a string") };
        self.store_bytes_at(len_at, bytes_at, cap, s.as_bytes())
    }
    fn store_bytes_at(&mut self, len_at: usize, bytes_at: usize, cap: usize, b: &[u8]) -> Result<(), Error> {
        if b.len() > cap { return Err(Error::TooLong(format!("string of {} bytes does not fit string<{}>", b.len(), cap))); }
        self.put(bytes_at, b); self.wr(len_at, 4, b.len() as u64); Ok(())
    }
    /// The bytes of a string field, read in place (no copy).
    pub fn str_bytes(&self, l: &Loc) -> Option<&[u8]> {
        match l {
            Loc::Field(cid, idx, ci) if matches!(self.col_kind(*cid, *ci), CKind::StrLen) => { let a = self.caddr(*cid, *idx, *ci); let b = self.caddr(*cid, *idx, ci + 1); Some(self.bytes(b, self.rd(a, 4) as usize)) }
            Loc::Plain(off, Ty::Str(_)) => Some(self.bytes(off + 4, self.rd(*off, 4) as usize)),
            _ => None,
        }
    }
    /// Store bytes into a string field or plain string.
    pub fn store_str_bytes(&mut self, l: &Loc, b: &[u8]) -> Result<(), Error> {
        match l {
            Loc::Field(cid, idx, ci) => { let CKind::StrBytes(m) = *self.col_kind(*cid, ci + 1) else { return bad("not a string field") }; let (a, bb) = (self.caddr(*cid, *idx, *ci), self.caddr(*cid, *idx, ci + 1)); self.store_bytes_at(a, bb, m, b) }
            Loc::Plain(off, Ty::Str(n)) => { let (off, n) = (*off, *n); self.store_bytes_at(off, off + 4, n, b) }
            _ => bad("not a string"),
        }
    }
    /// Follow a reference without building an error: `None` if the slot is not live or was reused.
    pub fn deref_quiet(&self, target: usize, r: (usize, u32)) -> Option<usize> {
        let (slot, g) = r;
        let c = &self.prog.colls[target];
        if slot >= c.cap || !self.item_live(target, slot) { return None; }
        if c.checked() && self.rd(self.gaddr(target, slot), 4) as u32 != g { return None; }
        Some(slot)
    }
    pub fn is_str(&self, l: &Loc) -> bool { matches!(l, Loc::Plain(_, Ty::Str(_))) || matches!(l, Loc::Field(c, _, ci) if matches!(self.col_kind(*c, *ci), CKind::StrLen)) }
    fn read_ref(&self, a: usize) -> Val { let slot1 = self.rd(a, 4) as usize; Val::Ref(if slot1 == 0 { None } else { Some((slot1 - 1, self.rd(a + 4, 4) as u32)) }) }
    fn store_ref(&mut self, a: usize, v: &Val) -> Result<(), Error> {
        match v { Val::Ref(None) => { self.wr(a, 8, 0); Ok(()) } Val::Ref(Some((s, g))) => { self.wr(a, 4, *s as u64 + 1); self.wr(a + 4, 4, *g as u64); Ok(()) } _ => bad("expected a reference") }
    }

    pub fn field_get(&self, cid: usize, idx: usize, ci: usize) -> Result<Val, Error> {
        let a = self.caddr(cid, idx, ci);
        Ok(match self.col_kind(cid, ci).clone() {
            CKind::Scalar(s) => self.scalar_val(a, s),
            CKind::StrLen => { let CKind::StrBytes(_) = self.col_kind(cid, ci + 1) else { unreachable!() }; let b = self.caddr(cid, idx, ci + 1); Val::Str(String::from_utf8_lossy(self.bytes(b, self.rd(a, 4) as usize)).into_owned()) }
            CKind::Ref(_) => self.read_ref(a),
            _ => return bad("an array field needs an index"),
        })
    }
    pub fn field_set(&mut self, cid: usize, idx: usize, ci: usize, v: &Val) -> Result<(), Error> {
        let a = self.caddr(cid, idx, ci);
        match self.col_kind(cid, ci).clone() {
            CKind::Scalar(s) => self.store_scalar(a, s, v),
            CKind::StrLen => { let CKind::StrBytes(m) = *self.col_kind(cid, ci + 1) else { unreachable!() }; let b = self.caddr(cid, idx, ci + 1); self.store_str(a, b, m, v) }
            CKind::Ref(_) => self.store_ref(a, v),
            _ => bad("an array field needs an index"),
        }
    }
    fn elem_addr(&self, cid: usize, idx: usize, ci: usize, j: usize) -> (usize, Scalar) { let CKind::Arr(s, _) = *self.col_kind(cid, ci) else { unreachable!() }; (self.caddr(cid, idx, ci) + j * s.size(), s) }

    pub fn plain_get(&self, off: usize, t: &Ty) -> Result<Val, Error> {
        Ok(match t {
            Ty::Scalar(s) => self.scalar_val(off, *s),
            Ty::Str(_) => Val::Str(String::from_utf8_lossy(self.bytes(off + 4, self.rd(off, 4) as usize)).into_owned()),
            Ty::Ref(_) => self.read_ref(off),
            _ => return bad("not a scalar, string or reference"),
        })
    }
    pub fn plain_set(&mut self, off: usize, t: &Ty, v: &Val) -> Result<(), Error> {
        match t { Ty::Scalar(s) => self.store_scalar(off, *s, v), Ty::Str(n) => self.store_str(off, off + 4, *n, v), Ty::Ref(_) => self.store_ref(off, v), _ => bad("not a scalar, string or reference") }
    }

    /// Follow a reference into its pool. In a checked pool a reference whose generation has moved on (the slot was freed,
    /// whether or not it was reused since) is `Stale`; otherwise a slot that is not live is `NotLive`.
    pub fn deref(&self, target: usize, r: (usize, u32)) -> Result<usize, Error> {
        let (slot, g) = r;
        let c = &self.prog.colls[target];
        if slot >= c.cap { return Err(Error::NotLive(format!("{} slot {} does not exist", c.name, slot))); }
        let moved_on = c.checked() && slot < self.hget(target, EVER, 0) && self.rd(self.gaddr(target, slot), 4) as u32 != g;
        if moved_on { return Err(Error::Stale(format!("{} slot {} was freed since this reference was made", c.name, slot))); }
        if !self.item_live(target, slot) { return Err(Error::NotLive(format!("{} slot {} is not live", c.name, slot))); }
        Ok(slot)
    }

    // ---- paths: rooms[2].messages[5].body, units[7@2].hp ----
    pub fn resolve(&self, path: &str) -> Result<Loc, Error> {
        let mut loc: Option<Loc> = None;
        for part in path.split('.') {
            let (name, mut rest) = match part.find('[') { Some(i) => (&part[..i], &part[i..]), None => (part, "") };
            loc = Some(match loc {
                None => {
                    let g = self.prog.global(name).ok_or_else(|| Error::Bad(format!("no global `{}`", name)))?;
                    let g = &self.prog.globals[g];
                    match g.coll { Some(c) => Loc::Coll(c, 0), None => Loc::Plain(g.off, g.ty.clone()) }
                }
                Some(l) => self.step_field(l, name)?,
            });
            while let Some(r) = rest.strip_prefix('[') {
                let e = r.find(']').ok_or_else(|| Error::Bad("missing `]`".into()))?;
                let (k, g) = match r[..e].split_once('@') { Some((k, g)) => (k, Some(g)), None => (&r[..e], None) };
                let k: usize = k.parse().map_err(|_| Error::Bad("bad index".into()))?;
                let g: Option<u32> = match g { Some(g) => Some(g.parse().map_err(|_| Error::Bad("bad generation".into()))?), None => None };
                loc = Some(self.step_index(loc.unwrap(), k, g)?);
                rest = &r[e + 1..];
            }
        }
        loc.ok_or_else(|| Error::Bad("empty path".into()))
    }
    pub fn step_field(&self, l: Loc, name: &str) -> Result<Loc, Error> {
        match l {
            Loc::Plain(off, Ty::Struct(s)) => {
                let sd = &self.prog.structs[s];
                let k = sd.fields.iter().position(|f| f.0 == name).ok_or_else(|| Error::Bad(format!("no field `{}`", name)))?;
                let t = sd.fields[k].1.clone();
                if t.is_coll() { Ok(Loc::Coll(self.prog.plain_coll(off, s, name).unwrap(), 0)) } else { Ok(Loc::Plain(off + sd.offs[k], t)) }
            }
            Loc::Item(cid, idx, prefix) => {
                let p = if prefix.is_empty() { name.to_string() } else { format!("{}.{}", prefix, name) };
                if let Some(ci) = self.col(cid, &p) { return Ok(Loc::Field(cid, idx, ci)); }
                if let Some(k) = self.child(cid, &p) { return Ok(Loc::Coll(k, idx)); }
                let deeper = format!("{}.", p);
                let c = &self.prog.colls[cid];
                if c.cols.iter().any(|x| x.path.starts_with(&deeper)) || c.children.iter().any(|x| x.0.starts_with(&deeper)) { return Ok(Loc::Item(cid, idx, p)); }
                bad(format!("no field `{}`", p))
            }
            _ => bad(format!("`.{}` needs a struct or an item", name)),
        }
    }
    /// An item, or for a collection of non-struct values (`list<i32, 8>`), the value itself.
    pub fn item_loc(&self, cid: usize, idx: usize) -> Loc {
        if let Some(ci) = self.col(cid, "") { return Loc::Field(cid, idx, ci); }
        if let Some(k) = self.child(cid, "") { return Loc::Coll(k, idx); }
        Loc::Item(cid, idx, String::new())
    }
    pub fn step_index(&self, l: Loc, k: usize, g: Option<u32>) -> Result<Loc, Error> {
        if g.is_some() && !matches!(&l, Loc::Coll(c, _) if self.prog.colls[*c].checked()) { return bad("`[k@generation]` needs a checked pool"); }
        match l {
            Loc::Coll(cid, inst) => {
                let cap = self.cap(cid);
                if self.is_list(cid) { return self.list_at(cid, inst, k).map(|i| self.item_loc(cid, i)).ok_or_else(|| Error::NotLive(format!("position {} is past the end", k))); }
                let idx = inst * cap + k;
                if k < cap { if let Some(g) = g { let cur = self.gen(cid, idx).unwrap(); if k < self.hget(cid, EVER, inst) && cur != g { return Err(Error::Stale(format!("slot {} is now generation {}, not {}", k, cur, g))); } } }
                if k >= cap || !self.item_live(cid, idx) { return Err(Error::NotLive(format!("slot {} is not live", k))); }
                Ok(self.item_loc(cid, idx))
            }
            Loc::Plain(off, Ty::Arr(e, n)) => { if k >= n { return bad(format!("index {} out of range (fixed size {})", k, n)); } let s = self.prog.plain_size(&e).0; Ok(Loc::Plain(off + k * s, *e)) }
            Loc::Field(cid, idx, ci) => match *self.col_kind(cid, ci) { CKind::Arr(_, n) if k < n => Ok(Loc::Elem(cid, idx, ci, k)), CKind::Arr(_, n) => bad(format!("index {} out of range (fixed size {})", k, n)), _ => bad("not an array") },
            _ => bad("cannot index here"),
        }
    }
    pub fn loc_get(&self, l: &Loc) -> Result<Val, Error> {
        match l {
            Loc::Plain(off, t) => self.plain_get(*off, t),
            Loc::Field(cid, idx, ci) => self.field_get(*cid, *idx, *ci),
            Loc::Elem(cid, idx, ci, j) => { let (a, s) = self.elem_addr(*cid, *idx, *ci, *j); Ok(self.scalar_val(a, s)) }
            _ => bad("not a value"),
        }
    }
    pub fn loc_set(&mut self, l: &Loc, v: &Val) -> Result<(), Error> {
        match l {
            Loc::Plain(off, t) => self.plain_set(*off, &t.clone(), v),
            Loc::Field(cid, idx, ci) => self.field_set(*cid, *idx, *ci, v),
            Loc::Elem(cid, idx, ci, j) => { let (a, s) = self.elem_addr(*cid, *idx, *ci, *j); self.store_scalar(a, s, v) }
            _ => bad("not a value"),
        }
    }

    // ---- script commands by path ----
    fn coll_at(&self, path: &str) -> Result<(usize, usize), Error> { match self.resolve(path)? { Loc::Coll(c, i) => Ok((c, i)), _ => bad(format!("`{}` is not a list or pool", path)) } }
    /// Returns (position for lists / slot for pools, generation for checked pools).
    pub fn push(&mut self, path: &str) -> Result<(usize, Option<u32>), Error> {
        let (cid, inst) = self.coll_at(path)?;
        let idx = self.push_item(cid, inst)?;
        Ok(if self.is_list(cid) { (self.count(cid, inst) - 1, None) } else { (idx % self.cap(cid), self.gen(cid, idx)) })
    }
    pub fn pop(&mut self, path: &str) -> Result<(), Error> { let (c, i) = self.coll_at(path)?; if !self.is_list(c) { return bad("pop is for lists; free pool slots with remove"); } self.pop_item(c, i) }
    pub fn clear(&mut self, path: &str) -> Result<(), Error> { let (c, i) = self.coll_at(path)?; self.clear_items(c, i); Ok(()) }
    pub fn remove(&mut self, path: &str, spec: &str) -> Result<(), Error> {
        let (k, g) = match spec.split_once('@') { Some((k, g)) => (k, g.parse::<u32>().ok()), None => (spec, None) };
        let k: usize = k.trim().parse().map_err(|_| Error::Bad("expected a slot number".into()))?;
        let (cid, inst) = self.coll_at(path)?;
        match self.step_index(Loc::Coll(cid, inst), k, g)? { Loc::Item(c, idx, _) => self.free_item(c, idx), _ => unreachable!() }
    }
    pub fn set(&mut self, path: &str, val: &str) -> Result<(), Error> {
        let l = self.resolve(path)?;
        let v = match self.loc_get(&l)? {
            Val::Str(_) => Val::Str(val.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(val).to_string()),
            Val::Float(_) => Val::Float(val.parse().map_err(|_| Error::Bad("expected a number".into()))?),
            Val::Bool(_) => Val::Bool(matches!(val, "true" | "1")),
            Val::Int(_) => Val::Int(val.parse().map_err(|_| Error::Bad("expected an integer".into()))?),
            Val::Ref(_) => return bad("set references from the endpoint language"),
        };
        self.loc_set(&l, &v)
    }
    pub fn get(&self, path: &str) -> Result<String, Error> {
        match self.resolve(path)? {
            Loc::Coll(c, i) => Ok(if self.is_list(c) { format!("len {}", self.count(c, i)) } else { format!("live {}, high-water {}", self.count(c, i), self.hget(c, HW, i)) }),
            l => Ok(match self.loc_get(&l)? { Val::Str(s) => format!("{:?}", s), Val::Int(i) => i.to_string(), Val::Float(f) => f.to_string(), Val::Bool(b) => b.to_string(), Val::Ref(r) => format!("{:?}", r) }),
        }
    }

    /// Sum a scalar field over every live item with a native loop. Returns (sum, live items, ns per slot scanned).
    pub fn sum(&self, path: &str, field: &str, reps: usize) -> Result<(f64, usize, f64), Error> {
        let (cid, inst) = self.coll_at(path)?;
        let ci = self.col(cid, field).ok_or_else(|| Error::Bad(format!("no field `{}`", field)))?;
        let CKind::Scalar(sc) = *self.col_kind(cid, ci) else { return bad("sum needs a scalar field") };
        if !matches!(sc, Scalar::F32 | Scalar::I32 | Scalar::U32) { return bad("sum supports f32, i32, u32"); }
        let cap = self.cap(cid);
        let base = unsafe { self.base.add(self.prog.colls[cid].cols[ci].off + inst * cap * 4) };
        let read = |r: usize| unsafe { let q = base.add(r * 4); match sc { Scalar::F32 => (q as *const f32).read_unaligned() as f64, Scalar::I32 => (q as *const i32).read_unaligned() as f64, _ => (q as *const u32).read_unaligned() as f64 } };
        let list = self.is_list(cid);
        let (len, head, hw) = if list { (self.hget(cid, LEN, inst), self.hget(cid, HEAD, inst), 0) } else { (0, 0, self.hget(cid, HW, inst)) };
        let t = std::time::Instant::now();
        let mut total = 0.0;
        for _ in 0..reps {
            let mut acc = 0.0f64;
            if list { let end = (head + len).min(cap); for r in head..end { acc += read(r); } for r in 0..(head + len).saturating_sub(cap) { acc += read(r); } }
            else { for r in 0..hw { if self.bit(cid, inst, r) { acc += read(r); } } }
            total = std::hint::black_box(acc);
        }
        let scanned = if list { len } else { hw };
        Ok((total, self.count(cid, inst), t.elapsed().as_nanos() as f64 / (reps * scanned.max(1)) as f64))
    }

    // ---- live ranges and trimming ----
    /// Byte ranges in use by one collection (all instances, not its nested collections).
    fn coll_ranges(&self, cid: usize, out: &mut Vec<(usize, usize)>) {
        let c = &self.prog.colls[cid];
        let cap = c.cap;
        let insts = if c.parent.is_none() { None } else { Some(self.instances(cid)) }; // a top-level collection has one instance
        for &inst in insts.as_deref().unwrap_or(&[0]) {
            for f in 0..c.hdr_fields() { let a = self.haddr(cid, f, inst); out.push((a, a + 4)); }
            let mut runs = [(0usize, 0usize); 2];
            let mut nr = 0;
            let pool = !c.is_list();
            if pool {
                let hw = self.hget(cid, HW, inst);
                if c.checked() { let ever = self.hget(cid, EVER, inst); out.push((c.gen + 4 * inst * cap, c.gen + 4 * (inst * cap + ever))); }
                if hw == 0 { continue; }
                out.push((self.waddr(cid, inst, 0), self.waddr(cid, inst, 0) + hw.div_ceil(64) * 8));
                runs[0] = (0, hw); nr = 1;
            } else {
                let (len, head) = (self.hget(cid, LEN, inst), self.hget(cid, HEAD, inst));
                if len == 0 { continue; }
                runs[0] = (head, (head + len).min(cap)); nr = 1;
                if head + len > cap { runs[1] = (0, head + len - cap); nr = 2; }
            }
            for (ci, col) in c.cols.iter().enumerate() {
                let sz = col.kind.size();
                if let CKind::StrBytes(m) = col.kind {
                    let lc = &c.cols[ci - 1];
                    for &(a, b) in &runs[..nr] { for s in a..b {
                        if pool && !self.bit(cid, inst, s) { continue; }
                        let idx = inst * cap + s;
                        let l = self.rd(lc.off + 4 * idx, 4) as usize;
                        if l > 0 { out.push((col.off + idx * m, col.off + idx * m + l)); }
                    } }
                } else {
                    for &(a, b) in &runs[..nr] { out.push((col.off + (inst * cap + a) * sz, col.off + (inst * cap + b) * sz)); }
                }
            }
        }
    }
    /// Instances of a collection that exist: one for a top-level collection, one per live item of the parent otherwise.
    pub fn instances(&self, cid: usize) -> Vec<usize> {
        match self.prog.colls[cid].parent { None => vec![0], Some(p) => self.instances(p).into_iter().flat_map(|i| self.items(p, i)).collect() }
    }
    fn coll_ranges_deep(&self, cid: usize, out: &mut Vec<(usize, usize)>) { self.coll_ranges(cid, out); for k in &self.prog.colls[cid].children { self.coll_ranges_deep(k.1, out); } }
    fn all_ranges(&self) -> Vec<(usize, usize)> {
        let mut r = vec![(0, self.prog.plain_end)];
        for cid in 0..self.prog.colls.len() { self.coll_ranges(cid, &mut r); }
        r
    }
    pub fn live_total(&self) -> usize { self.all_ranges().iter().map(|(a, b)| b - a).sum() }
    pub fn coll_live(&self, cid: usize) -> usize { let mut r = Vec::new(); self.coll_ranges(cid, &mut r); r.iter().map(|(a, b)| b - a).sum() }
    /// Committed bytes inside one collection's span (with its nested collections).
    pub fn coll_committed(&self, cid: usize) -> usize { let c = &self.prog.colls[cid]; self.committed[c.start / PAGE..up(c.end, PAGE) / PAGE].iter().filter(|x| **x).count() * PAGE }

    /// Return pages that hold no live byte to the OS. Returns bytes released.
    pub fn trim(&mut self) -> usize {
        let t0 = std::time::Instant::now();
        let mut r = std::mem::take(&mut self.ranges_buf);
        r.push((0, self.prog.plain_end));
        for cid in 0..self.prog.colls.len() { self.coll_ranges(cid, &mut r); }
        let pages = self.committed.len();
        self.release_dead(r, 0, pages, t0)
    }
    /// Trim one collection and its nested collections; cost follows their size, not the program's.
    pub fn trim_coll(&mut self, cid: usize) -> usize {
        let t0 = std::time::Instant::now();
        let mut r = std::mem::take(&mut self.ranges_buf);
        self.coll_ranges_deep(cid, &mut r);
        let c = &self.prog.colls[cid];
        let (lo, hi) = (c.start / PAGE, up(c.end, PAGE) / PAGE);
        self.release_dead(r, lo, hi, t0)
    }
    pub fn trim_path(&mut self, path: &str) -> Result<usize, Error> { let (cid, _) = self.coll_at(path)?; Ok(self.trim_coll(cid)) }

    fn release_dead(&mut self, ranges: Vec<(usize, usize)>, lo: usize, hi: usize, t0: std::time::Instant) -> usize {
        let mut live = std::mem::take(&mut self.live_buf);
        live.clear(); live.resize(self.committed.len(), false);
        let mut st = TrimStats { ranges: ranges.len(), ..Default::default() };
        for &(a, b) in &ranges { if b > a { for p in a / PAGE..=(b - 1) / PAGE { live[p] = true; } } }
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
                self.committed_pages -= p - s;
                freed += (p - s) * PAGE;
            } else { p += 1; }
        }
        st.os_ns = t1.elapsed().as_nanos();
        self.last_trim = st;
        self.live_buf = live;
        let mut r = ranges; r.clear(); self.ranges_buf = r;
        freed
    }

    /// Rows of (path, type, start, reserved, live): plain values, then every collection.
    pub fn inspect(&self) -> Rows {
        let mut rows = vec![("<plain values>".to_string(), String::new(), 0, self.prog.plain_end, self.prog.plain_end)];
        for (cid, c) in self.prog.colls.iter().enumerate() {
            let own_end = c.children.first().map(|k| self.prog.colls[k.1].start).unwrap_or(c.end);
            let (kind, opt) = match c.kind { Kind::List(Full::Fail) => ("list", ""), Kind::List(Full::DropOldest) => ("list", " drop_oldest"), Kind::Pool(false) => ("pool", ""), Kind::Pool(true) => ("pool", " checked") };
            let ty = format!("{}<{}, {}>{}{}", kind, self.prog.show(&c.elem), c.cap, opt, if c.mult > 1 { format!(" x{}", c.mult) } else { String::new() });
            rows.push((c.name.clone(), ty, c.start, own_end - c.start, self.coll_live(cid)));
        }
        rows
    }
}

/// A collection whose trim released nothing is tried again only after this many frees, pops or clears in it.
pub const RETRY_AFTER_SHRINKS: u64 = 256;

/// Memory-pressure policy (DESIGN.md concept 9). Marks are committed bytes.
pub struct Pressure {
    pub soft: usize,
    pub hard: usize,
    pub refusing: bool,
    stuck: Vec<u64>, // per collection: its shrink count when a trim released nothing (skip until something shrinks)
    pub trims: usize,
    pub full_trims: usize,
    pub freed: usize,
}
impl Pressure {
    pub fn new(inst: &Instance, soft: usize, hard: usize) -> Pressure { Pressure { soft, hard, refusing: false, stuck: vec![u64::MAX; inst.prog.colls.len()], trims: 0, full_trims: 0, freed: 0 } }
}

impl Instance {
    /// Top-level collection with the most committed memory that is worth trying: never trimmed without result,
    /// or shrunk enough since. The counter check comes first so a steady state costs no page counting.
    fn trim_candidate(&self, p: &Pressure) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for (cid, c) in self.prog.colls.iter().enumerate() {
            if c.parent.is_some() { continue; }
            let worth = p.stuck[cid] == u64::MAX || self.shrinks[cid] >= p.stuck[cid] + RETRY_AFTER_SHRINKS;
            if !worth { continue; }
            let cc = self.coll_committed(cid);
            if cc > 0 && best.map_or(true, |b| cc > b.1) { best = Some((cid, cc)); }
        }
        best.map(|b| b.0)
    }
    fn trim_one(&mut self, p: &mut Pressure, cid: usize) {
        let freed = self.trim_coll(cid);
        p.trims += 1; p.freed += freed;
        p.stuck[cid] = if freed == 0 { self.shrinks[cid] } else { u64::MAX };
    }
    /// Call at an epoch boundary (end of a request or tick). Below the soft mark: nothing. Above it: trim the largest
    /// collection once. Above the hard mark: trim everything, and refuse new work while still above it.
    pub fn relieve(&mut self, p: &mut Pressure) {
        if self.committed() > p.soft { if let Some(cid) = self.trim_candidate(p) { self.trim_one(p, cid); } }
        if self.committed() > p.hard && !p.refusing { p.freed += self.trim(); p.full_trims += 1; }
        p.refusing = self.committed() > p.hard;
    }
    /// Quiet time: release slack from the largest collection even below the soft mark, so a past burst does not stay committed.
    pub fn idle(&mut self, p: &mut Pressure) { if let Some(cid) = self.trim_candidate(p) { self.trim_one(p, cid); } p.refusing = self.committed() > p.hard; }
}

impl Drop for Instance { fn drop(&mut self) { os::release(self.base); } }

#[cfg(test)]
mod tests {
    use super::*;
    fn inst(src: &str) -> Instance { Instance::start(Program::parse(src).unwrap(), 1 << 32).unwrap() }
    const POOLS: &str = "struct U { hp: i32; name: string<8>; } global p: pool<U, 200>; global c: pool<U, 200> checked;";

    #[test]
    fn pool_reuses_the_lowest_free_slot_and_the_live_bit_guards_access() {
        let mut i = inst(POOLS);
        for k in 0..5 { assert_eq!(i.push("p").unwrap().0, k); i.set(&format!("p[{}].hp", k), &(k * 10).to_string()).unwrap(); }
        i.remove("p", "1").unwrap(); i.remove("p", "3").unwrap();
        assert!(matches!(i.get("p[1].hp"), Err(Error::NotLive(_))));
        assert_eq!(i.get("p").unwrap(), "live 3, high-water 5");
        assert_eq!(i.push("p").unwrap().0, 1);
        assert_eq!(i.push("p").unwrap().0, 3);
        assert_eq!(i.push("p").unwrap().0, 5);
        assert_eq!(i.get("p[1].hp").unwrap(), "0");
        assert_eq!(i.get("p[4].hp").unwrap(), "40");
    }

    #[test]
    fn checked_pool_rejects_stale_handles() {
        let mut i = inst(POOLS);
        let (s, g) = i.push("c").unwrap(); let g = g.unwrap();
        i.remove("c", &format!("{}@{}", s, g)).unwrap();
        let (s2, g2) = i.push("c").unwrap();
        assert_eq!((s2, g2), (s, Some(g + 1)));
        assert!(matches!(i.get(&format!("c[{}@{}].hp", s, g)), Err(Error::Stale(_))));
        assert!(i.get(&format!("c[{}@{}].hp", s2, g + 1)).is_ok());
    }

    #[test]
    fn pool_full_high_water_and_trim() {
        let mut i = inst(POOLS);
        for _ in 0..200 { i.push("p").unwrap(); }
        assert!(matches!(i.push("p"), Err(Error::Full(_))));
        i.remove("p", "50").unwrap();
        assert_eq!(i.push("p").unwrap().0, 50);
        for k in (100..200).rev() { i.remove("p", &k.to_string()).unwrap(); }
        assert_eq!(i.get("p").unwrap(), "live 100, high-water 100");
        i.set("p[7].name", "keep").unwrap();
        i.trim();
        assert_eq!(i.get("p[7].name").unwrap(), "\"keep\"");
        assert!(matches!(i.set("p[7].name", "123456789"), Err(Error::TooLong(_))));
    }

    #[test]
    fn list_fails_when_full_unless_it_drops_the_oldest() {
        let mut i = inst("struct M { n: i32; } global a: list<M, 3>; global r: list<M, 3> drop_oldest;");
        for k in 0..3 { i.push("a").unwrap(); i.push("r").unwrap(); i.set(&format!("a[{}].n", k), &k.to_string()).unwrap(); i.set(&format!("r[{}].n", k), &k.to_string()).unwrap(); }
        assert!(matches!(i.push("a"), Err(Error::Full(_))));
        assert_eq!(i.push("r").unwrap().0, 2); // newest position
        i.set("r[2].n", "3").unwrap();
        assert_eq!((i.get("r[0].n").unwrap(), i.get("r[2].n").unwrap()), ("1".into(), "3".into())); // 0 was dropped
        i.pop("r").unwrap();
        assert_eq!(i.get("r").unwrap(), "len 2");
        assert!(matches!(i.get("r[2].n"), Err(Error::NotLive(_))));
    }

    #[test]
    fn nested_collections_are_per_parent_and_reset_with_their_parent() {
        let mut i = inst("struct M { body: string<16>; } struct Room { name: string<8>; msgs: list<M, 4> drop_oldest; } global rooms: pool<Room, 3>;");
        i.push("rooms").unwrap(); i.push("rooms").unwrap();
        for k in 0..6 { i.push("rooms[1].msgs").unwrap(); let p = i.get("rooms[1].msgs").unwrap(); let n: usize = p[4..].parse().unwrap(); i.set(&format!("rooms[1].msgs[{}].body", n - 1), &format!("m{}", k)).unwrap(); }
        i.push("rooms[0].msgs").unwrap();
        assert_eq!(i.get("rooms[0].msgs").unwrap(), "len 1");
        assert_eq!(i.get("rooms[1].msgs").unwrap(), "len 4");
        assert_eq!(i.get("rooms[1].msgs[0].body").unwrap(), "\"m2\""); // m0, m1 dropped
        assert_eq!(i.get("rooms[1].msgs[3].body").unwrap(), "\"m5\"");
        i.remove("rooms", "1").unwrap();
        assert_eq!(i.push("rooms").unwrap().0, 1); // slot reused
        assert_eq!(i.get("rooms[1].msgs").unwrap(), "len 0"); // nested list came back empty
    }

    #[test]
    fn refs_detect_freed_and_reused_targets() {
        let mut i = inst("struct S { name: string<8>; } struct M { who: ref<s>; } global s: pool<S, 4> checked; global m: list<M, 4>;");
        let cs = i.prog.coll_of("s").unwrap();
        let idx = i.push_item(cs, 0).unwrap();
        let r = (idx, i.gen(cs, idx).unwrap());
        assert_eq!(i.deref(cs, r).unwrap(), idx);
        i.free_item(cs, idx).unwrap();
        assert!(matches!(i.deref(cs, r), Err(Error::Stale(_)))); // freed: generation moved on
        let again = i.push_item(cs, 0).unwrap();
        assert_eq!(again, idx);
        assert!(matches!(i.deref(cs, r), Err(Error::Stale(_)))); // reused by someone else
    }

    #[test]
    fn rollback_restores_every_write() {
        let mut i = inst("struct M { n: i32; t: string<8>; } global r: list<M, 2> drop_oldest; global p: pool<M, 4>;");
        i.push("r").unwrap(); i.set("r[0].n", "1").unwrap(); i.set("r[0].t", "one").unwrap();
        i.push("r").unwrap(); i.set("r[1].n", "2").unwrap();
        i.begin();
        i.push("r").unwrap(); i.set("r[1].n", "3").unwrap(); i.set("r[0].t", "changed").unwrap(); // drops "one"
        i.push("p").unwrap();
        i.rollback();
        assert_eq!(i.get("r[0].n").unwrap(), "1");
        assert_eq!(i.get("r[0].t").unwrap(), "\"one\"");
        assert_eq!(i.get("r[1].n").unwrap(), "2");
        assert_eq!(i.get("p").unwrap(), "live 0, high-water 0");
    }

    #[test]
    fn layout_rejects_what_it_cannot_bound() {
        assert!(Program::parse("struct A { b: B; } struct B { a: A; } global g: A;").is_err());
        assert!(Program::parse("struct N { kids: list<N, 2>; } global g: pool<N, 2>;").is_err()); // nests forever
        assert!(Program::parse("struct M { t: list<i32, 2>; } global g: list<M, 2> drop_oldest;").is_err());
        assert!(Program::parse("struct M { who: ref<nope>; } global g: list<M, 2>;").is_err());
        let p = Program::parse("struct M { t: i32; } struct R { m: list<M, 1000>; } global rooms: pool<R, 64>;").unwrap();
        assert_eq!(p.colls[1].mult, 64);
    }

    #[test]
    fn structs_holding_only_collections_stay_distinct() {
        let mut i = inst("struct M { n: i32; } struct W { s: pool<M, 4>; } global w0: W; global w1: W;");
        i.push("w1.s").unwrap();
        assert_eq!(i.get("w0.s").unwrap(), "live 0, high-water 0");
        assert_eq!(i.get("w1.s").unwrap(), "live 1, high-water 1");
    }

    #[test]
    fn nested_headers_commit_with_their_parent_items() {
        let mut i = inst("struct M { n: i32; } struct R { m: list<M, 8>; } global rs: pool<R, 100000>;");
        assert!(i.committed() <= 3 * PAGE, "empty world committed {}", i.committed()); // not 100000 headers
        i.push("rs").unwrap();
        i.push("rs[0].m").unwrap();
        assert_eq!(i.get("rs[0].m").unwrap(), "len 1");
        assert!(i.committed() <= 8 * PAGE);
    }

    #[test]
    fn pressure_returns_burst_memory_and_does_not_rescan_without_new_frees() {
        let mut i = inst("struct J { name: string<64>; } global jobs: pool<J, 100000>;");
        let mut p = Pressure::new(&i, 64 * 1024, 1 << 30);
        let c = i.prog.coll_of("jobs").unwrap();
        let n = i.col(c, "name").unwrap();
        let idxs: Vec<usize> = (0..50_000).map(|k| { let x = i.push_item(c, 0).unwrap(); i.field_set(c, x, n, &Val::Str(format!("job {}", k))).unwrap(); x }).collect();
        i.relieve(&mut p); i.relieve(&mut p); i.relieve(&mut p);
        assert_eq!(p.trims, 1, "nothing was freed, so only the first relieve scans");
        for x in idxs { i.free_item(c, x).unwrap(); }
        i.relieve(&mut p);
        assert!(i.committed() <= 3 * PAGE, "burst memory came back: {}", i.committed());
    }
}
