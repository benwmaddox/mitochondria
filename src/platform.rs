//! What a program is built for: `platform web;`, `platform cli;`, `platform game;`, `platform test;` on a line of its own in the entry file
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
    // loaded when the game starts (SDL3 and the system's OpenGL), not linked: nothing is bundled but the driver (runtime/game.c)
    Library { name: "sdl3", marker: "#define MITO_USES_SDL3", what: "a window and OpenGL (SDL3)" },
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
        match self { Platform::Web => "the HTTP server (runtime/http.c)", Platform::Cli => "the command-line driver (runtime/cli.c)", Platform::Game => "the window loop (runtime/game.c)", Platform::Test => "the test runner (`mito test`)" }
    }
    /// The libraries bundled with the platform, by name (from `LIBRARIES`).
    pub fn libraries(self) -> &'static [&'static str] {
        match self {
            Platform::Web | Platform::Test => &["sqlite", "zlib", "html", "crypto"],
            Platform::Cli => &["crypto"],
            Platform::Game => &["sdl3", "crypto"],
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
    let game = api.eps.iter().any(|e| matches!(e.method.as_str(), "FRAME" | "SHADER"));
    let web = api.eps.iter().any(|e| !matches!(e.method.as_str(), "MAIN" | "START" | "FRAME" | "SHADER"));
    match (game, main, web) { (true, _, _) => Some(Platform::Game), (_, true, _) => Some(Platform::Cli), (_, _, true) => Some(Platform::Web), _ => None }
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

/// Built-ins that touch the process's own standard error, standard input and files: for a command-line program (`@main`) only.
const CLI_ONLY: &[&str] = &["eprint", "print", "flush", "stdin_text", "read_file", "file_exists"];

/// The platform of a program, given what its entry file declares and what it has: an error saying what does not fit.
pub fn resolve(declared: Option<(Platform, usize)>, api: &Api, code: &str) -> Result<Option<Platform>, String> {
    let r = resolve_entries(declared, api, code)?;
    if let Some(p) = r {
        if p != Platform::Cli {
            let w = words(code);
            for (i, word) in w.iter().enumerate() { if CLI_ONLY.contains(word) && (i == 0 || w[i - 1] != "fn") { return Err(format!("platform {}: `{}` belongs to a command-line program (`@main`, platform cli): a {} has no standard input or command line", p.name(), word, if p == Platform::Web { "server" } else { "program of this platform" })); } }
        }
    }
    Ok(r)
}

fn resolve_entries(declared: Option<(Platform, usize)>, api: &Api, code: &str) -> Result<Option<Platform>, String> {
    let Some((p, _)) = declared else { return Ok(inferred(api)) };
    let main = api.eps.iter().any(|e| e.method == "MAIN");
    let frame = api.eps.iter().any(|e| e.method == "FRAME");
    let game_eps = api.eps.iter().any(|e| matches!(e.method.as_str(), "FRAME" | "SHADER"));
    let endpoints: Vec<&str> = api.eps.iter().filter(|e| !matches!(e.method.as_str(), "MAIN" | "START" | "FRAME" | "SHADER")).map(|e| e.text.as_str()).collect();
    match p {
        Platform::Web => {
            if main { return Err("platform web: `@main` is for a command-line program (`platform cli;`)".into()); }
            if game_eps { return Err("platform web: `@frame` and `@shader` are for a game (`platform game;`)".into()); }
        }
        Platform::Cli => {
            if let Some(e) = endpoints.first() { return Err(format!("platform cli: `{}` is an endpoint, and a command-line program has none (`platform web;` is for servers)", e)); }
            if game_eps { return Err("platform cli: `@frame` and `@shader` are for a game (`platform game;`)".into()); }
            if !main { return Err("platform cli: the program needs an `@main` function".into()); }
            for w in words(code) { if WEB_ONLY.contains(&w) { return Err(format!("platform cli: `{}` belongs to a server (platform web): a command-line program has no request, cookie or answer cache", w)); } }
        }
        Platform::Test => {
            if main || game_eps || !endpoints.is_empty() { return Err("platform test: a program of tests has no entry points (`@main`, `@get` ...); it is run with `mito test`".into()); }
        }
        Platform::Game => {
            if let Some(e) = endpoints.first() { return Err(format!("platform game: `{}` is an endpoint, and a game has none (`platform web;` is for servers)", e)); }
            if main { return Err("platform game: `@main` is for a command-line program (`platform cli;`)".into()); }
            if !frame { return Err("platform game: the program needs an `@frame` function".into()); }
            if !api.eps.iter().any(|e| e.method == "SHADER") { return Err("platform game: the program needs at least one `@shader` function (the frame names the shader it draws with)".into()); }
            for w in words(code) { if WEB_ONLY.contains(&w) { return Err(format!("platform game: `{}` belongs to a server (platform web): a game has no request, cookie or answer cache", w)); } }
        }
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
        assert!(resolve(Some((Platform::Game, 1)), &cli, &cc).unwrap_err().contains("`@main` is for a command-line program"));
        // a cookie is a server's: named in a command-line program it is refused, in a string or a comment it is not
        let (cookie, kc) = api(&format!("{}@main fn main() -> Response<text> {{ set_cookie(\"a\", \"b\", 60); return ok(\"hi\"); }}", HEAD));
        assert!(resolve(Some((Platform::Cli, 1)), &cookie, &kc).unwrap_err().contains("`set_cookie` belongs to a server"));
        let (quiet, qc) = api(&format!("{}// set_cookie is not called\n@main fn main() -> Response<text> {{ return ok(\"set_cookie(\"); }}", HEAD));
        assert_eq!(resolve(Some((Platform::Cli, 1)), &quiet, &qc).unwrap(), Some(Platform::Cli));
        let (none, nc) = api(&format!("{}@start fn init() {{ a.n = 1; }}", HEAD));
        assert!(resolve(Some((Platform::Cli, 1)), &none, &nc).unwrap_err().contains("needs an `@main`"));
    }

    #[test]
    fn a_game_has_a_frame_and_shaders_and_nothing_of_a_server() {
        let game = format!("{}@shader fn sky() -> Response<text> {{ return ok(\"void main() {{}}\"); }}
@frame fn frame(dt: f64 from time, w: bool from key(\"w\"), dx: f64 from mouse(\"dx\")) -> Response<text> {{ return ok(\"sky\"); }}", HEAD);
        let (g, gc) = api(&game);
        assert_eq!(resolve(None, &g, &gc).unwrap(), Some(Platform::Game));
        assert_eq!(resolve(Some((Platform::Game, 1)), &g, &gc).unwrap(), Some(Platform::Game));
        assert!(resolve(Some((Platform::Web, 1)), &g, &gc).unwrap_err().contains("`@frame` and `@shader` are for a game"));
        assert!(resolve(Some((Platform::Cli, 1)), &g, &gc).unwrap_err().contains("`@frame` and `@shader` are for a game"));
        assert!(resolve(Some((Platform::Test, 1)), &g, &gc).unwrap_err().contains("no entry points"));
        // a frame without a shader draws nothing; a game with an endpoint is a server
        let (n, nc) = api(&format!("{}@frame fn frame(dt: f64 from time) -> Response<text> {{ return ok(\"sky\"); }}", HEAD));
        assert!(resolve(Some((Platform::Game, 1)), &n, &nc).unwrap_err().contains("at least one `@shader`"));
        let (e, ec) = api(&format!("{}@get(\"/x\") fn x() -> Response<{{}}> {{ return ok({{}}); }}
@frame fn frame(dt: f64 from time) -> Response<text> {{ return ok(\"sky\"); }}", HEAD));
        assert!(resolve(Some((Platform::Game, 1)), &e, &ec).unwrap_err().contains("is an endpoint"));
        let (s, sc) = api(&format!("{}@shader fn sky() -> Response<text> {{ return ok(\"x\"); }}", HEAD));
        assert!(resolve(Some((Platform::Game, 1)), &s, &sc).unwrap_err().contains("needs an `@frame`"));
        // the window is a library of the game platform only
        assert!(check_bundle(Platform::Game, &["#define MITO_USES_SDL3 1
".to_string()]).is_ok());
        assert!(check_bundle(Platform::Web, &["#define MITO_USES_SDL3 1
".to_string()]).unwrap_err().contains("SDL3"));
        assert!(check_bundle(Platform::Game, &["#define MITO_USES_SQLITE 1
".to_string()]).unwrap_err().contains("tables"));
    }

    /// A function that takes a plain struct (a field of the state) says what is wrong in the terms of the program: a value of another kind, a nullable struct, a result that is a plain struct, a struct that is nowhere in the state.
    #[test]
    fn plain_struct_parameter_errors_name_the_struct_and_the_parameter() {
        let head = "struct Pos { x: i64; }\nstruct Lone { z: i64; }
struct Solo { a: i64; }\nstruct Item { v: i64; }\nstruct W { solo: Solo; origin: Pos; items: pool<Item, 4>; pos: pool<Pos, 4>; }\nstate w: W;\nfn plen(p: Pos) -> i64 { return p.x; }\nfn take(it: Item) -> i64 { return it.v; }\n";
        let err = |tail: &str| -> String {
            let (st, code) = crate::source::load(&format!("{}{}", head, tail)).unwrap();
            let prog = crate::layout::Program::parse(&st).unwrap();
            match crate::cgen::check(&prog, &code) { Ok(_) => String::new(), Err(e) => e }
        };
        let ep = |body: &str| format!("@post(\"/a\") fn a() -> Response<{{}}> {{ {} return ok({{}}); }}", body);
        // a plain struct, or an item of the pool that holds the same struct, is fine; so is a function of a type on either
        assert_eq!(err(&ep("let a = plen(w.origin); let p = w.pos.add(); let b = plen(p);")), "");
        assert!(err(&ep("let a = plen(3);")).contains("parameter `p` of `plen` is `Pos`, given an integer"), "{}", err(&ep("let a = plen(3);")));
        assert!(err(&ep("let i = w.items.add(); let a = plen(i);")).contains("parameter `p` of `plen` is `Pos`, given an item of w.items"));
        assert!(err(&ep("let a = take(w.origin);")).contains("parameter `it` of `take` is `Item`, given a struct `Pos`"));
        assert!(err(&ep("let a = plen(w);")).contains("given a struct `W`"));
        assert!(err(&format!("fn maybe(p: Pos?) -> i64 {{ return 1; }}
{}", ep("let a = maybe(w.origin);"))).contains("parameter `p` of `maybe` is `Pos?`, given a struct `Pos`"));
        assert!(err(&format!("fn maybe(p: Solo?) -> i64 {{ return 1; }}
{}", ep("let a = maybe(w.solo);"))).contains("`Solo?`: a plain struct is part of the state, it cannot be null"));
        assert_eq!(err("fn same(p: Solo) -> Solo { return p; } @post(\"/a\") fn a() -> Response<{}> { let q = same(w.solo); return ok({}); }"), ""); // a struct may be returned into a `let`
        assert!(err("fn same(p: Solo) -> Solo { return p; } fn val(p: Solo) -> i64 { return p.z; } @post(\"/a\") fn a() -> Response<{}> { let q = val(same(w.solo)); return ok({}); }").contains("which has to go somewhere"));
        assert!(err(&format!("fn lone(p: Lone) -> i64 {{ return p.z; }}\n{}", ep("let a = lone(3);"))).contains("no collection holds `Lone` items"));
    }

    /// Receiver functions say what is wrong in the terms the program was written in: a type that lacks a function, a function declared twice, a name that is a collection operation, a call with the wrong number of arguments.
    #[test]
    fn receiver_function_errors_name_the_type_and_the_function() {
        let head = "struct P { hp: i64; }
struct M { hp: i64; }
struct W { ps: pool<P, 4>; ms: pool<M, 4>; }
state w: W;
fn hit(self: P) { self.hp -= 1; }
";
        let err = |tail: &str| -> String {
            let (st, code) = crate::source::load(&format!("{}{}", head, tail)).unwrap();
            let prog = crate::layout::Program::parse(&st).unwrap();
            match crate::cgen::check(&prog, &code) { Ok(_) => String::new(), Err(e) => e }
        };
        assert!(err("@post(\"/a\") fn a() -> Response<{}> { let m = w.ms.add(); m.hit(); return ok({}); }").contains("`M` has no function `hit`"));
        assert!(err("@post(\"/a\") fn a() -> Response<{}> { let m = w.ms.add(); m.hit(); return ok({}); }").contains("`hit` is declared for P"));
        assert!(err("fn hit(self: P) { self.hp -= 2; }").contains("`hit` is declared twice for `P`"));
        assert_eq!(err("fn count(self: P) -> i64 { return 1; }"), "");   // a collection operation's name may be a function of a type
        assert!(err("@post(\"/a\") fn a() -> Response<{}> { let p = w.ps.add(); p.hit(1); return ok({}); }").contains("`P.hit` takes 0 arguments, given 1"));
        assert!(err("@post(\"/a\") fn a() -> Response<{}> { let p = w.ps.add(); p.fly(); return ok({}); }").contains("unknown function `p.fly`"));
        assert!(err("fn hit2(self: P?) { }").contains("`self` is a struct"));
        // the same name for two types is fine, and none of the messages shows an internal spelling
        assert_eq!(err("fn hit(self: M) { self.hp -= 1; }
@post(\"/a\") fn a() -> Response<{}> { let m = w.ms.add(); m.hit(); let p = w.ps.add(); p.hit(); return ok({}); }"), "");
        assert!(!err("@post(\"/a\") fn a() -> Response<{}> { let p = w.ps.add(); p.hit(1); return ok({}); }").contains("::"));
    }

    #[test]
    fn a_key_must_be_one_the_window_reads() {
        let bad = crate::source::load(&format!("{}@frame fn frame(k: bool from key(\"enter\")) -> Response<text> {{ return ok(\"x\"); }}", HEAD)).and_then(|(st, code)| {
            let prog = crate::layout::Program::parse(&st)?;
            crate::cgen::check(&prog, &code).map(|_| ())
        });
        assert!(bad.unwrap_err().contains("is not a key the game platform reads"));
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
