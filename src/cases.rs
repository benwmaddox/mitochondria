//! Language cases (`tests/cases/*.case`): a state, an api and a request script each. Every case runs interpreted and
//! compiled; the two logs must match each other and the recorded `.out` file, and every endpoint must compile.
//! `MITO_BLESS=1 cargo test cases` records the interpreted log as the new `.out`.
//!
//! Request script lines: `METHOD target [json body]`; `@Header: value` adds a header to the next request;
//! `set path value` writes state directly; `clock ms` sets the request clock.
use crate::layout::Program;
use crate::runtime::Instance;
use crate::serve::Server;

pub struct Case { pub name: String, pub state: String, pub api: String, pub requests: String }

pub fn load(path: &std::path::Path) -> Case {
    let text = std::fs::read_to_string(path).unwrap();
    let mut parts: [String; 3] = Default::default();
    let mut at = None;
    for line in text.lines() {
        match line.trim_end() { "--- state" => at = Some(0), "--- api" => at = Some(1), "--- requests" => at = Some(2), l => if let Some(k) = at { parts[k] += l; parts[k].push('\n'); } }
    }
    let [state, api, requests] = parts;
    Case { name: path.file_stem().unwrap().to_string_lossy().into_owned(), state, api, requests }
}

/// Run a case; returns the log and the compile notes.
pub fn run(case: &Case, compiled: bool) -> (Vec<String>, Vec<String>) {
    let inst = Instance::start(Program::parse(&case.state).unwrap_or_else(|e| panic!("{}: state: {}", case.name, e)), 1 << 30).unwrap();
    let mut s = Server::new(inst, &case.api).unwrap_or_else(|e| panic!("{}: api: {}", case.name, e));
    s.clock = Some(1000);
    s.rng.set(0x1234_5678_9abc_def0);
    let notes = if compiled { s.compile().unwrap_or_else(|e| panic!("{}: compile: {}", case.name, e)).0 } else { Vec::new() };
    let mut log = Vec::new();
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in case.requests.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("//")) {
        if let Some(h) = line.strip_prefix('@') { let (k, v) = h.split_once(':').unwrap(); headers.push((k.trim().into(), v.trim().into())); continue; }
        if let Some(r) = line.strip_prefix("set ") { let (p, v) = r.split_once(' ').unwrap(); s.inst.set(p, v.trim()).unwrap_or_else(|e| panic!("{}: set {}: {:?}", case.name, p, e)); continue; }
        if let Some(t) = line.strip_prefix("clock ") { s.clock = Some(t.trim().parse().unwrap()); continue; }
        let (m, rest) = line.split_once(' ').unwrap();
        let (target, body) = rest.split_once(' ').unwrap_or((rest, ""));
        let (code, out) = s.handle(m, target, &headers, body);
        log.push(format!("{} {} -> {} {}", m, target, code, out));
        headers.clear();
    }
    let rows = s.inst.inspect().into_iter().map(|r| format!("state {} {}", r.0, r.4));
    log.extend(rows);
    (log, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases_answer_alike_compiled_and_interpreted_and_match_their_records() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
        let bless = std::env::var("MITO_BLESS").is_ok();
        let only = std::env::var("MITO_CASE").ok();
        let mut problems = Vec::new();
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().map_or(false, |x| x == "case")).collect();
        files.sort();
        for f in files {
            let case = load(&f);
            if only.as_ref().map_or(false, |o| *o != case.name) { continue; }
            let (a, _) = run(&case, false);
            let (b, notes) = run(&case, true);
            for n in notes.iter().filter(|n| !n.ends_with(": compiled")) { problems.push(format!("{}: {}", case.name, n)); }
            for (x, y) in a.iter().zip(&b) { if x != y { problems.push(format!("{}: interpreted vs compiled\n    {}\n    {}", case.name, x, y)); } }
            if a.len() != b.len() { problems.push(format!("{}: logs differ in length", case.name)); }
            let out = f.with_extension("out");
            let text = a.join("\n") + "\n";
            if bless { std::fs::write(&out, &text).unwrap(); continue; }
            match std::fs::read_to_string(&out) {
                Ok(want) => for (x, y) in want.lines().zip(text.lines()) { if x != y { problems.push(format!("{}: differs from the record\n    want {}\n    got  {}", case.name, x, y)); break; } },
                Err(_) => problems.push(format!("{}: no record; run with MITO_BLESS=1 after checking the log:\n{}", case.name, text)),
            }
        }
        assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    }
}
