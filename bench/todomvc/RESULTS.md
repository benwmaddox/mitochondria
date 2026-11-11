# TodoMVC: Mitochondria against crudspec

Written by `bench.py` from `results.json` (2026-11-11, Linux 6.6.87.2-microsoft-standard-WSL2, best of 3 alternating rounds); do not edit by hand.

Both are TodoMVC rendered on the server, from this repository's `examples/todomvc` and crudspec's `apps/todomvc`, each server alone on one pinned CPU. The client submits each app's own forms (url-encoded for Mitochondria, multipart with a signed field for crudspec). Times are the client's round trip on one kept-open connection, in microseconds (Python's client adds its own share to both). Both keep the lists in SQLite on disk (WAL): Mitochondria as tables (`table<T>`), crudspec as its entities. crudspec keeps a rendered page until its data changes, so its page time barely grows with the list; Mitochondria renders every page.

**The first three columns are the comparison that loses no confirmed data.** Both servers sync every commit to disk before answering: Mitochondria's default `--db-sync full` (SQLite `synchronous=FULL`) and crudspec built with `synchronous=FULL` (as shipped, its runtime hard-codes `synchronous=NORMAL`; the benchmark's build changes that one line in a temporary copy of its Go runtime, cs-full.sh). An answered change then survives a power cut. The last three columns are the faster, less safe setting for both (`--db-sync normal` and crudspec as shipped): WAL commits are synced at checkpoints, a process crash loses nothing, an OS crash or power cut can lose the last moments.

| Measure | Mitochondria (`full`) | crudspec (`FULL`) | Mito / crudspec | Mitochondria (`normal`) | crudspec (`NORMAL`, as shipped) | Mito / crudspec |
|---|---:|---:|---:|---:|---:|---:|
| 10 todos: page, median (us) | 100 | 105 | 0.95x | 99 | 105 | 0.94x |
| 10 todos: page, 99th percentile (us) | 174 | 190 | 0.92x | 185 | 204 | 0.91x |
| 10 todos: toggle a todo, median (us) | 1565 | 1631 | 0.96x | 110 | 168 | 0.65x |
| 10 todos: toggle a todo, 99th percentile (us) | 2028 | 2778 | 0.73x | 212 | 345 | 0.62x |
| 10 todos: pages per second, 4 clients | 32,355 | 32,975 | 0.98x | 32,330 | 33,011 | 0.98x |
| 10 todos: page size (bytes) | 7,447 | 11,650 | | 7,447 | 11,650 | |
| 100 todos: page, median (us) | 139 | 109 | 1.27x | 139 | 110 | 1.26x |
| 100 todos: page, 99th percentile (us) | 230 | 211 | 1.09x | 230 | 211 | 1.09x |
| 100 todos: toggle a todo, median (us) | 765 | 881 | 0.87x | 108 | 162 | 0.67x |
| 100 todos: toggle a todo, 99th percentile (us) | 1795 | 2330 | 0.77x | 209 | 285 | 0.73x |
| 100 todos: pages per second, 4 clients | 15,615 | 26,985 | 0.58x | 15,712 | 27,102 | 0.58x |
| 100 todos: page size (bytes) | 63,794 | 96,166 | | 63,794 | 96,166 | |
| add a todo (100 to 300 todos), median (us) | 793 | 914 | 0.87x | 112 | 202 | 0.55x |
| add a todo (100 to 300 todos), 99th percentile (us) | 1749 | 2083 | 0.84x | 229 | 366 | 0.63x |
| peak memory (MB, VmHWM) | 4.6 | 35.1 | 0.13x | 4.5 | 35.4 | 0.13x |
