//! What a program is built for: `platform web;`, `platform cli;`, `platform test;` (and `game`, not built yet) on a line of its own in the entry file
//! (the file given to `mito build`, `run`, `serve` or `check`: a server and a tool over the same modules are two entry files, one platform each;
//! there is no command-line switch). A platform fixes the program's driver (what its `main` is), the entry points it may have, the
//! built-ins it may call and the libraries bundled with it; a program that asks for more says which platform it is for and what is
//! missing. A program that names none is the platform its entry points show (`@get` and the like: web; `@main`: cli).

use crate::lang::Api;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Platform { Web, Cli, Game, Test }

/// A library a program can use: a name, the line its C carries when the program uses it, and what to call it in a message. Several platforms may list the same
/// library (web and a game both bundle `crypto`); a platform is nothing but a driver and a list of these.
pub struct Library { pub name: &'static str, pub marker: &'static str, pub what: &'static str }
pub const LIBRARIES: &[Library] = &[
    Library { name: "sqlite", marker: "#define MITO_USES_SQLITE", what: "tables (SQLite)" },
    Library { name: "zlib", marker: "#define MITO_USES_ZLIB", what: "`gzip;` (zlib)" },
    Library { name: "html", marker: "#define MITO_USES_HTML", what: "HTML parsing (gumbo)" },
    Library { name: "crypto", marker: "#define MITO_USES_CRYPTO", what: "crypto" },
];

impl Platform {
    pub fn parse(name: &str) -> Result<Platform, String> {
        match name {
            "web" => Ok(Platform::Web), "cli" => Ok(Platform::Cli), "game" => Ok(Platform::Game), "test" => Ok(Platform::Test),
            n => Err(format!("`{}` is not a platform: web, cli, game or test", n)),
        }
    }
    pub fn name(self) -> &'static str { match self { Platform::Web => "web", Platform::Cli => "cli", Platform::Game => "game", Platform::Test => "test" } }
    /// The driver: what the executable's `main` is.
    pub fn driver(self) -> &'static str {
        match self { Platform::Web => "the HTTP server (runtime/http.c)", Platform::Cli => "the command-line driver (runtime/cli.c)", Platform::Game => "the window loop", Platform::Test => "the test runner (`mito test`)" }
    }
    /// The libraries bundled with the platform, by name (from `LIBRARIES`).
    pub fn libraries(self) -> &'static [&'static str] {
        match self {
            Platform::Web | Platform::Test => &["sqlite", "zlib", "html", "crypto"],
            Platform::Cli | Platform::Game => &["crypto"],
        }
    }
}

/// `platform NAME;` on a line of its own (a comment after it allowed): the name and its line. At most one.
pub fn declared(src: &str) -> Result<Option<(Platform, usize)>, String> {
    let mut found: Option<(Platform, usize)> = None;
    for (k, l) in src.lines().enumerate() {
        let Some(rest) = l.strip_prefix("platform") else { continue };
        if !rest.starts_with(|c: char| c == ' ' || c == '\t') { continue; }
        let rest = rest.split("//").next().unwrap_or("").trim();
        let Some(name) = rest.strip_suffix(';') else { return Err(format!("line {}: `platform NAME;` (web, cli, game or test)", k + 1)) };
        let p = Platform::parse(name.trim()).map_err(|e| format!("line {}: {}", k + 1, e))?;
        if found.is_some() { return Err(format!("line {}: a program has one `platform`", k + 1)); }
        found = Some((p, k + 1));
    }
    Ok(found)
}

/// What the program's entry points say it is: `@main` a command-line program, endpoints a web program, neither nothing.
pub fn inferred(api: &Api) -> Option<Platform> {
    let main = api.eps.iter().any(|e| e.method == "MAIN");
    let web = api.eps.iter().any(|e| !matches!(e.method.as_str(), "MAIN" | "START"));
    match (main, web) { (true, _) => Some(Platform::Cli), (false, true) => Some(Platform::Web), _ => None }
}

/// Built-ins that only make sense in a server: they read or write an HTTP request or its cache.
const WEB_ONLY: &[&str] = &["set_cookie", "cache_get", "cache_put", "cache_start", "cache_part", "cache_hole", "cache_label", "data_version", "html_parse", "html_parse_in", "request_target", "socket_send", "socket_send_all"];

/// The words of the code outside strings and comments.
fn words(code: &str) -> Vec<&str> {
    let b = code.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        match b[i] {
            b'"' => { i += 1; while i < b.len() && b[i] != b'"' { if b[i] == b'\\' { i += 1; } i += 1; } i += 1; }
            b'/' if b.get(i + 1) == Some(&b'/') => { while i < b.len() && b[i] != b'\n' { i += 1; } }
            c if c.is_ascii_alphabetic() || c == b'_' => { let s = i; while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; } out.push(&code[s..i]); }
            _ => i += 1,
        }
    }
    out
}

/// The platform of a program, given what its entry file declares and what it has: an error saying what does not fit.
pub fn resolve(declared: Option<(Platform, usize)>, api: &Api, code: &str) -> Result<Option<Platform>, String> {
    let Some((p, _)) = declared else { return Ok(inferred(api)) };
    let main = api.eps.iter().any(|e| e.method == "MAIN");
    let endpoints: Vec<&str> = api.eps.iter().filter(|e| !matches!(e.method.as_str(), "MAIN" | "START")).map(|e| e.text.as_str()).collect();
    match p {
        Platform::Web => {
            if main { return Err("platform web: `@main` is for a command-line program (`platform cli;`)".into()); }
        }
        Platform::Cli => {
            if let Some(e) = endpoints.first() { return Err(format!("platform cli: `{}` is an endpoint, and a command-line program has none (`platform web;` is for servers)", e)); }
            if !main { return Err("platform cli: the program needs an `@main` function".into()); }
            for w in words(code) { if WEB_ONLY.contains(&w) { return Err(format!("platform cli: `{}` belongs to a server (platform web): a command-line program has no request, cookie or answer cache", w)); } }
        }
        Platform::Test => {
            if main || !endpoints.is_empty() { return Err("platform test: a program of tests has no entry points (`@main`, `@get` ...); it is run with `mito test`".into()); }
        }
        Platform::Game => return Err("platform game: not built yet (a window, input and shaders through SDL3 are the next platform)".into()),
    }
    Ok(Some(p))
}

/// Do the libraries the program's C needs belong to its platform's list? (A table needs sqlite, `gzip;` zlib, `html_parse` html, a password or a digest crypto.)
pub fn check_bundle(p: Platform, units: &[String]) -> Result<(), String> {
    for lib in LIBRARIES {
        if units.iter().any(|u| u.contains(lib.marker)) && !p.libraries().contains(&lib.name) {
            return Err(format!("platform {}: the program uses {}, which this platform does not bundle (it bundles: {})", p.name(), lib.what, p.libraries().join(", ")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(src: &str) -> (Api, String) {
        let (st, code) = crate::source::load(src).unwrap();
        let prog = crate::layout::Program::parse(&st).unwrap();
        (crate::cgen::check(&prog, &code).unwrap_or_else(|e| panic!("{}", e)), code)
    }
    const HEAD: &str = "struct A { n: i64; }\nstate a: A;\n";

    #[test]
    fn a_declaration_is_found_and_checked() {
        assert_eq!(declared("struct A { n: i64; }\nplatform cli; // a tool\n").unwrap(), Some((Platform::Cli, 2)));
        assert_eq!(declared("struct A { n: i64; }\n").unwrap(), None);
        assert!(declared("platform nowhere;\n").unwrap_err().contains("not a platform"));
        assert!(declared("platform cli\n").unwrap_err().contains("platform NAME;"));
        assert!(declared("platform cli;\nplatform web;\n").unwrap_err().contains("one `platform`"));
        assert_eq!(declared("platformer: i64;\n").unwrap(), None);
    }

    #[test]
    fn the_entry_points_show_the_platform_when_none_is_named() {
        let (a, c) = api(&format!("{}@get(\"/x\") fn x() -> Response<{{}}> {{ return ok({{}}); }}", HEAD));
        assert_eq!(resolve(None, &a, &c).unwrap(), Some(Platform::Web));
        let (a, c) = api(&format!("{}@main fn main() -> Response<text> {{ return ok(\"hi\"); }}", HEAD));
        assert_eq!(resolve(None, &a, &c).unwrap(), Some(Platform::Cli));
        assert_eq!(resolve(Some((Platform::Cli, 1)), &a, &c).unwrap(), Some(Platform::Cli));
    }

    #[test]
    fn a_platform_refuses_what_does_not_belong_to_it() {
        let (web, wc) = api(&format!("{}@get(\"/x\") fn x() -> Response<{{}}> {{ set_cookie(\"a\", \"b\", 60); return ok({{}}); }}", HEAD));
        assert!(resolve(Some((Platform::Cli, 1)), &web, &wc).unwrap_err().contains("is an endpoint"));
        assert!(resolve(Some((Platform::Test, 1)), &web, &wc).unwrap_err().contains("no entry points"));
        let (cli, cc) = api(&format!("{}@main fn main() -> Response<text> {{ return ok(\"hi\"); }}", HEAD));
        assert!(resolve(Some((Platform::Web, 1)), &cli, &cc).unwrap_err().contains("`@main` is for a command-line program"));
        assert!(resolve(Some((Platform::Game, 1)), &cli, &cc).unwrap_err().contains("not built yet"));
        // a cookie is a server's: named in a command-line program it is refused, in a string or a comment it is not
        let (cookie, kc) = api(&format!("{}@main fn main() -> Response<text> {{ set_cookie(\"a\", \"b\", 60); return ok(\"hi\"); }}", HEAD));
        assert!(resolve(Some((Platform::Cli, 1)), &cookie, &kc).unwrap_err().contains("`set_cookie` belongs to a server"));
        let (quiet, qc) = api(&format!("{}// set_cookie is not called\n@main fn main() -> Response<text> {{ return ok(\"set_cookie(\"); }}", HEAD));
        assert_eq!(resolve(Some((Platform::Cli, 1)), &quiet, &qc).unwrap(), Some(Platform::Cli));
        let (none, nc) = api(&format!("{}@start fn init() {{ a.n = 1; }}", HEAD));
        assert!(resolve(Some((Platform::Cli, 1)), &none, &nc).unwrap_err().contains("needs an `@main`"));
    }

    #[test]
    fn a_platform_bundles_its_libraries() {
        let units = vec!["#define MITO_USES_SQLITE 1\n".to_string()];
        assert!(check_bundle(Platform::Web, &units).is_ok());
        assert!(check_bundle(Platform::Cli, &units).unwrap_err().contains("tables (SQLite)"));
        // libraries are shared between platforms by name
        assert!(Platform::Web.libraries().contains(&"crypto") && Platform::Cli.libraries().contains(&"crypto") && Platform::Game.libraries().contains(&"crypto"));
        assert!(Platform::Cli.libraries().iter().all(|l| LIBRARIES.iter().any(|x| x.name == *l)));
        assert!(check_bundle(Platform::Cli, &["#define MITO_USES_CRYPTO 1\n".to_string()]).is_ok());
    }
}
