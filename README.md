# Mitochondria

Codename for an experimental language whose memory is a **declared tree of fixed-size state**, all reserved
up front, with the runtime tracking what is actually in use.

```
struct Msg     { seq: i64; author: ref<chat.sessions>; body: string<2000>; }
struct Room    { name: string<48>; messages: list<Msg, 1000> drop_oldest; }
struct Session { name: string<32>; secret: i64; }
struct Chat    { next_seq: i64; sessions: pool<Session, 2000> checked; rooms: pool<Room, 64> checked; }
state chat: Chat;

@post("/rooms/{room}/messages")
fn post_message(who: Session from header("X-Session"), room: Room, body: string<1..2000>) -> Response<{ seq: i64 }> {
    chat.next_seq += 1;
    let m = room.messages.add { seq: chat.next_seq, author: who, body: body };
    return created({ seq: m.seq });
}
```

- `list<T, N>` is ordered, with a declared full policy (`fail`, or `drop_oldest` for a ring). `pool<T, N>` has stable slots and a
  live bit; `checked` adds generations. Items of struct type are stored as columns. Collections nest, with a capacity per parent.
- A program has one state, a struct holding everything. `ref<game.players>` references check liveness (and
  generation); `string<N>` and `T[N]` are bounded values.
- The layout is exact and computed when the program is built; the program refuses to start over `--budget`. One
  reservation holds everything; pages are committed where bytes are live; trims are scoped and can be driven by a
  pressure policy (`--soft`, `--hard`).
- Everything is typed and every kind is known when compiling. Endpoints are functions with an attribute
  (`@post("/path")`); their parameters are typed inputs checked before the body runs, and their answers are a typed
  `Response<T>`. Each request is atomic: a failed request is rolled back exactly. The runtime has no heap allocator.
- Growing past a maximum is a typed failure (503, 413), never a crash or out-of-memory kill.
- A program compiles to one C program: its endpoints, the C runtime (`runtime/mito.c`) and an HTTP server
  (`runtime/http.c`). `build` makes an executable with clang -O2 (release), or with the bundled TinyCC in milliseconds
  (`--tcc`, development; `third_party/tinycc`, LGPL-2.1). Rust is only the compiler.
- `emit-c` writes that C for review: the layout table, the runtime, and each endpoint next to its source lines.
- A system file (`system { ... }`) runs several programs as shared-nothing worker threads with bounded mailboxes (`runtime/system.c`);
  `forward kind[key] { ... }` hands a request to another worker as part of the handler's transaction.

```
cargo run --release -- check examples/chat/chat.mito
cargo run --release -- serve examples/chat/chat.mito --port 8080           # TinyCC build, run at once
cargo run --release -- build examples/chat/chat.mito -o chat.exe           # clang -O2
cargo run --release -- serve examples/chat-sharded/chat.mito --port 8080   # worker threads
cargo run --release -- emit-c examples/chat/chat.mito -o chat.c            # a system: -o dir
```

New here? Start with the [tutorial](docs/tutorial.md).

See [docs/chat-api-findings.md](docs/chat-api-findings.md) for what worked and what did not in the chat API.

Windows only for now. Design: [DESIGN.md](DESIGN.md). Measurements: [docs/measurements.md](docs/measurements.md).

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
