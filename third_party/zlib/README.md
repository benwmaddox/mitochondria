# zlib 1.4, bundled (deflate side)

Programs whose `web` block says `gzip;` compress their answers with zlib (runtime/gzip.c). This directory holds the unmodified zlib
files the compressor needs: `deflate.c`, `trees.c`, `crc32.c`, `adler32.c`, `zutil.c` and their headers (`gzguts.h` because
`zutil.c` includes it). The compiler building a program (TinyCC or clang) compiles them with runtime/gzip.c into one object, once per
compiler and flags, kept in a cache (src/build.rs, `zlib_object`).

Source: https://zlib.net/zlib-1.4.tar.gz
(SHA-256 02dd159783677a749c04adbdd02b55b8a2bcc7e054f51b76b549199a916ad584, as listed on https://zlib.net/).

Licence: the zlib licence (`LICENSE`), copyright Jean-loup Gailly and Mark Adler. It is not covered by Mitochondria's Apache-2.0 licence.
