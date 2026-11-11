"""Mitochondria ports of LangArena tests against LangArena's own Rust implementation, same machine, same core, alternating rounds.

usage: python -I run.py [--mode run|test] [--rounds N] [--mito-only] [--write-only] [test_dir ...]      e.g.  python -I run.py --rounds 3 etc_sieve sort_quick

Methodology (LangArena's, kostya/LangArena, MIT):
  - the parameters are run.js (or test.js with --mode test), vendored with two test names shortened (BF::Array, BF::Recursion);
  - LangArena times only `run_all()`, after `prepare()` and the warmup iterations (default max(0.2 * iterations, 1) unless the
    entry names warmup_iterations), and the checksum covers the warmup runs too. A Mitochondria port is a program with
        POST /prepare/...   untimed: builds the inputs and resets the state (and the random generator)
        POST /warmup/{n}    untimed: n runs, state kept
        POST /run/{n}       timed: the iterations of run_all(), measured by the server itself (X-Handler-Ns)
        GET  /checksum      the result, compared with run.js's checksum on every measured run
    Tests whose checksum hashes a formatted float answer the float instead (GET /result) and this harness formats and hashes it
    exactly as LangArena's helper does (format "{:.7}", then hash = hash * 33 + byte, mod 2^32).
  - the Rust side is LangArena's own binary, built with its max-perf profile, timed by its own in-process timer.
  - both processes are pinned to the same core (pin.py), rounds alternate, best of N and the median are reported.
Rust with overflow checks on (RUST_CHECKED) is an extra column: Mitochondria always checks integer overflow, the reference's max-perf
profile does not."""
import ctypes, datetime, http.client, json, math, os, platform, re, socket, statistics, subprocess, sys, time

here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(here, "..", "ordered"))
from pin import pin

exe_suffix = ".exe" if os.name == "nt" else ""
mito = os.environ.get("MITO_EXE", os.path.join(here, "..", "..", "target", "release", "mito" + exe_suffix))  # MITO_EXE: another compiler, for A/B runs (MITO_BUILD_TAG keeps its builds apart)
if os.name == "nt":
    rust = os.environ.get("RUST_BENCH", os.path.join(here, "target-rust", "max-perf", "benchmarks.exe"))
    rust_checked = os.environ.get("RUST_CHECKED", os.path.join(here, "target-rust-checked", "max-perf", "benchmarks.exe"))
    crystal = os.environ.get("CRYSTAL", "D:/tools/langarena-crystal/target/bin_crystal.exe")
else: # Linux (WSL2 here): LangArena's Rust built under the home directory; Crystal when CRYSTAL names a build
    rust = os.environ.get("RUST_BENCH", os.path.expanduser("~/langarena-target/max-perf/benchmarks"))
    rust_checked = os.environ.get("RUST_CHECKED", os.path.expanduser("~/langarena-target-checked/max-perf/benchmarks"))
    crystal = os.environ.get("CRYSTAL", "")  # LangArena's original (Crystal) implementation, built with `crystal build main.cr --release`
arena = os.environ.get("LANGARENA", "D:/tmp/langarena" if os.name == "nt" else "/mnt/d/tmp/langarena")
build_dir = os.path.join(here, "build" if os.name == "nt" else "build-linux")
port = 18150

args = sys.argv[1:]
mode, rounds = "run", 3
if "--mode" in args: i = args.index("--mode"); mode = args[i + 1]; del args[i:i + 2]
if "--rounds" in args: i = args.index("--rounds"); rounds = int(args[i + 1]); del args[i:i + 2]
# --cflags "-O3 -DX ...": build the Mitochondria side with these clang flags instead of the default (measuring builds); --mito-only: skip the other implementations
cflags = []
if "--cflags" in args: i = args.index("--cflags"); cflags = args[i + 1].split(); del args[i:i + 2]
mito_only = "--mito-only" in args
if mito_only: args.remove("--mito-only")
config = {c["name"]: c for c in json.load(open(os.path.join(here, f"{mode}.js")))}
# The vendored run.js / test.js differ from LangArena's only in two test names (BF::Array, BF::Recursion). The reference binaries read
# LangArena's own files and print its names, so a test's name there is the one at the same position in LangArena's file.
def reference_names():
    ours = [c["name"] for c in json.load(open(os.path.join(here, f"{mode}.js")))]
    try: theirs = [c["name"] for c in json.load(open(os.path.join(arena, f"{mode}.js")))]
    except OSError: return {}
    return dict(zip(ours, theirs)) if len(ours) == len(theirs) else {}
ref_name = reference_names()
if "--write-only" in args: args.remove("--write-only"); write_only = True
else: write_only = False
tests = args or sorted(d for d in os.listdir(here) if os.path.isfile(os.path.join(here, d, "main.mito")))
if cflags: build_dir = os.path.join(here, "build", "flags-" + "_".join(f.strip("-").replace("=", "-") for f in cflags))
if os.environ.get("MITO_BUILD_TAG"): build_dir = os.path.join(build_dir, os.environ["MITO_BUILD_TAG"])
os.makedirs(build_dir, exist_ok=True)

# results.json keeps every implementation's latest numbers per test; RESULTS.md is written from it after each run
# one file per system: Windows' results.json / RESULTS.md, Linux's results-linux.json / RESULTS-linux.md
plat = "" if os.name == "nt" else "-linux"
results_path = os.path.join(here, f"results{plat}.json")
markdown_path = os.path.join(here, f"RESULTS{plat}.md")
results = json.load(open(results_path, encoding="utf-8")) if os.path.exists(results_path) else {}
def save_results():
    with open(results_path, "w", encoding="utf-8") as f: json.dump(results, f, indent=1, sort_keys=True)
def audit_of(test):
    try: return open(os.path.join(here, test, "audit"), encoding="utf-8").readline().strip()
    except OSError: return ""
def write_markdown():
    names = sorted(results, key=lambda n: n.lower())
    def val(n, k, f): x = results[n].get(k); return x.get(f) if x else None
    def num(x, d=3): return "" if x is None else f"{x:.{d}f}"
    def ratio(a, b): return "" if a is None or not b else f"{a / b:.2f}x"
    def gmean(xs): xs = [x for x in xs if x]; return math.exp(sum(math.log(x) for x in xs) / len(xs)) if xs else None
    fair = [n for n in names if audit_of(results[n]["test"]) in ("like-for-like", "rewritten-to-match")]
    # a column with no numbers at all (no Crystal on this machine, no checked Rust build) is left out
    has_c = any(val(n, "c", "s") for n in names); has_y = any(val(n, "y", "s") for n in names); has_ym = any(val(n, "y", "mb") for n in names)
    dates = sorted({v["date"] for n in names for k, v in results[n].items() if isinstance(v, dict) and "date" in v})
    o = ["# LangArena results: Mitochondria, Rust and Crystal" + ("" if os.name == "nt" else " (Linux)"), "",
         "Written by `run.py` (`python -I bench/langarena/run.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its "
         "latest run (" + (dates[0] + " to " + dates[-1] if len(dates) > 1 else (dates[0] if dates else "no runs")) + "): a run of only some implementations "
         "(`--mito-only`) keeps the others' last numbers. How the comparison is made, and each port's audit, are in [README.md](README.md).", "",
         f"Machine: {platform.system()} {platform.release()}, {platform.processor() or platform.machine()}. Run parameters (`run.js`), best of the rounds of a run, "
         "every checksum verified against LangArena's. Rust is LangArena's implementation (max-perf profile: opt-level 3, fat LTO, overflow checks off; the "
         "checked column turns them on, as Mitochondria always checks); Crystal is LangArena's original implementation. Ratios are Mitochondria / the column's implementation.", "",
         "Each table is sorted by its Mito/Rust ratio, best first.", "",
         "## Time (seconds for the timed iterations)", "",
         "| Workload | Mitochondria | Rust | Mito/Rust |" + (" Rust (checked) | Mito/Rust (checked) |" if has_c else "") + (" Crystal | Mito/Crystal |" if has_y else "") + " audit |",
         "|---|---:|---:|---:|" + ("---:|---:|" if has_c else "") + ("---:|---:|" if has_y else "") + "---|"]
    # best Mitochondria / Rust ratio first (ports without both numbers last)
    def by(field): return lambda n: (val(n, "m", field) / val(n, "r", field)) if val(n, "m", field) and val(n, "r", field) else float("inf")
    for n in sorted(names, key=by("s")):
        m, r, c, y = (val(n, k, "s") for k in "mrcy")
        o.append(f"| {n} | {num(m)} | {num(r)} | {ratio(m, r)} |" + (f" {num(c)} | {ratio(m, c)} |" if has_c else "") + (f" {num(y)} | {ratio(m, y)} |" if has_y else "") + f" {audit_of(results[n]['test'])} |")
    gr = gmean([val(n, "m", "s") / val(n, "r", "s") for n in fair if val(n, "m", "s") and val(n, "r", "s")])
    gc = gmean([val(n, "m", "s") / val(n, "c", "s") for n in fair if val(n, "m", "s") and val(n, "c", "s")])
    gy = gmean([val(n, "m", "s") / val(n, "y", "s") for n in fair if val(n, "m", "s") and val(n, "y", "s")])
    ga = gmean([val(n, "m", "s") / val(n, "r", "s") for n in names if val(n, "m", "s") and val(n, "r", "s")])
    o += ["", f"Geometric mean over the {len(fair)} like-for-like and rewritten-to-match ports: Mitochondria / Rust {num(gr, 3)}x"
          + (f", / Rust (checked) {num(gc, 3)}x" if has_c else "") + (f", / Crystal {num(gy, 3)}x" if has_y else "") + f"; over all {len(names)} ports, / Rust {num(ga, 3)}x.", "",
          "## Memory (peak, MB)", "",
          "The most physical memory each process held at once, measured by the operating system (" + ("Windows' peak working set" if os.name == "nt" else "Linux's peak resident set: VmHWM, or ru_maxrss for a process that ended") + "). The reference implementations run one test per process; "
          "Mitochondria is its server process for that test, measured after prepare, warmup and the timed run (its connection threads and request areas included; "
          "reserved address space that was never touched is not counted).", "",
          "| Workload | Mitochondria MB | Rust MB | Mito/Rust |" + (" Crystal MB | Mito/Crystal |" if has_ym else ""),
          "|---|---:|---:|---:|" + ("---:|---:|" if has_ym else "")]
    for n in sorted(names, key=by("mb")):
        m, r, y = val(n, "m", "mb"), val(n, "r", "mb"), val(n, "y", "mb")
        o.append(f"| {n} | {num(m, 1)} | {num(r, 1)} | {ratio(m, r)} |" + (f" {num(y, 1)} | {ratio(m, y)} |" if has_ym else ""))
    mr = gmean([val(n, "m", "mb") / val(n, "r", "mb") for n in names if val(n, "m", "mb") and val(n, "r", "mb")])
    my = gmean([val(n, "m", "mb") / val(n, "y", "mb") for n in names if val(n, "m", "mb") and val(n, "y", "mb")])
    o += ["", f"Geometric mean over all {len(names)} ports: Mitochondria / Rust {num(mr, 2)}x" + (f", / Crystal {num(my, 2)}x" if has_ym else "") + ".", ""]
    with open(markdown_path, "w", encoding="utf-8") as f: f.write("\n".join(o))
    print(f"wrote {markdown_path}", file=sys.stderr)


def checksum_bytes(data):
    h = 5381
    for b in data:
        h = (h * 33 + b) % 4294967296
    return h


# peak memory: a process's peak working set (the most physical memory it held at once), read from its handle, which stays
# open after the process ends while the Popen object lives
class _PMC(ctypes.Structure):
    _fields_ = [("cb", ctypes.c_uint32), ("PageFaultCount", ctypes.c_uint32), ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t), ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t), ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t)]
def peak_mb(p):
    if os.name != "nt":
        # Linux: a running process's peak resident set (VmHWM); one that ended reports it through wait4 (see run_rust)
        try:
            for line in open(f"/proc/{p.pid}/status"):
                if line.startswith("VmHWM:"): return int(line.split()[1]) / 1024
        except OSError: pass
        return None
    m = _PMC(); m.cb = ctypes.sizeof(_PMC)
    k32 = ctypes.windll.kernel32
    k32.K32GetProcessMemoryInfo.argtypes = [ctypes.c_void_p, ctypes.POINTER(_PMC), ctypes.c_uint32]
    if not k32.K32GetProcessMemoryInfo(ctypes.c_void_p(int(p._handle)), ctypes.byref(m), m.cb): return None
    return m.PeakWorkingSetSize / (1 << 20)


def checksum_f64(v):
    return checksum_bytes(("%.7f" % v).encode())


def call(method, path, body=None):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600)
    c.request(method, path, body=json.dumps(body) if body is not None else None, headers={"Content-Type": "application/json"} if body is not None else {}); r = c.getresponse(); body = r.read()
    if r.status >= 300: raise RuntimeError(f"{method} {path}: {r.status} {body[:200]}")
    h = r.getheader("X-Handler-Ns")
    return (int(h) / 1e9 if h else None), json.loads(body)


def build(test):
    out = os.path.join(build_dir, test + exe_suffix)
    src = os.path.join(here, test, "main.mito")
    if not os.path.exists(out) or os.path.getmtime(out) < max(os.path.getmtime(src), os.path.getmtime(mito)):
        r = subprocess.run([mito, "build", src, "-o", out] + [x for f in cflags for x in ("--cflag", f)], capture_output=True, text=True)
        if r.returncode: raise RuntimeError(f"{test}: build failed\n{r.stdout}{r.stderr}")
    return out


def prepare_path(cfg, params):
    return "/prepare" + "".join(f"/{cfg[p]}" for p in params)


def run_mito(test, name, exe, cfg, params, float_result):
    iters = cfg["iterations"]
    warm = cfg.get("warmup_iterations", max(int(iters * 0.2), 1))
    # a server left on the port by an earlier run would answer instead of this one: refuse to measure
    try: socket.create_connection(("127.0.0.1", port), 0.2).close(); raise RuntimeError(f"port {port} is already in use: stop the process listening there first")
    except OSError: pass
    p = subprocess.Popen([exe, "--port", str(port), "--timing", "--loop-budget", "1000000000000000"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); pin(p)
    try:
        time.sleep(0.05)
        if p.poll() is not None: raise RuntimeError(f"{exe} exited at start (port {port} in use?)")
        for _ in range(300):
            try: socket.create_connection(("127.0.0.1", port), 0.2).close(); break
            except OSError: time.sleep(0.05)
        if params == "words":
            last = 42
            def next_int(mx):
                nonlocal last
                last = (last * 3877 + 29573) % 139968
                return int(last / 139968.0 * mx)
            parts = []
            for _ in range(cfg["words"]):
                wl = next_int(cfg["word_len"]) + next_int(3) + 3
                parts.append("".join(chr(97 + next_int(26)) for _ in range(wl)))
            call("POST", "/prepare", {"text": " ".join(parts)})
        elif params == "program": call("POST", "/prepare", {"program": cfg["program"], "warmup": cfg["warmup_program"]})
        else: call("POST", prepare_path(cfg, params))
        call("POST", f"/warmup/{warm}")
        secs, _ = call("POST", f"/run/{iters}")
        _, res = call("GET", "/result" if float_result else "/checksum")
        if float_result == "matmul":      # the centre value is the same every run: the reference adds its hash once per run
            got = (checksum_f64(res["result"]) * (warm + iters)) % 4294967296
        elif float_result == "nbody":     # (hash(energy before) << 5) & hash(energy after), in 32 bits
            got = ((checksum_f64(res["v1"]) << 5) & 0xFFFFFFFF) & checksum_f64(res["v2"])
        elif float_result == "csv":       # the three averages are the same every run: (hash(x) + hash(y) + hash(z)) per run, plus the hash of the text
            got = (((checksum_f64(res["x"]) + checksum_f64(res["y"]) + checksum_f64(res["z"])) * (warm + iters)) + res["hash"]) % 4294967296
        elif float_result == "sum":       # the answer is one float, hashed once
            got = checksum_f64(res["result"])
        elif float_result == "spectral":  # one checksum, from sqrt(vBv / vv) of the final vectors
            got = checksum_f64(math.sqrt(res["result"]))
        else:
            got = res["checksum"]
        return secs, got == cfg["checksum"], got, peak_mb(p)
    finally:
        p.kill(); p.wait()


def run_rust(exe, name):
    name = ref_name.get(name, name)
    p = subprocess.Popen([exe, os.path.join(arena, f"{mode}.js"), name], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=os.path.dirname(exe)); pin(p)
    if os.name == "nt":
        out = p.communicate()[0]
        mem = peak_mb(p)
    else:
        # Linux: read the output, then reap the process with wait4 for its peak resident set (ru_maxrss, KB)
        out = p.stdout.read(); p.stderr.read()
        _, status, ru = os.wait4(p.pid, 0)
        p.returncode = os.waitstatus_to_exitcode(status)
        mem = ru.ru_maxrss / 1024
    m = re.search(rf"{re.escape(name)}: (OK|ERR\[[^\]]*\]) in ([0-9.]+)s", out)
    if not m: return None, False, mem
    return float(m.group(2)), m.group(1) == "OK", mem


# per test: the config keys /prepare takes, and whether the checksum comes from a float
META = {
    "etc_sieve": (["limit"], False),
    "clbg_fannkuchredux": (["n"], False),
    "sort_quick": (["size"], False),
    "sort_merge": (["size"], False),
    "matmul_single": (["n"], "matmul"),
    "clbg_spectralnorm": (["size"], "spectral"),
    "clbg_mandelbrot": (["w", "h"], False),
    "clbg_nbody": ([], "nbody"),
    "hash_crc32": (["size"], False),
    "hash_sha256": (["size"], False),
    "etc_gameoflife": (["w", "h"], False),
    "graph_bfs": (["vertices", "jumps", "jump_len"], False),
    "graph_dfs": (["vertices", "jumps", "jump_len"], False),
    "graph_astar": (["vertices", "jumps", "jump_len"], False),
    "maze_generator": (["w", "h"], False),
    "maze_bfs": (["w", "h"], False),
    "maze_astar": (["w", "h"], False),
    "binarytrees_obj": (["depth"], False),
    "binarytrees_arena": (["depth"], False),
    "etc_neuralnet": ([], "sum"),
    "etc_textraytracer": (["w", "h"], False),
    "compress_bwtencode": (["size"], False),
    "compress_bwtdecode": (["size"], False),
    "compress_huffencode": (["size"], False),
    "compress_huffdecode": (["size"], False),
    "compress_arithencode": (["size"], False),
    "compress_arithdecode": (["size"], False),
    "compress_lzwencode": (["size"], False),
    "compress_lzwdecode": (["size"], False),
    "distance_jaro": (["count", "size"], False),
    "distance_ngram": (["count", "size"], False),
    "template_parse": (["count"], False),
    "calculator_interpreter": (["operations"], False),
    "csv_parse": (["rows"], "csv"),
    "etc_words": ("words", False),
    "etc_cachesimulation": (["values", "size"], False),
    "calculator_ast": (["operations"], False),
    "base64_encode": (["size"], False),
    "base64_decode": (["size"], False),
    "bf_array": ("program", False),
    "bf_recursion": ("program", False),
}
rows = []
if write_only: write_markdown(); sys.exit(0) # --write-only: RESULTS.md from results.json, nothing measured
for test in tests:
    name = open(os.path.join(here, test, "name")).read().strip()
    params, floaty = META[test]
    cfg = config[name]
    exe = build(test)
    M, R, RC, Y, ok_m, ok_r, ok_rc, ok_y = [], [], [], [], True, True, True, True
    MEM = {"m": [], "r": [], "c": [], "y": []}
    for r in range(rounds):
        order = ["m"] if mito_only else ["m", "r", "c", "y"] if r % 2 == 0 else ["y", "c", "r", "m"]
        for who in order:
            if who == "m":
                s, ok, got, mem = run_mito(test, name, exe, cfg, params, floaty); M.append(s); ok_m &= ok; MEM["m"].append(mem)
                if not ok: print(f"  {name}: Mitochondria checksum {got}, expected {cfg['checksum']}", file=sys.stderr)
            elif who == "r":
                s, ok, mem = run_rust(rust, name); R.append(s); ok_r &= ok; MEM["r"].append(mem)
            elif who == "c" and os.path.exists(rust_checked):
                s, ok, mem = run_rust(rust_checked, name); RC.append(s); ok_rc &= ok; MEM["c"].append(mem)
            elif who == "y" and os.path.exists(crystal):
                s, ok, mem = run_rust(crystal, name); Y.append(s); ok_y &= ok; MEM["y"].append(mem)
    rows.append((name, M, R, RC, Y, ok_m and ok_r and ok_y))
    # the results file: each implementation's best time and peak memory, replaced only for the implementations measured now
    if mode == "run":
        entry = results.setdefault(name, {"test": test})
        stamp = datetime.date.today().isoformat()
        for key, times, oks in (("m", M, ok_m), ("r", R, ok_r), ("c", RC, ok_rc), ("y", Y, ok_y)):
            if times and None not in times:
                mems = [x for x in MEM[key] if x is not None]
                entry[key] = {"s": min(times), "mb": min(mems) if mems else None, "verified": oks, "date": stamp}
        save_results()
    print(f"  {name} done", file=sys.stderr)

if mode == "run": write_markdown()

for label, agg in (("best of rounds", min), ("median of rounds", statistics.median)):
    print(f"\n{label} ({mode} parameters, {rounds} rounds, seconds for the timed iterations, ratio = Mitochondria / Rust):")
    print(f"{'Workload':<26}{'Mitochondria s':>16}{'Rust (reference) s':>20}{'Mito/Rust':>11}{'Rust, overflow checks s':>25}{'Mito/Rust checked':>19}{'Crystal (original) s':>22}{'Mito/Crystal':>14}  checksums")
    for name, M, R, RC, Y, ok in rows:
        m, r = agg(M), (agg(R) if R else 0)
        ratio = lambda a, b: f"{a / b:.2f}x" if b > 0 else "n/a"
        rc = f"{agg(RC):.3f}" if RC and None not in RC else "n/a"
        rcr = ratio(m, agg(RC)) if RC and None not in RC else "n/a"
        cy = f"{agg(Y):.3f}" if Y and None not in Y else "n/a"
        cyr = ratio(m, agg(Y)) if Y and None not in Y else "n/a"
        print(f"{name:<26}{m:>16.3f}{r:>20.3f}{ratio(m, r):>11}{rc:>25}{rcr:>19}{cy:>22}{cyr:>14}  {'verified' if ok else 'MISMATCH'}")
