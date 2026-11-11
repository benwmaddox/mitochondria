# Ordered maps: Mitochondria against Rust's BTreeMap

Written by `results.py` (`python -I bench/ordered/results.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run (2026-11-11; `--mito-only` keeps Rust's last numbers).

Machine: Windows 11, Intel64 Family 6 Model 170 Stepping 4, GenuineIntel. The same workloads on the same key sequence (MINSTD; present keys 2r, absent keys 2r+1), i64 keys to i64 values. `btree` is lib/btree.mito (items in their own pool with stable slots, each found by key), `ordmap` lib/ordmap.mito (values inside the leaves); Rust is `BTreeMap<u64,u64>` (opt-level 3, fat LTO, one codegen unit). Both Mitochondria maps are unlogged (the default; before 2026-11-11 declared `unlogged`) (no marker): kept out of the undo log, so a failed request does not roll the map back, as a Rust map has no rollback; their arithmetic is still overflow-checked. Both run on one pinned core, timed inside their own process, alternating rounds; best of the rounds. Ratios are Mitochondria / Rust; rows are sorted by the better of the two ratios, best first.

## 10,000 keys: time (ns per operation; a range scan is per scan)

Operations per lookup workload: 1,000,000.

| Workload | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Increment existing value | 40.3 | 38.8 | 48.9 | 0.82x | 0.79x |
| Lookup: hit | 41.8 | 39.2 | 48.8 | 0.86x | 0.80x |
| Range scan, up to 100 entries | 375.6 | 169.2 | 210.5 | 1.78x | 0.80x |
| Lookup: miss | 39.9 | 39.6 | 49.0 | 0.81x | 0.81x |
| Lookup: 50% hits, 50% misses | 44.7 | 42.0 | 50.7 | 0.88x | 0.83x |
| Insert new keys | 93.6 | 73.5 | 72.7 | 1.29x | 1.01x |
| Delete existing key | 104.1 | 72.9 | 68.7 | 1.51x | 1.06x |

## 10,000 keys: memory (peak working set, MB)

The most physical memory each process held at once (Windows' peak working set), over the whole run: insert, lookups, scans and deletes. Mitochondria's includes its server (connection threads, request areas); reserved address space never touched is not counted.

| | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Peak memory | 6.4 | 6.1 | 4.4 | 1.47x | 1.40x |

## 1,000,000 keys: time (ns per operation; a range scan is per scan)

Operations per lookup workload: 1,000,000.

| Workload | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Range scan, up to 100 entries | 1230.8 | 452.4 | 1287.3 | 0.96x | 0.35x |
| Lookup: miss | 114.6 | 108.5 | 195.6 | 0.59x | 0.55x |
| Lookup: 50% hits, 50% misses | 136.8 | 134.2 | 207.6 | 0.66x | 0.65x |
| Increment existing value | 141.6 | 140.2 | 187.6 | 0.75x | 0.75x |
| Lookup: hit | 139.8 | 141.0 | 182.6 | 0.77x | 0.77x |
| Insert new keys | 157.6 | 130.7 | 148.6 | 1.06x | 0.88x |
| Delete existing key | 261.9 | 223.2 | 175.6 | 1.49x | 1.27x |

## 1,000,000 keys: memory (peak working set, MB)

The most physical memory each process held at once (Windows' peak working set), over the whole run: insert, lookups, scans and deletes. Mitochondria's includes its server (connection threads, request areas); reserved address space never touched is not counted.

| | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |
|---|---:|---:|---:|---:|---:|
| Peak memory | 50.0 | 32.3 | 32.1 | 1.56x | 1.01x |
