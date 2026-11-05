# Mitochondria measurements and experiment log

The running log of prototypes and measurements since 2026-11-03, oldest first. Numbers stay valid for what they
measured; design statements in older rounds are history, superseded by [DESIGN.md](../DESIGN.md). Measurements that used
the Rust runtime cannot be rerun after it was removed (last commit with it: `0070fb9`).

## What we learned (summary)
- **Reserving is free and the runtime's accounting matches the OS.** Reserving 1088 MB moved OS commit by 0.0 MB;
  after writing 300 MB the runtime reported 300.0 MB committed and the OS 300.6 MB (Claims, first results).
- **Trim costs per page run, not per item:** about 0.5 µs per page plus 2.5 µs per OS call. Page granularity is the
  real limit: one live value keeps a whole 4 KB page (Cost of trim).
- **`string<N>` wastes memory on uneven text:** 2000-byte chat bodies averaging 120 bytes left 39.5 MB committed for
  3.1 MB live (Round 3). Packed text is planned.
- **Columns win scans about 6x; an id lookup costs 2-3x a row index; following ids beats pointers 2-6x** (Data access).
- **Workers: the handler's own work dominates;** handing a request to another worker costs 2-6 µs a hop (Round 5).
- **Compiled C is as fast as hand-written Rust.** The C runtime built by clang -O2 answers the chat's GET messages in
  2.5 µs against 2.3 µs for the same requests written directly in Rust; the TinyCC build is about 4x slower but builds
  in 20-30 ms against about 1 s (Round 11, Compile times).
- **HTTP: the connection is the cost.** On Windows a connection costs 200-500 µs, the same with a bare Rust server,
  against about 2 µs of handling; only keep-alive removes it (Rounds 15, 16).
- **Typed, static programs found no wrong answers** in the existing examples when every answer's shape was checked
  (Round 17).


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

**Built (2026-11-03): `pool<T,N>` and `pool<T,N> checked`.** The list below says what each kind does; `table` was built earlier.
- `table<T,N>`: dense, swap-remove, rows move. For bulk data, scans, and per-tick work; reference rows only within a tick.
- `pool<T,N>`: stable slots, live bit, free slots reused lowest-first, tracked with a high-water mark so scans run over `0..hw` and `trim` can release everything above it. References are indexes. This is the default for things other data points at.
- `pool<T,N> checked`: adds a generation column and 8-byte handles. Opt in per type where reuse bugs are expensive.

**When to opt in to `checked`**
- Handles that leave the program (a client sends "attack target 17"): a stale one must be rejected, not misapplied.
- Long-lived references to things freed asynchronously and reused quickly (sessions, connections).
- Debug builds of any pool: generation on in tests to find stale-reference bugs, off in release.

**Cheap mitigation without a generation:** reuse freed slots in FIFO order (oldest-freed first) so a stale index is likely to point at a still-empty slot for a long time. This conflicts with lowest-first reuse (which keeps live rows near the front for `trim`), so choose one per pool.

**Costs of holes:** a pool with live rows scattered across a wide range keeps those pages committed even if most rows are dead; lowest-first reuse keeps live rows near the front. Scans test one bit per row up to `hw`.

## Pool, as built (`examples/pool.mito`, `examples/pool.script`)
- **Layout:** header (high-water mark, live count, lowest-free hint, ever-allocated mark), a live bitset (1 bit per slot), a generation column only if `checked`, then the same field columns as a `table`.
- **Allocate (`push`):** the lowest free slot at or after the hint and below the high-water mark, found with word-wide bit scans; otherwise the high-water mark itself. A reused slot starts zeroed. A full pool returns `FULL` even if a slot below the mark is free, only when none is.
- **Free (`remove p k`, or `remove p k@gen` on a checked pool):** clears the bit, lowers the hint, bumps the generation if checked, and if it was the top slot, lowers the high-water mark to the highest remaining live slot + 1. Other slots do not move.
- **Access:** `p[k].field` checks `k < high-water` and the live bit; `p[k@g].field` on a checked pool also checks the generation. Plain `p[k]` is allowed on a checked pool (no generation check).
- **Trim:** live ranges of a pool are the header, the live bits up to the mark, the generation column up to the ever-allocated mark (generations must outlive their slot), fixed columns up to the mark, and string/list bytes of live slots only. Everything above the mark is dead and releasable. Holes below the mark keep their pages.
- **Measured here:** scan of 1M slots with a live bit per slot, 0.63 ns/slot, in line with the table scan (0.56-0.72 ns/row). The script shows: slots 5 and 6 freed and reported "not live", reused lowest-first; the accepted stale-index hazard (slot 7 reused, old index sees the new item); and the checked pool rejecting `0@0` after slot 0 moved to generation 1.
- **Tests:** lowest-free reuse and live-bit guard, unchecked accepts a stale index while checked rejects it, Full / high-water mark falling / trim keeping live data, clear.
- **Not done:** FIFO reuse option; per-page live counters; the debug-only generation mode; pools of rows with lists-of-lists.

## Choosing: list, table or pool
| | `list<T,N>` | `table<T,N>` | `pool<T,N>` |
|---|---|---|---|
| Storage | record after record | one column per field, rows dense | one column per field, slots stable, live bit |
| Remove | pop last only | swap-remove (rows move, order changes) | free a slot (nothing moves) |
| Reference by | position | row, valid only until the next removal | slot index (or `slot@gen` if `checked`) |
| Scan one field | strided (slow) | fastest (no check) | fast (one bit test per slot, up to the mark) |
| Holes | none | none | yes, below the high-water mark |
| Trim | by length | exact (live is a prefix) | above the mark; holes keep pages |

**Use `table` when** nothing outside the table refers to a row for longer than one tick or handler: particles, bullets, per-frame work lists, batch records, columns you only scan, results and scratch rows. Order may change; rows are cheap to move.

**Use `pool` when** other data holds on to an item between ticks: units that others target, owners and parents, sessions, connections, inventory slots, anything in a mailbox message. Slots do not move, so a stored index stays meaningful until the item is freed.

**Use `pool ... checked` when** a stale reference would be costly: handles sent outside the program, long-lived references to things freed and reused quickly, or any pool in a debug build.

**Use `list` when** the data is small, order matters, and you add and remove at the end (a stack, a short log) or you read whole records at a time.

Rules of thumb: start with `table`; switch to `pool` the first time you need to store a reference to a row. Frequent removal in the middle with no references stored is still `table` (swap-remove is cheap). Many references into data that is also scanned every tick is `pool` (the scan pays one bit test per slot, about 0.6 vs 0.45 ns). A pool that is mostly empty below a high mark wastes scan time and pages; consider a smaller capacity or a `table` plus a separate id map.

## Mailbox/worker vs Erlang (draft; Mitochondria side is design only)
Erlang is the reference point for message-passing concurrency. The two models differ in granularity, overload behavior and who reclaims memory. Mitochondria's mailbox/worker model is only a sketch (see "Trimming part vs whole"); Erlang's has run production systems for decades.

| | Erlang | Mitochondria mailbox/worker (design) |
|---|---|---|
| Unit of isolation | A process per task or entity; millions are normal (about 2-3 KB each when idle) | A worker with its own page-aligned region; dozens to thousands (4 KB minimum) |
| Mailbox | Unbounded queue per process | Fixed ring of fixed-size slots, always committed |
| Overload | Mailbox grows until the VM runs out of memory unless the program adds backpressure | A full mailbox is a typed `FULL` the sender must handle |
| Messages | Copied into the receiver's heap (large binaries shared by reference) | Written into a fixed slot; payload strings have declared maxima |
| Memory reclaim | Per-process garbage collector | No collector; owner trims its region when its mailbox empties |
| Process death | Heap freed at once | Region reset (O(1)), then trim |
| Peak memory | Not known ahead of time | The declared total |
| Receive | Pattern matching, including selective receive (can scan the whole mailbox) | Next slot only; no selective receive |
| Scheduling | Preemptive, by reduction count | Not designed |
| Failure handling | Supervisors, links, monitors | Not designed |
| Hot code upgrade | Built in | Layout-checked swap (Stasis idea), not built |
| Distribution | Built in | None |

**Where this model could be better:** bounded mailboxes make "a slow process eats the machine" impossible; worst-case memory is known before launch; no per-process collector and no message copy into a heap; a dead worker's region resets in microseconds.

**Where Erlang is clearly better:** one process per connection, request or entity is natural there, whereas a worker here is page-aligned and sized by declaration, so "one process per entity" should be "one pool row per entity, many rows per worker"; supervision and fault isolation are mature; preemption stops a runaway process from starving others (a worker loop that never returns would here); variable-size messages, dynamic structure and selective receive.

**Closer cousins than Erlang:** shared-nothing, thread-per-core designs (Seastar) and Go channels with a bounded buffer.

**Ideas to borrow from Erlang**
1. Supervision tree: a failed worker's region is reset and the parent restarts it. (next-lang's job-failure rules in `docs/design/job-failures-revision-2.md` are the likely source.)
2. Links and monitors: tell the holder of a handle when the worker or item dies. A checked pool's generation is the passive version.
3. A declared overflow policy per mailbox: block, drop oldest, drop newest, or fail the sender.
4. No mailbox scan for matches. Selective receive is O(n) per message; a bounded ring makes that cost visible and capped, and the design should not offer it.

## Round 3 (2026-11-03): the coherent design, built
`table` is gone; lists and pools both store struct items as columns; lists have `drop_oldest`; collections nest with per-parent
segments; `ref<pool>` follows checked generations; handlers have `fn`/`var` and are journaled (exact rollback, deferred frees);
requests run in a declared epoch; a pressure policy trims at epoch boundaries. 20 tests.

**Chat benchmark** (`bench examples/chat/chat.mito examples/chat/chat.api --n 20000`, microseconds; heap allocations per request):

| Endpoint | Round 1 | Round 2 | Round 3 | Allocs (round 3) |
|---|---|---|---|---|
| GET messages, limit 50 | 2,880 | 52 | **18.5** | 0 |
| POST message, store full | 951 | 4.3 | **1.6** | 0 |
| POST message, mixed | 1,124 | 4.6 | **1.8** | 0 |
| POST typing | 20.9 | 3.6 | 1.8 | 0 |
| GET typing | 10.7 | 2.9 | 1.1 | 0 |
| POST /sessions | 3.5 | 2.0 | 0.8 | 0 |
| DELETE /session | 3.1 | 1.1 | 0.3 | 0 |

Before the epoch work, requests made 13 to 537 heap allocations each (49 for a post, 537 for a 50-message listing, partly from the
benchmark harness itself). The handler's own sources were: replacing the per-request buffers with fresh ones (22 per request),
cloning a pool global's type (it owns a `Box`), building a field-path string to reach a nested collection, and cloning a
collection's name in case `alloc` failed. `GET /stats` still allocates (21): computing live bytes walks every range; it is a
maintenance read.

**Pressure policy, bursty pool** (`exp pressure`: 8 bursts of 50k-140k 72-byte jobs, all freed after each burst; soft 4 MB):

| | Peak | Committed after each burst (MB) |
|---|---|---|
| no policy | 9.64 MB | 3.46 5.52 7.59 9.64 9.64 9.64 9.64 9.64 |
| policy | 9.64 MB | 0.01 after every burst |

776 policy calls cost 4.0 ms in total (5.1 us each); 17 scoped trims released 52.4 MB. The first version keyed its "nothing to trim
here" memo on committed size, which does not change when items are freed, so only the first burst came back; the memo is now keyed on
a per-collection shrink counter.

**Pressure policy, chat** (soft 16 MB, hard 32 MB): committed stops at 32.00 MB; from there writes are refused with 503 (3,572 of
the 20,000 filling posts, then all later posts and typing pings) while reads keep working (GET messages 18.4 us, 0 allocations).
8 scoped trims and 1 full trim in the whole run. Two fixes were needed: `committed()` counted the whole page bitmap (31,000 pages,
about 10 us per request) and is now a running counter; and a collection that yielded nothing was rescanned after every single free
(1,003 trims), now only after 256 frees.

**A debugging note.** A segfault after the nested-header change was not undefined behaviour: `cargo test` rebuilt only the test
binary, and the server being run was the stale build from before the fix. Rebuild the binary before running it.

## Round 4 (2026-11-04): workers and mailboxes
The chat sharded across worker threads (`examples/chat-sharded`): auth shards check the session and rate limit and forward to
the room's shard; rooms[0] owns room ids and the directory. 29 tests, including the sharded chat end to end across 2 auth and 3
room workers, restart after a panic, rollback when a forward cannot be sent, and the in-flight bound.

**Throughput** (`bench-system examples/chat-sharded/chat.system --clients 32 --ops 100000 --reps 2 --spin 0`; 22 logical cores,
shared with other work; 100,000 requests per run, 70% GET messages / 20% POST message / 5% typing ping / 5% typing list):

| Configuration | req/s, run 1 | req/s, run 2 | GET p50 (us) | GET p99 (us) |
|---|---|---|---|---|
| one Server, one thread, direct calls (no mailboxes) | 69,971 | 70,416 | 14 (mean) | |
| 1 auth x 1 rooms | 46,905 | 24,354 | 566-1,327 | ~1,700 |
| 1 x 2 | 48,155 | 51,012 | 621-655 | ~1,450 |
| 2 x 4 | 104,796 | 101,351 | 182-192 | ~1,130 |
| 4 x 8 | 104,917 | 136,135 | 83-96 | 1,218-1,852 |

Workers make no heap allocations per message (measured inside each worker thread). 66 requests per run got 503: typing pings
when a room's 256 typing slots were full.

**Reading it.**
- ~~The handoff dominates.~~ **Corrected in Round 5:** measuring inside the workers showed the room worker's handler is the
  cost; the handoff adds a few microseconds per hop. One auth and one room worker are slower than one direct thread because
  the single room worker is saturated, not because of the mailboxes.
- **Sharding does pay:** 4 x 8 workers reach 1.5-1.9x the single direct-call server, and about 2.5x the 1 x 1 system. That is
  far below 12 threads' worth, because of the handoffs and because 32 client threads and 12 workers share 22 logical cores
  with other jobs.
- **Spinning before sleeping hurt here.** With a 50 us spin, 4 x 8 fell to 53-57k req/s (oversubscribed cores burn time
  spinning). The default is now to sleep at once; spinning is a setting (`spin N;`) for machines with idle cores.
- **Runs are noisy:** 1 x 1 varied 2x between runs on this shared machine.

**What sharding cost in the design** (see DESIGN.md, "What one-way forwarding cannot do"): authors stored by value instead of
`ref<sessions>`, so the `[deleted]` fallback is gone; `/stats` reports one shard; a directory owner is needed for room ids and
the room list; creating a room is two committed steps on two workers, not one transaction.

## Round 5 (2026-11-04): where the worker time goes
Three changes to cut handoff cost: signal a mailbox's consumer only when it is actually asleep; take every queued message
(up to 32) under one lock; split the in-flight table's free list into 16 locks. **They made no measurable difference** in
three alternating before/after rounds (1 x 1: 46.2-47.8k vs 46.7-48.7k req/s; 2 x 4: 75-96k vs 75-98k; 4 x 8: 115-138k vs
115-137k). They are kept (correct, tested, cheap), but they were aimed at the wrong cost.

Each worker now reports how busy it is and how long its handler takes (`bench-system`, 32 clients, 100,000 requests):

| Config | req/s | auth: busy, us per message (handler) | rooms: busy, us per message (handler) |
|---|---|---|---|
| 1 x 1 | 48,021 | 10%, 2.2 (1.9) | **100%**, 20.8 (17.6) |
| 1 x 2 | 48,676-51,067 | 17%, 3.4 (2.8) | 93-95%, 37-38 (31-32) |
| 2 x 4 | 99,920-102,634 | 17%, 3.4 (2.7) | 85-89%, 34-35 (30) |
| 4 x 8 | 100,260-136,205 | 12-13%, 3.9-4.9 (2.6-3.0) | 35-98%, 37-40 (32-35) |

The no-mailbox baseline in the same run: 70,947 req/s (14.1 us mean per request on one thread).

**What it shows**
- **The handoff is cheap:** an auth worker's whole cost per message is 2-5 us, of which the handler is 2-3 us; a room worker
  adds 3-5 us beyond its handler (decode, encode the reply, answer). At low load (4 clients, 4 x 8) a GET took 38.6 us at the
  client against about 19 us of handler work: roughly 6 us per thread crossing, three crossings per request.
- **1 x 1 is limited by one saturated room worker:** 1 / 20.8 us = 48k req/s, exactly what was measured.
- **With more threads, each handler gets slower:** 17.6 us alone, 30-35 us when 32 client threads and up to 12 workers share
  22 logical cores (with other jobs on this machine). That is CPU contention (shared cores and hyperthreads), not queueing.
  The same 4 x 8 config measured 251k req/s in a quieter moment (handler 22 us), and 100-136k in this run.
- **Shards are unbalanced:** 20 rooms over 8 shards by `id % 8` leaves room workers between 35% and 98% busy.

**So the levers are:** a cheaper handler (the interpreter's GET of 50 messages is about 18 us; the native loop is 6 us), fewer
competing threads than cores, and placing rooms by load instead of by `id % count`. Cutting handoff cost further would gain
at most a few microseconds per request.

## Round 6 (2026-11-04): binding pass and the native floor
A binder now runs once when a server loads its endpoints: every local gets a fixed slot, every name is resolved to a local,
a global or a function, builtins become an enum, argument counts and assignments are checked, and each field access caches
where its column is. Unknown names, wrong argument counts and assigning to a `let` are now load errors, not run-time 500s.

**Binding alone gained about 5%**: GET messages 17.9 -> 17.0 us in two alternating before/after rounds; other endpoints
within noise. Name lookup was not where the time went.

**The floor** (`exp json`): the same 50-message response (8,682 bytes) written by native Rust from the same state:

| | Before faster escaping | After |
|---|---|---|
| native, `write!` for integers | 7.74-7.82 us | 4.65-4.77 us |
| native, hand-written integer format | 7.23-7.24 us | 4.19-4.41 us |
| interpreter, whole request | 17.58-17.68 us | 14.72-14.75 us |

Most of the native time was escaping the message bodies byte by byte. JSON escaping now checks 8 bytes at a time for a
quote, backslash or control character (tested against a byte-by-byte reference on 2,000 generated strings). That helped
both paths by about 3 us.

**What is left:** the interpreter spends about 10.5 us per request beyond the native floor of about 4.2 us (3.4x). That is
evaluating ~250 expressions per request one tree node at a time: building and matching values, following references,
copying strings into the text area for `or(...)`. Removing it needs generated code.

## Round 7 (2026-11-04): endpoints compiled to C with TinyCC
Endpoints are now generated as C (`src/cgen.rs`) and compiled in memory with the bundled TinyCC (`third_party/tinycc`,
LGPL-2.1, loaded at run time). Known items and collections become loads at constant offsets from the reservation's base;
integer arithmetic (with the interpreter's overflow and division errors), comparisons and list/pool loops are inline;
everything dynamic (request values, strings, conditions on them, builtins) goes through a table of Rust helpers that run
the interpreter's own code, so both modes answer alike. An endpoint using something not compiled yet stays interpreted
(only `each ... order by` in the chat APIs). `--compile` turns it on for `serve`, `bench`, `system` and `bench-system`.

**Correctness checks:** the chat tests run in both modes (8 tests, twice); a differential test sends one scripted session
of 70+ requests (successes, 400/401/404/413/429/500/503 failures, rate limiting, typing expiry, deleted authors) to an
interpreted and a compiled server with the same random seed and compares every status, body and the final state; the
worker tests (forwarding, restart after a panic, sharded chat) run in both modes; the zero-allocation test passes compiled.
A crash in TinyCC 0.9.27's library on compile errors is avoided by checking the C with the bundled `tcc.exe` first.

**One server, one thread** (`bench ... --n 20000`, us per request; 0 heap allocations per request in both modes):

| Endpoint | Interpreted | Compiled |
|---|---|---|
| GET messages, limit 50 | 15.1 | **5.3** |
| POST message, mixed | 1.7 | **0.9** |
| POST message, store full | 1.5 | **0.8** |
| POST typing | 1.5 | **0.7** |
| GET typing | 1.2 | **0.5** |
| POST /sessions | 0.8 | 0.6 |

`exp json`: the 50-message GET compiled takes 4.97 us against 4.8-5.4 us for hand-written native Rust producing the same
8.7 KB; most of what is left is escaping and copying in the shared helpers. Compiling the chat API takes about 100 ms
(about 16 ms of it TinyCC, the rest the `tcc.exe` check).

**Worker system** (`bench-system ... --clients 32 --ops 100000`, two alternating rounds, req/s):

| auth x rooms | Interpreted | Compiled | room handler, us per message (interpreted -> compiled) |
|---|---|---|---|
| 1 x 1 | 70,230 / 70,570 | **144,131 / 144,805** | 12.2 -> 4.9-5.0 |
| 2 x 4 | 175,528 / 157,214 | **375,129 / 378,390** | 17.3-18.1 -> 6.6-6.7 |
| 4 x 8 | 121,066 / 147,102 | **473,012 / 479,730** | 27.0-27.8 -> 8.7 |

GET messages p99 at 4 x 8: 1,124-1,624 us interpreted, 213-215 us compiled. Compiled runs varied far less between rounds.
Against the original single interpreted server in the same session (about 71k req/s), 4 x 8 compiled is about 6.7x.

## Round 8 (2026-11-04): every construct compiles; language cases
`tests/cases/*.case` hold eight small programs (ordering, lists, arrays, nested struct fields, recursion, floats, values
whose kind changes, strings), each with a request script. Each runs interpreted and compiled; the two logs (status and
body of every request, then live bytes per collection) must match each other and a recorded `.out` file, and every
endpoint must compile. They do, as do the chat examples, so compiled is now the default (`--interpret` runs the
interpreter, which stays only as the reference for these comparisons).

What was added to the generator: floats as C doubles (helpers still write them, Rust's shortest round-trip text);
struct fields of items as column loads; fixed arrays with the interpreter's bounds errors; list indexing; collections of
plain values; references in plain state and in `var`s; loops that collect their items before running the body (for
`order by`, for a body that allocates, and for a `where` whose body writes state), as the interpreter's `select` does;
kinds that change (a `var` or a function result given values of two kinds is generated again with the joined kind:
value or null becomes a nullable reference or string, otherwise a tagged value); recursive functions as C functions,
one per combination of argument kinds, whose result kind is found by trying kinds and checking the body agrees, with
the interpreter's limit of 64 nested calls counted the same way (inlined calls count too).

The cases also found four interpreter bugs, fixed before their output was recorded: an integer stored in a narrower
field wrapped silently (40000 in an `i16` became -25536; now 413, as for a string that does not fit); `1.0 / 0.0` was
written as `inf`, which is not JSON (now `null`); `order by ... desc` reversed ties (now ties keep their order); and
assigning `a[-1] = v` wrote element 0 (now 404, as reading does).

Speed is unchanged: GET messages 5.2-5.7 us compiled before and after these changes, in alternating runs.

## Round 9 (2026-11-04): behaviour as recorded cases, behind one interface
Tests that checked runtime behaviour through Rust APIs are now cases: 16 files in `tests/cases` (8 language cases, chat,
auth, input, pressure, frees, and three systems of workers), each run through a small `Backend` interface (handle a
request, set the clock or pressure marks, report state and counters). The Rust runtime is that interface's first
implementation, interpreted and compiled, as one program or as a system. Logs must match each other and the record; the
record now ends with reserved, committed and live bytes. Every successful request of every case is replayed after its
script and must make no heap allocations when compiled (one exemption: `stat("live_kb")` in the Rust runtime).

Found and fixed on the way: request bodies were parsed leniently (an unterminated string was accepted; nested values
silently ended parsing); they must now be empty or one valid JSON object, else 400. Query values and path parameters are
percent-decoded. `try_alloc` on a full collection built an error message it then dropped (3 allocations). Tests went
from 39 to 22 Rust tests plus the 16 cases, with more scenarios covered than before.

`dynamic.case` checks values whose kind is only known at run time where the language needs an integer (index, `limit`,
`forward` key, `min`/`max`, negation), items held as such values (field reads, assignment, `free`, `slot`), a recursive
call inside `or()`, a function returning a collection, and `or()`/`exists()` over slots that are not live. Before it, four
of these did not compile, and `or(ts[3].n, -1)` answered 404 compiled where the interpreter took the fallback: every
helper call inside `or()`'s first argument now takes the fallback on a dead reference or a slot that is not live, as the
interpreter does. The interpreter itself wrapped `-(i64 minimum)` silently (now 500, integer overflow, as compiled) and
stored `true` into integer fields as 1 (now refused, as a float there is). A program the compiler refuses no longer loads
at all, so the two modes cannot disagree about which programs run.

Floats are now written as JavaScript writes numbers (Ben's choice): the fewest digits that read back exactly,
positional from 1e-6 up to 1e21 and with an exponent outside that (`1e+21`, `1e-7`); `0` for both zeros; in text
`Infinity` and `NaN`, in JSON `null`. Before, Rust's form never used an exponent (1e300 printed 301 digits). The C runtime
reproduces this with a shortest-digits formatter of its own.

## Round 10 (2026-11-04): the C runtime
`runtime/mito.c` (about 900 lines) runs a compiled program without Rust: the reservation with pages committed where
bytes are live, the write journal and rollback, lists and pools with live bits and generations, live ranges, trims and
the pressure policy, the request epoch (text and response areas), strict JSON bodies, routing and percent-decoding, every
helper the generated code calls, and the `mito_*` interface. It uses no heap: every buffer is a reservation made at start
and committed as it fills. Floats are written with an exact shortest-digits algorithm (Burger and Dybvig, with small
fixed-size big integers) and parsed exactly (the nearest double, ties to even); a test checks both against Rust on
300,000 random values (bit patterns, scaled decimals, subnormals), two texts each. The one difference found: an exact
tie between two 17-digit candidates (201409170638417.125), which Rust rounds up; the C code now does too.

The generator emits the program's layout and routes as C tables, and `emit-c` writes the whole program as one file
(1,949 lines for the chat example, compiling on its own). In the cases the C runtime is a third backend: all 14
single-program cases match their records on it from its first run. Worker systems are not ported yet.

Speed, compiled by TinyCC (the runtime too, so almost unoptimised), chat GET 50 of 1000 messages / POST message:
Rust runtime 4.1 / 0.4 us, C runtime 10.4 / 0.8 us. The clang -O2 build is expected to close this; it waits on the LLVM
install.

## Round 11 (2026-11-04): the C runtime built by clang, against Rust
With LLVM installed, the cases also build each program with clang (-O2, as a DLL) and run it: every single-program
case matches its record there too. `cargo test --release c_runtime_speed -- --ignored --nocapture` runs the chat example
(1000 messages in a room; GET returns the newest 50, about 5 KB) on each, alongside the same chat written directly in
Rust over ordinary structures (a Vec of sessions, a VecDeque of messages, Strings), whose answers are checked to be
byte-identical. Microseconds per request, steady rounds:

| | GET messages | POST message |
|---|---|---|
| Rust runtime, endpoints compiled by TinyCC | 4.2 | 0.41 |
| C runtime, TinyCC | 9.7 | 0.80 |
| **C runtime, clang -O2** | **2.5** | **0.20** |
| native Rust (Vec, VecDeque, String) | 2.3 | 0.24 |

Built optimised, the whole program in C is on par with hand-written Rust: within 10% on the read, faster on the write
(which also journals every byte it changes so a failed request rolls back, and allocates nothing; the Rust version
allocates a String per message). The native version checks less (a simpler body scan, no JSON validation).

Also new: counted loops, `each i in lo..hi` with `reverse`, `where` and `limit` (which serves as an early exit, so the
language still has one loop and no `break`). The bounds are read once and the loop variable cannot be assigned, so the
loop ends. A range has no items to take a snapshot of: its `where` is tested just before each body runs.

What journaling costs: the same speed test with the C runtime built with `-DMITO_MEASURE_NO_JOURNAL` (writes are not
recorded, so failed requests are not rolled back; for measuring only). Two runs, steady rounds, POST message:
journaling on 0.18 / 0.18 us, off 0.19 / 0.17 us. No measurable difference: a POST message journals about 12 writes,
some 110 bytes, which is a few copies into a buffer that is already committed. Skipping the journal where the compiler
proves nothing can fail after a write (the "hybrid" option) would buy nothing for this workload.

## Round 12 (2026-11-04): modules, a sorted collection in the language, `order by` removed
Modules (lib/*.mito, expanded per use when compiling) and the first two: `bag<T, N>` and `sorted<T, N, key>`, a skip list
written in the language itself (about 90 lines), with no new runtime code: both runtimes run it as ordinary state and
functions. Checked by `sorted.case` (ties in the order added, walks from a key both ways, string keys, capacity, a
failed request rolled back including its links) and by a model test: 4000 random adds, key changes and removes on 48
items, the whole order and walks from random keys compared after every step, on the compiled Rust runtime and the
clang-built C runtime. `order by` is gone: the chat's rooms are `sorted<Room, 64, created>`, the sharded chat's
directory `sorted<Dir, 256, id>`. The collect-then-run loops remain for bodies that allocate or change what `where`
tests (collect.case), now without sort keys.

## Compile times (2026-11-04)
`cargo test --release compile_times -- --ignored --nocapture`, median of 5, ms:

| Stage | chat example | sorted.case (a module) |
|---|---|---|
| parse + bind (+ module expansion) | 0.6 | 1.2 |
| generate endpoint C | 1.9 (1,148 lines) | 9.6 (3,134 lines) |
| tcc.exe check alone (a child process) | 15.2 | 18.6 |
| TinyCC, endpoints only (Rust runtime), check included | 14.6 | 20.3 |
| TinyCC, whole program with the C runtime, check included | 17.9 (2,435 lines) | 26.7 (4,422 lines) |
| clang -O0, whole program, to a DLL | 186 | 208 |
| clang -O2, whole program, to a DLL | 846 | 983 |

Interpreted loading still runs the generator (it decides what loads). Most of TinyCC's time is starting `tcc.exe` to
check the C first (its library cannot report errors to this host); compiling in memory is a few ms. Module functions are
inlined into every endpoint that calls them, which is why sorted.case generates almost 3x the chat's endpoint C.

## Round 13 (2026-11-04): workers in C
`runtime/system.c` runs a system of workers without Rust: a thread per worker, bounded mailboxes (Windows SRW locks and
condition variables, loaded from kernel32 because TinyCC's import list predates them), reply slots with timeouts,
routing by a sharded session token or in turn, `forward` with the forwarding request committed only once handed on,
and restarts after `__crash()`. Each worker kind is its own translation unit with its interface prefixed (`k0_start`),
so a system builds as one binary; `emit-c x.system` writes one file per kind and `system.c`. The message encoding and
every reply text are the Rust system's, so the forward, crash and chat-sharded cases match their records on the C
system built by TinyCC, by clang and by clang with undefined-behaviour checks. Those checks found that the program
interface was declared with different pointer types in the two units (`Mito *` against `void *`); it now takes `void *`.

Workers now count a message before answering it (both runtimes), so counters read after an answer are exact.

## Round 14 (2026-11-04): HTTP in C, and the Rust runtime removed
`runtime/http.c` is the HTTP front end in C, a copy of the Rust servers' behaviour: one request per connection, headers
up to 64 KB, a body of Content-Length bytes (at most 1 MB), invalid UTF-8 read as U+FFFD, the same response head.
`tests/http.record` holds the Rust servers' raw answers to 30 requests chosen for edge cases (a body in pieces,
Content-Length as `+14`, `abc` or past 2^64, more and fewer body bytes than it says, invalid UTF-8 in headers and bodies,
Unicode spaces around header values, a bare `\r`, no end of headers, a 64 KB head, a 1 MB body). The executables built
by TinyCC and by clang matched it byte for byte, for the chat program and the sharded chat system.

Then the Rust runtime went: the interpreter, the Rust servers and worker system (`src/runtime.rs`, `serve.rs`,
`system.rs`, `region.rs`, `os.rs`, `alloc_count.rs`: about 2,970 lines), the `.script` state runner and the `exp` experiments
(`src/exp.rs`, `examples/*.script`). The measurements above that used them stand as recorded; they cannot be rerun
from this commit (the last commit with them is `0070fb9`). Recording moved first: cases and the HTTP record are recorded
by the TinyCC build, and every `.out` file and the HTTP record were unchanged by the switch. Tests of the Rust runtime
that described language behaviour moved to the C runtime (`tests/cases/runtime.case`, JavaScript float texts and JSON
escaping, mailbox and reply-slot limits, a forward rolled back); the allocation counter went with the Rust heap. The
live Rust comparison in `c_runtime_speed` is now only the hand-written Rust baseline; the Rust runtime's own figures are
in Rounds 11 and earlier.

Found on the way: the `recursion` case overflowed the debug-build interpreter's 2 MB test-thread stack (it passed with
16 MB); the C runtime caps calls at 64 deep, so the C builds were unaffected.

## Round 15 (2026-11-04): HTTP, one send and buffers committed as used
`http_load` (an ignored test) drives the chat executable (clang -O2) over real connections, one per request, with
`MITO_LOAD_OTHER` alternating two HTTP servers in every round so a busy machine slows both alike. The machine was
shared with other work: the same build measured 240 us and 480 us for one client minutes apart, so only the alternated
comparison counts.

| one connection per request | 1 client | 8 clients | 32 clients |
|---|---|---|---|
| GET /rooms (125 bytes), two sends | 1,600-2,000 req/s | 5,800-5,900 | 5,600-6,000 |
| GET /rooms, one send + TCP_NODELAY | 1,600-1,700 | 6,000-6,150 | 5,850-6,250 |
| GET 50 messages (6 KB), two sends | 1,550-1,600 | 5,200-5,550 | 5,000-5,350 |
| GET 50 messages, one send + TCP_NODELAY | 1,600 | 5,600-6,050 | 5,600-5,850 |

One send is 3-10% faster with 8 and 32 clients and the same with one; neither showed a delayed-acknowledgement stall
(the worst request took about 5 ms in both). The point is elsewhere: a request costs about 500 us of connection
handling against about 2 us in the handler, so reusing connections (keep-alive) is the next step.

Committing connection buffers as used, idle commit charge (Windows private bytes): one program 10.2 -> 1.3 MB; the
sharded chat system with 16 connection threads 171.5 -> 34.1 MB. The rest of the system's is its own (mailboxes and
worker buffers), not HTTP.

## Round 16 (2026-11-04): Windows Defender, and what a connection costs
Windows Defender blocked a freshly built chat executable as `Trojan:Win32/Wacatac.B!ml` (`!ml`: a machine-learning
guess, a known false positive for small unsigned programs). The likely trigger was ours: the runtime looked up its
socket, lock and random-number functions by name at run time (`LoadLibraryA` + `GetProcAddress`), a pattern malware
loaders use. They are now ordinary imports (`ws2_32`, `bcrypt`, and kernel32's SRW locks and condition variables, added
to TinyCC's import lists). TinyCC executables import no lookup functions at all; clang executables list
`GetProcAddress` only through Microsoft's C runtime startup code, as every MSVC-style program does. No Defender
setting was changed. The executables have run since without a detection; a machine-learning guess can still recur,
and signing the executables would be the further step.

What a connection costs here, one client, GET /nope, 2000 connections per measurement, alternating:

| round | bare Rust server (accept, read, fixed answer, close) | chat executable |
|---|---|---|
| 1 | p50 212 us, p99 337 | p50 209 us, p99 345 |
| 2 | p50 504 us, p99 642 | p50 484 us, p99 691 |
| 3 | p50 479 us, p99 733 | p50 493 us, p99 808 |

The executable costs the same as a server that does nothing: the time is Windows' connection setup and teardown
(the machine's load moves it between 200 and 500 us), not Mitochondria's. Only reusing connections (keep-alive)
removes it.

## Round 17 (2026-11-04): the syntax change (docs/proposals/syntax.md)
Built in steps, each converted by script and checked against the recorded answers before the next. One named state,
one `.mito` file per program, collection functions, and endpoints as attribute functions returning `Response<T>` left
every recorded answer unchanged apart from checked renames (the state's name in paths and error texts) and a few bytes
less padding. Two steps changed answers on purpose, case by case:
- **Typed inputs**: errors now name the input and are found before the body runs (`` `points`: expected an integer ``,
  `` `name`: 41 bytes, at most 24 ``); items are written and named by id (`"0.0"`); a body field the endpoint does not
  take is 400. A non-string name or message body was a 500 (`len(text)`) and is now a 400. In the sharded chat a bad
  room id is refused at the auth worker instead of one hop later.
- **Static kinds**: the endpoints whose only purpose was a `var` or a function changing kind (dynamic.case `/anyitem`,
  kinds.case `/describe` and `/mixed`) are load errors now, tested in cgen's unit tests.
Compiling the shape of every answer found no wrong answer in the existing programs.

## Round 18 (2026-11-04): an ordered map against Rust's BTreeMap, and what churn does over time
Goal set by Ben: every workload within +20% of Rust's `BTreeMap<u64,u64>`. `bench/btree-rust` and `bench/ordered`
(`ab.py`: alternating builds, best of rounds, because other work on the machine moves any single run by 2x) run the
same eight workloads on the same MINSTD key sequence, present keys 2r, absent keys 2r+1. Windows, clang -O2.
Batched 64-lane lookup has no Mito equivalent and is not reported.

Where the time went, found by switching safety layers off one at a time (in a build used only for measuring):
| 100,000 keys, lookup of a present key | ns |
|---|---|
| skip list as first written | 415 |
| calls to the runtime's helpers made direct, overflow checks by compiler builtins | 380 |
| `h_index` without two divisions, `below()` reading each link and key once | 230 |
| B+tree, binary search in nodes | 207 |
| nodes read once per search (`let node = ...`) | 169 |
| pool liveness checked in line from the layout | 136 |
| array bounds checked in line | 117 |
| node search as a linear scan over a loop counter with constant bounds (no check emitted) | 70 |
| Rust BTreeMap | 72 |
Array bounds checks alone were 30% of a lookup even as a never-taken branch in a helper; a counter whose range is a
compile-time constant needs none. Journaling costs nothing on reads.

Final, best of rounds, ratio to Rust (the B+tree is `lib/btree.mito`, i64 keys; `sorted` is the skip list):
| workload | B+tree 10k | B+tree 1M | skip list 1M |
|---|---|---|---|
| insert new keys | 4.5x | 2.2x | 6.7x |
| lookup, hit | 0.95x | 0.73x | 7.8x |
| lookup, miss | 0.92x | 1.20x | 8.3x |
| lookup, 50% hits / 50% misses | 0.98x | 1.12x | 9.1x |
| increment existing value | 1.05x | 0.83x | 9.5x |
| range scan, up to 100 entries | 1.64x | 0.98x | 12.9x |
| delete existing key | 4.1x | 4.5x | 9.6x |
Reads are inside the target at 1M and at parity at 10k except the 10k range scan. Insert and delete are not: a write
pays for a tagged-value store (now typed for integer columns), an undo-log entry (now the first write to each 64-byte
line saves the line; later writes cost a bit test), and the generic allocator (about 55 ns per `add`, two adds per
insert because each item has a location record).

**Remove is logical; trim is separate.** Removing all 1,000,000 keys left committed memory unchanged at 44,508 KB
(live fell to 3,906 KB): `remove` frees slots, pages stay committed. With a trim policy (`--soft 1`) committed went
28,756, then 9,672, then 5,644 KB over the next three requests; each trim returns part of it. Without `--soft`, never.

**Churn (steady state, 100,000 live keys, one key out and one in per step, each key replaced 40x; `churn.py`):**
- memory is stable: B+tree committed 4,504 KB filled, 4,944 KB after 4,000,000 steps (skip list 11,472 -> 12,408 KB);
  Rust's private bytes did not move (3,916 KB).
- the tree stays the same shape: height 4, leaves 8,876 -> about 8,860, inner nodes 861 -> 1,168 (leaves merge only
  under one parent, inner nodes never merge, so inner nodes creep up and plateau).
- speed does not drift with use. Pinned to one core, the B+tree's step stays 984 -> 1,015 ns and its lookup 116 -> 120
  ns over the 40 rounds, the skip list's step 2,129 -> 2,200 ns and lookup 464 -> 438 ns, and the in-order scan stays
  3.8 ns per item. (An earlier version of this note reported lookups slowing from 72 to 128 ns after 10-20 rounds and
  recovering in a fresh process. That was not memory or layout: this machine's CPU mixes performance and efficiency
  cores, and an unpinned benchmark process was moved to a slower one mid-run. The same loop takes 80 ns on CPUs 0, 10
  and 12-18, 121 ns on CPUs 2-8 and 251 ns on CPU 20; `pin.py` fixes the core, and the drivers use it.)

**How the two sides are timed.** Rust times the bare loop in its own process (`Instant`). Mitochondria is timed by the
server itself: started with `--timing`, every answer carries `X-Handler-Ns`, the handler's time measured by the
high-resolution counter, and the driver reads that. (Timing the Python HTTP round trip instead added 330-700 us per
request with 15 ms stalls in the tail, which inflated insert and delete.) Both processes are pinned to the same core.
Still not the same work: Mito has no direct `find`, so a lookup is `each x from key limit 1` and an equality test; a
delete is a lookup and a remove; every request is atomic (undo log); an insert also allocates the item and its
location record; its arithmetic is overflow-checked.

## Round 19 (2026-11-04): a fair 0.9x attempt
Target from Ben: Mitochondria at 0.9x of Rust's time on every workload. What was made fair first:
- Rust: `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"` (verified from the verbose build; rustc 1.92.0),
  the standard `BTreeMap` and allocator. Mitochondria: clang 23.1.5 `-O3` (now the default of `mitochondria build`; `--cflag -O2`
  selects the old level), whole program in one translation unit, `-fno-strict-aliasing`. Neither uses `-march=native`; Rust built
  with `target-cpu=native` measured the same within noise, so it was not adopted. The tighter Rust profile moved range scan at
  10k 246 -> 213 ns and delete 74 -> 69 ns, nothing else.
- The operations match: `btree` gained `find(key)` and `remove_key(key)`, and the benchmark calls them, so a lookup is one call
  (Rust `get`), an increment a find and a write (`get_mut`), a delete one call (`remove`). Both processes are pinned to the same
  core (CPU 10) and timed inside their own process (`--timing`).
- Rows count as 0.9x only when best-of and median are both at or under it.

Ratios to Rust (Mitochondria / Rust; `btree` = items in their own pool with stable slots, merged; `ordmap` = values inside the
leaves, i64 keys to i64 values, merged on Ben's decision):
| workload | btree 10k | ordmap 10k | btree 1M | ordmap 1M |
|---|---|---|---|---|
| Insert new keys | 2.60x | 1.57x | 1.89x | 1.27x |
| Lookup: hit | 0.83x | 0.82x | 0.81x | 0.89x (median 1.02x) |
| Lookup: miss | 0.80x | 0.80x | 0.60x | 0.57x |
| Lookup: 50% hits, 50% misses | 0.86x | 0.84x | 0.79x | 0.77x |
| Batched lookup, 64 lanes | n/a | n/a | n/a | n/a |
| Increment existing value | 0.93x | 0.89x | 0.80x | 0.86x |
| Range scan, up to 100 entries | 1.79x | 0.81x | 1.09x | 0.45x |
| Delete existing key | 2.05x | 1.37x | 2.02x | 1.68x |
At or under 0.9x on best-of and median: `btree` hit, miss and mixed at both sizes and increment at 1M; `ordmap` hit, miss, mixed, increment
and range scan at 10k, and miss, mixed, increment and range scan at 1M. Not reached by either: insert and delete (and `ordmap` hit at 1M by median).

Why insert and delete stop short: with the undo log switched off (a measuring-only build) `ordmap` insert is 88 ns against
Rust's 71 and delete 74 against 69, and the undo log itself costs 35 ns per insert and 22 ns per delete. So a request that is
atomic (a failed request leaves nothing behind) cannot be within 0.9x on writes with this design; the remaining cost beyond
the undo log is the search (about 25-30 ns), the stores and the leaf splits.

Other findings: SIMD (AVX2 compare of four 64-bit keys, `array.count_below`) is not faster than the early-exit loop for nodes of
about 11 keys (hit 41 ns with the loop, 45 with count_below); it is kept as a language operation and opt-in build flag. A hit at
1M costs more than a miss because the value is a second array in the leaf, a second cache line. `-O3` over `-O2` on `ordmap` at
10k: range scan 1.10x -> 0.81x, increment 0.92x -> 0.88x, insert 1.71x -> 1.56x.

## Round 20 (2026-11-04): LangArena tests against the Rust reference
Seven of LangArena's 50 tests are ported (bench/langarena, with the parameters and licence vendored and a README with the method, the
audit verdicts and the triage of all 50), measured against LangArena's own Rust implementation (its max-perf profile, overflow checks off) on the
same pinned core, alternating rounds, the checksum verified on both sides in every measured run. Mitochondria / Rust, best of 3: Etc::Sieve
4.93x, CLBG::Fannkuchredux 2.84x, Sort::Quick 1.61x, Sort::Merge 2.54x, CLBG::Spectralnorm 0.70x, CLBG::Mandelbrot 1.00x, Matmul::Single 1.25x
(closest-possible); headline geometric mean of the six like-for-like and rewritten-to-match ports 1.85x. Rust built with overflow checks on is an
extra column (it moves Sieve 1.35 -> 1.80 s, so checked arithmetic alone does not explain the gap): loops of many small array stores are the
slow case, because every store is range-checked, committed and undo-logged. Seven more tests are portable with the language as it is;
27 need a language feature (bit operators, string bytes, sqrt/exp, a pool reset) and 9 are not comparable (threads, library).

## Round 21 (2026-11-04): making stores cheap, language additions, and the Crystal original
LangArena (bench/langarena) now compares Mitochondria with the Rust reference and the original Crystal implementation (Crystal 1.22.2 installed from its release, built with
`--release`), same core, alternating rounds, all checksums verified on all three sides. Mitochondria / Rust, best of 3, from the first run to now: Sieve 4.93x -> 1.70x, Fannkuchredux
2.84x -> 1.47x, Sort::Quick 1.61x -> 1.22x, Sort::Merge 2.54x -> 1.38x, Spectralnorm 0.70x, Mandelbrot 0.98x, Matmul::Single 1.23x; geometric mean of the six like-for-like and
rewritten-to-match ports 1.19x of Rust, 1.02x of Crystal; faster than Crystal on both sorts (0.87x, 0.83x).
What did it (each merged): plain-state stores skip the page-commit check (plain state is committed at start and never trimmed); a bulk array.fill; stores of aligned scalars are inline with
the undo-log and commit tests against per-request locals (jons, dirty map and commit flags are fixed for a request); value-range analysis for names that never change (sound, tested, no
measurable effect on these ports). Attribution on the sieve per run: 24.7 ms as built, 19.6 without the undo log, 13.3 without the commit check, 7.8 without both, Rust 5.0.
Language added: bit operators and hex literals, sqrt exp floor abs, string byte reads, coll.clear(), array.fill/move/count_below. They unblock Binarytrees, BF, NeuralNet, Nbody,
TextRaytracer, CRC32 and SHA256 for porting (not yet ported).

## Round 22 (2026-11-04): 19 LangArena tests ported
Added since round 21 (Mitochondria / Rust, best of 3; all verified against LangArena's checksums on all three sides): Hash::CRC32 0.83x, Hash::SHA256 1.53x, CLBG::Nbody 1.36x (closest-possible),
Etc::GameOfLife 0.83x, Graph::BFS 1.19x, Graph::DFS 1.33x, Graph::AStar 0.74x, Maze::Generator 0.70x, Maze::BFS 0.60x, Maze::AStar 0.88x, Binarytrees::Obj 1.09x, Binarytrees::Arena 3.98x.
Against the original Crystal: 0.87x, 1.55x, 1.18x, 0.87x, 0.81x, 0.90x, 0.44x, 0.36x, 0.30x, 0.24x, 2.14x, 4.02x. Geometric mean of the 17 like-for-like and rewritten-to-match ports: 1.11x of Rust,
0.87x of Crystal. The graph and maze ports use the reference's data shapes (pools of objects with their own neighbour lists or arrays), with array capacities set by simulating the reference's algorithms
(which also reproduced the expected checksums exactly). Slowest: Binarytrees::Arena, bound by pool add (about 28 ns per node against a Vec push of a few).

## Round 23: all 41 LangArena ports (2026-11-04)

41 of LangArena's 50 tests are ported (see bench/langarena/README.md for the table, audit verdicts and the 9 that are not comparable). One run of all of them, 3 rounds, pinned to one core, in-server timing:
geometric mean 1.11x of Rust over the 28 like-for-like and rewritten-to-match ports (0.88x of Crystal), 1.13x over all 41. Slowest: Base64 (scalar loop against Rust's SIMD crate, 15x and 9x), Binarytrees::Arena (3.8x, pool add), Hash::SHA256 (1.7x), Etc::Sieve (1.8x), Compress::HuffDecode (2.1x);
fastest: Compress::LZWEncode (0.18x, a different dictionary structure than the reference), Calculator::Ast (0.35x, bump nodes), Etc::CacheSimulation (0.59x).

A mistake found on the way: a first version of the per-iteration text-area release called into the runtime on every iteration of every loop and made the store-heavy loops 8-14% slower (Sieve 2.26 s to 2.48 s, SHA256 1.01 s to 1.16 s); it now emits the call only for loop bodies whose code can build a string, and the timings are back (A/B against the earlier compiler on the same core: Sieve 2.27 / 2.29, SHA256 1.01 / 1.00).

## Round 24: fewer per-store tests (2026-11-05)

Pool add fast path and bulk zeroing in clear (Binarytrees::Arena 3.76x to about 2x of Rust), array.move saving only its destination, no commit test on stores to live items, and loop versioning (a guard before a loop proves its indexes in range and saves the stored ranges at once). Sieve 2.33 to 1.77 s, SHA256 1.02 to 0.86 s, Sort::Merge 1.93 to 1.74 s (A/B on one core). Full LangArena run after all of it: 1.06x of Rust over the 27 like-for-like and rewritten-to-match ports (0.81x of Crystal), 1.04x over all 41. A measuring mistake on the way: a crashed script left a server on its port and later comparisons were silently served by that stale build; bench/langarena/run.py now refuses to measure when its port is already taken.

## Round 25: best case without the safety tests (2026-11-05)

What the tests cost, measured: the same 41 LangArena ports built three ways, Mitochondria only, 3 rounds each, best of rounds, pinned core (`run.py --mito-only --cflags ...`):
as built; with `-DMITO_MEASURE_NO_RANGE -DMITO_MEASURE_NO_COMMIT` (no index range tests, all state committed at start so no commit tests); and with `-DMITO_MEASURE_NO_JOURNAL` as well
(no undo log: a failed request is then not rolled back). Integer overflow checks stay on in all three. These are measuring builds only: they break the language's guarantees. The Rust column is the
reference from round 24's full run (same day, same core).

| Workload | as built s | no range/commit tests s | also no undo log s | Rust s | as built / Rust | no tests / Rust | no tests, no undo / Rust | audit |
|---|---|---|---|---|---|---|---|---|
| Compress::LZWDecode | 0.203 | 0.172 | 0.081 | 1.120 | 0.18x | 0.15x | 0.07x | closest-possible |
| Compress::LZWEncode | 0.094 | 0.094 | 0.093 | 0.520 | 0.18x | 0.18x | 0.18x | closest-possible |
| Calculator::Ast | 0.224 | 0.213 | 0.160 | 0.680 | 0.33x | 0.31x | 0.24x | closest-possible |
| Graph::AStar | 0.893 | 0.709 | 0.514 | 1.288 | 0.69x | 0.55x | 0.40x | rewritten-to-match |
| Etc::GameOfLife | 1.560 | 1.054 | 0.868 | 1.901 | 0.82x | 0.55x | 0.46x | like-for-like |
| Etc::CacheSimulation | 0.459 | 0.454 | 0.401 | 0.837 | 0.55x | 0.54x | 0.48x | rewritten-to-match |
| Maze::BFS | 0.459 | 0.399 | 0.369 | 0.760 | 0.60x | 0.53x | 0.49x | rewritten-to-match |
| Binarytrees::Obj | 3.737 | 2.726 | 1.575 | 2.892 | 1.29x | 0.94x | 0.54x | like-for-like |
| Calculator::Interpreter | 0.536 | 0.522 | 0.493 | 0.864 | 0.62x | 0.60x | 0.57x | closest-possible |
| Maze::Generator | 0.535 | 0.366 | 0.422 | 0.726 | 0.74x | 0.50x | 0.58x | rewritten-to-match |
| Etc::Words | 0.583 | 0.566 | 0.482 | 0.783 | 0.74x | 0.72x | 0.62x | closest-possible |
| Template::Parse | 0.682 | 0.627 | 0.567 | 0.833 | 0.82x | 0.75x | 0.68x | closest-possible |
| Etc::NeuralNet | 0.873 | 0.886 | 0.589 | 0.859 | 1.02x | 1.03x | 0.69x | closest-possible |
| CLBG::Spectralnorm | 0.752 | 0.751 | 0.769 | 1.099 | 0.68x | 0.68x | 0.70x | rewritten-to-match |
| Maze::AStar | 0.952 | 0.853 | 0.777 | 1.102 | 0.86x | 0.77x | 0.71x | rewritten-to-match |
| Compress::ArithDecode | 0.983 | 0.934 | 0.903 | 1.251 | 0.79x | 0.75x | 0.72x | rewritten-to-match |
| CSV::Parse | 0.582 | 0.493 | 0.470 | 0.626 | 0.93x | 0.79x | 0.75x | closest-possible |
| Compress::BWTDecode | 0.873 | 0.857 | 0.845 | 1.082 | 0.81x | 0.79x | 0.78x | rewritten-to-match |
| Distance::Jaro | 0.953 | 0.899 | 0.878 | 1.076 | 0.89x | 0.84x | 0.82x | rewritten-to-match |
| Hash::CRC32 | 0.805 | 0.805 | 0.682 | 0.825 | 0.98x | 0.98x | 0.83x | like-for-like |
| Hash::SHA256 | 0.877 | 0.933 | 0.637 | 0.663 | 1.32x | 1.41x | 0.96x | like-for-like |
| CLBG::Mandelbrot | 0.701 | 0.689 | 0.685 | 0.709 | 0.99x | 0.97x | 0.97x | rewritten-to-match |
| Sort::Quick | 1.408 | 1.244 | 1.130 | 1.119 | 1.26x | 1.11x | 1.01x | rewritten-to-match |
| Matmul::Single | 3.277 | 3.295 | 3.331 | 3.291 | 1.00x | 1.00x | 1.01x | closest-possible |
| Sort::Merge | 1.726 | 1.596 | 1.336 | 1.300 | 1.33x | 1.23x | 1.03x | rewritten-to-match |
| Distance::NGram | 0.425 | 0.405 | 0.354 | 0.344 | 1.24x | 1.18x | 1.03x | rewritten-to-match |
| Compress::HuffEncode | 0.746 | 0.623 | 0.625 | 0.594 | 1.26x | 1.05x | 1.05x | rewritten-to-match |
| Compress::ArithEncode | 1.046 | 1.036 | 0.943 | 0.896 | 1.17x | 1.16x | 1.05x | like-for-like |
| Graph::BFS | 0.990 | 0.926 | 0.880 | 0.825 | 1.20x | 1.12x | 1.07x | rewritten-to-match |
| CLBG::Fannkuchredux | 1.361 | 1.148 | 0.978 | 0.912 | 1.49x | 1.26x | 1.07x | like-for-like |
| Compress::BWTEncode | 0.584 | 0.521 | 0.480 | 0.443 | 1.32x | 1.18x | 1.08x | closest-possible |
| Binarytrees::Arena | 1.128 | 0.969 | 0.594 | 0.543 | 2.08x | 1.78x | 1.09x | like-for-like |
| Compress::HuffDecode | 1.041 | 0.758 | 0.685 | 0.614 | 1.70x | 1.23x | 1.12x | rewritten-to-match |
| CLBG::Nbody | 0.926 | 0.934 | 0.822 | 0.712 | 1.30x | 1.31x | 1.15x | closest-possible |
| Graph::DFS | 0.952 | 0.930 | 0.846 | 0.713 | 1.34x | 1.30x | 1.19x | rewritten-to-match |
| Etc::TextRaytracer | 0.735 | 0.750 | 0.743 | 0.618 | 1.19x | 1.21x | 1.20x | rewritten-to-match |
| BF::Recursion | 1.981 | 1.881 | 1.734 | 1.330 | 1.49x | 1.41x | 1.30x | closest-possible |
| Etc::Sieve | 1.735 | 1.777 | 1.741 | 1.312 | 1.32x | 1.35x | 1.33x | rewritten-to-match |
| BF::Array | 1.393 | 1.459 | 1.437 | 0.918 | 1.52x | 1.59x | 1.57x | rewritten-to-match |
| Base64::Decode | 1.642 | 1.279 | 0.761 | 0.184 | 8.92x | 6.95x | 4.14x | closest-possible |
| Base64::Encode | 2.509 | 2.242 | 0.782 | 0.165 | 15.21x | 13.59x | 4.74x | closest-possible |

geomean vs Rust, like-for-like + rewritten (27): as built 1.06x, no range/commit tests 0.96x, also no undo log 0.85x
geomean vs Rust, all 41: 1.05x, 0.95x, 0.81x

Reading it: the range and commit tests together cost about 10% overall (1.06x to 0.96x of Rust), the undo log another 12% (to 0.85x). They matter most where items are written a lot
(Binarytrees::Obj 1.29x to 0.54x, Binarytrees::Arena 2.08x to 1.09x, Base64 4x) and in loops the versioning does not cover (GameOfLife, Huffman decode, Fannkuch). The sieve and BF
do not move: their loops are versioned already, and what is left against Rust is overflow checking (Rust with overflow checks takes 1.79 s on the sieve, Mitochondria 1.74 s).
Run-to-run noise is a few percent (Binarytrees::Obj as built measured 3.2 s in round 24 and 3.7 s here).

## Round 26: HTTP keep-alive (2026-11-05)

The HTTP front end now keeps connections open (HTTP/1.1 unless `Connection: close`; HTTP/1.0 only with `Connection: keep-alive`), answers back-to-back requests in order,
and frames bodies by Content-Length exactly (bytes after it are the next request; a malformed or cut-short request closes the connection). `--conns` connection threads (16)
take connections that have a request waiting; one poller thread (WSAPoll) accepts and watches idle connections, closing them after 10 s; a thread waits 1 ms for a client's next
request before handing the connection back. One program's handler runs under a lock, one request at a time, as before.

A trivial endpoint (`GET /n`, handler about 200 ns), Windows loopback, unpinned:

| client | before (new connection each request) | after |
|---|---|---|
| Python http.client, one client, new connection each request | 600-1,100 req/s | 600-900 req/s |
| Python http.client, one client, kept connection | (not possible) | 6,500-6,700 req/s |
| raw socket, kept, one request at a time | (not possible) | 9,500-9,800 req/s |
| raw socket, kept, 32 requests pipelined | (not possible) | 51,000-57,000 req/s |
| 8 Python client threads, kept connections | | 48,000-57,000 req/s (the client is the limit) |

With 40 connections that never sent anything and 40 idle kept ones open (more than the 16 threads), a new client was answered in 0.7 ms.

## Round 27: `unlogged` scratch state (2026-11-05)

`unlogged` on a plain field (`scratch: u8[4096] unlogged`) or a whole collection (`pool<Node, N> unlogged`, with what its items hold) keeps it out of the undo log: a failed request
leaves what it wrote there, while logged data rolls back as before (removals still happen only when a request succeeds). The runtime marks those lines saved once at start and never
journals them, so every save path skips them; generated stores to them have no undo-log test. Unlogged plain fields get their own 64-byte lines (the log saves and restores whole lines).

30 LangArena ports now declare the state they rebuild every run (the reference's locals: scratch arrays, output buffers, per-run arenas and trees) unlogged; their audit files say which.
Same compiler, logged sources against unlogged ones, best of 2 alternating rounds:

| Workload | logged s | unlogged scratch s | change |
|---|---|---|---|
| Base64::Encode | 2.531 | 1.188 | -53% |
| Base64::Decode | 1.625 | 1.059 | -35% |
| Hash::SHA256 | 0.938 | 0.637 | -32% |
| CLBG::Fannkuchredux | 1.332 | 0.916 | -31% |
| Graph::AStar | 0.894 | 0.640 | -28% |
| Compress::HuffEncode | 0.755 | 0.600 | -21% |
| Compress::HuffDecode | 1.054 | 0.900 | -15% |
| Template::Parse, BF::Recursion | | | -11% |
| Calculator::Ast, Graph::DFS, Sort::Merge, Binarytrees::Arena, Sort::Quick, Distance::Jaro | | | -7% to -9% |
| Compress::BWTEncode, Etc::Sieve, BF::Array, LZWEncode, Maze::AStar | | | -4% to -6% |
| the other 10 | | | -3% to +3% |

Geometric mean over the 31 measured: -10.9%. Maze::Generator was measured too and got 20-25% slower with its two stack arrays unlogged; with the undo log compiled out entirely it is also slower
(0.63-0.65 s against 0.49 s logged, and Round 25 showed the same: 0.37 s without range and commit tests, 0.42 s with the log also off). Something about journaling those lines makes this
port faster; not explained yet, so Maze::Generator keeps its arrays logged.

Full run after all of it (3 rounds, 41 ports, 2026-11-05 evening): 0.97x of Rust over the 27 like-for-like and rewritten-to-match ports (0.74x of Crystal), 0.94x over all 41 (morning: 1.06x and 1.04x).

## Round 28: no ref<>, and T? (2026-11-05)

With `ref<>` removed, Binarytrees::Obj names a node's children by slot + 1 and follows them by a checked index instead of a generation-checked reference: 2.78 s to 2.04 s (0.68x of Rust; audit now rewritten-to-match).

## Round 29: general savings, every port (2026-11-05)

Small changes aimed at all ports rather than one outlier, measured as a whole-suite A/B against the previous compiler (3 rounds each, one pinned core):

- the undo log records a 4 KB page that is still all zeros as one "zeros" entry instead of copying it (a request filling fresh state, as every `prepare` does);
- a bounds test of a plain C variable stays good into later statements until a line writes that variable (it was dropped at every statement);
- a value loaded from a narrow field (`u8`, `i32`, ...) and a `let` holding it keep that range, so arithmetic and stores on them lose overflow and fit tests;
- a function enters the loop-selection mark only when one of its loops collects items (it was any loop);
- `int(x)` of a float is truncated in line (it was a boxed call to the generic builtin; 16 ports use it in the LangArena random-number helper);
- clang gets `-fno-math-errno` (sqrt is one instruction) and `-falign-loops=32`.

The last flag matters more than it looks: before it, Compress::HuffEncode and HuffDecode were 24% and 11% slower with strictly leaner C, and the same
inner loop compiled to the same instructions either way; only where clang placed it differed. Aligning loop heads removed that swing.

| | before | after |
|---|---:|---|
| time, geometric mean of Mito/Rust over all 41 | 0.928x | 0.909x |
| peak memory, geometric mean of Mito/Rust over all 41 | 1.13x | 0.94x |

Largest time gains: BF::Recursion -17%, BF::Array -14%, Etc::NeuralNet and Etc::GameOfLife -15%, Maze::Generator -10%; geometric mean -2.8% against the
previous compiler. Memory: Maze ports -45% to -49%, Template::Parse, GameOfLife, BWTDecode -42% to -45%, Graph ports -37% (the undo log no longer copies
the zero pages `prepare` fills).
