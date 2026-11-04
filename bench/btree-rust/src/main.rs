// The Rust baseline for bench/ordered: std BTreeMap<u64, u64>, the same workloads and key sequence as ordered.mito.
// Keys: r_i = MINSTD sequence from seed 20261104 (x = x * 48271 % 2147483647), unique below 2^31. A present key is
// 2 * r, an absent key 2 * r + 1. Workload j replays the sequence from the seed every n operations.
use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

const SEED: u64 = 20261104;
fn next(x: u64) -> u64 { x * 48271 % 2147483647 }

// churn W C ROUNDS: a FIFO window of W live keys; each step removes the oldest key and inserts the next new one. Per
// round: ns per step, ns per lookup of the live keys, and the process's private KB. Same key sequence as churn.py.
fn churn(w: u64, c: u64, rounds: u64) {
    let mut m: BTreeMap<u64, u64> = BTreeMap::new();
    let (mut x, mut y) = (SEED, SEED);
    for _ in 0..w { x = next(x); m.insert(x * 2, x); }
    for r in 1..=rounds {
        let t = Instant::now();
        for _ in 0..c { y = next(y); m.remove(&(y * 2)); x = next(x); m.insert(x * 2, x); }
        let step = t.elapsed().as_nanos() as f64 / c as f64;
        let ops = 200_000u64;
        let t = Instant::now();
        let (mut z, mut sum) = (y, 0u64);
        for j in 0..ops { if j % w == 0 { z = y; } z = next(z); if let Some(v) = m.get(&(z * 2)) { sum += *v; } }
        black_box(sum);
        let look = t.elapsed().as_nanos() as f64 / ops as f64;
        println!("{r}	{step:.1}	{look:.1}	{}	{}", private_kb(), m.len());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("churn") {
        let n = |i: usize, d: u64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
        churn(n(2, 100_000), n(3, 100_000), n(4, 40));
        return;
    }
    let n: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let ops: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2_000_000);
    let mut m: BTreeMap<u64, u64> = BTreeMap::new();
    let mut out = String::new();
    let mut run = |name: &str, count: u64, f: &mut dyn FnMut()| {
        let t = Instant::now();
        f();
        let ns = t.elapsed().as_nanos() as f64 / count as f64;
        out.push_str(&format!("{name}\t{ns:.2}\n"));
    };
    run("insert", n, &mut || {
        let mut x = SEED;
        for _ in 0..n { x = next(x); m.insert(x * 2, x); }
    });
    let private_kb = private_kb();
    let mut sum = 0u64;
    run("hit", ops, &mut || {
        let mut x = SEED;
        for j in 0..ops { if j % n == 0 { x = SEED; } x = next(x); if let Some(v) = m.get(&(x * 2)) { sum += *v; } }
    });
    run("miss", ops, &mut || {
        let mut x = SEED;
        for j in 0..ops { if j % n == 0 { x = SEED; } x = next(x); if let Some(v) = m.get(&(x * 2 + 1)) { sum += *v; } }
    });
    run("mixed", ops, &mut || {
        let mut x = SEED;
        for j in 0..ops { if j % n == 0 { x = SEED; } x = next(x); if let Some(v) = m.get(&(x * 2 + (j & 1))) { sum += *v; } }
    });
    run("increment", ops, &mut || {
        let mut x = SEED;
        for j in 0..ops { if j % n == 0 { x = SEED; } x = next(x); if let Some(v) = m.get_mut(&(x * 2)) { *v += 1; } }
    });
    run("range100", ops / 10, &mut || {
        let mut x = SEED;
        for j in 0..ops / 10 { if j % n == 0 { x = SEED; } x = next(x); for (_, v) in m.range(x * 2..).take(100) { sum += *v; } }
    });
    run("delete", n, &mut || {
        let mut x = SEED;
        for _ in 0..n { x = next(x); m.remove(&(x * 2)); }
    });
    black_box(sum);
    print!("{out}");
    println!("private_kb_after_insert\t{private_kb}");
}

#[cfg(windows)]
fn private_kb() -> u64 {
    #[repr(C)] #[allow(non_snake_case)]
    struct Pmc { cb: u32, PageFaultCount: u32, PeakWorkingSetSize: usize, WorkingSetSize: usize, QuotaPeakPagedPoolUsage: usize, QuotaPagedPoolUsage: usize, QuotaPeakNonPagedPoolUsage: usize, QuotaNonPagedPoolUsage: usize, PagefileUsage: usize, PeakPagefileUsage: usize, PrivateUsage: usize }
    #[link(name = "kernel32")] extern "system" { fn GetCurrentProcess() -> isize; }
    #[link(name = "psapi")] extern "system" { fn GetProcessMemoryInfo(p: isize, c: *mut Pmc, cb: u32) -> i32; }
    unsafe {
        let mut c: Pmc = std::mem::zeroed();
        c.cb = std::mem::size_of::<Pmc>() as u32;
        GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb);
        c.PrivateUsage as u64 / 1024
    }
}
#[cfg(not(windows))]
fn private_kb() -> u64 { 0 }
