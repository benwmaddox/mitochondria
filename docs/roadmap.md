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

### 2.3 Typed function parameters and results
- Every function has a full signature: `fn build(item: i32, depth: i64) -> Node { }`; `-> T?` as today; no arrow for no result.
- Parameter types and the result type are required (as Stasis); a call with a value of another kind is a compile error at the call,
  and a body that returns something other than its declared result is an error in the body.
- This removes the recursion rule (below) and the guessing behind it, gives better errors, and lets functions be compiled once
  instead of once per argument kind.
- Done when: the library modules, ports and examples are typed, and the per-kind specialisation of recursive functions is gone.

**The recursion rule today, and why it goes.** Parameters and results are not declared, so the compiler works out each function's
kinds from its calls. A normal function is inlined at every call with the argument kinds of that call. A recursive one cannot be
inlined forever, so it becomes a C function per combination of argument kinds, and its result kind must be known to compile the
recursive call inside its own body. The compiler guesses (no value, then integer, bool, float, text...) and recompiles the body until
the guess fits. A guess that is wrong in a way the body cannot even express ends the search with an unrelated error: in
`fn fact(n) { return n * fact(n - 1); }` the first guess makes `fact(n - 1)` "no value", and `n * <no value>` is a kind error, not a
"try again". So the rule became: return something before the first recursive call (`if n <= 1 { return 1; }`), which fixes the kind
before the guess is needed. The ports used dummy guards like `if g.pos < 0 { return 0; }` for it. With the result type written in the
signature there is nothing to guess.

### 2.4 Enums
- `enum Op { Plus, Minus, Right, Left, Open, Close, Out }`: named values, stored in the smallest integer that holds them,
  usable as field types, parameters, `match` subjects and in answers (as their name in JSON).
- Optional explicit values: `enum Cmd: u8 { Plus = 43, Minus = 45 }` (for byte codes such as the BF interpreter's).
- Open: values carrying data (Rust-style variants)? Proposal: not now; plain enums first.
- Done when: the BF interpreters and the compress ports use enums where they use magic numbers today.

### 2.5 `match`
- `match x { Op.Plus => { ... } Op.Minus, Op.Left => { ... } 0..10 => { ... } else => { ... } }`: on integers and enums; arms are
  constants, lists of constants and ranges; no fall-through; on an enum every value must be handled (or `else`).
- Compiles to a C `switch` (clang already turns if-chains into jump tables; this is readability and exhaustiveness, and speed under TinyCC).
- Done when: exhaustiveness errors are tested and the BF ports use it.

## Phase 3: tests in the language

### 3.1 `test` declarations and `mito test`
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

## Phase 4: text

### 4.1 `ascii<N>` and `utf8<N>`: fixed-size text you build in place
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

## Phase 5: later

- **Stored ids that come back as `T?`:** a field holding an item's slot and generation (the role `ref<>` had), read back as `T?` so a
  reused slot is `null` and must be handled. Design to come.
- **`log(...)`:** a builtin that writes to the server's standard error (not part of the answer), for debugging.
- **Editor support:** syntax highlighting for VS Code, then errors on save through `mito check`, then hover for inferred kinds.
- **Seeing inferred kinds:** `mito check --kinds` printing each local's and function's kind.
- **Persistence across restarts** (so `serve --watch` keeps state): a design question for the language as a whole, not only tooling.
