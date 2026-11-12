# Mitochondria tutorial

This builds one small program, a game scoreboard (`examples/scoreboard/score.mito`), one idea at a time. Every answer
shown is the program's real answer: `tests/cases/scoreboard.case` sends the same requests (sections 12 to 16 use small programs of their own and name their case files).

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

## 12. Functions of a type

A function whose first parameter is `self` belongs to that type, and is called with a dot (`tests/cases/tutorial_more_receivers.case` sends these requests):

```
struct Player { name: string<16>; hp: i64; }
struct Monster { hp: i64; rage: i64; }
struct World { players: pool<Player, 8>; monsters: pool<Monster, 8>; }
state world: World;

fn hurt(self: Player, n: i64) { self.hp -= n; }
fn hurt(self: Monster, n: i64) { self.hp -= n * 2; self.rage += 1; }
fn alive(self: Player) -> bool { return self.hp > 0; }
fn double(self: i64) -> i64 { return self * 2; }
fn shout(self: string) -> text { return self + "!"; }
```

An endpoint calls them as `let p = world.players[id]; p.hurt(n); ... p.alive()`, and `m.hurt(n)` for a monster. The same name can be declared for any number of types: the type of what is before the dot picks the function, at compile time, so the call costs nothing. A number or a string can be `self` too: `x.double()` and `"hey".shout()`.

```
POST /players {"name": "ada"}              201 {"id":0}
POST /monsters                             201 {"id":0}
POST /players/0/hurt {"n": 3}              200 {"hp":7,"alive":true}
POST /monsters/0/hurt {"n": 3}             200 {"hp":4,"rage":1}
POST /players/0/hurt {"n": 9}              200 {"hp":-2,"alive":false}
GET /words/21                              200 {"twice":42,"loud":"hey!"}
```

The monster took twice the damage and got angrier: `hurt` is one name with one rule per type. The name of a collection operation (`add`, `remove`, `count`, ...) can be used this way too: on a monster it is your function, on a pool it is still the pool's operation.

## 13. Struct parameters

A struct that is a field of the state, plain, not in a pool, can be passed to a function too (`tests/cases/tutorial_more_params.case`):

```
struct Pos { x: i64; y: i64; }
struct Hero { name: string<16>; pos: Pos; }
struct World { origin: Pos; hero: Hero; }
state world: World;

fn move_by(p: Pos, dx: i64, dy: i64) { p.x += dx; p.y += dy; }
fn len2(self: Pos) -> i64 { return self.x * self.x + self.y * self.y; }
```

`move_by(world.origin, 1, 2)` and `move_by(world.hero.pos, 3, 4)` are two calls of one function. What is passed is a view of where that struct lives, as an item is a view of its slot: nothing is copied, a read sees the state and a write through `p` changes it, exactly as `world.origin.x += dx` would. A function of a type works on a nested struct the same way: `world.hero.pos.len2()`.

```
POST /origin {"dx": 1, "dy": 2}   200 {"x":1,"y":2}
POST /origin {"dx": 1, "dy": 2}   200 {"x":2,"y":4}
POST /hero {"dx": 3, "dy": 4}     200 {"x":3,"y":4,"len2":25}
POST /hero {"dx": 3, "dy": 4}     200 {"x":6,"y":8,"len2":100}
```

Not allowed yet: a function that returns such a struct (`fn copy_of(p: Pos) -> Pos` is refused, with ``returns `Pos`, a struct that is part of the state: a function can take it as a parameter but not return it (return its fields, or an item)``), and a parameter that may be null (`p: Pos?`: ``a plain struct is part of the state, it cannot be null``). A struct that is also an item of a pool can be passed either way: the same function takes the plain one or an item.

## 14. Constants

A `const` group names values fixed when the program is compiled, and a name can be used wherever a number is written (`tests/cases/tutorial_more_consts.case`):

```
const Limits { MAX_HP: i64 = 100; MAX_NAME = 8; SLOTS = 4; }
const Key { W = 119; S = 115; }

struct Unit { name: string<Limits.MAX_NAME>; hp: i64 in 0..Limits.MAX_HP; tag: i64[Limits.SLOTS]; }
struct World { units: pool<Unit, Limits.SLOTS>; }
state world: World;
```

In the program, `u.hp = Limits.MAX_HP`, a loop `foreach i in 0..Limits.SLOTS - 1 { u.tag[i] = v + i; }` (both ends included, so it fits the array exactly), and match arms: `match c { Key.W => { d = 1; } Key.S => { d = 2; } else => { d = 0; } }`.
A constant may use earlier ones (`DOUBLE: i64 = Limits.MAX_HP * 2`); a float constant needs its type (`GRAVITY: f64 = 9.8`).

```
POST /units {"name": "ada"}                       201 {"id":0,"hp":100}
POST /units/0/tag {"v": 10}                       200 {"sum":46}
GET /key/119                                      200 {"dir":1}
GET /key/115                                      200 {"dir":2}
GET /key/7                                        200 {"dir":0}
POST /units {"name": "a name too long"}           413 {"error":"`name`: 15 bytes, at most 8"}
```

The tags are 10, 11, 12 and 13, which add to 46. Change `MAX_NAME` once and the state's size and the endpoint's limit change together.

## 15. Generic structs

A struct can take a type and a size, and be used more than once (`tests/cases/tutorial_more_generics.case`):

```
struct Stack<T, N: count> {
    items: T[N];
    len: i64 in 0..N;
}
struct Game { scores: Stack<i64, 3>; names: Stack<string<12>, 2>; }
state game: Game;

fn push(self: Stack<T, N>, x: T) {
    if self.len == N { fail 409 "stack is full"; }
    self.items[self.len] = x;
    self.len += 1;
}
fn room(self: Stack<T, N>) -> i64 { return N - self.len; }
```

`T` is any type and `N: count` a whole number of at least 1. Each use is its own ordinary struct with its own storage and its own ranges (`len` is `0..3` in `scores` and `0..2` in `names`); the functions are functions of a type (section 12), and `game.scores.push(40)` checks `40` against `i64` and `game.names.push(name)` the name against `string<12>`. The program's endpoints call `game.scores.push(points)` and `game.scores.room()`, and the same for `names`:

```
POST /scores {"points": 40}   200 {"room":2}
POST /scores {"points": 50}   200 {"room":1}
POST /names {"name": "ada"}   200 {"room":1}
POST /scores {"points": 60}   200 {"room":0}
POST /scores {"points": 70}   409 {"error":"stack is full",...}
POST /names {"name": "bo"}    200 {"room":0}
POST /names {"name": "cy"}    409 {"error":"stack is full",...}
```

Limits today (DESIGN.md, "Generics"): the count must be a written number, not a constant or an expression; a field of type `table<...>` cannot use a parameter; a function of a generic type needs its `self` (there are no free generic functions); it cannot return `T` when `T` is text or a struct, as no function can return those by value; a function the program calls is copied for every use of its struct; there are at most 1000 uses, nested at most 64 deep, so a type that contains itself is an error. A library module (`sorted<...>`, `queue<...>`) is the older way to do the same job, and is being replaced by generic structs one at a time.

## 16. The standard library

A few plain files come with the compiler. `use "std/math.mito";` on a line of its own puts one in the program; they are all functions of a type, so their names do not clash with yours (the cases include the files, since a case file has no place to look beside; `tests/cases/tutorial_more_std.case`):

```
use "std/math.mito";
use "std/random.mito";
use "std/vec.mito";

struct Rng { state: i64; }                    // the generator's state is yours, in your state
struct Ship { pos: Vec3; vel: Vec3; }
struct Game { rng: Rng; ship: Ship; }
state game: Game;
```

- `std/math.mito`: `x.clamp(lo, hi)`, `a.lerp(b, t)`, `x.smoothstep(e0, e1)` and more, and the constants `Math.PI`, `Math.TAU`, `Math.E`. With `let x = n * 0.25;` and `let lo = 10.0;`, an endpoint answers `x.clamp(0.0, 1.0)`, `lo.lerp(20.0, x)`, `x.smoothstep(0.0, 1.0)` and `Math.PI`.
- `std/random.mito`: a deterministic generator, the same stream for a seed everywhere (not for secrets). You declare `struct Rng { state: i64; }` and keep it in your state: `game.rng.seed(42)`, then `game.rng.range(1, 6)` (both ends included), `game.rng.unit()` (a float from 0 up to 1), `game.rng.chance(0.25)`.
- `std/vec.mito`: `Vec2` and `Vec3` are plain structs nested in your state and changed in place, section 13's views: `game.ship.pos.set(1.0, 2.0, 3.0)`, `game.ship.pos.plus(game.ship.vel)`, `game.ship.vel.length()`.

```
GET /math/1    200 {"clamped":0.25,"mid":12.5,"smooth":0.15625,"pi":3.141592653589793}
GET /math/2    200 {"clamped":0.5,"mid":15,"smooth":0.5,"pi":3.141592653589793}
GET /math/6    200 {"clamped":1,"mid":25,"smooth":1,"pi":3.141592653589793}
POST /roll/42  200 {"a":1,"b":2,"c":2}
POST /fly      200 {"x":1,"y":5,"z":7,"speed":5}
```

`/roll/42` seeds the generator with 42 and rolls three dice. The ship starts at (1, 2, 3), moves by (0, 3, 4) and the velocity's length is 5. Every function and constant is in [reference.md](reference.md).

## 17. A tool and a game

A tool is one `@main` function; its inputs are the command line (section 10 explains them). In `tool.mito`:

```
platform cli;
struct T { n: i64; }
state t: T;

@main
fn main(who: ascii<1..40> from arg(0), doc: utf8<1000000> from stdin, shout: bool = false) -> Response<text> {
    let line = who + " sent " + str(len(doc)) + " bytes";
    if shout { return ok(upper(line)); }
    return ok(line);
}
```

```
echo -n hello | mito run tool.mito -- ada            ada sent 5 bytes
echo -n hello | mito run tool.mito -- ada --shout    ADA SENT 5 BYTES
echo -n hello | mito run tool.mito                   {"error":"`who` is required"}   (on standard error, exit code 1)
```

A game is a program with `platform game;`, an `@frame` function that is called every frame and answers what to draw, and `@shader` functions that give the shaders' sources. `examples/world` is built this way; here is its skeleton, shortened (its README shows the whole program, and how to build and run it with SDL3 installed):

```
@shader
fn paint() -> Response<text> { return ok(file("paint.frag")); }

@frame
fn frame(dt: f64 from time, forward: bool from key("w"), look_x: f64 from mouse("dx")) -> Response<text> {
    ...
    return ok("scene@0.25 px=" + str(world.player.x) + " ..." + "\npaint");   // a line for each pass
}
```

The inputs are what the window saw (`from time`, `from key("w")`, `from mouse("dx")`), the state and rules are ordinary Mitochondria, and the picture is made by the fragment shaders on the GPU. `mito build examples/world/world.mito -o world` makes the executable; `world --frames 1 --screenshot frame.ppm` renders one frame without a window.

## Next

- `mito build examples/scoreboard/score.mito -o score.exe` makes an optimized executable; `emit-c` writes the C it is made of.
- [reference.md](reference.md) lists every command, option, built-in and library module; `examples/notes` is a multi-file app on tables and `examples/jx` a command-line tool.
- `examples/world` is a game: [its README](../examples/world/README.md) shows the window, the shaders and the frame. `examples/todomvc` renders HTML pages from tables in SQLite.
- `examples/chat` adds sessions (`who: Session from header("X-Session")`); `examples/chat-sharded` runs workers on
  several threads.
- [DESIGN.md](../DESIGN.md) has the whole design and what is not built yet.
