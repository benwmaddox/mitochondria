# TodoMVC, rendered on the server (design, 2026-11-05)

A working [TodoMVC](https://todomvc.com/app-spec) in Mitochondria: every page rendered on the server, every change an HTML form post.
Inspired by crudspec's `apps/todomvc` (github.com/benwmaddox/crudspec), without its DSL: the app is an ordinary Mitochondria program, and
what it needs is added to the language as general features.

## Decisions (Ben, 2026-11-05)

| Question | Decision |
| --- | --- |
| Whose list | One list per browser. A cookie holds the list's token; the browser's first todo creates the list. |
| Writing HTML | HTML literals in functions (`<li class={c}>{t.title}</li>`, with `foreach` and `if` inside), checked for well-formedness when compiling, escaped automatically, compiled to appends straight into the response. |
| Script | Pages and forms work with none. A tiny script (about 20 lines) adds double-click to edit, Escape to cancel, save on blur. |
| Persistence | In memory for now; persistence (docs/persistence.md) is its own step afterwards. |
| Pool of lists full | Refuse with a message on the page; nobody loses a list. |
| Filter addresses | `/`, `/active`, `/completed`. |
| Look | The official TodoMVC CSS (todomvc-app-css, MIT), vendored; the markup is what it expects. |
| Tests | crudspec's app-spec checks (e2e.mjs, Playwright) ported and run against the server. |
| Benchmark | Page render and action latency and memory against crudspec's Go build of the same app. |

## Behaviour

- `GET /`, `/active`, `/completed`: the whole page. Each form posts and the answer is `303 See Other` back to the page it came from.
- New todo: the title is trimmed; an empty one is ignored. Toggle one, toggle all (all complete, or all active again when all
  already are), delete, clear completed, "N item(s) left", filter links with the current one marked; main section and footer hidden
  with no todos.
- Editing: double-click (script) or the edit link `?edit=<id>` (no script) shows the row as a text field; Enter or blur saves,
  Escape cancels; an empty title deletes the todo.
- Fixed state: a declared number of lists, todos per list and title length. Past a limit the page says so (no silent loss).

## Language features it needs

1. HTML answers: HTML literals, automatic escaping, compiled to appends into the response (text in declared storage, no copies).
2. Form-encoded bodies (`application/x-www-form-urlencoded`) as typed inputs, as JSON bodies are now.
3. A redirect answer (`see_other(path)`, 303).
4. Cookies: an input `from cookie("todos")`, and setting one on an answer.
5. The stylesheet and script served (static text in the program, or a static directory).
