# LangArena results: Mitochondria, Rust and Crystal (Linux)

Written by `run.py` (`python -I bench/langarena/run.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run (2026-11-05): a run of only some implementations (`--mito-only`) keeps the others' last numbers. How the comparison is made, and each port's audit, are in [README.md](README.md).

Machine: Linux 6.6.87.2-microsoft-standard-WSL2, x86_64. Run parameters (`run.js`), best of the rounds of a run, every checksum verified against LangArena's. Rust is LangArena's implementation (max-perf profile: opt-level 3, fat LTO, overflow checks off; the checked column turns them on, as Mitochondria always checks); Crystal is LangArena's original implementation. Ratios are Mitochondria / the column's implementation.

Each table is sorted by its Mito/Rust ratio, best first.

## Time (seconds for the timed iterations)

| Workload | Mitochondria | Rust | Mito/Rust | Rust (checked) | Mito/Rust (checked) | audit |
|---|---:|---:|---:|---:|---:|---|
| Compress::LZWEncode | 0.092 | 0.451 | 0.20x | 0.455 | 0.20x | closest-possible |
| Calculator::Ast | 0.173 | 0.491 | 0.35x | 0.495 | 0.35x | closest-possible |
| Compress::LZWDecode | 0.199 | 0.337 | 0.59x | 0.338 | 0.59x | closest-possible |
| Graph::AStar | 0.786 | 1.296 | 0.61x | 1.307 | 0.60x | rewritten-to-match |
| Calculator::Interpreter | 0.357 | 0.550 | 0.65x | 0.550 | 0.65x | closest-possible |
| Maze::Generator | 0.515 | 0.755 | 0.68x | 0.752 | 0.68x | rewritten-to-match |
| Maze::BFS | 0.427 | 0.611 | 0.70x | 0.607 | 0.70x | rewritten-to-match |
| Template::Parse | 0.596 | 0.795 | 0.75x | 0.773 | 0.77x | closest-possible |
| Etc::Words | 0.560 | 0.704 | 0.80x | 0.708 | 0.79x | closest-possible |
| Etc::CacheSimulation | 0.451 | 0.550 | 0.82x | 0.543 | 0.83x | rewritten-to-match |
| Compress::ArithDecode | 0.990 | 1.197 | 0.83x | 1.197 | 0.83x | rewritten-to-match |
| CLBG::Mandelbrot | 0.686 | 0.715 | 0.96x | 0.698 | 0.98x | rewritten-to-match |
| Maze::AStar | 0.918 | 0.945 | 0.97x | 0.848 | 1.08x | rewritten-to-match |
| Compress::BWTDecode | 0.860 | 0.875 | 0.98x | 0.847 | 1.02x | rewritten-to-match |
| CLBG::Spectralnorm | 0.764 | 0.776 | 0.98x | 0.772 | 0.99x | rewritten-to-match |
| Hash::CRC32 | 0.820 | 0.819 | 1.00x | 0.779 | 1.05x | like-for-like |
| Etc::GameOfLife | 1.787 | 1.745 | 1.02x | 1.775 | 1.01x | like-for-like |
| Distance::Jaro | 0.959 | 0.936 | 1.03x | 0.944 | 1.02x | rewritten-to-match |
| Etc::NeuralNet | 0.834 | 0.812 | 1.03x | 0.788 | 1.06x | closest-possible |
| Hash::SHA256 | 0.667 | 0.630 | 1.06x | 0.637 | 1.05x | like-for-like |
| CSV::Parse | 0.547 | 0.513 | 1.07x | 0.522 | 1.05x | closest-possible |
| Matmul::Single | 3.448 | 3.196 | 1.08x | 3.179 | 1.08x | closest-possible |
| Etc::TextRaytracer | 0.734 | 0.658 | 1.12x | 0.657 | 1.12x | rewritten-to-match |
| Graph::BFS | 0.960 | 0.838 | 1.15x | 0.840 | 1.14x | rewritten-to-match |
| Sort::Merge | 1.472 | 1.278 | 1.15x | 1.288 | 1.14x | rewritten-to-match |
| CLBG::Fannkuchredux | 1.083 | 0.939 | 1.15x | 0.984 | 1.10x | like-for-like |
| Compress::ArithEncode | 1.035 | 0.867 | 1.19x | 0.906 | 1.14x | like-for-like |
| Binarytrees::Obj | 2.101 | 1.741 | 1.21x | 1.751 | 1.20x | rewritten-to-match |
| Graph::DFS | 0.923 | 0.738 | 1.25x | 0.772 | 1.20x | rewritten-to-match |
| Sort::Quick | 1.377 | 1.071 | 1.29x | 1.161 | 1.19x | rewritten-to-match |
| Distance::NGram | 0.406 | 0.312 | 1.30x | 0.328 | 1.24x | rewritten-to-match |
| CLBG::Nbody | 0.918 | 0.703 | 1.31x | 0.701 | 1.31x | closest-possible |
| Compress::HuffEncode | 0.746 | 0.570 | 1.31x | 0.648 | 1.15x | rewritten-to-match |
| BF::Recursion | 2.019 | 1.410 | 1.43x | 1.429 | 1.41x | closest-possible |
| Compress::BWTEncode | 0.460 | 0.303 | 1.52x | 0.303 | 1.52x | closest-possible |
| Etc::Sieve | 1.769 | 1.162 | 1.52x | 1.645 | 1.08x | rewritten-to-match |
| BF::Array | 1.528 | 0.985 | 1.55x | 1.396 | 1.09x | rewritten-to-match |
| Compress::HuffDecode | 1.060 | 0.613 | 1.73x | 0.600 | 1.77x | rewritten-to-match |
| Binarytrees::Arena | 1.021 | 0.345 | 2.96x | 0.354 | 2.88x | like-for-like |
| Base64::Decode | 1.252 | 0.204 | 6.14x | 0.204 | 6.14x | closest-possible |
| Base64::Encode | 1.380 | 0.178 | 7.75x | 0.178 | 7.75x | closest-possible |

Geometric mean over the 27 like-for-like and rewritten-to-match ports: Mitochondria / Rust 1.108x, / Rust (checked) 1.071x; over all 41 ports, / Rust 1.095x.

## Memory (peak, MB)

The most physical memory each process held at once, measured by the operating system (Linux's peak resident set: VmHWM, or ru_maxrss for a process that ended). The reference implementations run one test per process; Mitochondria is its server process for that test, measured after prepare, warmup and the timed run (its connection threads and request areas included; reserved address space that was never touched is not counted).

| Workload | Mitochondria MB | Rust MB | Mito/Rust |
|---|---:|---:|---:|
| CLBG::Fannkuchredux | 2.1 | 20.5 | 0.10x |
| Etc::TextRaytracer | 2.2 | 20.6 | 0.11x |
| BF::Array | 2.2 | 20.5 | 0.11x |
| BF::Recursion | 2.2 | 20.5 | 0.11x |
| CLBG::Nbody | 2.2 | 20.5 | 0.11x |
| CLBG::Spectralnorm | 2.2 | 20.5 | 0.11x |
| Distance::Jaro | 2.4 | 20.6 | 0.12x |
| Etc::NeuralNet | 2.4 | 20.6 | 0.12x |
| Compress::ArithEncode | 2.4 | 20.5 | 0.12x |
| Compress::LZWEncode | 2.6 | 20.6 | 0.12x |
| Compress::ArithDecode | 2.6 | 20.5 | 0.13x |
| Etc::CacheSimulation | 2.9 | 20.6 | 0.14x |
| Distance::NGram | 3.4 | 20.6 | 0.17x |
| Hash::SHA256 | 4.3 | 24.2 | 0.18x |
| Hash::CRC32 | 4.6 | 24.2 | 0.19x |
| Base64::Encode | 4.0 | 20.5 | 0.19x |
| Base64::Decode | 4.1 | 20.5 | 0.20x |
| CLBG::Mandelbrot | 4.3 | 20.5 | 0.21x |
| Compress::BWTEncode | 4.5 | 20.5 | 0.22x |
| Compress::HuffEncode | 4.6 | 20.5 | 0.23x |
| Etc::Sieve | 5.0 | 20.6 | 0.24x |
| Etc::Words | 6.7 | 24.2 | 0.28x |
| Sort::Quick | 6.9 | 24.2 | 0.28x |
| Compress::HuffDecode | 5.8 | 20.5 | 0.29x |
| Compress::LZWDecode | 6.4 | 20.5 | 0.31x |
| Calculator::Ast | 6.5 | 20.5 | 0.32x |
| Sort::Merge | 8.4 | 24.2 | 0.35x |
| Binarytrees::Obj | 34.4 | 67.2 | 0.51x |
| Calculator::Interpreter | 31.1 | 50.4 | 0.62x |
| CSV::Parse | 17.5 | 20.6 | 0.85x |
| Etc::GameOfLife | 39.9 | 45.2 | 0.88x |
| Maze::BFS | 173.8 | 179.3 | 0.97x |
| Binarytrees::Arena | 26.6 | 27.3 | 0.98x |
| Maze::Generator | 167.2 | 170.6 | 0.98x |
| Maze::AStar | 191.5 | 191.0 | 1.00x |
| Template::Parse | 44.3 | 38.2 | 1.16x |
| Compress::BWTDecode | 51.7 | 42.2 | 1.23x |
| Matmul::Single | 40.7 | 27.8 | 1.46x |
| Graph::AStar | 166.7 | 81.6 | 2.04x |
| Graph::DFS | 163.5 | 78.4 | 2.09x |
| Graph::BFS | 166.9 | 78.1 | 2.14x |

Geometric mean over all 41 ports: Mitochondria / Rust 0.33x.
