# Proposal: syntax review (2026-11-04)

Status: **for review.** Nothing here is built yet. Decided by Ben so far: one file extension for everything, and
collections used through their own functions (`players.add()` returns the new `Player`), not keywords.

## 1. One extension: `.mito`

Today a program is spread over `.mito` (state), `.api` (functions and endpoints) and `.system` (workers). Proposed: all
of it is `.mito`.

- **A program is one file**: its state, structs, functions and endpoints together.
- **A system is a `.mito` file** with a `system { ... }` block naming its worker programs (each one a `.mito` file).
- **A module is a `.mito` file** in `lib/`, as now, but written in the same syntax as a program (no `--- state` and
  `--- functions` sections).
- Test data stays as it is (`tests/cases/*.case` and the recorded `.out`, `tests/http.record`): it is not the language.

## 2. One state block

Ben prefers the whole state as one global, built from nested structs. Proposed: a program's state is one `state { ... }`
block, which is that global. Its fields are named directly (`players`, not `app.players`), so one global costs no
extra typing:

```
struct Player { name: string<24>; best: i64; entry: ref<board>; }
struct Entry  { who: ref<players>; points: i64; }
struct Game   { who: ref<players>; points: i64; at: i64; }

state {
    games: i64;
    players: pool<Player, 1000> checked;
    board: sorted<Entry, 1000, points>;
    recent: list<Game, 20> drop_oldest;
}
```

Deeper grouping is ordinary nesting, `state { scores: Scores; chat: Chat; }`, and paths follow it (`chat.rooms`). A
reference or a module instance needs a collection that exists once, so it may sit anywhere under `state` through plain
structs, but not inside another collection's items.

## 3. Collections are used through their functions

Every collection, built in or from a library module, is used the same way: by calling its functions.

| today | proposed |
|---|---|
| `let p = alloc players;` | `let p = players.add();` (a `Player`; the request fails with 503 when full) |
| `let p = try_alloc players;` | `let p = players.try_add();` (null when full) |
| `free p;` | `players.remove(p);` |
| `let m = alloc room.messages;` | `let m = room.messages.add();` (on a full `drop_oldest` list, overwrites the oldest) |
| — (not built) | `room.messages.pop()`, `room.messages.clear()` |
| `count(players)`, `capacity(players)` | `players.count()`, `players.capacity()` |
| `board.add(0)`, `board.remove(e)` | unchanged: `sorted` already works this way |

The keywords `alloc`, `try_alloc` and `free` go. A library collection and a built-in one then look the same, so a
program can switch a pool for a `sorted` collection without rewriting its calls. The current load error "remove items of
board with its functions, not `free`" disappears too, because there is no longer another way.

Removal stays deferred as it is now: `players.remove(p)` takes effect when the request succeeds, so `p` can still be read
for the rest of the request.

## 4. The type of an item

`players.add()` returns a `Player`. A local or a parameter can say so: `fn best(p: Player) -> i64`. A **stored**
reference (a field) also has to know which pool it points into, because it is a slot number plus a generation. Options:

- **A (recommended):** fields keep naming the pool, `who: ref<players>`; locals, parameters and return types name the
  item type, `Player`. If `Player` is stored in more than one collection, a parameter says which: `p: Player in players`.
- **B:** fields name the item type too, `who: Player in players`, so `ref<...>` goes away entirely.

## 5. Making an item with its fields

Optional, and composes with section 3: `players.add { name: name, best: 0 }` adds the item and sets the fields in one
statement. Fields not named are zero. Without it, `let p = players.add(); p.name = name;` still works.

## 6. Smaller items, each for a decision

- **Request inputs as typed parameters**, not lookups by name. `POST /players/{p: players}/games (points: i64)` would
  resolve `p` to a live player (or 404) and parse `points` (or 400) before the body runs. Then a function such as
  `player()` no longer reads the running request behind the caller's back: `param()` inside a `fn` goes away.
- **Unquoted keys** in `respond`: `respond 200 { best: p.best };`. An item written as a value becomes its id.
- **Compound assignment**: `games += 1`.

## The scoreboard in this syntax

With sections 1-5 (option A) and the items in section 6:

```
// scoreboard.mito
struct Player { name: string<24>; best: i64; entry: ref<board>; }
struct Entry  { who: ref<players>; points: i64; }
struct Game   { who: ref<players>; points: i64; at: i64; }

state {
    games: i64;
    players: pool<Player, 1000> checked;
    board: sorted<Entry, 1000, points>;
    recent: list<Game, 20> drop_oldest;
}

endpoint POST /players (name: string<1..24>) {
    let p = players.add { name: name };
    p.entry = board.add(0);
    p.entry.who = p;
    respond 201 { id: p };
}

endpoint POST /players/{p: players}/games (points: i64) {
    games += 1;
    recent.add { who: p, points: points, at: now() };
    if points > p.best { p.best = points; board.set_key(p.entry, points); }
    if points > 1000000 { fail 422 "not a believable score"; }
    respond 200 { best: p.best };
}

endpoint GET /board (top: i64 = 10) {
    respond 200 { top: [each e in board reverse limit min(top, 100) { name: e.who.name, points: e.points }] };
}

endpoint DELETE /players/{p: players} {
    board.remove(p.entry);
    players.remove(p);
    respond 200 { ok: true };
}
```

## Questions for Ben

1. Section 2: one unnamed `state { ... }` block, or one named global (`global game: Game;`) with `game.` in every path?
2. Section 4: option A or B?
3. Sections 5 and 6: which of them, if any?
4. Anything else in the syntax you want changed: braces and semicolons, `let`/`var`, `endpoint`?
