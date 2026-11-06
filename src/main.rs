mod build;
#[cfg(test)]
mod cases;
#[cfg(test)]
mod http_check;
#[cfg(test)]
mod error_corpus;
#[cfg(test)]
mod test_runner;
mod lang;
mod modules;
mod layout;
mod cgen;
mod source;
mod spec;
mod web;
// the in-memory compiler drives the tests; uild runs tcc.exe from the same directory
#[cfg_attr(not(test), allow(dead_code))]
mod tcc;

const USAGE: &str = "usage: mito <command> <program> [options]

A program is one `.mito` file: a program, or a system of workers (a file holding `system { ... }`).

  check  <program> [--kinds]        report the problems found, or say it loads; --kinds lists every local's kind
  build  <program> [-o app.exe] [--tcc]
                                    an executable serving HTTP (clang -O3; --tcc builds in milliseconds, unoptimised)
  serve  <program> [--clang] [--watch] [executable options]
                                    build with TinyCC (or clang) and run it at once; --watch rebuilds and restarts
                                    when the program changes (state starts empty each time)
  test   <program> [--clang]        run the program's tests (`test \"name\" { ... }`), each on a fresh state
  emit-c <program> [-o file.c]       (a system: -o dir, one file per worker kind and system.c)
                                    the generated C, to read or build yourself

Executable options: --port N (8080); --budget MB, --soft MB, --hard MB (one program's memory; soft and hard turn on
memory pressure); --conns N (a system's connection threads, 16); --seed N and --clock MS (fixed, for tests).";

fn die(e: String) -> ! { eprintln!("error: {}", e); std::process::exit(1) }

/// A program's error as `file:line:col: message`, with the line quoted and a caret under the column, when it names a
/// place (`line:col: ...`) inside the file; otherwise `file: message`. Several errors (one per line) are reported one by one.
fn report(file: &str, text: &str, e: &str) -> String {
    // several independent errors, one per line: each reported on its own
    if e.contains('\n') { return e.lines().map(|x| report(file, text, x)).collect::<Vec<_>>().join("\nerror: "); }
    let mut parts = e.splitn(3, ':');
    let (Some(l), Some(c), Some(rest)) = (parts.next(), parts.next(), parts.next()) else { return format!("{}: {}", file, e) };
    let (Ok(l), Ok(c)) = (l.parse::<usize>(), c.parse::<usize>()) else { return format!("{}: {}", file, e) };
    let rest = rest.trim_start();
    match text.lines().nth(l.wrapping_sub(1)) {
        Some(line) => {
            let shown = line.replace('\t', " ");
            let caret: String = shown.chars().take(c.saturating_sub(1)).map(|_| ' ').collect();
            format!("{}:{}:{}: {}\n{:>5} | {}\n      | {}^", file, l, c, rest, l, shown, caret)
        }
        None => format!("{}: {} (in library module code)", file, rest),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    // `file("...")` in a program reads beside it
    if let Some(f) = inputs(rest).first() { lang::set_file_dir(std::path::Path::new(f.as_str()).parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."))); }
    match args.first().map(|s| s.as_str()) {
        Some("check") if rest.iter().any(|a| a == "--kinds") => kinds(rest),
        Some("check") => { let (_, name) = program(&inputs(rest)); println!("{}: loads", name); }
        Some("build") => build_exe(rest),
        Some("serve") => serve(rest),
        Some("test") => test(rest),
        Some("emit-c") => emit_c(rest),
        Some("sigs") => sigs(rest),
        _ => { eprintln!("{}", USAGE); std::process::exit(2) }
    }
}

/// The program files among `args`: everything before the first option.
fn inputs(args: &[String]) -> Vec<&String> { args.iter().take_while(|a| !a.starts_with('-')).collect() }

/// Read a program file: (its text, whether it is a system of workers).
fn read_program(files: &[&String]) -> (String, String, bool) {
    let [f] = files else { eprintln!("{}", USAGE); std::process::exit(2) };
    let text = std::fs::read_to_string(f.as_str()).unwrap_or_else(|e| die(format!("{}: {}", f, e)));
    let system = source::is_system(&text);
    (f.to_string(), text, system)
}

/// A program's C translation units for an executable, and a name for it.
fn program(files: &[&String]) -> (Vec<String>, String) {
    let (f, text, system) = read_program(files);
    compile(&f, &text, system).unwrap_or_else(|e| die(e))
}

/// The same from a program's text; the error is ready to print.
fn compile(f: &str, text: &str, system: bool) -> Result<(Vec<String>, String), String> {
    let path = std::path::Path::new(f);
    let name = path.file_stem().map_or("app".into(), |n| n.to_string_lossy().into_owned());
    if system {
        let spec = spec::parse_spec(text, path.parent().unwrap_or(std::path::Path::new("."))).map_err(|e| format!("{}: {}", f, e))?;
        return Ok((cgen::system_exe(&spec)?, name));
    }
    let file = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    // the `web` block (src/web.rs) first: its manifest names the assets `asset("...")` in the code resolves
    let web = match web::find(text) {
        Some((w, line)) => Some(web::parse(&w, line, path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."))).map_err(|e| report(f, text, &e))?),
        None => None,
    };
    lang::set_assets(web.as_ref().map_or(Vec::new(), |w| w.assets.clone()));
    let (s, code) = source::load(text).map_err(|e| report(f, text, &e))?;
    let prog = layout::Program::parse(&s).map_err(|e| report(f, text, &e))?;
    let a = cgen::check(&prog, &code).map_err(|e| report(f, text, &e))?;
    let web = web.map_or(String::new(), |w| web::c(&w));
    Ok((cgen::program_exe(&prog, &a, Some((&file, &code)), &web)?, name))
}

/// `build <program> [-o app.exe] [--tcc]`
fn build_exe(args: &[String]) {
    let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).cloned();
    let (units, name) = program(&inputs(args));
    let out = std::path::PathBuf::from(out.unwrap_or_else(|| format!("{}{}", name, std::env::consts::EXE_SUFFIX)));
    // `--cflag X` (repeatable) hands X to clang in place of the default `-O3`, e.g. `--cflag -O2 --cflag -mavx2`
    let cflags: Vec<String> = args.windows(2).filter(|w| w[0] == "--cflag").map(|w| w[1].clone()).collect();
    let cflags: Vec<&str> = cflags.iter().map(|s| s.as_str()).collect();
    build::exe(&units, &out, args.iter().any(|a| a == "--tcc"), &cflags).unwrap_or_else(|e| die(e));
    println!("built {}", out.display());
}

/// `serve <program> [--clang] [--watch] [executable options]`: build into the temp directory and run, passing the options on.
fn serve(args: &[String]) {
    let files = inputs(args);
    let clang = args.iter().any(|a| a == "--clang");
    let pass: Vec<&String> = args[files.len()..].iter().filter(|a| *a != "--clang" && *a != "--watch").collect();
    if args.iter().any(|a| a == "--watch") { return serve_watch(&files, clang, &pass); }
    let (units, name) = program(&files);
    let exe = std::env::temp_dir().join(format!("mito-serve-{}-{}{}", name, std::process::id(), std::env::consts::EXE_SUFFIX));
    build::exe(&units, &exe, !clang, &[]).unwrap_or_else(|e| die(e));
    let status = std::process::Command::new(&exe).args(pass).status().unwrap_or_else(|e| die(format!("{}: {}", exe.display(), e)));
    let _ = std::fs::remove_file(&exe);
    std::process::exit(status.code().unwrap_or(1));
}

/// `serve --watch`: rebuild and restart whenever the program changes (for a system: any `.mito` file beside it). A
/// program that does not compile is reported and the last good build keeps serving. State starts empty on each restart.
fn serve_watch(files: &[&String], clang: bool, pass: &[&String]) {
    let (f, _, system) = read_program(files);
    let path = std::path::PathBuf::from(&f);
    let watched = |p: &std::path::Path| -> Vec<std::path::PathBuf> {
        if !system { return vec![p.to_path_buf()]; }
        let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
        std::fs::read_dir(dir).map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|x| x.extension().map_or(false, |e| e == "mito")).collect()).unwrap_or_default()
    };
    // the files' modification times: a change in any is a change
    let stamp = || -> Vec<(std::path::PathBuf, Option<std::time::SystemTime>)> {
        let mut v: Vec<_> = watched(&path).into_iter().map(|p| { let t = std::fs::metadata(&p).and_then(|m| m.modified()).ok(); (p, t) }).collect();
        v.sort();
        v
    };
    let (mut child, mut exe): (Option<std::process::Child>, Option<std::path::PathBuf>) = (None, None);
    let mut seen = stamp();
    let mut generation = 0u32;
    loop {
        // build; on success replace the running server (the old one stops first: they share the port)
        let started = std::time::Instant::now();
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {}", f, e));
        let built = text.and_then(|t| compile(&f, &t, system)).and_then(|(units, name)| {
            generation += 1;
            let next = std::env::temp_dir().join(format!("mito-serve-{}-{}-{}{}", name, std::process::id(), generation, std::env::consts::EXE_SUFFIX));
            build::exe(&units, &next, !clang, &[]).map(|_| next)
        });
        match built {
            Err(e) => eprintln!("error: {}\n(watching {}: {})", e, f, if child.is_some() { "the last good build keeps serving" } else { "nothing is serving yet" }),
            Ok(next) => {
                let built_ms = started.elapsed().as_millis();
                if let Some(mut c) = child.take() { let _ = c.kill(); let _ = c.wait(); }
                if let Some(old) = exe.take() { let _ = std::fs::remove_file(old); }
                match std::process::Command::new(&next).args(pass).spawn() {
                    Ok(c) => { stop_with_me(&c); eprintln!("(watching {}: built in {} ms, serving {} ms later)", f, built_ms, started.elapsed().as_millis() - built_ms); child = Some(c); }
                    Err(e) => eprintln!("error: {}: {}", next.display(), e),
                }
                exe = Some(next);
            }
        }
        // wait for a change; report a server that stops on its own
        loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if let Some(c) = child.as_mut() {
                if let Ok(Some(st)) = c.try_wait() { eprintln!("(watching {}: the server stopped ({}); waiting for a change)", f, st); child = None; }
            }
            let now = stamp();
            if now != seen {
                // editors write in steps: let the file settle
                std::thread::sleep(std::time::Duration::from_millis(50));
                seen = stamp();
                break;
            }
        }
    }
}

/// The server stops when the watcher does, however it stops: it joins a job object that ends its processes when the
/// watcher's handle to it closes.
#[cfg(windows)]
fn stop_with_me(child: &std::process::Child) {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    extern "system" {
        fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(job: *mut c_void, class: i32, info: *mut c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
    }
    static JOB: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let job = *JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
        if job.is_null() { return 0; }
        // JOBOBJECT_EXTENDED_LIMIT_INFORMATION (144 bytes): LimitFlags at byte 16 = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        let mut info = [0u64; 18];
        info[2] = 0x2000;
        SetInformationJobObject(job, 9, info.as_mut_ptr() as *mut c_void, 144);
        job as usize
    });
    if job != 0 { unsafe { AssignProcessToJobObject(job as *mut c_void, child.as_raw_handle() as *mut c_void); } }
}
#[cfg(not(windows))]
fn stop_with_me(_: &std::process::Child) {}

/// `emit-c <program> [-o file.c]`; for a system, `-o dir`: one file per worker kind, and system.c.
fn emit_c(args: &[String]) {
    let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).cloned();
    let (f, text, system) = read_program(&inputs(args));
    let path = std::path::Path::new(&f);
    if system {
        let spec = spec::parse_spec(&text, path.parent().unwrap_or(std::path::Path::new("."))).unwrap_or_else(|e| die(format!("{}: {}", f, e)));
        let to = std::path::PathBuf::from(out.unwrap_or_else(|| ".".into()));
        // one file per worker kind (its program, interface prefixed k<n>_) and system.c: build them together
        let units = cgen::system_c(&spec).unwrap_or_else(|e| die(e));
        let names: Vec<String> = spec.kinds.iter().map(|k| format!("{}.c", k.name)).chain(["system.c".to_string()]).collect();
        for (u, n) in units.iter().zip(&names) {
            let p = to.join(n);
            std::fs::write(&p, u).unwrap_or_else(|e| die(format!("{}: {}", p.display(), e)));
            println!("wrote {}", p.display());
        }
        return;
    }
    let file = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let (state, code) = source::load(&text).unwrap_or_else(|e| die(report(&file, &text, &e)));
    let prog = layout::Program::parse(&state).unwrap_or_else(|e| die(report(&file, &text, &e)));
    let a = cgen::check(&prog, &code).unwrap_or_else(|e| die(report(&file, &text, &e)));
    let c = cgen::emit(&prog, &a, (&file, &code)).unwrap_or_else(|e| die(e));
    match out { Some(p) => { std::fs::write(&p, c).unwrap_or_else(|e| die(format!("{}: {}", p, e))); println!("wrote {}", p); } None => print!("{}", c) }
}

/// `sigs <program>` (a tool, not in the usage): every function's parameter and result kinds as the compiler sees them at its calls.
fn sigs(args: &[String]) {
    let (f, text, _) = read_program(&inputs(args));
    let file = std::path::Path::new(&f).file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let (state, code) = source::load(&text).unwrap_or_else(|e| die(report(&file, &text, &e)));
    let prog = layout::Program::parse(&state).unwrap_or_else(|e| die(report(&file, &text, &e)));
    let a = cgen::check(&prog, &code).unwrap_or_else(|e| die(report(&file, &text, &e)));
    for (name, ps, rs) in cgen::sigs(&prog, &a) {
        let params: Vec<String> = ps.iter().map(|k| k.join("|")).collect();
        println!("{}\t({})\t-> {}", name, params.join(", "), rs.join("|"));
    }
}

/// `test <program> [--clang]`: the program's tests, built with TinyCC (or clang) into a runner and run; exits 1 if any fails.
fn test(args: &[String]) {
    let files = inputs(args);
    let (f, text, system) = read_program(&files);
    if system { die(format!("{}: a system's tests are not supported yet: test each worker's program", f)); }
    let path = std::path::Path::new(&f);
    let file = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let name = path.file_stem().map_or("app".into(), |n| n.to_string_lossy().into_owned());
    let (s, code) = source::load(&text).unwrap_or_else(|e| die(report(&file, &text, &e)));
    let prog = layout::Program::parse(&s).unwrap_or_else(|e| die(report(&file, &text, &e)));
    let a = cgen::check(&prog, &code).unwrap_or_else(|e| die(report(&file, &text, &e)));
    if cgen::test_names(&a).is_empty() { println!("{}: no tests (write them as `test \"what it checks\" {{ ... }}`)", file); return; }
    let units = cgen::program_test_exe(&prog, &a, Some((&file, &code))).unwrap_or_else(|e| die(e));
    let exe = std::env::temp_dir().join(format!("mito-test-{}-{}{}", name, std::process::id(), std::env::consts::EXE_SUFFIX));
    build::exe(&units, &exe, !args.iter().any(|a| a == "--clang"), &[]).unwrap_or_else(|e| die(e));
    let status = std::process::Command::new(&exe).arg(&file).status().unwrap_or_else(|e| die(format!("{}: {}", exe.display(), e)));
    let _ = std::fs::remove_file(&exe);
    std::process::exit(status.code().unwrap_or(1));
}

/// `check <program> --kinds`: every local's kind as the compiler sees it, by function and endpoint, with its line.
fn kinds(args: &[String]) {
    let (f, text, _) = read_program(&inputs(args));
    let file = std::path::Path::new(&f).file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let (state, code) = source::load(&text).unwrap_or_else(|e| die(report(&file, &text, &e)));
    let prog = layout::Program::parse(&state).unwrap_or_else(|e| die(report(&file, &text, &e)));
    let a = cgen::check(&prog, &code).unwrap_or_else(|e| die(report(&file, &text, &e)));
    let mut last = String::new();
    for (place, line, name, kind) in cgen::local_kinds(&prog, &a) {
        if place != last { println!("{}", place); last = place.clone(); }
        // a library module's code comes after the file's lines
        if line as usize > text.lines().count() { println!("  (module code)  {}: {}", name, kind); } else { println!("  {}:{}  {}: {}", file, line, name, kind); }
    }
}
