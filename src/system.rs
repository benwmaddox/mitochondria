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
pub struct Spec { pub kinds: Vec<KindSpec>, pub entry: usize, pub inflight: usize, pub budget: usize, pub spin: Duration, pub batch: usize, pub compile: bool }

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
    let mut spec = Spec { kinds: Vec::new(), entry: usize::MAX, inflight: 4096, budget: 4096 * MB, spin: Duration::ZERO, batch: 32, compile: false }; // sleep at once by default: spinning only pays when there are idle cores
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
            "batch" => { spec.batch = num(next(&mut i)?)?.max(1); next(&mut i)?; }
            "compile" => { spec.compile = match next(&mut i)?.as_str() { "tcc" => true, "off" => false, o => return Err(format!("compile `{}`: expected `tcc` or `off`", o)) }; next(&mut i)?; }
            "spin" => { spec.spin = Duration::from_micros(num(next(&mut i)?)? as u64); next(&mut i)?; }
            o => return Err(format!("expected `worker`, `entry`, `inflight`, `budget`, `spin`, `batch` or `compile`, found `{}`", o)),
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

struct Ring { buf: Vec<u8>, head: usize, len: usize, closed: bool, waiting: bool } // waiting: the consumer is asleep on the condition variable
/// A bounded queue of fixed-size message slots, allocated once.
pub struct Mailbox { ring: Mutex<Ring>, cv: Condvar, cap: usize, slot: usize, refused: AtomicU64, queued: AtomicUsize, spin: Duration }
#[derive(Debug, PartialEq)]
pub enum SendErr { Full, TooBig, Closed }
enum Recv { Got, Idle, Closed }

/// Busy-wait up to `d` for `ready()`, checking the clock only every 64 rounds. Waking a sleeping thread costs far more
/// than a short spin when the next message is about to arrive.
fn spin_for(d: Duration, ready: impl Fn() -> bool) -> bool {
    if d.is_zero() { return ready(); }
    let end = std::time::Instant::now() + d;
    loop {
        for _ in 0..64 { if ready() { return true; } std::hint::spin_loop(); }
        if std::time::Instant::now() >= end { return ready(); }
    }
}

impl Mailbox {
    fn new(cap: usize, slot: usize, spin: Duration) -> Mailbox { Mailbox { ring: Mutex::new(Ring { buf: vec![0; cap * slot], head: 0, len: 0, closed: false, waiting: false }), cv: Condvar::new(), cap, slot, refused: AtomicU64::new(0), queued: AtomicUsize::new(0), spin } }
    /// Put one message in; `fill` writes it into the slot and returns its length (or `None` if it does not fit). Never blocks.
    fn send(&self, fill: impl FnOnce(&mut [u8]) -> Option<usize>) -> Result<(), SendErr> {
        let mut r = self.ring.lock().unwrap();
        if r.closed { return Err(SendErr::Closed); }
        if r.len == self.cap { self.refused.fetch_add(1, Relaxed); return Err(SendErr::Full); }
        let at = ((r.head + r.len) % self.cap) * self.slot;
        let used = fill(&mut r.buf[at + 4..at + self.slot]).ok_or(SendErr::TooBig)?;
        r.buf[at..at + 4].copy_from_slice(&(used as u32).to_le_bytes());
        r.len += 1;
        self.queued.store(r.len, Relaxed);
        let wake = r.waiting; // set under this lock by a consumer that is about to sleep
        drop(r);
        if wake { self.cv.notify_one(); } // a busy consumer will see the message without being signalled
        Ok(())
    }
    /// Take the oldest message into `out`, waiting up to `wait`. After `close`, what is queued is still delivered.
    #[cfg(test)]
    fn recv(&self, out: &mut Vec<u8>, wait: Duration) -> Recv {
        let mut ends = Vec::new();
        let r = self.recv_batch(out, &mut ends, 1, wait);
        if let Recv::Got = r { let n = ends[0]; out.truncate(n); }
        r
    }
    /// Take up to `max` messages under one lock: their bytes are appended to `out`, and `ends` gets where each one ends.
    fn recv_batch(&self, out: &mut Vec<u8>, ends: &mut Vec<usize>, max: usize, wait: Duration) -> Recv {
        spin_for(self.spin, || self.queued.load(Relaxed) > 0);
        let mut r = self.ring.lock().unwrap();
        while r.len == 0 {
            if r.closed { return Recv::Closed; }
            r.waiting = true;
            let (g, t) = self.cv.wait_timeout(r, wait).unwrap();
            r = g;
            r.waiting = false;
            if t.timed_out() && r.len == 0 { return if r.closed { Recv::Closed } else { Recv::Idle }; }
        }
        out.clear(); ends.clear();
        let k = r.len.min(max.max(1));
        for _ in 0..k {
            let at = r.head * self.slot;
            let n = u32::from_le_bytes(r.buf[at..at + 4].try_into().unwrap()) as usize;
            out.extend_from_slice(&r.buf[at + 4..at + 4 + n]);
            ends.push(out.len());
            r.head = (r.head + 1) % self.cap; r.len -= 1;
        }
        self.queued.store(r.len, Relaxed);
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

/// One reply slot. State: 0 free, 1 waiting for an answer, 2 answered, 3 abandoned by a caller that timed out.
struct SlotState { state: u8, code: u16, body: Vec<u8>, sleeping: bool }
struct Slot { st: Mutex<SlotState>, cv: Condvar, answered: std::sync::atomic::AtomicBool }
const FREE_SHARDS: usize = 16;
/// A fixed table of reply slots: the number of requests in flight is bounded by it. Free slots are kept in 16 lists,
/// so callers on different threads rarely touch the same lock.
struct Replies { slots: Vec<Slot>, free: Vec<Mutex<Vec<u32>>>, next: AtomicUsize, spin: Duration }
impl Replies {
    fn new(n: usize, spin: Duration) -> Replies {
        let free = (0..FREE_SHARDS).map(|s| Mutex::new((0..n as u32).filter(|i| *i as usize % FREE_SHARDS == s).rev().collect())).collect();
        Replies { slots: (0..n).map(|_| Slot { st: Mutex::new(SlotState { state: 0, code: 0, body: Vec::with_capacity(4096), sleeping: false }), cv: Condvar::new(), answered: false.into() }).collect(), free, next: AtomicUsize::new(0), spin }
    }
    fn give_back(&self, id: u32) { self.free[id as usize % FREE_SHARDS].lock().unwrap().push(id); }
    fn take(&self) -> Option<u32> {
        let start = self.next.fetch_add(1, Relaxed);
        let id = (0..FREE_SHARDS).find_map(|k| self.free[(start + k) % FREE_SHARDS].lock().unwrap().pop())?;
        let s = &self.slots[id as usize];
        s.answered.store(false, Relaxed);
        s.st.lock().unwrap().state = 1;
        Some(id)
    }
    fn release(&self, id: u32) { self.slots[id as usize].st.lock().unwrap().state = 0; self.give_back(id); }
    /// Answer a request. Each request is answered once; an answer to an abandoned slot just frees it.
    fn put(&self, id: u32, code: u16, body: &[u8]) {
        let Some(s) = self.slots.get(id as usize) else { return };
        let mut g = s.st.lock().unwrap();
        match g.state {
            1 => {
                g.state = 2; g.code = code; g.body.clear(); g.body.extend_from_slice(body);
                let wake = g.sleeping;
                drop(g);
                s.answered.store(true, std::sync::atomic::Ordering::Release);
                if wake { s.cv.notify_one(); } // a caller that has not gone to sleep yet will see the answer itself
            }
            3 => { g.state = 0; drop(g); self.give_back(id); }
            _ => {} // already answered: never happens by construction; ignored rather than double-answering
        }
    }
    fn wait(&self, id: u32, timeout: Duration) -> Option<(u16, Vec<u8>)> {
        let s = &self.slots[id as usize];
        spin_for(self.spin, || s.answered.load(std::sync::atomic::Ordering::Acquire));
        let mut g = s.st.lock().unwrap();
        let deadline = std::time::Instant::now() + timeout;
        while g.state == 1 {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() { g.state = 3; return None; }
            g.sleeping = true;
            g = s.cv.wait_timeout(g, left).unwrap().0;
            g.sleeping = false;
        }
        let out = (g.code, g.body.clone());
        g.state = 0; drop(g);
        self.give_back(id);
        Some(out)
    }
}

// ---------------------------------------------------------------- the system

/// Counters each worker publishes (it counts its own heap allocations, which only its thread can see).
#[derive(Default)]
pub struct WStats { pub handled: AtomicU64, pub forwarded: AtomicU64, pub allocs: AtomicU64, pub restarts: AtomicU64, pub committed: AtomicUsize, pub live: AtomicUsize, pub busy_ns: AtomicU64, pub handler_ns: AtomicU64 }

struct Shared { batch: usize, boxes: Vec<Mailbox>, base: Vec<usize>, counts: Vec<usize>, entry: usize, replies: Replies, stats: Vec<WStats>, names: Vec<String> }

pub struct System { shared: Arc<Shared>, threads: Vec<std::thread::JoinHandle<()>>, rr: AtomicUsize }

pub struct WorkerReport { pub name: String, pub index: usize, pub handled: u64, pub forwarded: u64, pub allocs: u64, pub restarts: u64, pub committed: usize, pub live: usize, pub refused: u64, pub busy_ns: u64, pub handler_ns: u64 }

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
        for k in &spec.kinds { base.push(n); for _ in 0..k.count { boxes.push(Mailbox::new(k.mailbox, k.slot, spec.spin)); } n += k.count; }
        let shared = Arc::new(Shared {
            batch: spec.batch, boxes, base, counts: spec.kinds.iter().map(|k| k.count).collect(), entry: spec.entry, replies: Replies::new(spec.inflight, spec.spin),
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
                v.push(WorkerReport { name: name.clone(), index: i, handled: s.handled.load(Relaxed), forwarded: s.forwarded.load(Relaxed), allocs: s.allocs.load(Relaxed), restarts: s.restarts.load(Relaxed), committed: s.committed.load(Relaxed), live: s.live.load(Relaxed), refused: sh.boxes[w].refused.load(Relaxed), busy_ns: s.busy_ns.load(Relaxed), handler_ns: s.handler_ns.load(Relaxed) });
            }
        }
        v
    }

    /// Close every mailbox; workers answer what is already queued, then stop.
    pub fn shutdown(self) { for b in &self.shared.boxes { b.close(); } for t in self.threads { let _ = t.join(); } }
}

thread_local! { static HANDLER_NS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

fn recycle<'x, 'y>(mut v: Vec<(&'x str, &'x str)>) -> Vec<(&'y str, &'y str)> { v.clear(); unsafe { std::mem::transmute(v) } }

fn worker(sh: Arc<Shared>, spec: Arc<Spec>, k: usize, i: usize) {
    let ks = &spec.kinds[k];
    let info = SysInfo { kinds: spec.kinds.iter().map(|k| (k.name.clone(), k.count)).collect(), me: (k, i) };
    let make = || -> Server {
        let inst = Instance::start(Program::parse(&ks.state).expect("checked at start"), usize::MAX).expect("reserve");
        let mut s = Server::new(inst, &ks.api).expect("checked at start");
        if spec.compile { if let Err(e) = s.compile() { eprintln!("{}{}: compiling endpoints failed, they stay interpreted: {}", ks.name, i, e); } }
        s.system = Some(info.clone());
        if let Some(soft) = ks.soft { s.pressure = Some(Pressure::new(&s.inst, soft, ks.hard.unwrap_or(usize::MAX))); }
        s
    };
    let mut server = make();
    let me = sh.base[k] + i;
    let stats = &sh.stats[me];
    let batch = sh.batch.max(1);
    let mut msgs: Vec<u8> = Vec::with_capacity(ks.slot * batch);
    let mut ends: Vec<usize> = Vec::with_capacity(batch);
    let (mut hv, mut fv): (Vec<(&'static str, &'static str)>, Vec<(&'static str, &'static str)>) = (Vec::with_capacity(32), Vec::with_capacity(32));
    let publish = |s: &Server| { stats.committed.store(s.inst.committed(), Relaxed); };
    loop {
        // take every queued message (up to `batch`) under one lock, then handle them without touching the mailbox
        match sh.boxes[me].recv_batch(&mut msgs, &mut ends, batch, Duration::from_millis(500)) {
            Recv::Idle => { server.idle(); publish(&server); continue; }
            Recv::Closed => break,
            Recv::Got => {}
        }
        let a0 = crate::alloc_count::allocs();
        let busy = std::time::Instant::now();
        let mut start = 0;
        for &end in &ends {
            let msg = &msgs[start..end];
            start = end;
            let r = catch_unwind(AssertUnwindSafe(|| handle(&mut server, msg, &sh, &mut hv, &mut fv)));
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
        }
        stats.busy_ns.fetch_add(busy.elapsed().as_nanos() as u64, Relaxed);
        stats.allocs.fetch_add((crate::alloc_count::allocs() - a0) as u64, Relaxed);
        stats.handled.fetch_add(ends.len() as u64, Relaxed);
        stats.handler_ns.store(HANDLER_NS.with(|c| c.get()), Relaxed);
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
            let t0 = std::time::Instant::now();
            let code = server.handle_req(method, target, &StrFields(&h), body, &StrFields(&f));
            HANDLER_NS.with(|c| c.set(c.get() + t0.elapsed().as_nanos() as u64));
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

// ---------------------------------------------------------------- socket front end

/// Serve a system over HTTP: `conns` threads each accept and handle one connection at a time.
pub fn serve(sys: Arc<System>, port: u16, conns: usize) -> std::io::Result<()> {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind(("127.0.0.1", port))?;
    println!("listening on http://127.0.0.1:{} ({} connection threads)", port, conns);
    let mut ts = Vec::new();
    for _ in 0..conns {
        let (l, sys) = (l.try_clone()?, sys.clone());
        ts.push(std::thread::spawn(move || {
            for conn in l.incoming() {
                let Ok(mut c) = conn else { continue };
                let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
                let (mut data, mut chunk) = (Vec::new(), [0u8; 4096]);
                let he = loop {
                    if let Some(p) = data.windows(4).position(|w| w == b"\r\n\r\n") { break Some(p); }
                    if data.len() > 64 * 1024 { break None; }
                    match c.read(&mut chunk) { Ok(0) | Err(_) => break None, Ok(n) => data.extend_from_slice(&chunk[..n]) }
                };
                let Some(he) = he else { let _ = c.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"); continue };
                let head = String::from_utf8_lossy(&data[..he]).into_owned();
                let mut lines = head.lines();
                let mut rl = lines.next().unwrap_or("").split(' ');
                let (method, target) = (rl.next().unwrap_or("").to_string(), rl.next().unwrap_or("/").to_string());
                let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
                let want: usize = headers.iter().find(|h| h.0.eq_ignore_ascii_case("content-length")).and_then(|h| h.1.parse().ok()).unwrap_or(0).min(1 << 20);
                while data.len() < he + 4 + want { match c.read(&mut chunk) { Ok(0) | Err(_) => break, Ok(n) => data.extend_from_slice(&chunk[..n]) } }
                let body = String::from_utf8_lossy(&data[(he + 4).min(data.len())..]).into_owned();
                let hv: Vec<(&str, &str)> = headers.iter().map(|h| (h.0.as_str(), h.1.as_str())).collect();
                let (code, out) = sys.request(&method, &target, &hv, &body, Duration::from_secs(10));
                let reason = match code { 200 => "OK", 201 => "Created", 204 => "No Content", 400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found", 413 => "Payload Too Large", 429 => "Too Many Requests", 503 => "Service Unavailable", 504 => "Gateway Timeout", _ => "Error" };
                let _ = c.write_all(format!("HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", code, reason, out.len()).as_bytes());
                let _ = c.write_all(&out);
            }
        }));
    }
    for t in ts { let _ = t.join(); }
    Ok(())
}

// ---------------------------------------------------------------- benchmark

fn pct(v: &mut [u64], p: usize) -> f64 { if v.is_empty() { return 0.0; } v.sort_unstable(); v[(v.len() * p / 100).min(v.len() - 1)] as f64 / 1000.0 }
fn xorshift(r: &mut u64) -> u64 { *r ^= *r << 13; *r ^= *r >> 7; *r ^= *r << 17; *r }

/// The sharded chat under load from `clients` threads, for each (auth, rooms) worker count in `configs`.
/// Also measures one plain `Server` on one thread running the same mix (examples/chat), as the no-mailbox baseline.
pub fn bench(src: &str, dir: &Path, clients: usize, configs: &[(usize, usize)], ops: usize, reps: usize, spin: Option<u64>, batch: Option<usize>, compile: bool) {
    use std::time::Instant;
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    println!("logical cores: {}; client threads: {}; {} mixed requests per run (70% GET messages, 20% POST message, 5% POST typing, 5% GET typing); runs per config: {}", cores, clients, ops, reps);
    let body = "{\"body\":\"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.\"}";

    // baseline: one Server, one thread, no mailboxes
    {
        let prog = Program::parse(include_str!("../examples/chat/chat.mito")).unwrap();
        let mut s = Server::new(Instance::start(prog, usize::MAX).unwrap(), include_str!("../examples/chat/chat.api")).unwrap();
        let _ = s.inst.resolve("meta.rate").and_then(|l| s.inst.loc_set(&l, &crate::runtime::Val::Int(1_000_000)));
        let mut toks = Vec::new();
        for i in 0..1000 { s.handle_raw("POST", "/sessions", &[], &format!("{{\"name\":\"u{}\"}}", i)); toks.push(vec![("X-Session".to_string(), s.response().split("\"token\":\"").nth(1).unwrap().split('"').next().unwrap().to_string())]); }
        for i in 0..20 { s.handle_raw("POST", "/rooms", &toks[0], &format!("{{\"name\":\"r{}\"}}", i)); }
        let posts: Vec<String> = (0..20).map(|r| format!("/rooms/{}/messages", r)).collect();
        let gets: Vec<String> = (0..20).map(|r| format!("/rooms/{}/messages?limit=50", r)).collect();
        let typ: Vec<String> = (0..20).map(|r| format!("/rooms/{}/typing", r)).collect();
        let mut rng = 7u64;
        for _ in 0..10_000 { let (u, r) = (xorshift(&mut rng) as usize % 1000, xorshift(&mut rng) as usize % 20); s.handle_raw("POST", &posts[r], &toks[u], body); }
        let t = Instant::now();
        for _ in 0..ops {
            let (u, r, k) = (xorshift(&mut rng) as usize % 1000, xorshift(&mut rng) as usize % 20, xorshift(&mut rng) % 100);
            if k < 70 { s.handle_raw("GET", &gets[r], &toks[u], ""); } else if k < 90 { s.handle_raw("POST", &posts[r], &toks[u], body); } else if k < 95 { s.handle_raw("POST", &typ[r], &toks[u], ""); } else { s.handle_raw("GET", &typ[r], &toks[u], ""); }
        }
        let secs = t.elapsed().as_secs_f64();
        println!("\n  baseline: one Server on one thread, direct calls, no mailboxes: {:.0} req/s ({:.1} us mean)", ops as f64 / secs, secs * 1e6 / ops as f64);
    }

    println!("\n  {:<12} {:>4} {:>10} {:>9} {:>9} {:>9} {:>9} {:>9}  {}", "auth x rooms", "run", "req/s", "GET p50", "GET p99", "POST p50", "POST p99", "w-allocs", "status codes");
    for &(a, r) in configs {
        for rep in 0..reps {
            let mut spec = parse_spec(src, dir).expect("system file");
            for k in &mut spec.kinds { if k.name == "auth" { k.count = a; } if k.name == "rooms" { k.count = r; } }
            if let Some(us) = spin { spec.spin = Duration::from_micros(us); }
            if let Some(b) = batch { spec.batch = b.max(1); }
            spec.compile = compile;
            if rep == 0 && a == configs[0].0 && r == configs[0].1 { println!("  spin before sleeping: {} us; batch: {} messages per lock; endpoints {}", spec.spin.as_micros(), spec.batch, if spec.compile { "compiled with TinyCC" } else { "interpreted" }); }
            let sys = System::start(spec).expect("start");
            let to = Duration::from_secs(10);
            let toks: Vec<String> = (0..3000).map(|i| { let res = sys.request("POST", "/sessions", &[], &format!("{{\"name\":\"u{}\"}}", i), to); String::from_utf8_lossy(&res.1).split("\"token\":\"").nth(1).unwrap_or("").split('"').next().unwrap_or("").to_string() }).collect();
            let ids: Vec<usize> = (0..20).map(|i| { let res = sys.request("POST", "/rooms", &[("X-Session", toks[0].as_str())], &format!("{{\"name\":\"r{}\"}}", i), to); String::from_utf8_lossy(&res.1).split("\"id\":").nth(1).and_then(|x| x.split(',').next()).and_then(|x| x.parse().ok()).unwrap_or(1) }).collect();
            let posts: Vec<String> = ids.iter().map(|r| format!("/rooms/{}/messages", r)).collect();
            let gets: Vec<String> = ids.iter().map(|r| format!("/rooms/{}/messages?limit=50", r)).collect();
            let typ: Vec<String> = ids.iter().map(|r| format!("/rooms/{}/typing", r)).collect();
            let run = |n: usize, mix: bool| -> (f64, Vec<(u8, u64, u16)>) {
                let t = Instant::now();
                let parts: Vec<Vec<(u8, u64, u16)>> = std::thread::scope(|sc| {
                    let hs: Vec<_> = (0..clients).map(|c| { let (sys, toks, posts, gets, typ) = (&sys, &toks, &posts, &gets, &typ); sc.spawn(move || {
                        let mut rng = 0x9E37_79B9_7F4A_7C15u64 ^ ((c as u64 + 1) * 0x1234_5678);
                        let mut out = Vec::with_capacity(n / clients + 1);
                        for _ in 0..n / clients {
                            let (u, r, k) = (xorshift(&mut rng) as usize % toks.len(), xorshift(&mut rng) as usize % posts.len(), if mix { xorshift(&mut rng) % 100 } else { 80 });
                            let h = [("X-Session", toks[u].as_str())];
                            let t0 = Instant::now();
                            let (kind, code) = if k < 70 { (0u8, sys.request("GET", &gets[r], &h, "", to).0) } else if k < 90 { (1, sys.request("POST", &posts[r], &h, body, to).0) } else if k < 95 { (2, sys.request("POST", &typ[r], &h, "", to).0) } else { (3, sys.request("GET", &typ[r], &h, "", to).0) };
                            out.push((kind, t0.elapsed().as_nanos() as u64, code));
                        }
                        out
                    }) }).collect();
                    hs.into_iter().map(|h| h.join().unwrap()).collect()
                });
                (t.elapsed().as_secs_f64(), parts.into_iter().flatten().collect())
            };
            run(10_000, false); // fill the rooms with messages
            let rep0 = sys.report();
            let before = rep0.iter().fold((0u64, 0u64), |s, w| (s.0 + w.allocs, s.1 + w.handled));
            let (secs, all) = run(ops, true);
            let after = sys.report().iter().fold((0u64, 0u64), |s, w| (s.0 + w.allocs, s.1 + w.handled));
            let mut by: Vec<Vec<u64>> = vec![Vec::new(); 4];
            let mut codes: std::collections::BTreeMap<u16, usize> = Default::default();
            for (k, ns, c) in &all { by[*k as usize].push(*ns); *codes.entry(*c).or_default() += 1; }
            let wa = (after.0 - before.0) as f64 / (after.1 - before.1).max(1) as f64;
            let rep1 = sys.report();
            let mut util = String::new();
            for kind in ["auth", "rooms"] {
                let ws: Vec<(&WorkerReport, &WorkerReport)> = rep0.iter().zip(rep1.iter()).filter(|(x, _)| x.name == kind).collect();
                let busy: Vec<f64> = ws.iter().map(|(x, y)| (y.busy_ns - x.busy_ns) as f64 / 1e9 / secs * 100.0).collect();
                let hand: f64 = ws.iter().map(|(x, y)| (y.handler_ns - x.handler_ns) as f64).sum::<f64>() / ws.iter().map(|(x, y)| (y.handled - x.handled) as f64).sum::<f64>().max(1.0) / 1000.0;
                let per: f64 = ws.iter().map(|(x, y)| (y.busy_ns - x.busy_ns) as f64).sum::<f64>() / ws.iter().map(|(x, y)| (y.handled - x.handled) as f64).sum::<f64>().max(1.0) / 1000.0;
                util += &format!("  {} busy {:.0}-{:.0}% ({:.1} us/msg, handler {:.1} us)", kind, busy.iter().cloned().fold(f64::MAX, f64::min), busy.iter().cloned().fold(0.0, f64::max), per, hand);
            }
            println!("  {:<12} {:>4} {:>10.0} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.2}  {:?}", format!("{} x {}", a, r), rep + 1, all.len() as f64 / secs, pct(&mut by[0], 50), pct(&mut by[0], 99), pct(&mut by[1], 50), pct(&mut by[1], 99), wa, codes);
            println!("             {}", util);
            sys.shutdown();
        }
    }
    println!("  (latencies in us, measured by the client threads, including the trip through the mailboxes; w-allocs = heap allocations per message handled inside workers)");
}

#[cfg(test)]
mod tests {
    use super::*;
    const T: Duration = Duration::from_secs(5);

    fn kind(name: &str, state: &str, api: &str, count: usize) -> KindSpec { KindSpec { name: name.into(), state: state.into(), api: api.into(), count, mailbox: 64, slot: 4096, soft: None, hard: None } }
    fn body(r: &(u16, Vec<u8>)) -> String { String::from_utf8_lossy(&r.1).into_owned() }

    #[test]
    fn forwards_reach_the_right_shard_in_order_and_each_request_is_answered_once() {
        for compile in [false, true] {
            let front = kind("front", "struct M { n: i64; } global m: M;", "endpoint POST /inc/{k} { forward count[int(param(\"k\"))] { \"from\": worker_index() }; }", 2);
            let count = kind("count", "struct C { n: i64; } global c: C;", "endpoint POST /inc/{k} { c.n = c.n + 1; respond 200 { \"shard\": worker_index(), \"n\": c.n, \"from\": int(fwd(\"from\")) }; }", 3);
            let sys = System::start(Spec { kinds: vec![front, count], entry: 0, inflight: 16, budget: 1 << 30, spin: Duration::ZERO, batch: 8, compile }).unwrap();
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
    }

    #[test]
    fn a_panicking_worker_is_restarted_with_fresh_state() {
        for compile in [false, true] {
            let w = kind("w", "struct C { n: i64; } global c: C;", "endpoint POST /inc { c.n = c.n + 1; respond 200 { \"n\": c.n }; } endpoint POST /crash { c.n = 99; __crash(); }", 1);
            let sys = System::start(Spec { kinds: vec![w], entry: 0, inflight: 4, budget: 1 << 30, spin: Duration::ZERO, batch: 8, compile }).unwrap();
            assert_eq!(body(&sys.request("POST", "/inc", &[], "", T)), "{\"n\":1}");
            assert_eq!(body(&sys.request("POST", "/inc", &[], "", T)), "{\"n\":2}");
            assert_eq!(sys.request("POST", "/crash", &[], "", T).0, 500);
            assert_eq!(body(&sys.request("POST", "/inc", &[], "", T)), "{\"n\":1}"); // state was rebuilt
            assert_eq!(sys.report()[0].restarts, 1);
            sys.shutdown();
        }
    }

    #[test]
    fn mailbox_full_and_too_big_are_typed() {
        let m = Mailbox::new(2, 64, Duration::ZERO);
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
        let r = Replies::new(2, Duration::ZERO);
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
        for compile in [false, true] {
            let k = |n: &str, st: &str, api: &str, c| kind(n, st, api, c);
            let auth = k("auth", include_str!("../examples/chat-sharded/auth.mito"), include_str!("../examples/chat-sharded/auth.api"), 2);
            let rooms = k("rooms", include_str!("../examples/chat-sharded/rooms.mito"), include_str!("../examples/chat-sharded/rooms.api"), 3);
            let sys = System::start(Spec { kinds: vec![auth, rooms], entry: 0, inflight: 64, budget: 4 << 30, spin: Duration::ZERO, batch: 8, compile }).unwrap();
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
}

#[cfg(test)]
mod compile_tests {
    use super::*;
    /// The worker tests run in both modes, but a worker whose compile fails only warns and stays interpreted;
    /// this checks the sharded chat really compiles.
    #[test]
    fn sharded_chat_endpoints_compile() {
        let info = SysInfo { kinds: vec![("auth".into(), 2), ("rooms".into(), 3)], me: (0, 0) };
        for (state, api, want) in [
            (include_str!("../examples/chat-sharded/auth.mito"), include_str!("../examples/chat-sharded/auth.api"), 9),
            (include_str!("../examples/chat-sharded/rooms.mito"), include_str!("../examples/chat-sharded/rooms.api"), 5),
        ] {
            let inst = Instance::start(Program::parse(state).unwrap(), usize::MAX).unwrap();
            let mut s = Server::new(inst, api).unwrap();
            s.system = Some(info.clone());
            let (notes, _) = s.compile().unwrap();
            let n = notes.iter().filter(|x| x.ends_with(": compiled")).count();
            assert_eq!(n, want, "{:#?}", notes);
        }
    }
}
