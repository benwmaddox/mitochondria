//! Regions: slices of one reservation, each tracking reserved / committed / live.
use crate::os::{self, PAGE};

#[derive(Debug, PartialEq, Eq)]
pub struct Full;

fn up(n: usize, to: usize) -> usize { (n + to - 1) / to * to }

/// One reservation holding every region of the program, laid out at startup.
pub struct Space { base: *mut u8, size: usize, next: usize }

impl Space {
    pub fn new(total: usize) -> Space {
        let size = up(total, PAGE);
        Space { base: os::reserve(size), size, next: 0 }
    }
    /// Carve a region of `reserved` bytes (declared maximum). Fails if over budget.
    pub fn region(&mut self, reserved: usize, chunk: usize) -> Result<Region, Full> {
        let reserved = up(reserved, PAGE);
        if self.next + reserved > self.size { return Err(Full); }
        let r = Region { base: unsafe { self.base.add(self.next) }, reserved, committed: 0, chunk: up(chunk, PAGE) };
        self.next += reserved;
        Ok(r)
    }
    pub fn reserved_total(&self) -> usize { self.next }
}
impl Drop for Space { fn drop(&mut self) { os::release(self.base); } }

/// A span of the reservation with an explicit commit watermark.
pub struct Region { pub base: *mut u8, pub reserved: usize, committed: usize, chunk: usize }

impl Region {
    pub fn committed(&self) -> usize { self.committed }
    /// Make at least `n` bytes from the start usable, in whole chunks.
    pub fn ensure(&mut self, n: usize) -> Result<(), Full> {
        if n <= self.committed { return Ok(()); }
        if n > self.reserved { return Err(Full); }
        let want = up(n, self.chunk).min(self.reserved);
        if !os::commit(unsafe { self.base.add(self.committed) }, want - self.committed) { return Err(Full); }
        self.committed = want;
        Ok(())
    }
    /// Give back everything above `live`, keeping `slack` extra bytes committed.
    pub fn trim(&mut self, live: usize, slack: usize) {
        let keep = up(live + slack, self.chunk).min(self.committed);
        if keep < self.committed {
            os::decommit(unsafe { self.base.add(keep) }, self.committed - keep);
            self.committed = keep;
        }
    }
}

/// Scratch: bump allocator, live = top. `reset` is O(1): live goes to zero.
pub struct Scratch { pub r: Region, top: usize }
impl Scratch {
    pub fn new(r: Region) -> Scratch { Scratch { r, top: 0 } }
    pub fn live(&self) -> usize { self.top }
    pub fn alloc(&mut self, n: usize) -> Result<*mut u8, Full> {
        let n = up(n, 16);
        self.r.ensure(self.top + n)?;
        let p = unsafe { self.r.base.add(self.top) };
        self.top += n;
        Ok(p)
    }
    pub fn reset(&mut self) { self.top = 0; }
    pub fn trim(&mut self, slack: usize) { self.r.trim(self.top, slack); }
}

/// Id into a pool: slot index + generation, so a stale id is detected, not reused silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Id { idx: u32, gen: u32 }

/// Dense pool: live items are always the prefix `0..len`; removal swaps the last item in.
pub struct DensePool<const SZ: usize> {
    pub r: Region, len: usize, cap: usize,
    slot_of: Vec<u32>, gen: Vec<u32>, id_of_slot: Vec<u32>, free: Vec<u32>,
}
impl<const SZ: usize> DensePool<SZ> {
    pub fn new(r: Region, cap: usize) -> DensePool<SZ> {
        assert!(cap * SZ <= r.reserved);
        DensePool { r, len: 0, cap, slot_of: vec![0; cap], gen: vec![0; cap], id_of_slot: vec![0; cap], free: (0..cap as u32).rev().collect() }
    }
    pub fn len(&self) -> usize { self.len }
    pub fn live_bytes(&self) -> usize { self.len * SZ }
    pub fn insert(&mut self, item: &[u8; SZ]) -> Result<Id, Full> {
        if self.len == self.cap { return Err(Full); }
        self.r.ensure((self.len + 1) * SZ)?;
        let idx = self.free.pop().unwrap();
        unsafe { std::ptr::copy_nonoverlapping(item.as_ptr(), self.r.base.add(self.len * SZ), SZ) };
        self.slot_of[idx as usize] = self.len as u32;
        self.id_of_slot[self.len] = idx;
        self.len += 1;
        Ok(Id { idx, gen: self.gen[idx as usize] })
    }
    pub fn get(&self, id: Id) -> Option<&[u8; SZ]> {
        if self.gen[id.idx as usize] != id.gen || !self.is_live(id.idx) { return None; }
        let s = self.slot_of[id.idx as usize] as usize;
        Some(unsafe { &*(self.r.base.add(s * SZ) as *const [u8; SZ]) })
    }
    fn is_live(&self, idx: u32) -> bool { let s = self.slot_of[idx as usize] as usize; s < self.len && self.id_of_slot[s] == idx }
    pub fn remove(&mut self, id: Id) -> bool {
        if self.get(id).is_none() { return false; }
        let s = self.slot_of[id.idx as usize] as usize;
        let last = self.len - 1;
        if s != last {
            unsafe { std::ptr::copy_nonoverlapping(self.r.base.add(last * SZ), self.r.base.add(s * SZ), SZ) };
            let moved = self.id_of_slot[last];
            self.id_of_slot[s] = moved;
            self.slot_of[moved as usize] = s as u32;
        }
        self.len -= 1;
        self.gen[id.idx as usize] += 1;
        self.free.push(id.idx);
        true
    }
    pub fn trim(&mut self, slack: usize) { self.r.trim(self.live_bytes(), slack); }
}
