//! Runs endpoints against declared state: an interpreter, a small HTTP/1.1 server, and an in-process benchmark.
//!
//! Each request is atomic: the runtime records every write, and a failed request is rolled back; frees happen only when
//! the request succeeds. Each request also has an epoch: its strings live in a fixed text area and its response in a
//! fixed output area, both carved from a declared reservation and emptied per request. Locals, arguments and item
//! lists use buffers kept from earlier requests, so a warm request does no heap allocation.
use crate::lang::*;
use crate::layout::{CKind, Ty};
use crate::os;
use crate::region::{Scratch, Space};
use crate::runtime::{Error, Instance, Loc, Pressure, Val};
use std::borrow::Cow;
use std::cell::Cell;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::time::Instant;

/// A string in the request's text area.
#[derive(Clone, Copy, Debug)]
struct Txt { off: u32, len: u32 }

#[derive(Clone, Debug)]
enum Value {
    Null, Int(i64), Float(f64), Bool(bool), Str(Txt),
    Item(usize, usize),            // collection, item index
    Coll(usize, usize),            // collection, instance
    Part(usize, usize, Txt),       // part of an item: a nested struct's field path prefix
    ArrField(usize, usize, usize), // an array column of an item
    Plain(usize, Ty),              // a plain struct or fixed array at an offset
}

enum Ctl<'a> { Done(u16), Fail(u16, Cow<'a, str>), Return(Value), Err(Error), Dead, Forward(usize, i64) }
impl<'a> From<Error> for Ctl<'a> { fn from(e: Error) -> Self { Ctl::Err(e) } }
type R<'a, T> = Result<T, Ctl<'a>>;
fn fail<'a, T>(code: u16, m: impl Into<Cow<'a, str>>) -> R<'a, T> { Err(Ctl::Fail(code, m.into())) }

/// A fixed block from the declared request epoch, filled from the start and emptied per request.
struct Area { ptr: *mut u8, cap: usize, len: usize }
impl Area {
    fn push(&mut self, b: &[u8]) -> Option<Txt> {
        if self.len + b.len() > self.cap { return None; }
        unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), self.ptr.add(self.len), b.len()) };
        let t = Txt { off: self.len as u32, len: b.len() as u32 };
        self.len += b.len(); Some(t)
    }
    /// Copy an earlier string to the end (for concatenation).
    fn dup(&mut self, t: Txt) -> Option<()> {
        if self.len + t.len as usize > self.cap { return None; }
        unsafe { std::ptr::copy_nonoverlapping(self.ptr.add(t.off as usize), self.ptr.add(self.len), t.len as usize) };
        self.len += t.len as usize; Some(())
    }
    fn get(&self, t: Txt) -> &[u8] { unsafe { std::slice::from_raw_parts(self.ptr.add(t.off as usize), t.len as usize) } }
    fn all(&self) -> &[u8] { unsafe { std::slice::from_raw_parts(self.ptr, self.len) } }
    fn since(&self, start: usize) -> Txt { Txt { off: start as u32, len: (self.len - start) as u32 } }
}
impl std::fmt::Write for Area { fn write_str(&mut self, s: &str) -> std::fmt::Result { self.push(s.as_bytes()).map(|_| ()).ok_or(std::fmt::Error) } }

/// Write `s` as a JSON string.
/// A float as JSON: the shortest text that reads back as the same number, never with an exponent; JSON has no
/// infinity or NaN, so those are written as null.
fn json_f64(out: &mut Area, f: f64) -> bool { if f.is_finite() { write!(out, "{}", f).is_ok() } else { out.push(b"null").is_some() } }

pub(crate) fn escaped(out: &mut Area, s: &[u8]) -> bool {
    if out.push(b"\"").is_none() { return false; }
    let mut start = 0;
    let mut i = 0;
    // Skip 8 bytes at a time while none of them is a quote, a backslash or a control character
    // (the usual "has a zero byte" and "has a byte below n" tricks on a u64).
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let special = |v: u64| { let q = v ^ (ONES * b'"' as u64); let b = v ^ (ONES * b'\\' as u64); ((q.wrapping_sub(ONES) & !q) | (b.wrapping_sub(ONES) & !b) | (v.wrapping_sub(ONES * 0x20) & !v)) & HIGH != 0 };
    while i < s.len() {
        if i + 8 <= s.len() && !special(u64::from_le_bytes(s[i..i + 8].try_into().unwrap())) { i += 8; continue; }
        let c = s[i];
        i += 1;
        let esc: &[u8] = match c { b'"' => b"\\\"", b'\\' => b"\\\\", b'\n' => b"\\n", b'\r' => b"\\r", b'\t' => b"\\t", c if c < 0x20 => b"", _ => continue };
        let i = i - 1;
        if out.push(&s[start..i]).is_none() { return false; }
        if esc.is_empty() { const HEX: &[u8] = b"0123456789abcdef"; if out.push(&[b'\\', b'u', b'0', b'0', HEX[(c >> 4) as usize], HEX[(c & 15) as usize]]).is_none() { return false; } }
        else if out.push(esc).is_none() { return false; }
        start = i + 1;
    }
    out.push(&s[start..]).is_some() && out.push(b"\"").is_some()
}

/// Flat JSON object of strings, numbers, booleans and null, with strings copied into the text area. Nested values end parsing.
fn parse_json(s: &[u8], text: &mut Area, out: &mut Vec<(Txt, Value)>) -> bool {
    out.clear();
    let mut i = 0;
    let ws = |i: &mut usize| while *i < s.len() && s[*i].is_ascii_whitespace() { *i += 1; };
    ws(&mut i);
    if s.get(i) != Some(&b'{') { return true; }
    i += 1;
    let string = |i: &mut usize, text: &mut Area| -> Option<Txt> {
        if s.get(*i) != Some(&b'"') { return None; }
        *i += 1;
        let start = text.len;
        while *i < s.len() && s[*i] != b'"' {
            if s[*i] == b'\\' && *i + 1 < s.len() {
                *i += 1;
                if s[*i] == b'u' {
                    let hex = |at: usize| s.get(at..at + 4).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u32::from_str_radix(h, 16).ok());
                    let mut cp = hex(*i + 1)?; *i += 4;
                    if (0xD800..0xDC00).contains(&cp) && s.get(*i + 1..*i + 3) == Some(b"\\u".as_slice()) { // surrogate pair
                        if let Some(lo) = hex(*i + 3).filter(|lo| (0xDC00..0xE000).contains(lo)) { cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00); *i += 6; }
                    }
                    let mut b = [0u8; 4];
                    text.push(char::from_u32(cp).unwrap_or('\u{FFFD}').encode_utf8(&mut b).as_bytes())?;
                } else {
                    let c = match s[*i] { b'n' => b'\n', b't' => b'\t', b'r' => b'\r', b'b' => 8, b'f' => 12, c => c };
                    text.push(&[c])?;
                }
            } else {
                let run = *i; while *i < s.len() && s[*i] != b'"' && s[*i] != b'\\' { *i += 1; }
                text.push(&s[run..*i])?; continue;
            }
            *i += 1;
        }
        *i += 1;
        Some(text.since(start))
    };
    loop {
        ws(&mut i);
        let Some(k) = string(&mut i, text) else { break };
        ws(&mut i); if s.get(i) != Some(&b':') { break; } i += 1; ws(&mut i);
        let v = match s.get(i) {
            Some(b'"') => match string(&mut i, text) { Some(t) => Value::Str(t), None => return false },
            Some(c) if c.is_ascii_digit() || *c == b'-' => {
                let st = i; i += 1; while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'.') { i += 1; }
                let t = std::str::from_utf8(&s[st..i]).unwrap_or("");
                match t.parse::<i64>() { Ok(n) => Value::Int(n), Err(_) => t.parse::<f64>().map(Value::Float).unwrap_or(Value::Null) }
            }
            Some(b't') => { i += 4; Value::Bool(true) }
            Some(b'f') => { i += 5; Value::Bool(false) }
            Some(b'n') => { i += 4; Value::Null }
            _ => break,
        };
        out.push((k, v));
        ws(&mut i);
        if s.get(i) == Some(&b',') { i += 1; } else { break; }
    }
    true
}

/// Request headers or forwarded fields: looked up by name, case-insensitively.
pub trait Fields { fn field(&self, k: &[u8]) -> Option<&str>; }
/// Owned (name, value) pairs, as in tests and the socket front end.
pub struct OwnedFields<'x>(pub &'x [(String, String)]);
impl Fields for OwnedFields<'_> { fn field(&self, k: &[u8]) -> Option<&str> { self.0.iter().find(|h| h.0.as_bytes().eq_ignore_ascii_case(k)).map(|h| h.1.as_str()) } }
/// Borrowed (name, value) pairs, as decoded from a mailbox message.
pub struct StrFields<'x, 'y>(pub &'x [(&'y str, &'y str)]);
impl Fields for StrFields<'_, '_> { fn field(&self, k: &[u8]) -> Option<&str> { self.0.iter().find(|h| h.0.as_bytes().eq_ignore_ascii_case(k)).map(|h| h.1) } }
const NO_FIELDS: StrFields<'static, 'static> = StrFields(&[]);

/// Who this server is inside a system of workers: every worker kind (name, count) and its own (kind, index).
#[derive(Clone)]
pub struct SysInfo { pub kinds: Vec<(String, usize)>, pub me: (usize, usize) }

/// Buffers reused by every request (allocated at startup, grown only if a request needs more).
struct Bufs { vars: Vec<Value>, args: Vec<Value>, frees: Vec<(usize, usize)>, idx: Vec<Vec<usize>>, keyed: Vec<Vec<(f64, usize)>>, params: Vec<(usize, Txt)>, query: Vec<(Txt, Txt)>, json: Vec<(Txt, Value)>, sel: Vec<(f64, u32)> }
impl Bufs {
    /// Placeholder while a request holds the real buffers (allocates nothing).
    fn empty() -> Bufs { Bufs { vars: Vec::new(), args: Vec::new(), frees: Vec::new(), idx: Vec::new(), keyed: Vec::new(), params: Vec::new(), query: Vec::new(), json: Vec::new(), sel: Vec::new() } }
    fn startup() -> Bufs {
        Bufs { vars: Vec::with_capacity(256), args: Vec::with_capacity(64), frees: Vec::with_capacity(64), idx: (0..8).map(|_| Vec::with_capacity(1024)).collect(),
            keyed: (0..4).map(|_| Vec::with_capacity(1024)).collect(), params: Vec::with_capacity(8), query: Vec::with_capacity(16), json: Vec::with_capacity(32), sel: Vec::with_capacity(4096) }
    }
}

pub struct Server {
    pub inst: Instance,
    pub api: Api,
    out: Area,
    text: Area,
    bufs: Bufs,
    pub clock: Option<i64>, // fixed time in ms, for tests and benchmarks; otherwise the system clock
    /// A random seed for `random()`, so two servers can be compared; 0 (the default) uses the operating system.
    pub rng: std::cell::Cell<u64>,
    pub pressure: Option<Pressure>,
    pub system: Option<SysInfo>,
    /// Set when the last request forwarded: (worker kind, key). Its writes stay uncommitted until `finish_forward`.
    pending: Option<(usize, i64)>,
    pending_frees: Vec<(usize, usize)>,
    /// Endpoints compiled to native code (see `compile`); `None` entries stay interpreted.
    compiled: Option<Compiled>,
    fwd_out: Vec<(Txt, Txt)>, // the forwarded fields, in the text area
    _scratch: Scratch,
    _space: Space,
}

pub const RESPONSE_CAP: usize = 256 * 1024;
pub const TEXT_CAP: usize = 256 * 1024;

impl Server {
    pub fn new(inst: Instance, api_src: &str) -> Result<Server, String> {
        let mut api = parse(api_src)?;
        bind(&mut api, &|n| inst.prog.global(n))?; // every name resolved once, before the first request
        let mut space = Space::new(RESPONSE_CAP + TEXT_CAP + 8192);
        let region = space.region(RESPONSE_CAP + TEXT_CAP + 4096, 64 * 1024).map_err(|_| "request epoch region".to_string())?;
        let mut scratch = Scratch::new(region);
        let out = scratch.alloc(RESPONSE_CAP).map_err(|_| "response area".to_string())?;
        let text = scratch.alloc(TEXT_CAP).map_err(|_| "text area".to_string())?;
        Ok(Server { inst, api, out: Area { ptr: out, cap: RESPONSE_CAP, len: 0 }, text: Area { ptr: text, cap: TEXT_CAP, len: 0 }, bufs: Bufs::startup(), clock: None, rng: std::cell::Cell::new(0), pressure: None, system: None, pending: None, pending_frees: Vec::with_capacity(64), compiled: None, fwd_out: Vec::with_capacity(16), _scratch: scratch, _space: space })
    }

    /// The last response body.
    pub fn response(&self) -> &str { std::str::from_utf8(self.out.all()).unwrap_or("") }

    /// Handle one request and return its status and body.
    pub fn handle(&mut self, method: &str, target: &str, headers: &[(String, String)], body: &str) -> (u16, String) {
        let c = self.handle_raw(method, target, headers, body);
        (c, self.response().to_string())
    }

    fn route(&self, method: &str, path: &str) -> Option<usize> {
        let n = path.split('/').filter(|s| !s.is_empty()).count();
        'eps: for (i, e) in self.api.eps.iter().enumerate() {
            if e.method != method || e.segs.len() != n { continue; }
            for (s, p) in e.segs.iter().zip(path.split('/').filter(|s| !s.is_empty())) { if let Seg::Lit(l) = s { if l != p { continue 'eps; } } }
            return Some(i);
        }
        None
    }
    fn out_err(out: &mut Area, m: &str) { out.len = 0; let _ = out.push(b"{\"error\":"); let _ = escaped(out, m.as_bytes()); let _ = out.push(b"}"); }

    /// Handle one request; the body is left in the response area (see `response`).
    pub fn handle_raw(&mut self, method: &str, target: &str, headers: &[(String, String)], body: &str) -> u16 {
        let code = self.handle_req(method, target, &OwnedFields(headers), body, &NO_FIELDS);
        if code == 0 { self.finish_forward(false); Self::out_err(&mut self.out, "forward needs a system of workers"); return 500; }
        code
    }

    /// Handle one request with forwarded fields. Returns 0 if the handler forwarded: then the caller sends the request on and
    /// calls `finish_forward` with whether the send worked, which commits or rolls back this handler's writes.
    pub fn handle_req(&mut self, method: &str, target: &str, headers: &dyn Fields, body: &str, fwd: &dyn Fields) -> u16 {
        self.out.len = 0; self.text.len = 0; // the request epoch starts empty
        let (path, qs) = target.split_once('?').unwrap_or((target, ""));
        let Some(ei) = self.route(method, path) else { Self::out_err(&mut self.out, "no such endpoint"); return 404 };
        if self.pressure.as_ref().map_or(false, |p| p.refusing) && method != "GET" && method != "DELETE" { Self::out_err(&mut self.out, "memory pressure: try again later"); return 503; }
        let cfn = self.compiled.as_ref().and_then(|c| c.eps[ei]);
        let mut b = std::mem::replace(&mut self.bufs, Bufs::empty());
        let Server { inst, api, out, text, clock, rng, .. } = self;
        let ep = &api.eps[ei];
        b.params.clear(); b.query.clear();
        let mut fits = true;
        for (k, (s, p)) in ep.segs.iter().zip(path.split('/').filter(|s| !s.is_empty())).enumerate() {
            if let Seg::Param(_) = s { match text.push(p.as_bytes()) { Some(t) => b.params.push((k, t)), None => fits = false } }
        }
        for kv in qs.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            match (text.push(k.as_bytes()), text.push(v.as_bytes())) { (Some(k), Some(v)) => b.query.push((k, v)), _ => fits = false }
        }
        fits &= parse_json(body.as_bytes(), text, &mut b.json);
        if !fits { Self::out_err(out, "request too large for the request text area"); self.bufs = b; return 413; }
        let now = clock.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0));
        inst.begin();
        let mut run = Run {
            inst, api, ep, headers, fwd, rng, sys: self.system.as_ref(), fwd_out: &mut self.fwd_out, params: &b.params, query: &b.query, json: &b.json, text, out, now,
            vars: { let mut v = std::mem::take(&mut b.vars); v.clear(); v.resize(ep.slots as usize, Value::Null); v }, frame: 0, depth: 0,
            args: std::mem::take(&mut b.args), frees: std::mem::take(&mut b.frees), idx: std::mem::take(&mut b.idx), keyed: std::mem::take(&mut b.keyed),
            sel: { let mut v = std::mem::take(&mut b.sel); v.clear(); v }, cerr: None, panicked: false,
        };
        let res = match cfn {
            None => run.exec(&ep.body),
            Some(f) => {
                let mut ctx = Ctx { base: run.inst.base_ptr(), run: &mut run as *mut Run as *mut c_void, h: &HELPERS };
                let r = unsafe { f(&mut ctx) };
                if run.panicked { panic!("a runtime helper panicked under compiled code"); } // the worker's supervisor restarts it
                match r { 0 => Ok(()), x if x > 0 => Err(Ctl::Done(x as u16)), _ => Err(run.cerr.take().unwrap_or(Ctl::Fail(500, Cow::Borrowed("compiled code failed without a reason")))) }
            }
        };
        let (code, ok) = match res {
            Ok(()) | Err(Ctl::Return(_)) => { run.out.len = 0; (204, true) }
            Err(Ctl::Done(code)) => (code, true),
            Err(Ctl::Fail(code, m)) => { Self::out_err(run.out, &m); (code, false) }
            Err(Ctl::Err(Error::Full(m))) => { Self::out_err(run.out, &format!("capacity: {}", m)); (503, false) }
            Err(Ctl::Err(Error::TooLong(m))) => { Self::out_err(run.out, &m); (413, false) }
            Err(Ctl::Err(Error::NotLive(m))) | Err(Ctl::Err(Error::Stale(m))) => { Self::out_err(run.out, &m); (404, false) }
            Err(Ctl::Err(Error::Bad(m))) => { Self::out_err(run.out, &m); (400, false) }
            Err(Ctl::Dead) => { Self::out_err(run.out, "reference is not live"); (404, false) }
            Err(Ctl::Forward(kind, key)) => { run.out.len = 0; self.pending = Some((kind, key)); (0, true) }
        };
        let Run { vars, args, mut frees, idx, keyed, sel, .. } = run;
        b.vars = vars; b.args = args; b.args.clear(); b.idx = idx; b.keyed = keyed; b.sel = sel; // ends the borrow of the endpoint code
        if code == 0 { self.pending_frees.clear(); self.pending_frees.extend_from_slice(&frees); } // decided by finish_forward
        else if ok { for &(c, i) in &frees { let _ = self.inst.free_item(c, i); } self.inst.commit_changes(); } else { self.inst.rollback(); }
        frees.clear(); b.frees = frees;
        self.bufs = b;
        if code != 0 { if let Some(p) = self.pressure.as_mut() { self.inst.relieve(p); } }
        code
    }

    /// Where the last request was forwarded: (worker kind, key), if it was.
    pub fn pending_forward(&self) -> Option<(usize, i64)> { self.pending }
    /// The forwarded fields of the last request, as (name, value) bytes.
    pub fn forward_fields(&self) -> impl Iterator<Item = (&[u8], &[u8])> + '_ { self.fwd_out.iter().map(move |(k, v)| (self.text.get(*k), self.text.get(*v))) }
    /// Commit the forwarding handler's writes if the request was handed on, or roll them back if it could not be.
    pub fn finish_forward(&mut self, sent: bool) {
        if self.pending.take().is_none() { return; }
        if sent { for k in 0..self.pending_frees.len() { let (c, i) = self.pending_frees[k]; let _ = self.inst.free_item(c, i); } self.inst.commit_changes(); }
        else { self.inst.rollback(); }
        self.pending_frees.clear();
        if let Some(p) = self.pressure.as_mut() { self.inst.relieve(p); }
    }

    /// Quiet time: release slack (see `Instance::idle`).
    pub fn idle(&mut self) { if let Some(p) = self.pressure.as_mut() { self.inst.idle(p); } }
}

struct Run<'a> {
    inst: &'a mut Instance, api: &'a Api, ep: &'a Endpoint, headers: &'a dyn Fields, fwd: &'a dyn Fields,
    sys: Option<&'a SysInfo>, fwd_out: &'a mut Vec<(Txt, Txt)>,
    params: &'a [(usize, Txt)], query: &'a [(Txt, Txt)], json: &'a [(Txt, Value)],
    text: &'a mut Area, out: &'a mut Area, now: i64,
    vars: Vec<Value>, // frames of local slots: the endpoint's, then one per active function call
    frame: usize, depth: usize,
    args: Vec<Value>, frees: Vec<(usize, usize)>, idx: Vec<Vec<usize>>, keyed: Vec<Vec<(f64, usize)>>,
    sel: Vec<(f64, u32)>, // items selected by compiled loops that collect before running their body: (order key, item)
    rng: &'a std::cell::Cell<u64>,
    cerr: Option<Ctl<'a>>, // an error a helper stored for compiled code
    panicked: bool,
}

fn from_scalar(v: Val) -> Value { match v { Val::Int(i) => Value::Int(i), Val::Float(f) => Value::Float(f), Val::Bool(b) => Value::Bool(b), _ => Value::Null } }

impl<'a> Run<'a> {
    fn txt(&mut self, b: &[u8]) -> R<'a, Txt> { self.text.push(b).ok_or(Ctl::Fail(507, Cow::Borrowed("request text area is full"))) }
    /// The session item named by the request's `X-Session` token, checked against pool `c`.
    fn auth_item(&self, c: usize) -> R<'a, usize> {
        let Some(tok) = self.headers.field(b"x-session") else { return fail(401, "missing X-Session header") };
        let mut it = tok.split('.');
        if let Some(s) = self.sys { if it.next().and_then(|w| w.parse::<usize>().ok()) != Some(s.me.1) { return fail(401, "session token belongs to another shard"); } }
        let (Some(Ok(slot)), Some(Ok(gen)), Some(Ok(sec))) = (it.next().map(str::parse::<usize>), it.next().map(str::parse::<u32>), it.next().map(|h| u64::from_str_radix(h, 16))) else { return fail(401, "malformed session token") };
        let Some(idx) = self.inst.deref_quiet(c, (slot, gen)) else { return fail(401, "session ended") };
        let Some(ci) = self.inst.col(c, "secret") else { return fail(500, "auth needs a `secret` field") };
        let Val::Int(real) = self.inst.field_get(c, idx, ci)? else { return fail(500, "`secret` must be an integer") };
        if real as u64 != sec { return fail(403, "bad session secret"); }
        Ok(idx)
    }
    fn slot(&self, s: u32) -> Value { self.vars[self.frame + s as usize].clone() }
    fn set_slot(&mut self, s: u32, v: Value) { let i = self.frame + s as usize; self.vars[i] = v; }
    /// Where field `f` is in collection `c`: from the access site's cache, or looked up once and remembered there.
    fn hit(&self, c: usize, f: &str, cache: &Cell<FieldHit>) -> FieldHit {
        match cache.get() {
            h @ FieldHit::Col(hc, _) | h @ FieldHit::Child(hc, _) if hc as usize == c => h,
            _ => {
                let h = match (self.inst.col(c, f), self.inst.child(c, f)) { (Some(ci), _) => FieldHit::Col(c as u32, ci as u32), (None, Some(k)) => FieldHit::Child(c as u32, k as u32), _ => FieldHit::None };
                if h != FieldHit::None { cache.set(h); }
                h
            }
        }
    }
    fn arg(&self, base: usize, i: usize) -> Value { self.args.get(base + i).cloned().unwrap_or(Value::Null) }
    fn arg_txt(&self, base: usize, i: usize) -> R<'a, Txt> { match self.args.get(base + i) { Some(Value::Str(t)) => Ok(*t), _ => fail(500, "expected a string argument") } }

    /// A string location's bytes, copied into the text area (values have value semantics).
    fn read_str(&mut self, l: &Loc) -> R<'a, Value> {
        let b = self.inst.str_bytes(l).unwrap_or(&[]);
        self.text.push(b).map(Value::Str).ok_or(Ctl::Fail(507, Cow::Borrowed("request text area is full")))
    }
    fn follow(&self, v: Val, target: usize) -> R<'a, Value> {
        match v { Val::Ref(None) => Ok(Value::Null), Val::Ref(Some(r)) => self.inst.deref_quiet(target, r).map(|i| Value::Item(target, i)).ok_or(Ctl::Dead), o => Ok(from_scalar(o)) }
    }
    fn ref_target(&self, l: &Loc) -> Option<usize> {
        match l {
            Loc::Field(c, _, ci) => match self.inst.col_kind(*c, *ci) { CKind::Ref(t) => Some(*t), _ => None },
            Loc::Plain(_, Ty::Ref(n)) => self.inst.prog.coll_of(n),
            _ => None,
        }
    }
    /// Turn a location into a value: read scalars and strings, follow references, keep structs, collections and arrays as places.
    fn loc_value(&mut self, l: Loc) -> R<'a, Value> {
        match l {
            Loc::Coll(c, i) => Ok(Value::Coll(c, i)),
            Loc::Item(c, i, p) => if p.is_empty() { Ok(Value::Item(c, i)) } else { let t = self.txt(p.as_bytes())?; Ok(Value::Part(c, i, t)) },
            Loc::Field(c, i, ci) if matches!(self.inst.col_kind(c, ci), CKind::Arr(..)) => Ok(Value::ArrField(c, i, ci)),
            Loc::Plain(off, t @ (Ty::Struct(_) | Ty::Arr(..))) => Ok(Value::Plain(off, t)),
            l if self.inst.is_str(&l) => self.read_str(&l),
            l => { let t = self.ref_target(&l); let v = self.inst.loc_get(&l)?; match t { Some(t) => self.follow(v, t), None => Ok(from_scalar(v)) } }
        }
    }
    /// What a loop variable holds: the item, or for a collection of plain values, the value.
    fn item_value(&mut self, c: usize, idx: usize) -> R<'a, Value> { if self.inst.prog.colls[c].cols.len() == 1 && self.inst.col(c, "").is_some() || self.inst.child(c, "").is_some() { let l = self.inst.item_loc(c, idx); self.loc_value(l) } else { Ok(Value::Item(c, idx)) } }
    fn part_loc(&self, c: usize, i: usize, p: Txt) -> Loc { Loc::Item(c, i, String::from_utf8_lossy(self.text.get(p)).into_owned()) }
    fn field(&mut self, base: Value, f: &str, cache: &Cell<FieldHit>) -> R<'a, Value> {
        match base {
            Value::Item(c, i) => {
                let h = self.hit(c, f, cache);
                if let FieldHit::Child(_, k) = h { return Ok(Value::Coll(k as usize, i)); } // a nested collection of this item
                if let FieldHit::Col(_, ci) = h { // the common case: one column, no path building
                    let ci = ci as usize;
                    return match self.inst.col_kind(c, ci) {
                        CKind::Arr(..) => Ok(Value::ArrField(c, i, ci)),
                        CKind::StrLen => self.read_str(&Loc::Field(c, i, ci)),
                        CKind::Ref(t) => { let t = *t; let v = self.inst.field_get(c, i, ci)?; self.follow(v, t) }
                        _ => Ok(from_scalar(self.inst.field_get(c, i, ci)?)),
                    };
                }
                let l = self.inst.step_field(Loc::Item(c, i, String::new()), f)?;
                self.loc_value(l)
            }
            Value::Part(c, i, p) => { let l = self.inst.step_field(self.part_loc(c, i, p), f)?; self.loc_value(l) }
            Value::Plain(off, t) => { let l = self.inst.step_field(Loc::Plain(off, t), f)?; self.loc_value(l) }
            Value::Null => Err(Ctl::Dead),
            _ => fail(500, format!("`.{}` needs an item or a state struct", f)),
        }
    }
    fn index(&mut self, base: Value, k: Value) -> R<'a, Value> {
        let Value::Int(k) = k else { return fail(400, "an index must be an integer") };
        if k < 0 { return Err(Error::NotLive(format!("index {} is negative", k)).into()); }
        let l = match base {
            Value::Coll(c, i) => Loc::Coll(c, i),
            Value::Plain(off, t) => Loc::Plain(off, t),
            Value::ArrField(c, i, ci) => Loc::Field(c, i, ci),
            _ => return fail(500, "cannot index this value"),
        };
        let l = self.inst.step_index(l, k as usize, None)?;
        self.loc_value(l)
    }
    /// The place an assignment writes to.
    fn place(&mut self, e: &'a Expr) -> R<'a, Loc> {
        match e {
            Expr::Field(b, f, _) => match self.eval(b)? {
                Value::Item(c, i) => Ok(match self.inst.col(c, f) { Some(ci) => Loc::Field(c, i, ci), None => self.inst.step_field(Loc::Item(c, i, String::new()), f)? }),
                Value::Part(c, i, p) => Ok(self.inst.step_field(self.part_loc(c, i, p), f)?),
                Value::Plain(off, t) => Ok(self.inst.step_field(Loc::Plain(off, t), f)?),
                Value::Null => Err(Ctl::Dead),
                _ => fail(500, "assignment target is not an item or a state struct"),
            },
            Expr::Index(b, k) => {
                let (bv, kv) = (self.eval(b)?, self.eval(k)?);
                let Value::Int(k) = kv else { return fail(400, "an index must be an integer") };
                let l = match bv { Value::ArrField(c, i, ci) => Loc::Field(c, i, ci), Value::Plain(off, t) => Loc::Plain(off, t), _ => return fail(500, "only fixed arrays can be assigned by index") };
                if k < 0 { return Err(Error::NotLive(format!("index {} is negative", k)).into()); }
                Ok(self.inst.step_index(l, k as usize, None)?)
            }
            _ => fail(500, "cannot assign to this"),
        }
    }
    fn store(&mut self, l: Loc, v: Value) -> R<'a, ()> {
        let val = match (self.ref_target(&l), v) {
            (None, Value::Str(t)) => { let b = self.text.get(t); return Ok(self.inst.store_str_bytes(&l, b)?); }
            (Some(_), Value::Null) => Val::Ref(None),
            (Some(t), Value::Item(c, i)) if c == t => Val::Ref(Some((i, self.inst.gen(c, i).unwrap_or(0)))),
            (Some(_), _) => return fail(500, "this field holds a reference to a different pool"),
            (None, Value::Int(i)) => Val::Int(i), (None, Value::Float(f)) => Val::Float(f), (None, Value::Bool(b)) => Val::Bool(b),
            (None, _) => return fail(500, "cannot store this value in state"),
        };
        Ok(self.inst.loc_set(&l, &val)?)
    }

    fn truthy(&self, v: &Value) -> R<'a, bool> { match v { Value::Bool(b) => Ok(*b), Value::Null => Ok(false), _ => fail(500, "condition is not a boolean") } }
    fn num(v: &Value) -> Option<f64> { match v { Value::Int(i) => Some(*i as f64), Value::Float(f) => Some(*f), _ => None } }

    fn eval(&mut self, e: &'a Expr) -> R<'a, Value> {
        match e {
            Expr::Int(n) => Ok(Value::Int(*n)),
            Expr::Float(f) => Ok(Value::Float(*f)),
            Expr::Str(s) => Ok(Value::Str(self.txt(s.as_bytes())?)),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            Expr::Var(n, res) => {
                let g = match res { VarRes::Local(s) => return Ok(self.slot(*s)), VarRes::Global(g) => *g as usize, VarRes::Unbound => return fail(500, format!("`{}` was not bound", n)) };
                if let Some(c) = self.inst.prog.globals[g].coll { return Ok(Value::Coll(c, 0)); } // no type clone: a pool's type owns a Box
                let (off, ty) = { let g = &self.inst.prog.globals[g]; (g.off, g.ty.clone()) };
                self.loc_value(Loc::Plain(off, ty))
            }
            Expr::Field(b, f, cache) => { let bv = self.eval(b)?; self.field(bv, f, cache) }
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
            Expr::Call(n, args, res) => self.call(n, *res, args),
        }
    }

    /// Append a value's text to the text area.
    fn show_into(&mut self, v: &Value) -> R<'a, ()> {
        let ok = match v { Value::Str(t) => self.text.dup(*t).is_some(), Value::Int(i) => write!(self.text, "{}", i).is_ok(), Value::Float(f) => write!(self.text, "{}", f).is_ok(), Value::Bool(b) => write!(self.text, "{}", b).is_ok(), Value::Null => self.text.push(b"null").is_some(), _ => return fail(500, "cannot show this value") };
        if ok { Ok(()) } else { fail(507, "request text area is full") }
    }

    fn binop(&mut self, op: &str, l: Value, r: Value) -> R<'a, Value> {
        if op == "==" || op == "!=" {
            let eq = match (&l, &r) {
                (Value::Null, Value::Null) => true, (Value::Null, _) | (_, Value::Null) => false,
                (Value::Str(a), Value::Str(b)) => self.text.get(*a) == self.text.get(*b), (Value::Bool(a), Value::Bool(b)) => a == b,
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
        if op == "+" { if let Value::Str(_) = &l { let start = self.text.len; self.show_into(&l)?; self.show_into(&r)?; return Ok(Value::Str(self.text.since(start))); } }
        if let (Some(a), Some(b)) = (Self::num(&l), Self::num(&r)) {
            return Ok(match op { "+" => Value::Float(a + b), "-" => Value::Float(a - b), "*" => Value::Float(a * b), "/" => Value::Float(a / b),
                "<" => Value::Bool(a < b), "<=" => Value::Bool(a <= b), ">" => Value::Bool(a > b), ">=" => Value::Bool(a >= b), _ => return fail(500, "bad operator") });
        }
        if let (Value::Str(a), Value::Str(b)) = (&l, &r) { let (a, b) = (self.text.get(*a), self.text.get(*b)); return Ok(Value::Bool(match op { "<" => a < b, "<=" => a <= b, ">" => a > b, ">=" => a >= b, _ => return fail(500, "bad operator") })); }
        fail(500, "operator does not fit these values")
    }

    fn call(&mut self, n: &'a str, res: CallRes, args: &'a [Expr]) -> R<'a, Value> {
        // forms that look at their arguments before (or instead of) evaluating them
        match res {
            CallRes::Builtin(B::Or) => {
                let first = match args.first() { Some(e) => self.eval(e), None => return fail(500, "or(a, b)") };
                return match first {
                    Ok(Value::Null) | Err(Ctl::Dead) | Err(Ctl::Err(Error::NotLive(_))) | Err(Ctl::Err(Error::Stale(_))) => match args.get(1) { Some(e) => self.eval(e), None => Ok(Value::Null) },
                    other => other,
                };
            }
            CallRes::Builtin(B::Exists) => return match args.first().map(|e| self.eval(e)) {
                Some(Ok(Value::Null)) | Some(Err(Ctl::Dead)) | Some(Err(Ctl::Err(Error::NotLive(_)))) | Some(Err(Ctl::Err(Error::Stale(_)))) => Ok(Value::Bool(false)),
                Some(Ok(_)) => Ok(Value::Bool(true)), Some(Err(e)) => Err(e), None => fail(500, "exists(x)"),
            },
            CallRes::Builtin(B::MaxLen) => {
                let Some(Expr::Field(b, f, _)) = args.first() else { return fail(500, "maxlen(item.field)") };
                return match self.eval(b)? {
                    Value::Item(c, _) => match self.inst.col(c, f).map(|ci| self.inst.col_kind(c, ci + 1).clone()) { Some(CKind::StrBytes(m)) => Ok(Value::Int(m as i64)), _ => fail(500, "maxlen needs a string field") },
                    Value::Plain(off, t) => match self.inst.step_field(Loc::Plain(off, t), f)? { Loc::Plain(_, Ty::Str(m)) => Ok(Value::Int(m as i64)), _ => fail(500, "maxlen needs a string field") },
                    _ => fail(500, "maxlen(item.field)"),
                };
            }
            _ => {}
        }
        let base = self.args.len();
        let r = self.call_with(n, res, args, base);
        self.args.truncate(base);
        r
    }

    fn call_with(&mut self, n: &'a str, res: CallRes, args: &'a [Expr], base: usize) -> R<'a, Value> {
        for x in args { let v = self.eval(x)?; self.args.push(v); }
        let b = match res { CallRes::User(k) => return self.call_fn(&self.api.fns[k as usize], base), CallRes::Builtin(b) => b, CallRes::Unbound => return fail(500, format!("`{}` was not bound", n)) };
        self.apply(b, base)
    }

    /// A builtin applied to the arguments in `self.args[base..]`. Compiled code calls this too (through the `builtin`
    /// helper), so both run the same code for every builtin it covers.
    fn apply(&mut self, b: B, base: usize) -> R<'a, Value> {
        let key = |me: &Self| -> R<'a, Txt> { me.arg_txt(base, 0) };
        Ok(match b {
            B::Body => { let k = key(self)?; let k = self.text.get(k); self.json.iter().find(|x| self.text.get(x.0) == k).map(|x| x.1.clone()).unwrap_or(Value::Null) }
            B::Param => {
                let k = key(self)?; let k = self.text.get(k);
                let seg = self.ep.segs.iter().position(|s| matches!(s, Seg::Param(p) if p.as_bytes() == k));
                seg.and_then(|si| self.params.iter().find(|p| p.0 == si)).map(|p| Value::Str(p.1)).unwrap_or(Value::Null)
            }
            B::Query => { let k = key(self)?; let k = self.text.get(k); self.query.iter().find(|x| self.text.get(x.0) == k).map(|x| Value::Str(x.1)).unwrap_or(Value::Null) }
            B::Header => {
                let k = key(self)?;
                let found = { let k = self.text.get(k); self.headers.field(k) };
                match found { Some(h) => Value::Str(self.txt(h.as_bytes())?), None => Value::Null }
            }
            B::Int => match self.arg(base, 0) {
                Value::Int(i) => Value::Int(i), Value::Float(f) => Value::Int(f as i64),
                Value::Str(t) => match std::str::from_utf8(self.text.get(t)).ok().and_then(|s| s.trim().parse::<i64>().ok()) { Some(i) => Value::Int(i), None => return fail(400, format!("`{}` is not a number", String::from_utf8_lossy(self.text.get(t)))) },
                Value::Null => return fail(400, "missing number"), _ => return fail(500, "int(x)"),
            },
            B::Str => { let v = self.arg(base, 0); let start = self.text.len; self.show_into(&v)?; Value::Str(self.text.since(start)) }
            B::Len => match self.arg(base, 0) { Value::Str(t) => Value::Int(t.len as i64), Value::Null => Value::Int(0), _ => return fail(500, "len(text)") },
            B::Now => Value::Int(self.now),
            B::Fwd => { let k = key(self)?; let found = { let k = self.text.get(k); self.fwd.field(k) }; match found { Some(v) => Value::Str(self.txt(v.as_bytes())?), None => Value::Null } }
            B::WorkerIndex => Value::Int(self.sys.map_or(0, |s| s.me.1) as i64),
            B::WorkerCount => match self.args.get(base) {
                None => Value::Int(self.sys.map_or(1, |s| s.kinds[s.me.0].1) as i64),
                Some(Value::Str(t)) => { let n = self.text.get(*t); match self.sys.and_then(|s| s.kinds.iter().find(|k| k.0.as_bytes() == n)) { Some(k) => Value::Int(k.1 as i64), None => return fail(500, "no such worker kind") } }
                _ => return fail(500, "worker_count(\"kind\")"),
            },
            B::Crash => panic!("__crash() called: test hook for worker supervision"),
            B::Random => Value::Int(match self.rng.get() { 0 => os::random_u64(), mut x => { x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.rng.set(x); x } } as i64),
            B::Min | B::Max => match (self.arg(base, 0), self.arg(base, 1)) { (Value::Int(p), Value::Int(q)) => Value::Int(if b == B::Min { p.min(q) } else { p.max(q) }), _ => return fail(500, "min/max need integers") },
            B::Count => match self.arg(base, 0) { Value::Coll(c, i) => Value::Int(self.inst.count(c, i) as i64), _ => return fail(500, "count(collection)") },
            B::Capacity => match self.arg(base, 0) { Value::Coll(c, _) => Value::Int(self.inst.cap(c) as i64), _ => return fail(500, "capacity(collection)") },
            B::Slot => match self.arg(base, 0) { Value::Item(c, i) => Value::Int((i % self.inst.cap(c)) as i64), _ => return fail(500, "slot(item)") },
            B::Stat => {
                let k = key(self)?;
                let v = match self.text.get(k) { b"committed_kb" => self.inst.committed() / 1024, b"live_kb" => self.inst.live_total() / 1024, b"reserved_kb" => self.inst.reserved() / 1024, _ => return fail(500, "unknown stat") };
                Value::Int(v as i64)
            }
            B::Token => match self.arg(base, 0) {
                Value::Item(c, i) => {
                    let Some(ci) = self.inst.col(c, "secret") else { return fail(500, "token needs a `secret` field") };
                    let Val::Int(sec) = self.inst.field_get(c, i, ci)? else { return fail(500, "`secret` must be an integer") };
                    let start = self.text.len;
                    let shard = self.sys.map(|s| s.me.1);
                    let r = match shard { Some(w) => write!(self.text, "{}.{}.{}.{:x}", w, i, self.inst.gen(c, i).unwrap_or(0), sec as u64), None => write!(self.text, "{}.{}.{:x}", i, self.inst.gen(c, i).unwrap_or(0), sec as u64) };
                    if r.is_err() { return fail(507, "request text area is full"); }
                    Value::Str(self.text.since(start))
                }
                _ => return fail(500, "token(item)"),
            },
            B::Auth => {
                let Value::Coll(c, 0) = self.arg(base, 0) else { return fail(500, "auth(pool)") };
                Value::Item(c, self.auth_item(c)?)
            }
            B::Or | B::Exists | B::MaxLen => unreachable!("handled before the arguments are evaluated"),
        })
    }

    /// Call a declared function in a fresh frame: it sees its parameters and the state, not the caller's locals.
    fn call_fn(&mut self, f: &'a Func, base: usize) -> R<'a, Value> {
        if self.depth >= 64 { return fail(500, "calls nested more than 64 deep"); }
        let (saved, vbase) = (self.frame, self.vars.len());
        self.vars.resize(vbase + f.slots as usize, Value::Null);
        for k in 0..f.params.len() { self.vars[vbase + k] = self.args[base + k].clone(); } // parameters are the first slots
        self.frame = vbase; self.depth += 1;
        let r = self.exec(&f.body);
        self.vars.truncate(vbase); self.frame = saved; self.depth -= 1;
        match r { Ok(()) => Ok(Value::Null), Err(Ctl::Return(v)) => Ok(v), Err(e) => Err(e) }
    }

    /// Items of `it.coll` passing `where`, in order (reversed if asked), ordered and limited. Give the vector back with `self.idx.push`.
    fn select(&mut self, it: &'a Iter) -> R<'a, (usize, Vec<usize>)> {
        let Value::Coll(c, inst) = self.eval(&it.coll)? else { return fail(500, "each needs a list or pool") };
        let limit = match &it.limit { Some(e) => match self.eval(e)? { Value::Int(n) if n >= 0 => Some(n as usize), _ => return fail(400, "bad limit") }, None => None };
        let mut items = self.idx.pop().unwrap_or_default();
        self.inst.items_into(c, inst, &mut items);
        if it.reverse { items.reverse(); }
        if it.wher.is_none() && it.order.is_none() { if let Some(l) = limit { items.truncate(l); } return Ok((c, items)); }
        let mut hits = self.keyed.pop().unwrap_or_default();
        hits.clear();
        let mut res = Ok(());
        for &idx in items.iter() {
            let v = self.item_value(c, idx)?; self.set_slot(it.slot, v);
            let keep = match &it.wher { Some(w) => self.eval(w).and_then(|v| self.truthy(&v)), None => Ok(true) };
            let key = match (&keep, &it.order) { (Ok(true), Some((k, _))) => match self.eval(k) { Ok(Value::Int(i)) => Ok(i as f64), Ok(Value::Float(f)) => Ok(f), Ok(_) => fail(500, "order by needs a number"), Err(e) => Err(e) }, _ => Ok(0.0) };
            match (keep, key) { (Ok(true), Ok(k)) => hits.push((k, idx)), (Ok(false), _) => continue, (Err(e), _) | (_, Err(e)) => { res = Err(e); break; } }
            if it.order.is_none() && limit.map_or(false, |l| hits.len() >= l) { break; }
        }
        if let Some((_, desc)) = &it.order { let desc = *desc; hits.sort_by(|a, b| { let o = a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal); if desc { o.reverse() } else { o } }); } // stable: ties keep their order
        if let Some(l) = limit { hits.truncate(l); }
        items.clear(); items.extend(hits.iter().map(|h| h.1));
        self.keyed.push(hits);
        match res { Ok(()) => Ok((c, items)), Err(e) => { self.idx.push(items); Err(e) } }
    }

    fn exec(&mut self, stmts: &'a [Stmt]) -> R<'a, ()> {
        for s in stmts { self.stmt(s)?; }
        Ok(())
    }

    fn stmt(&mut self, s: &'a Stmt) -> R<'a, ()> {
        match s {
            Stmt::Let(_, slot, e) | Stmt::Var(_, slot, e) => { let v = self.eval(e)?; self.set_slot(*slot, v); }
            Stmt::Alloc { slot, coll, try_, .. } => {
                let Value::Coll(c, inst) = self.eval(coll)? else { return fail(500, "alloc needs a list or pool") };
                match self.inst.push_item(c, inst) {
                    Ok(idx) => self.set_slot(*slot, Value::Item(c, idx)),
                    Err(Error::Full(_)) if *try_ => self.set_slot(*slot, Value::Null),
                    Err(e) => return Err(e.into()),
                }
            }
            Stmt::Assign(Expr::Var(_, VarRes::Local(s)), rhs) => { let v = self.eval(rhs)?; self.set_slot(*s, v); } // mutability checked by the binder
            Stmt::Assign(lhs, rhs) => { let l = self.place(lhs)?; let v = self.eval(rhs)?; self.store(l, v)?; }
            Stmt::If(c, t, e) => { let v = self.eval(c)?; if self.truthy(&v)? { self.exec(t)?; } else { self.exec(e)?; } }
            Stmt::Each(it, body) => {
                let (c, items) = self.select(it)?;
                let mut res = Ok(());
                for &idx in items.iter() {
                    let v = self.item_value(c, idx)?; self.set_slot(it.slot, v);
                    let r = self.exec(body);
                    if r.is_err() { res = r; break; }
                }
                self.idx.push(items);
                res?;
            }
            Stmt::Free(e) => match self.eval(e)? {
                Value::Item(c, i) => { if self.inst.is_list(c) { return fail(500, "list items are not freed; lists drop their oldest or pop their newest"); } if !self.frees.contains(&(c, i)) { self.frees.push((c, i)); } }
                Value::Null => {}
                _ => return fail(500, "free needs an item"),
            },
            Stmt::Fail(code, m) => return Err(Ctl::Fail(*code, Cow::Borrowed(m.as_str()))),
            Stmt::Respond(code, j) => { self.json(j)?; return Err(Ctl::Done(*code)); }
            Stmt::Return(e) => { let v = match e { Some(e) => self.eval(e)?, None => Value::Null }; return Err(Ctl::Return(v)); }
            Stmt::Do(e) => { self.eval(e)?; }
            Stmt::Forward { kind, key, fields } => {
                let Some(s) = self.sys else { return fail(500, "forward needs a system of workers") };
                let Some(k) = s.kinds.iter().position(|x| x.0 == *kind) else { return fail(500, format!("no worker kind `{}`", kind)) };
                let Value::Int(key) = self.eval(key)? else { return fail(500, "a forward key must be an integer") };
                self.fwd_out.clear();
                for (name, e) in fields {
                    let v = self.eval(e)?;
                    let kt = self.txt(name.as_bytes())?;
                    let start = self.text.len;
                    if !matches!(v, Value::Null) { self.show_into(&v)?; }
                    self.fwd_out.push((kt, self.text.since(start)));
                }
                return Err(Ctl::Forward(k, key));
            }
        }
        Ok(())
    }

    fn out(&mut self, b: &[u8]) -> R<'a, ()> { if self.out.push(b).is_some() { Ok(()) } else { fail(507, "response too large") } }

    fn json(&mut self, j: &'a Json) -> R<'a, ()> {
        match j {
            Json::Obj(fields) => {
                self.out(b"{")?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 { self.out(b",")?; }
                    if !escaped(self.out, k.as_bytes()) { return fail(507, "response too large"); }
                    self.out(b":")?; self.json(v)?;
                }
                self.out(b"}")
            }
            Json::Arr(it, inner) => {
                let (c, items) = self.select(it)?;
                let mut res = self.out(b"[");
                for (n, &idx) in items.iter().enumerate() {
                    if res.is_err() { break; }
                    if n > 0 { if let Err(e) = self.out(b",") { res = Err(e); break; } }
                    let v = self.item_value(c, idx)?; self.set_slot(it.slot, v);
                    res = self.json(inner);
                }
                self.idx.push(items);
                res?;
                self.out(b"]")
            }
            Json::E(e) => {
                let v = match e {
                    Expr::Field(b, f, cache) => {
                        let bv = self.eval(b)?;
                        if let Value::Item(c, i) = bv {
                            if let FieldHit::Col(_, ci) = self.hit(c, f, cache) {
                                let l = Loc::Field(c, i, ci as usize);
                                if let Some(bytes) = self.inst.str_bytes(&l) { return if escaped(self.out, bytes) { Ok(()) } else { fail(507, "response too large") }; }
                            }
                        }
                        self.field(bv, f, cache)?
                    }
                    e => self.eval(e)?,
                };
                let ok = match v {
                    Value::Int(i) => write!(self.out, "{}", i).is_ok(),
                    Value::Float(f) => json_f64(self.out, f),
                    Value::Bool(b) => write!(self.out, "{}", b).is_ok(),
                    Value::Str(t) => escaped(self.out, self.text.get(t)),
                    Value::Null => self.out.push(b"null").is_some(),
                    _ => return fail(500, "cannot write this value as JSON"),
                };
                if ok { Ok(()) } else { fail(507, "response too large") }
            }
        }
    }
}

// ---------------------------------------------------------------- HTTP

pub fn serve(server: &mut Server, port: u16) -> std::io::Result<()> {
    let l = std::net::TcpListener::bind(("127.0.0.1", port))?;
    l.set_nonblocking(true)?;
    println!("listening on http://127.0.0.1:{} (single-threaded; Ctrl-C to stop)", port);
    let mut quiet_ms = 0;
    loop {
        let mut c = match l.accept() {
            Ok((c, _)) => { quiet_ms = 0; c }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(20));
                quiet_ms += 20;
                if quiet_ms >= 1000 { server.idle(); quiet_ms = 0; } // a quiet second: release slack from the largest collection
                continue;
            }
            Err(_) => continue,
        };
        let _ = c.set_nonblocking(false);
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
        let code = server.handle_raw(&method, &target, &headers, &body);
        let reason = match code { 200 => "OK", 201 => "Created", 204 => "No Content", 400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found", 413 => "Payload Too Large", 429 => "Too Many Requests", 503 => "Service Unavailable", 507 => "Insufficient Storage", _ => "Error" };
        let out = server.response();
        let _ = c.write_all(format!("HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", code, reason, out.len()).as_bytes());
        let _ = c.write_all(out.as_bytes());
    }
}

// ---------------------------------------------------------------- benchmark

fn token_of(body: &str) -> String { body.split("\"token\":\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("").to_string() }

struct Stat { name: &'static str, ns: Vec<u128>, codes: std::collections::BTreeMap<u16, usize>, allocs: usize }
impl Stat {
    fn new(name: &'static str) -> Stat { Stat { name, ns: Vec::with_capacity(32768), codes: Default::default(), allocs: 0 } }
    fn row(&mut self) {
        if self.ns.is_empty() { return; }
        self.ns.sort_unstable();
        let n = self.ns.len();
        let mean = self.ns.iter().sum::<u128>() as f64 / n as f64 / 1000.0;
        println!("  {:<34} {:>7} {:>10.1} {:>10.1} {:>10.1} {:>9.2}   {:?}", self.name, n, mean, self.ns[n / 2] as f64 / 1000.0, self.ns[(n * 99 / 100).min(n - 1)] as f64 / 1000.0, self.allocs as f64 / n as f64, self.codes);
    }
}
/// Time one request; inputs are built beforehand so only the handler's own allocations are counted.
fn timed(s: &mut Server, st: &mut Stat, method: &str, target: &str, h: &[(String, String)], body: &str) -> u16 {
    let (a0, t) = (crate::alloc_count::allocs(), Instant::now());
    let c = s.handle_raw(method, target, h, body);
    st.ns.push(t.elapsed().as_nanos());
    st.allocs += crate::alloc_count::allocs() - a0;
    *st.codes.entry(c).or_default() += 1;
    c
}

pub fn bench(s: &mut Server, mixed: usize) {
    let mb = |n: usize| n as f64 / (1 << 20) as f64;
    let snap = |s: &Server, label: &str| println!("  [{:<28}] committed {:>7.2} MB  live {:>7.2} MB  reserved {:>7.2} MB  OS commit {:>7.2} MB", label, mb(s.inst.committed()), mb(s.inst.live_total()), mb(s.inst.reserved()), mb(os::os_mem().private));
    let mut rng = 0x2545F4914F6CDD1Du64;
    let mut rnd = move |n: usize| { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; (rng % n as u64) as usize };
    s.clock = Some(1_000_000);
    let _ = s.inst.resolve("meta.rate").and_then(|l| s.inst.loc_set(&l, &Val::Int(1_000_000))); // lift the per-session rate limit for load
    println!("in-process benchmark: handler calls only, no sockets. Times in microseconds (interpreter-bound); allocs = heap allocations per request.");
    println!("  {:<34} {:>7} {:>10} {:>10} {:>10} {:>9}", "endpoint", "calls", "mean us", "p50 us", "p99 us", "allocs");
    snap(s, "start");
    let names: Vec<String> = (0..1000).map(|i| format!("{{\"name\":\"user{}\"}}", i)).collect();
    let mut st = Stat::new("POST /sessions");
    let mut auths: Vec<Vec<(String, String)>> = Vec::new();
    for n in &names { timed(s, &mut st, "POST", "/sessions", &[], n); auths.push(vec![("X-Session".to_string(), token_of(s.response()))]); }
    st.row();
    let mut st = Stat::new("POST /rooms");
    for i in 0..20 { let b = format!("{{\"name\":\"room{}\"}}", i); timed(s, &mut st, "POST", "/rooms", &auths[0], &b); }
    st.row();
    snap(s, "1000 sessions, 20 rooms");

    let body = "{\"body\":\"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.\"}";
    let posts: Vec<String> = (0..20).map(|r| format!("/rooms/{}/messages", r)).collect();
    let gets: Vec<String> = (0..20).map(|r| format!("/rooms/{}/messages?limit=50", r)).collect();
    let typing: Vec<String> = (0..20).map(|r| format!("/rooms/{}/typing", r)).collect();
    let mut st = Stat::new("POST messages (lists filling)");
    for _ in 0..20_000 { let (u, r) = (rnd(1000), rnd(20)); timed(s, &mut st, "POST", &posts[r], &auths[u], body); }
    st.row();
    snap(s, "~20000 messages");
    let mut st = Stat::new("POST messages (full: drop oldest)");
    for _ in 0..2000 { let (u, r) = (rnd(1000), rnd(20)); timed(s, &mut st, "POST", &posts[r], &auths[u], body); }
    st.row();

    let (mut st_get, mut st_post, mut st_tping, mut st_tlist) = (Stat::new("GET messages (limit 50)"), Stat::new("POST messages (mixed)"), Stat::new("POST typing"), Stat::new("GET typing"));
    for i in 0..mixed {
        let (u, r, k) = (rnd(1000), rnd(20), rnd(100));
        s.clock = Some(1_000_000 + i as i64 * 5);
        if k < 70 { timed(s, &mut st_get, "GET", &gets[r], &auths[u], ""); }
        else if k < 90 { timed(s, &mut st_post, "POST", &posts[r], &auths[u], body); }
        else if k < 95 { timed(s, &mut st_tping, "POST", &typing[r], &auths[u], ""); }
        else { timed(s, &mut st_tlist, "GET", &typing[r], &auths[u], ""); }
    }
    for st in [&mut st_get, &mut st_post, &mut st_tping, &mut st_tlist] { st.row(); }
    snap(s, "after mixed load");

    let mut st = Stat::new("DELETE /session");
    for a in &auths { timed(s, &mut st, "DELETE", "/session", a, ""); }
    st.row();
    let (c, b) = s.handle("GET", "/stats", &[], "");
    println!("  GET /stats -> {} {}", c, b);
    snap(s, "sessions deleted");
    if let Some(p) = &s.pressure { println!("  pressure policy: soft {:.0} MB, hard {:.0} MB: {} scoped trims, {} full trims, {:.2} MB released, refusing writes now: {}", mb(p.soft), mb(p.hard), p.trims, p.full_trims, mb(p.freed), p.refusing); }
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
    thread_local! { static COMPILED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    /// Says which mode a failing test was in.
    struct Mode(bool);
    impl Drop for Mode { fn drop(&mut self) { if std::thread::panicking() { eprintln!("(failed with endpoints {})", if self.0 { "compiled" } else { "interpreted" }); } } }
    fn mode(compiled: bool) -> Mode { COMPILED.with(|c| c.set(compiled)); Mode(compiled) }
    fn chat(compiled: bool) -> Server {
        let inst = Instance::start(Program::parse(STATE).unwrap(), 1 << 30).unwrap();
        let mut s = Server::new(inst, include_str!("../examples/chat/chat.api")).unwrap();
        s.clock = Some(1000);
        if compiled { let (notes, _) = s.compile().unwrap(); assert!(notes.iter().filter(|n| n.ends_with(": compiled")).count() >= 8, "{:?}", notes); }
        s
    }
    fn server() -> Server { chat(COMPILED.with(|c| c.get())) }
    fn login(s: &mut Server, name: &str) -> Vec<(String, String)> {
        let (c, b) = s.handle("POST", "/sessions", &[], &format!("{{\"name\":\"{}\"}}", name));
        assert_eq!(c, 201, "{}", b);
        vec![("X-Session".into(), token_of(&b))]
    }

    #[test]
    fn auth_distinguishes_missing_wrong_secret_and_ended_sessions() {
        for compiled in [false, true] {
            let _m = mode(compiled);
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
    }

    #[test]
    fn error_codes_and_rollback() {
        for compiled in [false, true] {
            let _m = mode(compiled);
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
    }

    #[test]
    fn room_list_drops_the_oldest_and_lists_newest_first() {
        for compiled in [false, true] {
            let _m = mode(compiled);
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
    }

    #[test]
    fn deleted_author_shows_as_deleted_instead_of_failing_the_listing() {
        for compiled in [false, true] {
            let _m = mode(compiled);
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
    }

    #[test]
    fn rate_limit_and_typing_upsert_and_expiry() {
        for compiled in [false, true] {
            let _m = mode(compiled);
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
    }

    #[test]
    fn fast_escaping_matches_a_byte_by_byte_reference() {
        fn reference(s: &[u8]) -> Vec<u8> {
            let mut o = vec![b'"'];
            for &c in s { match c { b'"' => o.extend_from_slice(b"\\\""), b'\\' => o.extend_from_slice(b"\\\\"), b'\n' => o.extend_from_slice(b"\\n"), b'\r' => o.extend_from_slice(b"\\r"), b'\t' => o.extend_from_slice(b"\\t"), c if c < 0x20 => o.extend_from_slice(format!("\\u{:04x}", c).as_bytes()), c => o.push(c) } }
            o.push(b'"'); o
        }
        let mut buf = vec![0u8; 1 << 16];
        let mut out = Area { ptr: buf.as_mut_ptr(), cap: buf.len(), len: 0 };
        let alphabet: &[u8] = b"ab\"\\\n\t\x01\x1f \x7f\xc3\xa9z0";
        let mut r = 12345u64;
        for n in 0..2000 {
            let len = n % 70;
            let s: Vec<u8> = (0..len).map(|_| { r ^= r << 13; r ^= r >> 7; r ^= r << 17; if r % 3 == 0 { alphabet[(r >> 8) as usize % alphabet.len()] } else { b'a' + (r >> 16) as u8 % 26 } }).collect();
            out.len = 0;
            assert!(escaped(&mut out, &s));
            assert_eq!(out.all(), reference(&s).as_slice(), "input {:?}", s);
        }
    }

    #[test]
    fn the_binder_rejects_bad_names_and_calls_at_load() {
        let st = || Instance::start(Program::parse("struct C { n: i64; } global c: C;").unwrap(), 1 << 20).unwrap();
        for (api, want) in [
            ("endpoint GET /a { respond 200 { \"x\": nope }; }", "unknown name `nope`"),
            ("endpoint GET /a { respond 200 { \"x\": frob(1) }; }", "unknown function `frob`"),
            ("fn f(a, b) { return a; } endpoint GET /a { respond 200 { \"x\": f(1) }; }", "`f` takes 2 arguments, given 1"),
            ("endpoint GET /a { x = 1; respond 200 {}; }", "unknown variable `x`"),
            ("endpoint GET /a { if true { let y = 1; } respond 200 { \"y\": y }; }", "unknown name `y`"), // block scope
            ("fn len(s) { return 1; }", "name of a builtin"),
        ] {
            let e = Server::new(st(), api).err().unwrap_or_default();
            assert!(e.contains(want), "{} -> {}", api, e);
        }
        // shadowing and slot reuse across blocks still work
        let mut s = Server::new(st(), "endpoint GET /a { let x = 1; var t = 0; if true { let x = 10; t = t + x; } each k in [] {} respond 200 { \"x\": x, \"t\": t }; }".replace("each k in [] {} ", "").as_str()).unwrap();
        assert_eq!(s.handle("GET", "/a", &[], ""), (200, "{\"x\":1,\"t\":10}".to_string()));
    }

    #[test]
    fn compiled_and_interpreted_servers_answer_alike() {
        let mut ss = [chat(false), chat(true)];
        for s in ss.iter_mut() { s.rng.set(0x1234_5678_9abc_def0); }
        let mut log: [Vec<String>; 2] = [Vec::new(), Vec::new()];
        for (k, s) in ss.iter_mut().enumerate() {
            let mut req = |s: &mut Server, m: &str, t: &str, h: &[(String, String)], b: &str| { let (c, r) = s.handle(m, t, h, b); log[k].push(format!("{} {} -> {} {}", m, t, c, r)); r };
            let ann = token_of(&req(s, "POST", "/sessions", &[], "{\"name\":\"ann\"}"));
            let bob = token_of(&req(s, "POST", "/sessions", &[], "{\"name\":\"bob \\\"b\\\"\"}"));
            req(s, "POST", "/sessions", &[], "{}");
            req(s, "POST", "/sessions", &[], "{\"name\":5}");
            let (a, b) = (vec![("X-Session".to_string(), ann.clone())], vec![("X-Session".to_string(), bob.clone())]);
            for body in ["{\"name\":\"lobby\"}", "{\"name\":\"way too long\"}", "{}", "{\"name\":\"r2\"}", "{\"name\":\"r3\"}"] { req(s, "POST", "/rooms", &a, body); }
            req(s, "GET", "/rooms", &b, "");
            for i in 0..8 { req(s, "POST", "/rooms/0/messages", if i % 2 == 0 { &a } else { &b }, &format!("{{\"body\":\"m{} \\u00e9\"}}", i)); }
            req(s, "POST", "/rooms/0/messages", &a, "{}");
            req(s, "POST", "/rooms/0/messages", &a, &format!("{{\"body\":\"{}\"}}", "x".repeat(2001)));
            req(s, "POST", "/rooms/9/messages", &a, "{\"body\":\"x\"}");
            for q in ["", "?limit=3", "?limit=0", "?limit=-1", "?limit=abc", "?limit=1000"] { req(s, "GET", &format!("/rooms/0/messages{}", q), &b, ""); }
            req(s, "GET", "/rooms/x/messages", &b, "");
            req(s, "GET", "/rooms/1/messages", &b, "");
            req(s, "POST", "/rooms/0/typing", &a, ""); req(s, "POST", "/rooms/0/typing", &b, ""); req(s, "POST", "/rooms/0/typing", &a, "");
            req(s, "GET", "/rooms/0/typing", &b, "");
            s.clock = Some(1000 + 6000);
            req(s, "GET", "/rooms/0/typing", &b, "");
            req(s, "POST", "/rooms/0/typing", &b, "");
            req(s, "GET", "/rooms/0/typing", &a, "");
            req(s, "DELETE", "/session", &a, "");
            req(s, "GET", "/rooms/0/messages", &b, "");
            req(s, "GET", "/rooms/0/messages", &a, "");
            req(s, "GET", "/rooms", &[("X-Session".to_string(), "0.9.ffff".to_string())], "");
            req(s, "GET", "/stats", &[], "");
            for _ in 0..40 { req(s, "POST", "/rooms/0/messages", &b, "{\"body\":\"flood\"}"); }
            req(s, "GET", "/nope", &[], "");
        }
        for (x, y) in log[0].iter().zip(log[1].iter()) { assert_eq!(x, y, "interpreted vs compiled"); }
        if std::env::var("MITO_SHOW_LOG").is_ok() { for l in &log[1] { println!("{}", &l[..l.len().min(150)]); } }
        assert_eq!(log[0].len(), log[1].len());
        let rows = |s: &Server| s.inst.inspect().into_iter().map(|r| (r.0, r.4)).collect::<Vec<_>>();
        assert_eq!(rows(&ss[0]), rows(&ss[1]), "final state");
    }

    #[test]
    fn functions_and_vars() {
        let inst = Instance::start(Program::parse("struct C { n: i64; } global c: C;").unwrap(), 1 << 20).unwrap();
        let api = "fn double(x: i64) -> i64 { return x * 2; }
            fn sum_to(n) { var t = 0; if n > 0 { t = n + sum_to(n - 1); } return t; }
            endpoint GET /t { var a = double(21); a = a + 1; c.n = sum_to(10); respond 200 { \"a\": a, \"s\": c.n, \"j\": \"x\" + a }; }
            endpoint GET /bad { let x = 1; x = 2; respond 200 { \"x\": x }; }";
        let err = Server::new(Instance::start(Program::parse("struct C { n: i64; } global c: C;").unwrap(), 1 << 20).unwrap(), api).err().unwrap();
        assert!(err.contains("`x` is a `let`"), "{}", err); // caught when the server loads, not at run time
        let api = api.replace("x = 2;", "");
        let mut s = Server::new(inst, &api).unwrap();
        assert_eq!(s.handle("GET", "/t", &[], ""), (200, "{\"a\":43,\"s\":55,\"j\":\"x43\"}".to_string()));
        assert_eq!(s.handle("GET", "/bad", &[], ""), (200, "{\"x\":1}".to_string()));
    }

    #[test]
    fn warm_requests_do_not_allocate() {
        for compiled in [false, true] {
            let _m = mode(compiled);
            let mut s = server();
            let h = login(&mut s, "ann");
            s.handle("POST", "/rooms", &h, "{\"name\":\"r\"}");
            let post = "{\"body\":\"hello \\\"there\\\"\"}";
            for _ in 0..10 { s.handle_raw("POST", "/rooms/0/messages", &h, post); s.handle_raw("GET", "/rooms/0/messages?limit=50", &h, ""); s.handle_raw("POST", "/rooms/0/typing", &h, ""); }
            let a0 = crate::alloc_count::allocs();
            assert_eq!(s.handle_raw("POST", "/rooms/0/messages", &h, post), 201);
            assert_eq!(s.handle_raw("GET", "/rooms/0/messages?limit=50", &h, ""), 200);
            assert_eq!(s.handle_raw("POST", "/rooms/0/typing", &h, ""), 200);
            assert_eq!(s.handle_raw("GET", "/rooms/0/typing", &h, ""), 200);
            assert_eq!(crate::alloc_count::allocs() - a0, 0, "heap allocations in warm requests");
            assert!(s.response().contains("ann"));
        }
    }

    #[test]
    fn pressure_refuses_writes_above_the_hard_mark_and_still_serves_reads() {
        for compiled in [false, true] {
            let _m = mode(compiled);
            let mut s = server();
            let h = login(&mut s, "ann");
            s.handle("POST", "/rooms", &h, "{\"name\":\"r\"}");
            s.pressure = Some(Pressure::new(&s.inst, 0, 0)); // every committed byte is over the marks
            assert_eq!(s.handle("POST", "/rooms/0/messages", &h, "{\"body\":\"x\"}").0, 201); // decided after this request
            assert_eq!(s.handle("POST", "/rooms/0/messages", &h, "{\"body\":\"y\"}").0, 503);
            assert_eq!(s.handle("GET", "/rooms/0/messages", &h, "").0, 200);
            assert!(s.pressure.as_ref().unwrap().full_trims >= 1);
        }
    }

    #[test]
    fn json_input_escapes_and_unicode() {
        for compiled in [false, true] {
            let _m = mode(compiled);
            let mut s = server();
            let h = login(&mut s, "ann");
            s.handle("POST", "/rooms", &h, "{\"name\":\"r\"}");
            let bs = char::from(92); // a backslash, built at run time so the escapes below stay escapes in the request text
            let body = format!("{{\"body\":\"caf{b}u00e9 {b}ud83d{b}ude00 tab{b}t {b}\"q{b}\"\"}}", b = bs);
            assert_eq!(s.handle("POST", "/rooms/0/messages", &h, &body).0, 201);
            let (_, out) = s.handle("GET", "/rooms/0/messages", &h, "");
            let want = format!("caf{} {} tab{}t {}\"q{}\"", char::from_u32(0xE9).unwrap(), char::from_u32(0x1F600).unwrap(), bs, bs, bs);
            assert!(out.contains(&want), "{}", out);
        }
    }

    #[test]
    fn forward_outside_a_system_is_a_clear_error_and_rolls_back() {
        let inst = Instance::start(Program::parse("struct C { n: i64; } global c: C;").unwrap(), 1 << 20).unwrap();
        let mut s = Server::new(inst, "endpoint POST /f { c.n = 5; forward other[1] { \"x\": 1 }; }").unwrap();
        let (code, body) = s.handle("POST", "/f", &[], "");
        assert_eq!(code, 500, "{}", body);
        assert_eq!(s.inst.get("c.n").unwrap(), "0");
    }

    #[test]
    fn frees_happen_only_when_the_request_succeeds() {
        let inst = Instance::start(Program::parse("struct T { n: i32; } global ts: pool<T, 2>;").unwrap(), 1 << 20).unwrap();
        let api = "endpoint POST /t { let t = alloc ts; t.n = 7; respond 201 { \"slot\": slot(t) }; }
            endpoint DELETE /t/{k} { let t = ts[int(param(\"k\"))]; free t; if param(\"k\") == \"0\" { fail 409 \"changed my mind\"; } respond 200 { \"n\": t.n }; }
            endpoint POST /swap { each t in ts { free t; } let u = try_alloc ts; respond 200 { \"got\": exists(u) }; }";
        let mut s = Server::new(inst, api).unwrap();
        assert_eq!(s.handle("POST", "/t", &[], ""), (201, "{\"slot\":0}".to_string()));
        assert_eq!(s.handle("POST", "/t", &[], ""), (201, "{\"slot\":1}".to_string()));
        assert_eq!(s.handle("DELETE", "/t/0", &[], "").0, 409); // failed: the free did not happen
        assert_eq!(s.inst.count(s.inst.prog.coll_of("ts").unwrap(), 0), 2);
        assert_eq!(s.handle("DELETE", "/t/1", &[], ""), (200, "{\"n\":7}".to_string())); // still readable after `free` in the same request
        assert_eq!(s.inst.count(s.inst.prog.coll_of("ts").unwrap(), 0), 1);
        // a slot freed in a request is not reusable by that request
        assert_eq!(s.handle("POST", "/t", &[], "").0, 201);
        assert_eq!(s.handle("POST", "/swap", &[], ""), (200, "{\"got\":false}".to_string()));
        assert_eq!(s.inst.count(s.inst.prog.coll_of("ts").unwrap(), 0), 0);
    }

    #[test]
    fn lists_of_plain_values_index_and_loop_as_values() {
        let inst = Instance::start(Program::parse("struct U { tags: list<i32, 4>; } global us: pool<U, 2>;").unwrap(), 1 << 20).unwrap();
        let api = "endpoint POST /u { let u = alloc us; let a = alloc u.tags; let b = alloc u.tags; respond 201 { \"n\": count(u.tags) }; }
            endpoint GET /u { var sum = 0; each t in us[0].tags { sum = sum + t; } respond 200 { \"sum\": sum, \"all\": [each t in us[0].tags t] }; }";
        let mut s = Server::new(inst, api).unwrap();
        assert_eq!(s.handle("POST", "/u", &[], "").0, 201);
        s.inst.set("us[0].tags[0]", "5").unwrap();
        s.inst.set("us[0].tags[1]", "6").unwrap();
        assert_eq!(s.inst.get("us[0].tags[1]").unwrap(), "6");
        assert_eq!(s.handle("GET", "/u", &[], ""), (200, "{\"sum\":11,\"all\":[5,6]}".to_string()));
    }
}

/// The floor for `GET /rooms/{room}/messages?limit=50`: the same JSON written by native code from the same state,
/// next to the interpreter. Integers formatted with `write!` and with a hand-written formatter.
pub fn json_floor() {
    use crate::layout::Program;
    let prog = Program::parse(include_str!("../examples/chat/chat.mito")).unwrap();
    let mut s = Server::new(Instance::start(prog, usize::MAX).unwrap(), include_str!("../examples/chat/chat.api")).unwrap();
    s.clock = Some(1_000_000);
    let _ = s.inst.resolve("meta.rate").and_then(|l| s.inst.loc_set(&l, &Val::Int(1_000_000)));
    s.handle_raw("POST", "/sessions", &[], "{\"name\":\"ann\"}");
    let h = vec![("X-Session".to_string(), s.response().split("\"token\":\"").nth(1).unwrap().split('"').next().unwrap().to_string())];
    s.handle_raw("POST", "/rooms", &h, "{\"name\":\"general\"}");
    let body = "{\"body\":\"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.\"}";
    for _ in 0..1000 { s.handle_raw("POST", "/rooms/0/messages", &h, body); }
    let reps = 20_000;
    let t = Instant::now();
    for _ in 0..reps { s.handle_raw("GET", "/rooms/0/messages?limit=50", &h, ""); }
    let interp = t.elapsed().as_nanos() as f64 / reps as f64 / 1000.0;
    let interp_len = s.out.len;
    let interp_body = s.response().to_string();
    let t = Instant::now();
    let (notes, src) = match s.compile() { Ok(x) => x, Err(e) => { println!("compile failed: {}", e); return; } };
    println!("compiled with TinyCC in {:.1} ms ({} lines of C):", t.elapsed().as_secs_f64() * 1e3, src.lines().count());
    for n in &notes { println!("  {}", n); }
    if std::env::var("MITO_EMIT_C").is_ok() { println!("{}", src); }
    s.handle_raw("GET", "/rooms/0/messages?limit=50", &h, "");
    println!("compiled response identical to the interpreter's: {}", s.response() == interp_body);
    let t = Instant::now();
    for _ in 0..reps { s.handle_raw("GET", "/rooms/0/messages?limit=50", &h, ""); }
    let compiled = t.elapsed().as_nanos() as f64 / reps as f64 / 1000.0;

    let mc = s.inst.prog.colls.iter().position(|c| c.name == "rooms.messages").unwrap();
    let sc = s.inst.prog.coll_of("sessions").unwrap();
    let (cseq, cat, cauth, cbody) = (s.inst.col(mc, "seq").unwrap(), s.inst.col(mc, "at").unwrap(), s.inst.col(mc, "author").unwrap(), s.inst.col(mc, "body").unwrap());
    let cname = s.inst.col(sc, "name").unwrap();
    let mut items = Vec::with_capacity(1000);
    let fast_int = |out: &mut Area, mut v: i64| { let mut b = [0u8; 20]; let mut i = 20; let neg = v < 0; if v == 0 { i -= 1; b[i] = b'0'; } while v != 0 { i -= 1; b[i] = b'0' + (v % 10).unsigned_abs() as u8; v /= 10; } if neg { i -= 1; b[i] = b'-'; } out.push(&b[i..]); };
    for fast in [false, true] {
        let t = Instant::now();
        for _ in 0..reps {
            let inst = &s.inst;
            let out = &mut s.out;
            out.len = 0;
            out.push(b"{\"room\":"); escaped(out, inst.str_bytes(&Loc::Plain(0, Ty::Str(0))).map(|_| b"general".as_slice()).unwrap_or(b"general")); out.push(b",\"messages\":[");
            inst.items_into(mc, 0, &mut items);
            for (n, &idx) in items.iter().rev().take(50).enumerate() {
                if n > 0 { out.push(b","); }
                let Val::Int(seq) = inst.field_get(mc, idx, cseq).unwrap() else { unreachable!() };
                let Val::Int(at) = inst.field_get(mc, idx, cat).unwrap() else { unreachable!() };
                out.push(b"{\"seq\":"); if fast { fast_int(out, seq); } else { let _ = write!(out, "{}", seq); }
                out.push(b",\"author\":");
                match inst.field_get(mc, idx, cauth).unwrap() { Val::Ref(Some(r)) => match inst.deref_quiet(sc, r) { Some(si) => { escaped(out, inst.str_bytes(&Loc::Field(sc, si, cname)).unwrap()); } None => { escaped(out, b"[deleted]"); } }, _ => { escaped(out, b"[deleted]"); } }
                out.push(b",\"at\":"); if fast { fast_int(out, at); } else { let _ = write!(out, "{}", at); }
                out.push(b",\"body\":"); escaped(out, inst.str_bytes(&Loc::Field(mc, idx, cbody)).unwrap());
                out.push(b"}");
            }
            out.push(b"]}");
        }
        let native = t.elapsed().as_nanos() as f64 / reps as f64 / 1000.0;
        println!("native, {:<28} {:>6.2} us  ({} bytes)", if fast { "hand-written integer format" } else { "write! integer format" }, native, s.out.len);
    }
    println!("interpreter (whole request)          {:>6.2} us  ({} bytes)", interp, interp_len);
    println!("TinyCC-compiled (whole request)      {:>6.2} us", compiled);
}

// ---------------------------------------------------------------- compiled endpoints

use std::ffi::c_void;

/// One compiled unit and the native entry point of each endpoint it covers (`None`: interpreted).
pub struct Compiled { _unit: crate::tcc::Unit, eps: Vec<Option<unsafe extern "C" fn(*mut Ctx) -> i32>>, pub notes: Vec<String> }

/// What generated code receives: the reservation's base, the running request, and the helper table (see cgen::PRELUDE).
#[repr(C)]
pub struct Ctx { base: *mut u8, run: *mut c_void, h: *const Helpers }
#[repr(C)]
struct CTxt { off: u32, len: u32, present: i32 }
/// A value of any kind, as generated code holds it. Tags as in cgen::PRELUDE.
#[repr(C)]
#[derive(Clone, Copy)]
struct CVal { tag: i32, a: u32, b: u32, i: i64, f: f64 }

#[repr(C)]
struct Helpers {
    out: extern "C" fn(*mut Ctx, *const u8, u32) -> i32,
    json_i64: extern "C" fn(*mut Ctx, i64) -> i32,
    json_col: extern "C" fn(*mut Ctx, u32, u32, u32) -> i32,
    json_txt: extern "C" fn(*mut Ctx, *const CTxt) -> i32,
    json_val: extern "C" fn(*mut Ctx, *const CVal) -> i32,
    auth: extern "C" fn(*mut Ctx, u32, *mut u32) -> i32,
    index: extern "C" fn(*mut Ctx, u32, u32, i64, *mut u32) -> i32,
    lit: extern "C" fn(*mut Ctx, *const u8, u32, *mut CTxt) -> i32,
    deref: extern "C" fn(*mut Ctx, u32, u32, u32, *mut u32) -> i32,
    fail: extern "C" fn(*mut Ctx, i32, *const u8, u32) -> i32,
    dead: extern "C" fn(*mut Ctx) -> i32,
    col_txt: extern "C" fn(*mut Ctx, u32, u32, u32, *mut CTxt) -> i32,
    plain_txt: extern "C" fn(*mut Ctx, u32, *mut CTxt) -> i32,
    binop: extern "C" fn(*mut Ctx, i32, *const CVal, *const CVal, *mut CVal) -> i32,
    truthy: extern "C" fn(*mut Ctx, *const CVal, *mut i32) -> i32,
    store: extern "C" fn(*mut Ctx, u32, u32, u32, *const CVal) -> i32,
    store_plain: extern "C" fn(*mut Ctx, u32, u32, u32, *const CVal) -> i32,
    alloc: extern "C" fn(*mut Ctx, u32, u32, i32, *mut u32) -> i32,
    free_: extern "C" fn(*mut Ctx, u32, u32) -> i32,
    fwd_begin: extern "C" fn(*mut Ctx, *const u8, u32) -> i32,
    fwd_put: extern "C" fn(*mut Ctx, *const u8, u32, *const CVal) -> i32,
    forward: extern "C" fn(*mut Ctx, *const u8, u32, i64) -> i32,
    crash: extern "C" fn(*mut Ctx) -> i32,
    builtin: extern "C" fn(*mut Ctx, i32, *const CVal, u32, *mut CVal) -> i32,
    json_f64: extern "C" fn(*mut Ctx, f64) -> i32,
    elem: extern "C" fn(*mut Ctx, i64, u32) -> i32,
    store_elem: extern "C" fn(*mut Ctx, u32, u32, u32, i64, *const CVal) -> i32,
    sel_mark: extern "C" fn(*mut Ctx, *mut u32) -> i32,
    sel_add: extern "C" fn(*mut Ctx, f64, u32) -> i32,
    sel_sort: extern "C" fn(*mut Ctx, u32, i32, i32, i64, *mut u32) -> i32,
    sel_get: extern "C" fn(*mut Ctx, u32, u32, *mut u32) -> i32,
    sel_end: extern "C" fn(*mut Ctx, u32) -> i32,
}
static HELPERS: Helpers = Helpers {
    out: h_out, json_i64: h_json_i64, json_col: h_json_col, json_txt: h_json_txt, json_val: h_json_val, auth: h_auth, index: h_index, lit: h_lit,
    deref: h_deref, fail: h_fail, dead: h_dead, col_txt: h_col_txt, plain_txt: h_plain_txt, binop: h_binop, truthy: h_truthy, store: h_store,
    store_plain: h_store_plain, alloc: h_alloc, free_: h_free, fwd_begin: h_fwd_begin, fwd_put: h_fwd_put, forward: h_forward, crash: h_crash, builtin: h_builtin,
    json_f64: h_json_f64, elem: h_elem, store_elem: h_store_elem,
    sel_mark: h_sel_mark, sel_add: h_sel_add, sel_sort: h_sel_sort, sel_get: h_sel_get, sel_end: h_sel_end,
};

/// Run a helper body on the request behind `c`. A stored error returns -1; a panic is caught here (it must not unwind
/// through C frames), remembered, and re-raised by `handle_req` once the compiled code has returned.
fn with_run(c: *mut Ctx, f: impl FnOnce(&mut Run<'static>) -> R<'static, i32>) -> i32 {
    let run = unsafe { &mut *((*c).run as *mut Run<'static>) };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut *run))) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => { run.cerr = Some(e); -1 }
        Err(_) => { run.panicked = true; -1 }
    }
}
fn bytes<'x>(p: *const u8, n: u32) -> &'x [u8] { if n == 0 { &[] } else { unsafe { std::slice::from_raw_parts(p, n as usize) } } }
fn put_txt(t: Txt, out: *mut CTxt) { unsafe { *out = CTxt { off: t.off, len: t.len, present: 1 }; } }
fn to_value(v: &CVal) -> Value {
    match v.tag { 1 => Value::Int(v.i), 2 => Value::Float(v.f), 3 => Value::Bool(v.i != 0), 4 => Value::Str(Txt { off: v.a, len: v.b }), 5 => Value::Item(v.a as usize, v.b as usize), _ => Value::Null }
}
fn put_value<'x>(v: Value, out: *mut CVal) -> R<'x, i32> {
    let c = match v {
        Value::Null => CVal { tag: 0, a: 0, b: 0, i: 0, f: 0.0 },
        Value::Int(i) => CVal { tag: 1, a: 0, b: 0, i, f: 0.0 },
        Value::Float(f) => CVal { tag: 2, a: 0, b: 0, i: 0, f },
        Value::Bool(b) => CVal { tag: 3, a: 0, b: 0, i: b as i64, f: 0.0 },
        Value::Str(t) => CVal { tag: 4, a: t.off, b: t.len, i: 0, f: 0.0 },
        Value::Item(c, i) => CVal { tag: 5, a: c as u32, b: i as u32, i: 0, f: 0.0 },
        _ => return fail(500, "compiled code cannot hold this kind of value"),
    };
    unsafe { *out = c; }
    Ok(0)
}

extern "C" fn h_out(c: *mut Ctx, p: *const u8, n: u32) -> i32 { with_run(c, |r| { r.out(bytes(p, n))?; Ok(0) }) }
extern "C" fn h_json_i64(c: *mut Ctx, v: i64) -> i32 { with_run(c, |r| if write!(r.out, "{}", v).is_ok() { Ok(0) } else { fail(507, "response too large") }) }
extern "C" fn h_json_col(c: *mut Ctx, cid: u32, idx: u32, ci: u32) -> i32 {
    with_run(c, |r| { let b = r.inst.str_bytes(&Loc::Field(cid as usize, idx as usize, ci as usize)).unwrap_or(&[]); if escaped(r.out, b) { Ok(0) } else { fail(507, "response too large") } })
}
extern "C" fn h_json_txt(c: *mut Ctx, t: *const CTxt) -> i32 {
    with_run(c, |r| { let t = unsafe { &*t }; if escaped(r.out, r.text.get(Txt { off: t.off, len: t.len })) { Ok(0) } else { fail(507, "response too large") } })
}
/// A value written as JSON, exactly as the interpreter's `respond` writes it.
extern "C" fn h_json_val(c: *mut Ctx, v: *const CVal) -> i32 {
    with_run(c, |r| {
        let ok = match to_value(unsafe { &*v }) {
            Value::Int(i) => write!(r.out, "{}", i).is_ok(), Value::Float(f) => json_f64(r.out, f), Value::Bool(b) => write!(r.out, "{}", b).is_ok(),
            Value::Str(t) => escaped(r.out, r.text.get(t)), Value::Null => r.out.push(b"null").is_some(),
            _ => return fail(500, "cannot write this value as JSON"),
        };
        if ok { Ok(0) } else { fail(507, "response too large") }
    })
}
extern "C" fn h_auth(c: *mut Ctx, cid: u32, out: *mut u32) -> i32 { with_run(c, |r| { let i = r.auth_item(cid as usize)?; unsafe { *out = i as u32 }; Ok(0) }) }
extern "C" fn h_index(c: *mut Ctx, cid: u32, inst: u32, k: i64, out: *mut u32) -> i32 {
    with_run(c, |r| {
        if k < 0 { return Err(Error::NotLive(format!("index {} is negative", k)).into()); }
        // the item's index; for a collection of plain values, the item's value column or nested collection is found from it
        match r.inst.step_index(Loc::Coll(cid as usize, inst as usize), k as usize, None)? { Loc::Item(_, i, _) | Loc::Field(_, i, _) | Loc::Coll(_, i) => { unsafe { *out = i as u32 }; Ok(0) } _ => fail(500, "compiled index of this collection") }
    })
}
extern "C" fn h_json_f64(c: *mut Ctx, f: f64) -> i32 { with_run(c, |r| if json_f64(r.out, f) { Ok(0) } else { fail(507, "response too large") }) }
/// Check an index into a fixed array of `n`, with the interpreter's errors.
extern "C" fn h_elem(c: *mut Ctx, k: i64, n: u32) -> i32 {
    with_run(c, |_| {
        if k < 0 { return Err(Error::NotLive(format!("index {} is negative", k)).into()); }
        if k >= n as i64 { return Err(Error::Bad(format!("index {} out of range (fixed size {})", k, n)).into()); }
        Ok(0)
    })
}
extern "C" fn h_sel_mark(c: *mut Ctx, out: *mut u32) -> i32 { with_run(c, |r| { unsafe { *out = r.sel.len() as u32 }; Ok(0) }) }
extern "C" fn h_sel_add(c: *mut Ctx, key: f64, idx: u32) -> i32 { with_run(c, |r| { r.sel.push((key, idx)); Ok(0) }) }
/// Order the items added since mark `h` as the interpreter does (a stable sort, so ties keep their order), keep at most
/// `limit` (when not negative), and say how many there are.
extern "C" fn h_sel_sort(c: *mut Ctx, h: u32, order: i32, desc: i32, limit: i64, out: *mut u32) -> i32 {
    with_run(c, |r| {
        let h = h as usize;
        if order != 0 { r.sel[h..].sort_by(|a, b| { let o = a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal); if desc != 0 { o.reverse() } else { o } }); }
        if limit >= 0 && r.sel.len() - h > limit as usize { r.sel.truncate(h + limit as usize); }
        unsafe { *out = (r.sel.len() - h) as u32 };
        Ok(0)
    })
}
extern "C" fn h_sel_get(c: *mut Ctx, h: u32, k: u32, out: *mut u32) -> i32 { with_run(c, |r| { unsafe { *out = r.sel[h as usize + k as usize].1 }; Ok(0) }) }
extern "C" fn h_sel_end(c: *mut Ctx, h: u32) -> i32 { with_run(c, |r| { r.sel.truncate(h as usize); Ok(0) }) }
extern "C" fn h_store_elem(c: *mut Ctx, cid: u32, idx: u32, ci: u32, k: i64, v: *const CVal) -> i32 {
    with_run(c, |r| { let l = r.inst.step_index(Loc::Field(cid as usize, idx as usize, ci as usize), k as usize, None)?; r.store(l, to_value(unsafe { &*v }))?; Ok(0) })
}
extern "C" fn h_lit(c: *mut Ctx, p: *const u8, n: u32, out: *mut CTxt) -> i32 { with_run(c, |r| { let t = r.txt(bytes(p, n))?; put_txt(t, out); Ok(0) }) }
extern "C" fn h_deref(c: *mut Ctx, target: u32, slot: u32, gen: u32, out: *mut u32) -> i32 {
    with_run(c, |r| Ok(match r.inst.deref_quiet(target as usize, (slot as usize, gen)) { Some(i) => { unsafe { *out = i as u32 }; 1 } None => 0 }))
}
extern "C" fn h_fail(c: *mut Ctx, code: i32, p: *const u8, n: u32) -> i32 { with_run(c, |_| fail(code as u16, String::from_utf8_lossy(bytes(p, n)).into_owned())) }
extern "C" fn h_dead(c: *mut Ctx) -> i32 { with_run(c, |_| Err(Ctl::Dead)) }
/// A string column of an item, copied into the text area (values never alias state, as in the interpreter).
extern "C" fn h_col_txt(c: *mut Ctx, cid: u32, idx: u32, ci: u32, out: *mut CTxt) -> i32 {
    with_run(c, |r| match r.read_str(&Loc::Field(cid as usize, idx as usize, ci as usize))? { Value::Str(t) => { put_txt(t, out); Ok(0) } _ => fail(500, "not a string") })
}
extern "C" fn h_plain_txt(c: *mut Ctx, off: u32, out: *mut CTxt) -> i32 {
    with_run(c, |r| match r.read_str(&Loc::Plain(off as usize, Ty::Str(0)))? { Value::Str(t) => { put_txt(t, out); Ok(0) } _ => fail(500, "not a string") })
}
extern "C" fn h_binop(c: *mut Ctx, op: i32, a: *const CVal, b: *const CVal, out: *mut CVal) -> i32 {
    with_run(c, |r| { let op = crate::cgen::OPS.get(op as usize).copied().unwrap_or("?"); let v = r.binop(op, to_value(unsafe { &*a }), to_value(unsafe { &*b }))?; put_value(v, out) })
}
extern "C" fn h_truthy(c: *mut Ctx, v: *const CVal, out: *mut i32) -> i32 { with_run(c, |r| { let t = r.truthy(&to_value(unsafe { &*v }))?; unsafe { *out = t as i32 }; Ok(0) }) }
extern "C" fn h_store(c: *mut Ctx, cid: u32, idx: u32, ci: u32, v: *const CVal) -> i32 {
    with_run(c, |r| { r.store(Loc::Field(cid as usize, idx as usize, ci as usize), to_value(unsafe { &*v }))?; Ok(0) })
}
extern "C" fn h_store_plain(c: *mut Ctx, off: u32, code: u32, cap: u32, v: *const CVal) -> i32 {
    with_run(c, |r| {
        // code 30: a reference to top-level pool `cap`
        if code == 30 {
            let val = match to_value(unsafe { &*v }) { Value::Null => Val::Ref(None), Value::Item(t, i) if t == cap as usize => Val::Ref(Some((i, r.inst.gen(t, i).unwrap_or(0)))), _ => return fail(500, "this field holds a reference to a different pool") };
            r.inst.plain_set(off as usize, &Ty::Ref(String::new()), &val)?; // the name is not needed to store
            return Ok(0);
        }
        let Some(t) = crate::cgen::plain_ty(code, cap) else { return fail(500, "bad plain kind") };
        r.store(Loc::Plain(off as usize, t), to_value(unsafe { &*v }))?; Ok(0)
    })
}
extern "C" fn h_alloc(c: *mut Ctx, cid: u32, inst: u32, try_: i32, out: *mut u32) -> i32 {
    with_run(c, |r| match r.inst.push_item(cid as usize, inst as usize) {
        Ok(i) => { unsafe { *out = i as u32 }; Ok(0) }
        Err(Error::Full(_)) if try_ != 0 => { unsafe { *out = u32::MAX }; Ok(0) }
        Err(e) => Err(e.into()),
    })
}
extern "C" fn h_free(c: *mut Ctx, cid: u32, idx: u32) -> i32 {
    with_run(c, |r| {
        let (cid, idx) = (cid as usize, idx as usize);
        if r.inst.is_list(cid) { return fail(500, "list items are not freed; lists drop their oldest or pop their newest"); }
        if !r.frees.contains(&(cid, idx)) { r.frees.push((cid, idx)); }
        Ok(0)
    })
}
extern "C" fn h_fwd_begin(c: *mut Ctx, p: *const u8, n: u32) -> i32 {
    with_run(c, |r| {
        let kind = bytes(p, n);
        let Some(s) = r.sys else { return fail(500, "forward needs a system of workers") };
        if !s.kinds.iter().any(|x| x.0.as_bytes() == kind) { return fail(500, format!("no worker kind `{}`", String::from_utf8_lossy(kind))); }
        r.fwd_out.clear();
        Ok(0)
    })
}
extern "C" fn h_fwd_put(c: *mut Ctx, p: *const u8, n: u32, v: *const CVal) -> i32 {
    with_run(c, |r| {
        let v = to_value(unsafe { &*v });
        let kt = r.txt(bytes(p, n))?;
        let start = r.text.len;
        if !matches!(v, Value::Null) { r.show_into(&v)?; }
        let t = r.text.since(start);
        r.fwd_out.push((kt, t));
        Ok(0)
    })
}
extern "C" fn h_forward(c: *mut Ctx, p: *const u8, n: u32, key: i64) -> i32 {
    with_run(c, |r| { let kind = bytes(p, n); let k = r.sys.and_then(|s| s.kinds.iter().position(|x| x.0.as_bytes() == kind)).unwrap_or(0); Err(Ctl::Forward(k, key)) })
}
extern "C" fn h_crash(c: *mut Ctx) -> i32 { with_run(c, |_| panic!("__crash() called: test hook for worker supervision")) }
/// Any builtin in cgen::BUILTINS, computed by the interpreter's own `apply`.
extern "C" fn h_builtin(c: *mut Ctx, code: i32, argv: *const CVal, n: u32, out: *mut CVal) -> i32 {
    with_run(c, |r| {
        let Some(&b) = crate::cgen::BUILTINS.get(code as usize) else { return fail(500, "unknown builtin code") };
        let base = r.args.len();
        for k in 0..n as usize { let v = to_value(unsafe { &*argv.add(k) }); r.args.push(v); }
        let res = r.apply(b, base);
        r.args.truncate(base);
        put_value(res?, out)
    })
}

impl Server {
    /// Compile what the generator handles to native code with TinyCC; the rest stays interpreted.
    /// Returns one line per endpoint saying what happened, and the generated C.
    pub fn compile(&mut self) -> Result<(Vec<String>, String), String> {
        let g = crate::cgen::generate(&self.inst.prog, &self.api);
        let unit = crate::tcc::Unit::compile(&g.src, &[])?;
        let mut eps = Vec::new();
        for name in &g.eps {
            eps.push(match name { Some(n) => Some(unsafe { std::mem::transmute::<*const c_void, unsafe extern "C" fn(*mut Ctx) -> i32>(unit.symbol(n).ok_or(format!("compiled unit has no {}", n))?) }), None => None });
        }
        let mut notes: Vec<String> = self.api.eps.iter().zip(&g.eps).filter(|(_, n)| n.is_some()).map(|(e, _)| format!("{}: compiled", e.text)).collect();
        notes.extend(g.notes.iter().cloned());
        self.compiled = Some(Compiled { _unit: unit, eps, notes: notes.clone() });
        Ok((notes, g.src))
    }
}
