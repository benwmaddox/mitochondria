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
pub const PRELUDE: &str = r#"typedef long long i64; typedef unsigned int u32; typedef unsigned char u8; typedef unsigned long long u64;
#define I64MAX 9223372036854775807LL
#define I64MIN (-I64MAX - 1)
typedef struct { u32 off, len; int present; } Txt;
/* tag: 0 null, 1 int (i), 2 float (f), 3 bool (i), 4 string (a = offset, b = length), 5 item (a = collection, b = index) */
typedef struct { int tag; u32 a, b; i64 i; double f; } Val;
typedef struct H H;
typedef struct { u8 *base; void *run; const H *h; u8 *dirty; u8 *committed; int jon; u8 *text; } Ctx; /* dirty, committed, jon: the undo log's line map, the page commit flags, whether the request journals: fixed for the request */
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
    int (*deref)(Ctx *, u32, u32, u32, u32 *);                 /* follow a reference (pool, slot+1, generation): index, or 0 if gone */
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
};
/* The safety profile (see `check_profile`). Endpoint bodies touch memory, do integer arithmetic and call helpers only
   through these forms. Loads under clang go through memcpy (one instruction), so reading bytes the runtime wrote is
   defined behaviour whatever the aliasing rules; TinyCC does no alias analysis, so a plain load is the same thing there. */
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
#define CALL(x) do { if ((st = (x)) != 0) return st; } while (0)
/* inside the first argument of or() and exists(): a dead reference or a slot that is not live takes the fallback */
#define CALL_OR_DEAD(x, l) do { if ((st = (x)) != 0) { if (st < 0 && c->h->was_dead(c)) goto l; return st; } } while (0)
#define TAIL(x) return (x)
#define FAIL(code, msg, n) return c->h->fail(c, (code), (msg), (n))
#define DEREF(t, s, g, out) c->h->deref(c, (t), (s), (g), (out))
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
#define EXP_(x) __builtin_exp(x)
#define FLOOR_(x) __builtin_floor(x)
#define FABS_(x) __builtin_fabs(x)
#else
double sqrt(double); double exp(double); double floor(double); double fabs(double);
#define SQRT_(x) sqrt(x)
#define EXP_(x) exp(x)
#define FLOOR_(x) floor(x)
#define FABS_(x) fabs(x)
#endif
#define ABS_(r, x) i64 r; if (UNLIKELY((x) == I64MIN)) OVERFLOW_; r = (x) < 0 ? -(x) : (x)
#define BAND(r, x, y) i64 r = (x) & (y)
#define BOR(r, x, y) i64 r = (x) | (y)
#define BXOR(r, x, y) i64 r = (x) ^ (y)
#define SHL(r, x, y) i64 r; if (UNLIKELY((u64)(y) > 63)) FAIL(500, "shift count out of range", 24); r = (i64)((u64)(x) << (y))
#define SHR(r, x, y) i64 r; if (UNLIKELY((u64)(y) > 63)) FAIL(500, "shift count out of range", 24); r = (x) >> (y)
#define NSHL(r, x, y) i64 r = (i64)((u64)(x) << (y))
#define NSHR(r, x, y) i64 r = (x) >> (y)
#define NADD(r, x, y) i64 r = (x) + (y)
#define NSUB(r, x, y) i64 r = (x) - (y)
#define NMUL(r, x, y) i64 r = (x) * (y)
#define NDIV(r, x, y) i64 r = (x) / (y)
#define NMOD(r, x, y) i64 r = (x) % (y)
#define TX_(i) (tx_[(i)]) /* a byte of the request's text area, where strings that are not in state live */
/* MITO_MEASURE_NO_JOURNAL, MITO_MEASURE_NO_COMMIT and MITO_MEASURE_NO_RANGE are for measuring what the tests cost only: without the undo log a failed
   request is not rolled back; without commit tests all state is committed at start; without range tests an index out of range is undefined behaviour */
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
#else
#define ST1_(a, v) (*(u8 *)(b + (a)) = (u8)(v))
#define ST2_(a, v) (*(u16 *)(b + (a)) = (u16)(v))
#define ST4_(a, v) (*(u32 *)(b + (a)) = (u32)(v))
#define ST8_(a, v) (*(u64 *)(b + (a)) = (u64)(v))
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
pub const BUILTINS: [B; 17] = [B::Body, B::Param, B::Query, B::Header, B::Fwd, B::Int, B::Str, B::Len, B::Now, B::Random, B::Token, B::Stat, B::WorkerIndex, B::WorkerCount, B::Min, B::Max, B::Slot];

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
            CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) => Shape::Str, CV::Null => Shape::Null,
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
            Shape::Null => write!(f, "null"), Shape::Any => write!(f, "any"), Shape::Nothing => write!(f, "none"),
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
                "string" => *shape == Shape::Str,
                "none" => *shape == Shape::Nothing,
                // an item type: the answer holds an item of a pool of that struct, written as its id
                _ if matches!(shape, Shape::Id(..)) => { let Shape::Id(c, _) = shape else { unreachable!() }; prog.colls[*c].item.map_or(false, |s| prog.structs[s].name == *n) }
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
pub struct Generated { pub src: String, pub notes: Vec<String>, pub lits: Vec<u8> }

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
    StateStr(usize, String, usize), // a string column of an item: collection, item index, column
    Plain(usize, usize),            // a plain struct: struct id, offset
    Part(usize, String, String),    // a struct-valued field of an item: collection, item index, path of the field
    ArrCol(usize, String, usize),   // a fixed-array field of an item: collection, item index, column
    PlainArr(usize, Ty, usize),     // a plain fixed array: offset, element type, length
    Null,
}

/// Where `return` goes inside an inlined function, and the kind of value it returns (known after the first `return`).
/// `func`: the return of a generated C function, which writes its result through `ret_v` (and `ret_s`) and returns 0.
/// `sel`: a mark taken on entry when the function has loops, so items a loop collected are dropped however it returns.
struct Ret { var: String, label: String, kind: Option<K>, func: bool, sel: Option<String> }

/// How an argument is passed to a generated C function: a value of a kind, or an instance of a collection.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PK { V(K), Coll(usize), Plain(usize, usize) }

/// A C function generated for a recursive function, for one combination of argument kinds. `ret` is the kind it
/// returns: while its body is being generated, the kind being tried.
/// The table is the program's: an endpoint reuses the functions earlier endpoints generated, and each is emitted once.
#[derive(Clone)]
struct Spec { key: String, name: String, ret: K, proto: String, body: Option<String> }

/// The interpreter refuses a call nested deeper than this.
const MAX_DEPTH: u32 = 64;
const DEPTH_MSG: &str = "calls nested more than 64 deep";

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
    /// The string literals of the program, each once, back to back: the runtime keeps them at the start of every
    /// request's text area, so a literal in generated code is a constant (offset, length) and costs nothing to use.
    static LITS: std::cell::RefCell<(Vec<u8>, Vec<(u32, u32)>)> = const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
    /// Why a kind was widened to one known only at run time, while the kinds of an endpoint are being worked out.
    static WHY: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}
fn why(m: String) { WHY.with(|w| { let mut w = w.borrow_mut(); if !w.contains(&m) { w.push(m); } }) }

/// A kind as the language names it, for errors.
fn kname(prog: &Program, k: K) -> String {
    match k {
        K::Int => "an integer".into(), K::Flt => "a number".into(), K::Bool => "a bool".into(), K::Txt => "a string".into(), K::Null => "null".into(),
        K::Item(c) | K::Ref(c) => format!("an item of {}", prog.colls[c].name), K::Any => "a value of any kind".into(),
        _ => "a collection or struct".into(),
    }
}

/// A kind that has to be widened, found while generating: the endpoint is generated again with it.
#[derive(Clone, Copy, Debug)]
enum Widen { Slot(Option<u32>, u32, K), Ret(u32, K) }

struct G<'p> {
    prog: &'p Program, api: &'p Api, ep: &'p Endpoint, body: String, n: usize, depth: usize, inlining: Vec<u32>, ret: Vec<Ret>,
    frame: Option<u32>,                                  // the function whose slots `env` holds (None: the endpoint)
    slot_kinds: &'p [((Option<u32>, u32), K)],           // widened kinds of `var` and `let` slots
    ret_kinds: &'p [(u32, K)],                           // widened result kinds of functions
    widen: Option<Widen>,
    var_names: std::collections::HashSet<String>, // the C names of `var`s: a `let` of one must copy it
    fast_sites: std::collections::HashSet<*const Expr>, // array indexes a versioned loop's guard proved in range (their stores' ranges already saved)
    versioned: u32, // how many versioned loops enclose the code being generated (each doubles its body; at most 3 deep)
    loop_hint: Option<(i64, i64)>, // the next range loop's counter interval, proven by a versioning guard (its fast copy)
    checked: Vec<(String, u64)>, // range tests done since the last statement start, block boundary, label or goto: (index, size) need not be tested again
    skip_save: bool, // the next inline store needs no undo-log test (its range was saved by a loop guard)
    txt_escapes: Vec<u32>, // slots of variables that were assigned a string (or a value of unknown kind), in the order it happened
    recursive: &'p [bool],                               // per function: is it part of a cycle of calls
    specs: &'p std::cell::RefCell<Vec<Spec>>,            // C functions generated for recursive functions
    dep: bool,                                           // generating a C function: its call depth is the parameter `dep`
    inl: u32,                                            // inlined calls the code being generated is nested in
    ranges: std::collections::HashMap<String, (i64, i64)>, // C names that never change after they are set (inputs, loop counters, results of arithmetic) with an inclusive interval their value is proven to lie in
    or_dead: Option<String>,                             // inside or()/exists(): where a dead reference or slot not live goes
}
type R<T> = Result<T, String>;
fn no<T>(what: impl Into<String>) -> R<T> { Err(what.into()) }
fn is_name(e: &str) -> bool { e.chars().all(|c| c.is_alphanumeric() || c == '_') }

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

pub fn generate(prog: &Program, api: &Api) -> Generated { generate_with(prog, api, None, "") }

/// Does a program load? Its api parses, every name binds, and every endpoint compiles; the error is the first problem.
pub fn check(prog: &Program, api_src: &str) -> Result<Api, String> {
    let mut api = parse(api_src)?;
    bind(&mut api, &|n| prog.global(n))?;
    if let Some(n) = generate(prog, &api).notes.first() { return Err(n.clone()); }
    Ok(api)
}

/// The C runtime (runtime/*.c), compiled in the same translation unit as a program.
pub const RUNTIME: &str = include_str!("../runtime/mito.c");
/// The runtime's types for the layout tables, ahead of them.
pub const RUNTIME_TYPES: &str = include_str!("../runtime/types.h");

/// A whole program as one C translation unit: the prelude, the layout and routes as data, the C runtime, and the
/// endpoints. `mito_*` (runtime/mito.c) is its interface. Fails if an endpoint does not compile.
pub fn program_c(prog: &Program, api: &Api, source: Option<(&str, &str)>) -> Result<String, String> { program_c_named(prog, api, source, "mito_") }

/// The same, its interface named `prefix`start, `prefix`handle and so on (programs of a system share one build).
pub fn program_c_named(prog: &Program, api: &Api, source: Option<(&str, &str)>, prefix: &str) -> Result<String, String> {
    let g = generate_with(prog, api, source, "static ");
    if let Some(n) = g.notes.first() { return Err(n.clone()); }
    // The profile was checked on `c->h->helper`; in the unit itself the helper table is the runtime's own constant
    // `mito_h`, so the optimizer sees the calls go straight to the helpers and can inline them.
    let direct = |s: &str| s.replace("c->h->", "mito_h.");
    let mut o = String::from("#if defined(__AVX2__)
#include <immintrin.h>
#endif
"); // vector compares, only when the build enables AVX2 (`--cflag -mavx2`)
    o += &direct(PRELUDE);
    o += RUNTIME_TYPES;
    o += &format!("#define MITO(x) {}##x\n", prefix);
    o += &layout_c(prog, api);
    o += &format!("/* the program's string literals, at the start of every request's text area */
#define MITO_LITS_LEN {}
static const unsigned char mito_lits[] = {{ {} }};
", g.lits.len(), if g.lits.is_empty() { "0".to_string() } else { g.lits.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",") });
    o += RUNTIME;
    o += &direct(&g.src[PRELUDE.len()..]);
    Ok(o)
}

/// The HTTP server and `main` (runtime/http.c), for executables.
pub const HTTP: &str = include_str!("../runtime/http.c");

/// One program as an executable's source: the program, then its HTTP server.
pub fn program_exe(prog: &Program, api: &Api, source: Option<(&str, &str)>) -> Result<Vec<String>, String> {
    Ok(vec![program_c(prog, api, source)? + "#define HTTP_SINGLE\n" + HTTP])
}

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
    table += "#if defined(_MSC_VER) || defined(__clang__)\n#define MITO_API __declspec(dllexport)\n#else\n#define MITO_API\n#endif\n";
    table += "typedef struct { const char *k; u32 kn; const char *v; u32 vn; } MitoPair;\n";
    table += "typedef struct { const char *name; u32 count, mailbox, slot; u64 soft, hard; int marks;\n    void *(*start)(u64, u64); void (*stop)(void *); void (*set_clock)(void *, i64, int); void (*set_marks)(void *, u64, u64);\n    int (*handle)(void *, const char *, u32, const char *, u32, const MitoPair *, u32, const MitoPair *, u32, const char *, u32);\n    const char *(*response)(void *, u32 *); void (*set_system)(void *, u32, const char *const *, const u32 *, u32, u32);\n    u32 (*forward_to)(void *, u32 *, i64 *, MitoPair *, u32); void (*finish_forward)(void *, int); int (*crashed)(void *); } SysKind;\n";
    let fns = ["start", "stop", "set_clock", "set_marks", "handle", "response", "set_system", "forward_to", "finish_forward", "crashed"];
    let sigs = ["void *{}start(u64, u64);", "void {}stop(void *);", "void {}set_clock(void *, i64, int);", "void {}set_marks(void *, u64, u64);",
        "int {}handle(void *, const char *, u32, const char *, u32, const MitoPair *, u32, const MitoPair *, u32, const char *, u32);", "const char *{}response(void *, u32 *);",
        "void {}set_system(void *, u32, const char *const *, const u32 *, u32, u32);", "u32 {}forward_to(void *, u32 *, i64 *, MitoPair *, u32);", "void {}finish_forward(void *, int);", "int {}crashed(void *);"];
    let mut rows = Vec::new();
    let mut total = 0usize;
    for (i, k) in spec.kinds.iter().enumerate() {
        let prog = Program::parse(&k.state).map_err(|e| format!("worker {}: {}", k.name, e))?;
        total = total.saturating_add(prog.total.saturating_mul(k.count));
        let mut api = crate::lang::parse(&k.api).map_err(|e| format!("worker {}: {}", k.name, e))?;
        crate::lang::bind(&mut api, &|n| prog.global(n)).map_err(|e| format!("worker {}: {}", k.name, e))?;
        let pre = format!("k{}_", i);
        units.push(program_c_named(&prog, &api, None, &pre).map_err(|e| format!("worker {}: {}", k.name, e))?);
        for s in &sigs { table += &s.replace("{}", &pre); table.push('\n'); }
        let f: Vec<String> = fns.iter().map(|f| format!("{}{}", pre, f)).collect();
        rows.push(format!("    {{ {}, {}, {}, {}, {}ULL, {}ULL, {}, {} }}", c_lit(k.name.as_bytes()), k.count, k.mailbox, k.slot, k.soft.unwrap_or(0), k.hard.unwrap_or(usize::MAX), k.soft.is_some() as i32,
            f.iter().zip(["(void *(*)(u64, u64))", "", "", "", "", "", "", "", "", ""]).map(|(n, c)| format!("{}{}", c, n)).collect::<Vec<_>>().join(", ")));
    }
    const MB: usize = 1 << 20;
    if total > spec.budget { return Err(format!("over budget: {} workers need {} MB, budget is {} MB", spec.kinds.iter().map(|k| k.count).sum::<usize>(), total / MB, spec.budget / MB)); }
    table += &format!("static const SysKind SYS_KINDS[] = {{\n{}\n}};\n#define SYS_NKINDS {}\n#define SYS_ENTRY {}\n#define SYS_INFLIGHT {}\n", rows.join(",\n"), spec.kinds.len(), spec.entry, spec.inflight);
    units.push(table + SYSTEM);
    Ok(units)
}

/// The layout and the routes as C data for the runtime: collections with their columns and nested collections,
/// endpoints with their path segments.
pub fn layout_c(prog: &Program, api: &Api) -> String {
    let mut o = String::from("\n/* ---- the program's layout and routes (generated) ---- */\n");
    for (k, c) in prog.colls.iter().enumerate() {
        o += &format!("static const MitoCol mito_cols{}[] = {{\n", k);
        for col in &c.cols {
            let (kind, sc, n, target) = match &col.kind {
                CKind::Scalar(s) => (0, plain_code(&Ty::Scalar(*s)).unwrap().0, 0, 0),
                CKind::Arr(s, n) => (1, plain_code(&Ty::Scalar(*s)).unwrap().0, *n as u32, 0),
                CKind::StrLen => (2, 0, 0, 0),
                CKind::StrBytes(m) => (3, 0, *m as u32, 0),
                CKind::Ref(t) => (4, 0, 0, *t as u32),
            };
            o += &format!("    {{ {}, {}, {}, {}, {}, {}, {} }},\n", c_lit(col.path.as_bytes()), kind, sc, n, col.off, col.kind.size(), target);
        }
        o += "    { 0, 0, 0, 0, 0, 0, 0 }\n};\n";
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
        for sg in &ep.segs { match sg { Seg::Lit(l) => o += &format!(" {{ 0, {} }},", c_lit(l.as_bytes())), Seg::Param(n) => o += &format!(" {{ 1, {} }},", c_lit(n.as_bytes())) } }
        o += " { 0, 0 } };\n";
    }
    o += "static const MitoRoute mito_routes[] = {\n";
    for (k, ep) in api.eps.iter().enumerate() { o += &format!("    {{ {}, {}, mito_segs{} }},\n", c_lit(ep.method.as_bytes()), ep.segs.len(), k); }
    o += "    { 0, 0, 0 }\n};\n";
    for k in 0..api.eps.len() { o += &format!("static int ep{}(Ctx *c);\n", k); }
    o += "typedef int (*EpFn)(Ctx *);\nstatic const EpFn mito_eps[] = {";
    for k in 0..api.eps.len() { o += &format!(" ep{},", k); }
    o += " 0 };\n";
    o += "static const u64 mito_unlogged[] = {";
    for (a, b) in &prog.unlogged { o += &format!(" {}ULL, {}ULL,", a, b); }
    o += " 0, 0 };\n";
    o += &format!("static const MitoProg mito_prog = {{ {}, mito_colls, {}, {}, {}, mito_routes, {}, mito_unlogged }};\n", prog.colls.len(), prog.plain_end, prog.total, api.eps.len(), prog.unlogged.len());
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
            let t = match &col.kind { CKind::Scalar(s) => format!("{:?}", s).to_lowercase(), CKind::Arr(s, n) => format!("{:?}[{}]", s, n).to_lowercase(), CKind::StrLen => "string length u32".into(), CKind::StrBytes(n) => format!("string bytes [{}]", n), CKind::Ref(t) => format!("ref to coll {} (slot+1 u32, generation u32)", t) };
            o += &format!("     col {:<2} {:<22} {:<40} at {} + idx * {}\n", ci, col.path, t, col.off, col.kind.size());
        }
    }
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

/// Do these statements call (directly or through other functions) one of instance `inst`'s functions that changes state?
fn changes_instance(api: &Api, inst: &str, stmts: &[Stmt]) -> bool {
    fn ex(e: &Expr, out: &mut Vec<u32>) {
        match e {
            Expr::Call(_, a, r) => { if let CallRes::User(k) = r { out.push(*k); } for x in a { ex(x, out); } }
            Expr::Field(b, ..) | Expr::Un(_, b) => ex(b, out),
            Expr::Index(a, b) | Expr::Bin(_, a, b) => { ex(a, out); ex(b, out); }
            _ => {}
        }
    }
    fn js(j: &Json, out: &mut Vec<u32>) { match j { Json::Obj(f) => for (_, v) in f { js(v, out); }, Json::Arr(it, inner) => { ex(&it.coll, out); if let Some(w) = &it.wher { ex(w, out); } js(inner, out); } Json::E(e) => ex(e, out), Json::Empty => {} } }
    fn st(stmts: &[Stmt], out: &mut Vec<u32>) {
        for s in stmts {
            match s {
                Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(e, out),
                Stmt::Remove(c, e) => { ex(c, out); ex(e, out); }
                Stmt::Clear(c) => ex(c, out),
                Stmt::Move(a, f, t2, n) => { ex(a, out); ex(f, out); ex(t2, out); ex(n, out); }
                Stmt::Fill(a, f, t2, n) => { ex(a, out); ex(f, out); ex(t2, out); ex(n, out); }
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
fn has_loop(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s { Stmt::Each(..) => true, Stmt::If(_, t, e) => has_loop(t) || has_loop(e), _ => false })
}

/// Which functions are part of a cycle of calls (directly or through others).
fn recursive_fns(api: &Api) -> Vec<bool> {
    fn calls(stmts: &[Stmt], out: &mut Vec<u32>) {
        fn ex(e: &Expr, out: &mut Vec<u32>) {
            match e {
                Expr::Call(_, a, r) => { if let CallRes::User(k) = r { out.push(*k); } for x in a { ex(x, out); } }
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
                Stmt::Fill(a, f, t2, n) => { ex(a, out); ex(f, out); ex(t2, out); ex(n, out); }
                Stmt::Alloc { coll, .. } => ex(coll, out),
                Stmt::Assign(a, b) => { ex(a, out); ex(b, out); }
                Stmt::If(c, t, e) => { ex(c, out); calls(t, out); calls(e, out); }
                Stmt::Each(it, b) => { ex(&it.coll, out); if let Some(h) = &it.hi { ex(h, out); } if let Some(w) = &it.wher { ex(w, out); } if let Some(l) = &it.limit { ex(l, out); } calls(b, out); }
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
                Json::Empty => {}
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

/// `linkage`: "static " in a whole C program (only its `mito_*` interface is external), "" when the Rust runtime looks
/// endpoints up by name.
fn generate_with(prog: &Program, api: &Api, source: Option<(&str, &str)>, linkage: &str) -> Generated {
    LITS.with(|l| *l.borrow_mut() = (Vec::new(), Vec::new()));
    let mut src = String::from(PRELUDE);
    let mut notes = Vec::new();
    let recursive = recursive_fns(api);
    let mut done: Vec<Spec> = Vec::new(); // functions for recursive calls, generated by endpoints that compiled
    for (k, ep) in api.eps.iter().enumerate() {
        let (mut slot_kinds, mut ret_kinds): (Vec<((Option<u32>, u32), K)>, Vec<(u32, K)>) = (Vec::new(), Vec::new());
        let mut tries = 0;
        let (res, body, specs) = loop {
            tries += 1;
            let specs = std::cell::RefCell::new(done.clone());
            let mut g = G::new(prog, api, ep, &slot_kinds, &ret_kinds, &recursive, &specs);
            let mut env: Vec<Option<CV>> = vec![None; ep.slots as usize];
            let r = g.inputs(ep, &mut env).and_then(|_| g.block(&ep.body, &mut env));
            let (widen, body) = (g.widen, std::mem::take(&mut g.body));
            drop(g);
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
                    src += &format!("\n// functions for recursive calls, first used by {}, one per combination of argument kinds\n", ep.text);
                    for sp in new { src += &format!("{}int {}{};\n", linkage, sp.name, sp.proto); }
                    for sp in new { src += &format!("{}int {}{} {{\n    int st; u8 *b = c->base; (void)st; (void)b; (void)dep; u8 *dr_ = c->dirty; u8 *cm_ = c->committed; int jn_ = c->jon; u8 *tx_ = c->text; (void)dr_; (void)cm_; (void)jn_; (void)tx_;\n{}    return 0;\n}}\n", linkage, sp.name, sp.proto, sp.body.as_deref().unwrap_or("")); }
                }
                done = specs;
                src += &format!("{}{}int {}(Ctx *c) {{\n    int st; u8 *b = c->base; (void)st; (void)b; u8 *dr_ = c->dirty; u8 *cm_ = c->committed; int jn_ = c->jon; u8 *tx_ = c->text; (void)dr_; (void)cm_; (void)jn_; (void)tx_;\n{}    return 0;\n}}\n", head, linkage, name, body);
            }
            Err(why) => {
                if let Some(sf) = source { src += &source_comment(ep, sf); src += &format!("// not compiled: {}\n", why); }
                notes.push(format!("{}: {}", ep.text, why));
            }
        }
    }
    let lits = LITS.with(|l| l.borrow().0.clone());
    Generated { src, notes, lits }
}

/// What generating a C function's body for one guess of its result kind gave.
enum SpecErr { Conflict(K), Other(String) }

/// Does running these statements (including inlined calls) write state or allocate? A loop with `where` whose body
/// does collects its items first, as the interpreter does, so the body cannot change which items the filter keeps.
fn writes(api: &Api, stmts: &[Stmt], seen: &mut Vec<u32>) -> bool {
    fn ex(api: &Api, e: &Expr, seen: &mut Vec<u32>) -> bool {
        match e {
            Expr::Call(_, a, r) => a.iter().any(|x| ex(api, x, seen)) || match r { CallRes::User(k) if !seen.contains(k) => { seen.push(*k); writes(api, &api.fns[*k as usize].body, seen) } _ => false },
            Expr::Field(b, ..) | Expr::Un(_, b) => ex(api, b, seen),
            Expr::Index(a, b) | Expr::Bin(_, a, b) => ex(api, a, seen) || ex(api, b, seen),
            _ => false,
        }
    }
    stmts.iter().any(|s| match s {
        Stmt::Alloc { .. } | Stmt::Move(..) | Stmt::Fill(..) | Stmt::Clear(..) | Stmt::Assign(Expr::Field(..) | Expr::Index(..), _) => true,
        Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(api, e, seen),
        Stmt::Remove(c, e) => ex(api, c, seen) || ex(api, e, seen),
        Stmt::Clear(c) => ex(api, c, seen),
        Stmt::Move(a, f, t2, n) => ex(api, a, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
        Stmt::Fill(a, f, t2, n) => ex(api, a, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
        Stmt::Assign(a, b) => ex(api, a, seen) || ex(api, b, seen),
        Stmt::If(c, t, e) => ex(api, c, seen) || writes(api, t, seen) || writes(api, e, seen),
        Stmt::Each(it, b) => ex(api, &it.coll, seen) || it.hi.as_ref().map_or(false, |h| ex(api, h, seen)) || writes(api, b, seen),
        Stmt::Forward { key, fields, .. } => ex(api, key, seen) || fields.iter().any(|f| ex(api, &f.1, seen)),
        _ => false,
    })
}

/// Does running these statements (including inlined calls) allocate? A loop whose body allocates collects its items
/// first, as the interpreter does, so it does not visit items its body adds.
fn allocates(api: &Api, stmts: &[Stmt], seen: &mut Vec<u32>) -> bool {
    fn ex(api: &Api, e: &Expr, seen: &mut Vec<u32>) -> bool {
        match e {
            Expr::Call(_, a, r) => a.iter().any(|x| ex(api, x, seen)) || match r { CallRes::User(k) if !seen.contains(k) => { seen.push(*k); allocates(api, &api.fns[*k as usize].body, seen) } _ => false },
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
        Stmt::Fill(a, f, t2, n) => ex(api, a, seen) || ex(api, f, seen) || ex(api, t2, seen) || ex(api, n, seen),
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
        CV::Int(_) => K::Int, CV::Flt(_) => K::Flt, CV::Bool(_) => K::Bool, CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) => K::Txt,
        CV::Any(_) => K::Any, CV::Item(c, _) => K::Item(*c), CV::Ref(c, ..) => K::Ref(*c), CV::Null => K::Null,
        CV::Coll(c, _) => K::Coll(*c), CV::Plain(s, o) => K::Plain(*s, *o),
        CV::Part(..) | CV::ArrCol(..) | CV::PlainArr(..) => return None,
    })
}
/// Can a value of kind `v` be stored where kind `k` is held? (Collections and structs are places, not values: they fit
/// only themselves.)
fn fits(v: K, k: K) -> bool { if matches!(v, K::Coll(_) | K::Plain(..)) { return v == k; } k == K::Any || unify(v, k) == k }

impl<'p> G<'p> {
    #[allow(clippy::too_many_arguments)]
    fn new(prog: &'p Program, api: &'p Api, ep: &'p Endpoint, slot_kinds: &'p [((Option<u32>, u32), K)], ret_kinds: &'p [(u32, K)], recursive: &'p [bool], specs: &'p std::cell::RefCell<Vec<Spec>>) -> G<'p> {
        G { prog, api, ep, body: String::new(), n: 0, depth: 0, inlining: Vec::new(), ret: Vec::new(), frame: None, slot_kinds, ret_kinds, widen: None, var_names: std::collections::HashSet::new(), fast_sites: std::collections::HashSet::new(), skip_save: false, checked: Vec::new(), loop_hint: None, versioned: 0, txt_escapes: Vec::new(), recursive, specs, dep: false, inl: 0, ranges: std::collections::HashMap::new(), or_dead: None }
    }

    /// An endpoint's typed inputs, read and checked before its body, into slots 0.. in order. For a request with a body,
    /// its fields must be among the body inputs: an unknown field is 400.
    fn inputs(&mut self, ep: &'p Endpoint, env: &mut Vec<Option<CV>>) -> R<()> {
        if matches!(ep.method.as_str(), "POST" | "PUT" | "PATCH") || ep.inputs.iter().any(|i| i.src == Src::Body) {
            let names: String = ep.inputs.iter().filter(|i| i.src == Src::Body).map(|i| format!("{}\0", i.name)).collect();
            self.call(&format!("c->h->fields(c, {}, {})", c_lit(names.as_bytes()), names.len()));
        }
        for (k, inp) in ep.inputs.iter().enumerate() {
            let src = match &inp.src { Src::Body => 0, Src::Path => 1, Src::Query => 2, Src::Header(_) => 3, Src::Fwd => 4 };
            let name = match &inp.src { Src::Header(h) => h.clone(), _ => inp.name.clone() };
            let (kind, lo, hi, pool) = match &inp.ty {
                InTy::Int(a, b) => (1, *a, *b, 0),
                InTy::Flt => (2, 0, 0, 0),
                InTy::Bool => (3, 0, 0, 0),
                InTy::Str(a, b) => (4, *a, *b, 0),
                InTy::Item(t, p) => { let c = self.item_pool(&inp.name, t, p.as_deref())?; (if matches!(inp.src, Src::Header(_)) { 5 } else { 6 }, c as i64, 0, c) }
            };
            let v = self.tmp("in");
            self.line(&format!("Val {};", v));
            self.call(&format!("c->h->input(c, {}, {}, {}, {}, {}, {}, &{})", src, c_lit(name.as_bytes()), name.len(), kind, i64_lit(lo), i64_lit(hi), v));
            // absent: the default, null when optional, else 400
            match (&inp.default, inp.optional) {
                (Some(d), _) => { let dv = self.expr(d, env, None)?; let dv = self.val(dv)?; self.line(&format!("if ({}.tag == 0) {} = {};", v, v, dv)); }
                (None, true) => {}
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
                InTy::Str(..) => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, {}.tag == 4 }};", t, v, v, v)); CV::Txt(t) }
                InTy::Item(..) => { let x = self.tmp("i"); self.line(&format!("u32 {} = {}.b;", x, v)); CV::Item(pool, x) }
            };
            env[k] = Some(cv);
        }
        Ok(())
    }
    /// The pool an input of item type `t` names: the one top-level pool of `t` items, or `in` the one given.
    fn item_pool(&self, input: &str, t: &str, pool: Option<&str>) -> R<usize> {
        let Some(sid) = self.prog.structs.iter().position(|s| s.name == t) else { return no(format!("input `{}`: `{}` is not a type", input, t)) };
        let found: Vec<usize> = self.prog.colls.iter().enumerate().filter(|(_, c)| c.item == Some(sid) && c.parent.is_none() && matches!(c.kind, Kind::Pool(_)) && pool.map_or(true, |p| c.name == p)).map(|(i, _)| i).collect();
        match found.as_slice() {
            [c] => Ok(*c),
            [] => no(format!("input `{}`: no pool that exists once holds `{}`{}", input, t, pool.map_or(String::new(), |p| format!(" as {}", p)))),
            _ => no(format!("input `{}`: `{}` is kept in more than one pool; say which: `{}: {} in {}`", input, t, input, t, self.prog.colls[found[0]].name)),
        }
    }

    /// The module instance whose function is being generated (None: the program's own code).
    fn module_code(&self) -> Option<String> { let k = *self.inlining.last()?; let n = &self.api.fns[k as usize].name; n.rfind('.').map(|i| n[..i].to_string()) }
    /// The instance a collection belongs to: (instance, its items or nested in them (the program's data), its items exactly).
    fn coll_owner(&self, c: usize) -> Option<(&'p crate::layout::ModInst, bool, bool)> {
        let name = &self.prog.colls[c].name;
        let m = self.prog.instances.iter().find(|m| name.starts_with(&format!("{}.", m.name)))?;
        let items = format!("{}.items", m.name);
        Some((m, *name == items || name.starts_with(&format!("{}.", items)), *name == items))
    }
    fn module_err(&self, m: &crate::layout::ModInst, what: &str) -> String { format!("{} belongs to {} (module {}); change it through {}'s functions", what, m.name, m.module, m.name) }
    /// Writing column `path` of collection `c` from here: refused outside a module for its own state, and for its key.
    fn may_write_col(&self, c: usize, path: &str) -> R<()> {
        let Some((m, data, items)) = self.coll_owner(c) else { return Ok(()) };
        if self.module_code().as_deref() == Some(m.name.as_str()) { return Ok(()); }
        if !data { return Err(self.module_err(m, &format!("`{}`", self.prog.colls[c].name))); }
        if items && m.key.as_deref() == Some(path) { return Err(format!("`{}` is the key of {}; change it with {}.set_key(item, value)", path, m.name, m.name)); }
        Ok(())
    }
    /// Writing the plain value at `off` from here: an instance's plain state is its own.
    fn may_write_plain(&self, off: usize) -> R<()> {
        for m in &self.prog.instances {
            let Some((start, size)) = self.prog.plain_path(&m.name) else { continue };
            if off >= start && off < start + size && self.module_code().as_deref() != Some(m.name.as_str()) { return Err(self.module_err(m, &format!("the state of {}", m.name))); }
        }
        Ok(())
    }
    /// Adding or freeing items of collection `c` from here: an instance's items and state change only through its functions.
    fn may_add_or_free(&self, c: usize, verb: &str) -> R<()> {
        let Some((m, data, items)) = self.coll_owner(c) else { return Ok(()) };
        if self.module_code().as_deref() == Some(m.name.as_str()) || (data && !items) { return Ok(()); }
        Err(format!("{} items of {} with its functions, not `{}.{}()`", verb, m.name, self.prog.colls[c].name, verb))
    }
    /// A field store or free through a value whose kind is known only at run time: refused outside module code when it
    /// could reach a module's key or private state.
    fn may_write_dynamic(&self, field: Option<&str>) -> R<()> {
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

    /// Call the C function for recursive function `k` with these argument values, generating it if needed.
    fn call_spec(&mut self, k: u32, vals: Vec<CV>) -> R<CV> {
        let (mut pks, mut cargs) = (Vec::new(), Vec::new());
        for v in vals {
            match v {
                CV::Coll(c, inst) => { pks.push(PK::Coll(c)); cargs.push(format!("(u32)({})", inst)); }
                v @ (CV::Lit(_) | CV::StateStr(..) | CV::Txt(_)) => { let t = self.txt_of(v)?; pks.push(PK::V(K::Txt)); cargs.push(t); }
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
        let (name, ret) = { let sp = &self.specs.borrow()[si]; (sp.name.clone(), sp.ret) };
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
        Ok(out)
    }

    /// Generate the C function for recursive function `k` with argument kinds `pks`. Its result kind is found by trying
    /// kinds in turn (null, int, bool, float, string, then any kind a body's returns asked for, then any value): a kind
    /// is kept when the body, generated with recursive calls returning that kind, returns only values of it.
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
                PK::V(K::Txt) => proto += &format!(", Txt p{}", i),
                PK::V(K::Any) => proto += &format!(", Val p{}", i),
                PK::V(K::Null) | PK::Plain(..) => {}
                PK::V(K::Coll(_)) => proto += &format!(", u32 p{}", i),
                PK::V(K::Plain(..)) => {}
            }
        }
        self.specs.borrow_mut().push(Spec { key, name, ret: K::Null, proto: String::new(), body: None });
        let mut tries = vec![K::Null, K::Int, K::Bool, K::Flt, K::Txt];
        let mut t = 0;
        while t < tries.len() {
            let cand = tries[t];
            t += 1;
            { let mut sp = self.specs.borrow_mut(); sp.truncate(si + 1); sp[si].ret = cand; } // functions generated under an earlier guess go
            match self.spec_body(k, pks, cand) {
                Ok(body) => {
                    let rp = match cand { K::Null | K::Plain(..) => "", K::Int => ", i64 *ret_v", K::Flt => ", double *ret_v", K::Bool => ", int *ret_v", K::Txt => ", Txt *ret_v", K::Any => ", Val *ret_v", K::Item(_) | K::Coll(_) => ", u32 *ret_v", K::Ref(_) => ", u32 *ret_s, u32 *ret_v" };
                    let mut sp = self.specs.borrow_mut();
                    sp[si].proto = format!("{}{})", proto, rp);
                    sp[si].body = Some(body);
                    return Ok(si);
                }
                Err(SpecErr::Conflict(u)) => {
                    let next = unify(cand, u);
                    if !tries.contains(&next) { if next == K::Any { tries.push(K::Any); } else { tries.insert(t, next); } }
                    else if !tries.contains(&K::Any) { tries.push(K::Any); }
                }
                Err(SpecErr::Other(e)) => { self.specs.borrow_mut().truncate(si); return Err(e); }
            }
        }
        self.specs.borrow_mut().truncate(si);
        no("no result kind fits a recursive function")
    }

    /// The body of a C function for `k`, assuming its result kind is `cand`; widens its own locals as endpoints do.
    fn spec_body(&mut self, k: u32, pks: &[PK], cand: K) -> Result<String, SpecErr> {
        let f = &self.api.fns[k as usize];
        let (mut slot_kinds, mut ret_kinds): (Vec<((Option<u32>, u32), K)>, Vec<(u32, K)>) = (Vec::new(), Vec::new());
        for _ in 0..32 {
            let mut g = G::new(self.prog, self.api, self.ep, &slot_kinds, &ret_kinds, self.recursive, self.specs);
            g.dep = true;
            g.frame = Some(k);
            g.inlining.push(k);
            let sel = if has_loop(&f.body) { Some("hm".to_string()) } else { None };
            if sel.is_some() { g.line("u32 hm;"); g.call("c->h->sel_mark(c, &hm)"); }
            g.ret.push(Ret { var: String::new(), label: String::new(), kind: Some(cand), func: true, sel: sel.clone() });
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
                (Ok(()), _) if !ends_by_itself && !fits(K::Null, cand) => return Err(SpecErr::Conflict(K::Null)), // falling off the end returns null
                (Ok(()), _) => return check_profile(&g.body).map(|_| g.body).map_err(|e| SpecErr::Other(format!("C safety profile: {}", e))),
                (Err(_), Some(Widen::Ret(f2, u))) if f2 == k => return Err(SpecErr::Conflict(u)),
                (Err(_), Some(Widen::Slot(fr, sl, kk))) => match slot_kinds.iter_mut().find(|x| x.0 == (fr, sl)) { Some(x) => x.1 = unify(x.1, kk), None => slot_kinds.push(((fr, sl), kk)) },
                (Err(_), Some(Widen::Ret(f2, kk))) => match ret_kinds.iter_mut().find(|x| x.0 == f2) { Some(x) => x.1 = unify(x.1, kk), None => ret_kinds.push((f2, kk)) },
                (Err(e), None) => return Err(SpecErr::Other(e)),
            }
        }
        Err(SpecErr::Other("kinds did not settle".into()))
    }

    fn tmp(&mut self, p: &str) -> String {
        self.n += 1;
        let t = format!("{}{}", p, self.n);
        if ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"].contains(&t.as_str()) { format!("{}_{}", p, self.n) } else { t } // never a type's name
    }
    fn line(&mut self, s: &str) {
        if s.ends_with(": ;") || s.contains("goto ") { self.checked.clear(); } // control joins or leaves here: a test seen before may not have run
        for _ in 0..self.depth + 1 { self.body.push_str("    "); } self.body.push_str(s); self.body.push('\n');
    }
    /// The interval a C integer is proven to lie in: a literal, or a name recorded in `ranges` (names that are never assigned again).
    fn interval(&self, s: &str) -> Option<(i64, i64)> {
        if let Some(d) = s.strip_suffix("LL") { if let Ok(v) = d.parse::<i64>() { return Some((v, v)); } }
        self.ranges.get(s).copied()
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
    fn unlogged_at(&self, off: usize) -> bool { self.prog.unlogged.iter().any(|(a, b)| off >= *a && off < *b) }
    /// A value read from a field declared with a range: the range analysis may assume it.
    fn with_field_range(&mut self, v: CV, range: Option<(i64, i64)>) -> CV {
        match (v, range) {
            (CV::Int(e), Some(r)) => { let n = self.hold_int(e); if !self.var_names.contains(&n) { self.ranges.insert(n.clone(), r); } CV::Int(n) }
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
        self.line(&format!("if (RANGE_({k} < 0 || {k} >= {n}LL)) {{"));
        self.depth += 1;
        self.call(&format!("c->h->elem(c, {}, {})", k, n));
        self.depth -= 1;
        self.line("}");
        self.checked.push((k.to_string(), n));
    }
    /// Generate a loop statement (the loop, its body, and the text-area mark and cut when the body can build a string).
    fn loop_stmt(&mut self, it: &'p Iter, body: &'p [Stmt], env: &mut Vec<Option<CV>>, collect: bool) -> R<()> {
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

    /// Loop versioning. For a range loop (`each v in lo..hi`), and for a loop that steps a variable (`while j <= limit` with one
    /// `j = j + step` at the top level of the body), find the accesses to plain-state arrays whose index is the counter (or the
    /// stepped variable) plus or minus a value the loop does not change. Emit, before the loop, a test that every such index
    /// stays inside its array for the whole loop, and save the ranges the loop stores into to the undo log in one pass when it
    /// does. Returns the test's C name and the accesses the fast copy may do without their own range test and undo-log test;
    /// None when the loop has no such access. The checked copy runs when the test fails, so behaviour is unchanged.
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
            for site in sites {
                let CV::PlainArr(off, e, n) = self.expr(site.arr, env, None)? else { continue };
                let esz = self.prog.plain_size(&e).0;
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
                self.line(&format!("if ({guard} && {nonempty} && !({smin} >= 0 && {smax} < {n}LL)) {guard} = 0;"));
                if site.store { saves.push((nonempty, format!("{}ULL + (u64)({}) * {}ULL", off, smin, esz), format!("(u64)({} - {} + 1) * {}ULL", smax, smin, esz))); }
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
            self.line(&format!("if (RANGE_({q} < 0 || {q} >= {cap}LL || {q} >= (i64)LD_U32({hdr}, {inst}) || !((LD_U64({live}, (i64)({inst}) * {words} + ({q} >> 6)) >> ({q} & 63)) & 1))) {{"));
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
        match self.or_dead.clone() { Some(l) => self.line(&format!("CALL_OR_DEAD({}, {});", s, l)), None => self.line(&format!("CALL({});", s)) }
    }
    fn fail(&mut self, code: u16, m: &str) { self.line(&format!("FAIL({}, {}, {});", code, c_lit(m.as_bytes()), m.len())); }
    fn open(&mut self, head: &str) { self.checked.clear(); self.line(head); self.depth += 1; }
    fn close(&mut self, tail: &str) { self.checked.clear(); self.depth -= 1; self.line(tail); }

    fn block(&mut self, stmts: &'p [Stmt], env: &mut Vec<Option<CV>>) -> R<()> {
        for s in stmts { self.checked.clear(); self.stmt(s, env)?; }
        Ok(())
    }

    fn stmt(&mut self, s: &'p Stmt, env: &mut Vec<Option<CV>>) -> R<()> {
        match s {
            Stmt::Let(_, slot, e) | Stmt::Var(_, slot, e) => {
                let mutable = matches!(s, Stmt::Var(..));
                let v = self.expr(e, env, None)?;
                let v = match self.slot_kind(*slot) { Some(k) => self.coerce(v, k, mutable)?, None => self.hold(v, mutable)? };
                env[*slot as usize] = Some(v);
            }
            Stmt::Assign(Expr::Var(n, VarRes::Local(slot)), rhs) => {
                let cur = env[*slot as usize].clone().ok_or_else(|| format!("`{}` has no compiled value", n))?;
                let v = self.expr(rhs, env, None)?;
                if matches!(cur, CV::Txt(_) | CV::Any(_)) { self.txt_escapes.push(*slot); }
                if let Err(e) = self.assign_var(&cur, v.clone()) {
                    if let (Some(a), Some(b)) = (kind(&cur), kind(&v)) {
                        if unify(a, b) == K::Any { why(format!("`{}` holds {} and then {}: a var keeps the kind of its first value", n, kname(self.prog, a), kname(self.prog, b))); }
                        self.widen = Some(Widen::Slot(self.frame, *slot, unify(a, b)));
                    }
                    return Err(e);
                }
            }
            Stmt::Assign(Expr::Field(base, f), rhs) => {
                let bv = self.expr(base, env, None)?;
                let (c, idx, path) = match bv {
                    CV::Item(c, idx) => (c, idx, f.clone()),
                    CV::Part(c, idx, p) => (c, idx, format!("{}.{}", p, f)),
                    CV::Ref(t, s, k) => { self.line(&format!("if ({} == 0) TAIL(c->h->dead(c));", s)); (t, k, f.clone()) }
                    CV::Plain(sid, off) => {
                        let sd = &self.prog.structs[sid];
                        let Some(k) = sd.fields.iter().position(|x| x.0 == *f) else { return no(format!("no field `{}`", f)) };
                        let (code, cap) = match &sd.fields[k].1 { Ty::Ref(p) => (30, self.prog.ref_pool(p).ok_or("ref to an unknown pool")? as u32), t => plain_code(t).ok_or_else(|| format!("assigning plain `.{}` not compiled yet", f))? };
                        let at = off + sd.offs[k];
                        let range = sd.ranges[k];
                        self.may_write_plain(at)?;
                        let v = self.expr(rhs, env, None)?;
                        let v = match range { Some(r) => self.in_field_range(v, r, f)?, None => v };
                        self.skip_save = self.unlogged_at(at);
                        if self.scalar_store(code, format!("{}ULL", at), &v, &format!("c->h->store_plain(c, {}, {}, {}, &@VAL)", at, code, cap), true, at % scalar_bytes(code) as usize == 0)? { return Ok(()); }
                        self.skip_save = false;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store_plain(c, {}, {}, {}, &{})", at, code, cap, vv));
                        return Ok(());
                    }
                    CV::Null => { self.line("TAIL(c->h->dead(c));"); return Ok(()); }
                    CV::Any(x) => {
                        if self.prog.structs.iter().any(|sd| sd.fields.iter().zip(&sd.ranges).any(|(fd, r)| fd.0 == *f && r.is_some())) {
                            return no(format!("`{}` has a declared range, and this value's collection is only known when the program runs; assign it through a value of a known collection", f));
                        }
                        self.may_write_dynamic(Some(f))?;
                        self.line(&format!("if ({}.tag == 0) TAIL(c->h->dead(c));", x));
                        self.line(&format!("if ({}.tag != 5) FAIL(500, \"assignment target is not an item or a state struct\", 50);", x));
                        let v = self.expr(rhs, env, None)?;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store_field(c, &{}, {}, {}, &{})", x, c_lit(f.as_bytes()), f.len(), vv));
                        return Ok(());
                    }
                    _ => { self.fail(500, "assignment target is not an item or a state struct"); return Ok(()); }
                };
                let Some(ci) = self.prog.colls[c].cols.iter().position(|x| x.path == path && !matches!(x.kind, CKind::StrBytes(_) | CKind::Arr(..))) else {
                    let prefix = format!("{}.", path);
                    let known = self.prog.colls[c].cols.iter().any(|x| x.path == path || x.path.starts_with(&prefix)) || self.prog.colls[c].children.iter().any(|x| x.0 == path);
                    return no(if known { format!("assigning `.{}` as a whole is not supported yet; assign its fields", path) } else { format!("no field `{}`", path) });
                };
                self.may_write_col(c, &path)?;
                let v = self.expr(rhs, env, None)?;
                let v = match self.prog.colls[c].cols[ci].range { Some(r) => self.in_field_range(v, r, &path)?, None => v };
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
                        let esz = self.prog.plain_size(&e).0;
                        self.skip_save = covered || self.unlogged_at(off);
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
                let cv = self.expr(c, env, None)?;
                let cond = self.cond(cv)?;
                self.open(&format!("if ({}) {{", cond));
                self.block(t, env)?;
                self.close("}");
                if !e.is_empty() { self.open("else {"); self.block(e, env)?; self.close("}"); }
            }
            Stmt::Each(it, body) => {
                if let (Some(_), Some(n)) = (&it.via, crate::lang::path_of(&it.coll)) {
                    let n = &n;
                    if changes_instance(self.api, n, body) { return no(format!("a loop over {} calls a function that changes {}; change it after the loop", n, n)); }
                }
                let collect = allocates(self.api, body, &mut Vec::new()) || (it.wher.is_some() && writes(self.api, body, &mut Vec::new()));
                match self.version_guard(it, body, env)? {
                    Some((guard, sites, hints, counter)) => {
                        // the fast copy: the guard proved every listed array access of the loop in range and saved the stored ranges to the undo log
                        self.versioned += 1;
                        self.open(&format!("if ({}) {{", guard));
                        let joined: std::collections::HashSet<*const Expr> = sites.union(&self.fast_sites).copied().collect();
                        let kept = std::mem::replace(&mut self.fast_sites, joined);
                        let ranges_kept = self.ranges.clone();
                        for (name, iv) in hints { self.ranges.insert(name, iv); }
                        self.loop_hint = counter;
                        self.loop_stmt(it, body, env, collect)?;
                        self.loop_hint = None;
                        self.ranges = ranges_kept;
                        self.fast_sites = kept;
                        self.close("}");
                        self.open("else {");
                        self.loop_stmt(it, body, env, collect)?;
                        self.close("}");
                        self.versioned -= 1;
                    }
                    None => self.loop_stmt(it, body, env, collect)?,
                }
            }
            Stmt::Move(arr, from, to, count) => {
                let av = self.expr(arr, env, None)?;
                let mut pos = |g: &mut Self, e: &'p Expr, env: &mut Vec<Option<CV>>| -> R<String> { let v = g.expr(e, env, None)?; let k = g.int_or(v, 400, "a position and a count must be integers"); Ok(g.hold_int(k)) };
                let (f, t2, n) = (pos(self, from, env)?, pos(self, to, env)?, pos(self, count, env)?);
                let (base, esize, len) = match av {
                    CV::ArrCol(c, idx, ci) => {
                        self.may_write_col(c, &self.prog.colls[c].cols[ci].path.clone())?;
                        let col = &self.prog.colls[c].cols[ci];
                        let CKind::Arr(_, len) = col.kind else { unreachable!() };
                        (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), col.kind.size() / len, len)
                    }
                    CV::PlainArr(off, e, len) => { self.may_write_plain(off)?; (format!("{}ULL", off), self.prog.plain_size(&e).0, len) }
                    _ => return no("`move` works on a fixed array"),
                };
                self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {len}LL || {t2} < 0 || {t2} > {len}LL || {n} < 0 || {n} > {len}LL || {f} + {n} > {len}LL || {t2} + {n} > {len}LL)) FAIL(400, \"move out of range\", 17);"));
                self.call(&format!("c->h->move_elems(c, {}, {}, {}, {}, {})", base, esize, f, t2, n));
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
                    CV::PlainArr(off, e, len) => { self.may_write_plain(off)?; (format!("{}ULL", off), plain_code(&e).map_or(0, |x| x.0), len, 1) }
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
                let CV::Coll(c, inst) = self.expr(coll, env, None)? else { return no("`clear` is a function of a collection") };
                self.may_add_or_free(c, "clear")?;
                self.call(&format!("c->h->clear_coll(c, {}, {})", c, inst));
            }
            Stmt::Remove(coll, e) => {
                let CV::Coll(want, _) = self.expr(coll, env, None)? else { return no("`remove` is a function of a collection") };
                let wname = self.prog.colls[want].name.clone();
                let other = |c: usize| no(format!("an item of {} cannot be removed from {}", self.prog.colls[c].name, wname));
                match self.expr(e, env, None)? {
                    CV::Item(c, i) => { if c != want { return other(c); } self.may_add_or_free(c, "remove")?; self.call(&format!("c->h->free_(c, {}, {})", c, i)) }
                    CV::Ref(t, s, k) => { if t != want { return other(t); } self.may_add_or_free(t, "remove")?; self.line(&format!("if ({} != 0) {{ CALL(c->h->free_(c, {}, {})); }}", s, t, k)) }
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
            Stmt::Respond(code, j) => {
                let shape = self.json(j, env)?;
                if let Err(e) = fits_shape(self.prog, &shape, &self.ep.resp, "") { return no(format!("the answer is {}, not Response<{}>: {}", shape, show_rtype(&self.ep.resp), e)); }
                self.line(&format!("return {};", code));
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
                self.may_add_or_free(c, "add")?;
                let t = self.tmp("i");
                self.line(&format!("u32 {};", t));
                self.call(&format!("c->h->alloc(c, {}, {}, {}, &{})", c, inst, *try_ as i32, t));
                env[*slot as usize] = Some(if *try_ {
                    // try_alloc gives an item or null: keep it as a value
                    let v = self.tmp("v");
                    self.line(&format!("Val {} = {{ {} == 0xFFFFFFFFu ? 0 : 5, {}, {}, 0, 0 }};", v, t, c, t));
                    CV::Any(v)
                } else { CV::Item(c, t) });
            }
        }
        Ok(())
    }

    fn ret_with(&mut self, v: CV) -> R<()> {
        let v = self.hold(v, false)?;
        let Some(kv) = kind(&v) else { return no("a function returning this kind of value is not compiled yet") };
        let r = self.ret.last().unwrap();
        let (var, label, f) = (r.var.clone(), r.label.clone(), *self.inlining.last().unwrap());
        let k = match r.kind { None => { self.ret.last_mut().unwrap().kind = Some(kv); kv } Some(k) => k };
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
            (K::Txt, v @ (CV::Txt(_) | CV::Lit(_) | CV::StateStr(..))) => { let x = self.txt_of(v)?; if mutable || self.var_names.contains(&x) { let t = self.tmp("t"); self.line(&format!("Txt {} = {};", t, x)); CV::Txt(t) } else { CV::Txt(x) } }
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
            CV::Int(e) if !shared(self, &e) => { let t = self.tmp("v"); self.line(&format!("i64 {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Int(t) }
            CV::Flt(e) if !shared(self, &e) => { let t = self.tmp("v"); self.line(&format!("double {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Flt(t) }
            CV::Bool(e) if !shared(self, &e) => { let t = self.tmp("v"); self.line(&format!("int {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Bool(t) }
            CV::Item(c, e) if !shared(self, &e) => { let t = self.tmp("i"); self.line(&format!("u32 {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Item(c, t) }
            CV::Txt(e) if mutable || self.var_names.contains(&e) => { let t = self.tmp("t"); self.line(&format!("Txt {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Txt(t) }
            CV::Any(e) if mutable || self.var_names.contains(&e) => { let t = self.tmp("v"); self.line(&format!("Val {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Any(t) }
            CV::Lit(s) if mutable => { let t = self.txt_of(CV::Lit(s))?; self.var_names.insert(t.clone()); CV::Txt(t) }
            CV::StateStr(..) => { let t = self.txt_of(v)?; if mutable { self.var_names.insert(t.clone()); } CV::Txt(t) } // a value is copied when read, as in the interpreter
            CV::Null if mutable => { let t = self.tmp("v"); self.line(&format!("Val {} = {{ 0, 0, 0, 0, 0 }};", t)); self.var_names.insert(t.clone()); CV::Any(t) }
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
    fn hold_int(&mut self, e: String) -> String { if is_name(&e) { e } else { let t = self.tmp("v"); self.line(&format!("i64 {} = {};", t, e)); t } }

    fn assign_var(&mut self, cur: &CV, v: CV) -> R<()> {
        match (cur, v) {
            (CV::Int(t), CV::Int(e)) | (CV::Bool(t), CV::Bool(e)) | (CV::Flt(t), CV::Flt(e)) => self.line(&format!("{} = {};", t, e)),
            (CV::Item(a, t), CV::Item(b, e)) if *a == b => self.line(&format!("{} = {};", t, e)),
            (CV::Txt(t), v @ (CV::Txt(_) | CV::Lit(_) | CV::StateStr(..))) => { let s = self.txt_of(v)?; self.line(&format!("{} = {};", t, s)); }
            (CV::Any(t), v) => { let s = self.val(v)?; self.line(&format!("{} = {};", t, s)); }
            (CV::Txt(t), CV::Null) => self.line(&format!("{} = (Txt){{ 0, 0, 0 }};", t)),
            (CV::Ref(a, s, k), CV::Ref(b, s2, k2)) if *a == b => self.line(&format!("{} = {}; {} = {};", s, s2, k, k2)),
            (CV::Ref(a, s, k), CV::Item(b, e)) if *a == b => self.line(&format!("{} = 1; {} = {};", s, k, e)),
            (CV::Ref(_, s, _), CV::Null) => self.line(&format!("{} = 0;", s)),
            _ => return no("a `var` assigned a value of another kind"),
        }
        Ok(())
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
            v @ (CV::Lit(_) | CV::Txt(_) | CV::StateStr(..)) => { let x = self.txt_of(v)?; self.line(&format!("Val {} = {{ {}.present ? 4 : 0, {}.off, {}.len, 0, 0 }};", t, x, x, x)); }
            CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::PlainArr(..) => return no("a collection, struct or array used as a value is not compiled yet"),
        }
        Ok(t)
    }

    fn txt_of(&mut self, v: CV) -> R<String> {
        match v {
            CV::Txt(t) => Ok(t),
            CV::Lit(s) => { let t = self.tmp("t"); let off = intern_lit(s.as_bytes()); self.line(&format!("Txt {} = {{ {}, {}, 1 }};", t, off, s.len())); Ok(t) }
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
            Expr::Var(n, VarRes::Local(s)) => env.get(*s as usize).cloned().flatten().ok_or_else(|| format!("`{}` has no compiled value", n)),
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
                    CV::PlainArr(off, Ty::Scalar(sc), n) => {
                        let k = self.hold_int(k);
                        if !self.fast_sites.contains(&(&**k_expr as *const Expr)) { self.elem_check(&k, n as u64); }
                        self.load(&off.to_string(), &k, sc)
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

    /// A plain (record-style) value at offset `off`.
    fn plain(&mut self, off: usize, t: &Ty, dead: Option<&str>) -> R<CV> {
        Ok(match t {
            Ty::Struct(s) => CV::Plain(*s, off),
            Ty::Scalar(s) => self.load(&off.to_string(), "0", *s)?,
            Ty::Str(_) => { let x = self.tmp("t"); self.line(&format!("Txt {};", x)); self.call(&format!("c->h->plain_txt(c, {}, &{})", off, x)); CV::Txt(x) }
            Ty::Arr(e, n) => CV::PlainArr(off, (**e).clone(), *n),
            Ty::Ref(p) => { let t = self.prog.ref_pool(p).ok_or("ref to an unknown pool")?; self.load_ref(t, &off.to_string(), "0", dead)? }
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
        let (c, idx, path) = match base {
            CV::Item(c, idx) => (c, idx, f.to_string()),
            CV::Part(c, idx, p) => (c, idx, format!("{}.{}", p, f)),
            CV::Ref(t, s, k) => {
                // reading through a null reference is a dead reference, as in the interpreter
                let on_dead = match dead { Some(l) => format!("goto {};", l), None => "TAIL(c->h->dead(c));".into() };
                self.line(&format!("if ({} == 0) {}", s, on_dead));
                (t, k, f.to_string())
            }
            CV::Plain(sid, off) => {
                let sd = &self.prog.structs[sid];
                let Some(k) = sd.fields.iter().position(|x| x.0 == f) else { return no(format!("no field `{}`", f)) };
                let ty = sd.fields[k].1.clone();
                let range = sd.ranges[k];
                if ty.is_coll() { return Ok(CV::Coll(self.prog.plain_coll(off, sid, f).ok_or("plain collection")?, "0".into())); }
                let v = self.plain(off + sd.offs[k], &ty, dead)?;
                return Ok(self.with_field_range(v, range));
            }
            CV::Null => { match dead { Some(l) => self.line(&format!("goto {};", l)), None => self.line("TAIL(c->h->dead(c));") } return Ok(CV::Null); }
            CV::Any(x) => {
                // an item whose collection is known only at run time: the interpreter's own field lookup
                if let Some(l) = dead { self.line(&format!("if ({}.tag == 0) goto {};", x, l)); }
                let r = self.tmp("v");
                self.line(&format!("Val {};", r));
                self.call(&format!("c->h->field_val(c, &{}, {}, {}, &{})", x, c_lit(f.as_bytes()), f.len(), r));
                return Ok(CV::Any(r));
            }
            _ => { self.fail(500, &format!("`.{}` needs an item or a state struct", f)); return Ok(CV::Null); }
        };
        let coll = &self.prog.colls[c];
        if let Some(k) = coll.children.iter().find(|x| x.0 == path) { return Ok(CV::Coll(k.1, idx)); }
        if let Some(ci) = coll.cols.iter().position(|x| x.path == path && !matches!(x.kind, CKind::StrBytes(_))) { return self.col_value(c, idx, ci, dead); }
        let inner = format!("{}.", path);
        if coll.cols.iter().any(|x| x.path.starts_with(&inner)) || coll.children.iter().any(|x| x.0.starts_with(&inner)) { return Ok(CV::Part(c, idx, path)); }
        no(format!("no field `{}`", path))
    }

    /// The value of column `ci` of item `idx`: scalars are loaded, strings are read where used, references followed.
    fn col_value(&mut self, c: usize, idx: String, ci: usize, dead: Option<&str>) -> R<CV> {
        let col = self.prog.colls[c].cols[ci].clone();
        Ok(match col.kind {
            CKind::Scalar(s) => { let v = self.load(&col.off.to_string(), &idx, s)?; self.with_field_range(v, col.range) }
            CKind::StrLen => CV::StateStr(c, idx, ci),
            CKind::Ref(t) => self.load_ref(t, &col.off.to_string(), &idx, dead)?,
            CKind::Arr(..) => CV::ArrCol(c, idx, ci),
            CKind::StrBytes(_) => unreachable!(),
        })
    }

    /// What an item of collection `c` is as a value: the item, or for a collection of plain values, its value.
    fn item_value(&mut self, c: usize, idx: String, dead: Option<&str>) -> R<CV> {
        let coll = &self.prog.colls[c];
        if let Some(k) = coll.children.iter().find(|x| x.0.is_empty()) { return Ok(CV::Coll(k.1, idx)); }
        match coll.cols.iter().position(|x| x.path.is_empty()) { Some(ci) => self.col_value(c, idx, ci, dead), None => Ok(CV::Item(c, idx)) }
    }

    /// Follow a reference stored at `off` + item `idx` * 8 (slot+1, then generation) to pool `t`.
    fn load_ref(&mut self, t: usize, off: &str, idx: &str, dead: Option<&str>) -> R<CV> {
        let (s, gv, k) = (self.tmp("r"), self.tmp("g"), self.tmp("i"));
        self.line(&format!("u32 {} = LD_U32({}, (i64)({}) * 2), {} = LD_U32({} + 4, (i64)({}) * 2), {} = 0;", s, off, idx, gv, off, idx, k));
        let on_dead = match dead { Some(l) => format!("goto {};", l), None => "TAIL(c->h->dead(c));".into() };
        self.line(&format!("if ({} != 0 && !DEREF({}, {} - 1, {}, &{})) {}", s, t, s, gv, k, on_dead));
        Ok(CV::Ref(t, s, k))
    }

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
                (CV::Null, CV::Int(_) | CV::Flt(_) | CV::Bool(_) | CV::Lit(_) | CV::Item(..) | CV::StateStr(..)) | (CV::Int(_) | CV::Flt(_) | CV::Bool(_) | CV::Lit(_) | CV::Item(..) | CV::StateStr(..), CV::Null) => Some("0".into()),
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
                "&" | "|" | "^" | "<<" | ">>" => {
                    let (ix, iy) = (self.interval(&x), self.interval(&y));
                    let nonneg = |i: Option<(i64, i64)>| i.filter(|(l, _)| *l >= 0);
                    match op {
                        "&" => {
                            // with a non-negative operand the result is between 0 and that operand's largest value, whatever the other is
                            let hi = [nonneg(ix), nonneg(iy)].iter().flatten().map(|(_, h)| *h).min();
                            self.line(&format!("BAND({}, {}, {});", r, x, y));
                            if let Some(h) = hi { self.ranges.insert(r.clone(), (0, h)); }
                        }
                        "|" | "^" => {
                            self.line(&format!("{}({}, {}, {});", if op == "|" { "BOR" } else { "BXOR" }, r, x, y));
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
                            self.line(&format!("{}{}({}, {}, {});", if counted { "N" } else { "" }, if left { "SHL" } else { "SHR" }, r, x, y));
                            if let (Some((xl, xh)), Some((sl, sh))) = (ix, iy) {
                                if counted {
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
                    } else {
                        self.line(&format!("{}({}, {}, {});", mac, r, x, y));
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
        let kind = |v: &CV| match v { CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) => "string", CV::Int(_) => "i64", CV::Flt(_) => "f64", CV::Bool(_) => "bool", CV::Null => "null", CV::Any(_) => "?", _ => "an item or collection" };
        let (lk, rk) = (kind(&l), kind(&r));
        let joined = op == "+" && (lk == "string" || rk == "string") && [lk, rk].iter().all(|k| matches!(*k, "string" | "i64" | "f64" | "bool" | "null"));
        let compared = matches!(op, "<" | "<=" | ">" | ">=" | "==" | "!=") && lk == "string" && rk == "string";
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
        let (lv, rv) = (self.val(l)?, self.val(r)?);
        let t = self.tmp("v");
        self.line(&format!("Val {};", t));
        self.call(&format!("c->h->binop(c, {}, &{}, &{}, &{})", code, lv, rv, t));
        if joined { let x = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, 1 }};", x, t, t)); return Ok(CV::Txt(x)); }
        if compared { return Ok(CV::Bool(format!("({}.i != 0)", t))); }
        Ok(CV::Any(t))
    }

    fn callx(&mut self, n: &str, args: &'p [Expr], res: CallRes, env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        match res {
            CallRes::User(k) => self.inline(k, args, env, dead),
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
                        let av = self.val(a?)?;
                        self.line(&format!("if ({}.tag == 0) goto {};", av, l)); // null also takes the fallback
                        self.line(&format!("{} = {}; goto {};", res, av, end));
                        self.line("}");
                        self.line(&format!("{}: {{", l));
                        let fb = self.expr(&args[1], env, dead)?;
                        let fv = self.val(fb)?;
                        self.line(&format!("{} = {};", res, fv));
                        self.line("}");
                        self.line(&format!("{}: ;", end));
                        Ok(CV::Any(res))
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
        let generic = match bi {
            B::Min | B::Max => !matches!(a.as_slice(), [CV::Int(_), CV::Int(_)]),
            B::Slot => !matches!(a.first(), Some(CV::Item(..) | CV::Ref(..))),
            _ => BUILTINS[..14].contains(&bi),
        };
        if let (true, Some(code)) = (generic, BUILTINS.iter().position(|x| *x == bi)) {
            // arguments as one Val array; the interpreter's builtin computes the result
            let mut names = Vec::new();
            for v in a { names.push(self.val(v)?); }
            let argv = self.tmp("argv");
            if names.is_empty() { self.line(&format!("Val {}[1];", argv)); } else { self.line(&format!("Val {}[{}] = {{ {} }};", argv, names.len(), names.join(", "))); }
            let r = self.tmp("v");
            self.line(&format!("Val {};", r));
            self.call(&format!("c->h->builtin(c, {}, {}, {}, &{})", code, argv, names.len(), r));
            let _ = n;
            let _ = int_out;
            return Ok(match bi {
                B::Int | B::Len | B::Now | B::Random | B::Stat | B::WorkerIndex | B::WorkerCount | B::Min | B::Max | B::Slot => CV::Int(format!("{}.i", r)),
                B::Str | B::Token => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, 1 }};", t, r, r)); CV::Txt(t) }
                B::Param | B::Query | B::Header => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, {}.tag == 4 }};", t, r, r, r)); CV::Txt(t) }
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
                let (Some(CV::Int(x)), Some(CV::Int(y))) = (it.next(), it.next()) else { return no("min/max of something other than integers") };
                let (x, y) = (self.hold_int(x), self.hold_int(y));
                CV::Int(format!("({} {} {} ? {} : {})", x, if bi == B::Min { "<" } else { ">" }, y, x, y))
            }
            B::Count => {
                let Some(CV::Coll(c, inst)) = a.first() else { return no("count(collection)") };
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
                Some(CV::Item(c, i)) => CV::Int(format!("(i64)({} % {})", i, self.prog.colls[*c].cap)),
                Some(CV::Ref(c, s, i)) => { self.line(&format!("if ({} == 0) FAIL(500, \"slot(item)\", 10);", s)); CV::Int(format!("(i64)({} % {})", i, self.prog.colls[*c].cap)) }
                _ => unreachable!("generic above"),
            },
            B::Sqrt | B::Exp | B::Floor => {
                // IEEE results, as the platform's math library gives them (a negative square root is NaN)
                let name = match bi { B::Sqrt => "sqrt", B::Exp => "exp", _ => "floor" };
                let x = match a.as_slice() { [CV::Flt(e)] => e.clone(), [CV::Int(e)] => format!("(double)({})", e), _ => return no(format!("`{}` takes one number", name)) };
                CV::Flt(format!("{}_({})", name.to_uppercase(), x))
            }
            B::Substr => {
                // a piece of a string, `substr(text, from, count)`: it shares the text's bytes (no copy), the range is checked
                let [text, from, count] = <[CV; 3]>::try_from(a).map_err(|_| "`substr` takes a string, a start and a count".to_string())?;
                let t = self.txt_of(text)?;
                let (from, count) = (self.int_or(from, 400, "a start and a count must be integers"), self.int_or(count, 400, "a start and a count must be integers"));
                let (from, count) = (self.hold_int(from), self.hold_int(count));
                self.line(&format!("if (UNLIKELY({from} < 0 || {count} < 0 || {from} + {count} > (i64){t}.len)) FAIL(400, \"substring out of range\", 22);"));
                let r = self.tmp("t");
                self.line(&format!("Txt {r} = {{ (u32)({t}.off + {from}), (u32){count}, 1 }};"));
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
            B::Crash => { self.line("TAIL(c->h->crash(c));"); CV::Null }
            _ => unreachable!("handled above"),
        })
    }

    /// Inline a user function: its parameters become values in a fresh frame; `return` jumps to the end of the inlined block.
    fn inline(&mut self, k: u32, args: &'p [Expr], env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        let mut vals = Vec::new();
        for a in args { let v = self.expr(a, env, dead)?; vals.push(self.hold(v, false)?); }
        self.call_values(k, vals, dead)
    }

    /// Call user function `k` with argument values already computed: inlined, or a generated C function if recursive.
    fn call_values(&mut self, k: u32, vals: Vec<CV>, dead: Option<&str>) -> R<CV> {
        let f = &self.api.fns[k as usize];
        self.depth_check();
        if self.recursive[k as usize] {
            return self.call_spec(k, vals);
        }
        let mut fenv: Vec<Option<CV>> = vec![None; f.slots as usize];
        for (i, v) in vals.into_iter().enumerate() { fenv[i] = Some(v); }
        self.inl += 1;
        let r = self.inline_body(k, f, &mut fenv, dead);
        self.inl -= 1;
        r
    }

    fn inline_body(&mut self, k: u32, f: &'p Func, fenv: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        // a function that is just `return e;` needs no block
        if let [Stmt::Return(Some(body))] = f.body.as_slice() {
            self.inlining.push(k);
            let r = self.expr(body, fenv, dead);
            self.inlining.pop();
            return r;
        }
        let (var, label) = (self.tmp("ret"), self.tmp("end"));
        let saved = std::mem::take(&mut self.body);
        self.inlining.push(k);
        let forced = self.ret_kinds.iter().find(|x| x.0 == k).map(|x| x.1);
        let sel = if has_loop(&f.body) { Some(self.tmp("hm")) } else { None };
        self.ret.push(Ret { var: var.clone(), label: label.clone(), kind: forced, func: false, sel: sel.clone() });
        self.depth += 1;
        let frame = std::mem::replace(&mut self.frame, Some(k));
        let r = self.block(&f.body, fenv);
        self.frame = frame;
        self.depth -= 1;
        let ret = self.ret.pop().unwrap();
        self.inlining.pop();
        let inner = std::mem::replace(&mut self.body, saved);
        r?;
        let ends_by_itself = matches!(f.body.last(), Some(Stmt::Return(_)) | Some(Stmt::Fail(..)) | Some(Stmt::Respond(..)) | Some(Stmt::Forward { .. }));
        let kind = ret.kind.unwrap_or(K::Null);
        if !ends_by_itself && kind != K::Null {
            // falling off the end returns null, as in the interpreter
            if !fits(K::Null, kind) {
            if unify(kind, K::Null) == K::Any { why(format!("`{}` returns {} on one path and nothing on another: return a value on every path", self.api.fns[k as usize].name, kname(self.prog, kind))); }
            self.widen = Some(Widen::Ret(k, unify(kind, K::Null))); return no("a function that can end without returning its value");
        }
        }
        let (decl, out) = match kind {
            K::Int => (format!("i64 {}_v = 0;", var), CV::Int(format!("{}_v", var))),
            K::Flt => (format!("double {}_v = 0;", var), CV::Flt(format!("{}_v", var))),
            K::Bool => (format!("int {}_v = 0;", var), CV::Bool(format!("{}_v", var))),
            K::Txt => (format!("Txt {}_v = {{ 0, 0, 0 }};", var), CV::Txt(format!("{}_v", var))),
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
    fn each(&mut self, it: &'p Iter, env: &mut Vec<Option<CV>>, collect: bool, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        if let Some(v) = it.via { return self.walk(it, v, env, body); }
        if let Some(hi) = &it.hi { return self.range_loop(it, hi, env, body); }
        let CV::Coll(c, inst) = self.expr(&it.coll, env, None)? else { return no("each over something other than a collection") };
        let coll = self.prog.colls[c].clone();
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
            body(self, env, &hits)?;
            self.close("}");
            self.close("}");
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
        body(self, env, &hits)?;
        self.close("}");
        self.call(&format!("c->h->sel_end(c, {})", h));
        self.close("}");
        Ok(())
    }

    /// `each s in instance [from k] [reverse] ...`: walk a module instance with its iteration functions, at most its
    /// capacity of steps, stopping at a null cursor or the limit. Order of evaluation as in the interpreter: `from`,
    /// `limit`, then the first cursor.
    fn walk(&mut self, it: &'p Iter, v: crate::lang::Via, env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        let Some(n) = crate::lang::path_of(&it.coll) else { return no("walking something other than a module instance") };
        let steps = self.prog.colls.iter().find(|c| c.name == format!("{}.items", n)).map_or(0, |c| c.cap);
        let from = match &it.from { Some(e) => { let x = self.expr(e, env, None)?; self.hold(x, false)? } None => CV::Null };
        let lim = match &it.limit { Some(e) => { let x = self.expr(e, env, None)?; Some(self.int_or(x, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        // a cursor is an integer, -1 when there is none
        let c0 = self.call_values(if it.reverse { v.last.ok_or("no reverse walk")? } else { v.first }, vec![from], None)?;
        let c0 = self.int_or(c0, 500, "a walk's cursor is an integer (-1 for none)");
        let (cur, k, hits) = (self.tmp("cur"), self.tmp("k"), self.tmp("hits"));
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
        body(self, env, &hits)?;
        if filtered { self.close("}"); }
        let nx = self.call_values(if it.reverse { v.prev.ok_or("no reverse walk")? } else { v.next }, vec![CV::Int(cur.clone())], None)?;
        let nx = self.int_or(nx, 500, "a walk's cursor is an integer (-1 for none)");
        self.line(&format!("{} = {};", cur, nx));
        self.close("}");
        self.close("}");
        Ok(())
    }

    /// `each i in lo..hi`: the bounds are read once, values are computed as the loop goes (where is tested just before
    /// each body), and `limit` counts the bodies run.
    fn range_loop(&mut self, it: &'p Iter, hi: &'p Expr, env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        let v = self.expr(&it.coll, env, None)?;
        let lo = self.int_or(v, 500, "a range needs integers");
        let lo = self.hold_int(lo);
        let v = self.expr(hi, env, None)?;
        let hi = self.int_or(v, 500, "a range needs integers");
        let hi = self.hold_int(hi);
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        let (k, hits) = (self.tmp("k"), self.tmp("hits"));
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
        env[it.slot as usize] = Some(CV::Int(k.clone()));
        // the counter is never assigned in the body: between the smallest lower bound and one below the largest upper bound
        if let (Some((lo_min, _)), Some((_, hi_max))) = (self.interval(&lo), self.interval(&hi)) { if hi_max > lo_min { self.ranges.insert(k.clone(), (lo_min, hi_max - 1)); } }
        if let Some(h) = self.loop_hint.take() { self.ranges.entry(k.clone()).or_insert(h); } // a versioning guard proved the bounds within 2^40
        if let Some(w) = &it.whil { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) break;", wc)); }
        if let Some(w) = &it.wher {
            let wv = self.expr(w, env, None)?;
            let wc = self.cond(wv)?;
            self.line(&format!("if (!({})) continue;", wc));
        }
        self.line(&format!("{}++;", hits));
        body(self, env, &hits)?;
        self.close("}");
        self.close("}");
        Ok(())
    }

    /// Write `j` as JSON; returns the shape it has (known while generating), for checking against `Response<T>`.
    fn json(&mut self, j: &'p Json, env: &mut Vec<Option<CV>>) -> R<Shape> {
        match j {
            Json::Obj(fields) => {
                let mut text = String::from("{");
                let mut shape = Vec::new();
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 { text.push(','); }
                    text += &json_lit(k); text.push(':');
                    self.out(&text); text.clear();
                    shape.push((k.clone(), self.json(v, env)?));
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
                self.each(it, env, false, |g, env, hits| {
                    g.line(&format!("if ({} > 1) {{ CALL(c->h->out(c, \",\", 1)); }}", hits));
                    elem = g.json(inner, env)?;
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
            Json::E(e) => { let v = self.expr(e, env, None)?; let s = Shape::of(&v); self.write(v)?; Ok(s) }
            Json::Empty => Ok(Shape::Nothing),
        }
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
            CV::Any(v) => self.call(&format!("c->h->json_val(c, &{})", v)),
            CV::StateStr(c, idx, ci) => self.call(&format!("c->h->json_col(c, {}, {}, {})", c, idx, ci)),
            CV::Null => self.out("null"),
            CV::Ref(t, s, k) => {
                if self.prog.colls[t].parent.is_some() { return no("an item of a collection inside another's items has no id; write `slot(x)`"); }
                self.line(&format!("if ({} == 0) {{ CALL(c->h->out(c, \"null\", 4)); }}", s));
                self.line(&format!("else CALL(c->h->json_id(c, {}, {}));", t, k));
            }
            CV::Item(c, i) if self.prog.colls[c].parent.is_none() && matches!(self.prog.colls[c].kind, Kind::Pool(_)) => self.call(&format!("c->h->json_id(c, {}, {})", c, i)),
            CV::Item(..) | CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::PlainArr(..) => self.fail(500, "cannot write this value as JSON"),
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
pub fn check_profile(body: &str) -> Result<(), String> {
    let t = ctoks(body);
    const CALLS: &[&str] = &["if", "for", "CALL", "CALL_OR_DEAD", "TAIL", "FAIL", "DEREF", "ADD", "SUB", "MUL", "DIV", "MOD", "NEG", "UNLIKELY", "BITS_F64", "NADD", "NSUB", "NMUL", "NDIV", "NMOD", "BAND", "BOR", "BXOR", "SHL", "SHR", "NSHL", "NSHR", "TX_", "SQRT_", "EXP_", "FLOOR_", "FABS_", "ABS_", "SAVE_", "COMMIT_", "RANGE_", "ST1_", "ST2_", "ST4_", "ST8_", "LD_I64", "LD_U64", "LD_I32", "LD_U32", "LD_I16", "LD_U16", "LD_I8", "LD_U8"];
    const MORE_CALLS: [&str; 2] = ["LD_F32", "LD_F64"];
    const FORBIDDEN: [&str; 24] = ["while", "do", "switch", "static", "extern", "register", "volatile", "asm", "__asm", "__asm__", "sizeof", "union", "struct", "typedef", "enum",
        "malloc", "calloc", "realloc", "free", "memcpy", "memmove", "memset", "alloca", "setjmp"];
    const TYPES: [&str; 6] = ["i64", "u32", "int", "Val", "Txt", "double"];
    let is_id = |s: &str| s.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_');
    let is_num = |s: &str| s.chars().next().map_or(false, |c| c.is_ascii_digit());
    let mut declared: Vec<&str> = vec!["st", "b", "c"];
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
        if ["ADD", "SUB", "MUL", "DIV", "MOD", "NEG", "NADD", "NSUB", "NMUL", "NDIV", "NMOD", "BAND", "BOR", "BXOR", "SHL", "SHR", "NSHL", "NSHR", "ABS_"].contains(&tok) && next == "(" { if let Some(r) = t.get(i + 2) { declared.push(r); } }
        if is_id(tok) && next == "(" {
            let member = prev == "->";
            if member {
                if !(i >= 4 && t[i - 4] == "c" && t[i - 3] == "->" && t[i - 2] == "h") { return err(i, "member call other than c->h->helper"); }
                if !owners.iter().any(|o| ["CALL", "CALL_OR_DEAD", "TAIL", "DEREF"].contains(o)) { return err(i, "helper called without CALL, TAIL or DEREF"); }
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
                let argv_decl = prev == "]" && i >= 4 && t[i - 4].starts_with("argv") && i >= 5 && t[i - 5] == "Val";
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
            ("fn f(a, b) { return a; } @get(\"/a\") fn get_a() -> Response<{}> { return ok({ \"x\": f(1) }); }", "`f` takes 2 arguments, given 1"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { x = 1; return ok({}); }", "unknown variable `x`"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { if true { let y = 1; } return ok({ \"y\": y }); }", "unknown name `y`"), // block scope
            ("fn len(s) { return 1; }", "name of a builtin"),
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
        let st = "struct I { n: i64; } struct E { who: ref<app.items>; n: i64; } struct App { items: pool<I, 4>; board: sorted<E, 4, n>; } state app: App;";
        let (st, _) = crate::modules::expand(st, "").unwrap();
        let load = |api: &str| -> String {
            let (_, api) = crate::modules::expand("struct I { n: i64; } struct E { who: ref<app.items>; n: i64; } struct App { items: pool<I, 4>; board: sorted<E, 4, n>; } state app: App;", api).unwrap();
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

    /// Every answer is checked against the endpoint's `Response<T>` when the program is compiled.
    #[test]
    fn answers_are_checked_against_their_response_type() {
        let st = "struct Row { name: string<8>; n: i64; } struct I { name: string<8>; n: i64; } struct App { items: pool<I, 4>; total: i64; } state app: App;";
        let load = |api: &str| -> String { super::check(&Program::parse(st).unwrap(), api).err().unwrap_or_default() };
        for (api, want) in [
            ("@get(\"/a\") fn a() -> Response<{ n: i64 }> { return ok({ n: 1, m: 2 }); }", "has `m`, which { n: i64 } does not declare"),
            ("@get(\"/a\") fn a() -> Response<{ n: i64, m: i64 }> { return ok({ n: 1 }); }", "has no `m`"),
            ("@get(\"/a\") fn a() -> Response<{ n: string }> { return ok({ n: 1 }); }", "`n` is i64, declared string"),
            ("@get(\"/a\") fn a() -> Response<{ x: [i64] }> { return ok({ x: [each it in app.items it.name] }); }", "`x[]` is string, declared i64"),
            ("@get(\"/a\") fn a() -> Response<[Row]> { return ok([each it in app.items { name: it.name }]); }", "has no `n`"),
            ("@get(\"/a\") fn a() -> Response<none> { return ok({}); }", "declared none"),
            ("@get(\"/a\") fn a() -> Response<{}> { return status(404, {}); }", "an answer is a success (2xx)"),
            ("@get(\"/a\") fn a() -> Response<{}> { return {}; }", "an endpoint returns its answer"),
            ("@get(\"/a\") fn a() -> Response<{}> { respond 200 {}; }", "`respond` was replaced"),
            ("endpoint GET /a { respond 200 {}; }", "endpoints are functions with an attribute"),
            ("@get(\"/a\") fn a() { return ok({}); }", "returns a `Response<T>`"),
            ("@fetch(\"/a\") fn a() -> Response<{}> { return ok({}); }", "`@fetch`"),
            ("@get(\"/a\") fn a() -> Response<{}> { if app.total > 1 { return ok({}); } }", "can finish without an answer"),
            // every value's kind is known when the program is compiled
            ("@get(\"/a\") fn a() -> Response<{ n: i64 }> { var v = 1; v = \"x\"; return ok({ n: 1 }); }", "`v` holds an integer and then a string: a var keeps the kind of its first value"),
            ("fn f(k: i64) { if k > 0 { return 1; } return \"x\"; } @get(\"/a\") fn a() -> Response<{}> { let x = f(1); return ok({}); }", "`f` returns an integer and a string: a function returns one kind"),
            ("fn f(k: i64) { if k > 0 { return 1; } } @get(\"/a\") fn a() -> Response<{}> { let x = f(1); return ok({}); }", "returns an integer on one path and nothing on another"),
            ("@get(\"/a\") fn a() -> Response<{ n: string }> { return ok({ n: \"x\" - 1 }); }", "operator `-` does not fit string and i64"),
        ] {
            let e = load(api);
            assert!(e.contains(want), "{} -> {}", api, e);
        }
        // what fits: a declared struct, optional values, run-time kinds, no body, a status of choice
        for api in [
            "@get(\"/a\") fn a() -> Response<[Row]> { return ok([each it in app.items { name: it.name, n: it.n }]); }",
            "@get(\"/a\") fn a(x: string<8>?) -> Response<{ n: string? }> { return ok({ n: or(x, null) }); }",
            "@delete(\"/a\") fn a() -> Response<none> { return no_content(); }",
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
                Stmt::Each(it, b) => { out.insert(it.slot); walk(b, out); }
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
