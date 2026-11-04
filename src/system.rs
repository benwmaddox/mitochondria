//! A system of workers. Each worker is a program (declared state plus endpoints) on its own OS thread, with its own
//! reservation, commit tracking and pressure policy: workers share nothing. They talk through bounded mailboxes of
//! fixed-size message slots (a list with a declared size and a `fail`-when-full policy). A request enters at an entry
//! worker, may be forwarded from worker to worker, and is answered exactly once through a fixed table of reply slots.
//!
//! v1 choices: mailboxes are a mutex and a condition variable, not lock-free; worker-to-worker sends never block (a full
//! mailbox fails the forward, which rolls the forwarding handler back and answers 503); a worker that panics is rebuilt
//! from its source with a fresh reservation, so its state is lost, as an Erlang process's is.
use crate::layout::Program;
use crate::runtime::{Instance, Pressure};
use crate::serve::{Server, StrFields, SysInfo};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const MB: usize = 1 << 20;

// ---------------------------------------------------------------- spec

pub struct KindSpec { pub name: String, pub state: String, pub api: String, pub count: usize, pub mailbox: usize, pub slot: usize, pub soft: Option<usize>, pub hard: Option<usize> }
pub struct Spec { pub kinds: Vec<KindSpec>, pub entry: usize, pub inflight: usize, pub budget: usize }

/// Parse a `.system` file. Paths in it are relative to `dir`.
///
/// ```text
/// worker auth  { state "auth.mito";  api "auth.api";  count 2; mailbox 1024; }
/// worker rooms { state "rooms.mito"; api "rooms.api"; count 4; mailbox 1024; slot 8192; soft 64; hard 256; }
/// entry auth;
/// inflight 4096;
/// budget 4096;
/// ```
pub fn parse_spec(src: &str, dir: &Path) -> Result<Spec, String> {
    let toks = tokens(src);
    let mut i = 0;
    let mut spec = Spec { kinds: Vec::new(), entry: usize::MAX, inflight: 4096, budget: 4096 * MB };
    let mut entry = String::new();
    let next = |i: &mut usize| -> Result<String, String> { let t = toks.get(*i).cloned().ok_or("unexpected end of system file")?; *i += 1; Ok(t) };
    let num = |s: String| -> Result<usize, String> { s.parse().map_err(|_| format!("expected a number, found `{}`", s)) };
    let read = |f: &str| -> Result<String, String> { std::fs::read_to_string(dir.join(f)).map_err(|e| format!("{}: {}", f, e)) };
    while i < toks.len() {
        match next(&mut i)?.as_str() {
            "worker" => {
                let name = next(&mut i)?;
                if next(&mut i)? != "{" { return Err("expected `{`".into()); }
                let mut k = KindSpec { name, state: String::new(), api: String::new(), count: 1, mailbox: 1024, slot: 8192, soft: None, hard: None };
                loop {
                    let key = next(&mut i)?;
                    if key == "}" { break; }
                    let v = next(&mut i)?;
                    match key.as_str() {
                        "state" => k.state = read(&v)?, "api" => k.api = read(&v)?,
                        "count" => k.count = num(v)?.max(1), "mailbox" => k.mailbox = num(v)?.max(1), "slot" => k.slot = num(v)?.max(256),
                        "soft" => k.soft = Some(num(v)? * MB), "hard" => k.hard = Some(num(v)? * MB),
                        o => return Err(format!("unknown worker setting `{}`", o)),
                    }
                    if next(&mut i)? != ";" { return Err(format!("expected `;` after `{}`", key)); }
                }
                spec.kinds.push(k);
            }
            "entry" => { entry = next(&mut i)?; next(&mut i)?; }
            "inflight" => { spec.inflight = num(next(&mut i)?)?.max(1); next(&mut i)?; }
            "budget" => { spec.budget = num(next(&mut i)?)? * MB; next(&mut i)?; }
            o => return Err(format!("expected `worker`, `entry`, `inflight` or `budget`, found `{}`", o)),
        }
    }
    spec.entry = spec.kinds.iter().position(|k| k.name == entry).ok_or(format!("entry `{}` is not a declared worker", entry))?;
    Ok(spec)
}
fn tokens(src: &str) -> Vec<String> {
    let c: Vec<char> = src.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    while i < c.len() {
        if c[i].is_whitespace() { i += 1; }
        else if c[i] == '/' && c.get(i + 1) == Some(&'/') { while i < c.len() && c[i] != '\n' { i += 1; } }
        else if c[i] == '"' { let s = i + 1; i += 1; while i < c.len() && c[i] != '"' { i += 1; } out.push(c[s..i].iter().collect()); i += 1; }
        else if c[i].is_alphanumeric() || c[i] == '_' || c[i] == '.' { let s = i; while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '.') { i += 1; } out.push(c[s..i].iter().collect()); }
        else { out.push(c[i].to_string()); i += 1; }
    }
    out
}

// ---------------------------------------------------------------- mailboxes

struct Ring { buf: Vec<u8>, head: usize, len: usize, closed: bool }
/// A bounded queue of fixed-size message slots, allocated once.
pub struct Mailbox { ring: Mutex<Ring>, cv: Condvar, cap: usize, slot: usize, refused: AtomicU64 }
#[derive(Debug, PartialEq)]
pub enum SendErr { Full, TooBig, Closed }
enum Recv { Got, Idle, Closed }

impl Mailbox {
    fn new(cap: usize, slot: usize) -> Mailbox { Mailbox { ring: Mutex::new(Ring { buf: vec![0; cap * slot], head: 0, len: 0, closed: false }), cv: Condvar::new(), cap, slot, refused: AtomicU64::new(0) } }
    /// Put one message in; `fill` writes it into the slot and returns its length (or `None` if it does not fit). Never blocks.
    fn send(&self, fill: impl FnOnce(&mut [u8]) -> Option<usize>) -> Result<(), SendErr> {
        let mut r = self.ring.lock().unwrap();
        if r.closed { return Err(SendErr::Closed); }
        if r.len == self.cap { self.refused.fetch_add(1, Relaxed); return Err(SendErr::Full); }
        let at = ((r.head + r.len) % self.cap) * self.slot;
        let used = fill(&mut r.buf[at + 4..at + self.slot]).ok_or(SendErr::TooBig)?;
        r.buf[at..at + 4].copy_from_slice(&(used as u32).to_le_bytes());
        r.len += 1;
        drop(r);
        self.cv.notify_one();
        Ok(())
    }
    /// Take the oldest message into `out`, waiting up to `wait`. After `close`, what is queued is still delivered.
    fn recv(&self, out: &mut Vec<u8>, wait: Duration) -> Recv {
        let mut r = self.ring.lock().unwrap();
        while r.len == 0 {
            if r.closed { return Recv::Closed; }
            let (g, t) = self.cv.wait_timeout(r, wait).unwrap();
            r = g;
            if t.timed_out() && r.len == 0 { return if r.closed { Recv::Closed } else { Recv::Idle }; }
        }
        let at = r.head * self.slot;
        let n = u32::from_le_bytes(r.buf[at..at + 4].try_into().unwrap()) as usize;
        out.clear(); out.extend_from_slice(&r.buf[at + 4..at + 4 + n]);
        r.head = (r.head + 1) % self.cap; r.len -= 1;
        Recv::Got
    }
    fn close(&self) { self.ring.lock().unwrap().closed = true; self.cv.notify_all(); }
}

/// Message layout: reply id, method, target, body, headers, forwarded fields. Strings are length-prefixed.
fn encode<'h>(dst: &mut [u8], reply: u32, method: &str, target: &str, body: &str, headers: impl Iterator<Item = (&'h [u8], &'h [u8])>, fwd: impl Iterator<Item = (&'h [u8], &'h [u8])>) -> Option<usize> {
    struct W<'b> { b: &'b mut [u8], n: usize }
    impl W<'_> {
        fn raw(&mut self, x: &[u8]) -> Option<()> { let e = self.n + x.len(); if e > self.b.len() { return None; } self.b[self.n..e].copy_from_slice(x); self.n = e; Some(()) }
        fn s(&mut self, x: &[u8]) -> Option<()> { self.raw(&(x.len() as u32).to_le_bytes())?; self.raw(x) }
        fn pairs<'i>(&mut self, it: impl Iterator<Item = (&'i [u8], &'i [u8])>) -> Option<()> {
            let at = self.n; self.raw(&[0, 0])?;
            let mut k = 0u16;
            for (a, b) in it { self.s(a)?; self.s(b)?; k += 1; }
            self.b[at..at + 2].copy_from_slice(&k.to_le_bytes()); Some(())
        }
    }
    let mut w = W { b: dst, n: 0 };
    w.raw(&reply.to_le_bytes())?; w.s(method.as_bytes())?; w.s(target.as_bytes())?; w.s(body.as_bytes())?;
    w.pairs(headers)?; w.pairs(fwd)?;
    Some(w.n)
}
struct Rd<'m> { b: &'m [u8], n: usize }
impl<'m> Rd<'m> {
    fn take(&mut self, k: usize) -> Option<&'m [u8]> { let e = self.n.checked_add(k)?; let x = self.b.get(self.n..e)?; self.n = e; Some(x) }
    fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    fn u16(&mut self) -> Option<u16> { Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?)) }
    fn s(&mut self) -> Option<&'m str> { let k = self.u32()? as usize; std::str::from_utf8(self.take(k)?).ok() }
    fn pairs(&mut self, out: &mut Vec<(&'m str, &'m str)>) -> Option<()> { out.clear(); for _ in 0..self.u16()? { let a = self.s()?; let b = self.s()?; out.push((a, b)); } Some(()) }
}

// ---------------------------------------------------------------- replies

struct Slot { st: Mutex<(u8, u16, Vec<u8>)>, cv: Condvar } // state 0 free, 1 waiting, 2 answered, 3 abandoned by a timed-out caller
/// A fixed table of reply slots: the number of requests in flight is bounded by it.
struct Replies { slots: Vec<Slot>, free: Mutex<Vec<u32>> }
impl Replies {
    fn new(n: usize) -> Replies { Replies { slots: (0..n).map(|_| Slot { st: Mutex::new((0, 0, Vec::with_capacity(4096))), cv: Condvar::new() }).collect(), free: Mutex::new((0..n as u32).rev().collect()) } }
    fn take(&self) -> Option<u32> { let id = self.free.lock().unwrap().pop()?; self.slots[id as usize].st.lock().unwrap().0 = 1; Some(id) }
    fn release(&self, id: u32) { self.slots[id as usize].st.lock().unwrap().0 = 0; self.free.lock().unwrap().push(id); }
    /// Answer a request. Each request is answered once; an answer to an abandoned slot just frees it.
    fn put(&self, id: u32, code: u16, body: &[u8]) {
        let Some(s) = self.slots.get(id as usize) else { return };
        let mut g = s.st.lock().unwrap();
        match g.0 {
            1 => { g.0 = 2; g.1 = code; g.2.clear(); g.2.extend_from_slice(body); drop(g); s.cv.notify_one(); }
            3 => { g.0 = 0; drop(g); self.free.lock().unwrap().push(id); }
            _ => {} // already answered: never happens by construction; ignored rather than double-answering
        }
    }
    fn wait(&self, id: u32, timeout: Duration) -> Option<(u16, Vec<u8>)> {
        let s = &self.slots[id as usize];
        let mut g = s.st.lock().unwrap();
        let deadline = std::time::Instant::now() + timeout;
        while g.0 == 1 {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() { g.0 = 3; return None; }
            g = s.cv.wait_timeout(g, left).unwrap().0;
        }
        let out = (g.1, g.2.clone());
        g.0 = 0; drop(g);
        self.free.lock().unwrap().push(id);
        Some(out)
    }
}

// ---------------------------------------------------------------- the system

/// Counters each worker publishes (it counts its own heap allocations, which only its thread can see).
#[derive(Default)]
pub struct WStats { pub handled: AtomicU64, pub forwarded: AtomicU64, pub allocs: AtomicU64, pub restarts: AtomicU64, pub committed: AtomicUsize, pub live: AtomicUsize }

struct Shared { boxes: Vec<Mailbox>, base: Vec<usize>, counts: Vec<usize>, entry: usize, replies: Replies, stats: Vec<WStats>, names: Vec<String> }

pub struct System { shared: Arc<Shared>, threads: Vec<std::thread::JoinHandle<()>>, rr: AtomicUsize }

pub struct WorkerReport { pub name: String, pub index: usize, pub handled: u64, pub forwarded: u64, pub allocs: u64, pub restarts: u64, pub committed: usize, pub live: usize, pub refused: u64 }

impl System {
    /// Check every program, check the declared total against the budget, and start one thread per worker.
    pub fn start(spec: Spec) -> Result<System, String> {
        let mut total = 0usize;
        for k in &spec.kinds {
            let p = Program::parse(&k.state).map_err(|e| format!("worker {} state: {}", k.name, e))?;
            crate::lang::parse(&k.api).map_err(|e| format!("worker {} api: {}", k.name, e))?;
            total += p.total * k.count;
        }
        if total > spec.budget { return Err(format!("over budget: {} workers need {} MB, budget is {} MB", spec.kinds.iter().map(|k| k.count).sum::<usize>(), total / MB, spec.budget / MB)); }
        let (mut base, mut boxes, mut n) = (Vec::new(), Vec::new(), 0);
        for k in &spec.kinds { base.push(n); for _ in 0..k.count { boxes.push(Mailbox::new(k.mailbox, k.slot)); } n += k.count; }
        let shared = Arc::new(Shared {
            boxes, base, counts: spec.kinds.iter().map(|k| k.count).collect(), entry: spec.entry, replies: Replies::new(spec.inflight),
            stats: (0..n).map(|_| WStats::default()).collect(), names: spec.kinds.iter().map(|k| k.name.clone()).collect(),
        });
        let spec = Arc::new(spec);
        let mut threads = Vec::new();
        for k in 0..spec.kinds.len() {
            for i in 0..spec.kinds[k].count {
                let (sh, sp) = (shared.clone(), spec.clone());
                let t = std::thread::Builder::new().name(format!("{}{}", sp.kinds[k].name, i)).spawn(move || worker(sh, sp, k, i)).map_err(|e| e.to_string())?;
                threads.push(t);
            }
        }
        Ok(System { shared, threads, rr: AtomicUsize::new(0) })
    }

    /// Send a request in at the entry worker and wait for its answer. Requests with a sharded session token
    /// (`shard.slot.generation.secret`) go to that shard; others are spread round-robin.
    pub fn request(&self, method: &str, target: &str, headers: &[(&str, &str)], body: &str, timeout: Duration) -> (u16, Vec<u8>) {
        let sh = &self.shared;
        let Some(id) = sh.replies.take() else { return (503, b"{\"error\":\"too many requests in flight\"}".to_vec()) };
        let n = sh.counts[sh.entry];
        let shard = headers.iter().find(|h| h.0.eq_ignore_ascii_case("x-session")).and_then(|h| { let p: Vec<&str> = h.1.split('.').collect(); if p.len() == 4 { p[0].parse::<usize>().ok() } else { None } });
        let shard = shard.unwrap_or_else(|| self.rr.fetch_add(1, Relaxed)) % n;
        let pairs = headers.iter().map(|h| (h.0.as_bytes(), h.1.as_bytes()));
        match sh.boxes[sh.base[sh.entry] + shard].send(|slot| encode(slot, id, method, target, body, pairs, std::iter::empty())) {
            Ok(()) => {}
            Err(e) => { sh.replies.release(id); return if e == SendErr::TooBig { (413, b"{\"error\":\"request does not fit a mailbox slot\"}".to_vec()) } else { (503, b"{\"error\":\"entry worker is busy\"}".to_vec()) }; }
        }
        sh.replies.wait(id, timeout).unwrap_or_else(|| (504, b"{\"error\":\"no answer in time\"}".to_vec()))
    }

    pub fn report(&self) -> Vec<WorkerReport> {
        let sh = &self.shared;
        let mut v = Vec::new();
        for (k, name) in sh.names.iter().enumerate() {
            for i in 0..sh.counts[k] {
                let w = sh.base[k] + i;
                let s = &sh.stats[w];
                v.push(WorkerReport { name: name.clone(), index: i, handled: s.handled.load(Relaxed), forwarded: s.forwarded.load(Relaxed), allocs: s.allocs.load(Relaxed), restarts: s.restarts.load(Relaxed), committed: s.committed.load(Relaxed), live: s.live.load(Relaxed), refused: sh.boxes[w].refused.load(Relaxed) });
            }
        }
        v
    }

    /// Close every mailbox; workers answer what is already queued, then stop.
    pub fn shutdown(self) { for b in &self.shared.boxes { b.close(); } for t in self.threads { let _ = t.join(); } }
}

fn recycle<'x, 'y>(mut v: Vec<(&'x str, &'x str)>) -> Vec<(&'y str, &'y str)> { v.clear(); unsafe { std::mem::transmute(v) } }

fn worker(sh: Arc<Shared>, spec: Arc<Spec>, k: usize, i: usize) {
    let ks = &spec.kinds[k];
    let info = SysInfo { kinds: spec.kinds.iter().map(|k| (k.name.clone(), k.count)).collect(), me: (k, i) };
    let make = || -> Server {
        let inst = Instance::start(Program::parse(&ks.state).expect("checked at start"), usize::MAX).expect("reserve");
        let mut s = Server::new(inst, &ks.api).expect("checked at start");
        s.system = Some(info.clone());
        if let Some(soft) = ks.soft { s.pressure = Some(Pressure::new(&s.inst, soft, ks.hard.unwrap_or(usize::MAX))); }
        s
    };
    let mut server = make();
    let me = sh.base[k] + i;
    let stats = &sh.stats[me];
    let mut msg: Vec<u8> = Vec::with_capacity(ks.slot);
    let (mut hv, mut fv): (Vec<(&'static str, &'static str)>, Vec<(&'static str, &'static str)>) = (Vec::with_capacity(32), Vec::with_capacity(32));
    let publish = |s: &Server| { stats.committed.store(s.inst.committed(), Relaxed); };
    loop {
        match sh.boxes[me].recv(&mut msg, Duration::from_millis(500)) {
            Recv::Idle => { server.idle(); publish(&server); continue; }
            Recv::Closed => break,
            Recv::Got => {}
        }
        let a0 = crate::alloc_count::allocs();
        let r = catch_unwind(AssertUnwindSafe(|| handle(&mut server, &msg, &sh, &mut hv, &mut fv)));
        match r {
            Ok(forwarded) => { if forwarded { stats.forwarded.fetch_add(1, Relaxed); } }
            Err(_) => {
                // supervision: the worker's state is discarded and rebuilt from source (a fresh reservation)
                stats.restarts.fetch_add(1, Relaxed);
                server = make();
                hv = Vec::with_capacity(32); fv = Vec::with_capacity(32);
                if let Some(id) = msg.get(0..4).map(|b| u32::from_le_bytes(b.try_into().unwrap())) { sh.replies.put(id, 500, b"{\"error\":\"worker failed and was restarted\"}"); }
            }
        }
        stats.allocs.fetch_add((crate::alloc_count::allocs() - a0) as u64, Relaxed);
        stats.handled.fetch_add(1, Relaxed);
        publish(&server);
    }
}

/// Handle one message. Returns whether it was forwarded.
fn handle(server: &mut Server, msg: &[u8], sh: &Shared, hv: &mut Vec<(&'static str, &'static str)>, fv: &mut Vec<(&'static str, &'static str)>) -> bool {
    let mut h = recycle(std::mem::take(hv));
    let mut f = recycle(std::mem::take(fv));
    let mut rd = Rd { b: msg, n: 0 };
    let parsed = (|| { let reply = rd.u32()?; let m = rd.s()?; let t = rd.s()?; let b = rd.s()?; rd.pairs(&mut h)?; rd.pairs(&mut f)?; Some((reply, m, t, b)) })();
    let mut forwarded = false;
    match parsed {
        None => { if let Some(id) = msg.get(0..4).map(|b| u32::from_le_bytes(b.try_into().unwrap())) { sh.replies.put(id, 400, b"{\"error\":\"malformed message\"}"); } }
        Some((reply, method, target, body)) => {
            let code = server.handle_req(method, target, &StrFields(&h), body, &StrFields(&f));
            if code != 0 {
                sh.replies.put(reply, code, server.response().as_bytes());
            } else {
                let (kind, key) = server.pending_forward().expect("forward pending");
                let dst = sh.base[kind] + key.rem_euclid(sh.counts[kind] as i64) as usize;
                let headers = h.iter().map(|p| (p.0.as_bytes(), p.1.as_bytes()));
                let sent = sh.boxes[dst].send(|slot| encode(slot, reply, method, target, body, headers, server.forward_fields()));
                server.finish_forward(sent.is_ok()); // commit only if the request was handed on
                match sent {
                    Ok(()) => forwarded = true,
                    Err(SendErr::TooBig) => sh.replies.put(reply, 413, b"{\"error\":\"forwarded request does not fit a mailbox slot\"}"),
                    Err(SendErr::Full) => sh.replies.put(reply, 503, b"{\"error\":\"the next worker is busy\"}"),
                    Err(SendErr::Closed) => sh.replies.put(reply, 503, b"{\"error\":\"shutting down\"}"),
                }
            }
        }
    }
    *hv = recycle(h); *fv = recycle(f);
    forwarded
}

#[cfg(test)]
mod tests {
    use super::*;
    const T: Duration = Duration::from_secs(5);

    fn kind(name: &str, state: &str, api: &str, count: usize) -> KindSpec { KindSpec { name: name.into(), state: state.into(), api: api.into(), count, mailbox: 64, slot: 4096, soft: None, hard: None } }
    fn body(r: &(u16, Vec<u8>)) -> String { String::from_utf8_lossy(&r.1).into_owned() }

    #[test]
    fn forwards_reach_the_right_shard_in_order_and_each_request_is_answered_once() {
        let front = kind("front", "struct M { n: i64; } global m: M;", "endpoint POST /inc/{k} { forward count[int(param(\"k\"))] { \"from\": worker_index() }; }", 2);
        let count = kind("count", "struct C { n: i64; } global c: C;", "endpoint POST /inc/{k} { c.n = c.n + 1; respond 200 { \"shard\": worker_index(), \"n\": c.n, \"from\": int(fwd(\"from\")) }; }", 3);
        let sys = System::start(Spec { kinds: vec![front, count], entry: 0, inflight: 16, budget: 1 << 30 }).unwrap();
        for round in 1..=4 {
            for k in 0..6 {
                let r = sys.request("POST", &format!("/inc/{}", k), &[], "", T);
                assert_eq!(r.0, 200, "{}", body(&r));
                let b = body(&r);
                assert!(b.contains(&format!("\"shard\":{}", k % 3)), "{}", b);
                assert!(b.contains(&format!("\"n\":{}", round * 2 - if k < 3 { 1 } else { 0 })), "round {} key {}: {}", round, k, b); // keys k and k+3 share a shard
            }
        }
        let rep = sys.report();
        assert_eq!(rep.iter().filter(|w| w.name == "front").map(|w| w.forwarded).sum::<u64>(), 24);
        sys.shutdown();
    }

    #[test]
    fn a_panicking_worker_is_restarted_with_fresh_state() {
        let w = kind("w", "struct C { n: i64; } global c: C;", "endpoint POST /inc { c.n = c.n + 1; respond 200 { \"n\": c.n }; } endpoint POST /crash { c.n = 99; __crash(); }", 1);
        let sys = System::start(Spec { kinds: vec![w], entry: 0, inflight: 4, budget: 1 << 30 }).unwrap();
        assert_eq!(body(&sys.request("POST", "/inc", &[], "", T)), "{\"n\":1}");
        assert_eq!(body(&sys.request("POST", "/inc", &[], "", T)), "{\"n\":2}");
        assert_eq!(sys.request("POST", "/crash", &[], "", T).0, 500);
        assert_eq!(body(&sys.request("POST", "/inc", &[], "", T)), "{\"n\":1}"); // state was rebuilt
        assert_eq!(sys.report()[0].restarts, 1);
        sys.shutdown();
    }

    #[test]
    fn mailbox_full_and_too_big_are_typed() {
        let m = Mailbox::new(2, 64);
        assert!(m.send(|s| { s[0] = 1; Some(1) }).is_ok());
        assert!(m.send(|s| { s[0] = 2; Some(1) }).is_ok());
        assert_eq!(m.send(|_| Some(1)), Err(SendErr::Full));
        let mut out = Vec::new();
        assert!(matches!(m.recv(&mut out, T), Recv::Got)); assert_eq!(out, vec![1]); // first in, first out
        assert_eq!(m.send(|_| None), Err(SendErr::TooBig));
        m.close();
        assert!(matches!(m.recv(&mut out, T), Recv::Got)); assert_eq!(out, vec![2]); // queued messages still arrive after close
        assert!(matches!(m.recv(&mut out, T), Recv::Closed));
        assert_eq!(m.send(|_| Some(1)), Err(SendErr::Closed));
    }

    #[test]
    fn a_failed_forward_rolls_the_forwarding_handler_back() {
        let inst = Instance::start(Program::parse("struct C { n: i64; } global c: C;").unwrap(), 1 << 20).unwrap();
        let mut s = Server::new(inst, "endpoint POST /f { c.n = c.n + 1; forward next[0] { \"n\": c.n }; }").unwrap();
        s.system = Some(SysInfo { kinds: vec![("me".into(), 1), ("next".into(), 1)], me: (0, 0) });
        assert_eq!(s.handle_req("POST", "/f", &StrFields(&[]), "", &StrFields(&[])), 0);
        assert_eq!(s.forward_fields().next().map(|f| f.1.to_vec()), Some(b"1".to_vec()));
        s.finish_forward(false); // the next mailbox was full
        assert_eq!(s.inst.get("c.n").unwrap(), "0");
        assert_eq!(s.handle_req("POST", "/f", &StrFields(&[]), "", &StrFields(&[])), 0);
        s.finish_forward(true);
        assert_eq!(s.inst.get("c.n").unwrap(), "1");
    }

    #[test]
    fn in_flight_requests_are_bounded() {
        let r = Replies::new(2);
        let (a, b) = (r.take().unwrap(), r.take().unwrap());
        assert!(r.take().is_none());
        r.put(a, 200, b"x");
        assert_eq!(r.wait(a, T).unwrap(), (200, b"x".to_vec()));
        assert!(r.wait(b, Duration::from_millis(10)).is_none()); // abandoned after a timeout
        r.put(b, 200, b"late"); // the late answer frees the slot instead of being delivered
        assert!(r.take().is_some() && r.take().is_some());
    }

    #[test]
    fn sharded_chat_end_to_end() {
        let k = |n: &str, st: &str, api: &str, c| kind(n, st, api, c);
        let auth = k("auth", include_str!("../examples/chat-sharded/auth.mito"), include_str!("../examples/chat-sharded/auth.api"), 2);
        let rooms = k("rooms", include_str!("../examples/chat-sharded/rooms.mito"), include_str!("../examples/chat-sharded/rooms.api"), 3);
        let sys = System::start(Spec { kinds: vec![auth, rooms], entry: 0, inflight: 64, budget: 4 << 30 }).unwrap();
        let login = |n: &str| { let r = sys.request("POST", "/sessions", &[], &format!("{{\"name\":\"{}\"}}", n), T); assert_eq!(r.0, 201, "{}", body(&r)); body(&r).split("\"token\":\"").nth(1).unwrap().split('"').next().unwrap().to_string() };
        let (ann, bob) = (login("ann"), login("bob"));
        assert_ne!(ann.split('.').next(), bob.split('.').next(), "round-robin puts them on different auth shards");
        fn h(t: &str) -> Vec<(&str, &str)> { vec![("X-Session", t)] }
        let mut ids = Vec::new();
        for name in ["a", "b", "c", "d"] { let r = sys.request("POST", "/rooms", &h(&ann), &format!("{{\"name\":\"{}\"}}", name), T); assert_eq!(r.0, 201, "{}", body(&r)); ids.push(body(&r).split("\"id\":").nth(1).unwrap().split(',').next().unwrap().to_string()); }
        assert_eq!(ids, vec!["1", "2", "3", "4"]);
        let list = sys.request("GET", "/rooms", &h(&bob), "", T);
        assert!(body(&list).contains("\"name\":\"d\""), "{}", body(&list));
        for (who, text) in [(&ann, "hi"), (&bob, "hello"), (&ann, "bye")] { assert_eq!(sys.request("POST", "/rooms/2/messages", &h(who), &format!("{{\"body\":\"{}\"}}", text), T).0, 201); }
        let r = sys.request("GET", "/rooms/2/messages", &h(&bob), "", T);
        let b = body(&r);
        assert!(b.find("bye").unwrap() < b.find("hello").unwrap() && b.contains("\"author\":\"ann\""), "{}", b);
        assert_eq!(sys.request("GET", "/rooms/9/messages", &h(&bob), "", T).0, 404);
        assert_eq!(sys.request("POST", "/rooms/3/typing", &h(&ann), "", T).0, 200);
        assert!(body(&sys.request("GET", "/rooms/3/typing", &h(&bob), "", T)).contains("ann"));
        assert_eq!(sys.request("DELETE", "/session", &h(&ann), "", T).0, 200);
        assert_eq!(sys.request("GET", "/rooms/2/messages", &h(&ann), "", T).0, 401);
        assert_eq!(sys.request("GET", "/rooms", &[], "", T).0, 401);
        sys.shutdown();
    }
}
