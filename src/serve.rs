//! Runs endpoints against declared state: an interpreter, a small HTTP/1.1 server, and an in-process benchmark.
use crate::lang::*;
use crate::os;
use crate::region::{Scratch, Space};
use crate::runtime::{Error, Instance, Val};
use std::io::{Read, Write};
use std::time::Instant;

#[derive(Clone, Debug)]
enum Value { Null, Int(i64), Float(f64), Bool(bool), Str(String), Row(usize, usize), Coll(usize), Path(String) }

enum Ctl { Done(u16), Fail(u16, String), Err(Error) }
impl From<Error> for Ctl { fn from(e: Error) -> Ctl { Ctl::Err(e) } }
type R<T> = Result<T, Ctl>;
fn fail<T>(code: u16, m: impl Into<String>) -> R<T> { Err(Ctl::Fail(code, m.into())) }

/// Response bytes live in a fixed block inside the declared reservation, reused by every request.
struct Buf { ptr: *mut u8, cap: usize, len: usize }
impl Buf {
    fn push(&mut self, s: &[u8]) -> bool {
        if self.len + s.len() > self.cap { return false; }
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), self.ptr.add(self.len), s.len()) };
        self.len += s.len(); true
    }
    fn text(&self) -> String { String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(self.ptr, self.len) }).into_owned() }
}

struct Req { params: Vec<(String, String)>, query: Vec<(String, String)>, headers: Vec<(String, String)>, json: Vec<(String, Value)> }

pub struct Server {
    pub inst: Instance,
    pub eps: Vec<Endpoint>,
    buf: Buf,
    pub clock: Option<i64>, // fixed time in ms, for tests and benchmarks; otherwise the system clock
    _scratch: Scratch,
    _space: Space,
}

pub const RESPONSE_CAP: usize = 256 * 1024;

impl Server {
    pub fn new(inst: Instance, api_src: &str) -> Result<Server, String> {
        let eps = parse(api_src)?;
        let mut space = Space::new(2 * RESPONSE_CAP);
        let region = space.region(RESPONSE_CAP + 4096, 64 * 1024).map_err(|_| "response region".to_string())?;
        let mut scratch = Scratch::new(region);
        let ptr = scratch.alloc(RESPONSE_CAP).map_err(|_| "response buffer".to_string())?;
        Ok(Server { inst, eps, buf: Buf { ptr, cap: RESPONSE_CAP, len: 0 }, clock: None, _scratch: scratch, _space: space })
    }

    /// Handle one request. `target` is the path plus optional `?query`.
    pub fn handle(&mut self, method: &str, target: &str, headers: &[(String, String)], body: &str) -> (u16, String) {
        let (path, qs) = target.split_once('?').unwrap_or((target, ""));
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut hit = None;
        'eps: for (i, e) in self.eps.iter().enumerate() {
            if e.method != method || e.segs.len() != parts.len() { continue; }
            let mut params = Vec::new();
            for (s, p) in e.segs.iter().zip(&parts) {
                match s { Seg::Lit(l) => if l != p { continue 'eps; }, Seg::Param(n) => params.push((n.clone(), p.to_string())) }
            }
            hit = Some((i, params)); break;
        }
        let Some((ei, params)) = hit else { return (404, "{\"error\":\"no such endpoint\"}".into()) };
        let query = qs.split('&').filter(|s| !s.is_empty()).map(|kv| { let (k, v) = kv.split_once('=').unwrap_or((kv, "")); (k.to_string(), v.to_string()) }).collect();
        let req = Req { params, query, headers: headers.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect(), json: parse_json(body) };
        self.buf.len = 0; // the per-request epoch starts empty
        let now = self.clock.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0));
        let Server { inst, eps, buf, .. } = self;
        let mut run = Run { inst, req: &req, vars: Vec::new(), buf, now, undo: Vec::new() };
        let res = run.exec(&eps[ei].body);
        if !matches!(res, Ok(()) | Err(Ctl::Done(_))) { run.rollback(); }
        match res {
            Ok(()) => (204, String::new()),
            Err(Ctl::Done(code)) => (code, run.buf.text()),
            Err(Ctl::Fail(code, m)) => (code, format!("{{\"error\":{}}}", quote(&m))),
            Err(Ctl::Err(Error::Full(m))) => (503, format!("{{\"error\":{}}}", quote(&format!("capacity: {}", m)))),
            Err(Ctl::Err(Error::TooLong(m))) => (413, format!("{{\"error\":{}}}", quote(&m))),
            Err(Ctl::Err(Error::Bad(m))) => (if m.contains("not live") { 404 } else { 400 }, format!("{{\"error\":{}}}", quote(&m))),
        }
    }
}

/// What a failed request must put back: writes are restored and allocations released. Frees are not undone.
enum Undo { Row(usize, usize, String, Val), Path(String, Val), Alloc(usize, usize) }

struct Run<'a> { inst: &'a mut Instance, req: &'a Req, vars: Vec<(&'a str, Value)>, buf: &'a mut Buf, now: i64, undo: Vec<Undo> }

fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() { match c { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), '\n' => o.push_str("\\n"), '\r' => o.push_str("\\r"), '\t' => o.push_str("\\t"), c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)), c => o.push(c) } }
    o.push('"'); o
}

/// Flat JSON object of strings, numbers, booleans and null. Nested values are ignored.
fn parse_json(s: &str) -> Vec<(String, Value)> {
    let b: Vec<char> = s.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    let ws = |i: &mut usize| while *i < b.len() && b[*i].is_whitespace() { *i += 1; };
    ws(&mut i);
    if b.get(i) != Some(&'{') { return out; }
    i += 1;
    let string = |i: &mut usize| -> Option<String> {
        if b.get(*i) != Some(&'"') { return None; }
        *i += 1; let mut o = String::new();
        while *i < b.len() && b[*i] != '"' {
            if b[*i] == '\\' && *i + 1 < b.len() { *i += 1; o.push(match b[*i] { 'n' => '\n', 't' => '\t', 'r' => '\r', c => c }); } else { o.push(b[*i]); }
            *i += 1;
        }
        *i += 1; Some(o)
    };
    loop {
        ws(&mut i);
        let Some(k) = string(&mut i) else { break };
        ws(&mut i); if b.get(i) != Some(&':') { break; } i += 1; ws(&mut i);
        let v = match b.get(i) {
            Some('"') => match string(&mut i) { Some(s) => Value::Str(s), None => break },
            Some(c) if c.is_ascii_digit() || *c == '-' => { let st = i; i += 1; while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') { i += 1; } let t: String = b[st..i].iter().collect(); match t.parse::<i64>() { Ok(n) => Value::Int(n), Err(_) => t.parse::<f64>().map(Value::Float).unwrap_or(Value::Null) } }
            Some('t') => { i += 4; Value::Bool(true) }
            Some('f') => { i += 5; Value::Bool(false) }
            Some('n') => { i += 4; Value::Null }
            _ => break,
        };
        out.push((k, v));
        ws(&mut i);
        if b.get(i) == Some(&',') { i += 1; } else { break; }
    }
    out
}

fn val_of(v: Val) -> Value { match v { Val::Int(i) => Value::Int(i), Val::Float(f) => Value::Float(f), Val::Bool(b) => Value::Bool(b), Val::Str(s) => Value::Str(s) } }

impl<'a> Run<'a> {
    fn rollback(&mut self) {
        while let Some(u) = self.undo.pop() {
            match u {
                Undo::Row(g, slot, f, old) => { if self.inst.row_live(g, slot) { let _ = self.inst.row_set(g, slot, &f, &old); } }
                Undo::Path(p, old) => { let _ = self.inst.path_set(&p, &old); }
                Undo::Alloc(g, slot) => { let _ = self.inst.free_row(g, slot); }
            }
        }
    }
    fn lookup(&self, n: &str) -> Option<Value> { self.vars.iter().rev().find(|v| v.0 == n).map(|v| v.1.clone()) }

    /// Resolve structure paths (global struct fields) to a scalar or string when one is needed.
    fn prim(&self, v: Value) -> R<Value> {
        match v { Value::Path(p) => match self.inst.path_get(&p) { Ok(x) => Ok(val_of(x)), Err(e) => Err(e.into()) }, o => Ok(o) }
    }
    fn to_val(&self, v: Value) -> R<Val> {
        Ok(match self.prim(v)? { Value::Int(i) => Val::Int(i), Value::Float(f) => Val::Float(f), Value::Bool(b) => Val::Bool(b), Value::Str(s) => Val::Str(s), _ => return fail(500, "cannot store this value in state") })
    }
    fn truthy(&self, v: Value) -> R<bool> {
        Ok(match self.prim(v)? { Value::Bool(b) => b, Value::Null => false, Value::Int(i) => i != 0, _ => return fail(500, "condition is not a boolean") })
    }
    fn num(&self, v: &Value) -> Option<f64> { match v { Value::Int(i) => Some(*i as f64), Value::Float(f) => Some(*f), _ => None } }

    fn eval(&mut self, e: &Expr) -> R<Value> {
        match e {
            Expr::Int(n) => Ok(Value::Int(*n)),
            Expr::Float(f) => Ok(Value::Float(*f)),
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            Expr::Var(n) => {
                if let Some(v) = self.lookup(n) { return Ok(v); }
                match self.inst.gindex(n) {
                    Some(g) if self.inst.coll_kind(g).is_some() => Ok(Value::Coll(g)),
                    Some(_) => Ok(Value::Path(n.clone())),
                    None => fail(500, format!("unknown name `{}`", n)),
                }
            }
            Expr::Field(b, f) => match self.eval(b)? {
                Value::Row(g, s) => Ok(val_of(self.inst.row_get(g, s, f)?)),
                Value::Path(p) => {
                    let np = format!("{}.{}", p, f);
                    match self.inst.path_get(&np) { Ok(v) => Ok(val_of(v)), Err(Error::Bad(m)) if m.contains("not a scalar") => Ok(Value::Path(np)), Err(e) => Err(e.into()) }
                }
                _ => fail(500, format!("`.{}` needs a row or a state struct", f)),
            },
            Expr::Index(b, i) => {
                let (bv, iv) = (self.eval(b)?, self.eval(i)?);
                match (bv, iv) {
                    (Value::Coll(g), Value::Int(k)) if k >= 0 => if self.inst.row_live(g, k as usize) { Ok(Value::Row(g, k as usize)) } else { Err(Error::Bad(format!("slot {} is not live", k)).into()) },
                    _ => fail(500, "bad index"),
                }
            }
            Expr::Un(op, x) => {
                let v = self.eval(x)?;
                if *op == '!' { return Ok(Value::Bool(!self.truthy(v)?)); }
                match self.prim(v)? { Value::Int(i) => Ok(Value::Int(-i)), Value::Float(f) => Ok(Value::Float(-f)), _ => fail(500, "cannot negate") }
            }
            Expr::Bin(op, a, b) => {
                if op == "&&" { let l = self.eval(a)?; return if !self.truthy(l)? { Ok(Value::Bool(false)) } else { let r = self.eval(b)?; Ok(Value::Bool(self.truthy(r)?)) }; }
                if op == "||" { let l = self.eval(a)?; return if self.truthy(l)? { Ok(Value::Bool(true)) } else { let r = self.eval(b)?; Ok(Value::Bool(self.truthy(r)?)) }; }
                let (l, r) = (self.eval(a)?, self.eval(b)?);
                let (l, r) = (self.prim(l)?, self.prim(r)?);
                self.binop(op, l, r)
            }
            Expr::Call(n, args) => { let mut a = Vec::with_capacity(args.len()); for x in args { a.push(self.eval(x)?); } self.call(n, a) }
        }
    }

    fn binop(&self, op: &str, l: Value, r: Value) -> R<Value> {
        if op == "==" || op == "!=" {
            let eq = match (&l, &r) {
                (Value::Null, Value::Null) => true, (Value::Null, _) | (_, Value::Null) => false,
                (Value::Str(a), Value::Str(b)) => a == b, (Value::Bool(a), Value::Bool(b)) => a == b,
                (Value::Row(a, b), Value::Row(c, d)) => a == c && b == d,
                _ => match (self.num(&l), self.num(&r)) { (Some(a), Some(b)) => a == b, _ => return fail(500, "cannot compare these values") },
            };
            return Ok(Value::Bool(eq == (op == "==")));
        }
        if let (Value::Int(a), Value::Int(b)) = (&l, &r) {
            let ov = || fail::<Value>(500, "integer overflow");
            return match op {
                "+" => a.checked_add(*b).map(Value::Int).map_or_else(ov, Ok), "-" => a.checked_sub(*b).map(Value::Int).map_or_else(ov, Ok),
                "*" => a.checked_mul(*b).map(Value::Int).map_or_else(ov, Ok),
                "/" => if *b == 0 { fail(500, "division by zero") } else { Ok(Value::Int(a / b)) },
                "%" => if *b == 0 { fail(500, "division by zero") } else { Ok(Value::Int(a % b)) },
                "<" => Ok(Value::Bool(a < b)), "<=" => Ok(Value::Bool(a <= b)), ">" => Ok(Value::Bool(a > b)), ">=" => Ok(Value::Bool(a >= b)),
                _ => fail(500, "bad operator"),
            };
        }
        if op == "+" { if let Value::Str(a) = &l { return Ok(Value::Str(format!("{}{}", a, self.show(&r)?))); } }
        if let (Some(a), Some(b)) = (self.num(&l), self.num(&r)) {
            return Ok(match op { "+" => Value::Float(a + b), "-" => Value::Float(a - b), "*" => Value::Float(a * b), "/" => Value::Float(a / b),
                "<" => Value::Bool(a < b), "<=" => Value::Bool(a <= b), ">" => Value::Bool(a > b), ">=" => Value::Bool(a >= b), _ => return fail(500, "bad operator") });
        }
        if let (Value::Str(a), Value::Str(b)) = (&l, &r) { return Ok(Value::Bool(match op { "<" => a < b, "<=" => a <= b, ">" => a > b, ">=" => a >= b, _ => return fail(500, "bad operator") })); }
        fail(500, "operator does not fit these values")
    }

    fn show(&self, v: &Value) -> R<String> {
        Ok(match v { Value::Int(i) => i.to_string(), Value::Float(f) => f.to_string(), Value::Bool(b) => b.to_string(), Value::Str(s) => s.clone(), Value::Null => "null".into(), _ => return fail(500, "cannot show this value") })
    }

    fn call(&mut self, n: &str, a: Vec<Value>) -> R<Value> {
        let arg = |i: usize| a.get(i).cloned().unwrap_or(Value::Null);
        Ok(match n {
            "body" => { let Value::Str(k) = arg(0) else { return fail(500, "body(name)") }; self.req.json.iter().find(|x| x.0 == k).map(|x| x.1.clone()).unwrap_or(Value::Null) }
            "param" => { let Value::Str(k) = arg(0) else { return fail(500, "param(name)") }; self.req.params.iter().find(|x| x.0 == k).map(|x| Value::Str(x.1.clone())).unwrap_or(Value::Null) }
            "query" => { let Value::Str(k) = arg(0) else { return fail(500, "query(name)") }; self.req.query.iter().find(|x| x.0 == k).map(|x| Value::Str(x.1.clone())).unwrap_or(Value::Null) }
            "header" => { let Value::Str(k) = arg(0) else { return fail(500, "header(name)") }; let k = k.to_lowercase(); self.req.headers.iter().find(|x| x.0 == k).map(|x| Value::Str(x.1.clone())).unwrap_or(Value::Null) }
            "int" => match self.prim(arg(0))? {
                Value::Int(i) => Value::Int(i), Value::Float(f) => Value::Int(f as i64),
                Value::Str(s) => match s.trim().parse::<i64>() { Ok(i) => Value::Int(i), Err(_) => return fail(400, format!("`{}` is not a number", s)) },
                Value::Null => return fail(400, "missing number"), _ => return fail(500, "int(x)"),
            },
            "str" => Value::Str(self.show(&self.prim(arg(0))?)?),
            "len" => match self.prim(arg(0))? { Value::Str(s) => Value::Int(s.len() as i64), Value::Null => Value::Int(0), _ => return fail(500, "len(text)") },
            "now" => Value::Int(self.now),
            "random" => Value::Int(os::random_u64() as i64),
            "min" | "max" => {
                let (x, y) = (self.prim(arg(0))?, self.prim(arg(1))?);
                match (&x, &y) { (Value::Int(p), Value::Int(q)) => Value::Int(if n == "min" { *p.min(q) } else { *p.max(q) }), _ => return fail(500, "min/max need integers") }
            }
            "count" => match arg(0) { Value::Coll(g) => Value::Int(self.inst.count(g) as i64), _ => return fail(500, "count(collection)") },
            "or" => match arg(0) { Value::Null => arg(1), v => v },
            "capacity" => match arg(0) { Value::Coll(g) => Value::Int(self.inst.capacity(g) as i64), _ => return fail(500, "capacity(collection)") },
            "slot" => match arg(0) { Value::Row(_, s) => Value::Int(s as i64), _ => return fail(500, "slot(row)") },
            "stat" => {
                let Value::Str(k) = arg(0) else { return fail(500, "stat(name)") };
                Value::Int(match k.as_str() { "committed_kb" => self.inst.committed() / 1024, "live_kb" => self.inst.live_total() / 1024, "reserved_kb" => self.inst.reserved() / 1024, _ => return fail(500, "unknown stat") } as i64)
            }
            "token" => match arg(0) {
                Value::Row(g, s) => {
                    let gen = self.inst.row_gen(g, s).unwrap_or(0);
                    let Val::Int(sec) = self.inst.row_get(g, s, "secret")? else { return fail(500, "token needs an int `secret` field") };
                    Value::Str(format!("{}.{}.{:x}", s, gen, sec as u64))
                }
                _ => return fail(500, "token(row)"),
            },
            "auth" => {
                let Value::Coll(g) = arg(0) else { return fail(500, "auth(collection)") };
                let Some(tok) = self.req.headers.iter().find(|x| x.0 == "x-session").map(|x| x.1.clone()) else { return fail(401, "missing X-Session header") };
                let mut it = tok.split('.');
                let (Some(Ok(slot)), Some(Ok(gen)), Some(Ok(sec))) = (it.next().map(str::parse::<usize>), it.next().map(str::parse::<u32>), it.next().map(|h| u64::from_str_radix(h, 16))) else { return fail(401, "malformed session token") };
                if !self.inst.row_live(g, slot) || self.inst.row_gen(g, slot).map_or(false, |cur| cur != gen) { return fail(401, "session ended"); }
                let Val::Int(real) = self.inst.row_get(g, slot, "secret")? else { return fail(500, "auth needs an int `secret` field") };
                if real as u64 != sec { return fail(403, "bad session secret"); }
                Value::Row(g, slot)
            }
            o => return fail(500, format!("unknown function `{}`", o)),
        })
    }

    /// Rows of `it.coll` passing `where`, ordered and limited.
    fn select(&mut self, it: &'a Iter) -> R<(usize, Vec<usize>)> {
        let Some(g) = self.inst.gindex(&it.coll).filter(|g| self.inst.coll_kind(*g).map_or(false, |k| k.0 > 0)) else { return fail(500, format!("`{}` is not a table or pool", it.coll)) };
        let limit = match &it.limit { Some(e) => { let v = self.eval(e)?; match self.prim(v)? { Value::Int(n) if n >= 0 => Some(n as usize), _ => return fail(400, "bad limit") } } None => None };
        let mut hits: Vec<(f64, usize)> = Vec::new();
        for slot in self.inst.slots(g) {
            self.vars.push((it.var.as_str(), Value::Row(g, slot)));
            let keep = match &it.wher { Some(w) => { let v = self.eval(w); match v { Ok(v) => self.truthy(v), Err(e) => Err(e) } } None => Ok(true) };
            let key = match (&keep, &it.order) { (Ok(true), Some((k, _))) => { let v = self.eval(k).and_then(|v| self.prim(v)); match v { Ok(Value::Int(i)) => Ok(i as f64), Ok(Value::Float(f)) => Ok(f), Ok(_) => fail(500, "order by needs a number"), Err(e) => Err(e) } } _ => Ok(0.0) };
            self.vars.pop();
            if !keep? { continue; }
            hits.push((key?, slot));
            if it.order.is_none() && limit.map_or(false, |l| hits.len() >= l) { break; }
        }
        if let Some((_, desc)) = &it.order { hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)); if *desc { hits.reverse(); } }
        if let Some(l) = limit { hits.truncate(l); }
        Ok((g, hits.into_iter().map(|h| h.1).collect()))
    }

    fn exec(&mut self, stmts: &'a [Stmt]) -> R<()> {
        let mark = self.vars.len();
        for s in stmts {
            let r = self.stmt(s);
            if r.is_err() { self.vars.truncate(mark); return r; }
        }
        self.vars.truncate(mark);
        Ok(())
    }

    fn stmt(&mut self, s: &'a Stmt) -> R<()> {
        match s {
            Stmt::Let(n, e) => { let v = self.eval(e)?; self.vars.push((n.as_str(), v)); }
            Stmt::Alloc { var, coll, try_ } => {
                let Some(g) = self.inst.gindex(coll) else { return fail(500, format!("no state named `{}`", coll)) };
                match self.inst.alloc_row(g) {
                    Ok((slot, _)) => { self.undo.push(Undo::Alloc(g, slot)); self.vars.push((var.as_str(), Value::Row(g, slot))) }
                    Err(Error::Full(_)) if *try_ => self.vars.push((var.as_str(), Value::Null)),
                    Err(e) => return Err(e.into()),
                }
            }
            Stmt::Assign(lhs, rhs) => {
                let Expr::Field(base, f) = lhs else { return fail(500, "can only assign to a field") };
                let target = self.eval(base)?;
                let v = self.eval(rhs)?;
                let val = self.to_val(v)?;
                match target {
                    Value::Row(g, slot) => { if let Ok(old) = self.inst.row_get(g, slot, f) { self.undo.push(Undo::Row(g, slot, f.clone(), old)); } self.inst.row_set(g, slot, f, &val)? }
                    Value::Path(p) => { let np = format!("{}.{}", p, f); if let Ok(old) = self.inst.path_get(&np) { self.undo.push(Undo::Path(np.clone(), old)); } self.inst.path_set(&np, &val)? }
                    _ => return fail(500, "assignment target is not a row or state struct"),
                }
            }
            Stmt::If(c, t, e) => { let v = self.eval(c)?; if self.truthy(v)? { self.exec(t)?; } else { self.exec(e)?; } }
            Stmt::Each(it, body) => {
                let (g, slots) = self.select(it)?;
                for slot in slots {
                    if !self.inst.row_live(g, slot) { continue; } // an earlier iteration may have freed it
                    self.vars.push((it.var.as_str(), Value::Row(g, slot)));
                    let r = self.exec(body);
                    self.vars.pop();
                    r?;
                }
            }
            Stmt::Free(e) => match self.eval(e)? { Value::Row(g, slot) => self.inst.free_row(g, slot)?, Value::Null => {}, _ => return fail(500, "free needs a row") },
            Stmt::Fail(code, m) => return fail(*code, m.clone()),
            Stmt::Respond(code, j) => { self.json(j)?; return Err(Ctl::Done(*code)); }
        }
        Ok(())
    }

    fn out(&mut self, s: &str) -> R<()> { if self.buf.push(s.as_bytes()) { Ok(()) } else { fail(507, "response too large") } }

    fn json(&mut self, j: &'a Json) -> R<()> {
        match j {
            Json::Obj(fields) => {
                self.out("{")?;
                for (i, (k, v)) in fields.iter().enumerate() { if i > 0 { self.out(",")?; } self.out(&quote(k))?; self.out(":")?; self.json(v)?; }
                self.out("}")
            }
            Json::Arr(it, inner) => {
                let (g, slots) = self.select(it)?;
                self.out("[")?;
                let mut first = true;
                for slot in slots {
                    if !first { self.out(",")?; }
                    first = false;
                    self.vars.push((it.var.as_str(), Value::Row(g, slot)));
                    let r = self.json(inner);
                    self.vars.pop();
                    r?;
                }
                self.out("]")
            }
            Json::E(e) => {
                let v = self.eval(e)?;
                let t = match self.prim(v)? { Value::Int(i) => i.to_string(), Value::Float(f) => f.to_string(), Value::Bool(b) => b.to_string(), Value::Str(s) => quote(&s), Value::Null => "null".into(), _ => return fail(500, "cannot write this value as JSON") };
                self.out(&t)
            }
        }
    }
}

// ---------------------------------------------------------------- HTTP

pub fn serve(server: &mut Server, port: u16) -> std::io::Result<()> {
    let l = std::net::TcpListener::bind(("127.0.0.1", port))?;
    println!("listening on http://127.0.0.1:{} (single-threaded; Ctrl-C to stop)", port);
    for conn in l.incoming() {
        let Ok(mut c) = conn else { continue };
        let _ = c.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let mut data = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            if let Some(p) = data.windows(4).position(|w| w == b"\r\n\r\n") { break Some(p); }
            if data.len() > 64 * 1024 { break None; }
            match c.read(&mut chunk) { Ok(0) | Err(_) => break None, Ok(n) => data.extend_from_slice(&chunk[..n]) }
        };
        let Some(he) = head_end else { let _ = c.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"); continue };
        let head = String::from_utf8_lossy(&data[..he]).into_owned();
        let mut lines = head.lines();
        let mut rl = lines.next().unwrap_or("").split(' ');
        let (method, target) = (rl.next().unwrap_or("").to_string(), rl.next().unwrap_or("/").to_string());
        let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
        let want: usize = headers.iter().find(|h| h.0.eq_ignore_ascii_case("content-length")).and_then(|h| h.1.parse().ok()).unwrap_or(0).min(1 << 20);
        while data.len() < he + 4 + want { match c.read(&mut chunk) { Ok(0) | Err(_) => break, Ok(n) => data.extend_from_slice(&chunk[..n]) } }
        let body = String::from_utf8_lossy(&data[(he + 4).min(data.len())..]).into_owned();
        let (code, out) = server.handle(&method, &target, &headers, &body);
        let reason = match code { 200 => "OK", 201 => "Created", 204 => "No Content", 400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found", 413 => "Payload Too Large", 429 => "Too Many Requests", 503 => "Service Unavailable", 507 => "Insufficient Storage", _ => "Error" };
        let _ = c.write_all(format!("HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", code, reason, out.len(), out).as_bytes());
    }
    Ok(())
}

// ---------------------------------------------------------------- benchmark

fn token_of(body: &str) -> String {
    body.split("\"token\":\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("").to_string()
}

struct Stat { name: &'static str, ns: Vec<u128>, codes: std::collections::BTreeMap<u16, usize> }
impl Stat {
    fn new(name: &'static str) -> Stat { Stat { name, ns: Vec::new(), codes: Default::default() } }
    fn row(&mut self) {
        if self.ns.is_empty() { return; }
        self.ns.sort_unstable();
        let n = self.ns.len();
        let mean = self.ns.iter().sum::<u128>() as f64 / n as f64 / 1000.0;
        println!("  {:<34} {:>7} {:>10.1} {:>10.1} {:>10.1}   {:?}", self.name, n, mean, self.ns[n / 2] as f64 / 1000.0, self.ns[(n * 99 / 100).min(n - 1)] as f64 / 1000.0, self.codes);
    }
}

pub fn bench(s: &mut Server, mixed: usize) {
    let mb = |n: usize| n as f64 / (1 << 20) as f64;
    let snap = |s: &Server, label: &str| println!("  [{:<28}] committed {:>7.2} MB  live {:>7.2} MB  reserved {:>7.2} MB  OS commit {:>7.2} MB", label, mb(s.inst.committed()), mb(s.inst.live_total()), mb(s.inst.reserved()), mb(os::os_mem().private));
    let mut rng = 0x2545F4914F6CDD1Du64;
    let mut rnd = move |n: usize| { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; (rng % n as u64) as usize };
    s.clock = Some(1_000_000);
    s.inst.path_set("meta.rate", &Val::Int(1_000_000)).ok(); // lift the per-session rate limit for load
    println!("in-process benchmark: handler calls only, no sockets. Times in microseconds (interpreter-bound: the interpreter, not the memory layout, dominates).");
    println!("  {:<34} {:>7} {:>10} {:>10} {:>10}", "endpoint", "calls", "mean us", "p50 us", "p99 us");
    snap(s, "start");

    let mut st_sess = Stat::new("POST /sessions");
    let mut tokens = Vec::new();
    for i in 0..1000 {
        let t = Instant::now();
        let (c, b) = s.handle("POST", "/sessions", &[], &format!("{{\"name\":\"user{}\"}}", i));
        st_sess.ns.push(t.elapsed().as_nanos()); *st_sess.codes.entry(c).or_default() += 1;
        tokens.push(token_of(&b));
    }
    st_sess.row();
    let auth = |t: &str| vec![("X-Session".to_string(), t.to_string())];
    let mut st_room = Stat::new("POST /rooms");
    for i in 0..20 { let t = Instant::now(); let (c, _) = s.handle("POST", "/rooms", &auth(&tokens[0]), &format!("{{\"name\":\"room{}\"}}", i)); st_room.ns.push(t.elapsed().as_nanos()); *st_room.codes.entry(c).or_default() += 1; }
    st_room.row();
    snap(s, "1000 sessions, 20 rooms");

    let body = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.";
    let mut st_fill = Stat::new("POST messages (pool filling)");
    for _ in 0..20_000 {
        let (u, r) = (rnd(1000), rnd(20));
        let t = Instant::now();
        let (c, _) = s.handle("POST", &format!("/rooms/{}/messages", r), &auth(&tokens[u]), &format!("{{\"body\":\"{}\"}}", body));
        st_fill.ns.push(t.elapsed().as_nanos()); *st_fill.codes.entry(c).or_default() += 1;
    }
    st_fill.row();
    snap(s, "20000 messages (pool full)");

    let mut st_evict = Stat::new("POST messages (full: evicts)");
    for _ in 0..200 {
        let (u, r) = (rnd(1000), rnd(20));
        let t = Instant::now();
        let (c, _) = s.handle("POST", &format!("/rooms/{}/messages", r), &auth(&tokens[u]), &format!("{{\"body\":\"{}\"}}", body));
        st_evict.ns.push(t.elapsed().as_nanos()); *st_evict.codes.entry(c).or_default() += 1;
    }
    st_evict.row();

    let (mut st_get, mut st_post, mut st_tping, mut st_tlist) = (Stat::new("GET messages (limit 50)"), Stat::new("POST messages (mixed)"), Stat::new("POST typing"), Stat::new("GET typing"));
    for i in 0..mixed {
        let (u, r) = (rnd(1000), rnd(20));
        s.clock = Some(1_000_000 + i as i64 * 5);
        let t = Instant::now();
        let k = rnd(100);
        let (c, st) = if k < 70 { (s.handle("GET", &format!("/rooms/{}/messages?limit=50", r), &auth(&tokens[u]), "").0, &mut st_get) }
            else if k < 90 { (s.handle("POST", &format!("/rooms/{}/messages", r), &auth(&tokens[u]), &format!("{{\"body\":\"{}\"}}", body)).0, &mut st_post) }
            else if k < 95 { (s.handle("POST", &format!("/rooms/{}/typing", r), &auth(&tokens[u]), "").0, &mut st_tping) }
            else { (s.handle("GET", &format!("/rooms/{}/typing", r), &auth(&tokens[u]), "").0, &mut st_tlist) };
        st.ns.push(t.elapsed().as_nanos()); *st.codes.entry(c).or_default() += 1;
    }
    for st in [&mut st_get, &mut st_post, &mut st_tping, &mut st_tlist] { st.row(); }
    snap(s, "after mixed load");

    let mut st_del = Stat::new("DELETE /session");
    for t in tokens.iter() { let tm = Instant::now(); let (c, _) = s.handle("DELETE", "/session", &auth(t), ""); st_del.ns.push(tm.elapsed().as_nanos()); *st_del.codes.entry(c).or_default() += 1; }
    st_del.row();
    let (c, b) = s.handle("GET", "/stats", &[], "");
    println!("  GET /stats -> {} {}", c, b);
    snap(s, "sessions deleted");
    let freed = s.inst.trim();
    let t = s.inst.last_trim;
    println!("  trim released {:.2} MB (scan {:.1} ms over {} ranges, {} OS calls)", mb(freed), t.scan_ns as f64 / 1e6, t.ranges, t.os_calls);
    snap(s, "after trim");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Program;

    const STATE: &str = "struct Meta { next_seq: i64; rate: i64; typing_ttl: i64; }
        struct Session { name: string<32>; secret: i64; window: i64; sent: i64; }
        struct Room { name: string<8>; created: i64; }
        struct Msg { room: i32; author: i32; seq: i64; at: i64; author_name: string<32>; body: string<2000>; }
        struct Typing { room: i32; who: i32; until: i64; name: string<32>; }
        global meta: Meta; global sessions: pool<Session, 3> checked; global rooms: pool<Room, 2>;
        global messages: pool<Msg, 5>; global typing: pool<Typing, 10>;";
    fn server() -> Server {
        let inst = Instance::start(Program::parse(STATE).unwrap(), 1 << 30).unwrap();
        let mut s = Server::new(inst, include_str!("../examples/chat/chat.api")).unwrap();
        s.clock = Some(1000);
        s
    }
    fn login(s: &mut Server, name: &str) -> Vec<(String, String)> {
        let (c, b) = s.handle("POST", "/sessions", &[], &format!("{{\"name\":\"{}\"}}", name));
        assert_eq!(c, 201);
        vec![("X-Session".into(), token_of(&b))]
    }

    #[test]
    fn auth_distinguishes_missing_wrong_secret_and_ended_sessions() {
        let mut s = server();
        let mut h = login(&mut s, "ann");
        assert_eq!(s.handle("GET", "/rooms", &[], "").0, 401);
        assert_eq!(s.handle("GET", "/rooms", &h, "").0, 200);
        let mut wrong = h.clone(); let last = h[0].1.pop().unwrap(); wrong[0].1 = format!("{}{}", h[0].1, if last == '0' { '1' } else { '0' }); h[0].1.push(last); // same slot and generation, different secret
        assert_eq!(s.handle("GET", "/rooms", &wrong, "").0, 403);
        assert_eq!(s.handle("DELETE", "/session", &h, "").0, 200);
        assert_eq!(s.handle("GET", "/rooms", &h, "").0, 401);
        let h2 = login(&mut s, "bob"); // reuses slot 0 with a new generation
        assert_eq!(s.handle("GET", "/rooms", &h, "").0, 401); // the old token does not work on the reused slot
        assert_eq!(s.handle("GET", "/rooms", &h2, "").0, 200);
    }

    #[test]
    fn error_codes_separate_client_mistakes_from_capacity() {
        let mut s = server();
        let h = login(&mut s, "ann");
        assert_eq!(s.handle("POST", "/rooms", &h, "{\"name\":\"123456789\"}").0, 413); // string<8>
        assert_eq!(s.handle("POST", "/rooms", &h, "{}").0, 400);
        assert_eq!(s.handle("POST", "/rooms", &h, "{\"name\":\"a\"}").0, 201);
        assert_eq!(s.handle("POST", "/rooms", &h, "{\"name\":\"b\"}").0, 201);
        assert_eq!(s.handle("POST", "/rooms", &h, "{\"name\":\"c\"}").0, 503); // pool<Room, 2> is full
        assert_eq!(s.handle("GET", "/rooms/7/messages", &h, "").0, 404);
        assert_eq!(s.handle("GET", "/rooms/x/messages", &h, "").0, 400);
    }

    #[test]
    fn full_message_pool_evicts_the_oldest_and_listing_is_newest_first() {
        let mut s = server();
        let h = login(&mut s, "ann");
        s.handle("POST", "/rooms", &h, "{\"name\":\"r\"}");
        for i in 1..=8 { assert_eq!(s.handle("POST", "/rooms/0/messages", &h, &format!("{{\"body\":\"m{}\"}}", i)).0, 201); }
        let (c, b) = s.handle("GET", "/rooms/0/messages?limit=10", &h, "");
        assert_eq!(c, 200);
        assert!(b.contains("\"body\":\"m8\"") && b.contains("\"body\":\"m4\"") && !b.contains("\"body\":\"m3\""), "{}", b);
        assert!(b.find("m8").unwrap() < b.find("m7").unwrap()); // newest first
        let (_, b) = s.handle("GET", "/stats", &[], "");
        assert!(b.contains("\"messages\":5"), "{}", b);
    }

    #[test]
    fn rate_limit_and_typing_expiry() {
        let mut s = server();
        let h = login(&mut s, "ann");
        s.handle("POST", "/rooms", &h, "{\"name\":\"r\"}");
        for _ in 0..30 { assert_eq!(s.handle("POST", "/rooms/0/messages", &h, "{\"body\":\"x\"}").0, 201); }
        assert_eq!(s.handle("POST", "/rooms/0/messages", &h, "{\"body\":\"x\"}").0, 429);
        s.clock = Some(1000 + 61_000); // the window passes
        assert_eq!(s.handle("POST", "/rooms/0/messages", &h, "{\"body\":\"x\"}").0, 201);
        s.handle("POST", "/rooms/0/typing", &h, "");
        assert!(s.handle("GET", "/rooms/0/typing", &h, "").1.contains("ann"));
        s.clock = Some(1000 + 61_000 + 6_000); // 5 s ttl has passed
        assert!(!s.handle("GET", "/rooms/0/typing", &h, "").1.contains("ann"));
    }

    #[test]
    fn json_escaping_and_response_overflow() {
        let mut s = server();
        let h = login(&mut s, "a\\\"b");
        let (_, b) = s.handle("POST", "/rooms", &h, "{\"name\":\"q\\\"\"}");
        assert!(b.contains("q\\\""), "{}", b);
        assert_eq!(s.buf.cap, RESPONSE_CAP);
    }
}
