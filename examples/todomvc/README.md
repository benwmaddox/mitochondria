# TodoMVC in Mitochondria

[TodoMVC](https://todomvc.com/app-spec) rendered on the server: every page is HTML written in the program, every change a form
post answered with a redirect back to the page it came from. Each browser has its own list (a cookie names it), as the
reference implementations do with local storage. Decisions behind it: [docs/todomvc.md](../../docs/todomvc.md).

| File | |
| --- | --- |
| `todomvc.mito` | state, pages (markup), changes |
| `assets/app.js` | the only script: a ticked checkbox submits, double-click edits, Enter or blur saves, Escape cancels |
| `assets/todomvc.css` | [todomvc-app-css](https://github.com/tastejs/todomvc-app-css) 2.4.3, CC-BY-4.0 (c) Sindre Sorhus (`todomvc.css.LICENSE`) |
| `e2e.mjs` | the app spec's behaviours in a browser, with script and again without it: 53 checks |

```
mito build examples/todomvc/todomvc.mito -o todomvc.exe
todomvc.exe --port 4301 --data ./todo-data
node examples/todomvc/e2e.mjs http://127.0.0.1:4301
```

The stylesheet and script are compiled into the program (`file(...)`), served with an ETag. Without script every page and
form still works: checkboxes get a Done button, and each todo an Edit link (`?edit=<id>`).

The lists are tables (`table<Board>`, each board's `table<Todo>`), kept in SQLite in the `--data` directory (default `./data`):
they survive restarts, and any SQLite tool can read them while the server runs. Titles are at most 1000 bytes; tables have no
fixed number of rows.

Caching: a page, or just its list and footer, is kept in the answer cache under the list's version (`Board.version`, which every change to the list moves, so another
browser's changes do not touch it). `TODOMVC_CACHE=none|fragment|page` chooses at start what is kept (default `page`); the browser tests (`e2e.mjs`) pass in all three modes.
`bench/todomvc` compares each mode with crudspec's equivalent, and both servers with `--db-sync normal` (the default: the log is synced at checkpoints) and `full` (every commit
synced before the answer).

Differences from the app spec: filters are the pages `/active` and `/completed` rather than `#/active`; every change reloads
the page.
