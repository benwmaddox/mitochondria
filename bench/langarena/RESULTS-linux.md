# LangArena results: Mitochondria, Rust and Crystal (Linux)

Written by `run.py` (`python -I bench/langarena/run.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run (2026-11-05): a run of only some implementations (`--mito-only`) keeps the others' last numbers. How the comparison is made, and each port's audit, are in [README.md](README.md).

Machine: Linux 6.6.87.2-microsoft-standard-WSL2, x86_64. Run parameters (`run.js`), best of the rounds of a run, every checksum verified against LangArena's. Rust is LangArena's implementation (max-perf profile: opt-level 3, fat LTO, overflow checks off; the checked column turns them on, as Mitochondria always checks); Crystal is LangArena's original implementation. Ratios are Mitochondria / the column's implementation.

Each table is sorted by its Mito/Rust ratio, best first.

## Time (seconds for the timed iterations)

| Workload | Mitochondria | Rust | Mito/Rust | Rust (checked) | Mito/Rust (checked) | audit |
|---|---:|---:|---:|---:|---:|---|
| Compress::LZWEncode | 0.087 | 0.451 | 0.19x | 0.455 | 0.19x | closest-possible |
| Calculator::Ast | 0.156 | 0.491 | 0.32x | 0.495 | 0.32x | closest-possible |
| Compress::LZWDecode | 0.173 | 0.337 | 0.51x | 0.338 | 0.51x | closest-possible |
| Graph::AStar | 0.681 | 1.296 | 0.53x | 1.307 | 0.52x | rewritten-to-match |
| Maze::Generator | 0.428 | 0.755 | 0.57x | 0.752 | 0.57x | rewritten-to-match |
| Etc::NeuralNet | 0.465 | 0.812 | 0.57x | 0.788 | 0.59x | closest-possible |
| Calculator::Interpreter | 0.352 | 0.550 | 0.64x | 0.550 | 0.64x | closest-possible |
| Maze::BFS | 0.422 | 0.611 | 0.69x | 0.607 | 0.70x | rewritten-to-match |
| Template::Parse | 0.570 | 0.795 | 0.72x | 0.773 | 0.74x | closest-possible |
| Etc::Words | 0.521 | 0.704 | 0.74x | 0.708 | 0.74x | closest-possible |
| Etc::CacheSimulation | 0.410 | 0.550 | 0.74x | 0.543 | 0.75x | rewritten-to-match |
| Compress::ArithDecode | 0.921 | 1.197 | 0.77x | 1.197 | 0.77x | rewritten-to-match |
| Etc::GameOfLife | 1.347 | 1.745 | 0.77x | 1.775 | 0.76x | like-for-like |
| Maze::AStar | 0.861 | 0.945 | 0.91x | 0.848 | 1.02x | rewritten-to-match |
| CLBG::Spectralnorm | 0.737 | 0.776 | 0.95x | 0.772 | 0.95x | rewritten-to-match |
| CLBG::Mandelbrot | 0.679 | 0.715 | 0.95x | 0.698 | 0.97x | rewritten-to-match |
| Distance::Jaro | 0.916 | 0.936 | 0.98x | 0.944 | 0.97x | rewritten-to-match |
| Hash::CRC32 | 0.803 | 0.819 | 0.98x | 0.779 | 1.03x | like-for-like |
| Compress::BWTDecode | 0.860 | 0.875 | 0.98x | 0.847 | 1.02x | rewritten-to-match |
| Hash::SHA256 | 0.650 | 0.630 | 1.03x | 0.637 | 1.02x | like-for-like |
| Compress::HuffEncode | 0.594 | 0.570 | 1.04x | 0.648 | 0.92x | rewritten-to-match |
| CSV::Parse | 0.536 | 0.513 | 1.04x | 0.522 | 1.03x | closest-possible |
| Matmul::Single | 3.355 | 3.196 | 1.05x | 3.179 | 1.06x | closest-possible |
| Etc::TextRaytracer | 0.694 | 0.658 | 1.06x | 0.657 | 1.06x | rewritten-to-match |
| Compress::ArithEncode | 0.928 | 0.867 | 1.07x | 0.906 | 1.02x | like-for-like |
| CLBG::Fannkuchredux | 1.025 | 0.939 | 1.09x | 0.984 | 1.04x | like-for-like |
| CLBG::Nbody | 0.774 | 0.703 | 1.10x | 0.701 | 1.10x | closest-possible |
| Sort::Merge | 1.429 | 1.278 | 1.12x | 1.288 | 1.11x | rewritten-to-match |
| Graph::BFS | 0.951 | 0.838 | 1.13x | 0.840 | 1.13x | rewritten-to-match |
| BF::Recursion | 1.701 | 1.410 | 1.21x | 1.429 | 1.19x | closest-possible |
| Distance::NGram | 0.380 | 0.312 | 1.22x | 0.328 | 1.16x | rewritten-to-match |
| Binarytrees::Obj | 2.139 | 1.741 | 1.23x | 1.751 | 1.22x | rewritten-to-match |
| Sort::Quick | 1.321 | 1.071 | 1.23x | 1.161 | 1.14x | rewritten-to-match |
| Graph::DFS | 0.911 | 0.738 | 1.23x | 0.772 | 1.18x | rewritten-to-match |
| BF::Array | 1.323 | 0.985 | 1.34x | 1.396 | 0.95x | rewritten-to-match |
| Compress::HuffDecode | 0.848 | 0.613 | 1.38x | 0.600 | 1.41x | rewritten-to-match |
| Etc::Sieve | 1.729 | 1.162 | 1.49x | 1.645 | 1.05x | rewritten-to-match |
| Compress::BWTEncode | 0.474 | 0.303 | 1.56x | 0.303 | 1.56x | closest-possible |
| Binarytrees::Arena | 0.994 | 0.345 | 2.88x | 0.354 | 2.81x | like-for-like |
| Base64::Decode | 1.174 | 0.204 | 5.76x | 0.204 | 5.76x | closest-possible |
| Base64::Encode | 1.282 | 0.178 | 7.20x | 0.178 | 7.20x | closest-possible |

Geometric mean over the 27 like-for-like and rewritten-to-match ports: Mitochondria / Rust 1.029x, / Rust (checked) 0.995x; over all 41 ports, / Rust 1.005x.

## Memory (peak, MB)

The most physical memory each process held at once, measured by the operating system (Linux's peak resident set: VmHWM, or ru_maxrss for a process that ended). The reference implementations run one test per process; Mitochondria is its server process for that test, measured after prepare, warmup and the timed run (its connection threads and request areas included; reserved address space that was never touched is not counted).

| Workload | Mitochondria MB | Rust MB | Mito/Rust |
|---|---:|---:|---:|
| BF::Array | 2.1 | 20.5 | 0.10x |
| BF::Recursion | 2.1 | 20.5 | 0.10x |
| CLBG::Fannkuchredux | 2.1 | 20.5 | 0.10x |
| CLBG::Nbody | 2.1 | 20.5 | 0.10x |
| CLBG::Spectralnorm | 2.1 | 20.5 | 0.10x |
| Distance::Jaro | 2.2 | 20.6 | 0.11x |
| Etc::NeuralNet | 2.2 | 20.6 | 0.11x |
| Etc::TextRaytracer | 2.2 | 20.6 | 0.11x |
| Compress::ArithDecode | 2.2 | 20.5 | 0.11x |
| Compress::ArithEncode | 2.2 | 20.5 | 0.11x |
| Compress::LZWEncode | 2.4 | 20.6 | 0.12x |
| Etc::CacheSimulation | 2.8 | 20.6 | 0.13x |
| Hash::CRC32 | 3.3 | 24.2 | 0.13x |
| Hash::SHA256 | 3.3 | 24.2 | 0.13x |
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
| Template::Parse | 24.6 | 38.2 | 0.64x |
| Compress::BWTDecode | 28.4 | 42.2 | 0.67x |
| Binarytrees::Arena | 26.5 | 27.3 | 0.97x |
| Matmul::Single | 27.2 | 27.8 | 0.98x |
| Graph::AStar | 103.5 | 81.6 | 1.27x |
| Graph::DFS | 100.2 | 78.4 | 1.28x |
| Graph::BFS | 103.6 | 78.1 | 1.33x |

Geometric mean over all 41 ports: Mitochondria / Rust 0.25x.
