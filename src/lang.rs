//! The endpoint language: tokens, syntax tree and parser.
//! A file holds `fn` declarations and `endpoint` handlers. Types written in `fn` signatures are documentation for now.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok { Id(String), Int(i64), Flt(f64), Str(String), Sym(String) }

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64), Float(f64), Str(String), Bool(bool), Null,
    Var(String),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Un(char, Box<Expr>),
    Bin(String, Box<Expr>, Box<Expr>),
}

/// `each m in room.messages reverse where ... order by ... desc limit n`
#[derive(Debug, Clone)]
pub struct Iter { pub var: String, pub coll: Expr, pub reverse: bool, pub wher: Option<Expr>, pub order: Option<(Expr, bool)>, pub limit: Option<Expr> }

#[derive(Debug, Clone)]
pub enum Json { Obj(Vec<(String, Json)>), Arr(Iter, Box<Json>), E(Expr) }

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(String, Expr),
    Var(String, Expr),
    Alloc { var: String, coll: Expr, try_: bool },
    Assign(Expr, Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    Each(Iter, Vec<Stmt>),
    Free(Expr),
    Fail(u16, String),
    Respond(u16, Json),
    Return(Option<Expr>),
    Do(Expr),
}

#[derive(Debug, Clone)]
pub enum Seg { Lit(String), Param(String) }
#[derive(Debug, Clone)]
pub struct Endpoint { pub method: String, pub segs: Vec<Seg>, pub body: Vec<Stmt>, pub text: String }
#[derive(Debug, Clone)]
pub struct Func { pub name: String, pub params: Vec<String>, pub body: Vec<Stmt> }
#[derive(Debug, Clone, Default)]
pub struct Api { pub eps: Vec<Endpoint>, pub fns: Vec<Func> }

pub fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = src.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    while i < c.len() {
        let ch = c[i];
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
            i += 1; out.push(Tok::Str(s));
        }
        else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            if ["==", "!=", "<=", ">=", "&&", "||", "->"].contains(&two.as_str()) { out.push(Tok::Sym(two)); i += 2; }
            else { out.push(Tok::Sym(ch.to_string())); i += 1; }
        }
    }
    Ok(out)
}

struct P { t: Vec<Tok>, i: usize }
type R<T> = Result<T, String>;

pub fn parse(src: &str) -> R<Api> {
    let mut p = P { t: lex(src)?, i: 0 };
    let mut api = Api::default();
    while p.i < p.t.len() {
        if p.is_kw("fn") {
            p.i += 1;
            let name = p.id()?; p.eat("(")?;
            let mut params = Vec::new();
            while !p.sym(")") { params.push(p.id()?); if p.sym(":") { p.skip_type()?; } p.sym(","); }
            if p.sym("->") { p.skip_type()?; }
            let body = p.block()?;
            api.fns.push(Func { name, params, body });
            continue;
        }
        p.kw("endpoint")?;
        let method = p.id()?;
        let mut segs = Vec::new();
        while p.sym("/") {
            if p.sym("{") { let n = p.id()?; p.eat("}")?; segs.push(Seg::Param(n)); } else { segs.push(Seg::Lit(p.id()?)); }
        }
        let body = p.block()?;
        let text = format!("{} {}", method, segs.iter().map(|s| match s { Seg::Lit(l) => format!("/{}", l), Seg::Param(n) => format!("/{{{}}}", n) }).collect::<String>());
        api.eps.push(Endpoint { method, segs, body, text });
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
        Ok(Iter { var, coll, reverse, wher, order, limit })
    }

    fn stmt(&mut self) -> R<Stmt> {
        if self.is_kw("let") || self.is_kw("var") {
            let is_var = self.is_kw("var");
            self.i += 1; let n = self.id()?; self.eat("=")?;
            if !is_var && (self.is_kw("alloc") || self.is_kw("try_alloc")) {
                let try_ = self.is_kw("try_alloc"); self.i += 1; let coll = self.postfix()?; self.eat(";")?;
                return Ok(Stmt::Alloc { var: n, coll, try_ });
            }
            let e = self.expr()?; self.eat(";")?;
            return Ok(if is_var { Stmt::Var(n, e) } else { Stmt::Let(n, e) });
        }
        if self.is_kw("if") { return self.if_stmt(); }
        if self.is_kw("each") { self.i += 1; let it = self.iter()?; let b = self.block()?; return Ok(Stmt::Each(it, b)); }
        if self.is_kw("free") { self.i += 1; let e = self.expr()?; self.eat(";")?; return Ok(Stmt::Free(e)); }
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
            if self.sym(".") { e = Expr::Field(Box::new(e), self.id()?); }
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
                    return Ok(Expr::Call(n, a));
                }
                Ok(Expr::Var(n))
            }
            _ => Err(format!("unexpected {}", self.here())),
        }
    }
}
