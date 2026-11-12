# Generics (design; steps 1 to 5 built, except field privacy)

**Status.** Decided by the owner: generics replace modules. Steps 1 to 4 of the plan (section 9) and the function half of step 5 are built in `src/generics.rs`: generic structs, functions of a generic type copied per call, instance names in messages, `K: field of T`, file-level privacy for functions. Section 12 lists where the build differs from this text.

**Why.** Today the only way to write a reusable container is a library module (`lib/*.mito`): `module sorted<T: struct, N: count, key: field of T>`.
Modules work, but they are a second mechanism beside structs and functions of a type, they are expanded by *text substitution* before parsing
(`src/modules.rs`: `subst`, `instantiate`), they can only be used as a field of a state struct, and their parameter kinds
(`struct`, `count`, `field of`) exist nowhere else. The owner wants generics and prefers one approach, so this proposal asks whether generic
structs plus functions of a type can *replace* modules rather than sit beside them. It argues yes.

## 1. What the language looks like

### Generic structs

```
struct Stack<T, N: count> {
    items: T[N];
    len: i64 in 0..N;
}

fn push(self: Stack<T, N>, x: T) {   // T and N are in scope because they appear in the receiver type
    self.items[self.len] = x;
    self.len = self.len + 1;
}
fn is_full(self: Stack<T, N>) -> bool { return self.len == N; }
```

- A parameter list after the struct name. A bare name (`T`) is a type; `N: count` is a compile-time integer (what modules call `count`);
  `K: field of T` is a field of the type `T` (what modules call `field of`).
- Field types may use the parameters: `T[N]`, `pool<T, N>`, `string<N>`, `list<Item<T>, N>`.
- Use: `a: Stack<i64, 16>;`, `names: Stack<string<12>, 8>;`, a function parameter `s: Stack<Entry, 100>`.
  Arguments are literal types, literal counts, or `const` names. No arithmetic in arguments at first.

### Generic functions of a type

A function of a type already exists (`fn hurt(self: Player, n: i64)`), found by the *type of the receiver where the call is written*.
Generic receivers extend that rule: `fn push(self: Stack<T, N>, x: T)` is a *template*; the call `a.push(3)` binds `T = i64, N = 16`
from the receiver, and `x` is then checked against `T`. No new call syntax, no turbofish.

Free generic functions, only when no receiver can carry the parameters (a later step):

```
fn sum_of<T: number>(a: T[8]) -> T { ... }
```

Inference is *one pass, from the arguments, left to right*: each parameter is bound by its first occurrence in an argument type. No
inference from the return type or the context. An unbound parameter is an error that says so
(`generic parameter T of first<T> is not given by any argument; write first<i64>(...)`).

### Constraints

A constraint is the kind written after the colon, checked at *instantiation*, before the body is looked at:

| written | meaning |
|---|---|
| `T` | any type whose size is known at compile time |
| `T: struct` | a declared struct (what `sorted` needs, since it makes a pool of them) |
| `T: number` | an integer or float type |
| `T: int` | an integer type (so `%`, bit operators and `in lo..hi` are allowed) |
| `T: text` | a `string<N>` / `ascii<N>` type |
| `N: count` | a compile-time integer >= 1 |
| `K: field of S` | a field of struct `S` (another parameter or a struct name), read as `x.K`, typed `S.K` |

A generic body is **type-checked only after instantiation** (as C++ templates and Zig `comptime`, unlike Rust traits). A constraint gives a
better error early; it is not a proof the body relies on. The list is short and closed; user-defined interfaces are a non-goal.

Why: the binder, range analysis and C generator all need concrete types and sizes. Checking a body once, generically, would need a second
type checker for "unknown T". Instantiation-time checking reuses the one that exists.

## 2. How an instance is made: monomorphization, early

Instantiation is **expansion of the template into ordinary declarations before binding and layout**, so everything after it sees only the
non-generic language it sees today.

1. Parse: structs and functions keep a parameter list (`generics: Vec<(String, PKind)>` on each).
2. A new pass (`src/generics.rs`, replacing `src/modules.rs`, called where `modules::expand(state, api)` is called today) walks the state and
   every function signature and body, collecting *uses*: `Stack<i64, 16>` in a type, or a receiver call matching a template.
3. For each distinct use (key: template plus evaluated arguments, `const`s normalized to numbers) it makes one copy with a mangled name,
   `Stack__i64__16`, with parameters substituted **in the parse tree, not in text**. (This differs from `modules.rs::subst`, which replaces
   identifiers in source text outside strings and comments and cannot be made precise about `T.key` or shadowing.)
4. Copies are processed to a fixed point (a copy may use further generics) with a depth limit (64, as `paths` in modules.rs already uses)
   and an instance cap (say 1000), so a recursive generic type cannot blow up the compiler.
5. The result is a plain program. `layout.rs` (`StructDef`, `Ty::Arr/Pool/Struct`, offsets) and `cgen.rs` never see a generic.

Memory model: unchanged. A `Stack<T, N>` has a size known at compile time, lives in the declared state, and is reserved, committed and rolled
back like any other struct. Nothing is allocated at run time. The instantiation *is* the layout, which matches "allocate at startup" and
"compile-time first".

### Where in the source

| step | place |
|---|---|
| parse `struct Name<...>`, `fn name<...>`, type arguments `Name<...>` | `src/layout.rs` (`tokenize`, `P`, `resolve`; `Ty::Named` is "only before resolution", so a `Ty::Generic(name, args)` before resolution fits) and `src/lang.rs` (`PTy`, the signature type, grows the same) |
| the expansion pass | new `src/generics.rs` replacing `src/modules.rs` |
| receiver matching for templates | where `set_methods` (`src/cgen.rs` ~1060, builds `METHODS` from `Func.recv` names) and the binder pick a function by receiver type. Unify `Stack<T, N>` with the receiver's type in the expansion pass so `set_methods` stays plain |
| error messages | the pass keeps `mangled name -> "Stack<i64, 16>"` and the error printer maps back (section 6) |

### Inlining and code size

Functions of a type are already inlined at the call. A generic instance is another such function: a `push` for `Stack<i64, 16>` and one for
`Stack<string<12>, 8>` are two bodies, each inlined. Recursive and outlined functions (`recursive_fns`, `Spec` in cgen.rs) are already
generated per combination of argument kinds, so they need no change.

## 3. Generics and the existing modules

Can `sorted<T, N, key>` become a generic struct plus functions of a type? Almost directly:

```
struct Link { level: u8; next: u32[12]; prev: u32[12]; }
struct Sorted<T: struct, N: count, key: field of T> {      // was: module header + state { ... }
    items: pool<T, N> checked;
    links: pool<Link, N>;
    head: Head;
}
pub fn add(self: Sorted<T, N, key>, kv: T.key) -> T { ... }   // was: pub fn add(kv: T.key) -> T with an implicit self
```

The module's `state { ... }` block *is* the struct body; the implicit `self` becomes an explicit receiver; the module's functions become
ordinary functions of a type in a library file. What this removes: the `module` header, `state {}` block, `parse_module`, `subst`, the
`instance` line, and the rule "a struct holding one must be reached exactly once from the state" (a struct is just a struct).
`foreach s in board` keeps working if iteration looks up functions of the receiver's type, as it does for modules today (not verified,
see section 11).

What modules do that plain structs do not yet:

- **Private helpers.** Module functions without `pub` are private to the instance. Needs a rule: a function without `pub` can be called only
  from functions in the same file.
- **Read-only keys.** "Keys are read-only outside the module." Same mechanism: a field without `pub` is read-only outside its file.
- **`ModInst`** (layout.rs ~113) records that a path is a module instance and its key field. It probably survives as "generic instance of X"
  for `foreach` and `set_key`, a name record rather than a concept.
- **STDLIB table** (name -> embedded source). After the change a library is a normal file; see question 6.

**Recommendation.** Do not keep both. Build generics, move modules one at a time (`intmap`, the smallest, first; `sorted` last), then
delete `modules.rs`. Until the last one moves, `docs/proposals/modules.md` gets a "being superseded by generics" paragraph at the top; when
done, its text is *rewritten* to the generic form (superseded text is rewritten, not kept), and `module` has an end date: gone when the
last lib file has moved.

## 4. Generics and the whole-program analysis

**Range analysis.** `FIELD_RANGES` (cgen.rs ~588, `HashMap<usize, (i64, i64)>` keyed by byte offset into the state) works on offsets after
layout, so it sees instances as ordinary fields: `Stack<i64, 16>.len` and `Stack<i64, 8>.len` are different offsets with their own ranges.
A declared `len: i64 in 0..N` becomes `0..16` and `0..8`. `generate_with` builds twice and checks that assumed ranges hold; it sees
nothing new.

**Whole-program checks** (`writes`, `allocates`, `recursive_fns`, the reader flag, costs) run on bound `Api.fns` after expansion, where an
instance function is an ordinary `Func`. No change, except *reporting*: costs and diagnostics must print `Stack<i64, 16>.push`, not
`Stack__i64__16.push`.

**What could go wrong.**

1. *Code growth.* Each instance is a new body. Mitigation: instance cap; `check --costs` prints the instance count per template.
   (Later: share bodies that do not use the size.)
2. *Instance explosion through nesting* (`Stack<Stack<Stack<...>>>`): depth limit and instance cap.
3. *Name collisions.* Mangled names must not collide with user names: use `__`, and a hash of the arguments when long (MAX_NAME is 128).
4. *Bad constraint messages from deep inside a body* (the C++ template problem). Constraints are checked before the body is expanded, and
   every body error inside an instance is prefixed with the instantiation chain.
5. *`logged`* (opt-in since 2026-11-11): the attribute at the use site applies to the whole instance; needs a test.
6. *Tables.* `table<T>` rows live in SQLite; columns (`TCol`, `Table` in layout.rs) come from the instantiated struct, so a table of a
   generic row type works only after expansion. v1: generic structs may not contain `table<...>` fields (the error says so).
7. *Shadowing:* a parameter named like a struct or field. Parameters shadow nothing: a clash is an error at the declaration.

## 5. Compile-time cost

Expansion is linear in uses x template size. The pipeline already reads the whole program as one unit and builds it twice
(`generate_with`), so hundreds of instances of a hundred lines each are small beside that. Not measured: step 1 of the plan times `mito check`
through the module path on a synthetic 50-instance program as a baseline, and again after each step. The instance count appears in
`check --costs`.

## 6. Error messages that name the instantiation

```
error: `a.push(...)`: x must be i64, not string<12>
  in Stack<i64, 16>.push, from game.mito line 40 (a.push("hi"))
```

and constraint failures before expansion:

```
error: `names: Stack<Entry, 100>`: T is `number` in Stack<T, N>, but Entry is a struct
```

Mechanism: the pass records each mangled name's source form and use site (`Vec<Inst { template, args, use_line }>`), and the error printer
substitutes it back. An error in a template body points to the template line plus `instantiated as Stack<i64, 16> at file:line`.
`src/error_corpus.rs` and `tests/errors` hold expected texts; every new message gets a case there.

## 7. Worked example: one stack, two element types

```
// std/stack.mito  (a library file)
struct Stack<T, N: count> {
    items: T[N];
    len: i64 in 0..N;
}
pub fn push(self: Stack<T, N>, x: T) {
    if self.len == N { fail 409 "stack is full"; }
    self.items[self.len] = x;
    self.len = self.len + 1;
}
pub fn pop(self: Stack<T, N>) -> T {
    if self.len == 0 { fail 409 "stack is empty"; }
    self.len = self.len - 1;
    return self.items[self.len];
}
pub fn size(self: Stack<T, N>) -> i64 { return self.len; }

// app.mito
use "std/stack.mito";
struct Game {
    undo: Stack<i64, 64>;            // score history
    names: Stack<string<12>, 8>;     // recent player names
}
state game: Game;

@post("/play") fn play(score: i64 in 0..1000, name: string<12>) {
    game.undo.push(score);           // instance Stack<i64, 64>; push inlined
    game.names.push(name);           // instance Stack<string<12>, 8>
}
@post("/undo") fn undo() -> i64 { return game.undo.pop(); }
```

(The syntax of endpoints here is schematic; the stack part is the point.) After expansion the compiler works with:

```
struct Stack__i64__64 { items: i64[64]; len: i64 in 0..64; }
struct Stack__string12__8 { items: string<12>[8]; len: i64 in 0..8; }
// plus push/pop/size copies, reported as Stack<i64, 64>.push and so on
```

In the first copy the range analysis knows `items[self.len]` is in bounds because `len` is in `0..64`, so the C needs no bounds check; the
same proof is made separately for the second copy.

## 8. Tests that would prove it

Test the logic, not exact text of generated C beyond one narrow assertion. `tests/cases/*.case` + `.out` is the existing format.

1. `generic_stack.case`: the example; push/pop on both instances, full and empty failures, sizes.
2. `generic_instances_independent.case`: pushing to one leaves the other's `len` unchanged.
3. `generic_bounds_proof`: emit-c of the stack has no bounds check for `items[self.len]` (one narrow assertion).
4. `generic_nested.case`: `Stack<Pair<i64, i64>, 4>`, a generic inside a generic.
5. `generic_limit`: a recursive type such as `struct R<T> { r: R<R<T>>; }` gives the depth error, not a hang.
6. `tests/errors`: wrong constraint (`Stack<i64, 0>`; `Sorted<i64, ...>` with a non-struct), unbound parameter, wrong argument count, wrong
   element pushed; each names the instantiation.
7. Migration parity: before moving a lib module, the old and generic versions of `intmap` produce the same state layout (`layout_c` text)
   and the same test outputs. For `sorted`, the existing `tests/btree`, `tests/ordmap`, `tests/rowmap` suites and the benches are unchanged.
8. `check --costs` lists instance counts; a test asserts `Stack` shows 2.

## 9. Incremental plan

1. **Read, change nothing:** how receiver functions are bound (`Func.recv`, `METHODS`, binder) and how `layout.rs` parses struct bodies and
   types. Time `mito check` on a 50-instance synthetic program for a baseline.
2. **First small step:** generic *structs only*, with type parameters and `N: count`, no generic functions, no receivers.
   `struct Pair<A, B> { a: A; b: B; }`, `x: Pair<i64, string<8>>;` expanded in the new pass before layout. Functions over it are ordinary
   functions of the instance type. Proves parsing, expansion, mangling, instance caching, the limits and error naming, with no receiver logic.
3. Generic receivers (`self: Stack<T, N>`): unify receiver with template, infer `T` and `N`.
4. Constraints and `field of`, with their messages.
5. `pub` privacy and read-only fields.
6. Move `intmap`, then `rowmap`, `strmap`, `bag`, `btree`, `ordmap`, `sorted`; run suites and benches after each.
7. Delete `modules.rs`; rewrite `docs/proposals/modules.md` and the module text in `docs/reference.md` (the `docs_mention_everything` test
   in platform.rs will demand the new syntax appears in the docs).
8. Free generic functions, only if the receiver form proves insufficient.

## 10. Questions for the owner

1. **Replace modules with generic structs?** Recommend yes: one mechanism, migrate in the order above, delete `modules.rs` at the end.
2. **Monomorphize before layout** (this design) rather than any run-time scheme? Recommend monomorphize; there is no run-time type
   information and sizes must be known at compile time.
3. **Check generic bodies after instantiation only**, with the short closed constraint table for early errors? Recommend yes; no
   user-defined interfaces.
4. **Syntax for a type parameter:** bare `T` (this design) or `T: type` (Stasis)? Recommend bare `T`; constraints (`struct`, `number`,
   `int`, `text`) then read naturally after the colon.
5. **Free generic functions now or later?** Recommend later; receiver-bound generics cover containers and inference is where complexity hides.
6. **Library lookup:** `use "std/stack.mito"` like any file, dropping the STDLIB name table? Recommend yes.
7. **Privacy:** file-level privacy for non-`pub` functions and read-only non-`pub` fields? Recommend yes; `sorted`'s `set_key`
   guarantee needs it.
8. **Limits:** instance cap 1000, depth 64, as errors with the chain printed? Recommend yes.
9. **Tables and logged in generic structs:** forbid `table<>` in v1, allow `logged` at the use site? Recommend yes.
10. **Arithmetic in counts** (`Buf<N * 2>`)? Recommend not in v1; literals and `const` only.

## 11. What I could not verify

Read narrowly: `src/modules.rs` (parse, subst, instantiate, expand), type definitions in `src/layout.rs` and `src/lang.rs`, and the analysis
functions in `src/cgen.rs`. Not verified: how the binder resolves `self` and `T.key` in signatures, how `foreach` finds `iter_first` for
module instances, how `ModInst` is consumed in cgen, the exact endpoint syntax, and the expected format in `tests/errors`. Step 1 of the
plan reads these before anything is written.

## 12. Deviations from the design (as built)

1. **Expansion works on token streams of the program text, not on a parse tree.** The state and the code are still text when `modules::expand` is called (it calls `generics::expand`, which wraps the module expansion), so the pass lexes them
   (identifiers, numbers, strings, comments), replaces parameters token by token (never inside a string or comment, never a field after a `.`, except a parameter that names a field) and writes text the existing parsers read. Generated text is appended to the state and the code.
2. **Functions are copied per call (fixed; was: for every instance once the name appeared).** The expansion cannot know a receiver's type, so the choice made, and the only mechanism: `generics::expand` expands the program with the copies asked for so far (none at first),
   runs the normal compile once as a probe, and the code generator, where it resolves `x.f(...)` by the receiver's type (`callx`), records every call whose instance has no function `f` (`note_missing`: instance as written, function name). The next expansion copies exactly those
   functions for exactly those instances; this repeats until a probe asks for nothing new. While probing, the generator goes on past a call that lacks a copy (with a placeholder value), so one probe finds every such call it reaches; a call made only from inside a copy is found by the next probe, so a program needs the depth of its chains of calls between copies, plus one, probes (the generics cases: 2 each; `MITO_EXPLAIN_GENERICS=1` prints each probe). The text of the last expansion is the program; its errors are reported as any program's are. The binder
   cannot find a function that has no copy yet, so it takes a name that some generic struct declares as "declared for some type" (`is_template_method`) and leaves the call to the code generator. Costs: a program with a generic struct and functions of it is compiled once more per probe at load; programs without are unchanged.
   Consequences: `fn pop(self: Stack<T, N>) -> T` works for `Stack<i64, 8>` while `Stack<string<12>, 8>` exists, because that instance never calls it; calling it on the text instance reports the error inside the copy, named `Stack<string<12>, 8>.pop`. Whole-program analyses see the copies that exist; a call `x.f()` is
   treated as reaching every function named `f`, as before, so a missing copy never hides a callee.
3. **No `check --costs` instance count and no `generic_bounds_proof` emit-c assertion**; the range proof is tested through the generated checks (`generic_instances_are_separate_state_to_the_range_analyses` in `src/cgen.rs`, both directions).
4. **Lines (fixed for copies).** A copy of a function is preceded by `//# line N <instance>`; the lexer (`lex_lines` in `src/lang.rs`) continues from line N, so a runtime `fail` and every error inside a copy report the template function's line in the program's own source
   (and `main.rs` maps it to the right file of a program of several files). Struct copies in the state text and errors found while expanding (wrong arguments) still carry no line.
5. **`K: field of T` (step 4, built).** `T` must be a `struct` parameter of the same struct, or a struct name. The argument must be a field name of the struct the other argument stands for (a plain struct, or an instance such as `Pair<i64, i64>`, read from its expanded body), and an integer type, with or without `in lo..hi`;
   the messages name the instance: `` `Sorted<Entry, 3, name>`: key is a field of T in Sorted<T, N, key>, but Entry.name is string<8>, not an integer ``. The parameter's name is replaced by the field's name everywhere in a copy, also after a `.` (a field of the template's own named like the parameter is therefore not usable in it),
   and `T.key` in a signature is the field's type. Fields of type `table<...>` are still rejected; counts are literals only.
6. **`pub` privacy (step 5): functions only.** A copy of a function of a generic struct without `pub` is private to the file it was written in; a call from code in another file of the same program is an error (`Stack<i64, 8>.grow is private to lib.mito (it is not pub), and is called from main.mito`),
   checked where the code generator resolves the call, with the call's line mapped to its file. Not built: privacy for *fields* (read-only keys): a field is reached by many routes in the code generator (state paths, pool columns, array columns, table rows, struct values), each resolving the name by itself, and there is no single place yet where "this field of this instance read from this file" is decided.
7. **Mangled names** start with `__mito__` (as module instance names do) and sanitize the arguments (`__mito__Stack__string_12__8`); a name that would be too long or that collides is numbered.
