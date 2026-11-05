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
| Base64::Decode | 1.632 | 0.184 | 8.87x | 0.184 | 8.87x | 1.136 | 1.44x | closest-possible |
| Base64::Encode | 2.507 | 0.165 | 15.19x | 0.165 | 15.19x | 1.118 | 2.24x | closest-possible |
| Binarytrees::Arena | 1.127 | 0.543 | 2.08x | 0.522 | 2.16x | 0.538 | 2.10x | like-for-like |
| Binarytrees::Obj | 3.686 | 2.892 | 1.27x | 2.876 | 1.28x | 1.489 | 2.48x | like-for-like |
| Brainfuck::Array | 1.373 | 0.918 | 1.50x | 1.313 | 1.05x | 2.065 | 0.66x | rewritten-to-match |
| Brainfuck::Recursion | 1.994 | 1.330 | 1.50x | 1.462 | 1.36x | 1.471 | 1.36x | closest-possible |
| Calculator::Ast | 0.223 | 0.680 | 0.33x | 0.672 | 0.33x | 0.398 | 0.56x | closest-possible |
| Calculator::Interpreter | 0.522 | 0.864 | 0.60x | 0.872 | 0.60x | 0.476 | 1.10x | closest-possible |
| CLBG::Fannkuchredux | 1.362 | 0.912 | 1.49x | 0.923 | 1.48x | 1.018 | 1.34x | like-for-like |
| CLBG::Mandelbrot | 0.696 | 0.709 | 0.98x | 0.723 | 0.96x | 0.690 | 1.01x | rewritten-to-match |
| CLBG::Nbody | 0.930 | 0.712 | 1.31x | 0.690 | 1.35x | 0.795 | 1.17x | closest-possible |
| CLBG::Spectralnorm | 0.760 | 1.099 | 0.69x | 1.097 | 0.69x | 0.744 | 1.02x | rewritten-to-match |
| Compress::ArithDecode | 1.001 | 1.251 | 0.80x | 1.227 | 0.82x | 0.905 | 1.11x | rewritten-to-match |
| Compress::ArithEncode | 1.049 | 0.896 | 1.17x | 0.963 | 1.09x | 1.115 | 0.94x | like-for-like |
| Compress::BWTDecode | 0.868 | 1.082 | 0.80x | 1.069 | 0.81x | 0.923 | 0.94x | rewritten-to-match |
| Compress::BWTEncode | 0.488 | 0.443 | 1.10x | 0.435 | 1.12x | 1.126 | 0.43x | closest-possible |
| Compress::HuffDecode | 1.036 | 0.614 | 1.69x | 0.615 | 1.68x | 0.904 | 1.15x | rewritten-to-match |
| Compress::HuffEncode | 0.739 | 0.594 | 1.24x | 0.602 | 1.23x | 0.908 | 0.81x | rewritten-to-match |
| Compress::LZWDecode | 0.202 | 1.120 | 0.18x | 1.123 | 0.18x | 0.513 | 0.39x | closest-possible |
| Compress::LZWEncode | 0.094 | 0.520 | 0.18x | 0.514 | 0.18x | 0.867 | 0.11x | closest-possible |
| CSV::Parse | 0.580 | 0.626 | 0.93x | 0.597 | 0.97x | 3.159 | 0.18x | closest-possible |
| Distance::Jaro | 0.958 | 1.076 | 0.89x | 0.987 | 0.97x | 1.147 | 0.84x | rewritten-to-match |
| Distance::NGram | 0.428 | 0.344 | 1.24x | 0.361 | 1.19x | 0.638 | 0.67x | rewritten-to-match |
| Etc::CacheSimulation | 0.471 | 0.837 | 0.56x | 0.804 | 0.59x | 1.940 | 0.24x | rewritten-to-match |
| Etc::GameOfLife | 1.586 | 1.901 | 0.83x | 1.958 | 0.81x | 1.829 | 0.87x | like-for-like |
| Etc::NeuralNet | 0.876 | 0.859 | 1.02x | 0.859 | 1.02x | 0.927 | 0.94x | closest-possible |
| Etc::Sieve | 1.725 | 1.312 | 1.31x | 1.785 | 0.97x | 1.991 | 0.87x | rewritten-to-match |
| Etc::TextRaytracer | 0.760 | 0.618 | 1.23x | 0.617 | 1.23x | 0.787 | 0.97x | rewritten-to-match |
| Etc::Words | 0.577 | 0.783 | 0.74x | 0.775 | 0.74x | 0.788 | 0.73x | closest-possible |
| Graph::AStar | 0.888 | 1.288 | 0.69x | 1.302 | 0.68x | 2.047 | 0.43x | rewritten-to-match |
| Graph::BFS | 0.988 | 0.825 | 1.20x | 0.830 | 1.19x | 1.208 | 0.82x | rewritten-to-match |
| Graph::DFS | 0.944 | 0.713 | 1.32x | 0.726 | 1.30x | 1.036 | 0.91x | rewritten-to-match |
| Hash::CRC32 | 0.807 | 0.825 | 0.98x | 0.837 | 0.96x | 0.800 | 1.01x | like-for-like |
| Hash::SHA256 | 0.856 | 0.663 | 1.29x | 0.662 | 1.29x | 0.654 | 1.31x | like-for-like |
| Matmul::Single | 3.270 | 3.291 | 0.99x | 3.376 | 0.97x | 3.129 | 1.04x | closest-possible |
| Maze::AStar | 0.944 | 1.102 | 0.86x | 1.141 | 0.83x | 4.075 | 0.23x | rewritten-to-match |
| Maze::BFS | 0.465 | 0.760 | 0.61x | 0.763 | 0.61x | 1.514 | 0.31x | rewritten-to-match |
| Maze::Generator | 0.525 | 0.726 | 0.72x | 0.727 | 0.72x | 1.438 | 0.36x | rewritten-to-match |
| Sort::Merge | 1.706 | 1.300 | 1.31x | 1.304 | 1.31x | 2.201 | 0.78x | rewritten-to-match |
| Sort::Quick | 1.406 | 1.119 | 1.26x | 1.155 | 1.22x | 1.552 | 0.91x | rewritten-to-match |
| Template::Parse | 0.699 | 0.833 | 0.84x | 0.837 | 0.83x | 1.803 | 0.39x | closest-possible |

Geometric mean of the 27 like-for-like and rewritten-to-match ports: 1.06x of Rust and 0.81x of Crystal (the 14 closest-possible ports are left out; over all 41 it is 1.04x of Rust). All 41 ports were measured together in one run (3 rounds, best of rounds, 2026-11-05, after the pool, array.move, commit-test and loop-versioning changes below). Crystal is LangArena's original implementation, 1.22.2 built with `crystal build main.cr --release` from a patched copy (the one JSON-library call, in a test that is not
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
| 47-48 | Compress::LZWEncode / LZWDecode | ported (encode keys its dictionary by (prefix code, byte) instead of the whole string; decode keeps its entries in one array instead of one Vec each; both favour Mitochondria: excluded from the geometric mean) |
| 49-50 | Distance::Jaro / NGram | ported (NGram uses the new lib/intmap hash map) |

Counts now: 41 ported (the 7 in the first batch, both Brainfuck tests, NeuralNet, TextRaytracer, then Nbody, CRC32, SHA256, GameOfLife, the three graph tests, the three maze tests, and both Binarytrees), 9 not comparable (the three threaded
matmuls, the three JSON tests, LogParser, Template::Regex, Sort::Self) and 0 still to do or blocked. Passing a string literal to a function (`put("text")`, reading its bytes) and mutually recursive functions already work, which is what made the text generator and the parser expressible. Language features added along the way (each tested, all merged): bit operators and hex literals
(`& | ^ << >>`, Rust's precedence), `sqrt` `exp` `floor` `abs`, reading a string's bytes by index (`s[i]`; strings stay read-only), `coll.clear()`, `array.fill`, `array.move` and `array.count_below`.
Still blocked by something else: building text (Base64 output, the parsers' string results), hash maps (CacheSimulation, Words, Template::Parse, Distance, Calculator::Interpreter, parts of Compress) and CSV.
Where the time goes in what is slower than the reference: Binarytrees::Arena (was 3.98x of Rust, now 2.16x) is pool `add` (now about 12 ns, was 28; a `Vec` push is a few); the sieve, Fannkuchredux and
the merge sort are loops of many small array stores, which pay for atomic requests.


Language and library added for the string tests: `lib/strmap.mito` (hash map from string keys to i64, chained, entries numbered so a caller can keep its own data per entry, slots reused at once) and a text-area rule: strings built in a loop body are dropped at the end of each iteration unless the body assigns one to a variable declared before the loop (the 256 KB text area of a request otherwise fills after about 20,000 string keys; CacheSimulation builds 5.8 million).

Also added: `substr(text, from, count)` (a piece of a string sharing its bytes, range checked) and a 4 MB text area (was 256 KB). Found and fixed while porting Words: a `let` of a `var` (`let start = at;` and then `at = at + 1`) shared the var's C variable, so the `let` changed with it (numbers, floats, strings, references); a `let` now copies a `var`'s value (tests/cases/letcopy.case). No earlier port was affected (their checksums still verify).

`lib/intmap.mito` (hash map from i64 keys to i64 values, same design as strmap) was added for NGram.

`text_from(bytes, from, count)` (a state array of u8 as a string, copied into the text area, range checked) was added for Template::Parse.

`wrapping_add`, `wrapping_sub` and `wrapping_mul` (two's-complement arithmetic; the ordinary operators fail the request on overflow) were added for Calculator::Interpreter.

Pool `add` was sped up afterwards (Binarytrees::Arena 3.76x to about 2x of Rust; the other pool-heavy ports measured within noise): a fast path for adding at the high-water mark in a pool with one instance and no generations or nested collections, `coll.clear()` zeroing the used slots in one pass per column (so refilling finds zeroed memory), and a small inline check-and-log for the few bytes each add touches. `array.move` now saves and commits only the range it writes; copying into a far-away range had made it pay for everything in between (200x on LZWDecode's copy into a 1 MB array).

Then two cuts to the per-store tests (2026-11-05): stores to live items no longer test whether their page is committed (`add` commits an item's memory and trimming never takes back a live range), and loops are versioned: when a loop's array indexes are its counter (or a variable it steps up to a limit) plus or minus a value the loop does not change, a test before the loop checks that every index stays in its array and saves the stored ranges to the undo log in one pass, and the loop then runs without per-access range and undo-log tests (the checked loop runs otherwise, and for loops of fewer than 32 steps). A/B against the previous compiler on the same core: Sieve 2.33 to 1.77 s, SHA256 1.02 to 0.86 s, Sort::Merge 1.93 to 1.74 s; Fannkuch, Quick, CRC32, GameOfLife within noise.
