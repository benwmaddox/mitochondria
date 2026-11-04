# Mitochondria tutorial

This tutorial builds one small program, a game scoreboard, and uses it to show each idea in the language once. It
takes about half an hour. The finished program is `examples/scoreboard/`, and `tests/cases/scoreboard.case` runs every
request shown here, so the answers below are the program's real answers.

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

A program is two files: the **state** (`.mito`, what exists) and the **API** (`.api`, the HTTP endpoints that read
and change it). The compiler turns both into one C program, which is built into an executable that serves HTTP.

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

## Step 1: state

`examples/scoreboard/score.mito`:

```
struct Player { name: string<24>; best: i64; entry: ref<board>; }
struct Entry  { who: ref<players>; points: i64; }
struct Game   { who: ref<players>; points: i64; at: i64; }
struct Totals { games: i64; }

global totals: Totals;
global players: pool<Player, 1000> checked;
global board: sorted<Entry, 1000, points>;
global recent: list<Game, 20> drop_oldest;
```

Reading it from the bottom:

- **`list<Game, 20> drop_oldest`** keeps items in the order they arrive, at most 20. `drop_oldest` says what happens
  when it is full: the new item overwrites the oldest, which makes it a ring buffer. The other choice, the default, is
  to fail.
- **`sorted<Entry, 1000, points>`** keeps up to 1000 entries in order of their `points` field. It is not built into
  the language: it is a module from the standard library (`lib/sorted.mito`), written in Mitochondria itself.
- **`pool<Player, 1000> checked`** holds up to 1000 players in slots that never move. A new player takes the lowest
  free slot. `checked` adds a generation number to each slot, so a reference to a player who has left is detected
  even after the slot is reused.
- **`global totals: Totals`** is a plain struct: one value, always there.
- **`ref<players>`** is a reference to a slot of the `players` pool. Reading through it checks that the player is
  still there.
- **`string<24>`** holds at most 24 bytes; **`i64`** is a 64-bit integer. Other scalars are `i8`..`i64`, `u8`..`u64`,
  `f32`, `f64` and `bool`; `T[N]` is a fixed array of exactly N values.

Every collection here is stored as **columns**: all the names together, all the best scores together. A loop that
reads one field touches only that field's memory.

Which collection to use: a **list** for things you append and read in order or from the newest end (messages, logs,
events); a **pool** for things that other data refers to and that come and go in any order (users, sessions, rooms);
a **sorted** collection when you need them in order of a field. Collections can also be fields of an item: a `Room`
can hold `messages: list<Msg, 1000>`, 1000 per room (see `examples/chat`).

## Step 2: endpoints

`examples/scoreboard/score.api` starts with a function and an endpoint:

```
fn player() -> ref<players> { return players[int(param("id"))]; }

endpoint POST /players {
    let name = body("name");
    if len(name) == 0 { fail 400 "name is required"; }
    let p = alloc players;
    p.name = name;
    let e = board.add(0);
    e.who = p;
    p.entry = e;
    respond 201 { "id": slot(p) };
}
```

- **`endpoint METHOD /path/{param}`** handles one kind of request. `param("id")` reads a path parameter,
  `body("name")` a field of the JSON body, `query("top")` a query parameter, `header("X-Session")` a header.
- **`let`** names a value; **`var`** names one you can change.
- **`alloc players`** takes a free slot and returns the new item. `slot(p)` is its slot number.
- **`board.add(0)`** calls the `sorted` module's `add`, which places a new entry with 0 points.
- **`respond CODE { ... }`** ends the request with a JSON answer. **`fail CODE "message"`** ends it with an error,
  `{"error":"message"}`.
- `players[k]` reads slot k. If no player is there, the request fails with 404.

State is read and written directly: `p.name = name` writes the player's name into the pool.

## Step 3: check, then run

```
mitochondria check examples/scoreboard/score.mito examples/scoreboard/score.api
```

`check` reports the first problem, or prints `score: loads`. Problems are found here, before the program ever runs:
misspell a field (`p.nmae = name;`) and you get

```
error: score.api: POST /players: no field `nmae`
```

Then run it:

```
mitochondria serve examples/scoreboard/score.mito examples/scoreboard/score.api --port 8080
```

`serve` builds the program with TinyCC and runs it at once. From another terminal (PowerShell 7):

```
curl.exe -X POST http://127.0.0.1:8080/players -d '{"name": "ada"}'
{"id":0}
curl.exe -X POST http://127.0.0.1:8080/players/0/games -d '{"points": 120}'
{"best":120}
curl.exe http://127.0.0.1:8080/board
{"top":[{"name":"ada","points":120}]}
```

The rest of this tutorial shows requests in the short form the test cases use,
`POST /players {"name": "ada"}`, each followed by its answer.

## Step 4: loops, and data kept in order

Recording a game:

```
endpoint POST /players/{id}/games {
    let p = player();
    let points = int(body("points"));
    totals.games = totals.games + 1;
    let g = alloc recent;
    g.who = p;
    g.points = points;
    g.at = now();
    if points > p.best { p.best = points; board.set_key(p.entry, points); }
    // checked last on purpose: everything above is undone when this fails
    if points > 1000000 { fail 422 "not a believable score"; }
    respond 200 { "best": p.best };
}
```

`alloc recent` appends to the list (once it holds 20, it overwrites the oldest). `board.set_key` moves the player's
entry to its new place in the order; the key of a sorted item can only be changed this way, so the order is never
wrong.

Reading them back:

```
endpoint GET /board {
    let n = min(int(or(query("top"), "10")), 100);
    respond 200 { "top": [each e in board reverse limit n { "name": e.who.name, "points": e.points }] };
}

endpoint GET /recent {
    respond 200 { "games": [each g in recent reverse { "name": or(g.who.name, "[left]"), "points": g.points }] };
}
```

**`each x in collection`** visits the items. Clauses narrow it: `reverse` (newest first for a list, highest first for
a sorted collection), `where cond`, `limit n`, and for a sorted collection `from key`. Inside `[ ... ]` in a
`respond`, it builds a JSON array. `each i in 0..10` counts. Every loop is bounded by a capacity or a range, so every
loop ends.

There is no `order by`. The board is already in order because it is kept in order; reading the top 10 is reading the
last 10 entries. `or(a, b)` gives `b` when `a` is missing: here, a query parameter not given, or a player who left.

After three players and five games:

```
GET /board          {"top":[{"name":"ada","points":400},{"name":"bo","points":340},{"name":"cy","points":90}]}
GET /board?top=2    {"top":[{"name":"ada","points":400},{"name":"bo","points":340}]}
GET /recent         {"games":[{"name":"ada","points":50},{"name":"ada","points":400},{"name":"cy","points":90},
                              {"name":"bo","points":340},{"name":"ada","points":120}]}
```

## Step 5: every request is all or nothing

Every endpoint is **atomic**: if it fails, every change it made is undone, as if it never ran. The runtime remembers
the old bytes of each write during a request and puts them back on failure. Look at the game endpoint again: it counts
the game, appends it to `recent` and may move the board, and only then checks the score.

```
GET /stats                       {"players":3,"games":5,...}
POST /players/1/games {"points": 5000000}
                                 422 {"error":"not a believable score"}
GET /stats                       {"players":3,"games":5,...}
GET /board                       {"top":[{"name":"ada","points":400},{"name":"bo","points":340},{"name":"cy","points":90}]}
```

`games` is still 5, the game is not in `recent`, and bo is not at the top. You never write clean-up code for a
failure.

## Step 6: limits are errors, not crashes

Everything has a declared maximum, and going past one is an ordinary, typed error:

```
POST /players {"name": "a name much longer than twenty-four bytes"}
                          413 {"error":"string of 41 bytes does not fit string<24>"}
POST /players/7/games {"points": 1}
                          404 {"error":"slot 7 is not live"}
POST /players/0/games {"points": "lots"}
                          400 {"error":"`lots` is not a number"}
```

A full pool answers 503, for example `{"error":"capacity: players is full (1000 of 1000)"}`. The kinds of failure:
`Full` (503), a value too long (413), a slot not in use (404), a stale reference (401 for a session, else 404), and
your own `fail`.

## Step 7: references, and leaving

```
endpoint DELETE /players/{id} {
    let p = player();
    board.remove(p.entry);
    free p;
    respond 200 { "ok": true };
}
```

`free p` frees the slot when the request succeeds. Bo's games are still in `recent`, holding a reference to a player
who has gone:

```
DELETE /players/1   {"ok":true}
GET /recent         {"games":[...,{"name":"[left]","points":340},...]}
POST /players {"name": "dee"}
                    {"id":1}
GET /recent         {"games":[{"name":"dee","points":10},...,{"name":"[left]","points":340},...]}
```

Dee takes slot 1, bo's old slot, yet bo's game still says `[left]` and not `dee`: because the pool is `checked`, the
reference carries the generation of the slot it was made for, and the reused slot has a new one. Without `checked`,
references are smaller (4 bytes rather than 8) and only detect a slot that is empty.

Note also `board.remove(p.entry)` rather than `free p.entry`. A module's items change only through its functions;
`free p.entry` does not load:

```
error: score.api: DELETE /players/{id}: remove items of board with its functions, not `free`
```

## Step 8: memory you can see

```
endpoint GET /stats {
    respond 200 {
        "players": count(players), "games": totals.games,
        "reserved_kb": stat("reserved_kb"), "committed_kb": stat("committed_kb"), "live_kb": stat("live_kb")
    };
}
```

```
GET /stats   {"players":3,"games":26,"reserved_kb":172,"committed_kb":52,"live_kb":1}
```

The whole program, room for 1000 players and their board entries, reserves 172 KB. 52 KB is committed (page-sized
pieces where something was written), and about 1 KB is live. `count(x)` and `capacity(x)` read a collection's size
and maximum; `maxlen(s)` a string's.

Options for the running program:

- `--budget MB`: refuse to start if the declared state needs more than this.
- `--soft MB` and `--hard MB`: memory pressure. Above the soft mark, the runtime trims (gives back pages holding
  nothing live) between requests; above the hard mark it trims everything and refuses writes (503) until it is below
  again. Reads keep working.

## Step 9: building for real

```
mitochondria build examples/scoreboard/score.mito examples/scoreboard/score.api -o score.exe
score.exe --port 8080
```

`build` uses clang with optimisation, which makes requests several times faster than the TinyCC build; `--tcc` builds
with TinyCC instead. The executable needs nothing else: the program, the runtime and the HTTP server are one file.

To read what the compiler made:

```
mitochondria emit-c examples/scoreboard/score.mito examples/scoreboard/score.api -o score.c
```

`score.c` is a complete C program: a table of every collection and column with its offset, the runtime
(`runtime/mito.c`), and each endpoint as a C function, with the source lines it came from in a comment above it.

## Step 10: testing your program

A test case is one file in `tests/cases/`: the state, the API, and requests, one per line.
`tests/cases/scoreboard.case`:

```
--- state
include examples/scoreboard/score.mito
--- api
include examples/scoreboard/score.api
--- requests
POST /players {"name": "ada"}
POST /players/0/games {"points": 120}
GET /board
*20 POST /players/2/games {"points": 1}
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

## Step 11: more than one thread

A program runs on one thread. For more, a `.system` file runs several programs as **workers**, each on its own
thread with its own memory, sharing nothing:

```
worker auth  { state "auth.mito";  api "auth.api";  count 2; mailbox 1024; slot 4096; }
worker rooms { state "rooms.mito"; api "rooms.api"; count 4; mailbox 1024; slot 4096; }
entry auth;
inflight 4096;
budget 8192;
```

Requests arrive at an `auth` worker. A handler passes a request on with `forward rooms[key] { "field": value };`, to
worker `key % count` of that kind, which reads the fields with `fwd("field")`. Each worker has a bounded mailbox; if
it is full, the sender's changes are undone and the client gets 503. See `examples/chat-sharded`, and run it with
`mitochondria serve examples/chat-sharded/chat.system`.

## Where to go next

- `examples/chat`: a chat server, with sessions (`auth()` and `token()`), rooms holding their own message lists, and
  typing indicators that expire.
- `DESIGN.md`: the whole design, and what is not built yet.
- `docs/measurements.md`: what was measured, including what did not work.
- `lib/sorted.mito`: a skip list written in the language, as an example of a module.

### Built-in functions

| Function | Gives |
|---|---|
| `body("f")`, `param("p")`, `query("q")`, `header("H")` | a field of the request, as text |
| `int(x)`, `str(x)`, `len(s)` | a number from text, text from a value, a string's length in bytes |
| `min(a, b)`, `max(a, b)` | the smaller, the larger |
| `now()`, `random()` | the time in milliseconds, a random 64-bit number |
| `count(c)`, `capacity(c)`, `maxlen(s)` | items in a collection, its maximum, a string's maximum |
| `slot(item)` | an item's slot or position |
| `or(a, b)`, `exists(x)` | `a`, or `b` when `a` is missing; whether `x` is there |
| `try_alloc c` | like `alloc c`, but null when full instead of failing |
| `token(item)`, `auth(pool)` | a session token for a checked pool's item; the item a request's `X-Session` names |
| `stat("committed_kb")` and `"reserved_kb"`, `"live_kb"` | memory, as above |
| `worker_index()`, `worker_count("kind")`, `fwd("f")` | in a system: this worker, how many of a kind, a forwarded field |
