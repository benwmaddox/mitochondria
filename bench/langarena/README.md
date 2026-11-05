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

## Results (2026-11-04; run parameters, 3 rounds, best of rounds; ratio = Mitochondria / the column's implementation)

| Workload | Mitochondria s | Rust (reference) s | Mito/Rust | Rust with overflow checks s | Mito/Rust (checked) | Crystal (original) s | Mito/Crystal | audit |
|---|---|---|---|---|---|---|---|---|
| Etc::Sieve | 2.252 | 1.326 | 1.70x | 1.807 | 1.25x | 2.000 | 1.13x | rewritten-to-match |
| CLBG::Fannkuchredux | 1.368 | 0.933 | 1.47x | 0.942 | 1.45x | 1.011 | 1.35x | like-for-like |
| Sort::Quick | 1.351 | 1.106 | 1.22x | 1.148 | 1.18x | 1.558 | 0.87x | rewritten-to-match |
| Sort::Merge | 1.806 | 1.311 | 1.38x | 1.311 | 1.38x | 2.173 | 0.83x | rewritten-to-match |
| CLBG::Spectralnorm | 0.746 | 1.068 | 0.70x | 1.075 | 0.69x | 0.743 | 1.00x | rewritten-to-match |
| CLBG::Mandelbrot | 0.692 | 0.709 | 0.98x | 0.716 | 0.97x | 0.687 | 1.01x | rewritten-to-match |
| Matmul::Single | 4.038 | 3.278 | 1.23x | 3.226 | 1.25x | 3.063 | 1.32x | closest-possible |

Geometric mean of the six like-for-like and rewritten-to-match ports: 1.19x of Rust (1.02x of Crystal). Medians agree with best-of within 2%. Crystal is
LangArena's original implementation, 1.22.2 built with `crystal build main.cr --release` from a patched copy (the one JSON-library call, in a test that is not
comparable, replaced by the standard library's), run with the same runner on the same core.

How it got here (Mitochondria / Rust, best of 3; each step merged): the first run had Sieve 4.93x, Fannkuchredux 2.84x, Sort::Quick 1.61x, Sort::Merge 2.54x. Stores to plain
state skip the page-commit check (state is committed at start): 1.97x, 1.90x, 1.30x, 1.71x. A bulk `array.fill` for the sieve's per-run array. Stores of aligned scalars inline, with
the undo-log and commit tests against per-request locals: 1.66x, 1.52x, 1.21x, 1.44x. Value-range analysis for names that never change: no measurable change on these ports
(their indices are mutable variables, or sizes read from state).

Where the time was, for the sieve, per run (measuring-only builds with parts switched off): 24.7 ms as built; 19.6 ms without the undo log; 13.3 ms without the commit check; 7.8 ms
without both; Rust 5.0 ms. A loop of many small array stores pays for atomicity (every store is range-checked, committed and undo-logged so that a failed request leaves nothing behind).

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

Counts when the table was made: 7 ported, 7 portable, 9 not comparable, 27 blocked. Language features added since for this suite (each tested, all merged):
bit operators and hex literals (`& | ^ << >>`, Rust's precedence), `sqrt` `exp` `floor` `abs`, reading a string's bytes by index (`s[i]`; strings stay read-only),
`coll.clear()`, `array.fill`, `array.move` and `array.count_below`. They remove the stated blocker of: Binarytrees::Obj and ::Arena (pool reset), Brainfuck::Array and
::Recursion (string bytes; recursion is still limited to 64 deep), Etc::NeuralNet (exp), CLBG::Nbody (sqrt), Etc::TextRaytracer (sqrt, floor), Hash::CRC32 and Hash::SHA256 (bit
operators, string bytes), and the bit-operator part of Base64, the compression tests and Calculator. Not yet ported. Still blocked by something else: building text (Base64
output, the parsers' string results), hash maps (CacheSimulation, Words, Template::Parse, Distance, Calculator::Interpreter, parts of Compress) and CSV.
