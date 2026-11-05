# Ordered maps: Mitochondria against Rust's BTreeMap

Written by `results.py` (`python -I bench/ordered/results.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run (2026-11-05; `--mito-only` keeps Rust's last numbers).

Machine: Windows 11, Intel64 Family 6 Model 170 Stepping 4, GenuineIntel. The same workloads on the same key sequence (MINSTD; present keys 2r, absent keys 2r+1), i64 keys to i64 values. `btree` is lib/btree.mito (items in their own pool with stable slots, each found by key), `ordmap` lib/ordmap.mito (values inside the leaves); Rust is `BTreeMap<u64,u64>` (opt-level 3, fat LTO, one codegen unit). Every Mitochondria request is atomic (undo-logged) and overflow-checked. Both run on one pinned core, timed inside their own process, alternating rounds; best of the rounds. Ratios are Mitochondria / Rust; rows are sorted by the better of the two ratios, best first.

## 10,000 keys: time (ns per operation; a range scan is per scan)

Operations per lookup workload: 1,000,000.

| Workload | btree | ordmap | Rust | btree/Rust | ordmap/Rust |
|---|---:|---:|---:|---:|---:|
| Lookup: miss | 40.8 | 38.6 | 49.8 | 0.82x | 0.77x |
| Range scan, up to 100 entries | 387.6 | 171.6 | 214.4 | 1.81x | 0.80x |
| Lookup: hit | 41.7 | 40.4 | 49.4 | 0.84x | 0.82x |
| Increment existing value | 43.4 | 41.0 | 49.1 | 0.88x | 0.83x |
| Lookup: 50% hits, 50% misses | 44.7 | 43.4 | 51.0 | 0.88x | 0.85x |
| Delete existing key | 125.7 | 77.3 | 69.7 | 1.80x | 1.11x |
| Insert new keys | 126.2 | 86.9 | 72.7 | 1.74x | 1.20x |

## 10,000 keys: memory (peak working set, MB)

The most physical memory each process held at once (Windows' peak working set), over the whole run: insert, lookups, scans and deletes. Mitochondria's includes its server (connection threads, request areas); reserved address space never touched is not counted.

| | btree | ordmap | Rust | btree/Rust | ordmap/Rust |
|---|---:|---:|---:|---:|---:|
| Peak memory | 6.6 | 6.0 | 4.4 | 1.52x | 1.37x |

## 1,000,000 keys: time (ns per operation; a range scan is per scan)

Operations per lookup workload: 1,000,000.

| Workload | btree | ordmap | Rust | btree/Rust | ordmap/Rust |
|---|---:|---:|---:|---:|---:|
| Range scan, up to 100 entries | 1500.4 | 598.8 | 1459.3 | 1.03x | 0.41x |
| Lookup: miss | 128.8 | 134.1 | 223.6 | 0.58x | 0.60x |
| Increment existing value | 160.7 | 189.5 | 219.2 | 0.73x | 0.86x |
| Lookup: 50% hits, 50% misses | 156.0 | 158.9 | 199.4 | 0.78x | 0.80x |
| Lookup: hit | 170.5 | 200.8 | 209.9 | 0.81x | 0.96x |
| Insert new keys | 214.2 | 164.1 | 151.3 | 1.42x | 1.08x |
| Delete existing key | 357.7 | 284.7 | 194.4 | 1.84x | 1.46x |

## 1,000,000 keys: memory (peak working set, MB)

The most physical memory each process held at once (Windows' peak working set), over the whole run: insert, lookups, scans and deletes. Mitochondria's includes its server (connection threads, request areas); reserved address space never touched is not counted.

| | btree | ordmap | Rust | btree/Rust | ordmap/Rust |
|---|---:|---:|---:|---:|---:|
| Peak memory | 94.4 | 61.3 | 32.1 | 2.94x | 1.91x |
