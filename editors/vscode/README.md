# Mitochondria for VS Code

Syntax highlighting for `.mito` programs: declarations (`struct`, `enum`, `state`, `request`, `fn`, `test`), endpoint attributes
(`@get` ...), loops and `match`, types, builtins, a test's requests, literals and comments; `//` comments and bracket pairs for editing.

Install from this folder: copy it into `~/.vscode/extensions/mitochondria` (on Windows `%USERPROFILE%\.vscode\extensions\mitochondria`)
and reload VS Code, or package it with `npx @vscode/vsce package` and install the `.vsix`.

Errors in the Problems panel: the `$mito` problem matcher reads `mito check` output (`error: file:line:col: message`). A task in
`.vscode/tasks.json` runs it on demand (bind it to a key, or run it on save with an extension such as "Run on Save"):

```json
{ "version": "2.0.0", "tasks": [ { "label": "mito check", "type": "shell", "command": "mito check ${file}", "problemMatcher": "$mito" } ] }
```

`mito check app.mito --kinds` lists every local's kind by function and endpoint; hover for kinds inside the editor is still to do.
