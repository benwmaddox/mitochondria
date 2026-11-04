# Syntax (decided and built, 2026-11-04)

The decisions behind the current syntax, with the reason for each. DESIGN.md describes the result.

| Decision (Ben's unless noted) | Why |
|---|---|
| One extension, `.mito`, for programs, systems and modules; a program is one file | state and code are always compiled together and read together |
| One named state per program (`state game: Game;`), required exactly once; `global` gone | everything in one struct, built by nesting; paths say where data lives |
| Collections used through their functions (`add`, `try_add`, `remove`, `count`, `capacity`), not `alloc` / `free` keywords | built-in collections and library modules look the same; one way to do each thing |
| Stored references name their pool (`ref<game.players>`); locals and parameters name the item type (`Player`) | a stored reference is a slot and generation of one pool; a local only needs its type |
| `add { field: value }`, bare JSON keys, `+=` and friends | less bookkeeping per line |
| Request inputs are typed parameters, checked before the body runs; no `body()`, `param()`, `query()` | the signature says what an endpoint takes; no function reads the request behind its caller's back |
| A session is a typed input (`who: Session from header("X-Session")`); forwarded fields too (`forwarded user: string<16>`) | the same rule for every input, between workers included |
| Unknown body fields are 400 (strict, for now) | a client's typo is caught instead of silently using a default |
| Ids of checked pools carry the generation (`"3.7"`) | a URL naming a reused slot gets a 404, not the new occupant |
| Endpoints are functions with an attribute (`@get("/path")`), returning `Response<T>`; `ok`, `created`, `no_content`, `status` set the status | less to learn: an endpoint is a function; the answer's shape is typed and checked, as C#'s `ActionResult<T>` |
| No `any`: a `var` keeps one kind, a function returns one kind (decided while building, Ben agreed) | every kind known when compiling, so every answer can be checked |
| C-style braces and semicolons stay | familiar |

The scoreboard in this syntax: `examples/scoreboard/score.mito`, explained step by step in docs/tutorial.md.

**Built how.** In stages, each converted by script and checked against the recorded answers before the next. The
recorded answers changed only where intended: inputs checked at the edge (error texts, ids, one request with an unknown
field) and endpoints that needed a kind changing at run time, which no longer load. docs/measurements.md, Round 17.
