# Struct values (design proposal, not built)

**Status.** For the owner's review. Nothing here is implemented; no compiler changes accompany it. The owner has decided that struct values are wanted, so this
is about *how*: clearly in the syntax, cheaply in the generated C.

**The question first, and the answer in one paragraph.** Today a plain struct can only be a *place* in the state (a view: "Struct parameters" in DESIGN.md).
A value is something you can hold in a local, build from a literal, return, copy with `=`, and put in a response. Recommendation: add **copy values** with one new
storage kind, *a struct local is a place in a per-request scratch area at a compile-time offset*. The existing `CV::Plain(struct, offset)` already carries every analysis
(ranges, reads, logged, text windows, nested structs, arrays), so a local reuses all of it for free. Then, as a pure optimization checked by a differential test, lower
non-escaping locals to C scalars. Struct parameters are copies by default; `self` and parameters marked `view` stay views. Arithmetic is method calls
(`a.sum(b).scaled(2.0)`); operators wait.

## 1. What exists and what is missing

| fact | where |
|---|---|
| a plain struct argument is `CV::Plain(struct id, offset)`: a view of a place in the state blob; reads are `LD_F64(off, 0)`, writes `ST_F64`; field access in the `CV::Plain(sid, off)` arm (~3793) calls `touch_request(off, sid, k)` | `enum CV`, `src/cgen.rs` 501 |
| the kind of a value as the compiler tracks it: `K::Plain(struct, offset)`; arguments to outlined or recursive functions are `PK::Plain(struct, offset)`, one C function **per place** | `enum K` 596, `enum PK` 533, `call_spec` 1836 |
| a function result of kind `K::Plain` is refused ("a function can take it as a parameter but not return it") | `ret_kind`, ~2006 |
| `to_declared` checks an argument against a declared parameter type; a nullable struct parameter is refused | ~2058, ~2048 |
| using a struct as a value (`let a = world.p;`) reaches "a collection, struct or array used as a value is not compiled yet" | ~3544 |
| inlining is the default: `inline` -> `call_values` -> `call_values_inner` -> `inline_body`; a recursive or outlined function becomes a C function with the result written through `ret_v` (`Ret { func: true }`, ~1923) | 4867-4990 |
| scalar locals are C locals (`let x = ...` becomes a `double` or `i64` C variable) | statement lowering in `cgen.rs` |
| `Elem`, `Part`, `ArrCol`, `PlainArr`: views of structs and arrays inside items and plain arrays | `CV`, 515-518 |

The library author's pain points in lib/std/vec.mito all come from the missing rows: a returned `a + b`; `normalize(a - b)` needing a scratch field in the user's
state; a whole vector in a response; a returned pair; temporaries in `cross_to`, `rotate`, `reflect_to`.

I could not verify (section 12): how the per-request scratch/text area is laid out, and whether it can hold constant-offset slots. Section 3 says what is assumed and the fallback.

## 2. Value semantics and the parameter list (question a)

Model: **a struct variable is a place; `=` copies the contents of one place into another; a view is a second name for a place.** Nothing else.

- `let v = world.pos;` makes a new local place and copies. `world.pos` is unchanged by later writes to `v`.
- `v = world.pos;` copies into the existing place `v`. `world.pos = v;` copies into state (a state write: logged if the state is, range-checked).
- Passing to a function: a copy, unless the parameter is a view.

### Three candidates, the same four examples

Examples: vector add (returns), normalize, in-place plus, a function that mutates a player in state.

**A. Explicit keyword `view`** (a keyword, not `&`).
```
fn sum(self: Vec3, other: Vec3) -> Vec3 { return Vec3 { x: self.x + other.x, y: self.y + other.y, z: self.z + other.z }; }
fn normalized(self: Vec3) -> Vec3 { let len = self.length(); if len > 0.0 { return self.scaled(1.0 / len); } return self; }
fn plus(view self: Vec3, other: Vec3) { self.x += other.x; self.y += other.y; self.z += other.z; }
fn heal(view p: Player, n: i64) { p.hp += n; }          // heal(world.player, 5) changes the state
```
**B. By position: `self` is a view, every other struct parameter is a copy.**
```
fn sum(self: Vec3, other: Vec3) -> Vec3 { ... }               // same as A
fn plus(self: Vec3, other: Vec3) { self.x += other.x; ... }   // self written: in place
fn heal(self: Player, n: i64) { self.hp += n; }               // fine, but only as a method
fn add_into(dst: Pos, src: Pos)                               // dst is a copy: writing it is useless -> error, and there is no way to say "mutate this"
```
B breaks today's `add_into(dst, src)` and every free function that mutates a state struct.

**C. By argument kind: a state place is passed as a view, anything else (local, literal, call result) as a copy.**
```
fn sum(a: Vec3, b: Vec3) -> Vec3 { ... }     // sum(world.a, world.b): views, no write, invisible
fn plus(a: Vec3, b: Vec3) { a.x += b.x; }    // plus(world.a, ...) changes state; plus(local, ...) changes the local;
                                             // plus(Vec3 { ... }, ...) writes a temporary: silently useless
```
C is what today's code does, but one function means two things at two call sites, and "silently useless" writes are the worst confusion.

### Recommendation: A, with `self` implicitly a view

- `self` is the place the method is called on, so `world.pos.normalize()` and `local.normalize()` both work in place, and `a.sum(b).normalize()` normalizes the
  temporary. This is how today's in-place methods already read, so the library keeps working.
- A parameter other than `self` is a **copy** unless declared `view p: T`. Writing a non-view, non-self struct parameter is a compile error: "`dst` is a copy: writes to it are lost;
  declare it `view dst: Pos` to change the caller's struct" (the migration note, section 9).
- `view` accepts a place (state or local). It does not accept a literal or a call result ("a view needs a place"); that check turns C's silent bug into an error.
- A `view` parameter may alias another parameter (`add_into(a, a)` is fine, as today). A copy is never an alias.

### Copy elision (never visible to the user)

A copy parameter that the callee tree never writes, with a source place that the callee tree never writes either, is passed as a view: observationally identical, no copy.
The compiler already walks the call tree for writes (`fn writes`, ~1554); this needs the walk to report *which places* (struct id and offset range) instead of yes/no,
falling back to "copy" when it cannot tell. In inlined code a copy is loads of the fields into C locals, which the C compiler forwards anyway; elision matters mostly for outlined
or recursive functions and for large structs.

## 3. Storage and efficiency (question b)

### Locals: scratch places first, scalar replacement second

**Step-1 storage.** A struct local gets a *place in a per-request scratch area* at an offset fixed at compile time (locals in disjoint blocks may share slots; an inlined
function's locals get slots in the caller's frame). The local is `CV::Plain(sid, scratch_off)`, so every existing operation works unchanged: field reads and writes, ranges,
nested structs, arrays, text fields, passing as a view, `touch_request`. The cost is memory traffic through `LD_`/`ST_` instead of registers: a few loads and stores per
operation on a 3-field vector.

**Step-2 optimization: scalar replacement.** If a struct local never escapes (only read or written by field; never indexed with a run-time index; never passed by view to an
outlined or recursive function; never stored whole into an array), lower each scalar field to a C local (`double v_x, v_y, v_z;`) and flatten nested structs (`v_pos_x`).
The C compiler then removes them. Eligibility is decided after inlining, where use sites are known. A **differential test** runs every struct test with scalar replacement on
and off (a test-only flag) and compares output, so the optimization cannot change behavior.

*Why not scalar replacement first?* It needs a second place abstraction (`Place::State(off) | Place::Locals(names)`) threaded through the ~40 `CV::Plain` use sites (field access,
assignment, ranges, arrays, text, equality, JSON output). Scratch-first gets correct semantics in the existing machinery in one change; the optimization is then additive.

### Size limit

A struct value (local, literal, copied parameter, result) is at most **256 bytes** (a constant, easy to raise). The size comes from `layout.rs`. The message:
```
error: `Level` is 1,312 bytes: a struct used as a value can be at most 256 bytes (pass it as `view`, or split it)
  --> game.mito:14: let l = world.level;
```
A `view` parameter has no limit; only copies do. Arrays inside a value count in full (`f64[16]` is 128 bytes).

### Returns: out-parameter convention, no heap, no allocation

An inlined function returning a struct writes its `return` expression into a destination the **caller** reserved, so `let c = a.cross(b)` has no intermediate copy: the return
expression is evaluated straight into `c`'s place. If a source place may overlap the destination (`a = a.cross(b)`), the compiler evaluates into a hidden temporary slot first
(the same job the library does by hand today with its local `x, y, z`).

An outlined or recursive function (`Ret { func: true }`, today `i64 *ret_v`) takes `S_Vec3 *ret_v`, a pointer to a slot in the caller's frame. `call_spec` gets a result kind
`K::Val(sid)` and an argument kind `PK::Val(sid)` (a copy passed by `const` pointer to its slot; a view keeps `PK::Plain` and its per-place specialization). No run-time allocation
anywhere: every value has a compile-time slot.

Pairs: return a declared struct (`struct Hit { point: Vec3; normal: Vec3; }`). No tuples: one way.

Recursive functions with struct locals need a frame (the slot differs per depth). Step 1 refuses struct locals inside recursive functions with a clear message; a later step gives
recursive specializations a frame base. (Struct *results* of recursive functions work: the slot is the caller's.)

### Generated C

Assume `struct Vec3 { x: f64; y: f64; z: f64; }`. `fn dot3(a: Vec3, b: Vec3) -> f64 { return a.x*b.x + a.y*b.y + a.z*b.z; }` inlined at `world.a.dot3(world.b)`
(copies elided: the callee writes nothing):
```c
double r_1 = LD_F64(A_OFF + 0, 0) * LD_F64(B_OFF + 0, 0)
           + LD_F64(A_OFF + 8, 0) * LD_F64(B_OFF + 8, 0)
           + LD_F64(A_OFF + 16, 0) * LD_F64(B_OFF + 16, 0);
```
`let c = a.cross(b)` in step 1 (scratch slot `S`; direct evaluation, no overlap here):
```c
{ double t0 = LD_F64(A_OFF+8,0)*LD_F64(B_OFF+16,0) - LD_F64(A_OFF+16,0)*LD_F64(B_OFF+8,0);
  double t1 = LD_F64(A_OFF+16,0)*LD_F64(B_OFF+0,0)  - LD_F64(A_OFF+0,0)*LD_F64(B_OFF+16,0);
  double t2 = LD_F64(A_OFF+0,0)*LD_F64(B_OFF+8,0)   - LD_F64(A_OFF+8,0)*LD_F64(B_OFF+0,0);
  ST_LOCAL_F64(S+0, t0); ST_LOCAL_F64(S+8, t1); ST_LOCAL_F64(S+16, t2); }
```
After step 2, if `c` only feeds `c.length()`: the stores vanish and the code is `sqrt(t0*t0 + t1*t1 + t2*t2)`. An outlined struct-returning function, called as `let c = f(n)`:
```c
static int f_v3(Req *rq, i64 n, S_Vec3 *ret_v);       /* writes *ret_v, returns 0 */
S_Vec3 c; f_v3(rq, n, &c);                            /* the slot in the caller's frame */
```

## 4. Syntax (question c)

### Literals

```
let v = Vec3 { x: 1.0, y: 0.0, z: 0.0 };       // named fields, every field given, any order
let z = Vec3 {};                                 // all zero (the one allowed omission, same as fresh state)
let w = Vec3 { ..v, y: 5.0 };                    // update: copy v, change the listed fields
world.pos = Vec3 { x: 1.0, y: 2.0, z: 3.0 };     // field-by-field store into state, logged if the state is
```
- Named fields only; no positional `Vec3(1.0, 2.0, 3.0)` (a second way, and field order would become API). No shorthand `Vec3 { x, y, z }` in step 1; revisit if it proves noisy.
- Field update: `..base` inside a literal is the only form; `v.x = 2.0` on a local is ordinary assignment; there is no `with` keyword.
- Parsing: `if a == Foo { x: 1 }` is ambiguous with a block. Rule: a literal is recognized only when the name is a declared struct and the token after `{` is `ident :`, `..` or `}`;
  in an `if`/`while`/`foreach` header it must be parenthesized (the error says so). The anonymous object in `ok({ ... })` is unchanged; inside it a struct-valued field takes a value: `ok({ pos: v })`.
  Alternative if the owner dislikes the rule: a different delimiter.

### Method chains

Each method returns a value, the receiver of the next: `a.sum(b).scaled(2.0).normalized()`. Intermediates are scratch slots; after step 2 they are scalars.
An in-place method and a returning method of the same name and receiver type would be ambiguous, so the library separates them by name (section 9): in place `plus`, `scale`,
`normalize`; returning `sum`, `scaled`, `normalized`. A reader sees from the name whether the receiver changes.

### Operators (recommend: not now)

`a + b` mapping to `fn add(self: Vec3, other: Vec3) -> Vec3`:
- For: reads like math; `a + b * 2.0` is the biggest readability gain for vector code.
- Against: one-way principle: `a.add(b)` and `a + b` are two spellings. `k * v` (scalar on the left) needs a lookup on the *right* operand, which breaks "methods belong to the receiver".
  Operators drag in unary minus, compound assignment (`pos += vel`: in place or a new value?), precedence with the existing int/float coercion rules, and none of that is needed by step 1.
- Recommendation: **method chains only through step 5**; revisit after the library is in the new style and the owner has seen how chains read. If added, the operator is *the* spelling for
  the arithmetic set (`add sub mul div neg`, only for types that declare the named method with the matching signature), and the method name is not documented as a second way.

## 5. What a value can contain (question d)

Allowed: scalars (including ranged ones), nested structs, fixed arrays of numbers or structs (`f64[4]`, `Vec3[2]`), and text with a declared size (stored inline as length plus bytes;
a copy is a `memcpy` of at most the declared size). Not allowed: pools, lists, tables, item references (`Ref`), nullable fields. Reason: a copy must be a bounded memcpy whose meaning does
not change; a copied reference would be a second owner of nothing, and a table cannot be copied without a policy. The error names the field:
"`Level` contains a table (`tiles`): a struct with a table inside is part of the state; it cannot be a value".

## 6. Interactions (question e)

**Ranges.** A field declared `n: i64 in 0..100` keeps its range in every copy, because the local is a `CV::Plain` of the same declared struct, with the same `ranges[k]`. Each with a test:
`let q = world.p;` gives `q.n` in 0..100; `q.n = 500` is rejected exactly as `world.p.n = 500` is; `world.p = q` adds nothing to the stored-values union beyond what the field can
hold; a literal `Vec3 { n: 500 }` is rejected at the literal. For flow-sensitive narrowing (`Ret.ivs`, the per-variable interval of an inlined return) a returned struct carries one
interval per scalar field; step 3 may widen to the declared range (sound, less precise) and say so. Ranges flow through copies by construction: a copy is a field-by-field assignment
checked by the existing store check.

**Generics.** `Stack<Vec3, 8>`: element storage is `Vec3[8]` inside the module instance's state (a plain array of structs place); `push(v: T)` copies in, `get(i) -> T` copies out
(needs struct results: step 3), `peek` returns a copy. No view of an element escapes. Today's generic modules take scalars and items as `T`; struct `T` is a step-5 task, specialized
by struct id as `Spec` already specializes by place.

**Endpoint responses and inputs.** `Response<{ pos: Vec3 }>` is allowed: the response type walker spells struct fields out as JSON objects (`{"pos":{"x":..,"y":..,"z":..}}`), and
`ok({ pos: v })` takes a value or a place (read at that point). As an input, a JSON-object parameter of struct type parses field by field into a local slot with the same range checks
the scalar parameters have (a missing or out-of-range field is a 400 naming the field path). Step 4.

**`logged` state.** Writes to a local are not logged (not state). `world.pos = v` is logged as the scalar stores it is, one entry per scalar, exactly as the same code written field by
field, so undo restores the old fields. A `view` parameter onto a logged place logs through the place as today. A write to a copy parameter is an error, not a quiet non-log.

**Parallel readers and `reads`.** Copies from state read the touched fields (`touch_request`). Locals are not state writes, so an endpoint that builds vectors from state and
returns them is still a reader. Reader detection scans the *generated C text* for write helpers (`->tset(`, `->store(`, ... see effects.md section 1), so scratch stores must use a
macro the scan does not count (`ST_LOCAL_*`); otherwise every endpoint with a struct local would lose the parallel path. Test both directions: a GET with locals stays a reader; a
GET (or any endpoint) that copies a value into state is not.

**Whole-program checks.** Range unions, the module write restriction ("a module instance's state that only its own functions may write") and `check --costs` see a copy into state as
the stores it is; copies themselves cost no queries. A copy of a module's private struct out of the module is a read, allowed.

## 7. Equality and printing (question f)

`a == b` and `a != b` on two values of the same struct type: fieldwise `&&` of scalar comparisons; arrays elementwise; text by content; nested structs recursively; floats follow IEEE
(`NaN != NaN`, so a struct holding a NaN is not equal to itself; documented). No ordering. Either side may be a place or a value. `str(v)` is not proposed: there is no one right
format. `log v` and responses print JSON text (`{"x":1,"y":2,"z":3}`) because they need a text form anyway, and tests see the same text. Recommend `==` in step 3, no `str`.

## 8. What the compiler would have to do and where

| change | where |
|---|---|
| parse `Struct { f: e, ..base }`, a `view` marker on parameters, `-> Struct` allowed | `src/lang.rs` (parser, `Func.params`, new `view` flag), the ambiguity rule above |
| bind: literal fields checked against the declaration; unknown/duplicate/missing fields with `lang::suggest`-style messages | `lang.rs` binding, messages like the existing `no field` |
| a struct local: reserve a scratch slot, produce `CV::Plain(sid, off)`; the 256-byte check | `Stmt::Let`/`Var` lowering in `src/cgen.rs`, new `scratch_alloc`, struct sizes from `layout.rs` |
| `a = b` between places: field-by-field store, type equality, overlap-safe via a temporary | the assignment arm that says "not compiled yet" (~3544) |
| struct results: `ret_kind` allows plain structs (~2006); `K::Val`/`PK::Val`; the caller reserves the destination | `ret_kind`, `call_spec` (1836-1960), `Ret` |
| `view` versus copy: `to_declared` (~2058) copies a copy parameter into a fresh slot unless elision allows; an error on a write to a non-view parameter | `to_declared`, `fn writes` returning places |
| scalar replacement | a pass over a function's lowered statements keyed on slot offsets, in `cgen.rs` |
| `==`, struct JSON output, response schema | expression lowering, the `Response<...>` type walker, the JSON writer |
| docs | DESIGN.md "Struct parameters" gains "Struct values" |

`layout.rs` and the whole-program range check change only by exposing struct sizes and adding the scratch size (a copy is stores, already checked).

## 9. Migration of lib/std/vec.mito (question g)

Compatibility: every existing in-place method keeps its name and behavior (`pos.plus(vel)` still changes `pos`); `self` stays a view; tests/cases/std_vec.case and struct_params.case
pass unchanged. The one intentional break: a non-`self` struct parameter written in the body now needs `view`. In the library the written struct is always `self`, so it is unaffected;
user code such as `fn add_into(dst: Pos, src: Pos)` gets a one-line fix the compiler spells out. To soften it: a **warning** in step 2, an **error** in step 3.

Same operations, old and new:
```
// old: in place, results into self
fn plus(self: Vec3, other: Vec3) { self.x += other.x; self.y += other.y; self.z += other.z; }
fn cross_to(self: Vec3, a: Vec3, b: Vec3) {
    let x = a.y * b.z - a.z * b.y; let y = a.z * b.x - a.x * b.z; let z = a.x * b.y - a.y * b.x;
    self.x = x; self.y = y; self.z = z;
}
fn normalize(self: Vec3) { let len = self.length(); if len > 0.0 { self.x /= len; self.y /= len; self.z /= len; } }
fn reflect_to(self: Vec3, v: Vec3, n: Vec3) { let d = 2.0 * (v.x*n.x + v.y*n.y + v.z*n.z); self.x = v.x - d*n.x; /* y, z likewise */ }
fn rotate(self: Vec2, radians: f64) { let c = cos(radians); let s = sin(radians); let x = ...; let y = ...; self.x = x; self.y = y; }

// new: returning forms (struct values)
fn sum(self: Vec3, other: Vec3) -> Vec3 { return Vec3 { x: self.x + other.x, y: self.y + other.y, z: self.z + other.z }; }
fn diff(self: Vec3, other: Vec3) -> Vec3 { return Vec3 { x: self.x - other.x, y: self.y - other.y, z: self.z - other.z }; }
fn scaled(self: Vec3, k: f64) -> Vec3 { return Vec3 { x: self.x * k, y: self.y * k, z: self.z * k }; }
fn cross(self: Vec3, o: Vec3) -> Vec3 {   // no temporaries: the literal is evaluated before any store
    return Vec3 { x: self.y * o.z - self.z * o.y, y: self.z * o.x - self.x * o.z, z: self.x * o.y - self.y * o.x };
}
fn normalized(self: Vec3) -> Vec3 { let len = self.length(); if len > 0.0 { return self.scaled(1.0 / len); } return self; }   // `return self` copies
fn reflected(self: Vec3, n: Vec3) -> Vec3 { return self.diff(n.scaled(2.0 * self.dot(n))); }
fn rotated(self: Vec2, radians: f64) -> Vec2 { let c = cos(radians); let s = sin(radians); return Vec2 { x: self.x * c - self.y * s, y: self.x * s + self.y * c }; }
```
Out-style `cross_to`, `lerp_to`, `reflect_to` become redundant: `world.n = a.cross(b)` assigns into any place, and `a = a.cross(b)` is overlap-safe. Deprecate them after step 3 (keep until then);
`assign` is redundant with `=` and is deprecated at the same time.

**In-place functions that stay useful:** `plus`, `minus`, `scale`, `add_scaled`, `negate`, `normalize`, `perp`, `rotate`, `set`, `project_on`. They mutate a state place with no temporary,
and the in-place call is the clear spelling for per-tick updates (`world.player.pos.add_scaled(world.player.vel, dt)`). A returning form assigned back
(`world.player.pos = world.player.pos.sum(...)`) is longer and stores the same fields. Queries (`dot`, `length`, `distance`) are unaffected.

## 10. Worked example end to end: a physics step

```
struct Body { pos: Vec3; vel: Vec3; mass: f64 in 0.1..1000.0; }
state world: World;   // struct World { b: Body; gravity: Vec3; }   (old style adds: scratch: Vec3;)
include lib/std/vec.mito
```
**Old style** (today): a scratch field in the user's state exists only to hold `vel * dt`:
```
@post("/step/{dt}")
fn step(dt: f64) -> Response<{ px: f64, py: f64, pz: f64, speed: f64 }> {
    world.b.vel.add_scaled(world.gravity, dt);
    world.scratch.assign(world.b.vel);
    world.scratch.scale(dt);
    world.b.pos.plus(world.scratch);
    return ok({ px: world.b.pos.x, py: world.b.pos.y, pz: world.b.pos.z, speed: world.b.vel.length() });
}
```
**New style** (after step 4):
```
@post("/step/{dt}")
fn step(dt: f64) -> Response<{ pos: Vec3, speed: f64 }> {
    world.b.vel.add_scaled(world.gravity, dt);                      // in place: the clear form for state
    let moved = world.b.vel.scaled(dt);                             // a local; no scratch field in the state
    world.b.pos = world.b.pos.sum(moved);                           // or world.b.pos.plus(moved)
    return ok({ pos: world.b.pos, speed: world.b.vel.length() });   // the vector whole
}
```
**Generated C sketch** after scalar replacement (step 2 shape; `LD_F64`/`ST_F64` are the existing state accessors; `moved` never touches memory; the `resp_*` names are illustrative):
```c
double vx = LD_F64(VEL+0,0) + LD_F64(G+0,0)*dt;  ST_F64(VEL+0,0,vx);   /* add_scaled inlined, three of these */
double vy = ...;  double vz = ...;
double mx = vx*dt, my = vy*dt, mz = vz*dt;                             /* `moved`: three C locals */
ST_F64(POS+0,0, LD_F64(POS+0,0)+mx);  ST_F64(POS+8,0, LD_F64(POS+8,0)+my);  ST_F64(POS+16,0, LD_F64(POS+16,0)+mz);
resp_obj_begin(rq); resp_key(rq,"pos"); resp_vec3(rq, LD_F64(POS+0,0), LD_F64(POS+8,0), LD_F64(POS+16,0));
resp_key(rq,"speed"); resp_f64(rq, sqrt(vx*vx + vy*vy + vz*vz));
```
In the step-1 form `moved` is three `ST_LOCAL_F64` stores and three loads into the scratch slot; the arithmetic and the state stores are identical. This endpoint writes state, so
it is not a reader in either style; a GET that builds `Vec3` locals from state stays a reader in both.

## 11. The staged plan (question h)

Each step merges alone and leaves the old library working.

**Step 1: struct locals, literals and copy-assignment, with scratch places. No returns, no `view` change.**
`let v = Vec3 { x: 1.0, y: 2.0, z: 3.0 };`, `let c = world.pos;` (copy), `world.pos = c;`, `v = w;`, `world.pos = Vec3 { ... };`, `Vec3 {}`, `..base`. Passing a local to today's struct
parameters already works because a local is a `CV::Plain`. Struct locals are refused in recursive functions. Useful at once: a temporary vector needs no scratch field in user state.
Files: `src/lang.rs` (literal parse), `src/cgen.rs` (`Let` lowering, scratch allocation, the assignment arm at ~3544, `ST_LOCAL_*`, size limit); `layout.rs` only exposes sizes.
Tests (new `tests/cases/struct_values.case`): all fields given; missing, unknown and duplicate field errors; copy independence in both directions (write `v`: state unchanged; write state: `v` unchanged);
`v = w`; same place both sides (`world.pos = world.pos`); the size limit at 257 bytes (error) and 256 (ok); nested struct, array and text field copies;
**ranges both ways**: `let q = world.p; q.n = 500;` rejected, `world.p = q;` accepted, an out-of-range literal rejected;
**reads both ways**: a GET building locals from state is a reader (no write helper in its C), a GET that copies into state is not;
**logged both ways**: `world.p = v` is undone by undo, writes to `v` are not logged; a struct local in a recursive function gives the clear error; std_vec.case and struct_params.case unchanged.

**Step 2: scalar replacement and the differential harness.** Escape analysis on locals; a test flag to disable it; every step-1 test runs both ways and compares output; a physics-loop benchmark must be
no slower (expect a clear gain on vector chains). Also the non-`view` write **warning**. Files: `src/cgen.rs`.

**Step 3: struct returns, copy parameters with elision, `view`, `==`.** `-> Vec3` for inlined, outlined and recursive functions; `K::Val`, `PK::Val`; the `view` keyword; non-view write becomes an **error**.
Files: `ret_kind`, `call_spec`, `to_declared`, `fn writes` (places), `lang.rs`. Tests: `a.sum(b).scaled(2.0)` chains; alias safety (`a = a.cross(b)`, `world.v = world.v.cross(world.w)`);
copy versus view observed (the callee writes the source while holding a copy: the copy keeps the old value, a view sees the new one; both directions); a recursive function returning a struct; a returned pair
struct; struct equality including NaN. The library gains the returning forms.

**Step 4: responses and inputs.** `Response<{ pos: Vec3 }>`, `ok({ pos: v })`, struct JSON inputs with ranges. Tests: JSON shape; an out-of-range input gives a 400 with the field path.

**Step 5: generic modules over value types** (`Stack<Vec3, 8>`), arrays of struct values as locals, frames for struct locals in recursive functions.

**Step 6 (optional, after the owner has seen chains in use):** operators for the arithmetic set; remove the deprecated `*_to` and `assign`.

## 12. Risks and what I could not verify

- **Scratch area.** I assumed a per-request region addressable by constant offsets with `LD_`/`ST_`. I did not verify how the request text area, workers and `touch_request` interact. If requests have no
  private fixed-layout area, the fallback is a generated C `S_Vec3 c;` per local with a second place kind (the `Place` abstraction paid up front, as in section 3).
- **Copy elision needs place-level write sets** (the current `writes` walk is yes/no). Until then every copy parameter really copies: correct, slower for large structs.
- **Aliasing** in `a = a.cross(b)`: a bug here is silent wrong math; the test is mandatory.
- **Float semantics** in `==` (documented). **Parse ambiguity** of literals in conditions (parenthesize rule).
- **The 256-byte limit** will make some big structs a compile error where `view` is the workaround; intentional.
- **Neighbors.** next-lang principles (plain records, fixed stack, copy on assign, no special cases) match this proposal; the difference is that Mitochondria has *places in a state blob*, so `view` exists
  because mutating state is a core use. Stasis passes struct parameters as views and has no struct returns (spec_implementation_status.md lists structs and views without value returns); this proposal
  goes further and keeps views for the state case.

## 13. Questions for the owner, with recommendations

| # | question | recommendation |
|---|---|---|
| 1 | Struct parameters: copies by default, a `view` keyword, `self` implicitly a view | yes (candidate A) |
| 2 | Literal syntax: named fields, all required, `{}` for zero, `..base` for update; no positional, no shorthand | yes |
| 3 | Operators `a + b` | not now; revisit at step 6; if added they are the documented spelling |
| 4 | Value size limit | 256 bytes, a constant; `view` bypasses it |
| 5 | Text and fixed arrays inside a value; collections never | yes |
| 6 | A write to a non-view, non-self struct parameter | warning in step 2, error in step 3 |
| 7 | Library names: returning `sum diff scaled normalized cross reflected rotated`; in-place keep `plus minus scale normalize rotate`; deprecate `*_to` and `assign` | yes |
| 8 | Tuples for multiple returns | no; return a declared struct |
| 9 | `==` on structs, `str(v)` | `==` in step 3; no `str` |
| 10 | Storage: scratch places first, scalar replacement second | yes: it reuses every existing analysis |
