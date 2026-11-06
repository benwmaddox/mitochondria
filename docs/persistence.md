# Persistence and outside libraries (design, 2026-11-05)

Today a program's state lives only in its process: a restart, a crash or `serve --watch` rebuilding it starts from empty. This page lays out
the ways to keep it, what each costs, and a recommendation. Nothing here is built yet.

## What the language already gives

- **One fixed layout.** All state is one reservation at fixed offsets; nothing points outside it (items name each other by slot or stored id).
  Copying the bytes is a complete copy of the program's state.
- **Atomic requests.** Every request is all or nothing: the undo log holds the old bytes of each 64-byte line a request writes, and a
  failed request writes them back. So there is a clean boundary after every successful request.
- **One request at a time per program.** Between two requests nothing is half written; a worker of a system is the same.
- **Determinism, almost.** A request's effect depends on the state, the request, the clock (`now()`) and randomness (`random()`, session
  secrets). The runtime already fixes the last two for tests (`--clock`, `--seed`).
- **What changes between builds.** A program edited and rebuilt can have a different layout: fields added, removed, renamed, resized,
  collections with new capacities. Any saved state that outlives a build has to survive that.

## The options

### A. Snapshots only

Between two requests, write the state's live pages to a file (with the layout's description); on start, read it back. Taken every N seconds,
every N requests, and on a clean shutdown.

- Cost: nothing per request; a snapshot is the live bytes (the committed pages, not the reservation).
- Durability: whatever happened since the last snapshot is lost on a crash.
- Across builds: the snapshot is bytes of one layout; loading it into another needs a migration (option E).
- Good for: development (`serve --watch` keeping state), caches and game state that can lose seconds.

### B. A request log (and snapshots to keep it short)

Append every successful request to a file before answering it: method, path, the inputs the endpoint read, and the clock and random values
it used. On start, load the last snapshot and replay the requests after it. A snapshot lets the log before it be dropped.

- Cost per request: one append of the request (usually tens to hundreds of bytes); durable when the append is flushed (`fsync` per request,
  or a group of requests per flush, answering them after it).
- Durability: every answered request, with `fsync` before answering.
- Replay is exact only if the program is the same: the log is of requests, not of their effects. Across builds, replaying old requests
  through new code gives new results (sometimes wanted, often not), so a build change takes a snapshot first and starts a new log.
- Needs: `now()` and `random()` recorded per request (the runtime reads them in one place each), and the system's forwards logged where
  they arrive.
- Good for: servers whose requests are small and whose state matters; also gives an audit trail and a way to reproduce a bug.

### C. A change log (redo of written lines)

The undo log already knows every line a request writes. After a request succeeds, append the new contents of those lines (offset and 64
bytes each) to a file; recovery is the last snapshot plus the lines after it, applied in order.

- Cost per request: 64 bytes (plus an offset) per line written, so a request writing a 4 KB string field logs 4 KB; a flush per request or
  per group, as in B.
- Durability: as B. Replay needs no program code and is exact, whatever the code does.
- Across builds: the lines are bytes of one layout, so as A a build change takes a snapshot (migrated, E) and starts a new log.
- Good for: when requests are expensive to rerun or depend on things outside the program; it is the classic database design (write-ahead log).

### D. The state as a mapped file

Map a file as the reservation itself, so the operating system writes changed pages back.

- Cost: none per request until the system writes pages; very little code.
- Durability and consistency: the system writes pages when it chooses, so after a crash the file can hold half of a request (a torn
  write) unless every request also logs (C) and the file is only trusted up to a checkpoint. On its own it is not crash-safe.
- Good for: nothing on its own; as the snapshot mechanism of A or C it saves the copy.

### E. Migration between layouts (needed by A, B and C)

A saved state carries its layout's description (the struct and field names, types, capacities, enum values). Loading it into a program
with another layout maps it by name: a field kept keeps its value, a new field starts zero, a removed field is dropped, a collection
keeps its items up to the new capacity (more is an error that names it), a string longer than its new maximum is an error that names it,
an enum value that no longer exists is an error. Items keep their slots, so ids and stored ids stay valid.

- This is what lets `serve --watch` keep state while a program is edited, and a deployed program be upgraded without losing data.
- Renames need a hint in the program (`renamed from old_name`); otherwise a rename is a removal and an addition.
- It runs once per load, not per request.

## Recommendation

1. **E and A first:** layout-described snapshots with migration by name, taken on clean shutdown, on `serve --watch` rebuilds and every
   N seconds. That gives `serve --watch` its kept state (the roadmap's wish), restarts without loss, and the migration that every later
   step needs.
2. **Then C for durability:** a line log after each successful request, flushed in groups (answers wait for their group's flush, a few
   milliseconds), with a snapshot to start each log. C over B because replay does not run program code, so it is exact even when the code
   or the outside world changed, and the undo log already collects exactly what to write.
3. **B as an option later:** a request log for audit and for reproducing bugs, not for recovery.

## Decisions (Ben, 2026-11-05)

| Question | Decision |
| --- | --- |
| Durable by default? | Yes. A request is answered only after its changes are flushed to the log (C); requests that arrive together share one flush, so it costs a few milliseconds of latency, not throughput. A program can opt out to snapshots only (A). |
| Migration that does not fit | Refuse to start, naming each thing that does not fit (a collection now too small, a string too long, an enum value gone). Nothing is dropped silently; a flag drops it deliberately. |
| Where state files live | A directory named on the command line, `--data dir` (default `./data` where the server runs): snapshots and logs there, one set per program, and per worker in a system. |
| Outside libraries | Both layers: a general way to declare C functions with typed signatures and the library to link, checked at the boundary; and curated modules on top (`lib/sqlite` first) that wrap common libraries safely, the library's memory given from a declared fixed pool (SQLite's `SQLITE_CONFIG_HEAP`), so peak memory is still known before the program runs. |

How the parts fit: the program's own state (one fixed reservation, atomic requests) is kept by snapshots plus the change log; outside
libraries are for what fixed state holds poorly (data of unknown size, ad-hoc queries), not for keeping the state itself.

## Build order

1. The layout's description saved with a snapshot; snapshot on clean shutdown and load on start from `--data`; migration by name,
   refusing what does not fit.
2. The change log: after each successful request, the new contents of the lines it wrote appended; answers wait for their group's
   flush; recovery is the last snapshot plus the log; a snapshot now and then starts a new log.
3. `serve --watch` keeps state across rebuilds (snapshot, rebuild, migrate, load); systems keep one set of files per worker.
4. Declared C functions: signatures with the language's kinds (integers, floats, bools, text in and out through declared storage),
   the library named for the build to link.
5. `lib/sqlite`: SQLite through those declarations, given a fixed memory pool.
