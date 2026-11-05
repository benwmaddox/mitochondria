//! The error corpus (`tests/errors/*.mito`): a program with one mistake each, whose first line `// expect: <text>`
//! names text its error must contain (usually `line:col: message`); several such lines, several independent errors. The messages are part of the language.

#[test]
fn every_mistake_gets_its_message() {
    let mut bad = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir("tests/errors").unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().map_or(false, |x| x == "mito")).collect();
    paths.sort();
    assert!(!paths.is_empty());
    for p in paths {
        let text = std::fs::read_to_string(&p).unwrap();
        let wants: Vec<&str> = text.lines().map_while(|l| l.strip_prefix("// expect: ")).collect();
        assert!(!wants.is_empty(), "{}: no `// expect:` line", p.display());
        let want = wants.join(" / ");
        let got = crate::source::load(&text)
            .and_then(|(s, code)| crate::layout::Program::parse(&s).map(|prog| (prog, code)))
            .and_then(|(prog, code)| crate::cgen::check(&prog, &code).map(|_| ()));
        match got {
            Ok(()) => bad.push(format!("{}: loads, expected an error containing {}", p.display(), want)),
            Err(e) if !wants.iter().all(|w| e.contains(w)) => bad.push(format!("{}:\n  expected: {}\n  got:      {}", p.display(), want, e)),
            Err(_) => {}
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}
