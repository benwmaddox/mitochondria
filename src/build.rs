//! `mitochondria build`: a program or a system as one executable, its HTTP server included (runtime/http.c).
//! clang (optimised) by default; `--tcc` builds with the bundled TinyCC in a few milliseconds, for development.

/// The clang to build with: `MITO_CLANG`, or LLVM's default install. None if neither exists.
pub fn clang() -> Option<String> {
    let p = std::env::var("MITO_CLANG").unwrap_or_else(|_| "C:/Program Files/LLVM/bin/clang.exe".into());
    if std::path::Path::new(&p).exists() { Some(p) } else { None }
}

/// The bundled TinyCC compiler.
pub fn tcc_exe() -> String { format!("{}/tcc.exe", crate::tcc::dir()) }

/// Build C translation units into the executable `out`: with TinyCC (`tcc`), or else clang with `flags` (default -O2).
pub fn exe(units: &[String], out: &std::path::Path, tcc: bool, flags: &[&str]) -> Result<(), String> {
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
    let mut cmd = if tcc {
        let mut c = std::process::Command::new(tcc_exe());
        c.args(["-w"]).args(flags);
        c
    } else {
        let mut c = std::process::Command::new(clang().ok_or("clang not found (install LLVM, set MITO_CLANG, or build with --tcc)")?);
        c.args(["-fno-strict-aliasing", "-w"]).args(if flags.is_empty() { &["-O2"][..] } else { flags });
        c
    };
    cmd.arg("-o").arg(out).args(&cs);
    if !tcc { cmd.arg("-lkernel32"); }
    for l in crate::tcc::LIBS { cmd.arg(format!("-l{}", l)); } // after the sources: system DLLs imported normally
    let r = cmd.output().map_err(|e| e.to_string())?;
    for c in &cs { let _ = std::fs::remove_file(c); }
    let _ = std::fs::remove_dir(&dir);
    if !r.status.success() { return Err(format!("{} failed:\n{}{}", if tcc { "tcc" } else { "clang" }, String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr))); }
    Ok(())
}
