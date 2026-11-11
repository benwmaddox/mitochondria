# Ordered maps: Mitochondria against Rust's BTreeMap

Written by `results.py` (`python -I bench/ordered/results.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run (2026-11-11; `--mito-only` keeps Rust's last numbers).

Machine: Windows 11, Intel64 Family 6 Model 170 Stepping 4, GenuineIntel. The same workloads on the same key sequence (MINSTD; present keys 2r, absent keys 2r+1), i64 keys to i64 values. `btree` is lib/btree.mito (items in their own pool with stable slots, each found by key), `ordmap` lib/ordmap.mito (values inside the leaves); Rust is `BTreeMap<u64,u64>` (opt-level 3, fat LTO, one codegen unit). Both Mitochondria maps are unlogged (the default; before 2026-11-11 declared `unlogged`) (no marker): kept out of the undo log, so a failed request does not roll the map back, as a Rust map has no rollback; their arithmetic is still overflow-checked. Both run on one pinned core, timed inside their own process, alternating rounds; best of the rounds. Ratios are Mitochondria / Rust; rows are sorted by the better of the two ratios, best first.

## 10,000 keys: time (ns per operation; a range scan is per scan)

Operations per lookup workload: 1,000,000.

| Workload | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Lookup: miss | 38.9 | 38.1 | 49.0 | 0.79x | 0.78x |
| Lookup: hit | 40.5 | 38.7 | 48.8 | 0.83x | 0.79x |
| Increment existing value | 40.7 | 39.1 | 48.9 | 0.83x | 0.80x |
| Range scan, up to 100 entries | 375.3 | 172.7 | 210.5 | 1.78x | 0.82x |
| Lookup: 50% hits, 50% misses | 45.1 | 42.2 | 50.7 | 0.89x | 0.83x |
| Insert new keys | 99.0 | 77.0 | 72.7 | 1.36x | 1.06x |
| Delete existing key | 102.5 | 73.6 | 68.7 | 1.49x | 1.07x |

## 10,000 keys: memory (peak working set, MB)

The most physical memory each process held at once (Windows' peak working set), over the whole run: insert, lookups, scans and deletes. Mitochondria's includes its server (connection threads, request areas); reserved address space never touched is not counted.

| | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Peak memory | 6.4 | 6.1 | 4.4 | 1.47x | 1.39x |

## 1,000,000 keys: time (ns per operation; a range scan is per scan)

Operations per lookup workload: 1,000,000.

| Workload | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Range scan, up to 100 entries | 1311.3 | 463.7 | 1287.3 | 1.02x | 0.36x |
| Lookup: miss | 109.4 | 107.0 | 195.6 | 0.56x | 0.55x |
| Lookup: 50% hits, 50% misses | 135.2 | 143.1 | 207.6 | 0.65x | 0.69x |
| Increment existing value | 139.9 | 147.0 | 187.6 | 0.75x | 0.78x |
| Lookup: hit | 139.0 | 141.5 | 182.6 | 0.76x | 0.77x |
| Insert new keys | 155.0 | 131.6 | 148.6 | 1.04x | 0.89x |
| Delete existing key | 251.6 | 218.3 | 175.6 | 1.43x | 1.24x |

## 1,000,000 keys: memory (peak working set, MB)

The most physical memory each process held at once (Windows' peak working set), over the whole run: insert, lookups, scans and deletes. Mitochondria's includes its server (connection threads, request areas); reserved address space never touched is not counted.

| | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Peak memory | 50.0 | 32.4 | 32.1 | 1.56x | 1.01x |
