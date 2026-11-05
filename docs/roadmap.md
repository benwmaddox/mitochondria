# Roadmap: syntax and developer experience

Written 2026-11-05 from a review of the language as it stands (after the LangArena ports, `unlogged`, range-typed fields and `T?`).
Items are worked through roughly in order; each lists the decision taken, what is still open, and when it counts as done.
Decided against, for now: request-local scratch arrays (`unlogged` state covers that need).

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
