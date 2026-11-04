use crate::os::{self, os_mem, OsMem};
use crate::region::*;
use std::time::Instant;

const MB: usize = 1 << 20;
fn mb(n: usize) -> f64 { n as f64 / MB as f64 }
fn delta(a: OsMem, b: OsMem) -> (f64, f64) {
    (mb(b.private) - mb(a.private), mb(b.resident) - mb(a.resident))
}
fn row(step: &str, live: usize, committed: usize, base: OsMem) {
    let (p, r) = delta(base, os_mem());
    println!("  {:<28} live {:>7.1}  committed {:>7.1}  | OS commit {:>+8.1}  OS resident {:>+8.1}  (MB)", step, mb(live), mb(committed), p, r);
}

pub fn run() {
    // Static layout: declared maxima, summed at startup. Over budget = refuse to start.
    let mut space = Space::new(2048 * MB);
    println!("E0 reserve: reserving 2 GB of address space");
    let t = os_mem();
    let scratch_r = space.region(512 * MB, 1 * MB).unwrap();
    let pool_r = space.region(512 * MB, 1 * MB).unwrap();
    let waiters_r = space.region(64 * MB, 64 * 1024).unwrap();
    let (p, r) = delta(t, os_mem());
    println!("  declared total {:.0} MB; OS commit {:+.1} MB, resident {:+.1} MB (reserve alone costs ~nothing)", mb(space.reserved_total()), p, r);
    println!("  over-budget region -> {:?}", space.region(4096 * MB, MB).err());

    println!("\nE1 scratch epoch: fill 300 MB, reset, trim");
    let mut s = Scratch::new(scratch_r);
    let base = os_mem();
    for _ in 0..300 { let p = s.alloc(MB).unwrap(); unsafe { std::ptr::write_bytes(p, 0xAB, MB) }; }
    row("after 300 x 1 MB alloc", s.live(), s.r.committed(), base);
    let t0 = Instant::now(); s.reset(); let dt = t0.elapsed();
    row("after reset (O(1))", s.live(), s.r.committed(), base);
    println!("  reset took {:?}", dt);
    s.trim(8 * MB);
    row("after trim (keep 8 MB slack)", s.live(), s.r.committed(), base);
    println!("  alloc past reservation -> {:?}", s.alloc(600 * MB).err());

    println!("\nE2 dense pool: 1M x 64 B items, remove 90%, trim");
    let mut p = DensePool::<64>::new(pool_r, 1_000_000);
    let base = os_mem();
    let mut ids = Vec::new();
    for i in 0..1_000_000u32 { let mut it = [0u8; 64]; it[..4].copy_from_slice(&i.to_le_bytes()); ids.push(p.insert(&it).unwrap()); }
    row("after 1M inserts", p.live_bytes(), p.r.committed(), base);
    for (i, id) in ids.iter().enumerate() { if i % 10 != 0 { assert!(p.remove(*id)); } }
    row("after removing 90%", p.live_bytes(), p.r.committed(), base);
    p.trim(MB);
    row("after trim (keep 1 MB slack)", p.live_bytes(), p.r.committed(), base);
    let ok = ids.iter().enumerate().all(|(i, id)| (i % 10 == 0) == p.get(*id).is_some());
    let intact = ids.iter().step_by(10).enumerate().all(|(k, id)| { let it = p.get(*id).unwrap(); u32::from_le_bytes(it[..4].try_into().unwrap()) == (k * 10) as u32 });
    println!("  stale ids rejected, live ids intact: {} / {}", ok, intact);

    println!("\nE3 10,000 waiting jobs: suspend records vs one page-backed stack each");
    const REC: usize = 96; // size of the biggest suspend record the compiler would compute
    let mut w = DensePool::<REC>::new(waiters_r, 10_000);
    let base = os_mem();
    for i in 0..10_000u32 { let mut it = [0u8; REC]; it[..4].copy_from_slice(&i.to_le_bytes()); w.insert(&it).unwrap(); }
    let (pc, rs) = delta(base, os_mem());
    println!("  records: live {:.2} MB ({} B each); OS commit {:+.2} MB, resident {:+.2} MB", mb(w.live_bytes()), REC, pc, rs);
    let base = os_mem();
    let stacks = (0..10_000).map(|_| { let q = os::reserve(64 * 1024); os::commit(q, 4096); unsafe { *q = 1 }; q }).collect::<Vec<_>>();
    let (pc, rs) = delta(base, os_mem());
    println!("  stack each (1 page min):     OS commit {:+.2} MB, resident {:+.2} MB", pc, rs);
    drop(stacks);
}
