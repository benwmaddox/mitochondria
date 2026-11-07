# gumbo, Nokogiri's fork (Nokogiri 1.19.4), bundled

The HTML5 parser behind `html_parse` (runtime/html.h, runtime/html.c, runtime/html_parse.c; Phase 8 of docs/roadmap.md). It is the
parser Rails 8.2 apps parse markup with (Nokogiri::HTML5, the default sanitizer vendor), so a program parses as they do. This
directory holds the unmodified `.c` and `.h` files of `gumbo-parser/src`; the compiler building a program (TinyCC or clang) compiles
them with runtime/html_parse.c into one object, once per compiler and flags, kept in a cache (src/build.rs, `gumbo_object`).

Source: https://github.com/sparklemotion/nokogiri/archive/refs/tags/v1.19.4.tar.gz
(SHA-256 2ff188c100e78e13c6f71eb21c5ff796e4f32cbc4e58785c4a6478f2c85b9b08), `gumbo-parser/src`. Its `ext/nokogiri/gumbo.c` and
`lib/nokogiri/html5/node.rb` match those of the nokogiri-1.19.4 gem the Campfire reference image runs, byte for byte.
`UPSTREAM-README.md` is that directory's README, `THANKS` gumbo-parser's.

Licence: the Apache License 2.0 (`LICENSE`, the text Nokogiri's LICENSE-DEPENDENCIES.md gives for libgumbo), copyright Google Inc.
and Craig Barnes and its other authors. Mitochondria is Apache 2.0 too, but these files keep their own copyright notices.
