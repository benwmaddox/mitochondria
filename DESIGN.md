# Mitochondria design

All of a program's state is a declared tree of fixed-size values with declared maxima, reserved up front. The runtime
knows from the structure which bytes are live, which are committed and which are only reserved. It descends from
StasisLang (one visible state, bounded collections, data-oriented layout) and next-lang (allocate at startup, explicit
over implicit, compile time first). Measurements behind it: [docs/measurements.md](docs/measurements.md).

## Principles
1. **Everything has a declared maximum.** Exceeding one is a typed failure, never a crash or an out-of-memory kill.
2. **One reservation, memory on use.** Reserving address space is free; a page takes memory only once written. With memory pressure on,
   pages are committed only where bytes are live; without it, state up to 1 GiB is committed whole at start (see Commit and trim).
3. **Liveness is structural.** Lengths and live bits say what is in use; nothing is traced or guessed.
4. **No hidden work.** No garbage collector, no hidden allocation, no implicit copies of collections.
5. **Every kind is known when compiling.** Values, request inputs and answers are all typed.
6. **Few concepts.** A new one must absorb an existing need.

**Terms.** *Reserved*: address space set aside; costs nothing. *Committed*: pages the OS has promised to back with
memory. *Live*: bytes in use now. *Columns*: a collection of structs stored one array per field.

## Memory

**The state.** A program has exactly one `state`, a struct holding everything; every path starts at its name. The
layout (every offset and size) is fixed when the program is built, and the program refuses to start over `--budget`.

```
struct Room { name: string<48>; messages: list<Msg, 1000> drop_oldest; }
struct Chat { sessions: pool<Session, 2000> checked; rooms: pool<Room, 64> checked; }
state chat: Chat;
```

**Values.** Scalars (`i8`..`i64`, `u8`..`u64`, `f32`, `f64`, `bool`), `string<N>` (at most N bytes) and `T[N]`
(exactly N). Code reads maxima rather than repeating them (`x.capacity()`, `maxlen(s)`). An integer field may declare a range,
`limit: i64 in 0..3000000` (both ends included, 0 inside, since state starts zeroed): every write is tested (400 outside), and reads let the
compiler drop overflow and index tests on what is computed from them. `unlogged` after a plain field or a collection keeps it out of the
undo log: a failed request leaves what it wrote there (for scratch state rebuilt by every request); removals still wait for success.

**`list<T, N>`** keeps insertion order: add, read by position, walk forwards or in `reverse`. When full it fails
(`Full`), or with `drop_oldest` overwrites the oldest (a ring). Positions shift as items drop, so a list item is
named by a key it carries, never referenced. Use it for messages, logs, recent events.

**`pool<T, N>`** keeps items in slots that never move, with a live bit each; new items take the lowest free slot. When
full, `add` fails. `checked` adds a generation per slot, so an id from a request or a session token that names a reused
slot is detected (`Stale`). Use it
for things that are referred to and come and go: sessions, users, rooms.

**Naming another item.** An item names another by its slot in an integer field (`who: i64`, set from `slot(p)`), read
back with `pool[slot]`, which checks the slot is live (`or(pool[slot].name, "[gone]")` for a fallback). A slot stored
in state carries no generation, so a reused slot is not detected there; a generation-checked id held in state is
planned (`ref<path>` did this and was removed; a different design is wanted).

**`T` and `T?`.** A plain item value (`T`) is always a live item: removals take effect only when the request succeeds, so
nothing a request holds turns into `null` under it. A value that may be `null` has type `T?`: what `try_add()` gives, and
what a function declared `fn find(name) -> Node?` returns (a function that can return `null` must say so). The compiler
rejects reading, writing, removing or taking `slot()` of a `T?` until it is checked: inside `if exists(x) { ... }` (or
`if x != null`), in the `else` of `if x == null`, and after a guard that leaves (`if !exists(x) { fail 404 "..."; }`), `x`
is a plain `T`; `or(x.name, "-")` and `exists(x)` take a `T?` as it is. So no item access can fail at run time for being `null`.

**Nesting.** A collection can be a field of an item, with a maximum per parent: `Room.messages: list<Msg, 1000>`
reserves 1000 per room. Each parent's items sit together, so a room's newest 50 messages are the last 50 of its list,
with no index. A parent that does not exist costs nothing; removing it empties what it holds.

**Access is a lookup.** An id is a slot; every column's place is known, so reading is address arithmetic.

**Epochs.** Each request gets a text area (every string it reads or builds is copied there) and a response area, both
reset when it ends. Running out fails that request (507, or 413 if the request itself does not fit). There is no heap
allocator anywhere.

**Commit and trim.** Without `--soft`/`--hard`, state up to 1 GiB (`COMMIT_ALL_MAX`) is committed whole at start: committing reserves
the system's commit charge, not physical memory (a page is backed when first written), and nothing is ever trimmed, so no store, add or
helper tests or commits pages. Larger state, or any program with pressure marks, commits pages when first written. A trim returns pages
holding no live byte, one collection at a time, driven by pressure:

| Committed memory | Behaviour |
|---|---|
| below `--soft` | never trim |
| above `--soft` | after each request, trim the collection with the most committed memory |
| above `--hard` | trim everything; refuse writes (503) until below; reads continue |

A collection whose trim released nothing is skipped until 256 of its items have gone since.

## Code

**Functions.** `fn name(params) -> type { ... }`; `let` and `var`; `+=` and friends. No closures. A `var` keeps the
kind of its first value and a function returns one kind; a program needing a kind known only at run time does not load.

**Endpoints** are functions with an attribute and typed inputs:

```
@get("/rooms/{room}/messages")
fn messages(who: Session from header("X-Session"), room: Room, limit: i64 in 0..100 = 50)
    -> Response<{ messages: [{ seq: i64, author: string, body: string }] }> {
    return ok({ messages: [foreach m in room.messages reverse limit limit {
        seq: m.seq, author: or(m.author.name, "[deleted]"), body: m.body }] });
}
```

- Inputs come from the path (`{room}`: an id gives the item), the JSON body (POST, PUT, PATCH) or the query (GET,
  DELETE), a header (`from header("X-Session")`: a session token gives the session), or another worker
  (`forwarded`). They are checked before the body runs: absent, wrong kind or out of range 400, too long 413, no such
  item 404, a body field not taken 400.
- `Response<T>` declares the answer's JSON shape, checked against every answer when compiling. `ok` 200, `created`
  201, `no_content` 204, `status(code, x)`; `fail CODE "message"` ends a request with an error from any function. An
  item is written as its id (`"slot.generation"` for a checked pool).
- Collections are used through their functions: `add` (or `add { field: value }`), `try_add`, `remove`, `count`,
  `capacity`.

**Every request is atomic.** The runtime records the old bytes of each write; a failed request writes them back.
`remove` takes effect when the request succeeds, so a removed slot cannot be reused in the same request.

**Loops.** `foreach x in c where cond limit n` visits the live items of a collection (`foreach x, i in c`: `i` counts the items
visited), `reverse` for newest first, `foreach i in lo..hi` both ends included (with `while` to stop early); the same clauses build
JSON arrays (`[foreach x in c expr]`). Order is kept in the data, never computed when read. `for i = 0; i < n; i += 1 { }` and
`while cond { }` loop as long as needed, `break` and `continue` work in every loop, and parentheses around a head are allowed but
never required. A `foreach` is bounded by a capacity or a range; a `while` or `for` counts its rounds against the request's step
budget (100 million, `--loop-budget N`), past which the request fails with "loop ran too long". A counted `for` (`i < n` or
`i <= n`, step `i += 1`, neither changed in the body) compiles as a range loop.

**Text.** Built text lives in declared fields. `s += x` appends to a string field in place (only the new bytes and the length
are written and saved to the undo log; past the capacity the request fails, 413), and `s = "n=" + str(n)` is written as the same
appends. A `let` that names a text field is a window onto it: nothing is copied, and a later change to the field shows through.
Text for the work of one request goes in `request work: Work;`, a struct of plain fields beside the state, zeroed before each
request, kept out of the undo log and never kept. Literals are stored once in the program and copied with memcpy where used. `ascii<N>` text holds bytes 0..127 and can be written by index
(`s[i] = b`); `utf8<N>` (also written `string<N>`) holds any bytes and changes only by assigning or appending.

**Enums and match.** `enum Op { Plus, Minus }` (or `enum Cmd: u8 { Plus = 43, Minus = 45 }`) names integer values; a field of an
enum type is stored in the smallest integer that holds them and a write outside their range fails (400), and an answer field
declared with an enum type is written as the value's name. `Op.Plus` is its integer in code. `match x { Op.Plus => { } Op.Minus,
Op.Out => { } 0..9 => { } else => { } }` tests constants, lists and ranges in order, without fall-through; on an enum every value
must be handled or an `else` given, and no value may be covered twice. It runs as an `if` chain (clang turns it into a jump table).

## Modules
A module is a `.mito` file in `lib/`: parameters, structs, a `state { ... }` block (what an instance holds) and
functions. A use is a field of the state, `scores: sorted<Score, 10000, points>;`, expanded when compiling into
ordinary state and functions (`game.scores.add(...)`), one copy per use. Outside its functions, an instance's state
cannot be written, its items not added or removed directly, and its key not assigned. A module with `iter_first`,
`iter_next` and `iter_item` can be walked by `each`.

Standard library: `sorted<T, N, key>` (a skip list, items in order of a field: `add(k)`, `remove`, `set_key`,
`first`, `last`, walks from a key) and `bag<T, N>` (the smallest module, for tests).

## Workers
A system runs several programs as workers, each on its own thread with its own memory; they share nothing.

```
system {
    worker auth  { program "auth.mito";  count 2; mailbox 1024; slot 4096; }
    worker rooms { program "rooms.mito"; count 4; mailbox 1024; slot 4096; }
    entry auth;       // requests enter here (by session-token shard, otherwise in turn)
    inflight 4096;    // the most requests in flight at once
    budget 8192;      // MB, checked when the system is built
}
```

- A mailbox is a bounded ring of fixed-size slots; a full one is 503.
- `forward kind[key] { field: expr };` ends the handler and hands the request to worker `key % count` of that kind,
  whose endpoint takes the fields as inputs. The forward is part of the transaction: if the mailbox is full, the
  sender's writes roll back. Sends never block, so forwards cannot deadlock.
- A worker that fails (only `__crash()`, the test hook, so far) is rebuilt from its declarations; its state is lost.
- Not possible by design, for now: references across workers (data is copied by value), fan-out, and distributed
  transactions (a later worker's failure does not undo an earlier one's writes).

## Failures
| Failure | Meaning | HTTP |
|---|---|---|
| `Full` | a collection or area is at its maximum | 503 |
| `TooLong` | a value exceeds its maximum | 413 |
| `NotLive` | a slot or position is not in use | 404 |
| `Stale` | an id or session token naming a reused slot | 401 for a session, else 404 |
| input | a request input absent, of the wrong kind or out of range | 400 |
| `fail CODE "msg"` | the program's own | as given |

## Fit
Good: game and simulation state, servers' hot state (sessions, queues, rate limits), embedded and real-time work,
anything whose peak memory must be known before it runs. Poor: unbounded or heavy-tailed sizes (documents, editors,
crawlers), irregular graphs, exploratory scripting. Durable data belongs in a database; this is the layer above it.

## Status (2026-11-04)
Everything above is built. A program or system compiles to one C program (runtime/mito.c, system.c, http.c), built by
clang -O3 or, in milliseconds, the bundled TinyCC; Rust is only the compiler. The 24 cases in `tests/cases` and
`tests/http.record` must give the same answers on the TinyCC build, the clang build and a clang build with
undefined-behaviour checks. Windows only so far.

Limits:
- Functions have full signatures (`fn f(p: Player, n: i64) -> f64`, `-> T?` for a result that may be null, no arrow for none;
  `pool<T>`, `list<T>`, and in modules `T.key`, the type of a field), checked at every call and return. Calls are still
  inlined (a recursive function is one C function per combination of argument places); recursion is limited to 64 nested calls.
- Only a string input can be optional; a number takes a default. An item input cannot be optional yet.
- A real fault in the C code ends the whole process, every worker with it.
- Pressure marks are absolute megabytes; no idle trim yet.
- `string<N>` reserves N bytes per slot: 2000-byte chat bodies averaging 120 bytes left 39.5 MB committed for 3.1 MB
  live. Packed text is planned (Next, 3).
- HTTP keeps connections open with a fixed pool of connection threads and one poller (WSAPoll, up to 4,096 idle connections);
  an event loop on IOCP would serve more connections with fewer threads. One program handles one request at a time.

## Next
1. Linux: an OS layer (memory, threads, locks, clock, randomness, sockets) with the cases and the HTTP record passing
   on both systems (Linux in WSL2).
2. HTTP: an event loop over a fixed table of connections (IOCP on Windows, epoll then io_uring on Linux) in place of
   the poller and connection threads. Keep-alive is done (2026-11-05): a kept connection costs about 100 µs a request
   on Windows loopback against 1-1.5 ms for a new connection each time.
3. Long uneven text: `text<N>` fields packed into a per-collection text area.
4. Shards placed by load rather than `id % count`.
5. Worker messages beyond `forward`: one-way `send`, fan-out with gathered replies, links and monitors.
