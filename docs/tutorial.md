# Mitochondria tutorial

This builds one small program, a game scoreboard (`examples/scoreboard/score.mito`), one idea at a time. Every answer
shown is the program's real answer: `tests/cases/scoreboard.case` sends the same requests.

## The idea

You declare all of a program's state up front, with a maximum for everything. The program reserves that address space
once, at start, which costs no memory; a page takes memory only where data is written, and with memory pressure on
(`--soft`, `--hard`) pages that hold nothing live are given back. Reaching a maximum is an ordinary error the program answers, never a crash.

## Setup

Windows, with Rust:

```
git clone https://github.com/benwmaddox/mitochondria
cd mitochondria
cargo build --release
```

The compiler is `target\release\mito.exe` (`mitochondria.exe` beside it is the same program under its long name). It builds with the bundled TinyCC, or with
clang if you install LLVM.

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
  slot is live, and `or(..., fallback)` answers the fallback when it is not.
- `string<24>`: at most 24 bytes.

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
    return ok({ top: [each e in game.board reverse limit top { name: game.players[e.who].name, points: e.points }] });
}
```

`p: Player` in the path turns `/players/0.0/games` into that player, or a 404. The board needs no sort: it is kept in
order, so the top 10 are its last 10 entries.

```
GET /board          {"top":[{"name":"ada","points":400},{"name":"bo","points":340},{"name":"cy","points":90}]}
GET /board?top=2    {"top":[{"name":"ada","points":400},{"name":"bo","points":340}]}
```

## 5. All or nothing

A request that fails is undone completely. `record` counts the game and appends it before checking the score:

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
fn remove_player(p: Player) -> Response<none> {
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

A case in `tests/cases/` is the program and its requests:

```
--- program
include examples/scoreboard/score.mito
--- requests
POST /players {"name": "ada"}
POST /players/0.0/games {"points": 120}
GET /board
```

With the environment variable `MITO_BLESS=1` set, `cargo test cases` records the answers; after that `cargo test`
checks them on every build (TinyCC, clang, and clang with undefined-behaviour checks).

## Next

- `mito build examples/scoreboard/score.mito -o score.exe` makes an optimised executable; `emit-c` writes the C it is made of.
- `examples/chat` adds sessions (`who: Session from header("X-Session")`); `examples/chat-sharded` runs workers on
  several threads.
- [DESIGN.md](../DESIGN.md) has the whole design and what is not built yet.
