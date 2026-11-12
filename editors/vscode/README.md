# Mitochondria for VS Code

Syntax coloring for `.mito` programs, with standard TextMate scopes so any theme works:

- declarations: `struct`, `enum`, `module`, `state`, `request`, `instance`, `fn`, `pub`, `test`, `use "file";`, `platform web|cli|game|test;`,
  and the `system { ... }` and `web { ... }` blocks with their settings;
- attributes: `@get @post @put @patch @delete @socket @socket_message @socket_close @every @start @main @frame @shader`, with route
  strings and their `{param}` parts;
- input sources: `from header(...)`, `cookie(...)`, `query`, `body`, `arg(N)`, `stdin`, `time`, `key("w")`, `mouse("dx")`;
- types: `i8` to `i64`, `u8` to `u64`, `f32`, `f64`, `bool`, `string`, `utf8`, `ascii`, `text`, `html`, `pool`, `list`, `table`, `sorted`,
  `Response`, module types (`intmap<16>`), and struct/enum names; `checked`, `logged`, `drop_oldest`, `unique(...)`, `index(...)`;
- control flow and loops (`foreach ... in ... by after reverse where limit scan while`), `as`, ranges `lo..hi`, `self`, `fail`, `forward`;
- every built-in function and collection operation (`add`, `remove`, `move`, `sort`, ...), numbers (decimal, `0x` hex, floats),
  strings with their escapes (an unknown escape is marked invalid), `true false null`, and `//` comments;
- HTML markup in functions that return `html` (tags, attributes, entities, `<!doctype html>`), with `{expr}` inside markup and
  attribute values colored as code, and markup text left plain (so `//` or quotes in text are not mistaken for code).

The `--- program` test-case files (`tests/cases/*.case`) are not covered: the extension is for `.mito` files.

Install from this folder: copy it into `~/.vscode/extensions/mitochondria` (on Windows `%USERPROFILE%\.vscode\extensions\mitochondria`)
and reload VS Code, or package it with `npx @vscode/vsce package` and install the `.vsix`.

Errors in the Problems panel: the `$mito` problem matcher reads `mito check` output (`error: file:line:col: message`). A task in
`.vscode/tasks.json` runs it on demand (bind it to a key, or run it on save with an extension such as "Run on Save"):

```json
{ "version": "2.0.0", "tasks": [ { "label": "mito check", "type": "shell", "command": "mito check ${file}", "problemMatcher": "$mito" } ] }
```

`mito check app.mito --kinds` lists every local's kind by function and endpoint; hover for kinds inside the editor is still to do.

## Testing the grammar

`test/check.js` loads the grammar into the same engine VS Code uses (`vscode-textmate` with `vscode-oniguruma`), tokenizes
`test/sample.mito` (which uses every construct), and checks a table of (line, token, scope) expectations, that no region is left open
at the end of the sample, and that every program under `examples/` and `lib/` ends back at the root scope. Install the two packages in a
folder of your choice (not in the repository) and run it from the repository root:

```
mkdir /tmp/mito-tm && cd /tmp/mito-tm && npm init -y && npm install vscode-textmate vscode-oniguruma
node editors/vscode/test/check.js /tmp/mito-tm/node_modules
```

(The folder can also be given as the `MITO_TM_MODULES` environment variable.) It prints `ok: ...` and exits 0, or lists each failed check.
