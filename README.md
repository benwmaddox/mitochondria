# Mitochondria

An experimental language in which a program's memory is declared up front: every collection, string and array has a
maximum, and the program's whole state is one struct of them.

```
struct Room { name: string<48>; messages: list<Msg, 1000> drop_oldest; }
struct Chat { sessions: pool<Session, 2000> checked; rooms: pool<Room, 64> checked; }
state chat: Chat;
```

**Reserved, not allocated.** From those maximums the compiler knows the exact size and place of everything, and the
program reserves that address space once, when it starts. Reserving costs no memory.

**Grows and shrinks by pages.** A page takes physical memory only when data is first written to it, and with memory
pressure on (`--soft`, `--hard`) pages are committed only where data is written and given back when the data in it is gone, in a
pressure-driven trim. Without pressure, state up to 1 GiB is committed whole at start (commit charge, not memory), so writes never test for it. A program declared for 64 rooms of 1,000 messages that holds 3
messages costs memory for 3. There is no heap allocator and no garbage collector.

**Access is a lookup.** Because every collection's place is fixed when the program is built, reading data is
arithmetic on an address, not a search: an id is a slot, so `game.players[id]` goes straight to that player, and a
room's messages sit together where the room's slot says. There are no pointers to follow, no hash maps and no queries
to run; a reference is checked for liveness on the way.

**What that changes when you design.** You decide limits early, per thing: how many sessions, how long a name, how
many messages a room keeps. Reaching a limit is an ordinary error the program answers (full, too long), never a crash or
an out-of-memory kill, and peak memory is known before the program runs. In return, data whose size you cannot bound
(documents, large uneven text, graphs of unknown shape) fits poorly; it belongs in a database or another language.

Start with the [tutorial](docs/tutorial.md). The design is in [DESIGN.md](DESIGN.md).

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
