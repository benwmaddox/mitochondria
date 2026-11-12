# Mitochondria tutorial

This builds one small program, a game scoreboard (`examples/scoreboard/score.mito`), one idea at a time. Every answer
shown is the program's real answer: `tests/cases/scoreboard.case` sends the same requests.

## The idea

You declare all of a program's state up front, with a maximum for everything. The program reserves that address space
once, at start, which costs no memory; a page takes memory only where data is written, and with memory pressure on
(`--soft`, `--hard`) pages that hold nothing live are given back. Reaching a maximum is an ordinary error the program answers, never a crash.

## Setup

Windows or Linux, with Rust:

```
git clone https://github.com/benwmaddox/mitochondria
cd mitochondria
cargo build --release
```

The compiler is `target/release/mito` (`mito.exe` on Windows; `mitochondria` beside it is the same program under its long name).
On Windows it builds with the bundled TinyCC, or with clang if you install LLVM; on Linux with the system's clang.

## 1. The state

```
struct Player { name: string<24>; best: i64; entry: i64; } // entry: its slot in game.board
struct Entry  { who: i64; points: i64; }                 // who: the player's slot
struct Played { who: i64; points: i64; at: i64; }
struct Row    { name: string<24>; points: i64; }

struct Game {
    games: i64;
    players: pool<Player, 1000> checked;
    board: sorted<Entry, 1000, points>;
    recent: list<Played, 20> drop_oldest;
}
state game: Game;
```

A program has one state, a struct holding everything; every path starts at its name (`game.players`).

- `pool<Player, 1000>`: up to 1000 players in slots that never move. `checked` adds a generation per slot, so an id
  from outside (`"1.0"`: slot 1, generation 0) that names a reused slot is refused.
- `list<Played, 20> drop_oldest`: the last 20 games, in order; the 21st overwrites the oldest.
- `sorted<Entry, 1000, points>`: entries kept in order of `points` (a library module, `lib/sorted.mito`).
- `who: i64`: one item names another by its slot (`slot(p)`), read back with `game.players[e.who]`; that read checks the
  slot is live, and `or(..., fallback)` answers the fallback when it is not. A field `best: Player?` would hold the item's id
  instead (slot and generation): it reads back as null once that player is gone, even if another took the slot.
- `string<24>`: at most 24 bytes of UTF-8 text (`utf8<24>` is the same; `ascii<24>` holds bytes 0..127 and can be written by
  index). Text a program builds goes in fields like this one, appended in place with `+=`; text needed only while a request runs
  goes in a `request` struct (`request work: Work;`), emptied before each request.

That is the whole memory of the program: 172 KB reserved, decided here.

## 2. An endpoint

```
@post("/players")
fn add_player(name: string<1..24>) -> Response<{ id: Player }> {
    let p = game.players.add { name: name };
    let e = game.board.add(0) { who: slot(p) };
    p.entry = slot(e);
    return created({ id: p });
}
```

- An endpoint is a function with an attribute. Its parameters are what the request must carry, checked before the body
  runs: here a name of 1 to 24 bytes from the JSON body.
- `game.players.add { name: name }` takes a free slot and sets fields. Collections are used through their functions:
  `add`, `try_add`, `remove`, `count`, `capacity`.
- `Response<{ id: Player }>` is the answer's shape; the compiler checks every answer against it. An item is written as
  its id, `"slot.generation"`.

## 3. Check and run

```
mito check examples/scoreboard/score.mito
mito serve examples/scoreboard/score.mito --port 8080
```

`check` finds problems before anything runs (a misspelt field gives ``no field `nmae` (did you mean `name`?)``, with the
line and a caret under the place); `serve` builds and runs it at once. With `--watch`, `serve` rebuilds and restarts each
time you save (a fraction of a second; the state starts empty again), and a program that does not compile is reported while the
last good build keeps serving. Then:

```
curl.exe -X POST http://127.0.0.1:8080/players -d '{"name": "ada"}'
{"id":"0.0"}
```

## 4. Items by id, and order kept in the data

```
@post("/players/{p}/games")
fn record(p: Player, points: i64) -> Response<{ best: i64 }> {
    game.games += 1;
    game.recent.add { who: slot(p), points: points, at: now() };
    if points > p.best { p.best = points; game.board.set_key(game.board.get(p.entry), points); }
    if points > 1000000 { fail 422 "not a believable score"; }
    return ok({ best: p.best });
}

@get("/board")
fn board(top: i64 in 1..100 = 10) -> Response<{ top: [Row] }> {
    return ok({ top: [foreach e in game.board reverse limit top { name: game.players[e.who].name, points: e.points }] });
}
```

`p: Player` in the path turns `/players/0.0/games` into that player, or a 404. The board needs no sort: it is kept in
order, so the top 10 are its last 10 entries.

```
GET /board          {"top":[{"name":"ada","points":400},{"name":"bo","points":340},{"name":"cy","points":90}]}
GET /board?top=2    {"top":[{"name":"ada","points":400},{"name":"bo","points":340}]}
```

## 5. All or nothing

State is unlogged by default: a request that fails leaves what it wrote, which is the cheap behavior and the right one when a program checks before it writes. Declare the state `logged` (`state game: Game logged;`) and a failed request is undone completely; this program does, because `record` counts the game and appends it before checking the score:

```
GET /stats                                 {"players":3,"games":5,...}
POST /players/1.0/games {"points": 5000000}  422 {"error":"not a believable score"}
GET /stats                                 {"players":3,"games":5,...}
```

`games` is still 5, and the game is not in `recent`.

## 6. Limits are answers

```
POST /players {}                                           400 {"error":"`name` is required"}
POST /players {"name": "a name much longer than twenty-four bytes"}
                                                           413 {"error":"`name`: 41 bytes, at most 24"}
POST /players/0.0/games {"points": "lots"}                 400 {"error":"`points`: expected an integer"}
POST /players/7.0/games {"points": 1}                      404 {"error":"`p`: no 7.0 in game.players"}
```

A full pool answers 503. Nothing grows past what was declared, so nothing runs out unexpectedly.

## 7. Leaving, and reused slots

```
@delete("/players/{p}")
fn remove_player(p: Player) -> Response<null> {
    game.board.remove(game.board.get(p.entry));
    game.players.remove(p);
    return no_content();
}
```

```
DELETE /players/1.0                      204
POST /players {"name": "dee"}            201 {"id":"1.1"}
POST /players/1.1/games {"points": 10}   200 {"best":10}
GET /recent                              {"games":[{"name":"dee","points":10},...,{"name":"dee","points":340},...]}
```

Dee reuses bo's slot 1 with a new generation (`"1.1"`), so the old id `1.0` gets a 404: the generation tells them apart.
But a slot stored in state carries no generation, so bo's old game now shows dee's name. Until the language has a way to
hold a generation-checked id in state (planned, not designed yet), copy what must survive the item (the name) into the
record, or clear stored slots when their item goes.

## 8. Memory you can see

```
GET /stats   {"players":3,"games":26,"reserved_kb":172,"committed_kb":172,"live_kb":1}
```

Room for 1000 players is reserved (172 KB); without pressure marks it is all committed at start (commit charge; pages take
memory only once written); about 1 KB is live. With `--soft MB` and `--hard MB` pages are committed only where data is written, and the program gives back pages that hold nothing live when committed memory passes the soft
mark, and refuses writes (503) above the hard one.

## 9. Tests

Tests live in the program, written like functions: `test "what it checks" { ... }` returns `true` (passed), `false`, or a
text saying what went wrong (an empty text passes). Inside, `get`, `post`, `put`, `patch` and `delete` make requests (a path, then
the body's fields and the headers, each as `{ name: value }`) and answer `.status` and `.body`; `field(text, "name")` reads a field
of a JSON answer; the state can be read directly, and changes only through requests:

```
test "an unbelievable score changes nothing" {
    let r = post("/players", { name: "ann" });
    let g = post("/players/" + field(r.body, "id") + "/games", { points: 2000000 });
    if g.status != 422 { return "expected 422, got " + str(g.status); }
    return game.games == 0 && game.recent.count() == 0;
}
```

`mito test examples/scoreboard/score.mito` runs each test on an empty state (the clock fixed at 2026-01-01) and prints the
line of the `return` that failed one, or of the statement whose error ended it. Tests are never part of a built or served program.

The compiler's own tests record whole conversations: a case in `tests/cases/` is the program and its requests:

```
--- program
include examples/scoreboard/score.mito
--- requests
POST /players {"name": "ada"}
POST /players/0.0/games {"points": 120}
GET /board
```

With the environment variable `MITO_BLESS=1` set, `cargo test cases` records the answers; after that `cargo test`
checks them on every build (TinyCC, clang, and clang with undefined-behavior checks).

## 10. A command-line program

The same language makes tools. A program whose entry is `@main` is run from a command line: its arguments, what is piped in and its flags are its inputs, and its answer is written to standard output.
This is `examples/jx`, a small jq, cut to its core:

```
platform cli;
struct J { n: i64; }
state j: J;

@main
fn main(path: ascii<1..100> from arg(0), doc: utf8<1000000> from stdin, c: bool = false) -> Response<text> {
    return ok(path + " of " + doc);
}
```

```
echo '{"a":1}' | mito run tool.mito -- '.a' -c
```

`from arg(0)` is the first argument, `from stdin` what was piped in (read only if the program asks for it), and any other input is a flag: `-c`, `--name value` or `--name=value`. Inputs are checked as an endpoint's are:
a missing or out-of-range one is an error on standard error and exit code 1, as is an answer with a status of 400 or more (`return status(400, "what went wrong")`). `mito build tool.mito -o tool` makes a standalone executable.
`platform cli;` says what the program is for; without it, `@main` shows it. A command-line program has no endpoints and no tables, and the compiler says so if it is given one.

## 11. Numbers

Integers are `i64` and floats `f64` in code. Narrower types are for storage and for values that must stay in a range:

```
var h: u32 = 2166136261;          // tested against 0..4294967295 now, and on every assignment
h = ((h ^ b) * 16777619) as u32;  // `as u32` keeps the low 32 bits; the compiler then knows the range and drops the test
let y: f32 = x;                   // rounded to a float
```

`as` converts between widths (`u8`, `u16`, `u32`, `i8`, `i16`, `i32`, `i64`, `f64`) and always succeeds: it keeps the low bits, and to a signed type the top bit is the sign (`255 as i8` is -1). A `let`, `var`,
parameter or result with a sized type is a promise: a value outside it fails the request (400), naming it, unless the compiler can prove it inside. `wrapping_add`, `wrapping_sub` and `wrapping_mul` wrap at 64 bits
where the operators `+ - *` fail on overflow. `u64` is stored in fields but is not yet a value in code.

## Next

- `mito build examples/scoreboard/score.mito -o score.exe` makes an optimized executable; `emit-c` writes the C it is made of.
- [reference.md](reference.md) lists every command, option, built-in and library module; `examples/notes` is a multi-file app on tables and `examples/jx` a command-line tool.
- `examples/chat` adds sessions (`who: Session from header("X-Session")`); `examples/chat-sharded` runs workers on
  several threads.
- [DESIGN.md](../DESIGN.md) has the whole design and what is not built yet.
