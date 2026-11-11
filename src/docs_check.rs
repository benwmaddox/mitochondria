//! The documentation names everything the language and its tools have: a test that fails when a built-in, an executable option, a command, a library module or an example
//! is in none of the docs. (docs/reference.md lists them all; this keeps it so.)

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")) }
    fn read(p: &str) -> String { std::fs::read_to_string(root().join(p)).unwrap_or_else(|e| panic!("{}: {}", p, e)) }

    /// Every doc page: the README, DESIGN, and everything in docs/ (its subdirectories are proposals and notes, left out).
    fn docs() -> String {
        let mut all = read("README.md") + &read("DESIGN.md");
        for e in std::fs::read_dir(root().join("docs")).unwrap().flatten() {
            let p = e.path();
            if p.extension().map_or(false, |x| x == "md") { all += &std::fs::read_to_string(&p).unwrap(); }
        }
        all
    }
    /// Is `word` in the docs as a word of its own (not inside a longer name)?
    fn mentioned(docs: &str, word: &str) -> bool {
        let b = docs.as_bytes();
        let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        docs.match_indices(word).any(|(i, _)| (i == 0 || !ident(b[i - 1])) && (i + word.len() >= b.len() || !ident(b[i + word.len()])))
    }
    /// The quoted names in `src` after `marker`'s line up to a line holding only `end`.
    fn quoted(src: &str, from: &str, to: &str) -> Vec<String> {
        let a = src.find(from).unwrap_or_else(|| panic!("no `{}`", from));
        let b = a + src[a..].find(to).unwrap_or_else(|| panic!("no `{}`", to));
        let body = &src[a..b];
        let mut out = Vec::new();
        let mut rest = body;
        while let Some(i) = rest.find('"') {
            let r = &rest[i + 1..];
            let Some(j) = r.find('"') else { break };
            let s = &r[..j];
            if !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') && !s.starts_with("__") { out.push(s.to_string()); }
            rest = &r[j + 1..];
        }
        out
    }

    #[test]
    fn docs_mention_everything() {
        let docs = docs();
        let mut missing: Vec<String> = Vec::new();
        // built-ins: the names the parser knows (replaced names, which only produce an error saying what to write instead, are left out)
        let lang = read("src/lang.rs");
        for n in quoted(&lang, "fn from(n: &str) -> Option<B> {", "\n    }\n") {
            if ["fwd", "param"].contains(&n.as_str()) { continue; }
            if !mentioned(&docs, &n) { missing.push(format!("built-in `{}`", n)); }
        }
        // executable options and the build's own
        for src in [read("runtime/http.c"), read("src/main.rs")] {
            let mut rest = src.as_str();
            while let Some(i) = rest.find("\"--") {
                let r = &rest[i + 1..];
                let j = r.find('"').unwrap_or(r.len());
                let flag = &r[..j];
                if flag.len() > 2 && flag.chars().all(|c| c.is_ascii_lowercase() || c == '-') && !docs.contains(flag) { missing.push(format!("option `{}`", flag)); }
                rest = &r[j.min(r.len())..];
            }
        }
        // commands
        let main = read("src/main.rs");
        for n in main.split("Some(\"").skip(1).filter_map(|x| x.split('"').next()) {
            if n.chars().all(|c| c.is_ascii_lowercase() || c == '-') && !docs.contains(&format!("`{}", n)) && !docs.contains(&format!("mito {}", n)) { missing.push(format!("command `{}`", n)); }
        }
        // library modules and examples
        for e in std::fs::read_dir(root().join("lib")).unwrap().flatten() {
            let n = e.path().file_stem().unwrap().to_string_lossy().into_owned();
            if !docs.contains(&format!("`{}<", n)) && !mentioned(&docs, &n) { missing.push(format!("library module `{}`", n)); }
        }
        let readme = read("README.md");
        for e in std::fs::read_dir(root().join("examples")).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !readme.contains(&format!("examples/{}", n)) { missing.push(format!("example `{}` (README.md)", n)); }
        }
        assert!(missing.is_empty(), "not in any doc (add it to docs/reference.md or DESIGN.md):\n  {}", missing.join("\n  "));
    }

    #[test]
    fn the_word_finder_matches_whole_names() {
        assert!(mentioned("use `abs(x)` here", "abs") && !mentioned("an absolute value", "abs") && mentioned("abs", "abs") && !mentioned("rabs", "abs") && mentioned("(min, max)", "min"));
    }
}
