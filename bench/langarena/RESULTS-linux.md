# LangArena results: Mitochondria, Rust and Crystal (Linux)

Written by `run.py` (`python -I bench/langarena/run.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run (2026-11-05): a run of only some implementations (`--mito-only`) keeps the others' last numbers. How the comparison is made, and each port's audit, are in [README.md](README.md).

Machine: Linux 6.6.87.2-microsoft-standard-WSL2, x86_64. Run parameters (`run.js`), best of the rounds of a run, every checksum verified against LangArena's. Rust is LangArena's implementation (max-perf profile: opt-level 3, fat LTO, overflow checks off; the checked column turns them on, as Mitochondria always checks); Crystal is LangArena's original implementation. Ratios are Mitochondria / the column's implementation.

Each table is sorted by its Mito/Rust ratio, best first.

## Time (seconds for the timed iterations)

| Workload | Mitochondria | Rust | Mito/Rust | Rust (checked) | Mito/Rust (checked) | audit |
|---|---:|---:|---:|---:|---:|---|
| Compress::LZWEncode | 0.089 | 0.451 | 0.20x | 0.455 | 0.20x | closest-possible |
| Calculator::Ast | 0.180 | 0.491 | 0.37x | 0.495 | 0.36x | closest-possible |
| Compress::LZWDecode | 0.170 | 0.337 | 0.50x | 0.338 | 0.50x | closest-possible |
| Graph::AStar | 0.716 | 1.296 | 0.55x | 1.307 | 0.55x | rewritten-to-match |
| Maze::Generator | 0.478 | 0.755 | 0.63x | 0.752 | 0.64x | rewritten-to-match |
| Calculator::Interpreter | 0.353 | 0.550 | 0.64x | 0.550 | 0.64x | closest-possible |
| Template::Parse | 0.588 | 0.795 | 0.74x | 0.773 | 0.76x | closest-possible |
| Etc::CacheSimulation | 0.412 | 0.550 | 0.75x | 0.543 | 0.76x | rewritten-to-match |
| Etc::GameOfLife | 1.323 | 1.745 | 0.76x | 1.775 | 0.75x | like-for-like |
| Etc::Words | 0.537 | 0.704 | 0.76x | 0.708 | 0.76x | closest-possible |
| Maze::BFS | 0.486 | 0.611 | 0.80x | 0.607 | 0.80x | rewritten-to-match |
| Etc::NeuralNet | 0.647 | 0.812 | 0.80x | 0.788 | 0.82x | closest-possible |
| Compress::ArithDecode | 0.962 | 1.197 | 0.80x | 1.197 | 0.80x | rewritten-to-match |
| CLBG::Spectralnorm | 0.724 | 0.776 | 0.93x | 0.772 | 0.94x | rewritten-to-match |
| CLBG::Mandelbrot | 0.676 | 0.715 | 0.95x | 0.698 | 0.97x | rewritten-to-match |
| Distance::Jaro | 0.898 | 0.936 | 0.96x | 0.944 | 0.95x | rewritten-to-match |
| Compress::BWTDecode | 0.844 | 0.875 | 0.97x | 0.847 | 1.00x | rewritten-to-match |
| Hash::CRC32 | 0.806 | 0.819 | 0.98x | 0.779 | 1.04x | like-for-like |
| Maze::AStar | 0.969 | 0.945 | 1.03x | 0.848 | 1.14x | rewritten-to-match |
| Etc::TextRaytracer | 0.693 | 0.658 | 1.05x | 0.657 | 1.05x | rewritten-to-match |
| CSV::Parse | 0.541 | 0.513 | 1.06x | 0.522 | 1.04x | closest-possible |
| Compress::HuffEncode | 0.603 | 0.570 | 1.06x | 0.648 | 0.93x | rewritten-to-match |
| Hash::SHA256 | 0.668 | 0.630 | 1.06x | 0.637 | 1.05x | like-for-like |
| CLBG::Fannkuchredux | 1.013 | 0.939 | 1.08x | 0.984 | 1.03x | like-for-like |
| Matmul::Single | 3.455 | 3.196 | 1.08x | 3.179 | 1.09x | closest-possible |
| Sort::Merge | 1.417 | 1.278 | 1.11x | 1.288 | 1.10x | rewritten-to-match |
| Graph::BFS | 0.956 | 0.838 | 1.14x | 0.840 | 1.14x | rewritten-to-match |
| Binarytrees::Obj | 1.998 | 1.741 | 1.15x | 1.751 | 1.14x | rewritten-to-match |
| Compress::ArithEncode | 1.003 | 0.867 | 1.16x | 0.906 | 1.11x | like-for-like |
| Graph::DFS | 0.909 | 0.738 | 1.23x | 0.772 | 1.18x | rewritten-to-match |
| Distance::NGram | 0.385 | 0.312 | 1.24x | 0.328 | 1.18x | rewritten-to-match |
| Sort::Quick | 1.329 | 1.071 | 1.24x | 1.161 | 1.15x | rewritten-to-match |
| CLBG::Nbody | 0.891 | 0.703 | 1.27x | 0.701 | 1.27x | closest-possible |
| BF::Recursion | 1.960 | 1.410 | 1.39x | 1.429 | 1.37x | closest-possible |
| Etc::Sieve | 1.641 | 1.162 | 1.41x | 1.645 | 1.00x | rewritten-to-match |
| BF::Array | 1.405 | 0.985 | 1.43x | 1.396 | 1.01x | rewritten-to-match |
| Compress::BWTEncode | 0.460 | 0.303 | 1.52x | 0.303 | 1.52x | closest-possible |
| Compress::HuffDecode | 1.023 | 0.613 | 1.67x | 0.600 | 1.71x | rewritten-to-match |
| Binarytrees::Arena | 1.001 | 0.345 | 2.90x | 0.354 | 2.83x | like-for-like |
| Base64::Decode | 1.158 | 0.204 | 5.68x | 0.204 | 5.68x | closest-possible |
| Base64::Encode | 1.241 | 0.178 | 6.97x | 0.178 | 6.97x | closest-possible |

Geometric mean over the 27 like-for-like and rewritten-to-match ports: Mitochondria / Rust 1.054x, / Rust (checked) 1.019x; over all 41 ports, / Rust 1.041x.

## Memory (peak, MB)

The most physical memory each process held at once, measured by the operating system (Linux's peak resident set: VmHWM, or ru_maxrss for a process that ended). The reference implementations run one test per process; Mitochondria is its server process for that test, measured after prepare, warmup and the timed run (its connection threads and request areas included; reserved address space that was never touched is not counted).

| Workload | Mitochondria MB | Rust MB | Mito/Rust |
|---|---:|---:|---:|
| Etc::TextRaytracer | 2.1 | 20.6 | 0.10x |
| BF::Array | 2.1 | 20.5 | 0.10x |
| BF::Recursion | 2.1 | 20.5 | 0.10x |
| CLBG::Fannkuchredux | 2.1 | 20.5 | 0.10x |
| CLBG::Nbody | 2.1 | 20.5 | 0.10x |
| CLBG::Spectralnorm | 2.1 | 20.5 | 0.10x |
| Distance::Jaro | 2.2 | 20.6 | 0.11x |
| Etc::NeuralNet | 2.2 | 20.6 | 0.11x |
| Compress::ArithDecode | 2.2 | 20.5 | 0.11x |
| Compress::ArithEncode | 2.2 | 20.5 | 0.11x |
| Compress::LZWEncode | 2.4 | 20.6 | 0.12x |
| Hash::CRC32 | 3.3 | 24.2 | 0.13x |
| Hash::SHA256 | 3.3 | 24.2 | 0.13x |
| Etc::CacheSimulation | 2.9 | 20.6 | 0.14x |
| Distance::NGram | 3.3 | 20.6 | 0.16x |
| Base64::Decode | 3.3 | 20.5 | 0.16x |
| Base64::Encode | 3.3 | 20.5 | 0.16x |
| Compress::HuffEncode | 3.3 | 20.5 | 0.16x |
| Compress::BWTEncode | 4.1 | 20.5 | 0.20x |
| CLBG::Mandelbrot | 4.3 | 20.5 | 0.21x |
| Compress::HuffDecode | 4.3 | 20.5 | 0.21x |
| Sort::Quick | 5.2 | 24.2 | 0.21x |
| Etc::Sieve | 5.0 | 20.6 | 0.24x |
| Compress::LZWDecode | 5.2 | 20.5 | 0.25x |
| Etc::Words | 6.7 | 24.2 | 0.28x |
| Sort::Merge | 6.7 | 24.2 | 0.28x |
| Calculator::Ast | 5.8 | 20.5 | 0.29x |
| Calculator::Interpreter | 20.6 | 50.4 | 0.41x |
| Etc::GameOfLife | 20.3 | 45.2 | 0.45x |
| CSV::Parse | 9.5 | 20.6 | 0.46x |
| Maze::BFS | 86.8 | 179.3 | 0.48x |
| Maze::Generator | 84.6 | 170.6 | 0.50x |
| Binarytrees::Obj | 34.4 | 67.2 | 0.51x |
| Maze::AStar | 104.5 | 191.0 | 0.55x |
| Template::Parse | 24.4 | 38.2 | 0.64x |
| Compress::BWTDecode | 28.4 | 42.2 | 0.67x |
| Binarytrees::Arena | 26.5 | 27.3 | 0.97x |
| Matmul::Single | 27.2 | 27.8 | 0.98x |
| Graph::AStar | 103.5 | 81.6 | 1.27x |
| Graph::DFS | 100.2 | 78.4 | 1.28x |
| Graph::BFS | 103.6 | 78.1 | 1.33x |

Geometric mean over all 41 ports: Mitochondria / Rust 0.25x.
