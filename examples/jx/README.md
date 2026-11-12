# jx: a small jq, as a command-line program

`jx` reads JSON on standard input and a filter on the command line, and writes what the filter selects. It is a
Mitochondria program that is not a server: its entry is `@main`, and its command line is its request.

```
echo '{"users":[{"n":"ann","age":31},{"n":"bo","age":17}]}' | mito run examples/jx/jx.mito -- '.users[] | select(.age >= 18) | .n'
"ann"
```

`mito build examples/jx/jx.mito -o jx.exe` makes a standalone executable: `jx.exe -c '.users[0]' < data.json`.

| File | |
| --- | --- |
| `jx.mito` | the state and `@main`: the filter is `arg(0)`, the JSON is `stdin`, `-c` and `-r` are flags |
| `json.mito` | reading JSON where it lies (find where a value ends, look up a field or an element) and writing it, compact or indented |
| `filter.mito` | the filter language, evaluated straight from its text |
| `e2e.sh` | checks of the filters, flags, errors and exit codes against a built `jx` |

## The filters

| | |
| --- | --- |
| `.` | the input, indented (`-c` for one line) |
| `.name`, `."odd key"`, `.[2]`, `.a[0].b` | a field or an element; null when absent |
| `.[]` | each element of an array, or each value of an object; what follows `\|` runs once for each |
| `select(.age >= 18)` | keep the value if the condition holds: a path alone (not null or false) or `path OP literal`, OP one of `==` `!=` `<` `>` `<=` `>=`, a literal a number, a `"string"`, `true`, `false` or `null` |
| `length` | elements, fields, or characters of a string |
| `type` | `"object"`, `"array"`, `"string"`, `"number"`, `"boolean"` or `"null"` |
| `a \| b` | the results of `a`, each given to `b` |

`-c` writes each result on one line; `-r` writes a string result without its quotes. Errors go to standard error with exit code 1.

Not yet: `map`, `keys`, arithmetic, object and array construction, `//`, `,`, variables. `length` and `type` make a new value, so they must be
the last step. Numbers are compared as integers, and `==` compares the written form (`1.0` is not `1`).

## What it shows

- **A program that is not a server.** `@main fn main(filter: utf8<1..1000> from arg(0), doc: utf8<4294967295> from stdin, c: bool = false, ...)`:
  the Nth argument is `from arg(N)`, what is piped in is `from stdin`, and any other input is a flag (`-c`, `--name value`). The answer's
  body goes to standard output; an answer with status 400 or more goes to standard error, and the exit code is 1.
- **Fixed memory, no allocation.** The input and the output are each declared at 4294967295 bytes (4 GiB - 1: a length is 32 bits, so that is the most Mitochondria can hold; a longer input stops with "the input is larger than 4294967295 bytes"). The memory is reserved address space, not committed: a 1 KB input commits a few MB, and a 500 MB input a bit over twice its size (the input as read, and its copy in the request text area, which the program works on). Both the copy and the output sit in a text area of 4 GiB - 1 bytes, so the real limit of the input is a few KB under 4 GiB - 1, less the other text a run builds. `out` is declared last in `J`: a field after it would start beyond 4 GiB, where offsets do not reach. A value is a position in the input
  text; nothing is parsed into a tree, and the filter is not parsed either: `eval` walks its text.
- **Several files, as in a web program** (`use "json.mito";`).
