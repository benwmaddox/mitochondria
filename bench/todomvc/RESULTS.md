# TodoMVC: Mitochondria against crudspec

Written by `bench.py` from `results.json` (2026-11-11, Linux 6.6.87.2-microsoft-standard-WSL2, best of 3 alternating rounds); do not edit by hand.

Both are TodoMVC rendered on the server, from this repository's `examples/todomvc` and crudspec's `apps/todomvc`, each server alone on one pinned CPU. The client submits each app's own forms (url-encoded for Mitochondria, multipart with a signed field for crudspec). Times are the client's round trip on one kept-open connection, in microseconds (Python's client adds its own share to both). Both keep the lists in SQLite on disk (WAL): Mitochondria as tables (`table<T>`), crudspec as its entities. By default Mitochondria syncs every commit to disk before answering (`--db-sync full`: an answered change survives a power cut); `--db-sync normal`, like crudspec's `synchronous=NORMAL`, syncs WAL commits at checkpoints (a process crash loses nothing, an OS crash or power cut can lose the last moments): the middle column is the like-for-like comparison. crudspec keeps a rendered page until its data changes, so its page time barely grows with the list; Mitochondria renders every page.

| Measure | Mitochondria (default, `--db-sync full`) | Mitochondria `--db-sync normal` | crudspec | full / crudspec | normal / crudspec |
|---|---:|---:|---:|---:|---:|
| 10 todos: page, median (us) | 99 | 99 | 105 | 0.94x | 0.95x |
| 10 todos: page, 99th percentile (us) | 177 | 173 | 204 | 0.87x | 0.85x |
| 10 todos: toggle a todo, median (us) | 1470 | 109 | 173 | 8.51x | 0.63x |
| 10 todos: toggle a todo, 99th percentile (us) | 1924 | 217 | 373 | 5.15x | 0.58x |
| 10 todos: pages per second, 4 clients | 32,551 | 32,063 | 32,868 | 0.99x | 0.98x |
| 10 todos: page size (bytes) | 7,447 | 7,447 | 11,650 | | |
| 100 todos: page, median (us) | 139 | 139 | 109 | 1.27x | 1.27x |
| 100 todos: page, 99th percentile (us) | 228 | 245 | 203 | 1.13x | 1.21x |
| 100 todos: toggle a todo, median (us) | 777 | 108 | 163 | 4.77x | 0.66x |
| 100 todos: toggle a todo, 99th percentile (us) | 1661 | 197 | 301 | 5.51x | 0.65x |
| 100 todos: pages per second, 4 clients | 15,708 | 15,676 | 27,174 | 0.58x | 0.58x |
| 100 todos: page size (bytes) | 63,794 | 63,794 | 96,166 | | |
| add a todo (100 to 300 todos), median (us) | 783 | 112 | 185 | 4.24x | 0.61x |
| add a todo (100 to 300 todos), 99th percentile (us) | 1767 | 241 | 403 | 4.39x | 0.60x |
| peak memory (MB, VmHWM) | 4.5 | 4.5 | 35.4 | 0.13x | 0.13x |
