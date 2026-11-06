# SQLite 3.54.1 amalgamation, bundled

Programs with a `table<T>` keep its rows in SQLite (docs/persistence.md). This directory holds the unmodified amalgamation files:

- `sqlite3.c`: all of SQLite as one C file, compiled into each program that has a table (compiled once per compiler and flags,
  then reused from a cache, so a build does not recompile it).
- `sqlite3.h`: its interface.

Source: https://www.sqlite.org/2026/sqlite-amalgamation-3540100.zip
(SHA3-256 92281c86e2957f9b9305890f9ac04204caa7b677cc868dc326d942689df28e30, as listed on https://www.sqlite.org/download.html).

Licence: SQLite is in the public domain (https://www.sqlite.org/copyright.html). It is not covered by Mitochondria's Apache-2.0 licence,
and needs none.
