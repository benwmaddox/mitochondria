//! The HTTP front end, end to end: raw requests over real sockets, one connection each, to every server that serves
//! examples/chat (one program) and examples/chat-sharded (a system). Each must answer exactly as `tests/http.record`
//! says, byte for byte. `MITO_BLESS=1` records the TinyCC build's answers. (The record was first made by the Rust
//! servers this front end replaced; the C executables matched it before those were removed.)
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

const SEED: u64 = 0x1234_5678_9abc_def0;

/// One request: its parts are sent with a pause between them; `$a` is replaced by the token captured by `=> a`.
struct Req { parts: Vec<Vec<u8>>, capture: bool }
fn req(s: &[u8]) -> Req { Req { parts: vec![s.to_vec()], capture: false } }

fn script() -> Vec<Req> {
    let mut v = vec![
        Req { capture: true, ..req(b"POST /sessions HTTP/1.1\r\nHost: x\r\nContent-Length: 14\r\n\r\n{\"name\":\"ann\"}") },
        req(b"POST /rooms HTTP/1.1\r\nX-Session: $a\r\nContent-Length: 12\r\n\r\n{\"name\":\"a\"}"),
        req(b"POST /rooms HTTP/1.1\r\nX-Session: $a\r\nContent-Length: 12\r\n\r\n{\"name\":\"b\"}"), // room 1 exists in both servers
        req(b"POST /rooms/1/messages HTTP/1.1\r\nX-Session: $a\r\nContent-Length: 15\r\n\r\n{\"body\":\"\xff\xe2\x82x\"}"), // invalid UTF-8, echoed back below
        // the body arrives after the head, in two pieces
        Req { parts: vec![b"POST /rooms/1/messages HTTP/1.1\r\nX-Session: $a\r\nContent-Length: 15\r\n\r\n".to_vec(), b"{\"body\":".to_vec(), b"\"hi\"}".to_vec()], capture: false },
        req(b"GET /rooms/1/messages HTTP/1.1\r\nx-session: $a\r\n\r\n"),
        req(b"GET /rooms/1/messages?limit=1 HTTP/1.1\r\n  X-Session  :   $a  \r\n\r\n"),
        req(b"GET /rooms HTTP/1.1\r\nX-Session: \xc2\xa0$a\xe3\x80\x80\r\n\r\n"),
        req(b"GET /rooms HTTP/1.1\r\nX-Session: $a\r\r\n\r\n"),
        req(b"GET /rooms HTTP/1.1\r\nno colon here\r\nX-Session: $a\r\n\r\n"),
        req(b"POST /sessions HTTP/1.1\r\ncontent-length: 14\r\n\r\n{\"name\":\"bob\"}"),
        req(b"POST /sessions HTTP/1.1\r\nContent-Length: +14\r\n\r\n{\"name\":\"cat\"}"),
        req(b"POST /sessions HTTP/1.1\r\nContent-Length: abc\r\n\r\n{\"name\":\"dan\"}"),
        req(b"POST /sessions HTTP/1.1\r\nContent-Length: 99999999999999999999\r\n\r\n{\"name\":\"eve\"}"),
        req(b"POST /sessions HTTP/1.1\r\nContent-Length: 2\r\n\r\n{\"name\":\"fay\"}"), // more bytes than Content-Length: all are read
        req(b"POST /sessions HTTP/1.1\r\nContent-Length: 50\r\n\r\n{\"name\":\"gus\"}"), // fewer: the client stops sending
        req(b"POST /sessions HTTP/1.1\r\nContent-Length: 15\r\n\r\n{\"name\":\"\xff\xfe\"}"),
        req(b"POST /sessions HTTP/1.1\r\nContent-Length: 17\r\n\r\n{\"name\":\"\xe2\x82\xac\xe2\x82\"}"),
        req(b"GET /rooms HTTP/1.1\r\nX-Session: \xff$a\r\n\r\n"),
        req(b"GET /nope HTTP/1.1\r\n\r\n"),
        req(b"DELETE /rooms HTTP/1.1\r\n\r\n"),
        req(b"GARBAGE\r\n\r\n"),
        req(b"\r\n\r\n"),
        req(b"GET  /rooms HTTP/1.1\r\n\r\n"),
        req(b"GET /rooms\r\nX-Session: $a\r\n\r\n"),
        req(b"GET /rooms HTTP/1.1\r\nX-Session: $a\r\n"), // no end of headers
        req(b""),
        req(b"GET /stats HTTP/1.1\r\n\r\n"),
    ];
    // headers past 64 KB without an end: exactly 65537 bytes, so the server has read all of them when it answers
    let mut big = b"GET /rooms HTTP/1.1\r\nX-Pad: ".to_vec();
    big.resize(65537, b'a');
    v.push(Req { parts: vec![big], capture: false });
    // a body at the 1 MB cap: exactly 1 MB is sent, Content-Length says more
    let mut body = b"{\"name\":\"".to_vec();
    body.resize((1 << 20) - 2, b'z');
    body.extend_from_slice(b"\"}");
    let mut r = b"POST /sessions HTTP/1.1\r\nContent-Length: 2000000\r\n\r\n".to_vec();
    r.extend_from_slice(&body);
    v.push(Req { parts: vec![r], capture: false });
    v
}

fn esc(b: &[u8]) -> String {
    let mut o = String::new();
    let (shown, rest) = if b.len() > 300 { (&b[..300], b.len() - 300) } else { (b, 0) };
    for &c in shown {
        match c { b'\r' => o += "\\r", b'\n' => o += "\\n", b'\\' => o += "\\\\", 0x20..=0x7e => o.push(c as char), _ => o += &format!("\\x{:02x}", c) }
    }
    if rest > 0 { o += &format!("...({} more bytes)", rest); }
    o
}

fn connect(port: u16) -> TcpStream {
    for _ in 0..500 { if let Ok(c) = TcpStream::connect(("127.0.0.1", port)) { return c; } std::thread::sleep(Duration::from_millis(10)); }
    panic!("no server on port {}", port)
}

/// Run the script against the server on `port`; the log has one `>` and one `<` line per request.
fn run(port: u16) -> Vec<String> {
    let mut log = Vec::new();
    let mut tok = String::new();
    for r in script() {
        let mut c = connect(port);
        c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let parts: Vec<Vec<u8>> = r.parts.iter().map(|p| replace(p, b"$a", tok.as_bytes())).collect();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 { std::thread::sleep(Duration::from_millis(50)); }
            c.write_all(p).unwrap();
        }
        let _ = c.shutdown(Shutdown::Write);
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out);
        let sent = esc(&parts.concat());
        log.push(format!("> {}", if tok.is_empty() { sent } else { sent.replace(&tok, "$a") }));
        if r.capture {
            let s = String::from_utf8_lossy(&out);
            tok = s.split("\"token\":\"").nth(1).and_then(|t| t.split('"').next()).unwrap_or("").to_string();
        }
        log.push(format!("< {}", esc(&out)));
    }
    log
}
fn replace(s: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    let mut i = 0;
    while i < s.len() { if s[i..].starts_with(from) { o.extend_from_slice(to); i += from.len(); } else { o.push(s[i]); i += 1; } }
    o
}

fn free_port() -> u16 { std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port() }

/// A built executable, stopped when dropped.
struct Exe(std::process::Child);
impl Drop for Exe { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
fn start_exe(path: &std::path::Path, port: u16) -> Exe {
    use std::io::BufRead;
    let mut c = std::process::Command::new(path).args(["--port", &port.to_string(), "--seed", &SEED.to_string(), "--clock", "1000", "--conns", "4"])
        .stdout(std::process::Stdio::piped()).spawn().unwrap_or_else(|e| panic!("{}: {}", path.display(), e));
    let mut line = String::new();
    std::io::BufReader::new(c.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert!(line.starts_with("listening on"), "{}: {}", path.display(), line);
    Exe(c)
}

fn sharded_spec() -> crate::system::Spec {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/chat-sharded");
    crate::system::parse_spec(&std::fs::read_to_string(dir.join("chat.system")).unwrap(), &dir).unwrap()
}
#[test]
fn http_servers_match_their_record() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/http.record");
    let mut logs: Vec<(String, Vec<String>)> = Vec::new();
    let chat = || {
        let (s, a) = crate::modules::expand(include_str!("../examples/chat/chat.mito"), include_str!("../examples/chat/chat.api")).unwrap();
        let prog = crate::layout::Program::parse(&s).unwrap();
        let mut api = crate::lang::parse(&a).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        crate::cgen::program_exe(&prog, &api, None).unwrap()
    };
    let dir = std::env::temp_dir().join(format!("mito-http-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (target, units) in [("chat", chat()), ("chat-sharded", crate::cgen::system_exe(&sharded_spec()).unwrap())] {
        let mut builds = vec![("TinyCC", true)];
        if crate::build::clang().is_some() { builds.push(("clang", false)); }
        for (name, tcc) in builds {
            let exe = dir.join(format!("{}-{}.exe", target, name));
            crate::build::exe(&units, &exe, tcc, &[]).unwrap_or_else(|e| panic!("{} {}: {}", target, name, e));
            let p = free_port();
            let _server = start_exe(&exe, p);
            logs.push((format!("{}, C ({})", target, name), run(p)));
        }
    }
    let record = |target: &str| -> String {
        let mut o = String::new();
        for (name, log) in &logs { if name.split(", ").next() == Some(target) && name.ends_with("(TinyCC)") { o += &format!("=== {}\n", target); for l in log { o += l; o.push('\n'); } } }
        o
    };
    if std::env::var("MITO_BLESS").is_ok() {
        std::fs::write(&path, record("chat") + &record("chat-sharded")).unwrap();
    }
    let want = std::fs::read_to_string(&path).expect("tests/http.record (MITO_BLESS=1 records it)");
    for (name, log) in &logs {
        let target = name.split(", ").next().unwrap();
        let sect = want.split(&format!("=== {}\n", target)).nth(1).unwrap_or("").split("=== ").next().unwrap();
        let w: Vec<&str> = sect.lines().collect();
        for (i, (got, exp)) in log.iter().zip(&w).enumerate() { assert!(got == exp, "{}: line {} differs\n  want {}\n  got  {}", name, i + 1, exp, got); }
        assert_eq!(log.len(), w.len(), "{}: {} lines, the record has {}", name, log.len(), w.len());
    }
}
