# TodoMVC: Mitochondria against crudspec

Written by `bench.py` from `results.json` (2026-11-11, Linux 6.6.87.2-microsoft-standard-WSL2, best of 3 alternating rounds); do not edit by hand.

Both are TodoMVC rendered on the server, from this repository's `examples/todomvc` and crudspec's `apps/todomvc`, each server alone on one pinned CPU. The client submits each app's own forms (url-encoded for Mitochondria, multipart with a signed field for crudspec). Times are the client's round trip on one kept-open connection, in microseconds (Python's client adds its own share to both). Both keep the lists in SQLite on disk (WAL): Mitochondria as tables (`table<T>`), crudspec as its entities, and both sync the log at checkpoints by default (`--db-sync normal`: a crash of the process loses nothing; an OS crash or power cut can lose the last moments of answered changes).

Each cache mode is compared like for like:

- **none**: every page is rendered afresh (Mitochondria `TODOMVC_CACHE=none`, crudspec `CRUDSPEC_NO_CACHE=1`).
- **fragment**: the page is assembled from cached pieces and fresh ones (Mitochondria `TODOMVC_CACHE=fragment`: the list and its footer are kept in the answer cache under the list's version, the rest rendered each time; crudspec `CRUDSPEC_NO_PAGE_CACHE=1`: its fragment cache alone).
- **page**: the whole page is kept until its data changes (Mitochondria `TODOMVC_CACHE=page`, the example's default: kept under the list's version, which every change to the list moves, so another browser's changes do not touch it; crudspec's default: kept under the data version, so any commit starts its page cache again).

## No cache

| Measure | Mitochondria | crudspec | Mito / crudspec |
|---|---:|---:|---:|
| 10 todos: page, median (us) | 90 | 189 | 0.47x |
| 10 todos: page, 99th percentile (us) | 168 | 361 | 0.46x |
| 10 todos: toggle a todo, median (us) | 102 | 152 | 0.67x |
| 10 todos: toggle a todo, 99th percentile (us) | 195 | 285 | 0.69x |
| 10 todos: pages per second, 4 clients | 33,065 | 8,808 | 3.75x |
| 10 todos: page size (bytes) | 7,447 | 11,650 | |
| 100 todos: page, median (us) | 135 | 532 | 0.25x |
| 100 todos: page, 99th percentile (us) | 230 | 989 | 0.23x |
| 100 todos: toggle a todo, median (us) | 107 | 147 | 0.72x |
| 100 todos: toggle a todo, 99th percentile (us) | 215 | 290 | 0.74x |
| 100 todos: pages per second, 4 clients | 16,251 | 2,049 | 7.93x |
| 100 todos: page size (bytes) | 63,794 | 96,166 | |
| add a todo (100 to 300 todos), median (us) | 114 | 170 | 0.67x |
| add a todo (100 to 300 todos), 99th percentile (us) | 1544 | 324 | 4.77x |
| peak memory (MB, VmHWM) | 4.5 | 35.9 | 0.12x |

## Fragment cache (mixed: cached pieces and fresh ones)

| Measure | Mitochondria | crudspec | Mito / crudspec |
|---|---:|---:|---:|
| 10 todos: page, median (us) | 84 | 156 | 0.54x |
| 10 todos: page, 99th percentile (us) | 169 | 338 | 0.50x |
| 10 todos: toggle a todo, median (us) | 104 | 150 | 0.69x |
| 10 todos: toggle a todo, 99th percentile (us) | 264 | 262 | 1.01x |
| 10 todos: pages per second, 4 clients | 38,892 | 11,738 | 3.31x |
| 10 todos: page size (bytes) | 7,447 | 11,650 | |
| 100 todos: page, median (us) | 96 | 260 | 0.37x |
| 100 todos: page, 99th percentile (us) | 176 | 447 | 0.39x |
| 100 todos: toggle a todo, median (us) | 101 | 154 | 0.66x |
| 100 todos: toggle a todo, 99th percentile (us) | 203 | 280 | 0.73x |
| 100 todos: pages per second, 4 clients | 32,745 | 5,312 | 6.16x |
| 100 todos: page size (bytes) | 63,794 | 96,166 | |
| add a todo (100 to 300 todos), median (us) | 106 | 173 | 0.61x |
| add a todo (100 to 300 todos), 99th percentile (us) | 1329 | 291 | 4.56x |
| peak memory (MB, VmHWM) | 4.8 | 35.8 | 0.13x |

## Page cache

| Measure | Mitochondria | crudspec | Mito / crudspec |
|---|---:|---:|---:|
| 10 todos: page, median (us) | 83 | 105 | 0.80x |
| 10 todos: page, 99th percentile (us) | 170 | 197 | 0.86x |
| 10 todos: toggle a todo, median (us) | 103 | 168 | 0.61x |
| 10 todos: toggle a todo, 99th percentile (us) | 205 | 357 | 0.57x |
| 10 todos: pages per second, 4 clients | 36,609 | 34,039 | 1.08x |
| 10 todos: page size (bytes) | 7,447 | 11,650 | |
| 100 todos: page, median (us) | 104 | 100 | 1.04x |
| 100 todos: page, 99th percentile (us) | 183 | 196 | 0.94x |
| 100 todos: toggle a todo, median (us) | 110 | 148 | 0.75x |
| 100 todos: toggle a todo, 99th percentile (us) | 265 | 273 | 0.97x |
| 100 todos: pages per second, 4 clients | 30,512 | 29,157 | 1.05x |
| 100 todos: page size (bytes) | 63,794 | 96,166 | |
| add a todo (100 to 300 todos), median (us) | 115 | 177 | 0.65x |
| add a todo (100 to 300 todos), 99th percentile (us) | 1372 | 362 | 3.79x |
| peak memory (MB, VmHWM) | 4.8 | 35.4 | 0.14x |

## Page cache, every answered change on disk (`--db-sync full`)

Both sync every commit to disk before answering (SQLite `synchronous=FULL`): an answered change survives a power cut. Reads are as in the page-cache table; writes wait for the disk.

| Measure | Mitochondria | crudspec | Mito / crudspec |
|---|---:|---:|---:|
| 10 todos: page, median (us) | 84 | 95 | 0.88x |
| 10 todos: page, 99th percentile (us) | 165 | 188 | 0.88x |
| 10 todos: toggle a todo, median (us) | 1320 | 1446 | 0.91x |
| 10 todos: toggle a todo, 99th percentile (us) | 2014 | 2134 | 0.94x |
| 10 todos: pages per second, 4 clients | 39,513 | 34,670 | 1.14x |
| 10 todos: page size (bytes) | 7,447 | 11,650 | |
| 100 todos: page, median (us) | 96 | 100 | 0.96x |
| 100 todos: page, 99th percentile (us) | 182 | 191 | 0.96x |
| 100 todos: toggle a todo, median (us) | 705 | 811 | 0.87x |
| 100 todos: toggle a todo, 99th percentile (us) | 1698 | 1716 | 0.99x |
| 100 todos: pages per second, 4 clients | 32,771 | 29,325 | 1.12x |
| 100 todos: page size (bytes) | 63,794 | 96,166 | |
| add a todo (100 to 300 todos), median (us) | 708 | 887 | 0.80x |
| add a todo (100 to 300 todos), 99th percentile (us) | 2901 | 1901 | 1.53x |
| peak memory (MB, VmHWM) | 4.8 | 35.2 | 0.14x |


## Struct views, before and after (2026-11-12)

`cd62aba` (before) against `6dc5b9f` (after, main with struct views), the unchanged `examples/todomvc` built with each compiler, against one crudspec build (the existing `~/cs-todomvc-out/app`). Same `bench.py`, run as 4 alternating single-round runs (before, after, before, after, ...) under the bench lock; each cell is the median of the 4 runs, page-cache mode (the example's default). Mitochondria columns marked "noise" differ by less than the spread between runs on either side.

| Measure | Before | After | Difference | crudspec |
|---|---:|---:|---|---:|
| 10 todos: page, median (us) | 85 | 86 | noise | 101 |
| 10 todos: page, 99th percentile (us) | 182 | 176 | noise | 233 |
| 10 todos: toggle a todo, median (us) | 105 | 105 | noise | 171 |
| 10 todos: pages per second, 4 clients | 37,173 | 36,126 | noise | 32,561 |
| 100 todos: page, median (us) | 98 | 100 | noise | 104 |
| 100 todos: page, 99th percentile (us) | 207 | 212 | noise | 232 |
| 100 todos: toggle a todo, median (us) | 104 | 106 | noise | 160 |
| 100 todos: pages per second, 4 clients | 27,171 | 28,447 | noise | 27,380 |
| add a todo (100 to 300 todos), median (us) | 111 | 115 | noise | 189 |
| add a todo (100 to 300 todos), 99th percentile (us) | 1449 | 1547 | noise | 891 |
| peak memory (MB, VmHWM) | 4.8 | 4.8 | noise | 35.2 |

Struct views changed nothing measurable here: every one of the 13 measures in each of the four modes (none, fragment, page, page with `--db-sync full`) is within run-to-run spread, and peak memory is identical. The app needed no source change on the new compiler. Single runs are noisy (a few outliers in page mode, for example 16k pages per second once); medians of 4 absorb them.
