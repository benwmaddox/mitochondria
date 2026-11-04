mod build;
#[cfg(test)]
mod cases;
#[cfg(test)]
mod http_check;
mod lang;
mod modules;
mod layout;
mod cgen;
mod source;
mod spec;
// the in-memory compiler drives the tests; uild runs tcc.exe from the same directory
#[cfg_attr(not(test), allow(dead_code))]
mod tcc;

const USAGE: &str = "usage: mitochondria <command> <program> [options]

A program is one `.mito` file: a program, or a system of workers (a file holding `system { ... }`).

  check  <program>                  report the first problem, or say it loads
  build  <program> [-o app.exe] [--tcc]
                                    an executable serving HTTP (clang -O2; --tcc builds in milliseconds, unoptimised)
  serve  <program> [--clang] [executable options]
                                    build with TinyCC (or clang) and run it at once
  emit-c <program> [-o file.c]       (a system: -o dir, one file per worker kind and system.c)
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
    let path = std::path::Path::new(&f);
    let name = path.file_stem().map_or("app".into(), |n| n.to_string_lossy().into_owned());
    if system {
        let spec = spec::parse_spec(&text, path.parent().unwrap_or(std::path::Path::new("."))).unwrap_or_else(|e| die(format!("{}: {}", f, e)));
        return (cgen::system_exe(&spec).unwrap_or_else(|e| die(e)), name);
    }
    let file = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let (s, code) = source::load(&text).unwrap_or_else(|e| die(format!("{}: {}", file, e)));
    let prog = layout::Program::parse(&s).unwrap_or_else(|e| die(format!("{}: {}", file, e)));
    let a = cgen::check(&prog, &code).unwrap_or_else(|e| die(format!("{}: {}", file, e)));
    (cgen::program_exe(&prog, &a, Some((&file, &code))).unwrap_or_else(|e| die(e)), name)
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
    let (state, code) = source::load(&text).unwrap_or_else(|e| die(format!("{}: {}", file, e)));
    let prog = layout::Program::parse(&state).unwrap_or_else(|e| die(format!("{}: {}", file, e)));
    let a = cgen::check(&prog, &code).unwrap_or_else(|e| die(format!("{}: {}", file, e)));
    let c = cgen::emit(&prog, &a, (&file, &code)).unwrap_or_else(|e| die(e));
    match out { Some(p) => { std::fs::write(&p, c).unwrap_or_else(|e| die(format!("{}: {}", p, e))); println!("wrote {}", p); } None => print!("{}", c) }
}
