# Roadmap: syntax and developer experience

Written 2026-11-05 from a review of the language as it stands (after the LangArena ports, `unlogged`, range-typed fields and `T?`).
Items are worked through roughly in order; each lists the decision taken, what is still open, and when it counts as done.
Decided against, for now: request-local scratch arrays (unlogged state, now the default, covers that need).

## Phase 1: the everyday loop

### 1.1 Line numbers in every error (done 2026-11-05)
- Compile errors name `file:line:col` and quote the line with a caret under the place: `app.mito:14:31: no field `totl` (did you mean `total`?)`.
- The lexer keeps a position per token; the parser and the binder carry it on expressions and statements; code generation
  errors use the position of the expression they are about.
- Runtime errors name the endpoint and the source line in the message: `{"error":"integer overflow","at":"app.mito:22 (POST /games)"}`.
- Done when: every message in `tests/cases` and the compile-error tests has a position, and the runtime test cases show them.
- Done: compile errors print `file:line:col`, the line and a caret (parse errors at the token; binding and code-generation errors at
  the start of their statement; layout errors at the declaration they name). Runtime errors from compiled code answer
  `{"error":"...","line":N}` (N: the statement's line; zero cost until something fails). Not yet: a column
  for the exact expression in binding errors, the endpoint name in runtime errors (the request already names it).

### 1.2 Better error messages (done 2026-11-05)
- No compiler internals in messages (`unexpected Some(Sym("="))` today): every parse error says what was expected, in words.
- Hints for what people coming from other languages write: a C-style `for`, `++`, `while` before 1.3 lands, `null` misspelt,
  a missing semicolon at the end of the previous line (point at that line, not the next token).
- "Did you mean" for unknown fields, functions, endpoints' inputs and builtins (edit distance 1-2).
- Report every independent error in a file, not only the first (parse errors stop at the first; binding and type errors continue).
- Done when: a corpus of common mistakes (`tests/errors/`, one per file, expected message beside it) reads well and is run by `cargo test`.
- Done: `for`, `while`, `foreach`, `break`, `continue` and `x++` explain what to write today; `nil`/`none`/`NULL` point to
  `null`; unknown names, functions and fields suggest the nearest declared one (one edit for three-letter names, two for longer,
  none for shorter); a missing `;` points at the end of what came before. Binding and code generation report every function and
  endpoint with a problem (a problem inside a function is reported once, at its own line); parsing still stops at the first.
  `tests/errors/*.mito` (first lines `// expect: <text>`) is run by `cargo test`.

### 1.3 `mito` as the command (done 2026-11-05)
- The executable is `mito` (`mito check`, `mito build`, `mito serve`, `mito test`); `mitochondria` stays as a second name for a while.
- Done when: the docs and examples say `mito`, and both names work.
- Done: Cargo builds `mito.exe` and `mitochondria.exe` from the same source; the usage, tutorial and bench scripts say `mito`.

### 1.4 `mito serve --watch` (done 2026-11-05)
- Rebuilds (TinyCC, milliseconds) and restarts when the program file or a file it includes changes; a compile error is printed and the
  last good build keeps serving.
- State is lost on restart (the reservation is the process's). Optional: `--replay requests.txt` sends a request script after
  each restart to rebuild state (the same request format as test files, 3.1).
- Done when: editing an example under `serve --watch` shows the new behaviour within a second.
- Done: the watcher polls the program's modification time (a system: every `.mito` file beside it) every 100 ms; a save is
  answered by the new build 200-340 ms later (TinyCC build 30-45 ms, restart 50-80 ms). A compile error is printed and the old
  server keeps serving; the servers stop with the watcher (a job object). Not yet: `--replay`, which waits for the request
  format of test files (3.1).

## Phase 2: syntax closer to StasisLang

### 2.1 Loops: `foreach`, `for`, `while`, `break`, `continue` (done 2026-11-05, with 2.2)
- `foreach item, i in collection { }` (index optional: `foreach item in collection`), over lists, pools, arrays, module collections
  and ranges (`foreach i in 0..n - 1`, see 2.2). It replaces `each`; its modifiers stay: `reverse`, `where cond`, `limit n`, `from k`.
- `for i = 0; i < n; i += 1 { }`: all three clauses.
- `while cond { }`.
- Parentheses around the head of `if`, `for`, `foreach` and `while` are allowed and never required: `if x > 3 { }` and `if (x > 3) { }`
  are the same.
- `break` and `continue`, in every loop.
- A loop with no fixed extent (`while`, `for`) has a per-request step budget (default 100 million iterations, `--loop-budget N`):
  past it the request fails (500, "loop ran too long", with its line), so a mistaken infinite loop cannot hold a worker.
- `each ... while` and `[each ...]` (in answers) become `foreach`; old forms get a message saying what to write.
- Done when: the ports and examples are rewritten without fake bounds (`each step in 0..1000000000 while ...`) or flag variables, and
  the loop-versioning optimisations still apply (they key on a counter and its bounds, which `for` gives directly).
- Done: `foreach item[, i] in coll` (the index counts the live items visited, not slots), `for [var] i = e; cond; step` (`i` is a new
  variable of the loop), `while cond`, `break`, `continue` (in a `for` it still runs the step), parentheses optional on every head.
  `each` and `[each ...]` are errors that say what to write. A counted `for` (`i < n` / `i <= n`, step `i += 1`, neither changed in the
  body, `n` a number or local) compiles as a range loop: the same C as `foreach i in 0..n - 1`, versioning included. `while` and
  other `for` loops are range loops with a condition, so every analysis applies to them unchanged; they charge the request's budget
  65536 rounds at a time (a test of the loop counter, no store per round; `--loop-budget N`, default 100 million; the LangArena runner
  passes a huge one, as BF::Array runs more rounds than that in one request).
- Measured on the 11 ports whose fake bounds became `while` (5 rounds, median): within 2% except Sort::Quick, 4.5% slower: its inner
  `while so.t[i] < pivot { i += 1; }` loops are two instructions and the budget test is a third. A first version that decremented a
  budget in memory each round cost 11% (BF::Array) and 16% (Sort::Quick).
- Not yet: `foreach` over a fixed array (`foreach x, i in app.arr`), and the ports' flag variables (`while going == 1`) rewritten
  with `break`.

### 2.2 One meaning for `..`: both ends included (done 2026-11-05, with 2.1)
- `a..b` includes both ends everywhere: `n: i64 in 0..100` (as inputs and fields already mean), `foreach i in 1..10` (ten values),
  `match x { 0..9 => ... }`.
- Today loop ranges (`each i in 0..n`) exclude the end: they change meaning. A loop over the positions of something of size `n` becomes
  `foreach i in 0..n - 1`, or better `foreach x, i in array` (no range at all). Every program is migrated by a one-time rewrite of loop
  ranges (`0..n` becomes `0..n - 1`), and for one release a loop range is flagged with a message saying it now includes its end.
- Open: a half-open form for the many `0..n - 1` loops (`0..<n`, as Swift)? Proposal: decide after the rewrite shows how often it is needed.
- Done when: the ports, examples, tests and docs use the new meaning, and the loop-versioning optimisations still apply.
- Done: shipped together with `foreach` so no loop changed meaning silently: `each` (end excluded) is an error, `foreach` includes the
  end. A one-time rewrite turned 653 loops `each i in a..b` into `foreach i in a..b - 1` (numbers lowered by one, `+ 1` dropped); the
  parser takes a trailing `- 1` off again, so 47 of 52 programs give byte-identical C and 5 fold a `constant + 1` they computed before.
  The half-open form is still open: `0..n - 1` costs nothing, so it is only a question of how it reads.

### 2.3 Typed function parameters and results (done 2026-11-05)
- Every function has a full signature: `fn build(item: i32, depth: i64) -> Node { }`; `-> T?` as today; no arrow for no result.
- Parameter types and the result type are required (as Stasis); a call with a value of another kind is a compile error at the call,
  and a body that returns something other than its declared result is an error in the body.
- This removes the recursion rule (below) and the guessing behind it, gives better errors, and lets functions be compiled once
  instead of once per argument kind.
- Done when: the library modules, ports and examples are typed, and the per-kind specialisation of recursive functions is gone.
- Done: parameters and results are required and checked at each call and `return` (`parameter `p` of `f` is `P`, given a string`;
  a value that may be null passed for `P` says to check it or declare `P?`; an integer widens to `f64`). Types: integers, `f32`/`f64`,
  `bool`, `string`, item types (`Player`, or `Player in game.players` when several collections hold it; inside a module instance
  its own items), `T?`, `pool<T>` / `list<T>`, and `T.key` (the type of a field: generic modules name their key type this way).
  The result kind of a recursive function is its declared one: the guessing loop is gone, and so is the rule of returning before
  the first recursive call (`fn fact(n: i64) -> i64 { if n > 1 { return n * fact(n - 1); } return 1; }` compiles). The 12 dummy
  guards in the calculator ports are removed. About 135 program signatures were written from the kinds the compiler saw at the
  calls (a one-time tool, `mito sigs`), the library modules' 106 by hand; the generated C is unchanged.
- Not yet: compiling a function once instead of inlining it at every call. Inlining is part of the speed (constants and ranges flow
  into the body), so it stays the default; a C function per signature is the natural next step for large, often-called functions.

**The recursion rule today, and why it goes.** Parameters and results are not declared, so the compiler works out each function's
kinds from its calls. A normal function is inlined at every call with the argument kinds of that call. A recursive one cannot be
inlined forever, so it becomes a C function per combination of argument kinds, and its result kind must be known to compile the
recursive call inside its own body. The compiler guesses (no value, then integer, bool, float, text...) and recompiles the body until
the guess fits. A guess that is wrong in a way the body cannot even express ends the search with an unrelated error: in
`fn fact(n) { return n * fact(n - 1); }` the first guess makes `fact(n - 1)` "no value", and `n * <no value>` is a kind error, not a
"try again". So the rule became: return something before the first recursive call (`if n <= 1 { return 1; }`), which fixes the kind
before the guess is needed. The ports used dummy guards like `if g.pos < 0 { return 0; }` for it. With the result type written in the
signature there is nothing to guess.

### 2.4 Enums (done 2026-11-05)
- `enum Op { Plus, Minus, Right, Left, Open, Close, Out }`: named values, stored in the smallest integer that holds them,
  usable as field types, parameters, `match` subjects and in answers (as their name in JSON).
- Optional explicit values: `enum Cmd: u8 { Plus = 43, Minus = 45 }` (for byte codes such as the BF interpreter's).
- Open: values carrying data (Rust-style variants)? Proposal: not now; plain enums first.
- Done when: the BF interpreters and the compress ports use enums where they use magic numbers today.
- Done: declarations in the state part; `Op.Plus` folds to its integer when names are bound; fields of an enum type are stored in
  the smallest integer (or the declared one) with a write check over the values' range, widened to include 0, the value a field
  starts with (so `enum Cmd: u8 { Plus = 43, Minus = 45 }` accepts 0..45: a range check, not a membership check); parameters of an
  enum type are integers; answer fields declared with an enum type are written as names (`"op":"Plus"`), others as numbers. Both BF
  ports use enums (`Op` in BF::Recursion, the byte-valued `Cmd` in BF::Array), at the same speed.
- Not yet: enum inputs (an endpoint parameter `op: Op` reading a name), enums kept apart from integers (an `i64` can be assigned
  to an `Op` field, checked only by its range), and the compress ports.

### 2.5 `match` (done 2026-11-05)
- `match x { Op.Plus => { ... } Op.Minus, Op.Left => { ... } 0..10 => { ... } else => { ... } }`: on integers and enums; arms are
  constants, lists of constants and ranges; no fall-through; on an enum every value must be handled (or `else`).
- Compiles to a C `switch` (clang already turns if-chains into jump tables; this is readability and exhaustiveness, and speed under TinyCC).
- Done when: exhaustiveness errors are tested and the BF ports use it.
- Done: arms are numbers, `-n`, enum values, lists of them and inclusive ranges; `else` last; each value at most once (`a match
  covers 5 twice`); arms of one enum must cover it or have an `else` (`does not handle Op.Out`). The parser turns a `match` into an
  `if` chain over a hidden local (the subject is read once), so every analysis applies unchanged; clang makes a jump table of it.
  Both BF ports use it (their `bad` and `done` flags became `break`). A function ending in a `match` that returns on every arm still
  needs a `return` after it (the end-of-function check does not know the arms are exhaustive).

## Phase 3: tests in the language

### 3.1 `test` declarations and `mito test` (done 2026-11-05, except the items below)
- A test is a special case of a function, as in Stasis: `test "name" { ... }` is shorthand for a function with no parameters whose
  result is `bool` or text (`utf8<N>`), named by the string, which the test runner calls and interprets. It lives in the program file or
  in `app.test.mito` beside it:

  ```
  test "a full pool answers 503" {
      foreach i in 1..app.players.capacity() { post("/players", { name: "p" + i }); }
      let r = post("/players", { name: "one more" });
      if r.status != 503 { return "expected 503, got " + r.status; }
      return app.players.count() == app.players.capacity();
  }
  ```
- The runner reads the result: `true` passes, `false` fails, text fails with that text as the message (an empty text passes).
  A test whose request or code fails (an error the test did not catch) fails with that error and its line.
- Inside a test, requests are calls (`get`, `post`, `put`, `patch`, `delete` with a path, an optional body and headers) answering
  `{ status, body }`, and the state can be read directly. Each test starts from a fresh state; time and randomness are fixed
  (`--clock`, `--seed` as today). Test functions are compiled only by `mito test`, never into a served program.
- `mito test app.mito` (or `--dir tests`) runs them and reports each failure with the test's name, its message and the line of the
  `return` that failed it.
- The recorded form stays available for whole conversations: a `.case`-style file (requests and the answers they must get, blessed
  with `--bless`) run by `mito test app.case`, as the compiler's own tests are now.
- Done when: the examples ship with tests run by `mito test`, and failures point at the line.
- Done: `test "name" { }` in the program file; `get`/`post`/`put`/`patch`/`delete(path, { body fields }, { headers })` answer
  `.status` and `.body`, `field(text, "name")` reads a top-level field of a JSON text. A test is compiled into a runner (TinyCC, or
  `--clang`) that calls it outside any request, on a fresh instance per test with the clock at 2026-01-01 and a fixed seed; a request
  goes through the program's own request handling, its text placed after the test's (`text_base`), so the test's strings survive.
  Results: `true`/`false`/text as above; a failure prints the line of its `return`, an error its message and line. A test reads the
  state but changes it only through requests (assignments, `add`, `remove` and functions that change state are errors), and may
  not make a request inside a loop over a collection (the loop reads the collection's extent once). `mito check` compiles tests;
  `build`, `serve` and `emit-c` never include them. The scoreboard (5 tests, the full-pool 503 one included) and chat (3, with
  sessions) examples ship with tests; `cargo test` runs them and a program whose failing tests must name their lines.
- Not yet: `app.test.mito` beside the program (lines would need a file offset), `--dir`, a `.case` file run by `mito test` with
  `--bless` (the recorded form is still `cargo test cases`), tests of a system of workers, and answers stored past `let r = ...`.

## Phase 4: text

### 4.1 `ascii<N>` and `utf8<N>`: fixed-size text you build in place (done 2026-11-05, except the items below)
Today a `string<N>` field holds a value: text built in the request's text area (`"n=" + 5`, `substr`) is copied in whole when assigned
(413 if longer than N), and text cannot be built in state piece by piece (the ports wrote bytes into `u8` arrays with helper functions,
or had the benchmark harness generate the text). There are two text kinds, both with a fixed maximum of N bytes:

- **`ascii<N>`**: one byte per character (values 0..127). Written by index (`s[i] = b`) and read by index; a builder.
- **`utf8<N>`**: UTF-8 text. Read by byte index, never written by index (that could split a character); it changes only by whole text:
  appending, assigning, truncating at a character boundary. A byte index that is not a character's start is refused where a
  character is asked for.
- Both: `s += x` appends in place, everywhere a text value can be named (fields, items' fields, locals, inputs held in a
  local): text, integers and floats (formatted), as `+` already joins them. `s.append(x)` is the same as a function call. Also `s.clear()`, `s.truncate(n)`, `len(s)`, `maxlen(s)`, `substr(s, from, count)`
  without copying, assignment from any text, comparison. Appending past N fails the request (413, naming the field), like a full collection.
  `ascii<N>` takes only ASCII: appending or assigning a byte above 127 is refused (400).
- A text literal (`"..."`) is either kind, taking the kind of where it is used. One holding a byte above 127 is UTF-8 only: using it
  as `ascii<N>` is a compile error (the check needs no run time, since the literal is known).
- `string<N>` is another name for `utf8<N>`, now and later: existing programs keep working unchanged, and either name may be used.
- Inputs: `name: utf8<1..32>` / `ascii<1..32>` (the length range, both ends included as everywhere).
- Done when: the ports that build text (Calculator, Template, CSV, Base64's message) use them instead of byte arrays and helpers.

**Decided 2026-11-05: built text lives only in declared storage** (as StasisLang, where data lives only in defined globals).
- A local holds numbers, bools, handles (an item, a plain struct, a struct-valued part of an item) and text *windows* (a field's
  text, an input, a literal, a `substr` of one): where the bytes are, never a copy. A `let` that names a field is a window onto it:
  reading through it always sees the field as it is now (a later change shows through), and writing through it writes the field.
  A local cannot own or build text: `var t = ""` and `let s = "n=" + str(n)` go.
- Text expressions flow only into a destination: a field (`row.label = "n=" + str(n)` and `row.label += x` append into it in place),
  an answer (already streamed), a window parameter. A function that would return text takes its destination as a parameter.
- Temporary text goes in a declared per-request struct beside the state: `request work: Work;` is zeroed at the start of each
  request, not journaled, never kept (a declared frame; state fields as scratch would cost undo-log writes and leave leftovers).
- The hidden text area shrinks to the request's input and the response: the "text area full" error goes, every overflow names its
  field. Order: in-place appends to fields; windows as the only local text; `request` storage; the text ports; then string locals and
  string results become errors that say what to write.
- Done so far (2026-11-05): in-place appends and assignments-as-appends to fields (`tests/cases/append.case`); text windows for
  `let` of plain and item text fields (`windows.case`; reading through one still copies at the use, to be removed); `request` storage
  (`request.case`). `substr` of a text field is a window (`windows.case`), and copying a window into a field is one memcpy from field
  to field (`row.name = substr(g.text, a, n)`; no separate copy function). Reads by index of a plain text field: the length is read
  once before a loop that writes no state, and loop versioning bounds them as it does arrays.
- Text ports (5 rounds, median, against the byte-array versions): Calculator::Ast 0.201 s (was 0.209), Calculator::Interpreter
  0.359 s (0.385: names are windows, not copies), CSV::Parse 0.573 s (0.602), Template::Parse 0.651 s (0.644; its output stays a byte
  array written a byte at a time, as the reference pushes characters). Their `put`/`put_int` helpers are gone.
- Done: text locals and text results are errors that say what to write (`var t` would hold its own text; `let s` would hold text built
  here; `label` would return text). Tests are exempt (their messages are built text). The cases that used them build in a `request`
  field now; `textloop.case` shows why: a loop rewriting a field 100,000 times passes where the text area used to fill (507).
  Etc::CacheSimulation builds its key in `work.key` (0.450 s against 0.441 s for the old text-area version on the previous compiler):
  an assignment of several pieces is one helper call (integers formatted straight into the field), and a loop that assigns no field
  reads the lengths of the text it indexes once, windows passed to inlined functions included.
- Done: `ascii<N>` holds bytes 0..127: storing, appending or copying a window with any other byte fails (400, "ascii text cannot
  hold byte 195 (at 3)"), a literal with one is a compile error; `s[i] = b` writes a byte (0..127, below the length) of ascii text,
  and is a compile error on `utf8<N>` / `string<N>` (`ascii.case`). Not yet: ascii inputs (`name: ascii<1..32>` checks lengths only).
- Done: the Base64 ports build the reference's final message ("encode aaaa... to YWFh...: <sum>") in a request field and answer its
  hash; the harness no longer formats it. With Calculator, CSV and Template above, the text ports build their text in fields.
- Done (the items left open): ascii inputs (`v: ascii<1..16>` refuses a byte above 127, 400); windows onto plain fields and pieces
  of fields are written into answers and compared (against literals and other fields) where they are, no copy; `s.clear()` and
  `s.truncate(n)`; in utf8 text `truncate` and `substr` refuse to split a character (a cut on a continuation byte, 400); comparing
  text built on the spot (`app.code == "x" + n`) is a compile error saying to build it in a field first (`textops.case`).

## Phase 5: later

- **Stored ids that come back as `T?` (done 2026-11-05):** a field holding an item's slot and generation (the role `ref<>` had), read back as `T?` so a
  reused slot is `null` and must be handled. Design (2026-11-05):
  - Written with the type syntax that already exists: `best: Player?` in a struct (`Player in game.players?` when several
    collections hold Player). No new keyword; a plain `Player` field stays what it is (a nested struct is not an item).
  - Stored in 8 bytes: the slot + 1 (0 for null) and the item's generation. Only a `checked` pool has generations, so the field
    must name one (an error says to add `checked`): without them a reused slot cannot be told from the item that was there.
  - Reading gives a `T?`: null when the field is null, the slot is no longer live, or its generation moved on (the item was removed
    and the slot reused). The existing rules apply unchanged: `if exists(x)` or `or(...)` before use, never a run-time surprise.
  - Writing takes an item, a `T?` or `null`; an answer writes it as the item's id or `null`, as a `T?` already is.
  - Not stored: ids of items of nested collections (their slot is per instance), until a design for them.
  - Done (2026-11-05): `tests/cases/ids.case` (removed, slot reused, set, cleared, answered) and the errors for a pool without
    generations, a non-item value and an item of another pool.
- **`log(...)` (done 2026-11-05):** a builtin that writes to the server's standard error (not part of the answer), for debugging:
  `log("n is", app.n)` prints `log: line 12: n is 5`.
- **Editor support:** syntax highlighting for VS Code, then errors on save through `mito check`, then hover for inferred kinds.
  Done (2026-11-05): highlighting and a `$mito` problem matcher for `mito check` output (`editors/vscode`); not yet: hover.
- **Seeing inferred kinds (done 2026-11-05):** `mito check app.mito --kinds` prints every local's kind by function, endpoint and test,
  with its line (`let e: Entry`, `let w: text (a window onto a field)`); functions' kinds are their signatures.
- **Persistence across restarts** (so `serve --watch` keeps state): a design question for the language as a whole, not only tooling.
  Options drafted in [persistence.md](persistence.md) (snapshots, request log, change log, mapped file, migration by name), with a
  recommendation: layout-described snapshots with migration first, then a change log for durability.

## Phase 6: from the Campfire port (review of 2026-11-06)

A 2,300-line Campfire app (Rails' behaviour, on Rails' database) showed the memory model paying off (49 MB under load, reads in
parallel with no app changes, 0.07-0.14 ms a sidebar) and where it costs the developer. Worked through in this order.

### 6.1 Request scratch zeroed only where an endpoint can touch it
- The request struct is zeroed before every request, whole: sized for the worst endpoint, it cost every endpoint (600 KB, 11% of a
  page's CPU). Each endpoint zeroes only the request fields its code (with every function it calls) reads or writes.
- Done when: an endpoint that never names a field does not zero it (measured), and every case still passes.
- Done (2026-11-06): code generation records each top-level request field an endpoint's code names (reads and writes, through every
  inlined function); the route carries those ranges and the runtime zeroes only them (all of it for an endpoint that calls a
  function generated for recursive calls, shared by endpoints). `each_endpoint_zeroes_only_the_request_fields_it_names` checks it.
  Campfire's sidebar still names its lists (its cache-miss path does), so it zeroes them; its other endpoints no longer do.

### 6.2 Functions that return text
- Built text has only fields to live in, so helpers write `work.*` out-fields (`avatar_path(u)` leaving `work.avatar`), fields get
  reused for unrelated things and helpers alias each other. A function may return text (`-> text`): the result lives in the
  request's text area, as a window the caller uses or stores like any other text.
- Done when: the app's out-field helpers are functions that return their text.
- Done (2026-11-06): `-> text` (and `text` parameters); `let s = f(...)` holds what a function returns; `tests/cases/text_return.case`.
  The app's `avatar_path`, `user_title`, `room_gid`, `signed_stream`, `csrf_form_token` and `browser` return their text; seven
  out-fields of its request struct are gone.

### 6.3 Narrowing that follows the code
- `exists(x)` narrows across `&&` (`if exists(r) && r.n > 0`), after an early exit (`if !exists(x) { return ...; }` /
  `{ break; }` / `{ continue; }` / `fail`), and `or(text, "")` is text; a nullable field compared after `exists` is a comparison.
- Done when: those forms check, and the app's nested `if`s written around them are gone.
- Done (2026-11-06): `&&` (compiled as nested tests, so nothing after the test runs before it), early `break`/`continue` and an `else`
  that leaves, fields of rows (nullable times and references), `or` of two texts, numbers or bools; `tests/cases/narrowing.case`.
  The app's workarounds are gone (sidebar lists, direct members, cable close, membership connected).

### 6.4 Ordered table loops compiled to SQL
- `foreach m in t by lower(f) where ...` (and `by f` on any column, `reverse`) orders in SQLite; across a reference
  (`by lower(m.room.name)`) the query joins. No sorting by hand.
- Done when: the sidebar's insertion sorts are loops ordered by SQL, the pages unchanged.
- Done (2026-11-06): `by lower(f)`, `by f.g` across a reference (a correlated subquery: the loop's own columns stay unqualified) and
  `by lower(f.g)`, with `reverse`; an expression order sorts the rows the `where` found (an index must lead it, or `scan`); `by f`
  takes `scan` too; `after` and `last` refuse expression orders, saying so. `tests/cases/ordered_loops.case`. The sidebar's three
  sorts are loops ordered in SQL written as they go (its 64 KB name buffer and two 32 KB lists gone): the pages unchanged for three
  users; cached 68,150 req/s (was 59,915, less to zero), uncached 27,791 (was 29,334: SQL sorts through a subquery per row).

### 6.5 A string library
- Unicode case (`lower`, `upper`: Unicode's simple mappings, as Ruby's downcase), `replace(t, a, b)`, `split` as a loop
  (`foreach part in split(t, ", ")`), `trim` variants; request-scoped text lists where a list of texts is needed.
- Done when: the user-agent port uses them (no byte loops for case or splitting), its 385 vectors still pass.
- Done (2026-11-06): `lower` / `upper` with Unicode 15's full mappings (runtime/unicase.c, written by tools/unicase.py, compiled
  in only when a program uses them; as Ruby 3.4: no final sigma, checked against Ruby), `replace(t, a, b)`, `trim_start`, `trim_end`,
  and `foreach p in split(t, sep)` / `split(t)` (words) with Ruby's pieces; `tests/cases/text_library.case`. The user-agent port's
  Kelvin-sign fold is one `lower` (385/385), the sidebar's first names and initials are split loops. Text lists were not needed:
  split loops covered the app's lists of texts, so they wait for a program that keeps one.

### 6.6 SQLite's memory bounded with readers
- With readers SQLite takes memory from the system allocator (the fixed pool's lock serialized them). Each connection gets its own
  bounded pool instead, so `--db-memory` bounds SQLite again without a shared lock.
- Done when: readers run on per-connection pools at the speed of the system allocator, and a connection that fills its pool fails
  its request clearly.
- Done (2026-11-06): SQLite's allocator (SQLITE_CONFIG_MALLOC) is a pool for each connection, of `--db-memory` MB, in power-of-two
  classes with a list of freed blocks each (as memsys5's classes), with no lock: a thread names the pool of the connection it is about
  to use (thread-local), a block goes back to the pool its address is in. Sidebar 28,848 / 67,860 req/s (system allocator: 27,791 /
  68,150); a 900 KB value on 1 MB pools fails its request with "database: out of memory" and the next request works.

### 6.7 Apps in several files, and a cost report per endpoint
- `use "rooms.mito";` brings another file's structs and functions into the program (one program, one state).
- `mito check --costs` lists per endpoint: request bytes zeroed, table queries in its code (and in loops), text copies.
- Done when: the Campfire app is split by feature, and the report shows the sidebar's numbers.
- Done (2026-11-06): `use "file.mito";` (a line of its own, a comment after it allowed; relative to the file that uses it; each file
  once, by any path) puts that file's text in its place: one program, one state. Errors name the file and line they are in, quoted;
  `serve --watch` watches every file used. `mito check app.mito --costs` prints per endpoint the request struct bytes it zeroes and
  its code's sites: queries, row reads, table writes, text built (each with those in loops, run once a round). Campfire is ten files
  by feature (`app/campfire.mito` keeps the tables, state, web block and request struct); its sidebar: "zeroes 22398 B; queries 25
  (2 in loops), row reads 71 (14 in loops), table writes 4, text built 164".

Phase 6 is done (2026-11-06).

## Phase 7: text and structs in locals (under consideration, 2026-11-06)

### 7.1 `let` with built text (and structs) in a scoped region
- A `let` (or capped `var`) may hold text it builds, or a struct with text fields: its own region, from a mark on the request's
  text area to the scope's end, where the area moves back to the mark (no zeroing, no bookkeeping).
- A value leaves its scope only by copy: into a field or item (its declared capacity, as today), or as a function's result, which
  the caller copies (decided 2026-11-06: copy for now, no sliding the result down into the caller's region).
- Windows onto region text (`substr`, `trim`, split pieces) stay in the same or an inner scope.
- Bounds always known: from the parts' maxima for `let`; `var` text that grows in a loop declares its cap (`var acc: text<8192>`).
- No region-backed local live at a recursive call, so each endpoint's worst-case region use is computed when compiling (shown by
  `--costs`; an endpoint that cannot fit the text area is refused).
- First step: `let` over bounded parts only, release at scope end, the recursion check, the worst case in `--costs`; then the
  sidebar's and cable's scratch fields as lets. Then capped `var`, then text-bearing structs.
- First step done (2026-11-06): `let k = a + b` (and `str(...)`) holds its text; a block that declared one marks the text area at its
  start and cuts back at its end, unless text built in it may outlive it (a `return` with a value, an outer variable given text, the
  answer cache's places); a call to a function generated for recursive calls after such a `let` in its block is refused. Bounds are
  tracked for literals, fields, inputs, table text columns, numbers' `str` and joins; `--costs` adds "lets of built text N (up to B
  bytes, K unbounded)" (a sum over the endpoint's lets: an upper bound, as scopes reuse their space). `tests/cases/text_let.case` (ten
  MiB of lets through a 4 MiB text area), `tests/errors/text_let_built.mito` (the recursion rule). Bounded parts are not yet required:
  a `let` over an unbounded part is counted as unbounded. Campfire: the sidebar's cache key and the cable's identifier, Origin and
  broadcast prefix are lets or `-> text` (three request fields gone; the cable endpoints zero 2,124 B, 260 B and 0 B, from 15,448,
  8,456 and 12,296).
- Capped `var` done (2026-11-06): `var acc: text<N> = ...` reserves N bytes in its scope's region (`txt_cap`); `=` and `+=` write the
  pieces in place in one call (`txt_set`, bytes moved, so the var's own text may be a piece); more than N fails the request (413, as a
  field's cap does). The recursion rule and the scope release are the `let`'s. `--costs` now gives the most region text live at once
  (scopes given back are subtracted; a region kept across a loop's iterations counts as unbounded): "region text N locals (at most B
  B at once, K unbounded)". `tests/cases/text_var.case` (26 MB of a capped var in a loop body through the 4 MiB text area, the cap's
  413, self-reference, a function returning one, a `let` of one as a window onto it), `tests/errors/text_var_recursive.mito`, `tests/errors/text_let_cap.mito`. Campfire:
  the session cookie's hash is a capped var and its other scratch text lets (`work.plain` gone; POST /session's region text is at
  most 12,600 B at once).
- Bounds required done (2026-11-07): sizes flow through the builtins (base64, hex, JSON and HTML quoting, form coding, case mapping,
  `replace`, the crypto results, `random_bytes(n)`, `format_time`), `substr`, `trim`, `json_field`, `split`'s pieces, and text a
  function returns (the most of its returns). A `let` whose size is not known when compiling (a header, the environment, stored data,
  a recursive function's result) is an error that says to give it a cap; an endpoint whose region text can reach more than the
  4 MiB text area is refused. A `return` leaves past its scopes' cuts, so a block that returns is cut too (what it returns stays
  until the caller's scope ends, and counts as live until then). `--costs` names any region kept across a loop's rounds (file:line).
  Tests: `tests/errors/text_let_unbounded.mito`, `tests/errors/text_area_full.mito`, `text_let.case`'s `/r` (text returned from a
  loop whose rounds are cut). Campfire: every endpoint's region text is bounded, at most 36,283 B at once.
- Text-bearing structs in locals: not started. No Campfire code wants one yet (the session's fields cross most endpoints and their
  markup, which the request struct serves); the likely design is a compile-time bundle (scalar fields as C locals, text fields as
  capped region text), with no layout or runtime change.

## Phase 8: HTML fragments (decided 2026-11-07)

From the Campfire port: Rails renders a message body through Action Text, which parses it as HTML5 (Nokogiri::HTML5, Rails 8.2's
default sanitizer vendor), walks and edits the tree (its attachments, the app's filters, Loofah's safe list) and writes it back as
Nokogiri does; `auto_link` then works on that text. Ben chose (2026-11-07) a parser in the runtime with a node API in the language,
so the app writes that pipeline in Mitochondria, over the other options (an app's C extension, a sanitizer builtin, deferring).

- The parser: Nokogiri's fork of gumbo at the version the reference runs (1.19.4, Apache 2.0, with its LICENSE and NOTICE beside it
  as zlib's and crypt_blowfish's), bundled in the runtime: the same parse Rails gets, not a lookalike.
- Nodes in a pool of each instance (as the text area: never shared state, so readers parse at once), given back with the request;
  a fixed cap that fails the request clearly when a document would need more (no silent cut).
- The operations: those the Rust port's `richtext` crate shows to be enough (`crates/richtext/src/dom.rs`): parse a fragment (and in
  a node's context), children, parent, name, the attributes (read, set, remove, list), text, append, insert before, detach, replace
  with nodes or parsed markup, clone, serialize as Nokogiri does (into the answer; as text only with a declared cap).
- Checked against Rails' output: the Rust port's Rails-generated rich text cases first, then the parity states.
- Order: the message frame without rich text (the app); the parser with fragment parsing and serializing (round trips); the node
  operations; the sanitizer layers; attachments; `auto_link` (a hand matcher, as the Rust port's: the language has no regular
  expressions, and adding them would be a decision of its own).
- The parser and the node operations done (2026-11-07): third_party/gumbo (Nokogiri 1.19.4's, its sources matching the reference
  image's gem), each file its own cached object; runtime/html_parse.c parses as `noko_gumbo_s_fragment` does; runtime/html.c holds the
  pool, the operations and Nokogiri's `html_standard_serialize`; builtins 72 to 97 (DESIGN.md, "HTML fragments").
  `tests/cases/html_fragment.case`: round trips (void elements, entities, `&nbsp;`, raw `<script>` text, a stray `</p>`, SVG and
  xlink, implied `<tbody>` and `</li>`), walking, attribute edits, insertion and moves, a parse in a `<table>` context: every output
  checked against Nokogiri::HTML5 in the reference image's Ruby, byte for byte.
