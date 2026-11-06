# TinyCC 0.9.27 (Windows x86-64), bundled

Mitochondria compiles generated C for endpoints in memory with libtcc. This directory holds the unmodified files it needs
from the official TinyCC 0.9.27 Windows binary release:

- `libtcc.dll`, `libtcc.h`: the compiler as a library. Mitochondria loads the DLL at run time (it is not linked into the
  Mitochondria binary), so TinyCC stays a separate, replaceable library under its own licence.
- `tcc.exe`: the command-line compiler (it uses `libtcc.dll`). Generated C is checked with it in a child process
  before being compiled in memory, because the 0.9.27 library crashes when it reports a compile error to a host built
  with MSVC/rustc.
- `lib/libtcc1-64.a`, `lib/msvcrt.def`, `lib/kernel32.def`: TinyCC's runtime support library and import definitions.
- `include/`: its C and Windows headers, for compiling the bundled SQLite (third_party/sqlite) with TinyCC. A few declarations SQLite
  needs that these headers predate are given by Mitochondria before SQLite (`SQLITE_TCC_SHIM` in src/build.rs); the headers are unmodified.

Changed or added here (import lists only, so generated programs import system functions normally instead of looking
them up by name at run time, which Windows Defender's heuristics took for malware):
- `lib/kernel32.def`: ten names appended at the end (`InitializeSRWLock` through `GetFileSizeEx`), which the 0.9.27
  list predates (the last two for SQLite).
- `lib/ws2_32.def`, `lib/bcrypt.def`: generated with `tcc -impdef C:\Windows\System32\<name>.dll` (Windows 11).

Licence: GNU Lesser General Public License 2.1 (`COPYING`). TinyCC is not covered by Mitochondria's Apache-2.0 licence.

Source of these files, and the corresponding source code:
- binaries: https://download.savannah.gnu.org/releases/tinycc/tcc-0.9.27-win64-bin.zip
  (sha256 34a721949a2583fdff725312da092fa0f5f1f284b702e6f811c6954714faabb2)
- source:   https://download.savannah.gnu.org/releases/tinycc/tcc-0.9.27.tar.bz2
  (sha256 de23af78fca90ce32dff2dd45b3432b2334740bb9bb7b05bf60fdbfc396ceb9c), included unmodified as
  `source/tcc-0.9.27.tar.bz2` so the corresponding source travels with the binaries.
