# Mitochondria

Codename for an experimental language whose memory is a **declared tree of fixed-size state**, all reserved
up front, with the runtime tracking what is actually in use.

```
struct Msg     { seq: i64; author: ref<sessions>; body: string<2000>; }
struct Room    { name: string<48>; messages: list<Msg, 1000> drop_oldest; }
struct Session { name: string<32>; secret: i64; }
global sessions: pool<Session, 2000> checked;
global rooms: pool<Room, 64>;
```

- `list<T, N>` is ordered, with a declared full policy (`fail`, or `drop_oldest` for a ring). `pool<T, N>` has stable slots and a
  live bit; `checked` adds generations. Items of struct type are stored as columns. Collections nest, with a capacity per parent.
- `ref<pool>` references check liveness (and generation); `string<N>` and `T[N]` are bounded values.
- The layout is exact and computed at startup; the program refuses to start over `--budget`. One reservation holds everything;
  pages are committed where bytes are live; trims are scoped and can be driven by a pressure policy (`--soft`, `--hard`).
- Endpoints (`fn`, `endpoint`, `let`/`var`, `each ... where ... reverse ... limit`) are atomic: a failed request is rolled back
  exactly. Requests run in a declared epoch and make no heap allocations once warm.
- Growing past a maximum is a typed failure (503, 413), never a crash or out-of-memory kill.
- `--compile` generates C for endpoints and compiles it in memory with the bundled TinyCC (`third_party/tinycc`, LGPL-2.1,
  loaded at run time); what is not compiled yet stays interpreted, and both modes answer alike.
- `emit-c` writes that C as one readable file for review: the layout table, the helper interface, and each endpoint
  next to its source lines.
- A `.system` file runs several programs as shared-nothing worker threads with bounded mailboxes; `forward kind[key] { ... }`
  hands a request to another worker as part of the handler's transaction. A worker that panics is rebuilt.

```
cargo run --release -- serve examples/chat/chat.mito examples/chat/chat.api --port 8080
cargo run --release -- bench examples/chat/chat.mito examples/chat/chat.api [--soft 16 --hard 32]
cargo run --release -- system examples/chat-sharded/chat.system --port 8080 [--auth 2 --rooms 4]   # worker threads
cargo run --release -- bench-system examples/chat-sharded/chat.system --clients 32
cargo run --release -- emit-c examples/chat/chat.mito examples/chat/chat.api -o chat.c   # or: emit-c x.system -o dir
cargo run --release -- examples/world.mito examples/world.script    # state scripts
cargo run --release -- exp pressure | trim | access | chat           # experiments
```

See [docs/chat-api-findings.md](docs/chat-api-findings.md) for what worked and what did not in the chat API.

Windows only for now. Design: [DESIGN.md](DESIGN.md). Measurements: [docs/measurements.md](docs/measurements.md).

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
