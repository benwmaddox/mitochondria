//! A program's `web { ... }` block: how its answers look on the wire, beyond what each endpoint says.
//!
//! ```text
//! web {
//!     headers { "X-Frame-Options": "SAMEORIGIN"; "Vary": "Accept"; }   // on every endpoint answer, in place of the built-in policy lines
//!     etag "max-age=0, private, must-revalidate";  // a weak ETag on every 200 answer to a GET (If-None-Match naming it: 304),
//!                                               // with this Cache-Control unless the endpoint set one (`etag;`: none added)
//!     gzip;                                     // bodies gzip-compressed when the request accepts gzip
//!     static "/" from "public" {                // the directory's files, kept in the program, answered before the endpoints
//!         "Cache-Control": "public, max-age=2592000";
//!         types { ".map": "text/plain"; }      // content types beyond the built-in ones, by file extension
//!     }
//! }
//! ```
//! `split` (src/source.rs) leaves the block out of the state and the code; `parse` reads it and `c` writes it as data for the
//! HTTP front end (runtime/http.c), which is all that uses it.

/// A `static` block: the URL prefix, the directory (relative to the program), its header lines, and its files (URL path,
/// content type, bytes), sorted by path.
pub struct Static { pub prefix: String, pub dir: String, pub headers: Vec<(String, String)>, pub files: Vec<(String, String, Vec<u8>)> }
pub struct Web { pub headers: Option<Vec<(String, String)>>, pub etag: Option<String>, pub gzip: bool, pub statics: Vec<Static> }

/// Content types by extension: the usual ones; a `types { }` block adds or replaces them.
const TYPES: &[(&str, &str)] = &[
    (".html", "text/html"), (".htm", "text/html"), (".css", "text/css"), (".js", "text/javascript"), (".mjs", "text/javascript"),
    (".txt", "text/plain"), (".json", "application/json"), (".xml", "application/xml"), (".svg", "image/svg+xml"),
    (".png", "image/png"), (".jpg", "image/jpeg"), (".jpeg", "image/jpeg"), (".gif", "image/gif"), (".webp", "image/webp"),
    (".ico", "image/vnd.microsoft.icon"), (".mp3", "audio/mpeg"), (".mp4", "video/mp4"), (".webm", "video/webm"),
    (".woff", "font/woff"), (".woff2", "font/woff2"), (".pdf", "application/pdf"), (".wasm", "application/wasm"),
    (".gz", "application/gzip"), (".webmanifest", "application/manifest+json"),
];

/// A header name: an HTTP token, and not one the front end writes itself.
pub fn header_name_ok(n: &str) -> Result<(), String> {
    if n.is_empty() || n.len() > 64 || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) {
        return Err(format!("`{}` is not a header name (letters, digits and !#$%&'*+-.^_`|~)", n));
    }
    let l = n.to_ascii_lowercase();
    if ["content-length", "connection", "transfer-encoding", "content-encoding", "keep-alive"].contains(&l.as_str()) {
        return Err(format!("`{}` is written by the server itself", n));
    }
    Ok(())
}
fn value_ok(v: &str) -> Result<(), String> {
    if v.len() > 8000 || !v.bytes().all(|b| b == b'\t' || (0x20..0x7f).contains(&b)) { return Err(format!("`{}`: a header value is printable ASCII, at most 8000 bytes", v)); }
    Ok(())
}

#[derive(Debug, PartialEq, Clone)]
enum T { Word(String), Str(String), Sym(char) }

/// The tokens of the block's text, each with its line in the file (`line0` is the block's first line).
fn lex(s: &str, line0: usize) -> Result<Vec<(T, usize)>, String> {
    let b = s.as_bytes();
    let (mut i, mut line, mut out) = (0, line0, Vec::new());
    while i < b.len() {
        let c = b[i];
        if c == b'\n' { line += 1; i += 1; continue; }
        if c.is_ascii_whitespace() { i += 1; continue; }
        if s[i..].starts_with("//") { while i < b.len() && b[i] != b'\n' { i += 1; } continue; }
        if c == b'"' {
            let mut v = String::new(); i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\n' { return Err(format!("line {}: a string ends on its line", line)); }
                if b[i] == b'\\' && i + 1 < b.len() { i += 1; match b[i] { b'"' => v.push('"'), b'\\' => v.push('\\'), _ => return Err(format!("line {}: only \\\" and \\\\ in a web block's strings", line)) } i += 1; continue; }
                let ch = s[i..].chars().next().unwrap_or(' '); v.push(ch); i += ch.len_utf8();
            }
            if i >= b.len() { return Err(format!("line {}: unclosed string", line)); }
            i += 1; out.push((T::Str(v), line)); continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let st = i; while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
            out.push((T::Word(s[st..i].to_string()), line)); continue;
        }
        if b"{};:".contains(&c) { out.push((T::Sym(c as char), line)); i += 1; continue; }
        return Err(format!("line {}: unexpected `{}` in the web block", line, s[i..].chars().next().unwrap_or(' ')));
    }
    Ok(out)
}

struct P { t: Vec<(T, usize)>, i: usize }
impl P {
    fn line(&self) -> usize { self.t.get(self.i).or(self.t.last()).map_or(0, |x| x.1) }
    fn next(&mut self) -> Option<T> { let x = self.t.get(self.i).map(|x| x.0.clone()); self.i += 1; x }
    fn eat(&mut self, c: char) -> Result<(), String> {
        let l = self.line();
        match self.next() { Some(T::Sym(s)) if s == c => Ok(()), _ => Err(format!("line {}: expected `{}` in the web block", l, c)) }
    }
    fn str(&mut self, what: &str) -> Result<String, String> {
        let l = self.line();
        match self.next() { Some(T::Str(s)) => Ok(s), _ => Err(format!("line {}: expected {} (a string) in the web block", l, what)) }
    }
    fn at_sym(&self, c: char) -> bool { matches!(self.t.get(self.i), Some((T::Sym(s), _)) if *s == c) }
    /// `"Name": "value";` lines up to the closing `}` (the `{` already eaten); a `types { }` block too when `types` is given.
    fn pairs(&mut self, mut types: Option<&mut Vec<(String, String)>>) -> Result<Vec<(String, String)>, String> {
        let mut v: Vec<(String, String)> = Vec::new();
        while !self.at_sym('}') {
            let l = self.line();
            if let (Some(ts), Some((T::Word(w), _))) = (types.as_deref_mut(), self.t.get(self.i)) {
                if w == "types" {
                    self.i += 1; self.eat('{')?;
                    while !self.at_sym('}') {
                        let l = self.line();
                        let ext = self.str("an extension")?; self.eat(':')?; let ty = self.str("a content type")?; self.eat(';')?;
                        if !ext.starts_with('.') || ext.len() < 2 { return Err(format!("line {}: `{}`: an extension starts with a dot, as in \".map\"", l, ext)); }
                        value_ok(&ty).map_err(|e| format!("line {}: {}", l, e))?;
                        ts.retain(|x| x.0 != ext); ts.push((ext, ty));
                    }
                    self.eat('}')?; continue;
                }
            }
            if self.i >= self.t.len() { return Err(format!("line {}: unclosed `{{` in the web block", l)); }
            let k = self.str("a header name")?; self.eat(':')?; let val = self.str("a header value")?; self.eat(';')?;
            header_name_ok(&k).map_err(|e| format!("line {}: {}", l, e))?;
            value_ok(&val).map_err(|e| format!("line {}: {}", l, e))?;
            if v.iter().any(|x| x.0.eq_ignore_ascii_case(&k)) { return Err(format!("line {}: `{}` is given twice", l, k)); }
            v.push((k, val));
        }
        self.eat('}')?;
        Ok(v)
    }
}

/// The `web` block of a program file, if it has one: `(text, first line)` as `split` found it.
pub fn find(src: &str) -> Option<(String, usize)> {
    let mut lines = 0;
    let mut rest = src;
    loop {
        let t = rest.trim_start();
        lines += rest[..rest.len() - t.len()].matches('\n').count();
        rest = t;
        if rest.is_empty() { return None; }
        if rest.starts_with("//") { let e = rest.find('\n').unwrap_or(rest.len()); rest = &rest[e..]; continue; }
        if rest.starts_with("web") && rest[3..].trim_start().starts_with('{') {
            let e = crate::source::end_of_block(rest, 0)?;
            return Some((rest[..e].to_string(), lines + 1));
        }
        // skip the item: to the end of its block, or of its line for one ending with `;`
        let semi = rest.find(';');
        let brace = rest.find('{');
        let e = match (semi, brace) {
            (Some(s), Some(b)) if s < b => s + 1,
            (_, Some(b)) => { let from = b; match crate::source::end_of_block(rest, from) { Some(e) => e, None => return None } }
            (Some(s), None) => s + 1,
            (None, None) => return None,
        };
        lines += rest[..e].matches('\n').count();
        rest = &rest[e..];
    }
}

/// Parse a `web { ... }` block (its text from `web` to the closing brace, starting on line `line0`), reading the files of its
/// `static` directories relative to `dir`.
pub fn parse(text: &str, line0: usize, dir: &std::path::Path) -> Result<Web, String> {
    let mut p = P { t: lex(text, line0)?, i: 0 };
    match p.next() { Some(T::Word(w)) if w == "web" => {} _ => return Err(format!("line {}: expected `web {{`", line0)) }
    p.eat('{')?;
    let mut w = Web { headers: None, etag: None, gzip: false, statics: Vec::new() };
    while !p.at_sym('}') {
        let l = p.line();
        match p.next() {
            Some(T::Word(k)) if k == "headers" => {
                if w.headers.is_some() { return Err(format!("line {}: one `headers` block", l)); }
                p.eat('{')?; w.headers = Some(p.pairs(None)?);
            }
            Some(T::Word(k)) if k == "etag" => {
                let cc = if matches!(p.t.get(p.i), Some((T::Str(_), _))) { p.str("a Cache-Control value")? } else { String::new() };
                value_ok(&cc).map_err(|e| format!("line {}: {}", l, e))?;
                p.eat(';')?; w.etag = Some(cc);
            }
            Some(T::Word(k)) if k == "gzip" => { p.eat(';')?; w.gzip = true; }
            Some(T::Word(k)) if k == "static" => {
                let prefix = p.str("the URL prefix")?;
                if !prefix.starts_with('/') || !prefix.ends_with('/') || prefix.contains("//") || !prefix.bytes().all(|b| (0x21..0x7f).contains(&b)) {
                    return Err(format!("line {}: `{}`: a static prefix is a path starting and ending with `/`, as in \"/\" or \"/assets/\"", l, prefix));
                }
                match p.next() { Some(T::Word(f)) if f == "from" => {} _ => return Err(format!("line {}: expected `static \"/prefix/\" from \"directory\" {{ ... }}`", l)) }
                let d = p.str("the directory")?;
                p.eat('{')?;
                let mut types: Vec<(String, String)> = Vec::new();
                let headers = p.pairs(Some(&mut types))?;
                let files = read_dir(&dir.join(&d), &prefix, &types).map_err(|e| format!("line {}: static \"{}\": {}", l, d, e))?;
                w.statics.push(Static { prefix, dir: d, headers, files });
            }
            _ => return Err(format!("line {}: a web block holds `headers {{ }}`, `etag [\"cache control\"];`, `gzip;` and `static \"/prefix/\" from \"dir\" {{ }}`", l)),
        }
    }
    p.eat('}')?;
    // two static blocks may not answer the same path
    let mut all: Vec<&str> = w.statics.iter().flat_map(|s| s.files.iter().map(|f| f.0.as_str())).collect();
    all.sort();
    if let Some(d) = all.windows(2).find(|x| x[0] == x[1]) { return Err(format!("line {}: two static directories hold {}", line0, d[0])); }
    Ok(w)
}

/// Every file under `root` (following no links), as (URL path, content type, bytes), sorted by path.
fn read_dir(root: &std::path::Path, prefix: &str, extra: &[(String, String)]) -> Result<Vec<(String, String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let md = std::fs::metadata(root).map_err(|e| format!("{}: {}", root.display(), e))?;
    if !md.is_dir() { return Err(format!("{} is not a directory", root.display())); }
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {}", d.display(), e))? {
            let e = e.map_err(|e| e.to_string())?;
            let ft = e.file_type().map_err(|e| e.to_string())?;
            let path = e.path();
            if ft.is_dir() { stack.push(path); continue; }
            if !ft.is_file() { continue; }
            let rel = path.strip_prefix(root).map_err(|e| e.to_string())?;
            let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            let url = format!("{}{}", prefix, parts.join("/"));
            if !url.bytes().all(|b| (0x21..0x7f).contains(&b) && !b"%?#\"\\".contains(&b)) { return Err(format!("{}: a file name served as a path is printable ASCII without %?#\"\\", rel.display())); }
            let name = parts.last().cloned().unwrap_or_default();
            let ext = name.rfind('.').map(|k| name[k..].to_ascii_lowercase()).unwrap_or_default();
            let ty = extra.iter().find(|t| t.0 == ext).or_else(|| None).map(|t| t.1.clone())
                .or_else(|| TYPES.iter().find(|t| t.0 == ext).map(|t| t.1.to_string()))
                .ok_or_else(|| format!("{}: no content type for `{}`: give it in `types {{ \"{}\": \"...\"; }}`", rel.display(), if ext.is_empty() { "(no extension)" } else { &ext }, if ext.is_empty() { "." } else { &ext }))?;
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
            out.push((url, ty, bytes));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn c_str(s: &str) -> String { crate::cgen::c_lit(s.as_bytes()) }
fn lines(h: &[(String, String)]) -> String { h.iter().map(|(k, v)| format!("{}: {}\r\n", k, v)).collect() }

/// The block as C for runtime/http.c, ahead of it in the unit: `MITO_WEB`, the header lines, the flags, and the static files.
pub fn c(w: &Web) -> String {
    let mut o = String::from("/* ---- the program's web block (generated) ---- */\n#define MITO_WEB 1\n");
    if let Some(h) = &w.headers { o += &format!("#define MITO_WEB_HEADERS 1\nstatic const char mito_web_headers[] = {};\n", c_str(&lines(h))); }
    if let Some(cc) = &w.etag { o += &format!("#define MITO_WEB_ETAG 1\nstatic const char mito_web_etag_cc[] = {};\n", c_str(cc)); }
    if w.gzip { o += "#define MITO_WEB_GZIP 1\n#define MITO_USES_ZLIB 1\n"; }
    let mut files: Vec<(String, String, usize, usize)> = Vec::new(); // path, type, block, file index
    for (b, s) in w.statics.iter().enumerate() {
        o += &format!("static const char mito_sh{}[] = {};\n", b, c_str(&lines(&s.headers)));
        for (k, f) in s.files.iter().enumerate() {
            // the bytes as 64-bit words (fast to compile), read back as bytes (little-endian)
            let mut words = String::with_capacity(f.2.len() * 3);
            for ch in f.2.chunks(8) {
                let mut x = [0u8; 8]; x[..ch.len()].copy_from_slice(ch);
                if !words.is_empty() { words.push(','); }
                words += &u64::from_le_bytes(x).to_string(); words += "u";
            }
            if words.is_empty() { words.push('0'); }
            o += &format!("static const u64 mito_sf{}_{}[] = {{{}}};\n", b, k, words);
            files.push((f.0.clone(), f.1.clone(), b, k));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    o += "typedef struct { const char *path; u32 pn; const char *type; const u64 *data; u64 len; const char *headers; } MitoStatic;\n";
    o += &format!("#define MITO_NSTATIC {}\nstatic const MitoStatic mito_static[] = {{\n", files.len());
    for (path, ty, b, k) in &files {
        let len = w.statics[*b].files[*k].2.len();
        o += &format!("  {{ {}, {}, {}, mito_sf{}_{}, {}, mito_sh{} }},\n", c_str(path), path.len(), c_str(ty), b, k, len, b);
    }
    if files.is_empty() { o += "  { 0, 0, 0, 0, 0, 0 },\n"; }
    o += "};\n";
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_block_parses_and_refuses_what_it_cannot_serve() {
        let d = std::env::temp_dir().join(format!("mito-web-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("pub/assets")).unwrap();
        std::fs::write(d.join("pub/robots.txt"), "User-agent: *\n").unwrap();
        std::fs::write(d.join("pub/assets/app.css"), "a{}").unwrap();
        std::fs::write(d.join("pub/assets/app.js.map"), "{}").unwrap();
        let src = "struct A { n: i64; }\nstate a: A;\n\nweb {\n    headers { \"X-Frame-Options\": \"SAMEORIGIN\"; }\n    etag;\n    gzip;\n    static \"/\" from \"pub\" {\n        \"Cache-Control\": \"public\";\n        types { \".map\": \"text/plain\"; }\n    }\n}\n";
        let (text, line) = find(src).unwrap();
        assert_eq!(line, 4);
        let w = parse(&text, line, &d).unwrap();
        assert!(w.etag == Some(String::new()) && w.gzip && w.headers.as_ref().unwrap()[0].0 == "X-Frame-Options");
        let f: Vec<(&str, &str)> = w.statics[0].files.iter().map(|f| (f.0.as_str(), f.1.as_str())).collect();
        assert_eq!(f, vec![("/assets/app.css", "text/css"), ("/assets/app.js.map", "text/plain"), ("/robots.txt", "text/plain")]);
        // an extension without a type is refused, naming it
        let e = parse("web {\n static \"/\" from \"pub\" { }\n}", 1, &d).err().unwrap();
        assert!(e.contains("line 2") && e.contains("`.map`"), "{}", e);
        assert!(parse("web { headers { \"Content-Length\": \"1\"; } }", 1, &d).err().unwrap().contains("server itself"));
        assert!(parse("web { headers { \"A\": \"x\nb\"; } }", 1, &d).is_err());
        assert!(parse("web { static \"assets\" from \"pub\" { } }", 1, &d).err().unwrap().contains("starting and ending"));
        assert!(parse("web { cache; }", 1, &d).err().unwrap().contains("holds"));
        assert!(find("struct A { n: i64; }\nstate a: A;\nfn f() { let web = 1; }\n").is_none());
        let c = c(&w);
        assert!(c.contains("#define MITO_NSTATIC 3") && c.contains("X-Frame-Options: SAMEORIGIN\\015\\012"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
