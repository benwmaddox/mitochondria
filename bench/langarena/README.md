# LangArena ports for Mitochondria

Mitochondria ports of [LangArena](https://github.com/kostya/LangArena) tests (MIT, see `LICENSE-LangArena`), measured against LangArena's own Rust implementation on the same machine. `run.js` and `test.js` are LangArena's parameter files, vendored with one change: two test names shortened to `BF::Array` and `BF::Recursion` (run.py gives the reference binaries LangArena's own names); each entry's `checksum` is what both sides must reproduce.

Run: build the Rust reference once (`CARGO_TARGET_DIR=bench/langarena/target-rust cargo build --profile max-perf --manifest-path <LangArena>/rust/Cargo.toml`, and the same with `--config 'profile.max-perf.overflow-checks=true'` into `target-rust-checked`), then `python -I bench/langarena/run.py [--mode run|test] [--rounds N] [test_dir ...]`. `LANGARENA` names the clone (default `D:/tmp/langarena`).

The latest numbers, time and peak memory for every port against Rust and Crystal, are in [RESULTS.md](RESULTS.md), which `run.py` writes (from `results.json`) at the end of every run of the run parameters.

## How the comparison is made

- LangArena times only `run_all()`, after `prepare()` and the warmup iterations (max(0.2 * iterations, 1) unless the entry says `warmup_iterations`); its checksum covers the warmup runs too, and its random generator is reset after warmup. A port is a program with `POST /prepare/...`, `POST /warmup/{n}`, `POST /run/{n}` and `GET /checksum`; prepare and warmup are untimed requests whose state persists, and only `/run` is timed.
- Mitochondria is timed by the server itself (`--timing`, header `X-Handler-Ns`), the Rust reference by its own in-process timer: both measure the iterations and nothing else.
- Both processes are pinned to the same core (`bench/ordered/pin.py`; this CPU mixes fast and slow cores), rounds alternate, best of N and the median are reported. Mitochondria is built by `mito build` (clang -O3); the Rust reference with its own `max-perf` profile (opt-level 3, fat LTO, one codegen unit, **overflow checks off**).
- Mitochondria always checks integer overflow, so the table has a second Rust column: the same reference built with overflow checks on.
- The checksum is compared with `run.js` on both sides in every measured run (`verified`). A checksum that hashes a formatted float is verified by the harness, which formats and hashes the answered float exactly as LangArena's helper does (`{:.7}`, then hash = hash * 33 + byte).
- State is unlogged by default (from 2026-11-11; before that the ports declared the state they rebuild every run `unlogged` and everything else was logged, and the numbers below were measured that way): no undo log, as a Rust local has none; before, everything else was logged, so a failed request rolls it back. Each audit file says which fields.
- Each port has an `audit` file with its verdict: `like-for-like`, `rewritten-to-match` (same algorithm, small mechanical rewrites the language forces, listed) or `closest-possible` (a difference in data-structure shape, listed). The headline geometric mean covers only the first two.
- "Original" in LangArena means the Crystal implementation. Crystal is not installed here; its sources are in `<LangArena>/crystal` and the ports follow the same algorithms (the Rust implementation is a faithful port of them).

## Results (Mitochondria 2026-11-05, Rust and Crystal 2026-11-04; run parameters, 3 rounds, best of rounds, all 41 ports in one run; ratio = Mitochondria / the column's implementation)

| Workload | Mitochondria s | Rust (reference) s | Mito/Rust | Rust with overflow checks s | Mito/Rust (checked) | Crystal (original) s | Mito/Crystal | audit |
|---|---|---|---|---|---|---|---|---|
| Base64::Decode | 1.085 | 0.186 | 5.83x | 0.191 | 5.68x | 1.144 | 0.95x | closest-possible |
| Base64::Encode | 1.123 | 0.167 | 6.72x | 0.167 | 6.72x | 1.116 | 1.01x | closest-possible |
| Binarytrees::Arena | 1.029 | 0.541 | 1.90x | 0.520 | 1.98x | 0.544 | 1.89x | like-for-like |
| Binarytrees::Obj | 2.042 | 3.009 | 0.68x | 2.970 | 0.69x | 1.511 | 1.35x | like-for-like |
| BF::Array | 1.397 | 0.943 | 1.48x | 1.346 | 1.04x | 2.133 | 0.65x | rewritten-to-match |
| BF::Recursion | 1.950 | 1.370 | 1.42x | 1.473 | 1.32x | 1.491 | 1.31x | closest-possible |
| Calculator::Ast | 0.189 | 0.686 | 0.28x | 0.688 | 0.27x | 0.415 | 0.46x | closest-possible |
| Calculator::Interpreter | 0.343 | 0.885 | 0.39x | 0.891 | 0.38x | 0.513 | 0.67x | closest-possible |
| CLBG::Fannkuchredux | 0.962 | 0.905 | 1.06x | 0.926 | 1.04x | 1.011 | 0.95x | like-for-like |
| CLBG::Mandelbrot | 0.701 | 0.726 | 0.97x | 0.738 | 0.95x | 0.701 | 1.00x | rewritten-to-match |
| CLBG::Nbody | 0.967 | 0.712 | 1.36x | 0.698 | 1.39x | 0.813 | 1.19x | closest-possible |
| CLBG::Spectralnorm | 0.755 | 1.095 | 0.69x | 1.108 | 0.68x | 0.766 | 0.99x | rewritten-to-match |
| Compress::ArithDecode | 1.020 | 1.254 | 0.81x | 1.242 | 0.82x | 0.918 | 1.11x | rewritten-to-match |
| Compress::ArithEncode | 1.010 | 0.913 | 1.11x | 0.969 | 1.04x | 1.128 | 0.90x | like-for-like |
| Compress::BWTDecode | 0.904 | 1.040 | 0.87x | 1.026 | 0.88x | 0.939 | 0.96x | rewritten-to-match |
| Compress::BWTEncode | 0.495 | 0.433 | 1.14x | 0.424 | 1.17x | 1.129 | 0.44x | closest-possible |
| Compress::HuffDecode | 1.057 | 0.629 | 1.68x | 0.618 | 1.71x | 0.918 | 1.15x | rewritten-to-match |
| Compress::HuffEncode | 0.664 | 0.595 | 1.12x | 0.609 | 1.09x | 0.930 | 0.71x | rewritten-to-match |
| Compress::LZWDecode | 0.180 | 1.133 | 0.16x | 1.136 | 0.16x | 0.529 | 0.34x | closest-possible |
| Compress::LZWEncode | 0.095 | 0.530 | 0.18x | 0.519 | 0.18x | 0.874 | 0.11x | closest-possible |
| CSV::Parse | 0.575 | 0.616 | 0.93x | 0.588 | 0.98x | 3.152 | 0.18x | closest-possible |
| Distance::Jaro | 0.949 | 1.062 | 0.89x | 0.986 | 0.96x | 1.151 | 0.82x | rewritten-to-match |
| Distance::NGram | 0.436 | 0.351 | 1.24x | 0.364 | 1.20x | 0.639 | 0.68x | rewritten-to-match |
| Etc::CacheSimulation | 0.447 | 0.813 | 0.55x | 0.800 | 0.56x | 1.887 | 0.24x | rewritten-to-match |
| Etc::GameOfLife | 1.593 | 1.964 | 0.81x | 1.979 | 0.80x | 1.853 | 0.86x | like-for-like |
| Etc::NeuralNet | 0.901 | 0.864 | 1.04x | 0.864 | 1.04x | 0.926 | 0.97x | closest-possible |
| Etc::Sieve | 1.742 | 1.320 | 1.32x | 1.795 | 0.97x | 2.027 | 0.86x | rewritten-to-match |
| Etc::TextRaytracer | 0.748 | 0.627 | 1.19x | 0.629 | 1.19x | 0.799 | 0.94x | rewritten-to-match |
| Etc::Words | 0.558 | 0.800 | 0.70x | 0.785 | 0.71x | 0.804 | 0.69x | closest-possible |
| Graph::AStar | 0.636 | 1.313 | 0.48x | 1.331 | 0.48x | 2.162 | 0.29x | rewritten-to-match |
| Graph::BFS | 0.965 | 0.840 | 1.15x | 0.853 | 1.13x | 1.239 | 0.78x | rewritten-to-match |
| Graph::DFS | 0.880 | 0.729 | 1.21x | 0.746 | 1.18x | 1.066 | 0.83x | rewritten-to-match |
| Hash::CRC32 | 0.808 | 0.846 | 0.96x | 0.838 | 0.96x | 0.813 | 0.99x | like-for-like |
| Hash::SHA256 | 0.650 | 0.671 | 0.97x | 0.676 | 0.96x | 0.661 | 0.98x | like-for-like |
| Matmul::Single | 3.334 | 3.303 | 1.01x | 3.298 | 1.01x | 3.172 | 1.05x | closest-possible |
| Maze::AStar | 0.941 | 1.151 | 0.82x | 1.159 | 0.81x | 4.144 | 0.23x | rewritten-to-match |
| Maze::BFS | 0.452 | 0.796 | 0.57x | 0.786 | 0.58x | 1.561 | 0.29x | rewritten-to-match |
| Maze::Generator | 0.466 | 0.751 | 0.62x | 0.753 | 0.62x | 1.464 | 0.32x | rewritten-to-match |
| Sort::Merge | 1.507 | 1.319 | 1.14x | 1.321 | 1.14x | 2.207 | 0.68x | rewritten-to-match |
| Sort::Quick | 1.369 | 1.126 | 1.22x | 1.181 | 1.16x | 1.580 | 0.87x | rewritten-to-match |
| Template::Parse | 0.603 | 0.981 | 0.61x | 1.050 | 0.57x | 1.938 | 0.31x | closest-possible |

Geometric mean of the 27 like-for-like and rewritten-to-match ports: 0.965x of Rust (0.939x of Rust with overflow checks) and 0.737x of Crystal (the 14 closest-possible ports are left out; over all 41 it is 0.926x of Rust). The Mitochondria column was measured again on 2026-11-05 (all 41 in one run, 3 rounds, best of rounds) after the language work of that day: loops (`foreach`, `while`, `for`), typed
signatures, enums and `match`, and text built in fields (`ascii<N>` with `+=`, `substr` windows, `request` structs); the Rust and Crystal columns are the 2026-11-04 run, unchanged.
Against the previous Mitochondria column: Calculator::Interpreter -35% and Calculator::Ast -10% (their text is an ascii field, names are windows onto it), BF::Array -10% (`match`),
Binarytrees::Obj -28% (that row was stale); Compress::HuffDecode +15% and HuffEncode +9% against the old column, +4% against the compiler before that day's work measured side by
side, with the same generated code for its endpoints (the runtime gained functions; the difference is the binary's layout, not the program's code). Earlier, all 41 were measured
together on 2026-11-05 after keep-alive, range-typed fields, wider loop versioning and `unlogged` scratch state. Crystal is LangArena's original implementation, 1.22.2 built with `crystal build main.cr --release` from a patched copy (the one JSON-library call, in a test that is not
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
(`& | ^ << >>`, Rust's precedence), `sqrt` `exp` `floor` `abs`, reading a string's bytes by index (`s[i]`), `coll.clear()`, `array.fill`, `array.move` and `array.count_below`.
Since then (2026-11-05) the ports that build text build it in fields: Calculator, CSV and Template keep their text in an `ascii<N>` field built with `+=` (the old byte-array `put` helpers are gone), names cut from it are `substr` windows (no copy), and the Base64 ports and CacheSimulation build their message or key in a `request` struct. Hash maps are lib/strmap.mito and lib/intmap.mito.
Where the time goes in what is slower than the reference: Binarytrees::Arena (was 3.98x of Rust, now 2.16x) is pool `add` (now about 12 ns, was 28; a `Vec` push is a few); the sieve, Fannkuchredux and
the merge sort are loops of many small array stores, which pay for atomic requests.


Language and library added for the string tests: `lib/strmap.mito` (hash map from string keys to i64, chained, entries numbered so a caller can keep its own data per entry, slots reused at once) and a text-area rule: strings built in a loop body are dropped at the end of each iteration unless the body assigns one to a variable declared before the loop (the 256 KB text area of a request otherwise fills after about 20,000 string keys; CacheSimulation builds 5.8 million).

Also added: `substr(text, from, count)` (a piece of a string sharing its bytes, range checked) and a 4 MB text area (was 256 KB). Found and fixed while porting Words: a `let` of a `var` (`let start = at;` and then `at = at + 1`) shared the var's C variable, so the `let` changed with it (numbers, floats, strings, references); a `let` now copies a `var`'s value (tests/cases/letcopy.case). No earlier port was affected (their checksums still verify).

`lib/intmap.mito` (hash map from i64 keys to i64 values, same design as strmap) was added for NGram.

`text_from(bytes, from, count)` (a state array of u8 as a string, copied into the text area, range checked) was added for Template::Parse.

`wrapping_add`, `wrapping_sub` and `wrapping_mul` (two's-complement arithmetic; the ordinary operators fail the request on overflow) were added for Calculator::Interpreter.

Pool `add` was sped up afterwards (Binarytrees::Arena 3.76x to about 2x of Rust; the other pool-heavy ports measured within noise): a fast path for adding at the high-water mark in a pool with one instance and no generations or nested collections, `coll.clear()` zeroing the used slots in one pass per column (so refilling finds zeroed memory), and a small inline check-and-log for the few bytes each add touches. `array.move` now saves and commits only the range it writes; copying into a far-away range had made it pay for everything in between (200x on LZWDecode's copy into a 1 MB array).

Then two cuts to the per-store tests (2026-11-05): stores to live items no longer test whether their page is committed (`add` commits an item's memory and trimming never takes back a live range), and loops are versioned: when a loop's array indexes are its counter (or a variable it steps up to a limit) plus or minus a value the loop does not change, a test before the loop checks that every index stays in its array and saves the stored ranges to the undo log in one pass, and the loop then runs without per-access range and undo-log tests (the checked loop runs otherwise, and for loops of fewer than 32 steps). A/B against the previous compiler on the same core: Sieve 2.33 to 1.77 s, SHA256 1.02 to 0.86 s, Sort::Merge 1.93 to 1.74 s; Fannkuch, Quick, CRC32, GameOfLife within noise.
