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

## Prototype 2: declared state tree (`examples/world.mito`)
Language so far: `struct`, `global`, scalars, `string<N>`, `list<T,N>`, `T[N]`, nested structs. A struct that contains itself is rejected (sizes must be fixed). Run a script of `push/pop/fill/set/get/clear/trim/inspect` against it.
- **Layout:** 30.59 MB reserved for a world with a 500,000-slot item list; committed 0.008 MB at start (headers and fixed fields only).
- **Live is structural:** after 200,000 pushes `inspect` reports per path reserved vs live (items: 32,000,004 reserved, 3,200,013 live). OS commit +12.3 MB matched the program's committed 12.2 MB.
- **Typed limits:** a 57-byte string into `string<40>` -> `FULL`; the 1001st push into a 1000-slot list -> `FULL`; `scores[8]` -> index error; `--budget 16` -> refused to start.
- **After clear + trim:** released 12.2 MB; committed 0.074 MB.
- **Finding (important):** 200,000 items have only 3.05 MB live but 12.2 MB committed, 4x. Each 64-byte item slot is mostly dead capacity (an empty 8-slot tag list, a 16-byte name), and every page contains some live bytes, so page trimming cannot reclaim the dead parts. Stasis solves this layout problem by lowering struct arrays to structure-of-arrays (one contiguous column per field). Doing that here puts all `tags` capacity together and all `name` capacity together, so dead space forms whole pages that `trim` can release. Next step.

## Prototype 3: `table<Struct, N>` (structure-of-arrays by type change)
`list<Item, N>` stores one record after another. `table<Item, N>` stores the same struct, with the same source, as one contiguous column per field (nested structs flatten to `pos.x`, `pos.y`; a `string<M>` becomes a length column plus a bytes column; a `list<scalar, M>` likewise). Table rows may hold scalars, scalar arrays, strings, lists of scalars and structs of those; anything else is a compile error for now. See `examples/soa.mito`.

**Access cost model.** For a field of row `i`: `column_base + i * field_size`. `column_base` is a compile-time constant offset from the global's fixed address, so there is no pointer to load. The only runtime check is liveness, `i < len`, one load of the table length; a loop over live rows (`0..len`) needs no per-access check at all. Nested strings and lists add one more load and compare (`j < row's length`). Growth operations (`push`) also check that the page is committed; reads never do, because live implies committed. That is the same shape as Stasis structure-of-arrays access plus one liveness compare.

**Measured (1M rows, same data in both layouts, native loop, `cargo run --release -- examples/soa.mito examples/soa.script`):**
| Sum of one field | `list<Item>` (64 B records) | `table<Item>` (column) |
|---|---|---|
| `hp` (f32) | 4.15 ns/row | 0.72 ns/row |
| `id` (i32) | 3.63 ns/row | 0.56 ns/row |

About 5.8-6.5x faster, as expected from reading 4 bytes per row instead of a 64-byte record. Memory: the list committed 68.7 MB for 22.9 MB live; the table committed about 22.9 MB for 22.9 MB live, because the name and tags columns only commit pages that hold a non-empty value. The third result of the earlier "finding" is therefore confirmed: SoA lets dead capacity form whole pages.

**Caveat.** That works because almost every name and tag list in the test is empty. If every row has a name averaging half its capacity, every page of the name column has live bytes and nothing is released; the dead half stays. Per-field page reclaim only helps sparse fields. `swap_remove` keeps rows dense but changes row order; stable ids for tables are not built.

Logic tests (`cargo test --release`): table/list agree, typed Full and not-live errors, swap-remove moving strings, and recommit after trim.

## Gaps and risks (honest list)
- Pool id tables (`slot_of`, `gen`, ...) are not in a region yet; they should be a `fixed` region sized from the pool declaration.
- Fixed-size records only. The size-class idea for suspend records is untested.
- Dense pools invalidate slot addresses on removal; only ids are stable.
- Decommit then re-touch costs page faults; latency after trim is not measured.
- Windows only (`VirtualAlloc`); Linux needs `mmap` + `madvise` behind a cfg.
- No language yet: no parser, layout compiler, or interpreter.

## Next
0. Done for scalar-ish rows (`table`). Next: stable ids and generation checks for tables; rows holding lists of structs; a `foreach` over live rows that proves bounds checks away; automatic choice of list vs table from a compiler hint.
1. Move pool id tables into regions; add `seq`, `text`, `fixed`, sparse pool; a Linux backend.
2. Measure re-fault cost after trim and pick default slack policy.
3. Tiny frontend: parse a `state { ... }` tree with capacities, compute layout, fail over budget, print `inspect`.
4. Suspend points: compute record layout per `await` in a toy job language, then rerun the 10,000-waiter test with real sizes.
