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

/// How much does `trim` cost, and what does touching released pages cost afterwards?
pub fn trim_cost() {
    use crate::layout::Program;
    use crate::runtime::Instance;
    const SRC: &str = "struct I { id: i32; name: string<16>; } global w: pool<I, 1000000>;";
    println!("pool<I,1M>, name column = 16 MB = 4096 pages (256 rows each). Every row gets a name, then all but every Nth are cleared.");
    println!("  {:>9} {:>8} | {:>9} {:>9} | {:>8} {:>9} {:>8} | {:>9} {:>14}", "kept rows", "pages", "scan ms", "ranges", "OS calls", "OS ms", "us/call", "freed MB", "recommit us/pg");
    for stride in [1usize, 255, 256, 1000, 100_000, 1_000_000] {
        let mut i = Instance::start(Program::parse(SRC).unwrap(), 1 << 30).unwrap();
        for _ in 0..1_000_000 { i.push("w").unwrap(); }
        for r in 0..1_000_000 { i.set(&format!("w[{}].name", r), "abcdefgh").unwrap(); }
        let full = i.committed();
        for r in 0..1_000_000 { if r % stride != 0 { i.set(&format!("w[{}].name", r), "").unwrap(); } }
        let freed = i.trim();
        let t = i.last_trim;
        let after = i.committed();
        // write a name into every 256th row: one write per page, each page that trim released must be committed again
        let t0 = Instant::now();
        for r in (0..1_000_000).step_by(256) { i.set(&format!("w[{}].name", r), "z").unwrap(); }
        let back = (i.committed() - after) / 4096;
        let rt = if back > 0 { t0.elapsed().as_secs_f64() * 1e6 / back as f64 } else { 0.0 };
        println!("  {:>9} {:>8} | {:>9.2} {:>9} | {:>8} {:>9.3} {:>8.2} | {:>9.2} {:>14.2}", 1_000_000 / stride + (1_000_000 % stride != 0) as usize, (full - after) / 4096, t.scan_ns as f64 / 1e6, t.ranges, t.os_calls, t.os_ns as f64 / 1e6,
            if t.os_calls > 0 { t.os_ns as f64 / 1e3 / t.os_calls as f64 } else { 0.0 }, freed as f64 / MB as f64, rt);
        i.trim();
        let t2 = i.last_trim;
        println!("  {:>9}   second trim: scan {:.2} ms, OS calls {}", "", t2.scan_ns as f64 / 1e6, t2.os_calls);
    }
}

/// Whole-program trim vs trimming only the part that changed.
pub fn scoped_cost() {
    use crate::layout::Program;
    use crate::runtime::Instance;
    const SRC: &str = "struct I { id: i32; name: string<16>; } global big: pool<I, 1000000>; global worker: pool<I, 50000>;";
    let mut i = Instance::start(Program::parse(SRC).unwrap(), 1 << 30).unwrap();
    for r in 0..1_000_000 { i.push("big").unwrap(); i.set(&format!("big[{}].name", r), "keepme").unwrap(); } // 1M live strings in a quiet region
    println!("\nscoped vs whole trim: `big` has 1M live strings and never changes; `worker` bursts to 50,000 rows with names, then clears");
    println!("  {:<22} {:>9} {:>9} {:>9} {:>9}", "", "scan ms", "ranges", "OS calls", "freed MB");
    for round in 0..2 {
        for r in 0..50_000 { i.push("worker").unwrap(); i.set(&format!("worker[{}].name", r), "burst").unwrap(); }
        i.clear("worker").unwrap();
        let before = i.committed();
        let f = if round == 0 { i.trim_path("worker").unwrap() } else { i.trim() };
        let t = i.last_trim;
        println!("  {:<22} {:>9.2} {:>9} {:>9} {:>9.2}  (committed {:.1} -> {:.1} MB)", if round == 0 { "trim worker" } else { "trim everything" }, t.scan_ns as f64 / 1e6, t.ranges, t.os_calls, f as f64 / MB as f64, before as f64 / MB as f64, i.committed() as f64 / MB as f64);
    }
    println!("  big untouched by scoped trim: {}", i.get("big[999999].name").unwrap());
}

/// Data access patterns: pointers vs row index vs id-with-generation, native Rust loops over the layouts.
pub fn access_cost() {
    use std::hint::black_box;
    #[derive(Clone, Copy)]
    struct Item { id: i32, hp: f32, x: f32, y: f32, _pad: [u8; 48] }
    const N: usize = 1 << 20; // 1M items, 64 MB as records
    const OPS: usize = 4 << 20;
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut rnd = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
    let perm = |rnd: &mut dyn FnMut() -> u64| { let mut v: Vec<u32> = (0..N as u32).collect(); for i in (1..N).rev() { v.swap(i, (rnd() % (i as u64 + 1)) as usize); } v };
    let time = |f: &mut dyn FnMut() -> f64| { f(); let t = Instant::now(); let r = f(); black_box(r); t.elapsed().as_nanos() as f64 / OPS as f64 };

    let aos: Vec<Item> = (0..N).map(|i| Item { id: i as i32, hp: 1.0, x: 2.0, y: 3.0, _pad: [0; 48] }).collect();
    let order = perm(&mut rnd);
    let boxed_seq: Vec<Box<Item>> = (0..N).map(|i| Box::new(aos[i])).collect();      // heap in allocation order
    let mut boxed_aged: Vec<Option<Box<Item>>> = (0..N).map(|_| None).collect();          // heap allocated in shuffled order
    for &k in &order { boxed_aged[k as usize] = Some(Box::new(aos[k as usize])); }
    let boxed_aged: Vec<Box<Item>> = boxed_aged.into_iter().map(|b| b.unwrap()).collect();
    let (_id_c, hp_c, x_c, y_c): (Vec<i32>, Vec<f32>, Vec<f32>, Vec<f32>) = (aos.iter().map(|a| a.id).collect(), aos.iter().map(|a| a.hp).collect(), aos.iter().map(|a| a.x).collect(), aos.iter().map(|a| a.y).collect());
    let slot_of = perm(&mut rnd);                  // id -> row (rows were reshuffled by removals)
    let gen: Vec<u32> = vec![1; N];
    let q: Vec<u32> = (0..OPS).map(|_| (rnd() % N as u64) as u32).collect();

    println!("\nmemory access, 1M items (64 B as records), {} random operations; ns per operation (lower is better)", OPS);
    println!("  random read of 3 fields of one object (hp + x + y):");
    let r = time(&mut || q.iter().map(|&k| { let a = &aos[k as usize]; (a.hp + a.x + a.y) as f64 }).sum());
    println!("    {:<46} {:>7.1}", "records, row index (list<T>)", r);
    let r = time(&mut || q.iter().map(|&k| { let a = &boxed_seq[k as usize]; (a.hp + a.x + a.y) as f64 }).sum());
    println!("    {:<46} {:>7.1}", "pointer, heap allocated in order", r);
    let r = time(&mut || q.iter().map(|&k| { let a = &boxed_aged[k as usize]; (a.hp + a.x + a.y) as f64 }).sum());
    println!("    {:<46} {:>7.1}", "pointer, aged heap (scattered)", r);
    let r = time(&mut || q.iter().map(|&k| (hp_c[k as usize] + x_c[k as usize] + y_c[k as usize]) as f64).sum());
    println!("    {:<46} {:>7.1}", "columns, row index (table<T>)", r);
    let r = time(&mut || q.iter().map(|&k| { let s = slot_of[k as usize] as usize; if gen[k as usize] != 1 { return 0.0; } (hp_c[s] + x_c[s] + y_c[s]) as f64 }).sum());
    println!("    {:<46} {:>7.1}", "columns, id -> slot with generation check", r);

    let alive: Vec<u64> = vec![!0u64; N / 64];
    let r = time(&mut || q.iter().map(|&k| { let k = k as usize; if alive[k >> 6] >> (k & 63) & 1 == 0 { return 0.0; } (hp_c[k] + x_c[k] + y_c[k]) as f64 }).sum());
    println!("    {:<46} {:>7.1}", "columns, row index + live bit", r);
    println!("  read ONE field of one random object (hp):");
    let r = time(&mut || q.iter().map(|&k| aos[k as usize].hp as f64).sum());
    println!("    {:<46} {:>7.1}", "records, row index", r);
    let r = time(&mut || q.iter().map(|&k| hp_c[k as usize] as f64).sum());
    println!("    {:<46} {:>7.1}", "columns, row index", r);
    let r = time(&mut || q.iter().map(|&k| { let k = k as usize; if alive[k >> 6] >> (k & 63) & 1 == 0 { return 0.0; } hp_c[k] as f64 }).sum());
    println!("    {:<46} {:>7.1}", "columns, row index + live bit", r);
    let r = time(&mut || q.iter().map(|&k| { let s = slot_of[k as usize] as usize; if gen[k as usize] != 1 { return 0.0; } hp_c[s] as f64 }).sum());
    println!("    {:<46} {:>7.1}", "columns, id with generation check", r);
    println!("  scan one field over all rows, skipping dead rows:");
    let r = { let t = Instant::now(); let mut a = 0.0f64; for _ in 0..4 { a += black_box(hp_c.iter().sum::<f32>() as f64); } black_box(a); t.elapsed().as_nanos() as f64 / (4 * N) as f64 };
    println!("    {:<46} {:>7.2}", "columns, dense 0..len (no check)", r);
    let r = { let t = Instant::now(); let mut a = 0.0f64; for _ in 0..4 { let mut acc = 0.0f32; for k in 0..N { if alive[k >> 6] >> (k & 63) & 1 != 0 { acc += hp_c[k]; } } a += black_box(acc as f64); } black_box(a); t.elapsed().as_nanos() as f64 / (4 * N) as f64 };
    println!("    {:<46} {:>7.2}", "columns, 0..hw with live bit per row", r);

    println!("  follow links (linked structure, random cycle through all 1M):");
    let cyc = { let p = perm(&mut rnd); let mut next = vec![0u32; N]; for i in 0..N { next[p[i] as usize] = p[(i + 1) % N]; } next };
    struct Node { next: *const Node, _p: [u8; 56] }
    let mut nodes: Vec<Box<Node>> = Vec::with_capacity(N);
    let mut at: Vec<Option<Box<Node>>> = (0..N).map(|_| None).collect();
    for &k in &order { at[k as usize] = Some(Box::new(Node { next: std::ptr::null(), _p: [0; 56] })); }
    for i in 0..N { nodes.push(at[i].take().unwrap()); }
    for i in 0..N { let t: *const Node = &*nodes[cyc[i] as usize]; nodes[i].next = t; }
    let r = time(&mut || { let mut p: *const Node = &*nodes[0]; for _ in 0..OPS { p = unsafe { (*p).next }; } p as usize as f64 });
    println!("    {:<46} {:>7.1}", "pointers (64 B nodes, aged heap)", r);
    let r = time(&mut || { let mut i = 0u32; for _ in 0..OPS { i = cyc[i as usize]; } i as f64 });
    println!("    {:<46} {:>7.1}", "ids into a column (4 B per link)", r);
    let handles: Vec<(u32, u32)> = cyc.iter().map(|&n| (n, 1)).collect();
    let r = time(&mut || { let mut h = handles[0]; let mut dead = 0u32; for _ in 0..OPS { if gen[h.0 as usize] != h.1 { dead += 1; } h = handles[h.0 as usize]; } (h.0 + dead) as f64 });
    println!("    {:<46} {:>7.1}", "ids + generation check (8 B handles)", r);
}

/// Native baseline for `GET /rooms/{r}/messages?limit=50`: the same scan, filter, sort and take, over columns, no interpreter.
pub fn chat_scan() {
    use std::hint::black_box;
    const N: usize = 20_000;
    let room: Vec<i32> = (0..N).map(|i| (i * 7 % 20) as i32).collect();
    let seq: Vec<i64> = (0..N as i64).collect();
    let reps = 20_000;
    let t = Instant::now();
    let mut total = 0usize;
    for rep in 0..reps {
        let r = (rep % 20) as i32;
        let mut hits: Vec<(i64, usize)> = Vec::new();
        for k in 0..N { if room[k] == r { hits.push((seq[k], k)); } }
        hits.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        hits.truncate(50);
        total += hits.len();
    }
    black_box(total);
    println!("native scan + sort + top 50 over {} messages (columns, no interpreter): {:.1} us per request", N, t.elapsed().as_nanos() as f64 / reps as f64 / 1000.0);
}

/// Pressure policy on bursty work: jobs arrive in bursts of different sizes and are all freed after each burst.
pub fn pressure() {
    use crate::layout::Program;
    use crate::runtime::{Instance, Pressure, Val};
    const SRC: &str = "struct J { id: i32; name: string<64>; } global jobs: pool<J, 400000>; global keep: pool<J, 1000>;";
    println!("bursts of 50k-140k jobs (72 bytes each), all freed after each burst; epoch boundary every 1000 operations");
    for policy in [false, true] {
        let mut i = Instance::start(Program::parse(SRC).unwrap(), 1 << 30).unwrap();
        let mut p = Pressure::new(&i, 4 << 20, 64 << 20);
        let cj = i.prog.coll_of("jobs").unwrap();
        let cn = i.col(cj, "name").unwrap();
        let (mut peak, mut relieve_ns, mut calls) = (0usize, 0u128, 0usize);
        let mut after = Vec::new();
        let mut idxs = Vec::with_capacity(140_000);
        for burst in 0..8 {
            let n = 50_000 + 30_000 * (burst % 4);
            idxs.clear();
            for k in 0..n {
                let idx = i.push_item(cj, 0).unwrap();
                i.field_set(cj, idx, cn, &Val::Str(format!("job {} of burst {}", k, burst))).unwrap();
                idxs.push(idx);
                if policy && k % 1000 == 999 { let t = Instant::now(); i.relieve(&mut p); relieve_ns += t.elapsed().as_nanos(); calls += 1; }
            }
            peak = peak.max(i.committed());
            for &idx in &idxs { i.free_item(cj, idx).unwrap(); }
            if policy { let t = Instant::now(); i.relieve(&mut p); i.idle(&mut p); relieve_ns += t.elapsed().as_nanos(); calls += 2; }
            after.push(i.committed() as f64 / MB as f64);
        }
        println!("  policy {:<5} peak {:>6.2} MB | committed after each burst: {}", if policy { "on" } else { "off" }, peak as f64 / MB as f64, after.iter().map(|m| format!("{:.2}", m)).collect::<Vec<_>>().join(" "));
        if policy { println!("               {} policy calls, {:.2} ms total ({:.1} us each), {} scoped trims, {:.1} MB released", calls, relieve_ns as f64 / 1e6, relieve_ns as f64 / 1e3 / calls as f64, p.trims, p.freed as f64 / MB as f64); }
    }
}
