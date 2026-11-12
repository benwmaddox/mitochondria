# Reference: commands, options, built-ins and libraries

The design, in order of how a program is written, is [DESIGN.md](../DESIGN.md); [tutorial.md](tutorial.md) builds a program step by step. This page lists
what can be typed: every command, every option of a built executable, every built-in that DESIGN does not describe where it first comes up, and the library modules.
A test (`docs_mention_everything` in src/platform.rs) fails when a built-in, option, command, library module or example is in none of the docs, so this page stays complete.

## Commands

`mito <command> <program> [options]`. A program is one `.mito` file (the entry file; it may `use "other.mito";` more files) or a system of workers (a file holding `system { ... }`).

| Command | What it does |
| --- | --- |
| `check <program>` | reports the problems found, or says the program loads. `--kinds` lists the kind of every local; `--costs` lists each endpoint's work (bytes zeroed, queries, row reads, table writes, text built). |
| `build <program> [-o app.exe] [--tcc] [--cflag X]` | an executable: a server (platform web) or a tool (platform cli). Built with clang `-O3`; `--tcc` builds in milliseconds with TinyCC, unoptimized; `--cflag X` (repeatable) hands X to clang in place of `-O3` (`--cflag -O2 --cflag -mavx2`). |
| `run <program> [--clang] [-- args]` | builds and runs a command-line program (`@main`) at once. What follows `--` is its command line; stdin and stdout are yours. The exit code is the program's. |
| `serve <program> [--clang] [--watch] [executable options]` | builds with TinyCC (or clang) and runs a server at once; `--watch` rebuilds and restarts when a file of the program changes (state starts empty each time). |
| `test <program> [--clang]` | runs the program's `test "name" { ... }` blocks, each on a fresh state with the clock fixed; exit code 1 if one fails. |
| `emit-c <program> [-o file.c]` | the generated C, to read or build yourself (a system: `-o dir`, one file per worker kind and system.c). |
| `fmt <file.mito>... [--check] [--write]` | the formatter: prints one file formatted, `--write` rewrites the files in place, `--check` lists the files that would change and exits 1 if any does. Changes only white space (DESIGN.md, Formatting). |
| `sigs <program>` | a tool: every function's parameter and result kinds as the compiler sees them at its calls. |

The platform (web, cli, test) is the entry file's own: `platform cli;` in it, or what its entry points show. There is no `--platform` switch (it is refused with a message saying so): one entry file, one platform (DESIGN.md, Platforms).

## Executable options (a server)

Given to a built server, or to `serve`. A command-line program (`@main`) has none of these: its flags are its own inputs (DESIGN.md, Command-line programs).

| Option | Meaning |
| --- | --- |
| `--port N` | the port to listen on (8080) |
| `--conns N` | connection threads (16; a system's connection threads) |
| `--workers N` | readers that run requests which only read, at once (Linux; default twice the CPUs the process may use; 1 runs one request at a time) |
| `--budget MB` | one program's memory, 1024 by default |
| `--soft MB`, `--hard MB` | turn on memory pressure: above soft pages are trimmed, above hard writes are refused (503) until below |
| `--data DIR` | where the program's tables are kept (SQLite, `DIR/data.db`; default `./data`) |
| `--db FILE` | the database of another program, for a program whose tables are `extern` |
| `--db-sync normal\|full` | when a commit reaches the disk: `normal` (default) at checkpoints; `full` before the answer, so an answered change survives a power cut |
| `--db-memory MB` | SQLite's memory (64 per connection) |
| `--db-fit refuse\|drop` | what to do with data that no longer fits a changed column: refuse to start (default) or drop it |
| `--cache-mb N` | the answer cache (`cache_get` / `cache_put`), 64 MB by default, 0 for none |
| `--files DIR` | the directory `read_stored` reads (an app's stored files) |
| `--loop-budget N` | steps one request's loops may take (at most 2^50, which the compiler's proofs assume) |
| `--seed N`, `--clock MS` | fix the random generator and the clock, for tests |
| `--timing` | every answer carries `X-Handler-Ns`, the handler's own time in nanoseconds |

## Options of a game

A game (platform game, entry `@frame`) is a built executable that opens a window. It needs SDL3 (3.2 or newer) beside it or on the system's library path (`SDL3.dll`, `libSDL3.so.0`); it is loaded when the game starts and
nothing is linked, so a build needs neither the library nor its headers. A game can also render a fixed number of frames in a window that is not shown, and save one.

| Option | Meaning |
| --- | --- |
| `--size WxH` | the window's size in pixels (1280x720; from 16x16 to 8192x8192) |
| `--frames N` | run N frames of 1/60 s in a window that is not shown, then stop (without it the game runs until the window is closed) |
| `--screenshot FILE.ppm` | write the last frame as a PPM image (implies `--frames 1`) |
| `--hold KEYS` | keys (`w,shift`) held down in every frame, to try a game's controls without a person |
| `--mouse DX,DY` | pointer movement added to every frame |
| `--seed N` | fix the random generator |

## Built-ins described here

DESIGN.md describes most built-ins where they come up (text, JSON, time, crypto, HTML, collections). These are the rest.

| Built-in | What it does |
| --- | --- |
| `abs(x)` | absolute value of an integer or a float |
| `min(a, b)`, `max(a, b)` | the smaller or larger of two numbers: integers give an integer, and a float on either side gives a float |
| `sqrt(x)`, `exp(x)`, `floor(x)`, `sin(x)`, `cos(x)`, `atan2(y, x)`, `pow(x, y)` | the platform's math functions on floats (a negative square root is NaN; angles are in radians); an integer is taken as a float |
| `wrapping_add(a, b)`, `wrapping_sub(a, b)`, `wrapping_mul(a, b)` | two's-complement arithmetic that wraps at 64 bits (the operators `+ - *` fail the request on overflow); for hashes and generators |
| `x as TYPE` | a conversion that keeps the low bits (DESIGN.md, Conversions) |
| `add_years(t, n)` | a time `n` calendar years later (or earlier), the same day and time of day; February 29 becomes February 28 where the year has none |
| `all_word(t)` | whether every character of `t` is a word character (letters, marks, numbers, connector punctuation, as Ruby 3.4's `\p{Word}`) |
| `text_from(bytes, from, count)` | bytes of a state array of `u8` (plain state or an item's) as text copied into the request's text area, range checked |
| `html_text(x);` | in a markup function (`-> html`), writes `x` escaped, as `{x}` does inside an element |
| `stat("committed_kb")`, `stat("live_kb")`, `stat("reserved_kb")` | memory figures of this program, in KB (committed, live, reserved) |
| `worker_index()`, `worker_count()`, `worker_count("kind")` | in a system of workers, which worker this is (0 up), how many workers there are of its kind, and of a named kind; outside a system 0 and 1 |

## Functions of a type

A function whose first parameter is `self: Type` belongs to that type: `fn hurt(self: Player, n: i64) { ... }` declares `hurt` for `Player`, and `p.hurt(3)` calls it. The same name may be declared for any number of types and beside a
function of no type. `self` is an item, an integer (`i64`, or any narrower integer type), a float (`f64`), a `string` or a `bool`, or a plain struct (below); not a nullable value. A collection operation's name (`add`, `remove`,
`count`, `sort` ...) cannot be used (DESIGN.md, Functions of a type).

A function can take a plain struct (a struct that is a field of the state, also nested): `fn move_by(p: Pos, dx: i64) { p.x += dx; }` is called `move_by(world.origin, 2)`, and `fn len2(self: Pos) -> i64` is called `world.hero.pos.len2()`. The parameter is a view of the struct where it lives, not a copy: reads see the state and writes through it change the state, with every range, `logged` rule and read-only check of a write written with the full path. A struct that is also an item type takes either a plain struct or an item (the argument decides). A plain struct cannot be a function's result or a nullable parameter (DESIGN.md, Struct parameters).

## Const groups

`const Limits { MAX_HP: i64 = 100; MAX_NAME = 16; DOUBLE: i64 = Limits.MAX_HP * 2; }` names compile-time constants, used as `Limits.MAX_HP`. The type is optional for integers (a value that does not fit the type is an error); a float needs one (`const Tune { GRAVITY: f64 = 9.8; }`). A value is a number, an earlier constant of any group, and `- + * / % << >> & | ^ ( )` of them, folded when read; overflow is an error naming the constant. A constant is usable in code (also as a `match` arm), in a field's `in lo..hi`, and in a size (`string<Limits.MAX_NAME>`, `u8[Limits.MAX_NAME]`, `pool<Item, Limits.N>`), not yet in an endpoint input's range (`DESIGN.md`, Const groups).

## Generic structs

`struct Stack<T, N: count> { items: T[N]; len: i64 in 0..N; }` declares a struct with parameters: `T` is a type, `N: count` a whole-number literal of at least 1; after the colon a constraint (`struct`, `number`, `int`, `text`) limits a type.
`undo: Stack<i64, 64>;` and `pool<Stack<string<12>, 8>, 4>` use it; each distinct use is its own ordinary struct (`Stack<i64, 64>` in messages), with its own storage and ranges. A function of a generic struct names the parameters in its receiver:
`fn push(self: Stack<T, N>, x: T) { ... }` is called `game.undo.push(3)`, `T` and `N` taken from the receiver's type. A function the program never names is not copied; an error in a copy names the instance. At most 1000 instances, nested at most 64 deep.
Not yet: `table<...>` fields, constant or computed counts, `K: field of T`, generic functions without a receiver (DESIGN.md, Generics).

## Entry points

`@get("/p")`, `@post`, `@put`, `@patch` and `@delete` are endpoints (a function with an HTTP route and typed inputs; `@get` and `@delete` read their inputs from the query, the others from the body, a path
`{param}`, `from header(...)` or `from cookie(...)`). `@socket`, `@socket_message` and `@socket_close` handle WebSockets, `@every(ms)` runs on a clock, `@start` once when the program starts, and
`@main` is a command-line program's one function. `@frame` and `@shader` are a game's: `@frame` is called every frame, `@shader fn name() -> Response<text>` gives one shader's source. A game's frame answers with a line for each pass: `scene@0.25 px=1 pz=2` draws the shader `scene` with those uniforms into an offscreen image a quarter of the window's size, and a later line, `paint`, draws on the window and reads that image as `uImage` (DESIGN.md, Games). A scene shader may declare `layout(location = 0) out vec4 color;` and `layout(location = 1) out vec4 aux;`: it then draws two pictures at once, and the window pass reads the second as `uniform sampler2D uImage1`. A program has the entry points of its platform only.

A game's `@frame` function takes its inputs from the window: `dt: f64 from time` (seconds since the last frame), `w: bool from key("w")` (whether a key is down; a letter, a digit, `space`, `up`, `down`, `left`, `right`, `shift` or `escape`) and `look: f64 from mouse("dx")` (`dx` or `dy`, the pointer's movement since the last frame; reading it captures the pointer).

## Library modules

A module is a `.mito` file in `lib/`, used with an `instance` declaration (DESIGN.md, Modules). The libraries:

| Module | What it is |
| --- | --- |
| `sorted<T, N, key>` | entries kept in order of a key |
| `btree<T, N, key>` | the `sorted` interface built as a B+ tree for speed: items of `T` kept in order of `key`, equal keys in the order added |
| `ordmap<N>` | an ordered map from integer keys to integer values |
| `rowmap<N, W>` | an `ordmap` with a row of `W` numbers per key (`put(key, col, value)`, `get_or`, `add_to`, `range_sum`) |
| `intmap<N>` | a hash map from `i64` keys to `i64` values, up to `N` entries; removing an entry frees its slot at once; putting a new key into a full map is an error (507) |
| `strmap<N>` | a hash map from string keys (up to 64 bytes) to `i64` values, up to `N` entries, the same rules |
| `bag<T, N>` | the smallest module, for tests |
| `queue<T, N>` | a first-in first-out queue of up to `N` struct items: `push()` returns the new item to fill in, `pop()`, `peek()`, `count()`, `clear()`; pushing when full is an error (507), popping or peeking when empty a 404 |
| `ring<T, N>` | a ring buffer of the last `N` struct items: `push()` returns the new newest item (reusing the oldest slot when full), `get(age)` reads from the newest (0), `count()`, `clear()`; reading past the items held is a 404 |
| `set<N>` | a hash set of up to `N` `i64` keys: `add`, `has`, `remove`, `count`, `clear`; adding a new key when full is an error (507) |
| `heap<N>` | a binary min-heap priority queue of up to `N` entries of `i64` priority and `i64` payload: `push(priority, payload)`, `pop_min()`, `peek()`, `peek_priority()`, `count()`; full is 507, empty is 404 |
| `bitset<N>` | `N` bits: `set`, `unset`, `flip`, `test`, `count`, `clear_all`; an index outside 0 to N - 1 is a 400 |
| `grid<W, H>` | a `W` by `H` grid of `i64` cells: `get(x, y)`, `set(x, y, value)`, `fill(value)`, `in_bounds(x, y)`; a position outside the grid is a 400 |

## Examples

| Example | Platform | What it shows |
| --- | --- | --- |
| `examples/scoreboard` | web | the smallest program: state, endpoints, tests |
| `examples/chat` | web | sessions, rooms, typing, a request-rate limit |
| `examples/chat-sharded` | web (a system) | workers on several threads, forwarding between them |
| `examples/todomvc` | web | server-rendered pages, tables in SQLite, the answer cache (53 browser checks) |
| `examples/notes` | web | a multi-file app on tables: accounts with bcrypt, per-user notes, search and tags |
| `examples/jx` | cli | a small jq in three files: positions in the input text, no tree, no allocation |
| `examples/world` | game | an open world to walk through, painted in broad brush strokes by one fragment shader from a seed: terrain, lakes, sky, sun, fog |
