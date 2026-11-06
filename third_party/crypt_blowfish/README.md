# crypt_blowfish 1.3, bundled

`bcrypt_hash` and `bcrypt_check` (DESIGN.md, "Bytes, encodings and cryptography") hash passwords with Openwall's crypt_blowfish, the
reference bcrypt implementation (compatible with OpenBSD's bcrypt and bcrypt-ruby for `$2a$`, `$2b$` and `$2y$`). This directory holds
the unmodified `crypt_blowfish.c`, `crypt_blowfish.h` and `ow-crypt.h` (and the upstream `README`). The compiler building a program that
uses them compiles them with runtime/bcrypt.c into one object, once per compiler and flags, kept in a cache (src/build.rs,
`bcrypt_object`).

Source: https://www.openwall.com/crypt/crypt_blowfish-1.3.tar.gz
(SHA-256 83fa01fca6996fe8d882b7f8e9ba0305a5664936100b01481ea3c6a8ce8d72fd).

Licence: placed in the public domain by Solar Designer; where that is not possible, released under "Redistribution and use in source
and binary forms, with or without modification, are permitted" (see the top of `crypt_blowfish.c`). Not covered by Mitochondria's
Apache-2.0 licence.
