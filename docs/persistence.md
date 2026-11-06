# Persistence and outside libraries (design, 2026-11-05)

Today a program's state lives only in its process: a restart, a crash or `serve --watch` rebuilding it starts from empty. This page records
how data will be kept, and how programs reach outside C libraries. Nothing here is built yet.

## Decisions (Ben, 2026-11-05)

| Question | Decision |
| --- | --- |
| Where kept data lives | In SQLite: the state is the database. Stored collections are tables and their fields columns; reads and writes in the program compile to SQL. |
| Scope | Two kinds of data, declared in the program. **Stored** data lives in SQLite (kept across restarts, sizes not fixed). **Working** state stays in memory as now (fixed size, for scratch arrays, caches, computation: the LangArena ports keep theirs in memory). TodoMVC's lists are stored. |
| Durability | Every answered request is kept: a request is one SQLite transaction (with the working state's undo log, all or nothing together), committed before the answer is sent. |
| Writes from outside | Other tools may read and query the database at any time; only the server writes while it runs. Edits made while it is stopped are loaded on start, checked against the program's declarations. |
| SQLite itself | Its single-file amalgamation (public domain) is vendored and compiled into every program that stores data: no system dependency, one version everywhere. |
| Schema changes between builds | The program's declarations are the schema. On start the database is migrated by name (a field added gets its column, a removed one is dropped); what cannot be carried (a value that no longer fits its type, an enum value gone) refuses to start, naming it, unless a flag drops it deliberately. |
| Where files live | A directory named on the command line, `--data dir` (default `./data` where the server runs): the database there, one per program, and per worker in a system. |
| Outside libraries | Both layers: a general way to declare C functions with typed signatures and the library to link, checked at the boundary; and curated modules on top that wrap common libraries safely (SQLite is used by the compiler directly for stored data; `lib/` wrappers for others as needed). |

## Still to decide (an interview once there is a sketch)

- How stored data is declared (a keyword on a collection or struct field), and what a stored collection's limits are (unbounded, or an
  optional maximum the program checks).
- How loops, `where`, counts and lookups over stored collections compile to SQL, and which are refused for being slow (a scan with no index).
- Ids of stored items (SQLite's row id), and references between stored items and from working state to stored items.
- Text and arrays in stored items: maximum lengths kept as checks, or unbounded.
- How a request's view of stored data stays consistent with its writes (one transaction per request makes this SQLite's job).

## Future option: keeping the in-memory state itself

Kept here as possible later work, not planned now. The working state is one fixed reservation at fixed offsets, and every request is all or
nothing, so it could be kept without a database:

- **Snapshots:** between two requests, write the live pages with the layout's description; load them on start.
- **A change log:** the undo log already knows every 64-byte line a request writes; after a request succeeds, append the new contents of
  those lines, flushed before answering (requests arriving together share a flush). Recovery is the last snapshot plus the log; replay runs
  no program code, so it is exact.
- **Migration by name** between layouts, carried in the snapshot, refusing what no longer fits.

This would make working state durable at memory speed (no SQL per read), at the cost of the data not being inspectable with database tools.
