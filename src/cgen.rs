//! Generates C for bound endpoints. Types are worked out while generating: a value whose collection is known becomes a
//! direct load at a constant offset from the reservation's base; integer arithmetic, comparisons and loops are inline.
//! Everything dynamic (request values, strings, conditions on them) goes through helper functions the runtime passes
//! in a table (`H` below); those helpers reuse the interpreter's own code for values, comparisons and errors, so compiled
//! and interpreted endpoints answer alike. An endpoint using something the generator does not handle yet stays
//! interpreted; `generate` says which endpoints compiled and why the others did not.
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
typedef struct { u8 *base; void *run; const H *h; } Ctx;
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
    int (*sel_add)(Ctx *, double, u32);                        /* collect an item with its order key */
    int (*sel_sort)(Ctx *, u32, int, int, i64, u32 *);         /* order (if asked, stable; desc) and limit what was collected */
    int (*sel_get)(Ctx *, u32, u32, u32 *);                    /* the k-th collected item */
    int (*sel_end)(Ctx *, u32);                                /* drop what was collected since the mark */
    int (*field_val)(Ctx *, const Val *, const char *, u32, Val *); /* field `name` of an item held as a value */
    int (*store_field)(Ctx *, const Val *, const char *, u32, const Val *); /* journaled write of field `name` of such an item */
    int (*free_val)(Ctx *, const Val *);                       /* free an item held as a value (null: nothing) */
    int (*was_dead)(Ctx *);                                    /* was the error just returned a dead reference or a slot not live? (clears it) */
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
#define ADD(r, x, y) i64 r; if (((y) > 0 && (x) > I64MAX - (y)) || ((y) < 0 && (x) < I64MIN - (y))) OVERFLOW_; r = (x) + (y)
#define SUB(r, x, y) i64 r; if (((y) < 0 && (x) > I64MAX + (y)) || ((y) > 0 && (x) < I64MIN + (y))) OVERFLOW_; r = (x) - (y)
#define MUL(r, x, y) i64 r; if ((x) > 0 ? ((y) > 0 ? (x) > I64MAX / (y) : (y) < I64MIN / (x)) : ((y) > 0 ? (x) < I64MIN / (y) : ((x) != 0 && (y) < I64MAX / (x)))) OVERFLOW_; r = (x) * (y)
#define DIV(r, x, y) i64 r; if ((y) == 0) FAIL(500, "division by zero", 16); if ((x) == I64MIN && (y) == -1) OVERFLOW_; r = (x) / (y)
#define MOD(r, x, y) i64 r; if ((y) == 0) FAIL(500, "division by zero", 16); if ((x) == I64MIN && (y) == -1) OVERFLOW_; r = (x) % (y)
#define NEG(r, x) i64 r; if ((x) == I64MIN) OVERFLOW_; r = -(x)
"#;

/// Builtins run by the generic `builtin` helper, by position: the interpreter's own code computes them.
/// The first 14 always go through it; min, max and slot only when an argument's kind is known only at run time.
pub const BUILTINS: [B; 17] = [B::Body, B::Param, B::Query, B::Header, B::Fwd, B::Int, B::Str, B::Len, B::Now, B::Random, B::Token, B::Stat, B::WorkerIndex, B::WorkerCount, B::Min, B::Max, B::Slot];

/// Operator codes for the `binop` helper (the interpreter's operator names, in this order).
pub const OPS: [&str; 11] = ["==", "!=", "<", "<=", ">", ">=", "+", "-", "*", "/", "%"];

/// Plain (record-style) scalar and string kinds for `store_plain`.
pub fn plain_code(t: &Ty) -> Option<(u32, u32)> {
    use Scalar::*;
    Some(match t {
        Ty::Scalar(s) => (1 + [I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, Bool].iter().position(|x| x == s)? as u32, 0),
        Ty::Str(n) => (20, *n as u32),
        _ => return None,
    })
}
pub fn plain_ty(code: u32, cap: u32) -> Option<Ty> {
    use Scalar::*;
    Some(match code { 1..=11 => Ty::Scalar([I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, Bool][code as usize - 1]), 20 => Ty::Str(cap as usize), _ => return None })
}

pub struct Generated { pub src: String, pub eps: Vec<Option<String>>, pub notes: Vec<String> }

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

/// A kind that has to be widened, found while generating: the endpoint is generated again with it.
#[derive(Clone, Copy, Debug)]
enum Widen { Slot(Option<u32>, u32, K), Ret(u32, K) }

struct G<'p> {
    prog: &'p Program, api: &'p Api, ep: &'p Endpoint, body: String, n: usize, depth: usize, inlining: Vec<u32>, ret: Vec<Ret>,
    frame: Option<u32>,                                  // the function whose slots `env` holds (None: the endpoint)
    slot_kinds: &'p [((Option<u32>, u32), K)],           // widened kinds of `var` and `let` slots
    ret_kinds: &'p [(u32, K)],                           // widened result kinds of functions
    widen: Option<Widen>,
    recursive: &'p [bool],                               // per function: is it part of a cycle of calls
    specs: &'p std::cell::RefCell<Vec<Spec>>,            // C functions generated for recursive functions
    dep: bool,                                           // generating a C function: its call depth is the parameter `dep`
    inl: u32,                                            // inlined calls the code being generated is nested in
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

pub fn generate(prog: &Program, api: &Api) -> Generated { generate_with(prog, api, None) }

/// The C runtime (runtime/*.c), compiled in the same translation unit as a program.
pub const RUNTIME: &str = include_str!("../runtime/mito.c");
/// The runtime's types for the layout tables, ahead of them.
pub const RUNTIME_TYPES: &str = include_str!("../runtime/types.h");

/// A whole program as one C translation unit: the prelude, the layout and routes as data, the C runtime, and the
/// endpoints. `mito_*` (runtime/mito.c) is its interface. Fails if an endpoint does not compile.
pub fn program_c(prog: &Program, api: &Api, source: Option<(&str, &str)>) -> Result<String, String> {
    let g = generate_with(prog, api, source);
    if let Some(n) = g.notes.first() { return Err(n.clone()); }
    let mut o = String::from(PRELUDE);
    o += RUNTIME_TYPES;
    o += &layout_c(prog, api);
    o += RUNTIME;
    o += &g.src[PRELUDE.len()..];
    o += "\nconst EpFn mito_eps[] = {";
    for (k, _) in api.eps.iter().enumerate() { o += &format!(" ep{},", k); }
    o += " 0 };\n";
    Ok(o)
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
    for k in 0..api.eps.len() { o += &format!("int ep{}(Ctx *c);\n", k); }
    o += "typedef int (*EpFn)(Ctx *);\nextern const EpFn mito_eps[];\n";
    o += &format!("static const MitoProg mito_prog = {{ {}, mito_colls, {}, {}, {}, mito_routes }};\n", prog.colls.len(), prog.plain_end, prog.total, api.eps.len());
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
    fn js(j: &Json, out: &mut Vec<u32>) { match j { Json::Obj(f) => for (_, v) in f { js(v, out); }, Json::Arr(it, inner) => { ex(&it.coll, out); if let Some(w) = &it.wher { ex(w, out); } js(inner, out); } Json::E(e) => ex(e, out) } }
    fn st(stmts: &[Stmt], out: &mut Vec<u32>) {
        for s in stmts {
            match s {
                Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Free(e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(e, out),
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
                Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Free(e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(e, out),
                Stmt::Alloc { coll, .. } => ex(coll, out),
                Stmt::Assign(a, b) => { ex(a, out); ex(b, out); }
                Stmt::If(c, t, e) => { ex(c, out); calls(t, out); calls(e, out); }
                Stmt::Each(it, b) => { ex(&it.coll, out); if let Some(h) = &it.hi { ex(h, out); } if let Some(w) = &it.wher { ex(w, out); } if let Some((o, _)) = &it.order { ex(o, out); } if let Some(l) = &it.limit { ex(l, out); } calls(b, out); }
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

fn generate_with(prog: &Program, api: &Api, source: Option<(&str, &str)>) -> Generated {
    let mut src = String::from(PRELUDE);
    let (mut eps, mut notes) = (Vec::new(), Vec::new());
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
            let r = g.block(&ep.body, &mut env);
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
        match res.and_then(|_| check_profile(&body).map_err(|e| format!("C safety profile: {}", e))) {
            Ok(()) => {
                let name = format!("ep{}", k);
                let head = match source { Some(sf) => source_comment(ep, sf), None => format!("\n// {}\n", ep.text) };
                let new = &specs[done.len()..];
                if !new.is_empty() {
                    src += &format!("\n// functions for recursive calls, first used by {}, one per combination of argument kinds\n", ep.text);
                    for sp in new { src += &format!("int {}{};\n", sp.name, sp.proto); }
                    for sp in new { src += &format!("int {}{} {{\n    int st; u8 *b = c->base; (void)st; (void)b; (void)dep;\n{}    return 0;\n}}\n", sp.name, sp.proto, sp.body.as_deref().unwrap_or("")); }
                }
                done = specs;
                src += &format!("{}int {}(Ctx *c) {{\n    int st; u8 *b = c->base; (void)st; (void)b;\n{}    return 0;\n}}\n", head, name, body);
                eps.push(Some(name));
            }
            Err(why) => {
                if let Some(sf) = source { src += &source_comment(ep, sf); src += &format!("// not compiled: {}\n", why); }
                notes.push(format!("{}: interpreted ({})", ep.text, why)); eps.push(None);
            }
        }
    }
    Generated { src, eps, notes }
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
        Stmt::Alloc { .. } | Stmt::Assign(Expr::Field(..) | Expr::Index(..), _) => true,
        Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Free(e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(api, e, seen),
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
        Stmt::Let(_, _, e) | Stmt::Var(_, _, e) | Stmt::Free(e) | Stmt::Do(e) | Stmt::Return(Some(e)) => ex(api, e, seen),
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
        G { prog, api, ep, body: String::new(), n: 0, depth: 0, inlining: Vec::new(), ret: Vec::new(), frame: None, slot_kinds, ret_kinds, widen: None, recursive, specs, dep: false, inl: 0, or_dead: None }
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
            let Some(g) = self.prog.global(&m.name) else { continue };
            let gl = &self.prog.globals[g];
            if off >= gl.off && off < gl.off + self.prog.plain_size(&gl.ty).0 && self.module_code().as_deref() != Some(m.name.as_str()) { return Err(self.module_err(m, &format!("the state of {}", m.name))); }
        }
        Ok(())
    }
    /// Adding or freeing items of collection `c` from here: an instance's items and state change only through its functions.
    fn may_add_or_free(&self, c: usize, verb: &str) -> R<()> {
        let Some((m, data, items)) = self.coll_owner(c) else { return Ok(()) };
        if self.module_code().as_deref() == Some(m.name.as_str()) || (data && !items) { return Ok(()); }
        Err(format!("{} items of {} with its functions, not `{}`", if verb == "alloc" { "add" } else { "remove" }, m.name, verb))
    }
    /// A field store or free through a value whose kind is known only at run time: refused outside module code when it
    /// could reach a module's key or private state.
    fn may_write_dynamic(&self, field: Option<&str>) -> R<()> {
        if self.prog.instances.is_empty() || self.module_code().is_some() { return Ok(()); }
        match field {
            None => Err("`free` of a value whose kind is only known at run time, in a program with module instances: free a named item".into()),
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
    fn line(&mut self, s: &str) { for _ in 0..self.depth + 1 { self.body.push_str("    "); } self.body.push_str(s); self.body.push('\n'); }
    /// Call a helper that returns 0 or -1.
    fn call(&mut self, s: &str) {
        match self.or_dead.clone() { Some(l) => self.line(&format!("CALL_OR_DEAD({}, {});", s, l)), None => self.line(&format!("CALL({});", s)) }
    }
    fn fail(&mut self, code: u16, m: &str) { self.line(&format!("FAIL({}, {}, {});", code, c_lit(m.as_bytes()), m.len())); }
    fn open(&mut self, head: &str) { self.line(head); self.depth += 1; }
    fn close(&mut self, tail: &str) { self.depth -= 1; self.line(tail); }

    fn block(&mut self, stmts: &'p [Stmt], env: &mut Vec<Option<CV>>) -> R<()> {
        for s in stmts { self.stmt(s, env)?; }
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
                if let Err(e) = self.assign_var(&cur, v.clone()) {
                    if let (Some(a), Some(b)) = (kind(&cur), kind(&v)) { self.widen = Some(Widen::Slot(self.frame, *slot, unify(a, b))); }
                    return Err(e);
                }
            }
            Stmt::Assign(Expr::Field(base, f, _), rhs) => {
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
                        self.may_write_plain(at)?;
                        let v = self.expr(rhs, env, None)?;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store_plain(c, {}, {}, {}, &{})", at, code, cap, vv));
                        return Ok(());
                    }
                    CV::Null => { self.line("TAIL(c->h->dead(c));"); return Ok(()); }
                    CV::Any(x) => {
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
                let Some(ci) = self.prog.colls[c].cols.iter().position(|x| x.path == path && !matches!(x.kind, CKind::StrBytes(_) | CKind::Arr(..))) else { return no(format!("assigning `.{}` not compiled yet", path)) };
                self.may_write_col(c, &path)?;
                let v = self.expr(rhs, env, None)?;
                let vv = self.val(v)?;
                self.call(&format!("c->h->store(c, {}, {}, {}, &{})", c, idx, ci, vv));
            }
            Stmt::Assign(Expr::Index(base, k), rhs) => {
                let bv = self.expr(base, env, None)?;
                let kv = self.expr(k, env, None)?;
                let k = self.int_or(kv, 400, "an index must be an integer");
                if let CV::Coll(..) | CV::ArrCol(..) | CV::PlainArr(..) = bv {} else { self.fail(500, "only fixed arrays and collections can be assigned by index"); return Ok(()); }
                match bv {
                    CV::ArrCol(c, idx, ci) => {
                        self.may_write_col(c, &self.prog.colls[c].cols[ci].path.clone())?;
                        let CKind::Arr(_, n) = self.prog.colls[c].cols[ci].kind else { unreachable!() };
                        self.call(&format!("c->h->elem(c, {}, {})", k, n));
                        let v = self.expr(rhs, env, None)?;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store_elem(c, {}, {}, {}, {}, &{})", c, idx, ci, k, vv));
                    }
                    CV::PlainArr(off, e, n) => {
                        self.may_write_plain(off)?;
                        let Some((code, cap)) = plain_code(&e) else { return no("assigning an element of this array not compiled yet") };
                        self.call(&format!("c->h->elem(c, {}, {})", k, n));
                        let v = self.expr(rhs, env, None)?;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store_plain(c, (u32)({} + {} * {}), {}, {}, &{})", off, k, self.prog.plain_size(&e).0, code, cap, vv));
                    }
                    CV::Coll(c, inst) => {
                        self.may_write_col(c, "")?;
                        let t = self.tmp("i");
                        self.line(&format!("u32 {};", t));
                        self.call(&format!("c->h->index(c, {}, {}, {}, &{})", c, inst, k, t));
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
                if let (Some(_), Expr::Var(_, VarRes::Global(g))) = (&it.via, &it.coll) {
                    let n = &self.prog.globals[*g as usize].name;
                    if changes_instance(self.api, n, body) { return no(format!("a loop over {} calls a function that changes {}; change it after the loop", n, n)); }
                }
                let collect = it.order.is_some() || allocates(self.api, body, &mut Vec::new()) || (it.wher.is_some() && writes(self.api, body, &mut Vec::new()));
                self.each(it, env, collect, |g, env, _| g.block(body, env))?;
            }
            Stmt::Free(e) => match self.expr(e, env, None)? {
                CV::Item(c, i) => { self.may_add_or_free(c, "free")?; self.call(&format!("c->h->free_(c, {}, {})", c, i)) }
                CV::Ref(t, s, k) => { self.may_add_or_free(t, "free")?; self.line(&format!("if ({} != 0) {{ CALL(c->h->free_(c, {}, {})); }}", s, t, k)) }
                CV::Null => {}
                CV::Any(x) => { self.may_write_dynamic(None)?; self.call(&format!("c->h->free_val(c, &{})", x)) }
                _ => self.fail(500, "free needs an item"),
            },
            Stmt::Fail(code, m) => self.fail(*code, m),
            Stmt::Respond(code, j) => { self.json(j, env)?; self.line(&format!("return {};", code)); }
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
                let CV::Coll(c, inst) = self.expr(coll, env, None)? else { return no("alloc of something other than a collection") };
                self.may_add_or_free(c, "alloc")?;
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
        if !fits(kv, k) { self.widen = Some(Widen::Ret(f, unify(k, kv))); return no("a function returns values of different kinds"); }
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
        Ok(match (k, v) {
            (K::Any, v) => { let x = self.val(v)?; if mutable { let t = self.tmp("v"); self.line(&format!("Val {} = {};", t, x)); CV::Any(t) } else { CV::Any(x) } }
            (K::Txt, CV::Null) => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ 0, 0, 0 }};", t)); CV::Txt(t) }
            (K::Txt, v @ (CV::Txt(_) | CV::Lit(_) | CV::StateStr(..))) => { let x = self.txt_of(v)?; if mutable { let t = self.tmp("t"); self.line(&format!("Txt {} = {};", t, x)); CV::Txt(t) } else { CV::Txt(x) } }
            (K::Ref(c), CV::Null) => { let (s, i) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = 0, {} = 0;", s, i)); CV::Ref(c, s, i) }
            (K::Ref(c), CV::Item(_, e)) => { let (s, i) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = 1, {} = {};", s, i, e)); CV::Ref(c, s, i) }
            (K::Ref(_), v @ CV::Ref(..)) => self.hold(v, mutable)?,
            (_, v) => self.hold(v, mutable)?,
        })
    }

    /// Give a value its own C variable (so it is computed once, and a `var` can be assigned later).
    fn hold(&mut self, v: CV, mutable: bool) -> R<CV> {
        Ok(match v {
            CV::Int(e) if mutable || !is_name(&e) => { let t = self.tmp("v"); self.line(&format!("i64 {} = {};", t, e)); CV::Int(t) }
            CV::Flt(e) if mutable || !is_name(&e) => { let t = self.tmp("v"); self.line(&format!("double {} = {};", t, e)); CV::Flt(t) }
            CV::Bool(e) if mutable || !is_name(&e) => { let t = self.tmp("v"); self.line(&format!("int {} = {};", t, e)); CV::Bool(t) }
            CV::Item(c, e) if mutable || !is_name(&e) => { let t = self.tmp("i"); self.line(&format!("u32 {} = {};", t, e)); CV::Item(c, t) }
            CV::Txt(e) if mutable => { let t = self.tmp("t"); self.line(&format!("Txt {} = {};", t, e)); CV::Txt(t) }
            CV::Any(e) if mutable => { let t = self.tmp("v"); self.line(&format!("Val {} = {};", t, e)); CV::Any(t) }
            CV::Lit(s) if mutable => CV::Txt(self.txt_of(CV::Lit(s))?),
            CV::StateStr(..) => CV::Txt(self.txt_of(v)?), // a value is copied when read, as in the interpreter
            CV::Null if mutable => { let t = self.tmp("v"); self.line(&format!("Val {} = {{ 0, 0, 0, 0, 0 }};", t)); CV::Any(t) }
            CV::Ref(c, s, k) if mutable => { let (s2, k2) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}, {} = {};", s2, s, k2, k)); CV::Ref(c, s2, k2) }
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
            CV::Lit(s) => { let t = self.tmp("t"); self.line(&format!("Txt {};", t)); self.call(&format!("c->h->lit(c, {}, {}, &{})", c_lit(s.as_bytes()), s.len(), t)); Ok(t) }
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
            Expr::Field(base, f, _) => { let bv = self.expr(base, env, dead)?; self.field(bv, f, dead) }
            Expr::Index(base, k) => {
                let bv = self.expr(base, env, dead)?;
                let kv = self.expr(k, env, dead)?;
                let k = self.int_or(kv, 400, "an index must be an integer");
                match bv {
                    CV::Coll(c, inst) => {
                        let t = self.tmp("i");
                        self.line(&format!("u32 {};", t));
                        self.call(&format!("c->h->index(c, {}, {}, {}, &{})", c, inst, k, t));
                        self.item_value(c, t, dead)
                    }
                    CV::ArrCol(c, idx, ci) => {
                        let (off, CKind::Arr(sc, n)) = (self.prog.colls[c].cols[ci].off, self.prog.colls[c].cols[ci].kind.clone()) else { unreachable!() };
                        let k = self.hold_int(k);
                        self.call(&format!("c->h->elem(c, {}, {})", k, n));
                        self.load(&off.to_string(), &format!("(i64)({}) * {} + {}", idx, n, k), sc)
                    }
                    CV::PlainArr(off, Ty::Scalar(sc), n) => {
                        let k = self.hold_int(k);
                        self.call(&format!("c->h->elem(c, {}, {})", k, n));
                        self.load(&off.to_string(), &k, sc)
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
                if ty.is_coll() { return Ok(CV::Coll(self.prog.plain_coll(off, sid, f).ok_or("plain collection")?, "0".into())); }
                return self.plain(off + sd.offs[k], &ty, dead);
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
            CKind::Scalar(s) => self.load(&col.off.to_string(), &idx, s)?,
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
                "+" | "-" | "*" | "/" | "%" => { self.line(&format!("{}({}, {}, {});", mac, r, x, y)); return Ok(CV::Int(r)); }
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
        // anything else (strings, mixed kinds): the interpreter's own operator
        let code = OPS.iter().position(|o| *o == op).ok_or_else(|| format!("operator `{}`", op))?;
        let (lv, rv) = (self.val(l)?, self.val(r)?);
        let t = self.tmp("v");
        self.line(&format!("Val {};", t));
        self.call(&format!("c->h->binop(c, {}, &{}, &{}, &{})", code, lv, rv, t));
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
                        let Some(Expr::Field(b, f, _)) = args.first() else { return no("maxlen(item.field)") };
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
            B::Capacity => { let Some(CV::Coll(c, _)) = a.first() else { return no("capacity(collection)") }; CV::Int(format!("{}LL", self.prog.colls[*c].cap)) }
            B::Slot => match a.first() {
                Some(CV::Item(c, i)) => CV::Int(format!("(i64)({} % {})", i, self.prog.colls[*c].cap)),
                Some(CV::Ref(c, s, i)) => { self.line(&format!("if ({} == 0) FAIL(500, \"slot(item)\", 10);", s)); CV::Int(format!("(i64)({} % {})", i, self.prog.colls[*c].cap)) }
                _ => unreachable!("generic above"),
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
            if !fits(K::Null, kind) { self.widen = Some(Widen::Ret(k, unify(kind, K::Null))); return no("a function that can end without returning its value"); }
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

    /// Emit a loop over a list or pool instance (with `reverse`, `where`, `order by` and `limit`), running `body` with the
    /// item bound. With `collect`, the items passing `where` are collected (with their order keys) before any body runs,
    /// then ordered and limited, as the interpreter's `select` does; otherwise filtering and the body run in one pass.
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
        let key = match &it.order {
            None => "0.0".to_string(),
            Some((e, _)) => match self.expr(e, env, None)? {
                CV::Int(x) => format!("(double)({})", x),
                CV::Flt(x) => x,
                CV::Any(v) => {
                    let t = self.tmp("key");
                    self.line(&format!("double {} = 0;", t));
                    self.line(&format!("if ({}.tag == 1) {} = (double){}.i; else if ({}.tag == 2) {} = {}.f; else FAIL(500, \"order by needs a number\", 23);", v, t, v, v, t, v));
                    t
                }
                _ => { self.fail(500, "order by needs a number"); "0.0".to_string() }
            },
        };
        self.call(&format!("c->h->sel_add(c, {}, {})", key, m));
        if it.order.is_none() { if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); } }
        self.close("}");
        let (n, q, m2) = (self.tmp("n"), self.tmp("q"), self.tmp("m"));
        self.line(&format!("u32 {};", n));
        let desc = matches!(it.order, Some((_, true)));
        self.call(&format!("c->h->sel_sort(c, {}, {}, {}, {}, &{})", h, it.order.is_some() as i32, desc as i32, lim.clone().unwrap_or("-1".into()), n));
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
        let Expr::Var(_, VarRes::Global(g)) = &it.coll else { return no("walking something other than a module instance") };
        let n = &self.prog.globals[*g as usize].name;
        let steps = self.prog.colls.iter().find(|c| c.name == format!("{}.items", n)).map_or(0, |c| c.cap);
        let from = match &it.from { Some(e) => { let x = self.expr(e, env, None)?; self.hold(x, false)? } None => CV::Null };
        let lim = match &it.limit { Some(e) => { let x = self.expr(e, env, None)?; Some(self.int_or(x, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        let c0 = self.call_values(if it.reverse { v.last.ok_or("no reverse walk")? } else { v.first }, vec![from], None)?;
        let c0 = self.val(c0)?;
        let (cur, k, hits) = (self.tmp("cur"), self.tmp("k"), self.tmp("hits"));
        self.open("{");
        self.line(&format!("Val {} = {};", cur, c0));
        self.line(&format!("i64 {} = 0;", hits));
        self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", k, k, steps, k));
        self.line(&format!("if ({}.tag == 0) break;", cur));
        if let Some(l) = &lim { self.line(&format!("if ({} >= {}) break;", hits, l)); }
        let item = self.call_values(v.item, vec![CV::Any(cur.clone())], None)?;
        let item = self.hold(item, false)?;
        env[it.slot as usize] = Some(item);
        if let Some(w) = &it.whil { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) break;", wc)); }
        let filtered = if let Some(w) = &it.wher { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.open(&format!("if ({}) {{", wc)); true } else { false };
        self.line(&format!("{}++;", hits));
        body(self, env, &hits)?;
        if filtered { self.close("}"); }
        let nx = self.call_values(if it.reverse { v.prev.ok_or("no reverse walk")? } else { v.next }, vec![CV::Any(cur.clone())], None)?;
        let nx = self.val(nx)?;
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

    fn json(&mut self, j: &'p Json, env: &mut Vec<Option<CV>>) -> R<()> {
        match j {
            Json::Obj(fields) => {
                let mut text = String::from("{");
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 { text.push(','); }
                    text += &json_lit(k); text.push(':');
                    self.out(&text); text.clear();
                    self.json(v, env)?;
                }
                text.push('}'); self.out(&text);
                Ok(())
            }
            Json::Arr(it, inner) => {
                self.out("[");
                if let (Some(_), Expr::Var(_, VarRes::Global(g))) = (&it.via, &it.coll) {
                    let n = &self.prog.globals[*g as usize].name;
                    if changes_instance(self.api, n, &[Stmt::Respond(200, (**inner).clone())]) { return no(format!("a loop over {} calls a function that changes {}; change it after the loop", n, n)); }
                }
                self.each(it, env, it.order.is_some(), |g, env, hits| {
                    g.line(&format!("if ({} > 1) {{ CALL(c->h->out(c, \",\", 1)); }}", hits));
                    g.json(inner, env)
                })?;
                self.out("]");
                Ok(())
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
                self.write(v)?;
                self.line(&format!("goto {};", end));
                self.line("}");
                self.line(&format!("{}: {{", l));
                let fb = self.expr(&args[1], env, None)?;
                self.write(fb)?;
                self.line("}");
                self.line(&format!("{}: ;", end));
                Ok(())
            }
            Json::E(e) => { let v = self.expr(e, env, None)?; self.write(v) }
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
            CV::Ref(_, s, _) => { self.line(&format!("if ({} == 0) {{ CALL(c->h->out(c, \"null\", 4)); }}", s)); self.line("else FAIL(500, \"cannot write this value as JSON\", 30);"); }
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
    const CALLS: [&str; 21] = ["if", "for", "CALL", "CALL_OR_DEAD", "TAIL", "FAIL", "DEREF", "ADD", "SUB", "MUL", "DIV", "MOD", "NEG", "LD_I64", "LD_U64", "LD_I32", "LD_U32", "LD_I16", "LD_U16", "LD_I8", "LD_U8"];
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
        if ["ADD", "SUB", "MUL", "DIV", "MOD", "NEG"].contains(&tok) && next == "(" { if let Some(r) = t.get(i + 2) { declared.push(r); } }
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
        for (state, api) in [(include_str!("../examples/chat/chat.mito"), include_str!("../examples/chat/chat.api")),
                             (include_str!("../examples/chat-sharded/rooms.mito"), include_str!("../examples/chat-sharded/rooms.api")),
                             (include_str!("../examples/chat-sharded/auth.mito"), include_str!("../examples/chat-sharded/auth.api"))] {
            let prog = Program::parse(state).unwrap();
            let mut api = crate::lang::parse(api).unwrap();
            crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
            let g = generate(&prog, &api);
            assert!(!g.notes.iter().any(|n| n.contains("profile")), "{:#?}", g.notes);
            assert!(g.eps.iter().filter(|e| e.is_some()).count() >= 5);
        }
    }

    #[test]
    fn emitted_files_compile_on_their_own_and_cite_their_source() {
        let (state, api) = (include_str!("../examples/chat/chat.mito"), include_str!("../examples/chat/chat.api"));
        let prog = Program::parse(state).unwrap();
        let mut a = crate::lang::parse(api).unwrap();
        crate::lang::bind(&mut a, &|n| prog.global(n)).unwrap();
        let c = emit(&prog, &a, ("chat.api", api)).unwrap();
        crate::tcc::Unit::check(&c).unwrap();
        assert!(c.contains("coll 2 rooms.messages: list cap 1000 drop_oldest"), "{}", &c[..800]);
        for ep in &a.eps { assert!(c.contains(&format!("// chat.api:{}-{}", ep.lines.0, ep.lines.1))); }
        assert!(c.contains("//   | endpoint DELETE /session {"));
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
