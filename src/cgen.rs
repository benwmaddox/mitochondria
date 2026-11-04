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
pub const BUILTINS: [B; 14] = [B::Body, B::Param, B::Query, B::Header, B::Fwd, B::Int, B::Str, B::Len, B::Now, B::Random, B::Token, B::Stat, B::WorkerIndex, B::WorkerCount];

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
    Bool(String),                   // C expression, 0 or 1
    Lit(String),                    // a string known when generating
    Txt(String),                    // a Txt: a string in the request's text area, or absent (null)
    Any(String),                    // a Val: a value whose kind is only known at run time
    Item(usize, String),            // collection, C expression for the item index
    Ref(usize, String, String),     // a followed reference: target pool, slot+1 (0 = null), index
    Coll(usize, String),            // collection, C expression for the instance
    StateStr(usize, String, usize), // a string column of an item: collection, item index, column
    Plain(usize, usize),            // a plain struct: struct id, offset
    Null,
}

/// Where `return` goes inside an inlined function.
struct Ret { var: String, label: String, kind: Option<CV> }

struct G<'p> { prog: &'p Program, api: &'p Api, ep: &'p Endpoint, body: String, n: usize, depth: usize, inlining: Vec<u32>, ret: Vec<Ret> }
type R<T> = Result<T, String>;
fn no<T>(what: impl Into<String>) -> R<T> { Err(what.into()) }
fn is_name(e: &str) -> bool { e.chars().all(|c| c.is_alphanumeric() || c == '_') }

/// A C string literal for `s`, safe for any bytes.
pub fn c_lit(s: &[u8]) -> String {
    let mut o = String::from("\"");
    for &b in s { match b { b'"' => o.push_str("\\\""), b'\\' => o.push_str("\\\\"), 0x20..=0x7e => o.push(b as char), _ => o.push_str(&format!("\\{:03o}", b)) } }
    o.push('"'); o
}
/// The JSON text of a string, escaped now, so the generated code can write it as is.
fn json_lit(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() { match c { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), '\n' => o.push_str("\\n"), '\r' => o.push_str("\\r"), '\t' => o.push_str("\\t"), c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)), c => o.push(c) } }
    o.push('"'); o
}

pub fn generate(prog: &Program, api: &Api) -> Generated { generate_with(prog, api, None) }

/// One readable C file for review: the layout, the helper interface, and every endpoint next to its source.
/// `source` is the api file's name and text. The C is the same code `generate` makes, with comments added.
pub fn emit(prog: &Program, api: &Api, source: (&str, &str)) -> String {
    let g = generate_with(prog, api, Some(source));
    let compiled = g.eps.iter().filter(|e| e.is_some()).count();
    let mut o = format!("/* Generated by Mitochondria from {}: {} of {} endpoints compiled.\n", source.0, compiled, g.eps.len());
    for n in &g.notes { o += &format!("   {}\n", n); }
    o += "\n";
    o += &layout_table(prog);
    o += "*/\n";
    o + &g.src
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

fn generate_with(prog: &Program, api: &Api, source: Option<(&str, &str)>) -> Generated {
    let mut src = String::from(PRELUDE);
    let (mut eps, mut notes) = (Vec::new(), Vec::new());
    for (k, ep) in api.eps.iter().enumerate() {
        let mut g = G { prog, api, ep, body: String::new(), n: 0, depth: 0, inlining: Vec::new(), ret: Vec::new() };
        let mut env: Vec<Option<CV>> = vec![None; ep.slots as usize];
        match g.block(&ep.body, &mut env).and_then(|_| check_profile(&g.body).map_err(|e| format!("C safety profile: {}", e))) {
            Ok(()) => {
                let name = format!("ep{}", k);
                let head = match source { Some(sf) => source_comment(ep, sf), None => format!("\n// {}\n", ep.text) };
                src += &format!("{}int {}(Ctx *c) {{\n    int st; u8 *b = c->base; (void)st; (void)b;\n{}    return 0;\n}}\n", head, name, g.body);
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

/// Does running these statements (including inlined calls) allocate? Loops whose body allocates are not compiled yet,
/// because the interpreter iterates over a snapshot taken before the body runs.
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
        Stmt::Each(it, b) => ex(api, &it.coll, seen) || allocates(api, b, seen),
        Stmt::Forward { key, fields, .. } => ex(api, key, seen) || fields.iter().any(|f| ex(api, &f.1, seen)),
        _ => false,
    })
}

impl<'p> G<'p> {
    fn tmp(&mut self, p: &str) -> String { self.n += 1; format!("{}{}", p, self.n) }
    fn line(&mut self, s: &str) { for _ in 0..self.depth + 1 { self.body.push_str("    "); } self.body.push_str(s); self.body.push('\n'); }
    /// Call a helper that returns 0 or -1.
    fn call(&mut self, s: &str) { self.line(&format!("CALL({});", s)); }
    fn fail(&mut self, code: u16, m: &str) { self.line(&format!("FAIL({}, {}, {});", code, c_lit(m.as_bytes()), m.len())); }
    fn open(&mut self, head: &str) { self.line(head); self.depth += 1; }
    fn close(&mut self, tail: &str) { self.depth -= 1; self.line(tail); }

    fn block(&mut self, stmts: &'p [Stmt], env: &mut Vec<Option<CV>>) -> R<()> {
        for s in stmts { self.stmt(s, env)?; }
        Ok(())
    }

    fn stmt(&mut self, s: &'p Stmt, env: &mut Vec<Option<CV>>) -> R<()> {
        match s {
            Stmt::Let(_, slot, e) => { let v = self.expr(e, env, None)?; let v = self.hold(v, false)?; env[*slot as usize] = Some(v); }
            Stmt::Var(_, slot, e) => { let v = self.expr(e, env, None)?; let v = self.hold(v, true)?; env[*slot as usize] = Some(v); }
            Stmt::Assign(Expr::Var(n, VarRes::Local(slot)), rhs) => {
                let cur = env[*slot as usize].clone().ok_or_else(|| format!("`{}` has no compiled value", n))?;
                let v = self.expr(rhs, env, None)?;
                self.assign_var(&cur, v)?;
            }
            Stmt::Assign(Expr::Field(base, f, _), rhs) => {
                let bv = self.expr(base, env, None)?;
                match bv {
                    CV::Item(c, idx) => {
                        let coll = &self.prog.colls[c];
                        let Some(ci) = coll.cols.iter().position(|x| x.path == *f && !matches!(x.kind, CKind::StrBytes(_))) else { return no(format!("assigning `.{}` not compiled yet", f)) };
                        let v = self.expr(rhs, env, None)?;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store(c, {}, {}, {}, &{})", c, idx, ci, vv));
                    }
                    CV::Ref(t, s, k) => {
                        let Some(ci) = self.prog.colls[t].cols.iter().position(|x| x.path == *f && !matches!(x.kind, CKind::StrBytes(_))) else { return no(format!("assigning `.{}` not compiled yet", f)) };
                        self.line(&format!("if ({} == 0) TAIL(c->h->dead(c));", s));
                        let v = self.expr(rhs, env, None)?;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store(c, {}, {}, {}, &{})", t, k, ci, vv));
                    }
                    CV::Plain(sid, off) => {
                        let sd = &self.prog.structs[sid];
                        let Some(k) = sd.fields.iter().position(|x| x.0 == *f) else { return no(format!("no field `{}`", f)) };
                        let Some((code, cap)) = plain_code(&sd.fields[k].1) else { return no(format!("assigning plain `.{}` not compiled yet", f)) };
                        let at = off + sd.offs[k];
                        let v = self.expr(rhs, env, None)?;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store_plain(c, {}, {}, {}, &{})", at, code, cap, vv));
                    }
                    _ => return no("assignment target not compiled yet"),
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
                if allocates(self.api, body, &mut Vec::new()) { return no("a loop whose body allocates is not compiled yet"); }
                self.each(it, env, |g, env, _| g.block(body, env))?;
            }
            Stmt::Free(e) => match self.expr(e, env, None)? {
                CV::Item(c, i) => self.call(&format!("c->h->free_(c, {}, {})", c, i)),
                CV::Ref(t, s, k) => self.line(&format!("if ({} != 0) {{ CALL(c->h->free_(c, {}, {})); }}", s, t, k)),
                CV::Null => {}
                _ => return no("free of this value not compiled yet"),
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
                let CV::Int(k) = self.expr(key, env, None)? else { return no("a forward key that is not an integer") };
                let k = self.hold_int(k);
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
        let r = self.ret.last().unwrap();
        let (var, label) = (r.var.clone(), r.label.clone());
        match (&r.kind, &v) {
            (None, _) => { self.ret.last_mut().unwrap().kind = Some(v.clone()); }
            (Some(k), _) if std::mem::discriminant(k) == std::mem::discriminant(&v) => {
                if let (CV::Item(a, _), CV::Item(b, _)) = (k, &v) { if a != b { return no("a function returns items of different collections"); } }
            }
            _ => return no("a function returns values of different kinds"),
        }
        let assign = match &v {
            CV::Int(e) | CV::Bool(e) | CV::Txt(e) | CV::Any(e) => format!("{}_v = {};", var, e),
            CV::Item(_, e) => format!("{}_v = {};", var, e),
            CV::Ref(_, s, k) => format!("{}_s = {}; {}_v = {};", var, s, var, k),
            CV::Null => String::new(),
            _ => return no("a function returning this kind of value is not compiled yet"),
        };
        if !assign.is_empty() { self.line(&assign); }
        self.line(&format!("goto {};", label));
        Ok(())
    }

    /// Give a value its own C variable (so it is computed once, and a `var` can be assigned later).
    fn hold(&mut self, v: CV, mutable: bool) -> R<CV> {
        Ok(match v {
            CV::Int(e) if mutable || !is_name(&e) => { let t = self.tmp("v"); self.line(&format!("i64 {} = {};", t, e)); CV::Int(t) }
            CV::Bool(e) if mutable || !is_name(&e) => { let t = self.tmp("v"); self.line(&format!("int {} = {};", t, e)); CV::Bool(t) }
            CV::Item(c, e) if mutable || !is_name(&e) => { let t = self.tmp("i"); self.line(&format!("u32 {} = {};", t, e)); CV::Item(c, t) }
            CV::Txt(e) if mutable => { let t = self.tmp("t"); self.line(&format!("Txt {} = {};", t, e)); CV::Txt(t) }
            CV::Any(e) if mutable => { let t = self.tmp("v"); self.line(&format!("Val {} = {};", t, e)); CV::Any(t) }
            CV::Lit(s) if mutable => CV::Txt(self.txt_of(CV::Lit(s))?),
            CV::StateStr(..) => CV::Txt(self.txt_of(v)?), // a value is copied when read, as in the interpreter
            CV::Null if mutable => { let t = self.tmp("v"); self.line(&format!("Val {} = {{ 0, 0, 0, 0, 0 }};", t)); CV::Any(t) }
            CV::Ref(..) if mutable => return no("a `var` holding a reference is not compiled yet"),
            o => o,
        })
    }
    fn hold_int(&mut self, e: String) -> String { if is_name(&e) { e } else { let t = self.tmp("v"); self.line(&format!("i64 {} = {};", t, e)); t } }

    fn assign_var(&mut self, cur: &CV, v: CV) -> R<()> {
        match (cur, v) {
            (CV::Int(t), CV::Int(e)) | (CV::Bool(t), CV::Bool(e)) => self.line(&format!("{} = {};", t, e)),
            (CV::Item(a, t), CV::Item(b, e)) if *a == b => self.line(&format!("{} = {};", t, e)),
            (CV::Txt(t), v @ (CV::Txt(_) | CV::Lit(_) | CV::StateStr(..))) => { let s = self.txt_of(v)?; self.line(&format!("{} = {};", t, s)); }
            (CV::Any(t), v) => { let s = self.val(v)?; self.line(&format!("{} = {};", t, s)); }
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
            CV::Bool(e) => self.line(&format!("Val {} = {{ 3, 0, 0, {}, 0 }};", t, e)),
            CV::Null => self.line(&format!("Val {} = {{ 0, 0, 0, 0, 0 }};", t)),
            CV::Item(c, e) => self.line(&format!("Val {} = {{ 5, {}, {}, 0, 0 }};", t, c, e)),
            CV::Ref(c, s, k) => self.line(&format!("Val {} = {{ {} == 0 ? 0 : 5, {}, {}, 0, 0 }};", t, s, c, k)),
            v @ (CV::Lit(_) | CV::Txt(_) | CV::StateStr(..)) => { let x = self.txt_of(v)?; self.line(&format!("Val {} = {{ {}.present ? 4 : 0, {}.off, {}.len, 0, 0 }};", t, x, x, x)); }
            CV::Coll(..) | CV::Plain(..) => return no("a collection or struct used as a value is not compiled yet"),
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
            Expr::Float(_) => no("floats are not compiled yet"),
            Expr::Var(n, VarRes::Local(s)) => env.get(*s as usize).cloned().flatten().ok_or_else(|| format!("`{}` has no compiled value", n)),
            Expr::Var(n, VarRes::Global(g)) => {
                let gl = &self.prog.globals[*g as usize];
                if let Some(c) = gl.coll { return Ok(CV::Coll(c, "0".into())); }
                self.plain(gl.off, &gl.ty.clone()).map_err(|_| format!("global `{}` of this kind not compiled yet", n))
            }
            Expr::Var(n, VarRes::Unbound) => no(format!("`{}` was not bound", n)),
            Expr::Field(base, f, _) => { let bv = self.expr(base, env, dead)?; self.field(bv, f, dead) }
            Expr::Index(base, k) => {
                let bv = self.expr(base, env, dead)?;
                let CV::Int(k) = self.expr(k, env, dead)? else { return no("an index that is not an integer") };
                let CV::Coll(c, inst) = bv else { return no("indexing something other than a collection") };
                if !matches!(self.prog.colls[c].kind, Kind::Pool(_)) { return no("list indexing not compiled yet"); }
                if self.prog.colls[c].cols.iter().any(|x| x.path.is_empty()) { return no("a pool of plain values not compiled yet"); }
                let t = self.tmp("i");
                self.line(&format!("u32 {};", t));
                self.call(&format!("c->h->index(c, {}, {}, {}, &{})", c, inst, k, t));
                Ok(CV::Item(c, t))
            }
            Expr::Un(op, x) => {
                let v = self.expr(x, env, dead)?;
                if *op == '!' { let c = self.cond(v)?; return Ok(CV::Bool(format!("!({})", c))); }
                match v { CV::Int(e) => { let e = self.hold_int(e); let r = self.tmp("v"); self.line(&format!("NEG({}, {});", r, e)); Ok(CV::Int(r)) } _ => no("negating something other than an integer") }
            }
            Expr::Bin(op, a, b) => self.bin(op, a, b, env, dead),
            Expr::Call(n, args, res) => self.callx(n, args, *res, env, dead),
        }
    }

    /// A plain (record-style) value at offset `off`.
    fn plain(&mut self, off: usize, t: &Ty) -> R<CV> {
        Ok(match t {
            Ty::Struct(s) => CV::Plain(*s, off),
            Ty::Scalar(s) => self.load(&off.to_string(), "0", *s)?,
            Ty::Str(_) => { let x = self.tmp("t"); self.line(&format!("Txt {};", x)); self.call(&format!("c->h->plain_txt(c, {}, &{})", off, x)); CV::Txt(x) }
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
            F32 | F64 => { let t = self.tmp("v"); self.line(&format!("Val {} = {{ 2, 0, 0, 0, (double){} }};", t, ld(if s == F32 { "F32" } else { "F64" }))); CV::Any(t) }
        })
    }

    fn field(&mut self, base: CV, f: &str, dead: Option<&str>) -> R<CV> {
        let (c, idx) = match base {
            CV::Item(c, idx) => (c, idx),
            CV::Ref(t, s, k) => {
                // reading through a null reference is a dead reference, as in the interpreter
                let on_dead = match dead { Some(l) => format!("goto {};", l), None => "TAIL(c->h->dead(c));".into() };
                self.line(&format!("if ({} == 0) {}", s, on_dead));
                (t, k)
            }
            CV::Plain(sid, off) => {
                let sd = &self.prog.structs[sid];
                let Some(k) = sd.fields.iter().position(|x| x.0 == f) else { return no(format!("no field `{}`", f)) };
                let ty = sd.fields[k].1.clone();
                if ty.is_coll() { return Ok(CV::Coll(self.prog.plain_coll(off, sid, f).ok_or("plain collection")?, "0".into())); }
                return self.plain(off + sd.offs[k], &ty);
            }
            CV::Null => { match dead { Some(l) => self.line(&format!("goto {};", l)), None => self.line("TAIL(c->h->dead(c));") } return Ok(CV::Null); }
            _ => return no(format!("`.{}` on this kind of value is not compiled yet", f)),
        };
        let coll = &self.prog.colls[c];
        if let Some(k) = coll.children.iter().find(|x| x.0 == f) { return Ok(CV::Coll(k.1, idx)); }
        let Some(ci) = coll.cols.iter().position(|x| x.path == f && !matches!(x.kind, CKind::StrBytes(_))) else { return no(format!("field `{}` (nested struct fields are not compiled yet)", f)) };
        let col = coll.cols[ci].clone();
        Ok(match col.kind {
            CKind::Scalar(s) => self.load(&col.off.to_string(), &idx, s)?,
            CKind::StrLen => CV::StateStr(c, idx, ci),
            CKind::Ref(t) => {
                let (s, gv, k) = (self.tmp("r"), self.tmp("g"), self.tmp("i"));
                self.line(&format!("u32 {} = LD_U32({}, (i64)({}) * 2), {} = LD_U32({}, (i64)({}) * 2), {} = 0;", s, col.off, idx, gv, col.off + 4, idx, k));
                let on_dead = match dead { Some(l) => format!("goto {};", l), None => "TAIL(c->h->dead(c));".into() };
                self.line(&format!("if ({} != 0 && !DEREF({}, {} - 1, {}, &{})) {}", s, t, s, gv, k, on_dead));
                CV::Ref(t, s, k)
            }
            CKind::Arr(..) => return no(format!("array field `{}` not compiled yet", f)),
            CKind::StrBytes(_) => unreachable!(),
        })
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
                (CV::Item(c1, x), CV::Item(c2, y)) => Some(if c1 == c2 { format!("({} == {})", x, y) } else { "0".into() }),
                (CV::Ref(c1, s, k), CV::Item(c2, y)) | (CV::Item(c2, y), CV::Ref(c1, s, k)) => Some(if c1 == c2 { format!("({} != 0 && {} == {})", s, k, y) } else { "0".into() }),
                (CV::Null, CV::Null) => Some("1".into()),
                (CV::Null, CV::Txt(t)) | (CV::Txt(t), CV::Null) => Some(format!("(!{}.present)", t)),
                (CV::Null, CV::Any(v)) | (CV::Any(v), CV::Null) => Some(format!("({}.tag == 0)", v)),
                (CV::Null, CV::Ref(_, s, _)) | (CV::Ref(_, s, _), CV::Null) => Some(format!("({} == 0)", s)),
                (CV::Null, CV::Int(_) | CV::Bool(_) | CV::Lit(_) | CV::Item(..) | CV::StateStr(..)) | (CV::Int(_) | CV::Bool(_) | CV::Lit(_) | CV::Item(..) | CV::StateStr(..), CV::Null) => Some("0".into()),
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
        // anything else (strings, mixed kinds, floats): the interpreter's own operator
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
                        let a = self.expr(&args[0], env, Some(&l))?;
                        let av = self.val(a)?;
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
                        let a = self.expr(&args[0], env, Some(&l))?;
                        let av = self.val(a)?;
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
    fn builtin(&mut self, bi: B, n: &str, args: &'p [Expr], a: Vec<CV>, lit: impl Fn(Option<&Expr>) -> Option<String>,
        int_out: impl Fn(&mut Self, String) -> CV, txt_out: impl Fn(&mut Self, String) -> CV, val_out: impl Fn(&mut Self, String) -> CV) -> R<CV> {
        let _ = (&lit, &txt_out, &val_out);
        if let Some(code) = BUILTINS.iter().position(|x| *x == bi) {
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
                B::Int | B::Len | B::Now | B::Random | B::Stat | B::WorkerIndex | B::WorkerCount => CV::Int(format!("{}.i", r)),
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
                _ => return no("slot(item)"),
            },
            B::Crash => { self.line("TAIL(c->h->crash(c));"); CV::Null }
            _ => unreachable!("handled above"),
        })
    }

    /// Inline a user function: its parameters become values in a fresh frame; `return` jumps to the end of the inlined block.
    fn inline(&mut self, k: u32, args: &'p [Expr], env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        if self.inlining.contains(&k) { return no("recursive functions are not compiled yet"); }
        let f = &self.api.fns[k as usize];
        let mut fenv: Vec<Option<CV>> = vec![None; f.slots as usize];
        for (i, a) in args.iter().enumerate() { let v = self.expr(a, env, dead)?; fenv[i] = Some(self.hold(v, false)?); }
        // a function that is just `return e;` needs no block
        if let [Stmt::Return(Some(body))] = f.body.as_slice() {
            self.inlining.push(k);
            let r = self.expr(body, &mut fenv, dead);
            self.inlining.pop();
            return r;
        }
        let (var, label) = (self.tmp("ret"), self.tmp("end"));
        let saved = std::mem::take(&mut self.body);
        self.inlining.push(k);
        self.ret.push(Ret { var: var.clone(), label: label.clone(), kind: None });
        self.depth += 1;
        let r = self.block(&f.body, &mut fenv);
        self.depth -= 1;
        let ret = self.ret.pop().unwrap();
        self.inlining.pop();
        let inner = std::mem::replace(&mut self.body, saved);
        r?;
        let ends_by_itself = matches!(f.body.last(), Some(Stmt::Return(_)) | Some(Stmt::Fail(..)) | Some(Stmt::Respond(..)) | Some(Stmt::Forward { .. }));
        let kind = ret.kind.unwrap_or(CV::Null);
        if !ends_by_itself && !matches!(kind, CV::Null) { return no("a function that can end without returning its value"); }
        let (decl, out) = match &kind {
            CV::Int(_) => (format!("i64 {}_v = 0;", var), CV::Int(format!("{}_v", var))),
            CV::Bool(_) => (format!("int {}_v = 0;", var), CV::Bool(format!("{}_v", var))),
            CV::Txt(_) => (format!("Txt {}_v;", var), CV::Txt(format!("{}_v", var))),
            CV::Any(_) => (format!("Val {}_v;", var), CV::Any(format!("{}_v", var))),
            CV::Item(c, _) => (format!("u32 {}_v = 0;", var), CV::Item(*c, format!("{}_v", var))),
            CV::Ref(c, _, _) => (format!("u32 {}_s = 0, {}_v = 0;", var, var), CV::Ref(*c, format!("{}_s", var), format!("{}_v", var))),
            CV::Null => (String::new(), CV::Null),
            _ => return no("a function returning this kind of value is not compiled yet"),
        };
        if !decl.is_empty() { self.line(&decl); }
        self.line("{");
        self.body.push_str(&inner);
        self.line("}");
        self.line(&format!("{}: ;", label));
        Ok(out)
    }

    /// Emit a loop over a list or pool instance (with `reverse`, `where` and `limit`), running `body` with the item bound.
    fn each(&mut self, it: &'p Iter, env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        if it.order.is_some() { return no("each ... order by not compiled yet"); }
        let CV::Coll(c, inst) = self.expr(&it.coll, env, None)? else { return no("each over something other than a collection") };
        let coll = self.prog.colls[c].clone();
        if coll.cols.iter().any(|x| x.path.is_empty()) || coll.children.iter().any(|x| x.0.is_empty()) { return no("each over a collection of plain values not compiled yet"); }
        let lim = match &it.limit { Some(e) => match self.expr(e, env, None)? { CV::Int(x) => Some(self.hold_int(x)), _ => return no("limit must be an integer") }, None => None };
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        let (k, m, hits) = (self.tmp("k"), self.tmp("m"), self.tmp("hits"));
        self.open("{");
        self.line(&format!("u32 {} = 0;", hits));
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
        if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); }
        env[it.slot as usize] = Some(CV::Item(c, m.clone()));
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
                self.each(it, env, |g, env, hits| {
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
                let v = self.expr(&args[0], env, Some(&l))?;
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
            CV::Bool(e) if e == "1" || e == "0" => self.out(if e == "1" { "true" } else { "false" }),
            CV::Bool(e) => self.line(&format!("CALL({} ? c->h->out(c, \"true\", 4) : c->h->out(c, \"false\", 5));", e)),
            CV::Lit(s) => { let j = json_lit(&s); self.out(&j); }
            CV::Txt(t) => self.line(&format!("CALL({}.present ? c->h->json_txt(c, &{}) : c->h->out(c, \"null\", 4));", t, t)),
            CV::Any(v) => self.call(&format!("c->h->json_val(c, &{})", v)),
            CV::StateStr(c, idx, ci) => self.call(&format!("c->h->json_col(c, {}, {}, {})", c, idx, ci)),
            CV::Null => self.out("null"),
            CV::Ref(_, s, _) => { self.line(&format!("if ({} == 0) {{ CALL(c->h->out(c, \"null\", 4)); }}", s)); self.line("else FAIL(500, \"cannot write this value as JSON\", 30);"); }
            CV::Item(..) | CV::Coll(..) | CV::Plain(..) => self.fail(500, "cannot write this value as JSON"),
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
    const CALLS: [&str; 20] = ["if", "for", "CALL", "TAIL", "FAIL", "DEREF", "ADD", "SUB", "MUL", "DIV", "MOD", "NEG", "LD_I64", "LD_U64", "LD_I32", "LD_U32", "LD_I16", "LD_U16", "LD_I8", "LD_U8"];
    const MORE_CALLS: [&str; 2] = ["LD_F32", "LD_F64"];
    const FORBIDDEN: [&str; 24] = ["while", "do", "switch", "static", "extern", "register", "volatile", "asm", "__asm", "__asm__", "sizeof", "union", "struct", "typedef", "enum",
        "malloc", "calloc", "realloc", "free", "memcpy", "memmove", "memset", "alloca", "setjmp"];
    const TYPES: [&str; 5] = ["i64", "u32", "int", "Val", "Txt"];
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
                if !owners.iter().any(|o| ["CALL", "TAIL", "DEREF"].contains(o)) { return err(i, "helper called without CALL, TAIL or DEREF"); }
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
                if unary || next == ")" { return err(i, "pointer dereference or pointer type"); }
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
        let c = emit(&prog, &a, ("chat.api", api));
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
