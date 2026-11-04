//! Language cases (`tests/cases/*.case`): a state, an api and a request script each. Every case runs interpreted and
//! compiled; the two logs must match each other and the recorded `.out` file, and every endpoint must compile.
//! `MITO_BLESS=1 cargo test cases` records the interpreted log as the new `.out`.
//!
//! In the state and api sections, `include path` takes a file (relative to the repository). Request script lines:
//! `METHOD target [json body]`, optionally ending in `=> name` to remember the response's `"token"` as `$name`;
//! `*N` before a request repeats it N times; `@Header: value` adds a header to the next request; `clock ms` sets the
//! request clock. `$name` is replaced anywhere in a line. `allocates target` exempts a GET from the allocation check
//! (a known limit of the current runtime, to be removed with it).
use crate::layout::Program;
use crate::runtime::Instance;
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

/// Run a case; returns the log, the compile notes, and the heap allocations made by successful GET requests replayed
/// after the script (once to warm up, then counted).
pub fn run(case: &Case, compiled: bool) -> (Vec<String>, Vec<String>, usize) {
    let inst = Instance::start(Program::parse(&case.state).unwrap_or_else(|e| panic!("{}: state: {}", case.name, e)), 1 << 30).unwrap();
    let mut s = Server::new(inst, &case.api).unwrap_or_else(|e| panic!("{}: api: {}", case.name, e));
    s.clock = Some(1000);
    s.rng.set(0x1234_5678_9abc_def0);
    let notes = if compiled { s.compile().unwrap_or_else(|e| panic!("{}: compile: {}", case.name, e)).0 } else { Vec::new() };
    let mut log = Vec::new();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut gets: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut vars: Vec<(String, String)> = Vec::new();
    let mut exempt: Vec<String> = Vec::new();
    for line in case.requests.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("//")) {
        let mut line = line.to_string();
        for (k, v) in vars.iter().rev() { line = line.replace(&format!("${}", k), v); } // later names first, so $a does not eat $ab
        if let Some(h) = line.strip_prefix('@') { let (k, v) = h.split_once(':').unwrap(); headers.push((k.trim().into(), v.trim().into())); continue; }
        if let Some(t) = line.strip_prefix("clock ") { s.clock = Some(t.trim().parse().unwrap()); continue; }
        if let Some(t) = line.strip_prefix("allocates ") { exempt.push(t.trim().to_string()); continue; }
        let (times, line) = match line.strip_prefix('*') { Some(r) => { let (n, rest) = r.split_once(' ').unwrap(); (n.parse::<usize>().unwrap(), rest.to_string()) } None => (1, line) };
        let (line, capture) = match line.rsplit_once(" => ") { Some((l, n)) => (l.to_string(), Some(n.trim().to_string())), None => (line, None) };
        let (m, rest) = line.split_once(' ').unwrap();
        let (target, body) = rest.split_once(' ').unwrap_or((rest, ""));
        for _ in 0..times {
            let (code, out) = s.handle(m, target, &headers, body);
            log.push(format!("{} {} -> {} {}", m, target, code, out));
            if m == "GET" && (200..300).contains(&code) { gets.push((target.to_string(), headers.clone())); }
            if let Some(n) = &capture {
                let tok = out.split("\"token\":\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or_else(|| panic!("{}: no token in {}", case.name, out));
                vars.push((n.clone(), tok.to_string()));
            }
        }
        headers.clear();
    }
    gets.retain(|g| !exempt.contains(&g.0));
    for (t, h) in &gets { s.handle_raw("GET", t, h, ""); }
    let a0 = crate::alloc_count::allocs();
    for (t, h) in &gets { s.handle_raw("GET", t, h, ""); }
    let allocs = crate::alloc_count::allocs() - a0;
    let rows = s.inst.inspect().into_iter().map(|r| format!("state {} {}", r.0, r.4));
    log.extend(rows);
    (log, notes, allocs)
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
            let (a, _, ia) = run(&case, false); if std::env::var("MITO_SHOW_ALLOCS").is_ok() { eprintln!("{}: interpreted warm GET allocs {}", case.name, ia); }
            let (b, notes, allocs) = run(&case, true);
            if allocs != 0 { problems.push(format!("{}: {} heap allocations in warm GET requests", case.name, allocs)); }
            for n in notes.iter().filter(|n| !n.ends_with(": compiled")) { problems.push(format!("{}: {}", case.name, n)); }
            for (x, y) in a.iter().zip(&b) { if x != y { problems.push(format!("{}: interpreted vs compiled\n    {}\n    {}", case.name, x, y)); } }
            if a.len() != b.len() { problems.push(format!("{}: logs differ in length", case.name)); }
            let out = f.with_extension("out");
            let text = a.join("\n") + "\n";
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
