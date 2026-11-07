
/* ---- the C runtime's types (runtime/types.h) ----
   The generator emits the program's layout and routes as constant tables of these types; runtime/mito.c reads them.
   Offsets are bytes from the start of the reservation. */
typedef unsigned short u16; typedef signed char i8; typedef short i16; typedef int i32;
/* One column of a collection. kind: 0 scalar, 1 fixed array of `n` scalars, 2 string length, 3 string bytes (`n` max).
   scalar: 1 i8, 2 i16, 3 i32, 4 i64, 5 u8, 6 u16, 7 u32, 8 u64, 9 f32, 10 f64,
   11 bool. `size`: bytes per item. */
typedef struct { const char *path; u32 kind, scalar, n; u64 off; u32 size; } MitoCol;
/* A collection nested in each item of another, at field path `path` ("" for a collection of collections). */
typedef struct { const char *path; u32 coll; } MitoKid;
/* A list or pool. `mult` instances of `cap` items; `parent` is the collection whose items hold it (-1: top level).
   `hdr`: header words (list: length, head; pool: high-water, count, hint, ever), each an array of `mult` u32.
   `live`: pool live bits (u64 words per instance); `gen`: checked pool generations (u32 per item). */
typedef struct { const char *name; u32 list, drop_oldest, checked, cap, mult; i32 parent; u64 hdr, live, gen, start, end; u32 ncols; const MitoCol *cols; u32 nkids; const MitoKid *kids; } MitoColl;
/* A path segment of an endpoint: a literal, or a parameter `{text}`. */
typedef struct { u32 param; const char *text; } MitoSeg;
typedef struct { const char *method; u32 nsegs; const MitoSeg *segs; u32 html; const u64 *zero; u32 nzero; } MitoRoute; /* html: the endpoint answers HTML (its errors are a page); zero: the request struct's ranges it finds zeroed (nzero pairs; all of it when nzero is ~0) */
/* `req_off`, `req_size`: the request struct, zeroed before each request (size 0: none). `unlogged`: `nunlogged` pairs of byte offsets [start, end), on whole 64-byte lines, kept out of the undo log. */
typedef struct { u32 ncolls; const MitoColl *colls; u64 plain_end, total; u32 neps; const MitoRoute *routes; u32 nunlogged; const u64 *unlogged; u64 req_off, req_size; } MitoProg;
/* A request header or a forwarded field: name and value bytes. */
typedef struct { const char *k; u32 kn; const char *v; u32 vn; } MitoPair;
