# Modules and a standard library (decided and built, 2026-11-04)

**Why.** Ordered data should be a collection written in the language, in a standard library, not a new built-in; the
same mechanism should carry later structures (priority queues, keyed indexes) without growing the core.

**What a module is.** One `.mito` file in `lib/`: compile-time parameters, structs, a `state { ... }` block (what an
instance holds) and functions.

```
// lib/sorted.mito (abridged)
module sorted<T: struct, N: count, key: field of T>
struct Link { level: u8; next: u32[12]; prev: u32[12]; }
state {
    items: pool<T, N> checked;
    links: pool<Link, N>;
    head: Head;
}
pub fn add(kv) { let it = self.items.add(); ... }
fn iter_first(from) { ... }   // the walk protocol: a cursor is an integer, -1 for none
```

A program uses it as a field of its state, `board: sorted<Entry, 1000, points>;`, and calls it by path:
`game.board.add(0)`, `game.board.set_key(e, 120)`, `each e in game.board reverse limit 10`.

**Decisions (Ben):**
- **One file per module.**
- **Compile-time copies, one per use.** The memory model is unchanged: an instance's pools are reserved, committed,
  trimmed and rolled back like any others, and nothing exists at run time that the layout did not declare.
- **Keys are read-only outside the module**, changed with `set_key`, so the order can never be wrong.
- **Changing an instance inside a loop over it does not load.**
- **`order by` is removed**; order is kept in the data.

**Built differently from the first sketch.** `sorted` is a skip list, not a B-tree: inner B-tree nodes need copies of
keys, which string keys cannot have. `self` is resolved by the binder, so no local of the module can shadow it. Loops
gained `while`, for walks that stop early.
