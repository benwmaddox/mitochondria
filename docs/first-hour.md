# Your first hour

The [tutorial](tutorial.md) shows every feature. This page teaches the one habit the language asks of you: **decide limits first**. Everything else follows from it.

## 1. A program in ten lines

[examples/guestbook](../examples/guestbook/guestbook.mito) holds at most 200 notes of at most 140 bytes each:

```
struct Note { who: string<24>; text: string<140>; }
struct Book { notes: pool<Note, 200> checked; }
state book: Book;
```

Read the numbers as a budget. `string<140>` is a text of at most 140 bytes. `pool<Note, 200>` is at most 200 notes. The program's whole state is `book`, and the compiler can tell you its exact size before anything runs.

Try it:

```bash
mito check examples/guestbook/guestbook.mito     # compile only; reports every mistake it finds
mito test  examples/guestbook/guestbook.mito     # run the tests in the file
mito serve examples/guestbook/guestbook.mito --port 8080
```

## 2. How to pick a limit

Ask three questions, in this order, for every collection and every text:

1. **What is the biggest it can honestly be?** A username: 24 bytes. A chat line: 500. The notes a small site keeps: a few hundred. Pick a number you could defend, not "as large as possible".
2. **Who pays when it is too small?** A limit is only a bad choice if hitting it hurts. Rejecting a 141-byte note with a clear message is fine; losing a customer's order is not. Where losing the oldest is fine, use `list<T, N> drop_oldest`; where it is not, refuse.
3. **Is the memory cost real?** Reserving is free; only data you write is paid for (the README explains the paging). A generous limit on a rarely full collection costs almost nothing, so lean up, not down.

If you cannot give a number because the data is truly unbounded (documents, uploads, graphs of unknown shape), keep that part in a database or another language. The README says so plainly.

## 3. What "full" looks like

Hitting a limit is an ordinary error your program answers, never a crash:

- An endpoint input that is too long or short is rejected before your code runs: `who: string<1..24>` means 1 to 24 bytes, and a 25-byte name gets a 413.
- A full collection is something you decide. The guestbook checks `count() >= capacity()` and answers `507` with a message. `try_add` is the other route: it returns something that may be null, and the compiler makes you handle that.
- A text you build past its field's maximum is an error that names the field.

Write a test for the full case, as the guestbook does. It is usually three lines, and it is the test most likely to catch a wrong limit.

## 4. Reading the compiler's messages

The compiler reports every mistake in a file at once, in line order, each with the line quoted and a caret under the name at fault:

```
error: app.mito:6:7: GET : no field `nme` (did you mean `name`?)
    6 |     s.nme = "a";
      |       ^
```

Things worth knowing:

- A value that may be null (`try_add`, a stored id whose item was removed) cannot be used until you check it: `if exists(x) { ... }` or `or(x, fallback)`.
- `let x: i64 = "hello"` is an error; a declared `i64` or `f64` is checked against its value.
- A text literal longer than its field is an error when you compile, not when the line runs.
- `mito check app.mito --kinds` prints the inferred kind of every local, which settles most "why does it think that is text?" questions.

## 5. Where next

- [Tutorial](tutorial.md): web programs, tests, command-line tools, functions of a type, constants, generic structs, the standard library, a game.
- [Reference](reference.md): every command, option, built-in and library module.
- [DESIGN.md](../DESIGN.md): why the language is the way it is.
- Larger examples: [todomvc](../examples/todomvc) (pages and tables), [chat](../examples/chat) (sessions, rooms, workers), [jx](../examples/jx) (a command-line tool).
