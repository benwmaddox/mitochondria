//! Cases (`tests/cases/*.case`): a state, an api and a request script each. Every case runs on every backend; the logs
//! must match each other and the recorded `.out` file, and every endpoint must compile. Warm successful requests must
//! make no heap allocations on a compiled backend. `MITO_BLESS=1 cargo test cases` records the reference backend's log.
//!
//! In the state and api sections, `include path` takes a file (relative to the repository). Request script lines:
//! `METHOD target [json body]`, optionally ending in `=> name` to remember the response's `"token"` as `$name`;
//! `*N` before a request repeats it N times; `@Header: value` adds a header to the next request; `clock ms` sets the
//! request clock; `marks soft hard` sets the memory pressure marks (bytes); `stats` logs the pressure counters.
//! `$name` is replaced anywhere in a line. `allocates target` exempts requests to that target from the allocation
//! check (a known limit of the current runtime, to be removed with it).
use crate::layout::Program;
use crate::runtime::{Instance, Pressure};
use crate::serve::Server;

pub struct Case { pub name: String, pub state: String, pub api: String, pub requests: String }

pub fn load(path: &std::path::Path) -> Case {
    let text = std::fs::read_to_string(path).unwrap();
    let mut parts: [String; 3] = Default::default();
    let mut at = None;
    for line in text.lines() {
        match line.trim_end() {
            "--- state" => at = Some(0), "--- api" => at = Some(1), "--- requests" => at = Some(2),
            l => if let Some(k) = at {
                match l.strip_prefix("include ") {
                    Some(f) if k < 2 => parts[k] += &std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(f.trim())).unwrap_or_else(|e| panic!("{}: {}", f, e)),
                    _ => parts[k] += l,
                }
                parts[k].push('\n');
            }
        }
    }
    let [state, api, requests] = parts;
    Case { name: path.file_stem().unwrap().to_string_lossy().into_owned(), state, api, requests }
}

/// A running program, as the cases drive it. Every way of running a program implements this, so the same scripts and
/// records check each one.
pub trait Backend {
    fn handle(&mut self, method: &str, target: &str, headers: &[(String, String)], body: &str) -> (u16, String);
    /// Status only; the response stays in the backend (no copy, so allocations can be counted).
    fn handle_quiet(&mut self, method: &str, target: &str, headers: &[(String, String)], body: &str) -> u16;
    fn set_clock(&mut self, ms: i64);
    fn set_marks(&mut self, soft: usize, hard: usize);
    /// `state <path> <live bytes>` for the plain values and every collection.
    fn state(&self) -> Vec<String>;
    /// Pressure counters, as one log line.
    fn stats(&self) -> String;
    /// One line per endpoint saying whether it compiled.
    fn notes(&self) -> Vec<String>;
    /// Does this backend promise warm requests without heap allocations?
    fn no_alloc(&self) -> bool;
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
    fn state(&self) -> Vec<String> { self.s.inst.inspect().into_iter().map(|r| format!("state {} {}", r.0, r.4)).collect() }
    fn stats(&self) -> String { match &self.s.pressure { Some(p) => format!("stats refusing={} full_trims={}", p.refusing, p.full_trims), None => "stats no marks".into() } }
    fn notes(&self) -> Vec<String> { self.notes.clone() }
    fn no_alloc(&self) -> bool { self.compiled }
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
        let a0 = crate::alloc_count::allocs();
        let code = be.handle_quiet(m, t, h, b);
        let n = crate::alloc_count::allocs() - a0;
        if (200..300).contains(&code) { allocs += n; }
        if n > 0 && std::env::var("MITO_SHOW_ALLOCS").is_ok() { eprintln!("{}: {} {} {} -> {}: {} allocations", case.name, m, t, b, code, n); }
    }
    (log, allocs)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let mut backends: Vec<(&str, Box<dyn Backend>)> = vec![("interpreted", Box::new(RustBackend::start(&case, false))), ("compiled", Box::new(RustBackend::start(&case, true)))];
            let mut logs = Vec::new();
            for (name, be) in backends.iter_mut() {
                for n in be.notes().iter().filter(|n| !n.ends_with(": compiled")) { problems.push(format!("{} ({}): {}", case.name, name, n)); }
                let (log, allocs) = run(&case, be.as_mut());
                if be.no_alloc() && allocs != 0 { problems.push(format!("{} ({}): {} heap allocations in warm requests", case.name, name, allocs)); }
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
