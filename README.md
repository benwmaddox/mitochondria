# Mitochondria

Codename for an experimental language whose memory is a **declared tree of fixed-size state**, all reserved
up front, with the runtime tracking what is actually in use.

```
struct Item  { id: i32; hp: f32; name: string<16>; tags: list<i32, 8>; }
struct World { title: string<40>; scores: i32[8]; items: list<Item, 500000>; }
global world: World;
```

- `list<T, N>`: a list with a maximum size (length + N slots). `T[N]`: exactly N elements. `string<N>`: a string of at most N bytes.
- Layout is exact and computed at startup. The program refuses to start if it is over `--budget`.
- One reservation holds everything; pages are committed only where bytes are live; `trim` returns the rest.
- Growing past a maximum is a typed `FULL` result, never a crash or out-of-memory kill.

```
cargo run --release -- examples/world.mito examples/world.script
cargo run --release -- examples/world.mito --budget 16     # refused: over budget
cargo run --release -- exp                                  # the earlier region experiments
```

Windows only for now. Design, results and gaps: [DESIGN.md](DESIGN.md).
