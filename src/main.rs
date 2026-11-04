mod alloc_count;
mod exp;
mod lang;
mod layout;
mod os;
mod region;
mod runtime;
mod serve;
mod cgen;
mod system;
mod tcc;
use runtime::{Error, Instance};

const MB: usize = 1 << 20;

#[global_allocator]
static ALLOC: alloc_count::Counting = alloc_count::Counting;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) == Some("exp") { return if args.get(1).map(|s| s.as_str()) == Some("json") { serve::json_floor() } else if args.get(1).map(|s| s.as_str()) == Some("pressure") { exp::pressure() } else if args.get(1).map(|s| s.as_str()) == Some("chat") { exp::chat_scan() } else if args.get(1).map(|s| s.as_str()) == Some("access") { exp::access_cost() } else if args.get(1).map(|s| s.as_str()) == Some("trim") { { exp::trim_cost(); exp::scoped_cost() } } else { exp::run() }; }
    if args.first().map(|s| s.as_str()) == Some("emit-c") { return emit_c(&args[1..]); }
    if matches!(args.first().map(|s| s.as_str()), Some("system") | Some("bench-system")) {
        let file = std::path::Path::new(args.get(1).expect("system file"));
        let src = std::fs::read_to_string(file).expect("read system file");
        let dir = file.parent().unwrap_or(std::path::Path::new("."));
        let opt = |name: &str, d: usize| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(d);
        if args[0] == "bench-system" {
            let configs: Vec<(usize, usize)> = args.iter().position(|a| a == "--configs").and_then(|i| args.get(i + 1)).map(|s| s.split(',').filter_map(|c| c.split_once('x')).filter_map(|(a, r)| Some((a.parse().ok()?, r.parse().ok()?))).collect()).unwrap_or(vec![(1, 1), (1, 2), (2, 4), (4, 8)]);
            system::bench(&src, dir, opt("--clients", 8), &configs, opt("--ops", 100_000), opt("--reps", 2), args.iter().position(|a| a == "--spin").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()), args.iter().position(|a| a == "--batch").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()), args.iter().any(|a| a == "--compile"));
        } else {
            let mut spec = system::parse_spec(&src, dir).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1) });
            for k in &mut spec.kinds { if let Some(n) = args.iter().position(|a| *a == format!("--{}", k.name)).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()) { k.count = n; } }
            if args.iter().any(|a| a == "--compile") { spec.compile = true; }
            let sys = system::System::start(spec).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1) });
            system::serve(std::sync::Arc::new(sys), opt("--port", 8080) as u16, opt("--conns", 16)).expect("serve");
        }
        return;
    }
    if matches!(args.first().map(|s| s.as_str()), Some("serve") | Some("bench")) {
        let (state, api) = (args.get(1).expect("state file"), args.get(2).expect("api file"));
        let prog = layout::Program::parse(&std::fs::read_to_string(state).expect("read state")).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1) });
        let inst = Instance::start(prog, 1024 * MB).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1) });
        let mut server = serve::Server::new(inst, &std::fs::read_to_string(api).expect("read api")).unwrap_or_else(|e| { eprintln!("error in api file: {}", e); std::process::exit(1) });
        let opt = |name: &str, d: usize| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(d);
        if args.iter().any(|a| a == "--soft") { let p = runtime::Pressure::new(&server.inst, opt("--soft", 0) * MB, opt("--hard", usize::MAX / MB) * MB); server.pressure = Some(p); }
        if args.iter().any(|a| a == "--compile") { match server.compile() { Ok((notes, _)) => for n in notes { println!("  {}", n); }, Err(e) => eprintln!("compiling failed, endpoints stay interpreted: {}", e) } }
        if args[0] == "bench" { serve::bench(&mut server, opt("--n", 2000)); } else { serve::serve(&mut server, opt("--port", 8080) as u16).expect("serve"); }
        return;
    }
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
            "push" => inst.push(a).map(|(i, g)| match g { Some(g) => println!("  slot {} generation {}", i, g), None => println!("  index {}", i) }),
            "fill" => (0..b.parse::<usize>().unwrap_or(0)).try_for_each(|_| inst.push(a).map(|_| ())),
            "pop" => inst.pop(a),
            "remove" => inst.remove(a, b.trim()),
            "sum" => inst.sum(a, b.trim(), 20).map(|(t, n, ns)| println!("  sum {} over {} rows, {:.3} ns/row", t, n, ns)),
            "clear" => inst.clear(a),
            "set" => inst.set(a, b.trim()),
            "get" => inst.get(a).map(|v| println!("  {}", v)),
            "trim" => { let f = if a.is_empty() { Ok(inst.trim()) } else { inst.trim_path(a) }.unwrap_or_else(|e| { println!("  error: {:?}", e); 0 }); let t = inst.last_trim; println!("  released {:.3} MB: scanned {} live ranges in {:.2} ms, {} OS calls in {:.2} ms", mb(f), t.ranges, t.scan_ns as f64 / 1e6, t.os_calls, t.os_ns as f64 / 1e6); Ok(()) }
            "inspect" => { inspect(&inst, base); Ok(()) }
            _ => Err(Error::Bad(format!("unknown command `{}`", cmd))),
        };
        match r { Ok(()) => {}, Err(Error::Full(m)) => println!("  FULL: {}", m), Err(Error::TooLong(m)) => println!("  TOO LONG: {}", m), Err(Error::NotLive(m)) => println!("  NOT LIVE: {}", m), Err(Error::Stale(m)) => println!("  STALE: {}", m), Err(Error::Bad(m)) => println!("  error: {}", m) }
    }
}

/// `emit-c <state.mito> <api> [-o file.c]`, or `emit-c <file.system> [-o dir]` for one file per worker kind.
fn emit_c(args: &[String]) {
    let die = |e: String| -> ! { eprintln!("error: {}", e); std::process::exit(1) };
    let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).cloned();
    let files: Vec<&String> = args.iter().enumerate().filter(|(i, a)| *a != "-o" && (*i == 0 || args[i - 1] != "-o")).map(|(_, a)| a).collect();
    let read = |f: &str| std::fs::read_to_string(f).unwrap_or_else(|e| die(format!("{}: {}", f, e)));
    let one = |state: &str, name: &str, text: &str| -> String {
        let prog = layout::Program::parse(state).unwrap_or_else(|e| die(e));
        let mut a = lang::parse(text).unwrap_or_else(|e| die(format!("{}: {}", name, e)));
        lang::bind(&mut a, &|n| prog.global(n)).unwrap_or_else(|e| die(format!("{}: {}", name, e)));
        cgen::emit(&prog, &a, (name, text))
    };
    match files.as_slice() {
        [sys] if sys.ends_with(".system") => {
            let file = std::path::Path::new(sys.as_str());
            let dir = file.parent().unwrap_or(std::path::Path::new("."));
            let spec = system::parse_spec(&std::fs::read_to_string(file).unwrap_or_else(|e| die(e.to_string())), dir).unwrap_or_else(|e| die(e));
            let to = std::path::PathBuf::from(out.unwrap_or_else(|| ".".into()));
            for k in &spec.kinds {
                let c = one(&k.state, &format!("{} api", k.name), &k.api);
                let p = to.join(format!("{}.c", k.name));
                std::fs::write(&p, c).unwrap_or_else(|e| die(format!("{}: {}", p.display(), e)));
                println!("wrote {}", p.display());
            }
        }
        [state, api] => {
            let name = std::path::Path::new(api.as_str()).file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
            let c = one(&read(state), &name, &read(api));
            match out { Some(p) => { std::fs::write(&p, c).unwrap_or_else(|e| die(format!("{}: {}", p, e))); println!("wrote {}", p); } None => print!("{}", c) }
        }
        _ => die("usage: emit-c <state.mito> <api> [-o file.c]  |  emit-c <file.system> [-o dir]".into()),
    }
}

fn mb(n: usize) -> f64 { n as f64 / MB as f64 }

fn inspect(inst: &Instance, base: os::OsMem) {
    println!("  {:<24} {:<44} {:>10} {:>11} {:>11}", "path", "type", "start", "reserved", "live");
    for (p, t, o, r, l) in inst.inspect() { println!("  {:<24} {:<44} {:>10} {:>11} {:>11}", p, t, o, r, l); }
    let now = os::os_mem();
    println!("  totals: reserved {:.3} MB, live {:.3} MB, committed {:.3} MB | OS commit {:+.3} MB, resident {:+.3} MB (since start)",
        mb(inst.reserved()), mb(inst.live_total()), mb(inst.committed()), mb(now.private) - mb(base.private), mb(now.resident) - mb(base.resident));
}
