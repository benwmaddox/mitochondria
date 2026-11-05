# LangArena results: Mitochondria, Rust and Crystal

Written by `run.py` (`python -I bench/langarena/run.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run (2026-11-05): a run of only some implementations (`--mito-only`) keeps the others' last numbers. How the comparison is made, and each port's audit, are in [README.md](README.md).

Machine: Windows 11, Intel64 Family 6 Model 170 Stepping 4, GenuineIntel. Run parameters (`run.js`), best of the rounds of a run, every checksum verified against LangArena's. Rust is LangArena's implementation (max-perf profile: opt-level 3, fat LTO, overflow checks off; the checked column turns them on, as Mitochondria always checks); Crystal is LangArena's original implementation. Ratios are Mitochondria / the column's implementation.

Each table is sorted by its Mito/Rust ratio, best first.

## Time (seconds for the timed iterations)

| Workload | Mitochondria | Rust | Mito/Rust | Rust (checked) | Mito/Rust (checked) | Crystal | Mito/Crystal | audit |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| Compress::LZWDecode | 0.172 | 1.158 | 0.15x | 1.137 | 0.15x | 0.527 | 0.33x | closest-possible |
| Compress::LZWEncode | 0.092 | 0.519 | 0.18x | 0.517 | 0.18x | 0.842 | 0.11x | closest-possible |
| Calculator::Ast | 0.181 | 0.668 | 0.27x | 0.663 | 0.27x | 0.415 | 0.44x | closest-possible |
| Calculator::Interpreter | 0.334 | 0.910 | 0.37x | 0.947 | 0.35x | 0.505 | 0.66x | closest-possible |
| Graph::AStar | 0.619 | 1.226 | 0.50x | 1.229 | 0.50x | 2.020 | 0.31x | rewritten-to-match |
| Etc::CacheSimulation | 0.411 | 0.813 | 0.51x | 0.808 | 0.51x | 2.007 | 0.21x | rewritten-to-match |
| Maze::BFS | 0.393 | 0.756 | 0.52x | 0.749 | 0.52x | 1.504 | 0.26x | rewritten-to-match |
| Maze::Generator | 0.441 | 0.735 | 0.60x | 0.742 | 0.59x | 1.428 | 0.31x | rewritten-to-match |
| Template::Parse | 0.587 | 0.870 | 0.68x | 0.852 | 0.69x | 1.823 | 0.32x | closest-possible |
| Binarytrees::Obj | 2.084 | 3.016 | 0.69x | 3.024 | 0.69x | 1.492 | 1.40x | rewritten-to-match |
| CLBG::Spectralnorm | 0.744 | 1.075 | 0.69x | 1.083 | 0.69x | 0.744 | 1.00x | rewritten-to-match |
| Etc::GameOfLife | 1.352 | 1.922 | 0.70x | 1.924 | 0.70x | 1.812 | 0.75x | like-for-like |
| Etc::Words | 0.554 | 0.768 | 0.72x | 0.768 | 0.72x | 0.792 | 0.70x | closest-possible |
| Compress::ArithDecode | 0.962 | 1.234 | 0.78x | 1.213 | 0.79x | 0.903 | 1.07x | rewritten-to-match |
| Maze::AStar | 0.886 | 1.091 | 0.81x | 1.135 | 0.78x | 4.011 | 0.22x | rewritten-to-match |
| Compress::BWTDecode | 0.879 | 1.074 | 0.82x | 1.056 | 0.83x | 0.927 | 0.95x | rewritten-to-match |
| Etc::NeuralNet | 0.757 | 0.865 | 0.88x | 0.864 | 0.88x | 0.924 | 0.82x | closest-possible |
| CSV::Parse | 0.550 | 0.612 | 0.90x | 0.599 | 0.92x | 3.113 | 0.18x | closest-possible |
| Distance::Jaro | 0.901 | 0.952 | 0.95x | 0.959 | 0.94x | 1.144 | 0.79x | rewritten-to-match |
| Hash::CRC32 | 0.815 | 0.838 | 0.97x | 0.832 | 0.98x | 0.797 | 1.02x | like-for-like |
| CLBG::Mandelbrot | 0.687 | 0.706 | 0.97x | 0.715 | 0.96x | 0.689 | 1.00x | rewritten-to-match |
| Hash::SHA256 | 0.656 | 0.664 | 0.99x | 0.676 | 0.97x | 0.662 | 0.99x | like-for-like |
| Matmul::Single | 3.309 | 3.335 | 0.99x | 3.359 | 0.99x | 3.153 | 1.05x | closest-possible |
| Compress::HuffEncode | 0.629 | 0.596 | 1.06x | 0.607 | 1.04x | 0.927 | 0.68x | rewritten-to-match |
| CLBG::Fannkuchredux | 0.981 | 0.905 | 1.08x | 0.932 | 1.05x | 1.003 | 0.98x | like-for-like |
| Sort::Merge | 1.470 | 1.295 | 1.14x | 1.299 | 1.13x | 2.171 | 0.68x | rewritten-to-match |
| Compress::ArithEncode | 1.010 | 0.887 | 1.14x | 0.948 | 1.07x | 1.107 | 0.91x | like-for-like |
| Etc::TextRaytracer | 0.700 | 0.614 | 1.14x | 0.613 | 1.14x | 0.782 | 0.90x | rewritten-to-match |
| Distance::NGram | 0.400 | 0.346 | 1.16x | 0.359 | 1.11x | 0.648 | 0.62x | rewritten-to-match |
| Graph::BFS | 0.970 | 0.817 | 1.19x | 0.826 | 1.17x | 1.203 | 0.81x | rewritten-to-match |
| Sort::Quick | 1.323 | 1.112 | 1.19x | 1.160 | 1.14x | 1.549 | 0.85x | rewritten-to-match |
| Graph::DFS | 0.894 | 0.717 | 1.25x | 0.726 | 1.23x | 1.037 | 0.86x | rewritten-to-match |
| CLBG::Nbody | 0.886 | 0.688 | 1.29x | 0.687 | 1.29x | 0.794 | 1.12x | closest-possible |
| Etc::Sieve | 1.694 | 1.304 | 1.30x | 1.784 | 0.95x | 1.963 | 0.86x | rewritten-to-match |
| BF::Recursion | 1.792 | 1.355 | 1.32x | 1.458 | 1.23x | 1.484 | 1.21x | closest-possible |
| Compress::BWTEncode | 0.558 | 0.411 | 1.36x | 0.405 | 1.38x | 1.103 | 0.51x | closest-possible |
| BF::Array | 1.340 | 0.934 | 1.43x | 1.344 | 1.00x | 2.101 | 0.64x | rewritten-to-match |
| Compress::HuffDecode | 0.929 | 0.627 | 1.48x | 0.628 | 1.48x | 0.908 | 1.02x | rewritten-to-match |
| Binarytrees::Arena | 0.982 | 0.544 | 1.80x | 0.524 | 1.87x | 0.546 | 1.80x | like-for-like |
| Base64::Decode | 1.080 | 0.188 | 5.74x | 0.187 | 5.77x | 1.163 | 0.93x | closest-possible |
| Base64::Encode | 1.186 | 0.168 | 7.06x | 0.168 | 7.06x | 1.122 | 1.06x | closest-possible |

Geometric mean over the 27 like-for-like and rewritten-to-match ports: Mitochondria / Rust 0.944x, / Rust (checked) 0.913x, / Crystal 0.717x; over all 41 ports, / Rust 0.909x.

## Memory (peak, MB)

The most physical memory each process held at once, measured by the operating system (Windows' peak working set). The reference implementations run one test per process; Mitochondria is its server process for that test, measured after prepare, warmup and the timed run (its connection threads and request areas included; reserved address space that was never touched is not counted).

| Workload | Mitochondria MB | Rust MB | Mito/Rust | Crystal MB | Mito/Crystal |
|---|---:|---:|---:|---:|---:|
| Calculator::Ast | 9.2 | 18.8 | 0.49x | 30.6 | 0.30x |
| Etc::GameOfLife | 23.5 | 46.9 | 0.50x | 59.6 | 0.39x |
| Maze::BFS | 90.2 | 179.3 | 0.50x | 431.3 | 0.21x |
| Calculator::Interpreter | 24.1 | 47.7 | 0.50x | 45.3 | 0.53x |
| Maze::Generator | 87.8 | 172.1 | 0.51x | 420.8 | 0.21x |
| Binarytrees::Obj | 37.5 | 69.4 | 0.54x | 174.8 | 0.21x |
| Maze::AStar | 107.8 | 192.8 | 0.56x | 478.9 | 0.23x |
| Compress::BWTDecode | 31.9 | 43.4 | 0.74x | 46.1 | 0.69x |
| Template::Parse | 27.6 | 37.5 | 0.74x | 63.3 | 0.44x |
| CSV::Parse | 12.6 | 16.3 | 0.77x | 38.2 | 0.33x |
| Compress::LZWDecode | 8.3 | 10.7 | 0.78x | 21.9 | 0.38x |
| Compress::HuffDecode | 7.5 | 8.5 | 0.88x | 20.7 | 0.36x |
| CLBG::Mandelbrot | 7.5 | 8.4 | 0.89x | 16.0 | 0.47x |
| Compress::BWTEncode | 7.6 | 8.3 | 0.92x | 15.5 | 0.49x |
| Matmul::Single | 30.2 | 30.3 | 1.00x | 43.1 | 0.70x |
| Binarytrees::Arena | 29.5 | 29.6 | 1.00x | 160.2 | 0.18x |
| Base64::Encode | 6.3 | 6.3 | 1.00x | 13.4 | 0.47x |
| Base64::Decode | 6.3 | 5.9 | 1.07x | 13.3 | 0.48x |
| Etc::TextRaytracer | 5.2 | 4.8 | 1.09x | 9.8 | 0.53x |
| CLBG::Fannkuchredux | 5.2 | 4.8 | 1.09x | 9.8 | 0.53x |
| Etc::NeuralNet | 5.2 | 4.8 | 1.09x | 9.8 | 0.53x |
| CLBG::Nbody | 5.2 | 4.8 | 1.09x | 9.7 | 0.53x |
| BF::Recursion | 5.2 | 4.8 | 1.10x | 10.7 | 0.49x |
| BF::Array | 5.2 | 4.8 | 1.10x | 10.7 | 0.49x |
| Compress::LZWEncode | 5.6 | 5.1 | 1.10x | 15.0 | 0.37x |
| CLBG::Spectralnorm | 5.2 | 4.8 | 1.10x | 10.7 | 0.49x |
| Distance::Jaro | 5.3 | 4.8 | 1.11x | 10.8 | 0.49x |
| Sort::Merge | 9.8 | 8.8 | 1.11x | 18.9 | 0.52x |
| Compress::HuffEncode | 6.5 | 5.8 | 1.11x | 18.4 | 0.35x |
| Compress::ArithEncode | 5.3 | 4.8 | 1.12x | 11.2 | 0.48x |
| Etc::Sieve | 8.0 | 7.1 | 1.13x | 18.4 | 0.44x |
| Sort::Quick | 8.3 | 7.3 | 1.13x | 15.8 | 0.52x |
| Compress::ArithDecode | 5.5 | 4.7 | 1.16x | 11.4 | 0.48x |
| Etc::CacheSimulation | 5.9 | 5.1 | 1.16x | 361.1 | 0.02x |
| Hash::CRC32 | 6.3 | 5.4 | 1.17x | 10.9 | 0.58x |
| Hash::SHA256 | 6.3 | 5.3 | 1.19x | 10.8 | 0.59x |
| Graph::AStar | 106.6 | 87.9 | 1.21x | 139.5 | 0.76x |
| Graph::DFS | 103.3 | 84.6 | 1.22x | 117.5 | 0.88x |
| Graph::BFS | 106.8 | 84.2 | 1.27x | 125.5 | 0.85x |
| Distance::NGram | 6.5 | 4.9 | 1.34x | 12.5 | 0.52x |
| Etc::Words | 9.9 | 6.1 | 1.61x | 13.2 | 0.75x |

Geometric mean over all 41 ports: Mitochondria / Rust 0.94x, / Crystal 0.42x.
