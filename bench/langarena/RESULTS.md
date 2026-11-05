# LangArena results: Mitochondria, Rust and Crystal

Written by `run.py` (`python -I bench/langarena/run.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run (2026-11-05): a run of only some implementations (`--mito-only`) keeps the others' last numbers. How the comparison is made, and each port's audit, are in [README.md](README.md).

Machine: Windows 11, Intel64 Family 6 Model 170 Stepping 4, GenuineIntel. Run parameters (`run.js`), best of the rounds of a run, every checksum verified against LangArena's. Rust is LangArena's implementation (max-perf profile: opt-level 3, fat LTO, overflow checks off; the checked column turns them on, as Mitochondria always checks); Crystal is LangArena's original implementation. Ratios are Mitochondria / the column's implementation.

## Time (seconds for the timed iterations)

| Workload | Mitochondria | Rust | Mito/Rust | Rust (checked) | Mito/Rust (checked) | Crystal | Mito/Crystal | audit |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| Base64::Decode | 1.062 | 0.188 | 5.65x | 0.187 | 5.68x | 1.163 | 0.91x | closest-possible |
| Base64::Encode | 1.109 | 0.168 | 6.60x | 0.168 | 6.60x | 1.122 | 0.99x | closest-possible |
| BF::Array | 1.546 | 0.934 | 1.66x | 1.344 | 1.15x | 2.101 | 0.74x | rewritten-to-match |
| BF::Recursion | 2.090 | 1.355 | 1.54x | 1.458 | 1.43x | 1.484 | 1.41x | closest-possible |
| Binarytrees::Arena | 0.965 | 0.544 | 1.77x | 0.524 | 1.84x | 0.546 | 1.77x | like-for-like |
| Binarytrees::Obj | 1.987 | 3.016 | 0.66x | 3.024 | 0.66x | 1.492 | 1.33x | rewritten-to-match |
| Calculator::Ast | 0.198 | 0.668 | 0.30x | 0.663 | 0.30x | 0.415 | 0.48x | closest-possible |
| Calculator::Interpreter | 0.338 | 0.910 | 0.37x | 0.947 | 0.36x | 0.505 | 0.67x | closest-possible |
| CLBG::Fannkuchredux | 0.916 | 0.905 | 1.01x | 0.932 | 0.98x | 1.003 | 0.91x | like-for-like |
| CLBG::Mandelbrot | 0.688 | 0.706 | 0.97x | 0.715 | 0.96x | 0.689 | 1.00x | rewritten-to-match |
| CLBG::Nbody | 0.934 | 0.688 | 1.36x | 0.687 | 1.36x | 0.794 | 1.18x | closest-possible |
| CLBG::Spectralnorm | 0.750 | 1.075 | 0.70x | 1.083 | 0.69x | 0.744 | 1.01x | rewritten-to-match |
| Compress::ArithDecode | 0.969 | 1.234 | 0.79x | 1.213 | 0.80x | 0.903 | 1.07x | rewritten-to-match |
| Compress::ArithEncode | 1.049 | 0.887 | 1.18x | 0.948 | 1.11x | 1.107 | 0.95x | like-for-like |
| Compress::BWTDecode | 0.864 | 1.074 | 0.80x | 1.056 | 0.82x | 0.927 | 0.93x | rewritten-to-match |
| Compress::BWTEncode | 0.584 | 0.411 | 1.42x | 0.405 | 1.44x | 1.103 | 0.53x | closest-possible |
| Compress::HuffDecode | 0.900 | 0.627 | 1.44x | 0.628 | 1.43x | 0.908 | 0.99x | rewritten-to-match |
| Compress::HuffEncode | 0.605 | 0.596 | 1.01x | 0.607 | 1.00x | 0.927 | 0.65x | rewritten-to-match |
| Compress::LZWDecode | 0.174 | 1.158 | 0.15x | 1.137 | 0.15x | 0.527 | 0.33x | closest-possible |
| Compress::LZWEncode | 0.090 | 0.519 | 0.17x | 0.517 | 0.17x | 0.842 | 0.11x | closest-possible |
| CSV::Parse | 0.541 | 0.612 | 0.88x | 0.599 | 0.90x | 3.113 | 0.17x | closest-possible |
| Distance::Jaro | 0.933 | 0.952 | 0.98x | 0.959 | 0.97x | 1.144 | 0.82x | rewritten-to-match |
| Distance::NGram | 0.421 | 0.346 | 1.22x | 0.359 | 1.17x | 0.648 | 0.65x | rewritten-to-match |
| Etc::CacheSimulation | 0.443 | 0.813 | 0.55x | 0.808 | 0.55x | 2.007 | 0.22x | rewritten-to-match |
| Etc::GameOfLife | 1.527 | 1.922 | 0.79x | 1.924 | 0.79x | 1.812 | 0.84x | like-for-like |
| Etc::NeuralNet | 0.877 | 0.865 | 1.01x | 0.864 | 1.02x | 0.924 | 0.95x | closest-possible |
| Etc::Sieve | 1.760 | 1.304 | 1.35x | 1.784 | 0.99x | 1.963 | 0.90x | rewritten-to-match |
| Etc::TextRaytracer | 0.728 | 0.614 | 1.19x | 0.613 | 1.19x | 0.782 | 0.93x | rewritten-to-match |
| Etc::Words | 0.561 | 0.768 | 0.73x | 0.768 | 0.73x | 0.792 | 0.71x | closest-possible |
| Graph::AStar | 0.624 | 1.226 | 0.51x | 1.229 | 0.51x | 2.020 | 0.31x | rewritten-to-match |
| Graph::BFS | 0.960 | 0.817 | 1.17x | 0.826 | 1.16x | 1.203 | 0.80x | rewritten-to-match |
| Graph::DFS | 0.859 | 0.717 | 1.20x | 0.726 | 1.18x | 1.037 | 0.83x | rewritten-to-match |
| Hash::CRC32 | 0.799 | 0.838 | 0.95x | 0.832 | 0.96x | 0.797 | 1.00x | like-for-like |
| Hash::SHA256 | 0.644 | 0.664 | 0.97x | 0.676 | 0.95x | 0.662 | 0.97x | like-for-like |
| Matmul::Single | 3.227 | 3.335 | 0.97x | 3.359 | 0.96x | 3.153 | 1.02x | closest-possible |
| Maze::AStar | 0.875 | 1.091 | 0.80x | 1.135 | 0.77x | 4.011 | 0.22x | rewritten-to-match |
| Maze::BFS | 0.421 | 0.756 | 0.56x | 0.749 | 0.56x | 1.504 | 0.28x | rewritten-to-match |
| Maze::Generator | 0.496 | 0.735 | 0.68x | 0.742 | 0.67x | 1.428 | 0.35x | rewritten-to-match |
| Sort::Merge | 1.508 | 1.295 | 1.16x | 1.299 | 1.16x | 2.171 | 0.69x | rewritten-to-match |
| Sort::Quick | 1.350 | 1.112 | 1.21x | 1.160 | 1.16x | 1.549 | 0.87x | rewritten-to-match |
| Template::Parse | 0.596 | 0.870 | 0.69x | 0.852 | 0.70x | 1.823 | 0.33x | closest-possible |

Geometric mean over the 27 like-for-like and rewritten-to-match ports: Mitochondria / Rust 0.961x, / Rust (checked) 0.929x, / Crystal 0.730x; over all 41 ports, / Rust 0.928x.

## Memory (peak working set, MB)

The most physical memory each process held at once, measured by the operating system (Windows' peak working set). Rust and Crystal run one test per process; Mitochondria is its server process for that test, measured after prepare, warmup and the timed run (its connection threads and request areas included; reserved address space that was never touched is not counted).

| Workload | Mitochondria MB | Rust MB | Mito/Rust | Crystal MB | Mito/Crystal |
|---|---:|---:|---:|---:|---:|
| Base64::Decode | 7.0 | 5.9 | 1.19x | 13.3 | 0.53x |
| Base64::Encode | 6.8 | 6.3 | 1.09x | 13.4 | 0.51x |
| BF::Array | 5.2 | 4.8 | 1.10x | 10.7 | 0.49x |
| BF::Recursion | 5.2 | 4.8 | 1.10x | 10.7 | 0.49x |
| Binarytrees::Arena | 29.5 | 29.6 | 1.00x | 160.2 | 0.18x |
| Binarytrees::Obj | 37.5 | 69.4 | 0.54x | 174.8 | 0.21x |
| Calculator::Ast | 9.7 | 18.8 | 0.52x | 30.6 | 0.32x |
| Calculator::Interpreter | 34.4 | 47.7 | 0.72x | 45.3 | 0.76x |
| CLBG::Fannkuchredux | 5.2 | 4.8 | 1.08x | 9.8 | 0.53x |
| CLBG::Mandelbrot | 7.5 | 8.4 | 0.89x | 16.0 | 0.47x |
| CLBG::Nbody | 5.2 | 4.8 | 1.09x | 9.7 | 0.53x |
| CLBG::Spectralnorm | 5.2 | 4.8 | 1.10x | 10.7 | 0.49x |
| Compress::ArithDecode | 5.6 | 4.7 | 1.19x | 11.4 | 0.50x |
| Compress::ArithEncode | 5.5 | 4.8 | 1.14x | 11.2 | 0.49x |
| Compress::BWTDecode | 55.1 | 43.4 | 1.27x | 46.1 | 1.20x |
| Compress::BWTEncode | 7.7 | 8.3 | 0.93x | 15.5 | 0.50x |
| Compress::HuffDecode | 8.8 | 8.5 | 1.04x | 20.7 | 0.43x |
| Compress::HuffEncode | 7.6 | 5.8 | 1.30x | 18.4 | 0.41x |
| Compress::LZWDecode | 9.6 | 10.7 | 0.89x | 21.9 | 0.44x |
| Compress::LZWEncode | 5.8 | 5.1 | 1.14x | 15.0 | 0.38x |
| CSV::Parse | 20.5 | 16.3 | 1.26x | 38.2 | 0.54x |
| Distance::Jaro | 5.4 | 4.8 | 1.13x | 10.8 | 0.50x |
| Distance::NGram | 6.5 | 4.9 | 1.34x | 12.5 | 0.52x |
| Etc::CacheSimulation | 5.9 | 5.1 | 1.17x | 361.1 | 0.02x |
| Etc::GameOfLife | 42.8 | 46.9 | 0.91x | 59.6 | 0.72x |
| Etc::NeuralNet | 5.2 | 4.8 | 1.09x | 9.8 | 0.53x |
| Etc::Sieve | 8.0 | 7.1 | 1.13x | 18.4 | 0.44x |
| Etc::TextRaytracer | 5.2 | 4.8 | 1.09x | 9.8 | 0.53x |
| Etc::Words | 9.9 | 6.1 | 1.61x | 13.2 | 0.75x |
| Graph::AStar | 169.7 | 87.9 | 1.93x | 139.5 | 1.22x |
| Graph::BFS | 169.9 | 84.2 | 2.02x | 125.5 | 1.35x |
| Graph::DFS | 166.4 | 84.6 | 1.97x | 117.5 | 1.42x |
| Hash::CRC32 | 7.6 | 5.4 | 1.41x | 10.9 | 0.70x |
| Hash::SHA256 | 7.4 | 5.3 | 1.39x | 10.8 | 0.69x |
| Matmul::Single | 43.9 | 30.3 | 1.45x | 43.1 | 1.02x |
| Maze::AStar | 194.6 | 192.8 | 1.01x | 478.9 | 0.41x |
| Maze::BFS | 177.0 | 179.3 | 0.99x | 431.3 | 0.41x |
| Maze::Generator | 170.4 | 172.1 | 0.99x | 420.8 | 0.40x |
| Sort::Merge | 11.5 | 8.8 | 1.31x | 18.9 | 0.61x |
| Sort::Quick | 10.0 | 7.3 | 1.37x | 15.8 | 0.63x |
| Template::Parse | 47.4 | 37.5 | 1.27x | 63.3 | 0.75x |

Geometric mean over all 41 ports: Mitochondria / Rust 1.13x, / Crystal 0.50x.
