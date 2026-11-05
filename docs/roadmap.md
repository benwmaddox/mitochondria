# Roadmap: syntax and developer experience

Written 2026-11-05 from a review of the language as it stands (after the LangArena ports, `unlogged`, range-typed fields and `T?`).
Items are worked through roughly in order; each lists the decision taken, what is still open, and when it counts as done.
Decided against, for now: request-local scratch arrays (`unlogged` state covers that need).

## Phase 1: the everyday loop

### 1.1 Line numbers in every error
- Compile errors name `file:line:col` and quote the line with a caret under the place: `app.mito:14:31: no field `totl` (did you mean `total`?)`.
- The lexer keeps a position per token; the parser and the binder carry it on expressions and statements; code generation
  errors use the position of the expression they are about.
- Runtime errors name the endpoint and the source line in the message: `{"error":"integer overflow","at":"app.mito:22 (POST /games)"}`.
- Done when: every message in `tests/cases` and the compile-error tests has a position, and the runtime test cases show them.

### 1.2 Better error messages
- No compiler internals in messages (`unexpected Some(Sym("="))` today): every parse error says what was expected, in words.
- Hints for what people coming from other languages write: a C-style `for`, `++`, `while` before 1.3 lands, `null` misspelt,
  a missing semicolon at the end of the previous line (point at that line, not the next token).
- "Did you mean" for unknown fields, functions, endpoints' inputs and builtins (edit distance 1-2).
- Report every independent error in a file, not only the first (parse errors stop at the first; binding and type errors continue).
- Done when: a corpus of common mistakes (`tests/errors/`, one per file, expected message beside it) reads well and is run by `cargo test`.

### 1.3 `mito` as the command
- The executable is `mito` (`mito check`, `mito build`, `mito serve`, `mito test`); `mitochondria` stays as a second name for a while.
- Done when: the docs and examples say `mito`, and both names work.

### 1.4 `mito serve --watch`
- Rebuilds (TinyCC, milliseconds) and restarts when the program file or a file it includes changes; a compile error is printed and the
  last good build keeps serving.
- State is lost on restart (the reservation is the process's). Optional: `--replay requests.txt` sends a request script after
  each restart to rebuild state (the same request format as test files, 3.1).
- Done when: editing an example under `serve --watch` shows the new behaviour within a second.

## Phase 2: syntax closer to StasisLang

### 2.1 Loops: `foreach`, `for`, `while`, `break`, `continue`
- `foreach item, i in collection { }` (index optional: `foreach item in collection`), over lists, pools, arrays, module collections
  and ranges (`foreach i in 0..n`). It replaces `each`; its modifiers stay: `reverse`, `where cond`, `limit n`, `from k`.
- `for i = 0; i < n; i += 1 { }` (parentheses allowed, as for `if`): all three clauses.
- `while cond { }`.
- `break` and `continue`, in every loop.
- A loop with no fixed extent (`while`, `for`) has a per-request step budget (default 100 million iterations, `--loop-budget N`):
  past it the request fails (500, "loop ran too long", with its line), so a mistaken infinite loop cannot hold a worker.
- `each ... while` and `[each ...]` (in answers) become `foreach`; old forms get a message saying what to write.
- Open: the `for` header without parentheses (as `if`) or with them (as Stasis)? Proposal: both accepted.
- Done when: the ports and examples are rewritten without fake bounds (`each step in 0..1000000000 while ...`) or flag variables, and
  the loop-versioning optimisations still apply (they key on a counter and its bounds, which `for` gives directly).

### 2.2 One meaning for `..`
- `a..b` is a half-open range everywhere: `a` included, `b` not. That is what loops already mean.
- `a..=b` is the inclusive range, for where the last value belongs: `n: i64 in 0..=100`, `level: i8 in -5..=5`.
- Today input and field ranges (`in 0..100`) include the end: they change meaning, so every program is migrated by a one-time
  rewrite (`in a..b` becomes `in a..=b`) and the compiler rejects `in a..b` without `=` for one release with a message saying which.
- Done when: the ports, examples, tests and docs use the new forms.

### 2.3 Typed function parameters and results
- `fn build(item: i32, depth: i64) -> Node { }`; `-> T?` as today; `-> void` or no arrow for no result.
- Parameter types are required (as Stasis); a call with a value of another kind is a compile error at the call.
- The result type is required when the function returns a value.
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
- A test is declared like a function: `test "a full pool answers 503" { ... }`, in the program file or in `app.test.mito` beside it.
- Inside, requests are calls with checks: `let r = post("/players", { name: "ada" }); expect(r.status == 201); expect(r.body.id == "0.0");`,
  and the state can be read directly (`expect(app.players.count() == 1)`). Each test starts from a fresh state; time and randomness
  are fixed (`--clock`, `--seed` as today).
- `mito test app.mito` runs them and reports each with its line on failure.
- The recorded form stays available for whole conversations: a `.case`-style file (requests and the answers they must get, blessed
  with `--bless`) run by `mito test app.case`, as the compiler's own tests are now.
- Open: the exact shape of requests and checks inside a test. Proposal above; to be tried on the examples first.
- Done when: the examples ship with tests run by `mito test`, and failures point at the line.

## Phase 4: text

### 4.1 `string<N>` as a fixed-size builder
- Today a `string<N>` field holds a value: text built in the request's text area (`"n=" + 5`, `substr`) is copied in whole when
  assigned (413 if longer than N), and there is no way to build text in state piece by piece (the ports wrote bytes into `u8` arrays
  with helper functions, or had the benchmark harness generate text).
- A `string<N>` (UTF-8, at most N bytes) becomes a builder in place: `s.append(x)` (text, integers and floats formatted),
  `s.append_byte(b)`, `s.clear()`, `s.truncate(n)`, `len(s)`, `s[i]` to read a byte, `substr(s, from, count)` without copying.
  Appending past N fails the request (413, naming the field), like a full collection.
- Assignment (`s = "x" + y`) and comparison keep working as now.
- Open: an `ascii<N>` kind (one byte per character, so byte writes `s[i] = b` are allowed) beside `string<N>`? Proposal: yes, for
  parsers and encoders; UTF-8 strings stay read-only by index.
- Done when: the ports that build text (Calculator, Template, CSV, Base64's message) use it instead of byte arrays and helpers.

## Phase 5: later

- **Stored ids that come back as `T?`:** a field holding an item's slot and generation (the role `ref<>` had), read back as `T?` so a
  reused slot is `null` and must be handled. Design to come.
- **`log(...)`:** a builtin that writes to the server's standard error (not part of the answer), for debugging.
- **Editor support:** syntax highlighting for VS Code, then errors on save through `mito check`, then hover for inferred kinds.
- **Seeing inferred kinds:** `mito check --kinds` printing each local's and function's kind.
- **Persistence across restarts** (so `serve --watch` keeps state): a design question for the language as a whole, not only tooling.
