//! A program is one `.mito` file: its structs, its state, and its functions and endpoints, in any order. The state
//! (structs, `state`) and the code (`fn`, `endpoint`) are parsed separately; `split` gives each its items, every line
//! kept where it was (the other part's lines are blank), so line numbers in errors and in emitted C are the file's.
//! A file whose first item is `system { ... }` is a system of workers instead (src/spec.rs).

/// The state text and the code text of one program file, each as many lines as the file.
pub fn split(src: &str) -> Result<(String, String), String> {
    let b = src.as_bytes();
    let (mut state, mut code) = (String::with_capacity(src.len()), String::with_capacity(src.len()));
    // `text` goes to one part; the other gets the same lines, blank
    let put = |to: &mut String, other: &mut String, text: &str| { to.push_str(text); for c in text.chars() { if c == '\n' { other.push('\n'); } } };
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' { state.push('\n'); code.push('\n'); i += 1; continue; }
        if c.is_ascii_whitespace() { i += 1; continue; }
        if src[i..].starts_with("//") { let e = src[i..].find('\n').map_or(b.len(), |e| i + e); i = e; continue; }
        // an endpoint: `@get("/path")` and the function after it
        if c == b'@' {
            let line = src[..i].matches('\n').count() + 1;
            let from = body_start(src, i).ok_or_else(|| format!("line {}: expected `fn name(...) -> Response<T> {{` after the attribute", line))?;
            let e = end_of_block(src, from).ok_or_else(|| format!("line {}: unclosed `{{`", line))?;
            put(&mut code, &mut state, &src[i..e]); i = e; continue;
        }
        let word_end = src[i..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(b.len(), |e| i + e);
        let word = &src[i..word_end];
        let line = src[..i].matches('\n').count() + 1;
        match word {
            "struct" | "enum" => { let e = end_of_block(src, i).ok_or_else(|| format!("line {}: unclosed struct", line))?; put(&mut state, &mut code, &src[i..e]); i = e; }
            "state" | "instance" | "request" => { let e = src[i..].find(';').map(|e| i + e + 1).ok_or_else(|| format!("line {}: expected `;`", line))?; put(&mut state, &mut code, &src[i..e]); i = e; }
            "fn" | "pub" | "endpoint" | "test" => {
                // an endpoint's path may hold `{param}`: its body starts after the path
                let from = if word == "endpoint" { skip_words(src, word_end, 2) } else { i };
                let e = end_of_block(src, from).ok_or_else(|| format!("line {}: unclosed `{{`", line))?;
                put(&mut code, &mut state, &src[i..e]); i = e;
            }
            // the `web` block is neither state nor code (src/web.rs reads it): its lines are blank in both
            "web" => {
                let e = end_of_block(src, i).ok_or_else(|| format!("line {}: unclosed `web {{`", line))?;
                for c in src[i..e].chars() { if c == '\n' { state.push('\n'); code.push('\n'); } }
                i = e;
            }
            "system" => return Err(format!("line {}: a `system` block is a file of its own, not part of a program", line)),
            "global" => return Err(format!("line {}: `global` was replaced by `state`: a program has one state, a struct holding everything (`state game: Game;`)", line)),
            "" => return Err(format!("line {}: unexpected `{}`", line, &src[i..i + src[i..].chars().next().map_or(1, |c| c.len_utf8())])),
            w => return Err(format!("line {}: expected `struct`, `enum`, `state`, `fn`, `test` or an endpoint, found `{}`", line, w)),
        }
    }
    Ok((state, code))
}

/// Is this file a system of workers (its first item `system`)?
pub fn is_system(src: &str) -> bool {
    let mut s = src;
    loop {
        s = s.trim_start();
        if s.starts_with("//") { s = s.find('\n').map_or("", |e| &s[e..]); continue; }
        return s.starts_with("system") && s[6..].trim_start().starts_with('{');
    }
}

/// Where the body of the endpoint function after the attribute at `i` starts: past its parameters and its return type
/// (`Response<{ id: i64 }>` holds braces of its own).
fn body_start(src: &str, i: usize) -> Option<usize> {
    let b = src.as_bytes();
    let f = i + src[i..].find(" fn ").or_else(|| src[i..].find("\nfn "))? + 1;
    let (mut k, mut depth) = (f + src[f..].find('(')?, 0i32);
    while k < b.len() { match b[k] { b'(' => depth += 1, b')' => { depth -= 1; if depth == 0 { break; } } b'"' => { k += 1; while k < b.len() && b[k] != b'"' { k += 1; } } _ => {} } k += 1; }
    let mut angle = 0i32;
    while k < b.len() {
        match b[k] { b'<' => angle += 1, b'>' if k > 0 && b[k - 1] != b'-' => angle -= 1, b'{' if angle == 0 => return Some(k), _ => {} }
        k += 1;
    }
    None
}

/// The position after `n` more whitespace-separated words from `i`.
fn skip_words(src: &str, mut i: usize, n: usize) -> usize {
    let b = src.as_bytes();
    for _ in 0..n {
        while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
        while i < b.len() && !b[i].is_ascii_whitespace() { i += 1; }
    }
    i
}

/// The end (just past the closing brace) of the item starting at `i`: its first `{` and the matching `}`, skipping
/// strings and comments.
pub(crate) fn end_of_block(src: &str, i: usize) -> Option<usize> {
    let b = src.as_bytes();
    let (mut k, mut depth) = (i, 0usize);
    while k < b.len() {
        match b[k] {
            b'"' => { k += 1; while k < b.len() && b[k] != b'"' { if b[k] == b'\\' { k += 1; } k += 1; } }
            b'/' if b.get(k + 1) == Some(&b'/') => { while k < b.len() && b[k] != b'\n' { k += 1; } continue; }
            b'{' => depth += 1,
            b'}' => { depth = depth.checked_sub(1)?; if depth == 0 { return Some(k + 1); } }
            _ => {}
        }
        k += 1;
    }
    None
}

/// Read and split a program file, and expand its module uses: the state and the code to compile.
pub fn load(src: &str) -> Result<(String, String), String> {
    let (state, code) = split(src)?;
    crate::modules::expand(&state, &code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_splits_into_state_and_code_with_lines_kept() {
        let src = "// a counter\nstruct C { n: i64; }\nstate c: C;\n\nfn bump() { c.n = c.n + 1; }\n@post(\"/n/{k}\") fn post_n_k() -> Response<{}> { bump(); return ok({ \"n\": c.n, \"s\": \"}\" }); }\n";
        let (st, code) = split(src).unwrap();
        assert_eq!(st.lines().count(), src.lines().count());
        assert_eq!(code.lines().count(), src.lines().count());
        assert!(st.contains("struct C") && st.contains("state c: C;") && !st.contains("fn bump"));
        assert!(code.lines().nth(4).unwrap().starts_with("fn bump") && code.lines().nth(5).unwrap().ends_with("}); }") && !code.contains("struct"));
        assert!(split("struct C { n: i64; }\nglobal c: C;\n").unwrap_err().contains("replaced by `state`"));
        assert!(split("struct C { n: i64; }\nstate c: C;\nwat\n").unwrap_err().contains("line 3"));
        assert!(is_system("// workers\nsystem {\n}\n") && !is_system(src));
    }
}
