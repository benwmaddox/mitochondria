# LangArena ports for Mitochondria

Mitochondria ports of [LangArena](https://github.com/kostya/LangArena) tests (MIT, see `LICENSE-LangArena`), measured against LangArena's own Rust implementation on the same machine. `run.js` and `test.js` are LangArena's parameter files, vendored unchanged; each entry's `checksum` is what both sides must reproduce.

Run: build the Rust reference once (`CARGO_TARGET_DIR=bench/langarena/target-rust cargo build --profile max-perf --manifest-path <LangArena>/rust/Cargo.toml`, and the same with `--config 'profile.max-perf.overflow-checks=true'` into `target-rust-checked`), then `python -I bench/langarena/run.py [--mode run|test] [--rounds N] [test_dir ...]`. `LANGARENA` names the clone (default `D:/tmp/langarena`).

## How the comparison is made

- LangArena times only `run_all()`, after `prepare()` and the warmup iterations (max(0.2 * iterations, 1) unless the entry says `warmup_iterations`); its checksum covers the warmup runs too, and its random generator is reset after warmup. A port is a program with `POST /prepare/...`, `POST /warmup/{n}`, `POST /run/{n}` and `GET /checksum`; prepare and warmup are untimed requests whose state persists, and only `/run` is timed.
- Mitochondria is timed by the server itself (`--timing`, header `X-Handler-Ns`), the Rust reference by its own in-process timer: both measure the iterations and nothing else.
- Both processes are pinned to the same core (`bench/ordered/pin.py`; this CPU mixes fast and slow cores), rounds alternate, best of N and the median are reported. Mitochondria is built by `mitochondria build` (clang -O3); the Rust reference with its own `max-perf` profile (opt-level 3, fat LTO, one codegen unit, **overflow checks off**).
- Mitochondria always checks integer overflow, so the table has a second Rust column: the same reference built with overflow checks on.
- The checksum is compared with `run.js` on both sides in every measured run (`verified`). A checksum that hashes a formatted float is verified by the harness, which formats and hashes the answered float exactly as LangArena's helper does (`{:.7}`, then hash = hash * 33 + byte).
- Each port has an `audit` file with its verdict: `like-for-like`, `rewritten-to-match` (same algorithm, small mechanical rewrites the language forces, listed) or `closest-possible` (a difference in data-structure shape, listed). The headline geometric mean covers only the first two.
- "Original" in LangArena means the Crystal implementation. Crystal is not installed here; its sources are in `<LangArena>/crystal` and the ports follow the same algorithms (the Rust implementation is a faithful port of them).

## Results (2026-11-04; run parameters, 3 rounds, best of rounds; ratio = Mitochondria / Rust)

| Workload | Mitochondria s | Rust (reference) s | Mito/Rust | Rust with overflow checks s | Mito/Rust (checked) | audit |
|---|---|---|---|---|---|---|
| Etc::Sieve | 6.639 | 1.346 | 4.93x | 1.797 | 3.69x | rewritten-to-match |
| CLBG::Fannkuchredux | 2.566 | 0.902 | 2.84x | 0.922 | 2.78x | like-for-like |
| Sort::Quick | 1.796 | 1.113 | 1.61x | 1.152 | 1.56x | rewritten-to-match |
| Sort::Merge | 3.284 | 1.294 | 2.54x | 1.284 | 2.56x | rewritten-to-match |
| CLBG::Spectralnorm | 0.749 | 1.066 | 0.70x | 1.080 | 0.69x | rewritten-to-match |
| CLBG::Mandelbrot | 0.703 | 0.706 | 1.00x | 0.714 | 0.98x | rewritten-to-match |
| Matmul::Single | 4.056 | 3.249 | 1.25x | 3.238 | 1.25x | closest-possible |

Headline geometric mean (the six like-for-like and rewritten-to-match ports): 1.85x. With Matmul::Single: 1.75x. Medians agree with best-of within 2%.

What the numbers say: a loop dominated by float arithmetic or reads is at parity or faster (Mandelbrot, Spectralnorm); a loop of many small array *stores* is several times slower (Sieve 4.9x, Fannkuchredux 2.8x, Merge 2.5x), because every store in a Mitochondria request is range-checked, committed and undo-logged so that a failed request leaves nothing behind. The checked-overflow Rust column shows that overflow checking alone is not the explanation (Sieve: 1.35 s -> 1.80 s).

## Triage of all 50 tests

Ported = verified here. Portable = expressible with the language as it is, not yet ported. Blocked = needs a language feature that does not exist (the feature is named). Not comparable = the reference's work is a library or threads.

| # | Test | Status |
|---|---|---|
| 1 | Binarytrees::Obj | blocked: no way to free or reset a whole pool in one step (about a million frees per iteration; a request may free 65,536) |
| 2 | Binarytrees::Arena | blocked: pool reset |
| 3 | Brainfuck::Array | blocked: bytes of a string (the program text) |
| 4 | Brainfuck::Recursion | blocked: bytes of a string; recursion depth limit of 64 |
| 5 | Matmul::Single | ported |
| 6-8 | Matmul::T4 / T8 / T16 | not comparable: the reference runs threads on a shared matrix; Mitochondria workers share nothing |
| 9 | Base64::Encode | blocked: bytes of a string, bit operators |
| 10 | Base64::Decode | blocked: bytes of a string, bit operators |
| 11-13 | Json::Generate / ParseDom / ParseMapping | not comparable: the reference uses a JSON library |
| 14 | CSV::Parse | blocked: strings, float parsing and formatting |
| 15 | Etc::Sieve | ported |
| 16 | Etc::TextRaytracer | blocked: sqrt |
| 17 | Etc::NeuralNet | blocked: exp / tanh |
| 18 | Etc::CacheSimulation | blocked: hash map with string keys |
| 19 | Etc::GameOfLife | portable: its hash has one xor with a 0/1 value (arithmetic parity) |
| 20 | Etc::Words | blocked: strings, hash map |
| 21 | Etc::LogParser | not comparable: regular expressions |
| 22 | Template::Regex | not comparable: regular expressions |
| 23 | Template::Parse | blocked: strings, hash map |
| 24 | Sort::Quick | ported |
| 25 | Sort::Merge | ported |
| 26 | Sort::Self | not comparable: the reference calls its library sort |
| 27-29 | Graph::BFS / DFS / AStar | portable: per-vertex neighbour lists, a queue / stack / heap in arrays |
| 30 | Hash::SHA256 | blocked: bit operators, bytes of a string |
| 31 | Hash::CRC32 | blocked: bit operators |
| 32 | Calculator::Ast | blocked: bytes of a string; node trees |
| 33 | Calculator::Interpreter | blocked: bytes of a string, hash map |
| 34-36 | Maze::Generator / BFS / AStar | portable: one xor in the hash (a small value) and abs, both arithmetic |
| 37 | CLBG::Fannkuchredux | ported |
| 38 | CLBG::Mandelbrot | ported (bit packing as arithmetic) |
| 39 | CLBG::Nbody | blocked: sqrt in the inner loop |
| 40 | CLBG::Spectralnorm | ported (the one sqrt is taken by the harness) |
| 41-48 | Compress::BWT / Huff / Arith / LZW (encode and decode) | blocked: bit operators, hash maps, byte buffers |
| 49-50 | Distance::Jaro / NGram | blocked: strings, hash map |

Counts: 7 ported, 7 more portable now (GameOfLife, the three graph tests, the three maze tests), 9 not comparable (the three threaded matmuls, the three JSON tests, LogParser, Template::Regex, Sort::Self) and 27 blocked by a missing language feature. The features that would unlock the most tests: bit operators (Base64 x2, SHA256, CRC32, the eight Compress tests), reading the bytes of a string (Brainfuck x2, Base64 x2, Calculator x2, CSV, Words, Template::Parse, Distance x2, CacheSimulation, the Compress tests), `sqrt` and `exp` (Nbody, TextRaytracer, NeuralNet) and a one-step reset of a pool (Binarytrees x2). Adding any of them is a language decision.
