# TinyCC 0.9.27 (Windows x86-64), bundled

Mitochondria compiles generated C for endpoints in memory with libtcc. This directory holds the unmodified files it needs
from the official TinyCC 0.9.27 Windows binary release:

- `libtcc.dll`, `libtcc.h`: the compiler as a library. Mitochondria loads the DLL at run time (it is not linked into the
  Mitochondria binary), so TinyCC stays a separate, replaceable library under its own licence.
- `tcc.exe`: the command-line compiler (it uses `libtcc.dll`). Generated C is checked with it in a child process
  before being compiled in memory, because the 0.9.27 library crashes when it reports a compile error to a host built
  with MSVC/rustc.
- `lib/libtcc1-64.a`, `lib/msvcrt.def`, `lib/kernel32.def`: TinyCC's runtime support library and import definitions.

Licence: GNU Lesser General Public License 2.1 (`COPYING`). TinyCC is not covered by Mitochondria's Apache-2.0 licence.

Source of these files, and the corresponding source code:
- binaries: https://download.savannah.gnu.org/releases/tinycc/tcc-0.9.27-win64-bin.zip
  (sha256 34a721949a2583fdff725312da092fa0f5f1f284b702e6f811c6954714faabb2)
- source:   https://download.savannah.gnu.org/releases/tinycc/tcc-0.9.27.tar.bz2
  (sha256 de23af78fca90ce32dff2dd45b3432b2334740bb9bb7b05bf60fdbfc396ceb9c)
