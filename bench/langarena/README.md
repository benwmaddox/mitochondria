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

## Results

The latest numbers, time and peak memory for every port against Rust, Rust with overflow checks and Crystal, are in [RESULTS.md](RESULTS.md), which `run.py` writes from `results.json` at the end of every run (do not copy its table here: it goes stale). Headline, 2026-11-11 (run parameters, best of the rounds, all 41 ports, every checksum verified against LangArena's):

- Mitochondria / Rust over all 41 ports: 0.745x (over the 27 like-for-like and rewritten-to-match ports: 0.878x, and 0.850x against Rust with overflow checks on); / Crystal 0.538x.
- 29 ports are faster than Rust, 7 within +20%, 5 slower by more: Compress::HuffDecode 1.46x, BF::Array 1.32x, BF::Recursion 1.26x, Graph::DFS 1.22x.
- Total time of the timed iterations: Mitochondria 32.6 s, Rust 39.5 s, Crystal 54.0 s.
- Base64::Encode, Base64::Decode and Compress::BWTEncode call the language's library routines (AVX2 `base64`/`unbase64`, `array.sort`), so they compare a library with a library (their audit files say so); the earlier versions written as loops in the language are kept as `main_loop.mito` and `main_merge.mito`.

How the numbers got here, step by step (the first run had Sieve 4.93x of Rust, Fannkuchredux 2.84x, Sort::Merge 2.54x; state unlogged by default, ranges of variables and arrays, inline pool `add`, narrow bitwise operations, AVX2 base64 and the library sort moved it to 0.74x overall), with what each change was worth and the measurements behind it: [docs/measurements.md](../../docs/measurements.md). Crystal is LangArena's original implementation, 1.22.2 built with `crystal build main.cr --release` from a patched copy (the one JSON-library call, in a test that is not comparable, replaced by the standard library's), run with the same runner on the same core.

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
