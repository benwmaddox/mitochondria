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

## How it compares, and which programs fit (draft)

Mitochondria is not a garbage collector (GC) and not a plain arena. Liveness is **declared**: a row is live because it is inside its table's `len`, a string is live up to its own length, and the program's own `push`/`pop`/`remove` calls maintain that. Nothing is traced. `trim` is the only GC-like step: it scans the declared structure (no pointer chasing) and returns pages that hold no live byte to the OS, when the program asks.

| | Arena | GC | Mitochondria |
|---|---|---|---|
| Who decides live | one bump pointer | tracing reachability | length/occupancy in the declared structure |
| Free one item | no | yes, eventually | yes (swap-remove) |
| Memory returns | at reset | at some collection | at `trim`, chosen by the program |
| Pauses | none | yes, or concurrent overhead | none; `trim` runs only when called |
| Peak memory knowable | yes | no | yes: the declared total |
| Access cost | pointer | pointer, maybe barriers | constant offset plus a `len` compare |
| Arbitrary object graphs | awkward | natural | no: use ids |
| Dangling references | caller's problem | impossible | detected by generation check |

### Good fit: the shape and bounds are known, data is mostly flat
- **Games and simulations** (the Stasis home turf): entities, particles, projectiles, inventory. Fixed caps, per-tick work over all live rows, replay and determinism matter. Columns make per-field passes (physics, culling) fast.
- **Servers with bounded concurrency:** sessions, connections, rooms, queues, rate-limit tables, with a declared maximum per kind. Overload becomes a typed `FULL` and a 503, and the memory budget is the capacity plan. This is the 10,000-waiting-jobs case.
- **Embedded and real-time:** no allocator, no pause, a known worst-case footprint, a build that fails when the budget is exceeded.
- **Data pipelines and analytics over batches:** read a bounded batch into tables, do column scans, reset the scratch epoch. Columns are the right layout and the epoch is the lifetime.
- **Compilers and tools with phases:** token and node arrays plus per-phase scratch, reset per file or per request (this is the next-lang selfhost shape).
- **Financial or monitoring state machines:** order books, per-account records, metrics ring buffers, anything where "how much memory at peak" must be answerable before launch.

What they share: items are mostly **identified by an index or id**, the **maximum is known or choosable**, lifetimes are **per tick, per request, or per collection membership**, and most work is **loops over many similar records**.

### Poor fit: irregular, unbounded, or pointer-rich data
- **Programs with large irregular object graphs:** DOM trees, ASTs with shared and rewritten subtrees, scene graphs with arbitrary reparenting, symbolic algebra, Prolog or Lisp style data. Everything becomes ids into tables, and every traversal pays an extra lookup plus bookkeeping a GC language gets free.
- **Unbounded or heavy-tailed sizes:** a text editor on arbitrary files, a browser, a general document store, a web crawler. A single declared maximum is either too small (visible `FULL` on real input) or wastefully large. Page reclaim helps only when whole pages are dead.
- **Highly dynamic shapes:** plugin systems, scripting hosts, JSON of unknown schema, anything that builds types or structures from input at run time.
- **Long-running programs with many mixed lifetimes and heavy sharing:** where "who owns this" is a real question and nobody wants to answer it by hand (large application frameworks, many-library glue code).
- **Exploratory scripting and quick prototypes:** declaring maxima up front is friction when the data shape is still being discovered.
- **Caches with no natural eviction rule:** the bound exists but the policy (what to drop) must be written by the program; a GC cache with weak references is easier to start.

### Middle ground: works with a deliberate pattern
- **Graphs and trees:** store nodes in a `table`, edges as ids (parent id, first child id, or an edge table). Good when the graph is large and traversed in bulk; awkward when it is small and edited by hand.
- **Variable-length data:** fixed-capacity strings and lists, or a `text` arena region for long text with a declared total. A larger typed `Full` budget is the price.
- **Shared state across jobs:** tables passed to jobs by explicit view, per the next-lang model; no hidden sharing.

### Cost the language is choosing to pay
- **Declare maxima everywhere.** The benefit is a known worst case and typed exhaustion.
- **Remove things yourself.** A forgotten removal is a leak, but a bounded one that shows up as `FULL`; in a GC language the same bug hides until the process is huge.
- **Ids instead of references.** The benefit is a generation check instead of a dangling pointer.
- **Fixed per-slot capacity.** The benefit is constant-time exact addressing; the cost is dead space inside slots, partly recovered by columns and `trim`.

### Possible later: debug-only reachability check
A development mode that reports rows no id field in the program refers to (orphans), the way a leak detector would, without changing how release builds run. Not a collector: it only reports. Useful because the only real hazard of declared liveness is forgetting to remove.

## Cost of `trim` (measured: `cargo run --release -- exp trim`, Windows 11, one thread)
Item removal itself never touches the OS: `remove` and `pop` only change a length (and swap-remove moves one row). The OS is involved only in `trim`, which is batched by **page run**, not by item. Test: `table<I, 1M>` with a 16 MB `name` column (4096 pages, 256 rows per page); every row gets a name, then all but every Nth are cleared, then `trim`.

| Names kept | Pages freed | Scan | OS calls | OS time | Notes |
|---|---|---|---|---|---|
| 1,000,000 (all) | 0 | 8.6 ms | 0 | 0 | scan is ~9 ns per live string |
| 3,907 (one per page) | 0 | 0.7 ms | 0 | 0 | 99.9% of the column is dead, nothing can be released |
| 1,000 | 2907 (11.4 MB) | 0.8 ms | 1000 | 3.2 ms | ~3 us per call on 3-page runs |
| 10 | 3897 (15.2 MB) | 1.1 ms | 10 | 2.2 ms | ~0.55 us per page on long runs |
| 1 | 3906 (15.3 MB) | 0.7 ms | 1 | 2.1 ms | one call, still ~0.5 us per page |

A second `trim` right after (nothing to free) costs just the scan, 0.5-9 ms depending on live strings.

**Reading the numbers**
- **Scan:** about 9 ns per live string or list (one range each), near zero for dense scalar columns (one range per column). 1M live strings is about 9 ms.
- **Release:** roughly 0.5 us per page plus about 2.5 us per OS call. The worst case is alternating live and dead pages, one call per dead page; coalescing already merges adjacent dead pages.
- **Re-touch:** writing into a released page commits and zeroes it again: about 4 us per page in the test (this includes the interpreter's path lookup, so the OS part is lower). Trimming memory you will reuse soon costs more than it saved, so keep slack and trim at epoch boundaries or idle time, not in the hot path.
- **Page granularity is the real limit:** one live 8-byte name per 256-row page keeps the whole 4 KB page. The fix is layout (denser columns), not a faster trim.

**Cost controls to add (not built):** skip dead runs shorter than a threshold (for example 16 pages) so tiny holes never reach the OS; keep a high-water slack that decays over several epochs; maintain per-page live counters on write so `trim` does not rescan live strings; run `trim` only at epoch end or when committed exceeds live by a set ratio. **Not measured:** multi-threaded decommit (it makes other cores flush their address-translation caches) and Linux `madvise` costs.

## Trimming part vs whole, and the mailbox/worker shape
`trim <path>` (and `Instance::trim_path`) trims only the pages wholly inside one global or field. Globals of 4 KB or more now start on a page boundary so a scope never shares a page with a neighbour; a scope's two edge pages are left alone if they are shared.

**Measured** (`cargo run --release -- exp trim`, second table): `big` holds 1M live strings and never changes; `worker` bursts to 50,000 rows with names and then clears. Both trims freed the same 1.14 MB.

| | Scan | Live ranges scanned | OS calls |
|---|---|---|---|
| `trim worker` | 0.02 ms | 3 | 1 |
| `trim` (whole) | 17.6 ms | 1,000,006 | 1 |

Whole-program trim pays for rescanning every live string in regions that did not change; scoped trim pays only for the part you name. The OS work is identical.

**Design for mailbox and worker programs (not built)**
- **One region per worker.** Each worker's private state and scratch is its own page-aligned region, owned by that worker alone. A worker trims only its own region, at a time it chooses.
- **Natural trigger:** the mailbox empties. Idle is the epoch boundary, so the burst's scratch is dead and the next burst is not imminent. Trimming mid-burst would pay the ~4 us per page recommit.
- **Mailbox in the always-committed tier.** A mailbox is a fixed ring of fixed-size message slots (string bodies bounded), committed at startup and never trimmed. Senders write into it without ever committing or releasing pages in the receiver's region, so there is no commit race between threads. Bursty payload stays in the worker's own region.
- **Admission, not growth:** a full mailbox is a typed `FULL` the sender sees (retry, drop, or backpressure), and the sum of all mailboxes plus worker regions is the static budget.
- **Commit state per region:** the prototype's page bitmap is one array and is single-threaded. Real workers need it split per region (each owner updates only its own), so scoped trim needs no lock. Decommit still makes other cores flush translation caches; that cost is not measured here.
- **Whole-program trim becomes a rare maintenance operation** (shutdown, memory-pressure signal), not the routine path.

## Data access in practice: columns and ids vs pointers (measured: `cargo run --release -- exp access`)
Native Rust loops over the same 1M items (64-byte records), 4M random operations, ns per operation. The pointer cases are `Vec<Box<Item>>` (an array of heap pointers), with the heap either allocated in order or "aged" (allocated in shuffled order, as after churn). Three runs; random-read numbers varied by under 10%.

| Access pattern | Records, row index | Pointer | Columns, row index | Columns, id + generation |
|---|---|---|---|---|
| Random read of 3 fields of one object | 12 | 21 | 10 | 30 |
| Random read of 1 field | 11 | n/a | 3 | 11-15 |
| Scan one field over all rows (earlier test) | 3.6-4.1 | n/a | 0.6-0.7 | n/a |
| Follow links through 1M objects | n/a | 182 | 29-82 (4 B ids) | 45-61 (8 B handles) |

**Reading it**
- **Row index beats pointer.** The pointer case pays an extra dependent load (fetch the pointer, then the object). Row index into a dense array has one.
- **Columns are not worse for random multi-field reads here** (10 vs 12): the three field loads are independent so the CPU overlaps their cache misses. This would change if each load depended on the previous one.
- **Scans are where columns win**: ~6x, because each cache line carries 16 useful values instead of 1.
- **The id + generation lookup is the real cost:** `id -> slot -> column` is two dependent loads plus the generation compare, so 3x slower than a row index for a 3-field read (30 vs 10) and 4-5x for a single field (11-15 vs 3). Each id lookup is about one extra cache miss.
- **Following links is faster with ids (2-6x), but partly for a layout reason:** the link column is 4-8 MB and stays in cache, while pointers jump among 64 MB of 64-byte nodes in a scattered heap. If the pointer nodes were allocated in traversal order or kept small, the gap would shrink. This number is noisy (29-82 ns across runs; other jobs share this machine).

**Practical rules this suggests**
1. Inside a tick, loops go over **row indices** (`0..len`), with no ids and no generation checks. This is the fast path and covers most work.
2. Resolve an **id to a row once** at the start of a function or message handler, then use the row for every field read. That amortizes the lookup to about one extra miss per object per handler.
3. Rows are stable until a `remove`, so do not cache a row across a removal; cache the id and resolve it again.
4. Keep ids for references that cross ticks or workers (owners, targets, mailbox contents). A per-access generation check there is the price of detecting a stale reference.
5. Per-object work that touches many fields of a few objects is the case where columns give no gain over records, and a pointer is the slowest of the three.
6. Native loops here measure layout cost only. A compiled Mitochondria program would add whatever its code generator adds, so treat these as the best case.

## Index + live bit instead of generation (Stasis style) — decision draft
Reference an item by plain row index; a bitset (1 bit per row) says whether the row is live. Removing an item clears its bit and leaves the row where it is, so indexes stay put (no swap-remove). A stale index can silently reach a different item that later reused the slot (the "ABA" problem: A, removed, B takes the slot, the old reference now reads B). We accept this by default.

**Measured** (`exp access`, two runs on a quieter machine; ns/op):
| | Row index | Row index + live bit | Id + generation |
|---|---|---|---|
| Random read, 3 fields | 7.3-9.2 | 7.7-9.7 | 17.6-19.9 |
| Random read, 1 field | 2.0-2.7 | 1.6-2.2 | 7.5-8.6 |
| Scan one field, per row | 0.43-0.47 (dense, no check) | 0.64-0.65 (live bit per row) | n/a |

The live bit is in the noise for random reads: 1M rows is 128 KB of bits, so the bit test almost always hits cache and does not delay the data load. The scan is ~40% slower than a check-free dense loop but still ~6x faster than scanning records (3.6-4.1 ns). Id + generation costs 2-3x because of the dependent `slot_of` load.

**What a stale index can and cannot do.** It can never read outside the table (the index is checked against capacity) or corrupt memory; it can only mean the wrong item. It is deterministic: the same inputs reproduce the same mix-up, which suits replay-based debugging (Stasis's record/replay).

**Proposed kinds**
- `table<T,N>`: dense, swap-remove, rows move. For bulk data, scans, and per-tick work; reference rows only within a tick.
- `pool<T,N>`: stable slots, live bit, free slots reused lowest-first, tracked with a high-water mark so scans run over `0..hw` and `trim` can release everything above it. References are indexes. This is the default for things other data points at.
- `pool<T,N> checked`: adds a generation column and 8-byte handles. Opt in per type where reuse bugs are expensive.

**When to opt in to `checked`**
- Handles that leave the program (a client sends "attack target 17"): a stale one must be rejected, not misapplied.
- Long-lived references to things freed asynchronously and reused quickly (sessions, connections).
- Debug builds of any pool: generation on in tests to find stale-reference bugs, off in release.

**Cheap mitigation without a generation:** reuse freed slots in FIFO order (oldest-freed first) so a stale index is likely to point at a still-empty slot for a long time. This conflicts with lowest-first reuse (which keeps live rows near the front for `trim`), so choose one per pool.

**Costs of holes:** a pool with live rows scattered across a wide range keeps those pages committed even if most rows are dead; lowest-first reuse keeps live rows near the front. Scans test one bit per row up to `hw`.
