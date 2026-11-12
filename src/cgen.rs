//! Generates C for bound endpoints. Types are worked out while generating: a value whose collection is known becomes a
//! direct load at a constant offset from the reservation's base; integer arithmetic, comparisons and loops are inline.
//! Everything dynamic (request values, strings, conditions on them) goes through helper functions the runtime passes
//! in a table (`H` below), which the C runtime (runtime/mito.c) provides. An endpoint using something the generator does
//! not handle does not compile, and then the program does not load; `generate` says why.
//!
//! Rules the generated code keeps: it never writes state itself (writes go through helpers, which journal them); it only
//! loads from items it reached by live iteration, a checked index or a followed reference; every helper returns 0, or -1
//! after storing an error, and the code then returns at once, so nothing unwinds through C frames.
use crate::lang::*;
use crate::layout::{CKind, Kind, Program, Scalar, Ty};

/// The C side of the boundary. `serve.rs` mirrors this table with `#[repr(C)]`, in the same order.
/// The request's text area (runtime/mito.c's TEXT_CAP): where region text, built text and the request's own text are
const TEXT_AREA: usize = 4 * 1024 * 1024;
pub const PRELUDE: &str = r#"typedef long long i64; typedef unsigned int u32; typedef unsigned char u8; typedef unsigned long long u64;
#define I64MAX 9223372036854775807LL
#define I64MIN (-I64MAX - 1)
typedef struct { u32 off, len; int present; } Txt;
/* tag: 0 null, 1 int (i), 2 float (f), 3 bool (i), 4 string (a = offset, b = length), 5 item (a = collection, b = index) */
typedef struct { int tag; u32 a, b; i64 i; double f; } Val;
typedef struct H H;
typedef struct { u8 *base; void *run; const H *h; u8 *dirty; u8 *committed; int jon; u8 *text; u32 at; i64 steps; } Ctx; /* dirty, committed, jon: the undo log's line map, the page commit flags, whether the request journals: fixed for the request */
/* The runtime's helpers. Every one returns 0, or a non-zero status after recording the error (the generated code then
   returns that status at once and the runtime rolls the request back). Collections, columns and items are numbered as
   in the layout table at the top of an emitted file; `idx` is an item's index across all instances of its collection. */
struct H {
    int (*out)(Ctx *, const char *, u32);                      /* append bytes to the response */
    int (*json_i64)(Ctx *, i64);                               /* append an integer as JSON */
    int (*json_col)(Ctx *, u32, u32, u32);                     /* append column (coll, idx, col) of a live item as JSON */
    int (*json_txt)(Ctx *, const Txt *);                       /* append a request-text string as JSON (null if absent) */
    int (*json_val)(Ctx *, const Val *);                       /* append any value as JSON */
    int (*auth)(Ctx *, u32, u32 *);                            /* the item of pool `coll` named by the request's token, or 401 */
    int (*index)(Ctx *, u32, u32, i64, u32 *);                 /* item k of instance `inst` of `coll`, checked live, or 404 */
    int (*lit)(Ctx *, const char *, u32, Txt *);               /* copy bytes into the request's text area */
    int (*fail)(Ctx *, int, const char *, u32);                /* fail the request with a status and message */
    int (*dead)(Ctx *);                                        /* fail: a reference that is no longer live was used */
    int (*col_txt)(Ctx *, u32, u32, u32, Txt *);               /* copy a string column of an item into the text area */
    int (*plain_txt)(Ctx *, u32, Txt *);                       /* copy a plain (record-style) string at an offset into the text area */
    int (*binop)(Ctx *, int, const Val *, const Val *, Val *); /* an operator (OPS order) on values of run-time kind */
    int (*truthy)(Ctx *, const Val *, int *);                  /* is a value true (bool), or an error for other kinds */
    int (*store)(Ctx *, u32, u32, u32, const Val *);           /* journaled write of column (coll, idx, col) */
    int (*store_plain)(Ctx *, u32, u32, u32, const Val *);     /* journaled write of a plain value: offset, kind, capacity */
    int (*alloc)(Ctx *, u32, u32, int, u32 *);                 /* add an item to instance `inst` of `coll`; try: u32 max when full */
    int (*free_)(Ctx *, u32, u32);                             /* free a pool item when the request succeeds */
    int (*fwd_begin)(Ctx *, const char *, u32);                /* start a forward to worker kind `name` */
    int (*fwd_put)(Ctx *, const char *, u32, const Val *);     /* add a field to the forward */
    int (*forward)(Ctx *, const char *, u32, i64);             /* send the forward to the worker for key; ends the request here */
    int (*crash)(Ctx *);                                       /* test hook: crash this worker */
    int (*builtin)(Ctx *, int, const Val *, u32, Val *);       /* a builtin (BUILTINS order) on argument values */
    int (*json_f64)(Ctx *, double);                            /* append a float as JSON (shortest form; null if not finite) */
    int (*elem)(Ctx *, i64, u32);                              /* check index k into a fixed array of n */
    int (*store_elem)(Ctx *, u32, u32, u32, i64, const Val *); /* journaled write of element k of array column (coll, idx, col) */
    int (*sel_mark)(Ctx *, u32 *);                             /* start collecting items for a loop: a mark to come back to */
    int (*sel_add)(Ctx *, u32);                                /* collect an item */
    int (*sel_limit)(Ctx *, u32, i64, u32 *);                  /* keep at most `limit` of what was collected; how many */
    int (*sel_get)(Ctx *, u32, u32, u32 *);                    /* the k-th collected item */
    int (*sel_end)(Ctx *, u32);                                /* drop what was collected since the mark */
    int (*field_val)(Ctx *, const Val *, const char *, u32, Val *); /* field `name` of an item held as a value */
    int (*store_field)(Ctx *, const Val *, const char *, u32, const Val *); /* journaled write of field `name` of such an item */
    int (*free_val)(Ctx *, const Val *);                       /* free an item held as a value (null: nothing) */
    int (*was_dead)(Ctx *);                                    /* was the error just returned a dead reference or a slot not live? (clears it) */
    int (*input)(Ctx *, int, const char *, u32, int, i64, i64, Val *); /* a typed input of the endpoint (src, name, kind, lo, hi) */
    int (*fields)(Ctx *, const char *, u32);                   /* the body's fields are among these (NUL-separated) */
    int (*json_id)(Ctx *, u32, u32);                           /* an item written as its id: "slot.generation" or the slot */
    int (*wr_plain)(Ctx *, u64, u32, i64);                     /* the same, for plain state: committed at start, never trimmed, so no commit check */
    int (*wr_int)(Ctx *, u64, u32, i64);                       /* an integer of `size` bytes at a byte offset (the value already known to fit) */
    int (*move_elems)(Ctx *, u64, u32, i64, i64, i64);        /* move `count` elements of `esize` bytes within an array at byte offset `base`: from, to, count (already in range) */
    int (*sort_elems)(Ctx *, u64, u32, i64, i64, u64);        /* put `count` integer elements of a fixed array (byte offset, scalar code, from, count) in ascending order, using the scratch array at the last offset */
    int (*fill_elems)(Ctx *, u64, u32, i64, i64, i64, int);   /* set `count` elements of `esize` bytes of an array at a byte offset to a value (its bits), from a position; the last is 1 for plain state */
    int (*clear_coll)(Ctx *, u32, u32);                         /* empty a collection (one instance) at once, with what its items hold; the writes are in the undo log */
    int (*count_below)(Ctx *, u64, u32, i64, i64, int, i64 *);      /* how many of the first n elements of an array (kind code, at a byte offset) are below a target (at most it when inclusive) */
    int (*str_cmp_lit)(Ctx *, int, u32, u32, u32, u32, u32, int *); /* a string column against a literal (op in OPS order), compared where they are: no copy */
    int (*str_cmp_col)(Ctx *, int, u32, u32, u32, u32, u32, u32, int *); /* a string column against another */
    int (*save_line)(Ctx *, u64, u32);                         /* the first write of the request to a line: the undo log saves it (the inline test found its bit clear) */
    int (*commit_range)(Ctx *, u64, u32);                      /* commit the page of a store the inline test found uncommitted */
    int (*save_range)(Ctx *, u64, u64);                        /* save a range to the undo log before a loop that stores into it (lines already saved are skipped) */
    int (*text_len)(Ctx *, u32 *);                             /* the text area's length now */
    int (*arr_txt)(Ctx *, u64, u32, Txt *);                    /* count bytes of a state byte array at a byte offset, copied into the text area as a string */
    int (*text_cut)(Ctx *, u32);                               /* cut the text area back to an earlier length (a loop body that keeps no string past its iteration) */
    int (*rq_put)(Ctx *, int, const char *, u32, const Val *);  /* a test's request: a body field (0) or a header (1), by name */
    int (*rq_send)(Ctx *, int, const Txt *, i64 *, Txt *);       /* a test's request: send it (method, path); its status and body */
    int (*json_field)(Ctx *, const Txt *, const char *, u32, Txt *); /* a top-level field of a JSON object text, as text (null: absent) */
    int (*str_app)(Ctx *, int, u64, u64, u32, const Val *);      /* append a value to a string field in place (kind: plain/column, and empty it first) */
    int (*str_app_at)(Ctx *, int, u64, u64, u32, u64, u32);      /* the same from n bytes of state at an offset (a window onto a text field): one copy */
    int (*str_set)(Ctx *, int, u64, u64, u32, int, const Val *, u32); /* several pieces at once (from the start when the int is 1): one call */
    int (*json_at)(Ctx *, u64, u32);                             /* n bytes of state at an offset (a window onto text) as a JSON string, read where they are */
    int (*str_cmp_at)(Ctx *, int, int, u64, u32, int, u64, u32, int *); /* two texts compared where they are (each in state or a literal) */
    int (*id_get)(Ctx *, u32, i64, u32 *, u32 *);                /* a stored id read back: still that item (1, its index) or not (0) */
    int (*id_of)(Ctx *, u32, u32, i64 *);                       /* the stored id of an item (slot + 1, generation) */
    int (*log)(Ctx *, u32, const Val *, u32);                    /* `log(...)`: values on a line of standard error, with the source line */
    int (*answer)(Ctx *, u32);                                   /* the answer's content type: 0 JSON, 1 HTML, 2 plain text */
    int (*raw_txt)(Ctx *, const Txt *);                          /* text of the text area added to the answer as it is */
    int (*raw_at)(Ctx *, u64, u32);                              /* n bytes of state at an offset added to the answer as they are */
    int (*raw_col)(Ctx *, u32, u32, u32);                        /* a text column added to the answer as it is */
    int (*see_other)(Ctx *, const Txt *);                        /* a 303 answer to a path on this server (checked) */
    int (*set_cookie)(Ctx *, const char *, u32, const Txt *, i64, int); /* a Set-Cookie line on the answer: name, value (checked), max age, HttpOnly */
    int (*html_txt)(Ctx *, const Txt *, u32);                    /* text of the text area into markup, escaped (1: inside an attribute) */
    int (*html_at)(Ctx *, u64, u32, u32);                        /* n bytes of state into markup, escaped */
    int (*html_col)(Ctx *, u32, u32, u32, u32);                  /* a text column into markup, escaped */
    int (*etag)(Ctx *, const char *, u32, int *);                /* a static answer's ETag: set, and *1 when the browser already has it (304) */
    int (*trim)(Ctx *, const Txt *, Txt *);                      /* a text without spaces, tabs and line breaks at either end (same bytes) */
    int (*tget)(Ctx *, u32, u32, u32, Val *);                    /* a table's row's column: table, row id, column */
    int (*tset)(Ctx *, u32, u32, u32, const Val *);              /* write a table's row's column */
    int (*tadd)(Ctx *, u32, u32, u32 *);                         /* add a row (to the owner's rows for a nested table): its id */
    int (*tdel)(Ctx *, u32, u32);                                /* remove a row (and the rows it owns) */
    int (*tcount)(Ctx *, u32, u32, i64 *);                       /* a table's rows (of one owner) */
    int (*tfind)(Ctx *, u32, u32, u32);                          /* `coll[id]`: that row of this table (and owner), or 404 */
    int (*titer)(Ctx *, const char *, u32, const Val *, u32, u32 *); /* a loop over a table: its query, the table, parameters: a cursor */
    int (*tnext)(Ctx *, u32, u32 *, int *);                      /* the cursor's next row id, or none */
    int (*tclose)(Ctx *, u32);                                   /* the end of the loop: the cursor given back */
    int (*tclear)(Ctx *, u32, u32);                              /* remove every row of a table (of one owner) */
    int (*texists)(Ctx *, u32, u32, u32 *);                      /* is there a row with this id (a stored id from working state) */
    int (*tmerge)(Ctx *, u32, u32, u32 *, int *);                /* a merged inner loop: the next row of this owner from the merge cursor */
    int (*set_header)(Ctx *, const char *, u32, const Txt *);   /* a header line on the answer: name (checked when compiling), value (checked) */
    int (*tinsert)(Ctx *, u32, u32, const char *, const u32 *, const Val *, u32, u32 *); /* a row added with its fields' values: one INSERT */
    int (*found)(Ctx *, const Txt *);                            /* a 302 answer to a path on this server, Location absolute (this Host) */
    int (*split_next)(Ctx *, const Txt *, const Txt *, u32 *, Txt *, int *); /* `split`: the next piece of a text from *pos (by a separator, or whitespace when 0), or none */
    int (*txt_cap)(Ctx *, u32, Txt *);                           /* `var t: text<cap>`: cap bytes reserved in the text area (the scope's region), empty */
    int (*txt_set)(Ctx *, Txt *, u32, int, const Val *, u32);   /* pieces written to such a var (cap; from the start when the int is 1) */
};
/* The safety profile (see `check_profile`). Endpoint bodies touch memory, do integer arithmetic and call helpers only
   through these forms. Loads under clang go through memcpy (one instruction), so reading bytes the runtime wrote is
   defined behavior whatever the aliasing rules; TinyCC does no alias analysis, so a plain load is the same thing there. */
#define AT(off, idx, size) (b + (off) + (i64)(idx) * (size))
#ifdef __clang__
#define LD_(T, off, idx, size) ({ T v_; __builtin_memcpy(&v_, AT(off, idx, size), size); v_; })
#else
#define LD_(T, off, idx, size) (*(const T *)AT(off, idx, size))
#endif
#define LD_I64(off, idx) LD_(i64, off, idx, 8)
#define LD_U64(off, idx) LD_(u64, off, idx, 8)
#define LD_I32(off, idx) LD_(int, off, idx, 4)
#define LD_U32(off, idx) LD_(u32, off, idx, 4)
#define LD_I16(off, idx) LD_(short, off, idx, 2)
#define LD_U16(off, idx) LD_(unsigned short, off, idx, 2)
#define LD_I8(off, idx) LD_(signed char, off, idx, 1)
#define LD_U8(off, idx) LD_(u8, off, idx, 1)
#define LD_F32(off, idx) LD_(float, off, idx, 4)
#define LD_F64(off, idx) LD_(double, off, idx, 8)
/* AT_: the source line of the statement being run, defined before each statement's code (0 outside any); every way out with an error
   records it in the context, so the answer can name the line. It costs nothing until something fails. */
#define AT_ 0
#define CALL(x) do { if ((st = (x)) != 0) { c->at = AT_; return st; } } while (0)
/* inside the first argument of or() and exists(): a dead reference or a slot that is not live takes the fallback */
#define CALL_OR_DEAD(x, l) do { if ((st = (x)) != 0) { if (st < 0 && c->h->was_dead(c)) goto l; c->at = AT_; return st; } } while (0)
#define TAIL(x) do { c->at = AT_; return (x); } while (0)
#define FAIL(code, msg, n) do { c->at = AT_; return c->h->fail(c, (code), (msg), (n)); } while (0)
/* a round of a loop without a fixed extent (`while`, a `for` that is not counted), k its round counter: charged to the request's
   step budget 65536 rounds at a time (a test of the counter in a register, not a store each round) */
/* a test's failure: its message and the line of the `return` that gave it */
#define TEST_FAIL_(t) do { *tmsg = (t); c->at = AT_; return 1; } while (0)
#define ROUND_(k) do { if (UNLIKELY(((k) & 65535) == 65535) && (c->steps -= 65536) < 0) FAIL(500, "loop ran too long", 17); } while (0)
#define OVERFLOW_ FAIL(500, "integer overflow", 16)
#ifdef __clang__
#define UNLIKELY(x) __builtin_expect(!!(x), 0)
#define BITS_F64(x) ({ double d_ = (x); i64 u_; __builtin_memcpy(&u_, &d_, 8); u_; })
#else
#define UNLIKELY(x) (x)
#define BITS_F64(x) (*(i64 *)&(double){ (x) })
#endif
/* The undo log and the page commit flags do not change for the length of a request, so a generated function keeps them in locals (jn_, dr_,
   cm_) and a store of a naturally aligned scalar (never across a 64-byte line or a page) is: commit test (pool items only), undo-log bit test,
   the store; the runtime is called only the first time a line is written or a page is touched. */
/* bit operators on 64-bit integers: << keeps the low 64 bits, >> is arithmetic; a shift count outside 0..63 is an error unless an interval proved it in range */
/* maths: the compiler's inline instructions where it has them, the platform's math library otherwise (the library Rust also calls) */
#ifdef __clang__
#define SQRT_(x) __builtin_sqrt(x)
#define SIN_(x) __builtin_sin(x)
#define COS_(x) __builtin_cos(x)
#define ATAN2_(y, x) __builtin_atan2(y, x)
#define POW_(x, y) __builtin_pow(x, y)
#define EXP_(x) __builtin_exp(x)
#define FLOOR_(x) __builtin_floor(x)
#define FABS_(x) __builtin_fabs(x)
#define TAN_(x) __builtin_tan(x)
#define ASIN_(x) __builtin_asin(x)
#define ACOS_(x) __builtin_acos(x)
#define ATAN_(x) __builtin_atan(x)
#define SINH_(x) __builtin_sinh(x)
#define COSH_(x) __builtin_cosh(x)
#define TANH_(x) __builtin_tanh(x)
#define LN_(x) __builtin_log(x)
#define LOG2_(x) __builtin_log2(x)
#define LOG10_(x) __builtin_log10(x)
#define CEIL_(x) __builtin_ceil(x)
#define ROUNDF_(x) __builtin_round(x)
#define TRUNC_(x) __builtin_trunc(x)
#define CBRT_(x) __builtin_cbrt(x)
#define HYPOT_(x, y) __builtin_hypot(x, y)
#define FMOD_(x, y) __builtin_fmod(x, y)
#else
double sqrt(double); double exp(double); double floor(double); double fabs(double); double sin(double); double cos(double); double atan2(double, double); double pow(double, double);
#define SQRT_(x) sqrt(x)
#define SIN_(x) sin(x)
#define COS_(x) cos(x)
#define ATAN2_(y, x) atan2(y, x)
#define POW_(x, y) pow(x, y)
#define EXP_(x) exp(x)
#define FLOOR_(x) floor(x)
#define FABS_(x) fabs(x)
double tan(double); double asin(double); double acos(double); double atan(double); double sinh(double); double cosh(double); double tanh(double); double log(double); double log2(double); double log10(double); double ceil(double); double round(double); double trunc(double); double cbrt(double); double hypot(double, double); double fmod(double, double); 
#define TAN_(x) tan(x)
#define ASIN_(x) asin(x)
#define ACOS_(x) acos(x)
#define ATAN_(x) atan(x)
#define SINH_(x) sinh(x)
#define COSH_(x) cosh(x)
#define TANH_(x) tanh(x)
#define LN_(x) log(x)
#define LOG2_(x) log2(x)
#define LOG10_(x) log10(x)
#define CEIL_(x) ceil(x)
#define ROUNDF_(x) round(x)
#define TRUNC_(x) trunc(x)
#define CBRT_(x) cbrt(x)
#define HYPOT_(x, y) hypot(x, y)
#define FMOD_(x, y) fmod(x, y)
#endif
#define ABS_(r, x) i64 r; if (UNLIKELY((x) == I64MIN)) OVERFLOW_; r = (x) < 0 ? -(x) : (x)
#define BAND(r, x, y) i64 r = (x) & (y)
#define BOR(r, x, y) i64 r = (x) | (y)
#define BXOR(r, x, y) i64 r = (x) ^ (y)
/* a bitwise operation on values proven to fit 8, 16 or 32 bits is done at that width (the C compiler then packs a loop of them into vector lanes of that width) */
#define BOPN_(T, op, r, x, y) i64 r = (i64)(T)((T)(x) op (T)(y))
#define BAND8(r, x, y) BOPN_(u8, &, r, x, y)
#define BAND16(r, x, y) BOPN_(u16, &, r, x, y)
#define BAND32(r, x, y) BOPN_(u32, &, r, x, y)
#define BOR8(r, x, y) BOPN_(u8, |, r, x, y)
#define BOR16(r, x, y) BOPN_(u16, |, r, x, y)
#define BOR32(r, x, y) BOPN_(u32, |, r, x, y)
#define BXOR8(r, x, y) BOPN_(u8, ^, r, x, y)
#define BXOR16(r, x, y) BOPN_(u16, ^, r, x, y)
#define BXOR32(r, x, y) BOPN_(u32, ^, r, x, y)
#define SHL(r, x, y) i64 r; if (UNLIKELY((u64)(y) > 63)) FAIL(500, "shift count out of range", 24); r = (i64)((u64)(x) << (y))
#define SHR(r, x, y) i64 r; if (UNLIKELY((u64)(y) > 63)) FAIL(500, "shift count out of range", 24); r = (x) >> (y)
#define SHRL(r, x, y) i64 r; if (UNLIKELY((u64)(y) > 63)) FAIL(500, "shift count out of range", 24); r = (i64)((u64)(x) >> (y))
#define NSHRL(r, x, y) i64 r = (i64)((u64)(x) >> (y))
#define NSHL(r, x, y) i64 r = (i64)((u64)(x) << (y))
#define NSHR(r, x, y) i64 r = (x) >> (y)
#define NADD(r, x, y) i64 r = (x) + (y)
#define NSUB(r, x, y) i64 r = (x) - (y)
#define NMUL(r, x, y) i64 r = (x) * (y)
#define NDIV(r, x, y) i64 r = (x) / (y)
#define NMOD(r, x, y) i64 r = (x) % (y)
#define TX_(i) (tx_[(i)]) /* a byte of the request's text area, where strings that are not in state live */
/* MITO_MEASURE_NO_JOURNAL, MITO_MEASURE_NO_COMMIT and MITO_MEASURE_NO_RANGE are for measuring what the tests cost only: without the undo log a failed
   request is not rolled back; without commit tests all state is committed at start; without range tests an index out of range is undefined behavior */
#ifdef MITO_MEASURE_NO_JOURNAL
#define SAVE_(a, size) do { } while (0)
#else
#define SAVE_(a, size) do { if (UNLIKELY(jn_ && !((dr_[(a) >> 9] >> (((a) >> 6) & 7)) & 1))) { CALL(c->h->save_line(c, (a), (size))); } } while (0)
#endif
#ifdef MITO_MEASURE_NO_COMMIT
#define COMMIT_(a, size) do { } while (0)
#else
#define COMMIT_(a, size) do { if (UNLIKELY(!cm_[(a) >> 12])) { CALL(c->h->commit_range(c, (a), (size))); } } while (0)
#endif
#ifdef MITO_MEASURE_NO_RANGE
#define RANGE_(x) 0
#else
#define RANGE_(x) UNLIKELY(x)
#endif
#ifdef __clang__
#define ST1_(a, v) do { u8 v_ = (u8)(v); __builtin_memcpy(b + (a), &v_, 1); } while (0)
#define ST2_(a, v) do { u16 v_ = (u16)(v); __builtin_memcpy(b + (a), &v_, 2); } while (0)
#define ST4_(a, v) do { u32 v_ = (u32)(v); __builtin_memcpy(b + (a), &v_, 4); } while (0)
#define ST8_(a, v) do { u64 v_ = (u64)(v); __builtin_memcpy(b + (a), &v_, 8); } while (0)
#define MOVE_(to, from, n) __builtin_memmove(b + (to), b + (from), (n))
#define COPY_(to, from, n) __builtin_memcpy(b + (to), b + (from), (n))
#define ZERO_(to, n) __builtin_memset(b + (to), 0, (n))
#define SCRMOVE(to, from, n) __builtin_memmove(b + (to), b + (from), (n)) /* struct values: a copy into the request's own scratch area (not a state write: a reader may do it) */
#define SCRZERO(to, n) __builtin_memset(b + (to), 0, (n))
#else
#define ST1_(a, v) (*(u8 *)(b + (a)) = (u8)(v))
#define ST2_(a, v) (*(u16 *)(b + (a)) = (u16)(v))
#define ST4_(a, v) (*(u32 *)(b + (a)) = (u32)(v))
#define ST8_(a, v) (*(u64 *)(b + (a)) = (u64)(v))
#define MOVE_(to, from, n) memmove(b + (to), b + (from), (n))
#define COPY_(to, from, n) memcpy(b + (to), b + (from), (n))
#define ZERO_(to, n) memset(b + (to), 0, (n))
#define SCRMOVE(to, from, n) memmove(b + (to), b + (from), (n))
#define SCRZERO(to, n) memset(b + (to), 0, (n))
#endif
#ifdef __clang__
#define ADD(r, x, y) i64 r; if (UNLIKELY(__builtin_add_overflow((x), (y), &r))) OVERFLOW_
#define SUB(r, x, y) i64 r; if (UNLIKELY(__builtin_sub_overflow((x), (y), &r))) OVERFLOW_
#define MUL(r, x, y) i64 r; if (UNLIKELY(__builtin_mul_overflow((x), (y), &r))) OVERFLOW_
#else
#define ADD(r, x, y) i64 r; if (((y) > 0 && (x) > I64MAX - (y)) || ((y) < 0 && (x) < I64MIN - (y))) OVERFLOW_; r = (x) + (y)
#define SUB(r, x, y) i64 r; if (((y) < 0 && (x) > I64MAX + (y)) || ((y) > 0 && (x) < I64MIN + (y))) OVERFLOW_; r = (x) - (y)
#define MUL(r, x, y) i64 r; if ((x) > 0 ? ((y) > 0 ? (x) > I64MAX / (y) : (y) < I64MIN / (x)) : ((y) > 0 ? (x) < I64MIN / (y) : ((x) != 0 && (y) < I64MAX / (x)))) OVERFLOW_; r = (x) * (y)
#endif
#define DIV(r, x, y) i64 r; if (UNLIKELY((y) == 0)) FAIL(500, "division by zero", 16); if (UNLIKELY((x) == I64MIN && (y) == -1)) OVERFLOW_; r = (x) / (y)
#define MOD(r, x, y) i64 r; if (UNLIKELY((y) == 0)) FAIL(500, "division by zero", 16); if (UNLIKELY((x) == I64MIN && (y) == -1)) OVERFLOW_; r = (x) % (y)
#define NEG(r, x) i64 r; if ((x) == I64MIN) OVERFLOW_; r = -(x)
"#;

/// Builtins run by the generic `builtin` helper, by position: the interpreter's own code computes them.
/// The first 14 always go through it; min, max and slot only when an argument's kind is known only at run time.
pub const BUILTINS: [B; 100] = [B::Body, B::Param, B::Query, B::Header, B::Fwd, B::Int, B::Str, B::Len, B::Now, B::Random, B::Token, B::Stat, B::WorkerIndex, B::WorkerCount, B::Min, B::Max, B::Slot,
    // bytes, encodings and cryptography (runtime/crypto.c): codes 17 to 34, always through the runtime's builtin
    B::Base64, B::Base64Url, B::Unbase64, B::Hex, B::Sha1, B::Sha256, B::HmacSha1, B::HmacSha256, B::Pbkdf2Sha256, B::RandomBytes, B::XorBytes,
    B::SecretEqual, B::GcmSeal, B::GcmOpen, B::BcryptHash, B::BcryptCheck, B::FormEncode, B::FormDecode,
    // the process's environment: code 35; a time as text: 36; texts within texts: 37 to 39
    B::Env, B::FormatTime, B::Contains, B::StartsWith, B::EndsWith,
    // the answer without the web block's header lines: 40; text escaped as markup: 41; random characters: 42
    B::NoDefaultHeaders, B::EscapeHtml, B::RandomText,
    // a time plus calendar years: 43; where a text is in a text: 44, 45
    B::AddYears, B::IndexOf, B::LastIndexOf,
    // a text as a JSON string and back: 46, 47
    B::JsonQuote, B::JsonUnquote,
    // the request's path and query as sent: 48; CRC-32: 49; a stored file's bytes: 50
    B::RequestTarget, B::Crc32, B::ReadStored,
    // sockets (runtime/http.c applies what a request asks once it succeeds): 51 to 57
    B::SocketSend, B::SocketClose, B::SocketSubscribe, B::SocketUnsubscribe, B::Broadcast, B::SocketSendAll, B::SocketTag,
    // a text's byte: 58; ASCII letters in lower or upper case: 59, 60
    B::ByteAt, B::Lower, B::Upper,
    // the answer cache (pieces of answers kept between requests, shared by every instance): 61 to 66; the data's version: 67
    B::CacheGet, B::CachePut, B::CacheStart, B::CacheHole, B::CachePart, B::CacheLabel, B::DataVersion,
    // text: every a as b (68); white space off the start or the end (69, 70); every character an emoji (71)
    B::Replace, B::TrimStart, B::TrimEnd, B::AllEmoji,
    // HTML fragments (runtime/html.c, gumbo linked in): 72 to 97
    B::HtmlParse, B::HtmlParseIn, B::NodeKind, B::NodeName, B::NodeParent, B::NodeFirst, B::NodeLast, B::NodeNext, B::NodePrev, B::NodeContent, B::NodeAttr, B::NodeAttrCount, B::NodeAttrName, B::NodeAttrValue, B::NodeSetAttr, B::NodeRemoveAttr, B::NodeNewElement, B::NodeNewText, B::NodeAppend, B::NodeInsertBefore, B::NodeDetach, B::NodeClone, B::NodeHtml, B::NodeInnerHtml, B::NodeSetText, B::NodeHtmlEscaped,
    // a code point as UTF-8: 98; every character a word character: 99
    B::Chr, B::AllWord];
/// The built-ins of HTML fragments: a program using one gets runtime/html.c and gumbo (program_c_full, build.rs).
const HTML_BUILTINS: [B; 26] = [B::HtmlParse, B::HtmlParseIn, B::NodeKind, B::NodeName, B::NodeParent, B::NodeFirst, B::NodeLast, B::NodeNext, B::NodePrev, B::NodeContent, B::NodeAttr, B::NodeAttrCount, B::NodeAttrName, B::NodeAttrValue, B::NodeSetAttr, B::NodeRemoveAttr, B::NodeNewElement, B::NodeNewText, B::NodeAppend, B::NodeInsertBefore, B::NodeDetach, B::NodeClone, B::NodeHtml, B::NodeInnerHtml, B::NodeSetText, B::NodeHtmlEscaped];
/// The built-ins that need the system's cryptography library (runtime/crypto.c linked in).
const CRYPTO_BUILTINS: [B; 11] = [B::Sha1, B::Sha256, B::HmacSha1, B::HmacSha256, B::Pbkdf2Sha256, B::RandomBytes, B::GcmSeal, B::GcmOpen, B::BcryptHash, B::BcryptCheck, B::RandomText];

/// Operator codes for the `binop` helper (the interpreter's operator names, in this order).
pub const OPS: [&str; 11] = ["==", "!=", "<", "<=", ">", ">=", "+", "-", "*", "/", "%"];

/// Plain (record-style) scalar and string kinds for `store_plain`.
/// Bytes in a scalar of kind `code` (see `plain_code`; 9 is f32, which has no typed store).
fn scalar_bytes(code: u32) -> u64 { match code { 1 | 5 | 11 => 1, 2 | 6 => 2, 3 | 7 | 9 => 4, _ => 8 } }

/// The offset of a string literal in the text area's fixed prefix (added once).
fn intern_lit(b: &[u8]) -> u32 {
    LITS.with(|l| {
        let (bytes, at) = &mut *l.borrow_mut();
        if let Some(&(o, n)) = at.iter().find(|&&(o, n)| n as usize == b.len() && &bytes[o as usize..(o + n) as usize] == b) { let _ = n; return o; }
        let o = bytes.len() as u32;
        bytes.extend_from_slice(b);
        at.push((o, b.len() as u32));
        o
    })
}

pub fn plain_code(t: &Ty) -> Option<(u32, u32)> {
    use Scalar::*;
    Some(match t {
        Ty::Scalar(s) => (1 + [I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, Bool].iter().position(|x| x == s)? as u32, 0),
        Ty::Str(n) => (20, *n as u32),
        _ => return None,
    })
}

/// The JSON shape of an answer, as known when it is generated: what `Response<T>` is checked against.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape { Int, Flt, Bool, Str, Null, Any, Opt(Box<Shape>), Arr(Box<Shape>), Obj(Vec<(String, Shape)>), Nothing, Id(usize, String) }
impl Shape {
    fn of(v: &CV) -> Shape {
        match v {
            CV::Int(_) => Shape::Int, CV::Flt(_) => Shape::Flt, CV::Bool(_) => Shape::Bool,
            CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) => Shape::Str, CV::Null => Shape::Null,
            CV::Item(c, _) => Shape::Id(*c, String::new()), CV::Ref(c, ..) => Shape::Opt(Box::new(Shape::Id(*c, String::new()))),
            _ => Shape::Any,
        }
    }
    /// `or(a, b)`: one of the two.
    fn either(a: Shape, b: Shape) -> Shape {
        match (a, b) { (a, b) if a == b => a, (Shape::Null, s) | (s, Shape::Null) => Shape::Opt(Box::new(s)), (Shape::Int, Shape::Flt) | (Shape::Flt, Shape::Int) => Shape::Flt, _ => Shape::Any }
    }
}
impl std::fmt::Display for Shape {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Shape::Int => write!(f, "i64"), Shape::Flt => write!(f, "f64"), Shape::Bool => write!(f, "bool"), Shape::Str => write!(f, "string"),
            Shape::Null => write!(f, "null"), Shape::Any => write!(f, "any"), Shape::Nothing => write!(f, "no body"),
            Shape::Opt(s) => write!(f, "{}?", s), Shape::Arr(s) => write!(f, "[{}]", s),
            Shape::Id(_, n) => write!(f, "{}", if n.is_empty() { "an item" } else { n }),
            Shape::Obj(fs) if fs.is_empty() => write!(f, "{{}}"),
            Shape::Obj(fs) => {
                write!(f, "{{ ")?;
                for (i, (k, v)) in fs.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    let bare = k.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_') && k.chars().all(|c| c.is_alphanumeric() || c == '_');
                    if bare { write!(f, "{}: {}", k, v)?; } else { write!(f, "{:?}: {}", k, v)?; }
                }
                write!(f, " }}")
            }
        }
    }
}

/// An i64 as a C literal (`I64MIN` for the one value that cannot be written as `-...LL`).
fn i64_lit(v: i64) -> String { if v == i64::MIN { "I64MIN".into() } else { format!("{}LL", v) } }

/// `T` of `Response<T>` as written.
fn show_rtype(t: &RType) -> String {
    match t {
        RType::Name(n) => n.clone(), RType::Opt(t) => format!("{}?", show_rtype(t)), RType::Arr(t) => format!("[{}]", show_rtype(t)),
        RType::Obj(f) if f.is_empty() => "{}".into(),
        RType::Obj(f) => format!("{{ {} }}", f.iter().map(|(k, v)| format!("{}: {}", k, show_rtype(v))).collect::<Vec<_>>().join(", ")),
    }
}
/// Does an answer of `shape` fit `Response<want>`? Every value's kind is known when the program is compiled; one known
/// only at run time does not fit.
fn fits_shape(prog: &Program, shape: &Shape, want: &RType, at: &str) -> Result<(), String> {
    let here = || if at.is_empty() { "the answer".to_string() } else { format!("`{}`", at) };
    let bad = || Err(format!("{} is {}, declared {}", here(), shape, show_rtype(want)));
    if *shape == Shape::Any { return Err(format!("{}'s kind is known only at run time", here())); }
    match want {
        RType::Name(n) if n == "none" => Err("`none` is not a type: an answer without a body is `Response<null>`".into()),
        RType::Name(n) if n == "any" => Err("`any` is not a type: every value's kind is known when the program is compiled".into()),
        RType::Opt(t) => if matches!(shape, Shape::Null) { Ok(()) } else if let Shape::Opt(s) = shape { fits_shape(prog, s, t, at) } else { fits_shape(prog, shape, t, at) },
        RType::Arr(t) => match shape { Shape::Arr(e) => fits_shape(prog, e, t, &format!("{}[]", at)), _ => bad() },
        RType::Obj(fields) => match shape {
            Shape::Obj(got) => {
                for (k, _) in got { if !fields.iter().any(|f| f.0 == *k) { return Err(format!("{} has `{}`, which {} does not declare", here(), k, show_rtype(want))); } }
                for (k, t) in fields {
                    let Some((_, s)) = got.iter().find(|g| g.0 == *k) else { return Err(format!("{} has no `{}`", here(), k)) };
                    fits_shape(prog, s, t, &if at.is_empty() { k.clone() } else { format!("{}.{}", at, k) })?;
                }
                Ok(())
            }
            _ => bad(),
        },
        RType::Name(n) => {
            let ok = match n.as_str() {
                "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" => *shape == Shape::Int,
                "f32" | "f64" => matches!(shape, Shape::Flt | Shape::Int),
                "bool" => *shape == Shape::Bool,
                "string" | "utf8" | "ascii" => *shape == Shape::Str,
                "null" => *shape == Shape::Nothing, // no body
                // an enum: written as its value's name
                n if prog.enums.iter().any(|e| e.name == n) => *shape == Shape::Int,
                // an item type: the answer holds an item of a pool of that struct, written as its id
                _ if matches!(shape, Shape::Id(..)) => { let Shape::Id(c, _) = shape else { unreachable!() }; let item = match tab(*c) { Some(t) => Some(prog.tables[t].item), None => prog.colls[*c].item }; item.map_or(false, |s| prog.structs[s].name == *n) }
                _ => {
                    // a declared struct: an object with its plain fields
                    let Some(sd) = prog.structs.iter().find(|s| s.name == *n) else { return Err(format!("{}: `{}` is not a type", here(), n)) };
                    let fields: Vec<(String, RType)> = sd.fields.iter().filter(|f| !f.1.is_coll()).map(|f| (f.0.clone(), rtype_of(prog, &f.1))).collect();
                    return fits_shape(prog, shape, &RType::Obj(fields), at);
                }
            };
            if ok { Ok(()) } else { bad() }
        }
    }
}
/// A struct field's type, as a `Response<T>` type.
fn rtype_of(prog: &Program, t: &Ty) -> RType {
    match t {
        Ty::Scalar(s) => RType::Name(match s { Scalar::F32 | Scalar::F64 => "f64", Scalar::Bool => "bool", _ => "i64" }.into()),
        Ty::Str(_) => RType::Name("string".into()),
        Ty::Struct(i) => RType::Name(prog.structs[*i].name.clone()),
        _ => RType::Name("any".into()),
    }
}

/// Generated endpoint C, and one note per endpoint that cannot be compiled (such a program does not load).
pub struct Generated { pub scratch: bool, pub src: String, pub notes: Vec<String>, pub lits: Vec<u8>,
    /// per endpoint, the request struct's byte ranges it must find zeroed (merged, in order), or None for the whole struct
    pub zeros: Vec<Option<Vec<(usize, usize)>>>,
    /// per endpoint, whether its code never changes state (the request struct and the text area are its own): a reader may run it
    pub reads: Vec<bool>,
    /// per endpoint, what its code does (`mito check --costs`)
    pub costs: Vec<Costs> }
/// What an endpoint's compiled code does, counted where it is written (a site in a loop runs once a round): table queries (loops and
/// lookups), reads of a row's fields, writes to tables (adds, stores, removes), and text built (literals copied, fields read as text,
/// joins, appends), each total and in loops.
#[derive(Clone, Default, Debug)]
pub struct Costs { pub queries: (u32, u32), pub reads: (u32, u32), pub writes: (u32, u32), pub texts: (u32, u32),
    /// region text (lets of built text, capped vars): how many, the most bytes live at once of those whose bound is known, and where
    /// those without one are (`fn name` or the endpoint, and the line)
    pub lets: (u32, usize, Vec<String>) }
/// Count an endpoint's C body's helper calls, each in or out of a loop (a block opened by `for (` / `while (`).
fn count_costs(body: &str) -> Costs {
    let mut c = Costs::default();
    let mut stack: Vec<bool> = Vec::new();
    for line in body.lines() {
        let t = line.trim();
        let in_loop = stack.iter().any(|x| *x);
        let bump = |x: &mut (u32, u32), n: usize| { x.0 += n as u32; if in_loop { x.1 += n as u32; } };
        bump(&mut c.queries, t.matches("->titer(").count() + t.matches("->tfind(").count() + t.matches("->tcount(").count());
        bump(&mut c.reads, t.matches("->tget(").count());
        bump(&mut c.writes, t.matches("->tinsert(").count() + t.matches("->tset(").count() + t.matches("->tadd(").count() + t.matches("->tdel(").count() + t.matches("->tclear(").count());
        bump(&mut c.texts, ["->lit(", "->col_txt(", "->plain_txt(", "->arr_txt(", "->str_app(", "->str_app_at(", "->str_set("].iter().map(|h| t.matches(h).count()).sum::<usize>() + t.matches("->binop(c, 6,").count());
        for ch in t.chars() { if ch == '}' { stack.pop(); } }
        if t.ends_with('{') { stack.push(t.starts_with("for (") || t.starts_with("while (")); }
    }
    c
}

#[derive(Default, Clone)]
struct LoopInv {
    assumed: std::collections::HashMap<(usize, u32), (i64, i64)>,
    entries: std::collections::HashMap<(usize, u32), (i64, i64)>,
    assigns: std::collections::HashMap<(usize, u32), Option<(i64, i64)>>,
}
fn iv_union(a: (i64, i64), b: (i64, i64)) -> (i64, i64) { (a.0.min(b.0), a.1.max(b.1)) }

/// What an expression produced, in C terms. String-valued variants are C variable names.
#[derive(Clone, Debug)]
enum CV {
    Int(String),                    // C expression of type i64
    Flt(String),                    // C expression of type double
    Bool(String),                   // C expression, 0 or 1
    Lit(String),                    // a string known when generating
    Txt(String),                    // a Txt: a string in the request's text area, or absent (null)
    Any(String),                    // a Val: a value whose kind is only known at run time
    Item(usize, String),            // collection, C expression for the item index
    Ref(usize, String, String),     // a followed reference: target pool, slot+1 (0 = null), index
    Coll(usize, String),            // collection, C expression for the instance
    StateStr(usize, String, usize), // a string column of an item: collection, item index, column (read where it is used: a window)
    PlainStr(usize),                // a plain string at an offset (read where it is used: a window)
    Sub(Box<CV>, String, String),   // a piece of a text field (`substr`): the field (StateStr or PlainStr), the start and the count; a window
    Plain(usize, usize),            // a plain struct: struct id, offset
    Part(usize, String, String),    // a struct-valued field of an item: collection, item index, path of the field
    ArrCol(usize, String, usize),   // a fixed-array field of an item: collection, item index, column
    Elem(usize, String, String, String), // one struct of an item's array of structs: collection, item index, path of the array, element index (held)
    PlainArr(usize, Ty, usize),     // a plain fixed array: offset, element type, length
    Resp(String, String),           // a test's request's answer: its status (i64) and body (Txt), read as `.status` and `.body`
    Null,
}

/// Where `return` goes inside an inlined function, and the kind of value it returns (known after the first `return`).
/// `func`: the return of a generated C function, which writes its result through `ret_v` (and `ret_s`) and returns 0.
/// `sel`: a mark taken on entry when the function has loops, so items a loop collected are dropped however it returns.
/// `bound`: the most bytes of text a return so far gave (None: none yet, or one not known: `unbounded`)
/// `mark`: the text area's length when the function began, which a `return` of a value that is not text cuts back to (placeholders
/// `/*RCUT mark*/` until the body is compiled: kept as cuts when the body built region text, dropped otherwise)
struct Ret { dest: Option<(usize, usize)>, var: String, label: String, kind: Option<K>, func: bool, sel: Option<String>, bound: Option<usize>, unbounded: bool, mark: String, ivs: Vec<Option<(i64, i64)>> }

/// How an argument is passed to a generated C function: a value of a kind, or an instance of a collection.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PK { V(K), Coll(usize), Plain(usize, usize), TxtB(usize) }

/// A C function generated for a recursive or outlined function, for one combination of argument kinds. `ret` is the kind it
/// returns: while its body is being generated, the kind being tried.
/// The table is the program's: an endpoint reuses the functions earlier endpoints generated, and each is emitted once.
/// For an outlined function (`rec` false), what its body does to the request's text area, which a caller adds to its own: `peak` the
/// most region text it holds at once, `kept` what stays after it returns (text it returns), `ret_bound` the most bytes its text
/// result holds (None: not known).
#[derive(Clone)]
struct Spec { key: String, name: String, ret: K, proto: String, body: Option<String>, rec: bool, peak: usize, kept: usize, ret_bound: Option<usize> }

/// A loop's body, as the code generator sees it before generating it.
#[derive(Clone, Copy)]
enum LoopBody<'p> { Stmts(&'p [Stmt]), Json(&'p Json) }
/// The loops in a body over a child table of the row in `slot` (`foreach t in b.todos` inside a loop over `b`), not inside another
/// loop (each runs at most once per round of the outer loop), and walked plainly (id order, no `by`, `reverse` or `after`).
fn child_loops<'p>(b: LoopBody<'p>, slot: u32, out: &mut Vec<&'p Iter>) {
    fn is_child(it: &Iter, slot: u32) -> bool {
        matches!(&it.coll, Expr::Field(base, _) if matches!(&**base, Expr::Var(_, VarRes::Local(s)) if *s == slot))
            && it.hi.is_none() && it.via.is_none() && it.by.is_none() && !it.reverse && it.after.is_none()
    }
    fn st<'p>(s: &'p Stmt, slot: u32, out: &mut Vec<&'p Iter>) {
        match s {
            Stmt::If(_, t, e) => for x in t.iter().chain(e.iter()) { st(x, slot, out); },
            Stmt::Each(it, _) => if is_child(it, slot) { out.push(it); },
            Stmt::Respond(_, j) => js(j, slot, out),
            _ => {}
        }
    }
    fn js<'p>(j: &'p Json, slot: u32, out: &mut Vec<&'p Iter>) {
        match j {
            Json::Obj(f) => for (_, v) in f { js(v, slot, out); },
            Json::Arr(it, _) => if is_child(it, slot) { out.push(it); },
            _ => {}
        }
    }
    match b { LoopBody::Stmts(ss) => for s in ss { st(s, slot, out); }, LoopBody::Json(j) => js(j, slot, out) }
}
/// A value a merged inner loop's `where` compares with, worked out before the outer loop: built from literals and locals bound before
/// it and not assigned in its body.
fn bound_before(e: &Expr, env: &[Option<CV>], assigned: &std::collections::HashSet<u32>) -> bool {
    match e {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null => true,
        Expr::Var(_, VarRes::Local(s)) => env.get(*s as usize).map_or(false, |v| v.is_some()) && !assigned.contains(s),
        Expr::Un(_, a) => bound_before(a, env, assigned),
        Expr::Bin(_, a, b) => bound_before(a, env, assigned) && bound_before(b, env, assigned),
        _ => false,
    }
}

/// A table is collection TBASE + its index (in `Program::tables`) for the code that handles items: a row is an item whose index is its
/// row id (u32), a table of one owner's rows a collection whose instance is the owner's row id (0 for a top-level table).
const TBASE: usize = crate::layout::TABLE_BASE;
fn tab(c: usize) -> Option<usize> { if c >= TBASE { Some(c - TBASE) } else { None } }
/// A collection's name (a pool's or list's, or a table's path).
fn coll_name(prog: &Program, c: usize) -> String { match tab(c) { Some(t) => prog.tables[t].name.clone(), None => prog.colls[c].name.clone() } }

/// The interpreter refuses a call nested deeper than this.
const MAX_DEPTH: u32 = 64;
const DEPTH_MSG: &str = "calls nested more than 64 deep";

#[derive(Default)]
struct ScratchUse { unique: std::collections::HashMap<(String, usize), (usize, usize)>, top: usize, bottom: usize }

/// The kind of a value that a `var` or a function's result holds, when it must be the same everywhere.
#[derive(Clone, Copy, Debug, PartialEq)]
enum K { Null, Int, Flt, Bool, Txt, Any, Item(usize), Ref(usize), Coll(usize), Plain(usize, usize) }

/// The smallest kind that holds values of both kinds: null joins strings and references (which have their own empty
/// value) or becomes a general value; an item and a reference to the same pool join as a reference.
fn unify(a: K, b: K) -> K {
    use K::*;
    match (a, b) {
        (a, b) if a == b => a,
        (Null, Txt) | (Txt, Null) => Txt,
        (Null, Item(c) | Ref(c)) | (Item(c) | Ref(c), Null) => Ref(c),
        (Item(c), Ref(d)) | (Ref(c), Item(d)) | (Ref(c), Ref(d)) if c == d => Ref(c),
        _ => Any,
    }
}

thread_local! {
    /// Per function: compiled as a C function of its own though not recursive (see `outlined_fns`), for the program being generated.
    static OUTLINED: std::cell::RefCell<Vec<bool>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The scratch area for struct values (see `G::scratch_alloc`): slots of the C functions generated apart, handed out from the area's top end
    /// (`unique`: (function's spec key, n-th struct local) -> offset and size; `top`: bytes used from the end), and the most bytes any endpoint's own locals
    /// (handed out from the start, and given back at the end of their block) take at once (`bottom`).
    static SCRATCH: std::cell::RefCell<ScratchUse> = std::cell::RefCell::new(Default::default());
    /// Ranges of plain integer fields worked out from the whole program (see `generate_with`): `.0` the ranges reads may assume, `.1` what
    /// each field was seen given by every store compiled (None once a store's value has no proven range).
    // pools nothing in the program removes from (assumed by this build; see `generate_with`), and the pools this build saw a removal from
    static REMOVAL: std::cell::RefCell<(std::collections::HashSet<usize>, std::collections::HashSet<usize>)> = std::cell::RefCell::new(Default::default());
    // loop-carried variables: the interval a build assumes a variable has at the head of a loop (keyed by the loop and the variable's slot), and what it saw: the intervals the variable has on
    // entering the loop, and the union of what its assignments in the loop gave (None: one had no known interval). See `generate_with`.
    static LOOP_INV: std::cell::RefCell<LoopInv> = std::cell::RefCell::new(Default::default());
    static FIELD_RANGES: std::cell::RefCell<(std::collections::HashMap<usize, (i64, i64)>, std::collections::HashMap<usize, Option<(i64, i64)>>)> = std::cell::RefCell::new(Default::default());
    /// The string literals of the program, each once, back to back: the runtime keeps them at the start of every
    /// request's text area, so a literal in generated code is a constant (offset, length) and costs nothing to use.
    static LITS: std::cell::RefCell<(Vec<u8>, Vec<(u32, u32)>)> = const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
    /// Why a kind was widened to one known only at run time, while the kinds of an endpoint are being worked out.
    static WHY: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The kinds of every call's arguments and result, per function (for `mito sigs`, which writes signatures from them).
    static SIGS: std::cell::RefCell<Vec<(u32, Vec<String>, String)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Every local's kind as it is compiled (for `mito check --kinds`): where (a function or an endpoint), the line, the name, the kind.
    static KINDS: std::cell::RefCell<Vec<(String, u32, String, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}
/// A value's kind as a type name: what a parameter of a function that receives it would be declared as.
fn type_text(prog: &Program, v: &CV) -> String {
    let item = |c: usize| if let Some(t) = tab(c) { prog.structs[prog.tables[t].item].name.clone() } else { prog.colls[c].item.map_or(format!("?{}", prog.colls[c].name), |s| { let n = &prog.structs[s].name; let others = prog.colls.iter().filter(|x| x.item == Some(s)).count(); if others > 1 { format!("{} in {}", n, prog.colls[c].name) } else { n.clone() } }) };
    match v {
        CV::Int(_) => "i64".into(), CV::Flt(_) => "f64".into(), CV::Bool(_) => "bool".into(),
        CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) => "string".into(), CV::Null => "null".into(), CV::Any(_) => "any".into(),
        CV::Item(c, _) => item(*c), CV::Ref(c, ..) => format!("{}?", item(*c)),
        CV::Coll(c, _) => format!("coll {}", coll_name(prog, *c)),
        CV::Plain(sd, o) => format!("plain {}@{}", prog.structs[*sd].name, o),
        CV::Part(..) => "part".into(), CV::ArrCol(..) => "arrcol".into(), CV::Elem(..) => "elem".into(), CV::Resp(..) => "response".into(), CV::PlainArr(_, t, n) => format!("plainarr {:?}[{}]", t, n),
    }
}
/// Every local of the program and its kind (`mito check --kinds`), in source order, each once: (where, line, name, kind).
pub fn local_kinds(prog: &Program, api: &Api) -> Vec<(String, u32, String, String)> {
    KINDS.with(|k| k.borrow_mut().clear());
    let _ = generate_tests(prog, api);
    let mut v = KINDS.with(|k| k.borrow().clone());
    v.sort_by_key(|x| x.1);
    v.dedup();
    v
}
/// For every function called anywhere: its name, each parameter's kinds and its result's kinds over all calls.
pub fn sigs(prog: &Program, api: &Api) -> Vec<(String, Vec<Vec<String>>, Vec<String>)> {
    SIGS.with(|s| s.borrow_mut().clear());
    let _ = generate(prog, api);
    let rec = SIGS.with(|s| s.borrow().clone());
    let mut out = Vec::new();
    for (k, f) in api.fns.iter().enumerate() {
        let mut ps: Vec<Vec<String>> = vec![Vec::new(); f.params.len()];
        let mut rs: Vec<String> = Vec::new();
        for (fk, a, r) in rec.iter().filter(|x| x.0 == k as u32) {
            let _ = fk;
            for (i, t) in a.iter().enumerate() { if i < ps.len() && !ps[i].contains(t) { ps[i].push(t.clone()); } }
            if !rs.contains(r) { rs.push(r.clone()); }
        }
        out.push((f.name.clone(), ps, rs));
    }
    out
}
fn why(m: String) { WHY.with(|w| { let mut w = w.borrow_mut(); if !w.contains(&m) { w.push(m); } }) }

/// A kind as the language names it, for errors.
fn kname(prog: &Program, k: K) -> String {
    match k {
        K::Int => "an integer".into(), K::Flt => "a number".into(), K::Bool => "a bool".into(), K::Txt => "a string".into(), K::Null => "null".into(),
        K::Item(c) | K::Ref(c) => if tab(c).is_some() { format!("a row of {}", coll_name(prog, c)) } else { format!("an item of {}", prog.colls[c].name) }, K::Any => "a value of any kind".into(),
        K::Plain(sd, _) => format!("a struct `{}`", prog.structs[sd].name),
        _ => "a collection".into(),
    }
}

/// A kind that has to be widened, found while generating: the endpoint is generated again with it.
#[derive(Clone, Copy, Debug)]
enum Widen { Slot(Option<u32>, u32, K), Ret(u32, K) }

struct G<'p> {
    /// The request struct's top-level fields the endpoint's code names (byte ranges): only these are zeroed before it runs; and
    /// whether it calls a function generated for recursive calls (shared by endpoints: then the whole struct is zeroed).
    rq_touched: Vec<(usize, usize)>, uses_spec: bool,
    // what is known of each mutable integer local's value at this point of the code (C name -> interval): from its initial value, its assignments, `if` joins, the
    // widening a counted loop gives a counter, and the condition a `while` or `if` holds; absent: nothing known. Consulted by `interval` after `ranges`.
    var_iv: std::collections::HashMap<String, (i64, i64)>,
    loop_frames: Vec<usize>, // the loops being compiled, outermost first (their ids: the address of their `Iter`)
    var_depth: std::collections::HashMap<String, usize>, // how many loops were open where each variable was declared
    loop_stmts: Option<&'p [Stmt]>, loop_entry: Option<std::collections::HashMap<String, (i64, i64)>>, loop_heads: Option<std::collections::HashMap<String, (i64, i64)>>,
    wrote_state: std::cell::Cell<bool>, // set when this endpoint's code can change state (plain state, a collection, a table): see `reads` in Generated
    /// fields of items known not null where the code is (`if exists(m.connected_at) { ... }`): collection, item index, path
    narrowed: Vec<(usize, String, String)>,
    /// the most bytes a text value (a Txt's C name) can hold, where known: literals, fields, inputs, joins of those
    txt_bounds: std::collections::HashMap<String, usize>,
    /// each `let` of text built here (in its scope's region): its most bytes, when known; `built_lets` counts them as compiled
    region_lets: Vec<Option<usize>>,
    /// the statement being compiled declared region text of its own (a `let` of built text, a capped `var`), not only text that
    /// calls inlined into it held in their own scopes
    own_region: bool,
    /// bytes of region text live now (lets and capped vars of the scopes open here), the most at once, and how many are unbounded (an
    /// unbounded part, or a region kept across a loop's iterations)
    region_live: usize,
    region_peak: usize,
    region_unbounded: Vec<String>,
    /// the C names of capped `var`s of text (`var acc: text<N>`) and their caps
    txt_caps: std::collections::HashMap<String, usize>,
    prog: &'p Program, api: &'p Api, ep: &'p Endpoint, body: String, n: usize, depth: usize, inlining: Vec<u32>, ret: Vec<Ret>, ret_ctx: Option<Option<usize>>, pending_ctx: Option<Option<usize>>,
    frame: Option<u32>,                                  // the function whose slots `env` holds (None: the endpoint)
    slot_kinds: &'p [((Option<u32>, u32), K)],           // widened kinds of `var` and `let` slots
    ret_kinds: &'p [(u32, K)],                           // widened result kinds of functions
    widen: Option<Widen>,
    u32_vars: std::collections::HashSet<String>, // `var`s kept in a `u32` of the C (their range fits 0..2^32-1); read as `(i64)name`
    rest: &'p [Stmt], // set by block_inner for the statement it compiles: the statements after it in its block
    var_names: std::collections::HashSet<String>, // the C names of `var`s: a `let` of one must copy it
    fast_sites: std::collections::HashSet<*const Expr>, // array indexes a versioned loop's guard proved in range (their stores' ranges already saved)
    versioned: u32, // how many versioned loops enclose the code being generated (each doubles its body; at most 3 deep)
    loop_hint: Option<(i64, i64)>, // the next range loop's counter interval, proven by a versioning guard (its fast copy)
    at: (u32, u32), // the source line and column of the statement being generated (for error messages)
    test: bool, // generating a test: `return` gives its result
    str_lens: std::collections::HashMap<usize, String>, // plain text fields' lengths read before a loop that writes no state (by offset)
    loop_n: u32, // labels of loops, numbered on their own
    loops: Vec<(String, String, bool, bool)>, // the loops being generated, innermost last: break label, continue label, whether each is used
    checked: Vec<(String, u64)>, // range tests done since the last statement start, block boundary, label or goto: (index, size) need not be tested again
    skip_save: bool, // the next inline store needs no undo-log test (its range was saved by a loop guard)
    /// inside an outermost loop: the small logged plain fields and arrays (byte offset, size) its stores skip the undo-log test for;
    /// the loop saves them whole once, before it starts (see `stmt`)
    presave: Option<Vec<(usize, usize)>>,
    txt_escapes: Vec<u32>, // slots of variables that were assigned a string (or a value of unknown kind), in the order it happened
    recursive: &'p [bool],                               // per function: is it part of a cycle of calls
    specs: &'p std::cell::RefCell<Vec<Spec>>,            // C functions generated for recursive functions
    dep: bool,                                           // generating a C function: its call depth is the parameter `dep`
    inl: u32,                                            // inlined calls the code being generated is nested in
    ranges: std::collections::HashMap<String, (i64, i64)>, // C names that never change after they are set (inputs, loop counters, results of arithmetic) with an inclusive interval their value is proven to lie in
    or_dead: Option<String>,                             // inside or()/exists(): where a dead reference or slot not live goes
    /// The body of the loop about to be generated (statements or a JSON element), for a table loop looking for a loop over its rows'
    /// children to merge (see `table_loop`); taken by the next loop.
    loop_body: Option<LoopBody<'p>>,
    /// Inner loops merged into their outer loop's query: the inner loop, the merge cursor's C name.
    merged: Vec<(*const Iter, String)>,
    /// Struct values (locals): where the next one goes in the scratch area (bytes from its start; given back at the end of a block); in a C function
    /// generated apart (`spec_key`), each local has its own place from the area's top end instead, `scr_n` counting them; `spec_rec`: that function is recursive
    scr_top: usize, spec_key: Option<String>, scr_n: usize, spec_rec: bool,
    /// slots of `let` struct locals (their fields may not be assigned): (function, slot)
    immut: std::collections::HashSet<(Option<u32>, u32)>,
}
type R<T> = Result<T, String>;
fn no<T>(what: impl Into<String>) -> R<T> { Err(what.into()) }
fn is_name(e: &str) -> bool { e.chars().all(|c| c.is_alphanumeric() || c == '_') }

/// `Response<html>`, `<text>`, `<css>`, `<js>`, `<svg>`: answers of text, with the runtime's content type code; `<bytes>`: any bytes
/// (`application/octet-stream` unless the endpoint sets its Content-Type), sent as they are.
fn text_answer(t: &crate::lang::RType) -> Option<(u32, &'static str)> {
    match t { crate::lang::RType::Name(n) => match n.as_str() { "html" => Some((1, "html")), "text" => Some((2, "text")), "css" => Some((3, "css")), "js" => Some((4, "js")), "svg" => Some((5, "svg")), "bytes" => Some((6, "bytes")), _ => None }, _ => None }
}
/// FNV-1a, 64 bits: a static answer's ETag.
fn fnv64(b: &[u8]) -> u64 { b.iter().fold(0xcbf29ce484222325u64, |h, x| (h ^ *x as u64).wrapping_mul(0x100000001b3)) }

/// Text as markup: & < > as entities, and " inside an attribute's quotes.
fn html_escape(s: &str, attr: bool) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() { match ch { '&' => o.push_str("&amp;"), '<' => o.push_str("&lt;"), '>' => o.push_str("&gt;"), '"' if attr => o.push_str("&quot;"), c => o.push(c) } }
    o
}

/// A C string literal for `s`, safe for any bytes.
pub fn c_lit(s: &[u8]) -> String {
    let mut o = String::from("\"");
    for &b in s { match b { b'"' => o.push_str("\\\""), b'\\' => o.push_str("\\\\"), 0x20..=0x7e => o.push(b as char), _ => o.push_str(&format!("\\{:03o}", b)) } }
    o.push('"'); o
}
/// A C double literal that is exactly `f` (Rust's shortest round-trip text, with a decimal point).
fn c_double(f: f64) -> String {
    let t = format!("{:?}", f);
    if t.contains('.') || t.contains('e') { t } else { format!("{}.0", t) }
}
/// The JSON text of a string, escaped now, so the generated code can write it as is.
fn json_lit(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() { match c { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), '\n' => o.push_str("\\n"), '\r' => o.push_str("\\r"), '\t' => o.push_str("\\t"), c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)), c => o.push(c) } }
    o.push('"'); o
}

pub fn generate(prog: &Program, api: &Api) -> Generated { generate_with(prog, api, None, "", false) }
/// The same with the program's tests (`test "name" { ... }`) as functions `test<k>(Ctx *, Txt *)` after the endpoints.
pub fn generate_tests(prog: &Program, api: &Api) -> Generated { generate_with(prog, api, None, "", true) }
/// The names of the program's tests, in order.
pub fn test_names(api: &Api) -> Vec<String> { api.fns.iter().filter_map(|f| f.test.clone()).collect() }

/// Does a program load? Its api parses, every name binds, and every endpoint compiles. The error holds every independent
/// problem found, one per line: parsing stops at the first; binding and code generation go on to the next function or
/// endpoint (a problem in a function several endpoints call is reported once, at its place).
pub fn check(prog: &Program, api_src: &str) -> Result<Api, String> {
    check_inner(prog, api_src).map_err(|e| crate::generics::name_instances(&e))
}
fn check_inner(prog: &Program, api_src: &str) -> Result<Api, String> {
    let mut api = parse(api_src)?;
    crate::lang::bind_with(&mut api, &|n| prog.global(n), &prog.enum_table(), &prog.const_table())?;
    set_methods(&api);
    let notes = generate_tests(prog, &api).notes;
    let mut seen: Vec<&str> = Vec::new();
    let mut out: Vec<&str> = Vec::new();
    for n in &notes {
        match split_place(n) { Some((place, _)) if seen.contains(&place) => continue, Some((place, _)) => seen.push(place), None => {} }
        out.push(n);
    }
    if out.is_empty() { Ok(api) } else { Err(out.join("\n")) }
}

/// The C runtime (runtime/*.c), compiled in the same translation unit as a program.
pub const RUNTIME: &str = include_str!("../runtime/mito.c");
/// Unicode's case mappings (generated by tools/unicase.py), for a program that uses `lower` or `upper`.
pub const UNICASE: &str = include_str!("../runtime/unicase.c");
/// The operating system's part (runtime/os.c: Windows and Linux), first in every translation unit after the prelude's types.
pub const OS: &str = include_str!("../runtime/os.c");
/// Stored data (runtime/store.c): tables in SQLite, for a program that has one.
pub const STORE: &str = include_str!("../runtime/store.c");
/// Cryptography from the system's library (runtime/crypto.c), for a program that uses a crypto built-in.
pub const CRYPTO: &str = include_str!("../runtime/crypto.c");
/// HTML fragments' node pool (runtime/html.c), for a program that uses an HTML built-in.
pub const HTML: &str = include_str!("../runtime/html.c");
/// The runtime's types for the layout tables, ahead of them.
pub const RUNTIME_TYPES: &str = include_str!("../runtime/types.h");

/// A whole program as one C translation unit: the prelude, the layout and routes as data, the C runtime, and the
/// endpoints. `mito_*` (runtime/mito.c) is its interface. Fails if an endpoint does not compile.
pub fn program_c(prog: &Program, api: &Api, source: Option<(&str, &str)>) -> Result<String, String> { program_c_named(prog, api, source, "mito_") }
/// A program with its tests, and the test runner's `main` (runtime/testmain.c), as an executable's source: `mito test`.
pub fn program_test_exe(prog: &Program, api: &Api, source: Option<(&str, &str)>) -> Result<Vec<String>, String> {
    Ok(vec![program_c_full(prog, api, source, "mito_", true)? + TESTMAIN])
}
/// The test runner (runtime/testmain.c).
pub const TESTMAIN: &str = include_str!("../runtime/testmain.c");

/// The same, its interface named `prefix`start, `prefix`handle and so on (programs of a system share one build).
pub fn program_c_named(prog: &Program, api: &Api, source: Option<(&str, &str)>, prefix: &str) -> Result<String, String> { program_c_full(prog, api, source, prefix, false) }
fn program_c_full(prog: &Program, api: &Api, source: Option<(&str, &str)>, prefix: &str, tests: bool) -> Result<String, String> {
    let g = generate_with(prog, api, source, "static ", tests);
    if let Some(n) = g.notes.first() { return Err(n.clone()); }
    // The profile was checked on `c->h->helper`; in the unit itself the helper table is the runtime's own constant
    // `mito_h`, so the optimizer sees the calls go straight to the helpers and can inline them.
    let direct = |s: &str| s.replace("c->h->", "mito_h.");
    let mut o = String::from("#if defined(__AVX2__)
#include <immintrin.h>
#endif
"); // vector compares, only when the build enables AVX2 (`--cflag -mavx2`)
    o += &direct(PRELUDE);
    o += OS;
    o += RUNTIME_TYPES;
    o += &format!("#define MITO(x) {}##x\n", prefix);
    o += &layout_c(prog, api, &g.zeros, &g.reads, g.scratch);
    if tests {
        // the tests, for the runner (MITO(run_test))
        let names = test_names(api);
        for k in 0..names.len() { o += &format!("static int test{}(Ctx *c, Txt *tmsg);\n", k); }
        o += "typedef int (*TestFn)(Ctx *, Txt *);\nstatic const TestFn mito_tests[] = {";
        for k in 0..names.len() { o += &format!(" test{},", k); }
        o += " 0 };\nstatic const char *const mito_test_names[] = {";
        for n in &names { o += &format!(" {},", c_lit(n.as_bytes())); }
        o += &format!(" 0 }};\n#define MITO_NTESTS {}\n", names.len());
    }
    o += &format!("/* the program's string literals, at the start of every request's text area */
#define MITO_LITS_LEN {}
static const unsigned char mito_lits[] = {{ {} }};
", g.lits.len(), if g.lits.is_empty() { "0".to_string() } else { g.lits.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",") });
    if !prog.tables.is_empty() {
        // tables: SQLite linked in (the build sees this line), the store runtime, and what the program declares of each table
        o += "#define MITO_USES_SQLITE 1\n";
        o += STORE;
        o += &store_tables_c(prog);
    }
    o += &format!("#define MITO_HAS_START {}\n", api.eps.iter().any(|e| e.method == "START") as u8); // a `@start` function (MITO(begin))
    // sockets (`@socket`): the front end upgrades connections; the handshake's SHA-1 is the system library's (crypto linked in)
    let sockets = api.eps.iter().any(|e| e.method == "SOCKET");
    if sockets { o += "#define MITO_SOCKETS 1\n"; }
    if let Some(e) = api.eps.iter().find(|e| e.method == "EVERY") { o += &format!("#define MITO_EVERY_MS {}\n", e.text.trim_start_matches("EVERY ")); }
    if g.src.contains("/*crypto*/") || sockets {
        // a crypto built-in: the system library's primitives (the build sees this line and links libcrypto and crypt_blowfish)
        o += "#define MITO_USES_CRYPTO 1\n";
        o += CRYPTO;
    }
    // HTML fragments: the node pool (the build sees this line and links gumbo with runtime/html_parse.c)
    if g.src.contains("/*html*/") { o += "#define MITO_USES_HTML 1\n"; o += HTML; }
    // lower / upper (builtins 59, 60): Unicode's case tables
    if g.src.contains("builtin(c, 59,") || g.src.contains("builtin(c, 60,") { o += "#define MITO_UNICASE 1\n"; o += UNICASE; }
    o += RUNTIME;
    o += &direct(&g.src[PRELUDE.len()..]);
    Ok(o)
}

/// The HTTP server and `main` (runtime/http.c), for executables.
pub const HTTP: &str = include_str!("../runtime/http.c");

/// One program as an executable's source: the program, its `web` block's data (src/web.rs; empty without one), then its HTTP server.
pub fn program_exe(prog: &Program, api: &Api, source: Option<(&str, &str)>, web: &str) -> Result<Vec<String>, String> {
    // a program whose entry is `@main` is a command-line program: its `main` is runtime/cli.c, not a server
    if let Some(e) = api.eps.iter().find(|e| e.method == "MAIN") {
        if api.eps.iter().any(|x| !matches!(x.method.as_str(), "MAIN" | "START")) { return Err("a program with `@main` is a command-line program: it has no endpoints (`@get` ...) beside it".into()); }
        let stdin = e.inputs.iter().any(|i| matches!(&i.src, Src::Header(h) if h == ":stdin"));
        return Ok(vec![program_c(prog, api, source)? + if stdin { "#define MITO_CLI_STDIN 1\n" } else { "" } + CLI]);
    }
    // a program whose entry is `@frame` is a game: its `main` is runtime/game.c, which loads SDL3 and OpenGL when it starts
    if api.eps.iter().any(|e| e.method == "FRAME") {
        let shaders: Vec<&str> = api.eps.iter().filter(|e| e.method == "SHADER").map(|e| e.name.as_str()).collect();
        let mut keys: Vec<String> = Vec::new();
        for e in api.eps.iter().filter(|e| e.method == "FRAME") {
            for i in &e.inputs { if let Src::Header(h) = &i.src { if let Some(k) = h.strip_prefix(":key_") { if !keys.iter().any(|x| x == k) { keys.push(k.to_string()); } } } }
        }
        let mouse = api.eps.iter().filter(|e| e.method == "FRAME").any(|e| e.inputs.iter().any(|i| matches!(&i.src, Src::Header(h) if h.starts_with(":mouse_"))));
        let list = |v: &[String]| v.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(", ");
        let defs = format!("{}#define MITO_USES_SDL3 1\n#define MITO_NSHADERS {}\nstatic const char *const mito_shader_names[] = {{ {} }};\n#define MITO_NKEYS {}\nstatic const char *const mito_key_names[] = {{ {} }};\n",
            if mouse { "#define MITO_MOUSE 1\n" } else { "" }, shaders.len(), list(&shaders.iter().map(|s| s.to_string()).collect::<Vec<_>>()), keys.len(), if keys.is_empty() { "0".to_string() } else { list(&keys) });
        return Ok(vec![program_c(prog, api, source)? + &defs + GAME]);
    }
    Ok(vec![program_c(prog, api, source)? + web + "#define HTTP_SINGLE\n" + HTTP])
}
/// The window driver and `main` (runtime/game.c), for programs with `@frame`.
pub const GAME: &str = include_str!("../runtime/game.c");
/// The command-line driver and `main` (runtime/cli.c), for programs with `@main`.
pub const CLI: &str = include_str!("../runtime/cli.c");

/// A system as an executable's sources: its translation units, the HTTP server after the system's.
pub fn system_exe(spec: &crate::spec::Spec) -> Result<Vec<String>, String> {
    let mut units = system_c(spec)?;
    if let Some(last) = units.last_mut() { *last += HTTP; }
    Ok(units)
}

/// The C system of workers (runtime/system.c): threads, mailboxes, reply slots, forwarding.
pub const SYSTEM: &str = include_str!("../runtime/system.c");

/// A system of workers as C translation units: one per worker kind (its program, interface prefixed `k<n>_`), then the
/// system itself with the table of kinds. Built together, `mito_system_*` is the interface.
pub fn system_c(spec: &crate::spec::Spec) -> Result<Vec<String>, String> {
    let mut units = Vec::new();
    let mut table = String::from("/* ---- the system's worker kinds (generated) ---- */\ntypedef unsigned char u8; typedef unsigned short u16; typedef unsigned int u32; typedef unsigned long long u64; typedef long long i64;\n");
    table += OS;
    table += "typedef struct { const char *k; u32 kn; const char *v; u32 vn; } MitoPair;\n";
    table += "typedef struct { const char *name; u32 count, mailbox, slot; u64 soft, hard; int marks;\n    void *(*start)(u64, u64); void (*stop)(void *); void (*set_clock)(void *, i64, int); void (*set_marks)(void *, u64, u64);\n    int (*handle)(void *, const char *, u32, const char *, u32, const MitoPair *, u32, const MitoPair *, u32, const char *, u32);\n    const char *(*response)(void *, u32 *); void (*set_system)(void *, u32, const char *const *, const u32 *, u32, u32);\n    u32 (*forward_to)(void *, u32 *, i64 *, MitoPair *, u32); void (*finish_forward)(void *, int); int (*crashed)(void *);\n    const char *(*response_head)(void *, u32 *); int (*has_tables)(void); int (*open_data)(void *, const char *, const char *, u64, int); const char *(*data_error)(void *); } SysKind;\n";
    let fns = ["start", "stop", "set_clock", "set_marks", "handle", "response", "set_system", "forward_to", "finish_forward", "crashed", "response_head", "has_tables", "open_data", "data_error"];
    let sigs = ["void *{}start(u64, u64);", "void {}stop(void *);", "void {}set_clock(void *, i64, int);", "void {}set_marks(void *, u64, u64);",
        "int {}handle(void *, const char *, u32, const char *, u32, const MitoPair *, u32, const MitoPair *, u32, const char *, u32);", "const char *{}response(void *, u32 *);",
        "void {}set_system(void *, u32, const char *const *, const u32 *, u32, u32);", "u32 {}forward_to(void *, u32 *, i64 *, MitoPair *, u32);", "void {}finish_forward(void *, int);", "int {}crashed(void *);", "const char *{}response_head(void *, u32 *);", "int {}has_tables(void);", "int {}open_data(void *, const char *, const char *, u64, int);", "const char *{}data_error(void *);"];
    let mut rows = Vec::new();
    let mut total = 0usize;
    for (i, k) in spec.kinds.iter().enumerate() {
        let prog = Program::parse(&k.state).map_err(|e| format!("worker {}: {}", k.name, e))?;
        total = total.saturating_add(prog.total.saturating_mul(k.count));
        let mut api = crate::lang::parse(&k.api).map_err(|e| format!("worker {}: {}", k.name, e))?;
        crate::lang::bind_with(&mut api, &|n| prog.global(n), &prog.enum_table(), &prog.const_table()).map_err(|e| format!("worker {}: {}", k.name, e))?;
        let pre = format!("k{}_", i);
        units.push(program_c_named(&prog, &api, None, &pre).map_err(|e| format!("worker {}: {}", k.name, e))?);
        for s in &sigs { table += &s.replace("{}", &pre); table.push('\n'); }
        let f: Vec<String> = fns.iter().map(|f| format!("{}{}", pre, f)).collect();
        rows.push(format!("    {{ {}, {}, {}, {}, {}ULL, {}ULL, {}, {} }}", c_lit(k.name.as_bytes()), k.count, k.mailbox, k.slot, k.soft.unwrap_or(0), k.hard.unwrap_or(usize::MAX), k.soft.is_some() as i32,
            f.iter().zip(["(void *(*)(u64, u64))", "", "", "", "", "", "", "", "", "", "", "", "", ""]).map(|(n, c)| format!("{}{}", c, n)).collect::<Vec<_>>().join(", ")));
    }
    const MB: usize = 1 << 20;
    if total > spec.budget { return Err(format!("over budget: {} workers need {} MB, budget is {} MB", spec.kinds.iter().map(|k| k.count).sum::<usize>(), total / MB, spec.budget / MB)); }
    table += &format!("static const SysKind SYS_KINDS[] = {{\n{}\n}};\n#define SYS_NKINDS {}\n#define SYS_ENTRY {}\n#define SYS_INFLIGHT {}\n", rows.join(",\n"), spec.kinds.len(), spec.entry, spec.inflight);
    units.push(table + SYSTEM);
    Ok(units)
}

/// The layout and the routes as C data for the runtime: collections with their columns and nested collections,
/// endpoints with their path segments.
pub fn layout_c(prog: &Program, api: &Api, zeros: &[Option<Vec<(usize, usize)>>], reads: &[bool], scratch: bool) -> String {
    let mut o = String::from("\n/* ---- the program's layout and routes (generated) ---- */\n");
    for (k, c) in prog.colls.iter().enumerate() {
        o += &format!("static const MitoCol mito_cols{}[] = {{\n", k);
        for col in &c.cols {
            let (kind, sc, n) = match &col.kind {
                CKind::Scalar(s) => (0, plain_code(&Ty::Scalar(*s)).unwrap().0, 0),
                CKind::Arr(s, n) => (1, plain_code(&Ty::Scalar(*s)).unwrap().0, *n as u32),
                CKind::StrLen => (2, col.range.is_some() as u32, 0), // 1: an ascii<N> column
                CKind::StrBytes(m) => (3, 0, *m as u32),
            };
            o += &format!("    {{ {}, {}, {}, {}, {}, {} }},\n", c_lit(col.path.as_bytes()), kind, sc, n, col.off, col.kind.size());
        }
        o += "    { 0, 0, 0, 0, 0, 0 }\n};\n";
        o += &format!("static const MitoKid mito_kids{}[] = {{", k);
        for (path, kid) in &c.children { o += &format!(" {{ {}, {} }},", c_lit(path.as_bytes()), kid); }
        o += " { 0, 0 } };\n";
    }
    o += "static const MitoColl mito_colls[] = {\n";
    for (k, c) in prog.colls.iter().enumerate() {
        let (list, drop) = match c.kind { Kind::List(f) => (1, (f == crate::layout::Full::DropOldest) as u32), Kind::Pool(_) => (0, 0) };
        o += &format!("    {{ {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, mito_cols{}, {}, mito_kids{} }},\n",
            c_lit(c.name.as_bytes()), list, drop, c.checked() as u32, c.cap, c.mult, c.parent.map_or(-1, |p| p as i64), c.hdr, c.live, c.gen, c.start, c.end, c.cols.len(), k, c.children.len(), k);
    }
    if prog.colls.is_empty() { o += "    { 0 }\n"; }
    o += "};\n";
    for (k, ep) in api.eps.iter().enumerate() {
        o += &format!("static const MitoSeg mito_segs{}[] = {{", k);
        for sg in &ep.segs { match sg { Seg::Lit(l) => o += &format!(" {{ 0, {} }},", c_lit(l.as_bytes())), Seg::Param(pre, n) if pre.is_empty() => o += &format!(" {{ 1, {} }},", c_lit(n.as_bytes())), Seg::Param(pre, n) => o += &format!(" {{ 1, {}, {} }},", c_lit(n.as_bytes()), c_lit(pre.as_bytes())) } }
        o += " { 0, 0 } };\n";
    }
    // the request struct's ranges each endpoint finds zeroed (only those its code names; all of it when it calls a shared function)
    for k in 0..api.eps.len() {
        o += &format!("static const u64 mito_zero{}[] = {{", k);
        if let Some(Some(rs)) = zeros.get(k) { for (a, b) in rs { o += &format!(" {}ULL, {}ULL,", a, b); } }
        o += " 0 };\n";
    }
    o += "static const MitoRoute mito_routes[] = {\n";
    for (k, ep) in api.eps.iter().enumerate() {
        let nzero = match zeros.get(k) { Some(Some(rs)) => rs.len() as u32, _ => u32::MAX };
        o += &format!("    {{ {}, {}, mito_segs{}, {}, mito_zero{}, {}u, {}, {} }},\n", c_lit(ep.method.as_bytes()), ep.segs.len(), k, matches!(&ep.resp, crate::lang::RType::Name(n) if n == "html") as u32, k, nzero, (ep.method != "GET" && writes(api, &ep.body, &mut Vec::new())) as u32, reads.get(k).copied().unwrap_or(false) as u32);
    }
    o += "    { 0, 0, 0, 0, 0, 0, 0, 0 }\n};\n";
    for k in 0..api.eps.len() { o += &format!("static int ep{}(Ctx *c);\n", k); }
    o += "typedef int (*EpFn)(Ctx *);\nstatic const EpFn mito_eps[] = {";
    for k in 0..api.eps.len() { o += &format!(" ep{},", k); }
    o += " 0 };\n";
    o += "static const u64 mito_unlogged[] = {";
    for (a, b) in &prog.unlogged { o += &format!(" {}ULL, {}ULL,", a, b); }
    // the scratch area for struct values, when the code uses it: after everything else, the request's own like the request struct
    let (so, sn) = if scratch { prog.scratch } else { (0, 0) };
    if scratch { o += &format!(" {}ULL, {}ULL,", so, so + sn); }
    o += " 0, 0 };\n";
    let (ro, rs) = prog.request.unwrap_or((0, 0));
    let total = if scratch { so + sn } else { prog.total };
    o += &format!("static const MitoProg mito_prog = {{ {}, mito_colls, {}, {}, {}, mito_routes, {}, mito_unlogged, {}, {}, {}, {} }};\n", prog.colls.len(), prog.plain_end, total, api.eps.len(), prog.unlogged.len() + scratch as usize, ro, rs, so, sn);
    o
}

/// One readable C file for review: the whole program (prelude and helper interface, layout and routes as data, the C
/// runtime, every endpoint next to its source), preceded by the layout as a table. `source` is the api file's name and text.
pub fn emit(prog: &Program, api: &Api, source: (&str, &str)) -> Result<String, String> {
    let body = program_c(prog, api, Some(source))?;
    let mut o = format!("/* Generated by Mitochondria from {}: a complete program (the C runtime is runtime/mito.c; its interface is mito_*).\n\n", source.0);
    o += &layout_table(prog);
    o += "*/\n";
    Ok(o + &body)
}

/// The declared layout as a table: every collection, its columns and their offsets from the reservation's base.
pub fn layout_table(prog: &Program) -> String {
    let mut o = format!("   Layout: {} bytes reserved; plain values at 0..{}; collections from {}.\n", prog.total, prog.plain_end, prog.colls.first().map_or(prog.plain_end, |c| c.start));
    for g in prog.globals.iter().filter(|g| g.coll.is_none()) { o += &format!("   plain {:<20} {:<28} at {}\n", g.name, prog.show(&g.ty), g.off); }
    for (k, c) in prog.colls.iter().enumerate() {
        let kind = match c.kind { Kind::List(f) => format!("list cap {}{}", c.cap, if f == crate::layout::Full::DropOldest { " drop_oldest" } else { "" }), Kind::Pool(ch) => format!("pool cap {}{}", c.cap, if ch { " checked" } else { "" }) };
        let parent = c.parent.map_or(String::new(), |p| format!(", {} instances (one per item of coll {})", c.mult, p));
        o += &format!("   coll {} {}: {}{}; bytes {}..{}; header at {}", k, c.name, kind, parent, c.start, c.end, c.hdr);
        if !c.is_list() { o += &format!(", live bits at {}", c.live); }
        if c.checked() { o += &format!(", generations at {}", c.gen); }
        o += "\n";
        for (ci, col) in c.cols.iter().enumerate() {
            let t = match &col.kind { CKind::Scalar(s) => format!("{:?}", s).to_lowercase(), CKind::Arr(s, n) => format!("{:?}[{}]", s, n).to_lowercase(), CKind::StrLen => "string length u32".into(), CKind::StrBytes(n) => format!("string bytes [{}]", n) };
            o += &format!("     col {:<2} {:<22} {:<40} at {} + idx * {}\n", ci, col.path, t, col.off, col.kind.size());
        }
    }
    // tables: in SQLite, not in the reservation
    for (k, t) in prog.tables.iter().enumerate() {
        o += &format!("   table {} {} (SQLite {}){}\n", k, t.name, t.sql, t.parent.map_or(String::new(), |p| format!(", each row owned by a row of table {}", p)));
    }
    if !prog.tables.is_empty() { o += "   Schema:\n"; for line in prog.schema_sql().lines() { o += &format!("     {}\n", line); } }
    o
}

/// The program's tables for the store runtime: each column's name, type and limits, each table's CREATE TABLE and indexes.
fn store_tables_c(prog: &Program) -> String {
    let mut o = String::from("/* the program's tables (see the layout description at the top) */\n");
    for (t, tb) in prog.tables.iter().enumerate() {
        o += &format!("static const StoreCol mito_tcols{}[] = {{", t);
        for c in &tb.cols {
            let (has, lo, hi) = match c.range { Some((a, b)) if matches!(c.kind, crate::layout::TKind::Int(_)) => (1, a, b), _ => (0, 0, 0) };
            let max = match &c.kind { crate::layout::TKind::Text(m, _) => *m, crate::layout::TKind::Blob(sc, n) => sc.size() * n, _ => 0 };
            let ascii = matches!(c.kind, crate::layout::TKind::Text(_, true)) as u32;
            let refers = match c.kind { crate::layout::TKind::Ref(t) => t as i64, _ => -1 };
            o += &format!(" {{ {}, {}, {}, {}, {}, {}, {}, {}, {}, {} }},", c_lit(c.sql.as_bytes()), c.kind.code(), has, i64_lit(lo), i64_lit(hi), max, ascii, c_lit(prog.col_decl(c).as_bytes()), refers, c.null as u32);
        }
        o += " { 0, 0, 0, 0, 0, 0, 0, 0, -1, 0 } };\n";
    }
    o += "static const StoreTable mito_tables[] = {\n";
    for (t, tb) in prog.tables.iter().enumerate() {
        // another program's table: the indexes it must have, one per line (`u` unique or `i`, then the columns), checked at start
        let need: String = tb.unique.iter().map(|fs| (1, fs)).chain(tb.index.iter().map(|fs| (0, fs)))
            .map(|(u, fs)| format!("{}{}\n", if u == 1 { "u" } else { "i" }, fs.iter().map(|f| tb.cols.iter().find(|c| c.path == *f).map_or(f.clone(), |c| c.sql.clone())).collect::<Vec<_>>().join(","))).collect();
        o += &format!("    {{ {}, {}, {}, {}, mito_tcols{}, {}, {}, {} }},\n", c_lit(tb.sql.as_bytes()), c_lit(prog.table_create_sql(t).as_bytes()), tb.parent.map_or(-1, |p| p as i64), tb.cols.len(), t, c_lit(prog.table_index_sql(t).as_bytes()), tb.ext as u32, c_lit(need.as_bytes()));
    }
    o += &format!("}};\n#define MITO_NTABLES {}\n", prog.tables.len());
    o
}

/// The api source lines of an endpoint as C comments.
fn source_comment(ep: &Endpoint, (name, text): (&str, &str)) -> String {
    let mut o = format!("\n// {}:{}-{}\n", name, ep.lines.0, ep.lines.1);
    for l in text.lines().skip(ep.lines.0.saturating_sub(1) as usize).take((ep.lines.1 + 1).saturating_sub(ep.lines.0) as usize) {
        let l = l.trim_end();
        let l = if l.ends_with('\\') { format!("{} (ends in a backslash)", &l[..l.len() - 1]) } else { l.to_string() }; // a final backslash would join the next line onto the comment
        o += &format!("//   | {}\n", l);
    }
    o
}

// Receiver functions: `fn heal(self: Player, n: i64)` is the function `Player.heal`, and `self.heal(3)` is the call `.heal` with the receiver first. Which function it is
// depends on the receiver's type, known only while generating, so the analyses that follow calls treat it as reaching every function declared for `heal`.
thread_local! { static METHODS: std::cell::RefCell<std::collections::HashMap<String, Vec<u32>>> = std::cell::RefCell::new(Default::default()); }
/// Make the table of methods from the program's functions (every `Type.name`, by `name`).
pub(crate) fn set_methods(api: &Api) {
    METHODS.with(|m| {
        let mut m = m.borrow_mut();
        m.clear();
        for (k, f) in api.fns.iter().enumerate() { if f.recv { if let Some((_, name)) = f.name.rsplit_once('.') { m.entry(name.to_string()).or_default().push(k as u32); } } }
    });
}
/// The functions a call can reach: its function, or every function declared for the method an `x.f(...)` names.
fn callees(n: &str, r: &CallRes) -> Vec<u32> {
    match r {
        CallRes::User(k) => vec![*k],
        CallRes::Unbound if n.starts_with('.') => METHODS.with(|m| m.borrow().get(&n[1..]).cloned().unwrap_or_default()),
        _ => Vec::new(),
    }
}

/// Does this expression read local `slot`?
fn expr_uses_slot(e: &Expr, slot: u32) -> bool {
    match e {
        Expr::Var(_, VarRes::Local(s)) => *s == slot,
        Expr::Field(b, _) | Expr::Un(_, b) => expr_uses_slot(b, slot),
        Expr::Index(a, b) | Expr::Bin(_, a, b) => expr_uses_slot(a, slot) || expr_uses_slot(b, slot),
        Expr::Call(_, args, _) => args.iter().any(|a| expr_uses_slot(a, slot)),
        _ => false,
    }
}

/// Do these statements call (directly or through other functions) one of instance `inst`'s functions that changes state?
fn changes_instance(api: &Api, inst: &str, stmts: &[Stmt]) -> bool {
    fn ex(e: &Expr, out: &mut Vec<u32>) {
        match e {
            Expr::Call(n, a, r) => { out.extend(callees(n, r)); for x in a { ex(x, out); } }
            Expr::Field(b, ..) | Expr::Un(_, b) => ex(b, out),
            Expr::Index(a, b) | Expr::Bin(_, a, b) => { ex(a, out); ex(b, out); }
            _ => {}
        }
    }
    fn js(j: &Json, out: &mut Vec<u32>) { match j { Json::Obj(f) => for (_, v) in f { js(v, out); }, Json::Arr(it, inner) => { ex(&it.coll, out); if let Some(w) = &it.wher { ex(w, out); } js(inner, out); } Json::E(e) => ex(e, out), Json::Empty | Json::Html => {} } }
    fn st(stmts: &[Stmt], out: &mut Vec<u32>) {
        for s in stmts {
            match s {
                Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(e, out),
                Stmt::Remove(c, e) => { ex(c, out); ex(e, out); }
                Stmt::Clear(c) => ex(c, out),
                Stmt::Move(a, f, t2, n) => { ex(a, out); ex(f, out); ex(t2, out); ex(n, out); }
                Stmt::Copy(d, src, f, t2, n) => { ex(d, out); ex(src, out); ex(f, out); ex(t2, out); ex(n, out); }
                Stmt::Fill(a, f, t2, n) => { ex(a, out); ex(f, out); ex(t2, out); ex(n, out); }
                Stmt::Sort(a, f, t2, n) => { ex(a, out); ex(f, out); ex(t2, out); ex(n, out); }
                Stmt::Alloc { coll, .. } => ex(coll, out),
                Stmt::Assign(a, b) => { ex(a, out); ex(b, out); }
                Stmt::If(c, t, e) => { ex(c, out); st(t, out); st(e, out); }
                Stmt::Each(it, b) => { ex(&it.coll, out); if let Some(w) = &it.wher { ex(w, out); } st(b, out); }
                Stmt::Respond(_, j) => js(j, out),
                Stmt::Forward { key, fields, .. } => { ex(key, out); for f in fields { ex(&f.1, out); } }
                _ => {}
            }
        }
    }
    let (mut todo, mut seen) = (Vec::new(), Vec::new());
    st(stmts, &mut todo);
    let pre = format!("{}.", inst);
    while let Some(k) = todo.pop() {
        if seen.contains(&k) { continue; }
        seen.push(k);
        let f = &api.fns[k as usize];
        if f.name.starts_with(&pre) && writes(api, &f.body, &mut Vec::new()) { return true; }
        st(&f.body, &mut todo);
    }
    false
}

/// Does a function body contain a loop (which may collect items and be left by `return`)?
/// The range of a load of a narrow integer from state (`LD_U8(...)`, `(i64)LD_I32(...)`): what its C type can hold.
fn load_interval(e: &str) -> Option<(i64, i64)> {
    let e = e.strip_prefix("(i64)").unwrap_or(e);
    let (r, rest) = [("LD_I8(", (-128, 127)), ("LD_U8(", (0, 255)), ("LD_I16(", (-32768, 32767)), ("LD_U16(", (0, 65535)),
                     ("LD_I32(", (i32::MIN as i64, i32::MAX as i64)), ("LD_U32(", (0, u32::MAX as i64))]
        .iter().find_map(|(p, r)| e.strip_prefix(p).map(|rest| (*r, rest)))?;
    // the load's own parenthesis must close at the very end (not `LD_U8(a, i) + 1`)
    let mut depth = 1;
    for (i, ch) in rest.char_indices() {
        match ch { '(' => depth += 1, ')' => { depth -= 1; if depth == 0 { return if i + 1 == rest.len() { Some(r) } else { None }; } } _ => {} }
    }
    None
}

/// Whether two generated copies of a loop are the same code up to the names of their own temporaries (`tmp` names numbered above
/// `n_pre`, the counter before the guard): those are renamed in order of first appearance in each copy, and the results compared.
/// A copy that uses a name the guard (`guard`) declared is never the same.
fn same_code(n_pre: usize, guard: &str, a: &str, b: &str) -> bool {
    fn tokens(s: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c.is_ascii_alphabetic() || c == b'_' {
                let st = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
                out.push(&s[st..i]);
            } else if c.is_ascii_digit() {
                let st = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.') { i += 1; }
                out.push(&s[st..i]);
            } else if c.is_ascii_whitespace() { i += 1; }
            else { out.push(&s[i..i + 1]); i += 1; }
        }
        out
    }
    let (ta, tb, tg) = (tokens(a), tokens(b), tokens(guard));
    if ta.len() != tb.len() { return false; }
    // a temporary: lowercase letters, an optional `_`, then its number (see `tmp`)
    let fresh = |t: &str| {
        let d = t.trim_start_matches(|c: char| c.is_ascii_lowercase());
        let d = if d.len() < t.len() { d.strip_prefix('_').unwrap_or(d) } else { return false };
        !["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"].contains(&t) && !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()) && d.parse::<usize>().map_or(false, |n| n > n_pre)
    };
    let guard_names: std::collections::HashSet<&str> = tg.iter().copied().filter(|t| fresh(t)).collect();
    let (mut ma, mut mb) = (std::collections::HashMap::new(), std::collections::HashMap::new());
    for (x, y) in ta.iter().zip(&tb) {
        if guard_names.contains(x) || guard_names.contains(y) { return false; }
        let (fx, fy) = (fresh(x), fresh(y));
        if fx != fy { return false; }
        if fx {
            let (n, m) = (ma.len(), mb.len());
            if *ma.entry(*x).or_insert(n) != *mb.entry(*y).or_insert(m) { return false; }
        } else if x != y { return false; }
    }
    true
}

fn is_ident(x: &str) -> bool { x.bytes().next().map_or(false, |b| b.is_ascii_alphabetic() || b == b'_') && x.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') }

/// Whether a line of generated C may change the variable `x`: an assignment (`x = `, `x += `, ...), `x++`/`x--`, `++x`/`--x`, or its
/// address taken (`&x`). Errs on the side of yes.
fn writes_name(line: &str, x: &str) -> bool {
    let b = line.as_bytes();
    let mut from = 0;
    while let Some(p) = line[from..].find(x) {
        let (at, end) = (from + p, from + p + x.len());
        from = end;
        let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        if (at > 0 && word(b[at - 1])) || (end < b.len() && word(b[end])) { continue; }
        let before = line[..at].trim_end();
        if before.ends_with('&') && !before.ends_with("&&") || before.ends_with("++") || before.ends_with("--") { return true; }
        let after = line[end..].trim_start().as_bytes();
        match after {
            [b'+', b'+', ..] | [b'-', b'-', ..] => return true,
            [b'=', n, ..] if *n != b'=' => return true,
            [b'='] => return true,
            [op, b'=', ..] if b"+-*/%&|^".contains(op) => return true,
            [b'<', b'<', b'=', ..] | [b'>', b'>', b'=', ..] => return true,
            _ => {}
        }
    }
    false
}

/// Whether a function body holds a loop that collects items (the `collect` rule of `Stmt::Each`), at any depth: only then does the
/// function need a selection mark to drop what such a loop collected when a `return` leaves it early.
/// A value a generated C function takes as an argument (see `call_spec`).
fn spec_arg(v: &CV) -> bool {
    matches!(v, CV::Coll(..) | CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) | CV::Txt(_) | CV::Int(_) | CV::Flt(_) | CV::Bool(_) | CV::Any(_) | CV::Item(..) | CV::Ref(..) | CV::Null | CV::Plain(..))
}

fn has_collecting_loop(api: &Api, stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Each(it, body) => allocates(api, body, &mut Vec::new()) || (it.wher.is_some() && writes(api, body, &mut Vec::new())) || has_collecting_loop(api, body),
        Stmt::If(_, t, e) => has_collecting_loop(api, t) || has_collecting_loop(api, e),
        _ => false,
    })
}

/// Which functions are part of a cycle of calls (directly or through others).
fn recursive_fns(api: &Api) -> Vec<bool> {
    fn calls(stmts: &[Stmt], out: &mut Vec<u32>) {
        fn ex(e: &Expr, out: &mut Vec<u32>) {
            match e {
                Expr::Call(n, a, r) => { out.extend(callees(n, r)); for x in a { ex(x, out); } }
                Expr::Field(b, ..) | Expr::Un(_, b) => ex(b, out),
                Expr::Index(a, b) | Expr::Bin(_, a, b) => { ex(a, out); ex(b, out); }
                _ => {}
            }
        }
        for s in stmts {
            match s {
                Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(e, out),
                Stmt::Remove(c, e) => { ex(c, out); ex(e, out); }
                Stmt::Clear(c) => ex(c, out),
                Stmt::Move(a, f, t2, n) => { ex(a, out); ex(f, out); ex(t2, out); ex(n, out); }
                Stmt::Copy(d, src, f, t2, n) => { ex(d, out); ex(src, out); ex(f, out); ex(t2, out); ex(n, out); }
                Stmt::Fill(a, f, t2, n) => { ex(a, out); ex(f, out); ex(t2, out); ex(n, out); }
                Stmt::Sort(a, f, t2, n) => { ex(a, out); ex(f, out); ex(t2, out); ex(n, out); }
                Stmt::Alloc { coll, .. } => ex(coll, out),
                Stmt::Assign(a, b) => { ex(a, out); ex(b, out); }
                Stmt::If(c, t, e) => { ex(c, out); calls(t, out); calls(e, out); }
                Stmt::Each(it, b) => { ex(&it.coll, out); if let Some(h) = &it.hi { ex(h, out); } if let Some(w) = &it.wher { ex(w, out); } if let Some(l) = &it.limit { ex(l, out); } if let Some(a) = &it.after { ex(a, out); } calls(b, out); }
                Stmt::Respond(_, j) => json_calls(j, out),
                Stmt::Forward { key, fields, .. } => { ex(key, out); for f in fields { ex(&f.1, out); } }
                _ => {}
            }
        }
        fn json_calls(j: &Json, out: &mut Vec<u32>) {
            match j {
                Json::Obj(f) => for (_, v) in f { json_calls(v, out); },
                Json::Arr(it, inner) => { let mut v = Vec::new(); calls(&[Stmt::Each(it.clone(), Vec::new())], &mut v); out.extend(v); json_calls(inner, out); }
                Json::E(e) => ex(e, out),
                Json::Empty | Json::Html => {}
            }
        }
    }
    let direct: Vec<Vec<u32>> = api.fns.iter().map(|f| { let mut v = Vec::new(); calls(&f.body, &mut v); v }).collect();
    (0..api.fns.len()).map(|k| {
        let (mut seen, mut todo) = (vec![false; api.fns.len()], direct[k].clone());
        while let Some(x) = todo.pop() { if x as usize == k { return true; } if !seen[x as usize] { seen[x as usize] = true; todo.extend(direct[x as usize].iter().copied()); } }
        false
    }).collect()
}

/// The size a function's body has once its calls are inlined (statements and expression nodes, a call's callee counted in full
/// unless it is recursive or itself outlined), above which it is compiled once as a C function of its own and called: inlining
/// everything copies a deep call tree into each place that reaches it, and the copies multiply with the tree's depth.
const OUTLINE_SIZE: usize = 4000;

/// Which functions are compiled as C functions of their own though not recursive: those whose inlined size passes OUTLINE_SIZE.
/// A function that answers the request itself (`respond`, `forward`) stays inlined.
fn outlined_fns(api: &Api, recursive: &[bool]) -> Vec<bool> {
    fn ex(api: &Api, e: &Expr, memo: &mut Vec<Option<usize>>, out: &mut Vec<bool>, rec: &[bool]) -> usize {
        match e {
            Expr::Call(nm, a, r) => {
                let mut n = 1 + a.iter().map(|x| ex(api, x, memo, out, rec)).sum::<usize>();
                for k in callees(nm, r) { if !rec[k as usize] { let s = size(api, k as usize, memo, out, rec); if !out[k as usize] { n += s; } } }
                n
            }
            Expr::Field(b, ..) | Expr::Un(_, b) => 1 + ex(api, b, memo, out, rec),
            Expr::Index(a, b) | Expr::Bin(_, a, b) => 1 + ex(api, a, memo, out, rec) + ex(api, b, memo, out, rec),
            _ => 1,
        }
    }
    fn json(api: &Api, j: &Json, memo: &mut Vec<Option<usize>>, out: &mut Vec<bool>, rec: &[bool]) -> usize {
        match j {
            Json::Obj(f) => f.iter().map(|(_, v)| 1 + json(api, v, memo, out, rec)).sum(),
            Json::Arr(it, inner) => 1 + stmts(api, &[Stmt::Each(it.clone(), Vec::new())], memo, out, rec) + json(api, inner, memo, out, rec),
            Json::E(e) => ex(api, e, memo, out, rec),
            Json::Empty | Json::Html => 1,
        }
    }
    fn stmts(api: &Api, ss: &[Stmt], memo: &mut Vec<Option<usize>>, out: &mut Vec<bool>, rec: &[bool]) -> usize {
        let mut n = 0;
        for s in ss {
            n += 1;
            let mut e = |x: &Expr, memo: &mut Vec<Option<usize>>, out: &mut Vec<bool>| ex(api, x, memo, out, rec);
            n += match s {
                Stmt::Let(_, _, x) | Stmt::Var(_, _, x) | Stmt::Do(x) | Stmt::Return(Some(x)) | Stmt::Clear(x) => e(x, memo, out),
                Stmt::Remove(a, b) | Stmt::Assign(a, b) => e(a, memo, out) + e(b, memo, out),
                Stmt::Move(a, b, c, d) | Stmt::Fill(a, b, c, d) | Stmt::Sort(a, b, c, d) => e(a, memo, out) + e(b, memo, out) + e(c, memo, out) + e(d, memo, out),
                Stmt::Copy(a, b, c, d, f) => e(a, memo, out) + e(b, memo, out) + e(c, memo, out) + e(d, memo, out) + e(f, memo, out),
                Stmt::Alloc { coll, .. } => e(coll, memo, out),
                Stmt::If(c, t, f) => e(c, memo, out) + stmts(api, t, memo, out, rec) + stmts(api, f, memo, out, rec),
                Stmt::Each(it, b) => {
                    let mut m = e(&it.coll, memo, out);
                    for x in [&it.hi, &it.wher, &it.limit, &it.after].into_iter().flatten() { m += e(x, memo, out); }
                    m + stmts(api, b, memo, out, rec)
                }
                Stmt::Respond(_, j) => json(api, j, memo, out, rec),
                Stmt::Forward { key, fields, .. } => e(key, memo, out) + fields.iter().map(|f| e(&f.1, memo, out)).sum::<usize>(),
                _ => 0,
            };
        }
        n
    }
    fn answers(ss: &[Stmt]) -> bool {
        ss.iter().any(|s| match s {
            Stmt::Respond(..) | Stmt::Forward { .. } => true,
            Stmt::If(_, t, e) => answers(t) || answers(e),
            Stmt::Each(_, b) => answers(b),
            _ => false,
        })
    }
    fn size(api: &Api, k: usize, memo: &mut Vec<Option<usize>>, out: &mut Vec<bool>, rec: &[bool]) -> usize {
        if let Some(s) = memo[k] { return s; }
        memo[k] = Some(1); // (not reached again: only recursive functions call back, and they are not followed)
        let s = stmts(api, &api.fns[k].body, memo, out, rec);
        memo[k] = Some(s);
        if s > OUTLINE_SIZE && !answers(&api.fns[k].body) && api.fns[k].test.is_none() { out[k] = true; }
        s
    }
    let mut memo = vec![None; api.fns.len()];
    let mut out = vec![false; api.fns.len()];
    for k in 0..api.fns.len() { if !recursive[k] { size(api, k, &mut memo, &mut out, recursive); } }
    if std::env::var("MITO_OUTLINE_TRACE").is_ok() {
        let mut v: Vec<(usize, &str, bool)> = (0..api.fns.len()).filter_map(|k| memo[k].map(|s| (s, api.fns[k].name.as_str(), out[k]))).collect();
        v.sort(); v.reverse();
        for (s, n, o) in v.iter().take(40) { eprintln!("{:>8} {}{}", s, n, if *o { "  (outlined)" } else { "" }); }
    }
    out
}

/// `linkage`: "static " in a whole C program (only its `mito_*` interface is external), "" when the Rust runtime looks
/// endpoints up by name.
/// Generate the program twice when that helps. A plain integer field without a declared range whose every store in the program has a
/// proven range (`size = n` with `n: i64 in 2..900`) is given the union of those ranges and 0, its value at start, for its reads; the
/// second build is kept only if every store it compiled still lies within the ranges it assumed. Nothing else writes plain state:
/// a failed request puts back values those same stores wrote.
fn generate_with(prog: &Program, api: &Api, source: Option<(&str, &str)>, linkage: &str, tests: bool) -> Generated {
    set_methods(api);
    type Ranges = std::collections::HashMap<usize, (i64, i64)>;
    type Pools = std::collections::HashSet<usize>;
    type Loops = std::collections::HashMap<(usize, u32), (i64, i64)>;
    // One build with `ranges`, `never` (pools nothing removes from) and `loops` (intervals of loop-carried variables at the head of their loops) assumed. Returns it with what it saw, and
    // whether everything it compiled agrees with what it assumed.
    let build = |ranges: &Ranges, never: &Pools, loops: &Loops| -> (Generated, std::collections::HashMap<usize, Option<(i64, i64)>>, Pools, LoopInv, bool) {
        FIELD_RANGES.with(|f| *f.borrow_mut() = (ranges.clone(), Default::default()));
        REMOVAL.with(|r| *r.borrow_mut() = (never.clone(), Default::default()));
        LOOP_INV.with(|l| *l.borrow_mut() = LoopInv { assumed: loops.clone(), ..Default::default() });
        let g = generate_once(prog, api, source, linkage, tests);
        let seen = FIELD_RANGES.with(|f| std::mem::take(&mut *f.borrow_mut()).1);
        let removed = REMOVAL.with(|r| std::mem::take(&mut r.borrow_mut().1));
        let inv = LOOP_INV.with(|l| std::mem::take(&mut *l.borrow_mut()));
        let ranges_hold = ranges.iter().all(|(off, (lo, hi))| match seen.get(off) { Some(Some((a, b))) => a >= lo && b <= hi, Some(None) => false, None => true });
        let loops_hold = loops.iter().all(|(k, (lo, hi))| {
            let entry_ok = inv.entries.get(k).map_or(true, |(a, b)| a >= lo && b <= hi);
            let assign_ok = match inv.assigns.get(k) { Some(Some((a, b))) => a >= lo && b <= hi, Some(None) => false, None => true };
            entry_ok && assign_ok
        });
        let holds = ranges_hold && loops_hold && removed.is_disjoint(never) && g.notes.is_empty();
        (g, seen, removed, inv, holds)
    };
    let (mut best, mut seen, mut removed, inv0, _) = build(&Default::default(), &Default::default(), &Default::default());
    if !best.notes.is_empty() { return best; }
    let mut entries = inv0.entries;
    let mut assumed: Ranges = Default::default();
    let mut never: Pools = Default::default();
    // What one build learns lets the next prove more (a field fed by a loop counter, an array fed by that field, an index loaded from that array; a pool nothing removes from has no dead slot
    // below its high-water mark): build again with what the last one saw, until it stops changing. A loop-carried variable has, at the head of its loop, an interval that is guessed (what it
    // had on entering) and widened by what the loop's assignments give under the guess, until the guess holds. A build is kept only if what it compiled agrees with what it assumed.
    for _ in 0..6 {
        let next: Ranges = seen.iter().filter_map(|(off, r)| r.map(|(lo, hi)| (*off, (lo.min(0), hi.max(0))))).collect();
        let next_never: Pools = (0..prog.colls.len()).filter(|c| matches!(prog.colls[*c].kind, Kind::Pool(_)) && !removed.contains(c)).collect();
        // the loop guesses: from the entries, widened until a build agrees
        let mut guess: Loops = entries.clone();
        let mut verified = None;
        for round in 0..6 {
            let (g, sn, rm, inv, holds) = build(&next, &next_never, &guess);
            if holds { verified = Some((g, sn, rm, inv)); break; }
            // widen: an assignment outside the guess moves that end out to what the assignment gave (the first rounds: a variable that is masked or tested to a range each round settles there) or, later,
            // to infinity; one with no known interval drops the variable's guess. Every guess is checked by a build before it is used.
            let mut wide: Loops = Default::default();
            for (k, (lo, hi)) in &guess {
                match inv.assigns.get(k) {
                    Some(None) => {}
                    Some(Some((a, b))) => { wide.insert(*k, (if a < lo { if round < 3 { *a } else { i64::MIN } } else { *lo }, if b > hi { if round < 3 { *b } else { i64::MAX } } else { *hi })); }
                    None => { wide.insert(*k, (*lo, *hi)); }
                }
                if let (Some((ea, eb)), Some(w)) = (inv.entries.get(k), wide.get_mut(k)) { if *ea < w.0 { w.0 = if round < 3 { *ea } else { i64::MIN }; } if *eb > w.1 { w.1 = if round < 3 { *eb } else { i64::MAX }; } }
            }
            if wide == guess { break; }
            guess = wide;
        }
        // nothing assumed of loops (an empty guess) is always a verified build
        let Some((g, sn, rm, inv)) = verified.or_else(|| { let (g, sn, rm, inv, h) = build(&next, &next_never, &Default::default()); if h { Some((g, sn, rm, inv)) } else { None } }) else { break };
        let changed = next != assumed || next_never != never;
        best = g;
        assumed = next;
        never = next_never;
        seen = sn;
        removed = rm;
        entries = inv.entries;
        if !changed { break; }
    }
    best
}

fn generate_once(prog: &Program, api: &Api, source: Option<(&str, &str)>, linkage: &str, tests: bool) -> Generated {
    LITS.with(|l| *l.borrow_mut() = (Vec::new(), Vec::new()));
    SCRATCH.with(|s| *s.borrow_mut() = Default::default());
    let mut src = String::from(PRELUDE);
    let mut notes = Vec::new();
    let recursive = recursive_fns(api);
    OUTLINED.with(|o| *o.borrow_mut() = outlined_fns(api, &recursive));
    let mut done: Vec<Spec> = Vec::new(); // functions for recursive calls, generated by endpoints that compiled
    let mut zeros: Vec<Option<Vec<(usize, usize)>>> = Vec::new();
    let mut reads: Vec<bool> = Vec::new();
    let mut costs: Vec<Costs> = Vec::new();
    for (k, ep) in api.eps.iter().enumerate() {
        let mut touched: Option<Vec<(usize, usize)>> = None;
        let mut read_only = false;
        let mut lets = (0u32, 0usize, Vec::new());
        let (mut slot_kinds, mut ret_kinds): (Vec<((Option<u32>, u32), K)>, Vec<(u32, K)>) = (Vec::new(), Vec::new());
        let mut tries = 0;
        let (res, body, specs) = loop {
            tries += 1;
            let specs = std::cell::RefCell::new(done.clone());
            let mut g = G::new(prog, api, ep, &slot_kinds, &ret_kinds, &recursive, &specs);
            let mut env: Vec<Option<CV>> = vec![None; ep.slots as usize];
            let r = g.inputs(ep, &mut env).and_then(|_| g.block(&ep.body, &mut env));
            let (widen, body, at) = (g.widen, std::mem::take(&mut g.body), g.at);
            read_only = !g.wrote_state.get() && !g.uses_spec; // a call of a recursive or outlined function is compiled apart: not known to be read-only
            // the safety net: whatever the statements looked like, code that saves to the undo log, writes a table or frees an item is not read-only
            if read_only && ["SAVE_(", "->tinsert(", "->tset(", "->tadd(", "->tdel(", "->tclear(", "->free_(", "->free_val(", "->alloc(", "->clear_coll(", "->store(", "->store_elem(", "->store_field(", "MOVE_(", "COPY_("].iter().any(|t| body.contains(t)) { read_only = false; }
            lets = (g.region_lets.len() as u32, g.region_peak, std::mem::take(&mut g.region_unbounded));
            touched = if g.uses_spec { None } else { let mut t = std::mem::take(&mut g.rq_touched); t.sort(); let mut m: Vec<(usize, usize)> = Vec::new(); for r in t { match m.last_mut() { Some(l) if r.0 <= l.1 => l.1 = l.1.max(r.1), _ => m.push(r) } } Some(m) };
            drop(g);
            let r = r.map_err(|e| place_err(at, e));
            match (r, widen) {
                (Err(_), Some(w)) if tries < 32 => match w {
                    Widen::Slot(f, sl, k) => match slot_kinds.iter_mut().find(|x| x.0 == (f, sl)) { Some(x) => x.1 = unify(x.1, k), None => slot_kinds.push(((f, sl), k)) },
                    Widen::Ret(f, k) => match ret_kinds.iter_mut().find(|x| x.0 == f) { Some(x) => x.1 = unify(x.1, k), None => ret_kinds.push((f, k)) },
                },
                (r, _) => break (r, body, specs.into_inner()),
            }
        };
        // every value's kind is known when the program is compiled: one widened to a run-time kind does not load
        let res = res.and_then(|_| {
            let run_time = slot_kinds.iter().any(|x| x.1 == K::Any) || ret_kinds.iter().any(|x| x.1 == K::Any) || specs[done.len()..].iter().any(|sp| sp.ret == K::Any || sp.proto.contains("Val p"));
            if !run_time { return Ok(()); }
            no(WHY.with(|w| w.borrow().first().cloned()).unwrap_or_else(|| "a value whose kind is known only at run time".into()))
        });
        WHY.with(|w| w.borrow_mut().clear());
        match res.and_then(|_| check_profile(&body).map_err(|e| format!("C safety profile: {}", e))) {
            Ok(()) => {
                let name = format!("ep{}", k);
                let head = match source { Some(sf) => source_comment(ep, sf), None => format!("\n// {}\n", ep.text) };
                let new = &specs[done.len()..];
                if !new.is_empty() {
                    src += &format!("\n// functions for recursive and outlined calls, first used by {}, one per combination of argument kinds\n", ep.text);
                    for sp in new { src += &format!("{}int {}{};\n", linkage, sp.name, sp.proto); }
                    for sp in new { src += &format!("{}int {}{} {{\n#undef AT_\n#define AT_ 0\n    int st; u8 *b = c->base; (void)st; (void)b; (void)dep; u8 *dr_ = c->dirty; u8 *cm_ = c->committed; int jn_ = c->jon; u8 *tx_ = c->text; (void)dr_; (void)cm_; (void)jn_; (void)tx_;\n{}    return 0;\n}}\n", linkage, sp.name, sp.proto, sp.body.as_deref().unwrap_or("")); }
                }
                done = specs;
                zeros.push(touched);
                reads.push(read_only);
                costs.push(Costs { lets: lets.clone(), ..count_costs(&body) });
                src += &format!("{}{}int {}(Ctx *c) {{\n#undef AT_\n#define AT_ 0\n    int st; u8 *b = c->base; (void)st; (void)b; u8 *dr_ = c->dirty; u8 *cm_ = c->committed; int jn_ = c->jon; u8 *tx_ = c->text; (void)dr_; (void)cm_; (void)jn_; (void)tx_;\n{}    return 0;\n}}\n", head, linkage, name, body);
            }
            Err(why) => {
                zeros.push(None);
                reads.push(false);
                costs.push(Costs::default());
                if let Some(sf) = source { src += &source_comment(ep, sf); src += &format!("// not compiled: {}\n", why); }
                notes.push(match split_place(&why) { Some((place, rest)) => format!("{}{}: {}", place, ep.text, rest), None => format!("{}: {}", ep.text, why) });
            }
        }
    }
    // the tests: each a function the runner calls outside any request (it reads the state; requests change it)
    if tests {
        for (fk, f) in api.fns.iter().enumerate().filter(|x| x.1.test.is_some()) {
            let k = api.fns[..fk].iter().filter(|x| x.test.is_some()).count();
            let title = f.test.clone().unwrap_or_default();
            let ep = Endpoint { method: "TEST".into(), segs: Vec::new(), body: Vec::new(), text: format!("test \"{}\"", title), slots: f.slots, lines: (0, 0), name: f.name.clone(), resp: crate::lang::RType::Name("null".into()), inputs: Vec::new() };
            if writes_in(api, &f.body, &mut Vec::new(), true) {
                let at = match f.body.first() { Some(Stmt::At(l, c)) => format!("{}:{}: ", l, c), _ => String::new() };
                notes.push(format!("{}{}: calls a function that changes the state: a test changes it only through requests", at, ep.text));
                continue;
            }
            let mut slot_kinds: Vec<((Option<u32>, u32), K)> = Vec::new();
            let ret_kinds: Vec<(u32, K)> = Vec::new();
            let mut tries = 0;
            let (res, body, specs) = loop {
                tries += 1;
                let specs = std::cell::RefCell::new(done.clone());
                let mut g = G::new(prog, api, &ep, &slot_kinds, &ret_kinds, &recursive, &specs);
                g.test = true;
                let mut env: Vec<Option<CV>> = vec![None; f.slots as usize];
                let r = g.block(&f.body, &mut env);
                let (widen, body, at) = (g.widen, std::mem::take(&mut g.body), g.at);
                drop(g);
                let r = r.map_err(|e| place_err(at, e));
                match (r, widen) {
                    (Err(_), Some(Widen::Slot(fr, sl, kk))) if tries < 32 => match slot_kinds.iter_mut().find(|x| x.0 == (fr, sl)) { Some(x) => x.1 = unify(x.1, kk), None => slot_kinds.push(((fr, sl), kk)) },
                    (r, _) => break (r, body, specs.into_inner()),
                }
            };
            WHY.with(|w| w.borrow_mut().clear());
            match res.and_then(|_| check_profile(&body).map_err(|e| format!("C safety profile: {}", e))) {
                Ok(()) => {
                    let new = &specs[done.len()..];
                    for sp in new { src += &format!("{}int {}{};\n", linkage, sp.name, sp.proto); }
                    for sp in new { src += &format!("{}int {}{} {{\n#undef AT_\n#define AT_ 0\n    int st; u8 *b = c->base; (void)st; (void)b; (void)dep; u8 *dr_ = c->dirty; u8 *cm_ = c->committed; int jn_ = c->jon; u8 *tx_ = c->text; (void)dr_; (void)cm_; (void)jn_; (void)tx_;\n{}    return 0;\n}}\n", linkage, sp.name, sp.proto, sp.body.as_deref().unwrap_or("")); }
                    done = specs;
                    src += &format!("\n// {}\n{}int test{}(Ctx *c, Txt *tmsg) {{\n#undef AT_\n#define AT_ 0\n    int st; u8 *b = c->base; (void)st; (void)b; (void)tmsg; u8 *dr_ = c->dirty; u8 *cm_ = c->committed; int jn_ = c->jon; u8 *tx_ = c->text; (void)dr_; (void)cm_; (void)jn_; (void)tx_;\n{}    return 0;\n}}\n", ep.text, linkage, k, body);
                }
                Err(why) => notes.push(match split_place(&why) { Some((place, rest)) => format!("{}{}: {}", place, ep.text, rest), None => format!("{}: {}", ep.text, why) }),
            }
        }
    }
    let lits = LITS.with(|l| l.borrow().0.clone());
    let scratch = SCRATCH.with(|s| { let s = s.borrow(); s.bottom + s.top > 0 });
    Generated { scratch, src, notes, lits, zeros, reads, costs }
}

/// What generating a C function's body for one guess of its result kind gave.
enum SpecErr { Conflict(K), Other(String) }

/// Does running these statements (including inlined calls) write state or allocate? A loop with `where` whose body
/// does collects its items first, as the interpreter does, so the body cannot change which items the filter keeps.
fn writes(api: &Api, stmts: &[Stmt], seen: &mut Vec<u32>) -> bool { writes_in(api, stmts, seen, false) }
/// `locals`: an assignment to a field or element of a local (`v.x = 1;`, a struct value of the code's own: the code generator checks that it is one) is not a write of state here.
/// What a called function does is judged strictly: through a parameter it may write state.
/// Does this code change local `slot` (a parameter) or something inside it: an assignment to it or a part of it, an operation on a collection or array that is part of it, or
/// handing it to a function that takes that argument as a `view` (which may change it)?
fn param_written(api: &Api, stmts: &[Stmt], slot: u32) -> bool {
    let rooted = |e: &Expr| matches!(G::root_local(e), Some((_, s)) if s == slot);
    fn passes(api: &Api, e: &Expr, rooted: &dyn Fn(&Expr) -> bool) -> bool {
        match e {
            Expr::Call(nm, a, r) => {
                let viewed = |k: u32, j: usize| api.fns[k as usize].modes.get(j).copied() == Some(1);
                a.iter().enumerate().any(|(j, x)| (rooted(x) && callees(nm, r).into_iter().any(|k| viewed(k, j))) || passes(api, x, rooted))
            }
            Expr::Field(b, ..) | Expr::Un(_, b) => passes(api, b, rooted),
            Expr::Index(a, b) | Expr::Bin(_, a, b) => passes(api, a, rooted) || passes(api, b, rooted),
            _ => false,
        }
    }
    stmts.iter().any(|s| match s {
        Stmt::Assign(l, r) => rooted(l) || passes(api, r, &rooted),
        Stmt::Remove(c, _) | Stmt::Clear(c) | Stmt::Move(c, ..) | Stmt::Copy(c, ..) | Stmt::Sort(c, ..) | Stmt::Fill(c, ..) => rooted(c),
        Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Do(e) | Stmt::Return(Some(e)) => passes(api, e, &rooted),
        Stmt::If(c, t, e) => passes(api, c, &rooted) || param_written(api, t, slot) || param_written(api, e, slot),
        Stmt::Each(it, b) => passes(api, &it.coll, &rooted) || param_written(api, b, slot),
        _ => false,
    })
}

fn writes_in(api: &Api, stmts: &[Stmt], seen: &mut Vec<u32>, locals: bool) -> bool {
    fn ex(api: &Api, e: &Expr, seen: &mut Vec<u32>) -> bool {
        match e {
            Expr::Call(_, _, CallRes::Builtin(B::Truncate)) => true,
            Expr::Call(nm, _, CallRes::Unbound) if matches!(nm.as_str(), ".add" | ".try_add" | ".remove" | ".clear" | ".fill" | ".move" | ".sort" | ".copy_from" | ".truncate") => true,
            Expr::Call(nm, a, r) => a.iter().any(|x| ex(api, x, seen)) || callees(nm, r).into_iter().any(|k| if seen.contains(&k) { false } else { seen.push(k); writes(api, &api.fns[k as usize].body, seen) }),
            Expr::Field(b, ..) | Expr::Un(_, b) => ex(api, b, seen),
            Expr::Index(a, b) | Expr::Bin(_, a, b) => ex(api, a, seen) || ex(api, b, seen),
            _ => false,
        }
    }
    stmts.iter().any(|s| match s {
        Stmt::Assign(l @ (Expr::Field(..) | Expr::Index(..)), r) if locals && G::root_local(l).is_some() => ex(api, l, seen) || ex(api, r, seen),
        Stmt::Alloc { .. } | Stmt::Move(..) | Stmt::Copy(..) | Stmt::Sort(..) | Stmt::Fill(..) | Stmt::Clear(..) | Stmt::Assign(Expr::Field(..) | Expr::Index(..), _) => true,
        Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(api, e, seen),
        Stmt::Remove(c, e) => ex(api, c, seen) || ex(api, e, seen),
        Stmt::Clear(c) => ex(api, c, seen),
        Stmt::Move(a, f, t2, n) => ex(api, a, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
        Stmt::Copy(d, src, f, t2, n) => ex(api, d, seen) || ex(api, src, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
        Stmt::Fill(a, f, t2, n) => ex(api, a, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
        Stmt::Sort(a, f, t2, n) => ex(api, a, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
        Stmt::Assign(a, b) => ex(api, a, seen) || ex(api, b, seen),
        Stmt::If(c, t, e) => ex(api, c, seen) || writes_in(api, t, seen, locals) || writes_in(api, e, seen, locals),
        Stmt::Each(it, b) => ex(api, &it.coll, seen) || it.hi.as_ref().map_or(false, |h| ex(api, h, seen)) || writes_in(api, b, seen, locals),
        Stmt::Forward { key, fields, .. } => ex(api, key, seen) || fields.iter().any(|f| ex(api, &f.1, seen)),
        _ => false,
    })
}

/// Does running these statements (including inlined calls) allocate? A loop whose body allocates collects its items
/// first, as the interpreter does, so it does not visit items its body adds.
fn allocates(api: &Api, stmts: &[Stmt], seen: &mut Vec<u32>) -> bool {
    fn ex(api: &Api, e: &Expr, seen: &mut Vec<u32>) -> bool {
        match e {
            Expr::Call(nm, _, CallRes::Unbound) if matches!(nm.as_str(), ".add" | ".try_add") => true,
            Expr::Call(nm, a, r) => a.iter().any(|x| ex(api, x, seen)) || callees(nm, r).into_iter().any(|k| if seen.contains(&k) { false } else { seen.push(k); allocates(api, &api.fns[k as usize].body, seen) }),
            Expr::Field(b, ..) | Expr::Un(_, b) => ex(api, b, seen),
            Expr::Index(a, b) | Expr::Bin(_, a, b) => ex(api, a, seen) || ex(api, b, seen),
            _ => false,
        }
    }
    stmts.iter().any(|s| match s {
        Stmt::Alloc { .. } => true,
        Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(api, e, seen),
        Stmt::Remove(c, e) => ex(api, c, seen) || ex(api, e, seen),
        Stmt::Clear(c) => ex(api, c, seen),
        Stmt::Move(a, f, t2, n) => ex(api, a, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
        Stmt::Copy(d, src, f, t2, n) => ex(api, d, seen) || ex(api, src, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
        Stmt::Fill(a, f, t2, n) => ex(api, a, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
        Stmt::Sort(a, f, t2, n) => ex(api, a, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
        Stmt::Assign(a, b) => ex(api, a, seen) || ex(api, b, seen),
        Stmt::If(c, t, e) => ex(api, c, seen) || allocates(api, t, seen) || allocates(api, e, seen),
        Stmt::Each(it, b) => ex(api, &it.coll, seen) || it.hi.as_ref().map_or(false, |h| ex(api, h, seen)) || allocates(api, b, seen),
        Stmt::Forward { key, fields, .. } => ex(api, key, seen) || fields.iter().any(|f| ex(api, &f.1, seen)),
        _ => false,
    })
}

/// The kind of a value that a variable or result can hold; None for places (collections, structs, arrays).
fn kind(v: &CV) -> Option<K> {
    Some(match v {
        CV::Int(_) => K::Int, CV::Flt(_) => K::Flt, CV::Bool(_) => K::Bool, CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) => K::Txt,
        CV::Any(_) => K::Any, CV::Item(c, _) => K::Item(*c), CV::Ref(c, ..) => K::Ref(*c), CV::Null => K::Null,
        CV::Coll(c, _) => K::Coll(*c), CV::Plain(s, o) => K::Plain(*s, *o),
        CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..) => return None,
    })
}
/// Can a value of kind `v` be stored where kind `k` is held? (Collections and structs are places, not values: they fit
/// only themselves.)
fn fits(v: K, k: K) -> bool { if matches!(v, K::Coll(_) | K::Plain(..)) { return v == k; } k == K::Any || unify(v, k) == k }

impl<'p> G<'p> {
    #[allow(clippy::too_many_arguments)]
    fn new(prog: &'p Program, api: &'p Api, ep: &'p Endpoint, slot_kinds: &'p [((Option<u32>, u32), K)], ret_kinds: &'p [(u32, K)], recursive: &'p [bool], specs: &'p std::cell::RefCell<Vec<Spec>>) -> G<'p> {
        G { ret_ctx: None, pending_ctx: None, var_iv: std::collections::HashMap::new(), loop_frames: Vec::new(), var_depth: std::collections::HashMap::new(), loop_stmts: None, loop_entry: None, loop_heads: None, rq_touched: Vec::new(), uses_spec: false, wrote_state: std::cell::Cell::new(false), narrowed: Vec::new(), txt_bounds: std::collections::HashMap::new(), region_lets: Vec::new(), own_region: false, region_live: 0, region_peak: 0, region_unbounded: Vec::new(), txt_caps: std::collections::HashMap::new(), prog, api, ep, body: String::new(), n: 0, depth: 0, inlining: Vec::new(), ret: Vec::new(), frame: None, slot_kinds, ret_kinds, widen: None, u32_vars: std::collections::HashSet::new(), rest: &[], var_names: std::collections::HashSet::new(), fast_sites: std::collections::HashSet::new(), skip_save: false, presave: None, checked: Vec::new(), at: (0, 0), test: false, str_lens: std::collections::HashMap::new(), loop_n: 0, loops: Vec::new(), loop_hint: None, versioned: 0, txt_escapes: Vec::new(), recursive, specs, dep: false, inl: 0, ranges: std::collections::HashMap::new(), or_dead: None, loop_body: None, merged: Vec::new(), scr_top: 0, spec_key: None, scr_n: 0, spec_rec: false, immut: std::collections::HashSet::new() }
    }

    /// An endpoint's typed inputs, read and checked before its body, into slots 0.. in order. For a request with a body,
    /// its fields must be among the body inputs: an unknown field is 400.
    fn inputs(&mut self, ep: &'p Endpoint, env: &mut Vec<Option<CV>>) -> R<()> {
        if matches!(ep.method.as_str(), "POST" | "PUT" | "PATCH") || ep.inputs.iter().any(|i| i.src == Src::Body) {
            let names: String = ep.inputs.iter().filter(|i| i.src == Src::Body).map(|i| format!("{}\0", i.name)).collect();
            self.call(&format!("c->h->fields(c, {}, {})", c_lit(names.as_bytes()), names.len()));
        }
        for (k, inp) in ep.inputs.iter().enumerate() {
            let src = match &inp.src { Src::Body => 0, Src::Path => 1, Src::Query => 2, Src::Header(_) => 3, Src::Fwd => 4, Src::Cookie(_) => 5 };
            let name = match &inp.src { Src::Header(h) | Src::Cookie(h) => h.clone(), _ => inp.name.clone() };
            let (kind, lo, hi, pool) = match &inp.ty {
                InTy::Int(a, b) => (1, *a, *b, 0),
                InTy::Flt => (2, 0, 0, 0),
                InTy::Bool => (3, 0, 0, 0),
                InTy::Str(a, b, ascii) => (if *ascii { 7 } else { 4 }, *a, *b, 0),
                InTy::Item(t, p) => { let c = self.item_pool(&inp.name, t, p.as_deref())?; (match inp.src { Src::Header(_) => 5, Src::Cookie(_) => 8, _ => 6 }, c as i64, 0, c) }
            };
            let v = self.tmp("in");
            self.line(&format!("Val {};", v));
            self.call(&format!("c->h->input(c, {}, {}, {}, {}, {}, {}, &{})", src, c_lit(name.as_bytes()), name.len(), kind, i64_lit(lo), i64_lit(hi), v));
            // absent: the default, null when optional, else 400
            match (&inp.default, inp.optional) {
                (Some(d), _) => { let dv = self.expr(d, env, None)?; let dv = self.val(dv)?; self.line(&format!("if ({}.tag == 0) {} = {};", v, v, dv)); }
                (None, true) => {}
                (None, false) if matches!(inp.src, Src::Cookie(_)) && matches!(inp.ty, InTy::Item(..)) => { let msg = format!("no valid `{}` cookie", name); self.line(&format!("if ({}.tag == 0) FAIL(401, {}, {});", v, c_lit(msg.as_bytes()), msg.len())); }
                (None, false) => { let msg = format!("`{}` is required", inp.name); self.line(&format!("if ({}.tag == 0) FAIL(400, {}, {});", v, c_lit(msg.as_bytes()), msg.len())); }
            }
            let cv = match &inp.ty {
                InTy::Int(a, b) => {
                    let x = self.tmp("i");
                    self.line(&format!("i64 {} = {}.i;", x, v));
                    // a value the runtime range-checked, or a default that is an integer literal inside the range (an optional input
                    // with no default can be absent, and then it is 0)
                    let inside = match &inp.default { None => !inp.optional, Some(Expr::Int(d)) => d >= a && d <= b, _ => false };
                    if inside { self.ranges.insert(x.clone(), (*a, *b)); }
                    CV::Int(x)
                }
                InTy::Flt => { let x = self.tmp("f"); self.line(&format!("double {} = {}.f;", x, v)); CV::Flt(x) }
                InTy::Bool => { let x = self.tmp("b"); self.line(&format!("int {} = {}.i != 0;", x, v)); CV::Bool(x) }
                InTy::Str(_, hi, _) => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, {}.tag == 4 }};", t, v, v, v)); self.txt_bounds.insert(t.clone(), *hi as usize); CV::Txt(t) }
                InTy::Item(..) if inp.optional => { let (s, x) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}.tag != 0, {} = {}.b;", s, v, x, v)); CV::Ref(pool, s, x) }
                InTy::Item(..) => { let x = self.tmp("i"); self.line(&format!("u32 {} = {}.b;", x, v)); CV::Item(pool, x) }
            };
            env[k] = Some(cv);
        }
        Ok(())
    }
    /// The pool an input of item type `t` names: the one top-level pool of `t` items, or `in` the one given.
    fn item_pool(&self, input: &str, t: &str, pool: Option<&str>) -> R<usize> {
        let Some(sid) = self.prog.structs.iter().position(|s| s.name == t) else { return no(format!("input `{}`: `{}` is not a type", input, t)) };
        let mut found: Vec<usize> = self.prog.colls.iter().enumerate().filter(|(_, c)| c.item == Some(sid) && c.parent.is_none() && matches!(c.kind, Kind::Pool(_)) && pool.map_or(true, |p| c.name == p)).map(|(i, _)| i).collect();
        found.extend(self.prog.tables.iter().enumerate().filter(|(_, x)| x.item == sid && x.parent.is_none() && pool.map_or(true, |p| x.name == p)).map(|(i, _)| TBASE + i));
        match found.as_slice() {
            [c] => Ok(*c),
            [] => no(format!("input `{}`: no pool that exists once holds `{}`{}", input, t, pool.map_or(String::new(), |p| format!(" as {}", p)))),
            _ => no(format!("input `{}`: `{}` is kept in more than one pool; say which: `{}: {} in {}`", input, t, input, t, coll_name(self.prog, found[0]))),
        }
    }

    /// The module instance whose function is being generated (None: the program's own code).
    fn module_code(&self) -> Option<String> { let k = *self.inlining.last()?; let n = &self.api.fns[k as usize].name; n.rfind('.').map(|i| n[..i].to_string()) }
    /// The instance a collection belongs to: (instance, its items or nested in them (the program's data), its items exactly).
    fn coll_owner(&self, c: usize) -> Option<(&'p crate::layout::ModInst, bool, bool)> {
        if tab(c).is_some() { return None; }
        let name = &self.prog.colls[c].name;
        let m = self.prog.instances.iter().find(|m| name.starts_with(&format!("{}.", m.name)))?;
        let items = format!("{}.items", m.name);
        Some((m, *name == items || name.starts_with(&format!("{}.", items)), *name == items))
    }
    fn module_err(&self, m: &crate::layout::ModInst, what: &str) -> String { format!("{} belongs to {} (module {}); change it through {}'s functions", what, m.name, m.module, m.name) }
    /// Writing column `path` of collection `c` from here: refused outside a module for its own state, and for its key.
    /// `loop_stmt` for a loop whose body is `body`, with what is known of the variables set up as the handler of `Stmt::Each` says: `base` (the ones the loop assigns dropped),
    /// plus the counters `range_loop` can bound from the `entry` state.
    fn loop_vars(&mut self, it: &'p Iter, body: &'p [Stmt], env: &mut Vec<Option<CV>>, collect: bool, entry: &std::collections::HashMap<String, (i64, i64)>, base: &std::collections::HashMap<String, (i64, i64)>) -> R<()> {
        self.var_iv = base.clone();
        let loop_id = it as *const Iter as usize;
        for slot in assigned_slots(body) {
            if let Some(Some(CV::Int(n))) = env.get(slot as usize) {
                if let (true, Some(iv)) = (entry.contains_key(n), LOOP_INV.with(|l| l.borrow().assumed.get(&(loop_id, slot)).copied())) { self.var_iv.insert(n.clone(), iv); }
            }
        }
        self.loop_stmts = Some(body);
        self.loop_entry = Some(entry.clone());
        self.loop_heads = None;
        self.loop_frames.push(loop_id);
        let r = self.loop_stmt(it, body, env, collect);
        self.loop_frames.pop();
        self.loop_stmts = None;
        self.loop_entry = None;
        r
    }
    /// The variables a counted loop only steps up or down by constants (`n = n + 1`, `n = n - 2`; nowhere else assigned in the body, nor inside a loop of the body): each stays
    /// within its entry interval widened by (the steps of one round) x (the most rounds), in the body and after the loop. `trips`: the most rounds, when known.
    fn counter_heads(&self, stmts: &[Stmt], entry: &std::collections::HashMap<String, (i64, i64)>, trips: Option<i128>, env: &[Option<CV>]) -> std::collections::HashMap<String, (i64, i64)> {
        let mut heads = std::collections::HashMap::new();
        let Some(trips) = trips else { return heads };
        for slot in assigned_slots(stmts) {
            let Some(Some(CV::Int(name))) = env.get(slot as usize) else { continue };
            let Some((lo, hi)) = entry.get(name).copied() else { continue };
            let Some((up, down)) = step_sums(stmts, slot) else { continue };
            let (l, h) = (lo as i128 - down * trips, hi as i128 + up * trips);
            if l >= i64::MIN as i128 && h <= i64::MAX as i128 { heads.insert(name.clone(), (l as i64, h as i64)); }
        }
        heads
    }
    /// What a condition that holds tells of a variable: `x < e` and `x <= e` (and `e > x`, `e >= x`), joined by `&&`, give `x` the upper bound of `e`'s interval (below it).
    fn refine(&mut self, c: &'p Expr, env: &mut Vec<Option<CV>>) -> R<()> {
        let Expr::Bin(op, a, b) = c else { return Ok(()) };
        if op == "&&" { self.refine(a, env)?; return self.refine(b, env); }
        let (var, bound, strict) = match op.as_str() { "<" => (a, b, true), "<=" => (a, b, false), ">" => (b, a, true), ">=" => (b, a, false), _ => return Ok(()) };
        let Expr::Var(_, VarRes::Local(slot)) = &**var else { return Ok(()) };
        let Some(Some(CV::Int(name))) = env.get(*slot as usize).cloned() else { return Ok(()) };
        if !self.var_names.contains(&name) || self.ranges.contains_key(&name) || !pure(bound) { return Ok(()); }
        let bv = self.expr(bound, env, None)?;
        let CV::Int(bc) = bv else { return Ok(()) };
        let Some((_, bh)) = self.interval(&bc) else { return Ok(()) };
        let h = if strict { bh.saturating_sub(1) } else { bh };
        let (lo, hi) = self.var_iv.get(&name).copied().unwrap_or((i64::MIN, i64::MAX));
        self.var_iv.insert(name, (lo, hi.min(h)));
        Ok(())
    }
    fn may_write_col(&self, c: usize, path: &str) -> R<()> {
        self.wrote_state.set(true);
        let Some((m, data, items)) = self.coll_owner(c) else { return Ok(()) };
        if self.module_code().as_deref() == Some(m.name.as_str()) { return Ok(()); }
        if !data { return Err(self.module_err(m, &format!("`{}`", self.prog.colls[c].name))); }
        if items && m.key.as_deref() == Some(path) { return Err(format!("`{}` is the key of {}; change it with {}.set_key(item, value)", path, m.name, m.name)); }
        Ok(())
    }
    // ---- struct values: locals are places in the request region's scratch area ("Struct values" in DESIGN.md) ----
    fn in_scratch(&self, off: usize) -> bool { let (a, n) = self.prog.scratch; off >= a && off < a + n }
    /// A place of `size` bytes in the scratch area for a struct value: from the area's start in the code of an endpoint or an inlined function (given back at the end of the
    /// block, so locals of blocks that do not overlap share it); in a C function generated apart from the area's top end, one place for each local, kept for the whole program
    /// (such a function is never running twice at once, and its caller's places are lower).
    fn scratch_alloc(&mut self, size: usize) -> R<usize> {
        if self.spec_rec { return no("a struct value inside a recursive function is not supported yet (each call would need its own place: a frame); keep the struct in the state and pass it, or make the function not recursive"); }
        let (so, cap) = self.prog.scratch;
        let size = (size.max(1) + 7) / 8 * 8;
        let full = format!("the struct values live at once here need more than the {} bytes of scratch a request has: use fewer or smaller struct locals", cap);
        if let Some(key) = self.spec_key.clone() {
            let n = self.scr_n;
            self.scr_n += 1;
            return SCRATCH.with(|s| {
                let mut s = s.borrow_mut();
                if let Some((o, sz)) = s.unique.get(&(key.clone(), n)).copied() { if sz == size { return Ok(so + o); } }
                if s.top + size + s.bottom > cap { return Err(full.clone()); }
                s.top += size;
                let o = cap - s.top;
                s.unique.insert((key, n), (o, size));
                Ok(so + o)
            });
        }
        let o = self.scr_top;
        self.scr_top += size;
        let top = self.scr_top;
        SCRATCH.with(|s| { let mut s = s.borrow_mut(); s.bottom = s.bottom.max(top); if s.top + s.bottom > cap { Err(full.clone()) } else { Ok(()) } })?;
        Ok(so + o)
    }
    /// The bytes of the request struct the code reads or writes here are ones the endpoint must find zeroed first.
    fn touch_range(&mut self, off: usize, n: usize) {
        let Some((ro, rn)) = self.prog.request else { return };
        if off < ro || off >= ro + rn { return; }
        let r = (off, off + n.max(1));
        if !self.rq_touched.contains(&r) { self.rq_touched.push(r); }
    }
    /// A store into every number leaf of struct `sid` at `off` that no range is proven for (the whole-program ranges of what each field holds: see `generate_with`).
    fn seen_struct(&mut self, sid: usize, off: usize) {
        for k in 0..self.prog.structs[sid].fields.len() {
            let (t, at) = (self.prog.structs[sid].fields[k].1.clone(), off + self.prog.structs[sid].offs[k]);
            self.seen_ty(&t, at);
        }
    }
    fn seen_ty(&mut self, t: &Ty, at: usize) {
        match t {
            Ty::Scalar(_) => self.field_store_seen(at, None),
            Ty::Arr(e, n) => match &**e { Ty::Scalar(_) => self.field_store_seen(at, None), e => { let sz = self.prog.plain_size(e).0; for i in 0..*n { self.seen_ty(e, at + i * sz); } } },
            Ty::Struct(s) => self.seen_struct(*s, at),
            _ => {}
        }
    }
    /// Copy struct `sid` from the place at `src` to the one at `dst` (a whole copy, as a memmove: the places may be the same or overlap). A destination in the state is a state
    /// write: it saves the logged lines it covers for the undo log first, and counts for the endpoint's reads and the whole-program ranges; one in the request region (the
    /// scratch area, the request struct) is the request's own and is not.
    fn copy_struct(&mut self, sid: usize, dst: usize, src: usize) -> R<()> {
        let n = self.prog.structs[sid].size;
        if dst == src || n == 0 { return Ok(()); }
        self.touch_range(src, n);
        let (ro, rn) = self.prog.request.unwrap_or((0, 0));
        let own = (dst >= ro && dst < ro + rn) || self.in_scratch(dst);
        if own { self.touch_range(dst, n); } else {
            self.may_write_plain(dst)?;
            self.may_write_plain(dst + n - 1)?;
            self.seen_struct(sid, dst);
            for l in dst / 64..=(dst + n - 1) / 64 {
                if !self.unlogged_at((l * 64).max(dst)) { self.line(&format!("SAVE_({}ULL, 64);", l * 64)); }
            }
        }
        self.line(&format!("{}({}ULL, {}ULL, {});", if own { "SCRMOVE" } else { "MOVE_" }, dst, src, n));
        Ok(())
    }
    /// `Name { f: e, ..base }` (the parser's `__mito__struct("Name", mode, fields...)`): a new struct place, all zero: its fields are set by the assignments that follow.
    /// `mode`: "full" (every field given), "zero" (`Name {}`), "update" (`..base` and the fields listed, copied over a copy of `base`).
    fn struct_new(&mut self, args: &[Expr]) -> R<CV> {
        let strs: Vec<&str> = args.iter().filter_map(|a| if let Expr::Str(s) = a { Some(s.as_str()) } else { None }).collect();
        let (name, mode) = (strs[0], strs[1]);
        let Some(sid) = self.prog.structs.iter().position(|s| s.name == name) else { return no(format!("`{} {{ ... }}`: no struct `{}`{}", name, name, crate::lang::suggest(name, self.prog.structs.iter().map(|s| s.name.as_str())))) };
        let sd = &self.prog.structs[sid];
        for f in &strs[2..] {
            if !sd.fields.iter().any(|x| x.0 == *f) { return no(format!("`{} {{ ... }}`: no field `{}`{}", name, f, crate::lang::suggest(f, sd.fields.iter().map(|x| x.0.as_str())))); }
        }
        if mode == "full" {
            let missing: Vec<&str> = sd.fields.iter().filter(|x| !strs[2..].contains(&x.0.as_str())).map(|x| x.0.as_str()).collect();
            if !missing.is_empty() {
                let arr = sd.fields.iter().any(|x| missing.contains(&x.0.as_str()) && matches!(x.1, Ty::Arr(..)));
                return no(format!("`{} {{ ... }}` is missing {}: a struct literal gives every field{}", name, missing.iter().map(|m| format!("`{}`", m)).collect::<Vec<_>>().join(", "), if arr { " (an array field cannot be given in a literal: start from `Name {}` or `Name { ..base }` and set its elements)" } else { "; `Name {}` is all zero, `Name { ..base, f: e }` changes some" }));
            }
        }
        self.prog.value_check(sid)?;
        let off = self.scratch_alloc(self.prog.structs[sid].size)?;
        let n = self.prog.structs[sid].size;
        self.line(&format!("SCRZERO({}ULL, {});", off, n));
        Ok(CV::Plain(sid, off))
    }
    /// The slot of the local at the root of `e` (`v`, `v.pos.x`, `v.xs[i]`), and its name.
    fn root_local(e: &Expr) -> Option<(&str, u32)> {
        match e { Expr::Var(n, VarRes::Local(s)) => Some((n.as_str(), *s)), Expr::Field(b, _) | Expr::Index(b, _) => Self::root_local(b), _ => None }
    }
    /// Before an assignment to `lhs` (not a bare variable): a `let` struct local's fields are not assigned; a test changes only its own struct locals.
    fn check_assign_target(&self, lhs: &Expr, env: &[Option<CV>]) -> R<()> {
        if matches!(lhs, Expr::Var(..)) { return Ok(()); }
        let root = Self::root_local(lhs);
        if let Some((n, slot)) = root { if self.immut.contains(&(self.frame, slot)) { return no(format!("`{}` is a `let`: declare it with `var` to change its fields", n)); } }
        if self.test && self.frame.is_none() {
            let mine = root.map_or(false, |(_, s)| matches!(env.get(s as usize), Some(Some(CV::Plain(_, o))) if self.in_scratch(*o)));
            if !mine { return no("a test changes the state only through requests (`post(...)` and the others); it can read the state directly"); }
        }
        Ok(())
    }
    /// Writing the plain value at `off` from here: an instance's plain state is its own.
    fn may_write_plain(&self, off: usize) -> R<()> {
        if !self.in_scratch(off) && self.prog.request.map_or(true, |(a, n)| off < a || off >= a + n) { self.wrote_state.set(true); } // the request struct is the request's own
        for m in &self.prog.instances {
            let Some((start, size)) = self.prog.plain_path(&m.name) else { continue };
            if off >= start && off < start + size && self.module_code().as_deref() != Some(m.name.as_str()) { return Err(self.module_err(m, &format!("the state of {}", m.name))); }
        }
        Ok(())
    }
    /// Adding or freeing items of collection `c` from here: an instance's items and state change only through its functions.
    fn may_add_or_free(&self, c: usize, verb: &str) -> R<()> {
        self.wrote_state.set(true);
        if verb == "remove" { REMOVAL.with(|r| { r.borrow_mut().1.insert(c); }); }
        let Some((m, data, items)) = self.coll_owner(c) else { return Ok(()) };
        if self.module_code().as_deref() == Some(m.name.as_str()) || (data && !items) { return Ok(()); }
        Err(format!("{} items of {} with its functions, not `{}.{}()`", verb, m.name, self.prog.colls[c].name, verb))
    }
    /// A field store or free through a value whose kind is known only at run time: refused outside module code when it
    /// could reach a module's key or private state.
    fn may_write_dynamic(&self, field: Option<&str>) -> R<()> {
        self.wrote_state.set(true);
        if self.prog.instances.is_empty() || self.module_code().is_some() { return Ok(()); }
        match field {
            None => Ok(()),
            Some(f) => {
                for m in &self.prog.instances { if m.key.as_deref() == Some(f) { return Err(format!("`.{}` through a value whose kind is only known at run time could change the key of {}", f, m.name)); } }
                for (c, coll) in self.prog.colls.iter().enumerate() {
                    if let Some((m, false, _)) = self.coll_owner(c) { if coll.cols.iter().any(|x| x.path == f) { return Err(format!("`.{}` through a value whose kind is only known at run time could change the state of {}", f, m.name)); } }
                }
                Ok(())
            }
        }
    }

    /// A call is about to be made: refuse it, as the interpreter does, if it would nest more than MAX_DEPTH calls deep.
    fn depth_check(&mut self) {
        let n = self.inl + 1;
        if self.dep { self.open(&format!("if (dep + {} > {}) {{", n, MAX_DEPTH)); self.fail(500, DEPTH_MSG); self.close("}"); }
        else if n > MAX_DEPTH { self.fail(500, DEPTH_MSG); }
    }

    /// Call the C function for recursive or outlined function `k` with these argument values, generating it if needed.
    fn call_spec(&mut self, k: u32, vals: Vec<CV>) -> R<CV> {
        let rec = self.recursive[k as usize];
        let (mut pks, mut cargs) = (Vec::new(), Vec::new());
        for v in vals {
            match v {
                CV::Coll(c, inst) => { pks.push(PK::Coll(c)); cargs.push(format!("(u32)({})", inst)); }
                v @ (CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) | CV::Txt(_)) => {
                    // an outlined function's text parameter keeps its size bound (rounded up to a power of two: fewer copies), which
                    // the sizes of the text its body builds are worked out from
                    let b = if rec { None } else { self.txt_bound(&v).map(|b| b.max(1).next_power_of_two()) };
                    let t = self.txt_of(v)?;
                    pks.push(match b { Some(b) => PK::TxtB(b), None => PK::V(K::Txt) });
                    cargs.push(t);
                }
                CV::Int(e) => { pks.push(PK::V(K::Int)); cargs.push(e); }
                CV::Flt(e) => { pks.push(PK::V(K::Flt)); cargs.push(e); }
                CV::Bool(e) => { pks.push(PK::V(K::Bool)); cargs.push(e); }
                CV::Any(e) => { pks.push(PK::V(K::Any)); cargs.push(e); }
                CV::Item(c, e) => { pks.push(PK::V(K::Item(c))); cargs.push(e); }
                CV::Ref(c, s, i) => { pks.push(PK::V(K::Ref(c))); cargs.push(s); cargs.push(i); }
                CV::Null => pks.push(PK::V(K::Null)),
                CV::Plain(sd, o) => pks.push(PK::Plain(sd, o)),
                _ => return no("passing a struct field or array of an item to a recursive function is not compiled yet"),
            }
        }
        let key = format!("{}{:?}", k, pks);
        let found = self.specs.borrow().iter().position(|s| s.key == key);
        let si = match found { Some(i) => i, None => self.gen_spec(k, key, &pks)? };
        let (name, ret, peak, kept, ret_bound) = { let sp = &self.specs.borrow()[si]; (sp.name.clone(), sp.ret, sp.peak, sp.kept, sp.ret_bound) };
        if !rec {
            // its region text on top of the caller's; what it returns stays until the caller's scope ends, as an inlined call's does
            self.region_peak = self.region_peak.max(self.region_live + peak);
            if self.region_peak > TEXT_AREA { return no(format!("`{}`: the text in this endpoint's scopes can reach {} bytes, more than the request's text area holds ({} bytes): build less at once, or keep it in a field", self.api.fns[k as usize].name, self.region_peak, TEXT_AREA)); }
            if kept > 0 { self.region_lets.push(Some(kept)); self.region_live += kept; }
        }
        self.uses_spec = true;
        let level = if self.dep { format!("dep + {}", self.inl + 1) } else { format!("{}", self.inl + 1) };
        let mut a = vec!["c".to_string(), level];
        a.extend(cargs);
        let r = self.tmp("rv");
        let (decl, out) = match ret {
            K::Null => (None, CV::Null),
            K::Int => (Some(format!("i64 {} = 0;", r)), CV::Int(r.clone())),
            K::Flt => (Some(format!("double {} = 0;", r)), CV::Flt(r.clone())),
            K::Bool => (Some(format!("int {} = 0;", r)), CV::Bool(r.clone())),
            K::Txt => (Some(format!("Txt {} = {{ 0, 0, 0 }};", r)), CV::Txt(r.clone())),
            K::Any => (Some(format!("Val {} = {{ 0, 0, 0, 0, 0 }};", r)), CV::Any(r.clone())),
            K::Item(c) => (Some(format!("u32 {} = 0;", r)), CV::Item(c, r.clone())),
            K::Ref(c) => (Some(format!("u32 {}_s = 0, {} = 0;", r, r)), CV::Ref(c, format!("{}_s", r), r.clone())),
            K::Coll(c) => (Some(format!("u32 {} = 0;", r)), CV::Coll(c, r.clone())),
            K::Plain(sd, o) => (None, CV::Plain(sd, o)),
        };
        if let Some(d) = decl { self.line(&d); }
        if let K::Ref(_) = ret { a.push(format!("&{}_s", r)); }
        if !matches!(ret, K::Null | K::Plain(..)) { a.push(format!("&{}", r)); }
        self.call(&format!("{}({})", name, a.join(", ")));
        if let (CV::Txt(t), Some(b)) = (&out, ret_bound) { self.txt_bounds.insert(t.clone(), b); }
        Ok(out)
    }

    /// Generate the C function for recursive function `k` with argument kinds `pks`; its result kind is its signature's.
    fn gen_spec(&mut self, k: u32, key: String, pks: &[PK]) -> R<usize> {
        let si = self.specs.borrow().len();
        let name = format!("fn_{}_{}", k, si);
        let mut proto = String::from("(Ctx *c, u32 dep");
        for (i, pk) in pks.iter().enumerate() {
            match pk {
                PK::Coll(_) | PK::V(K::Item(_)) => proto += &format!(", u32 p{}", i),
                PK::V(K::Ref(_)) => proto += &format!(", u32 p{}s, u32 p{}k", i, i),
                PK::V(K::Int) => proto += &format!(", i64 p{}", i),
                PK::V(K::Flt) => proto += &format!(", double p{}", i),
                PK::V(K::Bool) => proto += &format!(", int p{}", i),
                PK::V(K::Txt) | PK::TxtB(_) => proto += &format!(", Txt p{}", i),
                PK::V(K::Any) => proto += &format!(", Val p{}", i),
                PK::V(K::Null) | PK::Plain(..) => {}
                PK::V(K::Coll(_)) => proto += &format!(", u32 p{}", i),
                PK::V(K::Plain(..)) => {}
            }
        }
        let rec = self.recursive[k as usize];
        self.specs.borrow_mut().push(Spec { key, name, ret: K::Null, proto: String::new(), body: None, rec, peak: 0, kept: 0, ret_bound: None });
        // the declared result (a function without one gives no value)
        let cand = match &self.api.fns[k as usize].ret { Some(t) => self.ret_kind(t, &self.api.fns[k as usize].name)?, None => K::Null };
        {
            self.specs.borrow_mut()[si].ret = cand;
            match self.spec_body(k, pks, cand) {
                Ok((body, peak, kept, ret_bound)) => {
                    let rp = match cand { K::Null | K::Plain(..) => "", K::Int => ", i64 *ret_v", K::Flt => ", double *ret_v", K::Bool => ", int *ret_v", K::Txt => ", Txt *ret_v", K::Any => ", Val *ret_v", K::Item(_) | K::Coll(_) => ", u32 *ret_v", K::Ref(_) => ", u32 *ret_s, u32 *ret_v" };
                    let mut sp = self.specs.borrow_mut();
                    sp[si].proto = format!("{}{})", proto, rp);
                    sp[si].body = Some(body);
                    (sp[si].peak, sp[si].kept, sp[si].ret_bound) = (peak, kept, ret_bound);
                    return Ok(si);
                }
                Err(SpecErr::Conflict(_)) => { self.specs.borrow_mut().truncate(si); no(format!("`{}` returns a value of another kind than it declares", self.api.fns[k as usize].name)) }
                Err(SpecErr::Other(e)) => { self.specs.borrow_mut().truncate(si); Err(e) }
            }
        }
    }

    /// The body of a C function for `k`, assuming its result kind is `cand`; widens its own locals as endpoints do.
    /// With the body: (for an outlined function) its region text's peak, what stays after it returns, its text result's bound.
    fn spec_body(&mut self, k: u32, pks: &[PK], cand: K) -> Result<(String, usize, usize, Option<usize>), SpecErr> {
        let rec = self.recursive[k as usize];
        let f = &self.api.fns[k as usize];
        let (mut slot_kinds, mut ret_kinds): (Vec<((Option<u32>, u32), K)>, Vec<(u32, K)>) = (Vec::new(), Vec::new());
        for _ in 0..32 {
            let mut g = G::new(self.prog, self.api, self.ep, &slot_kinds, &ret_kinds, self.recursive, self.specs);
            g.dep = true;
            g.spec_key = Some(format!("{}{:?}", k, pks));
            g.spec_rec = rec;
            g.frame = Some(k);
            g.inlining.push(k);
            let sel = if has_collecting_loop(self.api, &f.body) { Some("hm".to_string()) } else { None };
            if sel.is_some() { g.line("u32 hm;"); g.call("c->h->sel_mark(c, &hm)"); }
            g.ret.push(Ret { dest: None, var: String::new(), label: String::new(), kind: Some(cand), func: true, sel: sel.clone(), bound: None, unbounded: rec, mark: "rmark".into(), ivs: Vec::new() });
            let mut fenv: Vec<Option<CV>> = vec![None; f.slots as usize];
            for (i, pk) in pks.iter().enumerate() {
                fenv[i] = Some(match pk {
                    PK::Coll(c) => CV::Coll(*c, format!("p{}", i)),
                    PK::V(K::Item(c)) => CV::Item(*c, format!("p{}", i)),
                    PK::V(K::Ref(c)) => CV::Ref(*c, format!("p{}s", i), format!("p{}k", i)),
                    PK::V(K::Int) => CV::Int(format!("p{}", i)),
                    PK::V(K::Flt) => CV::Flt(format!("p{}", i)),
                    PK::V(K::Bool) => CV::Bool(format!("p{}", i)),
                    PK::V(K::Txt) => CV::Txt(format!("p{}", i)),
                    PK::TxtB(b) => { g.txt_bounds.insert(format!("p{}", i), *b); CV::Txt(format!("p{}", i)) }
                    PK::V(K::Any) => CV::Any(format!("p{}", i)),
                    PK::V(K::Null) => CV::Null,
                    PK::Plain(sd, o) | PK::V(K::Plain(sd, o)) => CV::Plain(*sd, *o),
                    PK::V(K::Coll(c)) => CV::Coll(*c, format!("p{}", i)),
                });
            }
            let r = g.block(&f.body, &mut fenv);
            if sel.is_some() && r.is_ok() { g.call("c->h->sel_end(c, hm)"); } // falling off the end
            let ends_by_itself = matches!(f.body.last(), Some(Stmt::Return(_)) | Some(Stmt::Fail(..)) | Some(Stmt::Respond(..)) | Some(Stmt::Forward { .. }));
            match (r, g.widen) {
                (Ok(()), _) if !ends_by_itself && !fits(K::Null, cand) => return Err(SpecErr::Other(format!("`{}` can end without returning its `{}`: return a value on every path", f.name, f.ret.as_ref().map_or(String::new(), |t| t.to_string())))),
                // the function's parameters (p0, p1, ...: in its prototype, not its body) are its locals too
                (Ok(()), _) => {
                    if !rec && !g.region_unbounded.is_empty() { return Err(SpecErr::Other(format!("region text kept across a loop's rounds: {}", g.region_unbounded[0]))); }
                    let body = resolve_return_cuts(&g.body, "rmark");
                    let ps: Vec<String> = (0..f.params.len()).map(|i| format!("p{}", i)).collect();
                    let ret_bound = match g.ret.first() { Some(r) if !r.unbounded => r.bound, _ => None };
                    let texty = matches!(cand, K::Txt | K::Any);
                    let kept = if texty { g.region_live.max(ret_bound.unwrap_or(0)) } else { 0 };
                    return check_profile_in(&body, &ps).map(|_| (body, g.region_peak, kept, ret_bound)).map_err(|e| SpecErr::Other(format!("C safety profile: {}", e)))
                }
                (Err(_), Some(Widen::Ret(f2, u))) if f2 == k => return Err(SpecErr::Conflict(u)),
                (Err(_), Some(Widen::Slot(fr, sl, kk))) => match slot_kinds.iter_mut().find(|x| x.0 == (fr, sl)) { Some(x) => x.1 = unify(x.1, kk), None => slot_kinds.push(((fr, sl), kk)) },
                (Err(_), Some(Widen::Ret(f2, kk))) => match ret_kinds.iter_mut().find(|x| x.0 == f2) { Some(x) => x.1 = unify(x.1, kk), None => ret_kinds.push((f2, kk)) },
                (Err(e), None) => return Err(SpecErr::Other(place_err(g.at, e))),
            }
        }
        Err(SpecErr::Other("kinds did not settle".into()))
    }

    /// Is struct `sid` a field of the state, directly or inside plain structs (so that a function can be given it as a view)?
    fn plain_used(&self, sid: usize) -> bool {
        fn walk(prog: &Program, ty: &Ty, sid: usize, seen: &mut Vec<usize>) -> bool {
            match ty {
                Ty::Struct(s) if *s == sid => true,
                Ty::Struct(s) => { if seen.contains(s) { return false; } seen.push(*s); prog.structs[*s].fields.iter().any(|f| walk(prog, &f.1, sid, seen)) }
                _ => false,
            }
        }
        let mut seen = Vec::new();
        self.prog.globals.iter().any(|g| walk(self.prog, &g.ty, sid, &mut seen))
    }
    /// The kind a function's declared result stands for: a struct that is part of the state cannot be returned (it has no place of its own to point at).
    fn ret_kind(&self, t: &crate::lang::PTy, f: &str) -> R<K> {
        match self.decl_kind(t, f)? {
            K::Plain(sd, _) if false => no(format!("`{}` returns `{}`, a struct that is part of the state: a function can take it as a parameter but not return it yet (struct returns are step 3 of \"Struct values\" in DESIGN.md: return its fields, or an item)", f, self.prog.structs[sd].name)),
            k => Ok(k),
        }
    }
    /// The kind a signature's type stands for (an item type `T?` is a reference; a scalar `T?` is its scalar, and also takes null).
    /// `f`: the function whose signature it is; in a module instance's function (`inst.f`), an item type names the instance's own items.
    fn decl_kind(&self, t: &crate::lang::PTy, f: &str) -> R<K> {
        let inst = f.rsplit_once('.').map(|x| format!("{}.", x.0));
        let scalar = |n: &str| -> Option<K> { Some(match n {
            "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" => K::Int, "f32" | "f64" => K::Flt, "bool" => K::Bool,
            "string" | "utf8" | "ascii" | "text" => K::Txt, _ => return None }) };
        if let Some(fld) = &t.field {
            // `T.key`: the type of a struct's field
            let Some(sd) = self.prog.structs.iter().find(|x| x.name == t.name) else { return no(format!("`{}` is not a struct", t.name)) };
            let Some((_, ty)) = sd.fields.iter().find(|x| x.0 == *fld) else { return no(format!("`{}` has no field `{}`", t.name, fld)) };
            let tn = self.prog.show(ty);
            let base = tn.split(|c: char| c == '<' || c == ' ' || c == '[').next().unwrap_or("").to_string();
            return scalar(&base).ok_or_else(|| format!("`{}.{}` is {}: a signature takes a field's type only when it is a number, bool or string", t.name, fld, tn));
        }
        if let Some(k) = scalar(&t.name) { return Ok(k); }
        if self.prog.enums.iter().any(|e| e.name == t.name) { return Ok(K::Int); }
        if t.name == "pool" || t.name == "list" {
            // a collection: `pool<T>` / `list<T>` (`in path` when several hold T)
            let Some(item) = &t.arg else { return no(format!("`{}` needs its item type: `{}<T>`", t.name, t.name)) };
            let Some(sid) = self.prog.structs.iter().position(|x| x.name == *item) else { return no(format!("`{}` is not a struct", item)) };
            let found: Vec<usize> = (0..self.prog.colls.len()).filter(|&c| { let x = &self.prog.colls[c]; x.item == Some(sid) && x.is_list() == (t.name == "list") && t.coll.as_ref().map_or(true, |p| x.name == *p) }).collect();
            return match found.as_slice() {
                [c] => Ok(K::Coll(*c)),
                [] => no(format!("no {} of `{}` items{}", t.name, item, t.coll.as_ref().map_or(String::new(), |p| format!(" at `{}`", p)))),
                _ => no(format!("several collections are `{}`: write `{} in path` ({})", t, t, found.iter().map(|&c| self.prog.colls[c].name.clone()).collect::<Vec<_>>().join(", "))),
            };
        }
        let Some(sid) = self.prog.structs.iter().position(|x| x.name == t.name) else {
            return no(format!("`{}` is not a type: a parameter is a number (i64, f64), bool, string, or an item (a struct held by a collection)", t.name));
        };
        let mut holders: Vec<usize> = (0..self.prog.colls.len()).filter(|&c| self.prog.colls[c].item == Some(sid) && t.coll.as_ref().map_or(true, |p| self.prog.colls[c].name == *p)).collect();
        holders.extend((0..self.prog.tables.len()).filter(|&k| self.prog.tables[k].item == sid && t.coll.as_ref().map_or(true, |p| self.prog.tables[k].name == *p)).map(|k| TBASE + k));
        if holders.len() > 1 { if let Some(i) = &inst { let own: Vec<usize> = holders.iter().copied().filter(|&c| coll_name(self.prog, c).starts_with(i.as_str())).collect(); if own.len() == 1 { holders = own; } } }
        let c = match holders.as_slice() {
            [c] => *c,
            // a struct that is a field of the state (not an item) is passed as a view of where it lives: the argument's own place decides, so this only stands for "that struct"
            [] if t.coll.is_none() && self.plain_used(sid) => {
                if t.opt { return no(format!("`{}?`: a plain struct is part of the state, it cannot be null", t.name)); }
                return Ok(K::Plain(sid, usize::MAX));
            }
            [] => return no(match &t.coll { Some(p) => format!("`{}` is not held by `{}`", t.name, p), None => format!("no collection holds `{}` items", t.name) }),
            _ => return no(format!("several collections hold `{}` items: write `{} in path` with the one this takes ({})", t.name, t.name, holders.iter().map(|&c| coll_name(self.prog, c)).collect::<Vec<_>>().join(", "))),
        };
        Ok(if t.opt { K::Ref(c) } else { K::Item(c) })
    }
    /// A value given where a signature declares type `t` (`what`: "parameter `x` of `f`" or "result of `f`"): checked, an integer
    /// widened to a number for a `f64`, an item kept as an item for `T?`.
    fn to_declared(&mut self, v: CV, t: &crate::lang::PTy, f: &str, what: &str) -> R<CV> {
        // a plain struct (a field of the state) given for its own type: a view, whether or not the same struct is also an item type somewhere
        if let CV::Plain(sd, _) = &v {
            if t.name == self.prog.structs[*sd].name && t.arg.is_none() && t.field.is_none() && t.coll.is_none() && !t.opt { return Ok(v); }
        }
        let want = self.decl_kind(t, f)?;
        let got = kind(&v);
        let ok = match (got, want) {
            (Some(K::Int), K::Flt) => return Ok(match v { CV::Int(e) => CV::Flt(format!("((double)({}))", e)), v => v }),
            (Some(K::Null), _) => t.opt,
            (Some(K::Item(c)), K::Ref(d)) => c == d,
            (Some(K::Ref(c)), K::Ref(d)) => c == d,
            (Some(K::Ref(_)), K::Item(_)) => return no(format!("{} is `{}`, given a value that may be null: check it first with `if exists(x)`, or declare `{}?`", what, t, t)),
            (Some(a), b) => a == b,
            (None, _) => false,
        };
        if !ok { return no(format!("{} is `{}`, given {}", what, t, got.map_or("a struct field or array".to_string(), |k| kname(self.prog, k)))); }
        Ok(v)
    }

    fn tmp(&mut self, p: &str) -> String {
        self.n += 1;
        let t = format!("{}{}", p, self.n);
        if ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"].contains(&t.as_str()) { format!("{}_{}", p, self.n) } else { t } // never a type's name
    }
    fn line(&mut self, s: &str) {
        if s.ends_with(": ;") || s.contains("goto ") { self.checked.clear(); } // control joins or leaves here: a test seen before may not have run
        else if !self.checked.is_empty() { self.checked.retain(|(x, _)| !writes_name(s, x)); } // an index given a new value must be tested again
        for _ in 0..self.depth + 1 { self.body.push_str("    "); } self.body.push_str(s); self.body.push('\n');
    }
    /// The interval a C integer is proven to lie in: a literal, or a name recorded in `ranges` (names that are never assigned again).
    fn interval(&self, s: &str) -> Option<(i64, i64)> {
        if let Some(d) = s.strip_suffix("LL") { if let Ok(v) = d.parse::<i64>() { return Some((v, v)); } }
        if let Some(r) = load_interval(s) { return Some(r); }
        let name = s.strip_prefix("(i64)").unwrap_or(s);
        let known = self.ranges.get(name).or_else(|| self.var_iv.get(name)).copied();
        // an index whose bounds test has passed in this straight run of code (`checked`: dropped at a branch and when the index is assigned) is inside that array, as the test said
        let tested = self.checked.iter().filter(|(x, n)| x == name && *n > 0).map(|(_, n)| (0i64, (*n).min(i64::MAX as u64) as i64 - 1)).fold(None, |acc: Option<(i64, i64)>, r| Some(acc.map_or(r, |a| (a.0.max(r.0), a.1.min(r.1)))));
        match (known, tested) { (Some(k), Some(t)) => Some((k.0.max(t.0), k.1.min(t.1))), (k, t) => k.or(t) }
    }
    /// A value stored into a field declared with a range (`n: i64 in 0..100`): an integer, tested against the range unless the range analysis
    /// proves it inside; a value outside fails the request (400), naming the field.
    fn in_field_range(&mut self, v: CV, (lo, hi): (i64, i64), field: &str) -> R<CV> {
        let x = self.int_or(v, 400, "a field with a range takes an integer");
        let x = self.hold_int(x);
        if let Some((a, b)) = self.interval(&x) { if a >= lo && b <= hi { return Ok(CV::Int(x)); } }
        let msg = format!("`{}` must be in {}..{}", field, lo, hi);
        self.line(&format!("if (UNLIKELY({x} < {} || {x} > {})) FAIL(400, {}, {});", c_i64(lo), c_i64(hi), c_lit(msg.as_bytes()), msg.len()));
        Ok(CV::Int(x))
    }
    /// Is the byte at plain-state offset `off` kept out of the undo log (an unlogged field)?
    fn unlogged_at(&self, off: usize) -> bool { self.in_scratch(off) || self.prog.unlogged.iter().any(|(a, b)| off >= *a && off < *b) }
    /// A store of `range` (None: not proven) into the plain integer field at `at`, for the whole-program field ranges.
    fn field_store_seen(&mut self, at: usize, range: Option<(i64, i64)>) {
        if self.in_scratch(at) || self.prog.request.map_or(false, |(a, n)| at >= a && at < a + n) { return; } // the request struct starts zeroed every request; left alone
        FIELD_RANGES.with(|fr| {
            let mut fr = fr.borrow_mut();
            let e = fr.1.entry(at).or_insert(range);
            *e = match (*e, range) { (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))), _ => None };
        });
    }
    /// A value read from a field declared with a range: the range analysis may assume it.
    fn with_field_range(&mut self, v: CV, range: Option<(i64, i64)>) -> CV {
        match (v, range) {
            (CV::Int(e), Some(r)) => {
                // a range recorded from the stores seen may be looser than what the load itself guarantees (a `u8` element is 0..255 whatever
                // was stored: a store of -255..255 is checked, and a failed store changes nothing): keep the narrower of the two ends
                let r = match self.interval(&e) { Some((a, b)) if a.max(r.0) <= b.min(r.1) => (a.max(r.0), b.min(r.1)), _ => r };
                let n = self.hold_int(e); if !self.var_names.contains(&n) { self.ranges.insert(n.clone(), r); } CV::Int(n)
            }
            (v, _) => v,
        }
    }
    /// Check that array position `k` is in 0..n: nothing for a constant in range, else a comparison in line, with the
    /// helper (which builds the error) only when it fails.
    fn elem_check(&mut self, k: &str, n: u64) {
        if let Some(v) = k.strip_suffix("LL").and_then(|d| d.parse::<u64>().ok()) { if v < n { return; } }
        if let Some((lo, hi)) = self.interval(k) { if lo >= 0 && (hi as u64) < n { return; } }
        // the same index against the same size, tested earlier in this straight run of code (as in `a[i] = a[i] + 1`)
        if self.checked.iter().any(|(x, m)| x == k && *m == n) { return; }
        // MITO_EXPLAIN_INDEX=1: say which index tests are kept and what is known of each index (to see what a proof is missing)
        if std::env::var_os("MITO_EXPLAIN_INDEX").is_some() { eprintln!("index test kept: line {}, index {} (known: {:?}), array of {}", self.at.0, k, self.interval(k), n); }
        self.line(&format!("if (RANGE_({k} < 0 || {k} >= {n}LL)) {{"));
        self.depth += 1;
        self.call(&format!("c->h->elem(c, {}, {})", k, n));
        self.depth -= 1;
        self.line("}");
        self.checked.push((k.to_string(), n));
    }
    /// Generate a loop statement (the loop, its body, and the text-area mark and cut when the body can build a string).
    fn loop_stmt(&mut self, it: &'p Iter, body: &'p [Stmt], env: &mut Vec<Option<CV>>, collect: bool) -> R<()> {
        self.loop_body = Some(LoopBody::Stmts(body));
        self.each(it, env, collect, |g, env, _| {
            // strings built in an iteration are dropped at its end unless the body hands one to a variable declared before the loop;
            // the mark and the cut are only emitted for a body whose code can build a string
            let (start, first) = (g.body.len(), g.txt_escapes.len());
            g.block(body, env)?;
            let code = &g.body[start..];
            let builds = ["->lit(", "->col_txt(", "->plain_txt(", "->arr_txt(", "->builtin(", "->binop(", "->input(", "->field_val("].iter().any(|h| code.contains(h))
                || g.specs.borrow().iter().any(|sp| code.contains(&format!("{}(c", sp.name)));
            if builds && !g.txt_escapes[first..].iter().any(|slot| *slot < it.slot) {
                let mark = g.tmp("tm");
                let kept = std::mem::take(&mut g.body);
                g.line(&format!("u32 {};", mark));
                g.call(&format!("c->h->text_len(c, &{})", mark));
                let head = std::mem::replace(&mut g.body, kept);
                g.body.insert_str(start, &head);
                g.call(&format!("c->h->text_cut(c, {})", mark));
            }
            Ok(())
        })
    }

    /// Loop versioning. For a range loop (`foreach v in lo..hi`, internally lo up to hi excluded), and for a loop that steps a variable (`while j <= limit` with one
    /// `j = j + step` at the top level of the body), find the accesses to plain-state arrays whose index is the counter (or the
    /// stepped variable) plus or minus a value the loop does not change. Emit, before the loop, a test that every such index
    /// stays inside its array for the whole loop, and save the ranges the loop stores into to the undo log in one pass when it
    /// does. Returns the test's C name and the accesses the fast copy may do without their own range test and undo-log test;
    /// None when the loop has no such access. The checked copy runs when the test fails, so behavior is unchanged.
    fn version_guard(&mut self, it: &'p Iter, body: &'p [Stmt], env: &mut Vec<Option<CV>>) -> R<Option<(String, std::collections::HashSet<*const Expr>, Vec<(String, (i64, i64))>, Option<(i64, i64)>)>> {
        let (Some(hi), None) = (&it.hi, &it.via) else { return Ok(None) };
        if self.versioned >= 3 { return Ok(None); }
        let assigned = assigned_slots(body);
        // the counter's values: lo..hi (evaluated again by the loop itself, so they must be free of effects)
        let mut plans: Vec<(u32, Vec<Site<'p>>, Bound<'p>)> = Vec::new();
        if pure(&it.coll) && pure(hi) {
            let mut sites = Vec::new();
            for st in body { stmt_sites(st, it.slot, &assigned, &mut sites); }
            if !sites.is_empty() { plans.push((it.slot, sites, Bound::Range(&it.coll, hi))); }
        }
        // a stepped variable: `while ... && j <= limit && ...`, one top-level `j = j + step`, the accesses in the statements before it
        if let Some(w) = &it.whil {
            for (slot, limit, inclusive) in upper_bounds(w) {
                if assigned.contains(&slot) && invariant(limit, &assigned) {
                    let steps: Vec<usize> = body.iter().enumerate().filter(|(_, st)| assigns_slot(st, slot)).map(|(i, _)| i).collect();
                    if steps.len() != 1 { continue; }
                    let Stmt::Assign(Expr::Var(_, VarRes::Local(s2)), Expr::Bin(op, a, step)) = &body[steps[0]] else { continue };
                    if *s2 != slot || op != "+" || !matches!(&**a, Expr::Var(_, VarRes::Local(s3)) if *s3 == slot) || !invariant(step, &assigned) { continue; }
                    if body.iter().enumerate().any(|(i, st)| i != steps[0] && assigns_slot(st, slot)) { continue; }
                    let mut sites = Vec::new();
                    for st in &body[..steps[0]] { stmt_sites(st, slot, &assigned, &mut sites); }
                    if !sites.is_empty() { plans.push((slot, sites, Bound::Stepped(limit, inclusive, step))); }
                }
            }
        }
        // a variable stepped once per iteration (`k = k + 1` at the top level of the body, nowhere else assigned), bounded by the number of
        // iterations of the range (as the merge loop's output position): the accesses before the step and those after it
        if pure(&it.coll) && pure(hi) {
            let stepped: Vec<u32> = plans.iter().map(|p| p.0).collect();
            for (si, st) in body.iter().enumerate() {
                let Stmt::Assign(Expr::Var(_, VarRes::Local(slot)), Expr::Bin(op, a, step)) = st else { continue };
                let slot = *slot;
                if op != "+" || !matches!(&**a, Expr::Var(_, VarRes::Local(s3)) if *s3 == slot) || !invariant(step, &assigned) || stepped.contains(&slot) || slot == it.slot { continue; }
                if body.iter().enumerate().any(|(i, other)| i != si && assigns_slot(other, slot)) { continue; }
                let (mut before, mut after) = (Vec::new(), Vec::new());
                for other in &body[..si] { stmt_sites(other, slot, &assigned, &mut before); }
                for other in &body[si + 1..] { stmt_sites(other, slot, &assigned, &mut after); }
                if !before.is_empty() { plans.push((slot, before, Bound::Counted(&it.coll, hi, step, false))); }
                if !after.is_empty() { plans.push((slot, after, Bound::Counted(&it.coll, hi, step, true))); }
            }
        }
        if plans.is_empty() { return Ok(None); }
        let guard = self.tmp("fast");
        self.line(&format!("int {} = 1;", guard));
        const BIG: &str = "1099511627776LL"; // 2^40: every value in the test is checked against it first, so no sum in it overflows
        let mut fast = std::collections::HashSet::new();
        let mut saves: Vec<(String, String, String)> = Vec::new(); // (nonempty test, byte offset, byte count)
        // what the guard proves about values, for the fast copy's range analysis: every bound is within 2^40, so the counter and a stepped
        // variable (which stays below its limit plus one step) are within 2^41, and arithmetic on them that cannot overflow needs no test
        const BOUND: i64 = 1 << 40;
        let mut hints: Vec<(String, (i64, i64))> = Vec::new();
        let mut counter: Option<(i64, i64)> = None;
        for (slot, sites, bound) in plans {
            // the smallest and largest value of the counter or stepped variable in the loop, and whether the loop runs at all
            let (vmin, vmax) = match bound {
                Bound::Range(lo, hi) => {
                    let lo = { let v = self.expr(lo, env, None)?; let k = self.int_or(v, 500, "a range needs integers"); self.hold_int(k) };
                    let hi = { let v = self.expr(hi, env, None)?; let k = self.int_or(v, 500, "a range needs integers"); self.hold_int(k) };
                    counter = Some((-BOUND, BOUND));
                    self.line(&format!("if (!({lo} >= -{BIG} && {lo} <= {BIG} && {hi} >= -{BIG} && {hi} <= {BIG})) {guard} = 0;"));
                    (lo, format!("({} - 1)", hi))
                }
                Bound::Counted(lo, hi, step, after) => {
                    let Some(CV::Int(start)) = env[slot as usize].clone() else { continue };
                    let start = self.hold_int(start);
                    let lo = { let v = self.expr(lo, env, None)?; let k = self.int_or(v, 500, "a range needs integers"); self.hold_int(k) };
                    let hi = { let v = self.expr(hi, env, None)?; let k = self.int_or(v, 500, "a range needs integers"); self.hold_int(k) };
                    let stepv = { let v = self.expr(step, env, None)?; let k = self.int_or(v, 500, "a step must be an integer"); self.hold_int(k) };
                    // steps times iterations within 2^40, so the bound below cannot overflow
                    self.line(&format!("if (!({start} >= -{BIG} && {start} <= {BIG} && {lo} >= -{BIG} && {lo} <= {BIG} && {hi} >= -{BIG} && {hi} <= {BIG} && {stepv} > 0 && {stepv} <= {BIG} && ({hi} <= {lo} || {stepv} <= {BIG} / ({hi} - {lo})))) {guard} = 0;"));
                    if let Some(CV::Int(name)) = env[slot as usize].clone() { hints.push((name, (-2 * BOUND, 2 * BOUND))); }
                    if let Expr::Var(_, VarRes::Local(ss)) = step { if let Some(CV::Int(name)) = env[*ss as usize].clone() { hints.push((name, (1, BOUND))); } }
                    let extra = if after { "" } else { " - 1" };
                    (start.clone(), format!("({} + ({} - {}{}) * {})", start, hi, lo, extra, stepv))
                }
                Bound::Stepped(limit, inclusive, step) => {
                    let step_expr = step;
                    let Some(CV::Int(start)) = env[slot as usize].clone() else { continue };
                    let start = self.hold_int(start);
                    let limit = { let v = self.expr(limit, env, None)?; let k = self.int_or(v, 500, "a bound must be an integer"); self.hold_int(k) };
                    let step = { let v = self.expr(step, env, None)?; let k = self.int_or(v, 500, "a step must be an integer"); self.hold_int(k) };
                    self.line(&format!("if (!({step} > 0 && {step} <= {BIG} && {start} >= -{BIG} && {start} <= {BIG} && {limit} >= -{BIG} && {limit} <= {BIG})) {guard} = 0;"));
                    if let Some(CV::Int(name)) = env[slot as usize].clone() { hints.push((name, (-2 * BOUND, 2 * BOUND))); }
                    if let Expr::Var(_, VarRes::Local(ss)) = step_expr { if let Some(CV::Int(name)) = env[*ss as usize].clone() { hints.push((name, (1, BOUND))); } }
                    (start, if inclusive { limit } else { format!("({} - 1)", limit) })
                }
            };
            self.line(&format!("if ({guard} && !({vmin} >= -{BIG} && {vmin} <= {BIG} && {vmax} >= -{BIG} && {vmax} <= {BIG})) {guard} = 0;"));
            // a short loop keeps its per-access tests: the test and the bulk save would cost more than they save
            self.line(&format!("if ({guard} && {vmax} - {vmin} < 32) {guard} = 0;"));
            // a loop that writes no state cannot change a text field's length: its reads by index are bounded by the length read here
            let reads_only = !writes(self.api, body, &mut Vec::new());
            for site in sites {
                let (off, n, esz) = match self.expr(site.arr, env, None)? {
                    CV::PlainArr(off, e, n) => (off, format!("{}LL", n), self.prog.plain_size(&e).0),
                    CV::PlainStr(off) if reads_only && !site.store => (off + 4, format!("(i64)LD_U32({}, 0)", off), 1),
                    _ => continue,
                };
                let (smin, smax) = match site.off {
                    Off::Zero => (vmin.clone(), vmax.clone()),
                    Off::Plus(c) | Off::Minus(c) | Off::From(c) => {
                        let c = { let v = self.expr(c, env, None)?; let k = self.int_or(v, 500, "an index must be an integer"); self.hold_int(k) };
                        self.line(&format!("if (!({c} >= -{BIG} && {c} <= {BIG})) {guard} = 0;"));
                        match site.off {
                            Off::Plus(_) => (format!("({} + {})", vmin, c), format!("({} + {})", vmax, c)),
                            Off::Minus(_) => (format!("({} - {})", vmin, c), format!("({} - {})", vmax, c)),
                            _ => (format!("({} - {})", c, vmax), format!("({} - {})", c, vmin)),
                        }
                    }
                };
                let nonempty = format!("{} >= {}", vmax, vmin);
                self.line(&format!("if ({guard} && {nonempty} && !({smin} >= 0 && {smax} < {n})) {guard} = 0;"));
                // an array kept out of the undo log has nothing to save before the loop stores into it
                if site.store && !self.unlogged_at(off) { saves.push((nonempty, format!("{}ULL + (u64)({}) * {}ULL", off, smin, esz), format!("(u64)({} - {} + 1) * {}ULL", smax, smin, esz))); }
                fast.insert(site.idx as *const Expr);
            }
        }
        if fast.is_empty() { return Ok(None); }
        // a counter whose loop has no array access to version still gets nothing: only plans with sites reach here
        for (nonempty, at, count) in saves {
            self.line(&format!("if ({guard} && {nonempty}) {{"));
            self.depth += 1;
            self.call(&format!("c->h->save_range(c, {}, {})", at, count));
            self.depth -= 1;
            self.line("}");
        }
        Ok(Some((guard, fast, hints, counter)))
    }

    /// One store of `size` bytes (the bits `bits`) at byte offset `addr`. A naturally aligned one (`aligned`: it cannot cross a 64-byte line or a
    /// page) is inline: the undo-log test and the store. No commit test: plain state is committed at start, and a store to an item is to a live
    /// item (from `add`, a checked index or a checked reference), whose memory `add` committed and which trimming (only between requests, only
    /// outside the live ranges) never gives back. Any other store goes to the runtime's helper, which does the checks for any alignment.
    fn emit_wr(&mut self, plain: bool, aligned: bool, addr: &str, size: u64, bits: &str) {
        if !aligned {
            let wr = if plain { "wr_plain" } else { "wr_int" };
            self.call(&format!("c->h->{}(c, {}, {}, {})", wr, addr, size, bits));
            return;
        }
        let a = self.tmp("a");
        self.open("{");
        self.line(&format!("u64 {} = (u64)({});", a, addr));
        if !std::mem::take(&mut self.skip_save) { self.line(&format!("SAVE_({}, {});", a, size)); }
        self.line(&format!("ST{}_({}, {});", size, a, bits));
        self.close("}");
    }
    /// Store `v` in a scalar column of item `idx` (element `k` of an array column when `k` is given) as a typed store in
    /// line, when its kind is known here: see `scalar_store`. Returns false when it does not apply.
    fn int_store(&mut self, c: usize, idx: &str, ci: usize, k: Option<&str>, v: &CV, slow: &str) -> R<bool> {
        let col = &self.prog.colls[c].cols[ci];
        let s = match &col.kind { CKind::Scalar(s) => *s, CKind::Arr(s, _) => *s, _ => return Ok(false) };
        let code = plain_code(&Ty::Scalar(s)).map_or(0, |x| x.0);
        let (off, whole) = (col.off, col.kind.size());
        let size = scalar_bytes(code);
        let addr = match k { Some(k) => format!("{}ULL + (u64)({}) * {}ULL + (u64)({}) * {}ULL", off, idx, whole, k, size), None => format!("{}ULL + (u64)({}) * {}ULL", off, idx, whole) };
        let aligned = off % size as usize == 0 && whole % size as usize == 0;
        self.scalar_store(code, addr, v, slow, false, aligned)
    }
    /// A typed store in line of an integer, bool or f64 `v` into a scalar of kind `code` (see `plain_code`) at byte
    /// offset `addr`: the generated code checks the value fits (an integer in a narrower column), and the generic helper
    /// (`slow`, with `@VAL` for the value) runs only for one that does not, to make the error. False if it does not apply.
    fn scalar_store(&mut self, code: u32, addr: String, v: &CV, slow: &str, plain: bool, aligned: bool) -> R<bool> {
        let w = self.tmp("w");
        let size = scalar_bytes(code);
        match (code, v) {
            (1..=8, CV::Int(e)) => {
                let (lo, hi): (Option<&str>, Option<&str>) = match code {
                    1 => (Some("-128"), Some("127")), 2 => (Some("-32768"), Some("32767")), 3 => (Some("(-2147483647LL - 1)"), Some("2147483647")),
                    4 => (None, None), 5 => (Some("0"), Some("255")), 6 => (Some("0"), Some("65535")), 7 => (Some("0"), Some("4294967295LL")), _ => (Some("0"), None),
                };
                self.open("{");
                self.line(&format!("i64 {} = {};", w, e));
                let cond = match (lo, hi) {
                    (Some(l), Some(h)) => Some(format!("{w} < {l} || {w} > {h}")),
                    (Some(l), None) => Some(format!("{w} < {l}")),
                    _ => None,
                };
                // no test when the value is proven to fit (as `x & 255` for a u8)
                let bounds: Option<(i64, i64)> = match code { 1 => Some((-128, 127)), 2 => Some((-32768, 32767)), 3 => Some((i32::MIN as i64, i32::MAX as i64)), 5 => Some((0, 255)), 6 => Some((0, 65535)), 7 => Some((0, u32::MAX as i64)), 8 => Some((0, i64::MAX)), _ => None };
                let cond = match (cond, bounds, self.interval(e)) { (Some(_), Some((bl, bh)), Some((vl, vh))) if vl >= bl && vh <= bh => None, (c, _, _) => c };
                match cond {
                    Some(cond) => {
                        self.line(&format!("if (UNLIKELY({})) {{", cond));
                        self.depth += 1;
                        let vv = self.val(CV::Int(w.clone()))?;
                        self.call(&slow.replace("@VAL", &vv));
                        self.depth -= 1;
                        self.line("} else {");
                        self.depth += 1;
                        self.emit_wr(plain, aligned, &addr, size, &w);
                        self.depth -= 1;
                        self.line("}");
                    }
                    None => self.emit_wr(plain, aligned, &addr, size, &w),
                }
                self.close("}");
            }
            (11, CV::Bool(e)) => self.emit_wr(plain, aligned, &addr, 1, &format!("(({}) ? 1 : 0)", e)),
            (10, CV::Flt(e)) => self.emit_wr(plain, aligned, &addr, 8, &format!("BITS_F64({})", e)),
            _ => return Ok(false),
        }
        Ok(true)
    }
    /// The item index of position `k` in collection `c`, instance `inst`, in a new `u32` (its name is returned). A pool
    /// is checked in line from its layout (in range, below the high-water mark, live bit set: what `h_index` checks); a
    /// failure, or a list, goes through the helper, which finds the position not live and says so.
    fn index_of(&mut self, c: usize, inst: &str, k: &str) -> String {
        let t = self.tmp("i");
        self.line(&format!("u32 {};", t));
        let coll = &self.prog.colls[c];
        if let Kind::Pool(_) = coll.kind {
            let (cap, hdr, live, words) = (coll.cap, coll.hdr, coll.live, coll.words());
            let q = self.tmp("q");
            self.open("{");
            self.line(&format!("i64 {} = {};", q, k));
            // below the high-water mark every slot is live when nothing in the program removes from the pool (a removal frees a slot; `clear` lowers the mark)
            if REMOVAL.with(|r| r.borrow().0.contains(&c)) {
                self.line(&format!("if (RANGE_({q} < 0 || {q} >= {cap}LL || {q} >= (i64)LD_U32({hdr}, {inst}))) {{"));
            } else {
                self.line(&format!("if (RANGE_({q} < 0 || {q} >= {cap}LL || {q} >= (i64)LD_U32({hdr}, {inst}) || !((LD_U64({live}, (i64)({inst}) * {words} + ({q} >> 6)) >> ({q} & 63)) & 1))) {{"));
            }
            self.depth += 1;
            self.call(&format!("c->h->index(c, {}, {}, {}, &{})", c, inst, q, t));
            self.depth -= 1;
            self.line("}");
            self.line(&format!("{} = (u32)((u64)({}) * {}ULL + (u64){});", t, inst, cap, q));
            self.close("}");
        } else {
            self.call(&format!("c->h->index(c, {}, {}, {}, &{})", c, inst, k, t));
        }
        t
    }
    /// Call a helper that returns 0 or -1.
    fn call(&mut self, s: &str) {
        // a table write or a hand-off to another worker: not seen by the offset-aware checks of plain state and collections
        if let Some(h) = s.strip_prefix("c->h->") {
            if ["tset(", "tadd(", "tdel(", "tclear(", "tinsert(", "forward"].iter().any(|w| h.starts_with(w)) { self.wrote_state.set(true); }
        }
        match self.or_dead.clone() { Some(l) => self.line(&format!("CALL_OR_DEAD({}, {});", s, l)), None => self.line(&format!("CALL({});", s)) }
    }
    fn fail(&mut self, code: u16, m: &str) { self.line(&format!("FAIL({}, {}, {});", code, c_lit(m.as_bytes()), m.len())); }
    fn open(&mut self, head: &str) { self.checked.clear(); self.line(head); self.depth += 1; }
    fn close(&mut self, tail: &str) { self.checked.clear(); self.depth -= 1; self.line(tail); }

    fn block(&mut self, stmts: &'p [Stmt], env: &mut Vec<Option<CV>>) -> R<()> {
        let (start, lets0, esc0, live0) = (self.body.len(), self.region_lets.len(), self.txt_escapes.len(), self.region_live);
        let bound_before: Vec<bool> = env.iter().map(|x| x.is_some()).collect();
        let mut live_from: Option<usize> = None; // where the first `let` of built text in this block was compiled
        let scr0 = self.scr_top; // struct locals of this block are given back at its end
        self.block_inner(stmts, env, &mut live_from)?;
        self.scr_top = scr0;
        if self.region_lets.len() > lets0 {
            let code = self.body[start..].to_string();
            // a text built in a `let` is not live at a recursive call (so each endpoint's worst case is known when compiling)
            if let Some(at) = live_from {
                let after = &self.body[at..];
                if self.specs.borrow().iter().any(|sp| sp.rec && after.contains(&format!("{}(c", sp.name))) {
                    return no("text a `let` built or a capped `var` holds is live at a recursive call here: copy it into a field first, or build it after the call");
                }
            }
            // the scope's region released at its end, unless text built in it may outlive it: an outer variable given text here, or the
            // answer cache (which keeps places in the text area between statements). A `return` leaves past the cut, so what it returns
            // stays (the caller's scope gives it back; until then its bytes count as live)
            let returns = stmts_return_value(stmts);
            let escapes = self.txt_escapes[esc0..].iter().any(|s| bound_before.get(*s as usize).copied().unwrap_or(false));
            let cached = code.contains("builtin(c, 61,") || code.contains("builtin(c, 64,");
            if !escapes && !cached {
                let mark = self.tmp("tm");
                let kept = std::mem::take(&mut self.body);
                self.line(&format!("u32 {};", mark));
                self.call(&format!("c->h->text_len(c, &{})", mark));
                let head = std::mem::replace(&mut self.body, kept);
                self.body.insert_str(start, &head);
                self.call(&format!("c->h->text_cut(c, {})", mark));
                // what a `return` leaves in the region outlives the scope only when the function returns text (or a value of a kind
                // known at run time); a number, a bool or an item leaves nothing behind
                let texty = self.ret.last().map_or(true, |r| matches!(r.kind, None | Some(K::Txt) | Some(K::Any)));
                if !returns || !texty { self.region_live = live0; }
            } else if !self.loops.is_empty() {
                // kept past its scope inside a loop: it may add up over the iterations
                let at = self.here(); self.region_unbounded.push(format!("{} (kept across a loop's rounds)", at));
            }
        }
        Ok(())
    }
    /// A `let` or capped `var` of region text, of at most `b` bytes: its size must be known when compiling (from its parts' sizes), and
    /// the endpoint's region text must fit the request's text area
    fn region_add(&mut self, b: Option<usize>, name: &str) -> R<()> {
        self.region_lets.push(b);
        let Some(b) = b else { return no(format!("`let {}` builds text whose size is not known when compiling (a part from a header, the environment, stored data or a recursive function): give it a cap, `var {}: text<N> = ...;`", name, name)) };
        self.region_live += b;
        self.region_peak = self.region_peak.max(self.region_live);
        if self.region_peak > TEXT_AREA { return no(format!("`{}`: the text in this endpoint's scopes can reach {} bytes, more than the request's text area holds ({} bytes): build less at once, or keep it in a field", name, self.region_peak, TEXT_AREA)); }
        Ok(())
    }
    /// Where code is being generated, for messages: the function being inlined (or the endpoint) and the line
    fn here(&self) -> String {
        match self.inlining.last() { Some(k) => format!("fn {} line {}", self.api.fns[*k as usize].name, self.at.0), None => format!("line {}", self.at.0) }
    }
    fn block_inner(&mut self, stmts: &'p [Stmt], env: &mut Vec<Option<CV>>, live_from: &mut Option<usize>) -> R<()> {
        let mut restore: Vec<(u32, CV)> = Vec::new();
        let mut fields_kept = 0usize;
        let mut skip = 0usize;
        for (i, s) in stmts.iter().enumerate() {
            if skip > 0 { skip -= 1; continue; }
            // a test of a plain C variable stays good into the next statement until a line writes that variable (see `line`)
            self.checked.retain(|(x, _)| is_ident(x));
            // `let r = table.add { a: x, b: y }`: the assignments to the new row that follow its `add` (as `add { ... }` is written) become
            // one INSERT with those columns
            if let Stmt::Alloc { slot, coll, .. } = s {
                let mut inits: Vec<(&'p str, &'p Expr)> = Vec::new();
                let mut k = i + 1;
                while k < stmts.len() {
                    match &stmts[k] {
                        Stmt::At(..) => {}
                        Stmt::Assign(Expr::Field(b, f), rhs) if matches!(&**b, Expr::Var(_, VarRes::Local(x)) if x == slot) && !expr_uses_slot(rhs, *slot) && !inits.iter().any(|x| x.0 == f) => inits.push((f.as_str(), rhs)),
                        _ => break,
                    }
                    k += 1;
                }
                if !inits.is_empty() {
                    if let CV::Coll(c, inst) = self.expr(coll, env, None)? {
                        if let Some(t) = tab(c) {
                            // the line markers among them are kept for errors' places; the assignments are compiled here
                            let tb = self.prog.tables[t].clone();
                            let mut cols = Vec::new(); let mut vals = Vec::new();
                            for (f, rhs) in &inits { let (ci, vv) = self.table_value(t, f, rhs, env)?; cols.push(ci); vals.push(vv); }
                            let names: Vec<String> = cols.iter().map(|ci| crate::layout::sql_ident(&tb.cols[*ci].sql)).collect();
                            let parent = tb.parent.is_some();
                            let marks: Vec<String> = (1..=names.len() + parent as usize).map(|n| format!("?{}", n)).collect();
                            let sql = format!("INSERT INTO {} ({}{}) VALUES ({})", tb.sql, if parent { "parent_id, " } else { "" }, names.join(", "), marks.join(", "));
                            let (cs, vs, r) = (self.tmp("argv"), self.tmp("argv"), self.tmp("i")); // argument arrays (the safety profile's name for them)
                            self.line(&format!("const u32 {}[{}] = {{ {} }};", cs, cols.len(), cols.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")));
                            self.line(&format!("Val {}[{}] = {{ {} }};", vs, vals.len(), vals.join(", ")));
                            self.line(&format!("u32 {};", r));
                            self.call(&format!("c->h->tinsert(c, {}, (u32)({}), {}, {}, {}, {}, &{})", t, inst, c_lit(sql.as_bytes()), cs, vs, cols.len(), r));
                            env[*slot as usize] = Some(CV::Item(c, r));
                            skip = k - i - 1;
                            continue;
                        }
                    }
                }
            }
            self.own_region = false;
            self.rest = &stmts[i + 1..];
            self.stmt(s, env)?;
            if matches!(s, Stmt::Let(..) | Stmt::Var(..)) && self.own_region && live_from.is_none() { *live_from = Some(self.body.len()); }
            self.own_region = false;
            // `if !exists(x) { ... }` (or `if x == null`) whose branch always leaves (return, fail, break, continue), or `if exists(x)
            // { ... } else { ... }` whose else always leaves: x is a plain item for the rest of this block; the same for a field `m.f`
            if let Stmt::If(c, t, e) = s {
                let leaves = |b: &[Stmt]| matches!(b.last(), Some(Stmt::Fail(..) | Stmt::Return(..) | Stmt::Respond(..) | Stmt::Break | Stmt::Continue));
                let after = |yes: bool| (!yes && leaves(t) && e.is_empty()) || (yes && !e.is_empty() && leaves(e));
                if let Some((slot, yes)) = narrow_test(c) {
                    if after(yes) && !assigned_slots(&stmts[i + 1..]).contains(&slot) {
                        if let Some(CV::Ref(cl, _, k)) = env[slot as usize].clone() { restore.push((slot, env[slot as usize].replace(CV::Item(cl, k)).unwrap())); }
                    }
                }
                if let Some((slot, f, yes)) = field_narrow_test(c) {
                    if after(yes) && !assigned_slots(&stmts[i + 1..]).contains(&slot) {
                        if let Some(CV::Item(cl, k)) = env[slot as usize].clone() { self.narrowed.push((cl, k, f)); fields_kept += 1; }
                    }
                }
            }
        }
        for _ in 0..fields_kept { self.narrowed.pop(); }
        for (slot, v) in restore.into_iter().rev() { env[slot as usize] = Some(v); }
        Ok(())
    }

    /// A whole plain field or array of at most PRESAVE_MAX bytes stored into inside a loop: saved to the undo log before the loop (whatever
    /// the loop then writes there is restored from that copy), so its stores skip the per-store test. False outside loops. The save is the
    /// stores' own inline test, once per line: a loop entered very often (a small loop in a recursive function) pays no more than before.
    fn presaved(&mut self, off: usize, size: usize) -> bool {
        const PRESAVE_MAX: usize = 512; // eight 64-byte lines: each is one inline test before the loop, about what one store's test costs
        let Some(list) = &mut self.presave else { return false };
        if size > PRESAVE_MAX { return false; }
        if !list.contains(&(off, size)) { list.push((off, size)); }
        true
    }
    /// Is this receiver one a function of a type was declared for (`fn clear(self: write Grid)`)? A name that is also a collection operation means the function
    /// when the receiver's type has one declared, and the collection operation otherwise.
    fn user_method_for(&mut self, op: &str, recv: &'p Expr, env: &mut Vec<Option<CV>>) -> R<bool> {
        let v = self.expr(recv, env, None)?;
        Ok(match self.receiver_type(&v) {
            Some(ty) => { let want = format!("{}.{}", ty, op); self.api.fns.iter().any(|f| (f.recv && f.name == want) || (!f.recv && f.test.is_none() && f.name == op && f.ptys.first().map_or(false, |t| t.is_some()))) }
            None => false,
        })
    }
    /// A statement the binder left as a call because its name is also declared for some type: where the receiver is a collection it becomes the collection statement.
    fn deferred_op(&mut self, s: &'p Stmt, env: &mut Vec<Option<CV>>) -> R<Option<&'p Stmt>> {
        let (Stmt::Let(_, _, Expr::Call(op, args, CallRes::Unbound)) | Stmt::Do(Expr::Call(op, args, CallRes::Unbound))) = s else { return Ok(None) };
        if !op.starts_with('.') || !crate::lang::AMBIGUOUS_OPS.contains(&&op[1..]) || args.is_empty() { return Ok(None); }
        if self.user_method_for(&op[1..], &args[0], env)? { return Ok(None); }
        Ok(crate::lang::lower_op(s, 0).map(|l| &*Box::leak(Box::new(l))))
    }
    fn stmt(&mut self, s: &'p Stmt, env: &mut Vec<Option<CV>>) -> R<()> {
        if let Some(l) = self.deferred_op(s, env)? {
            // the lowered statement is a collection's own: `Let` named the slot the binder declared, which the allocation takes over
            let l: &'p Stmt = match (s, l) { (Stmt::Let(_, slot, _), Stmt::Alloc { var, coll, try_, .. }) => Box::leak(Box::new(Stmt::Alloc { var: var.clone(), slot: *slot, coll: coll.clone(), try_: *try_ })), _ => l };
            return self.stmt(l, env);
        }
        if let (Stmt::Each(..), None) = (s, &self.presave) {
            // an outermost loop: generate it, then put the saves of the small fields it stores into in front of it
            let start = self.body.len();
            self.presave = Some(Vec::new());
            let r = self.stmt_inner(s, env);
            let list = self.presave.take().unwrap_or_default();
            r?;
            if !list.is_empty() {
                let kept = std::mem::take(&mut self.body);
                let mut lines: Vec<usize> = list.iter().flat_map(|(off, size)| off / 64..(off + size + 63) / 64).collect();
                lines.sort(); lines.dedup();
                for l in lines { self.line(&format!("SAVE_({}ULL, 64);", l * 64)); }
                let head = std::mem::replace(&mut self.body, kept);
                self.body.insert_str(start, &head);
            }
            return Ok(());
        }
        self.stmt_inner(s, env)
    }
    fn stmt_inner(&mut self, s: &'p Stmt, env: &mut Vec<Option<CV>>) -> R<()> {
        let rest = std::mem::take(&mut self.rest); // the statements after this one in its block
        if let Stmt::Assign(lhs, _) = s { self.check_assign_target(lhs, env)?; }
        match s {
            Stmt::At(l, c) => { self.at = (*l, *c); self.line("#undef AT_"); self.line(&format!("#define AT_ {}", l)); }
            Stmt::Break => { let l = self.loops.last_mut().ok_or("`break` outside a loop")?; l.2 = true; let g = format!("goto {};", l.0); self.line(&g); }
            Stmt::Continue => { let l = self.loops.last_mut().ok_or("`continue` outside a loop")?; l.3 = true; let g = format!("goto {};", l.1); self.line(&g); }
            Stmt::MatchArms(..) => {}
            Stmt::ContinueHere => { let l = self.loops.last_mut().ok_or("misplaced loop step")?; l.3 = false; let g = format!("{}: ;", l.1); self.line(&g); }
            Stmt::Var(n, slot, Expr::Call(f, args, _)) if f == "__mito__text_cap" => {
                // `var acc: text<N> = ...`: N bytes reserved in its scope's region (block() gives them back at the scope's end), written
                // in place by `=` and `+=`
                let (Some(Expr::Int(cap)), Some(init)) = (args.first(), args.get(1)) else { return no("`var x: text<N> = ...` expected") };
                let cap = *cap as usize;
                let t = self.tmp("t");
                self.line(&format!("Txt {} = {{ 0, 0, 1 }};", t));
                self.call(&format!("c->h->txt_cap(c, {}, &{})", cap, t));
                self.var_names.insert(t.clone());
                self.txt_caps.insert(t.clone(), cap);
                self.txt_bounds.insert(t.clone(), cap);
                self.region_add(Some(cap), n)?;
                self.txt_write(&t, cap, true, &[init], env)?;
                self.own_region = true;
                let place = match self.inlining.last() { Some(k) => format!("fn {}", self.api.fns[*k as usize].name), None => self.ep.text.clone() };
                KINDS.with(|k| k.borrow_mut().push((place, self.at.0, format!("var {}", n), format!("text (its own, up to {} bytes)", cap))));
                env[*slot as usize] = Some(CV::Txt(t));
            }
            // `let w = acc` of a capped var: a window onto it (each use reads the var as it is then), as a `let` of a field is
            Stmt::Let(n, slot, e @ Expr::Var(..)) if matches!(self.expr(e, env, None)?, CV::Txt(t) if self.txt_caps.contains_key(&t)) => {
                let v = self.expr(e, env, None)?;
                let place = match self.inlining.last() { Some(k) => format!("fn {}", self.api.fns[*k as usize].name), None => self.ep.text.clone() };
                KINDS.with(|k| k.borrow_mut().push((place, self.at.0, format!("let {}", n), "text (a window onto a var)".to_string())));
                env[*slot as usize] = Some(v);
            }
            Stmt::Let(_, slot, e) | Stmt::Var(_, slot, e) => {
                let mutable = matches!(s, Stmt::Var(..));
                self.immut.remove(&(self.frame, *slot));
                // a struct bound to a name is a view of the place it names (`V { ..x }` makes a place of its own); a struct literal builds one in the scratch area
                if let Expr::Call(f, args, _) = e { if f == "__mito__struct" { let v = self.struct_new(args)?; env[*slot as usize] = Some(v); return Ok(()); } }
                if matches!(e, Expr::Call(..)) { self.ret_ctx = Some(None); }
                let v = self.expr(e, env, None);
                self.ret_ctx = None;
                let v = v?;
                if let CV::Plain(sid, src) = v {
                    // a struct bound to a name is a view of it, never a copy (`let c = Pos { ..world.pos };` is a copy): a literal's hidden temporary is its own place
                    let v = CV::Plain(sid, src);
                    if !mutable && !matches!(s, Stmt::Let(n, ..) if n.starts_with("__mito__lit")) { self.immut.insert((self.frame, *slot)); }
                    env[*slot as usize] = Some(v);
                    return Ok(());
                }
                let init_iv = match &v { CV::Int(x) => self.interval(x), _ => None };
                // text lives in fields: a local is a window onto text (a field, an input, a literal, a piece of one), never its own copy
                // or text it builds (tests may: their messages are built text)
                if !self.test && matches!(v, CV::Txt(_) | CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..)) {
                    let n = match s { Stmt::Let(n, ..) | Stmt::Var(n, ..) => n.clone(), _ => String::new() };
                    if mutable { return no(format!("`var {}` would hold its own text: declare its cap, `var {}: text<1024> = ...;` (its bytes in its scope's region), or name a field with `let` (a window onto it)", n, n)); }
                    // (text a function returns lives in the request's text area, as a builtin's does: a window onto it)
                    // text built here (`let k = a + ":" + b`): it lives in its scope's region (block() releases it at the scope's end)
                    let built = match e { Expr::Bin(op, ..) => op == "+", Expr::Call(f, _, _) => f == "str", _ => false };
                    if built { let b = self.txt_bound(&v); self.region_add(b, &n)?; self.own_region = true; }
                }
                let v = match self.slot_kind(*slot) { Some(k) => self.coerce(v, k, mutable)?, None => self.hold(v, mutable)? };
                {
                    let place = match self.inlining.last() { Some(k) => format!("fn {}", self.api.fns[*k as usize].name), None => self.ep.text.clone() };
                    let name = match s { Stmt::Let(n, ..) | Stmt::Var(n, ..) => format!("{} {}", if mutable { "var" } else { "let" }, n), _ => String::new() };
                    let kind = match &v { CV::StateStr(..) | CV::PlainStr(_) | CV::Sub(..) => "text (a window onto a field)".to_string(), CV::Lit(_) => "text (a literal)".to_string(), other => type_text(self.prog, other) };
                    if !name.contains("__mito__") { KINDS.with(|k| k.borrow_mut().push((place, self.at.0, name, kind))); }
                }
                // a `var` that starts as a constant and is only ever assigned constants or `x & mask` stays inside one range for its whole life
                // (a hash kept to 32 bits: `h = (h * 33 + b) & 0xFFFFFFFF`): its uses may assume it, which drops overflow tests on its arithmetic
                if mutable {
                    if let (CV::Int(name), Some(r)) = (&v, masked_var_range(rest, *slot, e)) {
                        self.ranges.insert(name.clone(), r);
                        // a range inside 0..2^32-1: the C variable is a `u32` (a sum or hash kept to 32 bits then is a plain 32-bit reduction for the C compiler, which
                        // vectorizes it); every read casts it back to `i64`
                        if r.0 >= 0 && r.1 <= u32::MAX as i64 {
                            let decl = format!("i64 {} = ", name);
                            if let Some(at) = self.body.rfind(&decl) { self.body.replace_range(at..at + 3, "u32"); self.u32_vars.insert(name.clone()); }
                        }
                    }
                }
                if mutable { if let CV::Int(name) = &v { self.var_depth.insert(name.clone(), self.loop_frames.len()); } }
                if mutable { if let (CV::Int(name), Some(iv)) = (&v, init_iv) { if !self.ranges.contains_key(name) { self.var_iv.insert(name.clone(), iv); } } }
                env[*slot as usize] = Some(v);
            }
            Stmt::Assign(target @ Expr::Var(n, VarRes::Local(slot)), rhs) => {
                let cur = env[*slot as usize].clone().ok_or_else(|| format!("`{}` has no compiled value", n))?;
                // a struct local: the whole struct is copied in (its place stays where it is)
                if let CV::Plain(sid, off) = cur {
                    if matches!(rhs, Expr::Call(..)) { self.ret_ctx = Some(Some(off)); }
                    let v = self.expr(rhs, env, None);
                    self.ret_ctx = None;
                    let v = v?;
                    let CV::Plain(s2, src) = v else { return no(format!("`{}` is a `{}`: assign it a struct of that type", n, self.prog.structs[sid].name)) };
                    if s2 != sid { return no(format!("`{}` is a `{}`, not a `{}`", n, self.prog.structs[sid].name, self.prog.structs[s2].name)); }
                    return self.copy_struct(sid, off, src);
                }
                // a `write` text parameter is the caller's text field itself: assigning it stores into that field (`s = "x"`, `s += "x"`), as a field assignment does
                if matches!(cur, CV::PlainStr(_) | CV::StateStr(..)) {
                    let ascii = self.is_ascii(&cur);
                    let (clear, pieces) = match str_pieces(target, rhs) { Some(p) => p, None => (true, vec![rhs]) };
                    return match &cur {
                        CV::PlainStr(off) => {
                            let Some(cap) = self.plain_str_cap(*off) else { return no(format!("`{}` is a text of a size the compiler could not find", n)) };
                            self.may_write_plain(*off)?;
                            self.str_append(if ascii { 4 } else { 0 }, format!("{}ULL", off), format!("{}ULL", cap), 0, clear, &pieces, env)
                        }
                        CV::StateStr(c, idx, ci) => {
                            let path = self.prog.colls[*c].cols[*ci].path.clone();
                            self.may_write_col(*c, &path)?;
                            self.str_append(if ascii { 5 } else { 1 }, format!("{}ULL", c), format!("(u64)({})", idx), *ci as u32, clear, &pieces, env)
                        }
                        _ => unreachable!(),
                    };
                }
                // a capped var of text: written in place (`acc += a + b`: the pieces after what it holds; `acc = ...`: from the start)
                if let CV::Txt(t) = &cur {
                    if let Some(cap) = self.txt_caps.get(t).copied() {
                        let t = t.clone();
                        return match str_pieces(target, rhs) { Some((clear, pieces)) => self.txt_write(&t, cap, clear, &pieces, env), None => self.txt_write(&t, cap, true, &[rhs], env) };
                    }
                }
                let v = self.expr(rhs, env, None)?;
                if matches!(cur, CV::Txt(_) | CV::Any(_)) { self.txt_escapes.push(*slot); }
                if let Err(e) = self.assign_var(&cur, v.clone()) {
                    if let (Some(a), Some(b)) = (kind(&cur), kind(&v)) {
                        if unify(a, b) == K::Any { why(format!("`{}` holds {} and then {}: a var keeps the kind of its first value", n, kname(self.prog, a), kname(self.prog, b))); }
                        self.widen = Some(Widen::Slot(self.frame, *slot, unify(a, b)));
                    }
                    return Err(e);
                }
                // what is now known of the variable: the interval of the value just assigned (nothing when there is none)
                if let (CV::Int(name), CV::Int(c)) = (&cur, &v) {
                    if !self.ranges.contains_key(name) {
                        let now = self.interval(c);
                        match now { Some(iv) => { self.var_iv.insert(name.clone(), iv); } None => { self.var_iv.remove(name); } }
                        // every loop the variable is carried through must keep it inside the interval it assumes at its head: say what this assignment gives
                        if let Some(&d) = self.var_depth.get(name) {
                            for f in d..self.loop_frames.len() {
                                let key = (self.loop_frames[f], *slot);
                                LOOP_INV.with(|l| { let mut l = l.borrow_mut(); let e = l.assigns.entry(key).or_insert(now); *e = match (*e, now) { (Some(a), Some(b)) => Some(iv_union(a, b)), _ => None }; });
                            }
                        }
                    }
                }
            }
            Stmt::Assign(Expr::Field(base, f), rhs) => {
                let bv = self.expr(base, env, None)?;
                let (c, idx, path) = match bv {
                    CV::Item(c, idx) => (c, idx, f.clone()),
                    CV::Part(c, idx, p) => (c, idx, format!("{}.{}", p, f)),
                    CV::Elem(c, idx, p, k) => {
                        // `item.vals[i].a = x`: element i of the array column `vals.a`
                        let path = format!("{}.{}", p, f);
                        let Some(ci) = self.prog.colls[c].cols.iter().position(|x| x.path == path && matches!(x.kind, CKind::Arr(..))) else { return no(format!("no field `{}` in the elements of `{}`", f, p)) };
                        self.may_write_col(c, &path)?;
                        let v = self.expr(rhs, env, None)?;
                        self.skip_save = self.prog.colls[c].unlogged;
                        let inline = self.int_store(c, &idx, ci, Some(&k), &v, &format!("c->h->store_elem(c, {}, {}, {}, {}, &@VAL)", c, idx, ci, k))?;
                        self.skip_save = false;
                        if !inline {
                            let vv = self.val(v)?;
                            self.call(&format!("c->h->store_elem(c, {}, {}, {}, {}, &{})", c, idx, ci, k, vv));
                        }
                        return Ok(());
                    }
                    CV::Ref(..) => return no(format!("assigning `.{}` of a value that may be null: check it first with `if exists(x) {{ ... }}` (inside, x is the item), or give a fallback with `or(...)`", f)),
                    CV::Plain(sid, off) => {
                        let sd = &self.prog.structs[sid];
                        let Some(k) = sd.fields.iter().position(|x| x.0 == *f) else { return no(format!("no field `{}`{}", f, crate::lang::suggest(f, sd.fields.iter().map(|x| x.0.as_str())))) };
                        self.touch_request(off, sid, k);
                        let sd = &self.prog.structs[sid];
                        // a struct-valued field: the whole struct is copied in (`world.pos = v;`)
                        if let Ty::Struct(s2) = sd.fields[k].1 {
                            let at = off + sd.offs[k];
                            if matches!(rhs, Expr::Call(..)) { self.ret_ctx = Some(Some(at)); }
                            let v = self.expr(rhs, env, None);
                            self.ret_ctx = None;
                            let v = v?;
                            let CV::Plain(s3, src) = v else { return no(format!("`.{}` is a `{}`: assign it a struct of that type", f, self.prog.structs[s2].name)) };
                            if s3 != s2 { return no(format!("`.{}` is a `{}`, not a `{}`", f, self.prog.structs[s2].name, self.prog.structs[s3].name)); }
                            self.prog.value_check(s2)?;
                            return self.copy_struct(s2, at, src);
                        }
                        let (code, cap) = plain_code(&sd.fields[k].1).ok_or_else(|| format!("assigning plain `.{}` not compiled yet", f))?;
                        let at = off + sd.offs[k];
                        let range = sd.ranges[k];
                        self.may_write_plain(at)?;
                        if let Some(target) = self.prog.id_at(crate::layout::IdAt::Plain(at)) {
                            self.field_store_seen(at, None);
                            let v = self.expr(rhs, env, None)?;
                            let v = self.id_store_value(target, v, f)?;
                            let vv = self.val(v)?;
                            self.call(&format!("c->h->store_plain(c, {}, {}, {}, &{})", at, code, cap, vv));
                            return Ok(());
                        }
                        let ascii = code == 20 && range.is_some();
                        if code == 20 { if let Some((clear, pieces)) = str_pieces(&Expr::Field(base.clone(), f.clone()), rhs) { return self.str_append(if ascii { 4 } else { 0 }, format!("{}ULL", at), format!("{}ULL", cap), 0, clear, &pieces, env); } }
                        let v = self.expr(rhs, env, None)?;
                        if ascii { self.ascii_lit(&v, f)?; }
                        let mut v = match range { Some(r) if code != 20 => self.in_field_range(v, r, f)?, _ => v };
                        if range.is_none() && (1..=8).contains(&code) {
                            let v2 = match v { CV::Int(e) => { let n = self.hold_int(e); CV::Int(n) } v => v };
                            let r = match &v2 { CV::Int(n) => self.interval(n), _ => None };
                            self.field_store_seen(at, r);
                            v = v2;
                        }
                        let code = if ascii { 21 } else { code };
                        self.skip_save = self.unlogged_at(at) || (code != 21 && self.presaved(at, scalar_bytes(code) as usize));
                        if self.scalar_store(code, format!("{}ULL", at), &v, &format!("c->h->store_plain(c, {}, {}, {}, &@VAL)", at, code, cap), true, at % scalar_bytes(code) as usize == 0)? { return Ok(()); }
                        self.skip_save = false;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store_plain(c, {}, {}, {}, &{})", at, code, cap, vv));
                        return Ok(());
                    }
                    CV::Null => return no(format!("assigning `.{}` of `null`", f)),
                    CV::Any(_) => return no(format!("assigning `.{}` of a value whose kind is only known when the program runs, which may be null: give it a known kind (a plain item)", f)),
                    _ => { self.fail(500, "assignment target is not an item or a state struct"); return Ok(()); }
                };
                if let Some(t) = tab(c) { return self.table_set(t, &idx, &path, rhs, env); }
                let Some(ci) = self.prog.colls[c].cols.iter().position(|x| x.path == path && !matches!(x.kind, CKind::StrBytes(_) | CKind::Arr(..))) else {
                    let prefix = format!("{}.", path);
                    let known = self.prog.colls[c].cols.iter().any(|x| x.path == path || x.path.starts_with(&prefix)) || self.prog.colls[c].children.iter().any(|x| x.0 == path);
                    return no(if known { format!("assigning `.{}` as a whole is not supported yet; assign its fields", path) } else { format!("no field `{}`", path) });
                };
                self.may_write_col(c, &path)?;
                if let Some(target) = self.prog.id_at(crate::layout::IdAt::Col(c, ci)) {
                    let v = self.expr(rhs, env, None)?;
                    let v = self.id_store_value(target, v, &path)?;
                    let vv = self.val(v)?;
                    self.call(&format!("c->h->store(c, {}, {}, {}, &{})", c, idx, ci, vv));
                    return Ok(());
                }
                let is_text = matches!(self.prog.colls[c].cols[ci].kind, CKind::StrLen);
                let ascii = is_text && self.prog.colls[c].cols[ci].range.is_some();
                if is_text {
                    if let Some((clear, pieces)) = str_pieces(&Expr::Field(base.clone(), f.clone()), rhs) { return self.str_append(if ascii { 5 } else { 1 }, format!("{}ULL", c), format!("(u64)({})", idx), ci as u32, clear, &pieces, env); }
                }
                let v = self.expr(rhs, env, None)?;
                if ascii { self.ascii_lit(&v, &path)?; }
                let v = match self.prog.colls[c].cols[ci].range { Some(r) if !is_text => self.in_field_range(v, r, &path)?, _ => v };
                self.skip_save = self.prog.colls[c].unlogged;
                let inline = self.int_store(c, &idx, ci, None, &v, &format!("c->h->store(c, {}, {}, {}, &@VAL)", c, idx, ci))?;
                self.skip_save = false;
                if !inline {
                    let vv = self.val(v)?;
                    self.call(&format!("c->h->store(c, {}, {}, {}, &{})", c, idx, ci, vv));
                }
            }
            Stmt::Assign(Expr::Index(base, k_expr), rhs) => {
                let bv = self.expr(base, env, None)?;
                let kv = self.expr(k_expr, env, None)?;
                let k = self.int_or(kv, 400, "an index must be an integer");
                if let CV::PlainStr(_) | CV::StateStr(..) = &bv {
                    // `s[i] = b` on ascii text: a byte 0..127 at a position below its length
                    if !self.is_ascii(&bv) { return no("utf8 text is not written by index (a character may take several bytes): assign it or append to it, or make the field ascii<N>"); }
                    match &bv { CV::PlainStr(off) => self.may_write_plain(*off)?, CV::StateStr(c, _, ci) => { let p = self.prog.colls[*c].cols[*ci].path.clone(); self.may_write_col(*c, &p)? } _ => {} }
                    let k = self.hold_int(k);
                    let len = self.field_len(&bv);
                    self.line(&format!("if (RANGE_({k} < 0 || {k} >= {len})) FAIL(400, \"string index out of range\", 25);"));
                    let v = self.expr(rhs, env, None)?;
                    let x = self.int_or(v, 400, "a byte of ascii text is an integer 0..127");
                    let x = self.hold_int(x);
                    self.line(&format!("if (RANGE_({x} < 0 || {x} > 127)) FAIL(400, \"not an ASCII byte\", 17);"));
                    let at = self.sub_bytes_at(&bv, &k);
                    self.call(&format!("c->h->wr_int(c, {}, 1, {})", at, x));
                    return Ok(());
                }
                if let CV::Coll(..) | CV::ArrCol(..) | CV::PlainArr(..) = bv {} else { self.fail(500, "only fixed arrays and collections can be assigned by index"); return Ok(()); }
                match bv {
                    CV::ArrCol(c, idx, ci) => {
                        self.may_write_col(c, &self.prog.colls[c].cols[ci].path.clone())?;
                        let CKind::Arr(_, n) = self.prog.colls[c].cols[ci].kind else { unreachable!() };
                        self.elem_check(&k, n as u64);
                        let v = self.expr(rhs, env, None)?;
                        self.skip_save = self.prog.colls[c].unlogged;
                        let inline = self.int_store(c, &idx, ci, Some(&k), &v, &format!("c->h->store_elem(c, {}, {}, {}, {}, &@VAL)", c, idx, ci, k))?;
                        self.skip_save = false;
                        if !inline {
                            let vv = self.val(v)?;
                            self.call(&format!("c->h->store_elem(c, {}, {}, {}, {}, &{})", c, idx, ci, k, vv));
                        }
                    }
                    CV::PlainArr(off, e, n) => {
                        self.may_write_plain(off)?;
                        let Some((code, cap)) = plain_code(&e) else { return no("assigning an element of this array not compiled yet") };
                        let covered = self.fast_sites.contains(&(&**k_expr as *const Expr));
                        if !covered { self.elem_check(&k, n as u64); }
                        let v = self.expr(rhs, env, None)?;
                        // what the whole program stores into this array is what its reads may assume (see `generate_with`)
                        let stored = match &v { CV::Int(x) => self.interval(x), _ => None };
                        self.field_store_seen(off, stored);
                        let esz = self.prog.plain_size(&e).0;
                        self.skip_save = covered || self.unlogged_at(off) || self.presaved(off, n as usize * esz as usize);
                        if self.scalar_store(code, format!("{}ULL + (u64)({}) * {}ULL", off, k, esz), &v, &format!("c->h->store_plain(c, (u32)({} + {} * {}), {}, {}, &@VAL)", off, k, esz, code, cap), true, off % esz as usize == 0)? { self.skip_save = false; return Ok(()); }
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store_plain(c, (u32)({} + {} * {}), {}, {}, &{})", off, k, esz, code, cap, vv));
                        self.skip_save = false;
                    }
                    CV::Coll(c, inst) => {
                        self.may_write_col(c, "")?;
                        let t = self.index_of(c, &inst, &k);
                        let coll = &self.prog.colls[c];
                        let plain = if coll.children.iter().any(|x| x.0.is_empty()) { None } else { coll.cols.iter().position(|x| x.path.is_empty()) };
                        let Some(ci) = plain else { self.fail(500, "only an item that is a plain value can be assigned; assign its fields"); return Ok(()); };
                        let v = self.expr(rhs, env, None)?;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store(c, {}, {}, {}, &{})", c, t, ci, vv));
                    }
                    _ => return no("only fixed arrays and collections can be assigned by index"),
                }
            }
            Stmt::Assign(..) => return no("assignment target not compiled yet"),
            Stmt::If(c, t, e) => {
                // `if exists(x) && more { T } else { E }` is `if exists(x) { if more { T } else { E } } else { E }`: `more` and T see x
                // narrowed, and nothing of `more` runs before the test
                if let Some((a, b)) = split_after_narrowing(c) {
                    let inner: &'p Stmt = Box::leak(Box::new(Stmt::If(b, t.clone(), e.clone())));
                    let outer: &'p Stmt = Box::leak(Box::new(Stmt::If(a, vec![inner.clone()], e.clone())));
                    return self.stmt(outer, env);
                }
                // `if exists(m.f)`: in the branch where it holds, m's field f is not null (a nullable time a number, a reference an item)
                let field_key = field_narrow_test(c).and_then(|(slot, f, yes)| match env[slot as usize].clone() { Some(CV::Item(cl, k)) => Some(((cl, k, f), yes)), _ => None });
                // `if exists(x)` / `if x != null`: inside, x is a plain item; `if !exists(x)` / `if x == null`: in the else branch
                let narrowed = narrow_test(c).and_then(|(slot, yes)| match env[slot as usize].clone() { Some(CV::Ref(cl, _, k)) => Some((slot, yes, cl, k)), _ => None });
                let cv = self.expr(c, env, None)?;
                let cond = self.cond(cv)?;
                self.open(&format!("if ({}) {{", cond));
                let narrow = |g: &mut Self, env: &mut Vec<Option<CV>>, branch: &'p [Stmt], want: bool| -> Option<CV> {
                    let (slot, yes, cl, k) = narrowed.clone()?;
                    if yes != want || assigned_slots(branch).contains(&slot) { return None; }
                    let _ = g; env[slot as usize].replace(CV::Item(cl, k))
                };
                let iv_before = self.var_iv.clone();
                self.refine(c, env)?; // inside the branch the condition holds
                let kept = narrow(self, env, t, true);
                let fk = match &field_key { Some((k, true)) => { self.narrowed.push(k.clone()); true } _ => false };
                self.block(t, env)?;
                if fk { self.narrowed.pop(); }
                if let (Some(v), Some((slot, ..))) = (kept, narrowed.clone()) { env[slot as usize] = Some(v); }
                self.close("}");
                let iv_then = std::mem::replace(&mut self.var_iv, iv_before.clone());
                if !e.is_empty() {
                    self.open("else {");
                    let kept = narrow(self, env, e, false);
                    let fk = match &field_key { Some((k, false)) => { self.narrowed.push(k.clone()); true } _ => false };
                    self.block(e, env)?;
                    if fk { self.narrowed.pop(); }
                    if let (Some(v), Some((slot, ..))) = (kept, narrowed.clone()) { env[slot as usize] = Some(v); }
                    self.close("}");
                }
                // after the `if`: what both ways agree on (a way that always leaves counts for nothing)
                let leaves = |b: &[Stmt]| matches!(b.last(), Some(Stmt::Fail(..) | Stmt::Return(..) | Stmt::Respond(..) | Stmt::Break | Stmt::Continue));
                let iv_else = std::mem::take(&mut self.var_iv);
                self.var_iv = match (leaves(t), !e.is_empty() && leaves(e)) {
                    (true, false) => iv_else,
                    (false, true) => iv_then,
                    _ => iv_then.iter().filter_map(|(k, a)| iv_else.get(k).map(|b| (k.clone(), (a.0.min(b.0), a.1.max(b.1))))).collect(),
                };
            }
            Stmt::Each(it, body) => {
                if let (Some(_), Some(n)) = (&it.via, crate::lang::path_of(&it.coll)) {
                    let n = &n;
                    if changes_instance(self.api, n, body) { return no(format!("a loop over {} calls a function that changes {}; change it after the loop", n, n)); }
                }
                let collect = allocates(self.api, body, &mut Vec::new()) || (it.wher.is_some() && writes(self.api, body, &mut Vec::new()));
                // a loop that writes no state cannot change a text field's length: read it once, here, for the reads by index inside
                let mut cached = Vec::new();
                if !assigns_field(self.api, body, &mut Vec::new()) {
                    let mut bases = Vec::new();
                    index_bases(body, &mut bases);
                    for arr in bases {
                        if let CV::PlainStr(off) = self.expr(arr, env, None)? {
                            if self.str_lens.contains_key(&off) { continue; }
                            let t = self.tmp("sl");
                            self.line(&format!("i64 {} = (i64)LD_U32({}, 0);", t, off));
                            self.str_lens.insert(off, t);
                            cached.push(off);
                        }
                    }
                }
                // what is known of the variables the loop assigns is dropped for its body, except what a counted loop can say of a counter (see `counter_heads`); after it,
                // the variables it does not assign keep what they had
                let entry_iv = self.var_iv.clone();
                let mut base_iv = entry_iv.clone();
                for slot in assigned_slots(body) { if let Some(Some(CV::Int(n))) = env.get(slot as usize) { base_iv.remove(n); } }
                let loop_id = it as *const Iter as usize;
                for slot in assigned_slots(body) {
                    if let Some(Some(CV::Int(n))) = env.get(slot as usize) {
                        if let (true, Some(iv)) = (self.var_names.contains(n), entry_iv.get(n)) {
                            LOOP_INV.with(|l| { let mut l = l.borrow_mut(); let e = l.entries.entry((loop_id, slot)).or_insert(*iv); *e = iv_union(*e, *iv); });
                        }
                    }
                }
                let (pre, n_pre) = (self.body.len(), self.n);
                match self.version_guard(it, body, env)? {
                    Some((guard, sites, hints, counter)) => {
                        // the fast copy: the guard proved every listed array access of the loop in range and saved the stored ranges to the undo log
                        self.versioned += 1;
                        let fast_start = self.body.len();
                        self.open(&format!("if ({}) {{", guard));
                        let joined: std::collections::HashSet<*const Expr> = sites.union(&self.fast_sites).copied().collect();
                        let kept = std::mem::replace(&mut self.fast_sites, joined);
                        let ranges_kept = self.ranges.clone();
                        for (name, iv) in hints { self.ranges.insert(name, iv); }
                        self.loop_hint = counter;
                        self.loop_vars(it, body, env, collect, &entry_iv, &base_iv)?;
                        self.loop_hint = None;
                        self.ranges = ranges_kept;
                        self.fast_sites = kept;
                        self.close("}");
                        let slow_start = self.body.len();
                        self.open("else {");
                        self.loop_vars(it, body, env, collect, &entry_iv, &base_iv)?;
                        self.close("}");
                        self.versioned -= 1;
                        // the two copies came out the same (the range analysis had already proved every access, nothing was left to drop):
                        // keep one, without the guard
                        let guard_text = &self.body[pre..fast_start];
                        let fast = self.body[fast_start..slow_start].strip_prefix(&"    ".repeat(self.depth + 1)).and_then(|t| t.strip_prefix(&format!("if ({}) ", guard)));
                        let slow = self.body[slow_start..].strip_prefix(&"    ".repeat(self.depth + 1)).and_then(|t| t.strip_prefix("else "));
                        if let (Some(fast), Some(slow)) = (fast, slow) {
                            if same_code(n_pre, guard_text, fast, slow) {
                                let one = format!("{}{}", "    ".repeat(self.depth + 1), slow);
                                self.body.truncate(pre);
                                self.body.push_str(&one);
                            }
                        }
                    }
                    None => self.loop_vars(it, body, env, collect, &entry_iv, &base_iv)?,
                }
                let mut heads = self.loop_heads.take().unwrap_or_default();
                // a variable with no bound from the loop's rounds has what this build assumes of it at the loop's head, in the body and after (see `generate_with`)
                for slot in assigned_slots(body) {
                    if let Some(Some(CV::Int(n))) = env.get(slot as usize) {
                        if let (false, Some(iv)) = (heads.contains_key(n), LOOP_INV.with(|l| l.borrow().assumed.get(&(loop_id, slot)).copied())) { if entry_iv.contains_key(n) { heads.insert(n.clone(), iv); } }
                    }
                }
                self.var_iv = base_iv;
                self.var_iv.extend(heads);
                for off in cached { self.str_lens.remove(&off); }
            }
            Stmt::Move(arr, from, to, count) => {
                let av = self.expr(arr, env, None)?;
                let mut pos = |g: &mut Self, e: &'p Expr, env: &mut Vec<Option<CV>>| -> R<String> { let v = g.expr(e, env, None)?; let k = g.int_or(v, 400, "a position and a count must be integers"); Ok(g.hold_int(k)) };
                let (f, t2, n) = (pos(self, from, env)?, pos(self, to, env)?, pos(self, count, env)?);
                if let CV::Part(c, idx, p) = &av {
                    // an array of structs: every field's column moves alike
                    let inner = format!("{}.", p);
                    let cols: Vec<usize> = (0..self.prog.colls[*c].cols.len()).filter(|&i| self.prog.colls[*c].cols[i].path.starts_with(&inner) && matches!(self.prog.colls[*c].cols[i].kind, CKind::Arr(..))).collect();
                    if cols.is_empty() { return no("`move` works on a fixed array"); }
                    let CKind::Arr(_, len) = self.prog.colls[*c].cols[cols[0]].kind else { unreachable!() };
                    self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {len}LL || {t2} < 0 || {t2} > {len}LL || {n} < 0 || {n} > {len}LL || {f} + {n} > {len}LL || {t2} + {n} > {len}LL)) FAIL(400, \"move out of range\", 17);"));
                    for ci in cols {
                        let path = self.prog.colls[*c].cols[ci].path.clone();
                        self.may_write_col(*c, &path)?;
                        let col = &self.prog.colls[*c].cols[ci];
                        let (base, esize) = (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), col.kind.size() / len);
                        self.call(&format!("c->h->move_elems(c, {}, {}, {}, {}, {})", base, esize, f, t2, n));
                    }
                    return Ok(());
                }
                let mut unlogged_plain = false;
                let (base, esize, len) = match av {
                    CV::ArrCol(c, idx, ci) => {
                        self.may_write_col(c, &self.prog.colls[c].cols[ci].path.clone())?;
                        let col = &self.prog.colls[c].cols[ci];
                        let CKind::Arr(_, len) = col.kind else { unreachable!() };
                        (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), col.kind.size() / len, len)
                    }
                    CV::PlainArr(off, e, len) => { self.may_write_plain(off)?; unlogged_plain = self.unlogged_at(off); (format!("{}ULL", off), self.prog.plain_size(&e).0, len) }
                    _ => return no("`move` works on a fixed array"),
                };
                self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {len}LL || {t2} < 0 || {t2} > {len}LL || {n} < 0 || {n} > {len}LL || {f} + {n} > {len}LL || {t2} + {n} > {len}LL)) FAIL(400, \"move out of range\", 17);"));
                if unlogged_plain {
                    // an unlogged array of plain state: committed at start and kept out of the undo log, so the move is the copy alone (as an inline store of a scalar there is)
                    self.line(&format!("MOVE_({base} + (u64)({t2}) * {esize}ULL, {base} + (u64)({f}) * {esize}ULL, (u64)({n}) * {esize}ULL);"));
                } else {
                    self.call(&format!("c->h->move_elems(c, {}, {}, {}, {}, {})", base, esize, f, t2, n));
                }
            }
            Stmt::Copy(dst, src, from, to, count) => {
                let dv = self.expr(dst, env, None)?;
                let sv = self.expr(src, env, None)?;
                let mut pos = |g: &mut Self, e: &'p Expr, env: &mut Vec<Option<CV>>| -> R<String> { let v = g.expr(e, env, None)?; let k = g.int_or(v, 400, "a position and a count must be integers"); Ok(g.hold_int(k)) };
                let (f, t2, n) = (pos(self, from, env)?, pos(self, to, env)?, pos(self, count, env)?);
                let (CV::PlainArr(doff, de, dlen), CV::PlainArr(soff, se, slen)) = (&dv, &sv) else { return no("`copy_from` copies between two fixed arrays of plain state: `dst.copy_from(src, from, to, count)`") };
                self.may_write_plain(*doff)?;
                let from_src = FIELD_RANGES.with(|fr| fr.borrow().0.get(soff).copied());
                self.field_store_seen(*doff, from_src);
                let (desz, sesz) = (self.prog.plain_size(de).0, self.prog.plain_size(se).0);
                if desz != sesz { return no("`copy_from` copies between arrays of the same element type"); }
                self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {slen}LL || {t2} < 0 || {t2} > {dlen}LL || {n} < 0 || {n} > {slen}LL || {f} + {n} > {slen}LL || {t2} + {n} > {dlen}LL)) FAIL(400, \"copy out of range\", 17);"));
                // a logged destination first saves the lines it is about to overwrite (an `unlogged` one is scratch the program rebuilds, nothing to restore)
                if !self.unlogged_at(*doff) {
                    let (lo, hi) = (format!("({doff}ULL + (u64)({t2}) * {desz}ULL)"), format!("({doff}ULL + (u64)({t2} + {n}) * {desz}ULL)"));
                    self.open(&format!("if ({n} > 0) for (u64 l_ = {lo} >> 6; l_ <= ({hi} - 1) >> 6; l_++) {{"));
                    self.line("SAVE_(l_ << 6, 64);");
                    self.close("}");
                }
                // two different arrays never overlap: a plain copy
                self.line(&format!("COPY_({doff}ULL + (u64)({t2}) * {desz}ULL, {soff}ULL + (u64)({f}) * {desz}ULL, (u64)({n}) * {desz}ULL);"));
            }
            Stmt::Sort(arr, from, count, scratch) => {
                let av = self.expr(arr, env, None)?;
                let tv = self.expr(scratch, env, None)?;
                let mut pos = |g: &mut Self, e: &'p Expr, env: &mut Vec<Option<CV>>| -> R<String> { let v = g.expr(e, env, None)?; let k = g.int_or(v, 400, "a position and a count must be integers"); Ok(g.hold_int(k)) };
                let (f, n) = (pos(self, from, env)?, pos(self, count, env)?);
                let (CV::PlainArr(aoff, Ty::Scalar(asc), alen), CV::PlainArr(toff, Ty::Scalar(tsc), tlen)) = (&av, &tv) else { return no("`sort` puts an array of integers in order, with another array of the same type to work in: `a.sort(from, count, scratch)`") };
                let (code, tcode) = (plain_code(&Ty::Scalar(*asc)).map_or(0, |x| x.0), plain_code(&Ty::Scalar(*tsc)).map_or(0, |x| x.0));
                if !(1..=8).contains(&code) || code != tcode { return no("`sort` puts an array of integers in order, with another array of the same type to work in"); }
                if aoff == toff { return no("`sort` works in another array than the one it sorts"); }
                self.may_write_plain(*aoff)?;
                self.may_write_plain(*toff)?;
                // the values only move: the array keeps what it was assumed to hold; the scratch array is given a copy of them
                let held = FIELD_RANGES.with(|fr| fr.borrow().0.get(aoff).copied());
                self.field_store_seen(*toff, held);
                self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {alen}LL || {n} < 0 || {n} > {alen}LL || {f} + {n} > {alen}LL || {n} > {tlen}LL)) FAIL(400, \"sort out of range\", 17);"));
                self.call(&format!("c->h->sort_elems(c, {}ULL, {}, {}, {}, {}ULL)", aoff, code, f, n, toff));
            }
            Stmt::Fill(arr, from, count, value) => {
                let av = self.expr(arr, env, None)?;
                let mut pos = |g: &mut Self, e: &'p Expr, env: &mut Vec<Option<CV>>| -> R<String> { let v = g.expr(e, env, None)?; let k = g.int_or(v, 400, "a position and a count must be integers"); Ok(g.hold_int(k)) };
                let (f, n) = (pos(self, from, env)?, pos(self, count, env)?);
                let v = self.expr(value, env, None)?;
                let (base, code, len, plain) = match av {
                    CV::ArrCol(c, idx, ci) => {
                        self.may_write_col(c, &self.prog.colls[c].cols[ci].path.clone())?;
                        let col = &self.prog.colls[c].cols[ci];
                        let CKind::Arr(s, len) = col.kind else { unreachable!() };
                        (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), plain_code(&Ty::Scalar(s)).map_or(0, |x| x.0), len, 0)
                    }
                    CV::PlainArr(off, e, len) => {
                        self.may_write_plain(off)?;
                        let stored = match &v { CV::Int(x) => self.interval(x), _ => None };
                        self.field_store_seen(off, stored);
                        (format!("{}ULL", off), plain_code(&e).map_or(0, |x| x.0), len, 1)
                    }
                    _ => return no("`fill` works on a fixed array"),
                };
                let esize = scalar_bytes(code);
                let w = self.tmp("w");
                self.open("{");
                let bits = match (code, &v) {
                    (1..=8, CV::Int(e)) => {
                        self.line(&format!("i64 {} = {};", w, e));
                        let (lo, hi): (Option<&str>, Option<&str>) = match code {
                            1 => (Some("-128"), Some("127")), 2 => (Some("-32768"), Some("32767")), 3 => (Some("(-2147483647LL - 1)"), Some("2147483647")),
                            4 => (None, None), 5 => (Some("0"), Some("255")), 6 => (Some("0"), Some("65535")), 7 => (Some("0"), Some("4294967295LL")), _ => (Some("0"), None),
                        };
                        let cond = match (lo, hi) { (Some(l), Some(h)) => Some(format!("{w} < {l} || {w} > {h}")), (Some(l), None) => Some(format!("{w} < {l}")), _ => None };
                        if let Some(cond) = cond { self.line(&format!("if (UNLIKELY({})) FAIL(413, \"value does not fit the array's elements\", 39);", cond)); }
                        w.clone()
                    }
                    (11, CV::Bool(e)) => format!("(({}) ? 1 : 0)", e),
                    (10, CV::Flt(e)) => format!("BITS_F64({})", e),
                    _ => return no("`fill` stores an integer, bool or f64 matching the array"),
                };
                self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {len}LL || {n} < 0 || {n} > {len}LL || {f} + {n} > {len}LL)) FAIL(400, \"fill out of range\", 17);"));
                self.call(&format!("c->h->fill_elems(c, {}, {}, {}, {}, {}, {})", base, esize, f, n, bits, plain));
                self.close("}");
            }
            Stmt::Clear(coll) => {
                let v = self.expr(coll, env, None)?;
                if let CV::PlainStr(_) | CV::StateStr(..) = &v {
                    // `s.clear()`: the text becomes empty
                    match &v { CV::PlainStr(off) => self.may_write_plain(*off)?, CV::StateStr(c, _, ci) => { let p = self.prog.colls[*c].cols[*ci].path.clone(); self.may_write_col(*c, &p)? } _ => {} }
                    let at = self.len_at(&v);
                    self.call(&format!("c->h->wr_int(c, {}, 4, 0)", at));
                    return Ok(());
                }
                let CV::Coll(c, inst) = v else { return no("`clear` is a function of a collection or text") };
                if let Some(t) = tab(c) { self.call(&format!("c->h->tclear(c, {}, (u32)({}))", t, inst)); return Ok(()); } // every row (of this owner), and the rows they own
                self.may_add_or_free(c, "clear")?;
                self.call(&format!("c->h->clear_coll(c, {}, {})", c, inst));
            }
            Stmt::Remove(coll, e) => {
                let CV::Coll(want, _) = self.expr(coll, env, None)? else { return no("`remove` is a function of a collection") };
                let wname = coll_name(self.prog, want);
                let other = |c: usize| no(format!("an item of {} cannot be removed from {}", coll_name(self.prog, c), wname));
                match self.expr(e, env, None)? {
                    CV::Item(c, i) if tab(c).is_some() => { if c != want { return no(format!("a row of {} cannot be removed from {}", coll_name(self.prog, c), coll_name(self.prog, want))); } self.call(&format!("c->h->tdel(c, {}, {})", c - TBASE, i)) }
                    CV::Item(c, i) => { if c != want { return other(c); } self.may_add_or_free(c, "remove")?; self.call(&format!("c->h->free_(c, {}, {})", c, i)) }
                    CV::Ref(..) => return no("removing a value that may be null: check it first with `if exists(x) {{ ... }}` (inside, x is the item), or give a fallback with `or(...)`"),
                    CV::Null => {}
                    CV::Any(x) => {
                        // a value whose kind is known only at run time: it must be an item of this collection
                        self.may_add_or_free(want, "remove")?;
                        self.line(&format!("if ({}.tag == 5 && {}.a != {}) FAIL(500, {}, {});", x, x, want, c_lit(format!("not an item of {}", wname).as_bytes()), format!("not an item of {}", wname).len()));
                        self.call(&format!("c->h->free_val(c, &{})", x))
                    }
                    _ => self.fail(500, "remove needs an item"),
                }
            }
            Stmt::Fail(code, m) => self.fail(*code, m),
            Stmt::Respond(303, Json::E(e)) => {
                // `see_other(path)`: from any endpoint, whatever its Response<T>; the runtime checks the path is on this server
                let v = self.expr(e, env, None)?;
                if let CV::Lit(s) = &v {
                    let b = s.as_bytes();
                    if !(b.first() == Some(&b'/') && !matches!(b.get(1), Some(b'/' | b'\\')) && b.iter().all(|c| *c > 0x20 && *c < 0x7F)) {
                        return no(format!("`see_other(\"{}\")`: the address is a path on this server, one leading `/`, printable ASCII, no spaces (encode them as %20)", s));
                    }
                }
                let t = self.txt_of(v).map_err(|_| "`see_other` takes the path as text, as in `see_other(\"/active\")`".to_string())?;
                if let Some((kind, _)) = text_answer(&self.ep.resp) { self.call(&format!("c->h->answer(c, {})", kind)); }
                self.call(&format!("c->h->see_other(c, &{})", t));
                self.line("return 303;");
            }
            Stmt::Respond(302, Json::E(e)) => {
                // `found(path)`: a 302 from any endpoint, its Location the path made absolute with the request's Host (checked by the runtime)
                let v = self.expr(e, env, None)?;
                if let CV::Lit(s) = &v {
                    let b = s.as_bytes();
                    if !(b.first() == Some(&b'/') && !matches!(b.get(1), Some(b'/' | b'\\')) && b.iter().all(|c| *c > 0x20 && *c < 0x7F)) {
                        return no(format!("`found(\"{}\")`: the address is a path on this server, one leading `/`, printable ASCII, no spaces (encode them as %20)", s));
                    }
                }
                let t = self.txt_of(v).map_err(|_| "`found` takes the path as text, as in `found(\"/\")`".to_string())?;
                // the answer's content type is the endpoint's (an HTML endpoint's redirect says text/html, as Rails')
                if let Some((kind, _)) = text_answer(&self.ep.resp) { self.call(&format!("c->h->answer(c, {})", kind)); }
                self.call(&format!("c->h->found(c, &{})", t));
                self.line("return 302;");
            }
            Stmt::Respond(code, Json::Html) => {
                // `ok(<markup>)`: the statements before wrote the body
                if !self.html_answer() { return no("markup is an HTML answer: the endpoint returns `Response<html>`"); }
                self.call("c->h->answer(c, 1)");
                self.line(&format!("return {};", code));
            }
            Stmt::Respond(code, Json::E(e)) if text_answer(&self.ep.resp).is_some() => {
                // an answer of text (HTML, plain text, CSS, JavaScript, SVG): the text sent as it is; one that is a literal (as
                // `file("app.css")`) is the same every time, so it gets an ETag and a browser that has it is answered 304
                let (kind, what) = text_answer(&self.ep.resp).unwrap();
                let v = self.expr(e, env, None)?;
                self.call(&format!("c->h->answer(c, {})", kind));
                if let CV::Lit(s) = &v {
                    let tag = format!("\"{:016x}\"", fnv64(s.as_bytes()));
                    let nm = self.tmp("nm");
                    self.line(&format!("int {} = 0;", nm));
                    self.call(&format!("c->h->etag(c, {}, {}, &{})", c_lit(tag.as_bytes()), tag.len(), nm));
                    self.line(&format!("if ({}) return 304;", nm));
                }
                self.raw(v, what)?;
                self.line(&format!("return {};", code));
            }
            Stmt::Respond(code, j) => {
                let resp = self.ep.resp.clone();
                if text_answer(&resp).is_some() { return no(format!("a Response<{}> answer is text: `ok(page.html)`, `ok(\"...\")`, `ok(file(\"app.css\"))`", show_rtype(&resp))); }
                let shape = self.json(j, env, Some(&resp))?;
                if let Err(e) = fits_shape(self.prog, &shape, &self.ep.resp, "") { return no(format!("the answer is {}, not Response<{}>: {}", shape, show_rtype(&self.ep.resp), e)); }
                self.line(&format!("return {};", code));
            }
            Stmt::Return(e) if self.ret.is_empty() && self.test => {
                // a test's result: `true` passes, `false` fails, a text fails with it as the message (empty: passes)
                let v = match e { Some(e) => self.expr(e, env, None)?, None => CV::Bool("1".into()) };
                match v {
                    CV::Bool(b) => { self.open(&format!("if (!({})) {{", b)); let t = self.txt_of(CV::Lit("returned false".into()))?; self.line(&format!("TEST_FAIL_({});", t)); self.close("}"); }
                    v @ (CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..)) => { let t = self.txt_of(v)?; self.line(&format!("if ({}.present && {}.len > 0) TEST_FAIL_({});", t, t, t)); }
                    _ => return no("a test returns `true` or `false`, or a text (empty when it passed, otherwise what went wrong)"),
                }
                self.line("return 0;");
            }
            Stmt::Return(e) => {
                if self.ret.is_empty() { if let Some(e) = e { self.expr(e, env, None)?; } self.line("return 0;"); return Ok(()); }
                let v = match e { Some(e) => self.expr(e, env, None)?, None => CV::Null };
                self.ret_with(v)?;
            }
            Stmt::Forward { kind, key, fields } => {
                self.call(&format!("c->h->fwd_begin(c, {}, {})", c_lit(kind.as_bytes()), kind.len()));
                let kv = self.expr(key, env, None)?;
                let k = self.int_or(kv, 500, "a forward key must be an integer");
                for (name, e) in fields {
                    let v = self.expr(e, env, None)?;
                    let vv = self.val(v)?;
                    self.call(&format!("c->h->fwd_put(c, {}, {}, &{})", c_lit(name.as_bytes()), name.len(), vv));
                }
                self.line(&format!("TAIL(c->h->forward(c, {}, {}, {}));", c_lit(kind.as_bytes()), kind.len(), k));
            }
            Stmt::Do(e) => { self.expr(e, env, None)?; }
            Stmt::Alloc { slot, coll, try_, .. } => {
                let CV::Coll(c, inst) = self.expr(coll, env, None)? else { return no("`add` is a function of a collection") };
                if let Some(t) = tab(c) {
                    // a new row (a table has no capacity, so `try_add` always gives one)
                    let r = self.tmp("i");
                    self.line(&format!("u32 {};", r));
                    self.call(&format!("c->h->tadd(c, {}, (u32)({}), &{})", t, inst, r));
                    env[*slot as usize] = Some(CV::Item(c, r));
                    return Ok(());
                }
                self.may_add_or_free(c, "add")?;
                let t = self.tmp("i");
                self.line(&format!("u32 {};", t));
                let coll = &self.prog.colls[c];
                if matches!(coll.kind, Kind::Pool(false)) && coll.mult == 1 && coll.children.is_empty() && coll.unlogged {
                    // the common pool (one instance, no generations, no nested collections) kept out of the undo log: adding at the high-water mark, with nothing freed below it, is done here
                    // with the layout as constants, as the runtime does it for any pool; anything else (a freed slot to reuse, a full pool) is left to the runtime's helper
                    let (hdr, live, cap) = (coll.hdr, coll.live, coll.cap);
                    let cols: Vec<(usize, usize)> = coll.cols.iter().filter(|x| !matches!(x.kind, CKind::StrBytes(_))).map(|x| (x.off, x.kind.size())).collect();
                    let (h0, h1, h2, h3, sl, wa, wd) = (self.tmp("h"), self.tmp("h"), self.tmp("h"), self.tmp("h"), self.tmp("s"), self.tmp("a"), self.tmp("w"));
                    self.open("{");
                    self.line(&format!("u32 {h0} = LD_U32({hdr}, 0), {h1} = LD_U32({}, 0), {h2} = LD_U32({}, 0), {h3} = LD_U32({}, 0);", hdr + 4, hdr + 8, hdr + 12));
                    self.line(&format!("if (UNLIKELY(!({h2} == {h0} && {h0} < {cap}u))) {{"));
                    self.depth += 1;
                    self.call(&format!("c->h->alloc(c, {}, {}, {}, &{})", c, inst, *try_ as i32, t));
                    self.depth -= 1;
                    self.line("} else {");
                    self.depth += 1;
                    self.line(&format!("u64 {sl} = {h0};"));
                    self.line(&format!("u64 {wa} = {live}ULL + 8 * ({sl} / 64);"));
                    self.line(&format!("u64 {wd} = {sl} % 64 ? (u64)LD_U64({wa}, 0) : 0; // a word wholly above the high-water mark may be uncommitted: its bits are zero"));
                    self.line(&format!("COMMIT_({wa}, 8);"));
                    self.line(&format!("ST8_({wa}, {wd} | (1ULL << ({sl} % 64)));"));
                    for (off, size) in &cols {
                        self.line(&format!("COMMIT_({off}ULL + {sl} * {size}ULL, {size});"));
                        self.line(&format!("if ({sl} < {h3}) ZERO_({off}ULL + {sl} * {size}ULL, {size}); // a reused slot is cleared; a never-used one is zero already"));
                    }
                    self.line(&format!("COMMIT_({hdr}ULL, 16);"));
                    self.line(&format!("ST4_({hdr}ULL, {h0} + 1); ST4_({}ULL, {h1} + 1); ST4_({}ULL, {h0} + 1); ST4_({}ULL, {h3} > {h0} + 1 ? {h3} : {h0} + 1);", hdr + 4, hdr + 8, hdr + 12));
                    self.line(&format!("{t} = (u32){sl};"));
                    self.depth -= 1;
                    self.line("}");
                    self.close("}");
                } else {
                    self.call(&format!("c->h->alloc(c, {}, {}, {}, &{})", c, inst, *try_ as i32, t));
                }
                env[*slot as usize] = Some(if *try_ {
                    // try_add gives an item or null (a full collection): an item that may be null, narrowed by `if exists(x)` or
                    // `if !exists(x) { fail ...; }` as any other
                    let r = self.tmp("r");
                    self.line(&format!("u32 {} = {} != 0xFFFFFFFFu;", r, t));
                    CV::Ref(c, r, t)
                } else { CV::Item(c, t) });
            }
        }
        Ok(())
    }

    fn ret_with(&mut self, v: CV) -> R<()> {
        // a struct goes straight into the destination the call was given
        if let Some((sid, off)) = self.ret.last().and_then(|r| r.dest) {
            let f = *self.inlining.last().unwrap();
            let CV::Plain(s2, src) = v else { return no(format!("`{}` returns a `{}`: return a struct of that type", self.api.fns[f as usize].name, self.prog.structs[sid].name)) };
            if s2 != sid { return no(format!("`{}` returns a `{}`, not a `{}`", self.api.fns[f as usize].name, self.prog.structs[sid].name, self.prog.structs[s2].name)); }
            self.copy_struct(sid, off, src)?;
            let (m, label) = { let r = self.ret.last().unwrap(); (r.mark.clone(), r.label.clone()) };
            self.line(&format!("/*RCUT {}*/", m));
            self.line(&format!("goto {};", label));
            return Ok(());
        }
        let b = self.txt_bound(&v);
        if let Some(r) = self.ret.last_mut() { match b { Some(b) => r.bound = Some(r.bound.unwrap_or(0).max(b)), None => r.unbounded = true } }
        let v = self.hold(v, false)?;
        // what the call can give, for the range analysis in the caller: the union of the ranges of every value returned
        let iv = match &v { CV::Int(e) => self.interval(e), _ => None };
        if let Some(r) = self.ret.last_mut() { r.ivs.push(iv); }
        let Some(kv) = kind(&v) else { return no("a function returning this kind of value is not compiled yet") };
        let r = self.ret.last().unwrap();
        let (var, label, f) = (r.var.clone(), r.label.clone(), *self.inlining.last().unwrap());
        let k = match r.kind { None => { self.ret.last_mut().unwrap().kind = Some(kv); kv } Some(k) => k };
        let declared = self.api.fns[f as usize].ret.clone();
        if let Some(t) = &declared {
            let what = format!("the result of `{}`", self.api.fns[f as usize].name);
            let fname = self.api.fns[f as usize].name.clone();
            let v2 = self.to_declared(v.clone(), t, &fname, &what)?;
            if let (CV::Flt(_), CV::Int(_)) = (&v2, &v) { return self.ret_with(v2); }
        } else if self.api.fns[f as usize].ptys.iter().any(|p| p.is_some()) && kv != K::Null {
            return no(format!("`{}` returns a value: declare its result, `fn {}(...) -> T`", self.api.fns[f as usize].name, self.api.fns[f as usize].name));
        }
        if !fits(kv, k) {
            if unify(k, kv) == K::Any { why(format!("`{}` returns {} and {}: a function returns one kind", self.api.fns[f as usize].name, kname(self.prog, k), kname(self.prog, kv))); }
            self.widen = Some(Widen::Ret(f, unify(k, kv))); return no("a function returns values of different kinds");
        }
        let v = self.coerce(v, k, false)?;
        if self.ret.last().unwrap().func {
            match &v {
                CV::Int(e) | CV::Flt(e) | CV::Bool(e) | CV::Txt(e) | CV::Any(e) | CV::Item(_, e) => self.line(&format!("*ret_v = {};", e)),
                CV::Coll(_, e) => self.line(&format!("*ret_v = (u32)({});", e)),
                CV::Ref(_, s, i) => self.line(&format!("*ret_s = {}; *ret_v = {};", s, i)),
                CV::Null | CV::Plain(..) => {}
                _ => return no("a function returning this kind of value is not compiled yet"),
            }
            if let Some(h) = self.ret.last().unwrap().sel.clone() { self.call(&format!("c->h->sel_end(c, {})", h)); }
            if !matches!(k, K::Txt | K::Any) { let m = self.ret.last().unwrap().mark.clone(); self.line(&format!("/*RCUT {}*/", m)); }
            self.line("return 0;");
            return Ok(());
        }
        let assign = match &v {
            CV::Int(e) | CV::Flt(e) | CV::Bool(e) | CV::Txt(e) | CV::Any(e) | CV::Item(_, e) => format!("{}_v = {};", var, e),
            CV::Coll(_, e) => format!("{}_v = (u32)({});", var, e),
            CV::Ref(_, s, k) => format!("{}_s = {}; {}_v = {};", var, s, var, k),
            CV::Null | CV::Plain(..) => String::new(),
            _ => return no("a function returning this kind of value is not compiled yet"),
        };
        if !assign.is_empty() { self.line(&assign); }
        if !matches!(k, K::Txt | K::Any) { let m = self.ret.last().unwrap().mark.clone(); self.line(&format!("/*RCUT {}*/", m)); }
        self.line(&format!("goto {};", label));
        Ok(())
    }

    fn slot_kind(&self, slot: u32) -> Option<K> { self.slot_kinds.iter().find(|x| x.0 == (self.frame, slot)).map(|x| x.1) }

    /// Convert a value to kind `k` (which it fits), in its own variable if `mutable`.
    fn coerce(&mut self, v: CV, k: K, mutable: bool) -> R<CV> {
        let held = self.coerce_value(v, k, mutable)?;
        if mutable {
            match &held {
                CV::Int(n) | CV::Flt(n) | CV::Bool(n) | CV::Txt(n) | CV::Any(n) | CV::Item(_, n) => { self.var_names.insert(n.clone()); }
                CV::Ref(_, s, i) => { self.var_names.insert(s.clone()); self.var_names.insert(i.clone()); }
                _ => {}
            }
        }
        Ok(held)
    }
    fn coerce_value(&mut self, v: CV, k: K, mutable: bool) -> R<CV> {
        Ok(match (k, v) {
            (K::Any, v) => { let x = self.val(v)?; if mutable || self.var_names.contains(&x) { let t = self.tmp("v"); self.line(&format!("Val {} = {};", t, x)); CV::Any(t) } else { CV::Any(x) } }
            (K::Txt, CV::Null) => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ 0, 0, 0 }};", t)); CV::Txt(t) }
            (K::Txt, v @ (CV::Txt(_) | CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..))) => { let x = self.txt_of(v)?; if mutable || self.var_names.contains(&x) { let t = self.tmp("t"); self.line(&format!("Txt {} = {};", t, x)); CV::Txt(t) } else { CV::Txt(x) } }
            (K::Ref(c), CV::Null) => { let (s, i) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = 0, {} = 0;", s, i)); CV::Ref(c, s, i) }
            (K::Ref(c), CV::Item(_, e)) => { let (s, i) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = 1, {} = {};", s, i, e)); CV::Ref(c, s, i) }
            (K::Ref(_), v @ CV::Ref(..)) => self.hold(v, mutable)?,
            (_, v) => self.hold(v, mutable)?,
        })
    }

    /// Give a value its own C variable (so it is computed once, and a `var` can be assigned later).
    fn hold(&mut self, v: CV, mutable: bool) -> R<CV> {
        // a `let` may share a C name with the value it is given, unless that name is a `var`'s (which a later assignment changes)
        let shared = |g: &Self, e: &str| !mutable && is_name(e) && !g.var_names.contains(e);
        Ok(match v {
            CV::Int(e) if !shared(self, &e) => {
                let t = self.tmp("v");
                self.line(&format!("i64 {} = {};", t, e));
                if mutable { self.var_names.insert(t.clone()); } else if let Some(r) = self.interval(&e) { self.ranges.insert(t.clone(), r); } // a `let` keeps its value's range
                CV::Int(t)
            }
            CV::Flt(e) if !shared(self, &e) => { let t = self.tmp("v"); self.line(&format!("double {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Flt(t) }
            CV::Bool(e) if !shared(self, &e) => { let t = self.tmp("v"); self.line(&format!("int {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Bool(t) }
            CV::Item(c, e) if !shared(self, &e) => { let t = self.tmp("i"); self.line(&format!("u32 {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Item(c, t) }
            CV::Txt(e) if mutable || self.var_names.contains(&e) => { let t = self.tmp("t"); self.line(&format!("Txt {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Txt(t) }
            CV::Any(e) if mutable || self.var_names.contains(&e) => { let t = self.tmp("v"); self.line(&format!("Val {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Any(t) }
            CV::Lit(s) if mutable => { let t = self.txt_of(CV::Lit(s))?; self.var_names.insert(t.clone()); CV::Txt(t) }
            // a `let` of a text field is a window onto it (no copy: each use reads the field as it is then); a `var` holds a copy
            CV::StateStr(c, idx, ci) if !mutable => { let idx = if self.var_names.contains(&idx) { let t = self.tmp("i"); self.line(&format!("u32 {} = {};", t, idx)); t } else { idx }; CV::StateStr(c, idx, ci) }
            CV::PlainStr(off) if !mutable => CV::PlainStr(off),
            v @ CV::Sub(..) if !mutable => v,
            CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) => { let t = self.txt_of(v)?; self.var_names.insert(t.clone()); CV::Txt(t) }
            // `var x = null`: its kind is the first value given it later (a row makes it `T?`, text a text that may be null); the endpoint is
            // generated again with that kind, never a value whose kind is known only at run time
            CV::Null if mutable => CV::Null,
            CV::Ref(c, s, k) if mutable || self.var_names.contains(&s) || self.var_names.contains(&k) => { let (s2, k2) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}, {} = {};", s2, s, k2, k)); if mutable { self.var_names.insert(s2.clone()); self.var_names.insert(k2.clone()); } CV::Ref(c, s2, k2) }
            o => o,
        })
    }
    /// An integer where the language requires one: a value of run-time kind is checked; anything else fails the request
    /// with the interpreter's error (`code`, `msg`).
    fn int_or(&mut self, v: CV, code: u16, msg: &str) -> String {
        match v {
            CV::Int(e) => self.hold_int(e),
            CV::Any(x) => { self.open(&format!("if ({}.tag != 1) {{", x)); self.fail(code, msg); self.close("}"); format!("{}.i", x) }
            _ => { self.fail(code, msg); "0LL".into() }
        }
    }
    fn hold_int(&mut self, e: String) -> String {
        if is_name(&e) { return e; }
        let t = self.tmp("v");
        self.line(&format!("i64 {} = {};", t, e));
        if let Some(r) = load_interval(&e).or_else(|| if e.starts_with("(i64)") { self.interval(&e) } else { None }) { self.ranges.insert(t.clone(), r); } // a fresh name, never assigned again: it keeps the load's range
        t
    }

    fn assign_var(&mut self, cur: &CV, v: CV) -> R<()> {
        match (cur, v) {
            (CV::Int(t), CV::Int(e)) | (CV::Bool(t), CV::Bool(e)) | (CV::Flt(t), CV::Flt(e)) => self.line(&format!("{} = {};", t, e)),
            (CV::Item(a, t), CV::Item(b, e)) if *a == b => self.line(&format!("{} = {};", t, e)),
            (CV::Txt(t), v @ (CV::Txt(_) | CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..))) => { let s = self.txt_of(v)?; self.line(&format!("{} = {};", t, s)); }
            (CV::Any(t), v) => { let s = self.val(v)?; self.line(&format!("{} = {};", t, s)); }
            (CV::Txt(t), CV::Null) => self.line(&format!("{} = (Txt){{ 0, 0, 0 }};", t)),
            (CV::Ref(a, s, k), CV::Ref(b, s2, k2)) if *a == b => self.line(&format!("{} = {}; {} = {};", s, s2, k, k2)),
            (CV::Ref(a, s, k), CV::Item(b, e)) if *a == b => self.line(&format!("{} = 1; {} = {};", s, k, e)),
            (CV::Ref(_, s, _), CV::Null) => self.line(&format!("{} = 0;", s)),
            _ => return no("a `var` assigned a value of another kind"),
        }
        Ok(())
    }

    /// Append pieces to a string field in place (`kind` 0 plain, 1 an item's column; a, b, cc as `str_app` takes them), emptying it first
    /// when `clear` (an assignment). A piece that is not text is formatted as `+` would.
    fn str_append(&mut self, kind: i32, a: String, b: String, cc: u32, clear: bool, pieces: &[&'p Expr], env: &mut Vec<Option<CV>>) -> R<()> {
        // no window among the pieces: all of them in one call
        let vals: Vec<CV> = pieces.iter().map(|p| self.expr(p, env, None)).collect::<R<Vec<CV>>>()?;
        if kind & 4 != 0 { for v in &vals { self.ascii_lit(v, "the field")?; } }
        if !vals.iter().any(|v| matches!(v, CV::Sub(..) | CV::PlainStr(_) | CV::StateStr(..))) {
            let mut names = Vec::new();
            for v in vals {
                if matches!(v, CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..)) { return no("only text, numbers and bools can be added to a string"); }
                names.push(self.val(v)?);
            }
            let argv = self.tmp("argv");
            self.line(&format!("Val {}[{}] = {{ {} }};", argv, names.len(), names.join(", ")));
            self.call(&format!("c->h->str_set(c, {}, {}, {}, {}, {}, {}, {})", kind, a, b, cc, clear as i32, argv, names.len()));
            return Ok(());
        }
        for (i, v) in vals.into_iter().enumerate() {
            if matches!(v, CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..)) { return no("only text, numbers and bools can be added to a string"); }
            let k = kind | if clear && i == 0 { 2 } else { 0 };
            // a text field or a window onto one: copied from where it is, field to field
            let window = match v.clone() { CV::Sub(base, from, count) => Some((*base, from, count)), b @ (CV::PlainStr(_) | CV::StateStr(..)) => { let l = self.field_len(&b); Some((b, "0".to_string(), l)) } _ => None };
            if let Some((base, from, count)) = window {
                let n = self.hold_int(count);
                let at = self.sub_bytes_at(&base, &from);
                self.call(&format!("c->h->str_app_at(c, {}, {}, {}, {}, {}, (u32)({}))", k, a, b, cc, at, n));
                continue;
            }
            let vv = self.val(v)?;
            self.call(&format!("c->h->str_app(c, {}, {}, {}, {}, &{})", k, a, b, cc, vv));
        }
        Ok(())
    }

    /// Write pieces to a capped var of text (C name `t`, `cap` bytes), after what it holds or from the start when `clear`: one call. A
    /// piece that is not text is formatted as `+` would.
    fn txt_write(&mut self, t: &str, cap: usize, clear: bool, pieces: &[&'p Expr], env: &mut Vec<Option<CV>>) -> R<()> {
        let mut names = Vec::new();
        for p in pieces {
            let v = self.expr(p, env, None)?;
            if matches!(v, CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..)) { return no("only text, numbers and bools can be added to text"); }
            names.push(self.val(v)?);
        }
        let argv = self.tmp("argv");
        self.line(&format!("Val {}[{}] = {{ {} }};", argv, names.len(), names.join(", ")));
        self.call(&format!("c->h->txt_set(c, &{}, {}, {}, {}, {})", t, cap, clear as i32, argv, names.len()));
        Ok(())
    }

    /// A literal put into `ascii` text: refused when compiling if it holds a byte above 127.
    fn ascii_lit(&self, v: &CV, f: &str) -> R<()> {
        if let CV::Lit(s) = v { if let Some(b) = s.bytes().find(|b| *b > 127) { return no(format!("`{}` is ascii text: the literal \"{}\" holds byte {}, which is not ASCII", f, s, b)); } }
        Ok(())
    }
    /// Is this text field `ascii<N>`?
    fn is_ascii(&self, base: &CV) -> bool {
        match base {
            CV::StateStr(c, _, ci) => self.prog.colls[*c].cols[*ci].range.is_some(),
            CV::PlainStr(off) => self.plain_ascii(*off),
            _ => false,
        }
    }
    /// Whether the plain text field at `off` is ascii (its struct's field range says so).
    fn plain_ascii(&self, off: usize) -> bool {
        fn walk(p: &Program, s: usize, base: usize, off: usize) -> Option<bool> {
            let sd = &p.structs[s];
            for (k, (_, t)) in sd.fields.iter().enumerate() {
                let o = base + sd.offs[k];
                match t { Ty::Str(_) if o == off => return Some(sd.ranges[k].is_some()), Ty::Struct(c) => if let Some(r) = walk(p, *c, o, off) { return Some(r); }, _ => {} }
            }
            None
        }
        self.prog.globals.iter().find_map(|g| match g.ty { Ty::Struct(s) => walk(self.prog, s, g.off, off), _ => None }).unwrap_or(false)
    }

    /// The byte offset in state of byte `from` of a text field (a window's base).
    fn sub_bytes_at(&self, base: &CV, from: &str) -> String {
        match base {
            CV::PlainStr(off) => format!("{}ULL + (u64)({})", off + 4, from),
            CV::StateStr(c, idx, ci) => { let b = &self.prog.colls[*c].cols[*ci + 1]; let CKind::StrBytes(m) = b.kind else { unreachable!() }; format!("{}ULL + (u64)({}) * {}ULL + (u64)({})", b.off, idx, m, from) }
            _ => unreachable!("a window's base is a text field"),
        }
    }
    /// A stored id (the C expression of its 8 bytes) read back as a `T?` of pool `target`.
    fn id_value(&mut self, target: usize, v: String) -> CV {
        if let Some(t) = tab(target) {
            // a row's id kept in working state: null when 0, or when the row has gone since (row ids are never reused)
            let (s, i, x) = (self.tmp("r"), self.tmp("i"), self.tmp("v"));
            self.line(&format!("i64 {} = {}; u32 {} = {} > 0 && {} <= 4294967295LL, {} = (u32){};", x, v, s, x, x, i, x));
            self.line(&format!("if ({}) {{ CALL(c->h->texists(c, {}, {}, &{})); }}", s, t, i, s));
            return CV::Ref(target, s, i);
        }
        let (s, i) = (self.tmp("r"), self.tmp("i"));
        self.line(&format!("u32 {} = 0, {} = 0;", s, i));
        self.call(&format!("c->h->id_get(c, {}, {}, &{}, &{})", target, v, s, i));
        CV::Ref(target, s, i)
    }
    /// A value stored into an id field of pool `target`: an item, a `T?` or null, as its id (0 for null).
    fn id_store_value(&mut self, target: usize, v: CV, what: &str) -> R<CV> {
        if tab(target).is_some() {
            // a row's id (0 for null)
            return Ok(match v {
                CV::Null => CV::Int("0LL".into()),
                CV::Item(c, e) if c == target => CV::Int(format!("(i64)({})", e)),
                CV::Ref(c, s, e) if c == target => CV::Int(format!("({} ? (i64)({}) : 0LL)", s, e)),
                _ => return no(format!("`{}` holds a row of {}: give it a row, a value that may be null, or `null`", what, coll_name(self.prog, target))),
            });
        }
        let wrong = |g: &Self, c: usize| no(format!("`{}` holds an id of {}, given an item of {}", what, g.prog.colls[target].name, g.prog.colls[c].name));
        Ok(match v {
            CV::Null => CV::Int("0LL".into()),
            CV::Item(c, e) => { if c != target { return wrong(self, c); } let t = self.tmp("v"); self.line(&format!("i64 {} = 0;", t)); self.call(&format!("c->h->id_of(c, {}, {}, &{})", target, e, t)); CV::Int(t) }
            CV::Ref(c, s, e) => {
                if c != target { return wrong(self, c); }
                let t = self.tmp("v");
                self.line(&format!("i64 {} = 0;", t));
                self.open(&format!("if ({} != 0) {{", s)); self.call(&format!("c->h->id_of(c, {}, {}, &{})", target, e, t)); self.close("}");
                CV::Int(t)
            }
            _ => return no(format!("`{}` holds an id: give it an item, a value that may be null, or `null`", what)),
        })
    }
    /// The byte offset in state of a text field's length (u32).
    fn len_at(&self, base: &CV) -> String {
        match base {
            CV::PlainStr(off) => format!("{}ULL", off),
            CV::StateStr(c, idx, ci) => format!("{}ULL + (u64)({}) * 4ULL", self.prog.colls[*c].cols[*ci].off, idx),
            _ => unreachable!("a text field"),
        }
    }
    /// A text field's length now (a C expression).
    fn field_len(&self, base: &CV) -> String {
        match base {
            CV::PlainStr(off) => format!("(i64)LD_U32({}, 0)", off),
            CV::StateStr(c, idx, ci) => format!("(i64)LD_U32({}, {})", self.prog.colls[*c].cols[*ci].off, idx),
            _ => unreachable!("a text field"),
        }
    }

    /// The value as a `Val` variable, for helpers that take any kind of value.
    fn val(&mut self, v: CV) -> R<String> {
        let t = self.tmp("v");
        match v {
            CV::Any(e) => return Ok(e),
            CV::Int(e) => self.line(&format!("Val {} = {{ 1, 0, 0, {}, 0 }};", t, e)),
            CV::Flt(e) => self.line(&format!("Val {} = {{ 2, 0, 0, 0, {} }};", t, e)),
            CV::Bool(e) => self.line(&format!("Val {} = {{ 3, 0, 0, {}, 0 }};", t, e)),
            CV::Null => self.line(&format!("Val {} = {{ 0, 0, 0, 0, 0 }};", t)),
            CV::Item(c, e) => self.line(&format!("Val {} = {{ 5, {}, {}, 0, 0 }};", t, c, e)),
            CV::Ref(c, s, k) => self.line(&format!("Val {} = {{ {} == 0 ? 0 : 5, {}, {}, 0, 0 }};", t, s, c, k)),
            v @ (CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..)) => { let x = self.txt_of(v)?; self.line(&format!("Val {} = {{ {}.present ? 4 : 0, {}.off, {}.len, 0, 0 }};", t, x, x, x)); }
            CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) => return no("a collection, array or struct used as a value is not compiled here yet (a struct can be copied into a local or a struct field with `let`, `var` or `=`; passing, returning, comparing and answering with struct values come in later steps of \"Struct values\" in DESIGN.md)"),
            CV::Resp(..) => return no("an answer is read through `.status` and `.body`"),
        }
        Ok(t)
    }

    fn txt_of(&mut self, v: CV) -> R<String> {
        match v {
            CV::Txt(t) => Ok(t),
            CV::Lit(s) => { let t = self.tmp("t"); let off = intern_lit(s.as_bytes()); self.line(&format!("Txt {} = {{ {}, {}, 1 }};", t, off, s.len())); Ok(t) }
            CV::PlainStr(off) => { let t = self.tmp("t"); self.line(&format!("Txt {};", t)); self.call(&format!("c->h->plain_txt(c, {}, &{})", off, t)); Ok(t) }
            CV::Sub(base, from, count) => {
                let at = self.sub_bytes_at(&base, &from);
                let t = self.tmp("t");
                self.line(&format!("Txt {};", t));
                self.call(&format!("c->h->arr_txt(c, {}, (u32)({}), &{})", at, count, t));
                Ok(t)
            }
            CV::StateStr(c, idx, ci) => { let t = self.tmp("t"); self.line(&format!("Txt {};", t)); self.call(&format!("c->h->col_txt(c, {}, {}, {}, &{})", c, idx, ci, t)); Ok(t) }
            _ => no("expected a string"),
        }
    }

    /// A condition as a C expression, with the interpreter's rules (only booleans and null are conditions).
    fn cond(&mut self, v: CV) -> R<String> {
        Ok(match v {
            CV::Bool(e) => e,
            CV::Null => "0".into(),
            CV::Any(e) => { let t = self.tmp("v"); self.line(&format!("int {};", t)); self.call(&format!("c->h->truthy(c, &{}, &{})", e, t)); t }
            CV::Ref(_, s, _) => { self.fail(500, "condition is not a boolean"); let _ = s; "0".into() }
            v => { let vv = self.val(v)?; let t = self.tmp("v"); self.line(&format!("int {};", t)); self.call(&format!("c->h->truthy(c, &{}, &{})", vv, t)); t }
        })
    }

    /// `dead`: where to jump if a reference turns out not to be live (inside `or(...)` and `exists(...)`); otherwise the
    /// request fails as the interpreter's would.
    fn expr(&mut self, e: &'p Expr, env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        match e {
            Expr::Int(n) => Ok(CV::Int(if *n == i64::MIN { "I64MIN".into() } else { format!("{}LL", n) })),
            Expr::Bool(b) => Ok(CV::Bool(if *b { "1" } else { "0" }.into())),
            Expr::Str(s) => Ok(CV::Lit(s.clone())),
            Expr::Null => Ok(CV::Null),
            Expr::Float(f) => Ok(CV::Flt(c_double(*f))),
            Expr::Var(n, VarRes::Local(s)) => match env.get(*s as usize).cloned().flatten() { Some(CV::Int(name)) if self.u32_vars.contains(&name) => Ok(CV::Int(format!("(i64){}", name))), v => v.ok_or_else(|| format!("`{}` has no compiled value", n)) },
            Expr::Var(n, VarRes::Global(g)) => {
                let gl = &self.prog.globals[*g as usize];
                if let Some(c) = gl.coll { return Ok(CV::Coll(c, "0".into())); }
                self.plain(gl.off, &gl.ty.clone(), dead).map_err(|_| format!("global `{}` of this kind not compiled yet", n))
            }
            Expr::Var(n, VarRes::Unbound) => no(format!("`{}` was not bound", n)),
            Expr::Field(base, f) => { let bv = self.expr(base, env, dead)?; self.field(bv, f, dead) }
            Expr::Index(base, k_expr) => {
                let bv = self.expr(base, env, dead)?;
                let kv = self.expr(k_expr, env, dead)?;
                let k = self.int_or(kv, 400, "an index must be an integer");
                match bv {
                    CV::Coll(c, inst) if tab(c).is_some() => {
                        // `table[id]`: the row with that id, when it is this table's (and this owner's); otherwise 404
                        let k = self.hold_int(k);
                        let r = self.tmp("i");
                        let msg = format!("no such row in {}", coll_name(self.prog, c));
                        self.line(&format!("if ({k} < 0 || {k} > 4294967295LL) FAIL(404, {}, {});", c_lit(msg.as_bytes()), msg.len()));
                        self.line(&format!("u32 {} = (u32){};", r, k));
                        self.call(&format!("c->h->tfind(c, {}, {}, (u32)({}))", c - TBASE, r, inst));
                        Ok(CV::Item(c, r))
                    }
                    CV::Coll(c, inst) => {
                        let t = self.index_of(c, &inst, &k);
                        self.item_value(c, t, dead)
                    }
                    CV::ArrCol(c, idx, ci) => {
                        let (off, CKind::Arr(sc, n)) = (self.prog.colls[c].cols[ci].off, self.prog.colls[c].cols[ci].kind.clone()) else { unreachable!() };
                        let k = self.hold_int(k);
                        self.elem_check(&k, n as u64);
                        self.load(&off.to_string(), &format!("(i64)({}) * {} + {}", idx, n, k), sc)
                    }
                    // one struct of an array of structs (`item.vals[i]`): its fields are read as `.a`
                    CV::Part(c, idx, p) => {
                        let inner = format!("{}.", p);
                        let Some(CKind::Arr(_, n)) = self.prog.colls[c].cols.iter().find(|x| x.path.starts_with(&inner) && matches!(x.kind, CKind::Arr(..))).map(|x| x.kind.clone()) else { return no("only fixed arrays and collections can be indexed") };
                        let k = self.hold_int(k);
                        self.elem_check(&k, n as u64);
                        Ok(CV::Elem(c, idx, p, k))
                    }
                    CV::PlainArr(off, Ty::Scalar(sc), n) => {
                        let k = self.hold_int(k);
                        if !self.fast_sites.contains(&(&**k_expr as *const Expr)) { self.elem_check(&k, n as u64); }
                        let v = self.load(&off.to_string(), &k, sc)?;
                        // every store in the program put a value inside this range there (or none did: the array holds its starting zeros)
                        let range = FIELD_RANGES.with(|fr| fr.borrow().0.get(&off).copied());
                        Ok(self.with_field_range(v, range))
                    }
                    // a string's bytes are read by index (0..len); strings are never written by index
                    v @ (CV::Txt(_) | CV::Lit(_)) => {
                        let t = self.txt_of(v)?;
                        let k = self.hold_int(k);
                        self.line(&format!("if (RANGE_({k} < 0 || (u64){k} >= (u64){t}.len)) FAIL(400, \"string index out of range\", 25);"));
                        Ok(CV::Int(format!("(i64)TX_({}.off + {})", t, k)))
                    }
                    CV::StateStr(c, idx, ci) => {
                        let k = self.hold_int(k);
                        let lenoff = self.prog.colls[c].cols[ci].off;
                        let bytes = &self.prog.colls[c].cols[ci + 1];
                        let CKind::StrBytes(m) = bytes.kind else { return no("string column layout") };
                        let boff = bytes.off;
                        self.line(&format!("if (RANGE_({k} < 0 || (u64){k} >= (u64)LD_U32({lenoff}, {idx}))) FAIL(400, \"string index out of range\", 25);"));
                        Ok(CV::Int(format!("(i64)LD_U8({}, (i64)({}) * {} + {})", boff, idx, m, k)))
                    }
                    CV::Sub(base, from, count) => {
                        let k = self.hold_int(k);
                        self.line(&format!("if (RANGE_({k} < 0 || {k} >= (i64)({count}))) FAIL(400, \"string index out of range\", 25);"));
                        let at = self.sub_bytes_at(&base, &format!("{} + {}", from, k));
                        Ok(CV::Int(format!("(i64)LD_U8({}, 0)", at)))
                    }
                    // a plain string: its length (u32) then its bytes, read where they are
                    CV::PlainStr(off) => {
                        let k = self.hold_int(k);
                        let len = self.str_lens.get(&off).cloned().unwrap_or_else(|| format!("(i64)LD_U32({}, 0)", off));
                        if !self.fast_sites.contains(&(&**k_expr as *const Expr)) { self.line(&format!("if (RANGE_({k} < 0 || {k} >= {len})) FAIL(400, \"string index out of range\", 25);")); }
                        Ok(CV::Int(format!("(i64)LD_U8({}, {})", off + 4, k)))
                    }
                    _ => { self.fail(500, "cannot index this value"); Ok(CV::Null) }
                }
            }
            Expr::Un(op, x) => {
                let v = self.expr(x, env, dead)?;
                if *op == '!' { let c = self.cond(v)?; return Ok(CV::Bool(format!("!({})", c))); }
                match v {
                    CV::Int(e) => { let e = self.hold_int(e); let r = self.tmp("v"); self.line(&format!("NEG({}, {});", r, e)); Ok(CV::Int(r)) }
                    CV::Flt(e) => Ok(CV::Flt(format!("(-{})", e))),
                    CV::Any(x) => {
                        // as the interpreter: an integer negates with an overflow check, a float as a float, nothing else
                        let (r, n) = (self.tmp("v"), self.tmp("v"));
                        self.line(&format!("Val {} = {};", r, x));
                        self.open(&format!("if ({}.tag == 1) {{", x)); self.line(&format!("NEG({}, {}.i);", n, x)); self.line(&format!("{}.i = {};", r, n)); self.close("}");
                        self.open(&format!("else if ({}.tag == 2) {{", x)); self.line(&format!("{}.f = -{}.f;", r, x)); self.close("}");
                        self.open("else {"); self.fail(500, "cannot negate"); self.close("}");
                        Ok(CV::Any(r))
                    }
                    _ => { self.fail(500, "cannot negate"); Ok(CV::Null) }
                }
            }
            Expr::Bin(op, a, b) => self.bin(op, a, b, env, dead),
            Expr::Call(n, args, res) => self.callx(n, args, *res, env, dead),
        }
    }

    /// The most bytes a text value can hold, when known: a literal's, a field's capacity, a number's longest form, a tracked
    /// text's (an input, a table's text column, a join of those).
    fn txt_bound(&self, v: &CV) -> Option<usize> {
        match v {
            CV::Lit(s) => Some(s.len()),
            CV::Txt(n) => self.txt_bounds.get(n).copied(),
            CV::StateStr(c, _, ci) => match self.prog.colls[*c].cols.get(ci + 1).map(|x| &x.kind) { Some(crate::layout::CKind::StrBytes(m)) => Some(*m), _ => None },
            CV::PlainStr(off) => self.plain_str_cap(*off),
            CV::Sub(b, ..) => self.txt_bound(b),
            CV::Int(_) => Some(20), CV::Flt(_) => Some(32), CV::Bool(_) => Some(5), CV::Null => Some(0),
            _ => None,
        }
    }
    /// The most bytes of text a builtin gives, from its arguments' bounds (None: not known, as for a header or the environment)
    fn builtin_bound(&self, bi: B, a: &[CV]) -> Option<usize> {
        let n = |k: usize| a.get(k).and_then(|v| self.txt_bound(v));
        let int = |k: usize| match a.get(k) { Some(CV::Int(e)) => e.strip_suffix("LL").unwrap_or(e).parse::<usize>().ok(), _ => None };
        match bi {
            B::Str => match a.first() { Some(CV::Int(_)) => Some(20), Some(CV::Flt(_)) => Some(32), Some(CV::Bool(_)) => Some(5), _ => n(0) },
            B::Base64 | B::Base64Url => n(0).map(|x| (x + 2) / 3 * 4),
            B::Unbase64 => n(0).map(|x| (x + 3) / 4 * 3),
            B::Hex => n(0).map(|x| 2 * x),
            B::Sha1 | B::HmacSha1 => Some(20),
            B::Sha256 | B::HmacSha256 => Some(32),
            B::Pbkdf2Sha256 => int(3),
            B::RandomBytes => int(0),
            B::XorBytes => n(0).zip(n(1)).map(|(x, y)| x.max(y)),
            B::GcmSeal => n(2).map(|x| x + 16),
            B::GcmOpen => n(2),
            B::BcryptHash => Some(60),
            B::FormEncode => n(0).map(|x| 3 * x),
            B::FormDecode | B::JsonUnquote | B::TrimStart | B::TrimEnd => n(0),
            // each `%` directive writes at most 32 bytes (a zone or weekday name, a year)
            B::FormatTime => n(1).map(|x| 32 * x),
            B::EscapeHtml => n(0).map(|x| 6 * x),
            B::JsonQuote => n(0).map(|x| 6 * x + 2),
            // a character's other case takes at most three times its bytes
            B::Lower | B::Upper => n(0).map(|x| 3 * x),
            B::Chr => Some(4),
            B::RandomText => int(0).zip(n(1)).map(|(k, _)| 4 * k),
            B::Replace => match (n(0), n(1), n(2)) { (Some(x), Some(_), Some(z)) => match a.get(1) { Some(CV::Lit(s)) if !s.is_empty() => Some(x + x / s.len() * z), _ => None }, _ => None },
            _ => None,
        }
    }
    /// The capacity of the plain text field at `off`, found through the state's structs.
    fn plain_str_cap(&self, off: usize) -> Option<usize> {
        fn walk(prog: &Program, ty: &Ty, at: usize, off: usize) -> Option<usize> {
            match ty {
                Ty::Str(m) if at == off => Some(*m as usize),
                Ty::Struct(s) => { let sd = &prog.structs[*s]; sd.fields.iter().zip(sd.offs.iter()).find_map(|(f, o)| walk(prog, &f.1, at + o, off)) }
                _ => None,
            }
        }
        self.prog.globals.iter().find_map(|g| if g.coll.is_none() { walk(self.prog, &g.ty, g.off, off) } else { None })
    }
    /// Field k of the plain struct sid at `off`: when that is the request struct, its range is one the endpoint must find zeroed.
    fn touch_request(&mut self, off: usize, sid: usize, k: usize) {
        let Some((ro, _)) = self.prog.request else { return };
        if off != ro { return; }
        let (fo, fty) = (self.prog.structs[sid].offs[k], self.prog.structs[sid].fields[k].1.clone());
        let n = self.prog.plain_size(&fty).0;
        let r = (ro + fo, ro + fo + n.max(1));
        if !self.rq_touched.contains(&r) { self.rq_touched.push(r); }
    }
    /// A plain (record-style) value at offset `off`.
    fn plain(&mut self, off: usize, t: &Ty, dead: Option<&str>) -> R<CV> {
        if let (Ty::Scalar(_), Some(target)) = (t, self.prog.id_at(crate::layout::IdAt::Plain(off))) { let v = format!("LD_I64({}, 0)", off); return Ok(self.id_value(target, v)); }
        Ok(match t {
            Ty::Struct(s) => CV::Plain(*s, off),
            Ty::Scalar(s) => self.load(&off.to_string(), "0", *s)?,
            Ty::Str(_) => CV::PlainStr(off),
            Ty::Arr(e, n) => CV::PlainArr(off, (**e).clone(), *n),
            _ => return no("a plain value of this kind"),
        })
    }
    /// A scalar of item `idx` in the column (or plain value) at `off`, through the profile's load macros.
    fn load(&mut self, off: &str, idx: &str, s: Scalar) -> R<CV> {
        use Scalar::*;
        let ld = |m: &str| format!("LD_{}({}, {})", m, off, idx);
        Ok(match s {
            I64 => CV::Int(ld("I64")), U64 => CV::Int(format!("(i64){}", ld("U64"))),
            I32 => CV::Int(format!("(i64){}", ld("I32"))), U32 => CV::Int(format!("(i64){}", ld("U32"))),
            I16 => CV::Int(format!("(i64){}", ld("I16"))), U16 => CV::Int(format!("(i64){}", ld("U16"))),
            I8 => CV::Int(format!("(i64){}", ld("I8"))), U8 => CV::Int(format!("(i64){}", ld("U8"))),
            Bool => CV::Bool(format!("({} != 0)", ld("U8"))),
            F32 => CV::Flt(format!("(double){}", ld("F32"))), F64 => CV::Flt(ld("F64")),
        })
    }

    fn field(&mut self, base: CV, f: &str, dead: Option<&str>) -> R<CV> {
        if let CV::Resp(st, body) = &base {
            return match f { "status" => Ok(CV::Int(st.clone())), "body" => Ok(CV::Txt(body.clone())), _ => no(format!("an answer has `.status` and `.body`, not `.{}`", f)) };
        }
        let (c, idx, path) = match base {
            CV::Item(c, idx) => (c, idx, f.to_string()),
            CV::Part(c, idx, p) => (c, idx, format!("{}.{}", p, f)),
            CV::Ref(t, s, k) => {
                // a value that may be null is read only inside `or(...)` / `exists(...)`, or after `if exists(x)` made it a plain item
                let Some(l) = dead else { return no(format!("`.{}` of a value that may be null: check it first with `if exists(x) {{ ... }}` (inside, x is the item), or give a fallback with `or(...)`", f)) };
                self.line(&format!("if ({} == 0) goto {};", s, l));
                (t, k, f.to_string())
            }
            CV::Plain(sid, off) => {
                let sd = &self.prog.structs[sid];
                let Some(k) = sd.fields.iter().position(|x| x.0 == f) else { return no(format!("no field `{}`{}", f, crate::lang::suggest(f, sd.fields.iter().map(|x| x.0.as_str())))) };
                self.touch_request(off, sid, k);
                let sd = &self.prog.structs[sid];
                let ty = sd.fields[k].1.clone();
                let range = sd.ranges[k];
                if let Ty::Table(..) = ty { let t = self.prog.plain_tables.iter().find(|x| x.0 == off && x.1 == sid && x.2 == f).map(|x| x.3).ok_or("plain table")?; return Ok(CV::Coll(TBASE + t, "0".into())); }
                if ty.is_coll() { return Ok(CV::Coll(self.prog.plain_coll(off, sid, f).ok_or("plain collection")?, "0".into())); }
                let v = self.plain(off + sd.offs[k], &ty, dead)?;
                let range = range.or_else(|| FIELD_RANGES.with(|fr| fr.borrow().0.get(&(off + sd.offs[k])).copied()));
                return Ok(self.with_field_range(v, range));
            }
            CV::Elem(c, idx, p, k) => {
                let path = format!("{}.{}", p, f);
                let Some(ci) = self.prog.colls[c].cols.iter().position(|x| x.path == path && matches!(x.kind, CKind::Arr(..))) else { return no(format!("no field `{}` in the elements of `{}`", f, p)) };
                let (off, CKind::Arr(sc, n)) = (self.prog.colls[c].cols[ci].off, self.prog.colls[c].cols[ci].kind.clone()) else { unreachable!() };
                return self.load(&off.to_string(), &format!("(i64)({}) * {} + {}", idx, n, k), sc);
            }
            CV::Null => { let Some(l) = dead else { return no(format!("`.{}` of `null`", f)) }; self.line(&format!("goto {};", l)); return Ok(CV::Null); }
            CV::Any(x) => {
                // an item whose collection is known only at run time: the interpreter's own field lookup
                if let Some(l) = dead { self.line(&format!("if ({}.tag == 0) goto {};", x, l)); }
                if dead.is_none() { return no(format!("`.{}` of a value whose kind is only known when the program runs, which may be null: check it first with `if exists(x) {{ ... }}` (inside, x is the item), or give a fallback with `or(...)`", f)); }
                let r = self.tmp("v");
                self.line(&format!("Val {};", r));
                self.call(&format!("c->h->field_val(c, &{}, {}, {}, &{})", x, c_lit(f.as_bytes()), f.len(), r));
                return Ok(CV::Any(r));
            }
            _ => { self.fail(500, &format!("`.{}` needs an item or a state struct", f)); return Ok(CV::Null); }
        };
        if let Some(t) = tab(c) { return self.table_field(t, idx, &path); }
        let coll = &self.prog.colls[c];
        if let Some(k) = coll.children.iter().find(|x| x.0 == path) { return Ok(CV::Coll(k.1, idx)); }
        if let Some(ci) = coll.cols.iter().position(|x| x.path == path && !matches!(x.kind, CKind::StrBytes(_))) { return self.col_value(c, idx, ci, dead); }
        let inner = format!("{}.", path);
        if coll.cols.iter().any(|x| x.path.starts_with(&inner)) || coll.children.iter().any(|x| x.0.starts_with(&inner)) { return Ok(CV::Part(c, idx, path)); }
        let names: Vec<&str> = coll.cols.iter().map(|x| x.path.as_str()).chain(coll.children.iter().map(|x| x.0.as_str())).collect();
        no(format!("no field `{}`{}", path, crate::lang::suggest(&path, names.into_iter())))
    }

    /// The value of column `ci` of item `idx`: scalars are loaded, strings are read where used, references followed.
    fn col_value(&mut self, c: usize, idx: String, ci: usize, dead: Option<&str>) -> R<CV> {
        let col = self.prog.colls[c].cols[ci].clone();
        if let Some(target) = self.prog.id_at(crate::layout::IdAt::Col(c, ci)) { let v = format!("LD_I64({}, {})", col.off, idx); return Ok(self.id_value(target, v)); }
        Ok(match col.kind {
            CKind::Scalar(s) => { let v = self.load(&col.off.to_string(), &idx, s)?; self.with_field_range(v, col.range) }
            CKind::StrLen => CV::StateStr(c, idx, ci),
            CKind::Arr(..) => CV::ArrCol(c, idx, ci),
            CKind::StrBytes(_) => unreachable!(),
        })
    }

    /// What an item of collection `c` is as a value: the item, or for a collection of plain values, its value.
    /// A field of a table's row (`idx` its row id): a column, read through the row cache (`tget`); a plain struct inside the row
    /// (its fields are columns `path.x`); or a nested table, the rows this row owns.
    fn table_field(&mut self, t: usize, idx: String, path: &str) -> R<CV> {
        use crate::layout::TKind;
        let tb = &self.prog.tables[t];
        // `r.id`: the row's id (its table's own column; fields may not be named `id`)
        if path == "id" { return Ok(CV::Int(format!("(i64)({})", idx))); }
        if let Some(k) = tb.children.iter().find(|x| x.0 == path) { return Ok(CV::Coll(TBASE + k.1, idx)); }
        if let Some(ci) = tb.cols.iter().position(|x| x.path == path) {
            let col = tb.cols[ci].clone();
            if let TKind::Blob(..) = col.kind { return no(format!("`{}` is a fixed array in a table's row: reading it is not compiled yet", path)); }
            let v = self.tmp("v");
            self.line(&format!("Val {};", v));
            self.call(&format!("c->h->tget(c, {}, {}, {}, &{})", t, idx, ci, v));
            return Ok(match col.kind {
                TKind::Int(Scalar::Bool) => CV::Bool(format!("({}.i != 0)", v)),
                TKind::Int(_) => { let x = CV::Int(format!("{}.i", v)); self.with_field_range(x, col.range) }
                TKind::Real(_) => CV::Flt(format!("{}.f", v)),
                // a time: the runtime reads the column's text as microseconds since 1970
                // a nullable time (`time?`): the value, or null
                TKind::Time if col.null && self.narrowed.iter().any(|n| n.0 == TBASE + t && n.1 == idx && n.2 == path) => CV::Int(format!("{}.i", v)),
                TKind::Time if col.null => CV::Any(v),
                TKind::Time => CV::Int(format!("{}.i", v)),
                // a nullable column (`utf8<N>?`): null when the column is NULL
                TKind::Text(n, _) if col.null => { let tt = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, {}.tag == 4 }};", tt, v, v, v)); self.txt_bounds.insert(tt.clone(), n as usize); CV::Txt(tt) }
                TKind::Text(n, _) => { let tt = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, 1 }};", tt, v, v)); self.txt_bounds.insert(tt.clone(), n as usize); CV::Txt(tt) }
                // a reference: null, or a row (SQLite makes it null when its row goes, so it names a live row)
                TKind::Ref(target) if self.narrowed.iter().any(|n| n.0 == TBASE + t && n.1 == idx && n.2 == path) => { let k = self.tmp("i"); self.line(&format!("u32 {} = (u32){}.i;", k, v)); CV::Item(TBASE + target, k) }
                TKind::Ref(target) => { let (s, k) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}.tag != 0, {} = (u32){}.i;", s, v, k, v)); CV::Ref(TBASE + target, s, k) }
                TKind::Blob(..) => unreachable!(),
            });
        }
        let prefix = format!("{}.", path);
        if tb.cols.iter().any(|x| x.path.starts_with(&prefix)) || tb.children.iter().any(|x| x.0.starts_with(&prefix)) { return Ok(CV::Part(TBASE + t, idx, path.to_string())); }
        no(format!("no field `{}` in a row of {}", path, tb.name))
    }
    /// Writing a field of a table's row: the value checked as a field of working state is (an integer's type and range, a text's most
    /// bytes), then one UPDATE.
    fn table_set(&mut self, t: usize, idx: &str, path: &str, rhs: &'p Expr, env: &mut Vec<Option<CV>>) -> R<()> {
        let (ci, vv) = self.table_value(t, path, rhs, env)?;
        self.call(&format!("c->h->tset(c, {}, {}, {}, &{})", t, idx, ci, vv));
        Ok(())
    }
    /// The value for a field of a table's row, checked as a field of working state is (an integer's type and range, a text's most bytes,
    /// a reference's table): the column and the C name of the value (a Val).
    fn table_value(&mut self, t: usize, path: &str, rhs: &'p Expr, env: &mut Vec<Option<CV>>) -> R<(usize, String)> {
        use crate::layout::TKind;
        let cols = self.prog.tables[t].cols.clone();
        let Some(ci) = cols.iter().position(|x| x.path == path) else {
            let prefix = format!("{}.", path);
            let known = cols.iter().any(|x| x.path.starts_with(&prefix)) || self.prog.tables[t].children.iter().any(|x| x.0 == path);
            return no(if known { format!("assigning `.{}` as a whole is not supported; assign its fields", path) } else { format!("no field `{}` in a row of {}", path, self.prog.tables[t].name) });
        };
        let col = cols[ci].clone();
        let v = self.expr(rhs, env, None)?;
        let v = match &col.kind {
            TKind::Int(Scalar::Bool) => { let b = self.cond(v)?; CV::Bool(b) }
            TKind::Int(sc) => {
                let v = match col.range { Some(r) => self.in_field_range(v, r, path)?, None => v };
                let x = self.int_or(v, 400, "an integer field takes an integer");
                let x = self.hold_int(x);
                let (lo, hi): (i64, i64) = match sc { Scalar::I8 => (-128, 127), Scalar::I16 => (-32768, 32767), Scalar::I32 => (i32::MIN as i64, i32::MAX as i64), Scalar::U8 => (0, 255), Scalar::U16 => (0, 65535), Scalar::U32 => (0, u32::MAX as i64), Scalar::U64 => (0, i64::MAX), _ => (i64::MIN, i64::MAX) };
                if (lo, hi) != (i64::MIN, i64::MAX) && !matches!(self.interval(&x), Some((a, b)) if a >= lo && b <= hi) {
                    let msg = format!("`{}` does not fit {:?}", path, sc).to_lowercase();
                    self.line(&format!("if (UNLIKELY({x} < {} || {x} > {})) FAIL(400, {}, {});", c_i64(lo), c_i64(hi), c_lit(msg.as_bytes()), msg.len()));
                }
                CV::Int(x)
            }
            TKind::Real(_) => match v { CV::Int(e) => CV::Flt(format!("(double)({})", e)), CV::Flt(e) => CV::Flt(e), _ => return no(format!("`{}` takes a number", path)) },
            // a time: microseconds since 1970, written as text by the runtime
            TKind::Time if col.null && matches!(v, CV::Null) => CV::Null,
            TKind::Time => { let x = self.int_or(v, 400, "a time field takes microseconds since 1970 (an integer)"); CV::Int(self.hold_int(x)) }
            TKind::Text(..) if col.null && matches!(v, CV::Null) => CV::Null,
            TKind::Text(max, _) => {
                let tt = self.txt_of(v).map_err(|_| format!("`{}` takes text", path))?;
                let msg = format!("`{}` is at most {} bytes", path, max);
                self.line(&format!("if (UNLIKELY({tt}.len > {max})) FAIL(413, {}, {});", c_lit(msg.as_bytes()), msg.len()));
                if !col.null { let m2 = format!("`{}` cannot be null", path); self.line(&format!("if (UNLIKELY(!{tt}.present)) FAIL(400, {}, {});", c_lit(m2.as_bytes()), m2.len())); }
                CV::Txt(tt)
            }
            TKind::Blob(..) => return no(format!("`{}` is a fixed array in a table's row: writing it is not compiled yet", path)),
            TKind::Ref(target) => match v {
                CV::Null => CV::Null,
                CV::Item(c, _) | CV::Ref(c, ..) if c == TBASE + *target => v,
                v => return no(format!("`{}` names a row of {}, given {}", path, self.prog.tables[*target].name, kind(&v).map_or("a value of another kind".to_string(), |k| kname(self.prog, k)))),
            },
        };
        let vv = self.val(v)?;
        Ok((ci, vv))
    }
    /// A loop over one owner's rows merged into its outer loop: the rows of owner `inst` from the merge cursor `mcur` (its query holds the
    /// `where` that is SQL; the rest is tested here), with `limit` counted here.
    fn merged_loop(&mut self, it: &'p Iter, t: usize, inst: String, mcur: String, env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        let mut in_c = None;
        if let Some(w) = &it.wher {
            let (mut c, mut v, mut f) = (Vec::new(), Vec::new(), Vec::new());
            if !self.sql_where(w, it.slot, t, 0, &mut c, &mut v, &mut f) { in_c = Some(w); }
        }
        self.loop_enter();
        self.open("{");
        if it.last { return no("`last n` is for a loop over a table (the last n rows of its order)"); }
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        let (hits, rid, more, k) = (self.tmp("hits"), self.tmp("i"), self.tmp("more"), self.tmp("k"));
        self.line(&format!("u32 {} = 0;", hits));
        self.open(&format!("for (i64 {k} = 0LL; {k} < 9223372036854775807LL; {k}++) {{"));
        self.line(&format!("ROUND_({});", k));
        self.line(&format!("u32 {}; int {};", rid, more));
        self.call(&format!("c->h->tmerge(c, {}, (u32)({}), &{}, &{})", mcur, inst, rid, more));
        self.line(&format!("if (!{}) break;", more));
        env[it.slot as usize] = Some(CV::Item(TBASE + t, rid.clone()));
        if let Some(w) = in_c { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) continue;", wc)); }
        if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); }
        self.line(&format!("{}++;", hits));
        self.round_start(it, env, &hits);
        body(self, env, &hits)?;
        self.loop_round_end();
        self.close("}");
        self.loop_exit();
        self.close("}");
        Ok(())
    }
    /// `where` as SQL, when it is comparisons of the loop's row's fields with values the loop does not change, joined by `&&`
    /// (`t.done == false && t.rank > n`, `!t.done`): its conditions (`?n` for each value, numbered from `first`), the values, and the
    /// columns it uses. False when it is anything else.
    /// A row field's column in SQL, quoted: its path, or for a reference `<path>_id`.
    fn csql(&self, t: usize, path: &str) -> String { crate::layout::sql_ident(&self.prog.tables[t].cols.iter().find(|c| c.path == path).map_or(path.to_string(), |c| c.sql.clone())) }
    fn sql_where(&self, e: &'p Expr, slot: u32, t: usize, first: usize, conds: &mut Vec<String>, vals: &mut Vec<&'p Expr>, cols: &mut Vec<String>) -> bool {
        fn uses(e: &Expr, slot: u32) -> bool {
            match e {
                Expr::Var(_, VarRes::Local(s)) => *s == slot,
                Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null | Expr::Var(..) => false,
                Expr::Field(b, _) | Expr::Un(_, b) => uses(b, slot),
                Expr::Index(a, b) | Expr::Bin(_, a, b) => uses(a, slot) || uses(b, slot),
                Expr::Call(_, args, _) => args.iter().any(|a| uses(a, slot)),
            }
        }
        let col = |e: &Expr| -> Option<String> {
            let Expr::Field(b, f) = e else { return None };
            let Expr::Var(_, VarRes::Local(s)) = &**b else { return None };
            // a column, or the row's id (the primary key)
            if *s != slot || (f != "id" && !self.prog.tables[t].cols.iter().any(|c| c.path == *f && !matches!(c.kind, crate::layout::TKind::Blob(..)))) { return None; }
            Some(f.clone())
        };
        match e {
            Expr::Bin(op, a, b) if op == "&&" => self.sql_where(a, slot, t, first, conds, vals, cols) && self.sql_where(b, slot, t, first, conds, vals, cols),
            Expr::Bin(op, a, b) if ["==", "!=", "<", "<=", ">", ">="].contains(&op.as_str()) => {
                let (f, v, op) = match (col(a), col(b)) {
                    (Some(f), None) if !uses(b, slot) => (f, &**b, op.as_str()),
                    (None, Some(f)) if !uses(a, slot) => (f, &**a, match op.as_str() { "<" => ">", "<=" => ">=", ">" => "<", ">=" => "<=", o => o }),
                    _ => return false,
                };
                vals.push(v);
                conds.push(format!("{} {} ?{}", self.csql(t, &f), if op == "==" { "=" } else { op }, first + vals.len()));
                cols.push(f);
                true
            }
            Expr::Un('!', x) => match col(x) { Some(f) => { conds.push(format!("{} = 0", self.csql(t, &f))); cols.push(f); true } None => false },
            e => match col(e) { Some(f) => { conds.push(format!("{} != 0", self.csql(t, &f))); cols.push(f); true } None => false },
        }
    }
    /// A loop's order in SQL: `f` (one of the row's fields: also returned), `lower(f)` (SQLite's LOWER: ASCII letters), `f.g` across a
    /// reference (the referenced row's field, through a subquery, NULL when the reference is), `lower(f.g)`.
    fn order_sql(&self, t: usize, by: &str) -> R<(String, Option<String>)> {
        let (lower, path) = match by.strip_prefix("lower(").and_then(|x| x.strip_suffix(')')) { Some(p) => (true, p), None => (false, by) };
        let tb = &self.prog.tables[t];
        let plain = |tbl: &crate::layout::Table, f: &str| tbl.cols.iter().any(|c| c.path == f && !matches!(c.kind, crate::layout::TKind::Blob(..)));
        let wrap = |c: String| if lower { format!("LOWER({})", c) } else { c };
        match path.split_once('.') {
            None => {
                if path != "id" && !plain(tb, path) { return no(format!("`by {}`: not a field of a row of {}", by, tb.name)); }
                Ok((wrap(self.csql(t, path)), if lower { None } else { Some(path.to_string()) }))
            }
            Some((f, g)) => {
                let Some(col) = tb.cols.iter().find(|c| c.path == f) else { return no(format!("`by {}`: `{}` is not a field of a row of {}", by, f, tb.name)) };
                let crate::layout::TKind::Ref(target) = col.kind else { return no(format!("`by {}`: `{}` is not a reference to another table's row", by, f)) };
                let tt = &self.prog.tables[target];
                if g.contains('.') || !plain(tt, g) { return no(format!("`by {}`: `{}` is not a field of a row of {}", by, g, tt.name)); }
                let inner = wrap(format!("o.{}", self.csql(target, g)));
                Ok((format!("(SELECT {} FROM {} AS o WHERE o.id = {}.{})", inner, tt.sql, tb.sql, self.csql(t, f)), None))
            }
        }
    }
    /// `foreach r in table [where ...] [reverse] [limit n]`: one query (written here, prepared once) whose cursor gives the rows' ids
    /// and columns; a field of the loop's row is read from the cursor. A `where` that is comparisons with values goes into the query;
    /// over a whole top-level table it must be, on indexed fields (a scan of every row is refused). Over one owner's rows anything goes:
    /// what is not SQL is tested here. A body that adds, removes or writes collects the ids first, so its changes cannot feed the loop.
    fn table_loop(&mut self, it: &'p Iter, t: usize, inst: String, env: &mut Vec<Option<CV>>, collect: bool, loop_body: Option<LoopBody<'p>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        // an inner loop merged into its outer loop's query: its rows come from the outer loop's merge cursor, owner by owner
        if let Some(k) = self.merged.iter().position(|m| std::ptr::eq(m.0, it)) {
            let mcur = self.merged.remove(k).1;
            return self.merged_loop(it, t, inst, mcur, env, body);
        }
        let tb = self.prog.tables[t].clone();
        let nested = tb.parent.is_some();
        let mut sql = format!("SELECT id{} FROM {}", tb.cols.iter().map(|c| format!(", {}", crate::layout::sql_ident(&c.sql))).collect::<String>(), tb.sql);
        let mut conds: Vec<String> = Vec::new();
        if nested { conds.push("parent_id = ?1".into()); }
        let (mut wconds, mut wvals, mut wcols) = (Vec::new(), Vec::new(), Vec::new());
        let mut in_c = None; // the `where` tested here, when it is not SQL
        if let Some(w) = &it.wher {
            if self.sql_where(w, it.slot, t, conds.len(), &mut wconds, &mut wvals, &mut wcols) {
                // over a whole table, an index must lead the search: one whose first column the `where` constrains (SQLite then uses it,
                // and the other conditions narrow what it finds)
                // (the id, the primary key, is one)
                let led = wcols.iter().any(|f| f == "id" || tb.index.iter().chain(tb.unique.iter()).any(|ix| ix.first() == Some(f)));
                if !nested && !it.scan && !led {
                    for f in wcols.iter().take(1) {
                        if !tb.index.iter().chain(tb.unique.iter()).any(|ix| ix.first() == Some(f)) {
                            return no(format!("`where` over the whole table {} on `{}`, which has no index: every row would be read; declare one (`{}: table<{}> index({})`), or accept the scan (`scan where ...`) when the schema cannot have one", tb.name, f, tb.name.rsplit('.').next().unwrap_or(""), self.prog.structs[tb.item].name, f));
                        }
                    }
                }
                conds.extend(wconds);
            } else if !nested {
                return no(format!("`where` over the whole table {} is compiled to SQL only when it compares the row's indexed fields with values (joined by `&&`); this one would read every row", tb.name));
            } else { in_c = Some(w); }
        }
        // `after x`: the rows after row x in the loop's order (by id, or by the `by` field then id; backwards with `reverse`); null: all
        let mut after_val = None;
        if let Some(a) = &it.after {
            let n = nested as usize + wvals.len() + 1; // after the owner's and the `where`'s values
            let op = if it.reverse { "<" } else { ">" };
            conds.push(match &it.by {
                Some(f) if f.contains('(') || f.contains('.') => return no(format!("`after` with `by {}`: only an order by one of the row's own fields is compiled", f)),
                Some(f) => format!("(?{n} IS NULL OR ({}, id) {op} ((SELECT {} FROM {} WHERE id = ?{n}), ?{n}))", self.csql(t, f), self.csql(t, f), tb.sql),
                None => format!("(?{n} IS NULL OR id {op} ?{n})"),
            });
            after_val = Some(a);
        }
        if !conds.is_empty() { sql += &format!(" WHERE {}", conds.join(" AND ")); }
        // a constant `limit` with every condition in the query: SQLite stops there too
        let sql_limit = match (&it.limit, in_c.is_none() && !collect) { (Some(Expr::Int(n)), true) if *n >= 0 => Some(*n), _ => None };
        // `last n`: the last n rows in the loop's order, walked in that order (one query: the order turned round, n taken, turned back)
        if it.last && (sql_limit.is_none() || in_c.is_some()) { return no("`last n` takes a constant count, with the whole `where` compiled to SQL (comparisons of the row's fields with values)"); }
        // order: insertion (row ids), or `by` a field: one an index starts with, or any field when an index leads the `where` (SQLite
        // sorts the rows it found, as Rails' `where(room: r).order(:created_at)` does); ties in insertion order
        let led = !nested && wcols.iter().any(|f| f == "id" || tb.index.iter().chain(tb.unique.iter()).any(|ix| ix.first() == Some(f)));
        let rev = it.reverse != it.last;
        let order = match &it.by {
            Some(f) => {
                let (c, own) = self.order_sql(t, f)?;
                match &own {
                    // a field of the row: walked by an index that starts with it, or sorted when an index leads the `where`
                    Some(f) => if !tb.index.iter().chain(tb.unique.iter()).any(|ix| ix.first() == Some(f)) && !led && !nested && !it.scan {
                        return no(format!("`by {}` walks an index, and {} has none on `{}`: declare one (`{}: table<{}> index({})`), narrow the loop with a `where` an index leads, or accept sorting every row (`scan`)", f, tb.name, f, tb.name.rsplit('.').next().unwrap_or(""), self.prog.structs[tb.item].name, f));
                    },
                    // an expression (`lower(f)`, a field across a reference): SQLite sorts the rows the `where` found
                    None => {
                        if !led && !nested && !it.scan { return no(format!("`by {}` sorts the rows found: narrow the loop with a `where` an index leads, or accept sorting every row (`scan`)", f)); }
                        if it.last { return no(format!("`last n` with `by {}`: only an order by one of the row's own fields is compiled", f)); }
                    }
                }
                (format!("{c}{d}, id{d}", d = if rev { " DESC" } else { "" }), format!("{c}{d}, id{d}", d = if it.reverse { " DESC" } else { "" }))
            }
            None => ((if rev { "id DESC" } else { "id" }).to_string(), (if it.reverse { "id DESC" } else { "id" }).to_string()),
        };
        sql += &format!(" ORDER BY {}", order.0);
        if let Some(n) = sql_limit { sql += &format!(" LIMIT {}", n); }
        if it.last { sql = format!("SELECT * FROM ({}) ORDER BY {}", sql, order.1); }
        self.loop_enter();
        self.open("{");
        // the query's values: the owner, then the `where`'s
        let mut names = Vec::new();
        if nested { names.push(format!("{{ 1, 0, 0, (i64)({}), 0 }}", inst)); }
        for e in wvals { let v = self.expr(e, env, None)?; let v = self.hold(v, false)?; names.push(self.val(v)?); }
        if let Some(a) = after_val {
            // a row of this table, a row that may be null, null, or a row id
            let v = self.expr(a, env, None)?;
            let ok = match &v { CV::Item(c, _) | CV::Ref(c, ..) => *c == TBASE + t, CV::Null | CV::Int(_) => true, _ => false };
            if !ok { return no(format!("`after` takes a row of {} (or its id, or null)", tb.name)); }
            let v = self.hold(v, false)?;
            names.push(self.val(v)?);
        }
        let (tp, cur, hits) = (self.tmp("argv"), self.tmp("cur"), self.tmp("hits"));
        if names.is_empty() { self.line(&format!("Val {}[1];", tp)); } else { self.line(&format!("Val {}[{}] = {{ {} }};", tp, names.len(), names.join(", "))); }
        self.line(&format!("u32 {};", cur));
        self.call(&format!("c->h->titer(c, {}, {}, {}, {}, &{})", c_lit(sql.as_bytes()), t, tp, names.len(), cur));
        // one loop over each row's children in the body (`foreach t in b.todos` inside `foreach b in ...`): one query for every row's
        // children, walked in step with this loop (a merge), instead of one query per row
        let mut mcur = None;
        if let (Some(lb), false, None, false) = (loop_body, collect, &it.by, it.reverse) {
            let mut kids = Vec::new();
            child_loops(lb, it.slot, &mut kids);
            let writes_any = match lb { LoopBody::Stmts(ss) => writes(self.api, ss, &mut Vec::new()) || allocates(self.api, ss, &mut Vec::new()), LoopBody::Json(_) => false };
            if let ([inner], false) = (kids.as_slice(), writes_any) {
                let Expr::Field(_, f) = &inner.coll else { unreachable!() };
                if let Some(&(_, ct)) = tb.children.iter().find(|x| x.0 == *f) {
                    let ctb = self.prog.tables[ct].clone();
                    let assigned = match lb { LoopBody::Stmts(ss) => assigned_slots(ss), LoopBody::Json(_) => Default::default() };
                    let (mut iconds, mut ivals, mut icols) = (Vec::new(), Vec::new(), Vec::new());
                    let inner_ok = match &inner.wher { None => true, Some(w) => self.sql_where(w, inner.slot, ct, names.len(), &mut iconds, &mut ivals, &mut icols) && ivals.iter().all(|e| bound_before(e, env, &assigned)) };
                    if inner_ok {
                        let sub_limit = sql_limit.map_or(String::new(), |n| format!(" ORDER BY id LIMIT {}", n));
                        let outer_where = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
                        let mut msql = format!("SELECT parent_id, id{} FROM {} WHERE parent_id IN (SELECT id FROM {}{}{})", ctb.cols.iter().map(|c| format!(", {}", crate::layout::sql_ident(&c.sql))).collect::<String>(), ctb.sql, tb.sql, outer_where, sub_limit);
                        for c in &iconds { msql += &format!(" AND {}", c); }
                        msql += " ORDER BY parent_id, id";
                        let mut mnames = names.clone();
                        for e in ivals { let v = self.expr(e, env, None)?; let v = self.hold(v, false)?; mnames.push(self.val(v)?); }
                        let (ma, mc) = (self.tmp("argv"), self.tmp("mcur"));
                        if mnames.is_empty() { self.line(&format!("Val {}[1];", ma)); } else { self.line(&format!("Val {}[{}] = {{ {} }};", ma, mnames.len(), mnames.join(", "))); }
                        self.line(&format!("u32 {};", mc));
                        self.call(&format!("c->h->titer(c, {}, {}, {}, {}, &{})", c_lit(msql.as_bytes()), ct, ma, mnames.len(), mc));
                        self.merged.push((*inner as *const Iter, mc.clone()));
                        mcur = Some(mc);
                    }
                }
            }
        }
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        self.line(&format!("u32 {} = 0;", hits));
        let h = self.tmp("h");
        if collect { self.line(&format!("u32 {};", h)); self.call(&format!("c->h->sel_mark(c, &{})", h)); }
        let (rid, more, k) = (self.tmp("i"), self.tmp("more"), self.tmp("k"));
        // rows as long as the cursor gives them, charged to the request's step budget as any loop without a fixed extent
        self.open(&format!("for (i64 {k} = 0LL; {k} < 9223372036854775807LL; {k}++) {{"));
        self.line(&format!("ROUND_({});", k));
        self.line(&format!("u32 {}; int {};", rid, more));
        self.call(&format!("c->h->tnext(c, {}, &{}, &{})", cur, rid, more));
        self.line(&format!("if (!{}) break;", more));
        env[it.slot as usize] = Some(CV::Item(TBASE + t, rid.clone()));
        if let Some(w) = in_c { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) continue;", wc)); }
        if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); }
        self.line(&format!("{}++;", hits));
        if !collect {
            self.round_start(it, env, &hits);
            body(self, env, &hits)?;
            self.loop_round_end();
            self.close("}");
            self.loop_exit();
            self.call(&format!("c->h->tclose(c, {})", cur));
            if let Some(mc) = &mcur { self.call(&format!("c->h->tclose(c, {})", mc)); self.merged.retain(|m| m.1 != *mc); }
            self.close("}");
            return Ok(());
        }
        self.call(&format!("c->h->sel_add(c, {})", rid));
        self.close("}");
        self.call(&format!("c->h->tclose(c, {})", cur));
        let (n, q, m2) = (self.tmp("n"), self.tmp("q"), self.tmp("i"));
        self.line(&format!("u32 {};", n));
        self.call(&format!("c->h->sel_limit(c, {}, -1, &{})", h, n));
        self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", q, q, n, q));
        self.line(&format!("u32 {};", m2));
        self.call(&format!("c->h->sel_get(c, {}, {}, &{})", h, q, m2));
        self.line(&format!("{} = {} + 1;", hits, q));
        env[it.slot as usize] = Some(CV::Item(TBASE + t, m2));
        self.round_start(it, env, &hits);
        body(self, env, &hits)?;
        self.loop_round_end();
        self.close("}");
        self.loop_exit();
        self.call(&format!("c->h->sel_end(c, {})", h));
        self.close("}");
        Ok(())
    }
    fn item_value(&mut self, c: usize, idx: String, dead: Option<&str>) -> R<CV> {
        let coll = &self.prog.colls[c];
        if let Some(k) = coll.children.iter().find(|x| x.0.is_empty()) { return Ok(CV::Coll(k.1, idx)); }
        match coll.cols.iter().position(|x| x.path.is_empty()) { Some(ci) => self.col_value(c, idx, ci, dead), None => Ok(CV::Item(c, idx)) }
    }

    /// Follow a reference stored at `off` + item `idx` * 8 (slot+1, then generation) to pool `t`.

    fn bin(&mut self, op: &str, a: &'p Expr, b: &'p Expr, env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        if op == "&&" || op == "||" {
            let l = self.expr(a, env, dead)?;
            let lc = self.cond(l)?;
            let t = self.tmp("v");
            self.line(&format!("int {} = {};", t, lc));
            self.open(&format!("if ({}{}) {{", if op == "&&" { "" } else { "!" }, t));
            let r = self.expr(b, env, dead)?;
            let rc = self.cond(r)?;
            self.line(&format!("{} = {};", t, rc));
            self.close("}");
            return Ok(CV::Bool(t));
        }
        let (l, r) = (self.expr(a, env, dead)?, self.expr(b, env, dead)?);
        if op == "==" || op == "!=" {
            let neg = if op == "!=" { "!" } else { "" };
            let inline = match (&l, &r) {
                (CV::Int(x), CV::Int(y)) | (CV::Bool(x), CV::Bool(y)) => Some(format!("({} == {})", x, y)),
                (CV::Flt(x), CV::Flt(y)) => Some(format!("({} == {})", x, y)),
                (CV::Flt(x), CV::Int(y)) | (CV::Int(y), CV::Flt(x)) => Some(format!("({} == (double)({}))", x, y)),
                (CV::Item(c1, x), CV::Item(c2, y)) => Some(if c1 == c2 { format!("({} == {})", x, y) } else { "0".into() }),
                (CV::Ref(c1, s, k), CV::Item(c2, y)) | (CV::Item(c2, y), CV::Ref(c1, s, k)) => Some(if c1 == c2 { format!("({} != 0 && {} == {})", s, k, y) } else { "0".into() }),
                (CV::Null, CV::Null) => Some("1".into()),
                (CV::Null, CV::Txt(t)) | (CV::Txt(t), CV::Null) => Some(format!("(!{}.present)", t)),
                (CV::Null, CV::Any(v)) | (CV::Any(v), CV::Null) => Some(format!("({}.tag == 0)", v)),
                (CV::Null, CV::Ref(_, s, _)) | (CV::Ref(_, s, _), CV::Null) => Some(format!("({} == 0)", s)),
                (CV::Null, CV::Int(_) | CV::Flt(_) | CV::Bool(_) | CV::Lit(_) | CV::Item(..) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..)) | (CV::Int(_) | CV::Flt(_) | CV::Bool(_) | CV::Lit(_) | CV::Item(..) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..), CV::Null) => Some("0".into()),
                _ => None,
            };
            if let Some(e) = inline { return Ok(CV::Bool(format!("{}{}", neg, e))); }
        }
        if let (CV::Int(x), CV::Int(y)) = (&l, &r) {
            let (x, y) = (self.hold_int(x.clone()), self.hold_int(y.clone()));
            let r = self.tmp("v");
            let mac = match op { "+" => "ADD", "-" => "SUB", "*" => "MUL", "/" => "DIV", "%" => "MOD", _ => "" };
            match op {
                "<" | "<=" | ">" | ">=" => return Ok(CV::Bool(format!("({} {} {})", x, op, y))),
                "&" | "|" | "^" | "<<" | ">>" | ">>>" => {
                    let (ix, iy) = (self.interval(&x), self.interval(&y));
                    let nonneg = |i: Option<(i64, i64)>| i.filter(|(l, _)| *l >= 0);
                    match op {
                        "&" => {
                            // with a non-negative operand the result is between 0 and that operand's largest value, whatever the other is
                            let hi = [nonneg(ix), nonneg(iy)].iter().flatten().map(|(_, h)| *h).min();
                            self.line(&format!("BAND{}({}, {}, {});", width_of(hi), r, x, y));
                            if let Some(h) = hi { self.ranges.insert(r.clone(), (0, h)); }
                        }
                        "|" | "^" => {
                            let hi = match (nonneg(ix), nonneg(iy)) { (Some((_, xh)), Some((_, yh))) => Some(xh.max(yh)), _ => None };
                            self.line(&format!("{}{}({}, {}, {});", if op == "|" { "BOR" } else { "BXOR" }, width_of(hi), r, x, y));
                            if let (Some((_, xh)), Some((_, yh))) = (nonneg(ix), nonneg(iy)) {
                                let m = xh.max(yh) as u64;
                                let top = if m == 0 { 0 } else { u64::MAX >> m.leading_zeros() };
                                if top <= i64::MAX as u64 { self.ranges.insert(r.clone(), (0, top as i64)); }
                            }
                        }
                        _ => {
                            // a count proven inside 0..63 needs no test
                            let counted = iy.map_or(false, |(l, h)| l >= 0 && h <= 63);
                            let left = op == "<<";
                            let mac = match op { "<<" => "SHL", ">>>" => "SHRL", _ => "SHR" };
                            self.line(&format!("{}{}({}, {}, {});", if counted { "N" } else { "" }, mac, r, x, y));
                            if let (Some((xl, xh)), Some((sl, sh))) = (ix, iy) {
                                if counted && op == ">>>" && xl < 0 {
                                    // a negative operand becomes a large positive one unless the count is 0
                                    if sl >= 1 { self.ranges.insert(r.clone(), (0, i64::MAX >> (sl - 1))); }
                                } else if counted {
                                    let mut lo = i128::MAX; let mut hi = i128::MIN;
                                    for &xv in &[xl as i128, xh as i128] { for &sv in &[sl, sh] {
                                        let v = if left { xv * (1i128 << sv) } else { xv >> sv };
                                        lo = lo.min(v); hi = hi.max(v);
                                    } }
                                    if lo >= i64::MIN as i128 && hi <= i64::MAX as i128 { self.ranges.insert(r.clone(), (lo as i64, hi as i64)); }
                                }
                            }
                        }
                    }
                    return Ok(CV::Int(r));
                }
                "+" | "-" | "*" | "/" | "%" => {
                    // with intervals for both operands, arithmetic that provably cannot overflow (or divide by zero) has no check; the
                    // result's interval is recorded for what is computed from it
                    let (ix, iy) = (self.interval(&x), self.interval(&y));
                    let mut proven: Option<(i128, i128)> = None;
                    if let (Some((xl, xh)), Some((yl, yh))) = (ix, iy) {
                        let (xl, xh, yl, yh) = (xl as i128, xh as i128, yl as i128, yh as i128);
                        proven = match op {
                            "+" => Some((xl + yl, xh + yh)),
                            "-" => Some((xl - yh, xh - yl)),
                            "*" => { let c = [xl * yl, xl * yh, xh * yl, xh * yh]; Some((*c.iter().min().unwrap(), *c.iter().max().unwrap())) }
                            "%" if yl > 0 => Some((if xl >= 0 { 0 } else { -(yh - 1) }, yh - 1)),
                            // a positive divisor: the quotient (truncated toward zero, as the C division is) is monotone in each operand, so the corners bound it
                            "/" if yl > 0 => { let c = [xl / yl, xl / yh, xh / yl, xh / yh]; Some((*c.iter().min().unwrap(), *c.iter().max().unwrap())) }
                            _ => None,
                        };
                    } else if let (None, Some((yl, yh))) = (ix, iy) {
                        // a positive divisor cannot be zero or -1 whatever the dividend is
                        if yl > 0 && op == "%" { proven = Some((-((yh - 1) as i128), (yh - 1) as i128)); }
                    }
                    let fits = proven.filter(|(l, h)| *l >= i64::MIN as i128 && *h <= i64::MAX as i128);
                    let safe_div = matches!(op, "/" | "%") && iy.map_or(false, |(yl, _)| yl > 0);
                    if let Some((l, h)) = fits.filter(|_| matches!(op, "+" | "-" | "*" | "%")) {
                        self.line(&format!("N{}({}, {}, {});", mac, r, x, y));
                        self.ranges.insert(r.clone(), (l as i64, h as i64));
                    } else if safe_div {
                        self.line(&format!("N{}({}, {}, {});", mac, r, x, y));
                        if let (Some((l, h)), "/") = (proven, op) { self.ranges.insert(r.clone(), (l as i64, h as i64)); }
                    } else {
                        self.line(&format!("{}({}, {}, {});", mac, r, x, y));
                        // a checked operation that passed: its result is inside what its operands allow, cut to the range of an i64
                        if let Some((l, h)) = proven.filter(|_| matches!(op, "+" | "-" | "*")) { self.ranges.insert(r.clone(), (l.max(i64::MIN as i128) as i64, h.min(i64::MAX as i128) as i64)); }
                    }
                    return Ok(CV::Int(r));
                }
                _ => {}
            }
        }
        let num = |v: &CV| match v { CV::Int(e) => Some(format!("(double)({})", e)), CV::Flt(e) => Some(e.clone()), _ => None };
        if let (Some(x), Some(y), true) = (num(&l), num(&r), matches!(l, CV::Flt(_)) || matches!(r, CV::Flt(_))) {
            // as the interpreter: a float on either side makes it float arithmetic (no overflow checks; IEEE results)
            match op {
                "+" | "-" | "*" | "/" => return Ok(CV::Flt(format!("({} {} {})", x, op, y))),
                "<" | "<=" | ">" | ">=" => return Ok(CV::Bool(format!("({} {} {})", x, op, y))),
                _ => {}
            }
        }
        // strings (and a value joined to one): the runtime's operator, with the kind of its result known here
        let kind = |v: &CV| match v { CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) => "string", CV::Int(_) => "i64", CV::Flt(_) => "f64", CV::Bool(_) => "bool", CV::Null => "null", CV::Any(_) => "?", _ => "an item or collection" };
        let (lk, rk) = (kind(&l), kind(&r));
        let joined = op == "+" && (lk == "string" || rk == "string") && [lk, rk].iter().all(|k| matches!(*k, "string" | "i64" | "f64" | "bool" | "null"));
        let compared = matches!(op, "<" | "<=" | ">" | ">=" | "==" | "!=") && lk == "string" && rk == "string";
        // text compared is text that lives somewhere: built on the spot it would be a temporary copy (tests may: they report)
        if compared && !self.test && [a, b].iter().any(|e| matches!(e, Expr::Bin(o, ..) if o == "+")) {
            return no("text compared is text that lives somewhere: build it in a `let` first (`let key = a + b;`), then compare that");
        }
        if !joined && !compared && lk != "?" && rk != "?" {
            // a null here comes from a copy of a function made for a null argument, in a branch that cannot run with it
            // (a walk with no `from`): it fails if reached, as it always has; any other mix does not load
            if lk == "null" || rk == "null" { self.fail(500, "operator does not fit these values"); return Ok(CV::Bool("0".into())); }
            return no(format!("operator `{}` does not fit {} and {}", op, lk, rk));
        }
        let code = OPS.iter().position(|o| *o == op).ok_or_else(|| format!("operator `{}`", op))?;
        if compared {
            // a string column against a literal or another column is compared where it is, without copying it into the text area
            let flip = [0, 1, 4, 5, 2, 3][code];
            // a window onto a plain field or a piece of a field: compared where it is, against a literal or another field
            let place = |g: &mut Self, v: &CV| -> Option<(i32, String, String)> { match v {
                CV::Lit(s) => Some((1, format!("{}ULL", intern_lit(s.as_bytes())), format!("{}", s.len()))),
                b @ (CV::PlainStr(_) | CV::StateStr(..)) => Some((0, g.sub_bytes_at(b, "0"), format!("(u32)({})", g.field_len(b)))),
                CV::Sub(b, from, n) => Some((0, g.sub_bytes_at(b, from), format!("(u32)({})", n))),
                _ => None,
            } };
            if matches!(l, CV::PlainStr(_) | CV::Sub(..)) || matches!(r, CV::PlainStr(_) | CV::Sub(..)) {
                if let (Some((ak, ao, an)), Some((bk, bo, bn))) = (place(self, &l), place(self, &r)) {
                    let t = self.tmp("r");
                    self.line(&format!("int {};", t));
                    self.call(&format!("c->h->str_cmp_at(c, {}, {}, {}, {}, {}, {}, {}, &{})", code, ak, ao, an, bk, bo, bn, t));
                    return Ok(CV::Bool(format!("({} != 0)", t)));
                }
            }
            let res = match (&l, &r) {
                (CV::StateStr(c, i, ci), CV::Lit(s)) => Some((format!("c->h->str_cmp_lit(c, {}, {}, {}, {}, {}, {}, &@)", code, c, i, ci, intern_lit(s.as_bytes()), s.len()), ())),
                (CV::Lit(s), CV::StateStr(c, i, ci)) => Some((format!("c->h->str_cmp_lit(c, {}, {}, {}, {}, {}, {}, &@)", flip, c, i, ci, intern_lit(s.as_bytes()), s.len()), ())),
                (CV::StateStr(c, i, ci), CV::StateStr(d, j, cj)) => Some((format!("c->h->str_cmp_col(c, {}, {}, {}, {}, {}, {}, {}, &@)", code, c, i, ci, d, j, cj), ())),
                _ => None,
            };
            if let Some((call, ())) = res {
                let t = self.tmp("r");
                self.line(&format!("int {};", t));
                self.call(&call.replace('@', &t));
                return Ok(CV::Bool(format!("({} != 0)", t)));
            }
        }
        let (lb, rb) = (self.txt_bound(&l), self.txt_bound(&r));
        let (lv, rv) = (self.val(l)?, self.val(r)?);
        let t = self.tmp("v");
        self.line(&format!("Val {};", t));
        self.call(&format!("c->h->binop(c, {}, &{}, &{}, &{})", code, lv, rv, t));
        if joined {
            let x = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, 1 }};", x, t, t));
            if let (Some(a), Some(b)) = (lb, rb) { self.txt_bounds.insert(x.clone(), a + b); }
            return Ok(CV::Txt(x));
        }
        if compared { return Ok(CV::Bool(format!("({}.i != 0)", t))); }
        Ok(CV::Any(t))
    }

    /// The type a function declared for a receiver is looked up by: a struct's name (an item of a pool, list or table, or a plain struct), or `i64`, `f64`, `bool`, `string`.
    fn receiver_type(&self, v: &CV) -> Option<String> {
        let item = |c: usize| if let Some(t) = tab(c) { Some(self.prog.structs[self.prog.tables[t].item].name.clone()) } else { self.prog.colls[c].item.map(|s| self.prog.structs[s].name.clone()) };
        match v {
            CV::Int(_) => Some("i64".into()), CV::Flt(_) => Some("f64".into()), CV::Bool(_) => Some("bool".into()),
            CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) => Some("string".into()),
            CV::Item(c, _) => item(*c),
            CV::Plain(sd, _) => Some(self.prog.structs[*sd].name.clone()),
            _ => None,
        }
    }
    fn callx(&mut self, n: &str, args: &'p [Expr], res: CallRes, env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        match res {
            CallRes::User(k) => self.inline(k, args, env, dead),
            // `x.f(args)` on a receiver whose function depends on its type: the receiver is evaluated, its type names the function (`Type.f`), and it is called with the receiver first
            CallRes::Unbound if n.starts_with('.') && !args.is_empty() => {
                // a name that is also a collection operation (`count`, `capacity`, `truncate`): the collection's own unless the receiver's type declares the function
                let own = match (&n[1..], args.len()) { ("count", 1) => Some(B::Count), ("capacity", 1) => Some(B::Capacity), ("truncate", 2) => Some(B::Truncate), _ => None };
                if let Some(b) = own { if !self.user_method_for(&n[1..], &args[0], env)? { return self.callx(n, args, CallRes::Builtin(b), env, dead); } }
                let mut vals = Vec::new();
                let ctx = self.ret_ctx.take();
                for a in args { let v = self.expr(a, env, dead)?; vals.push(self.hold(v, false)?); }
                self.ret_ctx = ctx;
                let Some(ty) = self.receiver_type(&vals[0]) else { return no(format!("`{}` is called on something that is not a struct, a number, a string or a bool, and only those have functions of their own", &n[1..])) };
                let want = format!("{}.{}", ty, &n[1..]);
                let Some(k) = self.api.fns.iter().position(|f| f.recv && f.name == want).or_else(|| self.api.fns.iter().position(|f| !f.recv && f.test.is_none() && f.name == n[1..] && f.ptys.first().map_or(false, |t| t.is_some()))) else {
                    if crate::generics::note_missing(&ty, &n[1..]) { return Ok(CV::Int("0LL".into())); } // a probe of `generics::expand`: go on to find the next missing copy
                    let has: Vec<String> = self.api.fns.iter().filter(|f| f.recv).filter_map(|f| f.name.rsplit_once('.').filter(|(_, m)| *m == &n[1..]).map(|(t, _)| t.to_string())).collect();
                    if has.is_empty() { return no(format!("`{}` has no function `{}` (declare `fn {}(self: {}, ...)`)", ty, &n[1..], &n[1..], ty)); }
                    return no(format!("`{}` has no function `{}` (declare `fn {}(self: {}, ...)`; `{}` is declared for {})", ty, &n[1..], &n[1..], ty, &n[1..], has.join(", ")));
                };
                crate::generics::check_call(&want, self.at.0)?;
                self.call_values(k as u32, vals, dead)
            }
            CallRes::Unbound if n == "split" => no("`split` is what a loop walks: `foreach part in split(text, \", \") { ... }` (or `split(text)` for words)"),
            CallRes::Unbound => no("unbound call"),
            CallRes::Builtin(bi) => {
                let lit = |a: Option<&Expr>| match a { Some(Expr::Str(s)) => Some(s.clone()), _ => None };
                let int_out = |g: &mut Self, call: String| { let r = g.tmp("v"); g.line(&format!("i64 {};", r)); g.call(&call.replace("@", &format!("&{}", r))); CV::Int(r) };
                let txt_out = |g: &mut Self, call: String| { let r = g.tmp("t"); g.line(&format!("Txt {};", r)); g.call(&call.replace("@", &format!("&{}", r))); CV::Txt(r) };
                let val_out = |g: &mut Self, call: String| { let r = g.tmp("v"); g.line(&format!("Val {};", r)); g.call(&call.replace("@", &format!("&{}", r))); CV::Any(r) };
                match bi {
                    B::Or => {
                        let (l, end) = (self.tmp("dead"), self.tmp("done"));
                        let res = self.tmp("v");
                        self.line(&format!("Val {};", res));
                        self.line("{");
                        let saved = self.or_dead.replace(l.clone());
                        let a = self.expr(&args[0], env, Some(&l));
                        self.or_dead = saved;
                        let a = a?;
                        let ka = kind(&a);
                        let av = self.val(a)?;
                        self.line(&format!("if ({}.tag == 0) goto {};", av, l)); // null also takes the fallback
                        self.line(&format!("{} = {}; goto {};", res, av, end));
                        self.line("}");
                        self.line(&format!("{}: {{", l));
                        let fb = self.expr(&args[1], env, dead)?;
                        let kb = kind(&fb);
                        let fv = self.val(fb)?;
                        self.line(&format!("{} = {};", res, fv));
                        self.line("}");
                        self.line(&format!("{}: ;", end));
                        Ok(match (ka, kb) {
                            (Some(K::Item(c) | K::Ref(c)), Some(K::Item(d))) if c == d => CV::Item(c, format!("{}.b", res)),
                            (Some(K::Item(c) | K::Ref(c)), Some(K::Ref(d))) if c == d => { let (sv, kv) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}.tag != 0, {} = {}.b;", sv, res, kv, res)); CV::Ref(c, sv, kv) }
                            // text or a fallback text, a number or a fallback number: that kind (the result is never null)
                            (Some(K::Txt), Some(K::Txt)) => { let tt = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, 1 }};", tt, res, res)); CV::Txt(tt) }
                            (Some(K::Int), Some(K::Int)) => CV::Int(format!("{}.i", res)),
                            (Some(K::Flt), Some(K::Flt | K::Int)) => CV::Flt(format!("({}.tag == 2 ? {}.f : (double){}.i)", res, res, res)),
                            (Some(K::Bool), Some(K::Bool)) => CV::Bool(format!("({}.i != 0)", res)),
                            (Some(K::Item(c) | K::Ref(c)), Some(K::Null)) => { let (sv, kv) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}.tag != 0, {} = {}.b;", sv, res, kv, res)); CV::Ref(c, sv, kv) }
                            _ => CV::Any(res),
                        })
                    }
                    B::Exists => {
                        let (l, end, res) = (self.tmp("dead"), self.tmp("done"), self.tmp("v"));
                        self.line(&format!("int {} = 1;", res));
                        self.line("{");
                        let saved = self.or_dead.replace(l.clone());
                        let a = self.expr(&args[0], env, Some(&l));
                        self.or_dead = saved;
                        let av = self.val(a?)?;
                        self.line(&format!("if ({}.tag == 0) goto {};", av, l));
                        self.line(&format!("goto {};", end));
                        self.line("}");
                        self.line(&format!("{}: {} = 0;", l, res));
                        self.line(&format!("{}: ;", end));
                        Ok(CV::Bool(res))
                    }
                    B::MaxLen => {
                        let Some(Expr::Field(b, f)) = args.first() else { return no("maxlen(item.field)") };
                        match self.expr(b, env, dead)? {
                            CV::Item(c, _) | CV::Ref(c, _, _) => match self.prog.colls[c].cols.iter().position(|x| x.path == **f).and_then(|ci| self.prog.colls[c].cols.get(ci + 1)).map(|x| x.kind.clone()) {
                                Some(CKind::StrBytes(m)) => Ok(CV::Int(format!("{}LL", m))), _ => no("maxlen needs a string field") },
                            CV::Plain(sid, _) => { let sd = &self.prog.structs[sid]; match sd.fields.iter().find(|x| x.0 == **f).map(|x| &x.1) { Some(Ty::Str(m)) => Ok(CV::Int(format!("{}LL", m))), _ => no("maxlen needs a string field") } }
                            _ => no("maxlen(item.field)"),
                        }
                    }
                    _ => {
                        let mut a = Vec::new();
                        for x in args { a.push(self.expr(x, env, dead)?); }
                        self.builtin(bi, n, args, a, lit, int_out, txt_out, val_out)
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn builtin(&mut self, bi: B, n: &str, _args: &'p [Expr], a: Vec<CV>, lit: impl Fn(Option<&Expr>) -> Option<String>,
        int_out: impl Fn(&mut Self, String) -> CV, txt_out: impl Fn(&mut Self, String) -> CV, val_out: impl Fn(&mut Self, String) -> CV) -> R<CV> {
        let _ = (&lit, &txt_out, &val_out);
        if bi == B::Log {
            // `log(...)`: for debugging, to standard error; not part of the answer
            let mut names = Vec::new();
            for v in a {
                let v = match v { w @ (CV::PlainStr(_) | CV::StateStr(..) | CV::Sub(..)) => CV::Txt(self.txt_of(w)?), o => o };
                if matches!(v, CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..)) { return no("`log` takes text, numbers, bools and items"); }
                names.push(self.val(v)?);
            }
            let argv = self.tmp("argv");
            if names.is_empty() { self.line(&format!("Val {}[1];", argv)); } else { self.line(&format!("Val {}[{}] = {{ {} }};", argv, names.len(), names.join(", "))); }
            self.call(&format!("c->h->log(c, AT_, {}, {})", argv, names.len()));
            return Ok(CV::Null);
        }
        if matches!(bi, B::HtmlLit | B::HtmlEsc | B::HtmlEscAttr | B::HtmlAttr | B::HtmlRaw) {
            // markup written into the answer (see lang.rs `html`): only in an endpoint that answers HTML, and the `-> html` functions it calls
            if !self.html_answer() { return no("markup is written only into an HTML answer: an endpoint returning `Response<html>`, or a `-> html` function it calls"); }
            let mut a = a.into_iter();
            match bi {
                B::HtmlLit => { let Some(CV::Lit(s)) = a.next() else { return no("bad markup") }; self.out(&s); }
                // `html_raw(t)`: text the program trusts as markup, written unescaped (a stylesheet an administrator wrote, as Rails' html_safe)
                B::HtmlRaw => match a.next() {
                    Some(CV::Lit(s)) => self.out(&s),
                    Some(CV::Txt(t)) => self.line(&format!("if ({}.present) CALL(c->h->raw_txt(c, &{}));", t, t)),
                    Some(CV::StateStr(cc, idx, ci)) => self.call(&format!("c->h->raw_col(c, {}, {}, {})", cc, idx, ci)),
                    Some(CV::PlainStr(off)) => { let at = self.sub_bytes_at(&CV::PlainStr(off), "0"); let n = self.field_len(&CV::PlainStr(off)); self.call(&format!("c->h->raw_at(c, {}, (u32)({}))", at, n)); }
                    Some(CV::Null) => {}
                    _ => return no("`html_raw` takes text: `{html_raw(styles)}`"),
                },
                B::HtmlEsc | B::HtmlEscAttr => { let v = a.next().unwrap_or(CV::Null); self.html_esc(v, bi == B::HtmlEscAttr)?; }
                _ => {
                    let (Some(CV::Lit(name)), Some(v)) = (a.next(), a.next()) else { return no("bad markup") };
                    match v {
                        CV::Bool(b) if b == "1" => self.out(&format!(" {}", name)),
                        CV::Bool(b) if b == "0" => {}
                        CV::Bool(b) => { let o = format!(" {}", name); self.line(&format!("if ({}) CALL(c->h->out(c, {}, {}));", b, c_lit(o.as_bytes()), o.len())); }
                        v => { self.out(&format!(" {}=\"", name)); self.html_esc(v, true)?; self.out("\""); }
                    }
                }
            }
            return Ok(CV::Null);
        }
        if bi == B::SetCookie {
            // `set_cookie("name", value, max_age_seconds)`: a Set-Cookie line on the answer (Path=/, HttpOnly, SameSite=Lax); 0 deletes it
            // an optional fourth argument: whether the cookie is HttpOnly (true unless given)
            let mut a = a;
            let http_only = if a.len() == 4 { let h = a.pop().unwrap(); match h { CV::Bool(b) => b, _ => return no("`set_cookie`'s fourth argument says whether the cookie is HttpOnly: true or false") } } else { "1".to_string() };
            let [name, value, age] = <[CV; 3]>::try_from(a).map_err(|_| "`set_cookie` takes the name, the value, how long it lasts in seconds and (optionally) whether it is HttpOnly: `set_cookie(\"list\", token(b), 31536000)`".to_string())?;
            let CV::Lit(name) = name else { return no("`set_cookie`: the name is a literal, as in `set_cookie(\"list\", ...)`") };
            if !crate::lang::cookie_name_ok(&name) { return no(format!("`set_cookie`: `{}` is not a cookie name (letters, digits and !#$%&'*+-.^_`|~)", name)); }
            let t = self.txt_of(value).map_err(|_| "`set_cookie` takes the value as text".to_string())?;
            let age = self.int_or(age, 500, "`set_cookie` takes the number of seconds as an integer");
            self.call(&format!("c->h->set_cookie(c, {}, {}, &{}, {}, {})", c_lit(name.as_bytes()), name.len(), t, age, http_only));
            return Ok(CV::Null);
        }
        if bi == B::SetHeader {
            // `set_header("Link", value)`: a header line on the answer, in place of the `web` block's line of that name
            let [name, value] = <[CV; 2]>::try_from(a).map_err(|_| "`set_header` takes the name and the value: `set_header(\"Link\", links)`".to_string())?;
            let CV::Lit(name) = name else { return no("`set_header`: the name is a literal, as in `set_header(\"Link\", ...)`") };
            if let Err(e) = crate::web::header_name_ok(&name) { return no(format!("`set_header`: {}", e)); }
            let t = self.txt_of(value).map_err(|_| "`set_header` takes the value as text".to_string())?;
            self.call(&format!("c->h->set_header(c, {}, {}, &{})", c_lit(name.as_bytes()), name.len(), t));
            return Ok(CV::Null);
        }
        if bi == B::Truncate {
            // `s.truncate(n)`: keep the first n bytes (0 <= n <= length); in utf8 text, n must not split a character
            let [s, n] = <[CV; 2]>::try_from(a).map_err(|_| "`truncate` takes the number of bytes to keep: `s.truncate(n)`".to_string())?;
            if !matches!(s, CV::PlainStr(_) | CV::StateStr(..)) { return no("`truncate` is a function of a text field"); }
            match &s { CV::PlainStr(off) => self.may_write_plain(*off)?, CV::StateStr(c, _, ci) => { let p = self.prog.colls[*c].cols[*ci].path.clone(); self.may_write_col(*c, &p)? } _ => {} }
            let n = self.int_or(n, 400, "`truncate` takes an integer");
            let n = self.hold_int(n);
            let len = self.field_len(&s);
            let len = self.hold_int(len);
            self.line(&format!("if (UNLIKELY({n} < 0 || {n} > {len})) FAIL(400, \"truncate past the end\", 21);"));
            if !self.is_ascii(&s) {
                let at = self.sub_bytes_at(&s, &n);
                self.line(&format!("if (UNLIKELY({n} < {len} && (LD_U8({at}, 0) & 0xC0) == 0x80)) FAIL(400, \"truncate would split a character\", 32);"));
            }
            let at = self.len_at(&s);
            self.call(&format!("c->h->wr_int(c, {}, 4, {})", at, n));
            return Ok(CV::Null);
        }
        if bi == B::JsonField {
            // `json_field(text, "name")` (`field` in a test): the field of a JSON object (a string's characters as written, escapes kept; anything else
            // as written), or null
            let [j, k] = <[CV; 2]>::try_from(a).map_err(|_| "`field` takes a JSON text and a field name: `field(r.body, \"id\")`".to_string())?;
            let CV::Lit(key) = k else { return no("`field`'s name is a string written in the test") };
            let jb = self.txt_bound(&j);
            let j = self.txt_of(j)?;
            let r = self.tmp("t");
            self.line(&format!("Txt {} = {{ 0, 0, 0 }};", r));
            self.call(&format!("c->h->json_field(c, &{}, {}, {}, &{})", j, c_lit(key.as_bytes()), key.len(), r));
            if let Some(b) = jb { self.txt_bounds.insert(r.clone(), b); }
            return Ok(CV::Txt(r));
        }
        if let B::ReqGet | B::ReqPost | B::ReqPut | B::ReqPatch | B::ReqDelete = bi {
            // a test's request: path, the number of body fields, then (name, value) pairs for the body and the headers
            let method = match bi { B::ReqGet => 0, B::ReqPost => 1, B::ReqPut => 2, B::ReqPatch => 3, _ => 4 };
            let nb = match _args.get(1) { Some(Expr::Int(n)) => *n as usize, _ => 0 };
            let mut it = a.into_iter();
            let path = it.next().ok_or("a request needs its path")?;
            let path = self.txt_of(path).map_err(|_| "a request's path is a string, as in `post(\"/players\", { name: \"ann\" })`".to_string())?;
            let _ = it.next();
            let rest: Vec<CV> = it.collect();
            for (i, pair) in rest.chunks(2).enumerate() {
                let CV::Lit(key) = &pair[0] else { return no("a request's field names are names") };
                let vv = self.val(pair[1].clone())?;
                self.call(&format!("c->h->rq_put(c, {}, {}, {}, &{})", (i >= nb) as i32, c_lit(key.as_bytes()), key.len(), vv));
            }
            let (st, bd) = (self.tmp("st"), self.tmp("rb"));
            self.line(&format!("i64 {} = 0; Txt {} = {{ 0, 0, 0 }};", st, bd));
            self.call(&format!("c->h->rq_send(c, {}, &{}, &{}, &{})", method, path, st, bd));
            self.checked.clear(); // the request may have changed anything a test before it proved
            return Ok(CV::Resp(st, bd));
        }
        // `len` of a text field: its stored length, read where it is (no copy)
        if bi == B::Len {
            match a.first() {
                Some(CV::StateStr(c, idx, ci)) => return Ok(CV::Int(format!("(i64)LD_U32({}, {})", self.prog.colls[*c].cols[*ci].off, idx))),
                Some(CV::PlainStr(off)) => return Ok(CV::Int(format!("(i64)LD_U32({}, 0)", off))),
                Some(CV::Sub(_, _, n)) => return Ok(CV::Int(format!("(i64)({})", n))),
                Some(CV::Lit(s)) => return Ok(CV::Int(format!("{}LL", s.len()))),
                Some(CV::Txt(t)) => return Ok(CV::Int(format!("(i64){}.len", t))),
                _ => {}
            }
        }
        // `int` of a number: an integer as it is; a float truncated in line, as the runtime's builtin does (NaN is 0, beyond i64 the nearest end)
        if bi == B::Int {
            match a.first() {
                Some(CV::Int(e)) => return Ok(CV::Int(e.clone())),
                Some(CV::Flt(e)) => {
                    let (f, r) = (self.tmp("f"), self.tmp("v"));
                    self.line(&format!("double {f} = {e}; i64 {r} = {f} != {f} ? 0 : {f} >= 9223372036854775807.0 ? I64MAX : {f} <= -9223372036854775808.0 ? I64MIN : (i64){f};"));
                    return Ok(CV::Int(r));
                }
                _ => {}
            }
        }
        let generic = match bi {
            B::Min | B::Max => !matches!(a.as_slice(), [CV::Int(_) | CV::Flt(_), CV::Int(_) | CV::Flt(_)]),
            B::Slot => !matches!(a.first(), Some(CV::Item(..) | CV::Ref(..))),
            _ => BUILTINS[..14].contains(&bi) || BUILTINS[17..].contains(&bi),
        };
        // arity of the byte and crypto built-ins, checked when compiling
        let want = match bi {
            B::Base64 | B::Base64Url | B::Unbase64 | B::Hex | B::Sha1 | B::Sha256 | B::RandomBytes | B::FormEncode | B::FormDecode | B::Env | B::EscapeHtml | B::JsonQuote | B::JsonUnquote | B::Crc32 | B::ReadStored | B::Lower | B::Upper | B::CacheGet | B::CachePut | B::CachePart | B::CacheLabel | B::TrimStart | B::TrimEnd | B::AllEmoji | B::HtmlParse | B::NodeKind | B::NodeName | B::NodeParent | B::NodeFirst | B::NodeLast | B::NodeNext | B::NodePrev | B::NodeContent | B::NodeAttrCount | B::NodeNewElement | B::NodeNewText | B::NodeDetach | B::NodeClone | B::NodeHtml | B::NodeInnerHtml | B::NodeHtmlEscaped | B::Chr | B::AllWord => Some((1, "one argument")),
            B::HtmlParseIn | B::NodeAttr | B::NodeAttrName | B::NodeAttrValue | B::NodeRemoveAttr | B::NodeAppend | B::NodeInsertBefore | B::NodeSetText => Some((2, "two arguments")),
            B::NodeSetAttr => Some((3, "a node, a name and a value")),
            B::RequestTarget | B::CacheStart | B::DataVersion => Some((0, "no arguments")),
            B::SocketClose | B::SocketSendAll => Some((1, "one argument")),
            B::SocketSend | B::SocketUnsubscribe | B::Broadcast | B::SocketTag => Some((2, "two arguments")),
            B::SocketSubscribe => Some((4, "the socket, the stream, and the text before and after each message broadcast to it")),
            B::HmacSha1 | B::HmacSha256 | B::XorBytes | B::SecretEqual | B::BcryptHash | B::BcryptCheck | B::FormatTime | B::Contains | B::StartsWith | B::EndsWith | B::RandomText | B::AddYears | B::IndexOf | B::LastIndexOf | B::ByteAt | B::CacheHole => Some((2, "two arguments")),
            B::GcmSeal | B::GcmOpen => Some((3, "the key, the 12-byte nonce and the text")),
            B::Replace => Some((3, "the text, what to replace and what with")),
            B::Pbkdf2Sha256 => Some((4, "the password, the salt, the iterations and the length")),
            _ => None,
        };
        if let Some((k, what)) = want {
            if a.len() != k {
                let name = match bi { B::Base64 => "base64", B::Base64Url => "base64_url", B::Unbase64 => "unbase64", B::Hex => "hex", B::Sha1 => "sha1", B::Sha256 => "sha256",
                    B::RandomBytes => "random_bytes", B::Env => "env", B::EscapeHtml => "escape_html", B::JsonQuote => "json_quote", B::JsonUnquote => "json_unquote", B::RequestTarget => "request_target", B::Crc32 => "crc32", B::ByteAt => "byte_at", B::Lower => "lower", B::Upper => "upper", B::CacheGet => "cache_get", B::CachePut => "cache_put", B::CacheStart => "cache_start", B::CacheHole => "cache_hole", B::CachePart => "cache_part", B::CacheLabel => "cache_label", B::DataVersion => "data_version", B::Replace => "replace", B::TrimStart => "trim_start", B::TrimEnd => "trim_end", B::AllEmoji => "all_emoji", B::HtmlParse => "html_parse", B::HtmlParseIn => "html_parse_in", B::NodeKind => "node_kind", B::NodeName => "node_name", B::NodeParent => "node_parent", B::NodeFirst => "node_first", B::NodeLast => "node_last", B::NodeNext => "node_next", B::NodePrev => "node_prev", B::NodeContent => "node_content", B::NodeAttr => "node_attr", B::NodeAttrCount => "node_attr_count", B::NodeAttrName => "node_attr_name", B::NodeAttrValue => "node_attr_value", B::NodeSetAttr => "node_set_attr", B::NodeRemoveAttr => "node_remove_attr", B::NodeNewElement => "node_new_element", B::NodeNewText => "node_new_text", B::NodeAppend => "node_append", B::NodeInsertBefore => "node_insert_before", B::NodeDetach => "node_detach", B::NodeClone => "node_clone", B::NodeHtml => "node_html", B::NodeInnerHtml => "node_inner_html", B::NodeSetText => "node_set_text", B::NodeHtmlEscaped => "node_html_escaped", B::Chr => "chr", B::AllWord => "all_word", B::ReadStored => "read_stored", B::SocketSend => "socket_send", B::SocketClose => "socket_close", B::SocketSubscribe => "socket_subscribe", B::SocketUnsubscribe => "socket_unsubscribe", B::Broadcast => "broadcast", B::SocketSendAll => "socket_send_all", B::SocketTag => "socket_tag", B::RandomText => "random_text", B::AddYears => "add_years", B::IndexOf => "index_of", B::LastIndexOf => "last_index_of", B::FormEncode => "form_encode", B::FormDecode => "form_decode", B::HmacSha1 => "hmac_sha1", B::HmacSha256 => "hmac_sha256",
                    B::XorBytes => "xor_bytes", B::SecretEqual => "secret_equal", B::BcryptHash => "bcrypt_hash", B::BcryptCheck => "bcrypt_check", B::FormatTime => "format_time", B::Contains => "contains", B::StartsWith => "starts_with", B::EndsWith => "ends_with",
                    B::GcmSeal => "aes256gcm_seal", B::GcmOpen => "aes256gcm_open", _ => "pbkdf2_sha256" };
                return no(format!("`{}` takes {}", name, what));
            }
        }
        // `str(n)` of a number or bool: its longest text, for the bounds of lets of built text
        let str_bound = self.builtin_bound(bi, &a);
        if let (true, Some(code)) = (generic, BUILTINS.iter().position(|x| *x == bi)) {
            // arguments as one Val array; the interpreter's builtin computes the result
            let mut names = Vec::new();
            for v in a { names.push(self.val(v)?); }
            let argv = self.tmp("argv");
            if names.is_empty() { self.line(&format!("Val {}[1];", argv)); } else { self.line(&format!("Val {}[{}] = {{ {} }};", argv, names.len(), names.join(", "))); }
            let r = self.tmp("v");
            self.line(&format!("Val {};", r));
            // a crypto built-in is marked, so the unit gets runtime/crypto.c and its library (program_c_full)
            let mark = if CRYPTO_BUILTINS.contains(&bi) { "/*crypto*/" } else if HTML_BUILTINS.contains(&bi) { "/*html*/" } else { "" };
            self.call(&format!("c->h->builtin(c, {}, {}, {}, &{}){}", code, argv, names.len(), r, mark));
            let _ = n;
            let _ = int_out;
            return Ok(match bi {
                B::Int | B::Len | B::Now | B::Random | B::Stat | B::WorkerIndex | B::WorkerCount | B::Min | B::Max | B::Slot | B::AddYears | B::IndexOf | B::LastIndexOf | B::Crc32 | B::ByteAt | B::CacheGet | B::DataVersion
                    | B::HtmlParse | B::HtmlParseIn | B::NodeKind | B::NodeParent | B::NodeFirst | B::NodeLast | B::NodeNext | B::NodePrev | B::NodeAttrCount | B::NodeNewElement | B::NodeNewText | B::NodeClone => CV::Int(format!("{}.i", r)),
                B::SecretEqual | B::BcryptCheck | B::Contains | B::StartsWith | B::EndsWith | B::AllEmoji | B::AllWord => CV::Bool(format!("({}.i != 0)", r)),
                B::Str | B::Token | B::Base64 | B::Base64Url | B::Hex | B::Sha1 | B::Sha256 | B::HmacSha1 | B::HmacSha256 | B::Pbkdf2Sha256 | B::RandomBytes
                    | B::XorBytes | B::GcmSeal | B::BcryptHash | B::FormEncode | B::FormatTime | B::EscapeHtml | B::RandomText | B::JsonQuote | B::RequestTarget | B::Lower | B::Upper | B::CacheLabel | B::Replace | B::TrimStart | B::TrimEnd
                    | B::NodeName | B::NodeContent | B::NodeAttrName | B::NodeAttrValue | B::NodeHtml | B::NodeInnerHtml | B::NodeHtmlEscaped | B::Chr => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, 1 }};", t, r, r)); if let Some(b) = str_bound { self.txt_bounds.insert(t.clone(), b); } CV::Txt(t) }
                // a text, or null: decoding or opening that failed
                B::Param | B::Query | B::Header | B::Unbase64 | B::GcmOpen | B::FormDecode | B::Env | B::JsonUnquote | B::ReadStored | B::NodeAttr => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, {}.tag == 4 }};", t, r, r, r)); if let Some(b) = str_bound { self.txt_bounds.insert(t.clone(), b); } CV::Txt(t) }
                _ => CV::Any(r),
            });
        }
        Ok(match bi {
            B::Auth => {
                let Some(CV::Coll(c, inst)) = a.first() else { return no("auth(pool)") };
                if inst != "0" { return no("auth of a nested pool"); }
                let t = self.tmp("i"); self.line(&format!("u32 {};", t)); self.call(&format!("c->h->auth(c, {}, &{})", c, t));
                CV::Item(*c, t)
            }
            B::Min | B::Max => {
                let mut it = a.into_iter();
                let (Some(x), Some(y)) = (it.next(), it.next()) else { return no("min/max take two numbers") };
                let op = if bi == B::Min { "<" } else { ">" };
                match (x, y) {
                    (CV::Int(x), CV::Int(y)) => { let (x, y) = (self.hold_int(x), self.hold_int(y)); CV::Int(format!("({} {} {} ? {} : {})", x, op, y, x, y)) }
                    // a float and an integer give a float, as the arithmetic operators do
                    (x @ (CV::Flt(_) | CV::Int(_)), y @ (CV::Flt(_) | CV::Int(_))) => {
                        let f = |v: CV| match v { CV::Flt(e) => e, CV::Int(e) => format!("(double)({})", e), _ => unreachable!() };
                        let (x, y) = (f(x), f(y));
                        let (tx, ty) = (self.tmp("f"), self.tmp("f"));
                        self.line(&format!("double {} = {}, {} = {};", tx, x, ty, y));
                        CV::Flt(format!("({} {} {} ? {} : {})", tx, op, ty, tx, ty))
                    }
                    _ => return no("min/max of something other than numbers"),
                }
            }
            B::Count => {
                let Some(CV::Coll(c, inst)) = a.first() else { return no("count(collection)") };
                if let Some(t) = tab(*c) { let r = self.tmp("n"); self.line(&format!("i64 {};", r)); self.call(&format!("c->h->tcount(c, {}, (u32)({}), &{})", t, inst, r)); return Ok(CV::Int(r)); }
                let col = &self.prog.colls[*c];
                let f = if col.is_list() { 0 } else { 1 };
                CV::Int(format!("(i64)LD_U32({}, {})", col.hdr + 4 * f * col.mult, inst))
            }
            B::CountBelow => {
                // a null argument comes from a copy of a function made for a null argument, in a branch that cannot run with it
                // (a walk with no `from`): it fails if reached, as an operator on null does
                if a.iter().any(|v| matches!(v, CV::Null)) { self.fail(500, "operator does not fit these values"); return Ok(CV::Int("0LL".into())); }
                let mut it = a.into_iter();
                let (Some(arr), Some(CV::Int(n)), Some(CV::Int(target)), Some(incl)) = (it.next(), it.next(), it.next(), it.next()) else { return no("`count_below(n, target, inclusive)`: n and target are integers, inclusive a bool") };
                let incl = match incl { CV::Bool(e) => format!("(({}) ? 1 : 0)", e), CV::Int(e) => format!("(({}) != 0)", e), _ => return no("`count_below`: inclusive is a bool") };
                let (base, code, len) = match arr {
                    CV::ArrCol(c, idx, ci) => {
                        let col = &self.prog.colls[c].cols[ci];
                        let CKind::Arr(s, len) = col.kind else { unreachable!() };
                        (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), plain_code(&Ty::Scalar(s)).map_or(0, |x| x.0), len)
                    }
                    CV::PlainArr(off, e, len) => (format!("{}ULL", off), plain_code(&e).map_or(0, |x| x.0), len),
                    _ => return no("`count_below` works on a fixed array"),
                };
                if !(1..=8).contains(&code) { return no("`count_below` works on an array of integers"); }
                let (n, target) = (self.hold_int(n), self.hold_int(target));
                self.line(&format!("if (UNLIKELY({n} < 0 || {n} > {len}LL)) FAIL(400, \"count_below out of range\", 24);"));
                let r = self.tmp("r");
                self.line(&format!("i64 {};", r));
                self.call(&format!("c->h->count_below(c, {}, {}, {}, {}, {}, &{})", base, code, n, target, incl, r));
                CV::Int(r)
            }
            B::Capacity => { let Some(CV::Coll(c, _)) = a.first() else { return no("`capacity()` is a function of a collection") }; CV::Int(format!("{}LL", self.prog.colls[*c].cap)) }
            B::Slot => match a.first() {
                Some(CV::Item(c, i)) if tab(*c).is_some() => CV::Int(format!("(i64)({})", i)),
                Some(CV::Item(c, i)) => CV::Int(format!("(i64)({} % {})", i, self.prog.colls[*c].cap)),
                Some(CV::Ref(..)) => return no("`slot(x)` of a value that may be null: check it first with `if exists(x) {{ ... }}` (inside, x is the item), or give a fallback with `or(...)`"),
                _ => unreachable!("generic above"),
            },
            B::Atan2 | B::Pow | B::Hypot | B::Fmod => {
                let name = match bi { B::Atan2 => "atan2", B::Hypot => "hypot", B::Fmod => "fmod", _ => "pow" };
                let num = |v: &CV| match v { CV::Flt(e) => Some(e.clone()), CV::Int(e) => Some(format!("(double)({})", e)), _ => None };
                let (x, y) = match a.as_slice() { [p, q] => match (num(p), num(q)) { (Some(x), Some(y)) => (x, y), _ => return no(format!("`{}` takes two numbers", name)) }, _ => return no(format!("`{}` takes two numbers", name)) };
                CV::Flt(format!("{}_({}, {})", name.to_uppercase(), x, y))
            }
            B::Sqrt | B::Exp | B::Floor | B::Sin | B::Cos | B::Tan | B::Asin | B::Acos | B::Atan | B::Sinh | B::Cosh | B::Tanh | B::Ln | B::Log2 | B::Log10 | B::Ceil | B::Round | B::Trunc | B::Cbrt => {
                // IEEE results, as the platform's math library gives them (a negative square root is NaN)
                let name = match bi { B::Sqrt => "sqrt", B::Exp => "exp", B::Sin => "sin", B::Cos => "cos", B::Tan => "tan", B::Asin => "asin", B::Acos => "acos", B::Atan => "atan", B::Sinh => "sinh", B::Cosh => "cosh", B::Tanh => "tanh", B::Ln => "ln", B::Log2 => "log2", B::Log10 => "log10", B::Ceil => "ceil", B::Round => "round", B::Trunc => "trunc", B::Cbrt => "cbrt", _ => "floor" };
                let x = match a.as_slice() { [CV::Flt(e)] => e.clone(), [CV::Int(e)] => format!("(double)({})", e), _ => return no(format!("`{}` takes one number", name)) };
                let mac = if name == "round" { "ROUNDF_".to_string() } else { format!("{}_", name.to_uppercase()) };
                CV::Flt(format!("{}({})", mac, x))
            }
            B::Substr => {
                // a piece of a string, `substr(text, from, count)`: it shares the text's bytes (no copy), the range is checked
                let [text, from, count] = <[CV; 3]>::try_from(a).map_err(|_| "`substr` takes a string, a start and a count".to_string())?;
                if matches!(text, CV::PlainStr(_) | CV::StateStr(..) | CV::Sub(..)) {
                    let (from, count) = (self.int_or(from, 400, "a start and a count must be integers"), self.int_or(count, 400, "a start and a count must be integers"));
                    let (from, count) = (self.hold_int(from), self.hold_int(count));
                    let (base, start, have) = match text { CV::Sub(b, s0, n0) => (*b, s0, n0), b => { let l = self.field_len(&b); let l = self.hold_int(l); (b, "0".to_string(), l) } };
                    self.line(&format!("if (UNLIKELY({from} < 0 || {count} < 0 || {from} + {count} > (i64)({have}))) FAIL(400, \"substring out of range\", 22);"));
                    let s2 = if start == "0" { from.clone() } else { let t = self.tmp("v"); self.line(&format!("i64 {} = {} + {};", t, start, from)); t };
                    if !self.is_ascii(&base) {
                        // utf8: a piece starts and ends on a character's first byte (or the end)
                        let flen = self.field_len(&base);
                        let (a0, a1) = (self.sub_bytes_at(&base, &s2), self.sub_bytes_at(&base, &format!("{} + {}", s2, count)));
                        self.line(&format!("if (UNLIKELY(({s2} < {flen} && (LD_U8({a0}, 0) & 0xC0) == 0x80) || ({s2} + {count} < {flen} && (LD_U8({a1}, 0) & 0xC0) == 0x80))) FAIL(400, \"substring would split a character\", 33);"));
                    }
                    return Ok(CV::Sub(Box::new(base), s2, count));
                }
                let tb = self.txt_bound(&text);
                let t = self.txt_of(text)?;
                let (from, count) = (self.int_or(from, 400, "a start and a count must be integers"), self.int_or(count, 400, "a start and a count must be integers"));
                let (from, count) = (self.hold_int(from), self.hold_int(count));
                self.line(&format!("if (UNLIKELY({from} < 0 || {count} < 0 || {from} + {count} > (i64){t}.len)) FAIL(400, \"substring out of range\", 22);"));
                let r = self.tmp("t");
                self.line(&format!("Txt {r} = {{ (u32)({t}.off + {from}), (u32){count}, 1 }};"));
                if let Some(b) = tb { self.txt_bounds.insert(r.clone(), b); }
                CV::Txt(r)
            }
            B::Trim => {
                // `trim(text)`: the text without the spaces, tabs and line breaks at either end, a window onto the same bytes
                let [text] = <[CV; 1]>::try_from(a).map_err(|_| "`trim` takes one text".to_string())?;
                let tb = self.txt_bound(&text);
                let t = self.txt_of(text).map_err(|_| "`trim` takes a text".to_string())?;
                let r = self.tmp("t");
                self.line(&format!("Txt {} = {{ 0, 0, 0 }};", r));
                self.call(&format!("c->h->trim(c, &{}, &{})", t, r));
                if let Some(b) = tb { self.txt_bounds.insert(r.clone(), b); }
                CV::Txt(r)
            }
            B::WrapAdd | B::WrapSub | B::WrapMul => {
                // two's-complement wrapping arithmetic (the ordinary operators fail the request on overflow)
                let name = match bi { B::WrapAdd => "wrapping_add", B::WrapSub => "wrapping_sub", _ => "wrapping_mul" };
                let op = match bi { B::WrapAdd => '+', B::WrapSub => '-', _ => '*' };
                let [x, y] = <[CV; 2]>::try_from(a).map_err(|_| format!("`{}` takes two integers", name))?;
                let (x, y) = (self.int_or(x, 400, "wrapping arithmetic takes integers"), self.int_or(y, 400, "wrapping arithmetic takes integers"));
                CV::Int(format!("(i64)((u64)({}) {} (u64)({}))", x, op, y))
            }
            B::TextFrom => {
                // `text_from(bytes, from, count)`: the bytes of a state array of u8 as a string (copied into the request's text area), range checked
                let [arr, from, count] = <[CV; 3]>::try_from(a).map_err(|_| "`text_from` takes a byte array, a start and a count".to_string())?;
                let (base, len) = match arr {
                    CV::ArrCol(c, idx, ci) => {
                        let col = &self.prog.colls[c].cols[ci];
                        let CKind::Arr(_, len) = col.kind else { unreachable!() };
                        if col.kind.size() != len { return no("`text_from` reads an array of u8"); }
                        (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), len)
                    }
                    CV::PlainArr(off, e, len) => {
                        if self.prog.plain_size(&e).0 != 1 { return no("`text_from` reads an array of u8"); }
                        (format!("{}ULL", off), len)
                    }
                    _ => return no("`text_from` reads a fixed array of u8"),
                };
                let (from, count) = (self.int_or(from, 400, "a start and a count must be integers"), self.int_or(count, 400, "a start and a count must be integers"));
                let (from, count) = (self.hold_int(from), self.hold_int(count));
                self.line(&format!("if (UNLIKELY({from} < 0 || {count} < 0 || {from} + {count} > {len}LL)) FAIL(400, \"text_from out of range\", 22);"));
                let r = self.tmp("t");
                self.line(&format!("Txt {r};"));
                self.call(&format!("c->h->arr_txt(c, {base} + (u64){from}, (u32){count}, &{r})"));
                CV::Txt(r)
            }
            B::Abs => match a.as_slice() {
                [CV::Flt(e)] => CV::Flt(format!("FABS_({})", e)),
                [CV::Int(e)] => { let e = self.hold_int(e.clone()); let r = self.tmp("v"); self.line(&format!("ABS_({}, {});", r, e)); CV::Int(r) }
                _ => return no("`abs` takes one number"),
            },
            // a value kept in a `let`, `var`, parameter or result declared with a sized number type (`var h: u32`): tested against the type's range, as a
            // store into a field with that range is (the parser writes the call; `a` is the value, the type's code and the name for the message)
            B::InRange => {
                let [v, code, name] = <[CV; 3]>::try_from(a).map_err(|_| "internal: a typed value takes three arguments".to_string())?;
                let code = match &code { CV::Int(s) => s.trim_end_matches("LL").parse::<usize>().unwrap_or(0), _ => 0 };
                let name = match &name { CV::Lit(s) => s.clone(), _ => "value".to_string() };
                let (_, lo, hi) = NUM_TYPES[code.min(NUM_TYPES.len() - 1)];
                let v = self.in_field_range(v, (lo, hi), &name)?;
                self.with_field_range(v, Some((lo, hi)))
            }
            // an `f32`: a number rounded to the nearest float
            B::F32 => match a.as_slice() {
                [CV::Flt(e)] => CV::Flt(format!("((double)(float)({}))", e)),
                [CV::Int(e)] => CV::Flt(format!("((double)(float)({}))", e)),
                _ => return no("an f32 takes a number"),
            },
            B::Crash => { self.line("TAIL(c->h->crash(c));"); CV::Null }
            _ => unreachable!("handled above"),
        })
    }

    /// Inline a user function: its parameters become values in a fresh frame; `return` jumps to the end of the inlined block.
    fn inline(&mut self, k: u32, args: &'p [Expr], env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        let mut vals = Vec::new();
        // where a struct this call returns goes is meant for the call, not for a call among its arguments
        let ctx = self.ret_ctx.take();
        for a in args { let v = self.expr(a, env, dead)?; vals.push(self.hold(v, false)?); }
        self.ret_ctx = ctx;
        self.call_values(k, vals, dead)
    }

    /// Call user function `k` with argument values already computed: inlined, or a generated C function if recursive.
    fn call_values(&mut self, k: u32, vals: Vec<CV>, dead: Option<&str>) -> R<CV> {
        self.pending_ctx = self.ret_ctx.take();
        let f = &self.api.fns[k as usize];
        if vals.len() != f.params.len() {
            // a function of a type is called `p.hit(1)`: the receiver is not counted among its arguments
            let (shown, skip) = (f.name.clone(), if f.recv { 1 } else { 0 });
            let (want, given) = (f.params.len() - skip, vals.len().saturating_sub(skip));
            return no(format!("`{}` takes {} argument{}, given {}", shown, want, if want == 1 { "" } else { "s" }, given));
        }
        if let Some(t) = &f.ret { self.ret_kind(t, &f.name)?; } // (a one-line function never reaches its result kind otherwise)
        let mut checked = Vec::with_capacity(vals.len());
        for (i, v) in vals.into_iter().enumerate() {
            let v = match &f.ptys[i] { Some(t) => self.to_declared(v, t, &f.name, &format!("parameter `{}` of `{}`", f.params[i], f.name))?, None => v };
            // a struct parameter is read-only unless declared `write` (it may change the caller's struct, and may be handed to other `write` parameters) or `copy` (the function gets a copy
            // of its own, which it may change). A read-only one is passed as it is, which no one can tell from a copy, unless the function writes something else (it might write the very
            // place the argument is): then the copy is made.
            let mode = f.modes.get(i).copied().unwrap_or(0);
            if mode != 0 {
                let (word, pn) = ("write", f.params[i].as_str());
                let why = match &v {
                    CV::Plain(..) => None,
                    CV::Int(_) | CV::Flt(_) | CV::Bool(_) => Some("`write` of a number or bool is not supported yet: return the new value instead".to_string()),
                    CV::StateStr(..) | CV::PlainStr(..) if mode == 1 => None,
                    CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) if mode == 1 => Some("`write` needs a place to write to (a text field of the state or of an item), not a literal or computed text (a `var` text local is not supported yet)".to_string()),
                    CV::Item(..) | CV::Ref(..) => None, // an item is already a handle to its row: `write` is allowed (so one signature serves a state struct and an item) and changes nothing
                    _ => Some("only a plain struct (with its text and array fields) can be a `write` parameter".to_string()),
                };
                if let Some(why) = why { return no(format!("`{}: {}` in `{}`: {}", pn, word, f.name, why)); }
            }
            if let CV::Plain(sid, _) = v {
                if mode == 0 && f.test.is_none() {
                    if param_written(self.api, &f.body, i as u32) {
                        let ty = self.prog.structs[sid].name.clone();
                        return no(format!("`{}` is read-only here, and this changes it (or hands it to a `write` parameter): declare `{}: write {}` to change the caller's struct, or work on `var c = {} {{ ..{} }}`", f.params[i], f.params[i], ty, ty, f.params[i]));
                    }
                }
            }
            checked.push(v);
        }
        let vals = checked;
        let args: Vec<String> = vals.iter().map(|v| type_text(self.prog, v)).collect();
        let r = self.call_values_inner(k, vals, dead);
        self.pending_ctx = None;
        let r = r?;
        let rt = if returns_value(&self.api.fns[k as usize].body) { type_text(self.prog, &r) } else { "-".into() };
        SIGS.with(|s| s.borrow_mut().push((k, args, rt)));
        let (opt, name) = (self.api.fns[k as usize].ret_opt, self.api.fns[k as usize].name.clone());
        let gives = returns_value(&self.api.fns[k as usize].body); // a function without `return value` gives nothing, which is not null
        Ok(match r {
            // (a recursive function's result reads as null until its kind is known: only a definite item-or-null counts there)
            CV::Ref(..) if !opt && gives => return no(format!("`{}` can return null (it returns `null`, or a value that may be null, on some path): declare it `fn {}(...) -> T?` so that callers check it", name, name)),
            CV::Null if !opt && gives && !self.recursive[k as usize] => return no(format!("`{}` can return null (it returns `null`, or a value that may be null, on some path): declare it `fn {}(...) -> T?` so that callers check it", name, name)),
            CV::Item(c, e) if opt => { let s = self.tmp("r"); self.line(&format!("u32 {} = 1;", s)); CV::Ref(c, s, e) }
            r => r,
        })
    }
    fn call_values_inner(&mut self, k: u32, vals: Vec<CV>, dead: Option<&str>) -> R<CV> {
        let f = &self.api.fns[k as usize];
        self.depth_check();
        // a function that returns a struct is always inlined (its result is written into the caller's destination)
        let returns_plain = matches!(f.ret.as_ref().map(|t| self.decl_kind(t, &f.name)), Some(Ok(K::Plain(..))));
        if self.recursive[k as usize] {
            if returns_plain { return no(format!("`{}` is recursive and returns a struct: not supported yet (a recursive function cannot hold struct values)", f.name)); }
            self.pending_ctx = None;
            return self.call_spec(k, vals);
        }
        // an outlined function: called, when its arguments are of kinds a C function takes and its body compiles as one; inlined otherwise
        if !returns_plain && OUTLINED.with(|o| o.borrow().get(k as usize).copied().unwrap_or(false)) && vals.iter().all(spec_arg) {
            let (n0, body0, specs0) = (self.n, self.body.len(), self.specs.borrow().len());
            match self.call_spec(k, vals.clone()) {
                Ok(v) => return Ok(v),
                Err(_) => { self.n = n0; self.body.truncate(body0); self.specs.borrow_mut().truncate(specs0); self.widen = None; WHY.with(|w| w.borrow_mut().clear()); }
            }
        }
        let mut fenv: Vec<Option<CV>> = vec![None; f.slots as usize];
        for (i, v) in vals.into_iter().enumerate() { fenv[i] = Some(v); }
        self.inl += 1;
        let r = self.inline_body(k, f, &mut fenv, dead);
        self.inl -= 1;
        r
    }

    /// An error inside a function names the function's statement, not the call (the endpoint is named beside it).
    fn inline_body(&mut self, k: u32, f: &'p Func, fenv: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        let caller_at = self.at;
        if let Some(Stmt::At(l, c)) = f.body.first() { self.at = (*l, *c); }
        let r = self.inline_body_at(k, f, fenv, dead).map_err(|e| place_err(self.at, e));
        self.at = caller_at;
        r
    }
    fn inline_body_at(&mut self, k: u32, f: &'p Func, fenv: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        // a function that returns a struct writes it into the place the call was given (`target = f(x);`: the target itself; `let v = f(x);`: a new place that `v` names)
        let ctx = self.pending_ctx.take();
        let struct_dest = match f.ret.as_ref().map(|t| self.decl_kind(t, &f.name)) {
            Some(Ok(K::Plain(sid, _))) => match ctx {
                None => {
                    let short = f.name.rsplit('.').next().unwrap_or(&f.name).to_string();
                    return no(format!("`{}` returns a struct, which has to go somewhere: write `target = {}(...);`, `let x = {}(...);` or `var x = {}(...);` (name the middle of a chain: `let t = a.plus(b); world.pos = t.scaled(2.0);`)", f.name, short, short, short));
                }
                Some(Some(off)) => Some((sid, off)),
                Some(None) => { self.prog.value_check(sid)?; let off = self.scratch_alloc(self.prog.structs[sid].size)?; Some((sid, off)) }
            },
            _ => None,
        };
        // a function that is just `return e;` needs no block
        if let Some(Stmt::Return(Some(body))) = only_stmt(&f.body) {
            self.inlining.push(k);
            let r = self.expr(body, fenv, dead);
            self.inlining.pop();
            if let Some((sid, off)) = struct_dest {
                let CV::Plain(s2, src) = r? else { return no(format!("`{}` returns a `{}`: return a struct of that type", f.name, self.prog.structs[sid].name)) };
                if s2 != sid { return no(format!("`{}` returns a `{}`, not a `{}`", f.name, self.prog.structs[sid].name, self.prog.structs[s2].name)); }
                self.copy_struct(sid, off, src)?;
                return Ok(CV::Plain(sid, off));
            }
            return r;
        }
        let (var, label) = (self.tmp("ret"), self.tmp("end"));
        let saved = std::mem::take(&mut self.body);
        self.inlining.push(k);
        let forced = match &f.ret { Some(_) if struct_dest.is_some() => struct_dest.map(|(s, o)| K::Plain(s, o)), Some(t) => Some(self.ret_kind(t, &f.name)?), None => self.ret_kinds.iter().find(|x| x.0 == k).map(|x| x.1) };
        let sel = if has_collecting_loop(self.api, &f.body) { Some(self.tmp("hm")) } else { None };
        let rmark = self.tmp("rm");
        self.ret.push(Ret { dest: struct_dest, var: var.clone(), label: label.clone(), kind: forced, func: false, sel: sel.clone(), bound: None, unbounded: false, mark: rmark.clone(), ivs: Vec::new() });
        self.depth += 1;
        let frame = std::mem::replace(&mut self.frame, Some(k));
        let r = self.block(&f.body, fenv);
        self.frame = frame;
        self.depth -= 1;
        let ret = self.ret.pop().unwrap();
        self.inlining.pop();
        let inner = std::mem::replace(&mut self.body, saved);
        r?;
        let inner = resolve_return_cuts(&inner, &ret.mark);
        let ends_by_itself = matches!(f.body.last(), Some(Stmt::Return(_)) | Some(Stmt::Fail(..)) | Some(Stmt::Respond(..)) | Some(Stmt::Forward { .. }));
        let kind = ret.kind.unwrap_or(K::Null);
        if !ends_by_itself && kind != K::Null {
            if let Some(t) = &f.ret { if !t.opt { return no(format!("`{}` can end without returning its `{}`: return a value on every path", f.name, t)); } }
            // falling off the end returns null, as in the interpreter
            if !fits(K::Null, kind) {
            if unify(kind, K::Null) == K::Any { why(format!("`{}` returns {} on one path and nothing on another: return a value on every path", self.api.fns[k as usize].name, kname(self.prog, kind))); }
            self.widen = Some(Widen::Ret(k, unify(kind, K::Null))); return no("a function that can end without returning its value");
        }
        }
        let (decl, out) = match kind {
            K::Int => {
                // every value returned had a range: the call's result is inside their union
                if !ret.ivs.is_empty() && ret.ivs.iter().all(|x| x.is_some()) {
                    let (lo, hi) = ret.ivs.iter().flatten().fold((i64::MAX, i64::MIN), |a, r| (a.0.min(r.0), a.1.max(r.1)));
                    self.ranges.insert(format!("{}_v", var), (lo, hi));
                }
                (format!("i64 {}_v = 0;", var), CV::Int(format!("{}_v", var)))
            }
            K::Flt => (format!("double {}_v = 0;", var), CV::Flt(format!("{}_v", var))),
            K::Bool => (format!("int {}_v = 0;", var), CV::Bool(format!("{}_v", var))),
            K::Txt => {
                // the most any of its returns gives
                if let (false, Some(b)) = (ret.unbounded, ret.bound) { self.txt_bounds.insert(format!("{}_v", var), b); }
                (format!("Txt {}_v = {{ 0, 0, 0 }};", var), CV::Txt(format!("{}_v", var)))
            }
            K::Any => (format!("Val {}_v = {{ 0, 0, 0, 0, 0 }};", var), CV::Any(format!("{}_v", var))),
            K::Item(c) => (format!("u32 {}_v = 0;", var), CV::Item(c, format!("{}_v", var))),
            K::Ref(c) => (format!("u32 {}_s = 0, {}_v = 0;", var, var), CV::Ref(c, format!("{}_s", var), format!("{}_v", var))),
            K::Coll(c) => (format!("u32 {}_v = 0;", var), CV::Coll(c, format!("{}_v", var))),
            K::Plain(sd, o) => (String::new(), CV::Plain(sd, o)),
            K::Null => (String::new(), CV::Null),
        };
        if !decl.is_empty() { self.line(&decl); }
        if let Some(h) = &ret.sel { self.line(&format!("u32 {};", h)); self.call(&format!("c->h->sel_mark(c, &{})", h)); }
        self.line("{");
        self.body.push_str(&inner);
        self.line("}");
        self.line(&format!("{}: ;", label));
        if let Some(h) = &ret.sel { self.call(&format!("c->h->sel_end(c, {})", h)); }
        Ok(out)
    }

    /// Emit a loop over a list or pool instance (with `reverse`, `where` and `limit`), running `body` with the item bound.
    /// With `collect`, the items passing `where` are collected before any body runs, then limited, as the interpreter's
    /// `select` does; otherwise filtering and the body run in one pass.
    /// A loop's labels for `break` and `continue` (placed only when used).
    fn loop_enter(&mut self) { self.loop_n += 1; let (b, c) = (format!("brk_{}", self.loop_n), format!("cnt_{}", self.loop_n)); self.loops.push((b, c, false, false)); }
    /// The end of a round's body: where `continue` lands, unless a `for` loop's step placed it already.
    fn loop_round_end(&mut self) { if let Some(l) = self.loops.last_mut() { if l.3 { l.3 = false; let t = format!("{}: ;", l.1); self.line(&t); } } }
    /// After the loop: where `break` lands.
    fn loop_exit(&mut self) { if let Some(l) = self.loops.pop() { if l.2 { self.line(&format!("{}: ;", l.0)); } } }
    /// Each round: the index (`foreach item, i in ...`: the rounds before this one), and for a loop without a fixed extent its
    /// count against the request's step budget.
    fn round_start(&mut self, it: &'p Iter, env: &mut Vec<Option<CV>>, hits: &str) {
        if let Some((_, slot)) = &it.index { let ix = self.tmp("ix"); self.line(&format!("i64 {} = (i64){} - 1;", ix, hits)); env[*slot as usize] = Some(CV::Int(ix)); }
    }

    /// `foreach p in split(t, sep)` (or `split(t)`, by whitespace): each piece a window onto the text, as Ruby's split gives them
    /// (h_split_next); `where` and `limit` as over a collection, `p, i` the piece's number.
    fn split_loop(&mut self, it: &'p Iter, args: &'p [Expr], env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        if args.is_empty() || args.len() > 2 { return no("`split(text, separator)` or `split(text)` (by white space)"); }
        if it.by.is_some() || it.after.is_some() || it.reverse || it.last || it.from.is_some() || it.whil.is_some() { return no("a loop over `split` takes `where` and `limit`"); }
        let t = self.expr(&args[0], env, None)?; let t = self.txt_of(t).map_err(|_| "`split` splits a text".to_string())?;
        let sep = match args.get(1) { Some(e) => { let v = self.expr(e, env, None)?; Some(self.txt_of(v).map_err(|_| "`split`'s separator is a text".to_string())?) } None => None };
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        self.loop_enter();
        let (pos, part, more, hits) = (self.tmp("pos"), self.tmp("t"), self.tmp("more"), self.tmp("hits"));
        self.open("{");
        self.line(&format!("u32 {} = 0, {} = 0; int {} = 0; Txt {};", pos, hits, more, part));
        // a text of n bytes has at most n + 1 pieces
        let round = self.tmp("k");
        self.open(&format!("for (u32 {r} = 0; {r} <= {}.len; {r}++) {{", t, r = round));
        if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); }
        self.call(&format!("c->h->split_next(c, &{}, {}, &{}, &{}, &{})", t, sep.as_ref().map_or("0".to_string(), |s| format!("&{}", s)), pos, part, more));
        self.line(&format!("if (!{}) break;", more));
        if let Some(b) = self.txt_bounds.get(&t).copied() { self.txt_bounds.insert(part.clone(), b); }
        env[it.slot as usize] = Some(CV::Txt(part.clone()));
        if let Some((_, s)) = it.index { let ix = self.tmp("ix"); self.line(&format!("i64 {} = (i64){};", ix, hits)); env[s as usize] = Some(CV::Int(ix)); }
        if let Some(w) = &it.wher { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) continue;", wc)); }
        self.line(&format!("{}++;", hits));
        body(self, env, &hits)?;
        self.close("}");
        self.close("}");
        self.loop_exit();
        Ok(())
    }
    fn each(&mut self, it: &'p Iter, env: &mut Vec<Option<CV>>, collect: bool, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        let loop_body = self.loop_body.take();
        if let Some(v) = it.via { return self.walk(it, v, env, body); }
        if let Some(hi) = &it.hi { return self.range_loop(it, hi, env, body); }
        if let Expr::Call(f, args, _) = &it.coll { if f == "split" { return self.split_loop(it, args, env, body); } }
        let CV::Coll(c, inst) = self.expr(&it.coll, env, None)? else { return no("each over something other than a collection") };
        if let Some(t) = tab(c) { return self.table_loop(it, t, inst, env, collect, loop_body, body); }
        if let Some(f) = &it.by { return no(format!("`by {}`: only a table is walked in a field's order (a list keeps insertion order; keep other orders in the data)", f)); }
        if it.after.is_some() { return no("`after`: only a table's rows are paged this way"); }
        self.loop_enter();
        let coll = self.prog.colls[c].clone();
        if it.last { return no("`last n` is for a loop over a table (the last n rows of its order)"); }
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        let (k, m, hits, h) = (self.tmp("k"), self.tmp("m"), self.tmp("hits"), self.tmp("h"));
        self.open("{");
        self.line(&format!("u32 {} = 0;", hits));
        if collect { self.line(&format!("u32 {};", h)); self.call(&format!("c->h->sel_mark(c, &{})", h)); }
        if coll.is_list() {
            let (len, head) = (self.tmp("len"), self.tmp("head"));
            self.line(&format!("u32 {} = LD_U32({}, {}), {} = LD_U32({}, {});", len, coll.hdr, inst, head, coll.hdr + 4 * coll.mult, inst));
            self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", k, k, len, k));
            let pos = if it.reverse { format!("({} + {} - 1 - {})", head, len, k) } else { format!("({} + {})", head, k) };
            self.line(&format!("u32 {} = (u32)((i64)({}) * {} + {} % {});", m, inst, coll.cap, pos, coll.cap));
        } else {
            let hw = self.tmp("hw");
            self.line(&format!("u32 {} = LD_U32({}, {});", hw, coll.hdr, inst));
            if it.reverse { self.open(&format!("for (u32 {} = {}; {}-- > 0;) {{", k, hw, k)); } else { self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", k, k, hw, k)); }
            self.line(&format!("if (!((LD_U64({}, (i64)({}) * {} + {} / 64) >> ({} % 64)) & 1)) continue;", coll.live, inst, coll.words(), k, k));
            self.line(&format!("u32 {} = (u32)((i64)({}) * {} + {});", m, inst, coll.cap, k));
        }
        if !collect { if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); } }
        let v = self.item_value(c, m.clone(), None)?;
        env[it.slot as usize] = Some(v);
        if let Some(w) = &it.wher {
            let wv = self.expr(w, env, None)?;
            let wc = self.cond(wv)?;
            self.line(&format!("if (!({})) continue;", wc));
        }
        self.line(&format!("{}++;", hits));
        if !collect {
            self.round_start(it, env, &hits);
            body(self, env, &hits)?;
            self.loop_round_end();
            self.close("}");
            self.close("}");
            self.loop_exit();
            return Ok(());
        }
        self.call(&format!("c->h->sel_add(c, {})", m));
        if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); }
        self.close("}");
        let (n, q, m2) = (self.tmp("n"), self.tmp("q"), self.tmp("m"));
        self.line(&format!("u32 {};", n));
        self.call(&format!("c->h->sel_limit(c, {}, {}, &{})", h, lim.clone().unwrap_or("-1".into()), n));
        self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", q, q, n, q));
        self.line(&format!("u32 {};", m2));
        self.call(&format!("c->h->sel_get(c, {}, {}, &{})", h, q, m2));
        self.line(&format!("{} = {} + 1;", hits, q));
        let v = self.item_value(c, m2, None)?;
        env[it.slot as usize] = Some(v);
        self.round_start(it, env, &hits);
        body(self, env, &hits)?;
        self.loop_round_end();
        self.close("}");
        self.loop_exit(); // `break` ends the selection too
        self.call(&format!("c->h->sel_end(c, {})", h));
        self.close("}");
        Ok(())
    }

    /// `foreach s in instance [from k] [reverse] ...`: walk a module instance with its iteration functions, at most its
    /// capacity of steps, stopping at a null cursor or the limit. Order of evaluation as in the interpreter: `from`,
    /// `limit`, then the first cursor.
    fn walk(&mut self, it: &'p Iter, v: crate::lang::Via, env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        let Some(n) = crate::lang::path_of(&it.coll) else { return no("walking something other than a module instance") };
        let steps = self.prog.colls.iter().find(|c| c.name == format!("{}.items", n)).map_or(0, |c| c.cap);
        let from = match &it.from { Some(e) => { let x = self.expr(e, env, None)?; self.hold(x, false)? } None => CV::Null };
        if it.last { return no("`last n` is for a loop over a table (the last n rows of its order)"); }
        let lim = match &it.limit { Some(e) => { let x = self.expr(e, env, None)?; Some(self.int_or(x, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        // a cursor is an integer, -1 when there is none
        let c0 = self.call_values(if it.reverse { v.last.ok_or("no reverse walk")? } else { v.first }, vec![from], None)?;
        let c0 = self.int_or(c0, 500, "a walk's cursor is an integer (-1 for none)");
        let (cur, k, hits) = (self.tmp("cur"), self.tmp("k"), self.tmp("hits"));
        self.loop_enter();
        self.open("{");
        self.line(&format!("i64 {} = {};", cur, c0));
        self.line(&format!("i64 {} = 0;", hits));
        self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", k, k, steps, k));
        self.line(&format!("if ({} < 0) break;", cur));
        if let Some(l) = &lim { self.line(&format!("if ({} >= {}) break;", hits, l)); }
        let item = self.call_values(v.item, vec![CV::Int(cur.clone())], None)?;
        let item = self.hold(item, false)?;
        env[it.slot as usize] = Some(item);
        if let Some(w) = &it.whil { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) break;", wc)); }
        let filtered = if let Some(w) = &it.wher { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.open(&format!("if ({}) {{", wc)); true } else { false };
        self.line(&format!("{}++;", hits));
        self.round_start(it, env, &hits);
        body(self, env, &hits)?;
        if filtered { self.close("}"); }
        self.loop_round_end(); // before the step to the next cursor
        let nx = self.call_values(if it.reverse { v.prev.ok_or("no reverse walk")? } else { v.next }, vec![CV::Int(cur.clone())], None)?;
        let nx = self.int_or(nx, 500, "a walk's cursor is an integer (-1 for none)");
        self.line(&format!("{} = {};", cur, nx));
        self.close("}");
        self.close("}");
        self.loop_exit();
        Ok(())
    }

    /// `foreach i in lo..hi`: the bounds are read once, values are computed as the loop goes (where is tested just before
    /// each body), and `limit` counts the bodies run.
    fn range_loop(&mut self, it: &'p Iter, hi: &'p Expr, env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        let v = self.expr(&it.coll, env, None)?;
        let lo = self.int_or(v, 500, "a range needs integers");
        let lo = self.hold_int(lo);
        let v = self.expr(hi, env, None)?;
        let hi = self.int_or(v, 500, "a range needs integers");
        let hi = self.hold_int(hi);
        if it.last { return no("`last n` is for a loop over a table (the last n rows of its order)"); }
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        let (k, hits) = (self.tmp("k"), self.tmp("hits"));
        self.loop_enter();
        self.open("{");
        self.line(&format!("i64 {} = 0;", hits));
        if it.reverse {
            let kk = self.tmp("k");
            self.open(&format!("for (i64 {} = {}; {} > {};) {{", kk, hi, kk, lo));
            self.line(&format!("i64 {} = --{};", k, kk));
        } else {
            self.open(&format!("for (i64 {} = {}; {} < {}; {}++) {{", k, lo, k, hi, k));
        }
        if let Some(l) = &lim { self.line(&format!("if ({} >= {}) break;", hits, l)); }
        // a loop without a fixed extent counts its rounds against the request's budget, so a mistake cannot hold the program
        if it.budget { self.line(&format!("ROUND_({});", k)); }
        env[it.slot as usize] = Some(CV::Int(k.clone()));
        // the counter is never assigned in the body: between the smallest lower bound and one below the largest upper bound
        if let (Some((lo_min, _)), Some((_, hi_max))) = (self.interval(&lo), self.interval(&hi)) { if hi_max > lo_min { self.ranges.insert(k.clone(), (lo_min, hi_max - 1)); } }
        if let Some(h) = self.loop_hint.take() { self.ranges.entry(k.clone()).or_insert(h); } // a versioning guard proved the bounds within 2^40
        // a counter this loop only steps by constants has, in the body and after it, the interval its entry value plus (steps per round x most rounds) gives
        if let (Some(stmts), Some(entry)) = (self.loop_stmts.take(), self.loop_entry.take()) {
            let by_range = match (self.interval(&lo), self.interval(&hi)) { (Some((lo_min, _)), Some((_, hi_max))) => Some((hi_max as i128 - lo_min as i128).max(0)), _ => None };
            // a loop without a fixed extent charges its rounds to the request's budget, which the runtime never allows above 2^50 (the rounds it can run before failing: a little more)
            let by_budget = if it.budget { Some(1i128 << 51) } else { None };
            let trips = match (by_range, by_budget) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) };
            let heads = self.counter_heads(stmts, &entry, trips, env);
            for (n, iv) in &heads { self.var_iv.insert(n.clone(), *iv); }
            self.loop_heads = Some(heads);
        }
        if let Some(w) = &it.whil { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) break;", wc)); self.refine(w, env)?; }
        if let Some(w) = &it.wher {
            let wv = self.expr(w, env, None)?;
            let wc = self.cond(wv)?;
            self.line(&format!("if (!({})) continue;", wc));
        }
        self.line(&format!("{}++;", hits));
        self.round_start(it, env, &hits);
        body(self, env, &hits)?;
        self.loop_round_end();
        self.close("}");
        self.close("}");
        self.loop_exit();
        Ok(())
    }

    /// Write `j` as JSON; returns the shape it has (known while generating), for checking against `Response<T>`.
    /// An enum value (an integer) as the name of its value; a number that is none of them is written as the number.
    fn write_enum(&mut self, en: usize, e: &str) -> R<()> {
        let x = self.tmp("ev");
        self.line(&format!("i64 {} = {};", x, e));
        let vals = self.prog.enums[en].values.clone();
        for (i, (n, v)) in vals.iter().enumerate() {
            self.open(&format!("{}if ({} == {}) {{", if i > 0 { "else " } else { "" }, x, c_i64(*v)));
            let t = json_lit(n);
            self.out(&t);
            self.close("}");
        }
        self.open("else {");
        self.call(&format!("c->h->json_i64(c, {})", x));
        self.close("}");
        Ok(())
    }
    /// The enum a declared answer type names, if it is one (`Op` or `Op?`).
    fn enum_of(&self, want: Option<&crate::lang::RType>) -> Option<usize> {
        match want {
            Some(crate::lang::RType::Name(n)) => self.prog.enums.iter().position(|e| e.name == *n),
            Some(crate::lang::RType::Opt(t)) => self.enum_of(Some(t)),
            _ => None,
        }
    }

    /// Write `j` as JSON (`want`: its declared type, for enums written by name).
    fn json(&mut self, j: &'p Json, env: &mut Vec<Option<CV>>, want: Option<&crate::lang::RType>) -> R<Shape> {
        let want = match want { Some(crate::lang::RType::Opt(t)) if !matches!(**t, crate::lang::RType::Name(_)) => Some(&**t), w => w };
        match j {
            Json::Obj(fields) => {
                let mut text = String::from("{");
                let mut shape = Vec::new();
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 { text.push(','); }
                    text += &json_lit(k); text.push(':');
                    self.out(&text); text.clear();
                    let fw = match want { Some(crate::lang::RType::Obj(fs)) => fs.iter().find(|x| x.0 == *k).map(|x| x.1.clone()), _ => None };
                    shape.push((k.clone(), self.json(v, env, fw.as_ref())?));
                }
                text.push('}'); self.out(&text);
                Ok(Shape::Obj(shape))
            }
            Json::Arr(it, inner) => {
                self.out("[");
                if let (Some(_), Some(n)) = (&it.via, crate::lang::path_of(&it.coll)) {
                    let n = &n;
                    if changes_instance(self.api, n, &[Stmt::Respond(200, (**inner).clone())]) { return no(format!("a loop over {} calls a function that changes {}; change it after the loop", n, n)); }
                }
                let mut elem = Shape::Any;
                let ew = match want { Some(crate::lang::RType::Arr(t)) => Some((**t).clone()), _ => None };
                self.loop_body = Some(LoopBody::Json(inner));
                self.each(it, env, false, |g, env, hits| {
                    g.line(&format!("if ({} > 1) {{ CALL(c->h->out(c, \",\", 1)); }}", hits));
                    elem = g.json(inner, env, ew.as_ref())?;
                    Ok(())
                })?;
                self.out("]");
                Ok(Shape::Arr(Box::new(elem)))
            }
            Json::E(Expr::Call(_, args, CallRes::Builtin(B::Or))) => {
                // or(value that may be a dead reference or null, fallback): write the value, or the fallback
                let (l, end) = (self.tmp("dead"), self.tmp("done"));
                self.line("{");
                let saved = self.or_dead.replace(l.clone());
                let v = self.expr(&args[0], env, Some(&l));
                self.or_dead = saved;
                let v = v?;
                match &v { CV::Txt(t) => self.line(&format!("if (!{}.present) goto {};", t, l)), CV::Any(t) => self.line(&format!("if ({}.tag == 0) goto {};", t, l)), CV::Ref(_, s, _) => self.line(&format!("if ({} == 0) goto {};", s, l)), CV::Null => self.line(&format!("goto {};", l)), _ => {} }
                let a = Shape::of(&v);
                self.write(v)?;
                self.line(&format!("goto {};", end));
                self.line("}");
                self.line(&format!("{}: {{", l));
                let fb = self.expr(&args[1], env, None)?;
                let b = Shape::of(&fb);
                self.write(fb)?;
                self.line("}");
                self.line(&format!("{}: ;", end));
                Ok(Shape::either(a, b))
            }
            Json::E(e) => {
                let v = self.expr(e, env, None)?;
                let s = Shape::of(&v);
                match (self.enum_of(want), &v) { (Some(en), CV::Int(x)) => { let x = x.clone(); self.write_enum(en, &x)?; } _ => self.write(v)? }
                Ok(s)
            }
            Json::Empty | Json::Html => Ok(Shape::Nothing),
        }
    }

    /// Is this endpoint's answer HTML (markup may be written)?
    fn html_answer(&self) -> bool { matches!(&self.ep.resp, crate::lang::RType::Name(n) if n == "html") }
    /// A value written into markup, escaped (`attr`: inside an attribute's double quotes): text with & < > (and ") as entities,
    /// numbers as digits, bools as true / false, null as nothing; the result of a `-> html` call has already written itself.
    fn html_esc(&mut self, v: CV, attr: bool) -> R<()> {
        let mode = attr as u32;
        match v {
            CV::Null => {}
            CV::Lit(s) => { let e = html_escape(&s, attr); self.out(&e); }
            CV::Txt(t) => self.line(&format!("if ({}.present) CALL(c->h->html_txt(c, &{}, {}));", t, t, mode)),
            CV::PlainStr(off) => { let at = self.sub_bytes_at(&CV::PlainStr(off), "0"); let n = self.field_len(&CV::PlainStr(off)); self.call(&format!("c->h->html_at(c, {}, (u32)({}), {})", at, n, mode)); }
            CV::Sub(base, from, count) => { let at = self.sub_bytes_at(&base, &from); self.call(&format!("c->h->html_at(c, {}, (u32)({}), {})", at, count, mode)); }
            CV::StateStr(c, idx, ci) => self.call(&format!("c->h->html_col(c, {}, {}, {}, {})", c, idx, ci, mode)),
            CV::Int(e) => self.call(&format!("c->h->json_i64(c, {})", e)),
            CV::Flt(e) => self.call(&format!("c->h->json_f64(c, {})", e)),
            CV::Bool(e) if e == "1" || e == "0" => self.out(if e == "1" { "true" } else { "false" }),
            CV::Bool(e) => self.line(&format!("CALL({} ? c->h->out(c, \"true\", 4) : c->h->out(c, \"false\", 5));", e)),
            _ => return no("markup takes text, numbers and bools in `{ }` (or a `-> html` function's call)"),
        }
        Ok(())
    }
    /// Text added to the answer as it is (an HTML or plain-text answer).
    fn raw(&mut self, v: CV, what: &str) -> R<()> {
        match v {
            CV::Lit(s) => self.out(&s),
            CV::Txt(t) => self.line(&format!("if ({}.present) CALL(c->h->raw_txt(c, &{}));", t, t)),
            CV::PlainStr(off) => { let at = self.sub_bytes_at(&CV::PlainStr(off), "0"); let n = self.field_len(&CV::PlainStr(off)); self.call(&format!("c->h->raw_at(c, {}, (u32)({}))", at, n)); }
            CV::Sub(base, from, count) => { let at = self.sub_bytes_at(&base, &from); self.call(&format!("c->h->raw_at(c, {}, (u32)({}))", at, count)); }
            CV::StateStr(c, idx, ci) => self.call(&format!("c->h->raw_col(c, {}, {}, {})", c, idx, ci)),
            _ => return no(format!("a Response<{}> answer is text", what)),
        }
        Ok(())
    }
    fn out(&mut self, text: &str) { if !text.is_empty() { self.call(&format!("c->h->out(c, {}, {})", c_lit(text.as_bytes()), text.len())); } }
    fn write(&mut self, v: CV) -> R<()> {
        match v {
            CV::Int(e) => self.call(&format!("c->h->json_i64(c, {})", e)),
            CV::Flt(e) => self.call(&format!("c->h->json_f64(c, {})", e)),
            CV::Bool(e) if e == "1" || e == "0" => self.out(if e == "1" { "true" } else { "false" }),
            CV::Bool(e) => self.line(&format!("CALL({} ? c->h->out(c, \"true\", 4) : c->h->out(c, \"false\", 5));", e)),
            CV::Lit(s) => { let j = json_lit(&s); self.out(&j); }
            CV::Txt(t) => self.line(&format!("CALL({}.present ? c->h->json_txt(c, &{}) : c->h->out(c, \"null\", 4));", t, t)),
            CV::PlainStr(off) => { let at = self.sub_bytes_at(&CV::PlainStr(off), "0"); let n = self.field_len(&CV::PlainStr(off)); self.call(&format!("c->h->json_at(c, {}, (u32)({}))", at, n)); }
            CV::Sub(base, from, count) => { let at = self.sub_bytes_at(&base, &from); self.call(&format!("c->h->json_at(c, {}, (u32)({}))", at, count)); }
            CV::Any(v) => self.call(&format!("c->h->json_val(c, &{})", v)),
            CV::StateStr(c, idx, ci) => self.call(&format!("c->h->json_col(c, {}, {}, {})", c, idx, ci)),
            CV::Null => self.out("null"),
            CV::Ref(t, s, k) if tab(t).is_some() => { self.line(&format!("if ({} == 0) {{ CALL(c->h->out(c, \"null\", 4)); }}", s)); self.line(&format!("else CALL(c->h->json_i64(c, (i64)({})));", k)); }
            CV::Ref(t, s, k) => {
                if self.prog.colls[t].parent.is_some() { return no("an item of a collection inside another's items has no id; write `slot(x)`"); }
                self.line(&format!("if ({} == 0) {{ CALL(c->h->out(c, \"null\", 4)); }}", s));
                self.line(&format!("else CALL(c->h->json_id(c, {}, {}));", t, k));
            }
            CV::Item(c, i) if tab(c).is_some() => self.call(&format!("c->h->json_i64(c, (i64)({}))", i)),
            CV::Item(c, i) if self.prog.colls[c].parent.is_none() && matches!(self.prog.colls[c].kind, Kind::Pool(_)) => self.call(&format!("c->h->json_id(c, {}, {})", c, i)),
            CV::Item(..) | CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..) => self.fail(500, "cannot write this value as JSON"),
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- the C safety profile

/// Check a generated endpoint body against the safety profile. The generator is meant to emit only these shapes; the
/// check makes a generator bug fall back to the interpreter instead of running unsafe C. Rules:
///
/// 1. Memory is read only through `LD_*(offset, index)`; there is no other dereference (no unary `*`), and no
///    indexing except constant-size argument arrays. Nothing writes memory except helpers, so every state write is
///    journaled; assignments go only to local variables (or their fields).
/// 2. Integer arithmetic on language values goes through `ADD`/`SUB`/`MUL`/`DIV`/`MOD`/`NEG`, which check overflow and
///    division before the operation (signed overflow is undefined in C). Index arithmetic in loads and loops is on
///    bounded unsigned values.
/// 3. Every helper is called through `CALL(...)` (its error is checked), `TAIL(...)` (returned), `FAIL` or `DEREF`;
///    `c->h->name` is the only member access. No other functions are called: no libc, no recursion.
/// 4. Control flow: `if`, `for` with a bound (no `while`, `do`, empty-condition `for`, `switch`), `goto` only forward,
///    `return` only of a status (a number or `st`).
/// 5. No preprocessor, no `static`/`extern` (no globals), no `sizeof`, `asm`, `volatile`, unions or casts to pointers.
pub fn check_profile(body: &str) -> Result<(), String> { check_profile_in(body, &[]) }
/// The same for a function's body whose parameters (declared in its prototype) are named in `params`.
pub fn check_profile_in(body: &str, params: &[String]) -> Result<(), String> {
    // the statement-line markers are the only preprocessor lines allowed: `#undef AT_` and `#define AT_ <number>`
    let body: String = body.lines().filter(|l| { let l = l.trim(); l == "#undef AT_" || l.strip_prefix("#define AT_ ").map_or(false, |n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) } == false).map(|l| format!("{}\n", l)).collect();
    let t = ctoks(&body);
    const CALLS: &[&str] = &["if", "for", "CALL", "CALL_OR_DEAD", "TAIL", "FAIL", "ROUND_", "TEST_FAIL_", "ADD", "SUB", "MUL", "DIV", "MOD", "NEG", "UNLIKELY", "BITS_F64", "NADD", "NSUB", "NMUL", "NDIV", "NMOD", "BAND8", "BAND16", "BAND32", "BOR8", "BOR16", "BOR32", "BXOR8", "BXOR16", "BXOR32", "BAND", "BOR", "BXOR", "SHL", "SHR", "SHRL", "NSHRL", "NSHL", "NSHR", "TX_", "SQRT_", "SIN_", "COS_", "TAN_", "ASIN_", "ACOS_", "ATAN_", "SINH_", "COSH_", "TANH_", "LN_", "LOG2_", "LOG10_", "CEIL_", "ROUNDF_", "TRUNC_", "CBRT_", "HYPOT_", "FMOD_", "ATAN2_", "POW_", "EXP_", "FLOOR_", "FABS_", "ABS_", "SAVE_", "COMMIT_", "RANGE_", "ST1_", "ST2_", "ST4_", "ST8_", "MOVE_", "COPY_", "ZERO_", "SCRMOVE", "SCRZERO", "LD_I64", "LD_U64", "LD_I32", "LD_U32", "LD_I16", "LD_U16", "LD_I8", "LD_U8"];
    const MORE_CALLS: [&str; 2] = ["LD_F32", "LD_F64"];
    const FORBIDDEN: [&str; 24] = ["while", "do", "switch", "static", "extern", "register", "volatile", "asm", "__asm", "__asm__", "sizeof", "union", "struct", "typedef", "enum",
        "malloc", "calloc", "realloc", "free", "memcpy", "memmove", "memset", "alloca", "setjmp"];
    const TYPES: [&str; 6] = ["i64", "u32", "int", "Val", "Txt", "double"];
    let is_id = |s: &str| s.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_');
    let is_num = |s: &str| s.chars().next().map_or(false, |c| c.is_ascii_digit());
    let mut declared: Vec<&str> = vec!["st", "b", "c"];
    declared.extend(params.iter().map(|p| p.as_str()));
    let mut owners: Vec<&str> = Vec::new(); // what each open parenthesis belongs to
    let mut labels_seen: Vec<&str> = Vec::new();
    let err = |i: usize, m: &str| Err(format!("{} (near `{}`)", m, t[i.saturating_sub(3)..(i + 3).min(t.len())].join(" ")));
    let mut i = 0;
    while i < t.len() {
        let tok = t[i].as_str();
        let prev = if i > 0 { t[i - 1].as_str() } else { ";" };
        let next = t.get(i + 1).map(|s| s.as_str()).unwrap_or("");
        if tok.starts_with('#') { return err(i, "preprocessor line"); }
        if FORBIDDEN.contains(&tok) || tok == "longjmp" { return err(i, "forbidden word"); }
        if is_id(tok) && TYPES.contains(&tok) && is_id(next) && !TYPES.contains(&next) {
            // a declaration: `T a = ..., b = ...;` declares every name after T or after a top-level comma
            let mut j = i + 1;
            let mut depth = 0;
            loop {
                let x = t.get(j).map(|s| s.as_str()).unwrap_or(";");
                if depth == 0 && is_id(x) && (j == i + 1 || t[j - 1] == ",") { declared.push(&t[j]); }
                match x { "(" | "{" | "[" => depth += 1, ")" | "}" | "]" => depth -= 1, _ => {} }
                if (x == ";" && depth == 0) || depth < 0 { break; }
                j += 1;
            }
        }
        if ["ADD", "SUB", "MUL", "DIV", "MOD", "NEG", "NADD", "NSUB", "NMUL", "NDIV", "NMOD", "BAND8", "BAND16", "BAND32", "BOR8", "BOR16", "BOR32", "BXOR8", "BXOR16", "BXOR32", "BAND", "BOR", "BXOR", "SHL", "SHR", "SHRL", "NSHRL", "NSHL", "NSHR", "ABS_"].contains(&tok) && next == "(" { if let Some(r) = t.get(i + 2) { declared.push(r); } }
        if is_id(tok) && next == "(" {
            let member = prev == "->";
            if member {
                if !(i >= 4 && t[i - 4] == "c" && t[i - 3] == "->" && t[i - 2] == "h") { return err(i, "member call other than c->h->helper"); }
                if !owners.iter().any(|o| ["CALL", "CALL_OR_DEAD", "TAIL"].contains(o)) { return err(i, "helper called without CALL or TAIL"); }
            } else if tok.starts_with("fn_") {
                if !matches!(owners.last(), Some(&"CALL") | Some(&"CALL_OR_DEAD")) { return err(i, "generated function called without CALL"); }
            } else if !CALLS.contains(&tok) && !MORE_CALLS.contains(&tok) && tok != "return" && !TYPES.contains(&tok) {
                return err(i, "call of something other than a helper or profile macro");
            }
        }
        if tok == "->" {
            let ok = (prev == "c" && next == "h" && t.get(i + 2).map(|s| s.as_str()) == Some("->")) || (prev == "h" && i >= 2 && t[i - 2] == "->");
            if !ok { return err(i, "member access other than c->h->helper"); }
        }
        match tok {
            "(" => owners.push(if is_id(prev) && prev != "return" { t[i - 1].as_str() } else { "" }),
            ")" => { owners.pop(); }
            "*" => {
                let unary = ["(", "=", ",", ";", "{", "}", "return", "?", ":", "!", "&&", "||", "==", "!=", "<", ">", "<=", ">=", "+", "-", "*"].contains(&prev);
                let result_out = (next == "ret_v" || next == "ret_s") && t.get(i + 2).map(|s| s.as_str()) == Some("=");
                if (unary || next == ")") && !result_out { return err(i, "pointer dereference or pointer type"); }
            }
            "&" => {
                let unary = ["(", "=", ",", "?", ":"].contains(&prev);
                if unary && !(is_id(next) && declared.contains(&next)) { return err(i, "address of something that is not a local"); }
            }
            "[" => {
                let ok = prev.starts_with("argv") && is_num(next) && t.get(i + 2).map(|s| s.as_str()) == Some("]");
                if !ok { return err(i, "indexing (only constant-size argument arrays are allowed)"); }
            }
            "=" => {
                let argv_decl = prev == "]" && i >= 4 && t[i - 4].starts_with("argv") && i >= 5 && (t[i - 5] == "Val" || t[i - 5] == "u32");
                let ok = argv_decl || is_id(prev) && (i < 2 || t[i - 2] != "->") && (i < 2 || t[i - 2] != "." || (i >= 3 && is_id(&t[i - 3]) && (i < 4 || (t[i - 4] != "." && t[i - 4] != "->"))));
                if !ok { return err(i, "assignment to something other than a local"); }
            }
            "+=" | "-=" | "*=" | "/=" | "%=" | "<<=" | ">>=" | "&=" | "|=" | "^=" => return err(i, "compound assignment"),
            "++" | "--" => if !(is_id(prev) || is_id(next)) { return err(i, "increment of something other than a local"); },
            "goto" => { if labels_seen.contains(&next) { return err(i, "backward goto"); } }
            "return" => { if !((is_num(next) || next == "st") && t.get(i + 2).map(|s| s.as_str()) == Some(";")) { return err(i, "return of something other than a status"); } }
            "for" => {
                // for ( init ; cond ; step ): the condition must not be empty
                let mut j = i + 2;
                while j < t.len() && t[j] != ";" { j += 1; }
                if t.get(j + 1).map(|s| s.as_str()) == Some(";") { return err(i, "for loop without a bound"); }
            }
            ":" => { if is_id(prev) && i >= 2 && [";", "{", "}"].contains(&t[i - 2].as_str()) { labels_seen.push(&t[i - 1]); } }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// C tokens: identifiers, numbers, string literals (kept whole), and punctuation.
fn ctoks(s: &str) -> Vec<String> {
    let c: Vec<char> = s.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() { i += 1; }
        else if ch == '/' && c.get(i + 1) == Some(&'/') { while i < c.len() && c[i] != '\n' { i += 1; } }
        else if ch == '/' && c.get(i + 1) == Some(&'*') { i += 2; while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') { i += 1; } i += 2; }
        else if ch == '#' { let st = i; while i < c.len() && c[i] != '\n' { i += 1; } out.push(c[st..i].iter().collect()); }
        else if ch == '"' {
            let st = i; i += 1;
            while i < c.len() && c[i] != '"' { if c[i] == '\\' { i += 1; } i += 1; }
            i += 1; out.push(c[st..i.min(c.len())].iter().collect());
        }
        else if ch.is_alphanumeric() || ch == '_' { let st = i; while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') { i += 1; } out.push(c[st..i].iter().collect()); }
        else {
            let three: String = c[i..(i + 3).min(c.len())].iter().collect();
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            if ["<<=", ">>="].contains(&three.as_str()) { out.push(three); i += 3; }
            else if ["->", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "<<", ">>", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^="].contains(&two.as_str()) { out.push(two); i += 2; }
            else { out.push(ch.to_string()); i += 1; }
        }
    }
    out
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    /// A macro name defined twice in the prelude where both definitions can be in force (the same `#if` branch, or one
    /// of them outside every branch) silently replaces the first: a built-in's `ROUND_` once replaced the loop budget's.
    #[test]
    fn the_prelude_defines_each_macro_once_per_branch() {
        let (mut stack, mut next, mut defs): (Vec<String>, usize, Vec<(String, Vec<String>)>) = (vec![], 0, vec![]);
        for l in PRELUDE.lines() {
            let t = l.trim_start();
            if t.starts_with("#if") { next += 1; stack.push(format!("if{}", next)); }
            else if t.starts_with("#else") || t.starts_with("#elif") { if let Some(top) = stack.last_mut() { *top = top.replace("if", "else"); } }
            else if t.starts_with("#endif") { stack.pop(); }
            else if let Some(rest) = t.strip_prefix("#define ") {
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                for (n, st) in &defs {
                    assert!(*n != name || !(st.starts_with(&stack) || stack.starts_with(st)), "the prelude defines `{}` twice where both can be in force", name);
                }
                defs.push((name, stack.clone()));
            }
        }
    }

    #[test]
    fn each_endpoint_zeroes_only_the_request_fields_it_names() {
        // `small` names work.a (through a function, and by assignment), `large` reads work.big (an array, 8000 bytes): each finds
        // zeroed only what it names
        let src = "struct Work { a: i64; big: i64[1000]; s: utf8<64>; }\nrequest work: Work;\nstruct A { n: i64; }\nstate app: A;\n\
            fn bump() { work.a += 1; }\n\
            @get(\"/small\") fn small() -> Response<{ a: i64 }> { bump(); work.a += 1; return ok({ a: work.a }); }\n\
            @get(\"/large\") fn large() -> Response<{ n: i64 }> { work.big[3] = 7; work.s = \"x\"; return ok({ n: work.big[3] }); }\n";
        let (state, api) = crate::source::load(src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        let (ro, _) = prog.request.unwrap();
        let w = prog.structs.iter().position(|x| x.name == "Work").unwrap();
        let at = |f: &str| { let k = prog.structs[w].fields.iter().position(|x| x.0 == f).unwrap(); ro + prog.structs[w].offs[k] };
        assert_eq!(g.zeros[0], Some(vec![(at("a"), at("a") + 8)]));
        let z = g.zeros[1].clone().unwrap();
        let total: usize = z.iter().map(|(a, b)| b - a).sum();
        assert!(total >= 8000 + 68 && total < 8200 && z.iter().all(|(a, _)| *a >= at("big")), "{:?}", z);
    }

    /// An endpoint is read-only (a reader may run it) when nothing in its code changes state: plain state, a collection and a table do; the request struct
    /// and the text area are the request's own. `reads` is what the route table tells the runtime.
    #[test]
    fn endpoints_that_only_read_are_marked_for_readers() {
        let src = "struct Work { a: i64; t: utf8<32>; }
request work: Work;
struct Node { v: i64; }
struct A { n: i64; buf: u8[8]; xs: pool<Node, 4>; s: utf8<16>; }
state app: A;
            fn helper() -> i64 { return app.n + 1; }
            @get(\"/read\") fn read() -> Response<{ n: i64 }> { return ok({ n: app.n + app.buf[2] + app.xs.count() }); }
            @get(\"/call\") fn call() -> Response<{ n: i64 }> { return ok({ n: helper() }); }
            @get(\"/scratch\") fn scratch() -> Response<{ n: i64 }> { work.a = app.n; work.t = \"x\"; return ok({ n: work.a }); }
            @get(\"/plain\") fn plain() -> Response<{ n: i64 }> { app.n = 1; return ok({ n: app.n }); }
            @get(\"/arr\") fn arr() -> Response<{ n: i64 }> { app.buf[1] = 3; return ok({ n: 1 }); }
            @get(\"/add\") fn add() -> Response<{ n: i64 }> { let p = app.xs.add(); p.v = 1; return ok({ n: 1 }); }
            @get(\"/text\") fn text() -> Response<{ n: i64 }> { app.s = \"hi\"; return ok({ n: 1 }); }
            @get(\"/item\") fn item() -> Response<{ n: i64 }> { foreach p in app.xs { p.v = 2; } return ok({ n: 1 }); }
";
        let (state, api) = crate::source::load(src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        let want = [("read", true), ("call", true), ("scratch", true), ("plain", false), ("arr", false), ("add", false), ("text", false), ("item", false)];
        for (k, (name, reads)) in want.iter().enumerate() { assert_eq!(g.reads[k], *reads, "endpoint {}", name); }
    }

    /// A call of a function declared for a type counts as what the function it reaches does: a GET that calls one that writes is not a reader, one that calls only readers is, whichever types share the name.
    #[test]
    fn receiver_functions_count_as_what_they_do_for_readers() {
        let src = "struct Node { v: i64; }
struct Leaf { w: i64; }
struct A { xs: pool<Node, 4>; ls: pool<Leaf, 4>; }
state app: A;
            fn bump(self: write Node) { self.v += 1; }
            fn bump(self: write Leaf) -> i64 { return self.w; }
            fn peek(self: Node) -> i64 { return self.v; }
            @get(\"/peek\") fn peek() -> Response<{ n: i64 }> { var t = 0; foreach n in app.xs { t += n.peek(); } return ok({ n: t }); }
            @get(\"/leaf\") fn leaf() -> Response<{ n: i64 }> { var t = 0; foreach l in app.ls { t += l.bump(); } return ok({ n: t }); }
            @get(\"/bump\") fn bump() -> Response<{ n: i64 }> { foreach n in app.xs { n.bump(); } return ok({ n: 1 }); }
";
        let (state, api) = crate::source::load(src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        // `bump` writes for a Node and only reads for a Leaf: what a GET does is what its generated code does, so the call follows the function its receiver's type names
        let want = [("peek", true), ("leaf", true), ("bump", false)];
        for (k, (name, reads)) in want.iter().enumerate() { assert_eq!(g.reads[k], *reads, "endpoint {}", name); }
    }

    /// Struct locals live in the request's scratch area, which is the request's own: an endpoint that only builds locals (from state, from other locals, through a function that writes
    /// them) is still a reader; one that copies a struct (or a literal) into the state, or writes the state's struct through a function, is not. Both directions.
    #[test]
    fn struct_locals_are_the_requests_own_for_readers() {
        let src = "struct V { x: i64; y: i64; }
struct A { p: V; q: V logged; n: i64; }
state app: A;
            fn bump(p: write V) { p.x += 1; }
            fn shift(self: write V, d: i64) { self.x += d; self.y += d; }
            @get(\"/locals\") fn locals() -> Response<{ n: i64 }> { var a = V { ..app.p }; var b = V { ..app.q }; b = a; a.x += 2; let c = V { ..b, y: 4 }; return ok({ n: a.x + c.y }); }
            @get(\"/through\") fn through() -> Response<{ n: i64 }> { var a = V { ..app.p }; bump(a); a.shift(3); return ok({ n: a.x }); }
            @get(\"/into\") fn into() -> Response<{ n: i64 }> { var a = V { ..app.p }; app.q = a; return ok({ n: 1 }); }
            @get(\"/literal\") fn literal() -> Response<{ n: i64 }> { app.p = V { x: 1, y: 2 }; return ok({ n: 1 }); }
            @get(\"/state_through\") fn state_through() -> Response<{ n: i64 }> { var a = V { ..app.p }; bump(app.p); return ok({ n: a.x }); }
            @get(\"/plain\") fn plain() -> Response<{ n: i64 }> { return ok({ n: app.p.x }); }
";
        let g = gen_of(src);
        assert!(g.scratch, "an endpoint with struct locals uses the scratch area");
        let want = [("locals", true), ("through", true), ("into", false), ("literal", false), ("state_through", false), ("plain", true)];
        for (k, (name, reads)) in want.iter().enumerate() { assert_eq!(g.reads[k], *reads, "endpoint {}", name); }
        // a program with no struct value has no scratch area: its layout and totals are what they were
        let g = gen_of("struct V { x: i64; }\nstruct A { p: V; }\nstate app: A;\n@get(\"/a\") fn a() -> Response<{ n: i64 }> { return ok({ n: app.p.x }); }\n");
        assert!(!g.scratch);
    }

    /// A copy into the state counts as the stores it is for the whole-program ranges of fields (a read through an index may drop its test only when every store is proven small);
    /// stores into a local never count; a copy into one place does not widen another's field. Both directions.
    #[test]
    fn struct_copies_count_as_stores_for_field_ranges() {
        let head = "struct P { v: i64; }\nstruct A { p: P; q: P; buf: i64[8]; }\nstate app: A;\n";
        let read = "@get(\"/r\") fn r() -> Response<{ n: i64 }> { return ok({ n: app.buf[app.p.v] }); }\n";
        let small = "@post(\"/w\") fn w() -> Response<{}> { app.p.v = 3; return ok({}); }\n";
        let tested = |more: &str| gen_of(&format!("{}{}{}{}", head, small, more, read)).src.contains("c->h->elem(");
        assert!(!tested(""), "every store to p.v is 3: the index is inside buf");
        // a copy of a local, of a state struct, or of a literal into app.p could hold anything: the test stays
        assert!(tested("@post(\"/c\") fn c(x: i64) -> Response<{}> { var l = P { ..app.q }; l.v = x; app.p = l; return ok({}); }\n"), "a local copied into p");
        assert!(tested("@post(\"/c\") fn c() -> Response<{}> { app.p = app.q; return ok({}); }\n"), "a state struct copied into p (q is never proven small)");
        assert!(tested("@post(\"/c\") fn c(x: i64) -> Response<{}> { app.p = P { v: x }; return ok({}); }\n"), "a literal copied into p");
        // writes to locals, and copies into another place, leave p.v at its proven range
        assert!(!tested("@post(\"/c\") fn c(x: i64) -> Response<{}> { var l = P { ..app.p }; l.v = x; l.v = 1000; return ok({}); }\n"), "stores into a local are not stores into the state");
        assert!(!tested("@post(\"/c\") fn c(x: i64) -> Response<{}> { var l = P { ..app.p }; l.v = x; app.q = l; return ok({}); }\n"), "a copy into q does not change what p.v holds");
        assert!(!tested("@post(\"/c\") fn c() -> Response<{}> { let l = P { ..app.p }; var m = P { ..l }; m = l; return ok({}); }\n"), "copies between locals store nothing in the state");
    }

    /// A range-declared field keeps its check when a literal or a field store of a local writes it, unless the value is proven inside; a copy needs none (the source holds the range).
    #[test]
    fn struct_locals_keep_declared_ranges() {
        let head = "struct G { n: i64 in 0..100; }\nstruct A { g: G; }\nstate app: A;\n";
        let msg = "`n` must be in 0..100";
        let lit = |x: &str| gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in {}) -> Response<{{}}> {{ let l = G {{ n: x }}; app.g = l; return ok({{}}); }}\n", head, x)).src.contains(msg);
        assert!(lit("0..500"), "a literal field outside the range is tested");
        assert!(!lit("0..50"), "a literal field inside the range is proven");
        let store = |x: &str| gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in {}) -> Response<{{}}> {{ var l = app.g; l.n = x; app.g = l; return ok({{}}); }}\n", head, x)).src.contains(msg);
        assert!(store("0..500") && !store("0..50"));
        let copy_only = gen_of(&format!("{}@post(\"/a\") fn a() -> Response<{{}}> {{ var l = app.g; app.g = l; let m = app.g; return ok({{}}); }}\n", head));
        assert!(!copy_only.src.contains(msg), "a copy cannot leave the range: no test");
    }

    /// Locals of blocks that do not overlap share their scratch place; locals alive together do not; a C function generated apart has places of its own above everything an endpoint uses.
    #[test]
    fn struct_locals_get_places_by_scope() {
        let zeros = |s: &str| -> Vec<usize> {
            let mut v = Vec::new();
            for l in s.lines() { if let Some(r) = l.trim().strip_prefix("SCRZERO(") { v.push(r.split("ULL").next().unwrap().parse::<usize>().unwrap()); } }
            v
        };
        let head = "struct V { x: i64; y: i64; }\nstruct A { p: V; n: i64; }\nstate app: A;\n";
        let g = gen_of(&format!("{}@post(\"/a\") fn a() -> Response<{{ n: i64 }}> {{ var t = V {{}}; if app.n > 0 {{ let u = V {{ x: 1, y: 2 }}; t.x = u.x; }} else {{ let w = V {{ x: 3, y: 4 }}; t.y = w.y; }} return ok({{ n: t.x }}); }}\n", head));
        let z = zeros(&g.src);
        // t, then u and w (one place for both), then the hidden temporaries of the literals (also given back at the block's end)
        let t = z[0];
        assert!(z.iter().all(|o| (*o - t) % 8 == 0) && z.iter().filter(|o| **o == t).count() == 1, "t's place is its own: {:?}", z);
        let mut uniq = z.clone(); uniq.sort(); uniq.dedup();
        assert!(uniq.len() < z.len(), "blocks that do not overlap reuse places: {:?}", z);
    }

    /// A function too big to inline is a C function of its own: its struct locals have places of their own, apart from its caller's and from every other such function's, so a call in the
    /// middle of the caller's locals (or one such function calling another) cannot overwrite them; a caller's local passes to it as a view.
    #[test]
    fn struct_locals_of_functions_compiled_apart_have_places_of_their_own() {
        let body: String = (0..400).map(|_| "    p.x += 1.0;\n    t.y += 0.5;\n").collect();
        let src = format!("struct V {{ x: f64; y: f64; z: f64; }}\nstruct A {{ pos: V; b: V; }}\nstate app: A;\nfn big(p: write V, n: f64) -> f64 {{ var t = V {{ x: 1.0, y: 2.0, z: 3.0 }};\n{}    return t.x + t.y + n; }}\n\
            fn big2(p: write V) -> f64 {{ var t = V {{ ..p }};\n{}    let r = big(t, 1.0); return t.z + r; }}\n\
            @post(\"/a\") fn a() -> Response<{{ n: f64 }}> {{ var l = V {{ ..app.pos }}; let k = V {{ x: 5.0, y: 6.0, z: 7.0 }}; let a = big(l, 1.0); let c = big2(l); return ok({{ n: a + c + k.z }}); }}\n", body, body);
        let g = gen_of(&src);
        let mut places: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
        let mut cur = String::new();
        for l in g.src.lines() {
            if (l.starts_with("static int ") || l.starts_with("int ")) && l.ends_with('{') { cur = l.split('(').next().unwrap().to_string(); }
            if let Some(r) = l.trim().strip_prefix("SCRZERO(") { places.entry(cur.clone()).or_default().push(r.split("ULL").next().unwrap().parse().unwrap()); }
        }
        let apart: Vec<&Vec<usize>> = places.iter().filter(|(k, _)| k.contains("fn_")).map(|x| x.1).collect();
        assert!(apart.len() >= 2, "the big functions are compiled apart: {:?}", places);
        let own: Vec<usize> = places.iter().filter(|(k, _)| k.contains(" ep")).flat_map(|x| x.1.clone()).collect();
        let mut all: Vec<usize> = apart.iter().flat_map(|x| x.iter().copied()).collect();
        let n = all.len();
        all.sort(); all.dedup();
        assert_eq!(all.len(), n, "no two C functions share a place: {:?}", places);
        assert!(!own.is_empty() && own.iter().all(|o| *o < *all.first().unwrap()), "the endpoint's places are below those of the functions: {:?}", places);
    }

    fn gen_of(src: &str) -> Generated {
        let (state, api) = crate::source::load(src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        g
    }

    /// A function given a plain struct counts as what its body does: one that writes through the parameter makes a GET not a reader, one that only reads leaves it one.
    #[test]
    fn plain_struct_parameters_count_as_what_they_do_for_readers() {
        let head = "struct P { v: i64; }\nstruct A { p: P; q: P; }\nstate app: A;\nfn peek(p: P) -> i64 { return p.v; }\nfn bump(p: write P) { p.v += 1; }\nfn look(self: P) -> i64 { return self.v; }\nfn poke(self: write P) { self.v = 2; }\n";
        let g = gen_of(&format!("{}@get(\"/a\") fn a() -> Response<{{ n: i64 }}> {{ return ok({{ n: peek(app.p) + app.q.look() }}); }}\n@get(\"/b\") fn b() -> Response<{{ n: i64 }}> {{ bump(app.p); return ok({{ n: 1 }}); }}\n@get(\"/c\") fn c() -> Response<{{ n: i64 }}> {{ app.q.poke(); return ok({{ n: 1 }}); }}\n", head));
        let want = [("a", true), ("b", false), ("c", false)];
        for (k, (name, reads)) in want.iter().enumerate() { assert_eq!(g.reads[k], *reads, "endpoint {}", name); }
    }

    /// A range-declared field written through a parameter keeps its check unless the value is proven inside the range; a field without a range that is only ever
    /// given small values lets an index into a larger array go unchecked, until a function writes it through a parameter with a value nothing is known of.
    #[test]
    fn plain_struct_parameters_are_seen_by_the_range_analyses() {
        let head = "struct P { n: i64 in 0..100; v: i64; }\nstruct A { p: P; buf: i64[8]; }\nstate app: A;\nfn set_n(p: write P, x: i64) { p.n = x; }\nfn set_v(p: write P, x: i64) { p.v = x; }\n";
        let msg = "`n` must be in 0..100";
        // a value the endpoint's own range proves inside: no check; an unbounded one: checked
        let g = gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in 0..50) -> Response<{{}}> {{ set_n(app.p, x); return ok({{}}); }}\n", head));
        assert!(!g.src.contains(msg), "a value inside the range needs no check");
        let g = gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in 0..500) -> Response<{{}}> {{ set_n(app.p, x); return ok({{}}); }}\n", head));
        assert!(g.src.contains(msg), "a value outside the range is still checked through a parameter");
        // the whole-program range of `v`: stores of 0..3 only, so `buf[app.p.v]` needs no index test ...
        let read = "@get(\"/r\") fn r() -> Response<{ n: i64 }> { return ok({ n: app.buf[app.p.v] }); }\n";
        let g = gen_of(&format!("{}@post(\"/w\") fn w() -> Response<{{}}> {{ app.p.v = 3; return ok({{}}); }}\n{}", head, read));
        assert!(!g.src.contains("c->h->elem("), "every store to `v` is 3, so the index is inside buf");
        // ... and the same program with a store through a parameter of an unbounded value keeps the test
        let g = gen_of(&format!("{}@post(\"/w\") fn w() -> Response<{{}}> {{ app.p.v = 3; return ok({{}}); }}\n@post(\"/s/{{x}}\") fn s(x: i64) -> Response<{{}}> {{ set_v(app.p, x); return ok({{}}); }}\n{}", head, read));
        assert!(g.src.contains("c->h->elem("), "a store through a parameter widens the range of the field");
        // a proven small value written through a parameter keeps the range small
        let g = gen_of(&format!("{}@post(\"/w\") fn w() -> Response<{{}}> {{ app.p.v = 3; return ok({{}}); }}\n@post(\"/s/{{x}}\") fn s(x: i64 in 0..7) -> Response<{{}}> {{ set_v(app.p, x); return ok({{}}); }}\n{}", head, read));
        assert!(!g.src.contains("c->h->elem("), "stores of 0..7 through a parameter keep the index inside buf");
    }

    /// Generic instances are ordinary state to the range analyses, in both directions: a declared range is its own per instance (`in 0..N`), and the whole-program
    /// range of a field is made of the stores into that instance only.
    #[test]
    fn generic_instances_are_separate_state_to_the_range_analyses() {
        // `n: i64 in 0..N`: the same function copied for Box<100> and Box<20>; a value in 0..50 is proven inside 0..100 and not inside 0..20
        let head = "struct Box<N: count> { n: i64 in 0..N; }\nstruct A { big: Box<100>; small: Box<20>; }\nstate app: A;\nfn set_n(self: write Box<N>, x: i64) { self.n = x; }\n";
        let both = gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in 0..50) -> Response<{{}}> {{ app.big.set_n(x); app.small.set_n(x); return ok({{}}); }}\n", head));
        assert!(both.src.contains("`n` must be in 0..20") && !both.src.contains("`n` must be in 0..100"), "Box<100> proves 0..50 inside, Box<20> keeps its test");
        let big = gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in 0..50) -> Response<{{}}> {{ app.big.set_n(x); return ok({{}}); }}\n", head));
        assert!(!big.src.contains("`n` must be in 0..100"));
        let wide = gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in 0..500) -> Response<{{}}> {{ app.big.set_n(x); return ok({{}}); }}\n", head));
        assert!(wide.src.contains("`n` must be in 0..100"), "a value outside the instance's range is checked");
        // whole-program field ranges: `big.v` only ever holds 3, `small.v` takes any value; each read is judged by its own instance's stores
        let head = "struct Cell<N: count> { v: i64; buf: i64[N]; }\nstruct A { big: Cell<8>; small: Cell<4>; }\nstate app: A;\nfn put(self: write Cell<N>, x: i64) { self.v = x; }\n";
        let w = "@post(\"/w\") fn w() -> Response<{}> { app.big.put(3); return ok({}); }\n";
        let ws = "@post(\"/s/{x}\") fn s(x: i64) -> Response<{}> { app.small.put(x); return ok({}); }\n";
        let rb = "@get(\"/rb\") fn rb() -> Response<{ n: i64 }> { return ok({ n: app.big.buf[app.big.v] }); }\n";
        let rs = "@get(\"/rs\") fn rs() -> Response<{ n: i64 }> { return ok({ n: app.small.buf[app.small.v] }); }\n";
        assert!(!gen_of(&format!("{}{}{}", head, w, rb)).src.contains("c->h->elem("), "every store to big.v is 3: inside buf[8]");
        assert!(!gen_of(&format!("{}{}{}{}", head, w, ws, rb)).src.contains("c->h->elem("), "an unbounded store to small.v does not widen big.v");
        assert!(gen_of(&format!("{}{}{}{}", head, w, ws, rs)).src.contains("c->h->elem("), "small.v is unbounded: its index is tested");
    }

    /// How many overflow-checked operations (`ADD`, `SUB`, `MUL`) an endpoint's generated code keeps, for a body written in a program with one range-typed field and one free field.
    fn checked_ops(body: &str) -> usize {
        let src = format!("struct A {{ n: i64 in 0..1000; big: i64; }}\nstate app: A;\n@get(\"/t\") fn t() -> Response<{{ x: i64 }}> {{ {} }}\n", body);
        let (state, api) = crate::source::load(&src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        let code = &g.src[g.src.find("int ep0(Ctx").unwrap()..];
        code.lines().map(|l| l.trim_start()).filter(|l| ["ADD(", "SUB(", "MUL("].iter().any(|m| l.starts_with(m))).count()
    }

    /// What the compiler knows of a variable as the code runs (its initial value, its assignments, `if` joins, a counted loop's counter, a `while` condition) decides which
    /// overflow tests remain. Each case that must keep its test is the way a wrong proof would show.
    #[test]
    fn variable_ranges_drop_only_tests_that_cannot_fire() {
        // gone: a counter stepped by 1 in a loop of at most 1000 rounds; in a `while` (its rounds are budgeted); an `if` whose arms both give small values; a bound from a condition
        assert_eq!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { c = c + 1; } return ok({ x: c });"), 0);
        assert_eq!(checked_ops("var c = 0; var i = 0; while i < app.n { c = c + 1; i = i + 1; } return ok({ x: c });"), 0);
        assert_eq!(checked_ops("var c = 0; if app.n > 5 { c = 100; } else { c = 7; } let y = c + 1; return ok({ x: y });"), 0);
        assert_eq!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { if i > 3 { c = c + 1; } else { c = c - 1; } } return ok({ x: c });"), 0);
        // kept: the loop's rounds are not bounded; the step is large; the counter is also set or scaled; it is stepped inside an inner loop; the value is unknown on one way
        assert!(checked_ops("var c = 0; foreach i in 0..app.big { c = c + 1; } return ok({ x: c });") > 0);
        assert!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { c = c + 2097152; } return ok({ x: c });") > 0);
        assert!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { c = c + 1; if i == 5 { c = app.big; } } return ok({ x: c });") > 0);
        assert!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { c = c * 2 + 1; } return ok({ x: c });") > 0);
        assert!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { foreach j in 0..app.n - 1 { c = c + 1; } } return ok({ x: c });") > 0);
        assert!(checked_ops("var c = 0; if app.n > 5 { c = app.big; } let y = c + 1; return ok({ x: y });") > 0);
        // a start close to the end of the i64 range stays checked: ten rounds can pass it
        assert!(checked_ops("var c = 9223372036854775800; foreach i in 0..9 { c = c + 1; } return ok({ x: c });") > 0);
    }

    /// The endpoint code generated for `api` over `state`, from its first endpoint on.
    fn endpoint_code(state: &str, api: &str) -> String {
        let src = format!("{}\n{}\n", state, api);
        let (state, api) = crate::source::load(&src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind_with(&mut api, &|n| prog.global(n), &prog.enum_table(), &prog.const_table()).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        g.src[g.src.find("int ep0(Ctx").unwrap()..].to_string()
    }

    /// What the compiler can show of an index decides whether its test stays; a quotient has the range its operands allow, so an index computed with a division is proven or not by them.
    #[test]
    fn index_tests_follow_the_ranges_of_what_is_computed() {
        let state = "struct A { n: i64 in 0..1000; big: i64; xs: u8[10]; }\nstate app: A;";
        let tests = |body: &str| endpoint_code(state, &format!("@get(\"/t\") fn t() -> Response<{{ x: i64 }}> {{ {} }}", body)).matches("RANGE_(").count();
        assert_eq!(tests("let i = app.n / 200; return ok({ x: app.xs[i] });"), 0);        // 0..5
        assert_eq!(tests("let i = (app.n + 199) / 200; return ok({ x: app.xs[i] });"), 0); // 0..5
        assert!(tests("let i = app.n / 50; return ok({ x: app.xs[i] });") > 0);             // 0..20: can be out
        assert!(tests("let i = app.big / 2; return ok({ x: app.xs[i] });") > 0);           // nothing known
        assert!(tests("let i = (app.n - 500) / 100; return ok({ x: app.xs[i] });") > 0);   // -5..5: can be negative
    }

    /// A pool nothing in the program removes from has no dead slot below its high-water mark, so reaching an item by number tests the range and the mark only; any removal anywhere
    /// brings the live-bit test back.
    #[test]
    fn a_pool_nothing_removes_from_is_reached_without_a_live_bit_test() {
        let state = "struct N { v: i64; }\nstruct A { n: i64 in 0..7; nodes: pool<N, 8>; }\nstate app: A;";
        let reach = "@get(\"/t\") fn t() -> Response<{ x: i64 }> { let p = app.nodes[app.n]; return ok({ x: p.v }); }\n";
        let add = "@post(\"/add\") fn add() -> Response<{ x: i64 }> { let p = app.nodes.add(); p.v = 1; return ok({ x: 1 }); }\n";
        let remove = "@post(\"/rm\") fn rm() -> Response<{ x: i64 }> { foreach p in app.nodes { app.nodes.remove(p); } return ok({ x: 1 }); }\n";
        let live_tests = |api: &str| endpoint_code(state, api).matches(">> 6)) >> ").count();
        assert_eq!(live_tests(&format!("{}{}", reach, add)), 0);
        assert!(live_tests(&format!("{}{}{}", reach, add, remove)) > 0);
        assert!(live_tests(&format!("{}{}", remove, reach)) > 0); // the removal is compiled before the reach: still seen
    }

    /// `add` to a plain pool kept out of the undo log is done in the endpoint (the runtime's helper stays as the fallback for a freed slot or a full pool); a pool in the undo log, a
    /// checked pool (generations) and a pool with nested collections keep the helper alone.
    #[test]
    fn adding_to_a_plain_unlogged_pool_is_done_in_line() {
        let add = "@post(\"/add\") fn add() -> Response<{ x: i64 }> { let p = app.nodes.add(); p.v = 1; return ok({ x: 1 }); }";
        let inline = |state: &str| { let c = endpoint_code(state, add); (c.contains("ZERO_("), c.contains("->alloc(")) };
        assert_eq!(inline("struct N { v: i64; }\nstruct A { nodes: pool<N, 8>; }\nstate app: A;"), (true, true));          // in line, helper as the fallback
        assert_eq!(inline("struct N { v: i64; }\nstruct A { nodes: pool<N, 8>; }\nstate app: A logged;"), (false, true));   // in the undo log: the helper
        assert_eq!(inline("struct N { v: i64; }\nstruct A { nodes: pool<N, 8> checked; }\nstate app: A;"), (false, true));  // generations: the helper
        assert_eq!(inline("struct K { w: i64; }\nstruct N { v: i64; ks: list<K, 2>; }\nstruct A { nodes: pool<N, 8>; }\nstate app: A;"), (false, true)); // nested collections: the helper
    }

    /// A variable that a loop moves by amounts that are not constants still has an interval at the head of the loop: its first one, widened by what the loop's assignments give, until
    /// the guess holds. A walking offset that only grows (`off = off + step` with `step` from an unsigned byte) and is bounded by the loop's condition needs no index test; one that can
    /// go down, or grow by an unknown amount, keeps it.
    #[test]
    fn loop_carried_variables_have_intervals_from_a_guess_that_holds() {
        let state = "struct A { n: i64 in 0..1000; step: u8; big: i64; xs: u8[2000]; }\nstate app: A;";
        let tests = |body: &str| endpoint_code(state, &format!("@get(\"/t\") fn t() -> Response<{{ x: i64 }}> {{ {} }}", body)).matches("RANGE_(").count();
        assert_eq!(tests("var off = 0; var s = 0; while off < app.n { s = s + app.xs[off + 1]; off = off + app.step; } return ok({ x: s });"), 0);
        assert!(tests("var off = 0; var s = 0; while off < app.n { s = s + app.xs[off + 1]; off = off - app.step; } return ok({ x: s });") > 0);
        assert!(tests("var off = 0; var s = 0; while off < app.n { s = s + app.xs[off + 1]; off = off + app.big; } return ok({ x: s });") > 0);
        assert!(tests("var off = 0; var s = 0; while off < app.big { s = s + app.xs[off + 1]; off = off + app.step; } return ok({ x: s });") > 0);
    }

    /// A bounds test that passes tells the compiler where the index is for the rest of that code: stepping it afterwards cannot overflow. Without an access there is nothing to tell.
    #[test]
    fn a_passed_bounds_test_bounds_the_index() {
        let state = "struct A { n: i64 in 0..1000; big: i64; xs: u8[2000]; }\nstate app: A;";
        let checked = |body: &str| endpoint_code(state, &format!("@get(\"/t\") fn t() -> Response<{{ x: i64 }}> {{ {} }}", body)).lines().map(|l| l.trim_start()).filter(|l| ["ADD(", "SUB(", "MUL("].iter().any(|m| l.starts_with(m))).count();
        assert_eq!(checked("var i = app.big; let x = app.xs[i]; i = i + 1; i = i - 3; return ok({ x: i + x });"), 0);
        assert!(checked("var i = app.big; i = i + 1; return ok({ x: i });") > 0);
        assert!(checked("var i = app.big; i = i + 1; let x = app.xs[i]; return ok({ x: x });") > 0); // the step came before the access
        assert!(checked("var i = app.big; let h = app.n > 5 && app.xs[i] == 1; i = i + 1; return ok({ x: i });") > 0); // the test may not have run: it says nothing of the code after
        assert!(checked("var i = app.big; if app.n > 5 { let x = app.xs[i]; } i = i + 1; return ok({ x: i });") > 0);
    }

    /// What every store into an array put there is assumed of a read, but never wider than what the element type itself gives: a `u8` array that was fed values
    /// of -255..255 (each store is tested, so what is read is still 0..255) keeps the 16-bit shape of `lo | (hi << 8)`; an `i16` array fed only 0..100 keeps that, a narrower range than its type.
    #[test]
    fn a_range_seen_in_stores_never_widens_the_element_type() {
        let reader = "@get(\"/r\") fn r() -> Response<{ x: i64 }> { return ok({ x: app.xs[0] | (app.xs[1] << 8) }); }";
        let code = |state: &str, store: &str| endpoint_code(state, &format!("@post(\"/w\") fn w() -> Response<{{ x: i64 }}> {{ {} return ok({{ x: 0 }}); }}\n{}", store, reader));
        let wide_state = "struct A { n: i64 in -255..255; xs: u8[2]; }\nstate app: A;";
        assert!(code(wide_state, "app.xs[0] = app.n; app.xs[1] = app.n;").contains("BOR16("), "a u8 element read stays 0..255");
        let i16_state = "struct A { n: i64 in 0..100; m: i64 in -5..5; xs: i16[2]; }\nstate app: A;";
        assert!(code(i16_state, "app.xs[0] = app.n; app.xs[1] = app.n;").contains("BOR16("), "an i16 array that only ever held 0..100 reads as 0..100");
        assert!(!code(i16_state, "app.xs[0] = app.m; app.xs[1] = app.m;").contains("BOR16("), "an i16 array that held -5..5 may be negative: no 16-bit or");
    }

    /// A constant is exactly its value: code that uses one is the code written with the number (so a loop over a constant range is bounded, and its checks removed, as for a literal).
    #[test]
    fn a_constant_is_its_value_to_the_analyses() {
        let ep = |range: &str, bound: &str| format!("@get(\"/t\") fn t() -> Response<{{ x: i64 }}> {{ var s = 0; foreach i in 0..{} {{ app.xs[i] = 1; s += app.xs[i] + {}; }} return ok({{ x: s }}); }}", range, bound);
        let lit = |range: &str| endpoint_code("struct A { xs: u8[4]; } state app: A;", &ep(range, "7"));
        let cst = |range: &str| endpoint_code("const L { N = 4; K: i64 = L.N + 3; } struct A { xs: u8[L.N]; } state app: A;", &ep(range, "L.K"));
        assert_eq!(lit("3"), cst("L.N - 1"));
        assert_eq!(lit("4"), cst("L.N")); // one too many: the index test stays, as with the literal
        assert_ne!(lit("3"), lit("4"));
        let (a, b) = (cst("L.N - 1"), cst("L.N"));
        assert!(b.len() > a.len(), "the unbounded loop keeps a test the bounded one lost");
    }

    #[test]
    fn generated_chat_code_meets_the_profile() {
        for file in [include_str!("../examples/chat/chat.mito"), include_str!("../examples/chat-sharded/rooms.mito"), include_str!("../examples/chat-sharded/auth.mito")] {
            let (state, api) = crate::source::load(file).unwrap();
            let prog = Program::parse(&state).unwrap();
            let mut api = crate::lang::parse(&api).unwrap();
            crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
            let g = generate(&prog, &api);
            assert!(!g.notes.iter().any(|n| n.contains("profile")), "{:#?}", g.notes);
        }
    }

    #[test]
    fn emitted_files_compile_on_their_own_and_cite_their_source() {
        let (state, api) = crate::source::load(include_str!("../examples/chat/chat.mito")).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut a = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut a, &|n| prog.global(n)).unwrap();
        let c = emit(&prog, &a, ("chat.mito", &api)).unwrap();
        crate::tcc::Unit::check(&c).unwrap();
        assert!(c.contains("rooms.items.messages: list cap 1000 drop_oldest"), "{}", &c[..1500]);
        for ep in &a.eps { assert!(c.contains(&format!("// chat.mito:{}-{}", ep.lines.0, ep.lines.1))); }
        assert!(c.contains("//   | @delete(\"/session\")"));
    }

    #[test]
    fn the_binder_rejects_bad_names_and_calls_at_load() {
        let prog = Program::parse("struct C { n: i64; } state c: C;").unwrap();
        for (api, want) in [
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ \"x\": nope }); }", "unknown name `nope`"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ \"x\": frob(1) }); }", "unknown function `frob`"),
            ("fn f(a: i64, b: i64) -> i64 { return a; } @get(\"/a\") fn get_a() -> Response<{}> { return ok({ \"x\": f(1) }); }", "`f` takes 2 arguments, given 1"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { x = 1; return ok({}); }", "unknown variable `x`"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { if true { let y = 1; } return ok({ \"y\": y }); }", "unknown name `y`"), // block scope
            ("fn len(s: string) -> i64 { return 1; }", "name of a builtin"),
            (&format!("@get(\"{}\") fn a() -> Response<{{}}> {{ return ok({{}}); }}", "/a".repeat(33)), "at most 32 parts"),
            (&format!("@get(\"/a\") fn a() -> Response<{{}}> {{ let {} = 1; return ok({{}}); }}", "n".repeat(129)), "a name longer than 128 bytes"),
            (&format!("@get(\"/a\") fn a() -> Response<{{}}> {{ fail 400 \"{}\"; }}", "m".repeat(1025)), "a fail message longer than 1024 bytes"),
        ] {
            let e = super::check(&prog, api).err().unwrap_or_default();
            assert!(e.contains(want), "{} -> {}", api, e);
        }
    }

    /// Collections are used through their functions; the old keywords, and operations in the wrong place, do not load.
    #[test]
    fn collection_functions_replace_the_old_keywords() {
        let st = "struct I { n: i64; } struct E { who: i64; n: i64; } struct App { items: pool<I, 4>; board: sorted<E, 4, n>; } state app: App;";
        let (st, _) = crate::modules::expand(st, "").unwrap();
        let load = |api: &str| -> String {
            let (_, api) = crate::modules::expand("struct I { n: i64; } struct E { who: i64; n: i64; } struct App { items: pool<I, 4>; board: sorted<E, 4, n>; } state app: App;", api).unwrap();
            super::check(&Program::parse(&st).unwrap(), &api).err().unwrap_or_default()
        };
        for (api, want) in [
            ("@post(\"/a\") fn post_a() -> Response<{}> { let p = alloc app.items; return ok({}); }", "replaced by collection functions"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { let p = app.items[0]; free p; return ok({}); }", "replaced by the collection's function"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ n: count(app.items) }); }", "`count(x)` was replaced by `x.count()`"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ n: capacity(app.items) }); }", "`capacity(x)` was replaced by `x.capacity()`"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { return ok({ n: slot(app.items.add()) }); }", "makes an item"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { let p = app.items[0]; app.board.items.remove(p); return ok({}); }", "cannot be removed from"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { let p = app.items[0]; let n = app.items.remove(p); return ok({}); }", "is a statement"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ n: app.items.count(1) }); }", "takes no arguments, given 1"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { if app.items.add { n: 1 } { } return ok({}); }", "expected"),
        ] {
            let e = load(api);
            assert!(e.contains(want), "{} -> {}", api, e);
        }
        assert_eq!(load("@post(\"/a\") fn post_a() -> Response<{ n: i64, c: i64 }> { let p = app.items.add { n: 2 }; p.n += 1; app.items.remove(p); return ok({ n: p.n, c: app.items.count() }); }"), "");
    }

    /// `T?`: a value that may be null is used as an item only after `if exists(x)` (or a guard that leaves when it is null), or inside
    /// `or(...)` / `exists(...)`; a function that can return null says `-> T?`. A plain item is always live, so nothing here can fail at run time.
    #[test]
    fn values_that_may_be_null_are_checked_before_use() {
        let st = "struct I { n: i64; name: string<8>; } struct App { items: pool<I, 4>; } state app: App;";
        let load = |api: &str| -> String { super::check(&Program::parse(st).unwrap(), api).err().unwrap_or_default() };
        let find = "fn find(n: i64) -> I? { foreach it in app.items { if it.n == n { return it; } } return null; }\n";
        for (api, want) in [
            // reading, writing, removing and slot() of a value that may be null
            (format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = find(k); return ok({{ n: it.n }}); }}", find), "`.n` of a value that may be null"),
            (format!("{}@post(\"/a\") fn a(k: i64) -> Response<{{}}> {{ let it = find(k); it.n = 1; return ok({{}}); }}", find), "assigning `.n` of a value that may be null"),
            (format!("{}@post(\"/a\") fn a(k: i64) -> Response<{{}}> {{ let it = find(k); app.items.remove(it); return ok({{}}); }}", find), "removing a value that may be null"),
            (format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ s: i64 }}> {{ let it = find(k); return ok({{ s: slot(it) }}); }}", find), "`slot(x)` of a value that may be null"),
            // try_add gives a value that may be null
            ("@post(\"/a\") fn a() -> Response<{}> { let it = app.items.try_add(); it.n = 1; return ok({}); }".to_string(), "may be null"),
            // a function that can return null must say so
            ("fn find(n: i64) -> I { foreach it in app.items { if it.n == n { return it; } } return null; }\n@get(\"/a\") fn a(k: i64) -> Response<{ n: i64 }> { return ok({ n: or(find(k).n, 0) }); }".to_string(), "the result of `find` is `I`, given null"),
            // the narrowing ends with its branch, and does not hold when the branch assigns the variable
            (format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = find(k); if exists(it) {{ }} return ok({{ n: it.n }}); }}", find), "may be null"),
            (format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ var it = find(k); if exists(it) {{ it = find(k + 1); return ok({{ n: it.n }}); }} return ok({{ n: 0 }}); }}", find), "may be null"),
            // writing through null
            ("@post(\"/a\") fn a() -> Response<{}> { let it = null; it.n = 1; return ok({}); }".to_string(), "of `null`"),
        ] {
            let e = load(&api);
            assert!(e.contains(want), "{} -> {}", api, e);
        }
        // what is accepted: `if exists(x)`, `if x != null`, a guard that leaves, the else branch of `if x == null`, `or(...)`, `exists(...)`
        for api in [
            format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = find(k); if exists(it) {{ return ok({{ n: it.n }}); }} return ok({{ n: 0 }}); }}", find),
            format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = find(k); if it != null {{ it.n = 2; return ok({{ n: it.n }}); }} return ok({{ n: 0 }}); }}", find),
            format!("{}@post(\"/a\") fn a(k: i64) -> Response<{{ s: i64 }}> {{ let it = find(k); if !exists(it) {{ fail 404 \"no such item\"; }} it.n = 3; app.items.remove(it); return ok({{ s: slot(it) }}); }}", find),
            format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = find(k); if it == null {{ return ok({{ n: 0 }}); }} else {{ return ok({{ n: it.n }}); }} }}", find),
            format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64, there: bool }}> {{ return ok({{ n: or(find(k).n, -1), there: exists(find(k)) }}); }}", find),
            // `or` with a plain item as the fallback gives a plain item
            format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = or(find(k), app.items[0]); return ok({{ n: it.n }}); }}", find),
        ] {
            assert_eq!(load(&api), "", "{}", api);
        }
    }

    /// Every answer is checked against the endpoint's `Response<T>` when the program is compiled.
    #[test]
    fn answers_are_checked_against_their_response_type() {
        let st = "struct Row { name: string<8>; n: i64; } struct I { name: string<8>; n: i64; } struct App { items: pool<I, 4>; total: i64; } state app: App;";
        let load = |api: &str| -> String { super::check(&Program::parse(st).unwrap(), api).err().unwrap_or_default() };
        for (api, want) in [
            ("@get(\"/a\") fn a() -> Response<{ n: i64 }> { return ok({ n: 1, m: 2 }); }", "has `m`, which { n: i64 } does not declare"),
            ("@get(\"/a\") fn a() -> Response<{ n: i64, m: i64 }> { return ok({ n: 1 }); }", "has no `m`"),
            ("@get(\"/a\") fn a() -> Response<{ n: string }> { return ok({ n: 1 }); }", "`n` is i64, declared string"),
            ("@get(\"/a\") fn a() -> Response<{ x: [i64] }> { return ok({ x: [foreach it in app.items it.name] }); }", "`x[]` is string, declared i64"),
            ("@get(\"/a\") fn a() -> Response<[Row]> { return ok([foreach it in app.items { name: it.name }]); }", "has no `n`"),
            ("@get(\"/a\") fn a() -> Response<null> { return ok({}); }", "declared null"),
            ("@get(\"/a\") fn a() -> Response<{}> { return status(302, {}); }", "an answer is 2xx, 4xx or 5xx"),
            ("@get(\"/a\") fn a() -> Response<{}> { return {}; }", "an endpoint returns its answer"),
            ("@get(\"/a\") fn a() -> Response<{}> { respond 200 {}; }", "`respond` was replaced"),
            ("endpoint GET /a { respond 200 {}; }", "endpoints are functions with an attribute"),
            ("@get(\"/a\") fn a() { return ok({}); }", "returns a `Response<T>`"),
            ("@fetch(\"/a\") fn a() -> Response<{}> { return ok({}); }", "`@fetch`"),
            ("@get(\"/a\") fn a() -> Response<{}> { if app.total > 1 { return ok({}); } }", "can finish without an answer"),
            // every value's kind is known when the program is compiled
            ("@get(\"/a\") fn a() -> Response<{ n: i64 }> { var v = 1; v = \"x\"; return ok({ n: 1 }); }", "`v` holds an integer and then a string: a var keeps the kind of its first value"),
            ("fn f(k: i64) -> i64 { if k > 0 { return 1; } return \"x\"; } @get(\"/a\") fn a() -> Response<{}> { let x = f(1); return ok({}); }", "the result of `f` is `i64`, given a string"),
            ("fn f(k: i64) { if k > 0 { return 1; } } @get(\"/a\") fn a() -> Response<{}> { f(1); return ok({}); }", "`f` returns a value: declare its result"),
            ("fn f(k) -> i64 { return k; } @get(\"/a\") fn a() -> Response<{}> { let x = f(1); return ok({}); }", "parameter `k` needs a type"),
            ("fn f(k: i64) -> i64 { return k; } @get(\"/a\") fn a() -> Response<{}> { let x = f(\"one\"); return ok({}); }", "parameter `k` of `f` is `i64`, given a string"),
            ("fn f(k: Nope) -> i64 { return 1; } @get(\"/a\") fn a() -> Response<{}> { let x = f(1); return ok({}); }", "`Nope` is not a type"),
            ("fn f(k: i64) -> i64 { if k > 0 { return 1; } } @get(\"/a\") fn a() -> Response<{}> { let x = f(1); return ok({}); }", "`f` can end without returning its `i64`"),
            ("@get(\"/a\") fn a() -> Response<{ n: string }> { return ok({ n: \"x\" - 1 }); }", "operator `-` does not fit string and i64"),
        ] {
            let e = load(api);
            assert!(e.contains(want), "{} -> {}", api, e);
        }
        // what fits: a declared struct, optional values, run-time kinds, no body, a status of choice
        for api in [
            "@get(\"/a\") fn a() -> Response<[Row]> { return ok([foreach it in app.items { name: it.name, n: it.n }]); }",
            "@get(\"/a\") fn a(x: string<8>?) -> Response<{ n: string? }> { return ok({ n: or(x, null) }); }",
            "@delete(\"/a\") fn a() -> Response<null> { return no_content(); }",
            "@post(\"/a\") fn a() -> Response<{ total: i64 }> { app.total += 1; if app.total > 2 { return status(202, { total: app.total }); } return created({ total: app.total }); }",
        ] {
            assert_eq!(load(api), "", "{}", api);
        }
    }

    #[test]
    fn the_checker_rejects_each_kind_of_violation() {
        let ok = "    i64 v1 = LD_I64(64, i2);\n    ADD(v3, v1, 1LL);\n    CALL(c->h->json_i64(c, v3));\n    return 200;\n";
        assert_eq!(check_profile(ok), Ok(()));
        for (bad, why) in [
            ("    *(i64 *)(b + 8) = 1;\n", "pointer dereference"),
            ("    i64 v = *(const i64 *)(b + 8);\n", "pointer dereference"),
            ("    while (1) { }\n", "forbidden word"),
            ("    for (u32 k = 0;; k++) { }\n", "without a bound"),
            ("    c->h->out(c, \"x\", 1);\n", "without CALL"),
            ("    CALL(ep1(c));\n", "call of something other"),
            ("    CALL(memcpy(&v, b, 8));\n", "forbidden word"),
            ("    u32 k = 0;\nL1: ;\n    goto L1;\n", "backward goto"),
            ("    return 5 + 3;\n", "status"),
            ("    CALL(c->h->lit(c, \"x\", 1, &b));\n    CALL(c->h->lit(c, \"x\", 1, &zz));\n", "address of"),
            ("    i64 v = b[3];\n", "indexing"),
            ("    i64 v = 1; v += 2;\n", "compound assignment"),
            ("#include <stdio.h>\n", "preprocessor"),
            ("    static i64 g;\n", "forbidden word"),
            ("    i64 v = c->base;\n", "member access"),
        ] {
            let e = check_profile(bad).err().unwrap_or_default();
            assert!(e.contains(why), "{:?}: {}", bad, e);
        }
    }
}

/// An array access a versioned loop may make without its own tests: the array, the index expression, whether it stores,
/// and how the index relates to the counter (or stepped variable) `v`: v, v + c, v - c, or c - v (c unchanged by the loop).
struct Site<'p> { arr: &'p Expr, idx: &'p Expr, store: bool, off: Off<'p> }
#[derive(Clone, Copy)]
enum Off<'p> { Zero, Plus(&'p Expr), Minus(&'p Expr), From(&'p Expr) }
/// The values a versioned loop's variable takes: the range lo..hi, or from its value before the loop up to a limit (inclusive or not), stepping up.
enum Bound<'p> { Range(&'p Expr, &'p Expr), Stepped(&'p Expr, bool, &'p Expr), Counted(&'p Expr, &'p Expr, &'p Expr, bool) }
// Counted(lo, hi, step, after): a variable stepped up once per iteration of a loop over lo..hi; at most hi - lo iterations, so it stays below
// its start plus (hi - lo) steps; `after`: the accesses come after the step in the body (one step more)

/// The range a `var` stays in, when it starts as `init` and the statements after its declaration (its scope) only ever assign it a constant or
/// `x & mask` (a non-negative constant mask): from 0 or the constants up to the largest mask or constant. None when anything else is assigned to it.
fn masked_var_range(stmts: &[Stmt], slot: u32, init: &Expr) -> Option<(i64, i64)> {
    fn of(e: &Expr) -> Option<(i64, i64)> {
        match e {
            Expr::Int(v) => Some((*v, *v)),
            Expr::Bin(op, a, b) if op == "&" => match (&**a, &**b) { (_, Expr::Int(m)) | (Expr::Int(m), _) if *m >= 0 => Some((0, *m)), _ => None },
            _ => None,
        }
    }
    fn walk(stmts: &[Stmt], slot: u32, r: &mut (i64, i64)) -> bool {
        for st in stmts {
            match st {
                Stmt::Assign(Expr::Var(_, VarRes::Local(s)), rhs) if *s == slot => match of(rhs) { Some((lo, hi)) => { r.0 = r.0.min(lo); r.1 = r.1.max(hi); } None => return false },
                Stmt::Let(_, s, _) | Stmt::Var(_, s, _) if *s == slot => return false,
                Stmt::Alloc { slot: s, .. } if *s == slot => return false,
                Stmt::If(_, t, e) => { if !walk(t, slot, r) || !walk(e, slot, r) { return false; } }
                Stmt::Each(it, b) => { if it.slot == slot || it.index.as_ref().map_or(false, |(_, s)| *s == slot) || !walk(b, slot, r) { return false; } }
                _ => {}
            }
        }
        true
    }
    let mut r = of(init)?;
    if walk(stmts, slot, &mut r) { Some(r) } else { None }
}
/// The narrowest of 8, 16 or 32 bits that holds every value up to `hi` ("" when there is none, or `hi` is not known): the width a bitwise operation
/// whose result is bounded by it may be done at.
fn width_of(hi: Option<i64>) -> &'static str {
    match hi { Some(h) if (0..=255).contains(&h) => "8", Some(h) if (0..=65535).contains(&h) => "16", Some(h) if (0..=u32::MAX as i64).contains(&h) => "32", _ => "" }
}
/// How much `slot` can go up and down in one pass of these statements, when everything that assigns it is `x = x + k` or `x = x - k` with a small constant `k` >= 0
/// (anywhere but inside a loop of the statements); None when anything else assigns it. Of an `if`, the arm that moves it most counts.
fn step_sums(stmts: &[Stmt], slot: u32) -> Option<(i128, i128)> {
    const MAX_STEP: i64 = 1 << 20;
    let (mut up, mut down) = (0i128, 0i128);
    for st in stmts {
        match st {
            Stmt::Assign(Expr::Var(_, VarRes::Local(s)), rhs) if *s == slot => match rhs {
                Expr::Bin(op, a, b) if op == "+" => match (&**a, &**b) {
                    (Expr::Var(_, VarRes::Local(x)), Expr::Int(k)) | (Expr::Int(k), Expr::Var(_, VarRes::Local(x))) if *x == slot && (0..=MAX_STEP).contains(k) => up += *k as i128,
                    _ => return None,
                },
                Expr::Bin(op, a, b) if op == "-" => match (&**a, &**b) {
                    (Expr::Var(_, VarRes::Local(x)), Expr::Int(k)) if *x == slot && (0..=MAX_STEP).contains(k) => down += *k as i128,
                    _ => return None,
                },
                _ => return None,
            },
            Stmt::If(_, t, e) => { let (a, b) = step_sums(t, slot)?; let (c, d) = step_sums(e, slot)?; up += a.max(c); down += b.max(d); }
            other => { if assigns_slot(other, slot) { return None; } }
        }
    }
    Some((up, down))
}
/// The local slots assigned anywhere in these statements (nested ones included).
fn assigned_slots(stmts: &[Stmt]) -> std::collections::HashSet<u32> {
    let mut out = std::collections::HashSet::new();
    fn walk(stmts: &[Stmt], out: &mut std::collections::HashSet<u32>) {
        for st in stmts {
            match st {
                Stmt::Assign(Expr::Var(_, VarRes::Local(s)), _) => { out.insert(*s); }
                Stmt::Let(_, s, _) | Stmt::Var(_, s, _) => { out.insert(*s); }
                Stmt::Alloc { slot, .. } => { out.insert(*slot); }
                Stmt::If(_, t, e) => { walk(t, out); walk(e, out); }
                Stmt::Each(it, b) => { out.insert(it.slot); if let Some((_, s)) = &it.index { out.insert(*s); } walk(b, out); }
                _ => {}
            }
        }
    }
    walk(stmts, &mut out);
    out
}
fn assigns_slot(st: &Stmt, slot: u32) -> bool { assigned_slots(std::slice::from_ref(st)).contains(&slot) }
/// An expression without effects, as a range's bounds must be (the guard evaluates them once more): numbers, locals, state fields, arithmetic.
fn pure(e: &Expr) -> bool {
    match e {
        Expr::Int(_) | Expr::Var(_, VarRes::Local(_)) => true,
        Expr::Field(b, _) => state_path(b),
        Expr::Bin(op, a, b) => matches!(op.as_str(), "+" | "-" | "*") && pure(a) && pure(b),
        _ => false,
    }
}
fn state_path(e: &Expr) -> bool { match e { Expr::Var(_, VarRes::Global(_)) => true, Expr::Field(b, _) => state_path(b), _ => false } }
/// A value the loop does not change: a number, or a local the loop never assigns.
fn invariant(e: &Expr, assigned: &std::collections::HashSet<u32>) -> bool {
    match e {
        Expr::Int(_) => true,
        Expr::Var(_, VarRes::Local(s)) => !assigned.contains(s),
        Expr::Bin(op, a, b) => matches!(op.as_str(), "+" | "-" | "*") && invariant(a, assigned) && invariant(b, assigned),
        _ => false,
    }
}
/// `v <= limit` / `v < limit` terms of a condition joined by `&&`: (v's slot, limit, inclusive).
fn upper_bounds(e: &Expr) -> Vec<(u32, &Expr, bool)> {
    match e {
        Expr::Bin(op, a, b) if op == "&&" => { let mut v = upper_bounds(a); v.extend(upper_bounds(b)); v }
        Expr::Bin(op, a, b) if op == "<=" || op == "<" => match &**a { Expr::Var(_, VarRes::Local(s)) => vec![(*s, &**b, op == "<=")], _ => vec![] },
        _ => vec![],
    }
}
/// How index `idx` relates to variable `v`, if it is one of the shapes a guard can bound.
fn index_shape<'p>(idx: &'p Expr, v: u32, assigned: &std::collections::HashSet<u32>) -> Option<Off<'p>> {
    let is_v = |e: &Expr| matches!(e, Expr::Var(_, VarRes::Local(s)) if *s == v);
    match idx {
        e if is_v(e) => Some(Off::Zero),
        Expr::Bin(op, a, b) if op == "+" && is_v(a) && invariant(b, assigned) => Some(Off::Plus(b)),
        Expr::Bin(op, a, b) if op == "+" && is_v(b) && invariant(a, assigned) => Some(Off::Plus(a)),
        Expr::Bin(op, a, b) if op == "-" && is_v(a) && invariant(b, assigned) => Some(Off::Minus(b)),
        Expr::Bin(op, a, b) if op == "-" && is_v(b) && invariant(a, assigned) => Some(Off::From(a)),
        _ => None,
    }
}
/// The accesses in a statement (nested ones included) to an array of plain state whose index has a shape `index_shape` knows.
fn stmt_sites<'p>(st: &'p Stmt, v: u32, assigned: &std::collections::HashSet<u32>, out: &mut Vec<Site<'p>>) {
    fn ex<'p>(e: &'p Expr, v: u32, assigned: &std::collections::HashSet<u32>, out: &mut Vec<Site<'p>>) {
        match e {
            Expr::Index(arr, idx) => {
                if state_path(arr) { if let Some(off) = index_shape(idx, v, assigned) { out.push(Site { arr, idx, store: false, off }); } }
                ex(arr, v, assigned, out); ex(idx, v, assigned, out);
            }
            Expr::Field(b, _) | Expr::Un(_, b) => ex(b, v, assigned, out),
            Expr::Bin(_, a, b) => { ex(a, v, assigned, out); ex(b, v, assigned, out); }
            Expr::Call(_, args, _) => for a in args { ex(a, v, assigned, out); },
            _ => {}
        }
    }
    match st {
        Stmt::Assign(target, rhs) => {
            if let Expr::Index(arr, idx) = target {
                if state_path(arr) { if let Some(off) = index_shape(idx, v, assigned) { out.push(Site { arr, idx, store: true, off }); } }
                ex(idx, v, assigned, out);
            } else { ex(target, v, assigned, out); }
            ex(rhs, v, assigned, out);
        }
        Stmt::Let(_, _, e) | Stmt::Var(_, _, e) => ex(e, v, assigned, out),
        Stmt::If(c, t, e) => { ex(c, v, assigned, out); for s in t { stmt_sites(s, v, assigned, out); } for s in e { stmt_sites(s, v, assigned, out); } }
        Stmt::Each(it, b) => {
            ex(&it.coll, v, assigned, out);
            if let Some(h) = &it.hi { ex(h, v, assigned, out); }
            if let Some(w) = &it.whil { ex(w, v, assigned, out); }
            for s in b { stmt_sites(s, v, assigned, out); }
        }
        _ => {}
    }
}

/// An i64 as a C constant (the smallest one cannot be written as a literal).
fn c_i64(v: i64) -> String { if v == i64::MIN { "(-9223372036854775807LL - 1)".into() } else { format!("{}LL", v) } }

/// Does any `return` in these statements (or in blocks inside them) give a value?
fn stmts_return_value(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Return(Some(_)) | Stmt::Respond(..) => true,
        Stmt::If(_, t, e) => stmts_return_value(t) || stmts_return_value(e),
        Stmt::Each(_, b) => stmts_return_value(b),
        _ => false,
    })
}
/// `exists(m.f)`, `m.f != null` (and their negations, false) for a local m: its slot, the field's path, and whether the test holding
/// means the field is there.
fn field_narrow_test(c: &Expr) -> Option<(u32, String, bool)> {
    let field = |e: &Expr| match e { Expr::Field(b, f) => match &**b { Expr::Var(_, VarRes::Local(s)) => Some((*s, f.clone())), _ => None }, _ => None };
    match c {
        Expr::Call(n, a, _) if n == "exists" && a.len() == 1 => field(&a[0]).map(|(s, f)| (s, f, true)),
        Expr::Un('!', x) => field_narrow_test(x).map(|(s, f, y)| (s, f, !y)),
        Expr::Bin(op, a, b) if op == "!=" || op == "==" => {
            let (s, f) = match (&**a, &**b) { (x, Expr::Null) | (Expr::Null, x) => field(x)?, _ => return None };
            Some((s, f, op == "!="))
        }
        _ => None,
    }
}
/// `a && b && ...` with a test that narrows (`exists(x)`, `exists(m.f)`) before its last part: the parts up to that test, and the rest
fn split_after_narrowing(c: &Expr) -> Option<(Expr, Expr)> {
    fn parts<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) { match e { Expr::Bin(op, a, b) if op == "&&" => { parts(a, out); parts(b, out); } e => out.push(e) } }
    let mut ps = Vec::new(); parts(c, &mut ps);
    if ps.len() < 2 { return None; }
    let i = ps[..ps.len() - 1].iter().position(|p| matches!(narrow_test(p), Some((_, true))) || matches!(field_narrow_test(p), Some((_, _, true))))?;
    let join = |xs: &[&Expr]| -> Expr { let mut e = xs[0].clone(); for x in &xs[1..] { e = Expr::Bin("&&".into(), Box::new(e), Box::new((*x).clone())); } e };
    Some((join(&ps[..=i]), join(&ps[i + 1..])))
}
/// `exists(x)`, `x != null` (true: x is an item when the test holds), `!exists(x)`, `x == null` (false: when it fails), for a local x.
fn narrow_test(c: &Expr) -> Option<(u32, bool)> {
    let local = |e: &Expr| match e { Expr::Var(_, VarRes::Local(s)) => Some(*s), _ => None };
    match c {
        Expr::Call(n, a, _) if n == "exists" && a.len() == 1 => local(&a[0]).map(|s| (s, true)),
        Expr::Un('!', x) => narrow_test(x).map(|(s, y)| (s, !y)),
        Expr::Bin(op, a, b) if op == "!=" || op == "==" => {
            let s = match (&**a, &**b) { (x, Expr::Null) | (Expr::Null, x) => local(x)?, _ => return None };
            Some((s, op == "!="))
        }
        _ => None,
    }
}

/// Does any `return` in these statements give a value?
fn returns_value(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Return(Some(_)) => true,
        Stmt::If(_, t, e) => returns_value(t) || returns_value(e),
        Stmt::Each(_, b) => returns_value(b),
        _ => false,
    })
}

/// An error with the place (`line:col: `) of the statement it came from, unless it has one already.
/// Does running these statements (inlined calls included) assign a field? Only that changes a text field's length (writing an
/// element of an array, or an item's field by index, never does).
fn assigns_field(api: &Api, stmts: &[Stmt], seen: &mut Vec<u32>) -> bool {
    fn ex(api: &Api, e: &Expr, seen: &mut Vec<u32>) -> bool {
        match e {
            Expr::Call(_, _, CallRes::Builtin(B::Truncate)) => true,
            Expr::Call(nm, _, CallRes::Unbound) if nm == ".clear" => true,
            Expr::Call(nm, a, r) => a.iter().any(|x| ex(api, x, seen)) || callees(nm, r).into_iter().any(|k| if seen.contains(&k) { false } else { seen.push(k); assigns_field(api, &api.fns[k as usize].body, seen) }),
            Expr::Field(b, _) | Expr::Un(_, b) => ex(api, b, seen),
            Expr::Index(a, b) | Expr::Bin(_, a, b) => ex(api, a, seen) || ex(api, b, seen),
            _ => false,
        }
    }
    stmts.iter().any(|s| match s {
        Stmt::Assign(Expr::Field(..), _) | Stmt::Clear(..) => true,
        Stmt::Assign(a, b) => ex(api, a, seen) || ex(api, b, seen),
        Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(api, e, seen),
        Stmt::If(c, t, e) => ex(api, c, seen) || assigns_field(api, t, seen) || assigns_field(api, e, seen),
        Stmt::Each(it, b) => ex(api, &it.coll, seen) || it.hi.as_ref().map_or(false, |h| ex(api, h, seen)) || it.whil.as_ref().map_or(false, |w| ex(api, w, seen)) || assigns_field(api, b, seen),
        _ => false,
    })
}

/// The state paths indexed anywhere in these statements (`x.text[i]`: `x.text`), loops and branches included.
fn index_bases<'p>(stmts: &'p [Stmt], out: &mut Vec<&'p Expr>) {
    fn ex<'p>(e: &'p Expr, out: &mut Vec<&'p Expr>) {
        match e {
            Expr::Index(a, i) => { if (state_path(a) || matches!(**a, Expr::Var(_, VarRes::Local(_)))) && !out.iter().any(|x| format!("{:?}", x) == format!("{:?}", a)) { out.push(a); } ex(a, out); ex(i, out); }
            Expr::Field(b, _) | Expr::Un(_, b) => ex(b, out),
            Expr::Bin(_, a, b) => { ex(a, out); ex(b, out); }
            Expr::Call(_, args, _) => for a in args { ex(a, out); },
            _ => {}
        }
    }
    for st in stmts {
        match st {
            Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(e, out),
            Stmt::Assign(a, b) => { ex(a, out); ex(b, out); }
            Stmt::If(c, t, e) => { ex(c, out); index_bases(t, out); index_bases(e, out); }
            Stmt::Each(it, b) => { ex(&it.coll, out); if let Some(h) = &it.hi { ex(h, out); } if let Some(w) = &it.whil { ex(w, out); } if let Some(w) = &it.wher { ex(w, out); } index_bases(b, out); }
            _ => {}
        }
    }
}

/// `target = a + b + ...` or `target += ...` (the parser's `target = target + ...`) on a string field, as pieces to append in place:
/// (whether to empty it first, the pieces). Only a join that is surely text: an append (it starts with the field itself), or an
/// assignment starting with a literal or `str(...)` and not reading the field. A right operand that is itself such a join is split too.
fn str_pieces<'e>(target: &Expr, rhs: &'e Expr) -> Option<(bool, Vec<&'e Expr>)> {
    fn texty(e: &Expr) -> bool { matches!(e, Expr::Str(_)) || matches!(e, Expr::Call(n, _, _) if n == "str" || n == "substr") }
    fn leftmost(e: &Expr) -> &Expr { match e { Expr::Bin(op, l, _) if op == "+" => leftmost(l), o => o } }
    fn split<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
        match e {
            Expr::Bin(op, l, r) if op == "+" => { split(l, out); if texty(leftmost(r)) { split(r, out); } else { out.push(r); } }
            o => out.push(o),
        }
    }
    let t = format!("{:?}", target);
    if let Expr::Call(n, _, _) = rhs { if n == "substr" && !format!("{:?}", rhs).contains(&t) { return Some((true, vec![rhs])); } }
    let Expr::Bin(op, _, _) = rhs else { return None };
    if op != "+" { return None; }
    let mut pieces = Vec::new();
    split(rhs, &mut pieces);
    if format!("{:?}", pieces[0]) == t { pieces.remove(0); if pieces.iter().any(|p| format!("{:?}", p).contains(&t)) { return None; } return Some((false, pieces)); }
    if !texty(pieces[0]) || pieces.iter().any(|p| format!("{:?}", p).contains(&t)) { return None; }
    Some((true, pieces))
}

/// A function body's return placeholders (`/*RCUT mark*/`): when the body built region text (it cuts the text area somewhere), a mark
/// at its start and a cut back to it before each return of a value that is not text; otherwise nothing.
fn resolve_return_cuts(body: &str, mark: &str) -> String {
    let ph = format!("/*RCUT {}*/", mark);
    if !body.contains(&ph) { return body.to_string(); }
    if body.contains("->text_cut(") {
        let cut = body.replace(&ph, &format!("CALL(c->h->text_cut(c, {}));", mark));
        format!("u32 {}; CALL(c->h->text_len(c, &{}));\n{}", mark, mark, cut)
    } else {
        body.lines().filter(|l| l.trim() != ph).map(|l| format!("{}\n", l)).collect()
    }
}
fn place_err(at: (u32, u32), e: String) -> String { if at.0 == 0 || crate::lang::starts_with_place(&e) { e } else { format!("{}:{}: {}", at.0, at.1, e) } }
/// `"12:5: rest"` as (`"12:5: "`, `"rest"`).
fn split_place(e: &str) -> Option<(&str, &str)> {
    if !crate::lang::starts_with_place(e) { return None; }
    let k = e.match_indices(": ").next()?.0 + 2;
    Some((&e[..k], &e[k..]))
}
/// The one statement of a body, position markers aside.
fn only_stmt(body: &[Stmt]) -> Option<&Stmt> {
    let mut it = body.iter().filter(|s| !matches!(s, Stmt::At(..)));
    let first = it.next()?;
    if it.next().is_some() { None } else { Some(first) }
}
