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

/// Build C translation units into the executable `out`: with TinyCC (`tcc`), or else clang with `flags` (default -O3, the optimisation level of Rust's release profile, which the benchmarks compare with).
pub fn exe(units: &[String], out: &std::path::Path, tcc: bool, flags: &[&str]) -> Result<(), String> {
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
        // -fno-math-errno: maths never sets errno (nothing reads it), so `sqrt` is the bare instruction, as in Rust; -falign-loops=32: loop heads
        // start on a 32-byte boundary, so a hot loop's speed does not swing with where unrelated code moves it (measured: up to 20% on one port)
        c.args(["-fno-strict-aliasing", "-fno-math-errno", "-falign-loops=32", "-w"]).args(if quick { &["-O0"][..] } else if flags.is_empty() { &["-O3"][..] } else { flags });
        c
    };
    cmd.arg("-o").arg(out).args(&cs);
    if !tcc && cfg!(windows) { cmd.arg("-lkernel32"); }
    for l in crate::tcc::LIBS { cmd.arg(format!("-l{}", l)); } // after the sources: system DLLs imported normally
    let r = cmd.output().map_err(|e| e.to_string())?;
    for c in &cs { let _ = std::fs::remove_file(c); }
    let _ = std::fs::remove_dir(&dir);
    if !r.status.success() { return Err(format!("{} failed:\n{}{}", if tcc { "tcc" } else { "clang" }, String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr))); }
    Ok(())
}
