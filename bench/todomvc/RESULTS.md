# TodoMVC: Mitochondria against crudspec

Written by `bench.py` from `results.json` (2026-11-05, Linux 6.6.87.2-microsoft-standard-WSL2, best of 3 alternating rounds); do not edit by hand.

Both are TodoMVC rendered on the server, from this repository's `examples/todomvc` and crudspec's `apps/todomvc`, each server alone on one pinned CPU. The client submits each app's own forms (url-encoded for Mitochondria, multipart with a signed field for crudspec). Times are the client's round trip on one kept-open connection, in microseconds (Python's client adds its own share to both). Both keep the lists in SQLite on disk (WAL): Mitochondria as tables (`table<T>`), crudspec as its entities; Mitochondria syncs every commit to disk before answering (`synchronous=FULL`: an answered change survives a power cut), crudspec runs `synchronous=NORMAL` (WAL commits not synced one by one: a process crash loses nothing, an OS crash or power cut can lose the last moments), so the changes' times measure that guarantee more than SQL. crudspec keeps a rendered page until its data changes, so its page time barely grows with the list; Mitochondria renders every page.

| Measure | Mitochondria | crudspec | Mitochondria / crudspec |
|---|---:|---:|---:|
| 10 todos: page, median (us) | 91 | 95 | 0.96x |
| 10 todos: page, 99th percentile (us) | 162 | 177 | 0.91x |
| 10 todos: toggle a todo, median (us) | 1471 | 149 | 9.89x |
| 10 todos: toggle a todo, 99th percentile (us) | 2592 | 328 | 7.89x |
| 10 todos: pages per second, 4 clients | 32,071 | 36,042 | 0.89x |
| 10 todos: page size (bytes) | 7,447 | 11,650 | |
| 100 todos: page, median (us) | 151 | 101 | 1.50x |
| 100 todos: page, 99th percentile (us) | 247 | 199 | 1.24x |
| 100 todos: toggle a todo, median (us) | 757 | 147 | 5.14x |
| 100 todos: toggle a todo, 99th percentile (us) | 1661 | 275 | 6.04x |
| 100 todos: pages per second, 4 clients | 13,071 | 30,111 | 0.43x |
| 100 todos: page size (bytes) | 63,794 | 96,166 | |
| add a todo (100 to 300 todos), median (us) | 780 | 170 | 4.58x |
| add a todo (100 to 300 todos), 99th percentile (us) | 1981 | 317 | 6.25x |
| peak memory (MB, VmHWM) | 4.3 | 35.2 | 0.12x |
