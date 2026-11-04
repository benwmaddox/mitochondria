# Chat API on Mitochondria: what the first prototype taught (2026-11-03)

The first test of the language was a Campfire-style chat server's hot state and API (sessions, rooms, messages, typing)
on a tree-walking interpreter. It showed what fit and what was missing; every gap below shaped the current design.

**What fit from the start.** Capacity became part of the API (a full pool 503, a long string 413, a dead slot 404).
Checked pools gave real session hygiene: a deleted session's token stays rejected after its slot is reused. Memory was
visible from inside the app (`/stats`). Handlers were short.

**What was missing, and what it became:**

| Gap found | Now |
|---|---|
| A failed request leaked state (ghost rooms filled the pool) | every request is atomic: a write journal rolls it back exactly |
| No index: "newest 50 messages of a room" scanned all 20,000 (2.9 ms) | nested collections: a room's own list, read from the end (52 µs, then 2.5 µs compiled) |
| Evicting the oldest message sorted everything (951 µs a post) | `list<T, N> drop_oldest`, a ring (4.3 µs) |
| `body: string<2000>` left 39.5 MB committed for 3.1 MB live | still open: packed text is planned (DESIGN.md, Next) |
| No functions or mutable locals; auth copied into every endpoint | `fn` and `var`; then the session became a typed input |
| Author names copied into messages | `ref<chat.sessions>` with `or(m.author.name, "[deleted]")` |
| Declared maxima invisible to code | `x.capacity()`, `maxlen(s)` |
| Temporaries on the heap; single-threaded; interpreter-bound | no heap at all; workers on threads; compiled to C |

Numbers for each step: docs/measurements.md (Rounds 3 and after).
