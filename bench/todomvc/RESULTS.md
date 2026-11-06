# TodoMVC: Mitochondria against crudspec

Written by `bench.py` from `results.json` (2026-11-05, Linux 6.6.87.2-microsoft-standard-WSL2, best of 3 alternating rounds); do not edit by hand.

Both are TodoMVC rendered on the server, from this repository's `examples/todomvc` and crudspec's `apps/todomvc`, each server alone on one pinned CPU. The client submits each app's own forms (url-encoded for Mitochondria, multipart with a signed field for crudspec). Times are the client's round trip on one kept-open connection, in microseconds (Python's client adds its own share to both). Mitochondria keeps state in memory; crudspec in SQLite on disk: whole apps as built, not storage engines. crudspec keeps a rendered page until its data changes, so its page time barely grows with the list; Mitochondria renders every page.

| Measure | Mitochondria | crudspec | Mitochondria / crudspec |
|---|---:|---:|---:|
| 10 todos: page, median (us) | 90 | 102 | 0.88x |
| 10 todos: page, 99th percentile (us) | 157 | 199 | 0.79x |
| 10 todos: toggle a todo, median (us) | 73 | 165 | 0.45x |
| 10 todos: toggle a todo, 99th percentile (us) | 146 | 347 | 0.42x |
| 10 todos: pages per second, 4 clients | 37,761 | 34,847 | 1.08x |
| 10 todos: page size (bytes) | 7,441 | 11,650 | |
| 100 todos: page, median (us) | 124 | 104 | 1.19x |
| 100 todos: page, 99th percentile (us) | 200 | 188 | 1.06x |
| 100 todos: toggle a todo, median (us) | 73 | 152 | 0.48x |
| 100 todos: toggle a todo, 99th percentile (us) | 152 | 289 | 0.53x |
| 100 todos: pages per second, 4 clients | 19,524 | 28,728 | 0.68x |
| 100 todos: page size (bytes) | 63,782 | 96,166 | |
| add a todo (100 to 300 todos), median (us) | 80 | 181 | 0.44x |
| add a todo (100 to 300 todos), 99th percentile (us) | 169 | 334 | 0.50x |
| peak memory (MB, VmHWM) | 3.1 | 35.1 | 0.09x |
