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
    let cc = if tcc { tcc_exe() } else { clang().ok_or("clang not found (install LLVM, set MITO_CLANG, or build with --tcc)")? };
    let mut args: Vec<String> = SQLITE_FLAGS.iter().map(|s| s.to_string()).collect();
    if !tcc { args.push(opt.to_string()); args.push("-w".into()); if !cfg!(windows) { args.push("-fPIC".into()); } }
    else { args.push("-w".into()); if cfg!(windows) { let d = crate::tcc::dir(); args.push(format!("-I{}/include", d)); args.push(format!("-I{}/include/winapi", d)); } }
    let key = { let mut h: u64 = 0xcbf29ce484222325; for b in cc.bytes().chain(args.join(" ").bytes()).chain(SQLITE_C.len().to_string().bytes()) { h = (h ^ b as u64).wrapping_mul(0x100000001b3); } h };
    let dir = std::env::temp_dir().join("mitochondria-cache");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let obj = dir.join(format!("sqlite-3540100-{:016x}.o", key));
    if obj.exists() { return Ok(obj); }
    // sqlite3.c beside a unit that includes it (after the shim, for TinyCC)
    let sub = dir.join(format!("src-{}", std::process::id()));
    std::fs::create_dir_all(&sub).map_err(|e| format!("{}: {}", sub.display(), e))?;
    std::fs::write(sub.join("sqlite3.c"), SQLITE_C).map_err(|e| format!("{}: {}", sub.display(), e))?;
    let _ = std::fs::write(sub.join("sqlite3.h"), SQLITE_H);
    let src = sub.join("unit.c");
    std::fs::write(&src, if tcc { SQLITE_TCC_SHIM } else { "#include \"sqlite3.c\"\n" }).map_err(|e| format!("{}: {}", src.display(), e))?;
    let part = dir.join(format!("sqlite-{:016x}-{}.o.part", key, std::process::id()));
    let r = std::process::Command::new(&cc).args(&args).arg("-c").arg(&src).arg("-o").arg(&part).output().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&sub);
    if !r.status.success() { let _ = std::fs::remove_file(&part); return Err(format!("compiling SQLite failed:\n{}{}", String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr))); }
    if std::fs::rename(&part, &obj).is_err() && !obj.exists() { return Err(format!("{}: could not be kept", obj.display())); }
    let _ = std::fs::remove_file(&part);
    Ok(obj)
}

/// Build C translation units into the executable `out`: with TinyCC (`tcc`), or else clang with `flags` (default -O3, the optimisation level of Rust's release profile, which the benchmarks compare with).
pub fn exe(units: &[String], out: &std::path::Path, tcc: bool, flags: &[&str]) -> Result<(), String> {
    let sqlite = units.iter().any(|u| u.contains("#define MITO_USES_SQLITE")); // a program with tables links SQLite
    let tcc_wanted = tcc;
    let dir = std::env::temp_dir().join(format!("mito-build-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    if sqlite { cmd.arg(sqlite_object(tcc, "-O2")?); } // SQLite, compiled by the same compiler, from its cache
    if !tcc && cfg!(windows) { cmd.arg("-lkernel32"); }
    for l in crate::tcc::LIBS { cmd.arg(format!("-l{}", l)); } // after the sources: system DLLs imported normally
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
            super::exe(&crate::cgen::program_exe(&prog, &api, None).unwrap(), &exe, false, &["-O0"]).unwrap();
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
