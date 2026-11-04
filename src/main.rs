mod exp;
mod layout;
mod os;
mod region;
mod runtime;
use runtime::{Error, Instance};

const MB: usize = 1 << 20;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) == Some("exp") { return exp::run(); }
    let mut budget = 1024 * MB;
    let mut files = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--budget" { budget = it.next().and_then(|v| v.parse::<usize>().ok()).expect("--budget <MB>") * MB; } else { files.push(a.clone()); }
    }
    if files.is_empty() { eprintln!("usage: mitochondria <state.mito> [script] [--budget MB]  |  mitochondria exp"); std::process::exit(2); }
    let src = std::fs::read_to_string(&files[0]).expect("read state file");
    let prog = match layout::Program::parse(&src) { Ok(p) => p, Err(e) => { eprintln!("error: {}", e); std::process::exit(1); } };
    let base = os::os_mem();
    let mut inst = match Instance::start(prog, budget) { Ok(i) => i, Err(e) => { eprintln!("error: {}", e); std::process::exit(1); } };
    println!("started: reserved {:.2} MB; committed {:.3} MB (live bytes only); budget {} MB", mb(inst.reserved()), mb(inst.committed()), budget / MB);
    let Some(script) = files.get(1) else { return inspect(&inst, base) };
    for line in std::fs::read_to_string(script).expect("read script").lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let (a, b) = rest.split_once(' ').unwrap_or((rest, ""));
        println!("> {}", line);
        let r: Result<(), Error> = match cmd {
            "push" => inst.push(a).map(|i| println!("  index {}", i)),
            "fill" => (0..b.parse::<usize>().unwrap_or(0)).try_for_each(|_| inst.push(a).map(|_| ())),
            "pop" => inst.pop(a),
            "clear" => inst.clear(a),
            "set" => inst.set(a, b.trim()),
            "get" => inst.get(a).map(|v| println!("  {}", v)),
            "trim" => { let f = inst.trim(); println!("  released {:.3} MB", mb(f)); Ok(()) }
            "inspect" => { inspect(&inst, base); Ok(()) }
            _ => Err(Error::Bad(format!("unknown command `{}`", cmd))),
        };
        match r { Ok(()) => {}, Err(Error::Full(m)) => println!("  FULL: {}", m), Err(Error::Bad(m)) => println!("  error: {}", m) }
    }
}

fn mb(n: usize) -> f64 { n as f64 / MB as f64 }

fn inspect(inst: &Instance, base: os::OsMem) {
    println!("  {:<22} {:<34} {:>10} {:>11} {:>11}", "path", "type", "offset", "reserved", "live");
    for (p, t, o, r, l) in inst.inspect() { println!("  {:<22} {:<34} {:>10} {:>11} {:>11}", p, t, o, r, l); }
    let now = os::os_mem();
    println!("  totals: reserved {:.3} MB, live {:.3} MB, committed {:.3} MB | OS commit {:+.3} MB, resident {:+.3} MB (since start)",
        mb(inst.reserved()), mb(inst.live_total()), mb(inst.committed()), mb(now.private) - mb(base.private), mb(now.resident) - mb(base.resident));
}
