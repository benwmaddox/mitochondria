# Compiling to native code

## Decision (2026-11-04)
A program is generated as **C** (`src/cgen.rs`): its endpoints, with the C runtime, the worker system and the HTTP
server, as one C program. **TinyCC** (bundled, `third_party/tinycc`) builds it in 20-30 ms for development; **clang**
-O2 builds it in about 1 s for release, with requests about 4x faster. Both are tested on every case, and so is a clang
build with undefined-behavior checks.

Options weighed (2026-11-04): closures over the interpreter, Cranelift, C with TinyCC, MIR or clang, LLVM directly,
generated Rust, WebAssembly, an own code emitter. C won because one readable generator serves both a fast-compiling and
a fully optimized build, and the output can be reviewed (`emit-c`). The cost is memory-unsafe generated code, which the
profile below confines.

## The C safety profile (enforced)
Generated endpoint bodies use a strict subset of C, built from a few macros in the prelude, and `cgen::check_profile`
checks every function before it is compiled; a body that breaks a rule does not compile, and the program does not load.

| Rule | Rules out |
|---|---|
| Memory is read only through `LD_*(offset, index)` from items reached by live iteration, a checked index or a followed reference; no other dereference, no indexing except constant-size argument arrays | reads outside the declared layout or of uncommitted pages |
| Nothing writes memory except helpers; assignments only to locals or their fields, and a generated function's result through its `ret_v`/`ret_s` parameters | writes that skip the journal (rollback) |
| Loads use `memcpy` under clang (one instruction) and plain loads under TinyCC | strict-aliasing undefined behavior when reading bytes the runtime wrote |
| Integer arithmetic on language values only via `ADD`, `SUB`, `MUL`, `DIV`, `MOD`, `NEG`, which check before the operation | signed overflow and division by zero (undefined in C) |
| Every helper call wrapped in `CALL` (error checked), `TAIL`, `FAIL` or `DEREF`; `c->h->helper` is the only member access; the only other calls are generated functions (`fn_*`, for recursive functions), through `CALL`, each after a check that calls nest at most 64 deep; no libc | ignored errors, unwinding through C, unbounded stack |
| `if` and bounded `for` only (no `while`, `do`, `switch`, empty-condition `for`); `goto` only forward; `return` only of a status | non-terminating or tangled control flow |
| No preprocessor, `static`, `extern`, `sizeof`, `asm`, `volatile`, unions | globals (reentrancy across workers), hidden effects |

The checker is token-based: it trusts the generator for the types of index arithmetic (unsigned, bounded), which it
cannot see. The clang build with `-fsanitize=undefined,bounds` (trap mode) runs every case as a further check.
