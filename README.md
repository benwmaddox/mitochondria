# Mitochondria

Codename for an experimental language whose memory is a **declared tree of fixed-size state**, all reserved
up front, with the runtime tracking what is actually in use.

```
struct Item  { id: i32; hp: f32; name: string<16>; tags: list<i32, 8>; }
struct World { title: string<40>; scores: i32[8]; items: list<Item, 500000>; }
global world: World;
```

- `list<T, N>`: a list with a maximum size (length + N slots). `T[N]`: exactly N elements. `string<N>`: a string of at most N bytes.
- `table<T, N>` (column storage, dense rows) exists in the prototype but is being folded into `pool` and column-stored `list`; see DESIGN.md.
- `pool<T, N>` gives stable slots, a live bit per slot, and reuse of the lowest free slot; `pool<T, N> checked` adds a generation so stale handles are rejected. Plain indexes cost the same as a table row index plus one bit test.
- Layout is exact and computed at startup. The program refuses to start if it is over `--budget`.
- One reservation holds everything; pages are committed only where bytes are live; `trim` returns the rest.
- Growing past a maximum is a typed `FULL` result, never a crash or out-of-memory kill.

```
cargo run --release -- examples/world.mito examples/world.script
cargo run --release -- examples/world.mito --budget 16     # refused: over budget
cargo run --release -- exp                                  # the earlier region experiments
```

Endpoints: `mitochondria serve examples/chat/chat.mito examples/chat/chat.api --port 8080` runs a chat server's hot state and API;
`mitochondria bench ...` runs it in-process. See [docs/chat-api-findings.md](docs/chat-api-findings.md) for what worked and what did not.

Windows only for now. Design: [DESIGN.md](DESIGN.md). Measurements: [docs/measurements.md](docs/measurements.md).

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
