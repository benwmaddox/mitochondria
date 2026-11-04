//! The endpoint language: tokens, syntax tree, parser, and the binder.
//! A file holds `fn` declarations and `endpoint` handlers. Types written in `fn` signatures are documentation for now.
//!
//! The binder runs once when a server loads the file. It gives every local a fixed slot in its function's frame,
//! resolves every name to a local, a global or a function, checks argument counts and assignments, and turns builtin
//! calls into an enum. Errors that used to surface as a 500 at run time (an unknown name, assigning to a `let`)
//! now stop the server from starting.
use std::cell::Cell;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok { Id(String), Int(i64), Flt(f64), Str(String), Sym(String) }

/// What a name refers to, filled in by the binder.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum VarRes { #[default] Unbound, Local(u32), Global(u32) }

/// A field access remembers the last collection it saw and where the field was in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldHit { None, Col(u32, u32), Child(u32, u32) } // (collection, column or child collection)
impl Default for FieldHit { fn default() -> FieldHit { FieldHit::None } }

/// Built-in functions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum B { Body, Param, Query, Header, Fwd, Int, Str, Len, Now, Random, Min, Max, Count, Capacity, Slot, Stat, Token, Auth, WorkerIndex, WorkerCount, Crash, Or, Exists, MaxLen }
impl B {
    fn from(n: &str) -> Option<B> {
        use B::*;
        Some(match n {
            "body" => Body, "param" => Param, "query" => Query, "header" => Header, "fwd" => Fwd, "int" => Int, "str" => Str, "len" => Len, "now" => Now,
            "random" => Random, "min" => Min, "max" => Max, "count" => Count, "capacity" => Capacity, "slot" => Slot, "stat" => Stat, "token" => Token,
            "auth" => Auth, "worker_index" => WorkerIndex, "worker_count" => WorkerCount, "__crash" => Crash, "or" => Or, "exists" => Exists, "maxlen" => MaxLen,
            _ => return None,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CallRes { #[default] Unbound, Builtin(B), User(u32) }
impl Default for B { fn default() -> B { B::Now } }

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64), Float(f64), Str(String), Bool(bool), Null,
    Var(String, VarRes),
    Field(Box<Expr>, String, Cell<FieldHit>),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>, CallRes),
    Un(char, Box<Expr>),
    Bin(String, Box<Expr>, Box<Expr>),
}

/// `each m in room.messages reverse where ... order by ... desc limit n`
#[derive(Debug, Clone)]
pub struct Iter { pub var: String, pub slot: u32, pub coll: Expr, pub reverse: bool, pub wher: Option<Expr>, pub order: Option<(Expr, bool)>, pub limit: Option<Expr> }

#[derive(Debug, Clone)]
pub enum Json { Obj(Vec<(String, Json)>), Arr(Iter, Box<Json>), E(Expr) }

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(String, u32, Expr),
    Var(String, u32, Expr),
    Alloc { var: String, slot: u32, coll: Expr, try_: bool },
    Assign(Expr, Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    Each(Iter, Vec<Stmt>),
    Free(Expr),
    Fail(u16, String),
    Respond(u16, Json),
    Return(Option<Expr>),
    /// Hand the request on to another worker: `forward rooms[key] { "user": expr };`
    Forward { kind: String, key: Expr, fields: Vec<(String, Expr)> },
    Do(Expr),
}

#[derive(Debug, Clone)]
pub enum Seg { Lit(String), Param(String) }
#[derive(Debug, Clone)]
/// `lines`: first and last source line (1-based) of the endpoint, for comments in generated code.
pub struct Endpoint { pub method: String, pub segs: Vec<Seg>, pub body: Vec<Stmt>, pub text: String, pub slots: u32, pub lines: (u32, u32) }
#[derive(Debug, Clone)]
pub struct Func { pub name: String, pub params: Vec<String>, pub body: Vec<Stmt>, pub slots: u32 }
#[derive(Debug, Clone, Default)]
pub struct Api { pub eps: Vec<Endpoint>, pub fns: Vec<Func> }

pub fn lex(src: &str) -> Result<Vec<Tok>, String> { lex_lines(src).map(|t| t.0) }

/// Tokens, and the source line each one starts on.
fn lex_lines(src: &str) -> Result<(Vec<Tok>, Vec<u32>), String> {
    let c: Vec<char> = src.chars().collect();
    let (mut i, mut out, mut at, mut line, mut tl) = (0, Vec::new(), Vec::new(), 1u32, 1u32);
    while i < c.len() {
        let ch = c[i];
        while at.len() < out.len() { at.push(tl); } // tokens pushed by the last step started on its line
        tl = line;
        if ch == '\n' { line += 1; }
        if ch.is_whitespace() { i += 1; }
        else if ch == '/' && c.get(i + 1) == Some(&'/') { while i < c.len() && c[i] != '\n' { i += 1; } }
        else if ch.is_alphabetic() || ch == '_' { let s = i; while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') { i += 1; } out.push(Tok::Id(c[s..i].iter().collect())); }
        else if ch.is_ascii_digit() {
            let s = i; while i < c.len() && c[i].is_ascii_digit() { i += 1; }
            if c.get(i) == Some(&'.') && c.get(i + 1).map_or(false, |d| d.is_ascii_digit()) { i += 1; while i < c.len() && c[i].is_ascii_digit() { i += 1; } out.push(Tok::Flt(c[s..i].iter().collect::<String>().parse().unwrap())); }
            else { out.push(Tok::Int(c[s..i].iter().collect::<String>().parse().map_err(|_| "number too large".to_string())?)); }
        }
        else if ch == '"' {
            i += 1; let mut s = String::new();
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' && i + 1 < c.len() { i += 1; s.push(match c[i] { 'n' => '\n', 't' => '\t', o => o }); } else { s.push(c[i]); }
                i += 1;
            }
            if i >= c.len() { return Err("unterminated string".into()); }
            line += s.matches('\n').count() as u32;
            i += 1; out.push(Tok::Str(s));
        }
        else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            if ["==", "!=", "<=", ">=", "&&", "||", "->"].contains(&two.as_str()) { out.push(Tok::Sym(two)); i += 2; }
            else { out.push(Tok::Sym(ch.to_string())); i += 1; }
        }
    }
    while at.len() < out.len() { at.push(tl); }
    Ok((out, at))
}

struct P { t: Vec<Tok>, i: usize, lines: Vec<u32> }
type R<T> = Result<T, String>;

pub fn parse(src: &str) -> R<Api> {
    let (t, lines) = lex_lines(src)?;
    let mut p = P { t, i: 0, lines };
    let mut api = Api::default();
    while p.i < p.t.len() {
        if p.is_kw("fn") {
            p.i += 1;
            let name = p.id()?; p.eat("(")?;
            let mut params = Vec::new();
            while !p.sym(")") { params.push(p.id()?); if p.sym(":") { p.skip_type()?; } p.sym(","); }
            if p.sym("->") { p.skip_type()?; }
            let body = p.block()?;
            api.fns.push(Func { name, params, body, slots: 0 });
            continue;
        }
        let first = p.lines.get(p.i).copied().unwrap_or(0);
        p.kw("endpoint")?;
        let method = p.id()?;
        let mut segs = Vec::new();
        while p.sym("/") {
            if p.sym("{") { let n = p.id()?; p.eat("}")?; segs.push(Seg::Param(n)); } else { segs.push(Seg::Lit(p.id()?)); }
        }
        let body = p.block()?;
        let text = format!("{} {}", method, segs.iter().map(|s| match s { Seg::Lit(l) => format!("/{}", l), Seg::Param(n) => format!("/{{{}}}", n) }).collect::<String>());
        let last = p.lines.get(p.i.saturating_sub(1)).copied().unwrap_or(first);
        api.eps.push(Endpoint { method, segs, body, text, slots: 0, lines: (first, last) });
    }
    Ok(api)
}

impl P {
    fn peek(&self) -> Option<&Tok> { self.t.get(self.i) }
    fn here(&self) -> String { format!("{:?}", self.peek()) }
    fn sym(&mut self, s: &str) -> bool { if matches!(self.peek(), Some(Tok::Sym(x)) if x == s) { self.i += 1; true } else { false } }
    fn eat(&mut self, s: &str) -> R<()> { if self.sym(s) { Ok(()) } else { Err(format!("expected `{}`, found {}", s, self.here())) } }
    fn is_kw(&self, k: &str) -> bool { matches!(self.peek(), Some(Tok::Id(x)) if x == k) }
    fn kw(&mut self, k: &str) -> R<()> { if self.is_kw(k) { self.i += 1; Ok(()) } else { Err(format!("expected `{}`, found {}", k, self.here())) } }
    fn id(&mut self) -> R<String> { match self.t.get(self.i) { Some(Tok::Id(x)) => { self.i += 1; Ok(x.clone()) } _ => Err(format!("expected a name, found {}", self.here())) } }
    fn int(&mut self) -> R<i64> { match self.t.get(self.i) { Some(Tok::Int(n)) => { self.i += 1; Ok(*n) } _ => Err(format!("expected a number, found {}", self.here())) } }
    /// `name`, `name<...>` or `name[N]`: types in signatures are read but not checked yet.
    fn skip_type(&mut self) -> R<()> {
        self.id()?;
        if self.sym("<") { let mut depth = 1; while depth > 0 { if self.sym("<") { depth += 1; } else if self.sym(">") { depth -= 1; } else if self.peek().is_none() { return Err("unfinished type".into()); } else { self.i += 1; } } }
        while self.sym("[") { self.int()?; self.eat("]")?; }
        Ok(())
    }

    fn block(&mut self) -> R<Vec<Stmt>> {
        self.eat("{")?;
        let mut v = Vec::new();
        while !self.sym("}") { if self.peek().is_none() { return Err("missing `}`".into()); } v.push(self.stmt()?); }
        Ok(v)
    }

    fn iter(&mut self) -> R<Iter> {
        let var = self.id()?; self.kw("in")?; let coll = self.postfix()?;
        let reverse = if self.is_kw("reverse") { self.i += 1; true } else { false };
        let wher = if self.is_kw("where") { self.i += 1; Some(self.expr()?) } else { None };
        let order = if self.is_kw("order") { self.i += 1; self.kw("by")?; let e = self.expr()?; let desc = if self.is_kw("desc") { self.i += 1; true } else { false }; Some((e, desc)) } else { None };
        let limit = if self.is_kw("limit") { self.i += 1; Some(self.expr()?) } else { None };
        Ok(Iter { var, slot: 0, coll, reverse, wher, order, limit })
    }

    fn stmt(&mut self) -> R<Stmt> {
        if self.is_kw("let") || self.is_kw("var") {
            let is_var = self.is_kw("var");
            self.i += 1; let n = self.id()?; self.eat("=")?;
            if !is_var && (self.is_kw("alloc") || self.is_kw("try_alloc")) {
                let try_ = self.is_kw("try_alloc"); self.i += 1; let coll = self.postfix()?; self.eat(";")?;
                return Ok(Stmt::Alloc { var: n, slot: 0, coll, try_ });
            }
            let e = self.expr()?; self.eat(";")?;
            return Ok(if is_var { Stmt::Var(n, 0, e) } else { Stmt::Let(n, 0, e) });
        }
        if self.is_kw("if") { return self.if_stmt(); }
        if self.is_kw("each") { self.i += 1; let it = self.iter()?; let b = self.block()?; return Ok(Stmt::Each(it, b)); }
        if self.is_kw("free") { self.i += 1; let e = self.expr()?; self.eat(";")?; return Ok(Stmt::Free(e)); }
        if self.is_kw("forward") {
            self.i += 1;
            let kind = self.id()?; self.eat("[")?; let key = self.expr()?; self.eat("]")?;
            let mut fields = Vec::new();
            if self.sym("{") {
                while !self.sym("}") {
                    let k = match self.t.get(self.i) { Some(Tok::Str(s)) => { self.i += 1; s.clone() } _ => return Err(format!("expected a field name string, found {}", self.here())) };
                    self.eat(":")?; fields.push((k, self.expr()?)); self.sym(",");
                }
            }
            self.eat(";")?;
            return Ok(Stmt::Forward { kind, key, fields });
        }
        if self.is_kw("return") { self.i += 1; if self.sym(";") { return Ok(Stmt::Return(None)); } let e = self.expr()?; self.eat(";")?; return Ok(Stmt::Return(Some(e))); }
        if self.is_kw("fail") {
            self.i += 1; let code = self.int()? as u16;
            let msg = match self.t.get(self.i) { Some(Tok::Str(s)) => { self.i += 1; s.clone() } _ => return Err("fail needs a message string".into()) };
            self.eat(";")?; return Ok(Stmt::Fail(code, msg));
        }
        if self.is_kw("respond") { self.i += 1; let code = self.int()? as u16; let j = self.json()?; self.eat(";")?; return Ok(Stmt::Respond(code, j)); }
        let lhs = self.expr()?;
        if self.sym(";") { return Ok(Stmt::Do(lhs)); }
        self.eat("=")?;
        let rhs = self.expr()?; self.eat(";")?;
        Ok(Stmt::Assign(lhs, rhs))
    }

    fn if_stmt(&mut self) -> R<Stmt> {
        self.kw("if")?; let c = self.expr()?; let t = self.block()?;
        let e = if self.is_kw("else") { self.i += 1; if self.is_kw("if") { vec![self.if_stmt()?] } else { self.block()? } } else { vec![] };
        Ok(Stmt::If(c, t, e))
    }

    fn json(&mut self) -> R<Json> {
        if self.sym("{") {
            let mut f = Vec::new();
            while !self.sym("}") {
                let k = match self.t.get(self.i) { Some(Tok::Str(s)) => { self.i += 1; s.clone() } _ => return Err(format!("expected a key string, found {}", self.here())) };
                self.eat(":")?; f.push((k, self.json()?)); self.sym(",");
            }
            return Ok(Json::Obj(f));
        }
        if self.sym("[") { self.kw("each")?; let it = self.iter()?; let v = self.json()?; self.eat("]")?; return Ok(Json::Arr(it, Box::new(v))); }
        Ok(Json::E(self.expr()?))
    }

    fn expr(&mut self) -> R<Expr> { self.bin(0) }
    fn bin(&mut self, level: usize) -> R<Expr> {
        const LV: [&[&str]; 5] = [&["||"], &["&&"], &["==", "!=", "<", "<=", ">", ">="], &["+", "-"], &["*", "/", "%"]];
        if level == LV.len() { return self.unary(); }
        let mut l = self.bin(level + 1)?;
        loop {
            let op = match self.peek() { Some(Tok::Sym(s)) if LV[level].contains(&s.as_str()) => s.clone(), _ => break };
            self.i += 1;
            let r = self.bin(level + 1)?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn unary(&mut self) -> R<Expr> {
        if self.sym("!") { return Ok(Expr::Un('!', Box::new(self.unary()?))); }
        if self.sym("-") { return Ok(Expr::Un('-', Box::new(self.unary()?))); }
        self.postfix()
    }
    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.sym(".") { e = Expr::Field(Box::new(e), self.id()?, Cell::new(FieldHit::None)); }
            else if self.sym("[") { let i = self.expr()?; self.eat("]")?; e = Expr::Index(Box::new(e), Box::new(i)); }
            else { break; }
        }
        Ok(e)
    }
    fn primary(&mut self) -> R<Expr> {
        match self.t.get(self.i).cloned() {
            Some(Tok::Int(n)) => { self.i += 1; Ok(Expr::Int(n)) }
            Some(Tok::Flt(f)) => { self.i += 1; Ok(Expr::Float(f)) }
            Some(Tok::Str(s)) => { self.i += 1; Ok(Expr::Str(s)) }
            Some(Tok::Sym(s)) if s == "(" => { self.i += 1; let e = self.expr()?; self.eat(")")?; Ok(e) }
            Some(Tok::Id(n)) => {
                self.i += 1;
                match n.as_str() { "true" => return Ok(Expr::Bool(true)), "false" => return Ok(Expr::Bool(false)), "null" => return Ok(Expr::Null), _ => {} }
                if self.sym("(") {
                    let mut a = Vec::new();
                    while !self.sym(")") { a.push(self.expr()?); self.sym(","); }
                    return Ok(Expr::Call(n, a, CallRes::Unbound));
                }
                Ok(Expr::Var(n, VarRes::Unbound))
            }
            _ => Err(format!("unexpected {}", self.here())),
        }
    }
}

// ---------------------------------------------------------------- binder

struct Binder<'g> { names: Vec<(String, u32, bool)>, next: u32, max: u32, globals: &'g dyn Fn(&str) -> Option<usize>, fns: Vec<(String, usize)>, place: String }

/// Resolve every name in `api`. `globals` answers whether a name is a declared global (and which).
pub fn bind(api: &mut Api, globals: &dyn Fn(&str) -> Option<usize>) -> R<()> {
    let fns: Vec<(String, usize)> = api.fns.iter().map(|f| (f.name.clone(), f.params.len())).collect();
    for (k, f) in fns.iter().enumerate() { if fns[..k].iter().any(|g| g.0 == f.0) { return Err(format!("fn `{}` is declared twice", f.0)); } if B::from(&f.0).is_some() { return Err(format!("fn `{}` has the name of a builtin", f.0)); } }
    for f in &mut api.fns {
        let mut b = Binder { names: Vec::new(), next: 0, max: 0, globals, fns: fns.clone(), place: format!("fn {}", f.name) };
        for p in &f.params { b.declare(p, false); }
        b.block(&mut f.body)?;
        f.slots = b.max;
    }
    for e in &mut api.eps {
        let mut b = Binder { names: Vec::new(), next: 0, max: 0, globals, fns: fns.clone(), place: format!("endpoint {}", e.text) };
        b.block(&mut e.body)?;
        e.slots = b.max;
    }
    Ok(())
}

impl Binder<'_> {
    fn err<T>(&self, m: String) -> R<T> { Err(format!("{}: {}", self.place, m)) }
    fn declare(&mut self, n: &str, mutable: bool) -> u32 {
        let s = self.next; self.next += 1; self.max = self.max.max(self.next);
        self.names.push((n.to_string(), s, mutable)); s
    }
    fn block(&mut self, stmts: &mut [Stmt]) -> R<()> {
        let (mark, next) = (self.names.len(), self.next);
        for s in stmts.iter_mut() { self.stmt(s)?; }
        self.names.truncate(mark); self.next = next; // slots of a finished block are reused
        Ok(())
    }
    fn iter_head(&mut self, it: &mut Iter) -> R<()> {
        self.expr(&mut it.coll)?;
        if let Some(l) = &mut it.limit { self.expr(l)?; }
        it.slot = self.declare(&it.var, false);
        if let Some(w) = &mut it.wher { self.expr(w)?; }
        if let Some((o, _)) = &mut it.order { self.expr(o)?; }
        Ok(())
    }
    fn stmt(&mut self, s: &mut Stmt) -> R<()> {
        match s {
            Stmt::Let(n, slot, e) => { self.expr(e)?; *slot = self.declare(n, false); }
            Stmt::Var(n, slot, e) => { self.expr(e)?; *slot = self.declare(n, true); }
            Stmt::Alloc { var, slot, coll, .. } => { self.expr(coll)?; *slot = self.declare(var, false); }
            Stmt::Assign(lhs, rhs) => {
                self.expr(rhs)?;
                if let Expr::Var(n, res) = lhs {
                    match self.names.iter().rev().find(|x| x.0 == *n) {
                        Some(x) if x.2 => *res = VarRes::Local(x.1),
                        Some(_) => return self.err(format!("`{}` is a `let`; declare it with `var` to change it", n)),
                        None => return self.err(format!("unknown variable `{}`", n)),
                    }
                } else { self.expr(lhs)?; }
            }
            Stmt::If(c, t, e) => { self.expr(c)?; self.block(t)?; self.block(e)?; }
            Stmt::Each(it, body) => { let (mark, next) = (self.names.len(), self.next); self.iter_head(it)?; self.block(body)?; self.names.truncate(mark); self.next = next; }
            Stmt::Free(e) | Stmt::Do(e) => self.expr(e)?,
            Stmt::Return(e) => { if let Some(e) = e { self.expr(e)?; } }
            Stmt::Respond(_, j) => self.json(j)?,
            Stmt::Forward { key, fields, .. } => { self.expr(key)?; for f in fields { self.expr(&mut f.1)?; } }
            Stmt::Fail(..) => {}
        }
        Ok(())
    }
    fn json(&mut self, j: &mut Json) -> R<()> {
        match j {
            Json::Obj(f) => { for (_, v) in f { self.json(v)?; } Ok(()) }
            Json::Arr(it, inner) => { let (mark, next) = (self.names.len(), self.next); self.iter_head(it)?; self.json(inner)?; self.names.truncate(mark); self.next = next; Ok(()) }
            Json::E(e) => self.expr(e),
        }
    }
    fn expr(&mut self, e: &mut Expr) -> R<()> {
        match e {
            Expr::Var(n, res) => {
                *res = match self.names.iter().rev().find(|x| x.0 == *n) {
                    Some(x) => VarRes::Local(x.1),
                    None => match (self.globals)(n) { Some(g) => VarRes::Global(g as u32), None => return self.err(format!("unknown name `{}`", n)) },
                };
            }
            Expr::Field(b, _, _) => self.expr(b)?,
            Expr::Index(b, i) => { self.expr(b)?; self.expr(i)?; }
            Expr::Un(_, x) => self.expr(x)?,
            Expr::Bin(_, a, b) => { self.expr(a)?; self.expr(b)?; }
            Expr::Call(n, args, res) => {
                for a in args.iter_mut() { self.expr(a)?; }
                *res = if let Some(k) = self.fns.iter().position(|f| f.0 == *n) {
                    if self.fns[k].1 != args.len() { return self.err(format!("`{}` takes {} arguments, given {}", n, self.fns[k].1, args.len())); }
                    CallRes::User(k as u32)
                } else if let Some(b) = B::from(n) { CallRes::Builtin(b) }
                else { return self.err(format!("unknown function `{}`", n)); };
            }
            _ => {}
        }
        Ok(())
    }
}
