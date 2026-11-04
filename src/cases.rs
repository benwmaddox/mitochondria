//! Cases (`tests/cases/*.case`): a state, an api and a request script each. Every case runs on every backend; the logs
//! must match each other and the recorded `.out` file, and every endpoint must compile. Warm successful requests must
//! make no heap allocations on a compiled backend. `MITO_BLESS=1 cargo test cases` records the reference backend's log.
//!
//! A case is one program (`--- state`, `--- api`) or a system of workers (`--- system` holding a `.system` file, whose
//! programs are `--- file name` sections). In any section, `include path` takes a file (relative to the repository).
//! Request script lines:
//! `METHOD target [json body]`, optionally ending in `=> name` to remember the response's `"token"` as `$name`;
//! `*N` before a request repeats it N times; `@Header: value` adds a header to the next request; `clock ms` sets the
//! request clock; `marks soft hard` sets the memory pressure marks (bytes); `stats` logs the pressure counters (for a
//! system: what each worker handled, forwarded and how often it was restarted).
//! `$name` is replaced anywhere in a line. `allocates target` exempts requests to that target from the allocation
//! check (a known limit of the current runtime, to be removed with it).
use crate::layout::Program;
use crate::runtime::{Instance, Pressure};
use crate::serve::Server;

pub struct Case { pub name: String, pub state: String, pub api: String, pub requests: String, pub system: Option<String>, pub files: Vec<(String, String)> }

pub fn load(path: &std::path::Path) -> Case {
    let text = std::fs::read_to_string(path).unwrap();
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.trim_end().strip_prefix("--- ") { sections.push((name.to_string(), String::new())); continue; }
        let Some(cur) = sections.last_mut() else { continue };
        match line.strip_prefix("include ") {
            Some(f) if cur.0 != "requests" => cur.1 += &std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(f.trim())).unwrap_or_else(|e| panic!("{}: {}", f, e)),
            _ => cur.1 += line,
        }
        cur.1.push('\n');
    }
    let get = |n: &str| sections.iter().find(|s| s.0 == n).map(|s| s.1.clone());
    let files = sections.iter().filter_map(|s| s.0.strip_prefix("file ").map(|f| (f.trim().to_string(), s.1.clone()))).collect();
    // modules are expanded once here, so every backend compiles the same program
    let (state, api) = crate::modules::expand(&get("state").unwrap_or_default(), &get("api").unwrap_or_default()).unwrap_or_else(|e| panic!("{}: {}", path.display(), e));
    Case { name: path.file_stem().unwrap().to_string_lossy().into_owned(), state, api,
        requests: get("requests").unwrap_or_default(), system: get("system"), files }
}

/// A running program, as the cases drive it. Every way of running a program implements this, so the same scripts and
/// records check each one.
pub trait Backend {
    fn handle(&mut self, method: &str, target: &str, headers: &[(String, String)], body: &str) -> (u16, String);
    /// Status only; the response stays in the backend (no copy, so allocations can be counted).
    fn handle_quiet(&mut self, method: &str, target: &str, headers: &[(String, String)], body: &str) -> u16;
    fn set_clock(&mut self, ms: i64);
    fn set_marks(&mut self, soft: usize, hard: usize);
    /// `state <path> <live bytes>` for the plain values and every collection, then the memory totals.
    fn state(&self) -> Vec<String>;
    /// Pressure counters, as one log line.
    fn stats(&self) -> String;
    /// One line per endpoint saying whether it compiled.
    fn notes(&self) -> Vec<String>;
    /// Does this backend promise warm requests without heap allocations?
    fn no_alloc(&self) -> bool;
    /// Heap allocations made handling requests so far, counted by the backend itself (the Rust counter does not see
    /// another runtime's heap: a C backend counts its own calls, or makes none after it starts).
    fn allocs(&self) -> usize;
}

/// The Rust runtime, with endpoints interpreted or compiled.
pub struct RustBackend { s: Server, notes: Vec<String>, compiled: bool }
impl RustBackend {
    pub fn start(case: &Case, compiled: bool) -> RustBackend {
        let inst = Instance::start(Program::parse(&case.state).unwrap_or_else(|e| panic!("{}: state: {}", case.name, e)), 1 << 30).unwrap();
        let mut s = Server::new(inst, &case.api).unwrap_or_else(|e| panic!("{}: api: {}", case.name, e));
        s.clock = Some(1000);
        s.rng.set(0x1234_5678_9abc_def0);
        let notes = if compiled { s.compile().unwrap_or_else(|e| panic!("{}: compile: {}", case.name, e)).0 } else { Vec::new() };
        RustBackend { s, notes, compiled }
    }
}
impl Backend for RustBackend {
    fn handle(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> (u16, String) { self.s.handle(m, t, h, b) }
    fn handle_quiet(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> u16 { self.s.handle_raw(m, t, h, b) }
    fn set_clock(&mut self, ms: i64) { self.s.clock = Some(ms); }
    fn set_marks(&mut self, soft: usize, hard: usize) { self.s.pressure = Some(Pressure::new(&self.s.inst, soft, hard)); }
    fn state(&self) -> Vec<String> {
        let i = &self.s.inst;
        let mut v: Vec<String> = i.inspect().into_iter().map(|r| format!("state {} {}", r.0, r.4)).collect();
        v.push(format!("memory reserved={} committed={} live={}", i.reserved(), i.committed(), i.live_total()));
        v
    }
    fn stats(&self) -> String { match &self.s.pressure { Some(p) => format!("stats refusing={} full_trims={}", p.refusing, p.full_trims), None => "stats no marks".into() } }
    fn notes(&self) -> Vec<String> { self.notes.clone() }
    fn no_alloc(&self) -> bool { self.compiled }
    fn allocs(&self) -> usize { crate::alloc_count::allocs() }
}

/// The C runtime (runtime/mito.c) and the program's generated C compiled together, driven through `mito_*`: in memory
/// by TinyCC, or by clang (-O2) into a DLL.
pub struct CBackend { _code: Code, m: *mut std::ffi::c_void, f: CFns, pairs: Vec<MitoPair> }
/// A header or forwarded field, as runtime/types.h declares it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MitoPair { pub k: *const u8, pub kn: u32, pub v: *const u8, pub vn: u32 }
enum Code { Tcc(crate::tcc::Unit), Dll(*mut std::ffi::c_void) }
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut std::ffi::c_void;
    fn GetProcAddress(m: *mut std::ffi::c_void, name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
}
/// clang's undefined-behaviour checks in trap mode (no runtime library: a violation stops the process at once), and
/// array bounds: overflow of signed integers, shifts, out-of-range indexes into the C runtime's fixed arrays.
pub const UBSAN: &[&str] = &["-O1", "-fsanitize=undefined,bounds", "-fsanitize-trap=undefined,bounds"];
pub use crate::build::clang;
/// Build `src` with clang into a DLL (in the temp directory, named by `name`) and load it.
pub fn clang_dll(src: &str, name: &str, extra: &[&str]) -> Result<*mut std::ffi::c_void, String> { clang_dll_units(&[src], name, extra) }
/// The same for several translation units built into one DLL.
pub fn clang_dll_units(srcs: &[&str], name: &str, extra: &[&str]) -> Result<*mut std::ffi::c_void, String> {
    let cc = clang().ok_or("clang not found")?;
    let dir = std::env::temp_dir().join("mito-clang");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let stem = format!("{}-{}-{}", name, std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let dll = dir.join(format!("{}.dll", stem));
    let mut cs = Vec::new();
    for (k, src) in srcs.iter().enumerate() { let c = dir.join(format!("{}-{}.c", stem, k)); std::fs::write(&c, src).map_err(|e| e.to_string())?; cs.push(c); }
    let out = std::process::Command::new(cc).args(["-O2", "-shared", "-fno-strict-aliasing", "-w"]).args(extra).arg("-o").arg(&dll).args(&cs).arg("-lkernel32")
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(format!("clang failed:\n{}", String::from_utf8_lossy(&out.stderr))); }
    let w: Vec<u16> = dll.to_string_lossy().encode_utf16().chain([0]).collect();
    let h = unsafe { LoadLibraryW(w.as_ptr()) };
    if h.is_null() { return Err(format!("could not load {}", dll.display())); }
    Ok(h)
}
struct CFns {
    handle: unsafe extern "C" fn(*mut std::ffi::c_void, *const u8, u32, *const u8, u32, *const MitoPair, u32, *const MitoPair, u32, *const u8, u32) -> i32,
    response: unsafe extern "C" fn(*mut std::ffi::c_void, *mut u32) -> *const u8,
    set_clock: unsafe extern "C" fn(*mut std::ffi::c_void, i64, i32),
    set_marks: unsafe extern "C" fn(*mut std::ffi::c_void, u64, u64),
    state: unsafe extern "C" fn(*mut std::ffi::c_void, *mut u8, u32) -> u32,
    stats: unsafe extern "C" fn(*mut std::ffi::c_void, *mut u8, u32) -> u32,
    stop: unsafe extern "C" fn(*mut std::ffi::c_void),
}
impl CBackend {
    pub fn start(case: &Case) -> CBackend { Self::start_with(case, false) }
    pub fn start_with(case: &Case, use_clang: bool) -> CBackend { Self::start_flags(case, use_clang, &[]) }
    pub fn start_flags(case: &Case, use_clang: bool, flags: &[&str]) -> CBackend {
        let prog = Program::parse(&case.state).unwrap_or_else(|e| panic!("{}: state: {}", case.name, e));
        let mut api = crate::lang::parse(&case.api).unwrap_or_else(|e| panic!("{}: api: {}", case.name, e));
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap_or_else(|e| panic!("{}: api: {}", case.name, e));
        let src = crate::cgen::program_c(&prog, &api, None).unwrap_or_else(|e| panic!("{}: {}", case.name, e));
        // the C runtime makes no heap allocations because it has no allocator: check the program names none
        for w in ["malloc", "calloc", "realloc", "HeapAlloc", "LocalAlloc", "GlobalAlloc", "_alloca"] { assert!(!src.contains(w), "{}: the C program uses {}", case.name, w); }
        let code = if use_clang { Code::Dll(clang_dll(&src, &case.name, flags).unwrap_or_else(|e| panic!("{}: {}", case.name, e))) }
            else { Code::Tcc(crate::tcc::Unit::compile(&src, &[]).unwrap_or_else(|e| { let _ = std::fs::write(std::env::temp_dir().join(format!("mito-{}.c", case.name)), &src); panic!("{}: C runtime: {}", case.name, e) })) };
        let sym = |n: &str| -> *const std::ffi::c_void {
            let p = match &code { Code::Tcc(u) => u.symbol(n), Code::Dll(h) => { let c = std::ffi::CString::new(n).unwrap(); let p = unsafe { GetProcAddress(*h, c.as_ptr()) }; if p.is_null() { None } else { Some(p as *const std::ffi::c_void) } } };
            p.unwrap_or_else(|| panic!("no {}", n))
        };
        unsafe {
            let start: unsafe extern "C" fn(u64, u64) -> *mut std::ffi::c_void = std::mem::transmute(sym("mito_start"));
            let f = CFns { handle: std::mem::transmute(sym("mito_handle")), response: std::mem::transmute(sym("mito_response")), set_clock: std::mem::transmute(sym("mito_set_clock")),
                set_marks: std::mem::transmute(sym("mito_set_marks")), state: std::mem::transmute(sym("mito_state")), stats: std::mem::transmute(sym("mito_stats")), stop: std::mem::transmute(sym("mito_stop")) };
            let m = start(1 << 30, 0x1234_5678_9abc_def0);
            assert!(!m.is_null(), "{}: mito_start failed", case.name);
            (f.set_clock)(m, 1000, 1);
            CBackend { _code: code, m, f, pairs: Vec::with_capacity(64) }
        }
    }
    fn call(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> i32 {
        self.pairs.clear();
        for (k, v) in h { self.pairs.push(MitoPair { k: k.as_ptr(), kn: k.len() as u32, v: v.as_ptr(), vn: v.len() as u32 }); }
        unsafe { (self.f.handle)(self.m, m.as_ptr(), m.len() as u32, t.as_ptr(), t.len() as u32, self.pairs.as_ptr(), self.pairs.len() as u32, std::ptr::null(), 0, b.as_ptr(), b.len() as u32) }
    }
    fn text(&self, f: unsafe extern "C" fn(*mut std::ffi::c_void, *mut u8, u32) -> u32) -> String {
        let mut b = vec![0u8; 1 << 20];
        let n = unsafe { f(self.m, b.as_mut_ptr(), b.len() as u32) } as usize;
        String::from_utf8_lossy(&b[..n]).into_owned()
    }
}
impl Drop for CBackend { fn drop(&mut self) { unsafe { (self.f.stop)(self.m) } } }
impl Backend for CBackend {
    fn handle(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> (u16, String) {
        let code = self.call(m, t, h, b);
        let mut n = 0u32;
        let p = unsafe { (self.f.response)(self.m, &mut n) };
        let body = unsafe { std::slice::from_raw_parts(p, n as usize) };
        (code as u16, std::str::from_utf8(body).unwrap_or("").to_string())
    }
    fn handle_quiet(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> u16 { self.call(m, t, h, b) as u16 }
    fn set_clock(&mut self, ms: i64) { unsafe { (self.f.set_clock)(self.m, ms, 1) } }
    fn set_marks(&mut self, soft: usize, hard: usize) { unsafe { (self.f.set_marks)(self.m, soft as u64, hard as u64) } }
    fn state(&self) -> Vec<String> { self.text(self.f.state).lines().map(str::to_string).collect() }
    fn stats(&self) -> String { self.text(self.f.stats) }
    fn notes(&self) -> Vec<String> { Vec::new() } // program_c fails unless every endpoint compiles
    fn no_alloc(&self) -> bool { true }
    /// The C runtime has no allocator at all: it declares no malloc, and every buffer is a reservation made at start.
    fn allocs(&self) -> usize { 0 }
}

/// A system of worker threads in C (runtime/system.c with each kind's program), built by TinyCC or clang.
pub struct CSystemBackend { _code: Code, s: *mut std::ffi::c_void, f: SysFns, out: Vec<u8> }
struct SysFns {
    request: unsafe extern "C" fn(*mut std::ffi::c_void, *const u8, u32, *const u8, u32, *const MitoPair, u32, *const u8, u32, u32, *mut u8, u32, *mut u32) -> i32,
    set_clock: unsafe extern "C" fn(*mut std::ffi::c_void, i64, i32),
    report: unsafe extern "C" fn(*mut std::ffi::c_void, *mut u8, u32) -> u32,
    stop: unsafe extern "C" fn(*mut std::ffi::c_void),
}
impl CSystemBackend {
    pub fn start(case: &Case, use_clang: bool, flags: &[&str]) -> CSystemBackend {
        let read = |f: &str| case.files.iter().find(|x| x.0 == f).map(|x| x.1.clone()).ok_or_else(|| format!("no `--- file {}` section", f));
        let spec = crate::system::parse_spec_with(case.system.as_deref().unwrap(), &read).unwrap_or_else(|e| panic!("{}: system: {}", case.name, e));
        let units = crate::cgen::system_c(&spec).unwrap_or_else(|e| panic!("{}: {}", case.name, e));
        let refs: Vec<&str> = units.iter().map(|s| s.as_str()).collect();
        let code = if use_clang { Code::Dll(clang_dll_units(&refs, &case.name, flags).unwrap_or_else(|e| panic!("{}: {}", case.name, e))) }
            else { Code::Tcc(crate::tcc::Unit::compile_units(&refs, &[]).unwrap_or_else(|e| panic!("{}: C system: {}", case.name, e))) };
        let sym = |n: &str| -> *const std::ffi::c_void {
            let p = match &code { Code::Tcc(u) => u.symbol(n), Code::Dll(h) => { let c = std::ffi::CString::new(n).unwrap(); let p = unsafe { GetProcAddress(*h, c.as_ptr()) }; if p.is_null() { None } else { Some(p as *const std::ffi::c_void) } } };
            p.unwrap_or_else(|| panic!("no {}", n))
        };
        unsafe {
            let start: unsafe extern "C" fn(u64, i32) -> *mut std::ffi::c_void = std::mem::transmute(sym("mito_system_start"));
            let f = SysFns { request: std::mem::transmute(sym("mito_system_request")), set_clock: std::mem::transmute(sym("mito_system_set_clock")), report: std::mem::transmute(sym("mito_system_report")), stop: std::mem::transmute(sym("mito_system_stop")) };
            let s = start(0x1234_5678_9abc_def0, 1);
            assert!(!s.is_null(), "{}: mito_system_start failed", case.name);
            (f.set_clock)(s, 1000, 1);
            CSystemBackend { _code: code, s, f, out: vec![0; 300 * 1024] }
        }
    }
}
impl Drop for CSystemBackend { fn drop(&mut self) { unsafe { (self.f.stop)(self.s) } } }
impl Backend for CSystemBackend {
    fn handle(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> (u16, String) {
        let pairs: Vec<MitoPair> = h.iter().map(|(k, v)| MitoPair { k: k.as_ptr(), kn: k.len() as u32, v: v.as_ptr(), vn: v.len() as u32 }).collect();
        let mut n = 0u32;
        let code = unsafe { (self.f.request)(self.s, m.as_ptr(), m.len() as u32, t.as_ptr(), t.len() as u32, pairs.as_ptr(), pairs.len() as u32, b.as_ptr(), b.len() as u32, 10_000, self.out.as_mut_ptr(), self.out.len() as u32, &mut n) };
        (code as u16, String::from_utf8_lossy(&self.out[..n as usize]).into_owned())
    }
    fn handle_quiet(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> u16 { self.handle(m, t, h, b).0 }
    fn set_clock(&mut self, ms: i64) { unsafe { (self.f.set_clock)(self.s, ms, 1) } }
    fn set_marks(&mut self, _: usize, _: usize) { panic!("marks are set per worker kind in the system file"); }
    fn state(&self) -> Vec<String> { Vec::new() }
    fn stats(&self) -> String {
        let mut b = vec![0u8; 1 << 16];
        let n = unsafe { (self.f.report)(self.s, b.as_mut_ptr(), b.len() as u32) } as usize;
        String::from_utf8_lossy(&b[..n]).lines().map(|l| { let p: Vec<&str> = l.split(' ').collect(); format!("{}{} handled={} forwarded={} restarts={}", p[0], p[1], p[2], p[3], p[4]) }).collect::<Vec<_>>().join(" | ")
    }
    fn notes(&self) -> Vec<String> { Vec::new() }
    fn no_alloc(&self) -> bool { true }
    fn allocs(&self) -> usize { 0 }
}

/// A system of worker threads, from the Rust runtime.
pub struct SystemBackend { sys: Option<crate::system::System>, notes: Vec<String>, compiled: bool }
impl SystemBackend {
    pub fn start(case: &Case, compiled: bool) -> SystemBackend {
        let read = |f: &str| case.files.iter().find(|x| x.0 == f).map(|x| x.1.clone()).ok_or_else(|| format!("no `--- file {}` section", f));
        let mut spec = crate::system::parse_spec_with(case.system.as_deref().unwrap(), &read).unwrap_or_else(|e| panic!("{}: system: {}", case.name, e));
        spec.compile = compiled;
        spec.seed = Some(0x1234_5678_9abc_def0);
        let notes = if compiled { spec.kinds.iter().map(|k| format!("{}: compiled", k.name)).collect() } else { Vec::new() }; // start fails unless every endpoint compiles
        let sys = crate::system::System::start(spec).unwrap_or_else(|e| panic!("{}: {}", case.name, e));
        sys.set_clock(Some(1000));
        SystemBackend { sys: Some(sys), notes, compiled }
    }
    fn sys(&self) -> &crate::system::System { self.sys.as_ref().unwrap() }
}
impl Drop for SystemBackend { fn drop(&mut self) { if let Some(s) = self.sys.take() { s.shutdown(); } } }
impl Backend for SystemBackend {
    fn handle(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> (u16, String) {
        let hv: Vec<(&str, &str)> = h.iter().map(|x| (x.0.as_str(), x.1.as_str())).collect();
        let (code, out) = self.sys().request(m, t, &hv, b, std::time::Duration::from_secs(10));
        (code, String::from_utf8_lossy(&out).into_owned())
    }
    fn handle_quiet(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> u16 { self.handle(m, t, h, b).0 }
    fn set_clock(&mut self, ms: i64) { self.sys().set_clock(Some(ms)); }
    fn set_marks(&mut self, _: usize, _: usize) { panic!("marks are set per worker kind in the system file"); }
    fn state(&self) -> Vec<String> { Vec::new() }
    fn stats(&self) -> String {
        self.sys().report().iter().map(|w| format!("{}{} handled={} forwarded={} restarts={}", w.name, w.index, w.handled, w.forwarded, w.restarts)).collect::<Vec<_>>().join(" | ")
    }
    fn notes(&self) -> Vec<String> { self.notes.clone() }
    fn no_alloc(&self) -> bool { self.compiled }
    fn allocs(&self) -> usize { self.sys().report().iter().map(|w| w.allocs as usize).sum() } // counted inside the workers, before each answer
}

/// Run a case's script; returns the log, and the heap allocations made by warm successful requests (every successful
/// request of the script replayed twice after it, the second time counted).
pub fn run(case: &Case, be: &mut dyn Backend) -> (Vec<String>, usize) {
    let mut log = Vec::new();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut ok: Vec<(String, String, Vec<(String, String)>, String)> = Vec::new();
    let mut vars: Vec<(String, String)> = Vec::new();
    let mut exempt: Vec<String> = Vec::new();
    for line in case.requests.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("//")) {
        let mut line = line.to_string();
        for (k, v) in vars.iter().rev() { line = line.replace(&format!("${}", k), v); } // later names first, so $a does not eat $ab
        if let Some(h) = line.strip_prefix('@') { let (k, v) = h.split_once(':').unwrap(); headers.push((k.trim().into(), v.trim().into())); continue; }
        if let Some(t) = line.strip_prefix("clock ") { be.set_clock(t.trim().parse().unwrap()); continue; }
        if let Some(t) = line.strip_prefix("marks ") { let (a, b) = t.trim().split_once(' ').unwrap(); be.set_marks(a.parse().unwrap(), b.parse().unwrap()); continue; }
        if line == "stats" { log.push(be.stats()); continue; }
        if let Some(t) = line.strip_prefix("allocates ") { exempt.push(t.trim().to_string()); continue; }
        let (times, line) = match line.strip_prefix('*') { Some(r) => { let (n, rest) = r.split_once(' ').unwrap(); (n.parse::<usize>().unwrap(), rest.to_string()) } None => (1, line) };
        let (line, capture) = match line.rsplit_once(" => ") { Some((l, n)) => (l.to_string(), Some(n.trim().to_string())), None => (line, None) };
        let (m, rest) = line.split_once(' ').unwrap();
        let (target, body) = rest.split_once(' ').unwrap_or((rest, ""));
        for _ in 0..times {
            let (code, out) = be.handle(m, target, &headers, body);
            log.push(format!("{} {} -> {} {}", m, target, code, out));
            if (200..300).contains(&code) && !exempt.iter().any(|e| e == target) { ok.push((m.into(), target.into(), headers.clone(), body.into())); }
            if let Some(n) = &capture {
                let tok = out.split("\"token\":\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or_else(|| panic!("{}: no token in {}", case.name, out));
                vars.push((n.clone(), tok.to_string()));
            }
        }
        headers.clear();
    }
    log.extend(be.state());
    // allocations: state has moved on, so only replays that succeed again are counted (failures build error text)
    for (m, t, h, b) in &ok { be.handle_quiet(m, t, h, b); }
    let mut allocs = 0;
    for (m, t, h, b) in &ok {
        let a0 = be.allocs();
        let code = be.handle_quiet(m, t, h, b);
        let n = be.allocs() - a0;
        if (200..300).contains(&code) { allocs += n; }
        if n > 0 && std::env::var("MITO_SHOW_ALLOCS").is_ok() { eprintln!("{}: {} {} {} -> {}: {} allocations", case.name, m, t, b, code, n); }
    }
    (log, allocs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The C runtime's float text and float parsing against Rust's, on random bit patterns and on numbers written with
    /// more digits than needed (where rounding is hardest).
    #[test]
    fn c_floats_match_rust_text_and_parsing() {
        let prog = Program::parse("struct C { n: i64; } global c: C;").unwrap();
        let api = crate::lang::parse("").unwrap();
        let mut src = crate::cgen::program_c(&prog, &api, None).unwrap();
        src += "int t_fmt(double f, char *b, u32 cap) { Area a = { (u8 *)b, cap, 0 }; a_float(&a, f); return (int)a.len; }
";
        src += "double t_parse(const char *s, u32 n) { return parse_json_double((const u8 *)s, n); }
";
        let unit = crate::tcc::Unit::compile(&src, &[]).unwrap();
        let fmt: unsafe extern "C" fn(f64, *mut u8, u32) -> i32 = unsafe { std::mem::transmute(unit.symbol("t_fmt").unwrap()) };
        let parse: unsafe extern "C" fn(*const u8, u32) -> f64 = unsafe { std::mem::transmute(unit.symbol("t_parse").unwrap()) };
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut buf = [0u8; 64];
        let mut checked = 0;
        for i in 0..300_000u32 {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let f = match i % 4 { 0 => f64::from_bits(x), 1 => (x >> 11) as f64 / (1u64 << 53) as f64 * 10f64.powi((x % 40) as i32 - 20), 2 => f64::from_bits(x & 0x000F_FFFF_FFFF_FFFF), _ => (x % 1_000_000) as f64 / 1000.0 };
            if !f.is_finite() { continue; }
            let mut want = String::new();
            crate::serve::float_text(&mut want, f).unwrap();
            let n = unsafe { fmt(f, buf.as_mut_ptr(), 64) } as usize;
            assert_eq!(std::str::from_utf8(&buf[..n]).unwrap(), want, "text of {:e} ({:016x})", f, f.to_bits());
            // parsing: the shortest text, and the exact value with 25 significant digits
            for t in [format!("{:e}", f), format!("{:.24e}", f)] {
                let r: f64 = t.parse().unwrap();
                let c = unsafe { parse(t.as_ptr(), t.len() as u32) };
                assert_eq!(c.to_bits(), r.to_bits(), "parse {}", t);
            }
            checked += 1;
        }
        assert!(checked > 250_000);
    }

    /// The chat example's requests used by `c_runtime_speed`, written as ordinary Rust (a Vec of sessions, a VecDeque of
    /// messages per room, Strings for text): the baseline a program written directly in Rust would be.
    struct NativeChat { sessions: Vec<Option<(String, u64, u32, i64, i64)>>, gens: Vec<u32>, rooms: Vec<(String, std::collections::VecDeque<(i64, i64, (usize, u32), String)>)>, next_seq: i64, now: i64, out: String, rng: u64 }
    impl NativeChat {
        fn new() -> NativeChat { NativeChat { sessions: Vec::new(), gens: Vec::new(), rooms: Vec::new(), next_seq: 0, now: 1000, out: String::with_capacity(1 << 16), rng: 0x1234_5678_9abc_def0 } }
        fn err(&mut self, code: u16, m: &str) -> u16 { self.out.clear(); self.out.push_str("{\"error\":"); esc(&mut self.out, m); self.out.push('}'); code }
        fn auth(&self, h: &[(String, String)]) -> Result<usize, (u16, &'static str)> {
            let t = h.iter().find(|x| x.0.eq_ignore_ascii_case("x-session")).map(|x| x.1.as_str()).ok_or((401, "missing X-Session header"))?;
            let mut it = t.split('.');
            let (Some(Ok(slot)), Some(Ok(g)), Some(Ok(sec))) = (it.next().map(str::parse::<usize>), it.next().map(str::parse::<u32>), it.next().map(|x| u64::from_str_radix(x, 16))) else { return Err((401, "malformed session token")) };
            match self.sessions.get(slot) { Some(Some(s)) if s.2 == g => if s.1 == sec { Ok(slot) } else { Err((403, "bad session secret")) }, _ => Err((401, "session ended")) }
        }
        /// The string value of a field of a flat JSON body.
        fn field<'b>(body: &'b str, k: &str) -> Option<String> {
            let at = body.find(&format!("\"{}\":\"", k))? + k.len() + 4;
            let mut v = String::new(); let mut it = body[at..].chars();
            while let Some(c) = it.next() { match c { '"' => return Some(v), '\\' => match it.next()? { 'n' => v.push('\n'), 't' => v.push('\t'), c => v.push(c) }, c => v.push(c) } }
            None
        }
    }
    fn esc(o: &mut String, s: &str) {
        o.push('"');
        for c in s.chars() { match c { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), '\n' => o.push_str("\\n"), '\r' => o.push_str("\\r"), '\t' => o.push_str("\\t"), c if (c as u32) < 0x20 => { use std::fmt::Write; let _ = write!(o, "\\u{:04x}", c as u32); } c => o.push(c) } }
        o.push('"');
    }
    impl Backend for NativeChat {
        fn handle(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> (u16, String) { let c = self.handle_quiet(m, t, h, b); (c, self.out.clone()) }
        fn handle_quiet(&mut self, m: &str, t: &str, h: &[(String, String)], b: &str) -> u16 {
            use std::fmt::Write;
            let (path, q) = t.split_once('?').unwrap_or((t, ""));
            let segs: Vec<&str> = path.split('/').filter(|x| !x.is_empty()).collect();
            match (m, segs.as_slice()) {
                ("POST", ["sessions"]) => {
                    let Some(name) = Self::field(b, "name") else { return self.err(400, "name is required") };
                    let mut x = self.rng; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.rng = x;
                    let slot = self.sessions.iter().position(|s| s.is_none()).unwrap_or_else(|| { self.sessions.push(None); self.gens.push(0); self.sessions.len() - 1 });
                    let g = self.gens[slot];
                    self.sessions[slot] = Some((name, x, g, self.now, 0));
                    self.out.clear(); let _ = write!(self.out, "{{\"token\":\"{}.{}.{:x}\",\"slot\":{}}}", slot, g, x, slot); 201
                }
                ("POST", ["rooms"]) => {
                    if let Err((c, e)) = self.auth(h) { return self.err(c, e); }
                    let Some(name) = Self::field(b, "name") else { return self.err(400, "name is required") };
                    self.rooms.push((name.clone(), std::collections::VecDeque::with_capacity(1000)));
                    self.out.clear(); let _ = write!(self.out, "{{\"id\":{},\"name\":", self.rooms.len() - 1); esc(&mut self.out, &name); self.out.push('}'); 201
                }
                ("POST", ["rooms", r, "messages"]) => {
                    let who = match self.auth(h) { Ok(w) => w, Err((c, e)) => return self.err(c, e) };
                    let Ok(r) = r.parse::<usize>() else { return self.err(400, "bad room") };
                    let Some(text) = Self::field(b, "body") else { return self.err(400, "body is required") };
                    let now = self.now;
                    let s = self.sessions[who].as_mut().unwrap();
                    if now - s.3 > 60000 { s.3 = now; s.4 = 0; }
                    if s.4 >= 30 { return self.err(429, "slow down"); }
                    s.4 += 1;
                    self.next_seq += 1;
                    let g = s.2;
                    let room = &mut self.rooms[r].1;
                    if room.len() == 1000 { room.pop_front(); }
                    room.push_back((self.next_seq, now, (who, g), text));
                    self.out.clear(); let _ = write!(self.out, "{{\"seq\":{}}}", self.next_seq); 201
                }
                ("GET", ["rooms", r, "messages"]) => {
                    if let Err((c, e)) = self.auth(h) { return self.err(c, e); }
                    let Ok(r) = r.parse::<usize>() else { return self.err(400, "bad room") };
                    let n = q.split('&').find_map(|kv| kv.strip_prefix("limit=")).map_or(Ok(50), |v| v.parse::<i64>()).unwrap_or(50).min(100).max(0) as usize;
                    let NativeChat { rooms, sessions, out, .. } = self;
                    out.clear(); out.push_str("{\"room\":"); esc(out, &rooms[r].0); out.push_str(",\"messages\":[");
                    for (k, (seq, at, (slot, g), body)) in rooms[r].1.iter().rev().take(n).enumerate() {
                        if k > 0 { out.push(','); }
                        let _ = write!(out, "{{\"seq\":{},\"author\":", seq);
                        match sessions.get(*slot) { Some(Some(s)) if s.2 == *g => esc(out, &s.0), _ => esc(out, "[deleted]") }
                        let _ = write!(out, ",\"at\":{},\"body\":", at);
                        esc(out, body);
                        out.push('}');
                    }
                    out.push_str("]}"); 200
                }
                _ => self.err(404, "no such endpoint"),
            }
        }
        fn set_clock(&mut self, ms: i64) { self.now = ms; }
        fn set_marks(&mut self, _: usize, _: usize) {}
        fn state(&self) -> Vec<String> { Vec::new() }
        fn stats(&self) -> String { String::new() }
        fn notes(&self) -> Vec<String> { Vec::new() }
        fn no_alloc(&self) -> bool { false }
        fn allocs(&self) -> usize { crate::alloc_count::allocs() }
    }

    /// `cargo test --release c_runtime_speed -- --ignored --nocapture`: the chat example's GET messages (50 of 1000
    /// messages) and POST message, Rust runtime with compiled endpoints against the C runtime, alternating rounds.
    #[test]
    #[ignore]
    fn c_runtime_speed() {
        let (state, api) = crate::modules::expand(include_str!("../examples/chat/chat.mito"), include_str!("../examples/chat/chat.api")).unwrap();
        let case = Case { name: "speed".into(), state, api, requests: String::new(), system: None, files: Vec::new() };
        let mut bes: Vec<(&str, Box<dyn Backend>)> = vec![("Rust runtime", Box::new(RustBackend::start(&case, true))), ("C runtime, TinyCC", Box::new(CBackend::start(&case)))];
        if clang().is_some() {
            bes.push(("C runtime, clang -O2", Box::new(CBackend::start_with(&case, true))));
            bes.push(("C runtime, clang -O2, journal off (measurement only: no rollback)", Box::new(CBackend::start_flags(&case, true, &["-DMITO_MEASURE_NO_JOURNAL"]))));
        }
        bes.push(("native Rust (Vec, VecDeque, String)", Box::new(NativeChat::new())));
        let mut first: Option<String> = None;
        for (name, be) in bes.iter_mut() {
            println!("{}:", name);
            let (_, tok) = be.handle("POST", "/sessions", &[], "{\"name\":\"ann\"}");
            let tok = tok.split("\"token\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
            let h = vec![("X-Session".to_string(), tok)];
            be.handle("POST", "/rooms", &h, "{\"name\":\"lobby\"}");
            be.set_clock(1000);
            for i in 0..1000 { be.set_clock(1000 + i * 61_000); be.handle_quiet("POST", "/rooms/0/messages", &h, "{\"body\":\"hello there, a message of ordinary length\"}"); }
            let (_, body) = be.handle("GET", "/rooms/0/messages?limit=50", &h, "");
            match &first { None => first = Some(body), Some(f) => assert!(*f == body, "{} answers GET messages differently", name) }
            for round in 0..3 {
                let t = std::time::Instant::now();
                for _ in 0..5000 { assert_eq!(be.handle_quiet("GET", "/rooms/0/messages?limit=50", &h, ""), 200); }
                let get = t.elapsed().as_nanos() as f64 / 5000.0 / 1000.0;
                let t = std::time::Instant::now();
                for i in 0..5000 { be.set_clock(100_000_000 + i * 61_000); be.handle_quiet("POST", "/rooms/0/messages", &h, "{\"body\":\"x\"}"); }
                let post = t.elapsed().as_nanos() as f64 / 5000.0 / 1000.0;
                println!("round {}: GET messages {:.2} us, POST message {:.2} us", round, get, post);
            }
        }
    }

    /// lib/sorted.mito against a model (a list kept in key order, equal keys in the order they were placed): thousands
    /// of random adds, key changes and removes, the whole order and walks from random keys checked after every step.
    #[test]
    fn sorted_matches_a_model_under_random_changes() {
        let (state, api) = crate::modules::expand("struct E { v: i64; id: i64; }\nglobal s: sorted<E, 48, v>;\n",
            "endpoint POST /add/{v}/{id} { let e = s.add(int(param(\"v\"))); e.id = int(param(\"id\")); respond 201 { \"slot\": slot(e) }; }\n\
             endpoint POST /set/{k}/{v} { s.set_key(s.get(int(param(\"k\"))), int(param(\"v\"))); respond 200 {}; }\n\
             endpoint DELETE /x/{k} { s.remove(s.get(int(param(\"k\")))); respond 200 {}; }\n\
             endpoint GET /all { respond 200 { \"x\": [each e in s e.id] }; }\n\
             endpoint GET /from/{k} { let k = int(param(\"k\")); respond 200 { \"up\": [each e in s from k e.id], \"down\": [each e in s from k reverse e.id] }; }\n").unwrap();
        let case = Case { name: "sorted-model".into(), state, api, requests: String::new(), system: None, files: Vec::new() };
        let mut bes: Vec<(&str, Box<dyn Backend>)> = vec![("compiled", Box::new(RustBackend::start(&case, true)))];
        if clang().is_some() {
            bes.push(("C runtime, clang", Box::new(CBackend::start_with(&case, true))));
            bes.push(("C runtime, clang, undefined-behaviour checks", Box::new(CBackend::start_flags(&case, true, UBSAN))));
        }
        for (name, be) in bes.iter_mut() {
            let mut model: Vec<(i64, i64, i64)> = Vec::new(); // (slot, key, id), in order
            let mut x = 0x2545_F491_4F6C_DD1Du64;
            let mut rnd = |n: u64| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x % n) as i64 };
            let ids = |m: &[(i64, i64, i64)]| format!("[{}]", m.iter().map(|e| e.2.to_string()).collect::<Vec<_>>().join(","));
            for step in 0..4000 {
                let op = rnd(10);
                if model.len() < 48 && (op < 4 || model.is_empty()) {
                    let (v, id) = (rnd(12), step as i64);
                    let (code, b) = be.handle("POST", &format!("/add/{}/{}", v, id), &[], "");
                    assert_eq!(code, 201, "{} step {}: {}", name, step, b);
                    let slot: i64 = b.split("\"slot\":").nth(1).unwrap().trim_end_matches('}').parse().unwrap();
                    let at = model.iter().position(|e| e.1 > v).unwrap_or(model.len());
                    model.insert(at, (slot, v, id));
                } else if op < 7 {
                    let i = rnd(model.len() as u64) as usize; let v = rnd(12);
                    let mut e = model.remove(i);
                    assert_eq!(be.handle("POST", &format!("/set/{}/{}", e.0, v), &[], "").0, 200);
                    e.1 = v;
                    let at = model.iter().position(|f| f.1 > v).unwrap_or(model.len());
                    model.insert(at, e);
                } else {
                    let i = rnd(model.len() as u64) as usize;
                    let e = model.remove(i);
                    assert_eq!(be.handle("DELETE", &format!("/x/{}", e.0), &[], "").0, 200);
                }
                let (_, all) = be.handle("GET", "/all", &[], "");
                assert_eq!(all, format!("{{\"x\":{}}}", ids(&model)), "{} step {}", name, step);
                if step % 7 == 0 {
                    let k = rnd(14) - 1;
                    let up: Vec<_> = model.iter().filter(|e| e.1 >= k).cloned().collect();
                    let down: Vec<_> = model.iter().rev().filter(|e| e.1 <= k).cloned().collect();
                    let (_, b) = be.handle("GET", &format!("/from/{}", k), &[], "");
                    assert_eq!(b, format!("{{\"up\":{},\"down\":{}}}", ids(&up), ids(&down)), "{} step {} from {}", name, step, k);
                }
            }
        }
    }

    /// `cargo test --release compile_times -- --ignored --nocapture`: how long each way of running a program takes to
    /// get ready, stage by stage (median of 5), for the chat example and a program using a module.
    #[test]
    #[ignore]
    fn compile_times() {
        let sorted = load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/sorted.case"));
        let (cs, ca) = (include_str!("../examples/chat/chat.mito"), include_str!("../examples/chat/chat.api"));
        let progs: [(&str, &str, &str, bool); 2] = [("chat example", cs, ca, true), ("sorted.case (a module)", &sorted.state, &sorted.api, false)];
        let med = |mut v: Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] };
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        for (name, state, api, raw) in progs {
            let (mut front, mut gen, mut whole, mut check, mut tcc_ep, mut tcc_all, mut clang_o0, mut clang_o2) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
            let (mut ep_lines, mut all_lines) = (0, 0);
            for _ in 0..5 {
                let t = std::time::Instant::now();
                let (st, ap) = if raw { crate::modules::expand(state, api).unwrap() } else { (state.to_string(), api.to_string()) };
                let prog = Program::parse(&st).unwrap();
                let mut a = crate::lang::parse(&ap).unwrap();
                crate::lang::bind(&mut a, &|n| prog.global(n)).unwrap();
                front.push(ms(t));
                let t = std::time::Instant::now();
                let g = crate::cgen::generate(&prog, &a);
                gen.push(ms(t));
                ep_lines = g.src.lines().count();
                let t = std::time::Instant::now();
                let src = crate::cgen::program_c(&prog, &a, None).unwrap();
                whole.push(ms(t));
                all_lines = src.lines().count();
                let t = std::time::Instant::now();
                crate::tcc::Unit::check(&src).unwrap();
                check.push(ms(t));
                let t = std::time::Instant::now();
                let _u = crate::tcc::Unit::compile(&g.src, &[]).unwrap();
                tcc_ep.push(ms(t));
                let t = std::time::Instant::now();
                let _u = crate::tcc::Unit::compile(&src, &[]).unwrap();
                tcc_all.push(ms(t));
                if clang().is_some() {
                    let t = std::time::Instant::now();
                    let _ = clang_dll(&src, "ctime", &["-O0"]).unwrap();
                    clang_o0.push(ms(t));
                    let t = std::time::Instant::now();
                    let _ = clang_dll(&src, "ctime", &[]).unwrap();
                    clang_o2.push(ms(t));
                }
            }
            println!("{} ({} lines of endpoint C, {} lines as a whole program):", name, ep_lines, all_lines);
            println!("  parse + bind (+ module expansion)            {:8.2} ms", med(front));
            println!("  generate endpoint C                          {:8.2} ms", med(gen));
            println!("  whole program C (runtime + layout tables)    {:8.2} ms", med(whole));
            println!("  tcc.exe check (child process)                {:8.2} ms", med(check.clone()));
            println!("  TinyCC, endpoints only (Rust runtime)        {:8.2} ms  (check included)", med(tcc_ep));
            println!("  TinyCC, whole program (C runtime)            {:8.2} ms  (check included)", med(tcc_all));
            if !clang_o0.is_empty() {
                println!("  clang -O0, whole program, to a DLL           {:8.2} ms", med(clang_o0));
                println!("  clang -O2, whole program, to a DLL           {:8.2} ms", med(clang_o2));
            }
        }
    }

    #[test]
    fn cases_match_their_records_on_every_backend() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
        let bless = std::env::var("MITO_BLESS").is_ok();
        let only = std::env::var("MITO_CASE").ok();
        let mut problems = Vec::new();
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().map_or(false, |x| x == "case")).collect();
        files.sort();
        for f in files {
            let case = load(&f);
            if only.as_ref().map_or(false, |o| *o != case.name) { continue; }
            // the interpreter is the reference while it exists
            let mut backends: Vec<(&str, Box<dyn Backend>)> = if case.system.is_some() {
                let mut v: Vec<(&str, Box<dyn Backend>)> = vec![("interpreted", Box::new(SystemBackend::start(&case, false))), ("compiled", Box::new(SystemBackend::start(&case, true))), ("C system", Box::new(CSystemBackend::start(&case, false, &[])))];
                if clang().is_some() {
                    v.push(("C system, clang", Box::new(CSystemBackend::start(&case, true, &[]))));
                    if std::env::var("MITO_SKIP_UBSAN").is_err() { v.push(("C system, clang, undefined-behaviour checks", Box::new(CSystemBackend::start(&case, true, UBSAN)))); }
                }
                v
            } else {
                vec![("interpreted", Box::new(RustBackend::start(&case, false))), ("compiled", Box::new(RustBackend::start(&case, true))), ("C runtime", Box::new(CBackend::start(&case)))]
            };
            if case.system.is_none() && clang().is_some() {
                backends.push(("C runtime, clang", Box::new(CBackend::start_with(&case, true))));
                backends.push(("C runtime, clang, undefined-behaviour checks", Box::new(CBackend::start_flags(&case, true, UBSAN))));
            }
            let mut logs = Vec::new();
            for (name, be) in backends.iter_mut() {
                for n in be.notes().iter().filter(|n| !n.ends_with(": compiled")) { problems.push(format!("{} ({}): {}", case.name, name, n)); }
                let (log, allocs) = run(&case, be.as_mut());
                if be.no_alloc() && allocs != 0 { problems.push(format!("{} ({}): {} heap allocations in warm requests", case.name, name, allocs)); }
                if std::env::var("MITO_SHOW_LOG").map_or(false, |v| v == *name) { for l in &log { eprintln!("{}", &l[..l.len().min(200)]); } }
                logs.push((*name, log));
            }
            let (_, reference) = &logs[0];
            for (name, log) in &logs[1..] {
                if let Some((x, y)) = reference.iter().zip(log).find(|(x, y)| x != y) { problems.push(format!("{}: {} vs {}\n    {}\n    {}", case.name, logs[0].0, name, x, y)); }
                else if reference.len() != log.len() { problems.push(format!("{}: {} has {} lines, {} has {}", case.name, logs[0].0, reference.len(), name, log.len())); }
            }
            let out = f.with_extension("out");
            let text = reference.join("\n") + "\n";
            if bless { std::fs::write(&out, &text).unwrap(); continue; }
            match std::fs::read_to_string(&out) {
                Ok(want) => {
                    if let Some((x, y)) = want.lines().zip(text.lines()).find(|(x, y)| x != y) { problems.push(format!("{}: differs from the record\n    want {}\n    got  {}", case.name, x, y)); }
                    else if want.lines().count() != text.lines().count() { problems.push(format!("{}: {} lines recorded, {} now", case.name, want.lines().count(), text.lines().count())); }
                }
                Err(_) => problems.push(format!("{}: no record; run with MITO_BLESS=1 after checking the log:\n{}", case.name, text)),
            }
        }
        assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    }
}
