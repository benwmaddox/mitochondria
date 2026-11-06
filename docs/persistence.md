# Persistence and outside libraries (design, 2026-11-05)

Today a program's state lives only in its process: a restart, a crash or `serve --watch` rebuilding it starts from empty. This page records
how data will be kept, and how programs reach outside C libraries. Built since (2026-11-05/06): `table<T>` with everything below,
`extern` tables, `time` and nullable text columns; declared C functions are not built yet (system libraries come in as runtime built-ins).

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

## Stored data: `table<T>` (decided 2026-11-05)

A table is a collection kind beside `list` and `pool`, and it lives in SQLite. Everything else stays in memory as now.

| Question | Decision |
| --- | --- |
| Declaring | `boards: table<Board>;` a collection kind; a table can be a field of the working state or of another table's items. |
| Limits | None: a table never takes a capacity (`table<Board, 64>` is an error). Text keeps its maximum (`utf8<1000>` is a checked limit, 413 when longer), arrays their length. |
| What a table's items hold | Plain fields (numbers, bools, text, enums, fixed arrays) and nested tables (a board's todos: a child table whose rows name their board). A `pool` or `list` inside a table's item is refused: it would be memory per row. |
| Indexes | Declared on the collection, so the struct stays reusable: `users: table<User> index(email) unique(name);`. |
| Slow scans | A `where` over a whole top-level table on a field without an index is refused when compiling, with the index to declare; loops over one parent's children (a board's todos) are always allowed. |
| Order | Insertion order by default (row ids are never reused); `foreach u in data.users by name` walks an indexed field's order, refused without that index. |
| Ids | An item's id is its row id, never reused, so a stored id or a session token (`rowid.secret`) cannot name a later item. |

### Sketch: TodoMVC

```
struct Todo { title: utf8<1000>; done: bool; }
struct Board { secret: i64; todos: table<Todo>; }    // a child table: each todo row names its board
struct App { boards: table<Board>; }
state app: App;                                      // app.boards is in SQLite (--data dir); nothing of it in memory

@post("/todos/{id}/toggle")
fn toggle(board: Board from cookie("todos"), id: Todo in board.todos) -> Response<html> {
    id.done = !id.done;                              // UPDATE todo SET done = ?1 WHERE id = ?2
    return see_other("/");
}
fn lists(b: Board, show: i64) -> html {
    foreach t in b.todos where !t.done { ... }       // SELECT id, title FROM todo WHERE board = ?1 AND done = 0 ORDER BY id
}
```

How the program's operations become SQL (statements prepared once at start, typed, no SQL text built per request):

| In the program | In SQLite |
| --- | --- |
| an item struct of a table | a table; each plain field a column; a nested `table` a child table with a column naming the parent |
| `app.boards.add() { secret: s }`, `b.todos.add() { title: t }` | one `INSERT` naming those columns (the assignments to the new row right after an `add` count as its fields), so NOT NULL columns without defaults get their values at once; the item's id is its row id |
| `t.done = !t.done` | `UPDATE ... WHERE id = ?` (the writes of one statement to one row merged into one `UPDATE`) |
| `foreach t in b.todos where ...` | one `SELECT` of the columns the loop reads, `ORDER BY id` (or the `by` field: one an index starts with, or any field when an index leads the `where`) |
| `foreach m in app.messages by created_at where m.room == r last 40` | the last 40 in that order, walked in order: `SELECT * FROM (... ORDER BY created_at DESC, id DESC LIMIT 40) ORDER BY created_at, id` (Rails' `.last(40)`) |
| `b.todos.count()`, `count(... where ...)` | `SELECT COUNT(*) ...` |
| `b.todos.remove(t)` | `DELETE` (and the rows that belong to it) |
| an item from the path or a cookie | a lookup by row id (plus the `secret` check for a session token) |
| the request | one transaction: committed before the answer, rolled back with the working state's undo log on a failure |

Every read of a table is a query (fast for one row by id or an indexed field, a scan otherwise); in exchange a table has no fixed size,
and removing any item keeps the others in insertion order (TodoMVC's `order` array goes away).

### Columns beyond plain fields (2026-11-06)

| In the row's struct | Its column |
| --- | --- |
| `owner: User?` (a reference) | `owner_id INTEGER`, NULL when unset, set to NULL when its row goes |
| `bio: utf8<4000>?` (nullable text; table rows only) | `bio TEXT` that may be NULL: read as null, written with `= null` |
| `connected_at: time?` (a nullable time) | the same, NULL when unset: read as null, cleared with `= null` |
| `created_at: time` | text as Rails and SQLite's date functions write it, `2026-03-02 16:00:00` with `.ffffff` only when there are microseconds; in the program an i64 of microseconds since 1970 (UTC); `format_time(t, "%Y%m%d%H%M%S")` writes one out |

### Another program's database: `table<T> extern` (2026-11-06)

A program can work on a database another program owns (the Campfire port on Rails' `production.sqlite3`): every table declared
`users: table<User> unique(email_address) extern;`, the file named with `--db FILE` (`--data` is not used). Then:

- the database must exist (it is never created); every table and declared column must be there, of a type the program reads by SQLite's
  affinity rules (Rails' `varchar`, `text`, `json` as text, `datetime(6)` as a time, `integer`/`bigint`, `boolean`), not NULL where the
  field has no `?`, and the declared indexes must exist (`unique` as a unique index); otherwise the server refuses to start, naming it;
- nothing is created, migrated, dropped or scanned at start (the other program owns the schema); columns the program does not declare are
  left alone;
- values are read as the program declares them whatever SQLite holds (such tables are not STRICT); text longer than the field's limit, or
  a time that is not one, fails that request (500) naming the column;
- a NOT NULL column without a default that the program does not declare means the program can read and change rows but not add them:
  `add` fails, saying which column; tables are a table of their own each (no nested tables);
- a program's tables are all its own or all another's: one database.

## Build order

1. SQLite's amalgamation vendored (`third_party/sqlite`), compiled into programs that have a table, given a fixed memory pool.
2. `table<T>` in the layout: parsed with `index(...)` and `unique(...)`, kept out of the fixed reservation; the schema generated from it.
3. The runtime: the database opened in `--data` at start, the schema created or migrated by name (refusing what does not fit), one
   transaction per request committed before the answer.
4. Code: `add`, field reads and writes, `remove`, `count`, `foreach ... where ... by ...`, items from the path or a cookie, nested tables,
   compiled to prepared statements; the slow-scan check.
5. TodoMVC on tables (kept across restarts), its browser checks, and the benchmark against crudspec again.
6. Declared C functions for other outside libraries.

## Future option: keeping the in-memory state itself

Kept here as possible later work, not planned now. The working state is one fixed reservation at fixed offsets, and every request is all or
nothing, so it could be kept without a database:

- **Snapshots:** between two requests, write the live pages with the layout's description; load them on start.
- **A change log:** the undo log already knows every 64-byte line a request writes; after a request succeeds, append the new contents of
  those lines, flushed before answering (requests arriving together share a flush). Recovery is the last snapshot plus the log; replay runs
  no program code, so it is exact.
- **Migration by name** between layouts, carried in the snapshot, refusing what no longer fits.

This would make working state durable at memory speed (no SQL per read), at the cost of the data not being inspectable with database tools.
