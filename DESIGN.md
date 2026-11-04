# Mitochondria design

Mitochondria is an experimental language in which all program state is a declared tree of fixed-size values with declared
maxima. The whole tree is reserved up front. The runtime knows, from the structure itself, which bytes are live, which are committed
(backed by memory) and which are only reserved. It descends from StasisLang (one visible state model, ticks, bounded collections,
data-oriented layout) and borrows from next-lang (allocate at startup, explicit over implicit, compile time first).

This document is the current design. Measurements behind it are in [docs/measurements.md](docs/measurements.md) and
[docs/chat-api-findings.md](docs/chat-api-findings.md). Section "Status" says what the prototype implements today.

## Principles
1. **Everything has a declared maximum.** Exceeding one is a typed failure, never a crash and never an out-of-memory kill.
2. **One reservation, commit on use.** Reserving address space is free; memory is committed only where bytes are live.
3. **Liveness is structural.** Lengths and live bits say what is in use. Nothing is traced or guessed.
4. **No hidden work.** No garbage collector, no hidden allocation, no implicit copies of collections.
5. **Few concepts.** Each new concept must absorb an existing need, not add a parallel way to do the same thing.

## Terms
- **Reserved:** address space claimed for the declared maximum. Costs no memory.
- **Committed:** pages the operating system has promised to back with memory (Windows "commit charge").
- **Resident:** committed pages currently in RAM (the "working set").
- **Live:** bytes the program is using right now, known from lengths and live bits.
- **Columns:** a collection of structs stored one array per field (structure of arrays) rather than one record after another.

## The nine concepts

### 1. State tree
`struct` declarations and `global` roots form one tree. Its layout (every offset and size) is computed when the program is
built. The program refuses to start if the declared total exceeds the configured budget.

```
struct Room { name: string<48>; created: i64; messages: list<Msg, 1000> drop_oldest; }
global rooms: pool<Room, 64>;
```

### 2. Bounded values
Scalars (`i8`..`i64`, `u8`..`u64`, `f32`, `f64`, `bool`), `string<N>` (at most N bytes, length stored separately), and
`T[N]` (exactly N elements, always present). Code can read every declared maximum (`capacity(x)`, `maxlen(s)`), so limits are
never repeated by hand. A value that does not fit is a `TooLong` failure.

### 3. `list<T, N>`: ordered, with a declared full policy
Items in insertion order. Operations: append, remove last, clear, read by position, iterate forward or in `reverse`.
When full, the declared policy applies:
- `fail` (default): the append fails with `Full`.
- `drop_oldest`: the append overwrites the oldest item (a ring buffer).

Positions shift as the oldest items are dropped, so a list item is identified by a key it carries (for example a sequence
number), never by a stored reference. There is no removal from the middle; mark the item instead.

### 4. `pool<T, N>`: unordered, stable slots
Items live in slots that never move. A live bit per slot says which are in use. New items take the lowest free slot, and a
high-water mark bounds scans and trimming. When full, `alloc` fails with `Full` (there is no natural victim to drop).
`pool<T, N> checked` adds a generation per slot, so a reference to a freed and reused slot is detected (`Stale`).

Collections of structs (in lists and pools) are always stored as columns. A field scan reads only that field's array.

**Choosing:** use a list for ordered things you append to and read in order or from the end (messages, logs, mailboxes,
recent events). Use a pool for things other data refers to and that come and go in any order (sessions, users, rooms, units).
Use `checked` when a stale reference would be costly, for example a handle that left the program.

### 5. `ref<pool>`: typed references
A field or local of type `ref<sessions>` refers to a slot of that pool: 4 bytes, or 8 with a generation if the pool is `checked`.
Reading through a reference checks the live bit (and the generation). Reading through a dead reference fails with `NotLive`
or `Stale`; `or(a, b)` gives `b` instead when `a` is null or a dead-reference read:

```
"author": or(m.author.name, "[deleted]")
```

References point at top-level pools only (for now). Lists are never referenced; see concept 3.

### 6. Nesting
A list or pool can be a field of a row. Its maximum is per parent: `Room.messages: list<Msg, 1000>` reserves 1000 messages for each
of the 64 rooms. Reservation is free; only live messages commit memory. Each parent's items are stored contiguously in each column,
so one room's messages are adjacent in memory and a partly used room leaves whole pages unused.
Nesting replaces an index for "items grouped by parent": a room's newest 50 messages are the last 50 of its list, with no scan.

The declared worst case multiplies: 64 rooms x 1000 messages. That product is what the budget check counts.

### 7. Epochs
State lives as long as the program. Each request (or tick) gets a scratch region with a declared size that is reset when the request
ends: locals, strings built while handling, and the response. Temporaries therefore need no per-value maximum, only the epoch total.
Running out of scratch is a `Full` failure for that request.

### 8. Handlers and functions
- `fn name(params) -> type { ... }` plain functions. Parameters are values or references. No closures, no captures.
- `let` binds a value; `var` binds a mutable local.
- `endpoint METHOD /path/{param} { ... }` is a handler: an entry point that reads the request and ends in `respond` or `fail`.
- State (the global tree) is read and written directly, as in Stasis.
- **Every handler is atomic.** If it fails, its writes are undone, its allocations are released, and its frees never happen
  (a slot freed during a request becomes reusable only after the request succeeds). An item dropped by `drop_oldest` is restored.

Iteration: `each x in collection where cond order by key [desc] limit n { ... }`, and `each x in list reverse limit n` for the
newest items of a list. The same clause works inside a JSON array in `respond`.

### 9. Commit and trim policy
Pages are committed when first written. `trim` returns pages that hold no live byte. Trimming is scoped (one global or one field,
costing only that part) and driven by pressure, not by schedule:

| Committed memory | Behavior |
|---|---|
| below the soft mark | never trim; reuse stays a plain write |
| above the soft mark | trim the region with the most dead bytes, at an epoch boundary, until under the mark |
| above the hard mark | trim everything, drop all slack, refuse new work with `Full` |
| quiet after a burst | slowly release slack so a one-off spike does not pin memory |

Marks are fractions of the declared budget and also consider machine-level free memory.

## Failures
| Failure | Meaning | Default HTTP status |
|---|---|---|
| `Full` | a collection, pool or epoch is at its maximum | 503 |
| `TooLong` | a value exceeds its declared maximum | 413 |
| `NotLive` | position or slot is not in use | 404 |
| `Stale` | checked reference to a reused slot | 401 when it is a session, else 404 |
| `fail CODE "msg"` | handler-defined | as given |

A failure ends the handler and rolls it back (concept 8).

## Example: chat hot state in this design
```
struct Session { name: string<32>; secret: u64; window: i64; sent: i32; }
struct Msg     { seq: i64; at: i64; author: ref<sessions>; body: string<2000>; }
struct Room    { name: string<48>; messages: list<Msg, 1000> drop_oldest; }
global sessions: pool<Session, 2000> checked;
global rooms: pool<Room, 64>;

fn me() -> ref<sessions> { return auth(sessions); }

endpoint GET /rooms/{room}/messages {
    let who = me();
    let room = rooms[int(param("room"))];
    respond 200 { "messages": [each m in room.messages reverse limit 50 {
        "seq": m.seq, "author": or(m.author.name, "[deleted]"), "body": m.body }] };
}
```
Compared with the prototype chat API: no scan to find a room's messages, no sort to evict the oldest, no copied author name,
and auth is written once.

## Fit
Good: games and simulations, servers' hot state (sessions, presence, queues, rate limits), embedded and real-time work, batch
pipelines, phased tools, anything where peak memory must be known before launch. Poor: large irregular object graphs, unbounded or
heavy-tailed sizes (editors, browsers, crawlers), dynamic shapes, exploratory scripting. Durable data belongs in a database; this
is the layer above it. Details and the comparison with arenas, garbage collection and Erlang: docs/measurements.md.

## Deferred, with known limits
- **Long uneven text.** `string<N>` reserves N bytes per slot. Measured: 2000-byte chat bodies averaging ~120 bytes left 39.5 MB committed
  for 3.1 MB live, and trim could release none of it. Accepted for now; nesting improves locality but not this.
- **Workers and mailboxes.** A later layer: a mailbox is a `list` with a full policy (adding `block` and `drop_newest`), each worker owns
  a region and trims it when its mailbox empties. Supervision, scheduling and cross-thread commit are undesigned.
- **Compilation.** Endpoints are interpreted. The interpreter is ~470x a native loop on a 20,000-row scan. A compiler would bind columns
  once and turn `each ... where` into a column loop.
- **Keyed indexes** for lookups that are not by parent. Not planned until nesting proves insufficient.
- **Debug-only orphan report** (rows nothing refers to) and **FIFO slot reuse** for pools.

## Status: prototype vs this design
| Concept | Prototype today |
|---|---|
| 1 State tree, budget check | built |
| 2 Bounded values | built; `capacity` only for collections, no `maxlen` |
| 3 `list` | built with record storage, `fail` only; no `drop_oldest`, no `reverse`, no columns |
| 4 `pool`, `checked` | built, column storage |
| — `table` | built; to be removed |
| 5 `ref<pool>` | not built (slots stored as `i32`) |
| 6 Nesting | only `list<scalar>` and strings inside table/pool rows |
| 7 Epochs | response buffer only; interpreter temporaries use the Rust heap |
| 8 Handlers | endpoints, `let`, `each`, atomic writes and allocations; no `fn`, no `var`, frees not deferred |
| 9 Trim | manual and scoped trim built; pressure policy not built |

## Plan, in order
1. Lists stored as columns with `drop_oldest` and `reverse`; remove `table` (port its tests to pool and list).
2. Nesting: lists and pools of structs inside pool rows, per-parent contiguous columns, budget counts the product.
3. `ref<pool>` fields and locals, `or()` over dead references.
4. `fn` and `var`; deferred frees so handlers are fully atomic.
5. Port the chat example to this design and re-run `bench` against the numbers in docs/chat-api-findings.md.
6. Request epoch for interpreter temporaries; then the pressure-driven trim policy.
