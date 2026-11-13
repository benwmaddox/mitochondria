mod build;
mod fmt;
#[cfg(test)]
mod cases;
#[cfg(test)]
mod http_check;
mod platform;
mod docs_check;
#[cfg(test)]
mod error_corpus;
#[cfg(test)]
mod test_runner;
mod lang;
mod modules;
mod generics;
mod layout;
mod cgen;
mod scalar;
mod source;
mod spec;
mod web;
// the in-memory compiler drives the tests; uild runs tcc.exe from the same directory
#[cfg_attr(not(test), allow(dead_code))]
mod tcc;

const USAGE: &str = "usage: mito <command> <program> [options]

A program is one `.mito` file: a program, or a system of workers (a file holding `system { ... }`).

  check  <program> [--kinds|--costs] report the problems found, or say it loads; --kinds lists every local's kind; --costs each endpoint's work
  build  <program> [-o app.exe] [--tcc]
                                    an executable serving HTTP (clang -O3; --tcc builds in milliseconds, unoptimized)
  run    <program> [--clang] [-- args]   serve, for a command-line program (`@main`): what follows `--` is its command line, stdin and stdout are yours
  serve  <program> [--clang] [--watch] [executable options]
                                    build with TinyCC (or clang) and run it at once; --watch rebuilds and restarts
                                    when the program changes (state starts empty each time)
  test   <program> [--clang]        run the program's tests (`test \"name\" { ... }`), each on a fresh state
  emit-c <program> [-o file.c]       (a system: -o dir, one file per worker kind and system.c)
                                    the generated C, to read or build yourself
  fmt    <program>... [--check|--write]   print the program file formatted (--write: rewrite the files; --check: list the files that
                                    would change, exit 1 if any does)

Executable options: --port N (8080); --budget MB, --soft MB, --hard MB (one program's memory; soft and hard turn on
memory pressure); --conns N (a system's connection threads, 16); --seed N and --clock MS (fixed, for tests).";

fn die(e: String) -> ! { eprintln!("error: {}", e); std::process::exit(1) }

// A program in several files: `use "rooms.mito";` (a line of its own) puts that file's text there (relative to the file that uses
// it; each file once). The merged text is one program; where each piece came from is kept for errors' places and `serve --watch`.
thread_local! { static PIECES: std::cell::RefCell<Vec<Piece>> = std::cell::RefCell::new(Vec::new()); }
/// Lines `at..at + n` of the merged text are lines `from..` of `file`.
#[derive(Clone)]
struct Piece { at: usize, n: usize, file: String, from: usize, text: String }
/// `path`'s text with its `use`s expanded, and the pieces it is made of.
fn expand(path: &str, text: &str, seen: &mut Vec<String>, out: &mut String, pieces: &mut Vec<Piece>) -> Result<(), String> {
    let dir = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new(".")).to_path_buf();
    let mut from = 1; let mut run = 0; let mut start = out.lines().count() + 1;
    let flush = |pieces: &mut Vec<Piece>, start: usize, run: usize, from: usize| if run > 0 { pieces.push(Piece { at: start, n: run, file: path.to_string(), from, text: text.to_string() }); };
    for (k, line) in text.split('\n').enumerate() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("use \"") {
            // `use "file.mito";`, a comment after it allowed
            let Some((name, after)) = rest.split_once("\";").filter(|(_, a)| { let a = a.trim(); a.is_empty() || a.starts_with("//") }) else { return Err(format!("{}:{}:1: `use \"file.mito\";` names a file in double quotes", path, k + 1)) };
            flush(pieces, start, run, from);
            // `std/...` is a library file embedded in the compiler (modules::STD_FILES), before anything beside the program
            let std = name.starts_with("std/");
            let p = dir.join(name);
            let ps = if std { name.to_string() } else { p.to_string_lossy().replace('\\', "/") };
            let ps = ps.strip_prefix("./").map(String::from).unwrap_or(ps);
            // the same file by any path (`lib/../app.mito` is `app.mito`)
            let key = if std { ps.clone() } else { std::fs::canonicalize(&p).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| ps.clone()) };
            if seen.contains(&key) { return Err(format!("{}:{}:1: `{}` is used twice (or uses itself): each file once", path, k + 1, name)); }
            seen.push(key);
            let inner = if std { modules::std_file(name).map(String::from).map_err(|e| format!("{}:{}:1: {}", path, k + 1, e))? }
                else { std::fs::read_to_string(&p).map_err(|e| format!("{}:{}:1: {}: {}", path, k + 1, ps, e))? };
            expand(&ps, &inner, seen, out, pieces)?;
            // the `use` line itself stays (blank), so this file's lines after it keep their numbers in their own piece
            out.push('\n');
            pieces.push(Piece { at: out.lines().count(), n: 1, file: path.to_string(), from: k + 1, text: text.to_string() });
            start = out.lines().count() + 1; from = k + 2; run = 0;
            continue;
        }
        out.push_str(line); out.push('\n'); run += 1;
    }
    flush(pieces, start, run, from);
    Ok(())
}
/// Where line `l` of the merged program comes from: its file, its line there, and that file's text.
fn origin(l: usize) -> Option<(String, usize, String)> {
    PIECES.with(|p| p.borrow().iter().find(|x| l >= x.at && l < x.at + x.n).map(|x| (x.file.clone(), x.from + (l - x.at), x.text.clone())))
}

/// A program's error as `file:line:col: message`, with the line quoted and a caret under the column, when it names a
/// place (`line:col: ...`) inside the file; otherwise `file: message`. Several errors (one per line) are reported one by one.
fn report(file: &str, text: &str, e: &str) -> String {
    // several independent errors, one per line: each reported on its own
    if e.contains('\n') { return e.lines().map(|x| report(file, text, x)).collect::<Vec<_>>().join("\nerror: "); }
    let mut parts = e.splitn(3, ':');
    let (Some(l), Some(c), Some(rest)) = (parts.next(), parts.next(), parts.next()) else { return format!("{}: {}", file, e) };
    let (Ok(l), Ok(c)) = (l.parse::<usize>(), c.parse::<usize>()) else { return format!("{}: {}", file, e) };
    let rest = rest.trim_start();
    // a program in several files: the place in the file the line came from
    if let Some((f2, l2, t2)) = origin(l) { return quote(&f2, &t2, l2, c, rest); }
    quote(file, text, l, c, rest)
}
/// The column of the first `name` the message quotes, when that word is on the line at or after the statement's column: the caret goes under it, not under the statement's first character.
fn refine_column(line: &str, c: usize, rest: &str) -> usize {
    let Some(word) = rest.split('`').nth(1).filter(|w| !w.is_empty() && w.chars().all(|ch| ch.is_alphanumeric() || ch == '_')) else { return c };
    let ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    let chars: Vec<char> = line.chars().collect();
    let w: Vec<char> = word.chars().collect();
    let from = c.saturating_sub(1);
    for i in from..chars.len().saturating_sub(w.len() - 1) {
        if chars[i..i + w.len()] == w[..] && (i == 0 || !ident(chars[i - 1])) && chars.get(i + w.len()).map_or(true, |ch| !ident(*ch)) { return i + 1; }
    }
    c
}
/// `file:line:col: message`, the line quoted and a caret under the column.
fn quote(file: &str, text: &str, l: usize, c: usize, rest: &str) -> String {
    match text.lines().nth(l.wrapping_sub(1)) {
        Some(line) => {
            let shown = line.replace('\t', " ");
            let c = refine_column(&shown, c, rest);
            let caret: String = shown.chars().take(c.saturating_sub(1)).map(|_| ' ').collect();
            format!("{}:{}:{}: {}\n{:>5} | {}\n      | {}^", file, l, c, rest, l, shown, caret)
        }
        None => format!("{}: {} (in library module code)", file, rest),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().take_while(|a| *a != "--").any(|a| a == "--platform") { die("there is no --platform switch: the platform is the entry file's (`platform cli;` in it, or its entry points)".into()); }
    let rest = args.get(1..).unwrap_or(&[]);
    // `file("...")` in a program reads beside it
    if let Some(f) = inputs(rest).first() { lang::set_file_dir(std::path::Path::new(f.as_str()).parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."))); }
    match args.first().map(|s| s.as_str()) {
        Some("check") if rest.iter().any(|a| a == "--kinds") => kinds(rest),
        Some("check") if rest.iter().any(|a| a == "--costs") => costs(rest),
        Some("check") => { let (_, name) = program(&inputs(rest)); println!("{}: loads", name); }
        Some("build") => build_exe(rest),
        Some("serve") | Some("run") => serve(rest),
        Some("test") => test(rest),
        Some("emit-c") => emit_c(rest),
        Some("sigs") => sigs(rest),
        Some("fmt") => fmt::run(rest),
        _ => { eprintln!("{}", USAGE); std::process::exit(2) }
    }
}

/// `check <program> --costs`: per endpoint, the request struct bytes it zeroes and what its code does (sites, and those in loops)
fn costs(args: &[String]) {
    let (f, text, system) = read_program(&inputs(args));
    if system { die(format!("{}: --costs is for one program, not a system", f)); }
    let path = std::path::Path::new(&f);
    if let Some((w, line)) = web::find(&text) {
        let w = web::parse(&w, line, path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."))).unwrap_or_else(|e| die(report(&f, &text, &e)));
        lang::set_assets(w.assets.clone());
    }
    let (s, code) = source::load(&text).unwrap_or_else(|e| die(report(&f, &text, &e)));
    let prog = layout::Program::parse(&s).unwrap_or_else(|e| die(report(&f, &text, &e)));
    let api = cgen::check(&prog, &code).unwrap_or_else(|e| die(report(&f, &text, &e)));
    let g = cgen::generate(&prog, &api);
    let req = prog.request.map_or(0, |r| r.1);
    println!("{}: per endpoint, the request struct bytes zeroed (of {}), and sites in its code (in loops: run once a round)", f, req);
    for (k, ep) in api.eps.iter().enumerate() {
        let z = match g.zeros.get(k) { Some(Some(rs)) => rs.iter().map(|(a, b)| b - a).sum::<usize>(), _ => req };
        let c = g.costs.get(k).cloned().unwrap_or_default();
        let both = |x: (u32, u32)| if x.1 > 0 { format!("{} ({} in loops)", x.0, x.1) } else { x.0.to_string() };
        let lets = if c.lets.0 == 0 { String::new() } else { format!("; region text {} locals (at most {} B at once{})", c.lets.0, c.lets.1, if c.lets.2.is_empty() { String::new() } else { let mut w: Vec<String> = c.lets.2.iter().map(|x| place_in_file(&f, x)).collect(); w.dedup(); format!("; no bound at {}", w.join(", ")) }) };
        println!("  {}: zeroes {} B; queries {}, row reads {}, table writes {}, text built {}{}", ep.text, z, both(c.queries), both(c.reads), both(c.writes), both(c.texts), lets);
    }
}

/// A place cgen names ("fn f line N", "line N", maybe with a note after) with the line in the file it came from: "fn f (file:line)"
fn place_in_file(file: &str, p: &str) -> String {
    let Some(k) = p.find("line ") else { return p.to_string() };
    let digits: String = p[k + 5..].chars().take_while(|c| c.is_ascii_digit()).collect();
    let Ok(l) = digits.parse::<usize>() else { return p.to_string() };
    let (f2, l2) = match origin(l) { Some((f2, l2, _)) => (f2, l2), None => (file.to_string(), l) };
    format!("{}{}:{}{}", &p[..k], f2, l2, &p[k + 5 + digits.len()..])
}

/// The program files among `args`: everything before the first option.
fn inputs(args: &[String]) -> Vec<&String> { args.iter().take_while(|a| !a.starts_with('-')).collect() }

/// Read a program file: (its text, whether it is a system of workers).
fn read_program(files: &[&String]) -> (String, String, bool) {
    let [f] = files else { eprintln!("{}", USAGE); std::process::exit(2) };
    let text = load_text(f).unwrap_or_else(|e| die(e));
    let system = source::is_system(&text);
    (f.to_string(), text, system)
}
/// A program file's text, its `use`s expanded (the pieces kept for errors' places).
fn load_text(f: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(f).map_err(|e| format!("{}: {}", f, e))?;
    if source::is_system(&text) || !text.lines().any(|l| l.trim_start().starts_with("use \"")) { PIECES.with(|p| p.borrow_mut().clear()); return Ok(text); }
    let (mut out, mut pieces) = (String::new(), Vec::new());
    let root = std::fs::canonicalize(f).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| f.to_string());
    expand(f, &text, &mut vec![root], &mut out, &mut pieces)?;
    PIECES.with(|p| *p.borrow_mut() = pieces);
    Ok(out)
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
    // the platform is the entry file's: what it says (`platform cli;`), or what its entry points show
    let declared = platform::declared(text).map_err(|e| format!("{}: {}", f, e))?;
    let plat = platform::resolve(declared, &a, &code).map_err(|e| format!("{}: {}", f, e))?;
    let web = web.map_or(String::new(), |w| web::c(&w));
    let units = cgen::program_exe(&prog, &a, Some((&file, &code)), &web)?;
    if let Some(p) = plat { platform::check_bundle(p, &units).map_err(|e| format!("{}: {}", f, e))?; }
    Ok((units, name))
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
    // `mito run tool.mito -- a b < in.txt`: a command-line program (`@main`) built and run at once; what follows `--` is its command line
    let mut pass: Vec<&String> = args[files.len()..].iter().filter(|a| *a != "--clang" && *a != "--watch").collect();
    if let Some(k) = pass.iter().position(|a| *a == "--") { pass.remove(k); }
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
        if !system { let mut v = vec![p.to_path_buf()]; PIECES.with(|ps| for x in ps.borrow().iter() { let q = std::path::PathBuf::from(&x.file); if !v.contains(&q) { v.push(q); } }); return v; }
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
        let text = load_text(&f);
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

#[cfg(test)]
mod tests {
    #[test]
    fn std_files_come_from_the_compiler() {
        // a program anywhere (here: a directory with no lib/ at all) can use the embedded library files
        let dir = std::env::temp_dir().join(format!("mito-std-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let app = dir.join("app.mito");
        let head = "struct App { n: i64; }\nstate app: App;\n";
        let tail = "@get(\"/n\") fn n() -> Response<{ v: f64, g: i64 }> { return ok({ v: 1.5.clamp(0.0, 1.0), g: 12.gcd(18) }); }\n";
        std::fs::write(&app, format!("{}use \"std/math.mito\";\nuse \"std/random.mito\";\n{}", head, tail)).unwrap();
        let f = app.to_string_lossy().replace('\\', "/");
        let text = super::load_text(&f).unwrap();
        assert!(text.contains("fn clamp(self: f64") && text.contains("fn next(self: write Rng)"));
        // beside the program is not searched: a std/ directory there changes nothing
        std::fs::create_dir_all(dir.join("std")).unwrap();
        std::fs::write(dir.join("std/math.mito"), "this is not code\n").unwrap();
        assert!(super::load_text(&f).unwrap().contains("fn clamp(self: f64"));
        // an unknown library file names the known ones; a file twice is refused as for any file
        std::fs::write(&app, format!("{}use \"std/nope.mito\";\n{}", head, tail)).unwrap();
        let e = super::load_text(&f).unwrap_err();
        assert!(e.contains("std/nope.mito") && e.contains("`std/math.mito`") && e.contains("`std/random.mito`"), "{}", e);
        std::fs::write(&app, format!("{}use \"std/math.mito\";\nuse \"std/math.mito\";\n{}", head, tail)).unwrap();
        assert!(super::load_text(&f).unwrap_err().contains("used twice"));
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn a_program_in_several_files() {
        let dir = std::env::temp_dir().join(format!("mito-use-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        let app = dir.join("app.mito");
        std::fs::write(&app, "struct App { n: i64; }\nstate app: App;\nuse \"lib/pages.mito\";\n@post(\"/n\") fn bump() -> Response<{ n: i64 }> { app.n += 1; return ok({ n: twice(app.n) }); }\n").unwrap();
        std::fs::write(dir.join("lib/pages.mito"), "// helpers\nfn twice(x: i64) -> i64 { return x * nope; }\n").unwrap();
        let f = app.to_string_lossy().replace('\\', "/");
        let text = super::load_text(&f).unwrap();
        assert!(text.contains("fn twice") && text.contains("fn bump"));
        // an error in the used file is placed there: its own line, quoted
        let e = super::compile(&f, &text, false).unwrap_err();
        assert!(e.contains("pages.mito:2:") && e.contains("return x * nope"), "{}", e);
        // a file used twice (or using itself) is refused
        std::fs::write(dir.join("lib/pages.mito"), "use \"../app.mito\";\n").unwrap();
        let e = super::load_text(&f).unwrap_err();
        assert!(e.contains("used twice (or uses itself)"), "{}", e);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
