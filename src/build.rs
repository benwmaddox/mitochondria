//! `mito build`: a program or a system as one executable, its HTTP server included (runtime/http.c).
//! clang (optimised) by default; `--tcc` builds with the bundled TinyCC in a few milliseconds, for development.

/// The clang to build with: `MITO_CLANG`, or LLVM's default install on Windows, `clang` on the path elsewhere. None if not found.
pub fn clang() -> Option<String> {
    if let Ok(p) = std::env::var("MITO_CLANG") { return if std::path::Path::new(&p).exists() || on_path(&p) { Some(p) } else { None }; }
    if cfg!(windows) { let p = "C:/Program Files/LLVM/bin/clang.exe"; if std::path::Path::new(p).exists() { Some(p.into()) } else { None } }
    else if on_path("clang") { Some("clang".into()) } else { None }
}
/// Does `name` run (a command found on the path)?
fn on_path(name: &str) -> bool { std::process::Command::new(name).arg("--version").output().map(|o| o.status.success()).unwrap_or(false) }

/// The TinyCC compiler: bundled on Windows; elsewhere `MITO_TCC` or `tcc` on the path, if there is one.
pub fn tcc_exe() -> String {
    if cfg!(windows) { format!("{}/tcc.exe", crate::tcc::dir()) } else { std::env::var("MITO_TCC").unwrap_or_else(|_| "tcc".into()) }
}
/// Is there a TinyCC to build with? (Without one, `--tcc` builds with clang -O0: slower to build, still quick.)
pub fn has_tcc() -> bool { cfg!(windows) || on_path(&tcc_exe()) }

/// SQLite's amalgamation (third_party/sqlite), for programs with a `table<T>`: a unit that defines `MITO_USES_SQLITE` is linked with it.
pub const SQLITE_C: &str = include_str!("../third_party/sqlite/sqlite3.c");
pub const SQLITE_H: &str = include_str!("../third_party/sqlite/sqlite3.h");
/// How SQLite is compiled: connections used one thread each (a system's workers each have their own); its memory from one fixed pool the
/// runtime gives it (MEMSYS5, so peak memory is declared); no extensions loaded from files, no deprecated or shared-cache code; double-quoted
/// strings are not identifiers (one meaning only).
pub const SQLITE_FLAGS: &[&str] = &["-DSQLITE_THREADSAFE=2", "-DSQLITE_ENABLE_MEMSYS5", "-DSQLITE_DEFAULT_MEMSTATUS=0", "-DSQLITE_OMIT_LOAD_EXTENSION",
    "-DSQLITE_OMIT_DEPRECATED", "-DSQLITE_OMIT_SHARED_CACHE", "-DSQLITE_DQS=0", "-DSQLITE_LIKE_DOESNT_MATCH_BLOBS"];

/// TinyCC 0.9.27's Windows headers predate a few things SQLite uses (slim reader/writer locks, GetTickCount64, the UTF-8 conversions), and
/// MSVC's intrinsics are not TinyCC's:
/// declared here, before SQLite, so TinyCC compiles it unmodified. Elsewhere SQLite is compiled as it is.
const SQLITE_TCC_SHIM: &str = "#ifdef _WIN32
#include <windows.h>
#ifndef SRWLOCK_INIT
typedef struct _RTL_SRWLOCK { PVOID Ptr; } RTL_SRWLOCK, SRWLOCK, *PSRWLOCK;
#define SRWLOCK_INIT {0}
WINBASEAPI VOID WINAPI InitializeSRWLock(PSRWLOCK);
WINBASEAPI VOID WINAPI AcquireSRWLockExclusive(PSRWLOCK);
WINBASEAPI VOID WINAPI ReleaseSRWLockExclusive(PSRWLOCK);
WINBASEAPI BOOLEAN WINAPI TryAcquireSRWLockExclusive(PSRWLOCK);
WINBASEAPI ULONGLONG WINAPI GetTickCount64(VOID);
WINBASEAPI int WINAPI MultiByteToWideChar(UINT, DWORD, LPCSTR, int, LPWSTR, int);
WINBASEAPI int WINAPI WideCharToMultiByte(UINT, DWORD, LPCWSTR, int, LPSTR, int, LPCSTR, LPBOOL);
#define CP_ACP 0
#define CP_OEMCP 1
#define CP_UTF8 65001
#endif
/* MSVC's intrinsics are not TinyCC's: SQLite's own options turn them off and give the memory barrier as one instruction */
#define SQLITE_DISABLE_INTRINSIC 1
#define SQLITE_MEMORY_BARRIER __asm__ __volatile__(\"mfence\" ::: \"memory\")
#endif
#include \"sqlite3.c\"
";

/// SQLite compiled to an object file by the compiler building the program (TinyCC or clang), once per compiler and flags, then reused from a cache in the temp directory (the clang build takes
/// tens of seconds; nothing needs it twice). The file name carries a hash of what it was built from.
pub fn sqlite_object(tcc: bool, opt: &str) -> Result<std::path::PathBuf, String> {
    cached_object("SQLite", "sqlite-3540100", &[("sqlite3.c", SQLITE_C), ("sqlite3.h", SQLITE_H)], if tcc { SQLITE_TCC_SHIM } else { "#include \"sqlite3.c\"\n" }, SQLITE_FLAGS, tcc, opt)
}

/// zlib's deflate side (third_party/zlib) and runtime/gzip.c, for programs whose web block says `gzip;`: a unit that defines
/// `MITO_USES_ZLIB` is linked with them.
const ZLIB_FILES: &[(&str, &str)] = &[
    ("adler32.c", include_str!("../third_party/zlib/adler32.c")), ("crc32.c", include_str!("../third_party/zlib/crc32.c")),
    ("crc32.h", include_str!("../third_party/zlib/crc32.h")), ("deflate.c", include_str!("../third_party/zlib/deflate.c")),
    ("deflate.h", include_str!("../third_party/zlib/deflate.h")), ("trees.c", include_str!("../third_party/zlib/trees.c")),
    ("trees.h", include_str!("../third_party/zlib/trees.h")), ("zutil.c", include_str!("../third_party/zlib/zutil.c")),
    ("zutil.h", include_str!("../third_party/zlib/zutil.h")), ("zlib.h", include_str!("../third_party/zlib/zlib.h")),
    ("zconf.h", include_str!("../third_party/zlib/zconf.h")), ("gzguts.h", include_str!("../third_party/zlib/gzguts.h")),
    ("gzip.c", include_str!("../runtime/gzip.c")),
];
/// zlib and the front end's gzip, as one object (cached as SQLite is).
pub fn zlib_object(tcc: bool, opt: &str) -> Result<std::path::PathBuf, String> {
    cached_object("zlib", "zlib-1.4", ZLIB_FILES, "#include \"adler32.c\"\n#include \"crc32.c\"\n#include \"deflate.c\"\n#include \"trees.c\"\n#include \"zutil.c\"\n#include \"gzip.c\"\n", &[], tcc, opt)
}

/// crypt_blowfish (third_party/crypt_blowfish) and runtime/bcrypt.c, for programs that use a crypto built-in.
const BCRYPT_FILES: &[(&str, &str)] = &[
    ("crypt_blowfish.c", include_str!("../third_party/crypt_blowfish/crypt_blowfish.c")), ("crypt_blowfish.h", include_str!("../third_party/crypt_blowfish/crypt_blowfish.h")),
    ("bcrypt.c", include_str!("../runtime/bcrypt.c")),
];
pub fn bcrypt_object(tcc: bool, opt: &str) -> Result<std::path::PathBuf, String> {
    cached_object("crypt_blowfish", "crypt_blowfish-1.3", BCRYPT_FILES, "#include \"crypt_blowfish.c\"\n#include \"bcrypt.c\"\n", &[], tcc, opt)
}

/// What units need linked beside them, by the lines they define: objects (SQLite for tables, zlib for `gzip;`, crypt_blowfish for
/// crypto) built by the same compiler from the cache, and system libraries (libcrypto on Linux for crypto; Windows' BCrypt is always
/// linked).
pub fn extra_links(srcs: &[&str], tcc: bool, opt: &str) -> Result<(Vec<std::path::PathBuf>, Vec<&'static str>), String> {
    let has = |d: &str| srcs.iter().any(|u| u.contains(d));
    let (mut objs, mut libs) = (Vec::new(), Vec::new());
    if has("#define MITO_USES_SQLITE") { objs.push(sqlite_object(tcc, opt)?); }
    if has("#define MITO_USES_ZLIB") { objs.push(zlib_object(tcc, opt)?); }
    if has("#define MITO_USES_CRYPTO") { objs.push(bcrypt_object(tcc, opt)?); if !cfg!(windows) { libs.push("crypto"); } }
    Ok((objs, libs))
}

/// C sources (`files`, written beside `unit`, the file compiled) compiled to one object by the compiler building the program
/// (TinyCC or clang), once per compiler and flags, then kept in a cache in the temp directory; the name carries a hash of what it was
/// built from.
fn cached_object(what: &str, tag: &str, files: &[(&str, &str)], unit: &str, flags: &[&str], tcc: bool, opt: &str) -> Result<std::path::PathBuf, String> {
    let cc = if tcc { tcc_exe() } else { clang().ok_or("clang not found (install LLVM, set MITO_CLANG, or build with --tcc)")? };
    let mut args: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
    if !tcc { args.push(opt.to_string()); args.push("-w".into()); if !cfg!(windows) { args.push("-fPIC".into()); } }
    else { args.push("-w".into()); if cfg!(windows) { let d = crate::tcc::dir(); args.push(format!("-I{}/include", d)); args.push(format!("-I{}/include/winapi", d)); } }
    let key = {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in cc.bytes().chain(args.join(" ").bytes()).chain(unit.bytes()).chain(files.iter().flat_map(|f| f.1.bytes())) { h = (h ^ b as u64).wrapping_mul(0x100000001b3); }
        h
    };
    let dir = std::env::temp_dir().join("mitochondria-cache");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let obj = dir.join(format!("{}-{:016x}.o", tag, key));
    if obj.exists() { return Ok(obj); }
    // a directory of its own per compile (tests in one process may compile the same object at once)
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let sub = dir.join(format!("src-{}-{}-{}", tag, std::process::id(), n));
    std::fs::create_dir_all(&sub).map_err(|e| format!("{}: {}", sub.display(), e))?;
    for (name, text) in files { std::fs::write(sub.join(name), text).map_err(|e| format!("{}: {}", sub.display(), e))?; }
    let src = sub.join("unit.c");
    std::fs::write(&src, unit).map_err(|e| format!("{}: {}", src.display(), e))?;
    let part = dir.join(format!("{}-{:016x}-{}-{}.o.part", tag, key, std::process::id(), n));
    let r = std::process::Command::new(&cc).args(&args).arg("-c").arg(&src).arg("-o").arg(&part).output().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&sub);
    if !r.status.success() { let _ = std::fs::remove_file(&part); return Err(format!("compiling {} failed:\n{}{}", what, String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr))); }
    if std::fs::rename(&part, &obj).is_err() && !obj.exists() { return Err(format!("{}: could not be kept", obj.display())); }
    let _ = std::fs::remove_file(&part);
    Ok(obj)
}

/// Build C translation units into the executable `out`: with TinyCC (`tcc`), or else clang with `flags` (default -O3, the optimisation level of Rust's release profile, which the benchmarks compare with).
pub fn exe(units: &[String], out: &std::path::Path, tcc: bool, flags: &[&str]) -> Result<(), String> {
    let tcc_wanted = tcc;
    // a directory of its own per build: builds in one process (parallel tests) would otherwise remove each other's
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mito-build-{}-{}", std::process::id(), seq));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let mut cs = Vec::new();
    for (k, u) in units.iter().enumerate() {
        let c = dir.join(format!("{}-{}.c", seq, k));
        std::fs::write(&c, u).map_err(|e| format!("{}: {}", c.display(), e))?;
        cs.push(c);
    }
    let tcc = tcc && has_tcc();
    let quick = flags.is_empty() && !tcc && tcc_wanted; // `--tcc` without a TinyCC: clang without optimising
    let mut cmd = if tcc {
        let mut c = std::process::Command::new(tcc_exe());
        c.args(["-w"]).args(flags);
        c
    } else {
        let mut c = std::process::Command::new(clang().ok_or("clang not found (install LLVM, set MITO_CLANG, or build with --tcc)")?);
        // -fno-math-errno: maths never sets errno (nothing reads it), so `sqrt` is the bare instruction, as in Rust; -falign-loops=64: loop heads
        // start on a 64-byte boundary, so a hot loop's speed does not swing with where unrelated code moves it (measured: up to 20% on one port)
        c.args(["-fno-strict-aliasing", "-fno-math-errno", "-falign-loops=64", "-w"]).args(if quick { &["-O0"][..] } else if flags.is_empty() { &["-O3"][..] } else { flags });
        c
    };
    cmd.arg("-o").arg(out).args(&cs);
    // SQLite, zlib, crypt_blowfish as the program needs them, compiled by the same compiler, from the cache
    let refs: Vec<&str> = units.iter().map(|u| u.as_str()).collect();
    let (objs, libs) = extra_links(&refs, tcc, "-O2")?;
    cmd.args(&objs);
    if !tcc && cfg!(windows) { cmd.arg("-lkernel32"); }
    for l in crate::tcc::LIBS.iter().chain(libs.iter()) { cmd.arg(format!("-l{}", l)); } // after the sources: system libraries imported normally
    let r = cmd.output().map_err(|e| e.to_string())?;
    for c in &cs { let _ = std::fs::remove_file(c); }
    let _ = std::fs::remove_dir(&dir);
    if !r.status.success() { return Err(format!("{} failed:\n{}{}", if tcc { "tcc" } else { "clang" }, String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr))); }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The bundled SQLite builds with both compilers and works from its fixed memory pool: a file database in WAL mode, a row in and out.
    #[test]
    fn bundled_sqlite_builds_and_runs() {
        let prog = format!("#define MITO_USES_SQLITE\n{}\n{}", super::SQLITE_H, r#"
#include <stdio.h>
static unsigned char pool[8 << 20];
int main(int argc, char **argv) {
    sqlite3 *db; sqlite3_stmt *st; int n = -1;
    if (sqlite3_config(SQLITE_CONFIG_HEAP, pool, (int)sizeof pool, 64) != SQLITE_OK) return 2;
    if (sqlite3_open(argv[1], &db) != SQLITE_OK) return 3;
    if (sqlite3_exec(db, "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT); INSERT INTO t(v) VALUES ('hello');", 0, 0, 0) != SQLITE_OK) return 4;
    if (sqlite3_prepare_v2(db, "SELECT count(*) FROM t WHERE v = ?1", -1, &st, 0) != SQLITE_OK) return 5;
    sqlite3_bind_text(st, 1, "hello", 5, SQLITE_STATIC);
    if (sqlite3_step(st) == SQLITE_ROW) n = sqlite3_column_int(st, 0);
    sqlite3_finalize(st); sqlite3_close(db);
    printf("rows %d version %s\n", n, sqlite3_libversion());
    return n == 1 ? 0 : 6;
}
"#);
        let dir = std::env::temp_dir().join(format!("mito-sqlite-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for tcc in [true, false] {
            if !tcc && super::clang().is_none() { continue; }
            if tcc && !super::has_tcc() { continue; }
            let exe = dir.join(format!("sq-{}{}", tcc, std::env::consts::EXE_SUFFIX));
            super::exe(&[prog.clone()], &exe, tcc, &[]).unwrap_or_else(|e| panic!("tcc {}: {}", tcc, e));
            let db = dir.join(format!("t-{}.db", tcc));
            let _ = std::fs::remove_file(&db);
            let out = std::process::Command::new(&exe).arg(&db).output().unwrap();
            assert!(out.status.success(), "tcc {}: exit {:?} {}", tcc, out.status.code(), String::from_utf8_lossy(&out.stdout));
            assert!(String::from_utf8_lossy(&out.stdout).contains("rows 1 version 3.54.1"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A program's `web` block through the real server: its header lines (an endpoint's own replacing one), ETags and 304s, gzip
    /// when accepted, static files (binary ones byte for byte, dated by the server's clock) and HEAD.
    #[test]
    fn a_web_block_shapes_answers() {
        use std::io::{Read, Write};
        let dir = std::env::temp_dir().join(format!("mito-web-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pub/assets")).unwrap();
        let bin: Vec<u8> = (0..=255u8).collect();
        std::fs::write(dir.join("pub/assets/b.png"), &bin).unwrap();
        std::fs::write(dir.join("pub/robots.txt"), "User-agent: *\n").unwrap();
        let src = "struct A { n: i64; }\nstate a: A;\nweb {\n    headers { \"X-Frame-Options\": \"SAMEORIGIN\"; \"Vary\": \"Accept\"; \"Cache-Control\": \"no-cache\"; }\n    writes { \"Vary\": \"Accept-Encoding\"; }\n    etag \"max-age=0, private, must-revalidate\";\n    gzip;\n    static \"/\" from \"pub\" { \"Cache-Control\": \"public, max-age=60\"; }\n}\n@get(\"/p\") fn p() -> Response<html> { return ok(<p>{\"hello \" + str(a.n)}</p>); }\n@post(\"/p\") fn post_p() -> Response<html> { return ok(<p>posted</p>); }\n@get(\"/stored/{name}\") fn stored(name: utf8<64>) -> Response<bytes> { let b = read_stored(name); if !exists(b) { fail 404 \"no such file\"; } set_header(\"X-Crc\", str(crc32(b))); return ok(b); }\n@get(\"/deny\") fn deny() -> Response<html> { set_header(\"X-Frame-Options\", \"DENY\"); return ok(<p>no frames</p>); }\n";
        let (st, code) = crate::source::load(src).unwrap();
        let prog = crate::layout::Program::parse(&st).unwrap();
        let api = crate::cgen::check(&prog, &code).unwrap();
        let (w, line) = crate::web::find(src).unwrap();
        let web = crate::web::c(&crate::web::parse(&w, line, &dir).unwrap());
        let exe = dir.join(format!("web{}", std::env::consts::EXE_SUFFIX));
        if super::clang().is_none() { return; }
        super::exe(&crate::cgen::program_exe(&prog, &api, None, &web).unwrap(), &exe, false, &["-O0"]).unwrap();
        let port = 18000 + (std::process::id() % 1000) as u16;
        let mut srv = std::process::Command::new(&exe).args(["--port", &port.to_string(), "--clock", "1772467200000", "--files"]).arg(dir.join("pub")).stdout(std::process::Stdio::piped()).spawn().unwrap();
        { use std::io::BufRead; let mut l = String::new(); std::io::BufReader::new(srv.stdout.take().unwrap()).read_line(&mut l).unwrap(); }
        // one request on its own connection: (status line and headers, body)
        let ask = |req: &str| -> (String, Vec<u8>) {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(format!("{}\r\nHost: x\r\nConnection: close\r\n\r\n", req).as_bytes()).unwrap();
            let mut all = Vec::new(); s.read_to_end(&mut all).unwrap();
            let he = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            (String::from_utf8_lossy(&all[..he]).into_owned(), all[he + 4..].to_vec())
        };
        let lines = |h: &str, name: &str| -> Vec<String> { h.lines().filter(|l| l.to_ascii_lowercase().starts_with(&format!("{}:", name))).map(|l| l[name.len() + 1..].trim().to_string()).collect() };
        let crc32 = |b: &[u8]| -> u32 { let mut c = !0u32; for &x in b { c ^= x as u32; for _ in 0..8 { c = if c & 1 != 0 { (c >> 1) ^ 0xEDB88320 } else { c >> 1 }; } } !c };
        let (h, body) = ask("GET /p HTTP/1.1");
        assert!(h.starts_with("HTTP/1.1 200") && body == b"<p>hello 0</p>", "{}", h);
        assert_eq!(lines(&h, "x-frame-options"), vec!["SAMEORIGIN"]);
        assert_eq!(lines(&h, "vary"), vec!["Accept,Accept-Encoding"]);
        assert_eq!(lines(&h, "cache-control"), vec!["max-age=0, private, must-revalidate"]); // the ETag's, in place of the block's
        assert!(lines(&h, "content-security-policy").is_empty() && lines(&h, "content-encoding").is_empty());
        let tag = lines(&h, "etag")[0].clone();
        assert!(tag.starts_with("W/\"") && tag.len() == 36, "{}", tag);
        // a write's answer: the block's `writes` lines added as they are
        let (h, _) = ask("POST /p HTTP/1.1\r\nContent-Length: 0");
        assert_eq!(lines(&h, "vary"), vec!["Accept,Accept-Encoding", "Accept-Encoding"], "{}", h);
        // an endpoint's own line replaces the block's
        let (h, _) = ask("GET /deny HTTP/1.1");
        assert_eq!(lines(&h, "x-frame-options"), vec!["DENY"]);
        // the browser has it: 304, no body, no content type, Vary without Accept-Encoding
        let (h, body) = ask(&format!("GET /p HTTP/1.1\r\nIf-None-Match: {}", tag));
        assert!(h.starts_with("HTTP/1.1 304") && body.is_empty() && lines(&h, "content-type").is_empty() && lines(&h, "vary") == vec!["Accept"], "{}", h);
        // gzip when accepted (not with q=0): a gzip stream whose trailer gives the original's CRC-32 and length
        let (h, body) = ask("GET /p HTTP/1.1\r\nAccept-Encoding: deflate, gzip;q=0.5");
        assert_eq!(lines(&h, "content-encoding"), vec!["gzip"]);
        let n = body.len();
        assert!(body[0] == 0x1f && body[1] == 0x8b);
        assert_eq!(u32::from_le_bytes(body[n - 4..].try_into().unwrap()), 14);
        assert_eq!(u32::from_le_bytes(body[n - 8..n - 4].try_into().unwrap()), crc32(b"<p>hello 0</p>"));
        let (h, _) = ask("GET /p HTTP/1.1\r\nAccept-Encoding: gzip;q=0");
        assert!(lines(&h, "content-encoding").is_empty());
        // a static file: before the endpoints, whatever the query, binary bytes as they are, dated by the server's clock
        let (h, body) = ask("GET /assets/b.png?v=3 HTTP/1.1");
        assert!(h.starts_with("HTTP/1.1 200") && body == bin, "{}", h);
        assert_eq!(lines(&h, "content-type"), vec!["image/png"]);
        assert_eq!(lines(&h, "cache-control"), vec!["public, max-age=60"]);
        assert_eq!(lines(&h, "last-modified"), vec!["Mon, 02 Mar 2026 16:00:00 GMT"]);
        assert_eq!(lines(&h, "vary"), vec!["Accept-Encoding"]);
        assert!(lines(&h, "x-frame-options").is_empty());
        let (h, body) = ask("GET /robots.txt HTTP/1.1\r\nAccept-Encoding: gzip");
        let n = body.len();
        assert!(lines(&h, "content-encoding") == vec!["gzip"] && u32::from_le_bytes(body[n - 8..n - 4].try_into().unwrap()) == crc32(b"User-agent: *\n"));
        // HEAD: the same head, no body
        let (h, body) = ask("HEAD /robots.txt HTTP/1.1");
        assert!(h.starts_with("HTTP/1.1 200") && body.is_empty() && lines(&h, "content-length") == vec!["14"], "{}", h);
        // read_stored: a file under --files, its bytes as they are (crc32 of them in a header); a missing one null; `..` refused
        let (h, body) = ask("GET /stored/robots.txt HTTP/1.1");
        assert!(body == b"User-agent: *\n" && lines(&h, "x-crc") == vec![crc32(b"User-agent: *\n").to_string()], "{}", h);
        assert!(ask("GET /stored/none.txt HTTP/1.1").0.starts_with("HTTP/1.1 404"));
        assert!(ask("GET /stored/..%2Fsecret HTTP/1.1").0.starts_with("HTTP/1.1 500"));
        let (h, _) = ask("GET /missing.txt HTTP/1.1");
        assert!(h.starts_with("HTTP/1.1 404"), "{}", h);
        let _ = srv.kill(); let _ = srv.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Another program's database (a Rails schema: `datetime(6)`, `varchar`, nullable `text`, `<name>_id`, unique indexes), through the real
    /// server: read as its declared types (nullable text, times with and without microseconds, a reference), a time written back as Rails
    /// writes one, and a database that does not fit refused at start, saying why; nothing in it created or changed in shape.
    #[test]
    fn extern_tables_use_another_programs_database() {
        use std::io::{BufRead, Read, Write};
        if super::clang().is_none() { return; }
        let dir = std::env::temp_dir().join(format!("mito-extern-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("production.sqlite3");
        // the other program's database, and a way to read it back
        let tool = format!("#define MITO_USES_SQLITE\n{}\n{}", super::SQLITE_H, r#"
#include <stdio.h>
static unsigned char pool[4 << 20];
static int row(void *u, int n, char **v, char **c) { for (int i = 0; i < n; i++) printf("%s%s", i ? "|" : "", v[i] ? v[i] : "NULL"); printf("\n"); return 0; }
int main(int argc, char **argv) { sqlite3 *db; sqlite3_config(SQLITE_CONFIG_HEAP, pool, (int)sizeof pool, 64);
    if (sqlite3_open(argv[1], &db) != SQLITE_OK) return 1;
    return sqlite3_exec(db, argv[2], row, 0, 0) == SQLITE_OK ? 0 : 2; }
"#);
        let sq = dir.join(format!("sq{}", std::env::consts::EXE_SUFFIX));
        super::exe(&[tool], &sq, false, &["-O0"]).unwrap();
        let sql = |q: &str| -> String { let o = std::process::Command::new(&sq).arg(&db).arg(q).output().unwrap(); assert!(o.status.success(), "{}", q); String::from_utf8_lossy(&o.stdout).trim().to_string() };
        sql("PRAGMA journal_mode=WAL; CREATE TABLE accounts (id integer PRIMARY KEY AUTOINCREMENT NOT NULL, created_at datetime(6) NOT NULL, custom_styles text, join_code varchar NOT NULL, name varchar NOT NULL, updated_at datetime(6) NOT NULL, singleton_guard integer DEFAULT 0 NOT NULL);
            CREATE UNIQUE INDEX index_accounts_on_singleton_guard ON accounts (singleton_guard);
            CREATE TABLE users (id integer PRIMARY KEY AUTOINCREMENT NOT NULL, name varchar NOT NULL, email_address varchar, role integer DEFAULT 0 NOT NULL, created_at datetime(6) NOT NULL, updated_at datetime(6) NOT NULL);
            CREATE UNIQUE INDEX index_users_on_email_address ON users (email_address);
            CREATE TABLE sessions (id integer PRIMARY KEY AUTOINCREMENT NOT NULL, user_id integer NOT NULL REFERENCES users(id), token varchar NOT NULL, last_active_at datetime(6) NOT NULL, created_at datetime(6) NOT NULL, updated_at datetime(6) NOT NULL);
            CREATE UNIQUE INDEX index_sessions_on_token ON sessions (token); CREATE INDEX index_sessions_on_user_id ON sessions (user_id);
            INSERT INTO accounts VALUES (873240054, '2026-01-01 16:00:00', NULL, 'Ab12', '37signals', '2026-01-01 16:00:00', 0);
            INSERT INTO users VALUES (127326141, 'David', 'david@example.com', 1, '2026-01-02 16:00:00', '2026-01-02 16:00:00');
            INSERT INTO sessions VALUES (481019661, 127326141, 'tok3n', '2026-03-02 14:00:00.123456', '2026-01-07 16:00:00', '2026-01-07 16:00:00');");
        let program = |extra: &str| format!("struct Account {{ name: utf8<255>; custom_styles: utf8<65536>?; join_code: utf8<64>; updated_at: time; }}
struct User {{ name: utf8<255>; email_address: utf8<255>?; role: i64; {} }}
struct Session {{ user: User?; token: utf8<64>; last_active_at: time; created_at: time; updated_at: time; }}
struct Db {{ accounts: table<Account> extern; users: table<User> unique(email_address) extern; sessions: table<Session> unique(token) index(user) extern; }}
state db: Db;
@get(\"/account\") fn account() -> Response<html> {{
    foreach a in db.accounts limit 1 {{ return ok(<p>{{a.name}} {{format_time(a.updated_at, \"%Y%m%d%H%M%S\")}} {{if exists(a.custom_styles) {{ \"styled\"; }} else {{ \"plain\"; }}}}</p>); }}
    return ok(<p>no account</p>);
}}
@get(\"/sessions/{{token}}\") fn session(token: ascii<64>) -> Response<html> {{
    let s = first s in db.sessions where s.token == token;
    if !exists(s) {{ return ok(<p>signed out</p>); }}
    let u = s.user;
    if !exists(u) {{ return ok(<p>no user</p>); }}
    return ok(<p>{{u.name}} {{format_time(s.last_active_at, \"%Y-%m-%d %H:%M:%S.%N\")}}</p>);
}}
@post(\"/users/{{id}}/sessions/{{token}}\") fn start(id: i64, token: ascii<64>) -> Response<html> {{
    let u = app_user(id);
    let s = db.sessions.add {{ user: u, token: token, last_active_at: 1772467200000000, created_at: 1772467200000000, updated_at: 1772467200000000 }};
    return ok(<p>{{s.token}}</p>);
}}
fn app_user(id: i64) -> User {{ return db.users[id]; }}
@post(\"/sessions/{{token}}/touch\") fn touch(token: ascii<64>) -> Response<html> {{
    let s = first s in db.sessions where s.token == token;
    if exists(s) {{ s.last_active_at = 1772467200000005; }}
    return ok(<p>ok</p>);
}}
", extra);
        let build = |n: &str, src: &str| -> std::path::PathBuf {
            let (st, code) = crate::source::load(src).unwrap();
            let prog = crate::layout::Program::parse(&st).unwrap_or_else(|e| panic!("{}: {}", n, e));
            let api = crate::cgen::check(&prog, &code).unwrap_or_else(|e| panic!("{}: {}", n, e));
            let exe = dir.join(format!("{}{}", n, std::env::consts::EXE_SUFFIX));
            super::exe(&crate::cgen::program_exe(&prog, &api, None, "").unwrap(), &exe, false, &["-O0"]).unwrap();
            exe
        };
        let port = 17000 + (std::process::id() % 1000) as u16;
        let start = |exe: &std::path::Path| -> (std::process::Child, String) {
            let mut c = std::process::Command::new(exe).args(["--port", &port.to_string(), "--db"]).arg(&db).stdout(std::process::Stdio::piped()).spawn().unwrap();
            let mut l = String::new(); std::io::BufReader::new(c.stdout.take().unwrap()).read_line(&mut l).unwrap();
            (c, l.trim().to_string())
        };
        let ask = |req: &str| -> String {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(format!("{}\r\nHost: x\r\nConnection: close\r\nContent-Length: 0\r\n\r\n", req).as_bytes()).unwrap();
            let mut all = String::new(); s.read_to_string(&mut all).unwrap();
            all.split("\r\n\r\n").nth(1).unwrap_or("").to_string()
        };
        let good = build("good", &program(""));
        let (mut srv, line) = start(&good);
        assert!(line.starts_with("tables in"), "{}", line);
        assert_eq!(ask("GET /account HTTP/1.1"), "<p>37signals 20260101160000 plain</p>");
        assert_eq!(ask("GET /sessions/tok3n HTTP/1.1"), "<p>David 2026-03-02 14:00:00.123456</p>");
        assert_eq!(ask("GET /sessions/nope HTTP/1.1"), "<p>signed out</p>");
        assert_eq!(ask("POST /sessions/tok3n/touch HTTP/1.1"), "<p>ok</p>");
        // a row added with every NOT NULL column in one INSERT (Rails' sessions have no defaults)
        assert_eq!(ask("POST /users/127326141/sessions/n3w HTTP/1.1"), "<p>n3w</p>");
        let _ = srv.kill(); let _ = srv.wait();
        assert_eq!(sql("SELECT last_active_at FROM sessions WHERE token = 'tok3n'"), "2026-03-02 16:00:00.000005"); // as Rails writes it
        assert_eq!(sql("SELECT user_id, token, created_at FROM sessions WHERE token = 'n3w'"), "127326141|n3w|2026-03-02 16:00:00");
        sql("UPDATE accounts SET custom_styles = 'body {}'");
        let (mut srv, _) = start(&good);
        assert_eq!(ask("GET /account HTTP/1.1"), "<p>37signals 20260101160000 styled</p>");
        let _ = srv.kill(); let _ = srv.wait();
        // nothing created or changed in shape: the same three tables
        assert_eq!(sql("SELECT group_concat(name) FROM (SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name)"), "accounts,sessions,users");
        // what does not fit is refused at start, naming it
        let fails = "error: the database does not fit the program: ";
        assert_eq!(start(&build("missing", &program("bio: utf8<100>?;"))).1, format!("{}\"users\" has no column bio", fails));
        assert_eq!(start(&build("notnull", &program("").replace("email_address: utf8<255>?;", "email_address: utf8<255>;"))).1, format!("{}column \"users\".email_address may be NULL in the database: declare the field nullable (`?`)", fails));
        assert_eq!(start(&build("typed", &program("").replace("role: i64;", "role: utf8<10>;"))).1, format!("{}column \"users\".role is INTEGER in the database, TEXT in the program", fails));
        assert_eq!(start(&build("noindex", &program("").replace("table<User> unique(email_address)", "table<User> unique(name)"))).1, format!("{}\"users\" has no unique index on (name), which the program declares", fails));
        let other = dir.join("missing.sqlite3");
        let mut c = std::process::Command::new(&good).args(["--port", &port.to_string(), "--db"]).arg(&other).stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut l = String::new(); std::io::BufReader::new(c.stdout.take().unwrap()).read_line(&mut l).unwrap(); let _ = c.wait();
        assert!(l.starts_with("error:") && !other.exists(), "{}", l); // another program's database is never created
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A program's tables across builds, through the real server: created, then migrated by name (a column added, one dropped); a
    /// column whose type changed, or a row written from outside past a declared limit, stops the server at start, saying why.
    #[test]
    fn tables_migrate_between_builds() {
        let dir = std::env::temp_dir().join(format!("mito-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let data = dir.join("data");
        let build = |n: &str, todo: &str| -> std::path::PathBuf {
            let src = format!("struct Todo {{ {} }}\nstruct Board {{ secret: i64; todos: table<Todo>; }}\nstruct App {{ boards: table<Board>; }}\nstate app: App;\n@get(\"/\") fn home() -> Response<{{}}> {{ return ok({{}}); }}\n", todo);
            let (st, api) = crate::source::load(&src).unwrap();
            let prog = crate::layout::Program::parse(&st).unwrap();
            let mut api = crate::lang::parse(&api).unwrap();
            crate::lang::bind_with(&mut api, &|g| prog.global(g), &prog.enum_table()).unwrap();
            let exe = dir.join(format!("{}{}", n, std::env::consts::EXE_SUFFIX));
            // clang: Windows Defender's machine-learning heuristics now and then take a small unsigned TinyCC executable for malware
            // (Sabsik/Wacatac "!ml"), which would make this test about Defender instead of about migration
            super::exe(&crate::cgen::program_exe(&prog, &api, None, "").unwrap(), &exe, false, &["-O0"]).unwrap();
            exe
        };
        // a server that opened its tables keeps running: read its first line, then stop it; one that refused exits with the reason
        // its first line on standard output, and (`extra` flags given) what it said it dropped on standard error
        let run_with = |exe: &std::path::Path, extra: &[&str]| -> (String, String) {
            use std::io::{BufRead, Read};
            let mut c = std::process::Command::new(exe).args(["--port", "18131", "--data"]).arg(&data).args(extra)
                .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(c.stdout.take().unwrap()).read_line(&mut line).unwrap();
            let _ = c.kill(); let _ = c.wait();
            let mut err = String::new(); let _ = c.stderr.take().unwrap().read_to_string(&mut err);
            (line.trim().to_string(), err.trim().to_string())
        };
        let run = |exe: &std::path::Path| run_with(exe, &[]).0;
        assert!(run(&build("v1", "title: utf8<1000>; done: bool;")).starts_with("tables in"));
        assert!(run(&build("v2", "title: utf8<1000>; done: bool; due: i64;")).starts_with("tables in"));
        // a row from outside: a title of 10 bytes
        let insert = format!("#define MITO_USES_SQLITE\n{}\n{}", super::SQLITE_H, r#"
static unsigned char pool[4 << 20];
int main(int argc, char **argv) { sqlite3 *db; sqlite3_config(SQLITE_CONFIG_HEAP, pool, (int)sizeof pool, 64);
    if (sqlite3_open(argv[1], &db) != SQLITE_OK) return 1;
    return sqlite3_exec(db, "INSERT INTO boards(secret) VALUES (1); INSERT INTO \"boards.todos\"(parent_id, title) VALUES (1, 'ten bytes!');", 0, 0, 0) == SQLITE_OK ? 0 : 2; }
"#);
        let ins = dir.join(format!("insert{}", std::env::consts::EXE_SUFFIX));
        super::exe(&[insert], &ins, false, &["-O0"]).unwrap();
        assert!(std::process::Command::new(&ins).arg(data.join("data.db")).status().unwrap().success());
        let refused = "error: the database does not fit the program (--db-fit drop drops what does not fit): ";
        assert_eq!(run(&build("v3", "title: utf8<1000>; done: f64; due: i64;")), format!("{}column \"boards.todos\".done is INTEGER in the database, REAL in the program", refused));
        assert_eq!(run(&build("v4", "title: utf8<5>; done: bool; due: i64;")), format!("{}1 row of \"boards.todos\" holds a title longer than 5 bytes", refused));
        assert!(run(&build("v5", "title: utf8<10>; done: bool;")).starts_with("tables in")); // fits again; `due` dropped
        // dropping deliberately: the type change, then the row past a smaller limit, each said on standard error
        let (out, err) = run_with(&build("v6", "title: utf8<10>; done: f64;"), &["--db-fit", "drop"]);
        assert!(out.starts_with("tables in"), "{}", out);
        assert_eq!(err, "dropped: column \"boards.todos\".done is INTEGER in the database, REAL in the program");
        let (out, err) = run_with(&build("v7", "title: utf8<5>; done: f64;"), &["--db-fit", "drop"]);
        assert!(out.starts_with("tables in"), "{}", out);
        assert_eq!(err, "dropped: 1 row of \"boards.todos\" past the limit of title");
        assert!(run(&build("v8", "title: utf8<5>; done: f64;")).starts_with("tables in")); // nothing left that does not fit
        let _ = std::fs::remove_dir_all(&dir);
    }
}
