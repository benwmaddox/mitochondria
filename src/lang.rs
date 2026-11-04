//! The endpoint language: tokens, syntax tree, parser, and the binder.
//! A file holds `fn` declarations and `endpoint` handlers. Types written in `fn` signatures are documentation for now.
//!
//! The binder runs once when a server loads the file. It gives every local a fixed slot in its function's frame,
//! resolves every name to a local, a global or a function, checks argument counts and assignments, and turns builtin
//! calls into an enum. Errors that used to surface as a 500 at run time (an unknown name, assigning to a `let`)
//! now stop the server from starting.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok { Id(String), Int(i64), Flt(f64), Str(String), Sym(String) }

/// What a name refers to, filled in by the binder.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum VarRes { #[default] Unbound, Local(u32), Global(u32) }

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
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>, CallRes),
    Un(char, Box<Expr>),
    Bin(String, Box<Expr>, Box<Expr>),
}

/// `each m in room.messages reverse where ... limit n`
#[derive(Debug, Clone)]
/// `each var in coll ...`, or with `hi`, a range of integers `each var in coll..hi ...` (from `coll`, the low bound,
/// up to but not including `hi`).
/// `from`: where a loop over a module instance starts. `via`: that instance's iteration functions (set by the binder).
/// `whil`: `while cond` ends the loop as soon as `cond` is false (tested before `where` and the body).
pub struct Iter { pub var: String, pub slot: u32, pub coll: Expr, pub hi: Option<Expr>, pub from: Option<Expr>, pub reverse: bool, pub whil: Option<Expr>, pub wher: Option<Expr>, pub limit: Option<Expr>, pub via: Option<Via> }

/// A module instance's iteration functions: a cursor from `first(from)` (or `last(from)` for reverse), the next from
/// `next(cur)` (or `prev`), the item at it from `item(cur)`; a null cursor ends the loop.
#[derive(Debug, Clone, Copy)]
pub struct Via { pub first: u32, pub next: u32, pub item: u32, pub last: Option<u32>, pub prev: Option<u32> }

#[derive(Debug, Clone)]
/// An answer's JSON: an object, an array made by a loop, a value, or nothing (`no_content()`).
pub enum Json { Obj(Vec<(String, Json)>), Arr(Iter, Box<Json>), E(Expr), Empty }

/// The `T` of an endpoint's `Response<T>`: the shape of every answer it gives. `Name` is a scalar (`i64`, `bool`,
/// `string`, ...), `any` (a value whose kind is known only at run time), `none` (no body), or a declared struct.
#[derive(Debug, Clone, PartialEq)]
pub enum RType { Name(String), Opt(Box<RType>), Arr(Box<RType>), Obj(Vec<(String, RType)>) }

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(String, u32, Expr),
    Var(String, u32, Expr),
    Alloc { var: String, slot: u32, coll: Expr, try_: bool },
    Assign(Expr, Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    Each(Iter, Vec<Stmt>),
    /// `coll.remove(item)`: the item leaves the collection when the request succeeds.
    Remove(Expr, Expr),
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
/// An endpoint: a function with an attribute (`@post("/players") fn add_player() -> Response<T> { ... }`).
pub struct Endpoint { pub method: String, pub segs: Vec<Seg>, pub body: Vec<Stmt>, pub text: String, pub slots: u32, pub lines: (u32, u32), pub name: String, pub resp: RType }
#[derive(Debug, Clone)]
/// A function. A module instance's functions are named `instance.name` (written by `modules::expand`); `public` ones
/// (`pub fn`) can be called from outside the module.
pub struct Func { pub name: String, pub params: Vec<String>, pub body: Vec<Stmt>, pub slots: u32, pub public: bool }
#[derive(Debug, Clone, Default)]
pub struct Api { pub eps: Vec<Endpoint>, pub fns: Vec<Func> }

/// Names (of fields, collections, functions, workers) are at most this long, and `fail` messages at most MAX_FAIL
/// bytes, so error messages fit every runtime's buffer.
pub const MAX_NAME: usize = 128;
pub const MAX_FAIL: usize = 1024;

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
        else if ch.is_alphabetic() || ch == '_' {
            let s = i; while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') { i += 1; }
            let id: String = c[s..i].iter().collect();
            if id.len() > MAX_NAME { return Err(format!("line {}: a name longer than {} bytes", line, MAX_NAME)); }
            out.push(Tok::Id(id));
        }
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
            if ["==", "!=", "<=", ">=", "&&", "||", "->", "..", "+=", "-=", "*=", "/="].contains(&two.as_str()) { out.push(Tok::Sym(two)); i += 2; }
            else { out.push(Tok::Sym(ch.to_string())); i += 1; }
        }
    }
    while at.len() < out.len() { at.push(tl); }
    Ok((out, at))
}

/// `pre`: statements an initializer (`coll.add() { f: v }`) puts before the statement it is in; `init_ok`: whether
/// an initializer may appear here (not in a condition, a loop's clauses or a JSON array, where it would run at a
/// different time); `tmp`: hidden locals made so far.
struct P { t: Vec<Tok>, i: usize, lines: Vec<u32>, pre: Vec<Stmt>, init_ok: bool, tmp: u32, in_endpoint: bool }
type R<T> = Result<T, String>;

pub fn parse(src: &str) -> R<Api> {
    let (t, lines) = lex_lines(src)?;
    let mut p = P { t, i: 0, lines, pre: Vec::new(), init_ok: true, tmp: 0, in_endpoint: false };
    let mut api = Api::default();
    while p.i < p.t.len() {
        let public = p.is_kw("pub");
        if public { p.i += 1; if !p.is_kw("fn") { return Err(format!("expected `fn` after `pub`, found {}", p.here())); } }
        if p.is_kw("fn") {
            p.i += 1;
            let mut name = p.id()?;
            while p.sym(".") { name = format!("{}.{}", name, p.id()?); } // instance.function, from module expansion
            p.eat("(")?;
            let mut params = Vec::new();
            while !p.sym(")") { params.push(p.id()?); if p.sym(":") { p.skip_type()?; } p.sym(","); }
            if p.sym("->") { p.skip_type()?; }
            let body = p.block()?;
            api.fns.push(Func { name, params, body, slots: 0, public });
            continue;
        }
        if p.is_kw("endpoint") { return Err("endpoints are functions with an attribute: `@post(\"/path\") fn name() -> Response<T> { ... }`".into()); }
        // `@get("/path") fn name() -> Response<T> { ... }`
        let first = p.lines.get(p.i).copied().unwrap_or(0);
        p.eat("@")?;
        let verb = p.id()?;
        let method = match verb.as_str() { "get" | "post" | "put" | "patch" | "delete" => verb.to_uppercase(), v => return Err(format!("`@{}`: an endpoint is `@get`, `@post`, `@put`, `@patch` or `@delete`", v)) };
        p.eat("(")?;
        let path = match p.t.get(p.i) { Some(Tok::Str(s)) => { p.i += 1; s.clone() } _ => return Err(format!("`@{}(...)` takes the path as a string, as in `@{}(\"/players/{{p}}\")`", verb, verb)) };
        p.eat(")")?;
        let segs = path_segs(&path)?;
        p.kw("fn")?;
        let name = p.id()?;
        p.eat("(")?; p.eat(")")?;
        if !(p.sym("->") && p.is_kw("Response")) { return Err(format!("endpoint `{}` returns a `Response<T>`: `fn {}() -> Response<T>`", name, name)); }
        p.i += 1; p.eat("<")?; let resp = p.rtype()?; p.eat(">")?;
        p.in_endpoint = true;
        let body = p.block();
        p.in_endpoint = false;
        let body = body?;
        let text = format!("{} {}", method, segs.iter().map(|s| match s { Seg::Lit(l) => format!("/{}", l), Seg::Param(n) => format!("/{{{}}}", n) }).collect::<String>());
        let last = p.lines.get(p.i.saturating_sub(1)).copied().unwrap_or(first);
        api.eps.push(Endpoint { method, segs, body, text, slots: 0, lines: (first, last), name, resp });
    }
    Ok(api)
}

/// `/rooms/{room}/messages` as segments.
fn path_segs(path: &str) -> R<Vec<Seg>> {
    let Some(rest) = path.strip_prefix('/') else { return Err(format!("an endpoint path starts with `/`: `{}`", path)) };
    let mut segs = Vec::new();
    for part in rest.split('/').filter(|x| !x.is_empty()) {
        let ok = |n: &str| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.');
        match part.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
            Some(n) if ok(n) => segs.push(Seg::Param(n.to_string())),
            None if ok(part) => segs.push(Seg::Lit(part.to_string())),
            _ => return Err(format!("endpoint path `{}`: `{}` is not a name or a `{{parameter}}`", path, part)),
        }
        if segs.len() > 32 { return Err("an endpoint path has at most 32 parts".into()); }
    }
    Ok(segs)
}

impl P {
    /// The `T` of `Response<T>`: `{ name: T, ... }`, `[T]`, `T?`, or a name (`i64`, `string<24>`, `Row`, `any`, `none`).
    fn rtype(&mut self) -> R<RType> {
        let t = if self.sym("{") {
            let mut f = Vec::new();
            while !self.sym("}") { let k = self.key()?; self.eat(":")?; f.push((k, self.rtype()?)); if !self.sym(",") { self.sym(";"); } }
            RType::Obj(f)
        } else if self.sym("[") { let e = self.rtype()?; self.eat("]")?; RType::Arr(Box::new(e)) }
        else {
            let n = self.id()?;
            if self.sym("<") { let mut depth = 1; while depth > 0 { if self.sym("<") { depth += 1; } else if self.sym(">") { depth -= 1; } else if self.peek().is_none() { return Err("unfinished type".into()); } else { self.i += 1; } } }
            RType::Name(n)
        };
        Ok(if self.sym("?") { RType::Opt(Box::new(t)) } else { t })
    }
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
        while !self.sym("}") {
            if self.peek().is_none() { return Err("missing `}`".into()); }
            let st = self.stmt()?;
            v.append(&mut self.pre);
            v.push(st);
        }
        Ok(v)
    }

    /// Parse with initializers refused (they would run at another time than they are written).
    fn no_init<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> { let was = self.init_ok; self.init_ok = false; let r = f(self); self.init_ok = was; r }

    fn iter(&mut self) -> R<Iter> { self.no_init(|p| p.iter_inner()) }
    fn iter_inner(&mut self) -> R<Iter> {
        let var = self.id()?; self.kw("in")?; let coll = self.expr()?;
        let hi = if self.sym("..") { Some(self.expr()?) } else { None };
        let from = if self.is_kw("from") { self.i += 1; Some(self.expr()?) } else { None };
        let reverse = if self.is_kw("reverse") { self.i += 1; true } else { false };
        let whil = if self.is_kw("while") { self.i += 1; Some(self.expr()?) } else { None };
        let wher = if self.is_kw("where") { self.i += 1; Some(self.expr()?) } else { None };
        if self.is_kw("order") { return Err("`order by` was removed: keep the data in order with a sorted collection (lib/sorted.mito)".into()); }
        let limit = if self.is_kw("limit") { self.i += 1; Some(self.expr()?) } else { None };
        Ok(Iter { var, slot: 0, coll, hi, from, reverse, whil, wher, limit, via: None })
    }

    fn stmt(&mut self) -> R<Stmt> {
        if self.is_kw("let") || self.is_kw("var") {
            let is_var = self.is_kw("var");
            self.i += 1; let n = self.id()?; self.eat("=")?;
            if self.is_kw("alloc") || self.is_kw("try_alloc") { return Err(format!("`{}` was replaced by collection functions: `let {} = coll.add();` (or `coll.try_add()`)", self.here_word(), n)); }
            let e = self.expr()?; self.eat(";")?;
            return Ok(if is_var { Stmt::Var(n, 0, e) } else { Stmt::Let(n, 0, e) });
        }
        if self.is_kw("if") { return self.if_stmt(); }
        if self.is_kw("each") { self.i += 1; let it = self.iter()?; let b = self.block()?; return Ok(Stmt::Each(it, b)); }
        if self.is_kw("free") { return Err("`free x` was replaced by the collection's function: `coll.remove(x);`".into()); }
        if self.is_kw("forward") {
            self.i += 1;
            let kind = self.id()?; self.eat("[")?; let key = self.expr()?; self.eat("]")?;
            let mut fields = Vec::new();
            if self.sym("{") {
                while !self.sym("}") {
                    let k = self.key()?;
                    self.eat(":")?; fields.push((k, self.expr()?)); self.sym(",");
                }
            }
            self.eat(";")?;
            return Ok(Stmt::Forward { kind, key, fields });
        }
        if self.is_kw("return") {
            self.i += 1;
            // in an endpoint, the answer: ok(...) 200, created(...) 201, no_content() 204, status(code, ...)
            if self.in_endpoint {
                let how = self.here_word();
                let code: u16 = match how.as_str() { "ok" => 200, "created" => 201, "no_content" => 204, "status" => 0, _ => return Err("an endpoint returns its answer: `return ok(...)`, `created(...)`, `no_content()` or `status(code, ...)`".into()) };
                self.i += 1; self.eat("(")?;
                let code = if code == 0 {
                    let c = self.int()?;
                    if !(200..300).contains(&c) { return Err(format!("`status({}, ...)`: an answer is a success (2xx); end with `fail {} \"...\"` for an error", c, c)); }
                    self.eat(",")?; c as u16
                } else { code };
                let j = if code == 204 { Json::Empty } else { self.json()? };
                self.eat(")")?; self.eat(";")?;
                return Ok(Stmt::Respond(code, j));
            }
            if self.sym(";") { return Ok(Stmt::Return(None)); }
            let e = self.expr()?; self.eat(";")?; return Ok(Stmt::Return(Some(e)));
        }
        if self.is_kw("fail") {
            self.i += 1; let code = self.int()? as u16;
            let msg = match self.t.get(self.i) { Some(Tok::Str(s)) => { self.i += 1; s.clone() } _ => return Err("fail needs a message string".into()) };
            if msg.len() > MAX_FAIL { return Err(format!("a fail message longer than {} bytes", MAX_FAIL)); }
            self.eat(";")?; return Ok(Stmt::Fail(code, msg));
        }
        if self.is_kw("respond") { return Err("`respond` was replaced by returning the answer: `return ok(...);` (or `created`, `no_content`, `status`)".into()); }
        let lhs = self.expr()?;
        if self.sym(";") { return Ok(Stmt::Do(lhs)); }
        // `x += e` is `x = x + e`
        for op in ["+=", "-=", "*=", "/="] {
            if self.sym(op) { let rhs = self.expr()?; self.eat(";")?; return Ok(Stmt::Assign(lhs.clone(), Expr::Bin(op[..1].to_string(), Box::new(lhs), Box::new(rhs)))); }
        }
        self.eat("=")?;
        let rhs = self.expr()?; self.eat(";")?;
        Ok(Stmt::Assign(lhs, rhs))
    }

    fn if_stmt(&mut self) -> R<Stmt> {
        self.kw("if")?; let c = self.no_init(|p| p.expr())?; let t = self.block()?;
        let e = if self.is_kw("else") { self.i += 1; if self.is_kw("if") { vec![self.if_stmt()?] } else { self.block()? } } else { vec![] };
        Ok(Stmt::If(c, t, e))
    }

    /// A key of a JSON object or a forwarded field: a name, or a string for one that is not a name.
    fn key(&mut self) -> R<String> {
        match self.t.get(self.i) { Some(Tok::Str(s)) | Some(Tok::Id(s)) => { let s = s.clone(); self.i += 1; Ok(s) } _ => Err(format!("expected a key, found {}", self.here())) }
    }
    fn here_word(&self) -> String { match self.peek() { Some(Tok::Id(x)) => x.clone(), _ => self.here() } }

    fn json(&mut self) -> R<Json> {
        if self.sym("{") {
            let mut f = Vec::new();
            while !self.sym("}") {
                let k = self.key()?;
                self.eat(":")?; f.push((k, self.json()?)); self.sym(",");
            }
            return Ok(Json::Obj(f));
        }
        if self.sym("[") { self.kw("each")?; let it = self.iter()?; let v = self.no_init(|p| p.json())?; self.eat("]")?; return Ok(Json::Arr(it, Box::new(v))); }
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
            if self.sym(".") {
                let f = self.id()?;
                // `game.board.add(args)`: named by the receiver's path (a module instance's function, or the binder makes it
                // a collection operation); on any other value, `.f(value, args)`
                // `coll.add { ... }`: an initializer may follow `add` without parentheses
                let bare_init = (f == "add" || f == "try_add") && self.init_ok && matches!(self.peek(), Some(Tok::Sym(s)) if s == "{");
                if bare_init || matches!(self.peek(), Some(Tok::Sym(s)) if s == "(") {
                    let mut a = Vec::new();
                    if !bare_init { self.i += 1; while !self.sym(")") { a.push(self.expr()?); self.sym(","); } }
                    e = match path_of(&e) { Some(n) => Expr::Call(format!("{}.{}", n, f), a, CallRes::Unbound), None => { a.insert(0, e); Expr::Call(format!(".{}", f), a, CallRes::Unbound) } };
                    // `coll.add() { field: value, ... }`: made into a hidden local and assignments before this statement
                    if self.init_ok && matches!(self.peek(), Some(Tok::Sym(s)) if s == "{") {
                        self.i += 1;
                        let t = format!("__mito__new{}", self.tmp); self.tmp += 1;
                        let mut sets = Vec::new();
                        while !self.sym("}") { let k = self.id()?; self.eat(":")?; let v = self.expr()?; sets.push(Stmt::Assign(Expr::Field(Box::new(Expr::Var(t.clone(), VarRes::Unbound)), k), v)); self.sym(","); }
                        self.pre.push(Stmt::Let(t.clone(), 0, e));
                        self.pre.extend(sets);
                        e = Expr::Var(t, VarRes::Unbound);
                    }
                    continue;
                }
                e = Expr::Field(Box::new(e), f);
            }
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

/// `a.b.c` when `e` is a name followed by fields, else None.
pub fn path_of(e: &Expr) -> Option<String> {
    match e { Expr::Var(n, _) => Some(n.clone()), Expr::Field(b, f) => path_of(b).map(|p| format!("{}.{}", p, f)), _ => None }
}

/// `owner`: the module instance whose function is being bound (its private functions may be called from it).
struct Binder<'g> { names: Vec<(String, u32, bool)>, next: u32, max: u32, globals: &'g dyn Fn(&str) -> Option<usize>, fns: Vec<(String, usize, bool)>, place: String, owner: Option<String>, hidden: u32 }

/// Resolve every name in `api`. `globals` answers whether a name is a declared global (and which).
pub fn bind(api: &mut Api, globals: &dyn Fn(&str) -> Option<usize>) -> R<()> {
    let fns: Vec<(String, usize, bool)> = api.fns.iter().map(|f| (f.name.clone(), f.params.len(), f.public)).collect();
    for (k, f) in fns.iter().enumerate() { if fns[..k].iter().any(|g| g.0 == f.0) { return Err(format!("fn `{}` is declared twice", f.0)); } if B::from(&f.0).is_some() { return Err(format!("fn `{}` has the name of a builtin", f.0)); } }
    for f in &mut api.fns {
        let mut b = Binder { names: Vec::new(), next: 0, max: 0, globals, fns: fns.clone(), place: format!("fn {}", f.name), owner: f.name.rfind('.').map(|i| f.name[..i].to_string()), hidden: 0 };
        for p in &f.params { b.declare(p, false); }
        b.block(&mut f.body)?;
        f.slots = b.max;
    }
    for e in &mut api.eps {
        let mut b = Binder { names: Vec::new(), next: 0, max: 0, globals, fns: fns.clone(), place: format!("endpoint {}", e.text), owner: None, hidden: 0 };
        b.block(&mut e.body)?;
        e.slots = b.max;
        if !answers(&e.body) { return Err(format!("endpoint {} ({}) can finish without an answer: end every path with `return ok(...)` (or created, no_content, status), `fail` or `forward`", e.name, e.text)); }
    }
    Ok(())
}

/// Does every path through `body` end in an answer, a `fail`, a `forward` (or the test hook `__crash()`)?
fn answers(body: &[Stmt]) -> bool {
    match body.last() {
        Some(Stmt::Respond(..) | Stmt::Fail(..) | Stmt::Forward { .. }) => true,
        Some(Stmt::Do(Expr::Call(n, _, _))) => n == "__crash",
        Some(Stmt::If(_, t, e)) => answers(t) && answers(e),
        _ => false,
    }
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
        if let Some(h) = &mut it.hi { self.expr(h)?; }
        // a loop over a module instance walks it with the instance's iteration functions
        if let Some(n) = path_of(&it.coll) {
            let f = |x: &str| self.fns.iter().position(|g| g.0 == format!("{}.{}", n, x)).map(|k| k as u32);
            if let (Some(first), Some(next), Some(item)) = (f("iter_first"), f("iter_next"), f("iter_item")) {
                if it.reverse && (f("iter_last").is_none() || f("iter_prev").is_none()) { return self.err(format!("{} cannot be walked in reverse", n)); }
                it.via = Some(Via { first, next, item, last: f("iter_last"), prev: f("iter_prev") });
            }
        }
        if let Some(fr) = &mut it.from { if it.via.is_none() { return self.err(format!("`each {} in ... from`: only a module instance can start a loop from a key", it.var)); } self.expr(fr)?; }
        if let Some(l) = &mut it.limit { self.expr(l)?; }
        it.slot = self.declare(&it.var, false);
        if let Some(w) = &mut it.whil { if it.hi.is_none() && it.via.is_none() { return self.err(format!("`each {} in ... while`: only ranges and module instances stop early; a list or pool is walked whole", it.var)); } self.expr(w)?; }
        if let Some(w) = &mut it.wher { self.expr(w)?; }
        Ok(())
    }
    /// `x.f(args)` naming no function of a module instance: a collection operation, `.f(x, args)`.
    fn operation(&self, e: &mut Expr) {
        let Expr::Call(n, args, _) = e else { return };
        if n.starts_with('.') || self.fns.iter().any(|f| f.0 == *n) { return; }
        let Some((recv, op)) = n.rsplit_once('.') else { return };
        if !matches!(op, "add" | "try_add" | "remove" | "count" | "capacity") { return; }
        let mut parts = recv.split('.');
        let mut r = Expr::Var(parts.next().unwrap_or_default().to_string(), VarRes::Unbound);
        for f in parts { r = Expr::Field(Box::new(r), f.to_string()); }
        args.insert(0, r);
        *n = format!(".{}", op);
    }
    fn stmt(&mut self, s: &mut Stmt) -> R<()> {
        // collection operations that are statements: `let p = coll.add();`, `coll.add();`, `coll.remove(p);`
        match s {
            Stmt::Let(_, _, e) | Stmt::Do(e) => self.operation(e),
            _ => {}
        }
        let lowered = match s {
            Stmt::Let(n, _, Expr::Call(op, args, _)) if (op == ".add" || op == ".try_add") && args.len() == 1 => Some(Stmt::Alloc { var: n.clone(), slot: 0, coll: args.remove(0), try_: op == ".try_add" }),
            Stmt::Do(Expr::Call(op, args, _)) if (op == ".add" || op == ".try_add") && args.len() == 1 => { self.hidden += 1; Some(Stmt::Alloc { var: format!("__mito__added{}", self.hidden), slot: 0, coll: args.remove(0), try_: op == ".try_add" }) }
            Stmt::Do(Expr::Call(op, args, _)) if op == ".remove" && args.len() == 2 => { let item = args.remove(1); Some(Stmt::Remove(args.remove(0), item)) }
            _ => None,
        };
        if let Some(l) = lowered { *s = l; }
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
            Stmt::Remove(c, e) => { self.expr(c)?; self.expr(e)?; }
            Stmt::Do(e) => self.expr(e)?,
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
            Json::Empty => Ok(()),
        }
    }
    fn expr(&mut self, e: &mut Expr) -> R<()> {
        // in a module instance's function, `self` is the instance, written as its path (`game.board`); never a local,
        // so nothing shadows it
        if matches!(e, Expr::Var(n, _) if n == "self") {
            let Some(o) = self.owner.clone() else { return self.err("`self` is only for module code".into()) };
            let mut parts = o.split('.');
            let mut p = Expr::Var(parts.next().unwrap_or_default().to_string(), VarRes::Unbound);
            for f in parts { p = Expr::Field(Box::new(p), f.to_string()); }
            *e = p;
        }
        self.operation(e);
        match e {
            Expr::Var(n, res) => {
                *res = match self.names.iter().rev().find(|x| x.0 == *n) {
                    Some(x) => VarRes::Local(x.1),
                    None => match (self.globals)(n) { Some(g) => VarRes::Global(g as u32), None => return self.err(format!("unknown name `{}`", n)) },
                };
            }
            Expr::Field(b, _) => self.expr(b)?,
            Expr::Index(b, i) => { self.expr(b)?; self.expr(i)?; }
            Expr::Un(_, x) => self.expr(x)?,
            Expr::Bin(_, a, b) => { self.expr(a)?; self.expr(b)?; }
            Expr::Call(n, ..) if n.starts_with('.') => {
                let Expr::Call(n, args, res) = e else { unreachable!() };
                for a in args.iter_mut() { self.expr(a)?; }
                *res = match (n.as_str(), args.len()) {
                    (".count", 1) => CallRes::Builtin(B::Count),
                    (".capacity", 1) => CallRes::Builtin(B::Capacity),
                    (".add" | ".try_add", 1) => return self.err(format!("`{}()` makes an item: write `let x = coll{}();` or `coll{}();` as a statement", &n[1..], n, n)),
                    (".remove", 2) => return self.err("`remove(item)` is a statement: `coll.remove(item);`".into()),
                    (".count" | ".capacity" | ".add" | ".try_add", k) => return self.err(format!("`{}()` takes no arguments, given {}", &n[1..], k - 1)),
                    (".remove", k) => return self.err(format!("`remove` takes one item, given {}", k - 1)),
                    _ => return self.err(format!("unknown function `{}`", &n[1..])),
                };
            }
            Expr::Call(n, args, res) => {
                if n == "count" || n == "capacity" { return self.err(format!("`{}(x)` was replaced by `x.{}()`", n, n)); }
                for a in args.iter_mut() { self.expr(a)?; }
                if let (Some(rest), Some(o)) = (n.strip_prefix("self."), self.owner.as_ref()) { *n = format!("{}.{}", o, rest); }
                *res = if let Some(k) = self.fns.iter().position(|f| f.0 == *n) {
                    if self.fns[k].1 != args.len() { return self.err(format!("`{}` takes {} arguments, given {}", n, self.fns[k].1, args.len())); }
                    if let Some(i) = n.rfind('.') { if !self.fns[k].2 && self.owner.as_deref() != Some(&n[..i]) { return self.err(format!("`{}` is private to {}", n, &n[..i])); } }
                    CallRes::User(k as u32)
                } else if let Some(b) = B::from(n) { CallRes::Builtin(b) }
                else { return self.err(format!("unknown function `{}`", n)); };
            }
            _ => {}
        }
        Ok(())
    }
}
