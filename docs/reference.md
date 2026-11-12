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
| `tan(x)`, `asin(x)`, `acos(x)`, `atan(x)`, `sinh(x)`, `cosh(x)`, `tanh(x)`, `ln(x)`, `log2(x)`, `log10(x)`, `ceil(x)`, `round(x)`, `trunc(x)`, `cbrt(x)`, `hypot(x, y)`, `fmod(x, y)` | more of the same: `ln` is the natural logarithm (`log` prints for debugging); `round` goes half away from zero; `trunc` drops the fraction; `hypot` is `sqrt(x*x + y*y)` without overflow; `fmod` is the remainder with the sign of `x`. Edges are the C library's: `asin`/`acos` outside -1..1, `ln`/`log2`/`log10` of a negative number and `fmod(x, 0)` are NaN, the logarithms of 0 are minus infinity; NaN and infinity are all written as `null` in JSON (so a NaN and an infinity cannot be told apart there; compare in code). Last digits of the transcendental functions differ between math libraries (the platform's), so round before comparing. The standard library adds more (below) |
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

**Struct values.** A struct can be a local, a copy of where it came from: `let c = world.pos;`, `var v = Vec3 { x: 1.0, y: 0.0, z: 0.0 };`, `let z = Vec3 {};` (all zero), `Vec3 { ..base, y: 5.0 }` (a copy of `base` with `y` changed). A literal names its fields, in any order, and gives every one (except `{}` and the `..base` form); it is the whole value of `let`, `var` or `=`.
`v = other;` and `world.pos = v;` copy whole structs; `v.x += 1.0;` writes a field of a `var` local. The copy holds numbers, nested structs, fixed arrays and sized text (a range-declared field keeps its range), at most 256 bytes, no collection or stored id; reads and logging treat a copy out of the state as a read and a copy into it as the writes it is. A local passes to a function that takes the struct, and the function's writes change the local.
Not yet: returning a struct, taking one by copy, comparing, answering with one, a local inside a recursive function (each says so; "Struct values" in DESIGN.md and docs/design/struct-values.md).

## Const groups

`const Limits { MAX_HP: i64 = 100; MAX_NAME = 16; DOUBLE: i64 = Limits.MAX_HP * 2; }` names compile-time constants, used as `Limits.MAX_HP`. The type is optional for integers (a value that does not fit the type is an error); a float needs one (`const Tune { GRAVITY: f64 = 9.8; }`). A value is a number, an earlier constant of any group, and `- + * / % << >> & | ^ ( )` of them, folded when read; overflow is an error naming the constant. A constant is usable in code (also as a `match` arm), in a field's `in lo..hi`, and in a size (`string<Limits.MAX_NAME>`, `u8[Limits.MAX_NAME]`, `pool<Item, Limits.N>`), not yet in an endpoint input's range (`DESIGN.md`, Const groups).

## Generic structs

`struct Stack<T, N: count> { items: T[N]; len: i64 in 0..N; }` declares a struct with parameters: `T` is a type, `N: count` a whole-number literal of at least 1; after the colon a constraint (`struct`, `number`, `int`, `text`) limits a type; `key: field of T` names an integer field of the struct `T` (`Sorted<Entry, 8, score>`), read as `self.items[i].key` and typed `T.key`.
`undo: Stack<i64, 64>;` and `pool<Stack<string<12>, 8>, 4>` use it; each distinct use is its own ordinary struct (`Stack<i64, 64>` in messages), with its own storage and ranges. A function of a generic struct names the parameters in its receiver:
`fn push(self: Stack<T, N>, x: T) { ... }` is called `game.undo.push(3)`, `T` and `N` taken from the receiver's type. A function is copied only for the instances a call names, so `fn pop(self: Stack<T, N>) -> T` works for `Stack<i64, 8>` while a `Stack<string<12>, 8>` nobody pops exists; an error in a copy names the instance and is reported at the template's line.
Without `pub`, a function of a generic struct is private to the file it is written in. At most 1000 instances, nested at most 64 deep.
Not yet: `table<...>` fields, constant or computed counts, `pub` fields, generic functions without a receiver (DESIGN.md, Generics).

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

## Standard library

Plain library files, embedded in the compiler: `use "std/math.mito";` (a line of its own, from any program) puts the file's text there. A `use` path starting with `std/` names one of these files; any other path is looked up beside the program. An unknown `std/` name is an error listing the known ones. Their functions are functions of a type (`x.clamp(0.0, 1.0)`; the receiver's type picks the function), so the names do not clash with yours. Floats follow IEEE: nothing fails, NaN and infinity come out of the arithmetic (in JSON they are `null`).

**`std/math.mito`**

| Function | What it does |
| --- | --- |
| `Math.PI`, `Math.TAU`, `Math.E`, `Math.SQRT2` | float constants (a const group) |
| `x.clamp(lo, hi)` | `x` limited to `lo..hi`; floats and integers |
| `x.saturate()` | `x` limited to 0..1 |
| `x.sign()` | -1, 0 or 1 (a float for a float, an integer for an integer); NaN gives 0 |
| `x.fract()` | `x - floor(x)`, in 0 up to 1 (negative `x` too) |
| `a.lerp(b, t)` | `a + (b - a) * t`, not clamped |
| `a.inverse_lerp(b, v)` | the `t` for which `a.lerp(b, t) == v` (infinity or NaN when `a == b`) |
| `v.remap(in_lo, in_hi, out_lo, out_hi)` | `v` moved from one range to another, not clamped |
| `x.smoothstep(edge0, edge1)` | 0 below `edge0`, 1 above `edge1`, `3t^2 - 2t^3` between |
| `x.wrap(lo, hi)` | `x` brought into `lo..hi` (`hi` excluded) by whole widths |
| `x.to_radians()`, `x.to_degrees()` | angle units |
| `a.angle_diff(b)` | the shortest signed turn from angle `a` to angle `b`, in (-PI, PI] |
| `t.ease_in_quad()`, `ease_out_quad()`, `ease_in_out_quad()`, `ease_in_cubic()`, `ease_out_cubic()`, `ease_in_out_cubic()` | easing curves from 0 to 1 over `t` in 0..1 (`t` is limited to it) |
| `n.gcd(m)` | the greatest common divisor, never negative (`gcd(0, 0)` is 0) |
| `n.isqrt()` | the largest `r` with `r * r <= n`; a negative `n` is a 400 |
| `n.ipow(e)` | `n` to the power `e`; a negative `e` is a 400, a result that does not fit fails as the operators do (500) |

**`std/random.mito`**: a deterministic generator for a struct the program declares, `struct Rng { state: i64; }`, kept in its state (`game.rng.next()`). It is xorshift64* with wrapping arithmetic, so a seed gives the same stream on every platform; not for secrets.

| Function | What it does |
| --- | --- |
| `r.seed(s)` | starts the stream from `s` (an unseeded generator behaves as seed 0) |
| `r.next()` | the next 64-bit value, any `i64` |
| `r.range(lo, hi)` | a whole number from `lo` to `hi`, both included, unbiased (it draws again when a value falls in the short last block); `hi < lo` is a 400 |
| `r.unit()` | a float from 0 up to but not including 1 (53 random bits) |
| `r.chance(p)` | true with probability `p` |
| `hash2(x, y, seed)` | a 64-bit hash of a point and a seed, for value noise |
| `x.mix64()`, `x.shr_logical(n)` | the splitmix64 finalizer; a right shift filling with zeros (1 to 63) |

**`std/vec.mito`**: `Vec2 { x, y }` and `Vec3 { x, y, z }`, plain structs the program nests in its state (`struct Player { pos: Vec3; vel: Vec3; }`). Every function takes them as views and changes the receiver in place (`world.player.pos.plus(world.player.vel)`); a function that would return a vector writes into its receiver (`out.cross_to(a, b)`, where `out` may be `a` or `b`). `add`, `sub` and `copy_from` are collection operations, so the names are `plus`, `minus` and `assign`.

| Function | What it does |
| --- | --- |
| `v.set(x, y[, z])`, `v.assign(o)` | set the components; copy another vector |
| `v.plus(o)`, `v.minus(o)`, `v.scale(k)`, `v.add_scaled(o, k)`, `v.negate()` | in place: `v += o`, `v -= o`, `v *= k`, `v += o * k`, `v = -v` |
| `v.dot(o)`, `v.length_squared()`, `v.length()`, `v.distance(o)` | numbers |
| `v.normalize()` | to length 1, in place; a zero vector stays zero |
| `v.lerp_to(a, b, t)` | `v = a + (b - a) * t` |
| `v.reflect_to(w, n)` | `v = w - 2 (w . n) n` for a unit `n` |
| `v.project_on(o)` | `v` replaced by its projection on `o` (zero when `o` is zero) |
| `v.cross_to(a, b)` (Vec3) | `v = a x b` |
| `v.rotate(radians)`, `v.perp()` (Vec2) | turn counterclockwise by an angle, or a quarter turn: `(x, y)` becomes `(-y, x)` |

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
