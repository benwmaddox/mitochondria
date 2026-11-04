mod alloc_count;
mod exp;
mod lang;
mod layout;
mod os;
mod region;
mod runtime;
mod serve;
use runtime::{Error, Instance};

const MB: usize = 1 << 20;

#[global_allocator]
static ALLOC: alloc_count::Counting = alloc_count::Counting;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) == Some("exp") { return if args.get(1).map(|s| s.as_str()) == Some("pressure") { exp::pressure() } else if args.get(1).map(|s| s.as_str()) == Some("chat") { exp::chat_scan() } else if args.get(1).map(|s| s.as_str()) == Some("access") { exp::access_cost() } else if args.get(1).map(|s| s.as_str()) == Some("trim") { { exp::trim_cost(); exp::scoped_cost() } } else { exp::run() }; }
    if matches!(args.first().map(|s| s.as_str()), Some("serve") | Some("bench")) {
        let (state, api) = (args.get(1).expect("state file"), args.get(2).expect("api file"));
        let prog = layout::Program::parse(&std::fs::read_to_string(state).expect("read state")).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1) });
        let inst = Instance::start(prog, 1024 * MB).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1) });
        let mut server = serve::Server::new(inst, &std::fs::read_to_string(api).expect("read api")).unwrap_or_else(|e| { eprintln!("error in api file: {}", e); std::process::exit(1) });
        let opt = |name: &str, d: usize| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(d);
        if args.iter().any(|a| a == "--soft") { let p = runtime::Pressure::new(&server.inst, opt("--soft", 0) * MB, opt("--hard", usize::MAX / MB) * MB); server.pressure = Some(p); }
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

fn mb(n: usize) -> f64 { n as f64 / MB as f64 }

fn inspect(inst: &Instance, base: os::OsMem) {
    println!("  {:<24} {:<44} {:>10} {:>11} {:>11}", "path", "type", "start", "reserved", "live");
    for (p, t, o, r, l) in inst.inspect() { println!("  {:<24} {:<44} {:>10} {:>11} {:>11}", p, t, o, r, l); }
    let now = os::os_mem();
    println!("  totals: reserved {:.3} MB, live {:.3} MB, committed {:.3} MB | OS commit {:+.3} MB, resident {:+.3} MB (since start)",
        mb(inst.reserved()), mb(inst.live_total()), mb(inst.committed()), mb(now.private) - mb(base.private), mb(now.resident) - mb(base.resident));
}
