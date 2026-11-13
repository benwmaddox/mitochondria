# Mitochondria design

All of a program's state is a declared tree of fixed-size values with declared maxima, reserved up front. The runtime
knows from the structure which bytes are live, which are committed and which are only reserved. It descends from
StasisLang (one visible state, bounded collections, data-oriented layout) and next-lang (allocate at startup, explicit
over implicit, compile time first). Measurements behind it: [docs/measurements.md](docs/measurements.md). Every command, executable option, built-in and library module is listed in [docs/reference.md](docs/reference.md); the tutorial is [docs/tutorial.md](docs/tutorial.md).

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
integer), text of at most N bytes (`utf8<N>`, also written `string<N>`, or `ascii<N>` for bytes 0..127) and `T[N]` (exactly N). In the items of a pool or list, `T` may also be a struct of numbers and bools (`vals: V[17]`): kept as one array per field and used as `item.vals[i].a`, with `item.vals.move(from, to, n)` moving every field alike (not yet in plain state, and an element cannot yet be read or written as a whole). Code reads maxima rather than repeating them (`x.capacity()`, `maxlen(s)`). An integer field may declare a range,
`limit: i64 in 0..3000000` (both ends included, 0 inside, since state starts zeroed): every write is tested (400 outside), and reads let the
compiler drop overflow and index tests on what is computed from them. A field without a declared range whose every store in the program has a proven range
(`s.n = n` with `n: i64 in 2..900`) gets their union, with 0, the same way: the compiler works it out from the whole program. State is unlogged by default (see "Failed requests" below); `logged` after a plain field, a collection, a struct field (with the collections inside it), a module instance (`m: ordmap<N> logged;`) or the `state` declaration
(`state game: Game logged;`) keeps it in the undo log: a failed request is rolled back there; removals in unlogged collections still wait for success.

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
answer, and synced to the log at checkpoints (`--db-sync normal`, the default: nothing is lost on a process crash, but the last moments of answered changes can be on an OS crash or power
cut); `--db-sync full` syncs every commit to disk before the answer, so that an answered change survives a power cut, at several times the cost of a write (about 8x on TodoMVC). At start the database is created or migrated by field name; a column whose type changed, or rows that break a declared
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
literals, and a response area, both reset when it ends. Both are address space reserved at start and committed as they fill (`a_grow`, 1 MB or 1/16th more at a time): a server's are 4 MB each, a command-line program's (`@main`) 4 GiB - 1 each, the most a 32-bit length holds, of which a small input commits only what it touches. Plain state over 1 GiB (`COMMIT_ALL_MAX`) is not committed at start either: on Windows a fault handler commits 256 KB around each first touch, on Linux the kernel backs a page when it is first written. A command-line program's loop budget grows with its stdin (100 million steps plus 64 per byte). Text a request builds goes in declared fields, not there: the state's,
or those of the `request` struct, a struct of plain fields declared beside the state (`request work: Work;`), zeroed before each
request, kept out of the undo log and never kept. Running out of an area fails that request (413 if the request itself does not
fit, 507 otherwise). There is no heap allocator anywhere.

**Commit and trim.** Without `--soft`/`--hard`, state up to 1 GiB (`COMMIT_ALL_MAX`) is committed whole at start: committing reserves
the system's commit charge, not physical memory (a page is backed when first written), and nothing is ever trimmed, so no store, add or
helper tests or commits pages. Larger state, or any program with pressure marks, commits pages when first written. A trim returns pages
holding no live byte, one collection at a time, driven by pressure:
At start the map of lines already marked saved (`dirty`) is filled for unlogged ranges a whole byte at a time (a per-line loop cost about 8 ms per declared GiB of text field); a declared capacity costs only reserved address space plus that map, so a large array or text field is free when unused.

| Committed memory | Behavior |
|---|---|
| below `--soft` | never trim |
| above `--soft` | after each request, trim the collection with the most committed memory |
| above `--hard` | trim everything; refuse writes (503) until below; reads continue |

A collection whose trim released nothing is skipped until 256 of its items have gone since.

## Code

**Functions.** `fn name(p: Player, n: i64) -> f64 { ... }`: every parameter and the result are typed (`-> T?` when it may be
null, no arrow when it gives nothing), and checked at every call and `return`. `let` and `var`; `+=` and friends. No closures. A
`var` keeps the kind of its first value. A local holds numbers, bools, items and windows onto text (a field, an input, a literal,
a `substr` or `trim` of one, what a function or builtin returns) and text a `let` builds (`let key = "sidebar:" + str(u.id)`): that
lives in its scope's region, on the request's text area from a mark at the scope's start, which the scope's end moves back to (no
zeroing; kept when text built there may outlive it: a `return` with a value, an outer variable given text, the answer cache). Such a
`let` may not be live at a recursive call, and its size must be known when compiling (from its parts': fields' caps, inputs'
lengths, what builtins can give), so `mito check --costs` gives each endpoint's most bytes of them at once; an endpoint that could
need more than the text area is refused. A `var` holds text
of its own when it declares its cap, `var acc: text<8192> = "";`: that many bytes reserved in its scope's region, written in place
(`acc += a + b` appends, `acc = ...` writes from the start; past the cap the request fails, 413). It follows the `let`'s rules (not
live at a recursive call; given back at the scope's end); `var t = ""` without a cap is an error that says to declare one. A function may return text (`-> text`): built for the request in
its text area, as a builtin's result is; the caller copies it where it keeps it.

**Functions of a type.** A function whose first parameter is `self: Type` belongs to that type, as in Stasis: `fn hurt(self: Player, n: i64) { self.hp -= n; }` is the function `hurt` of `Player`, called `p.hurt(3)`.
The same name can be declared for any number of types (`fn hurt(self: Monster, n: i64)` is another function, with its own rule) and beside a function of no type (`hurt(3)`); which function `x.hurt(3)` is depends on the type of `x`,
as the compiler sees it where the call is written, so the call costs nothing at run time and is inlined like any other. `self` is an item of a pool, list or table, or a number (`fn double(self: i64)` applies to every integer type,
`fn half(self: f64)` to floats), a string (`fn shout(self: string) -> text`) or a bool. `self` may also be a plain struct (see "Struct parameters" below). It is not nullable. A name that is a collection operation (`add`,
`remove`, `count`, `sort` ...) may be a function of a type: the receiver's type decides (a function of that type if one is declared, the collection operation otherwise), and the compiler's own names for the functions are mangled with the type. These functions may call themselves and each other. A call on a type that does not have the function says which types do (`` `M` has no function `hit` (declare
`fn hit(self: M, ...)`; `hit` is declared for P) ``). What the compiler works out about functions (which ones may write, which are recursive, how big they are) follows every function a call could reach. (In module code `self` is the
instance, as before; a function of a type is not module code.)

**Generics.** A struct may take parameters, `struct Stack<T, N: count> { items: T[N]; len: i64 in 0..N; }`: a bare name (`T`) is a type, `N: count` a whole-number literal of at least 1; a constraint after the colon
(`struct`, `number`, `int`, `text`) limits a type, and `key: field of T` names a field of the struct `T` stands for (the argument is a field name of an integer type, checked with a message that names the instance). Field types may use them (`T[N]`, `pool<T, N>`, `string<N>`, `Pair<A, B>`). A use writes the arguments, `undo: Stack<i64, 64>;`, `names: Stack<string<12>, 8>;`, `pool<Stack<i64, 4>, 8>`, a nested
one `Stack<Pair<i64, i64>, 4>`, wherever a type is written, in the state or in a signature. Each distinct use is expanded, before the program is parsed, bound or laid out (`src/generics.rs`), into an ordinary struct named for it
(`__mito__Stack__i64__64`, written `Stack<i64, 64>` in a message), so layout, the range analyses and code generation never see a generic: an instance is plain state, with its own storage, its own declared ranges (`in 0..N` is `0..64` in one
instance and `0..8` in another), its own place in the undo log (`logged` at the use) and its own whole-program field ranges. A function of a generic type names the parameters in its receiver, `fn push(self: write Stack<T, N>, x: T)`; `T` and `N` are
bound from the receiver's type where the call is written, `game.undo.push(3)`, and `x` is then checked against `T`. A call makes the copy: the receiver's type is known only to the code generator, so the program is expanded with no copies, compiled once to see which calls `x.f(...)`
name a function their receiver's instance does not have yet (`generics::note_missing`), expanded again with those functions copied for those instances, and so on until a compile asks for nothing new (`generics::expand`; the loop is the only mechanism). A function is copied for exactly the instances it is called on: `fn pop(self: write Stack<T, N>) -> T` works for `Stack<i64, 8>` while a `Stack<string<12>, 8>` exists that nobody pops;
the body is checked once per copy, so an error names the instance (`Stack<string<12>, 8>.pop`). A copy keeps its template's line numbers (`//# line N` ahead of it, read by the lexer), so a runtime `fail` or an error in a copy reports the line in the program's own file. In a program of several files (`use`), a function of a generic struct
without `pub` is private to the file it is written in: calling it from another file is an error that names it. A parameter `key: field of T` is replaced by the field's name everywhere in a copy, also after a `.` (`self.items[i].key`), and `T.key` is that field's type.
Limits: instances nest 64 deep and there are at most 1000, so a recursive generic type is an error. Not yet: a field of type `table<...>`, a count that is a constant or an expression, a generic function that has no receiver, and `pub` privacy for the fields of an instance (functions have it);
a function of a generic type cannot return `T` when `T` is a text or a struct (as no function can return those by value), so it is copied only where `T` is a number. Library modules (below) are the older mechanism for the same job and are being replaced by generic structs one at a time.

**Struct parameters.** A function can take a plain struct, a struct that is a field of the state (`state world: World;` with `origin: Pos;` in `World`, or nested, `world.hero.pos`): `fn move_by(p: write Pos, dx: i64) { p.x += dx; }` is called `move_by(world.origin, 2)`, and `fn len2(self: Pos) -> i64` is called `world.hero.pos.len2()`.
What is passed is a view of where the struct lives in the state, as an item is a view of its slot: nothing is copied, reads see the state and writes through the parameter change it, exactly as the same assignment written with the full path would. There is no run-time value for it either:
the compiler knows the struct's place (its offset) at the call, so a function that takes one is inlined with that place, and a recursive or outlined function is compiled once for each place it is called with (as it is once for each collection). Every analysis therefore sees the writes at the places they happen:
the whole-program ranges of fields (a range-declared field is still checked, a field's union of stored values includes what functions store through a parameter), which endpoints only read (a GET that writes through a parameter is not a reader), `logged` state and the undo log, and a module instance's state that only its own functions may write.
Both arguments of `add_into(dst: Pos, src: Pos)` can be the same struct. A struct that is a plain field in one place and an item of a pool, list or table in another is a plain parameter when the argument is the plain struct and an item parameter when it is an item, so one function takes either;
the argument decides, and nothing is ambiguous. A struct held by several collections still asks for `Pos in path` when an item is passed, as before. Not yet: a plain struct cannot be a function's result (`-> Pos` is an error unless it is meant as an item; return its fields), and a parameter `Pos?` cannot be a plain struct (it cannot be null).

**Struct values.** (Step 1 of docs/design/struct-values.md.) A struct can be held in a local: `let c = world.pos;` names it (a view; `Pos { ..world.pos }` is a copy), `var v = Vec3 { x: 1.0, y: 0.0, z: 0.0 };` builds one, `let z = Vec3 {};` is all zero, `Vec3 { ..base, y: 5.0 }` copies `base` and changes the fields named.
A literal names its fields (any order) and gives every one, except `{}` and the `..base` form; an array field is never given in a literal (start from `{}` or `..base` and assign its elements), and a literal is the whole value of `let`, `var` or `=` (name it first to use it anywhere else).
`v = other;` (a `var`) and `world.pos = v;` copy whole structs, between locals and in and out of the state; a local struct's fields are read and written as the state's are (`v.x += 1.0`, `v.pos.y`, `v.hist[2]`, `v.name = "a"`), and a `let` struct's fields are not assigned (`var` for that).
A `V { ..src }` copy is independent of its source in both directions (an assignment copies already). Everything a struct holds is copied: numbers, nested structs, fixed arrays and sized text; what it cannot hold is a collection or a stored id, and a value is at most 256 bytes, with an error naming the struct and its size.
A struct local is a place, like the state's: its place is in a scratch area of 16,384 bytes in the request's own memory (the request struct's kind of memory: never zeroed, kept out of the undo log, a reader's own), at an offset fixed when compiling. Locals of blocks that do not overlap share places; a function compiled apart
(too big to inline) has places of its own at the area's top end; the area is part of a program only when its code has a struct value, so other programs are laid out and sized as they were. Because a local is a place, everything works on it as on the state: a range-declared field keeps its check when a literal or a field store writes it (a copy cannot leave the range), a local can be passed to a function, as an argument or as `self`
(a parameter is read-only unless declared `write`; a `write` number, `f64` or `bool` parameter (`n: write i64`) shares the caller's place: a `var` by its C name, a field or element read and stored by the code of a direct assignment, with the place's items and indices copied at the call; the caller forgets the interval of a `var` it handed over, and the loop and range analyses count such a call as an assignment (`stmt_write_locals`); recursive callees are refused, outlined ones inlined, so a function cannot change it by accident; a `write` parameter's writes change the place it views, local or state), and tests and endpoints, inlined or compiled apart, hold them alike. The analyses treat the two kinds of place differently, in both directions: a store into a local is the request's own and counts for nothing
(a GET that only builds locals is still a reader; a `let c = world.pos` of a `logged` struct is a read), and a copy into the state is a state write (not a reader; logged lines saved for the undo log first, so a failure restores them) that gives every number it writes an unknown range for the whole-program ranges of fields. A call written with a receiver resolves its `write` places the same way: `g.count.bump()` and `x.add_to(5)` for `fn bump(self: write i64)` and `fn add_to(self: write i64, k: i64)` (the receiver and any other `write` argument are places; a `let`, a literal or a computed value as the receiver of a `write self` method is an error); a local `var` that any method of that name takes as a scalar `write` parameter counts as assigned by the call.
A function can return a struct (`-> Pos`, inlined functions only; a recursive one cannot yet): the result is written into the place the call was given, `target = f(x);` (the target itself, one move) or `let t = f(x);` (a new place `t` names), and a call used any other way, as an argument or a receiver, is an error that says to name the middle. Not yet: take a struct by copy (not a mode: write `f(x.copy())`), compare two structs, answer or accept one (step 4), hold one in a generic module's parameters (step 5), or have one local in a recursive function (step 5: it needs a frame); each is an error that says so. In a test, struct locals work; a test still changes the state only through requests.

**Conversions.** `x as u8` (also `u16`, `u32`, `i8`, `i16`, `i32`, `i64`, `f64`) always succeeds: to a narrower integer it keeps the low bits (`x as u8` is `x & 255`; to a signed
type the top bit is the sign, so `255 as i8` is -1), so the ranges the compiler works out for masks apply to it and a `u32` computation (`h = ((h ^ b) * 16777619) as u32`) needs no overflow test; a float
becomes an integer by truncation, as `int(x)`. `as` binds tighter than `*` and looser than a prefix `-`. `u64` is stored in fields but is not yet a value in code.

**Sized numbers in code.** A `let` or `var` declared with a sized type (`var h: u32 = 2166136261;`, `let c: i16 = x;`, types `i8` `i16` `i32` `u8` `u16` `u32`), a parameter, and a result (`fn mix(h: u32, b: u8) -> u32`) are tested against the type's range, as a store into a field
declared `in lo..hi` is: a value outside fails the request (400, naming the variable) unless the range analysis proves it inside, in which case no test is written. Every later assignment to a typed `var` is tested the same way (`h = ...`, `h += ...`);
a parameter is tested when the function is entered. A returned value's range is known to the caller (the union of what every `return` gives), and a loop variable that is masked or tested to a range each round settles on that range, so
`h = mix(h, b) ` over a loop of any length has no test left. `f32` rounds a value to a float where it is stored or returned (a parameter is not rounded on entry). `i64` and `f64` are the plain kinds.

**Text.** A text literal takes the escapes `\n` `\t` `\r` `\0` `\"` `\\` `\'` and `\u{HEX}` (a code point); any other is an error.
`len`, `substr`, `contains`, `starts_with`, `ends_with`, `index_of`, `last_index_of`, `trim` (and `trim_start`,
`trim_end`), `replace(t, a, b)`, `byte_at`, `lower` / `upper` with Unicode's full case mappings (as Ruby's downcase and upcase), and
`all_emoji(t)` (every character an emoji, with Ruby 3.4's tables of the two Unicode properties), and `chr(cp)` (a code point
as UTF-8).
`foreach part in split(t, ", ")` walks the pieces as Ruby's `split` gives them (a leading empty one kept, trailing ones dropped);
`split(t)` walks the words. Each piece is a window onto the text.

**HTML fragments.** `html_parse(t)` parses markup as Rails 8.2 apps do (Nokogiri::HTML5: the same gumbo, bundled, as a fragment in a
`body`; `html_parse_in(t, node)` in that node's context) into the request's node pool and gives the fragment (or -1 for markup the parser refuses, as Nokogiri raises: nested more than
400 deep, an element with more than 400 attributes), a node: an integer, good
for the request that made it (-1 is none). `node_kind` (1 element, 3 text, 4 CDATA, 8 comment, 11 fragment), `node_name`,
`node_parent` / `node_first` / `node_last` / `node_next` / `node_prev`, `node_content` (a text's text, an element's text throughout),
`node_attr(n, name)` (or null), `node_attr_count` / `node_attr_name` / `node_attr_value`, `node_set_attr`, `node_remove_attr`,
`node_set_text`, `node_new_element`, `node_new_text`, `node_append`, `node_insert_before`, `node_detach`, `node_clone`; `node_html(n)`
writes a node back as Nokogiri writes HTML5 (`node_inner_html` its children; `node_html_escaped` with `<` `>` escaped in attribute
values too). The pool is the instance's own (readers parse at once), emptied when the next request starts; more than its caps (131,072
nodes, 262,144 attributes, 16 MiB of text) fails the request (413). The parser's own working memory is the C heap for the length of
one parse, freed before it returns.

**Files.** A program may be several files: `use "rooms.mito";` on a line of its own puts that file's text there (relative to
the file that uses it, each file once). It is still one program with one state; errors name the file and line they are in.
`mito check app.mito --costs` lists, per endpoint, the request struct bytes it zeroes and what its code does: queries, row reads,
table writes and text built, each with how many are inside loops.

**Narrowing.** A value that may be null (`T?`, a nullable field) is used after a test says it is there: inside `if exists(x)`
(or `x != null`), in the rest of an `&&` after it (`if exists(r) && r.n > 0`), in an `else` of `if !exists(x)`, and after
`if !exists(x) { return ...; }` (or `fail`, `break`, `continue`) for the rest of the block. A row's nullable field narrows the same
way (`if exists(m.connected_at) && m.connected_at > t`: a time, then a number; a reference, then a row). `or(x, fallback)` has the
kind of the two (text, a number, a bool, an item).

**Endpoints** are functions with an attribute and typed inputs:

```
@get("/rooms/{room}/messages")
fn messages(who: Session from header("X-Session"), room: Room, limit: i64 in 0..100 = 50)
    -> Response<{ messages: [{ seq: i64, author: string, body: string }] }> {
    return ok({ messages: [foreach m in room.messages reverse limit limit {
        seq: m.seq, author: or(m.author.name, "[deleted]"), body: m.body }] });
}
```

- Inputs come from the path (`{room}`: an id gives the item; a segment may put literal text before its parameter, `@{message}`), the body (POST, PUT, PATCH: JSON, or an HTML form's
  `application/x-www-form-urlencoded`, whose values are read as text the way query values are; a ticked checkbox's `on` is
  true, an unticked one is absent and takes the default; a name given twice keeps its first value) or the query (GET,
  DELETE), a header (`from header("X-Session")`: a session token gives the session), a cookie (`from cookie("list")`; an
  item from a cookie is named by a session token, and `Board?` makes it null when the cookie is absent or names no live
  item with that secret, as for a first visit), or another worker
  (`forwarded`). They are checked before the body runs: absent, wrong kind or out of range 400, too long 413, no such
  item 404, a body field not taken 400.
- Markup: an HTML answer is written as HTML in the code, `return ok(<ul>{foreach t in b.todos { item(t); }}</ul>);`, and a
  function declared `-> html` writes a reusable piece (`fn item(t: Todo) -> html { <li class="{if t.done { "done" }}">{t.title}</li> }`).
  Markup starts at `<tag` after `(`, `,`, `{`, `;` or `}`. An answer with no single element around it (a list of fragments) is written by code
  alone: `return ok({foreach t in b.todos { item(t); }});` (a `{` holding a statement, so never a JSON object). `{x}` writes a value escaped (text with `& < >` as entities, and `"` inside
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
- **Sockets** (WebSocket, RFC 6455). `@socket("/cable") fn open(socket: i64 from header(":socket")) -> Response<null>` runs on an
  upgrade request to that path, with the request's headers and cookies: a 2xx answer accepts it (the runtime answers 101 with the
  accept key and the endpoint's own lines, such as `Sec-WebSocket-Protocol`), anything else is answered as it is. `@socket_message`
  runs for each text message (`text: utf8<N> from header(":message")`), `@socket_close` when the socket ends; both read the socket's id
  (`:socket`) and the integer the program tagged it with (`:tag`, set by `socket_tag(id, n)`: whom the socket belongs to). What a
  request asks of sockets is applied once it succeeds, under the program's lock: `socket_send(id, t)`, `socket_close(id)`,
  `socket_send_all(t)`, `socket_subscribe(id, stream, before, after)` and `broadcast(stream, t)` (each subscriber gets its own
  `before` + t + `after`: Action Cable's identifier around a stream's message), `socket_unsubscribe(id, before)`. `@every(3000) fn
  tick()` runs on a clock (one such function), for pings. A socket's id never names a later one; client frames are masked and at most
  1 MB; pings are answered. The network can never send these requests itself. One program only (not yet a system of workers).
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
  `manifest "public/assets/.manifest.json" at "/assets/";` reads Propshaft's manifest when compiling, so `asset("email.svg")` in the code
  is that asset's digested URL (`/assets/email-6c595bc5.svg`) and a name the manifest lacks stops the build.
  `writes { ... }` lines are added as they are to the answers of requests other than GET and HEAD (what a caching proxy in front adds to
  answers it does not cache). `headers` replaces the built-in `nosniff`, Content-Security-Policy and Referrer-Policy lines; an endpoint's own line of the same
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
- `Response<bytes>` answers any bytes as they are (`application/octet-stream`, or the Content-Type the endpoint sets with
  `set_header`, which replaces the kind's for any answer); `file_base64("logos/icon.png")` is a file's bytes read when compiling, as
  base64 (`unbase64` it once, in `@start`). `no_default_headers()` sends the answer without the `web` block's header lines (as a
  file Rails sends). `escape_html(t)` is text with `& < > " '` as entities (ERB's escaping, for JSON or markup built as text);
  `contains`, `starts_with` and `ends_with` compare bytes, `index_of` and `last_index_of` give where (or -1); `random_text(n, alphabet)` draws characters uniformly with the system's
  generator (session tokens: base58); `json_field(text, "name")` reads a field of a JSON object (null if absent). `json_quote(t)` writes a JSON string as
  ActiveSupport does (`< > &` as `<` and so on), `json_unquote(t)` reads one back (null for a bad escape). `request_target()` is the
  request's path and query as sent; `crc32(t)` is zlib's CRC-32. `read_stored("ab/cd/key")` reads a file under the server's `--files DIR`
  (stored uploads), a relative path of letters, digits and `. _ - /` without `..`; null when it is not there.
  A loop's `scan where ...` accepts that a `where` over a whole table reads every row (a schema that cannot have the index).
- `Response<html>`, `<text>`, `<css>`, `<js>` and `<svg>` answer text as it is with that content type (UTF-8). `file("assets/app.css")`
  is a file's text, read when compiling (relative to the program) and kept in the program, so a stylesheet is served as
  `@get("/app.css") fn css() -> Response<css> { return ok(file("assets/app.css")); }`. An answer that is a literal is the same
  every time: it gets an `ETag` (`Cache-Control: no-cache`), and a request whose `If-None-Match` names it is answered 304. `return see_other("/path")`
  answers 303 from any endpoint (`return found("/path")` answers 302 with the Location made absolute from the request's Host, as Rails'
  `redirect_to` writes it; `status(401, <markup>)` answers a page with an error status); the address must be a path on this server (one leading `/`, printable ASCII), checked when
  compiling for a literal and when running otherwise, so a redirect can neither leave the site nor add header lines. Errors are
  answered in JSON.
- Collections are used through their functions: `add` (or `add { field: value }`), `try_add`, `remove`, `count`,
  `capacity`.

**Library routines in the runtime.** `base64` and `unbase64` use AVX2 where the CPU has it (checked once, at run time; a scalar loop elsewhere, and for every case the vector loops leave: the tail, the URL alphabet, text that is not base64), with the
same answers either way (tests/simd_base64 compares both builds with a reference). `array.sort(from, count, scratch)` puts integer elements of a fixed array in ascending order, stably, with a scratch array of the same type the program declares
(the language allocates nothing); it is a merge sort that skips the merge of halves already in order, so mostly sorted data costs little more than reading it.

**Adding to a pool.** `add` to a plain pool (one instance, no generations, no nested collections) kept out of the undo log is done in the endpoint, with the layout (live-bit word, column offsets and sizes, header) as constants:
at the high-water mark with nothing freed below it, one live bit, the columns of a reused slot cleared, the header; anything else (a freed slot to reuse, a full pool) goes to the runtime's helper.

**Failed requests.** State is unlogged by default: a request that fails after writing leaves what it wrote (the cheap default, as for a local array in any language). State declared `logged` is atomic: the runtime
records the old bytes of each write to it, and a failed request writes them back. (Tables are atomic in their database whatever is declared.) A `logged` field starts on a 64-byte line of its own, and unlogged fields pack together.
The log saves whole 64-byte lines on their first write in a request; a 4 KB page still all zeros (state never written, as an array a request
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

**Standard library files.** Math is built from two layers: the float functions that need the platform's math library (`tan`, `ln`, `round`, `cbrt`, `hypot`, `fmod` and the rest) are built-ins, each a C macro over `libm` (under TinyCC on Windows, which links msvcrt, the few functions msvcrt lacks come from ucrtbase through an import list), and everything that is plain arithmetic is Mitochondria code in `lib/std/*.mito` (`math`, `random`, `vec`), embedded in the compiler. A `use` path starting with `std/` is looked up among the embedded files before anything beside the program: one mechanism, so a program anywhere gets the same library. Helpers are functions of a type, so the receiver picks the function (`x.clamp(0.0, 1.0)` for floats and integers). Vectors are plain structs nested in the program's state and changed in place through views (`pos.plus(vel)`); with no struct values, a vector-valued operation is `out.cross_to(a, b)`, which computes into temporaries and then writes `out`, so `out` may be `a` or `b`. The names `add`, `sub` and `copy_from` are taken by collection operations, so vectors use `plus`, `minus` and `assign`.

**Const groups.** `const Limits { MAX_HP: i64 = 100; MAX_NAME = 16; }` names constants, read as `Limits.MAX_HP`; a value may use earlier constants and
integer arithmetic (folded when read, an overflow or a name that is not a constant is an error naming the constant), floats need a type (`G: f64 = 9.8`). They are
not enums: an enum is a type (a field of it is checked, `match` must cover it, its values differ), a group is only numbers, floats allowed, duplicates allowed. Groups share
the enum machinery in the binder: `Limits.MAX_HP` is replaced by its literal before any analysis, so a loop over `0..Limits.MAX_NAME - 1`, an index and a range are bounded exactly as
with the number written (and a sum of two numbers is folded the same way). Layout reads the groups first, so `in 0..Limits.MAX_HP` and sizes take them; endpoint input ranges do not yet.

**Debugging.** `log(a, b, ...)` writes its values (formatted as `+` would, separated by spaces) on one line of the server's
standard error, with the line it was called from (`log: line 12: n is 5`); it is not part of the answer.

**Tests.** `test "name" { ... }` in the program file is a function the test runner calls: inside, `get`, `post`, `put`, `patch`
and `delete` make requests (`post("/players", { name: "ann" }, { "X-Session": token })`) answering `.status` and `.body`,
`field(text, "name")` reads a field of a JSON answer, and the state is read directly (changed only through requests). It returns
`true`, `false` or a text saying what went wrong. `mito test app.mito` runs each test on a fresh state with the clock and
randomness fixed, and prints the line of the `return` that failed one, or of the statement whose error ended it. Tests are never
part of a built or served program.

**Ranges of variables.** Integer arithmetic is always checked; the compiler drops a check when it can show the operation cannot overflow, from the interval each operand lies in. A literal,
a range-typed field and an input with a range have one. A `var` has what the code gives it at each point, worked out as it is compiled: its first value; the interval of what an
assignment stores; the union of what the two ways of an `if` leave (a way that always leaves counts for nothing); inside the branch of an `if` or the body of a `while`, the upper bound its
condition gives (`x < e`, `x <= e`, joined by `&&`, e's upper bound known). A counted loop (`foreach i in lo..hi`) or a `while` that only steps a counter by constants (`n = n + 1`,
`n = n - 2`, constants up to 2^20, no other assignment to it in the body, none inside an inner loop) widens the counter's entry interval by the steps of one round times the most rounds, in the body
and after the loop; a variable the loop assigns in any other way is unknown from there. The most rounds of a counted loop is `hi - lo` from the intervals of its bounds; a `while` (a loop without a fixed
extent) charges every round to the request's budget, which the runtime never lets exceed 2^50 (`--loop-budget` above it is refused, with a message), so it is taken as 2^51. A start near the end of
the i64 range, or a bound that is not known, keeps the test; so does anything the rules above do not name. Declaring a range on the field a loop is bounded by (`ncmds: i64 in 0..4096`)
is what gives most loops a bound.

**What the whole program shows.** The compiler builds a program more than once. Each build records what every store in the program puts into each plain integer field and each plain array of
integers (an indexed store, `fill`, `copy_from`; `move` stays inside one array), and which pools anything removes from. The next build assumes them: a read of such a field or array element lies in the union of
what was stored there and 0, its value at start; and an item reached by number in a pool nothing removes from needs only the range and high-water-mark tests, as every slot below the mark is live. Builds repeat until
what they assume stops changing (a counter feeds a field, the field feeds an array, an index loaded from the array is then proven), and a build is kept only if everything it compiled agrees with what
it assumed. A variable a loop moves by amounts that are not constants (`off = off + frame`) gets an interval at the head of its loop the same way: guessed from the one it had on entering, then widened by what the loop's
assignments give under the guess (an end that grows goes to infinity; an assignment with no known interval drops the guess), until the guess holds; with a `while off < in_len` bound, an `off` that only grows needs
no index or overflow test. A store whose value has no known interval (a value from outside, a loaded number nothing bounds) leaves its field or array without a range. A quotient by a positive divisor has the range its
operands allow. `MITO_EXPLAIN_INDEX=1 mito emit-c prog.mito` lists the index tests that remain and what is known of each index.

## Platforms
A program says what it is built for with `platform web;`, `platform cli;` or `platform test;` or `platform game;` on a line of its own in its entry file. The platform is the
entry file's: the file given to `mito build`, `run`, `serve` or `check`, so a server and a tool can be two entry files over the same modules, each with its own platform. There is no
command-line switch. An entry file that names none is the platform its entry points show: `@get` and the like make it web, `@main` cli.

A platform is a driver, the entry points it allows, the built-ins it allows and the libraries bundled with it:

| | driver | entry points | bundled libraries | refused |
| --- | --- | --- | --- | --- |
| `web` | the HTTP server (runtime/http.c) | `@get` `@post` ... `@socket`, `@every`, `@start` | SQLite, zlib, HTML parsing, crypto | `@main` |
| `cli` | the command-line driver (runtime/cli.c) | `@main`, `@start` | crypto | endpoints; `set_cookie`, the answer cache, `html_parse`, sockets; tables, `gzip;` |
| `test` | the test runner (`mito test`) | none (`test "..." { }` blocks, `@start`) | as web | `@main`, endpoints |
| `game` | the window loop (runtime/game.c) | `@frame`, `@shader`, `@start` | sdl3, crypto | endpoints, `@main`; `set_cookie`, the answer cache, `html_parse`, sockets; tables |

A build has exactly one platform (its entry file's). Libraries are named items (`sqlite`, `zlib`, `html`, `crypto`; `sdl3` with the game) and a platform is a driver plus a list of them, so
platforms share libraries by name (web and a game both bundle `crypto`). Direction: developers are to be able to add libraries (a name, the C sources or system library to link, and what shows a
program uses it) and platforms (a driver, the entry attributes and built-ins it allows, a list of libraries) in files of their own, so neither list is a compiler-level fact. Not built: today
both are tables in src/platform.rs (`LIBRARIES`, and `Platform::libraries`), which is the shape a developer's definition would take.

The checks run when the program is compiled (`check`, `build`, `run`, `serve`) and say which platform the program is for and what does not belong to it. The library check looks at what
the generated C needs (`MITO_USES_SQLITE`, `_ZLIB`, `_HTML`, `_CRYPTO`) against the platform's bundle, so a command-line program that declares a table is refused at compile time, not when it runs.

## Games
A program whose entry is `@frame` is a game: `platform game;`, a `@frame` function and one or more `@shader` functions. The driver (runtime/game.c) opens a window with an OpenGL 3.3 core context, asks each `@shader` function for its source once, and
calls `@frame` every frame. SDL3 (3.2 or newer) and the system's OpenGL are loaded when the game starts, with no header and nothing linked or vendored; a machine without SDL3 is told so.

```
@shader fn terrain() -> Response<text> { return ok(file("terrain.frag")); }

@frame
fn frame(dt: f64 from time, forward: bool from key("w"), look_x: f64 from mouse("dx")) -> Response<text> {
    world.yaw += look_x * 0.0025;
    if forward { world.z += 7.0 * dt; }
    return ok("terrain z=" + str(world.z) + " yaw=" + str(world.yaw));
}
```

The frame function's inputs are what the window saw (`from time` the seconds since the last frame, `from key("w")` whether a key is down, `from mouse("dx")` the pointer's movement since the last frame). Its answer is the
frame: a line for each pass, the name of a shader and then its uniforms as `name=value` words. A shader is a fragment shader (`#version 330 core`) drawn over the whole of what it draws on with one triangle; the driver's vertex shader gives `in vec2 vUv`, 0..1 across it, and the
driver sets `uniform vec2 uResolution`, its size in pixels. A `uniform float` the answer names gets its value. A frame that names a shader that does not exist, or a uniform without a number, stops the game with a message.
A pass can draw into an image instead of the window: `scene@0.25 px=1 pz=2` draws the shader `scene` into an image a quarter of the window's width and height (one image, 16-bit floats so that light can exceed 1, kept between frames and made again when the window or the scale changes),
and a pass after it that draws on the window reads that image as `uniform sampler2D uImage`. The image can carry two pictures: a scene shader that declares `layout(location = 0) out vec4 color;` and `layout(location = 1) out vec4 aux;` draws both at once (two 16-bit float textures, both made again when the window or the scale changes), and the window pass reads the second as `uniform sampler2D uImage1` (linear filtering; `texelFetch` gives exact values such as an id). examples/world keeps in the second what the brush strokes follow: depth, the surface's normal and what each pixel is (sky, ground, water, the figure), so that the strokes run along edges and contours and the figure gets finer strokes. That is how an expensive picture is drawn small and used large: examples/world draws its world at a quarter of the window's size and paints from it, 23 times faster than doing both for every pixel (1.2 ms against 28 ms a frame at 1280x720).
The picture is made on the GPU and the rules stay in the program: state and rules are ordinary Mitochondria (fixed-size state, `sin`, `cos`, `min`, `max` on floats), and the game's logic can be tested without a window like any other program's.
`--frames N --screenshot f.ppm` renders N frames in a window that is not shown and saves the last, to look at or to test with (src/build.rs, `game_renders_the_world`, needs SDL3 and a GPU, so it runs only when asked).
**Record and replay.** `--record FILE` writes what the program was given, so that `--replay FILE` can run it again and prove the run is the same. A recording is the program's inputs and nothing else: each frame's `dt` as a 64-bit double, the state of the keys the program reads and the pointer's `dx`, `dy` as 32-bit floats; replay feeds them to `@frame` in place of the window's events (the window is still created, hidden, to render and to allow `--screenshot` and `--capture`). The run is made repeatable on both sides: the clock is fixed (`now()` starts at 2026-01-01 00:00 UTC as in `mito test` and moves on by the sum of the frames' `dt`, in whole milliseconds) and the random generator has the recording's seed (`--seed`; when it is 0, which means a seed of the machine's own, a fresh nonzero one is chosen and stored). Everything is little-endian:

```
offset  size  field
0       8     magic `MITOREP1`
8       4     format version (1)
12      4     checkpoint interval N (0: none but the last)
16      8     seed
24      4+4   window width, height
32      8     frame count (all ones until the recording is finished: such a file is refused)
40      8     hash of the state after the last frame
48      8     program hash: the key names and shader names in order and the state layout (sizes, offsets, collections' capacities and columns)
56      ...   u32 number of keys, each u16 length + name (the program's key order); u32 number of shaders, each u16 length + name
then for each frame: f64 dt; (keys + 7) / 8 bytes of key bits (key k is bit k % 8 of byte k / 8); f32 mouse dx; f32 mouse dy;
        and, when (frame number + 1) is a multiple of N, a u64 state hash taken after the frame
```

The state hash (`mito_state_hash` in runtime/mito.c, 64 bits, not cryptographic) covers exactly the live declared state: the plain values (offset 0 up to the request struct) and, for every collection, the ranges the memory trimmer treats as live (the header words, a pool's live bits and generations, the columns of the live items, a string column up to its length), each mixed with its offset and length. It leaves out the request struct, the text and answer areas, the undo log, SQLite tables and the clock, which may differ between runs without the state differing. A replay recomputes the hash at the same frames and on the first mismatch prints `replay diverged at frame K: expected H1, got H2` and exits with code 1 (K counts from 0 and is the checkpoint's frame: the state went wrong at or after the frame N before it); after the last frame the recorded final hash is compared the same way. A file that is not a recording, is cut short, has another version, is unfinished, or was made by another program (its program hash differs) is refused with exit code 2 and a message naming what is wrong. A change to the program's rules alone does not change the program hash: the replay then diverges at the first checkpoint. `--capture DIR` writes `DIR/frame_NNNNNN.png` every `--capture-every N` frames (default 1), numbered by frame, live, with `--frames` or with `--replay`; the PNG is written by the driver with no library (8-bit RGB, deflate stored blocks, Adler-32 and CRC-32 by hand). src/build.rs `game_record_and_replay` (needs SDL3 and a GPU) records `examples/world`, replays it twice and compares checkpoints and pictures, refuses damaged files and catches tampered inputs.

`examples/world` is an open world made this way. Not built yet: tables in a game, sound, mouse buttons, more than one offscreen pass.

## Command-line programs
A program whose entry is `@main` is a tool, not a server: `@main fn main(filter: utf8<1..1000> from arg(0), doc: utf8<4294967295> from stdin, c: bool = false) -> Response<text>`.
The command line is the request: `from arg(N)` is the Nth argument, `from stdin` what is piped in (read only when the program has such an input, up to 4 GiB - 1 bytes: a length is 32 bits; it is read straight into the request text area and the program's text input points at it, so the input is held once, not copied), and any
other input is a flag: `--name value`, `--name=value`, a bare `--name` or a short `-c` is true. Inputs are checked as for an endpoint (a missing or out-of-range one
answers 400). The answer's body goes to standard output with exit code 0; an answer of status 400 or more goes to standard error with exit code 1; a program that
fails to start exits 2. A program with `@main` has no endpoints beside it (`@start` is allowed), and no tables yet. `mito run tool.mito -- args < in` builds and
runs it; `mito build` makes a standalone executable. The driver is runtime/cli.c; `examples/jx` is a jq-like tool written this way.
A text field's declared size is at most 4294967295 (4 GiB - 1): a length is 32 bits. Where a field sits is not limited that way: a state's byte offsets are 64 bits, so any number of such fields may be declared, each reserved as address space and committed as it is written (the text values a request reads are 32-bit offsets into the request's text area, not into the state).
The exit code is the program's to choose: `return exit(n, body)` writes the body to standard output and exits with `n` (0 to 255), `return exit_error(n, body)` writes it, as it is, to standard error and exits with `n`; they exist only in `@main` (a jq-like tool exits 2 on a usage error, 5 on invalid input). They are answers of status 1000 + n and 1256 + n, a range no HTTP status uses, so every other rule is unchanged: 200-399 exit 0, 400 or more exit 1 with the body and a line break on standard error.
A program with an input `from args` gets every argument as written, each followed by a 0 byte (no argument can contain one), and does its own flag parsing: the runtime then binds no `--name` flags and every argument (`--tab`, `-n`) is a plain one; the first 32 are also available as `from arg(N)`. The arguments may total 1 MiB.
The inputs travel as the internal headers `:argN`, `:args` and `:stdin`, as a socket's travel as `:socket`.

## Modules
A module is a `.mito` file in `lib/`: parameters, structs, a `state { ... }` block (what an instance holds) and
functions. A use is a field of the state, `scores: sorted<Score, 10000, points>;`, expanded when compiling into
ordinary state and functions (`game.scores.add(...)`), one copy per use. Outside its functions, an instance's state
cannot be written, its items not added or removed directly, and its key not assigned. A module with `iter_first`,
`iter_next` and `iter_item` can be walked by `each`.

Standard library: `sorted<T, N, key>` (a skip list, items in order of a field: `add(k)`, `remove`, `set_key`,
`first`, `last`, walks from a key), `intmap<N>` and `strmap<N>` (hash maps from `i64` or string keys to `i64` values, up to N entries; a removed entry frees its slot at once) and `bag<T, N>` (the smallest module, for tests). The fixed-capacity collections: `queue<T, N>` (FIFO of struct items) and `ring<T, N>` (keeps the last N, overwriting the oldest) over a pool and an order array; `set<N>` (hash set of `i64`); `heap<N>` (binary min-heap of `i64` priority and payload); `bitset<N>`; `grid<W, H>` (2D `i64` cells). Each is fixed state, every operation bounded, and a full structure an ordinary error (507). `rowmap<N, W>` is `ordmap` with a row of W numbers per key (`put(key, col, value)`, `get_or`, `add_to`, `range_sum` by column): a record's numbers kept in the leaves beside its key.

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

## Readers
A program's GET and HEAD requests run at once on readers (`--workers N`, default twice the CPUs the process may use: a
connection thread holding a reader is at times descheduled for its I/O, and with as many readers as CPUs a fifth of them idled;
Linux only for now): each reader is an instance with its own request struct, text, answer and database connection, over the primary instance's
state mapped read-only (the state lives in a shared memory file; the request struct is on whole pages, which each reader covers
with pages of its own). Readers hold a readers-writer lock in shared mode; every other request, `@start`, `@every`, and socket
events hold it alone, and a waiting writer holds back new readers. The compiler marks each endpoint whose code never changes state (plain state, a collection, a table, a hand-off to another worker; the request struct and the text area are the
request's own) as read-only in its route table; a reader runs only those. Any other endpoint reaching a reader runs again alone on the primary at once, nothing of it staying
(a GET that writes, as a Rails app's `last_active_at` refresh once an hour does, is such a GET); a call of a recursive or outlined function counts as writing, as it is compiled apart.
State may be unlogged: the decision is made by the compiler, not by the undo log. Writes the compiler missed would fault on the read-only mapping rather than land, and a
reader's request that still reaches `save_line`, a table store, an `add` or a socket operation stops and runs again on the primary as before (tests/readers has a program and a stress script).
A program with memory-pressure marks (state not committed
whole at start) has no readers: asked for with `--workers`, the server refuses to start, saying why. With readers, SQLite's memory is
a pool of `--db-memory` MB for each connection (the primary's and each reader's) rather than one for the process: one pool's lock,
taken for every allocation, held readers back, and a connection touches only its own memory, so each pool needs no lock. A request
whose connection runs out of its pool fails ("database: out of memory"), and the next one runs as before.

## The answer cache
Pieces of answers kept between requests (`--cache-mb N`, default 64, 0 none), shared by every instance (readers fill it too), outside
the state: not journaled, not rolled back. `cache_start()` marks where the piece begins in the answer; `cache_hole(label, value)`
writes a value (escaped) that is not kept; `cache_put(key)` keeps what was written from the start, minus the holes. `cache_get(key)`
finds an entry: its number of holes (-1 when there is none); the request writes `cache_part(i)` and, between parts, a fresh value for
each hole, `cache_label(i)` saying which (a form's CSRF token, freshly masked as Rails masks it on every page). `data_version()` moves
whenever a request that changed the state or a table commits (for unlogged state, which the undo log does not see: whenever a successful POST, PUT or DELETE endpoint that can write state runs), so a key that names it is never found after a
change; keys name everything else the piece shows (the user, say). A full cache, or one whose index is half full, starts empty again.

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

## Formatting
`mito fmt <file.mito>...` is a conservative formatter (src/fmt.rs): it works on tokens, changes only white space, and never joins, splits or reflows lines, so a program means the same and its line numbers stay. Without an option it prints one file formatted; `--write` rewrites the files; `--check` lists the files that would change and exits 1 if any does. Each file of a program (`use "other.mito";`) is formatted on its own. A file that does not lex (an unclosed text, bad markup) is refused with the lexer's place. The rules:
- Indentation is four spaces per line that has an open `{`, `(`, `[` or markup element; several openers on one line count once (`{foreach t in ts {` ... `}}` is one level), and a line that starts with closers (`}`, `)`, `</li>`) is dedented by them. A line inside a bracket that opened on an earlier line keeps its offset from that line (aligned or hanging, as written). A line that continues an expression (the one before ends in, or it starts with, `&&`, `||`, `+`, `*`, `/`, `%`, `=`, `==`...) is one level deeper. An own-line comment takes the indentation of the line after it (the block's inside before a closing line).
- No trailing white space, LF line endings, one final newline, at most one blank line in a row, none at the start of the file or of a block (`{` at the end of a line), none at the end.
- Inside code, one space after `,` and `:`, none before `,` `;` `:` `)` `]` or after `(` `[`; one space around `=`, `==`, `!=`, `<=`, `>=`, `&&`, `||`, `+=`, `-=`, `*=`, `/=`, `=>`, `->`, and around `+` `*` `/` `%` between two operands; `{` and `}` get a space inside and `{` one before it (`) {`). `<`, `>` and `-` are left as written (they are also generics and signs), and any other run of spaces is kept, so aligned columns and `x;   // note` stay. A trailing comment keeps its gap (two spaces if it had none).
- Comments (`//`) are kept byte for byte, only indented.
- Markup is the dangerous part, since its white space can reach the page. Texts spanning lines, a quoted attribute over several lines, and `<!-- -->` comments are never touched inside; text between tags and the spacing between tags and attributes on a line are never touched; only the indentation at the start of a line (which the compiler already drops) and trailing white space (also dropped) change. The `{ ... }` code inside markup is formatted like any code, except the white space right inside its outer braces. A `//` in markup text is text.
- A `system { ... }` file and a `web { ... }` block are only re-indented by their braces.
`cargo test` checks it on every program in the repository (files, and the programs inside `tests/cases`): formatting is idempotent, also from the same files with their indentation removed; only white space differs; the compiler's lexer sees the same tokens before and after (markup text included); and the generated C is the same byte for byte (blank lines kept, `//` comment lines in the C, which quote the source, compared without white space).

## Status (2026-11-05)
Everything above is built. A program or system compiles to one C program (runtime/mito.c, system.c, http.c), built by
clang -O3 or, in milliseconds, the bundled TinyCC; Rust is only the compiler. The command is `mito` (`check`, `build`,
`serve [--watch]`, `test`, `emit-c`, and `check --kinds` listing every local's kind; `mitochondria` is the same program): `serve --watch` rebuilds and restarts on save, keeping
the last good build when a change does not compile. Errors name `file:line:col` with the line and a caret, suggest the nearest
name for a misspelt one, list every independent error of a file, and say what to write for a habit from other languages (`i++`,
`each`); `tests/errors` holds a program per mistake with its expected message. The 51 cases in `tests/cases` and
`tests/http.record` must give the same answers on the TinyCC build, the clang build and a clang build with
undefined-behavior checks. VS Code highlighting is in `editors/vscode`. Windows and Linux: everything the runtime asks of the
system is runtime/os.c (memory, clock, randomness, threads, locks, sockets and poll); on Linux programs build with the system's
clang (`--tcc` uses a `tcc` on the path, else clang -O0), and the cases, the HTTP record and the test runner pass there (WSL2,
Ubuntu 24.04, clang 18).

Limits:
- Functions have full signatures (`fn f(p: Player, n: i64) -> f64`, `-> T?` for a result that may be null, no arrow for none;
  `pool<T>`, `list<T>`, and in modules `T.key`, the type of a field), checked at every call and return. Calls are
  inlined, except a recursive function's and a large one's (past about 4000 statements and expression nodes once its own calls
  are inlined, as a deep tree of text handling grows), each one C function per combination of argument places, shared by every
  endpoint; a text argument's size bound is part of that combination (rounded up to a power of two), so a large function's
  text keeps known sizes, and its region text counts against each caller's as if inlined. Recursion is limited to 64 nested
  calls.
- Only a string input can be optional; a number takes a default. An item input cannot be optional yet, nor an enum input
  read by name.
- A stored id (`Player?` field) names only items of a top-level pool, not of a nested collection.
- A real fault in the C code ends the whole process, every worker with it.
- Pressure marks are absolute megabytes; no idle trim yet.
- `string<N>` reserves N bytes per slot: 2000-byte chat bodies averaging 120 bytes left 39.5 MB committed for 3.1 MB
  live. Packed text is planned (Next, 3).
- HTTP keeps connections open with a fixed pool of connection threads and one poller (WSAPoll, up to 4,096 idle connections);
  an event loop on IOCP would serve more connections with fewer threads. A program's reads run at once on Linux (Readers);
  elsewhere, and for writes, one request at a time.

## Next
1. Linux: done (2026-11-05), see Status. The LangArena runner still pins processes with Windows calls.
2. HTTP: an event loop over a fixed table of connections (IOCP on Windows, epoll then io_uring on Linux) in place of
   the poller and connection threads. Keep-alive is done (2026-11-05): a kept connection costs about 100 µs a request
   on Windows loopback against 1-1.5 ms for a new connection each time.
3. Long uneven text: `text<N>` fields packed into a per-collection text area.
4. Shards placed by load rather than `id % count`.
5. Worker messages beyond `forward`: one-way `send`, fan-out with gathered replies, links and monitors.
