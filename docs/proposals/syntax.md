# Proposal: syntax review (2026-11-04)

Status: **decided, except where marked "open".** Nothing here is built yet. Decisions are Ben's (2026-11-04):

1. One file extension, `.mito`, for everything.
2. One named state per program, holding all of it; the compiler requires exactly one.
3. Collections are used through their functions (`game.players.add()`), not keywords.
4. Stored references name their pool (`ref<game.players>`); locals and parameters name the item type (`Player`).
5. Items can be made with their fields in one statement; JSON keys are unquoted; `+=` and friends exist.
6. Request inputs are typed: no untyped strings for path parameters, the query or the body.
7. Endpoints are ordinary functions with an attribute, `@get("/path")`, `@post("/path")`; no `endpoint` keyword.
8. C-style braces and semicolons stay.

## 1. One extension: `.mito`

Today a program is spread over `.mito` (state), `.api` (functions and endpoints) and `.system` (workers). All of it
becomes `.mito`:

- **A program is one file**: its structs, its state, its functions.
- **A system is a `.mito` file** with a `system { ... }` block naming its worker programs, each one a `.mito` file with
  its own state.
- **A module is a `.mito` file** in `lib/`, in the same syntax as a program (no `--- state` and `--- functions`
  sections): it declares the struct an instance is (section 3) and its functions.
- Test data stays as it is (`tests/cases/*.case` and the recorded `.out`, `tests/http.record`): it is not the language.

## 2. One named state

A program's state is one declaration, naming one struct that holds everything:

```
state game: Game;
```

The compiler requires exactly one `state` per program (none, or two, does not load). `global` goes. Every path starts
at the state's name: `game.players`, `game.board`. Grouping is ordinary nesting: `struct Game { scores: Scores;
chat: Chat; }` gives `game.chat.rooms`.

A reference, or a module instance, needs a collection that exists exactly once, so it may sit anywhere in the state
reached through plain structs (`game.players`, `game.chat.rooms`), but not inside another collection's items (each item
would have its own).

## 3. Collections are used through their functions

Every collection, built in or from a library module, is used the same way: by calling its functions.

| today | proposed |
|---|---|
| `let p = alloc players;` | `let p = game.players.add();` (a `Player`; the request fails with 503 when full) |
| `let p = try_alloc players;` | `let p = game.players.try_add();` (null when full) |
| `free p;` | `game.players.remove(p);` |
| `let m = alloc room.messages;` | `let m = room.messages.add();` (on a full `drop_oldest` list, overwrites the oldest) |
| (not built) | `room.messages.pop()`, `room.messages.clear()` |
| `count(players)`, `capacity(players)` | `game.players.count()`, `game.players.capacity()` |
| `board.add(0)`, `board.remove(e)` | `game.board.add(0)`, `game.board.remove(e)`: `sorted` already works this way |

The keywords `alloc`, `try_alloc` and `free` go. A library collection and a built-in one then look the same, so a
program can switch a pool for a `sorted` collection without rewriting its calls, and the load error "remove items of
board with its functions, not `free`" disappears because there is no other way.

Removal stays deferred as it is now: `game.players.remove(p)` takes effect when the request succeeds, so `p` can still
be read for the rest of the request.

A module instance is a field whose type is the module (`board: sorted<Entry, 1000, points>;`). In the module's
functions `self` is that instance.

## 4. The type of an item

`game.players.add()` returns a `Player`. Locals, parameters and return types name the item type: `fn best(p: Player)
-> i64`. A **stored** reference (a field) also names its pool, because it is a slot number plus a generation of that
pool: `who: ref<game.players>`. If `Player` is kept in more than one collection, a parameter says which:
`p: Player in game.players`.

## 5. Making an item, JSON, assignment

- **An item with its fields**: `game.players.add { name: name, best: 0 }` adds the item and sets those fields in one
  expression; fields not named are zero. For `sorted`, whose `add` takes the key: `game.board.add(0) { who: p }`.
- **Unquoted keys**: `{ best: p.best, top: [each ...] }`. An item written as a value becomes its id (section 6).
- **Compound assignment**: `+=`, `-=`, `*=`, `/=`.

## 6. Typed request inputs

An endpoint's parameters say what it takes, with types; the runtime checks the request against them before the body
runs. `body("name")`, `param("id")`, `query("top")` and `int(...)` on request text go away, so a function can no
longer read the running request behind its caller's back: inputs are values passed to it.

- **Path parameters** are the function's parameters named in the path: `@post("/players/{p}/games")` with
  `fn record(p: Player, points: i64)`. An item type gives a live item of its pool, or answers 404 before the body runs.
- **The other parameters** are the JSON body for POST, PUT and PATCH, and the query string for GET and DELETE. A
  missing one is 400 unless it has a default (`top: i64 = 10`) or is optional (`note: string<200>?`, null when
  absent). A value of the wrong kind is 400 (`points: expected an integer`), one too long 413, one outside a range 400.
- **Types:** `i8`..`i64`, `u8`..`u64`, `f64`, `bool`, `string<N>`, `string<lo..hi>` (a length range, so "required"
  is `string<1..N>`), and integer ranges, `i64 in 0..1000000`.
- **Headers** stay for the runtime's own use (`auth()` reads `X-Session`); typed header parameters can come later.

**Open, recommended:**
- **Reused slots in URLs.** An id that is only a slot number (`/players/3/games`) cannot tell whether slot 3 still
  holds the player the client meant: if player 3 left and someone new took slot 3, the request would act on the new
  player. For a `checked` pool, the id carries the slot's generation too (`"3.7"`), and a stale `3.6` is 404. For a
  pool that is not `checked`, the id stays the slot. Recommended: yes.
- **Unknown fields** (`{"pionts": 500}`): answered 400, `unknown field pionts`, rather than ignored. Ignoring hides the
  typo whenever the field has a default. Recommended: strict.

## 7. Endpoints are functions with an attribute

```
@post("/players", status: 201)
fn add_player(name: string<1..24>) { ... }
```

- `@get`, `@post`, `@put`, `@patch`, `@delete`, each with a path; `{name}` in the path is the parameter of that name.
- The function's **return value is the JSON answer**, status 200, or the attribute's `status:` (for example 201 for
  something created). `fail CODE "message"` ends the request with an error as now. `respond` goes.
- An endpoint function can also be called from other code like any function.

**Open:** an answer whose success status is chosen at run time (200 or 201 by what happened) has no syntax yet;
`fail` covers errors. Recommended: wait until a program needs it.

## The scoreboard in this syntax

```
// scoreboard.mito
struct Player { name: string<24>; best: i64; entry: ref<game.board>; }
struct Entry  { who: ref<game.players>; points: i64; }
struct Played { who: ref<game.players>; points: i64; at: i64; }
struct Game {
    games: i64;
    players: pool<Player, 1000> checked;
    board: sorted<Entry, 1000, points>;
    recent: list<Played, 20> drop_oldest;
}

state game: Game;

@post("/players", status: 201)
fn add_player(name: string<1..24>) {
    let p = game.players.add { name: name };
    p.entry = game.board.add(0) { who: p };
    return { id: p };
}

@post("/players/{p}/games")
fn record(p: Player, points: i64) {
    game.games += 1;
    game.recent.add { who: p, points: points, at: now() };
    if points > p.best { p.best = points; game.board.set_key(p.entry, points); }
    if points > 1000000 { fail 422 "not a believable score"; }
    return { best: p.best };
}

@get("/board")
fn board(top: i64 in 1..100 = 10) {
    return { top: [each e in game.board reverse limit top { name: e.who.name, points: e.points }] };
}

@delete("/players/{p}")
fn remove_player(p: Player) {
    game.board.remove(p.entry);
    game.players.remove(p);
    return { ok: true };
}
```

## Questions for Ben

1. Section 6: ids of `checked` pools as `"slot.generation"`, and unknown fields rejected? (Both recommended.)
2. Section 7: `status:` in the attribute for the success status, and `return` as the answer: right?
3. How a system file reads (section 1): worth a sketch before building, or decide while building?
