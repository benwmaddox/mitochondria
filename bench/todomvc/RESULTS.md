# TodoMVC: Mitochondria against crudspec

Written by `bench.py` from `results.json` (2026-11-05, Linux 6.6.87.2-microsoft-standard-WSL2, best of 3 alternating rounds); do not edit by hand.

Both are TodoMVC rendered on the server, from this repository's `examples/todomvc` and crudspec's `apps/todomvc`, each server alone on one pinned CPU. The client submits each app's own forms (url-encoded for Mitochondria, multipart with a signed field for crudspec). Times are the client's round trip on one kept-open connection, in microseconds (Python's client adds its own share to both). Both keep the lists in SQLite on disk (WAL): Mitochondria as tables (`table<T>`), crudspec as its entities. By default Mitochondria syncs every commit to disk before answering (`--db-sync full`: an answered change survives a power cut); `--db-sync normal`, like crudspec's `synchronous=NORMAL`, syncs WAL commits at checkpoints (a process crash loses nothing, an OS crash or power cut can lose the last moments): the middle column is the like-for-like comparison. crudspec keeps a rendered page until its data changes, so its page time barely grows with the list; Mitochondria renders every page.

| Measure | Mitochondria (default, `--db-sync full`) | Mitochondria `--db-sync normal` | crudspec | full / crudspec | normal / crudspec |
|---|---:|---:|---:|---:|---:|
| 10 todos: page, median (us) | 97 | 98 | 95 | 1.01x | 1.02x |
| 10 todos: page, 99th percentile (us) | 183 | 170 | 173 | 1.06x | 0.99x |
| 10 todos: toggle a todo, median (us) | 1478 | 103 | 149 | 9.94x | 0.69x |
| 10 todos: toggle a todo, 99th percentile (us) | 2791 | 213 | 326 | 8.57x | 0.65x |
| 10 todos: pages per second, 4 clients | 32,891 | 34,813 | 36,335 | 0.91x | 0.96x |
| 10 todos: page size (bytes) | 7,447 | 7,447 | 11,650 | | |
| 100 todos: page, median (us) | 150 | 139 | 108 | 1.38x | 1.29x |
| 100 todos: page, 99th percentile (us) | 261 | 238 | 202 | 1.29x | 1.18x |
| 100 todos: toggle a todo, median (us) | 748 | 94 | 158 | 4.75x | 0.60x |
| 100 todos: toggle a todo, 99th percentile (us) | 1584 | 207 | 308 | 5.14x | 0.67x |
| 100 todos: pages per second, 4 clients | 13,294 | 14,498 | 29,809 | 0.45x | 0.49x |
| 100 todos: page size (bytes) | 63,794 | 63,794 | 96,166 | | |
| add a todo (100 to 300 todos), median (us) | 769 | 99 | 178 | 4.31x | 0.56x |
| add a todo (100 to 300 todos), 99th percentile (us) | 1615 | 219 | 303 | 5.33x | 0.72x |
| peak memory (MB, VmHWM) | 4.3 | 4.1 | 35.1 | 0.12x | 0.12x |
