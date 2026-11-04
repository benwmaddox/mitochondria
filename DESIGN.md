# Mitochondria design (draft 0, 2026-11-03)

## Terms
- **Reserve**: claim address space. Costs no memory.
- **Commit** (OS "commit charge" / `PrivateUsage`): the OS promises to back those addresses with memory.
- **Resident** (OS "working set"): committed pages currently in RAM.
- **Live**: bytes the program is actually using right now. Only the language runtime can know this.
- **Generational id**: an index plus a counter that bumps on removal, so a stale id is rejected instead of silently pointing at someone else's data.

## Model
One reservation holds the whole program, laid out at startup from declared maxima (Stasis: one global state struct, no hidden heap).
Each region kind defines what "live" means, so live is structural, not inferred from page faults.

| Kind | Declared | Live is | Prototype |
|---|---|---|---|
| `scratch(epoch)` | max bytes | bump top; reset to 0 at epoch end | yes |
| `pool<T,N>` dense | slot count | occupied prefix; swap-remove keeps it dense | yes |
| suspend-record pool | records | one exact-size record per waiting job | yes (fixed size) |
| `fixed`, `seq`, `text`, sparse pool | | | later |

Each region has an explicit **commit watermark** advanced in chunks, and `trim` returns memory above live (keep some slack).
Exhaustion is a typed `Full` result, or a refusal at startup if the declared total is over budget. It is never an OS out-of-memory kill.

## Where ideas come from
| Idea | From |
|---|---|
| Global state tree, no hidden heap, `inspect` | Stasis |
| Compiler-owned bounded families (pool, ring, queue, map, grid...) | Stasis |
| Tick/frame as the unit of work; here generalized to an epoch | Stasis |
| Layout-checked live swap | Stasis (planned here) |
| Reserve-ahead, admission control, declared maxima, no unbounded growth | next-lang |
| Occupancy per region kind, commit watermark, trim from live, suspend records | new |

## Claims and first results (`cargo run --release`, Windows 11)
1. **Reserve is free.** Reserving 1088 MB across 3 regions moved OS commit and resident by +0.0 MB.
2. **Self-reported live/committed matches the OS.** After filling 300 MB of scratch: committed 300.0 MB, OS commit +300.6, resident +300.0.
3. **Epoch reset is O(1) but does not release memory by itself.** Reset took 2.1 µs; OS numbers stayed at +300 MB until `trim`, which dropped them to +8.0 MB (the requested slack). So trim is the policy step, and it works.
4. **Dense pool live is a prefix, so trim is exact.** 1M x 64 B items (61 MB live), remove 90% -> live 6.1 MB but OS still +71 MB; after trim 62 -> 8 MB committed, OS commit +71 -> +17 MB. Stale ids rejected, surviving items intact. The ~9 MB above the region is the pool's id tables, which are still a plain `Vec` (see Gaps).
5. **Full is a value.** An over-budget region and a scratch allocation past the reservation both return `Full`.
6. **Suspended jobs.** 10,000 waiters as 96-byte records: 0.92 MB live, OS +0.94 MB. The baseline (one page-backed stack each) cost +40 MB, but that is arithmetic and a strawman: the real test needs the compiler to compute record sizes per suspend point.

## Gaps and risks (honest list)
- Pool id tables (`slot_of`, `gen`, ...) are not in a region yet; they should be a `fixed` region sized from the pool declaration.
- Fixed-size records only. The size-class idea for suspend records is untested.
- Dense pools invalidate slot addresses on removal; only ids are stable.
- Decommit then re-touch costs page faults; latency after trim is not measured.
- Windows only (`VirtualAlloc`); Linux needs `mmap` + `madvise` behind a cfg.
- No language yet: no parser, layout compiler, or interpreter.

## Next
1. Move pool id tables into regions; add `seq`, `text`, `fixed`, sparse pool; a Linux backend.
2. Measure re-fault cost after trim and pick default slack policy.
3. Tiny frontend: parse a `state { ... }` tree with capacities, compute layout, fail over budget, print `inspect`.
4. Suspend points: compute record layout per `await` in a toy job language, then rerun the 10,000-waiter test with real sizes.
