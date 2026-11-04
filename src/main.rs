mod build;
#[cfg(test)]
mod cases;
#[cfg(test)]
mod http_check;
mod lang;
mod modules;
mod layout;
mod cgen;
mod spec;
// the in-memory compiler drives the tests; uild runs tcc.exe from the same directory
#[cfg_attr(not(test), allow(dead_code))]
mod tcc;

const USAGE: &str = "usage: mitochondria <command> <program> [options]

A program is a state and an api (`app.mito app.api`) or a system of workers (`app.system`).

  check  <program>                  report the first problem, or say it loads
  build  <program> [-o app.exe] [--tcc]
                                    an executable serving HTTP (clang -O2; --tcc builds in milliseconds, unoptimised)
  serve  <program> [--clang] [executable options]
                                    build with TinyCC (or clang) and run it at once
  emit-c <state.mito> <api> [-o file.c]  |  emit-c <file.system> [-o dir]
                                    the generated C, to read or build yourself

Executable options: --port N (8080); --budget MB, --soft MB, --hard MB (one program's memory; soft and hard turn on
memory pressure); --conns N (a system's connection threads, 16); --seed N and --clock MS (fixed, for tests).";

fn die(e: String) -> ! { eprintln!("error: {}", e); std::process::exit(1) }

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(|s| s.as_str()) {
        Some("check") => { let (_, name) = program(&inputs(rest)); println!("{}: loads", name); }
        Some("build") => build_exe(rest),
        Some("serve") => serve(rest),
        Some("emit-c") => emit_c(rest),
        _ => { eprintln!("{}", USAGE); std::process::exit(2) }
    }
}

/// The program files among `args`: everything before the first option.
fn inputs(args: &[String]) -> Vec<&String> { args.iter().take_while(|a| !a.starts_with('-')).collect() }

/// A program's C translation units for an executable, and a name for it.
fn program(files: &[&String]) -> (Vec<String>, String) {
    let read = |f: &str| std::fs::read_to_string(f).unwrap_or_else(|e| die(format!("{}: {}", f, e)));
    let stem = |f: &str| std::path::Path::new(f).file_stem().map_or("app".into(), |n| n.to_string_lossy().into_owned());
    match files {
        [sys] if sys.ends_with(".system") => {
            let file = std::path::Path::new(sys.as_str());
            let spec = spec::parse_spec(&read(sys), file.parent().unwrap_or(std::path::Path::new("."))).unwrap_or_else(|e| die(e));
            (cgen::system_exe(&spec).unwrap_or_else(|e| die(e)), stem(sys))
        }
        [state, api] => {
            let (s, text) = modules::expand(&read(state), &read(api)).unwrap_or_else(|e| die(e));
            let prog = layout::Program::parse(&s).unwrap_or_else(|e| die(e));
            let src_name = std::path::Path::new(api.as_str()).file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
            let a = cgen::check(&prog, &text).unwrap_or_else(|e| die(format!("{}: {}", src_name, e)));
            (cgen::program_exe(&prog, &a, Some((&src_name, &text))).unwrap_or_else(|e| die(e)), stem(api))
        }
        _ => { eprintln!("{}", USAGE); std::process::exit(2) }
    }
}

/// `build <program> [-o app.exe] [--tcc]`
fn build_exe(args: &[String]) {
    let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).cloned();
    let (units, name) = program(&inputs(args));
    let out = std::path::PathBuf::from(out.unwrap_or_else(|| format!("{}.exe", name)));
    build::exe(&units, &out, args.iter().any(|a| a == "--tcc"), &[]).unwrap_or_else(|e| die(e));
    println!("built {}", out.display());
}

/// `serve <program> [--clang] [executable options]`: build into the temp directory and run, passing the options on.
fn serve(args: &[String]) {
    let files = inputs(args);
    let (units, name) = program(&files);
    let clang = args.iter().any(|a| a == "--clang");
    let exe = std::env::temp_dir().join(format!("mito-serve-{}-{}.exe", name, std::process::id()));
    build::exe(&units, &exe, !clang, &[]).unwrap_or_else(|e| die(e));
    let pass: Vec<&String> = args[files.len()..].iter().filter(|a| *a != "--clang").collect();
    let status = std::process::Command::new(&exe).args(pass).status().unwrap_or_else(|e| die(format!("{}: {}", exe.display(), e)));
    let _ = std::fs::remove_file(&exe);
    std::process::exit(status.code().unwrap_or(1));
}

/// `emit-c <state.mito> <api> [-o file.c]`, or `emit-c <file.system> [-o dir]`: one file per worker kind and system.c.
fn emit_c(args: &[String]) {
    let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).cloned();
    let read = |f: &str| std::fs::read_to_string(f).unwrap_or_else(|e| die(format!("{}: {}", f, e)));
    match inputs(args).as_slice() {
        [sys] if sys.ends_with(".system") => {
            let file = std::path::Path::new(sys.as_str());
            let dir = file.parent().unwrap_or(std::path::Path::new("."));
            let spec = spec::parse_spec(&read(sys), dir).unwrap_or_else(|e| die(e));
            let to = std::path::PathBuf::from(out.unwrap_or_else(|| ".".into()));
            // one file per worker kind (its program, interface prefixed k<n>_) and system.c: build them together
            let units = cgen::system_c(&spec).unwrap_or_else(|e| die(e));
            let names: Vec<String> = spec.kinds.iter().map(|k| format!("{}.c", k.name)).chain(["system.c".to_string()]).collect();
            for (u, n) in units.iter().zip(&names) {
                let p = to.join(n);
                std::fs::write(&p, u).unwrap_or_else(|e| die(format!("{}: {}", p.display(), e)));
                println!("wrote {}", p.display());
            }
        }
        [state, api] => {
            let name = std::path::Path::new(api.as_str()).file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
            let (state, text) = modules::expand(&read(state), &read(api)).unwrap_or_else(|e| die(e));
            let prog = layout::Program::parse(&state).unwrap_or_else(|e| die(e));
            let a = cgen::check(&prog, &text).unwrap_or_else(|e| die(format!("{}: {}", name, e)));
            let c = cgen::emit(&prog, &a, (&name, &text)).unwrap_or_else(|e| die(e));
            match out { Some(p) => { std::fs::write(&p, c).unwrap_or_else(|e| die(format!("{}: {}", p, e))); println!("wrote {}", p); } None => print!("{}", c) }
        }
        _ => die("usage: emit-c <state.mito> <api> [-o file.c]  |  emit-c <file.system> [-o dir]".into()),
    }
}
