# Notes: a web app on a database, in several files

A small multi-user notebook: sign up with an email and a password, write notes with tags, search them by title or tag.
Every page is HTML written in the program and every change is a form post answered with a redirect, so it works without script.

| File | Concern |
| --- | --- |
| `notes.mito` | the state (two tables) and the stylesheet route; `use`s the rest |
| `accounts.mito` | sign up, sign in, sign out |
| `notebook.mito` | the notes: list and search, open, add, save, delete |
| `pages.mito` | the markup every page is made of |
| `assets/style.css` | compiled into the program with `file(...)` |
| `e2e.mjs` | 22 checks over HTTP: accounts, notes, search, privacy between users, sign-out |

```
mito build examples/notes/notes.mito -o notes.exe
notes.exe --port 4311 --data ./notes-data
node examples/notes/e2e.mjs http://127.0.0.1:4311
```

## What it shows

- **Several files, one program.** `use "accounts.mito";` puts that file's text in at that line (paths are relative to the file that
  uses it). There is one state, declared once in `notes.mito`; the other files add endpoints and functions that use it.
- **Tables in SQLite.** `users: table<User> unique(email)` and each user's `notes: table<Note>` live in the `--data` directory, survive
  restarts, and can be read with any SQLite tool while the server runs. Each user owns their notes: `me.notes[id]` can only name a row of
  the signed-in user, so another user's note number answers 404.
- **`unique(email)`** makes the database refuse a second account for the same address.
- **Passwords** are kept as `bcrypt_hash(password, 10)` and checked with `bcrypt_check`.
- **Sessions** are a cookie holding `token(user)`: the row id signed with the row's secret. Signing out changes the secret, so every
  cookie issued earlier stops working on every browser.
- **Searching a table** with `foreach ... where` names its index at compile time: the compiler refused `where u.email == mail` until
  the column was declared `unique(email)`, rather than quietly reading every row.
- **Validated inputs.** `password: ascii<8..72>` is a length check made before the endpoint runs; markup escapes values, so a note
  containing `<b>` is shown as text.

Not here, on purpose: sending email, password reset, sharing notes between users, paging a long list.
