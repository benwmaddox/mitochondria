# LangArena results: Mitochondria, Rust and Crystal

Written by `run.py` (`python -I bench/langarena/run.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run (2026-11-11): a run of only some implementations (`--mito-only`) keeps the others' last numbers. How the comparison is made, and each port's audit, are in [README.md](README.md).

Machine: Windows 11, Intel64 Family 6 Model 170 Stepping 4, GenuineIntel. Run parameters (`run.js`), best of the rounds of a run, every checksum verified against LangArena's. Rust is LangArena's implementation (max-perf profile: opt-level 3, fat LTO, overflow checks off; the checked column turns them on, as Mitochondria always checks); Crystal is LangArena's original implementation. Ratios are Mitochondria / the column's implementation.

Each table is sorted by its Mito/Rust ratio, best first.

## Time (seconds for the timed iterations)

| Workload | Mitochondria | Rust | Mito/Rust | Rust (checked) | Mito/Rust (checked) | Crystal | Mito/Crystal | audit |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| Compress::LZWDecode | 0.082 | 1.172 | 0.07x | 1.164 | 0.07x | 0.521 | 0.16x | closest-possible |
| Compress::LZWEncode | 0.088 | 0.523 | 0.17x | 0.518 | 0.17x | 0.877 | 0.10x | closest-possible |
| Calculator::Ast | 0.154 | 0.676 | 0.23x | 0.675 | 0.23x | 0.406 | 0.38x | closest-possible |
| Calculator::Interpreter | 0.323 | 0.885 | 0.37x | 0.915 | 0.35x | 0.508 | 0.64x | closest-possible |
| Etc::CacheSimulation | 0.355 | 0.810 | 0.44x | 0.803 | 0.44x | 1.868 | 0.19x | rewritten-to-match |
| Graph::AStar | 0.635 | 1.258 | 0.50x | 1.268 | 0.50x | 2.076 | 0.31x | rewritten-to-match |
| Etc::GameOfLife | 1.062 | 1.977 | 0.54x | 2.004 | 0.53x | 1.843 | 0.58x | like-for-like |
| Maze::BFS | 0.428 | 0.771 | 0.56x | 0.764 | 0.56x | 1.517 | 0.28x | rewritten-to-match |
| Template::Parse | 0.559 | 0.861 | 0.65x | 0.862 | 0.65x | 1.817 | 0.31x | closest-possible |
| Etc::NeuralNet | 0.557 | 0.858 | 0.65x | 0.858 | 0.65x | 0.919 | 0.61x | closest-possible |
| Etc::Words | 0.508 | 0.779 | 0.65x | 0.774 | 0.66x | 0.799 | 0.64x | closest-possible |
| Compress::BWTEncode | 0.280 | 0.423 | 0.66x | 0.424 | 0.66x | 1.101 | 0.25x | closest-possible |
| Maze::Generator | 0.525 | 0.756 | 0.69x | 0.757 | 0.69x | 1.449 | 0.36x | rewritten-to-match |
| CLBG::Spectralnorm | 0.755 | 1.083 | 0.70x | 1.086 | 0.69x | 0.749 | 1.01x | rewritten-to-match |
| Binarytrees::Obj | 2.113 | 3.018 | 0.70x | 2.999 | 0.70x | 1.510 | 1.40x | rewritten-to-match |
| Compress::ArithDecode | 0.927 | 1.244 | 0.75x | 1.229 | 0.75x | 0.911 | 1.02x | rewritten-to-match |
| Base64::Decode | 0.146 | 0.185 | 0.79x | 0.185 | 0.79x | 1.147 | 0.13x | closest-possible |
| Maze::AStar | 0.883 | 1.113 | 0.79x | 1.146 | 0.77x | 4.066 | 0.22x | rewritten-to-match |
| Compress::BWTDecode | 0.835 | 1.031 | 0.81x | 1.033 | 0.81x | 0.926 | 0.90x | rewritten-to-match |
| Base64::Encode | 0.143 | 0.166 | 0.86x | 0.166 | 0.86x | 1.119 | 0.13x | closest-possible |
| Binarytrees::Arena | 0.479 | 0.551 | 0.87x | 0.525 | 0.91x | 0.567 | 0.85x | like-for-like |
| CSV::Parse | 0.532 | 0.608 | 0.87x | 0.585 | 0.91x | 3.119 | 0.17x | closest-possible |
| Matmul::Single | 3.007 | 3.304 | 0.91x | 3.332 | 0.90x | 3.128 | 0.96x | closest-possible |
| Etc::Sieve | 1.264 | 1.328 | 0.95x | 1.799 | 0.70x | 1.983 | 0.64x | rewritten-to-match |
| Distance::Jaro | 0.909 | 0.954 | 0.95x | 0.972 | 0.94x | 1.151 | 0.79x | rewritten-to-match |
| CLBG::Mandelbrot | 0.679 | 0.712 | 0.95x | 0.727 | 0.93x | 0.693 | 0.98x | rewritten-to-match |
| Hash::SHA256 | 0.650 | 0.663 | 0.98x | 0.664 | 0.98x | 0.652 | 1.00x | like-for-like |
| Hash::CRC32 | 0.815 | 0.826 | 0.99x | 0.827 | 0.99x | 0.799 | 1.02x | like-for-like |
| Compress::HuffEncode | 0.595 | 0.596 | 1.00x | 0.617 | 0.96x | 0.918 | 0.65x | rewritten-to-match |
| Distance::NGram | 0.373 | 0.352 | 1.06x | 0.360 | 1.04x | 0.623 | 0.60x | rewritten-to-match |
| CLBG::Fannkuchredux | 0.989 | 0.912 | 1.08x | 0.927 | 1.07x | 1.023 | 0.97x | like-for-like |
| Compress::ArithEncode | 0.973 | 0.896 | 1.09x | 0.954 | 1.02x | 1.110 | 0.88x | like-for-like |
| Sort::Merge | 1.411 | 1.299 | 1.09x | 1.298 | 1.09x | 2.179 | 0.65x | rewritten-to-match |
| Etc::TextRaytracer | 0.701 | 0.618 | 1.13x | 0.618 | 1.13x | 0.790 | 0.89x | rewritten-to-match |
| Graph::BFS | 0.953 | 0.832 | 1.15x | 0.842 | 1.13x | 1.222 | 0.78x | rewritten-to-match |
| Sort::Quick | 1.325 | 1.123 | 1.18x | 1.158 | 1.14x | 1.553 | 0.85x | rewritten-to-match |
| CLBG::Nbody | 0.837 | 0.695 | 1.21x | 0.698 | 1.20x | 0.798 | 1.05x | closest-possible |
| Graph::DFS | 0.879 | 0.718 | 1.22x | 0.732 | 1.20x | 1.044 | 0.84x | rewritten-to-match |
| BF::Recursion | 1.687 | 1.334 | 1.26x | 1.456 | 1.16x | 1.466 | 1.15x | closest-possible |
| BF::Array | 1.216 | 0.922 | 1.32x | 1.327 | 0.92x | 2.096 | 0.58x | rewritten-to-match |
| Compress::HuffDecode | 0.927 | 0.634 | 1.46x | 0.632 | 1.47x | 0.913 | 1.02x | rewritten-to-match |

Geometric mean over the 27 like-for-like and rewritten-to-match ports: Mitochondria / Rust 0.886x, / Rust (checked) 0.858x, / Crystal 0.676x; over all 41 ports, / Rust 0.745x.

## Memory (peak, MB)

The most physical memory each process held at once, measured by the operating system (Windows' peak working set). The reference implementations run one test per process; Mitochondria is its server process for that test, measured after prepare, warmup and the timed run (its connection threads and request areas included; reserved address space that was never touched is not counted).

| Workload | Mitochondria MB | Rust MB | Mito/Rust | Crystal MB | Mito/Crystal |
|---|---:|---:|---:|---:|---:|
| Calculator::Interpreter | 20.8 | 47.7 | 0.44x | 45.3 | 0.46x |
| Maze::Generator | 83.8 | 172.0 | 0.49x | 420.8 | 0.20x |
| Etc::GameOfLife | 22.9 | 46.9 | 0.49x | 59.6 | 0.39x |
| Maze::BFS | 88.7 | 180.2 | 0.49x | 431.2 | 0.21x |
| Calculator::Ast | 9.3 | 18.5 | 0.50x | 30.6 | 0.30x |
| Binarytrees::Obj | 37.5 | 69.3 | 0.54x | 174.8 | 0.21x |
| Maze::AStar | 106.4 | 192.8 | 0.55x | 478.8 | 0.22x |
| Compress::BWTDecode | 31.5 | 43.4 | 0.73x | 46.1 | 0.68x |
| Template::Parse | 27.3 | 37.5 | 0.73x | 63.3 | 0.43x |
| CSV::Parse | 12.5 | 16.3 | 0.77x | 38.2 | 0.33x |
| Compress::LZWDecode | 8.4 | 10.7 | 0.78x | 22.9 | 0.37x |
| Compress::HuffDecode | 7.5 | 8.5 | 0.89x | 20.6 | 0.36x |
| CLBG::Mandelbrot | 7.5 | 8.4 | 0.90x | 15.9 | 0.47x |
| Compress::BWTEncode | 8.1 | 8.3 | 0.97x | 15.4 | 0.52x |
| Matmul::Single | 30.0 | 30.3 | 0.99x | 43.1 | 0.70x |
| Binarytrees::Arena | 29.6 | 29.7 | 0.99x | 160.2 | 0.18x |
| Base64::Encode | 6.9 | 6.3 | 1.09x | 13.4 | 0.51x |
| Compress::ArithEncode | 5.4 | 4.9 | 1.09x | 11.3 | 0.48x |
| Etc::NeuralNet | 5.2 | 4.8 | 1.09x | 9.7 | 0.54x |
| Etc::TextRaytracer | 5.2 | 4.8 | 1.10x | 9.7 | 0.54x |
| CLBG::Fannkuchredux | 5.2 | 4.8 | 1.10x | 9.7 | 0.54x |
| CLBG::Nbody | 5.2 | 4.8 | 1.10x | 9.7 | 0.54x |
| Compress::LZWEncode | 5.6 | 5.1 | 1.10x | 14.6 | 0.38x |
| BF::Recursion | 5.3 | 4.8 | 1.10x | 10.7 | 0.49x |
| CLBG::Spectralnorm | 5.3 | 4.8 | 1.10x | 10.7 | 0.49x |
| BF::Array | 5.3 | 4.8 | 1.11x | 10.7 | 0.49x |
| Sort::Merge | 9.8 | 8.8 | 1.11x | 18.9 | 0.52x |
| Etc::CacheSimulation | 5.6 | 5.0 | 1.11x | 368.7 | 0.02x |
| Distance::Jaro | 5.3 | 4.8 | 1.11x | 10.7 | 0.50x |
| Compress::HuffEncode | 6.5 | 5.8 | 1.12x | 18.4 | 0.35x |
| Sort::Quick | 8.3 | 7.3 | 1.13x | 15.8 | 0.52x |
| Etc::Sieve | 8.1 | 7.1 | 1.14x | 18.3 | 0.44x |
| Compress::ArithDecode | 5.5 | 4.7 | 1.16x | 11.4 | 0.49x |
| Hash::CRC32 | 6.4 | 5.4 | 1.17x | 10.8 | 0.59x |
| Hash::SHA256 | 6.3 | 5.3 | 1.18x | 10.8 | 0.58x |
| Graph::AStar | 105.0 | 87.8 | 1.20x | 139.4 | 0.75x |
| Graph::DFS | 101.7 | 84.6 | 1.20x | 117.5 | 0.87x |
| Distance::NGram | 6.0 | 4.9 | 1.23x | 12.5 | 0.48x |
| Graph::BFS | 105.2 | 84.2 | 1.25x | 125.5 | 0.84x |
| Etc::Words | 8.1 | 6.2 | 1.31x | 13.1 | 0.62x |
| Base64::Decode | 8.6 | 6.1 | 1.41x | 13.3 | 0.65x |

Geometric mean over all 41 ports: Mitochondria / Rust 0.93x, / Crystal 0.42x.
