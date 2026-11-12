# Effect contracts (design proposal, not built)

**Status.** For the owner's review. Nothing here is implemented; no compiler changes accompany it.

**The question first.** Is a declared effect contract worth having in Mitochondria? The compiler already *infers* what code does and uses it
(readers, loop collection, `check --costs`). A declaration adds value only if it turns an inference into a *promise someone can rely on*:
a refactor that makes a pure helper write state fails loudly instead of silently losing the parallel-reader path. Recommendation (section 9):
yes, but only the smallest version first, and measure whether anyone writes the annotations.

## 1. What the compiler knows today

| fact | where | how it is found |
|---|---|---|
| a function/statement list writes (state, collections, truncate, alloc/move/copy/sort/fill/clear, stores to fields or indexes), following calls | `fn writes(api, stmts, seen)`, `src/cgen.rs` ~1554 | walk of the *bound AST* (`Api.fns`, `CallRes::User(k)` callees), with a `seen` list so recursion ends |
| allocates (collects a loop's items first) | `fn allocates`, ~1583 | same walk |
| recursion | `fn recursive_fns(api)`, ~1234 | call graph cycles; per function |
| an endpoint only reads, so a reader may run it | `reads: Vec<bool>` in `Generated` (~429), set around ~1456 as `!g.wrote_state && !g.uses_spec`, then further cleared if the generated C text contains a write helper (`->tinsert(`, `->tset(`, `->alloc(`, `->store(`, ...; ~1458) | the **generated C** of the endpoint, not the AST. A call to a recursive/outlined function is "not known to be read-only" |
| cost per endpoint: queries, reads, writes, texts, lets (each total and in loops) | `Costs`, `count_costs(body)` ~436, shown by `mito check --costs` (`main.rs` ~118) | counts helper calls in the endpoint's generated C, a block opened by `for (` / `while (` counting as a loop |
| size / state layout | `layout.rs` | declared structs |
| platform restrictions | `src/platform.rs` ~91-112: `@frame`/`@shader` only on `platform game`; a game needs `@frame` and a `@shader` | the entry file's endpoints |

Two different machineries therefore exist: AST walks (`writes`, `allocates`, `recursive_fns`), which are cheap and independent of code
generation, and facts read off generated C (`reads`, `Costs`). A declared contract should be checked against the **AST walks**, not the C text:
C-text facts are only known after the heavy pass, depend on inlining decisions, and cannot name *which* state was written.

## 2. What a contract could say

Effects are a small closed vocabulary. All are about what the function does *beyond* its parameters and locals (as in Stasis: reads and local
mutation are always allowed).

| effect | meaning |
|---|---|
| `reads` | reads declared state or tables (implied by anything else; `@effects()` = touches nothing declared) |
| `writes` | any write to declared state, a pool/list/table, or `logged` state |
| `writes(app.players)` | writes only below that state path (state paths as `game.players`, `game.board`; a table by name) |
| `allocates` | adds to a pool/list/table (`alloc`, `add`) or builds text in a region the caller does not own |
| `fails` | may `fail <code> "..."` (a fail rolls back writes; matters to caching) |
| `random`, `clock` | `random_bytes`, `now`, `random...` (nondeterministic: forbids caching and replay) |
| `io` | files, network, env |
| `recurses` | is in a cycle of calls (the compiler already knows) |

`writes(path)` implies `reads`. Paths nest: `writes(game)` allows `game.players`. Parameter-relative writes (`self.items[...]` where `self` is a
`Stack`) are mapped to a state path at each call site (Stasis does exactly this); if the root cannot be proven, a restricted function is
rejected with a message naming the call.

### Syntax

```
@effects(reads)                      fn score_of(p: Player) -> i64 { ... }
@effects(reads, writes(game.players)) fn hurt(self: Player, n: i64) { ... }
@effects()                           fn clamp(x: i64, lo: i64, hi: i64) -> i64 { ... }   // touches nothing

effects Pure = ();                   // named groups
effects Reading = reads;
effects Mutating = reads, writes;
effects PlayerWrites = reads, writes(game.players);

@effects(Pure)  fn clamp(...)
@effects(Reading, fails) @get("/top") fn top() -> Response<text> { ... }
```

The attribute form `@effects(...)` follows the existing `@get`, `@frame` style. A group name expands to its list; groups may use other groups;
a group is a compile-time name only (no runtime). Union of a function's groups and listed effects is its contract.

### What the compiler checks

For an annotated function `f`, take the set `inferred(f)` over its *complete resolved call tree* (calls resolve to `CallRes::User(k)`, receiver
calls included, recursion by the `seen` list). The contract holds if `inferred(f)` is a subset of the contract. Helpers called from an annotated
function need no annotation of their own (this is Stasis's rule: the contract covers the whole tree; Mitochondria already inlines
everything anyway). Compilation adds no run-time check.

An error names the function, the contract, the offending effect and the first place it comes from:

```
error: fn score_of is declared @effects(reads), but it writes game.players
  game.mito line 40: score_of -> bump -> `self.hits = self.hits + 1`
```

## 3. Where it would be checked

**Recommendation: a new pass on the bound `Api`, in a new `src/effects.rs`, called after binding and before code generation.**

- Not "earlier" (parsing): effects depend on resolved calls and on which struct field a write reaches, which only the binder knows.
- Not in `cgen.rs`: that file is 6000 lines, and a check that does not depend on code generation should not wait for it; an effect error
  should appear in `check` quickly, not after the build runs twice (`generate_with`).
- It generalizes `writes`/`allocates` from "does it?" to "what, and where": same walk, but collecting `(effect, path, location)` instead of
  returning a bool. `writes()` in cgen.rs could then call the new one and the two stay in step (one source of truth; the owner prefers one approach).
  First version: do **not** touch `writes()`; add the new walk beside it, then unify as its own step with the existing tests as the proof.

Parsing needs: the attribute `@effects(...)` (where `@get`/`@frame` are parsed, `src/lang.rs`), the `effects Name = ...;` declaration, and an
`effects: Option<Vec<Effect>>` on `Func`/`Endpoint`.

I could not verify exactly where the state path of a write can be recovered: `Stmt::Assign(Expr::Field(..) | Expr::Index(..), _)` carries a
base expression, and `VarRes` resolves state vs local, so a root path is probably recoverable by walking `Expr::Field` to its root. Step 1 checks this.

## 4. Interaction with parallel readers

Today an endpoint runs on a reader if the compiler infers it only reads (`Generated.reads`). The inference is conservative (a call that reaches
a recursive or outlined function disables it, `uses_spec`). A declared contract makes this *reviewable*:

- `@effects(reads)` on an endpoint: if the inference then says "reader-unsafe" because of `uses_spec` (conservative, not a real write), the
  compiler should say so, as a *warning that explains the lost reader path*, rather than silently serve it on the single writer. This is
  the immediate benefit, and it needs no new analysis.
- A proof: if the AST check says reads-only but the C-text check says not (a mismatch), that is a compiler bug; assert it in a test.
- A declared `reads` should never be trusted *instead of* the inference for scheduling, only checked against it. A contract the compiler
  cannot verify must not turn on readers.

## 5. Interaction with the answer cache

`cache_get` / `cache_put` (`--cache-mb`) keeps pieces of answers between requests. A function declared `@effects(reads)` plus no `random`, `clock`
or `io` is *deterministic in the data it reads*, so its result is valid until the data's version changes (the data version is builtin 67;
see the comment near cgen.rs 280). That would allow:

```
@cache @effects(reads) @get("/top") fn top() -> Response<text> { ... }
```

where the compiler caches the whole answer keyed by (endpoint, arguments, data version), with the contract as the *proof* the cache is safe.
This is a real feature, but a second step: it needs a decision on invalidation granularity (whole data version: coarse but safe; per-path
versions: needs `reads(path)`, below). **Not in the first version.**

`reads(path)` (reads only below a path) is a natural extension for finer cache invalidation and for proving two endpoints independent, but
state paths on the read side are more work (every `Expr::Field` root). Defer.

## 6. Interaction with platforms

A game's `@frame` is called every frame; the owner's constraint is that it may not touch tables (SQLite on the frame path). With contracts the rule
is checkable and local, and can be *built into the platform* so nobody writes it:

- `platform game`: `@frame` has the implicit contract `reads, writes(...state...), fails`, with **no `io`, no table effects**. Violations are
  reported through the same error text and the same pass; this replaces ad hoc checks in `platform.rs` (~91-112) which today only check *which*
  endpoints exist, not what they do.
- `platform web`: endpoints unrestricted by default; `@effects` is opt-in.
- `platform cli`/test: tests may declare `@effects()` for read-only observations, as in Stasis's testing guidance (`@effects()` on helpers
  that must not mutate), which also helps tests not depending on order.

Table effects need their own names (`reads(table users)`, `writes(table users)`), since tables are not in the state-path namespace; this is a
reason to wait for a proven first version before adding game-specific rules.

## 7. Interaction with generics and functions of a type

- **Functions of a type** (`fn hurt(self: Player, n)`) are ordinary `Func`s found by the receiver where the call is written; `writes()` already
  follows them via `callees`. A call `p.hurt(3)` contributes whatever `hurt` does. The contract on `hurt` can say `writes(self)` meaning "writes
  only through the receiver"; at each call site `self` is mapped to the path of the receiver argument (`game.players[i]` to `game.players`).
  This is the same parameter-relative mapping as in section 2 and is the most valuable form: `@effects(writes(self))` on `push`.
- **Generics** (see `generics.md`): templates are expanded *before* binding, so an instance is an ordinary function and the effect check runs
  on it unchanged. A contract written on a template is checked **per instance** (the body can differ per `T`: `T: text` branches might write
  differently); messages name the instance (`Stack<i64, 16>.push is declared @effects(writes(self)) but writes game.undo.len`... no: that is within
  `self`, so passes). A contract mentioning a state path of a *particular* instance cannot be written in a template; only `self`, `reads`,
  `writes`, `allocates`, ... can. A group applies to every instance.
- Order of work: effects without generics can ship first (the pass operates on bound functions either way); generics later change nothing in it.

## 8. What could go wrong

1. *Annotation burden.* If every function needs a contract, the feature failed. Contracts must be optional, and the platform defaults (section 6)
   carry most of the value.
2. *False rejections from conservative inference.* `writes` treats any assignment to a field or index as a write, even to a local struct
   (`Stmt::Assign(Expr::Field(..))` on a local). A local struct's field write must not count as an effect: the pass must distinguish local roots
   from state roots (`VarRes`). If it cannot, the contract is unusable, so this is the first check of the first step.
3. *Contract drift.* A group named `Pure` that is later loosened affects every function using it. Groups are therefore declared once and
   change in one place; that is the point, and a review burden.
4. *Two sources of truth.* `writes()` in cgen.rs and the new pass could disagree. Mitigation: a test that runs both over the whole corpus
   (`tests/cases`, `examples`) and asserts the new pass's "writes anything" equals `writes()`.
5. *Names changing with generics/modules:* path names in `writes(path)` must be the *declared* names, not mangled ones.
6. *Trusting a declaration for optimization.* Never; checked-only (section 4).

## 9. Is it worth it, and the smallest useful version

Cost: one parser addition (an attribute, one declaration), one new pass of about the size of `writes()` plus path tracking, and diagnostics.
Benefit today: low to moderate. The compiler already infers everything it uses, so a declaration catches *regressions* (an endpoint that
should be a reader but is not; a game frame touching tables) rather than enabling new behavior. Benefit later: the cache (section 5) and agent
safety: the owner's crudspec/agent-maintained-app goals value "an agent cannot silently make a function write", and a contract is exactly the
checked statement an agent maintaining code can rely on. That is the case for doing it.

**Smallest useful version (first step):**

- `@effects(reads)` and `@effects()` only (no paths, no groups, no other effects) on functions, endpoints and tests.
- The check: new `src/effects.rs` walk over bound `Api` collecting write/alloc/random/clock/io, local-vs-state aware; error as in section 2.
- Plus the endpoint warning in section 4 when `@effects(reads)` is declared but the reader path is lost.
- Tests: contract holds (compiles); each violation kind rejected with the chain naming the offender; a corpus test asserting the new walk and
  `writes()` agree.

Second: `writes(path)` and `writes(self)`. Third: named groups (cheap once the list parses). Fourth: platform defaults for `@frame`. Fifth: `@cache`.

## 10. Tests that would prove it

1. Read-only function with `@effects(reads)` compiles; same function plus `self.n = 1` on state is rejected, naming the line and the chain.
2. A write through a *local* struct is not an effect.
3. A write several calls deep (including through a function of a type and a recursive function) is found and the chain printed.
4. `@effects()` rejects any read of state.
5. Named groups expand, nest, and a cycle of groups is an error.
6. Reader flag: an endpoint declared `reads` that the C path marks not-read-only produces the warning; one that is a reader produces none.
7. Agreement over the corpus: new pass "writes anything" == `writes()` for every function in `tests/cases` and `examples`.
8. `platform game`: `@frame` touching a table is rejected (step four).
9. Generic instance (after generics): contract on a template is checked per instance and errors name `Stack<i64, 16>.push`.

## 11. Questions for the owner

1. **Worth building at all?** Recommend yes, but only the smallest version, and re-evaluate after using it on two example programs. If no
   annotation is ever written by choice, stop at the platform defaults.
2. **Checked-only, never trusted for optimization (readers, cache) without the independent inference agreeing?** Recommend yes.
3. **Where:** a new `src/effects.rs` pass on the bound `Api` before code generation, not in `cgen.rs`? Recommend yes; unify with `writes()`
   later as its own step.
4. **Syntax:** `@effects(...)` attribute and `effects Name = ...;` groups? Recommend yes, matching existing attributes and Stasis.
5. **Vocabulary:** `reads, writes, writes(path), allocates, fails, random, clock, io`? Recommend start with `reads`/`()` only; add `writes(path)` and
   `writes(self)` next; table-specific effects later.
6. **Platform defaults for `@frame`** (no tables, no io) built in rather than written by users? Recommend yes, after the first version works.
7. **`@cache` from a declared-pure endpoint:** yes as a later step, keyed by the data version? Recommend yes, later, and only with a proof that
   the function has no `random`/`clock`/`io`.
8. **Contracts on templates checked per instance, paths limited to `self` and generic names?** Recommend yes.
9. **Do contracts apply to tests?** Recommend yes (`@effects()` on observation helpers), as in Stasis's testing guidance.

## 12. What I could not verify

Read narrowly: `writes`/`allocates` (cgen.rs ~1554-1603), the reader flag (~1441-1490), `Costs`, `recursive_fns` (head only), `platform.rs`
(~91-112), and Stasis `docs/spec.md` (the `@effects` section). Not verified: how the binder marks state versus local roots on a write (needed
for the local-struct case), how endpoint attributes are parsed in `lang.rs`, how the answer cache's data version is exposed (builtin 67 is from a
comment), and whether `uses_spec` really keeps recursive callers off readers in every case (the code comment says so).
