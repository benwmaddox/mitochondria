# Reference: commands, options, built-ins and libraries

The design, in order of how a program is written, is [DESIGN.md](../DESIGN.md); [tutorial.md](tutorial.md) builds a program step by step. This page lists
what can be typed: every command, every option of a built executable, every built-in that DESIGN does not describe where it first comes up, and the library modules.
A test (`docs_mention_everything` in src/platform.rs) fails when a built-in, option, command, library module or example is in none of the docs, so this page stays complete.

## Commands

`mito <command> <program> [options]`. A program is one `.mito` file (the entry file; it may `use "other.mito";` more files) or a system of workers (a file holding `system { ... }`).

| Command | What it does |
| --- | --- |
| `check <program>` | reports the problems found, or says the program loads. `--kinds` lists the kind of every local; `--costs` lists each endpoint's work (bytes zeroed, queries, row reads, table writes, text built). |
| `build <program> [-o app.exe] [--tcc] [--cflag X]` | an executable: a server (platform web) or a tool (platform cli). Built with clang `-O3`; `--tcc` builds in milliseconds with TinyCC, unoptimised; `--cflag X` (repeatable) hands X to clang in place of `-O3` (`--cflag -O2 --cflag -mavx2`). |
| `run <program> [--clang] [-- args]` | builds and runs a command-line program (`@main`) at once. What follows `--` is its command line; stdin and stdout are yours. The exit code is the program's. |
| `serve <program> [--clang] [--watch] [executable options]` | builds with TinyCC (or clang) and runs a server at once; `--watch` rebuilds and restarts when a file of the program changes (state starts empty each time). |
| `test <program> [--clang]` | runs the program's `test "name" { ... }` blocks, each on a fresh state with the clock fixed; exit code 1 if one fails. |
| `emit-c <program> [-o file.c]` | the generated C, to read or build yourself (a system: `-o dir`, one file per worker kind and system.c). |
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

## Built-ins described here

DESIGN.md describes most built-ins where they come up (text, JSON, time, crypto, HTML, collections). These are the rest.

| Built-in | What it does |
| --- | --- |
| `abs(x)` | absolute value of an integer or a float |
| `min(a, b)`, `max(a, b)` | the smaller or larger of two numbers |
| `sqrt(x)`, `exp(x)`, `floor(x)` | the platform's math functions on a float (a negative square root is NaN); an integer is taken as a float |
| `wrapping_add(a, b)`, `wrapping_sub(a, b)`, `wrapping_mul(a, b)` | two's-complement arithmetic that wraps at 64 bits (the operators `+ - *` fail the request on overflow); for hashes and generators |
| `x as TYPE` | a conversion that keeps the low bits (DESIGN.md, Conversions) |
| `add_years(t, n)` | a time `n` calendar years later (or earlier), the same day and time of day; February 29 becomes February 28 where the year has none |
| `all_word(t)` | whether every character of `t` is a word character (letters, marks, numbers, connector punctuation, as Ruby 3.4's `\p{Word}`) |
| `text_from(bytes, from, count)` | bytes of a state array of `u8` (plain state or an item's) as text copied into the request's text area, range checked |
| `html_text(x);` | in a markup function (`-> html`), writes `x` escaped, as `{x}` does inside an element |
| `stat("committed_kb")`, `stat("live_kb")`, `stat("reserved_kb")` | memory figures of this program, in KB (committed, live, reserved) |
| `worker_index()`, `worker_count()`, `worker_count("kind")` | in a system of workers, which worker this is (0 up), how many workers there are of its kind, and of a named kind; outside a system 0 and 1 |

## Entry points

`@get("/p")`, `@post`, `@put`, `@patch` and `@delete` are endpoints (a function with an HTTP route and typed inputs; `@get` and `@delete` read their inputs from the query, the others from the body, a path
`{param}`, `from header(...)` or `from cookie(...)`). `@socket`, `@socket_message` and `@socket_close` handle WebSockets, `@every(ms)` runs on a clock, `@start` once when the program starts, and
`@main` is a command-line program's one function. A program has the entry points of its platform only.

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

## Examples

| Example | Platform | What it shows |
| --- | --- | --- |
| `examples/scoreboard` | web | the smallest program: state, endpoints, tests |
| `examples/chat` | web | sessions, rooms, typing, a request-rate limit |
| `examples/chat-sharded` | web (a system) | workers on several threads, forwarding between them |
| `examples/todomvc` | web | server-rendered pages, tables in SQLite, the answer cache (53 browser checks) |
| `examples/notes` | web | a multi-file app on tables: accounts with bcrypt, per-user notes, search and tags |
| `examples/jx` | cli | a small jq in three files: positions in the input text, no tree, no allocation |
