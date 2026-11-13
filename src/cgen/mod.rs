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
    int (*plain_txt)(Ctx *, u64, Txt *);                       /* copy a plain (record-style) string at an offset into the text area */
    int (*binop)(Ctx *, int, const Val *, const Val *, Val *); /* an operator (OPS order) on values of run-time kind */
    int (*truthy)(Ctx *, const Val *, int *);                  /* is a value true (bool), or an error for other kinds */
    int (*store)(Ctx *, u32, u32, u32, const Val *);           /* journaled write of column (coll, idx, col) */
    int (*store_plain)(Ctx *, u64, u32, u32, const Val *);     /* journaled write of a plain value: offset, kind, capacity */
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
#ifdef __clang__
#define CELL_(T, v) ({ T t_; __builtin_memcpy(&t_, &(v), sizeof t_); t_; }) /* a struct field kept in a C local (src/scalar.rs) */
#else
#define CELL_(T, v) (*(const T *)&(v))
#endif
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
/* the rest of jq's math functions: the platform's math library, so a result is what its libm gives (libms may differ in the last bit);
   what a library lacks (exp10, significand, drem, scalb, lgamma_r on Windows) is made of what it has */
double asinh(double); double acosh(double); double atanh(double); double exp2(double); double expm1(double); double log1p(double); double logb(double);
double nearbyint(double); double rint(double); double lgamma(double); double tgamma(double); double copysign(double, double); double remainder(double, double);
double nextafter(double, double); double fmin(double, double); double fmax(double, double); double ldexp(double, int); double scalbn(double, int); double fma(double, double, double);
double frexp(double, int *); double modf(double, double *); double fabs(double); double pow(double, double); double floor(double);
#ifdef _WIN32
double _j0(double); double _j1(double); double _y0(double); double _y1(double);
#define J0_(x) _j0(x)
#define J1_(x) _j1(x)
#define Y0_(x) _y0(x)
#define Y1_(x) _y1(x)
static double mito_exp10(double x) { return pow(10.0, x); }
static double mito_lgamma_r(double x, int *s) {
    *s = 1;
    if (x != x) return lgamma(x);
    if (x < 0.0 && x == floor(x)) { *s = 1; return lgamma(x); }
    if (x < 0.0 && floor(x) - 2.0 * floor(floor(x) / 2.0) != 0.0) *s = -1;
    if (x == 0.0 && (1.0 / x) < 0.0) *s = -1;
    return lgamma(x);
}
#else
double j0(double); double j1(double); double y0(double); double y1(double); double exp10(double); double lgamma_r(double, int *);
#define J0_(x) j0(x)
#define J1_(x) j1(x)
#define Y0_(x) y0(x)
#define Y1_(x) y1(x)
static double mito_exp10(double x) { return exp10(x); }
static double mito_lgamma_r(double x, int *s) { return lgamma_r(x, s); }
#endif
static double mito_scalb(double x, double y) {
    if (x != x || y != y) return x * y;
    if (y - y != 0.0) return y > 0.0 ? x * y : x / (-y);
    if (y != floor(y)) return (y - y) / (y - y);
    return scalbn(x, y > 65000.0 ? 65000 : y < -65000.0 ? -65000 : (int)y);
}
static int mito_clampi(double y) { return y != y ? 0 : y > 100000.0 ? 100000 : y < -100000.0 ? -100000 : (int)y; }
static double mito_significand(double x) { int e = 0; if (x == 0.0 || x != x || x - x != 0.0) return x; return frexp(x, &e) * 2.0; }
static double mito_frexp_m(double x) { int e = 0; if (x == 0.0 || x != x || x - x != 0.0) return x; return frexp(x, &e); }
static i64 mito_frexp_e(double x) { int e = 0; if (x == 0.0 || x != x || x - x != 0.0) return 0; frexp(x, &e); return e; }
static double mito_modf_f(double x) { double ip; return modf(x, &ip); }
static double mito_modf_i(double x) { double ip; modf(x, &ip); return ip; }
static double mito_lgamma(double x) { int s; return mito_lgamma_r(x, &s); }
static i64 mito_lgamma_s(double x) { int s; mito_lgamma_r(x, &s); return s; }
#define ASINH_(x) asinh(x)
#define ACOSH_(x) acosh(x)
#define ATANH_(x) atanh(x)
#define EXP2_(x) exp2(x)
#define EXP10_(x) mito_exp10(x)
#define POW10_(x) mito_exp10(x)
#define EXPM1_(x) expm1(x)
#define LOG1P_(x) log1p(x)
#define LOGB_(x) logb(x)
#define NEARBYINT_(x) nearbyint(x)
#define RINT_(x) rint(x)
#define SIGNIFICAND_(x) mito_significand(x)
#define GAMMA_(x) mito_lgamma(x)
#define LGAMMA_(x) mito_lgamma(x)
#define TGAMMA_(x) tgamma(x)
#define LGAMMA_SIGN_(x) mito_lgamma_s(x)
#define FREXP_MANTISSA_(x) mito_frexp_m(x)
#define FREXP_EXPONENT_(x) mito_frexp_e(x)
#define MODF_FRACTION_(x) mito_modf_f(x)
#define MODF_INTEGER_(x) mito_modf_i(x)
#define COPYSIGN_(x, y) copysign(x, y)
#define DREM_(x, y) remainder(x, y)
#define NEXTAFTER_(x, y) nextafter(x, y)
#define NEXTTOWARD_(x, y) nextafter(x, y)
#define SCALB_(x, y) mito_scalb(x, y)
#define FMIN_(x, y) fmin(x, y)
#define FMAX_(x, y) fmax(x, y)
#define LDEXP_(x, y) ldexp(x, mito_clampi(y))
#define SCALBLN_(x, y) scalbn(x, mito_clampi(y))
#define FMA_(x, y, z) fma(x, y, z)
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
#define BP_(off) ((const u8 *)(b + (off))) /* a text in state, for an in-line compare */
#define TP_(off) ((const u8 *)(tx_ + (off))) /* a literal in the text area */
#define BEQ_(p, q, n) (MEMCMP_((p), (q), (n)) == 0)
#define BCMP_(p, pn, q, qn) bcmp_((p), (pn), (q), (qn))
#define TX_(i) (tx_[(i)])/* a byte of the request's text area, where strings that are not in state live */
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
#define MEMCMP_(a, b, n) __builtin_memcmp((a), (b), (n))
#define MEMCHR_(a, c, n) __builtin_memchr((a), (c), (n))
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
#define MEMCMP_(a, b, n) memcmp((a), (b), (n))
#define MEMCHR_(a, c, n) memchr((a), (c), (n))
#endif
static inline int bcmp_(const u8 *p, u64 pn, const u8 *q, u64 qn) { int r = MEMCMP_(p, q, pn < qn ? pn : qn); return r ? r : pn < qn ? -1 : pn > qn; }
#define TXP_(off) ((const u8 *)tx_ + (off)) /* where a text of the request's text area starts */
#define STP_(off) ((const u8 *)b + (off)) /* where bytes of state start */
/* starts_with, ends_with (0 or 1), index_of, last_index_of (the offset or -1) of a literal: p and n the text, q and m the literal */
#define BSTARTS_(p, n, q, m) ((u64)(m) <= (u64)(n) && MEMCMP_((p), (q), (m)) == 0)
#define BENDS_(p, n, q, m) ((u64)(m) <= (u64)(n) && MEMCMP_((p) + (u64)(n) - (u64)(m), (q), (m)) == 0)
#define BFIND_(p, n, q, m) ({ const u8 *p_ = (p), *q_ = (q); u64 n_ = (n), m_ = (m); i64 r_ = -1;     if (m_ <= n_) { if (m_ == 0) r_ = 0; else if (m_ == 1) { const u8 *f_ = (const u8 *)MEMCHR_(p_, q_[0], n_); if (f_) r_ = (i64)(f_ - p_); }     else { for (u64 i_ = 0; i_ + m_ <= n_; i_++) if (p_[i_] == q_[0] && MEMCMP_(p_ + i_, q_, m_) == 0) { r_ = (i64)i_; break; } } } r_; })
#define BRFIND_(p, n, q, m) ({ const u8 *p_ = (p), *q_ = (q); u64 n_ = (n), m_ = (m); i64 r_ = -1;     if (m_ <= n_) { for (u64 i_ = n_ - m_ + 1; i_-- > 0;) if (m_ == 0 || (p_[i_] == q_[0] && MEMCMP_(p_ + i_, q_, m_) == 0)) { r_ = (i64)i_; break; } } r_; })
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
pub const BUILTINS: [B; 106] = [B::Body, B::Param, B::Query, B::Header, B::Fwd, B::Int, B::Str, B::Len, B::Now, B::Random, B::Token, B::Stat, B::WorkerIndex, B::WorkerCount, B::Min, B::Max, B::Slot,
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
    B::Chr, B::AllWord,
    // command-line programs only (`@main`): standard error, as text: 100; stdin on demand: 101; a file named at run time: 102, 103
    B::Eprint, B::StdinText, B::ReadFile, B::FileExists,
    // standard output as the program runs (`@main` only): 104, 105
    B::Print, B::Flush];
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

/// The place a `write` parameter of a number or bool stands for. `Var`: a `var` local of the caller (its C name, and its slot in the caller), which the parameter shares. The others: a field (`Field`) or
/// an element (`Index`) of the base the first hidden value is (and the index the second): the callee's hidden slots `hb`, `hb + 1` hold them, `hb + 2` the value being assigned, and `loc` is
/// the expression that names the place (read it for the parameter's value; assign it to store). The code that reads or assigns the place is the code for any field or element.
#[derive(Clone)]
enum WShape { Var(String, Option<u32>), Field(String), Index }
#[derive(Clone)]
struct WPlace<'p> { shape: WShape, hidden: Vec<CV>, hb: u32, loc: Option<&'p Expr> }

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
    /// Per function, per parameter: is it a `write` parameter of a number or bool (see `scalar_write_flags`)?
    static SCALARW: std::cell::RefCell<Vec<Vec<bool>>> = const { std::cell::RefCell::new(Vec::new()) };
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
    // `write` parameters of a number or bool in the functions being inlined (keyed by the function and the parameter): the caller's place each stands for (see `WPlace`);
    // `wargs`: the places of the arguments of the call about to be made; `wskip`: the next assignment to a parameter goes through its own C variable
    wslots: std::collections::HashMap<(Option<u32>, u32), WPlace<'p>>, wargs: Vec<Option<WPlace<'p>>>, wskip: bool,
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
    widen: Option<Widen>, errs: Vec<String>,
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
pub const RUNTIME: &str = include_str!("../../runtime/mito.c");
/// Unicode's case mappings (generated by tools/unicase.py), for a program that uses `lower` or `upper`.
pub const UNICASE: &str = include_str!("../../runtime/unicase.c");
/// The operating system's part (runtime/os.c: Windows and Linux), first in every translation unit after the prelude's types.
pub const OS: &str = include_str!("../../runtime/os.c");
/// Stored data (runtime/store.c): tables in SQLite, for a program that has one.
pub const STORE: &str = include_str!("../../runtime/store.c");
/// Cryptography from the system's library (runtime/crypto.c), for a program that uses a crypto built-in.
pub const CRYPTO: &str = include_str!("../../runtime/crypto.c");
/// HTML fragments' node pool (runtime/html.c), for a program that uses an HTML built-in.
pub const HTML: &str = include_str!("../../runtime/html.c");
/// The runtime's types for the layout tables, ahead of them.
pub const RUNTIME_TYPES: &str = include_str!("../../runtime/types.h");

/// A whole program as one C translation unit: the prelude, the layout and routes as data, the C runtime, and the
/// endpoints. `mito_*` (runtime/mito.c) is its interface. Fails if an endpoint does not compile.
pub fn program_c(prog: &Program, api: &Api, source: Option<(&str, &str)>) -> Result<String, String> { program_c_named(prog, api, source, "mito_") }
/// A program with its tests, and the test runner's `main` (runtime/testmain.c), as an executable's source: `mito test`.
pub fn program_test_exe(prog: &Program, api: &Api, source: Option<(&str, &str)>) -> Result<Vec<String>, String> {
    Ok(vec![program_c_full(prog, api, source, "mito_", true)? + TESTMAIN])
}
/// The test runner (runtime/testmain.c).
pub const TESTMAIN: &str = include_str!("../../runtime/testmain.c");

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
    o += &direct(&crate::scalar::replace(&g.src[PRELUDE.len()..]));
    Ok(o)
}

/// The HTTP server and `main` (runtime/http.c), for executables.
pub const HTTP: &str = include_str!("../../runtime/http.c");

/// One program as an executable's source: the program, its `web` block's data (src/web.rs; empty without one), then its HTTP server.
pub fn program_exe(prog: &Program, api: &Api, source: Option<(&str, &str)>, web: &str) -> Result<Vec<String>, String> {
    // a program whose entry is `@main` is a command-line program: its `main` is runtime/cli.c, not a server
    if let Some(e) = api.eps.iter().find(|e| e.method == "MAIN") {
        if api.eps.iter().any(|x| !matches!(x.method.as_str(), "MAIN" | "START")) { return Err("a program with `@main` is a command-line program: it has no endpoints (`@get` ...) beside it".into()); }
        let stdin = e.inputs.iter().any(|i| matches!(&i.src, Src::Header(h) if h == ":stdin"));
        // a program with `from args` reads the arguments as written and parses its own flags
        let raw = e.inputs.iter().any(|i| matches!(&i.src, Src::Header(h) if h == ":args"));
        let env = e.inputs.iter().any(|i| matches!(&i.src, Src::Header(h) if h == ":env"));
        let defs = [(stdin, "MITO_CLI_STDIN"), (raw, "MITO_CLI_ARGS"), (env, "MITO_CLI_ENV")].iter().filter(|d| d.0).map(|d| format!("#define {} 1\n", d.1)).collect::<String>();
        return Ok(vec![program_c(prog, api, source)? + &defs + CLI]);
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
pub const GAME: &str = include_str!("../../runtime/game.c");
/// The command-line driver and `main` (runtime/cli.c), for programs with `@main`.
pub const CLI: &str = include_str!("../../runtime/cli.c");

/// A system as an executable's sources: its translation units, the HTTP server after the system's.
pub fn system_exe(spec: &crate::spec::Spec) -> Result<Vec<String>, String> {
    let mut units = system_c(spec)?;
    if let Some(last) = units.last_mut() { *last += HTTP; }
    Ok(units)
}

/// The C system of workers (runtime/system.c): threads, mailboxes, reply slots, forwarding.
pub const SYSTEM: &str = include_str!("../../runtime/system.c");

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
        for (k, f) in api.fns.iter().enumerate() {
            if f.recv { if let Some((_, name)) = f.name.rsplit_once('.') { m.entry(name.to_string()).or_default().push(k as u32); } }
            // a plain function a receiver-form call may also reach (`x.f()` for `fn f(x: T)`): counted too, so the analyses stay conservative
            else if f.test.is_none() && f.ptys.first().map_or(false, |t| t.is_some()) { m.entry(f.name.clone()).or_default().push(k as u32); }
        }
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
    SCALARW.with(|o| *o.borrow_mut() = scalar_write_flags(api));
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
        let (res, body, specs, errs) = loop {
            tries += 1;
            let specs = std::cell::RefCell::new(done.clone());
            let mut g = G::new(prog, api, ep, &slot_kinds, &ret_kinds, &recursive, &specs);
            let mut env: Vec<Option<CV>> = vec![None; ep.slots as usize];
            let r = g.inputs(ep, &mut env).and_then(|_| g.block(&ep.body, &mut env));
            let (widen, body, at, errs) = (g.widen, std::mem::take(&mut g.body), g.at, std::mem::take(&mut g.errs));
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
                (r, _) => break (r, body, specs.into_inner(), errs),
            }
        };
        let res = report_recovered(&mut notes, &ep.text, errs, res);
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
            Err(why) if why.is_empty() => { zeros.push(None); reads.push(false); costs.push(Costs::default()); }
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
            let (res, body, specs, errs) = loop {
                tries += 1;
                let specs = std::cell::RefCell::new(done.clone());
                let mut g = G::new(prog, api, &ep, &slot_kinds, &ret_kinds, &recursive, &specs);
                g.test = true;
                let mut env: Vec<Option<CV>> = vec![None; f.slots as usize];
                let r = g.block(&f.body, &mut env);
                let (widen, body, at, errs) = (g.widen, std::mem::take(&mut g.body), g.at, std::mem::take(&mut g.errs));
                drop(g);
                let r = r.map_err(|e| place_err(at, e));
                match (r, widen) {
                    (Err(_), Some(Widen::Slot(fr, sl, kk))) if tries < 32 => match slot_kinds.iter_mut().find(|x| x.0 == (fr, sl)) { Some(x) => x.1 = unify(x.1, kk), None => slot_kinds.push(((fr, sl), kk)) },
                    (r, _) => break (r, body, specs.into_inner(), errs),
                }
            };
            let res = report_recovered(&mut notes, &ep.text, errs, res);
            WHY.with(|w| w.borrow_mut().clear());
            match res.and_then(|_| check_profile(&body).map_err(|e| format!("C safety profile: {}", e))) {
                Ok(()) => {
                    let new = &specs[done.len()..];
                    for sp in new { src += &format!("{}int {}{};\n", linkage, sp.name, sp.proto); }
                    for sp in new { src += &format!("{}int {}{} {{\n#undef AT_\n#define AT_ 0\n    int st; u8 *b = c->base; (void)st; (void)b; (void)dep; u8 *dr_ = c->dirty; u8 *cm_ = c->committed; int jn_ = c->jon; u8 *tx_ = c->text; (void)dr_; (void)cm_; (void)jn_; (void)tx_;\n{}    return 0;\n}}\n", linkage, sp.name, sp.proto, sp.body.as_deref().unwrap_or("")); }
                    done = specs;
                    src += &format!("\n// {}\n{}int test{}(Ctx *c, Txt *tmsg) {{\n#undef AT_\n#define AT_ 0\n    int st; u8 *b = c->base; (void)st; (void)b; (void)tmsg; u8 *dr_ = c->dirty; u8 *cm_ = c->committed; int jn_ = c->jon; u8 *tx_ = c->text; (void)dr_; (void)cm_; (void)jn_; (void)tx_;\n{}    return 0;\n}}\n", ep.text, linkage, k, body);
                }
                Err(why) if why.is_empty() => {}
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
            // a `write` parameter of a number or bool given a field or an element writes it
            Expr::Call(nm, a, r) if callees(nm, r).into_iter().any(|k| a.iter().enumerate().any(|(j, x)| matches!(x, Expr::Field(..) | Expr::Index(..)) && scalar_write_at(k, j))) => true,
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

mod state; mod stores; mod stmts; mod values; mod calls; mod places; mod inline_search; mod inline_misc; mod inline_eq;
#[cfg(test)] mod profile_tests;


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
    const CALLS: &[&str] = &["if", "BP_", "TP_", "BEQ_", "BCMP_", "for", "CALL", "CALL_OR_DEAD", "TAIL", "FAIL", "ROUND_", "TEST_FAIL_", "ADD", "SUB", "MUL", "DIV", "MOD", "NEG", "UNLIKELY", "BITS_F64", "NADD", "NSUB", "NMUL", "NDIV", "NMOD", "BAND8", "BAND16", "BAND32", "BOR8", "BOR16", "BOR32", "BXOR8", "BXOR16", "BXOR32", "BAND", "BOR", "BXOR", "SHL", "SHR", "SHRL", "NSHRL", "NSHL", "NSHR", "TX_", "TXP_", "STP_", "BSTARTS_", "BENDS_", "BFIND_", "BRFIND_", "SQRT_", "SIN_", "COS_", "TAN_", "ASIN_", "ACOS_", "ATAN_", "SINH_", "COSH_", "TANH_", "LN_", "LOG2_", "LOG10_", "CEIL_", "ROUNDF_", "TRUNC_", "CBRT_", "HYPOT_", "FMOD_", "ATAN2_", "POW_", "EXP_", "FLOOR_", "FABS_", "ABS_", "SAVE_", "COMMIT_", "RANGE_", "ST1_", "ST2_", "ST4_", "ST8_", "MOVE_", "COPY_", "ZERO_", "SCRMOVE", "SCRZERO", "LD_I64", "LD_U64", "LD_I32", "LD_U32", "LD_I16", "LD_U16", "LD_I8", "LD_U8"];
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
            } else if !CALLS.contains(&tok) && !(tok.ends_with('_') && crate::lang::MATH.iter().any(|m| tok.len() == m.0.len() + 1 && tok[..tok.len() - 1].eq_ignore_ascii_case(m.0))) && !MORE_CALLS.contains(&tok) && tok != "return" && !TYPES.contains(&tok) {
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
            let mut w = std::collections::HashSet::new();
            stmt_write_locals(st, &mut w);
            if w.contains(&slot) { return false; } // handed to a `write` parameter: anything may be assigned
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
        { let mut w = std::collections::HashSet::new(); stmt_write_locals(st, &mut w); if w.contains(&slot) { return None; } }
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
/// Which parameters of each function are `write` parameters of a number or bool (`n: write i64`, `x: write f64`, `b: write bool`): such an
/// argument is a place the call may assign, which the analyses of a caller's locals have to see as an assignment.
fn scalar_write_flags(api: &Api) -> Vec<Vec<bool>> {
    api.fns.iter().map(|f| (0..f.params.len()).map(|i| f.modes.get(i).copied() == Some(1) && f.ptys.get(i).and_then(|t| t.as_ref()).map_or(false, is_scalar_pty)).collect()).collect()
}
/// A parameter type that is a number or bool by name (an integer type, `f32`, `f64`, `bool`), not nullable.
fn is_scalar_pty(t: &PTy) -> bool {
    t.arg.is_none() && t.field.is_none() && t.coll.is_none() && !t.opt && matches!(t.name.as_str(), "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64" | "bool")
}
/// An expression with no call in it (evaluating it twice is harmless).
fn no_calls(e: &Expr) -> bool {
    match e { Expr::Call(..) => false, Expr::Field(b, ..) | Expr::Un(_, b) => no_calls(b), Expr::Index(a, b) | Expr::Bin(_, a, b) => no_calls(a) && no_calls(b), _ => true }
}
fn scalar_write_at(k: u32, j: usize) -> bool { SCALARW.with(|s| s.borrow().get(k as usize).and_then(|v| v.get(j)).copied().unwrap_or(false)) }
/// The locals this expression hands to a `write` parameter of a number or bool (so a call may assign them).
fn expr_write_locals(e: &Expr, out: &mut std::collections::HashSet<u32>) {
    match e {
        Expr::Call(nm, a, r) => {
            let cs = callees(nm, r);
            for (j, x) in a.iter().enumerate() {
                if let Expr::Var(_, VarRes::Local(s)) = x { if cs.iter().any(|&k| scalar_write_at(k, j)) { out.insert(*s); } }
                expr_write_locals(x, out);
            }
        }
        Expr::Field(b, ..) | Expr::Un(_, b) => expr_write_locals(b, out),
        Expr::Index(a, b) | Expr::Bin(_, a, b) => { expr_write_locals(a, out); expr_write_locals(b, out); }
        _ => {}
    }
}
fn json_write_locals(j: &Json, out: &mut std::collections::HashSet<u32>) {
    match j {
        Json::Obj(f) => for (_, v) in f { json_write_locals(v, out); },
        Json::Arr(it, inner) => { iter_write_locals(it, out); json_write_locals(inner, out); }
        Json::E(e) => expr_write_locals(e, out),
        _ => {}
    }
}
fn iter_write_locals(it: &Iter, out: &mut std::collections::HashSet<u32>) {
    expr_write_locals(&it.coll, out);
    for e in [&it.hi, &it.from, &it.after, &it.whil, &it.wher, &it.limit].into_iter().flatten() { expr_write_locals(e, out); }
}
/// The locals handed to a `write` parameter of a number or bool by the expressions of this one statement (not those of its nested blocks).
fn stmt_write_locals(st: &Stmt, out: &mut std::collections::HashSet<u32>) {
    match st {
        Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Do(e) | Stmt::Return(Some(e)) => expr_write_locals(e, out),
        Stmt::Alloc { coll, .. } => expr_write_locals(coll, out),
        Stmt::Assign(a, b) | Stmt::Remove(a, b) => { expr_write_locals(a, out); expr_write_locals(b, out); }
        Stmt::Clear(c) => expr_write_locals(c, out),
        Stmt::Move(a, b, c, d) | Stmt::Fill(a, b, c, d) | Stmt::Sort(a, b, c, d) => for e in [a, b, c, d] { expr_write_locals(e, out); },
        Stmt::Copy(a, b, c, d, e) => for x in [a, b, c, d, e] { expr_write_locals(x, out); },
        Stmt::If(c, ..) => expr_write_locals(c, out),
        Stmt::Each(it, _) => iter_write_locals(it, out),
        Stmt::Forward { key, fields, .. } => { expr_write_locals(key, out); for f in fields { expr_write_locals(&f.1, out); } }
        Stmt::Respond(_, j) => json_write_locals(j, out),
        _ => {}
    }
}
/// The local slots assigned anywhere in these statements (nested ones included).
fn assigned_slots(stmts: &[Stmt]) -> std::collections::HashSet<u32> {
    let mut out = std::collections::HashSet::new();
    fn walk(stmts: &[Stmt], out: &mut std::collections::HashSet<u32>) {
        for st in stmts {
            stmt_write_locals(st, out);
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

/// The errors a body's statements recovered from (each statement that failed was skipped and the rest compiled), noted like a failed body's, in
/// line order; a body with any is a failure whose own error, if it has one, comes last.
fn report_recovered(notes: &mut Vec<String>, ep_text: &str, errs: Vec<String>, res: Result<(), String>) -> Result<(), String> {
    for why in &errs {
        notes.push(match split_place(why) { Some((place, rest)) => format!("{}{}: {}", place, ep_text, rest), None => format!("{}: {}", ep_text, why) });
    }
    match res { Ok(()) if !errs.is_empty() => Err(String::new()), r => r }
}
