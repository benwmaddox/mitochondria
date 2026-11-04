# Chat API on Mitochondria: what worked and what did not (2026-11-03)

A Campfire-style chat server's hot state and API, written in the new endpoint language to see how the language holds up.
Files: `examples/chat/chat.mito` (13 lines of declared state), `examples/chat/chat.api` (96 lines, 9 endpoints),
interpreter and server `src/lang.rs` (212 lines) and `src/serve.rs` (600 lines, including tests).
Worked from general knowledge of how a chat server behaves; no Campfire source or specs were consulted.

## What was built
A tree-walking interpreter for a small endpoint language (`let`, assignment, `if`, `each ... where ... order by ... limit`,
`alloc`, `free`, `fail`, `respond` with JSON objects and array comprehensions, built-ins such as `body`, `param`, `query`,
`header`, `auth`, `token`, `now`, `random`, `count`, `capacity`, `stat`), plus a single-threaded HTTP/1.1 server using only `std`
and an in-process benchmark. 12 tests cover auth, error codes, eviction, rate limit, typing expiry, rollback and JSON escaping.

| Endpoint | Lines |
|---|---|
| POST /sessions | 10 |
| DELETE /session | 5 |
| POST /rooms | 9 |
| GET /rooms | 4 |
| POST /rooms/{room}/messages (auth, validation, rate limit, eviction, store) | 24 |
| GET /rooms/{room}/messages (filter, newest first, limit) | 11 |
| POST /rooms/{room}/typing | 12 |
| GET /rooms/{room}/typing | 6 |
| GET /stats | 7 |

## Works over real HTTP
Smoke test with `curl` against the socket server: create session (201, token), create room, post a message with escaped quotes,
list newest first, typing ping and list, stats, and every failure mode: no token 401, wrong secret 403, ended session 401,
missing room 404, string over `string<48>` 413, unknown path 404. Session tokens are `slot.generation.secret` where the secret
is 64 bits from the OS (`BCryptGenRandom`), because slot and generation are sequential and guessable on their own.

## Measured (in-process, no sockets; interpreter-bound)
`mitochondria bench examples/chat/chat.mito examples/chat/chat.api`, microseconds. Declared state reserves 39.6 MB.

| Endpoint | Calls | Mean | p50 | p99 |
|---|---|---|---|---|
| POST /sessions | 1000 | 3.5 | 3.1 | 9.3 |
| POST message, pool filling | 20000 | 8.1 | 7.8 | 12.1 |
| POST message, pool full (evicts oldest) | 200 | 951 | 957 | 1087 |
| GET messages, limit 50 (20,000 messages) | 1406 | 2880 | 2371 | 5152 |
| POST message, mixed load | 401 | 1124 | 961 | 1940 |
| POST typing | 91 | 20.9 | 18.2 | 42.6 |
| GET typing | 102 | 10.7 | 9.9 | 22.1 |
| DELETE /session | 1000 | 3.1 | 2.7 | 3.1 |

Memory: 1000 sessions and 20 rooms committed 0.10 MB; 20,000 messages committed 39.5 MB for 3.1 MB live.
Deleting every session and running `trim` released 0.07 MB.

**Interpreter versus layout.** A native Rust scan of the same 20,000-message columns (filter by room, sort by sequence,
take 50) takes 6.1 us. The interpreter takes 2,880 us after one optimization pass (it was 9,344 us first: per-row cloning of the loop variable's
name and a path built for every field read). So `GET messages` is about 470x the native cost, almost all interpreter overhead:
per-row `Value` handling, `slot(room)` re-evaluated for every row, no loop-invariant hoisting. This does not say anything bad about
the column layout; it says a tree-walking interpreter is the wrong place to scan 20,000 rows. A compiler would hoist, bind columns
and compile the filter into a column loop. Simple endpoints at 3 to 20 us are fine for an interpreter.

## Findings: what the language got right
- **Capacity as part of the API.** A full pool is a typed `FULL` that becomes a 503; a string over its declared length is a 413; a dead
  slot is a 404; these are different, and the interpreter maps each separately. This came for free from the declared types.
- **Checked pools gave real auth hygiene.** A deleted session's token is rejected as "session ended" even after its slot is reused
  by someone else (generation), and a guessed generation with the wrong secret is a different failure (403).
- **Overload and memory are visible.** `GET /stats` reports reserved, committed and live memory from inside the app.
- **Handlers read well.** Most endpoints are 4 to 12 lines. Request, state and response are all in one place.
- **Per-request epoch.** Responses are built in a fixed 256 KB block carved from the declared reservation and reset for every request.
  Overflow is a defined 507, not an unbounded string.

## Findings: gaps and awkward places (honest list)
1. **A failed request leaked state.** First version of `POST /rooms` allocated a slot, then failed on an over-long name; the pool filled with
   empty "ghost" rooms and later requests got 503. Fixed by adding request-level rollback (allocations released and writes restored when a
   request fails). Frees mid-request are not undone. This is a language-level need: handlers need atomicity, and without it every endpoint
   has to validate before it allocates. `POST messages` still validates length by hard-coding 2000 (the declared maximum is not available to code).
2. **No index, so reads are scans.** "Messages of room 3, newest first, limit 50" scans every live message. At 20,000 messages that is the
   2.9 ms above; a per-room index or a per-room ring would turn it into reading the last 50 entries. The language needs a keyed index
   or per-parent collections.
3. **No `ring` kind.** Eviction of the oldest message when the pool is full is `each old in messages order by old.seq limit 1 { free old; }`,
   which sorts all 20,000 rows: 951 us per post once full (vs 8 us before). A ring (or an ordered pool) makes this O(1).
4. **Large fixed string slots defeat page reclaim.** `body: string<2000>` reserves 40 MB for 20,000 messages whose bodies average about
   120 bytes. Every page of the body column holds at least one live prefix, so `trim` cannot release any of it: 39.5 MB committed for
   3.1 MB live, 92% waste. This is the page-granularity limit measured earlier, now in a realistic case. Chat text needs a variable-size
   text arena, or smaller slots with an overflow path. Fixed `string<N>` is the wrong tool for long, uneven text.
5. **No mutable local variables and no functions.** Upsert ("update if exists else insert") became "delete old, then insert". Every
   authenticated endpoint repeats `let me = auth(sessions);`. Common checks are copied into several endpoints.
6. **Denormalization is forced.** Messages store `author_name` because looking up a deleted author's session would turn the whole listing
   into a 404. A join-like lookup with a fallback needs a way to say "if present".
7. **Declared maxima are invisible to code.** Only `capacity(collection)` was added; string maxima are not queryable, hence the hard-coded 2000.
8. **No time-based expiry, no JSON beyond flat input, no percent-decoding of query strings, no HTTP/1.1 keep-alive or chunking.**
9. **Single-threaded.** The mailbox/worker design is not built. These numbers say nothing about concurrency.
10. **Temporaries use the Rust heap.** Request parsing, the interpreter's values and response copies are ordinary Rust allocations, outside
    the declared reservation. Only state and the response buffer are in declared memory, so "all memory declared" is not yet true of the server.
11. **Messages in memory are a cache.** A real Campfire keeps history in a database; this models the hot window only.

## Verdict
For the hot, bounded, ephemeral state of a chat server (sessions, presence, typing, rate limits, recent messages), the memory model
fits and the handlers are short and legible. For a complete app it is missing what a web language needs:
keyed indexes or ring collections, atomic requests (now partly present), reusable functions, a variable-size text store, and a
compiler to remove the interpreter's per-row cost. The first four are language design work, not engineering; the last is engineering.

## Next steps, in order
1. `ring<T,N>` kind and a keyed index (`pool<T,N> index room`) so `GET messages` and eviction stop scanning.
2. A variable-size text region with a declared total, for message bodies.
3. Functions, so `auth` and rate limiting are written once.
4. Compile endpoints to column loops (bind columns once, hoist invariants) and re-run the bench against the native baseline.
5. Build the worker/mailbox design and a socket load test, and compare against the existing Campfire benchmarks the user has.
