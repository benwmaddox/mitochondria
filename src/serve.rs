//! Runs endpoints against declared state: an interpreter, a small HTTP/1.1 server, and an in-process benchmark.
//! Each request is atomic: the runtime records every write, and a failed request is rolled back. Frees happen only
//! when the request succeeds.
use crate::lang::*;
use crate::layout::{CKind, Ty};
use crate::os;
use crate::region::{Scratch, Space};
use crate::runtime::{Error, Instance, Loc, Val};
use std::io::{Read, Write};
use std::time::Instant;

#[derive(Clone, Debug)]
enum Value {
    Null, Int(i64), Float(f64), Bool(bool), Str(String),
    Item(usize, usize),            // collection, item index
    Coll(usize, usize),            // collection, instance
    Part(usize, usize, String),    // part of an item: a nested struct's field path prefix
    ArrField(usize, usize, usize), // an array column of an item
    Plain(usize, Ty),              // a plain struct or fixed array at an offset
}

enum Ctl { Done(u16), Fail(u16, String), Return(Value), Err(Error) }
impl From<Error> for Ctl { fn from(e: Error) -> Ctl { Ctl::Err(e) } }
type R<T> = Result<T, Ctl>;
fn fail<T>(code: u16, m: impl Into<String>) -> R<T> { Err(Ctl::Fail(code, m.into())) }

/// Response bytes live in a fixed block inside a declared reservation, reused by every request.
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
    pub api: Api,
    buf: Buf,
    pub clock: Option<i64>, // fixed time in ms, for tests and benchmarks; otherwise the system clock
    _scratch: Scratch,
    _space: Space,
}

pub const RESPONSE_CAP: usize = 256 * 1024;

impl Server {
    pub fn new(inst: Instance, api_src: &str) -> Result<Server, String> {
        let api = parse(api_src)?;
        let mut space = Space::new(2 * RESPONSE_CAP);
        let region = space.region(RESPONSE_CAP + 4096, 64 * 1024).map_err(|_| "response region".to_string())?;
        let mut scratch = Scratch::new(region);
        let ptr = scratch.alloc(RESPONSE_CAP).map_err(|_| "response buffer".to_string())?;
        Ok(Server { inst, api, buf: Buf { ptr, cap: RESPONSE_CAP, len: 0 }, clock: None, _scratch: scratch, _space: space })
    }

    /// Handle one request. `target` is the path plus optional `?query`.
    pub fn handle(&mut self, method: &str, target: &str, headers: &[(String, String)], body: &str) -> (u16, String) {
        let (path, qs) = target.split_once('?').unwrap_or((target, ""));
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut hit = None;
        'eps: for (i, e) in self.api.eps.iter().enumerate() {
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
        self.buf.len = 0; // the request's response space starts empty
        let now = self.clock.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0));
        let Server { inst, api, buf, .. } = self;
        inst.begin();
        let mut run = Run { inst, api, req: &req, vars: Vec::new(), frame: 0, depth: 0, buf, now, frees: Vec::new() };
        let res = run.exec(&api.eps[ei].body);
        let frees = std::mem::take(&mut run.frees);
        let ok = matches!(res, Ok(()) | Err(Ctl::Done(_)) | Err(Ctl::Return(_)));
        let out = match res {
            Ok(()) | Err(Ctl::Return(_)) => (204, String::new()),
            Err(Ctl::Done(code)) => (code, run.buf.text()),
            Err(Ctl::Fail(code, m)) => (code, err_json(&m)),
            Err(Ctl::Err(Error::Full(m))) => (503, err_json(&format!("capacity: {}", m))),
            Err(Ctl::Err(Error::TooLong(m))) => (413, err_json(&m)),
            Err(Ctl::Err(Error::NotLive(m))) | Err(Ctl::Err(Error::Stale(m))) => (404, err_json(&m)),
            Err(Ctl::Err(Error::Bad(m))) => (400, err_json(&m)),
        };
        if ok {
            for (c, i) in frees { let _ = self.inst.free_item(c, i); }
            self.inst.commit_changes();
        } else {
            self.inst.rollback();
        }
        out
    }
}

fn err_json(m: &str) -> String { format!("{{\"error\":{}}}", quote(m)) }

struct Run<'a> {
    inst: &'a mut Instance, api: &'a Api, req: &'a Req,
    vars: Vec<(&'a str, Value, bool)>, // name, value, mutable
    frame: usize, depth: usize,
    buf: &'a mut Buf, now: i64,
    frees: Vec<(usize, usize)>,
}

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

impl<'a> Run<'a> {
    fn lookup(&self, n: &str) -> Option<Value> { self.vars[self.frame..].iter().rev().find(|v| v.0 == n).map(|v| v.1.clone()) }

    /// A value read from state. References are followed: a dead one is a `NotLive`/`Stale` error, a null one is `Null`.
    fn from_val(&self, v: Val, target: Option<usize>) -> R<Value> {
        Ok(match v {
            Val::Int(i) => Value::Int(i), Val::Float(f) => Value::Float(f), Val::Bool(b) => Value::Bool(b), Val::Str(s) => Value::Str(s),
            Val::Ref(None) => Value::Null,
            Val::Ref(Some(r)) => { let t = target.expect("reference target"); Value::Item(t, self.inst.deref(t, r)?) }
        })
    }
    fn ref_target(&self, l: &Loc) -> Option<usize> {
        match l {
            Loc::Field(c, _, ci) => match self.inst.col_kind(*c, *ci) { CKind::Ref(t) => Some(*t), _ => None },
            Loc::Plain(_, Ty::Ref(n)) => self.inst.prog.coll_of(n),
            _ => None,
        }
    }
    /// Turn a location into a value: read scalars and strings, keep structs, collections and arrays as places.
    fn loc_value(&self, l: Loc) -> R<Value> {
        match l {
            Loc::Coll(c, i) => Ok(Value::Coll(c, i)),
            Loc::Item(c, i, p) => Ok(if p.is_empty() { Value::Item(c, i) } else { Value::Part(c, i, p) }),
            Loc::Field(c, i, ci) if matches!(self.inst.col_kind(c, ci), CKind::Arr(..)) => Ok(Value::ArrField(c, i, ci)),
            Loc::Plain(off, t @ (Ty::Struct(_) | Ty::Arr(..))) => Ok(Value::Plain(off, t)),
            l => { let t = self.ref_target(&l); let v = self.inst.loc_get(&l)?; self.from_val(v, t) }
        }
    }
    fn field(&self, base: Value, f: &str) -> R<Value> {
        match base {
            Value::Item(c, i) => {
                if let Some(ci) = self.inst.col(c, f) { // the common case: one column, no path building
                    return match self.inst.col_kind(c, ci) {
                        CKind::Arr(..) => Ok(Value::ArrField(c, i, ci)),
                        CKind::Ref(t) => { let t = *t; let v = self.inst.field_get(c, i, ci)?; self.from_val(v, Some(t)) }
                        _ => Ok(self.from_val(self.inst.field_get(c, i, ci)?, None)?),
                    };
                }
                self.loc_value(self.inst.step_field(Loc::Item(c, i, String::new()), f)?)
            }
            Value::Part(c, i, p) => self.loc_value(self.inst.step_field(Loc::Item(c, i, p), f)?),
            Value::Plain(off, t) => self.loc_value(self.inst.step_field(Loc::Plain(off, t), f)?),
            Value::Null => Err(Error::NotLive(format!("`.{}` of a null reference", f)).into()),
            _ => fail(500, format!("`.{}` needs an item or a state struct", f)),
        }
    }
    fn index(&self, base: Value, k: Value) -> R<Value> {
        let Value::Int(k) = k else { return fail(400, "an index must be an integer") };
        if k < 0 { return Err(Error::NotLive(format!("index {} is negative", k)).into()); }
        let l = match base {
            Value::Coll(c, i) => Loc::Coll(c, i),
            Value::Plain(off, t) => Loc::Plain(off, t),
            Value::ArrField(c, i, ci) => Loc::Field(c, i, ci),
            _ => return fail(500, "cannot index this value"),
        };
        self.loc_value(self.inst.step_index(l, k as usize, None)?)
    }
    /// The place an assignment writes to.
    fn place(&mut self, e: &Expr) -> R<Loc> {
        match e {
            Expr::Field(b, f) => match self.eval(b)? {
                Value::Item(c, i) => Ok(match self.inst.col(c, f) { Some(ci) => Loc::Field(c, i, ci), None => self.inst.step_field(Loc::Item(c, i, String::new()), f)? }),
                Value::Part(c, i, p) => Ok(self.inst.step_field(Loc::Item(c, i, p), f)?),
                Value::Plain(off, t) => Ok(self.inst.step_field(Loc::Plain(off, t), f)?),
                _ => fail(500, "assignment target is not an item or a state struct"),
            },
            Expr::Index(b, k) => {
                let (bv, kv) = (self.eval(b)?, self.eval(k)?);
                let Value::Int(k) = kv else { return fail(400, "an index must be an integer") };
                let l = match bv { Value::ArrField(c, i, ci) => Loc::Field(c, i, ci), Value::Plain(off, t) => Loc::Plain(off, t), _ => return fail(500, "only fixed arrays can be assigned by index") };
                Ok(self.inst.step_index(l, k.max(0) as usize, None)?)
            }
            _ => fail(500, "cannot assign to this"),
        }
    }
    fn store(&mut self, l: Loc, v: Value) -> R<()> {
        let val = match (self.ref_target(&l), v) {
            (Some(_), Value::Null) => Val::Ref(None),
            (Some(t), Value::Item(c, i)) if c == t => Val::Ref(Some((i, self.inst.gen(c, i).unwrap_or(0)))),
            (Some(_), _) => return fail(500, "this field holds a reference to a different pool"),
            (None, Value::Int(i)) => Val::Int(i), (None, Value::Float(f)) => Val::Float(f), (None, Value::Bool(b)) => Val::Bool(b), (None, Value::Str(s)) => Val::Str(s),
            (None, _) => return fail(500, "cannot store this value in state"),
        };
        Ok(self.inst.loc_set(&l, &val)?)
    }

    fn truthy(&self, v: &Value) -> R<bool> { match v { Value::Bool(b) => Ok(*b), Value::Null => Ok(false), _ => fail(500, "condition is not a boolean") } }
    fn num(v: &Value) -> Option<f64> { match v { Value::Int(i) => Some(*i as f64), Value::Float(f) => Some(*f), _ => None } }

    fn eval(&mut self, e: &Expr) -> R<Value> {
        match e {
            Expr::Int(n) => Ok(Value::Int(*n)),
            Expr::Float(f) => Ok(Value::Float(*f)),
            Expr::Str(s) => Ok(Value::Str(s.clone())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            Expr::Var(n) => {
                if let Some(v) = self.lookup(n) { return Ok(v); }
                let Some(g) = self.inst.prog.global(n) else { return fail(500, format!("unknown name `{}`", n)) };
                let g = &self.inst.prog.globals[g];
                match g.coll { Some(c) => Ok(Value::Coll(c, 0)), None => { let l = Loc::Plain(g.off, g.ty.clone()); self.loc_value(l) } }
            }
            Expr::Field(b, f) => { let bv = self.eval(b)?; self.field(bv, f) }
            Expr::Index(b, i) => { let (bv, iv) = (self.eval(b)?, self.eval(i)?); self.index(bv, iv) }
            Expr::Un(op, x) => {
                let v = self.eval(x)?;
                if *op == '!' { return Ok(Value::Bool(!self.truthy(&v)?)); }
                match v { Value::Int(i) => Ok(Value::Int(-i)), Value::Float(f) => Ok(Value::Float(-f)), _ => fail(500, "cannot negate") }
            }
            Expr::Bin(op, a, b) => {
                if op == "&&" { let l = self.eval(a)?; return if !self.truthy(&l)? { Ok(Value::Bool(false)) } else { let r = self.eval(b)?; Ok(Value::Bool(self.truthy(&r)?)) }; }
                if op == "||" { let l = self.eval(a)?; return if self.truthy(&l)? { Ok(Value::Bool(true)) } else { let r = self.eval(b)?; Ok(Value::Bool(self.truthy(&r)?)) }; }
                let (l, r) = (self.eval(a)?, self.eval(b)?);
                self.binop(op, l, r)
            }
            Expr::Call(n, args) => self.call(n, args),
        }
    }

    fn binop(&self, op: &str, l: Value, r: Value) -> R<Value> {
        if op == "==" || op == "!=" {
            let eq = match (&l, &r) {
                (Value::Null, Value::Null) => true, (Value::Null, _) | (_, Value::Null) => false,
                (Value::Str(a), Value::Str(b)) => a == b, (Value::Bool(a), Value::Bool(b)) => a == b,
                (Value::Item(a, b), Value::Item(c, d)) => a == c && b == d,
                _ => match (Self::num(&l), Self::num(&r)) { (Some(a), Some(b)) => a == b, _ => return fail(500, "cannot compare these values") },
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
        if let (Some(a), Some(b)) = (Self::num(&l), Self::num(&r)) {
            return Ok(match op { "+" => Value::Float(a + b), "-" => Value::Float(a - b), "*" => Value::Float(a * b), "/" => Value::Float(a / b),
                "<" => Value::Bool(a < b), "<=" => Value::Bool(a <= b), ">" => Value::Bool(a > b), ">=" => Value::Bool(a >= b), _ => return fail(500, "bad operator") });
        }
        if let (Value::Str(a), Value::Str(b)) = (&l, &r) { return Ok(Value::Bool(match op { "<" => a < b, "<=" => a <= b, ">" => a > b, ">=" => a >= b, _ => return fail(500, "bad operator") })); }
        fail(500, "operator does not fit these values")
    }

    fn show(&self, v: &Value) -> R<String> {
        Ok(match v { Value::Int(i) => i.to_string(), Value::Float(f) => f.to_string(), Value::Bool(b) => b.to_string(), Value::Str(s) => s.clone(), Value::Null => "null".into(), _ => return fail(500, "cannot show this value") })
    }

    fn call(&mut self, n: &str, args: &[Expr]) -> R<Value> {
        // forms that look at their arguments before (or instead of) evaluating them
        match n {
            "or" => {
                let first = match args.first() { Some(e) => self.eval(e), None => return fail(500, "or(a, b)") };
                return match first {
                    Ok(Value::Null) | Err(Ctl::Err(Error::NotLive(_))) | Err(Ctl::Err(Error::Stale(_))) => match args.get(1) { Some(e) => self.eval(e), None => Ok(Value::Null) },
                    other => other,
                };
            }
            "exists" => return match args.first().map(|e| self.eval(e)) { Some(Ok(Value::Null)) | Some(Err(Ctl::Err(Error::NotLive(_)))) | Some(Err(Ctl::Err(Error::Stale(_)))) => Ok(Value::Bool(false)), Some(Ok(_)) => Ok(Value::Bool(true)), Some(Err(e)) => Err(e), None => fail(500, "exists(x)") },
            "maxlen" => {
                let Some(Expr::Field(b, f)) = args.first() else { return fail(500, "maxlen(item.field)") };
                return match self.eval(b)? {
                    Value::Item(c, _) => match self.inst.col(c, f).map(|ci| self.inst.col_kind(c, ci + 1).clone()) { Some(CKind::StrBytes(m)) => Ok(Value::Int(m as i64)), _ => fail(500, "maxlen needs a string field") },
                    Value::Plain(off, t) => match self.inst.step_field(Loc::Plain(off, t), f)? { Loc::Plain(_, Ty::Str(m)) => Ok(Value::Int(m as i64)), _ => fail(500, "maxlen needs a string field") },
                    _ => fail(500, "maxlen(item.field)"),
                };
            }
            _ => {}
        }
        let mut a = Vec::with_capacity(args.len());
        for x in args { a.push(self.eval(x)?); }
        if let Some(f) = self.api.fns.iter().find(|f| f.name == n) { return self.call_fn(f, a); }
        let arg = |i: usize| a.get(i).cloned().unwrap_or(Value::Null);
        let text = |i: usize| -> R<String> { match a.get(i) { Some(Value::Str(s)) => Ok(s.clone()), _ => fail(500, format!("{}(...) needs a string argument", n)) } };
        Ok(match n {
            "body" => { let k = text(0)?; self.req.json.iter().find(|x| x.0 == k).map(|x| x.1.clone()).unwrap_or(Value::Null) }
            "param" => { let k = text(0)?; self.req.params.iter().find(|x| x.0 == k).map(|x| Value::Str(x.1.clone())).unwrap_or(Value::Null) }
            "query" => { let k = text(0)?; self.req.query.iter().find(|x| x.0 == k).map(|x| Value::Str(x.1.clone())).unwrap_or(Value::Null) }
            "header" => { let k = text(0)?.to_lowercase(); self.req.headers.iter().find(|x| x.0 == k).map(|x| Value::Str(x.1.clone())).unwrap_or(Value::Null) }
            "int" => match arg(0) {
                Value::Int(i) => Value::Int(i), Value::Float(f) => Value::Int(f as i64),
                Value::Str(s) => match s.trim().parse::<i64>() { Ok(i) => Value::Int(i), Err(_) => return fail(400, format!("`{}` is not a number", s)) },
                Value::Null => return fail(400, "missing number"), _ => return fail(500, "int(x)"),
            },
            "str" => Value::Str(self.show(&arg(0))?),
            "len" => match arg(0) { Value::Str(s) => Value::Int(s.len() as i64), Value::Null => Value::Int(0), _ => return fail(500, "len(text)") },
            "now" => Value::Int(self.now),
            "random" => Value::Int(os::random_u64() as i64),
            "min" | "max" => match (arg(0), arg(1)) { (Value::Int(p), Value::Int(q)) => Value::Int(if n == "min" { p.min(q) } else { p.max(q) }), _ => return fail(500, "min/max need integers") },
            "count" => match arg(0) { Value::Coll(c, i) => Value::Int(self.inst.count(c, i) as i64), _ => return fail(500, "count(collection)") },
            "capacity" => match arg(0) { Value::Coll(c, _) => Value::Int(self.inst.cap(c) as i64), _ => return fail(500, "capacity(collection)") },
            "slot" => match arg(0) { Value::Item(c, i) => Value::Int((i % self.inst.cap(c)) as i64), _ => return fail(500, "slot(item)") },
            "stat" => Value::Int(match text(0)?.as_str() { "committed_kb" => self.inst.committed() / 1024, "live_kb" => self.inst.live_total() / 1024, "reserved_kb" => self.inst.reserved() / 1024, _ => return fail(500, "unknown stat") } as i64),
            "token" => match arg(0) {
                Value::Item(c, i) => {
                    let ci = self.inst.col(c, "secret").ok_or_else(|| Ctl::Fail(500, "token needs a `secret` field".into()))?;
                    let Val::Int(sec) = self.inst.field_get(c, i, ci)? else { return fail(500, "`secret` must be an integer") };
                    Value::Str(format!("{}.{}.{:x}", i, self.inst.gen(c, i).unwrap_or(0), sec as u64))
                }
                _ => return fail(500, "token(item)"),
            },
            "auth" => {
                let Value::Coll(c, 0) = arg(0) else { return fail(500, "auth(pool)") };
                let Some(tok) = self.req.headers.iter().find(|x| x.0 == "x-session").map(|x| x.1.clone()) else { return fail(401, "missing X-Session header") };
                let mut it = tok.split('.');
                let (Some(Ok(slot)), Some(Ok(gen)), Some(Ok(sec))) = (it.next().map(str::parse::<usize>), it.next().map(str::parse::<u32>), it.next().map(|h| u64::from_str_radix(h, 16))) else { return fail(401, "malformed session token") };
                let Ok(idx) = self.inst.deref(c, (slot, gen)) else { return fail(401, "session ended") };
                let ci = self.inst.col(c, "secret").ok_or_else(|| Ctl::Fail(500, "auth needs a `secret` field".into()))?;
                let Val::Int(real) = self.inst.field_get(c, idx, ci)? else { return fail(500, "`secret` must be an integer") };
                if real as u64 != sec { return fail(403, "bad session secret"); }
                Value::Item(c, idx)
            }
            o => return fail(500, format!("unknown function `{}`", o)),
        })
    }

    /// Call a declared function in a fresh frame: it sees its parameters and the state, not the caller's locals.
    fn call_fn(&mut self, f: &'a Func, args: Vec<Value>) -> R<Value> {
        if args.len() != f.params.len() { return fail(500, format!("{} takes {} arguments", f.name, f.params.len())); }
        if self.depth >= 64 { return fail(500, "calls nested more than 64 deep"); }
        let (saved, base) = (self.frame, self.vars.len());
        self.frame = base; self.depth += 1;
        for (p, v) in f.params.iter().zip(args) { self.vars.push((p.as_str(), v, false)); }
        let r = self.exec(&f.body);
        self.vars.truncate(base); self.frame = saved; self.depth -= 1;
        match r { Ok(()) => Ok(Value::Null), Err(Ctl::Return(v)) => Ok(v), Err(e) => Err(e) }
    }

    /// Items of `it.coll` passing `where`, in order (reversed if asked), ordered and limited.
    fn select(&mut self, it: &'a Iter) -> R<(usize, Vec<usize>)> {
        let Value::Coll(c, inst) = self.eval(&it.coll)? else { return fail(500, "each needs a list or pool") };
        let limit = match &it.limit { Some(e) => match self.eval(e)? { Value::Int(n) if n >= 0 => Some(n as usize), _ => return fail(400, "bad limit") }, None => None };
        let mut items = self.inst.items(c, inst);
        if it.reverse { items.reverse(); }
        if it.wher.is_none() && it.order.is_none() { if let Some(l) = limit { items.truncate(l); } return Ok((c, items)); }
        let mut hits: Vec<(f64, usize)> = Vec::new();
        for idx in items {
            self.vars.push((it.var.as_str(), Value::Item(c, idx), false));
            let keep = match &it.wher { Some(w) => self.eval(w).and_then(|v| self.truthy(&v)), None => Ok(true) };
            let key = match (&keep, &it.order) { (Ok(true), Some((k, _))) => match self.eval(k) { Ok(Value::Int(i)) => Ok(i as f64), Ok(Value::Float(f)) => Ok(f), Ok(_) => fail(500, "order by needs a number"), Err(e) => Err(e) }, _ => Ok(0.0) };
            self.vars.pop();
            if !keep? { continue; }
            hits.push((key?, idx));
            if it.order.is_none() && limit.map_or(false, |l| hits.len() >= l) { break; }
        }
        if let Some((_, desc)) = &it.order { hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)); if *desc { hits.reverse(); } }
        if let Some(l) = limit { hits.truncate(l); }
        Ok((c, hits.into_iter().map(|h| h.1).collect()))
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
            Stmt::Let(n, e) => { let v = self.eval(e)?; self.vars.push((n.as_str(), v, false)); }
            Stmt::Var(n, e) => { let v = self.eval(e)?; self.vars.push((n.as_str(), v, true)); }
            Stmt::Alloc { var, coll, try_ } => {
                let Value::Coll(c, inst) = self.eval(coll)? else { return fail(500, "alloc needs a list or pool") };
                match self.inst.push_item(c, inst) {
                    Ok(idx) => self.vars.push((var.as_str(), Value::Item(c, idx), false)),
                    Err(Error::Full(_)) if *try_ => self.vars.push((var.as_str(), Value::Null, false)),
                    Err(e) => return Err(e.into()),
                }
            }
            Stmt::Assign(Expr::Var(n), rhs) => {
                let v = self.eval(rhs)?;
                let frame = self.frame;
                match self.vars[frame..].iter_mut().rev().find(|x| x.0 == n.as_str()) {
                    Some(slot) if slot.2 => slot.1 = v,
                    Some(_) => return fail(500, format!("`{}` is a `let`; declare it with `var` to change it", n)),
                    None => return fail(500, format!("unknown variable `{}`", n)),
                }
            }
            Stmt::Assign(lhs, rhs) => { let l = self.place(lhs)?; let v = self.eval(rhs)?; self.store(l, v)?; }
            Stmt::If(c, t, e) => { let v = self.eval(c)?; if self.truthy(&v)? { self.exec(t)?; } else { self.exec(e)?; } }
            Stmt::Each(it, body) => {
                let (c, items) = self.select(it)?;
                for idx in items {
                    self.vars.push((it.var.as_str(), Value::Item(c, idx), false));
                    let r = self.exec(body);
                    self.vars.pop();
                    r?;
                }
            }
            Stmt::Free(e) => match self.eval(e)? {
                Value::Item(c, i) => { if self.inst.is_list(c) { return fail(500, "list items are not freed; lists drop their oldest or pop their newest"); } if !self.frees.contains(&(c, i)) { self.frees.push((c, i)); } }
                Value::Null => {}
                _ => return fail(500, "free needs an item"),
            },
            Stmt::Fail(code, m) => return fail(*code, m.clone()),
            Stmt::Respond(code, j) => { self.json(j)?; return Err(Ctl::Done(*code)); }
            Stmt::Return(e) => { let v = match e { Some(e) => self.eval(e)?, None => Value::Null }; return Err(Ctl::Return(v)); }
            Stmt::Do(e) => { self.eval(e)?; }
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
                let (c, items) = self.select(it)?;
                self.out("[")?;
                for (n, idx) in items.into_iter().enumerate() {
                    if n > 0 { self.out(",")?; }
                    self.vars.push((it.var.as_str(), Value::Item(c, idx), false));
                    let r = self.json(inner);
                    self.vars.pop();
                    r?;
                }
                self.out("]")
            }
            Json::E(e) => {
                let t = match self.eval(e)? { Value::Int(i) => i.to_string(), Value::Float(f) => f.to_string(), Value::Bool(b) => b.to_string(), Value::Str(s) => quote(&s), Value::Null => "null".into(), _ => return fail(500, "cannot write this value as JSON") };
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

fn token_of(body: &str) -> String { body.split("\"token\":\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("").to_string() }

struct Stat { name: &'static str, ns: Vec<u128>, codes: std::collections::BTreeMap<u16, usize> }
impl Stat {
    fn new(name: &'static str) -> Stat { Stat { name, ns: Vec::new(), codes: Default::default() } }
    fn add(&mut self, t: Instant, code: u16) { self.ns.push(t.elapsed().as_nanos()); *self.codes.entry(code).or_default() += 1; }
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
    if let Some(g) = s.inst.prog.global("meta") { let off = s.inst.prog.globals[g].off; let _ = s.inst.resolve("meta.rate").and_then(|l| s.inst.loc_set(&l, &Val::Int(1_000_000))); let _ = off; }
    println!("in-process benchmark: handler calls only, no sockets. Times in microseconds (interpreter-bound).");
    println!("  {:<34} {:>7} {:>10} {:>10} {:>10}", "endpoint", "calls", "mean us", "p50 us", "p99 us");
    snap(s, "start");

    let mut st = Stat::new("POST /sessions");
    let mut tokens = Vec::new();
    for i in 0..1000 { let t = Instant::now(); let (c, b) = s.handle("POST", "/sessions", &[], &format!("{{\"name\":\"user{}\"}}", i)); st.add(t, c); tokens.push(token_of(&b)); }
    st.row();
    let auth = |t: &str| vec![("X-Session".to_string(), t.to_string())];
    let mut st = Stat::new("POST /rooms");
    for i in 0..20 { let t = Instant::now(); let (c, _) = s.handle("POST", "/rooms", &auth(&tokens[0]), &format!("{{\"name\":\"room{}\"}}", i)); st.add(t, c); }
    st.row();
    snap(s, "1000 sessions, 20 rooms");

    let body = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.";
    let post = |s: &mut Server, u: &str, r: usize| s.handle("POST", &format!("/rooms/{}/messages", r), &auth(u), &format!("{{\"body\":\"{}\"}}", body)).0;
    let mut st = Stat::new("POST messages (lists filling)");
    for _ in 0..20_000 { let (u, r) = (rnd(1000), rnd(20)); let t = Instant::now(); let c = post(s, &tokens[u], r); st.add(t, c); }
    st.row();
    snap(s, "~20000 messages");
    let mut st = Stat::new("POST messages (full: drop oldest)");
    for _ in 0..2000 { let (u, r) = (rnd(1000), rnd(20)); let t = Instant::now(); let c = post(s, &tokens[u], r); st.add(t, c); }
    st.row();

    let (mut st_get, mut st_post, mut st_tping, mut st_tlist) = (Stat::new("GET messages (limit 50)"), Stat::new("POST messages (mixed)"), Stat::new("POST typing"), Stat::new("GET typing"));
    for i in 0..mixed {
        let (u, r) = (rnd(1000), rnd(20));
        s.clock = Some(1_000_000 + i as i64 * 5);
        let k = rnd(100);
        let t = Instant::now();
        if k < 70 { let c = s.handle("GET", &format!("/rooms/{}/messages?limit=50", r), &auth(&tokens[u]), "").0; st_get.add(t, c); }
        else if k < 90 { let c = post(s, &tokens[u], r); st_post.add(t, c); }
        else if k < 95 { let c = s.handle("POST", &format!("/rooms/{}/typing", r), &auth(&tokens[u]), "").0; st_tping.add(t, c); }
        else { let c = s.handle("GET", &format!("/rooms/{}/typing", r), &auth(&tokens[u]), "").0; st_tlist.add(t, c); }
    }
    for st in [&mut st_get, &mut st_post, &mut st_tping, &mut st_tlist] { st.row(); }
    snap(s, "after mixed load");

    let mut st = Stat::new("DELETE /session");
    for tk in tokens.iter() { let t = Instant::now(); let (c, _) = s.handle("DELETE", "/session", &auth(tk), ""); st.add(t, c); }
    st.row();
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
        struct Msg { seq: i64; at: i64; author: ref<sessions>; body: string<2000>; }
        struct Typing { who: ref<sessions>; until: i64; }
        struct Room { name: string<8>; created: i64; messages: list<Msg, 5> drop_oldest; typing: pool<Typing, 4>; }
        global meta: Meta; global sessions: pool<Session, 3> checked; global rooms: pool<Room, 2>;";
    fn server() -> Server {
        let inst = Instance::start(Program::parse(STATE).unwrap(), 1 << 30).unwrap();
        let mut s = Server::new(inst, include_str!("../examples/chat/chat.api")).unwrap();
        s.clock = Some(1000);
        s
    }
    fn login(s: &mut Server, name: &str) -> Vec<(String, String)> {
        let (c, b) = s.handle("POST", "/sessions", &[], &format!("{{\"name\":\"{}\"}}", name));
        assert_eq!(c, 201, "{}", b);
        vec![("X-Session".into(), token_of(&b))]
    }

    #[test]
    fn auth_distinguishes_missing_wrong_secret_and_ended_sessions() {
        let mut s = server();
        let mut h = login(&mut s, "ann");
        assert_eq!(s.handle("GET", "/rooms", &[], "").0, 401);
        assert_eq!(s.handle("GET", "/rooms", &h, "").0, 200);
        let mut wrong = h.clone(); let last = h[0].1.pop().unwrap(); wrong[0].1 = format!("{}{}", h[0].1, if last == '0' { '1' } else { '0' }); h[0].1.push(last);
        assert_eq!(s.handle("GET", "/rooms", &wrong, "").0, 403);
        assert_eq!(s.handle("DELETE", "/session", &h, "").0, 200);
        assert_eq!(s.handle("GET", "/rooms", &h, "").0, 401);
        let h2 = login(&mut s, "bob"); // reuses slot 0 with a new generation
        assert_eq!(s.handle("GET", "/rooms", &h, "").0, 401);
        assert_eq!(s.handle("GET", "/rooms", &h2, "").0, 200);
    }

    #[test]
    fn error_codes_and_rollback() {
        let mut s = server();
        let h = login(&mut s, "ann");
        assert_eq!(s.handle("POST", "/rooms", &h, "{\"name\":\"123456789\"}").0, 413); // string<8>: rolled back, no ghost room
        assert_eq!(s.handle("POST", "/rooms", &h, "{}").0, 400);
        assert_eq!(s.handle("POST", "/rooms", &h, "{\"name\":\"a\"}").0, 201);
        assert_eq!(s.handle("POST", "/rooms", &h, "{\"name\":\"b\"}").0, 201);
        assert_eq!(s.handle("POST", "/rooms", &h, "{\"name\":\"c\"}").0, 503);
        assert_eq!(s.handle("GET", "/rooms/7/messages", &h, "").0, 404);
        assert_eq!(s.handle("GET", "/rooms/x/messages", &h, "").0, 400);
        let long = "x".repeat(2001);
        assert_eq!(s.handle("POST", "/rooms/0/messages", &h, &format!("{{\"body\":\"{}\"}}", long)).0, 413);
        let (_, b) = s.handle("GET", "/stats", &[], "");
        assert!(b.contains("\"next_seq\":0"), "sequence number was rolled back: {}", b);
    }

    #[test]
    fn room_list_drops_the_oldest_and_lists_newest_first() {
        let mut s = server();
        let h = login(&mut s, "ann");
        s.handle("POST", "/rooms", &h, "{\"name\":\"r\"}");
        for i in 1..=8 { assert_eq!(s.handle("POST", "/rooms/0/messages", &h, &format!("{{\"body\":\"m{}\"}}", i)).0, 201); }
        let (c, b) = s.handle("GET", "/rooms/0/messages?limit=10", &h, "");
        assert_eq!(c, 200);
        assert!(b.contains("\"body\":\"m8\"") && b.contains("\"body\":\"m4\"") && !b.contains("\"body\":\"m3\""), "{}", b);
        assert!(b.find("m8").unwrap() < b.find("m7").unwrap());
        assert!(b.contains("\"author\":\"ann\""));
    }

    #[test]
    fn deleted_author_shows_as_deleted_instead_of_failing_the_listing() {
        let mut s = server();
        let a = login(&mut s, "ann");
        let b = login(&mut s, "bob");
        s.handle("POST", "/rooms", &a, "{\"name\":\"r\"}");
        s.handle("POST", "/rooms/0/messages", &a, "{\"body\":\"hi\"}");
        s.handle("DELETE", "/session", &a, "");
        let (c, body) = s.handle("GET", "/rooms/0/messages", &b, "");
        assert_eq!(c, 200);
        assert!(body.contains("[deleted]"), "{}", body);
    }

    #[test]
    fn rate_limit_and_typing_upsert_and_expiry() {
        let mut s = server();
        let h = login(&mut s, "ann");
        s.handle("POST", "/rooms", &h, "{\"name\":\"r\"}");
        for _ in 0..30 { assert_eq!(s.handle("POST", "/rooms/0/messages", &h, "{\"body\":\"x\"}").0, 201); }
        assert_eq!(s.handle("POST", "/rooms/0/messages", &h, "{\"body\":\"x\"}").0, 429);
        s.clock = Some(1000 + 61_000);
        assert_eq!(s.handle("POST", "/rooms/0/messages", &h, "{\"body\":\"x\"}").0, 201);
        s.handle("POST", "/rooms/0/typing", &h, "");
        s.handle("POST", "/rooms/0/typing", &h, ""); // updates, does not add a second entry
        let (_, b) = s.handle("GET", "/rooms/0/typing", &h, "");
        assert_eq!(b.matches("ann").count(), 1, "{}", b);
        s.clock = Some(1000 + 61_000 + 6_000);
        assert!(!s.handle("GET", "/rooms/0/typing", &h, "").1.contains("ann"));
    }

    #[test]
    fn functions_and_vars() {
        let inst = Instance::start(Program::parse("struct C { n: i64; } global c: C;").unwrap(), 1 << 20).unwrap();
        let api = "fn double(x: i64) -> i64 { return x * 2; }
            fn sum_to(n) { var t = 0; var i = 1; if n > 0 { t = n + sum_to(n - 1); } return t; }
            endpoint GET /t { var a = double(21); a = a + 1; c.n = sum_to(10); respond 200 { \"a\": a, \"s\": c.n }; }
            endpoint GET /bad { let x = 1; x = 2; respond 200 { \"x\": x }; }";
        let mut s = Server::new(inst, api).unwrap();
        assert_eq!(s.handle("GET", "/t", &[], ""), (200, "{\"a\":43,\"s\":55}".to_string()));
        assert_eq!(s.handle("GET", "/bad", &[], "").0, 500);
    }
}
