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

**Values.** Scalars (`i8`..`i64`, `u8`..`u64`, `f32`, `f64`, `bool`), enums (`enum Op { Plus, Minus }`, stored in the smallest
integer), text of at most N bytes (`utf8<N>`, also written `string<N>`, or `ascii<N>` for bytes 0..127) and `T[N]` (exactly N). Code reads maxima rather than repeating them (`x.capacity()`, `maxlen(s)`). An integer field may declare a range,
`limit: i64 in 0..3000000` (both ends included, 0 inside, since state starts zeroed): every write is tested (400 outside), and reads let the
compiler drop overflow and index tests on what is computed from them. A field without a declared range whose every store in the program has a proven range
(`s.n = n` with `n: i64 in 2..900`) gets their union, with 0, the same way: the compiler works it out from the whole program. `unlogged` after a plain field, a collection, a struct field (with the collections inside it) or a module instance (`m: ordmap<N> unlogged;`) keeps it out of the
undo log: a failed request leaves what it wrote there (for scratch state rebuilt by every request); removals still wait for success.

**`list<T, N>`** keeps insertion order: add, read by position, walk forwards or in `reverse`. When full it fails
(`Full`), or with `drop_oldest` overwrites the oldest (a ring). Positions shift as items drop, so a list item is
named by a key it carries, never referenced. Use it for messages, logs, recent events.

**`pool<T, N>`** keeps items in slots that never move, with a live bit each; new items take the lowest free slot. When
full, `add` fails. `checked` adds a generation per slot, so an id from a request or a session token that names a reused
slot is detected (`Stale`). Use it
for things that are referred to and come and go: sessions, users, rooms.

**`table<T>`** keeps its rows in SQLite (the bundled amalgamation, in `--data dir`, default `./data`): kept across restarts,
readable by any SQLite tool while the server runs, and with no fixed number of rows (it takes no capacity). Rows hold plain fields
(text keeps its maximum) and nested tables, a board's `todos: table<Todo>` being the rows each board owns, removed with it. Code
reads the same as for other collections: `add`, field reads and writes, `remove`, `count()`, `t[id]` (404 when it is not this
table's, or this owner's, row), `foreach ... where ... reverse limit`, a row from a cookie (`board: Board? from cookie("list")`,
named by `token(b)`). A row's id is its SQLite row id, never reused. `index(f)` and `unique(f)` on the collection declare
indexes; a `where` over a whole top-level table is compiled to SQL and must use indexed fields (one that would read every row is
refused), while a loop over one owner's rows may filter on anything. `foreach u in app.users by name` walks an indexed field's order; `after x` continues
after row x in the loop's order (keyset paging: `foreach t in b.todos after last limit 20`, null starting at the beginning);
`let u = first u in app.users where u.email == x;` is the first row a loop would visit, or null (one `SELECT ... LIMIT 1`; `first`
works on every collection). A row's field `assignee: User?` references a row of another table (a column `REFERENCES users(id)`,
set null when that row goes); working state can hold one too (the row id, null once the row is gone). A loop over each row's
children inside a loop over the rows (`foreach t in b.todos` in `foreach b in app.boards`) is one query for all the children,
walked in step with the outer loop, not one query per row. Each request is one transaction, committed before its
answer and synced to disk first (`--db-sync full`, the default; `--db-sync normal` syncs at checkpoints instead, losing nothing on a
process crash but possibly the last moments on an OS crash or power cut). At start the database is created or migrated by field name; a column whose type changed, or rows that break a declared
limit, stop the server with the reason; `--db-fit drop` drops them instead (the column's values, or the rows and what they
own), saying each on standard error (docs/persistence.md).

**Naming another item.** A field `best: Player?` holds an item's id (its slot and generation, 8 bytes) and reads back as a
`T?`: null when the item was removed, also after its slot was reused by another item, so it is handled as every `T?` is (`if
exists(b)` on a local, or `or(game.best.name, "-")`). It is set from an item, a `T?` or `null`, and an answer writes it as the
id or null. The pool must be a top-level `checked` one (generations tell a reused slot from its item; `Player in path?` when
several hold Player). An item can also name another by its slot in an integer field (`who: i64`, from `slot(p)`), read back
with `pool[slot]`, which checks the slot is live but not that it is still the same item.

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

**Epochs.** Each request gets a text area for its input (path, query, headers and body fields, decoded) and the program's
literals, and a response area, both reset when it ends. Text a request builds goes in declared fields, not there: the state's,
or those of the `request` struct, a struct of plain fields declared beside the state (`request work: Work;`), zeroed before each
request, kept out of the undo log and never kept. Running out of an area fails that request (413 if the request itself does not
fit, 507 otherwise). There is no heap allocator anywhere.

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

**Functions.** `fn name(p: Player, n: i64) -> f64 { ... }`: every parameter and the result are typed (`-> T?` when it may be
null, no arrow when it gives nothing), and checked at every call and `return`. `let` and `var`; `+=` and friends. No closures. A
`var` keeps the kind of its first value. A local holds numbers, bools, items and windows onto text (a field, an input, a literal,
a `substr` or `trim` of one), never text of its own: `var t = ""` and a function returning text are errors that say to build the text in a
field (a function takes the item or struct to write into).

**Endpoints** are functions with an attribute and typed inputs:

```
@get("/rooms/{room}/messages")
fn messages(who: Session from header("X-Session"), room: Room, limit: i64 in 0..100 = 50)
    -> Response<{ messages: [{ seq: i64, author: string, body: string }] }> {
    return ok({ messages: [foreach m in room.messages reverse limit limit {
        seq: m.seq, author: or(m.author.name, "[deleted]"), body: m.body }] });
}
```

- Inputs come from the path (`{room}`: an id gives the item), the body (POST, PUT, PATCH: JSON, or an HTML form's
  `application/x-www-form-urlencoded`, whose values are read as text the way query values are; a ticked checkbox's `on` is
  true, an unticked one is absent and takes the default; a name given twice keeps its first value) or the query (GET,
  DELETE), a header (`from header("X-Session")`: a session token gives the session), a cookie (`from cookie("list")`; an
  item from a cookie is named by a session token, and `Board?` makes it null when the cookie is absent or names no live
  item with that secret, as for a first visit), or another worker
  (`forwarded`). They are checked before the body runs: absent, wrong kind or out of range 400, too long 413, no such
  item 404, a body field not taken 400.
- Markup: an HTML answer is written as HTML in the code, `return ok(<ul>{foreach t in b.todos { item(t); }}</ul>);`, and a
  function declared `-> html` writes a reusable piece (`fn item(t: Todo) -> html { <li class="{if t.done { "done" }}">{t.title}</li> }`).
  Markup starts at `<tag` after `(`, `,`, `{` or `;`. `{x}` writes a value escaped (text with `& < >` as entities, and `"` inside
  an attribute's quotes; numbers as digits); `name={b}` with a bool writes the bare attribute when true; `{if ...}`, `{foreach ...}`
  and `{piece(x);}` are statements, in which a bare string writes itself. The compiler checks that tags match (void elements such
  as `<input>` need no closing tag), that text has no raw `<`, and that markup is written only into an HTML answer. It is lowered
  to plain statements that append to the answer, fixed text merged into one copy per run; text across lines is joined by one
  space and comments are dropped. Two ways to write markup that is not code: `{include_html("fragments/head.html");}` writes one of
  the program's files as it is (read when compiling, relative to the program: generated asset lists, fixed blocks), and
  `{html_raw(t)}` writes text the program trusts unescaped (an administrator's stylesheet in a `<style>`), as Rails' `html_safe`.
- For browsers: a request other than GET or HEAD whose `Origin` is not this `Host` (a form posted from another site's page) is
  refused 403; a request without `Origin` (an API client) is not a browser's cross-site one. Every answer says
  `X-Content-Type-Options: nosniff`, and a page adds a Content-Security-Policy allowing scripts, styles, images and form
  targets only from this site (`default-src 'self'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; base-uri
  'none'`) and `Referrer-Policy: same-origin`. `HEAD` is answered as `GET` without the body. An endpoint that answers HTML gets
  its errors as a small page (the message escaped) instead of JSON.
- `set_cookie("list", token(b), 31536000)` adds a Set-Cookie line to the answer (`Path=/; HttpOnly; SameSite=Lax`; max age 0
  deletes it). The value must be cookie characters (printable ASCII without spaces, quotes, commas, semicolons, backslashes).
  A fourth argument `false` leaves out HttpOnly (`set_cookie("last_room", str(id), 631152000, false)`).
  `set_header("Link", value)` adds a header line (the name a literal token, checked when compiling, and not one the server writes
  itself such as Content-Length; the value printable ASCII, checked when running).
- **`@start fn init() { ... }`** runs once when the program starts: after its tables open, before any request (in `mito test`,
  before each test), as a request without inputs whose writes are kept; a failure stops the server, saying why. It is never
  reachable as a request. It is where a program reads its environment (`env("SECRET_KEY_BASE")`, the variable's text or null) and
  keeps what it derives from it (a key from 1000 rounds of PBKDF2 is computed once, not per request).
- **Times.** A `time` field is an i64 of microseconds since 1970 (UTC); in a table's row it is kept as text (docs/persistence.md).
  `format_time(t, "%Y-%m-%d %H:%M:%S")` writes one out (%Y %m %d %H %M %S, %L milliseconds, %N microseconds, %s seconds, %%).
- **Bytes, encodings and cryptography.** A text holds any bytes, so these work on texts and give texts (in the request's text area):
  `base64(t)` (padded), `base64_url(t)` (URL alphabet, no padding), `unbase64(t)` (either alphabet, padding optional; null if not
  base64), `hex(t)`, `form_encode(t)` / `form_decode(t)` (as `URI.encode_www_form_component`; null for a bad `%`), `xor_bytes(a, b)`
  and `secret_equal(a, b)` (in time that depends only on the lengths). The primitives come from the operating system's library and
  are never written here (runtime/crypto.c: BCrypt on Windows, libcrypto on Linux, linked only into a program that uses them):
  `sha1(t)`, `sha256(t)`, `hmac_sha1(key, t)`, `hmac_sha256(key, t)`, `pbkdf2_sha256(password, salt, iterations, length)`,
  `random_bytes(n)` (the system's generator, never the seeded one tests use), `aes256gcm_seal(key, nonce, t)` (ciphertext then the
  16-byte tag) and `aes256gcm_open(key, nonce, sealed)` (null when the tag does not match). Passwords: `bcrypt_hash(password, cost)`
  and `bcrypt_check(password, digest)` from crypt_blowfish (third_party/crypt_blowfish, the reference implementation), counting the
  first 72 bytes as bcrypt does. tests/cases/crypto.case checks each against its published test vector.
- **The `web` block** (src/web.rs) says how a program's answers look on the wire, for a program that must answer as another
  server does (a port of an existing app):
  ```
  web {
      headers { "X-Frame-Options": "SAMEORIGIN"; "Vary": "Accept"; }  // every endpoint answer, in place of the policy lines above
      etag "max-age=0, private, must-revalidate";  // a weak ETag on every 200 answer to a GET, with this Cache-Control
      gzip;                                         // bodies gzip-compressed when the request accepts gzip
      static "/" from "public" {                    // the directory's files, kept in the program, answered before the endpoints
          "Cache-Control": "public, max-age=2592000";
          types { ".map": "text/plain"; }           // content types beyond the usual ones, by extension
      }
  }
  ```
  `headers` replaces the built-in `nosniff`, Content-Security-Policy and Referrer-Policy lines; an endpoint's own line of the same
  name (`set_header`) wins over the block's. `etag`: the tag is two 64-bit hashes of the body; `If-None-Match` naming it answers 304
  without a body or content type; its Cache-Control (if given) replaces the block's on tagged answers unless the endpoint set one.
  `gzip` compresses every answer with a body, whatever its type, at zlib's default level (third_party/zlib, linked only into such a
  program), and adds `Accept-Encoding` to `Vary`. A `static` block's files are read when compiling (binary files too, every file under
  the directory) and kept in the program as data; they answer GET and HEAD for their path whatever the query, with their type, the
  block's lines, `Last-Modified` (when the server started, by its clock) and, with `gzip`, a copy compressed once at start. A file whose
  extension has no known type is refused when compiling, naming it, rather than served as a guess. Systems of workers take no web block
  yet.
- `Response<T>` declares the answer's JSON shape, checked against every answer when compiling. `ok` 200, `created`
  201, `no_content` 204, `status(code, x)`; `fail CODE "message"` ends a request with an error from any function. An
  item is written as its id (`"slot.generation"` for a checked pool).
- `Response<html>`, `<text>`, `<css>`, `<js>` and `<svg>` answer text as it is with that content type (UTF-8). `file("assets/app.css")`
  is a file's text, read when compiling (relative to the program) and kept in the program, so a stylesheet is served as
  `@get("/app.css") fn css() -> Response<css> { return ok(file("assets/app.css")); }`. An answer that is a literal is the same
  every time: it gets an `ETag` (`Cache-Control: no-cache`), and a request whose `If-None-Match` names it is answered 304. `return see_other("/path")`
  answers 303 from any endpoint; the address must be a path on this server (one leading `/`, printable ASCII), checked when
  compiling for a literal and when running otherwise, so a redirect can neither leave the site nor add header lines. Errors are
  answered in JSON.
- Collections are used through their functions: `add` (or `add { field: value }`), `try_add`, `remove`, `count`,
  `capacity`.

**Every request is atomic.** The runtime records the old bytes of each write; a failed request writes them back.
It saves whole 64-byte lines on their first write in a request; a 4 KB page still all zeros (state never written, as an array a request
fills for the first time) is recorded as one "zeros" entry instead of a copy, so filling fresh state costs no undo-log memory.
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

**Debugging.** `log(a, b, ...)` writes its values (formatted as `+` would, separated by spaces) on one line of the server's
standard error, with the line it was called from (`log: line 12: n is 5`); it is not part of the answer.

**Tests.** `test "name" { ... }` in the program file is a function the test runner calls: inside, `get`, `post`, `put`, `patch`
and `delete` make requests (`post("/players", { name: "ann" }, { "X-Session": token })`) answering `.status` and `.body`,
`field(text, "name")` reads a field of a JSON answer, and the state is read directly (changed only through requests). It returns
`true`, `false` or a text saying what went wrong. `mito test app.mito` runs each test on a fresh state with the clock and
randomness fixed, and prints the line of the `return` that failed one, or of the statement whose error ended it. Tests are never
part of a built or served program.

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

## Status (2026-11-05)
Everything above is built. A program or system compiles to one C program (runtime/mito.c, system.c, http.c), built by
clang -O3 or, in milliseconds, the bundled TinyCC; Rust is only the compiler. The command is `mito` (`check`, `build`,
`serve [--watch]`, `test`, `emit-c`, and `check --kinds` listing every local's kind; `mitochondria` is the same program): `serve --watch` rebuilds and restarts on save, keeping
the last good build when a change does not compile. Errors name `file:line:col` with the line and a caret, suggest the nearest
name for a misspelt one, list every independent error of a file, and say what to write for a habit from other languages (`i++`,
`each`); `tests/errors` holds a program per mistake with its expected message. The 51 cases in `tests/cases` and
`tests/http.record` must give the same answers on the TinyCC build, the clang build and a clang build with
undefined-behaviour checks. VS Code highlighting is in `editors/vscode`. Windows and Linux: everything the runtime asks of the
system is runtime/os.c (memory, clock, randomness, threads, locks, sockets and poll); on Linux programs build with the system's
clang (`--tcc` uses a `tcc` on the path, else clang -O0), and the cases, the HTTP record and the test runner pass there (WSL2,
Ubuntu 24.04, clang 18).

Limits:
- Functions have full signatures (`fn f(p: Player, n: i64) -> f64`, `-> T?` for a result that may be null, no arrow for none;
  `pool<T>`, `list<T>`, and in modules `T.key`, the type of a field), checked at every call and return. Calls are still
  inlined (a recursive function is one C function per combination of argument places); recursion is limited to 64 nested calls.
- Only a string input can be optional; a number takes a default. An item input cannot be optional yet, nor an enum input
  read by name.
- A stored id (`Player?` field) names only items of a top-level pool, not of a nested collection.
- A real fault in the C code ends the whole process, every worker with it.
- Pressure marks are absolute megabytes; no idle trim yet.
- `string<N>` reserves N bytes per slot: 2000-byte chat bodies averaging 120 bytes left 39.5 MB committed for 3.1 MB
  live. Packed text is planned (Next, 3).
- HTTP keeps connections open with a fixed pool of connection threads and one poller (WSAPoll, up to 4,096 idle connections);
  an event loop on IOCP would serve more connections with fewer threads. One program handles one request at a time.

## Next
1. Linux: done (2026-11-05), see Status. The LangArena runner still pins processes with Windows calls.
2. HTTP: an event loop over a fixed table of connections (IOCP on Windows, epoll then io_uring on Linux) in place of
   the poller and connection threads. Keep-alive is done (2026-11-05): a kept connection costs about 100 µs a request
   on Windows loopback against 1-1.5 ms for a new connection each time.
3. Long uneven text: `text<N>` fields packed into a per-collection text area.
4. Shards placed by load rather than `id % count`.
5. Worker messages beyond `forward`: one-way `send`, fan-out with gathered replies, links and monitors.
