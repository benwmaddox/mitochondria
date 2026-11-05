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

## Results (2026-11-04; run parameters, 3 rounds, best of rounds, all 41 ports in one run; ratio = Mitochondria / the column's implementation)

| Workload | Mitochondria s | Rust (reference) s | Mito/Rust | Rust with overflow checks s | Mito/Rust (checked) | Crystal (original) s | Mito/Crystal | audit |
|---|---|---|---|---|---|---|---|---|
| Base64::Decode | 1.610 | 0.184 | 8.75x | 0.183 | 8.80x | 1.134 | 1.42x | closest-possible |
| Base64::Encode | 2.534 | 0.165 | 15.35x | 0.167 | 15.17x | 1.101 | 2.30x | closest-possible |
| Binarytrees::Arena | 2.124 | 0.565 | 3.76x | 0.566 | 3.75x | 0.556 | 3.82x | like-for-like |
| Binarytrees::Obj | 3.213 | 2.952 | 1.09x | 2.902 | 1.11x | 1.478 | 2.17x | like-for-like |
| Brainfuck::Array | 1.501 | 0.915 | 1.64x | 1.337 | 1.12x | 2.104 | 0.71x | rewritten-to-match |
| Brainfuck::Recursion | 2.203 | 1.340 | 1.64x | 1.453 | 1.52x | 1.468 | 1.50x | closest-possible |
| Calculator::Ast | 0.235 | 0.667 | 0.35x | 0.660 | 0.36x | 0.395 | 0.60x | closest-possible |
| Calculator::Interpreter | 0.530 | 0.883 | 0.60x | 0.893 | 0.59x | 0.497 | 1.07x | closest-possible |
| CLBG::Fannkuchredux | 1.371 | 0.920 | 1.49x | 0.932 | 1.47x | 1.005 | 1.36x | like-for-like |
| CLBG::Mandelbrot | 0.688 | 0.721 | 0.95x | 0.717 | 0.96x | 0.708 | 0.97x | rewritten-to-match |
| CLBG::Nbody | 0.938 | 0.699 | 1.34x | 0.691 | 1.36x | 0.804 | 1.17x | closest-possible |
| CLBG::Spectralnorm | 0.751 | 1.076 | 0.70x | 1.093 | 0.69x | 0.753 | 1.00x | rewritten-to-match |
| Compress::ArithDecode | 0.978 | 1.221 | 0.80x | 1.213 | 0.81x | 0.901 | 1.09x | rewritten-to-match |
| Compress::ArithEncode | 1.059 | 0.883 | 1.20x | 0.958 | 1.11x | 1.143 | 0.93x | like-for-like |
| Compress::BWTDecode | 0.878 | 1.083 | 0.81x | 1.049 | 0.84x | 0.921 | 0.95x | rewritten-to-match |
| Compress::BWTEncode | 0.553 | 0.421 | 1.31x | 0.422 | 1.31x | 1.093 | 0.51x | closest-possible |
| Compress::HuffDecode | 1.065 | 0.621 | 1.72x | 0.615 | 1.73x | 0.897 | 1.19x | rewritten-to-match |
| Compress::HuffEncode | 0.641 | 0.591 | 1.08x | 0.601 | 1.07x | 0.903 | 0.71x | rewritten-to-match |
| Compress::LZWDecode | 2.102 | 1.160 | 1.81x | 1.187 | 1.77x | 0.522 | 4.03x | rewritten-to-match |
| Compress::LZWEncode | 0.094 | 0.527 | 0.18x | 0.514 | 0.18x | 0.844 | 0.11x | closest-possible |
| CSV::Parse | 0.562 | 0.615 | 0.91x | 0.610 | 0.92x | 3.168 | 0.18x | closest-possible |
| Distance::Jaro | 0.964 | 1.083 | 0.89x | 0.996 | 0.97x | 1.150 | 0.84x | rewritten-to-match |
| Distance::NGram | 0.421 | 0.349 | 1.21x | 0.361 | 1.17x | 0.631 | 0.67x | rewritten-to-match |
| Etc::CacheSimulation | 0.469 | 0.804 | 0.58x | 0.795 | 0.59x | 2.022 | 0.23x | rewritten-to-match |
| Etc::GameOfLife | 1.599 | 1.929 | 0.83x | 1.933 | 0.83x | 1.817 | 0.88x | like-for-like |
| Etc::NeuralNet | 0.799 | 0.858 | 0.93x | 0.860 | 0.93x | 0.922 | 0.87x | closest-possible |
| Etc::Sieve | 2.279 | 1.338 | 1.70x | 1.815 | 1.26x | 2.003 | 1.14x | rewritten-to-match |
| Etc::TextRaytracer | 0.735 | 0.614 | 1.20x | 0.631 | 1.16x | 0.795 | 0.92x | rewritten-to-match |
| Etc::Words | 0.559 | 0.774 | 0.72x | 0.769 | 0.73x | 0.796 | 0.70x | closest-possible |
| Graph::AStar | 0.924 | 1.273 | 0.73x | 1.304 | 0.71x | 2.041 | 0.45x | rewritten-to-match |
| Graph::BFS | 0.986 | 0.823 | 1.20x | 0.826 | 1.19x | 1.225 | 0.80x | rewritten-to-match |
| Graph::DFS | 0.961 | 0.716 | 1.34x | 0.732 | 1.31x | 1.034 | 0.93x | rewritten-to-match |
| Hash::CRC32 | 0.697 | 0.837 | 0.83x | 0.830 | 0.84x | 0.807 | 0.86x | like-for-like |
| Hash::SHA256 | 1.000 | 0.661 | 1.51x | 0.660 | 1.51x | 0.658 | 1.52x | like-for-like |
| Matmul::Single | 4.011 | 3.222 | 1.24x | 3.228 | 1.24x | 3.078 | 1.30x | closest-possible |
| Maze::AStar | 0.957 | 1.078 | 0.89x | 1.103 | 0.87x | 3.968 | 0.24x | rewritten-to-match |
| Maze::BFS | 0.460 | 0.740 | 0.62x | 0.733 | 0.63x | 1.476 | 0.31x | rewritten-to-match |
| Maze::Generator | 0.513 | 0.720 | 0.71x | 0.725 | 0.71x | 1.418 | 0.36x | rewritten-to-match |
| Sort::Merge | 1.821 | 1.289 | 1.41x | 1.284 | 1.42x | 2.155 | 0.85x | rewritten-to-match |
| Sort::Quick | 1.383 | 1.136 | 1.22x | 1.150 | 1.20x | 1.541 | 0.90x | rewritten-to-match |
| Template::Parse | 0.689 | 0.811 | 0.85x | 0.876 | 0.79x | 1.831 | 0.38x | closest-possible |

Geometric mean of the 28 like-for-like and rewritten-to-match ports: 1.11x of Rust and 0.88x of Crystal (the 13 closest-possible ports are left out; over all 41 it is 1.13x of Rust). All 41 ports were measured together in one run (3 rounds, best of rounds, 2026-11-04). Crystal is LangArena's original implementation, 1.22.2 built with `crystal build main.cr --release` from a patched copy (the one JSON-library call, in a test that is not
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
| 14 | CSV::Parse | ported (reference uses the simd_csv crate; decimals written from integers) |
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

Counts now: 41 ported (the 7 in the first batch, both Brainfuck tests, NeuralNet, TextRaytracer, then Nbody, CRC32, SHA256, GameOfLife, the three graph tests, the three maze tests, and both Binarytrees), 9 not comparable (the three threaded
matmuls, the three JSON tests, LogParser, Template::Regex, Sort::Self) and 0 still to do or blocked. Passing a string literal to a function (`put("text")`, reading its bytes) and mutually recursive functions already work, which is what made the text generator and the parser expressible. Language features added along the way (each tested, all merged): bit operators and hex literals
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
