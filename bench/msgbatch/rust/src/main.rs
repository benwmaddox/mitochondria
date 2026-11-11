// Message batch filter, Rust version (safe code, bounds-checked slices). Same format, data and checksum as main.mito and go/main.go; see ../README.md.
use std::time::Instant;

// `--features allocs`: count heap allocations (the cold-start test prints them); the timed benchmark is built without it
#[cfg(feature = "allocs")]
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub static ALLOCS: AtomicU64 = AtomicU64::new(0);
    pub struct Counting;
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 { ALLOCS.fetch_add(1, Relaxed); System.alloc(l) }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) { System.dealloc(p, l) }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 { ALLOCS.fetch_add(1, Relaxed); System.realloc(p, l, n) }
    }
}
#[cfg(feature = "allocs")]
#[global_allocator]
static A: counting::Counting = counting::Counting;

const MAX_PAYLOAD: usize = 120;
const MAX_OUT: usize = 32768;

struct Rng(u64);
impl Rng {
    fn next(&mut self, max: u64) -> u64 {
        self.0 = (self.0 * 3877 + 29573) % 139968;
        self.0 * max / 139968
    }
}

fn build() -> Vec<u8> {
    let mut rng = Rng(42);
    let mut buf = Vec::with_capacity(32768);
    for i in 0..256u64 {
        let r = (i * 37 + 11) % 256;
        let len = 16 + rng.next(105) as usize;
        let (mut cat, mut magic, mut ver, mut flen) = (0u8, 0x4D53u16, 1u8, len);
        if r < 24 {
            cat = rng.next(8) as u8;
            match r % 3 { 0 => magic = 0x4D54, 1 => ver = 2, _ => flen = len + 1 }
        } else if r < 88 {
            cat = 3 + (r & 1) as u8 * 2;
        } else {
            cat = rng.next(6) as u8;
            if cat >= 3 { cat += 1; }
            if cat >= 5 { cat += 1; }
        }
        buf.extend_from_slice(&magic.to_le_bytes());
        buf.push(ver);
        buf.push(cat);
        buf.extend_from_slice(&((len + 8) as u16).to_le_bytes());
        buf.extend_from_slice(&(flen as u16).to_le_bytes());
        for _ in 0..len { buf.push(rng.next(256) as u8); }
    }
    buf
}

// one batch: filter and copy into `out`, then read the copy back; `out` is reset for the next batch
fn process(input: &[u8], out: &mut Vec<u8>) -> (u32, u32, u32, u32) {
    let (mut rejected, mut skipped, mut copied) = (0u32, 0u32, 0u32);
    let mut off = 0;
    while off < input.len() {
        let magic = u16::from_le_bytes([input[off], input[off + 1]]);
        let ver = input[off + 2];
        let cat = input[off + 3];
        let frame = u16::from_le_bytes([input[off + 4], input[off + 5]]) as usize;
        let len = u16::from_le_bytes([input[off + 6], input[off + 7]]) as usize;
        if magic != 0x4D53 || ver != 1 || len + 8 != frame || len > MAX_PAYLOAD {
            rejected += 1;
        } else if cat == 3 || cat == 5 {
            skipped += 1;
        } else {
            out.extend_from_slice(&input[off..off + frame]);
            copied += 1;
        }
        off += frame;
    }
    let mut h: u32 = 0;
    for (i, &b) in out.iter().enumerate() { h = h.wrapping_add((b ^ (i & 255) as u8) as u32); }
    h = h.wrapping_add((out.len() as u32) << 16);
    out.clear();
    (h, rejected, skipped, copied)
}

fn main() {
    let iters: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(500000);
    if std::env::args().nth(2).as_deref() == Some("cold") { return cold(iters); }
    if std::env::args().nth(2).as_deref() == Some("allocs") { return allocs(iters); }
    let warm = std::cmp::max(iters / 5, 1);
    let input = build();
    let mut out: Vec<u8> = Vec::with_capacity(MAX_OUT);
    let mut result: u32 = 0;
    let mut counts = (0, 0, 0);
    for _ in 0..warm { let r = process(&input, &mut out); result = result.wrapping_mul(31).wrapping_add(r.0); counts = (r.1, r.2, r.3); }
    let t = Instant::now();
    for _ in 0..iters { let r = process(&input, &mut out); result = result.wrapping_mul(31).wrapping_add(r.0); counts = (r.1, r.2, r.3); }
    println!("{:.6} {} {} {} {}", t.elapsed().as_secs_f64(), result, counts.0, counts.1, counts.2);
}

// cold start: no warmup; the time of each of the first batches, then the rest, and the allocations made (needs --features allocs)
fn cold(iters: u64) {
    let input = build();
    let mut out: Vec<u8> = Vec::with_capacity(MAX_OUT);
    let mut first: Vec<u128> = Vec::with_capacity(5);
    #[cfg(feature = "allocs")]
    let a0 = counting::ALLOCS.load(std::sync::atomic::Ordering::Relaxed);
    let mut result: u32 = 0;
    for _ in 0..5.min(iters) {
        let t1 = Instant::now();
        let r = process(&input, &mut out);
        result = result.wrapping_mul(31).wrapping_add(r.0);
        first.push(t1.elapsed().as_nanos());
    }
    let t = Instant::now();
    for _ in 5..iters {
        let r = process(&input, &mut out);
        result = result.wrapping_mul(31).wrapping_add(r.0);
    }
    let total = t.elapsed().as_secs_f64();
    let steady = iters - 5;
    #[cfg(feature = "allocs")]
    let allocs = counting::ALLOCS.load(std::sync::atomic::Ordering::Relaxed) - a0;
    #[cfg(not(feature = "allocs"))]
    let allocs = -1i64;
    println!("first batches ns {:?} total {:.6}s per batch (after the first 5) {:.0} ns allocations during batches {} checksum {}", first, total, total * 1e9 / steady as f64, allocs, result);
}

// heap allocations only: during the first 5 batches and during the rest (needs --features allocs)
fn allocs(iters: u64) {
    let input = build();
    let mut out: Vec<u8> = Vec::with_capacity(MAX_OUT);
    #[cfg(feature = "allocs")]
    let count = || counting::ALLOCS.load(std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(feature = "allocs"))]
    let count = || 0u64;
    let a0 = count();
    let mut h = 0;
    for _ in 0..5 { h = process(&input, &mut out).0; }
    let a1 = count();
    for _ in 5..iters { h = process(&input, &mut out).0; }
    println!("allocations first5 {} steady {} ({})", a1 - a0, count() - a1, h);
}
