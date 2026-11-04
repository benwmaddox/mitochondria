//! Generates C for bound endpoints. Types are worked out while generating: a value whose collection is known becomes a
//! direct load at a constant offset from the reservation's base; everything else goes through helper functions that
//! the runtime passes in a table (`H` below). An endpoint that uses something the generator does not handle yet stays
//! interpreted; `generate` reports which endpoints compiled and why the others did not.
//!
//! Rules the generated code keeps: it never writes state itself (writes go through helpers, which journal them); it only
//! loads from items it reached by live iteration or a checked index; every helper returns 0 or -1 (an error the runtime
//! has stored), and the code returns at once on -1, so nothing unwinds through C frames.
use crate::lang::*;
use crate::layout::{CKind, Kind, Program, Scalar};

/// The C side of the boundary. `serve.rs` mirrors this table with `#[repr(C)]` in the same order.
pub const PRELUDE: &str = r#"typedef long long i64; typedef unsigned int u32; typedef unsigned char u8;
typedef struct { u32 off, len; int present; } Txt;
typedef struct H H;
typedef struct { u8 *base; void *run; const H *h; } Ctx;
struct H {
    int (*out)(Ctx *, const char *, u32);
    int (*json_i64)(Ctx *, i64);
    int (*json_col)(Ctx *, u32, u32, u32);
    int (*json_txt)(Ctx *, const Txt *);
    int (*auth)(Ctx *, u32, u32 *);
    int (*index)(Ctx *, u32, u32, i64, u32 *);
    int (*param)(Ctx *, u32, Txt *);
    int (*query)(Ctx *, const char *, u32, Txt *);
    int (*lit)(Ctx *, const char *, u32, Txt *);
    int (*to_int)(Ctx *, const Txt *, i64 *);
    int (*deref)(Ctx *, u32, u32, u32, u32 *);
    int (*fail)(Ctx *, int, const char *, u32);
    int (*dead)(Ctx *);
};
"#;

pub struct Generated { pub src: String, pub eps: Vec<Option<String>>, pub notes: Vec<String> }

/// What an expression produced, in C terms.
#[derive(Clone, Debug)]
enum CV {
    Int(String),                     // a C expression of type i64
    Bool(String),                    // a C expression, 0 or 1
    Lit(String),                     // a string literal known when generating
    Txt(String),                     // a Txt variable (a string in the request's text area, or absent)
    Item(usize, String),             // collection, C expression for the item index
    Coll(usize, String),             // collection, C expression for the instance
    StateStr(usize, String, usize),  // a string column of an item: collection, item index, column
    Null,
}

struct G<'p> { prog: &'p Program, api: &'p Api, ep: &'p Endpoint, body: String, n: usize, depth: usize }
type R<T> = Result<T, String>;
fn no<T>(what: impl Into<String>) -> R<T> { Err(what.into()) }

/// A C string literal for `s`, safe for any bytes.
fn c_lit(s: &[u8]) -> String {
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

pub fn generate(prog: &Program, api: &Api) -> Generated {
    let mut src = String::from(PRELUDE);
    let (mut eps, mut notes) = (Vec::new(), Vec::new());
    for (k, ep) in api.eps.iter().enumerate() {
        let mut g = G { prog, api, ep, body: String::new(), n: 0, depth: 0 };
        let mut env: Vec<Option<CV>> = vec![None; ep.slots as usize];
        match g.block(&ep.body, &mut env) {
            Ok(()) => {
                let name = format!("ep{}", k);
                src += &format!("\n// {}\nint {}(Ctx *c) {{\n    int st; u8 *b = c->base; (void)st; (void)b;\n{}    return 0;\n}}\n", ep.text, name, g.body);
                eps.push(Some(name));
            }
            Err(why) => { notes.push(format!("{}: interpreted ({})", ep.text, why)); eps.push(None); }
        }
    }
    Generated { src, eps, notes }
}

impl<'p> G<'p> {
    fn tmp(&mut self, p: &str) -> String { self.n += 1; format!("{}{}", p, self.n) }
    fn line(&mut self, s: &str) { for _ in 0..self.depth + 1 { self.body.push_str("    "); } self.body.push_str(s); self.body.push('\n'); }
    /// Call a helper that returns 0 or -1.
    fn call(&mut self, s: &str) { self.line(&format!("if ((st = {}) != 0) return st;", s)); }

    fn block(&mut self, stmts: &'p [Stmt], env: &mut Vec<Option<CV>>) -> R<()> {
        for s in stmts { self.stmt(s, env)?; }
        Ok(())
    }
    fn stmt(&mut self, s: &'p Stmt, env: &mut Vec<Option<CV>>) -> R<()> {
        match s {
            Stmt::Let(_, slot, e) => { let v = self.expr(e, env, None)?; let v = self.keep(v); env[*slot as usize] = Some(v); }
            Stmt::Respond(code, j) => { self.json(j, env)?; self.line(&format!("return {};", code)); }
            Stmt::Fail(code, m) => self.line(&format!("return c->h->fail(c, {}, {}, {});", code, c_lit(m.as_bytes()), m.len())),
            Stmt::Return(None) => self.line("return 0;"),
            _ => return no(format!("statement `{}` not compiled yet", stmt_name(s))),
        }
        Ok(())
    }

    /// Give an expression a C variable, so it is computed once.
    fn keep(&mut self, v: CV) -> CV {
        match v {
            CV::Int(e) if !e.chars().all(|c| c.is_alphanumeric() || c == '_') => { let t = self.tmp("v"); self.line(&format!("i64 {} = {};", t, e)); CV::Int(t) }
            CV::Bool(e) if !e.chars().all(|c| c.is_alphanumeric() || c == '_') => { let t = self.tmp("v"); self.line(&format!("int {} = {};", t, e)); CV::Bool(t) }
            CV::Item(c, e) if !e.chars().all(|c| c.is_alphanumeric() || c == '_') => { let t = self.tmp("i"); self.line(&format!("u32 {} = {};", t, e)); CV::Item(c, t) }
            o => o,
        }
    }

    /// `dead`: where to jump if a reference turns out not to be live (inside `or(...)`); otherwise the request fails as the interpreter's would.
    fn expr(&mut self, e: &'p Expr, env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        match e {
            Expr::Int(n) => Ok(CV::Int(format!("{}LL", n))),
            Expr::Bool(b) => Ok(CV::Bool(if *b { "1" } else { "0" }.into())),
            Expr::Str(s) => Ok(CV::Lit(s.clone())),
            Expr::Null => Ok(CV::Null),
            Expr::Var(n, VarRes::Local(s)) => env.get(*s as usize).cloned().flatten().ok_or_else(|| format!("`{}` has no compiled value", n)),
            Expr::Var(n, VarRes::Global(g)) => match self.prog.globals[*g as usize].coll { Some(c) => Ok(CV::Coll(c, "0".into())), None => no(format!("plain global `{}` not compiled yet", n)) },
            Expr::Field(base, f, _) => { let bv = self.expr(base, env, dead)?; self.field(bv, f, dead) }
            Expr::Index(base, k) => {
                let (bv, kv) = (self.expr(base, env, dead)?, self.expr(k, env, dead)?);
                let (CV::Coll(c, inst), CV::Int(k)) = (bv, kv) else { return no("only pool[int] indexing is compiled yet") };
                if !matches!(self.prog.colls[c].kind, Kind::Pool(_)) { return no("list indexing not compiled yet"); }
                let t = self.tmp("i");
                self.line(&format!("u32 {};", t));
                self.call(&format!("c->h->index(c, {}, {}, {}, &{})", c, inst, k, t));
                Ok(CV::Item(c, t))
            }
            Expr::Call(n, args, res) => self.callx(n, args, *res, env, dead),
            _ => no("expression kind not compiled yet"),
        }
    }

    fn txt_of(&mut self, v: CV) -> R<String> {
        match v {
            CV::Txt(t) => Ok(t),
            CV::Lit(s) => { let t = self.tmp("t"); self.line(&format!("Txt {};", t)); self.call(&format!("c->h->lit(c, {}, {}, &{})", c_lit(s.as_bytes()), s.len(), t)); Ok(t) }
            _ => no("expected a string"),
        }
    }

    fn callx(&mut self, n: &str, args: &'p [Expr], res: CallRes, env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        match res {
            CallRes::User(k) => {
                // a function whose body is `return e;` is inlined: its parameters become values in a fresh frame
                let f = &self.api.fns[k as usize];
                let [Stmt::Return(Some(body))] = f.body.as_slice() else { return no(format!("fn `{}` is not a single return yet", f.name)) };
                let mut fenv: Vec<Option<CV>> = vec![None; f.slots as usize];
                for (i, a) in args.iter().enumerate() { let v = self.expr(a, env, dead)?; fenv[i] = Some(self.keep(v)); }
                self.expr(body, &mut fenv, dead)
            }
            CallRes::Builtin(b) => {
                let lit = |a: Option<&Expr>| match a { Some(Expr::Str(s)) => Some(s.clone()), _ => None };
                match b {
                    B::Auth => {
                        let Some(Expr::Var(_, VarRes::Global(g))) = args.first() else { return no("auth(pool) needs a global pool") };
                        let Some(c) = self.prog.globals[*g as usize].coll else { return no("auth needs a pool") };
                        let t = self.tmp("i"); self.line(&format!("u32 {};", t)); self.call(&format!("c->h->auth(c, {}, &{})", c, t));
                        Ok(CV::Item(c, t))
                    }
                    B::Param => {
                        let Some(name) = lit(args.first()) else { return no("param needs a literal name") };
                        let seg = self.ep.segs.iter().position(|s| matches!(s, Seg::Param(p) if *p == name));
                        let t = self.tmp("t"); self.line(&format!("Txt {};", t));
                        match seg { Some(si) => self.call(&format!("c->h->param(c, {}, &{})", si, t)), None => self.line(&format!("{}.present = 0;", t)) }
                        Ok(CV::Txt(t))
                    }
                    B::Query => {
                        let Some(name) = lit(args.first()) else { return no("query needs a literal name") };
                        let t = self.tmp("t"); self.line(&format!("Txt {};", t));
                        self.call(&format!("c->h->query(c, {}, {}, &{})", c_lit(name.as_bytes()), name.len(), t));
                        Ok(CV::Txt(t))
                    }
                    B::Int => match self.expr(&args[0], env, dead)? {
                        CV::Int(e) => Ok(CV::Int(e)),
                        v => { let t = self.txt_of(v)?; let r = self.tmp("v"); self.line(&format!("i64 {};", r)); self.call(&format!("c->h->to_int(c, &{}, &{})", t, r)); Ok(CV::Int(r)) }
                    },
                    B::Min | B::Max => {
                        let (CV::Int(a), CV::Int(b2)) = (self.expr(&args[0], env, dead)?, self.expr(&args[1], env, dead)?) else { return no("min/max need integers") };
                        let (a, b2) = (self.keep(CV::Int(a)), self.keep(CV::Int(b2)));
                        let (CV::Int(a), CV::Int(b2)) = (a, b2) else { unreachable!() };
                        Ok(CV::Int(format!("({} {} {} ? {} : {})", a, if b == B::Min { "<" } else { ">" }, b2, a, b2)))
                    }
                    B::Or => {
                        // or(string-or-absent, fallback) outside JSON: a string
                        let a = self.expr(&args[0], env, None)?;
                        let CV::Txt(t) = a else { return no("or(...) here only for strings yet") };
                        let fb = self.expr(&args[1], env, dead)?;
                        let f = self.txt_of(fb)?;
                        self.line(&format!("if (!{}.present) {} = {};", t, t, f));
                        Ok(CV::Txt(t))
                    }
                    _ => no(format!("builtin `{}` not compiled yet", n)),
                }
            }
            CallRes::Unbound => no("unbound call"),
        }
    }

    fn field(&mut self, base: CV, f: &str, dead: Option<&str>) -> R<CV> {
        let CV::Item(c, idx) = base else { return no(format!("`.{}` on something other than an item is not compiled yet", f)) };
        let coll = &self.prog.colls[c];
        if let Some(k) = coll.children.iter().find(|x| x.0 == f) { return Ok(CV::Coll(k.1, idx)); }
        let Some(ci) = coll.cols.iter().position(|x| x.path == f && !matches!(x.kind, CKind::StrBytes(_))) else { return no(format!("field `{}` not found", f)) };
        let col = &coll.cols[ci];
        let at = |sz: usize| format!("(b + {} + (i64)({}) * {})", col.off, idx, sz);
        Ok(match col.kind.clone() {
            CKind::Scalar(Scalar::I64) => CV::Int(format!("*(i64 *){}", at(8))),
            CKind::Scalar(Scalar::I32) => CV::Int(format!("(i64)*(int *){}", at(4))),
            CKind::Scalar(Scalar::U32) => CV::Int(format!("(i64)*(u32 *){}", at(4))),
            CKind::Scalar(Scalar::Bool) => CV::Bool(format!("(*(u8 *){} != 0)", at(1))),
            CKind::StrLen => CV::StateStr(c, idx, ci),
            CKind::Ref(t) => {
                let (s, gv, k) = (self.tmp("r"), self.tmp("g"), self.tmp("i"));
                self.line(&format!("u32 {} = *(u32 *){}, {} = *(u32 *)({} + 4), {};", s, at(8), gv, at(8), k));
                let on_dead = match dead { Some(l) => format!("goto {};", l), None => "return c->h->dead(c);".into() };
                self.line(&format!("if ({} == 0 || !c->h->deref(c, {}, {} - 1, {}, &{})) {}", s, t, s, gv, k, on_dead));
                CV::Item(t, k)
            }
            k => return no(format!("field `{}` of kind {:?} not compiled yet", f, k)),
        })
    }

    fn json(&mut self, j: &'p Json, env: &mut Vec<Option<CV>>) -> R<()> {
        match j {
            Json::Obj(fields) => {
                let mut first = true;
                let mut text = String::from("{");
                for (k, v) in fields {
                    if !first { text.push(','); }
                    first = false;
                    text += &json_lit(k); text.push(':');
                    self.out(&text); text.clear();
                    self.json(v, env)?;
                }
                text.push('}'); self.out(&text);
                Ok(())
            }
            Json::Arr(it, inner) => {
                if it.wher.is_some() || it.order.is_some() { return no("each ... where/order by not compiled yet"); }
                let CV::Coll(c, inst) = self.expr(&it.coll, env, None)? else { return no("each over something other than a collection") };
                let coll = &self.prog.colls[c];
                if !coll.is_list() { return no("each over a pool not compiled yet"); }
                let lim = match &it.limit { Some(e) => match self.expr(e, env, None)? { CV::Int(x) => Some(x), _ => return no("limit must be an integer") }, None => None };
                let (len, head, k, m) = (self.tmp("len"), self.tmp("head"), self.tmp("k"), self.tmp("m"));
                let (hdr, mult, cap) = (coll.hdr, coll.mult, coll.cap);
                self.out("[");
                self.line(&format!("{{ u32 {} = *(u32 *)(b + {} + 4 * (0 * {} + (i64)({}))), {} = *(u32 *)(b + {} + 4 * (1 * {} + (i64)({})));", len, hdr, mult, inst, head, hdr, mult, inst));
                if let Some(l) = &lim { self.line(&format!("if ({} < 0) return c->h->fail(c, 400, \"bad limit\", 9);", l)); }
                let cond = match &lim { Some(l) => format!("{} < {} && (i64){} < {}", k, len, k, l), None => format!("{} < {}", k, len) };
                self.line(&format!("for (u32 {} = 0; {}; {}++) {{", k, cond, k));
                self.depth += 1;
                let pos = if it.reverse { format!("({} + {} - 1 - {})", head, len, k) } else { format!("({} + {})", head, k) };
                self.line(&format!("u32 {} = (u32)((i64)({}) * {} + {} % {});", m, inst, cap, pos, cap));
                self.line(&format!("if ({}) {{ if ((st = c->h->out(c, \",\", 1)) != 0) return st; }}", k));
                env[it.slot as usize] = Some(CV::Item(c, m));
                let r = self.json(inner, env);
                self.depth -= 1;
                self.line("} }");
                r?;
                self.out("]");
                Ok(())
            }
            Json::E(Expr::Call(_, args, CallRes::Builtin(B::Or))) => {
                // or(value that may be a dead reference, fallback): write the value, or the fallback if it was dead
                let (l, end) = (self.tmp("dead"), self.tmp("done"));
                self.line("{");
                let v = self.expr(&args[0], env, Some(&l))?;
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
            CV::Bool(e) => self.line(&format!("if ((st = ({} ? c->h->out(c, \"true\", 4) : c->h->out(c, \"false\", 5))) != 0) return st;", e)),
            CV::Lit(s) => { let j = json_lit(&s); self.out(&j); }
            CV::Txt(t) => self.line(&format!("if ((st = ({}.present ? c->h->json_txt(c, &{}) : c->h->out(c, \"null\", 4))) != 0) return st;", t, t)),
            CV::StateStr(c, idx, ci) => self.call(&format!("c->h->json_col(c, {}, {}, {})", c, idx, ci)),
            CV::Null => self.out("null"),
            CV::Item(..) | CV::Coll(..) => return no("cannot write an item or collection as JSON"),
        }
        Ok(())
    }
}

fn stmt_name(s: &Stmt) -> &'static str {
    match s {
        Stmt::Let(..) => "let", Stmt::Var(..) => "var", Stmt::Alloc { .. } => "alloc", Stmt::Assign(..) => "assignment", Stmt::If(..) => "if", Stmt::Each(..) => "each",
        Stmt::Free(..) => "free", Stmt::Fail(..) => "fail", Stmt::Respond(..) => "respond", Stmt::Return(..) => "return", Stmt::Forward { .. } => "forward", Stmt::Do(..) => "call statement",
    }
}
