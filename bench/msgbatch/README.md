# Message batch filter: Mitochondria vs Rust vs Go

One thread checks, filters and copies batches of 256 messages already in memory (from a screenshot of a benchmark description; its byte format was not given, so this one is ours, identical in all three programs).

- Message: 8-byte header (magic u16 `0x4D53`, version u8 = 1, category u8, frame u16 = whole message size, len u16 = payload size) + 16..120 payload bytes.
- Per batch: reject 24 messages (8 bad magic, 8 bad version, 8 with `len + 8 != frame`), skip 64 valid ones of category 3 or 5, copy the other 168 (header + payload) into an output area, read the copy back for a checksum (the 32-bit sum of `byte ^ (position & 255)` over the copy, plus the copy's size << 16; a serial hash such as `h * 33 + byte` was tried first and made the checksum 88% of the runtime in every language), reset the output length.
- Data comes from one deterministic generator (LangArena's LCG, seed 42), built before timing. The run checksum is `result = result * 31 + batch_hash` over the warmup (20%) and timed batches; all three must print the same number.
- Mitochondria keeps the input and the output in separate arrays and copies with `output.copy_from(input, from, to, count)` (range checked on both, one plain copy; state is unlogged by default, so nothing is saved for a rollback, and a destination declared `logged` would first save the bytes it overwrites). Integer overflow is always checked. Rust uses safe slices, a `Vec` allocated once and `extend_from_slice`, then `clear()`; Go uses a slice made once, `append`, then `out[:0]`. Preallocating and reusing the buffer is the idiom in all three (the task itself says to reset the output so the next batch can reuse it); a fixed array with `copy_from_slice` / `copy` was also timed and is no faster (Rust 1.14 s vs 1.10 s, Go 2.12 s vs 2.05 s).
- `in_len` is declared `in 0..32768`; with it the compiler proves the walking offset `off` (it only ever grows by a frame size, and `while off < in_len` bounds it) stays inside the arrays, so the header reads carry no range tests. (Until 2026-11-11 the source masked the offset, `off & 32767`, to tell the compiler the same; the analysis of loop-carried variables made that unnecessary.)
- Optimization: Mitochondria is clang `-O3` (plus `-fno-strict-aliasing -fno-math-errno -falign-loops=64`), default x86-64 target; Rust is `opt-level = 3`, fat LTO, one codegen unit, `panic = abort`, default x86-64 target; Go is plain `go build`. With the native CPU on both (`-march=native`, `target-cpu=native`) Mitochondria was 0.645 s and Rust 0.681 s.

## Results (2026-11-11, this machine, one pinned core, 500,000 timed batches after 100,000 warmup, best of 5)

| | time | per batch | vs Rust |
|---|---|---|---|
| Rust | 1.086 s | 2.17 us | 1.00x |
| Mitochondria | 1.015 s | 2.03 us | 0.94x |
| Go | 2.039 s | 4.08 us | 1.88x |

(Mitochondria was 1.00x Rust before the compiler proved more of its checks unnecessary: see ../../docs/measurements.md, Rounds 28-29.)

All three give the same checksum and the same counts (24 rejected, 64 skipped, 168 copied). Filter and copy alone take Rust 0.417 s and Mitochondria 0.345 s; the rest is the checksum loop (about 0.62-0.67 s), which both compilers vectorise to the same width.

## Cold vs warm and allocations (`python -I cold.py`, fresh process, no warmup, median of 7)

- Mitochondria: the first batch costs 21 us, a repeat one-batch request 6-7 us, steady 2.19 us per batch. The 4-5 us between the last two is the request's own fixed cost (the in-process Rust and Go timings have none); the 14 us on the first call is first-touch of pages and code.
- Rust: first batch 9.4 us, then 2.3 us. Go: 5.6 us, then 4.1 us.
- Heap allocations during the batches: 0 in all three (Rust counted with a counting allocator, Go with `runtime.MemStats`, Mitochondria has no allocator: its generated C has no `malloc`, `calloc`, `realloc` or `free`, only OS address-space reservation at start). Rust and Go allocate their two buffers at startup.
- Peak memory: Mitochondria 5.3 MB, Rust 4.3 MB, Go 6.0 MB.

Run: `cargo build --release` in `rust/`, `go build -o msgbatch.exe .` in `go/`, then `python -I run.py --rounds 5` (all three pinned to the same core, alternating rounds).

## Goose

[Goose](https://github.com/aardappel/goose) (a new memory-safe, heap-free systems language that compiles to C) is added as a fourth implementation in `goose/main.goose`. It was written from Goose's docs and has **not been compiled yet** (the compiler could not be run in the session that wrote it): expect syntax fixes. To measure it: install or build a Goose compiler, `goose build goose/main.goose -o goose/msgbatch`, then `run.py` picks it up and checks its checksum like the others.
