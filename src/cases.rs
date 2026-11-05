//! Cases (`tests/cases/*.case`): a state, an api and a request script each. Every case runs on every backend (the C
//! runtime built by TinyCC, by clang, and by clang with undefined-behaviour checks); the logs must match each other and
//! the recorded `.out` file. `MITO_BLESS=1 cargo test cases` records the TinyCC build's log.
//!
//! A case is one program (`--- program`, a `.mito` file) or a system of workers (`--- system` holding a system file,
//! whose programs are `--- file name.mito` sections). In any section, `include path` takes a file (relative to the
//! repository), and `include path code` only its functions and endpoints (so a case can pair a small state of its own
//! with an example's code).
//! Request script lines:
//! `METHOD target [json body]`, optionally ending in `=> name` to remember the response's `"token"` as `$name`;
//! `*N` before a request repeats it N times; `@Header: value` adds a header to the next request; `clock ms` sets the
//! request clock; `marks soft hard` sets the memory pressure marks (bytes); `stats` logs the pressure counters (for a
//! system: what each worker handled, forwarded and how often it was restarted).
//! `$name` is replaced anywhere in a line.
use crate::layout::Program;

pub struct Case { pub name: String, pub state: String, pub api: String, pub requests: String, pub system: Option<String>, pub files: Vec<(String, String)> }

pub fn load(path: &std::path::Path) -> Case {
    let text = std::fs::read_to_string(path).unwrap();
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.trim_end().strip_prefix("--- ") { sections.push((name.to_string(), String::new())); continue; }
        let Some(cur) = sections.last_mut() else { continue };
        match line.strip_prefix("include ") {
            Some(f) if cur.0 != "requests" => {
                let (f, part) = match f.trim().split_once(' ') { Some((f, p)) => (f, Some(p.trim())), None => (f.trim(), None) };
                let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(f)).unwrap_or_else(|e| panic!("{}: {}", f, e));
                cur.1 += &match part {
                    None => text,
                    Some("code") => crate::source::split(&text).unwrap_or_else(|e| panic!("{}: {}", f, e)).1,
                    Some(p) => panic!("include {} {}: expected `code` or nothing", f, p),
                };
            }
            _ => cur.1 += line,
        }
        cur.1.push('\n');
    }
    let get = |n: &str| sections.iter().find(|s| s.0 == n).map(|s| s.1.clone());
    let files = sections.iter().filter_map(|s| s.0.strip_prefix("file ").map(|f| (f.trim().to_string(), s.1.clone()))).collect();
    // modules are expanded once here, so every backend compiles the same program
    let (state, api) = match get("program") { Some(p) => crate::source::load(&p).unwrap_or_else(|e| panic!("{}: {}", path.display(), e)), None => (String::new(), String::new()) };
    Case { name: path.file_stem().unwrap().to_string_lossy().into_owned(), state, api,
        requests: get("requests").unwrap_or_default(), system: get("system"), files }
}

/// A running program, as the cases drive it. Every way of running a program implements this, so the same scripts and
/// records check each one.
pub trait Backend {
    fn handle(&mut self, method: &str, target: &str, headers: &[(String, String)], body: &str) -> (u16, String);
    /// Status only; the response stays in the backend (no copy, for timing).
    fn handle_quiet(&mut self, method: &str, target: &str, headers: &[(String, String)], body: &str) -> u16;
    fn set_clock(&mut self, ms: i64);
    fn set_marks(&mut self, soft: usize, hard: usize);
    /// `state <path> <live bytes>` for the plain values and every collection, then the memory totals.
    fn state(&self) -> Vec<String>;
    /// Pressure counters, as one log line.
    fn stats(&self) -> String;
}

/// The C runtime (runtime/mito.c) and the program's generated C compiled together, driven through `mito_*`: in memory
/// by TinyCC, or by clang (-O2) into a DLL.
pub struct CBackend { _code: Code, m: *mut std::ffi::c_void, f: CFns, pairs: Vec<MitoPair> }
/// A header or forwarded field, as runtime/types.h declares it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MitoPair { pub k: *const u8, pub kn: u32, pub v: *const u8, pub vn: u32 }
enum Code { Tcc(crate::tcc::Unit), Dll(*mut std::ffi::c_void) }
use crate::tcc::{dl_open, dl_sym};
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
    let dll = dir.join(format!("{}.{}", stem, std::env::consts::DLL_EXTENSION));
    let mut cs = Vec::new();
    for (k, src) in srcs.iter().enumerate() { let c = dir.join(format!("{}-{}.c", stem, k)); std::fs::write(&c, src).map_err(|e| e.to_string())?; cs.push(c); }
    let out = std::process::Command::new(cc).args(["-O2", "-shared", "-fno-strict-aliasing", "-w"]).args(if cfg!(windows) { &[][..] } else { &["-fPIC"][..] }).args(extra).arg("-o").arg(&dll).args(&cs).args(if cfg!(windows) { &["-lkernel32"][..] } else { &[][..] }).args(crate::tcc::LIBS.iter().map(|l| format!("-l{}", l)))
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(format!("clang failed:\n{}", String::from_utf8_lossy(&out.stderr))); }
    let h = dl_open(&dll.to_string_lossy());
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
        crate::lang::bind_with(&mut api, &|n| prog.global(n), &prog.enum_table()).unwrap_or_else(|e| panic!("{}: api: {}", case.name, e));
        let src = crate::cgen::program_c(&prog, &api, None).unwrap_or_else(|e| panic!("{}: {}", case.name, e));
        // the C runtime makes no heap allocations because it has no allocator: check the program names none
        for w in ["malloc", "calloc", "realloc", "HeapAlloc", "LocalAlloc", "GlobalAlloc", "_alloca"] { assert!(!src.contains(w), "{}: the C program uses {}", case.name, w); }
        let use_clang = use_clang || !crate::tcc::available();
        let code = if use_clang { Code::Dll(clang_dll(&src, &case.name, flags).unwrap_or_else(|e| panic!("{}: {}", case.name, e))) }
            else { Code::Tcc(crate::tcc::Unit::compile(&src, &[]).unwrap_or_else(|e| { let _ = std::fs::write(std::env::temp_dir().join(format!("mito-{}.c", case.name)), &src); panic!("{}: C runtime: {}", case.name, e) })) };
        let sym = |n: &str| -> *const std::ffi::c_void {
            let p = match &code { Code::Tcc(u) => u.symbol(n), Code::Dll(h) => { let p = dl_sym(*h, n); if p.is_null() { None } else { Some(p as *const std::ffi::c_void) } } };
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
        let spec = crate::spec::parse_spec_with(case.system.as_deref().unwrap(), &read).unwrap_or_else(|e| panic!("{}: system: {}", case.name, e));
        let units = crate::cgen::system_c(&spec).unwrap_or_else(|e| panic!("{}: {}", case.name, e));
        let refs: Vec<&str> = units.iter().map(|s| s.as_str()).collect();
        let code = if use_clang { Code::Dll(clang_dll_units(&refs, &case.name, flags).unwrap_or_else(|e| panic!("{}: {}", case.name, e))) }
            else { Code::Tcc(crate::tcc::Unit::compile_units(&refs, &[]).unwrap_or_else(|e| panic!("{}: C system: {}", case.name, e))) };
        let sym = |n: &str| -> *const std::ffi::c_void {
            let p = match &code { Code::Tcc(u) => u.symbol(n), Code::Dll(h) => { let p = dl_sym(*h, n); if p.is_null() { None } else { Some(p as *const std::ffi::c_void) } } };
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
}

/// Run a case's script; returns the log.
pub fn run(case: &Case, be: &mut dyn Backend) -> Vec<String> {
    let mut log = Vec::new();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut vars: Vec<(String, String)> = Vec::new();
    for line in case.requests.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("//")) {
        let mut line = line.to_string();
        for (k, v) in vars.iter().rev() { line = line.replace(&format!("${}", k), v); } // later names first, so $a does not eat $ab
        if let Some(h) = line.strip_prefix('@') { let (k, v) = h.split_once(':').unwrap(); headers.push((k.trim().into(), v.trim().into())); continue; }
        if let Some(t) = line.strip_prefix("clock ") { be.set_clock(t.trim().parse().unwrap()); continue; }
        if let Some(t) = line.strip_prefix("marks ") { let (a, b) = t.trim().split_once(' ').unwrap(); be.set_marks(a.parse().unwrap(), b.parse().unwrap()); continue; }
        if line == "stats" { log.push(be.stats()); continue; }
        let (times, line) = match line.strip_prefix('*') { Some(r) => { let (n, rest) = r.split_once(' ').unwrap(); (n.parse::<usize>().unwrap(), rest.to_string()) } None => (1, line) };
        let (line, capture) = match line.rsplit_once(" => ") { Some((l, n)) => (l.to_string(), Some(n.trim().to_string())), None => (line, None) };
        let (m, rest) = line.split_once(' ').unwrap();
        let (target, body) = rest.split_once(' ').unwrap_or((rest, ""));
        for _ in 0..times {
            let (code, out) = be.handle(m, target, &headers, body);
            log.push(format!("{} {} -> {} {}", m, target, code, out));
            if let Some(n) = &capture {
                let tok = out.split("\"token\":\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or_else(|| panic!("{}: no token in {}", case.name, out));
                vars.push((n.clone(), tok.to_string()));
            }
        }
        headers.clear();
    }
    log.extend(be.state());
    log
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A float as text, as JavaScript writes numbers, built on Rust's `{:e}` (the shortest digits that read back as the
    /// same number): positional from 1e-6 up to (not including) 1e21, otherwise with an exponent (`1e+21`, `1.5e-7`).
    /// The oracle the C runtime's float text is checked against.
    fn float_text(f: f64) -> String {
        if f == 0.0 { return "0".into(); }
        let t = format!("{:e}", f.abs());
        let (mant, exp) = t.split_once('e').unwrap();
        let n = exp.parse::<i32>().unwrap() + 1; // the value is 0.digits * 10^n
        let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
        let k = digits.len() as i32;
        let body = if k <= n && n <= 21 { format!("{}{}", digits, "0".repeat((n - k) as usize)) }
            else if 0 < n && n <= 21 { format!("{}.{}", &digits[..n as usize], &digits[n as usize..]) }
            else if -6 < n && n <= 0 { format!("0.{}{}", "0".repeat(-n as usize), digits) }
            else { format!("{}{}e{}{}", &digits[..1], if k > 1 { format!(".{}", &digits[1..]) } else { String::new() }, if n - 1 >= 0 { "+" } else { "-" }, (n - 1).abs()) };
        if f < 0.0 { format!("-{}", body) } else { body }
    }

    #[test]
    fn the_float_oracle_writes_as_javascript_does() {
        for (f, t) in [(0.0, "0"), (-0.0, "0"), (1.0, "1"), (-1.5, "-1.5"), (0.1, "0.1"), (1e21, "1e+21"), (1e20, "100000000000000000000"), (1.5e-7, "1.5e-7"), (0.000001, "0.000001"), (123.456, "123.456"), (5e-324, "5e-324")] {
            assert_eq!(float_text(f), t);
        }
    }

    /// The C runtime's float text and float parsing against Rust's, on random bit patterns and on numbers written with
    /// more digits than needed (where rounding is hardest).
    #[test]
    fn c_floats_match_rust_text_and_parsing() {
        let prog = Program::parse("struct C { n: i64; } state c: C;").unwrap();
        let api = crate::lang::parse("").unwrap();
        let mut src = crate::cgen::program_c(&prog, &api, None).unwrap();
        src += "int t_fmt(double f, char *b, u32 cap) { Area a = { (u8 *)b, cap, 0 }; a_float(&a, f); return (int)a.len; }
";
        src += "double t_parse(const char *s, u32 n) { return parse_json_double((const u8 *)s, n); }
";
        src += "int t_esc(const u8 *s, u32 n, char *b, u32 cap) { Area a = { (u8 *)b, cap, 0 }; if (!a_json_str(&a, s, n)) return -1; return (int)a.len; }
";
        let unit = crate::tcc::Unit::compile(&src, &[]).unwrap();
        let fmt: unsafe extern "C" fn(f64, *mut u8, u32) -> i32 = unsafe { std::mem::transmute(unit.symbol("t_fmt").unwrap()) };
        let parse: unsafe extern "C" fn(*const u8, u32) -> f64 = unsafe { std::mem::transmute(unit.symbol("t_parse").unwrap()) };
        let esc: unsafe extern "C" fn(*const u8, u32, *mut u8, u32) -> i32 = unsafe { std::mem::transmute(unit.symbol("t_esc").unwrap()) };
        let text = |f: f64, buf: &mut [u8; 64]| { let n = unsafe { fmt(f, buf.as_mut_ptr(), 64) } as usize; String::from_utf8(buf[..n].to_vec()).unwrap() };
        // expected texts are JavaScript's String(x)
        for (x, want) in [(0.0, "0"), (-0.0, "0"), (1.0, "1"), (-1.5, "-1.5"), (0.1 + 0.2, "0.30000000000000004"), (123456789.0, "123456789"),
            (1e20, "100000000000000000000"), (1e21, "1e+21"), (1.5e21, "1.5e+21"), (0.000001, "0.000001"), (0.0000001, "1e-7"), (1.25e-7, "1.25e-7"),
            (5e-324, "5e-324"), (1.7976931348623157e308, "1.7976931348623157e+308"), (2.0f64.powi(53), "9007199254740992"), (0.1f32 as f64, "0.10000000149011612"),
            (100.0, "100"), (1.0e-6 * 1.5, "0.0000015"), (12.5, "12.5")] {
            assert_eq!(text(x, &mut [0; 64]), want, "{:e}", x);
        }
        // JSON string escaping against a byte-by-byte reference
        fn reference(s: &[u8]) -> Vec<u8> {
            let mut o = vec![b'"'];
            for &c in s { match c { b'"' => o.extend_from_slice(b"\\\""), b'\\' => o.extend_from_slice(b"\\\\"), b'\n' => o.extend_from_slice(b"\\n"), b'\r' => o.extend_from_slice(b"\\r"), b'\t' => o.extend_from_slice(b"\\t"), c if c < 0x20 => o.extend_from_slice(format!("\\u{:04x}", c).as_bytes()), c => o.push(c) } }
            o.push(b'"'); o
        }
        let alphabet: &[u8] = b"ab\"\\\n\t\x01\x1f \x7f\xc3\xa9z0";
        let mut r = 12345u64;
        let mut eb = vec![0u8; 1 << 12];
        for n in 0..2000 {
            let s: Vec<u8> = (0..n % 70).map(|_| { r ^= r << 13; r ^= r >> 7; r ^= r << 17; if r % 3 == 0 { alphabet[(r >> 8) as usize % alphabet.len()] } else { b'a' + (r >> 16) as u8 % 26 } }).collect();
            let k = unsafe { esc(s.as_ptr(), s.len() as u32, eb.as_mut_ptr(), eb.len() as u32) };
            assert!(k >= 0);
            assert_eq!(&eb[..k as usize], reference(&s).as_slice(), "input {:?}", s);
        }
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut buf = [0u8; 64];
        let mut checked = 0;
        for i in 0..300_000u32 {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let f = match i % 4 { 0 => f64::from_bits(x), 1 => (x >> 11) as f64 / (1u64 << 53) as f64 * 10f64.powi((x % 40) as i32 - 20), 2 => f64::from_bits(x & 0x000F_FFFF_FFFF_FFFF), _ => (x % 1_000_000) as f64 / 1000.0 };
            if !f.is_finite() { continue; }
            let want = float_text(f);
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
    }

    /// `cargo test --release c_runtime_speed -- --ignored --nocapture`: the chat example's GET messages (50 of 1000
    /// messages) and POST message on the C runtime's builds, and the same requests written directly in Rust.
    #[test]
    #[ignore]
    fn c_runtime_speed() {
        let (state, api) = crate::source::load(include_str!("../examples/chat/chat.mito")).unwrap();
        let case = Case { name: "speed".into(), state, api, requests: String::new(), system: None, files: Vec::new() };
        let mut bes: Vec<(&str, Box<dyn Backend>)> = vec![("C runtime, TinyCC", Box::new(CBackend::start(&case)))];
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

    /// What the system cases cannot make happen on purpose, checked on the C system's own parts: a mailbox that is full,
    /// a message too big for its slot, a closed mailbox; in-flight requests bounded, and a reply slot abandoned by a
    /// caller that gave up freed by its late answer; a forward that could not be sent rolls its handler back.
    #[test]
    fn c_system_parts_behave() {
        let one = "system {\n    worker w { program \"w.mito\"; count 1; mailbox 4; slot 256; }\n    entry w;\n    inflight 2;\n}\n";
        let read = |_: &str| Ok("struct C { n: i64; } state c: C;\n@get(\"/n\") fn get_n() -> Response<{ n: i64 }> { return ok({ \"n\": c.n }); }\n".to_string());
        let spec = crate::spec::parse_spec_with(one, &read).unwrap();
        let mut units = crate::cgen::system_c(&spec).unwrap();
        *units.last_mut().unwrap() += r#"
MITO_API int t_box(void) {
    if (!sys_os()) return 100;
    Box b = {0}; lk_init(&b.lk); cv_init(&b.cv); b.cap = 2; b.slot = 64; b.buf = (u8 *)sys_mem(128);
    u8 out[64]; u32 id; char big[100] = {0};
    if (box_send(&b, 1, "a", 1, "/", 1, "", 0, 0, 0, 0, 0) != SENT) return 1;
    if (box_send(&b, 2, "b", 1, "/", 1, "", 0, 0, 0, 0, 0) != SENT) return 2;
    if (box_send(&b, 3, "c", 1, "/", 1, "", 0, 0, 0, 0, 0) != FULL) return 3;
    if (!box_recv(&b, out)) return 4; memcpy(&id, out, 4); if (id != 1) return 5; /* first in, first out */
    if (box_send(&b, 4, big, 100, "/", 1, "", 0, 0, 0, 0, 0) != TOOBIG) return 6;
    box_close(&b);
    if (!box_recv(&b, out)) return 7; memcpy(&id, out, 4); if (id != 2) return 8; /* queued messages still arrive after close */
    if (box_recv(&b, out) != 0) return 9;
    if (box_send(&b, 5, "a", 1, "/", 1, "", 0, 0, 0, 0, 0) != CLOSED) return 10;
    return b.refused == 1 ? 0 : 11;
}
MITO_API int t_reply(void *sp) {
    Sys *s = (Sys *)sp; u32 a, b, c;
    if (!reply_take(s, &a) || !reply_take(s, &b)) return 1;
    if (reply_take(s, &c)) return 2; /* inflight 2 */
    reply_put(s, a, 200, "x", 1);
    if (s->replies[a].state != 2 || s->replies[a].code != 200 || s->replies[a].len != 1) return 3;
    s->replies[a].state = 0; reply_free(s, a);
    s->replies[b].state = 3; /* its caller gave up, as mito_system_request does at its deadline */
    reply_put(s, b, 200, "late", 4); /* the late answer frees the slot instead of being delivered */
    if (s->replies[b].state != 0) return 4;
    return reply_take(s, &a) && reply_take(s, &c) ? 0 : 5;
}
"#;
        let refs: Vec<&str> = units.iter().map(|s| s.as_str()).collect();
        let u = crate::tcc::Unit::compile_units(&refs, &[]).unwrap();
        unsafe {
            let t_box: unsafe extern "C" fn() -> i32 = std::mem::transmute(u.symbol("t_box").unwrap());
            assert_eq!(t_box(), 0, "mailbox check failed at step {}", t_box());
            let start: unsafe extern "C" fn(u64, i32) -> *mut std::ffi::c_void = std::mem::transmute(u.symbol("mito_system_start").unwrap());
            let stop: unsafe extern "C" fn(*mut std::ffi::c_void) = std::mem::transmute(u.symbol("mito_system_stop").unwrap());
            let t_reply: unsafe extern "C" fn(*mut std::ffi::c_void) -> i32 = std::mem::transmute(u.symbol("t_reply").unwrap());
            let s = start(1, 1);
            assert!(!s.is_null());
            assert_eq!(t_reply(s), 0, "reply slots");
            stop(s);
        }

        // a forward: the handler's writes are kept only if the request was handed on
        let case = Case { name: "forward-rollback".into(), state: "struct C { n: i64; } state c: C;".into(),
            api: "@post(\"/f\") fn post_f() -> Response<{}> { c.n = c.n + 1; forward next[0] { \"n\": c.n }; }\n@get(\"/n\") fn get_n() -> Response<{ n: i64 }> { return ok({ \"n\": c.n }); }".into(), requests: String::new(), system: None, files: Vec::new() };
        let mut be = CBackend::start(&case);
        let sym = |n: &str| match &be._code { Code::Tcc(u) => u.symbol(n).unwrap(), Code::Dll(h) => { let p = dl_sym(*h, n); assert!(!p.is_null(), "no {}", n); p as *const std::ffi::c_void } };
        unsafe {
            let set_system: unsafe extern "C" fn(*mut std::ffi::c_void, u32, *const *const u8, *const u32, u32, u32) = std::mem::transmute(sym("mito_set_system"));
            let forward_to: unsafe extern "C" fn(*mut std::ffi::c_void, *mut u32, *mut i64, *mut MitoPair, u32) -> u32 = std::mem::transmute(sym("mito_forward_to"));
            let finish: unsafe extern "C" fn(*mut std::ffi::c_void, i32) = std::mem::transmute(sym("mito_finish_forward"));
            let names = [b"me\0".as_ptr(), b"next\0".as_ptr()];
            set_system(be.m, 2, names.as_ptr(), [1u32, 1].as_ptr(), 0, 0);
            assert_eq!(be.handle_quiet("POST", "/f", &[], ""), 0); // 0: forward, not an answer
            let (mut kind, mut key) = (0u32, 0i64);
            let mut out = [MitoPair { k: std::ptr::null(), kn: 0, v: std::ptr::null(), vn: 0 }; 4];
            assert_eq!(forward_to(be.m, &mut kind, &mut key, out.as_mut_ptr(), 4), 1);
            assert_eq!((kind, key, std::slice::from_raw_parts(out[0].v, out[0].vn as usize)), (1, 0, b"1".as_slice()));
            finish(be.m, 0); // the next mailbox was full
            assert_eq!(be.handle("GET", "/n", &[], "").1, "{\"n\":0}");
            assert_eq!(be.handle_quiet("POST", "/f", &[], ""), 0);
            finish(be.m, 1);
            assert_eq!(be.handle("GET", "/n", &[], "").1, "{\"n\":1}");
        }
    }

    /// lib/sorted.mito against a model (a list kept in key order, equal keys in the order they were placed): thousands
    /// of random adds, key changes and removes, the whole order and walks from random keys checked after every step.
    #[test]
    fn sorted_matches_a_model_under_random_changes() {
        let (state, api) = crate::modules::expand("struct E { v: i64; id: i64; }\nstruct App { s: sorted<E, 48, v>; }\nstate app: App;\n",
            "@post(\"/add/{v}/{id}\") fn post_add_v_id(v: i64, id: i64) -> Response<{ slot: i64 }> { let e = app.s.add(v); e.id = id; return created({ slot: slot(e) }); }\n\
             @post(\"/set/{k}/{v}\") fn post_set_k_v(k: i64, v: i64) -> Response<{}> { app.s.set_key(app.s.get(k), v); return ok({}); }\n\
             @delete(\"/x/{k}\") fn delete_x_k(k: i64) -> Response<{}> { app.s.remove(app.s.get(k)); return ok({}); }\n\
             @get(\"/all\") fn get_all() -> Response<{ x: [i64] }> { return ok({ \"x\": [foreach e in app.s e.id] }); }\n\
             @get(\"/from/{k}\") fn get_from_k(k: i64) -> Response<{ up: [i64], down: [i64] }> { return ok({ \"up\": [foreach e in app.s from k e.id], \"down\": [foreach e in app.s from k reverse e.id] }); }\n").unwrap();
        let case = Case { name: "sorted-model".into(), state, api, requests: String::new(), system: None, files: Vec::new() };
        let mut bes: Vec<(&str, Box<dyn Backend>)> = vec![("C runtime, TinyCC", Box::new(CBackend::start(&case)))];
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
        let progs: [(&str, &str, &str, bool); 2] = [("chat example", include_str!("../examples/chat/chat.mito"), "", true), ("sorted.case (a module)", &sorted.state, &sorted.api, false)];
        let med = |mut v: Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] };
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        for (name, state, api, raw) in progs {
            let (mut front, mut gen, mut whole, mut check, mut tcc_all, mut clang_o0, mut clang_o2) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
            let (mut ep_lines, mut all_lines) = (0, 0);
            for _ in 0..5 {
                let t = std::time::Instant::now();
                let (st, ap) = if raw { crate::source::load(state).unwrap() } else { (state.to_string(), api.to_string()) };
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
            // the TinyCC build is the reference
            let mut backends: Vec<(&str, Box<dyn Backend>)> = if case.system.is_some() {
                let mut v: Vec<(&str, Box<dyn Backend>)> = vec![("C system", Box::new(CSystemBackend::start(&case, false, &[])))];
                if clang().is_some() {
                    v.push(("C system, clang", Box::new(CSystemBackend::start(&case, true, &[]))));
                    if std::env::var("MITO_SKIP_UBSAN").is_err() { v.push(("C system, clang, undefined-behaviour checks", Box::new(CSystemBackend::start(&case, true, UBSAN)))); }
                }
                v
            } else {
                vec![("C runtime", Box::new(CBackend::start(&case)))]
            };
            if case.system.is_none() && clang().is_some() {
                backends.push(("C runtime, clang", Box::new(CBackend::start_with(&case, true))));
                backends.push(("C runtime, clang, undefined-behaviour checks", Box::new(CBackend::start_flags(&case, true, UBSAN))));
            }
            let mut logs = Vec::new();
            for (name, be) in backends.iter_mut() {
                let log = run(&case, be.as_mut());
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
