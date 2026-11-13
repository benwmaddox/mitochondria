# Mitochondria

An experimental language in which a program's memory is declared up front: every collection, string and array has a
maximum, and the program's whole state is one struct of them.

```
struct Room { name: string<48>; messages: list<Msg, 1000> drop_oldest; }
struct Chat { sessions: pool<Session, 2000> checked; rooms: pool<Room, 64> checked; }
state chat: Chat;
```

**Reserved, not allocated.** From those maximums the compiler knows the exact size and place of everything, and the
program reserves that address space once, when it starts. Reserving costs no memory.

**Grows and shrinks by pages.** A page takes physical memory only when data is first written to it, and with memory
pressure on (`--soft`, `--hard`) pages are committed only where data is written and given back when the data in it is gone, in a
pressure-driven trim. Without pressure, state up to 1 GiB is committed whole at start (commit charge, not memory), so writes never test for it. A program declared for 64 rooms of 1,000 messages that holds 3
messages costs memory for 3. There is no heap allocator and no garbage collector.

**Access is a lookup.** Because every collection's place is fixed when the program is built, reading data is
arithmetic on an address, not a search: an id is a slot, so `game.players[id]` goes straight to that player, and a
room's messages sit together where the room's slot says. There are no pointers to follow, no hash maps and no queries
to run; an index is checked for liveness on the way.

**No hidden storage.** Text a program builds has a declared place too: a field of the state, or of a `request` struct that
is emptied before each request and never kept. `s += x` appends in place, and running past a field's maximum is an error
naming that field. A `let` may hold text it builds, and a `var` text up to a cap it declares (`var acc: text<4096> = "";`), in its
scope's region, given back when the scope ends; a function may return text built for the request, which goes with it. Each endpoint empties only the parts of the request struct its code names.

**What that changes when you design.** You decide limits early, per thing: how many sessions, how long a name, how
many messages a room keeps. Reaching a limit is an ordinary error the program answers (full, too long), never a crash or
an out-of-memory kill, and peak memory is known before the program runs. In return, data whose size you cannot bound
(documents, large uneven text, graphs of unknown shape) fits poorly; it belongs in a database or another language.

New here? [Your first hour](docs/first-hour.md) is the short version. Then the [tutorial](docs/tutorial.md) (a web program, tests, a command-line tool, functions of a type, struct parameters, constants, generic structs, the standard library, a game). The design is in [DESIGN.md](DESIGN.md); every command, option, built-in and library module is listed in [docs/reference.md](docs/reference.md).

**Kinds of programs.** A program is built for a *platform*, which is its entry file's: a **web** program has endpoints (`@get`, `@post`, ...) and is a server; a **cli** program has one `@main` function and is a
command-line tool (arguments, stdin and flags in, stdout out, an exit code); a **game** program has an `@frame` function and `@shader`s and opens a window (SDL3 and OpenGL, loaded when it starts); a **test** program is tests only. `platform cli;` on a line of its own says so, or the entry points show it. A platform is a driver plus a list of
named libraries, and the compiler refuses what does not belong to it (a cookie in a command-line tool, a table without SQLite).

```
mito serve examples/todomvc/todomvc.mito --port 8080                       # a server
echo '{"a":[1,2,3]}' | mito run examples/jx/jx.mito -- '.a[1]'             # a command-line tool: prints 2
mito build examples/jx/jx.mito -o jx                                         # a standalone executable
```

**Examples.** Each is checked by its own tests.

| | Platform | |
| --- | --- | --- |
| [examples/scoreboard](examples/scoreboard) | web | the smallest program: state, endpoints, tests |
| [examples/guestbook](examples/guestbook) | web | a first program: limits, a full collection, a test for it (see [your first hour](docs/first-hour.md)) |
| [examples/chat](examples/chat), [chat-sharded](examples/chat-sharded) | web | sessions and rooms; workers on several threads |
| [examples/todomvc](examples/todomvc) | web | server-rendered pages, tables in SQLite, the answer cache |
| [examples/notes](examples/notes) | web | a multi-file app on tables: accounts, per-user notes, search |
| [examples/jx](examples/jx) | cli | a small jq in three files |
| [examples/world](examples/world) | game | an open world to walk through, painted in broad brush strokes by one fragment shader |

**Web pages.** Programs can answer HTML written as markup in the code, take HTML form posts and cookies, and serve their
stylesheet and script compiled in. [examples/todomvc](examples/todomvc) is TodoMVC rendered on the server, checked in a browser
against the app spec with and without script. Against crudspec's build of the same app on Linux: [bench/todomvc/RESULTS.md](bench/todomvc/RESULTS.md).

**Benchmarks.** 41 [LangArena](https://github.com/kostya/LangArena) tests ported and measured against LangArena's Rust and Crystal
implementations, time and peak memory per test: [bench/langarena/RESULTS.md](bench/langarena/RESULTS.md) (written by the benchmark script;
how the comparison is made, and each port's audit, in [bench/langarena/README.md](bench/langarena/README.md)).
The same on Linux (WSL2, no Crystal there): [bench/langarena/RESULTS-linux.md](bench/langarena/RESULTS-linux.md).
Ordered maps (`lib/btree.mito`, `lib/ordmap.mito`) against Rust's `BTreeMap`, per workload at 10,000 and 1,000,000 keys:
[bench/ordered/RESULTS.md](bench/ordered/RESULTS.md) (written by `bench/ordered/results.py`).

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
