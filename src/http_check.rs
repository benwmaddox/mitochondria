//! The HTTP front end, end to end: raw requests over real sockets, one connection each, to every server that serves
//! examples/chat (one program) and examples/chat-sharded (a system). Each must answer exactly as `tests/http.record`
//! says, byte for byte. `MITO_BLESS=1` records the TinyCC build's answers. (The record was first made by the Rust
//! servers this front end replaced; the C executables matched it before those were removed.)
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

const SEED: u64 = 0x1234_5678_9abc_def0;

/// One request: its parts are sent with a pause between them; `$a` is replaced by the token captured by `capture`, and
/// `$r` by the id of the room `room` created (`"0.0"` in the chat, `1` in the sharded chat, whose rooms worker numbers
/// rooms itself).
struct Req { parts: Vec<Vec<u8>>, capture: bool, room: bool }
fn req(s: &[u8]) -> Req { Req { parts: vec![s.to_vec()], capture: false, room: false } }

fn script() -> Vec<Req> {
    let mut v = vec![
        Req { capture: true, ..req(b"POST /sessions HTTP/1.1\r\nHost: x\r\nContent-Length: 14\r\n\r\n{\"name\":\"ann\"}") },
        Req { room: true, ..req(b"POST /rooms HTTP/1.1\r\nX-Session: $a\r\nContent-Length: 12\r\n\r\n{\"name\":\"a\"}") },
        req(b"POST /rooms HTTP/1.1\r\nX-Session: $a\r\nContent-Length: 12\r\n\r\n{\"name\":\"b\"}"),
        req(b"POST /rooms/$r/messages HTTP/1.1\r\nX-Session: $a\r\nContent-Length: 15\r\n\r\n{\"body\":\"\xff\xe2\x82x\"}"), // invalid UTF-8, echoed back below
        // the body arrives after the head, in two pieces
        Req { parts: vec![b"POST /rooms/$r/messages HTTP/1.1\r\nX-Session: $a\r\nContent-Length: 15\r\n\r\n".to_vec(), b"{\"body\":".to_vec(), b"\"hi\"}".to_vec()], capture: false, room: false },
        req(b"GET /rooms/$r/messages HTTP/1.1\r\nx-session: $a\r\n\r\n"),
        req(b"GET /rooms/$r/messages?limit=1 HTTP/1.1\r\n  X-Session  :   $a  \r\n\r\n"),
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
    v.push(Req { parts: vec![big], capture: false, room: false });
    // a body at the 1 MB cap: exactly 1 MB is sent, Content-Length says more
    let mut body = b"{\"name\":\"".to_vec();
    body.resize((1 << 20) - 2, b'z');
    body.extend_from_slice(b"\"}");
    let mut r = b"POST /sessions HTTP/1.1\r\nContent-Length: 2000000\r\n\r\n".to_vec();
    r.extend_from_slice(&body);
    v.push(Req { parts: vec![r], capture: false, room: false });
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
    let (mut tok, mut room) = (String::new(), String::new());
    for r in script() {
        let mut c = connect(port);
        c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let parts: Vec<Vec<u8>> = r.parts.iter().map(|p| replace(&replace(p, b"$a", tok.as_bytes()), b"$r", room.as_bytes())).collect();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 { std::thread::sleep(Duration::from_millis(50)); }
            c.write_all(p).unwrap();
        }
        let _ = c.shutdown(Shutdown::Write);
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out);
        let sent = esc(&parts.concat());
        let sent = if room.is_empty() { sent } else { sent.replace(&format!("/rooms/{}/", room), "/rooms/$r/") };
        log.push(format!("> {}", if tok.is_empty() { sent } else { sent.replace(&tok, "$a") }));
        if r.room {
            let s = String::from_utf8_lossy(&out);
            let id = s.split("\"id\":").nth(1).unwrap_or("");
            room = if let Some(q) = id.strip_prefix('"') { q.split('"').next().unwrap_or("").to_string() } else { id.chars().take_while(|c| c.is_ascii_digit()).collect() };
        }
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

fn sharded_spec() -> crate::spec::Spec {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/chat-sharded");
    crate::spec::parse_spec(&std::fs::read_to_string(dir.join("chat.mito")).unwrap(), &dir).unwrap()
}
#[test]
fn http_servers_match_their_record() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/http.record");
    let mut logs: Vec<(String, Vec<String>)> = Vec::new();
    let chat = || {
        let (s, a) = crate::source::load(include_str!("../examples/chat/chat.mito")).unwrap();
        let prog = crate::layout::Program::parse(&s).unwrap();
        let mut api = crate::lang::parse(&a).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        crate::cgen::program_exe(&prog, &api, None, "").unwrap()
    };
    let dir = std::env::temp_dir().join(format!("mito-http-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (target, units) in [("chat", chat()), ("chat-sharded", crate::cgen::system_exe(&sharded_spec()).unwrap())] {
        let mut builds = vec![("TinyCC", true)];
        if crate::build::clang().is_some() { builds.push(("clang", false)); }
        for (name, tcc) in builds {
            let exe = dir.join(format!("{}-{}{}", target, name, std::env::consts::EXE_SUFFIX));
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

/// One request on its own connection: the time to the last byte of the answer, and the answer.
fn timed(port: u16, req: &[u8]) -> (Duration, Vec<u8>) {
    let t = std::time::Instant::now();
    let mut c = connect(port);
    c.write_all(req).unwrap();
    let mut out = Vec::with_capacity(16 * 1024);
    let _ = c.read_to_end(&mut out);
    (t.elapsed(), out)
}

/// `cargo test --release http_load -- --ignored --nocapture`: requests per second and latency of the chat executable
/// (clang -O2, or TinyCC without clang) from client threads, one connection per request, for a small answer (`GET /rooms`)
/// and a large one (the newest 50 of 1000 messages, about 6 KB). `MITO_LOAD_SECS` (default 1) per run, `MITO_LOAD_ROUNDS`
/// (default 3). `MITO_LOAD_OTHER=file.c` also builds with that HTTP server instead of runtime/http.c, and alternates the
/// two in every round, so a busy machine slows both alike.
#[test]
#[ignore]
fn http_load() {
    let (s, a) = crate::source::load(include_str!("../examples/chat/chat.mito")).unwrap();
    let prog = crate::layout::Program::parse(&s).unwrap();
    let api = crate::cgen::check(&prog, &a).unwrap();
    let tcc = crate::build::clang().is_none();
    let mut variants = vec![("this http.c".to_string(), crate::cgen::program_exe(&prog, &api, None, "").unwrap())];
    if let Ok(f) = std::env::var("MITO_LOAD_OTHER") {
        let other = std::fs::read_to_string(&f).unwrap();
        let mut u = crate::cgen::program_exe(&prog, &api, None, "").unwrap();
        u[0] = u[0].replace(crate::cgen::HTTP, &other);
        variants.push((f, u));
    }
    let secs: f64 = std::env::var("MITO_LOAD_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let rounds: usize = std::env::var("MITO_LOAD_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let mut servers = Vec::new();
    for (k, (name, units)) in variants.iter().enumerate() {
        let exe = std::env::temp_dir().join(format!("mito-load-{}-{}{}", std::process::id(), k, std::env::consts::EXE_SUFFIX));
        crate::build::exe(units, &exe, tcc, &[]).unwrap();
        let port = free_port();
        let server = start_exe(&exe, port);
        // 1000 messages in room 0, from 40 sessions (each may send 30 a minute)
        let mut tok = String::new();
        for i in 0..40 {
            let body = format!("{{\"name\":\"u{}\"}}", i);
            let (_, out) = timed(port, format!("POST /sessions HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}", body.len(), body).as_bytes());
            tok = String::from_utf8_lossy(&out).split("\"token\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
            if i == 0 { timed(port, format!("POST /rooms HTTP/1.1\r\nX-Session: {}\r\nContent-Length: 16\r\n\r\n{{\"name\":\"lobby\"}}", tok).as_bytes()); }
            let m = "{\"body\":\"hello there, a message of ordinary length, about one line of chat text\"}";
            for _ in 0..25 { timed(port, format!("POST /rooms/0/messages HTTP/1.1\r\nX-Session: {}\r\nContent-Length: {}\r\n\r\n{}", tok, m.len(), m).as_bytes()); }
        }
        servers.push((name.clone(), port, tok, server, exe));
    }
    println!("{} build; one connection per request; {} s per run, {} rounds", if tcc { "TinyCC" } else { "clang -O2" }, secs, rounds);
    println!("  {:<14} {:<24} {:>7} {:>10} {:>9} {:>9} {:>9} {:>7}", "server", "request", "clients", "req/s", "p50 us", "p99 us", "max us", "bytes");
    for round in 0..rounds {
        println!("  round {}", round + 1);
        for (name, target) in [("GET /rooms (small)", "/rooms"), ("GET 50 messages (6 KB)", "/rooms/0/messages?limit=50")] {
            for clients in [1usize, 8, 32] {
                for (sname, port, tok, _, _) in &servers {
                    let (port, req) = (*port, format!("GET {} HTTP/1.1\r\nX-Session: {}\r\n\r\n", target, tok).into_bytes());
                    let stop = std::time::Instant::now() + Duration::from_secs_f64(secs);
                    let ts: Vec<_> = (0..clients).map(|_| { let req = req.clone(); std::thread::spawn(move || {
                        let (mut lat, mut bad, mut bytes) = (Vec::new(), 0usize, 0usize);
                        while std::time::Instant::now() < stop {
                            let (d, out) = timed(port, &req);
                            if !out.starts_with(b"HTTP/1.1 200") { bad += 1; }
                            bytes = out.len();
                            lat.push(d.as_micros() as u64);
                        }
                        (lat, bad, bytes)
                    }) }).collect();
                    let (mut lat, mut bad, mut bytes) = (Vec::new(), 0, 0);
                    for t in ts { let (l, b, n) = t.join().unwrap(); lat.extend(l); bad += b; bytes = n; }
                    lat.sort_unstable();
                    let q = |p: usize| lat[(lat.len() * p / 100).min(lat.len() - 1)];
                    let short = std::path::Path::new(sname).file_name().map_or(sname.clone(), |f| f.to_string_lossy().into_owned());
                    println!("  {:<14} {:<24} {:>7} {:>10.0} {:>9} {:>9} {:>9} {:>7}{}", short, name, clients, lat.len() as f64 / secs, q(50), q(99), lat[lat.len() - 1], bytes, if bad > 0 { format!("  ({} not 200)", bad) } else { String::new() });
                }
            }
        }
    }
    for (_, _, _, server, exe) in servers { drop(server); let _ = std::fs::remove_file(&exe); }
}

/// `cargo test --release connection_floor -- --ignored --nocapture`: what a connection costs on this machine with no
/// Mitochondria in it: a bare Rust server that accepts, reads the request, writes a fixed 125-byte answer and closes,
/// next to the chat executable answering GET /nope (404, no handler work), one client, alternating rounds.
#[test]
#[ignore]
fn connection_floor() {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let bare = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", 125 - 58, "x".repeat(125 - 58));
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let _ = c.set_nodelay(true);
            let mut b = [0u8; 4096];
            let _ = c.read(&mut b);
            let _ = c.write_all(answer.as_bytes());
        }
    });
    let (s, a) = crate::source::load(include_str!("../examples/chat/chat.mito")).unwrap();
    let prog = crate::layout::Program::parse(&s).unwrap();
    let api = crate::cgen::check(&prog, &a).unwrap();
    let exe = std::env::temp_dir().join(format!("mito-floor-{}{}", std::process::id(), std::env::consts::EXE_SUFFIX));
    crate::build::exe(&crate::cgen::program_exe(&prog, &api, None, "").unwrap(), &exe, crate::build::clang().is_none(), &[]).unwrap();
    let port = free_port();
    let _server = start_exe(&exe, port);
    let req = b"GET /nope HTTP/1.1\r\n\r\n";
    let run = |port: u16, n: usize| { let mut v: Vec<u64> = (0..n).map(|_| timed(port, req).0.as_micros() as u64).collect(); v.sort_unstable(); (v[n / 2], v[n * 99 / 100]) };
    // the parts of one connection, on the bare server
    let parts = |port: u16, n: usize| {
        let (mut c_us, mut rest_us) = (Vec::new(), Vec::new());
        for _ in 0..n {
            let t = std::time::Instant::now();
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            c_us.push(t.elapsed().as_micros() as u64);
            let t = std::time::Instant::now();
            c.write_all(req).unwrap();
            let mut out = Vec::new();
            let _ = c.read_to_end(&mut out);
            rest_us.push(t.elapsed().as_micros() as u64);
        }
        c_us.sort_unstable(); rest_us.sort_unstable();
        (c_us[n / 2], rest_us[n / 2])
    };
    for round in 0..3 {
        let (b50, b99) = run(bare, 2000);
        let (m50, m99) = run(port, 2000);
        let (pc, pr) = parts(bare, 2000);
        println!("round {}: bare Rust server p50 {} us p99 {} us | chat executable p50 {} us p99 {} us | bare: connect {} us, request to close {} us", round + 1, b50, b99, m50, m99, pc, pr);
    }
    let _ = std::fs::remove_file(&exe);
}
