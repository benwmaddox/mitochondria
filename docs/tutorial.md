# Mitochondria tutorial

This tutorial builds one small program, a game scoreboard, and uses it to show each idea in the language once. It
takes about half an hour. The finished program is `examples/scoreboard/score.mito`, and `tests/cases/scoreboard.case`
sends every request shown here, so the answers below are the program's real answers.

## What kind of language this is

In most languages a program asks for memory as it goes, and something (you, or a garbage collector) gives it back.
In Mitochondria you instead **declare all of the program's state up front, with a maximum for everything**: at most
1000 players, names of at most 24 bytes, the last 20 games. The compiler works out exactly where every value lives,
and the program reserves that much address space once, when it starts.

Three words matter throughout:

- **Reserved:** address space set aside for the declared maximum. It costs no memory.
- **Committed:** the part the operating system has actually promised to back with memory. Mitochondria commits pages
  only as data is written into them, and can give pages back ("trim") when the data in them is gone.
- **Live:** the bytes the program is using right now. The runtime knows this exactly, from the lengths and live bits
  it keeps, without scanning or guessing.

So a program declared for 1000 players that has 3 costs memory for 3, and when it is full it says so with a typed
error rather than crashing or being killed for running out of memory.

Everything is typed, and every type is known when the program is compiled: the state, every value, what each endpoint
takes from a request and what it answers. A program is one `.mito` file. The compiler turns it into one C program,
which is built into an executable that serves HTTP.

## Setting up

You need Windows (the only platform so far) and Rust, to build the compiler:

```
git clone https://github.com/benwmaddox/mitochondria
cd mitochondria
cargo build --release
```

The compiler is `target\release\mitochondria.exe`; below it is just `mitochondria`. TinyCC, a small C compiler, is
bundled (`third_party/tinycc`) and builds programs in milliseconds. For optimised release builds, install LLVM
(clang); `mitochondria build` uses it by default. Nothing else is needed.

## Step 1: the state

The top of `examples/scoreboard/score.mito`:

```
struct Player { name: string<24>; best: i64; entry: ref<game.board>; }
struct Entry  { who: ref<game.players>; points: i64; }
struct Played { who: ref<game.players>; points: i64; at: i64; }
struct Row    { name: string<24>; points: i64; }

struct Game {
    games: i64;
    players: pool<Player, 1000> checked;
    board: sorted<Entry, 1000, points>;
    recent: list<Played, 20> drop_oldest;
}
state game: Game;
```

A program has **one state**, a struct that holds everything: `state game: Game;`. Every path starts at its name:
`game.players`, `game.board`. To group things, nest structs (`struct Game { scores: Scores; chat: Chat; }` gives
`game.chat.rooms`).

The fields of `Game`:

- **`list<Played, 20> drop_oldest`** keeps items in the order they arrive, at most 20. `drop_oldest` says what happens
  when it is full: the new item overwrites the oldest, which makes it a ring buffer. The other choice, the default, is
  to fail.
- **`sorted<Entry, 1000, points>`** keeps up to 1000 entries in order of their `points` field. It is not built into
  the language: it is a module from the standard library (`lib/sorted.mito`), written in Mitochondria itself.
- **`pool<Player, 1000> checked`** holds up to 1000 players in slots that never move. A new player takes the lowest
  free slot. `checked` adds a generation number to each slot, so a reference to a player who has left is detected
  even after the slot is reused.
- **`games: i64`** is a plain value: one, always there.
- **`ref<game.players>`** is a reference to a player in that pool. Reading through it checks that the player is still
  there. A reference points into a pool that exists once (reached from the state through plain structs, not inside
  another collection's items).
- **`string<24>`** holds at most 24 bytes; **`i64`** is a 64-bit integer. Other scalars are `i8`..`i64`, `u8`..`u64`,
  `f32`, `f64` and `bool`; `T[N]` is a fixed array of exactly N values.

Every collection here is stored as **columns**: all the names together, all the best scores together. A loop that
reads one field touches only that field's memory.

Which collection to use: a **list** for things you append and read in order or from the newest end (messages, logs,
events); a **pool** for things that other data refers to and that come and go in any order (users, sessions, rooms);
a **sorted** collection when you need them in order of a field. Collections can also be fields of an item: a `Room`
can hold `messages: list<Msg, 1000>`, 1000 per room (see `examples/chat`).

## Step 2: an endpoint

```
@post("/players")
fn add_player(name: string<1..24>) -> Response<{ id: Player }> {
    let p = game.players.add { name: name };
    p.entry = game.board.add(0) { who: p };
    return created({ id: p });
}
```

- **An endpoint is a function with an attribute**: `@get`, `@post`, `@put`, `@patch` or `@delete`, and its path.
- **Its parameters are what it takes, with types.** For a POST they come from the JSON body; for a GET or DELETE from
  the query string; a parameter named in the path (`{p}`) from the path. The runtime checks them before the body runs:
  here a name of 1 to 24 bytes, or the request gets a 400 (413 if it is too long). A field the endpoint does not take
  is an error too, so a client's typo is caught.
- **`game.players.add { name: name }`** adds a player and sets its fields; fields not named are zero. Collections are
  used through their functions: `add`, `try_add` (null when full instead of failing), `remove`, `count`, `capacity`.
  The `sorted` module's `add` takes the key: `game.board.add(0)` places an entry with 0 points.
- **`-> Response<{ id: Player }>`** declares the answer's JSON shape, and the compiler checks every answer against it.
  **`return created(...)`** answers 201; `ok(...)` 200, `no_content()` 204, `status(code, ...)` another success.
  **`fail CODE "message"`** ends the request with an error, `{"error":"message"}`.
- **An item in an answer is written as its id.** For a `checked` pool the id is `"slot.generation"`, so the first player
  is `"0.0"`.

State is read and written directly: `p.entry = ...` writes into the player.

## Step 3: check, then run

```
mitochondria check examples/scoreboard/score.mito
```

`check` reports the first problem, or prints `score: loads`. Problems are found here, before the program ever runs:
misspell a field (`{ nmae: name }`) and you get

```
error: score.mito: POST /players: no field `nmae`
```

Then run it:

```
mitochondria serve examples/scoreboard/score.mito --port 8080
```

`serve` builds the program with TinyCC and runs it at once. From another terminal (PowerShell 7):

```
curl.exe -X POST http://127.0.0.1:8080/players -d '{"name": "ada"}'
{"id":"0.0"}
curl.exe -X POST http://127.0.0.1:8080/players -d '{"nmae": "ada"}'
{"error":"unknown field `nmae`"}
```

The rest of this tutorial shows requests in the short form the test cases use, `POST /players {"name": "ada"}`, each
followed by its answer.

## Step 4: items by id, loops, data kept in order

Recording a game:

```
@post("/players/{p}/games")
fn record(p: Player, points: i64) -> Response<{ best: i64 }> {
    game.games += 1;
    game.recent.add { who: p, points: points, at: now() };
    if points > p.best { p.best = points; game.board.set_key(p.entry, points); }
    // checked last on purpose: everything above is undone when this fails
    if points > 1000000 { fail 422 "not a believable score"; }
    return ok({ best: p.best });
}
```

`p: Player` is named in the path, so the path's id (`/players/0.0/games`) gives the player itself, or the request gets
a 404 before the body runs. `game.board.set_key` moves the player's entry to its new place in the order; the key of a
sorted item can only be changed this way, so the order is never wrong.

Reading them back:

```
@get("/board")
fn board(top: i64 in 1..100 = 10) -> Response<{ top: [Row] }> {
    return ok({ top: [each e in game.board reverse limit top { name: e.who.name, points: e.points }] });
}

@get("/recent")
fn recent() -> Response<{ games: [Row] }> {
    return ok({ games: [each g in game.recent reverse { name: or(g.who.name, "[left]"), points: g.points }] });
}
```

`top: i64 in 1..100 = 10` takes `?top=` from the query, from 1 to 100 inclusive, and is 10 when absent; `?top=500` is
answered `` {"error":"`top`: 500 is outside 1..100"} ``. `[Row]` in the answer's type is an array of objects shaped
like `struct Row`.

**`each x in collection`** visits the items. Clauses narrow it: `reverse` (newest first for a list, highest first for
a sorted collection), `where cond`, `limit n`, and for a sorted collection `from key`. Inside `[ ... ]` in an answer,
it builds a JSON array. `each i in 0..10` counts. Every loop is bounded by a capacity or a range, so every loop ends.

There is no `order by`. The board is already in order because it is kept in order; reading the top 10 is reading the
last 10 entries. `or(a, b)` gives `b` when `a` is missing: here, a player who left.

After three players and five games:

```
GET /board          {"top":[{"name":"ada","points":400},{"name":"bo","points":340},{"name":"cy","points":90}]}
GET /board?top=2    {"top":[{"name":"ada","points":400},{"name":"bo","points":340}]}
GET /recent         {"games":[{"name":"ada","points":50},{"name":"ada","points":400},{"name":"cy","points":90},
                              {"name":"bo","points":340},{"name":"ada","points":120}]}
```

## Step 5: every request is all or nothing

Every endpoint is **atomic**: if it fails, every change it made is undone, as if it never ran. The runtime remembers
the old bytes of each write during a request and puts them back on failure. Look at `record` again: it counts the game,
appends it to `recent` and may move the board, and only then checks the score.

```
GET /stats                         {"players":3,"games":5,...}
POST /players/1.0/games {"points": 5000000}
                                   422 {"error":"not a believable score"}
GET /stats                         {"players":3,"games":5,...}
GET /board                         {"top":[{"name":"ada","points":400},{"name":"bo","points":340},{"name":"cy","points":90}]}
```

`games` is still 5, the game is not in `recent`, and bo is not at the top. You never write clean-up code for a failure.

## Step 6: limits are errors, not crashes

Everything has a declared maximum, and going past one is an ordinary, typed error. Most are caught at the edge, by the
endpoint's parameters:

```
POST /players {}                   400 {"error":"`name` is required"}
POST /players {"name": "a name much longer than twenty-four bytes"}
                                   413 {"error":"`name`: 41 bytes, at most 24"}
POST /players/0.0/games {"points": "lots"}
                                   400 {"error":"`points`: expected an integer"}
POST /players/7.0/games {"points": 1}
                                   404 {"error":"`p`: no 7.0 in game.players"}
```

A full pool answers 503, for example `{"error":"capacity: game.players is full (1000 of 1000)"}`. The kinds of
failure: a collection or area full (503), a value too long (413), an item not there (404), a stale session (401), an
input of the wrong kind (400), and your own `fail`.

## Step 7: references, and leaving

```
@delete("/players/{p}")
fn remove_player(p: Player) -> Response<none> {
    game.board.remove(p.entry);
    game.players.remove(p);
    return no_content();
}
```

`game.players.remove(p)` removes the player when the request succeeds. Bo's games are still in `recent`, holding a
reference to a player who has gone:

```
DELETE /players/1.0      204
GET /recent              {"games":[...,{"name":"[left]","points":340},...]}
POST /players {"name": "dee"}
                         201 {"id":"1.1"}
GET /recent              {"games":[{"name":"dee","points":10},...,{"name":"[left]","points":340},...]}
```

Dee takes slot 1, bo's old slot, with generation 1: id `"1.1"`. Bo's game still says `[left]` and not `dee`: because the
pool is `checked`, the reference carries the generation of the slot it was made for, and an old id `1.0` now gets a
404. Without `checked`, references are smaller (4 bytes rather than 8), ids are just slot numbers, and only an empty
slot is detected.

Note also `game.board.remove(p.entry)`. A module's items change only through its functions; reaching into its
internals does not load:

```
error: score.mito: DELETE /players/{p}: remove items of game.board with its functions, not `game.board.items.remove()`
```

## Step 8: memory you can see

```
@get("/stats")
fn stats() -> Response<{ players: i64, games: i64, reserved_kb: i64, committed_kb: i64, live_kb: i64 }> {
    return ok({
        players: game.players.count(), games: game.games,
        reserved_kb: stat("reserved_kb"), committed_kb: stat("committed_kb"), live_kb: stat("live_kb")
    });
}
```

```
GET /stats   {"players":3,"games":26,"reserved_kb":172,"committed_kb":52,"live_kb":1}
```

The whole program, room for 1000 players and their board entries, reserves 172 KB. 52 KB is committed (page-sized
pieces where something was written), and about 1 KB is live. `x.count()` and `x.capacity()` read a collection's size
and maximum; `maxlen(s)` a string's.

Options for the running program:

- `--budget MB`: refuse to start if the declared state needs more than this.
- `--soft MB` and `--hard MB`: memory pressure. Above the soft mark, the runtime trims (gives back pages holding
  nothing live) between requests; above the hard mark it trims everything and refuses writes (503) until it is below
  again. Reads keep working.

## Step 9: building for real

```
mitochondria build examples/scoreboard/score.mito -o score.exe
score.exe --port 8080
```

`build` uses clang with optimisation, which makes requests several times faster than the TinyCC build; `--tcc` builds
with TinyCC instead. The executable needs nothing else: the program, the runtime and the HTTP server are one file.

To read what the compiler made:

```
mitochondria emit-c examples/scoreboard/score.mito -o score.c
```

`score.c` is a complete C program: a table of every collection and column with its offset, the runtime
(`runtime/mito.c`), and each endpoint as a C function, with the source lines it came from in a comment above it.

## Step 10: testing your program

A test case is one file in `tests/cases/`: the program, and requests, one per line. `tests/cases/scoreboard.case`:

```
--- program
include examples/scoreboard/score.mito
--- requests
POST /players {"name": "ada"}
POST /players/0.0/games {"points": 120}
GET /board
*20 POST /players/2.0/games {"points": 1}
```

`*20` repeats a request; `@Header: value` adds a header to the next one; `=> name` saves the response's `"token"` as
`$name` for later lines; `clock ms` fixes the time `now()` returns. Record the answers once:

```
set MITO_BLESS=1          (PowerShell: $env:MITO_BLESS=1)
cargo test cases
```

This writes `scoreboard.out`: every answer, then how many bytes each collection has live and the memory totals. From
then on `cargo test` checks that the TinyCC build, the clang build and a clang build with checks for undefined
behaviour all give exactly those answers. `MITO_CASE=scoreboard` runs just one case.

## Step 11: sessions, and more than one thread

A session is a typed input too. In `examples/chat/chat.mito`:

```
@post("/rooms/{room}/messages")
fn post_message(who: Session from header("X-Session"), room: Room, body: string<1..2000>) -> Response<{ seq: i64 }> {
```

`who: Session from header("X-Session")` takes the session token from that header (`token(s)` makes one when a user signs
in) and gives the session item, or answers 401 before the body runs.

A program runs on one thread. For more, a system file runs several programs as **workers**, each on its own thread
with its own memory, sharing nothing (`examples/chat-sharded/chat.mito`):

```
system {
    worker auth  { program "auth.mito";  count 2; mailbox 1024; slot 4096; }
    worker rooms { program "rooms.mito"; count 4; mailbox 1024; slot 4096; }
    entry auth;
    inflight 4096;
    budget 8192;
}
```

Requests arrive at an `auth` worker. A handler passes a request on with `forward rooms[key] { user: who.name };`, to
worker `key % count` of that kind, whose endpoint takes the fields as typed parameters: `forwarded user: string<32>`.
Each worker has a bounded mailbox; if it is full, the sender's changes are undone and the client gets 503. Run it with
`mitochondria serve examples/chat-sharded/chat.mito`.

## Where to go next

- `examples/chat`: a chat server, with sessions, rooms holding their own message lists, and typing indicators that
  expire.
- `DESIGN.md`: the whole design, and what is not built yet.
- `docs/proposals/syntax.md`: why the syntax is what it is.
- `docs/measurements.md`: what was measured, including what did not work.
- `lib/sorted.mito`: a skip list written in the language, as an example of a module.

### Endpoint inputs

| Written | Takes |
|---|---|
| `points: i64`, `n: u8`, `top: i64 in 1..100` | an integer (in its type's range, or the one given) |
| `x: f64`, `on: bool` | a number, a bool |
| `name: string<24>`, `name: string<1..24>` | a string of at most 24 bytes; of 1 to 24 |
| `p: Player` (named in the path) | the item an id names; `p: Player in game.players` says which pool |
| `who: Session from header("X-Session")` | the session a token names |
| `= 10`, `?` (strings only) | the default when absent; null when absent |
| `from query`, `from body`, `from header("H")` | another source than the method's default |
| `forwarded user: string<32>` | a field another worker forwarded |

### Built-in functions

| Function | Gives |
|---|---|
| `int(s)`, `str(x)`, `len(s)` | a number from text, text from a value, a string's length in bytes |
| `min(a, b)`, `max(a, b)` | the smaller, the larger |
| `now()`, `random()` | the time in milliseconds, a random 64-bit number |
| `slot(item)` | an item's slot or position |
| `or(a, b)`, `exists(x)` | `a`, or `b` when `a` is missing; whether `x` is there |
| `token(item)` | a session token for an item of a checked pool with a `secret` field |
| `maxlen(s)` | a string's declared maximum |
| `stat("committed_kb")` and `"reserved_kb"`, `"live_kb"` | memory, as above |
| `worker_index()`, `worker_count("kind")` | in a system: this worker, how many of a kind |
