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
| Hash::CRC32 | 0.694 | 0.834 | 0.83x | 0.829 | 0.84x | 0.796 | 0.87x | like-for-like |
| Hash::SHA256 | 1.010 | 0.660 | 1.53x | 0.659 | 1.53x | 0.652 | 1.55x | like-for-like |
| Etc::GameOfLife | 1.602 | 1.935 | 0.83x | 1.944 | 0.82x | 1.847 | 0.87x | like-for-like |
| Graph::BFS | 0.989 | 0.833 | 1.19x | 0.849 | 1.16x | 1.221 | 0.81x | rewritten-to-match |
| Graph::DFS | 0.945 | 0.711 | 1.33x | 0.737 | 1.28x | 1.053 | 0.90x | rewritten-to-match |
| Graph::AStar | 0.929 | 1.263 | 0.74x | 1.284 | 0.72x | 2.103 | 0.44x | rewritten-to-match |
| Maze::Generator | 0.515 | 0.740 | 0.70x | 0.735 | 0.70x | 1.442 | 0.36x | rewritten-to-match |
| Maze::BFS | 0.457 | 0.768 | 0.60x | 0.758 | 0.60x | 1.507 | 0.30x | rewritten-to-match |
| Maze::AStar | 0.967 | 1.096 | 0.88x | 1.123 | 0.86x | 4.019 | 0.24x | rewritten-to-match |
| Binarytrees::Obj | 3.191 | 2.926 | 1.09x | 2.910 | 1.10x | 1.489 | 2.14x | like-for-like |
| Binarytrees::Arena | 2.132 | 0.536 | 3.98x | 0.519 | 4.11x | 0.530 | 4.02x | like-for-like |
| Matmul::Single | 4.038 | 3.278 | 1.23x | 3.226 | 1.25x | 3.063 | 1.32x | closest-possible |
| CLBG::Nbody | 0.938 | 0.690 | 1.36x | 0.709 | 1.32x | 0.796 | 1.18x | closest-possible |
| Brainfuck::Array | 1.606 | 0.929 | 1.73x | 1.338 | 1.20x | 2.116 | 0.76x | rewritten-to-match |
| Brainfuck::Recursion | 1.782 | 1.333 | 1.34x | 1.455 | 1.22x | 1.474 | 1.21x | closest-possible |
| Etc::NeuralNet | 0.808 | 0.870 | 0.93x | 0.868 | 0.93x | 0.928 | 0.87x | closest-possible |
| Etc::TextRaytracer | 0.742 | 0.618 | 1.20x | 0.626 | 1.18x | 0.785 | 0.94x | rewritten-to-match |
| Base64::Encode | 2.539 | 0.167 | 15.20x | 0.166 | 15.30x | 1.115 | 2.28x | closest-possible |
| Base64::Decode | 1.623 | 0.184 | 8.82x | 0.184 | 8.82x | 1.132 | 1.43x | closest-possible |
| Calculator::Ast | 0.229 | 0.665 | 0.34x | 0.670 | 0.34x | 0.402 | 0.57x | closest-possible |
| Compress::BWTEncode | 0.570 | 0.423 | 1.35x | 0.425 | 1.34x | 1.115 | 0.51x | closest-possible |
| Compress::BWTDecode | 0.881 | 1.056 | 0.83x | 1.077 | 0.82x | 0.913 | 0.97x | rewritten-to-match |
| Compress::HuffEncode | 0.651 | 0.598 | 1.09x | 0.604 | 1.08x | 0.918 | 0.71x | rewritten-to-match |
| Compress::HuffDecode | 1.294 | 0.623 | 2.08x | 0.620 | 2.09x | 0.906 | 1.43x | rewritten-to-match |
| Compress::ArithEncode | 1.020 | 0.887 | 1.15x | 0.952 | 1.07x | 1.101 | 0.93x | like-for-like |
| Compress::ArithDecode | 0.970 | 1.226 | 0.79x | 1.203 | 0.81x | 0.899 | 1.08x | rewritten-to-match |
| Compress::LZWEncode | 0.090 | 0.521 | 0.17x | 0.514 | 0.18x | 0.826 | 0.11x | closest-possible |
| Compress::LZWDecode | 2.116 | 1.188 | 1.78x | 1.162 | 1.82x | 0.523 | 4.05x | rewritten-to-match |
| Etc::CacheSimulation | 0.482 | 0.813 | 0.59x | 0.818 | 0.59x | 1.835 | 0.26x | rewritten-to-match |
| Etc::Words | 0.597 | 0.772 | 0.77x | 0.783 | 0.76x | 0.799 | 0.75x | closest-possible |
| Distance::Jaro | 0.994 | 1.063 | 0.94x | 0.986 | 1.01x | 1.145 | 0.87x | rewritten-to-match |
| Distance::NGram | 0.437 | 0.347 | 1.26x | 0.361 | 1.21x | 0.642 | 0.68x | rewritten-to-match |
| Template::Parse | 0.772 | 0.826 | 0.93x | 0.825 | 0.94x | 1.841 | 0.42x | closest-possible |
| Calculator::Interpreter | 0.551 | 0.874 | 0.63x | 0.874 | 0.63x | 0.476 | 1.16x | closest-possible |

Geometric mean of the 17 like-for-like and rewritten-to-match ports: 1.11x of Rust and 0.87x of Crystal (Matmul::Single and Nbody are closest-possible and left out). Medians agree with best-of within a few percent. Rows for the later ports (from Hash::CRC32 down) were measured in the same way, 3 rounds, after the store speed-ups. Crystal is
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
| 1 | Binarytrees::Obj | ported |
| 2 | Binarytrees::Arena | ported |
| 3 | Brainfuck::Array | ported |
| 4 | Brainfuck::Recursion | ported |
| 5 | Matmul::Single | ported |
| 6-8 | Matmul::T4 / T8 / T16 | not comparable: the reference runs threads on a shared matrix; Mitochondria workers share nothing |
| 9 | Base64::Encode | ported (the reference calls the base64-simd crate: a library comparison, kept out of the geometric mean) |
| 10 | Base64::Decode | ported (same: the reference calls base64-simd) |
| 11-13 | Json::Generate / ParseDom / ParseMapping | not comparable: the reference uses a JSON library |
| 14 | CSV::Parse | blocked: strings, float parsing and formatting |
| 15 | Etc::Sieve | ported |
| 16 | Etc::TextRaytracer | ported |
| 17 | Etc::NeuralNet | ported |
| 18 | Etc::CacheSimulation | ported (lib/strmap hash map; needed the text area freed per loop iteration) |
| 19 | Etc::GameOfLife | ported |
| 20 | Etc::Words | ported (text generated by the harness; `substr` builtin) |
| 21 | Etc::LogParser | not comparable: regular expressions |
| 22 | Template::Regex | not comparable: regular expressions |
| 23 | Template::Parse | ported (`text_from` builtin, lib/strmap) |
| 24 | Sort::Quick | ported |
| 25 | Sort::Merge | ported |
| 26 | Sort::Self | not comparable: the reference calls its library sort |
| 27-29 | Graph::BFS / DFS / AStar | ported |
| 30 | Hash::SHA256 | ported |
| 31 | Hash::CRC32 | ported |
| 32 | Calculator::Ast | ported (node arrays with a bump allocator against one heap allocation per node: closest-possible) |
| 33 | Calculator::Interpreter | ported (`wrapping_add/sub/mul` builtins, lib/strmap) |
| 34-36 | Maze::Generator / BFS / AStar | ported |
| 37 | CLBG::Fannkuchredux | ported |
| 38 | CLBG::Mandelbrot | ported (bit packing as arithmetic) |
| 39 | CLBG::Nbody | ported |
| 40 | CLBG::Spectralnorm | ported (the one sqrt is taken by the harness) |
| 41-42 | Compress::BWTEncode / BWTDecode | ported (encode's merge sort skips ordered merges: first version without that was 4.98x of Rust, because the reference's library sort detects ordered runs) |
| 43-44 | Compress::HuffEncode / HuffDecode | ported |
| 45-46 | Compress::ArithEncode / ArithDecode | ported |
| 47-48 | Compress::LZWEncode / LZWDecode | ported (encode keys its dictionary by (prefix code, byte) instead of the whole string, which favours Mitochondria: excluded from the geometric mean) |
| 49-50 | Distance::Jaro / NGram | ported (NGram uses the new lib/intmap hash map) |

Counts now: 40 ported (the 7 in the first batch, both Brainfuck tests, NeuralNet, TextRaytracer, then Nbody, CRC32, SHA256, GameOfLife, the three graph tests, the three maze tests, and both Binarytrees), 9 not comparable (the three threaded
matmuls, the three JSON tests, LogParser, Template::Regex, Sort::Self) and 1 still to do or blocked. Passing a string literal to a function (`put("text")`, reading its bytes) and mutually recursive functions already work, which is what made the text generator and the parser expressible. Language features added along the way (each tested, all merged): bit operators and hex literals
(`& | ^ << >>`, Rust's precedence), `sqrt` `exp` `floor` `abs`, reading a string's bytes by index (`s[i]`; strings stay read-only), `coll.clear()`, `array.fill`, `array.move` and `array.count_below`.
Still blocked by something else: building text (Base64 output, the parsers' string results), hash maps (CacheSimulation, Words, Template::Parse, Distance, Calculator::Interpreter, parts of Compress) and CSV.
Where the time goes in what is slower than the reference: Binarytrees::Arena (3.98x of Rust) is pool `add` at about 28 ns against a `Vec` push at a few (75 million adds); the sieve, Fannkuchredux and
the merge sort are loops of many small array stores, which pay for atomic requests.

Finding while porting LZWDecode: using `array.move` to copy each decoded string into a 1 MB array (instead of a byte loop) made a run about 200x slower (0.2 s against 1 ms); `array.move` between small ranges is fine. Not investigated yet, probably the undo log handling of the destination range.

Language and library added for the string tests: `lib/strmap.mito` (hash map from string keys to i64, chained, entries numbered so a caller can keep its own data per entry, slots reused at once) and a text-area rule: strings built in a loop body are dropped at the end of each iteration unless the body assigns one to a variable declared before the loop (the 256 KB text area of a request otherwise fills after about 20,000 string keys; CacheSimulation builds 5.8 million).

Also added: `substr(text, from, count)` (a piece of a string sharing its bytes, range checked) and a 4 MB text area (was 256 KB). Found and fixed while porting Words: a `let` of a `var` (`let start = at;` and then `at = at + 1`) shared the var's C variable, so the `let` changed with it (numbers, floats, strings, references); a `let` now copies a `var`'s value (tests/cases/letcopy.case). No earlier port was affected (their checksums still verify).

`lib/intmap.mito` (hash map from i64 keys to i64 values, same design as strmap) was added for NGram.

`text_from(bytes, from, count)` (a state array of u8 as a string, copied into the text area, range checked) was added for Template::Parse.

`wrapping_add`, `wrapping_sub` and `wrapping_mul` (two's-complement arithmetic; the ordinary operators fail the request on overflow) were added for Calculator::Interpreter.
