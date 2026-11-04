# Compiling endpoints to native code

## Decision (2026-11-04)
Endpoints are generated as **C** (`src/cgen.rs`). The default compiler is the **bundled TinyCC** library
(`third_party/tinycc`, loaded at run time), which compiles in memory in milliseconds and keeps live reload. **clang** is
the planned option for release builds (same C, built `-O2` into a DLL); it is not built yet because clang is not
installed. TinyCC 0.9.27's library crashes while reporting a compile error to an MSVC-built host, so generated C is first
checked with the bundled `tcc.exe` in a child process. Results are in docs/measurements.md, Round 7. The comparison
below is how the choice was made.

## The C safety profile (enforced)
Generated endpoint bodies use a strict subset of C, built from a few macros in the prelude, and `cgen::check_profile`
checks every function before it is compiled; a body that breaks a rule is not compiled (the note names the rule, and
starting the program fails). The rules, and the kind of bug each one rules out:

| Rule | Rules out |
|---|---|
| Memory is read only through `LD_*(offset, index)` from items reached by live iteration, a checked index or a followed reference; no other dereference, no indexing except constant-size argument arrays | reads outside the declared layout or of uncommitted pages |
| Nothing writes memory except helpers; assignments only to locals or their fields, and a generated function's result through its `ret_v`/`ret_s` parameters | writes that skip the journal (rollback) |
| Loads use `memcpy` under clang (one instruction) and plain loads under TinyCC | strict-aliasing undefined behaviour when reading bytes the runtime wrote |
| Integer arithmetic on language values only via `ADD`, `SUB`, `MUL`, `DIV`, `MOD`, `NEG`, which check before the operation | signed overflow and division by zero (undefined in C) |
| Every helper call wrapped in `CALL` (error checked), `TAIL`, `FAIL` or `DEREF`; `c->h->helper` is the only member access; the only other calls are generated functions (`fn_*`, for recursive functions), through `CALL`, each after a check that calls nest at most 64 deep; no libc | ignored errors, unwinding through C, unbounded stack |
| `if` and bounded `for` only (no `while`, `do`, `switch`, empty-condition `for`); `goto` only forward; `return` only of a status | non-terminating or tangled control flow |
| No preprocessor, `static`, `extern`, `sizeof`, `asm`, `volatile`, unions | globals (reentrancy across workers), hidden effects |

The checker is token-based: it trusts the generator for the types of index arithmetic (unsigned, bounded), which it
cannot see. Planned with the clang path: build with `-Wall -Werror -fno-strict-aliasing`, and run the differential test
under `-fsanitize=undefined,address`. The profile costs nothing: under TinyCC the macros expand to the code generated
before (GET messages 5.1 us, POST message 0.9 us).

## Options considered

Measured starting point (`exp json`, GET of a room's newest 50 messages, 8.7 KB of JSON): the interpreter takes 14.7 us;
native Rust writing the same response from the same state takes 4.2 us. The 10.5 us between them is interpretation:
about 250 tree-node evaluations, value building and matching, reference following, copies for `or(...)`.

## What any compiler here has to produce
Whatever the target, generated code for an endpoint does the same things, and most of the runtime stays in Rust:

- **Constants from the layout.** Every column's address is `base + column offset + item * size`, and all of those are known
  when the program is built. Generated code reads `m.seq` as one load, no lookup.
- **Calls into runtime helpers** (Rust functions exported with the C calling convention) for everything that is not a
  plain read: journaled writes (needed for rollback), `push_item`/`free_item`, `deref` with generation checks, string
  stores with length checks, JSON escaping, request fields, `auth`, `token`.
- **The same checks the interpreter makes:** liveness, bounds, declared maxima, integer overflow; a failure returns a
  status code and the runtime rolls the request back as today.
- **The same epoch:** response and text areas are passed in; nothing allocates.

A sketch of what the generated code for `GET /rooms/{room}/messages` would look like, as C:

```c
// generated from examples/chat/chat.api; offsets come from the declared layout
int ep_get_messages(Mito *m, Req *q) {
    u32 who; if (!mito_auth(m, q, COLL_SESSIONS, &who)) return q->status;          // 401/403 already written
    i64 rid;  if (!mito_param_int(q, PARAM_ROOM, &rid)) return 400;
    u32 room; if (!mito_pool_live(m, COLL_ROOMS, 0, rid, &room)) return mito_fail(q, 404, "slot is not live");
    i64 n = mito_min(mito_query_int_or(q, "limit", 50), 100);
    mito_out(q, "{\"room\":"); mito_json_str(q, MITO_STR(m, ROOM_NAME, room));
    mito_out(q, ",\"messages\":[");
    u32 len = MITO_LIST_LEN(m, ROOM_MESSAGES, room), head = MITO_LIST_HEAD(m, ROOM_MESSAGES, room);
    for (u32 k = 0; k < len && k < n; k++) {
        u32 i = room * 1000 + (head + len - 1 - k) % 1000;                        // reverse: newest first
        if (k) mito_out(q, ",");
        mito_out(q, "{\"seq\":"); mito_json_i64(q, MITO_I64(m, MSG_SEQ, i));
        mito_out(q, ",\"author\":");
        u32 s; if (mito_deref(m, COLL_SESSIONS, MITO_REF(m, MSG_AUTHOR, i), &s)) mito_json_str(q, MITO_STR(m, SESSION_NAME, s));
               else mito_json_lit(q, "[deleted]");
        mito_out(q, ",\"at\":");   mito_json_i64(q, MITO_I64(m, MSG_AT, i));
        mito_out(q, ",\"body\":"); mito_json_str(q, MITO_STR(m, MSG_BODY, i));
        mito_out(q, "}");
    }
    mito_out(q, "]}");
    return 200;
}
```

The same shape applies to every target below; only who turns it into machine code differs.

## The options

| | Compile time per app | Generated code | Live reload | Needs installed | Licence | Effort | GET messages, expected* |
|---|---|---|---|---|---|---|---|
| **Closures** (Rust, no machine code) | ~0 | interpreter without the tree walk | yes | nothing | ours | small | ~8-10 us |
| **Cranelift JIT** | ~1-5 ms | good (fast "-O1-ish") | yes, in process | nothing (crates already cached) | Apache-2.0 | high | ~5-6 us |
| **C + TinyCC (libtcc)** | ~1 ms | unoptimised C (weak registers) | yes, in process | libtcc (download or build) | LGPL-2.1 | medium | ~5-8 us |
| **C + MIR** (c2m / MIR JIT) | ~10 ms | optimised, near "gcc -O1" | yes, in process | MIR library | MIT | medium | ~5-6 us |
| **C + clang/gcc/MSVC -O2**, load as DLL | ~0.1-0.5 s | best (full LLVM/GCC optimiser) | yes, by DLL swap | a C toolchain | free | medium | ~4-5 us |
| **LLVM directly** (inkwell / llvm-sys) | ~10-100 ms | best | yes (ORC JIT) | LLVM (~100+ MB, version-pinned) | Apache-2.0 + LLVM exception | high | ~4-5 us |
| **Generated Rust**, built by cargo | ~5-30 s | best (rustc, LLVM -O3) | no (rebuild) | nothing new (rustc is here) | ours | medium | ~4-5 us |
| WebAssembly (wasmtime) | ~5-50 ms | good, sandboxed | yes | wasmtime (large) | Apache-2.0 | high | slower: state is outside wasm memory |
| Own x86-64 emitter / copy-and-patch | ~0.1 ms | fair to good | yes | nothing | ours | very high, per CPU | ~5-7 us |

\*Expected numbers are estimates, not measurements. The floor is 4.2 us because most of that time is in the Rust
helpers (escaping 8 KB, copying strings), which every option shares. So the gap between the best and worst
native options here is small; the gap to the interpreter is not.

### Notes per option
- **Closures:** compile each expression to a Rust closure with everything resolved. No new dependency, portable, safe
  Rust. Removes dispatch on node kinds but keeps dynamic `Value`s. A sensible stepping stone, not the end state.
- **Cranelift:** what StasisLang uses for its live code swaps. Pure Rust, fast enough to recompile on every edit, targets
  x86-64, AArch64, RISC-V, s390x on Windows, Linux and macOS. Writing it means building SSA IR by hand for each construct;
  generated-code bugs are harder to read than C. Adds the project's first dependencies (about a dozen crates).
- **C + TinyCC:** emit C text, hand it to libtcc in memory, get function pointers back in about a millisecond. The output
  is readable and easy to debug (you can open the C). TinyCC does almost no optimisation, but here the hot work is in
  helpers compiled by rustc, so that matters less than usual. It needs libtcc.dll (no C compiler is on this machine to
  build it, so a prebuilt Windows release would be downloaded). LGPL: fine to link dynamically from an Apache-2.0 app;
  static linking would bring LGPL obligations. Generated C is memory-unsafe: every check must be emitted, and a codegen
  bug can corrupt state instead of panicking.
- **C + MIR:** Vladimir Makarov's MIR has a C front end and an optimising JIT, MIT-licensed, small. Better code than TinyCC
  at somewhat slower compile. Less widely used; Windows support exists but is the least tested of the C options.
- **C + clang/gcc/MSVC:** the same generated C, compiled ahead of time with full optimisation into a DLL loaded with
  `LoadLibrary`/`dlopen`. Reload in under a second is acceptable for development. Needs a C toolchain installed.
- **One generator, two compilers:** because TinyCC (or MIR) and clang accept the same C, the generator can be written
  once: TinyCC for instant reload while editing, clang -O2 for release builds. That is the C route's main attraction.
- **LLVM directly:** the best code and a mature JIT, but a large, version-pinned install that is awkward on Windows, and
  slow compiles. Its quality is reachable more cheaply through clang on generated C.
- **Generated Rust:** the simplest generator to write and debug, and safe Rust for most of the output; but a cargo build
  per change (seconds to tens of seconds) ends live reload, and the crate would need splitting into a runtime library
  plus a generated app crate.
- **WebAssembly:** gives sandboxing, but the declared state lives in host memory, so every column read becomes a host call
  or a copy. A poor fit for this design.
- **Own emitter / copy-and-patch:** no dependencies and very fast compiles, but writing and maintaining it per CPU is the
  largest effort of all, for code no better than Cranelift's.

## Recommendation
Two realistic paths:

1. **Cranelift**, if staying pure Rust with nothing to install matters most. It matches StasisLang, works offline here,
   and keeps live reload. Cost: the hardest generator to write.
2. **Generated C, with TinyCC for development and clang for release**, if readable output and one generator for two
   quality levels matter most. Cost: installing libtcc and a C toolchain, LGPL for TinyCC, and memory-unsafe generated code.

Either way the first step is the same and is useful on its own: a lowering pass that turns a bound endpoint into a small,
explicit form (resolved columns, constant offsets, helper calls, explicit checks), which both generators and the
interpreter can share.
