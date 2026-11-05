# Ordered maps: Mitochondria against Rust's BTreeMap

Written by `results.py` (`python -I bench/ordered/results.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run (2026-11-05; `--mito-only` keeps Rust's last numbers).

Machine: Windows 11, Intel64 Family 6 Model 170 Stepping 4, GenuineIntel. The same workloads on the same key sequence (MINSTD; present keys 2r, absent keys 2r+1), i64 keys to i64 values. `btree` is lib/btree.mito (items in their own pool with stable slots, each found by key), `ordmap` lib/ordmap.mito (values inside the leaves); Rust is `BTreeMap<u64,u64>` (opt-level 3, fat LTO, one codegen unit). Both Mitochondria maps are declared `unlogged` (`m: ordmap<1048576> unlogged;`): kept out of the undo log, so a failed request does not roll the map back, as a Rust map has no rollback; their arithmetic is still overflow-checked. Both run on one pinned core, timed inside their own process, alternating rounds; best of the rounds. Ratios are Mitochondria / Rust; rows are sorted by the better of the two ratios, best first.

## 10,000 keys: time (ns per operation; a range scan is per scan)

Operations per lookup workload: 1,000,000.

| Workload | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Lookup: miss | 39.4 | 39.3 | 49.2 | 0.80x | 0.80x |
| Lookup: hit | 41.9 | 39.0 | 48.5 | 0.87x | 0.80x |
| Range scan, up to 100 entries | 371.3 | 170.4 | 210.3 | 1.77x | 0.81x |
| Increment existing value | 41.5 | 39.7 | 48.4 | 0.86x | 0.82x |
| Lookup: 50% hits, 50% misses | 43.0 | 41.7 | 50.5 | 0.85x | 0.82x |
| Insert new keys | 105.3 | 77.7 | 71.4 | 1.47x | 1.09x |
| Delete existing key | 118.4 | 75.5 | 67.9 | 1.74x | 1.11x |

## 10,000 keys: memory (peak working set, MB)

The most physical memory each process held at once (Windows' peak working set), over the whole run: insert, lookups, scans and deletes. Mitochondria's includes its server (connection threads, request areas); reserved address space never touched is not counted.

| | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Peak memory | 6.4 | 6.0 | 4.4 | 1.47x | 1.38x |

## 1,000,000 keys: time (ns per operation; a range scan is per scan)

Operations per lookup workload: 1,000,000.

| Workload | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Range scan, up to 100 entries | 1447.8 | 433.6 | 1791.1 | 0.81x | 0.24x |
| Lookup: miss | 117.2 | 104.2 | 198.8 | 0.59x | 0.52x |
| Lookup: 50% hits, 50% misses | 139.4 | 136.3 | 212.7 | 0.66x | 0.64x |
| Increment existing value | 145.9 | 146.0 | 199.7 | 0.73x | 0.73x |
| Lookup: hit | 144.5 | 145.7 | 174.6 | 0.83x | 0.83x |
| Insert new keys | 172.7 | 131.1 | 146.3 | 1.18x | 0.90x |
| Delete existing key | 280.3 | 222.6 | 214.9 | 1.30x | 1.04x |

## 1,000,000 keys: memory (peak working set, MB)

The most physical memory each process held at once (Windows' peak working set), over the whole run: insert, lookups, scans and deletes. Mitochondria's includes its server (connection threads, request areas); reserved address space never touched is not counted.

| | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Peak memory | 50.1 | 32.3 | 32.1 | 1.56x | 1.01x |
