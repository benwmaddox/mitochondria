# Mitochondria for VS Code

Syntax highlighting for `.mito` programs: declarations (`struct`, `enum`, `state`, `request`, `fn`, `test`), endpoint attributes
(`@get` ...), loops and `match`, types, builtins, a test's requests, literals and comments; `//` comments and bracket pairs for editing.

Install from this folder: copy it into `~/.vscode/extensions/mitochondria` (on Windows `%USERPROFILE%\.vscode\extensions\mitochondria`)
and reload VS Code, or package it with `npx @vscode/vsce package` and install the `.vsix`.

Errors on save through `mito check`, and hover for inferred kinds, are the next steps (docs/roadmap.md, Phase 5).
