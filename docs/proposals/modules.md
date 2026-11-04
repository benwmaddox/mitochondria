# Proposal: modules with compile-time parameters, and a standard library

Status: proposal for review (2026-11-04). Nothing here is built yet.

## Why
Ordered data (`sorted`, a B-tree) should be a collection written in the language, in a standard library, not a new
built-in. The same mechanism should carry later structures (priority queues, keyed indexes, interval maps) without
growing the core, which stays: plain values, `list`, `pool`, `ref`, structs, functions, endpoints.

## What a module is
A module is one file in `lib/` with **parameters**, a **state** part and **functions**:

```
// lib/sorted.mito
module sorted<T: struct, N: count, key: field of T>

--- state
struct Node { n: u16; leaf: bool; keys: key[15]; kids: u32[16]; slots: u32[15]; }
global items: pool<T, N> checked;
global nodes: pool<Node, N / 7 + 2>;
global top: Top;            // root node, height
--- functions
pub fn add() { ... }        // a new item, placed by its key (alloc + insert)
pub fn set_key(it, k) { ... }
pub fn remove(it) { ... }
pub fn first_at_least(k) { ... }
iter fn start(from) { ... } // the iteration protocol, below
iter fn step(cur) { ... }
```

A program uses it in its state with concrete arguments:

```
global scores: sorted<Score, 10000, points>;
```

That is **compile-time instantiation**: the compiler substitutes `Score`, `10000` and `points` and lays out the module's
state under the name `scores` (`scores.items`, `scores.nodes`, ...). Each use is its own copy, with its own layout,
so the memory model is unchanged: the instance's pools are reserved, committed and trimmed like any others, writes go
through the journal (rollback works), and `inspect` shows them. Nothing exists at run time that the layout did not
declare.

## Using an instance
- **Functions** by instance name: `let s = scores.add(); s.name = "ann"; scores.set_key(s, 120);`
- **Items** are ordinary items of `scores.items`: fields read and written as usual, `ref<scores>` refers to them (the
  module's item pool counts as a top-level pool), generations check stale references.
- **Loops**: `each s in scores where s.name != "" limit 10 { ... }` walks in key order (`reverse` for descending), and
  `each s in scores from 100 ...` starts at the first key at least 100.

## The iteration protocol
No closures and no unbounded loops, so a module that can be iterated declares two functions:

- `iter fn start(from)`: a cursor (an integer, or null when there is nothing) for the first item at or after `from`
  (null `from`: the first item).
- `iter fn step(cur)`: the cursor after `cur`, or null.

plus `iter fn item(cur)` giving the item at a cursor. The compiler turns `each s in scores ...` into
`start`, then `step` until null, binding `s` to `item(cur)`, with `where`, `limit` and `reverse` (`iter fn start_rev`,
`step_rev`) as for other loops. A loop takes at most `N` steps (the instance's capacity), so it always ends.

## Encapsulation
Outside the module, its state is read-only: only its `pub` functions change `nodes` and `top`. Item fields are
writable, except the parameter marked `key`, which only the module writes (`set_key`). This is what keeps the index
right; it is checked when the program is compiled.

## Parameters
Kinds, checked at instantiation: `struct` (a struct type), `count` (an integer known at compile time; arithmetic on
it is allowed in capacities), `field of T` (a field of a struct parameter, with its type usable as a type, as in
`keys: key[15]`). Errors are reported at the use, naming the module line.

## What this needs from the compiler
1. Parameterised parsing and substitution of a module's state and functions (both parsers already exist).
2. Instance-qualified names (`scores.add`, `scores.items`) in the binder.
3. The read-only rule for module state and key fields.
4. The iteration protocol in the interpreter and the generator (calls to generated functions, as for recursion).
Counted loops (built) cover the search inside a node.

## Decisions for review
1. **One file per module** holding state and functions (proposed), or keep state and functions in separate files as
   programs do now?
2. **Compile-time instantiation**, one copy per use (proposed), or shared code with run-time parameters? (Copies cost
   compile time and code size, not memory: state is per instance either way.)
3. **Key read-only outside the module** (proposed), or hooks that run when a field is assigned?
4. **Cursors as integers** returned by `start`/`step` (proposed): simple and allocation-free, but a module could hand out
   a cursor that is no longer valid if the structure changes during a loop. Proposed rule: changing a module instance
   inside a loop over it is a compile error (the generator already knows what loop bodies write).
5. **`order by`** goes once `sorted` exists; `GET /rooms` becomes `global rooms: sorted<Room, 64, created>` (rooms are
   then items of `rooms.items`).
