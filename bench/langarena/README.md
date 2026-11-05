# LangArena ports for Mitochondria

Mitochondria ports of [LangArena](https://github.com/kostya/LangArena) tests (MIT, see `LICENSE-LangArena`), measured against LangArena's own Rust implementation on the same machine. `run.js` and `test.js` are LangArena's parameter files, vendored with one change: two test names shortened to `BF::Array` and `BF::Recursion` (run.py gives the reference binaries LangArena's own names); each entry's `checksum` is what both sides must reproduce.

Run: build the Rust reference once (`CARGO_TARGET_DIR=bench/langarena/target-rust cargo build --profile max-perf --manifest-path <LangArena>/rust/Cargo.toml`, and the same with `--config 'profile.max-perf.overflow-checks=true'` into `target-rust-checked`), then `python -I bench/langarena/run.py [--mode run|test] [--rounds N] [test_dir ...]`. `LANGARENA` names the clone (default `D:/tmp/langarena`).

## How the comparison is made

- LangArena times only `run_all()`, after `prepare()` and the warmup iterations (max(0.2 * iterations, 1) unless the entry says `warmup_iterations`); its checksum covers the warmup runs too, and its random generator is reset after warmup. A port is a program with `POST /prepare/...`, `POST /warmup/{n}`, `POST /run/{n}` and `GET /checksum`; prepare and warmup are untimed requests whose state persists, and only `/run` is timed.
- Mitochondria is timed by the server itself (`--timing`, header `X-Handler-Ns`), the Rust reference by its own in-process timer: both measure the iterations and nothing else.
- Both processes are pinned to the same core (`bench/ordered/pin.py`; this CPU mixes fast and slow cores), rounds alternate, best of N and the median are reported. Mitochondria is built by `mitochondria build` (clang -O3); the Rust reference with its own `max-perf` profile (opt-level 3, fat LTO, one codegen unit, **overflow checks off**).
- Mitochondria always checks integer overflow, so the table has a second Rust column: the same reference built with overflow checks on.
- The checksum is compared with `run.js` on both sides in every measured run (`verified`). A checksum that hashes a formatted float is verified by the harness, which formats and hashes the answered float exactly as LangArena's helper does (`{:.7}`, then hash = hash * 33 + byte).
- State a port rebuilds every run (the reference's locals: scratch arrays, output buffers, per-run arenas) is declared `unlogged`: kept out of the undo log, as a Rust local has none; everything else is logged, so a failed request rolls it back. Each audit file says which fields.
- Each port has an `audit` file with its verdict: `like-for-like`, `rewritten-to-match` (same algorithm, small mechanical rewrites the language forces, listed) or `closest-possible` (a difference in data-structure shape, listed). The headline geometric mean covers only the first two.
- "Original" in LangArena means the Crystal implementation. Crystal is not installed here; its sources are in `<LangArena>/crystal` and the ports follow the same algorithms (the Rust implementation is a faithful port of them).

## Results (2026-11-04; run parameters, 3 rounds, best of rounds, all 41 ports in one run; ratio = Mitochondria / the column's implementation)

| Workload | Mitochondria s | Rust (reference) s | Mito/Rust | Rust with overflow checks s | Mito/Rust (checked) | Crystal (original) s | Mito/Crystal | audit |
|---|---|---|---|---|---|---|---|---|
| Base64::Decode | 1.066 | 0.186 | 5.73x | 0.191 | 5.58x | 1.144 | 0.93x | closest-possible |
| Base64::Encode | 1.179 | 0.167 | 7.06x | 0.167 | 7.06x | 1.116 | 1.06x | closest-possible |
| Binarytrees::Arena | 0.983 | 0.541 | 1.82x | 0.520 | 1.89x | 0.544 | 1.81x | like-for-like |
| Binarytrees::Obj | 2.836 | 3.009 | 0.94x | 2.970 | 0.95x | 1.511 | 1.88x | like-for-like |
| BF::Array | 1.544 | 0.943 | 1.64x | 1.346 | 1.15x | 2.133 | 0.72x | rewritten-to-match |
| BF::Recursion | 1.836 | 1.370 | 1.34x | 1.473 | 1.25x | 1.491 | 1.23x | closest-possible |
| Calculator::Ast | 0.209 | 0.686 | 0.30x | 0.688 | 0.30x | 0.415 | 0.50x | closest-possible |
| Calculator::Interpreter | 0.529 | 0.885 | 0.60x | 0.891 | 0.59x | 0.513 | 1.03x | closest-possible |
| CLBG::Fannkuchredux | 0.921 | 0.905 | 1.02x | 0.926 | 0.99x | 1.011 | 0.91x | like-for-like |
| CLBG::Mandelbrot | 0.702 | 0.726 | 0.97x | 0.738 | 0.95x | 0.701 | 1.00x | rewritten-to-match |
| CLBG::Nbody | 0.938 | 0.712 | 1.32x | 0.698 | 1.34x | 0.813 | 1.15x | closest-possible |
| CLBG::Spectralnorm | 0.769 | 1.095 | 0.70x | 1.108 | 0.69x | 0.766 | 1.00x | rewritten-to-match |
| Compress::ArithDecode | 0.972 | 1.254 | 0.78x | 1.242 | 0.78x | 0.918 | 1.06x | rewritten-to-match |
| Compress::ArithEncode | 1.074 | 0.913 | 1.18x | 0.969 | 1.11x | 1.128 | 0.95x | like-for-like |
| Compress::BWTDecode | 0.867 | 1.040 | 0.83x | 1.026 | 0.85x | 0.939 | 0.92x | rewritten-to-match |
| Compress::BWTEncode | 0.532 | 0.433 | 1.23x | 0.424 | 1.25x | 1.129 | 0.47x | closest-possible |
| Compress::HuffDecode | 0.918 | 0.629 | 1.46x | 0.618 | 1.49x | 0.918 | 1.00x | rewritten-to-match |
| Compress::HuffEncode | 0.607 | 0.595 | 1.02x | 0.609 | 1.00x | 0.930 | 0.65x | rewritten-to-match |
| Compress::LZWDecode | 0.181 | 1.133 | 0.16x | 1.136 | 0.16x | 0.529 | 0.34x | closest-possible |
| Compress::LZWEncode | 0.091 | 0.530 | 0.17x | 0.519 | 0.18x | 0.874 | 0.10x | closest-possible |
| CSV::Parse | 0.597 | 0.616 | 0.97x | 0.588 | 1.01x | 3.152 | 0.19x | closest-possible |
| Distance::Jaro | 0.911 | 1.062 | 0.86x | 0.986 | 0.92x | 1.151 | 0.79x | rewritten-to-match |
| Distance::NGram | 0.418 | 0.351 | 1.19x | 0.364 | 1.15x | 0.639 | 0.65x | rewritten-to-match |
| Etc::CacheSimulation | 0.466 | 0.813 | 0.57x | 0.800 | 0.58x | 1.887 | 0.25x | rewritten-to-match |
| Etc::GameOfLife | 1.541 | 1.964 | 0.78x | 1.979 | 0.78x | 1.853 | 0.83x | like-for-like |
| Etc::NeuralNet | 0.872 | 0.864 | 1.01x | 0.864 | 1.01x | 0.926 | 0.94x | closest-possible |
| Etc::Sieve | 1.692 | 1.320 | 1.28x | 1.795 | 0.94x | 2.027 | 0.83x | rewritten-to-match |
| Etc::TextRaytracer | 0.749 | 0.627 | 1.19x | 0.629 | 1.19x | 0.799 | 0.94x | rewritten-to-match |
| Etc::Words | 0.598 | 0.800 | 0.75x | 0.785 | 0.76x | 0.804 | 0.74x | closest-possible |
| Graph::AStar | 0.645 | 1.313 | 0.49x | 1.331 | 0.48x | 2.162 | 0.30x | rewritten-to-match |
| Graph::BFS | 0.976 | 0.840 | 1.16x | 0.853 | 1.14x | 1.239 | 0.79x | rewritten-to-match |
| Graph::DFS | 0.901 | 0.729 | 1.24x | 0.746 | 1.21x | 1.066 | 0.85x | rewritten-to-match |
| Hash::CRC32 | 0.813 | 0.846 | 0.96x | 0.838 | 0.97x | 0.813 | 1.00x | like-for-like |
| Hash::SHA256 | 0.644 | 0.671 | 0.96x | 0.676 | 0.95x | 0.661 | 0.97x | like-for-like |
| Matmul::Single | 3.271 | 3.303 | 0.99x | 3.298 | 0.99x | 3.172 | 1.03x | closest-possible |
| Maze::AStar | 0.936 | 1.151 | 0.81x | 1.159 | 0.81x | 4.144 | 0.23x | rewritten-to-match |
| Maze::BFS | 0.472 | 0.796 | 0.59x | 0.786 | 0.60x | 1.561 | 0.30x | rewritten-to-match |
| Maze::Generator | 0.502 | 0.751 | 0.67x | 0.753 | 0.67x | 1.464 | 0.34x | rewritten-to-match |
| Sort::Merge | 1.469 | 1.319 | 1.11x | 1.321 | 1.11x | 2.207 | 0.67x | rewritten-to-match |
| Sort::Quick | 1.285 | 1.126 | 1.14x | 1.181 | 1.09x | 1.580 | 0.81x | rewritten-to-match |
| Template::Parse | 0.624 | 0.981 | 0.64x | 1.050 | 0.59x | 1.938 | 0.32x | closest-possible |

Geometric mean of the 27 like-for-like and rewritten-to-match ports: 0.97x of Rust and 0.74x of Crystal (the 14 closest-possible ports are left out; over all 41 it is 0.94x of Rust). All 41 ports were measured together in one run (3 rounds, best of rounds, 2026-11-05, after keep-alive, range-typed fields, wider loop versioning and `unlogged` scratch state). Crystal is LangArena's original implementation, 1.22.2 built with `crystal build main.cr --release` from a patched copy (the one JSON-library call, in a test that is not
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
| 3 | BF::Array | ported |
| 4 | BF::Recursion | ported |
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

Counts now: 41 ported (the 7 in the first batch, both BF tests, NeuralNet, TextRaytracer, then Nbody, CRC32, SHA256, GameOfLife, the three graph tests, the three maze tests, and both Binarytrees), 9 not comparable (the three threaded
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
