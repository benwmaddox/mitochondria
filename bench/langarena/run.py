"""Mitochondria ports of LangArena tests against LangArena's own Rust implementation, same machine, same core, alternating rounds.

usage: python -I run.py [--mode run|test] [--rounds N] [test_dir ...]      e.g.  python -I run.py --rounds 3 etc_sieve sort_quick

Methodology (LangArena's, kostya/LangArena, MIT):
  - the parameters are run.js (or test.js with --mode test), vendored unchanged;
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
import http.client, json, math, os, re, socket, statistics, subprocess, sys, time

here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(here, "..", "ordered"))
from pin import pin

mito = os.path.join(here, "..", "..", "target", "release", "mitochondria.exe")
rust = os.environ.get("RUST_BENCH", os.path.join(here, "target-rust", "max-perf", "benchmarks.exe"))
rust_checked = os.environ.get("RUST_CHECKED", os.path.join(here, "target-rust-checked", "max-perf", "benchmarks.exe"))
crystal = os.environ.get("CRYSTAL", "D:/tools/langarena-crystal/target/bin_crystal.exe")  # LangArena's original (Crystal) implementation, built with `crystal build main.cr --release`
arena = os.environ.get("LANGARENA", "D:/tmp/langarena")
build_dir = os.path.join(here, "build")
port = 18150

args = sys.argv[1:]
mode, rounds = "run", 3
if "--mode" in args: i = args.index("--mode"); mode = args[i + 1]; del args[i:i + 2]
if "--rounds" in args: i = args.index("--rounds"); rounds = int(args[i + 1]); del args[i:i + 2]
config = {c["name"]: c for c in json.load(open(os.path.join(here, f"{mode}.js")))}
tests = args or sorted(d for d in os.listdir(here) if os.path.isfile(os.path.join(here, d, "main.mito")))
os.makedirs(build_dir, exist_ok=True)


def checksum_bytes(data):
    h = 5381
    for b in data:
        h = (h * 33 + b) % 4294967296
    return h


def checksum_f64(v):
    return checksum_bytes(("%.7f" % v).encode())


def call(method, path, body=None):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600)
    c.request(method, path, body=json.dumps(body) if body is not None else None, headers={"Content-Type": "application/json"} if body is not None else {}); r = c.getresponse(); body = r.read()
    if r.status >= 300: raise RuntimeError(f"{method} {path}: {r.status} {body[:200]}")
    h = r.getheader("X-Handler-Ns")
    return (int(h) / 1e9 if h else None), json.loads(body)


def build(test):
    out = os.path.join(build_dir, test + ".exe")
    src = os.path.join(here, test, "main.mito")
    if not os.path.exists(out) or os.path.getmtime(out) < max(os.path.getmtime(src), os.path.getmtime(mito)):
        r = subprocess.run([mito, "build", src, "-o", out], capture_output=True, text=True)
        if r.returncode: raise RuntimeError(f"{test}: build failed\n{r.stdout}{r.stderr}")
    return out


def prepare_path(cfg, params):
    return "/prepare" + "".join(f"/{cfg[p]}" for p in params)


def run_mito(test, name, exe, cfg, params, float_result):
    iters = cfg["iterations"]
    warm = cfg.get("warmup_iterations", max(int(iters * 0.2), 1))
    p = subprocess.Popen([exe, "--port", str(port), "--timing"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); pin(p)
    try:
        for _ in range(300):
            try: socket.create_connection(("127.0.0.1", port), 0.2).close(); break
            except OSError: time.sleep(0.05)
        if params == "program": call("POST", "/prepare", {"program": cfg["program"], "warmup": cfg["warmup_program"]})
        else: call("POST", prepare_path(cfg, params))
        call("POST", f"/warmup/{warm}")
        secs, _ = call("POST", f"/run/{iters}")
        _, res = call("GET", "/result" if float_result else "/checksum")
        if float_result == "matmul":      # the centre value is the same every run: the reference adds its hash once per run
            got = (checksum_f64(res["result"]) * (warm + iters)) % 4294967296
        elif float_result == "nbody":     # (hash(energy before) << 5) & hash(energy after), in 32 bits
            got = ((checksum_f64(res["v1"]) << 5) & 0xFFFFFFFF) & checksum_f64(res["v2"])
        elif float_result == "b64enc":    # the reference hashes "encode aaaa... to <first four encoded>...: <sum>"
            got = checksum_bytes(f"encode aaaa... to {''.join(chr(res['encoded' + str(k)]) for k in range(4))}...: {res['result']}".encode())
        elif float_result == "b64dec":    # "decode <first four encoded>... to aaaa...: <sum>"
            got = checksum_bytes(f"decode {''.join(chr(res['encoded' + str(k)]) for k in range(4))}... to {''.join(chr(res['decoded' + str(k)]) for k in range(4))}...: {res['result']}".encode())
        elif float_result == "sum":       # the answer is one float, hashed once
            got = checksum_f64(res["result"])
        elif float_result == "spectral":  # one checksum, from sqrt(vBv / vv) of the final vectors
            got = checksum_f64(math.sqrt(res["result"]))
        else:
            got = res["checksum"]
        return secs, got == cfg["checksum"], got
    finally:
        p.kill(); p.wait()


def run_rust(exe, name):
    p = subprocess.Popen([exe, os.path.join(arena, f"{mode}.js"), name], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=os.path.dirname(exe)); pin(p)
    out = p.communicate()[0]
    m = re.search(rf"{re.escape(name)}: (OK|ERR\[[^\]]*\]) in ([0-9.]+)s", out)
    if not m: return None, False
    return float(m.group(2)), m.group(1) == "OK"


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
    "calculator_ast": (["operations"], False),
    "base64_encode": (["size"], "b64enc"),
    "base64_decode": (["size"], "b64dec"),
    "brainfuck_array": ("program", False),
    "brainfuck_recursion": ("program", False),
}
rows = []
for test in tests:
    name = open(os.path.join(here, test, "name")).read().strip()
    params, floaty = META[test]
    cfg = config[name]
    exe = build(test)
    M, R, RC, Y, ok_m, ok_r, ok_rc, ok_y = [], [], [], [], True, True, True, True
    for r in range(rounds):
        order = ["m", "r", "c", "y"] if r % 2 == 0 else ["y", "c", "r", "m"]
        for who in order:
            if who == "m":
                s, ok, got = run_mito(test, name, exe, cfg, params, floaty); M.append(s); ok_m &= ok
                if not ok: print(f"  {name}: Mitochondria checksum {got}, expected {cfg['checksum']}", file=sys.stderr)
            elif who == "r":
                s, ok = run_rust(rust, name); R.append(s); ok_r &= ok
            elif who == "c" and os.path.exists(rust_checked):
                s, ok = run_rust(rust_checked, name); RC.append(s); ok_rc &= ok
            elif who == "y" and os.path.exists(crystal):
                s, ok = run_rust(crystal, name); Y.append(s); ok_y &= ok
    rows.append((name, M, R, RC, Y, ok_m and ok_r and ok_y))
    print(f"  {name} done", file=sys.stderr)

for label, agg in (("best of rounds", min), ("median of rounds", statistics.median)):
    print(f"\n{label} ({mode} parameters, {rounds} rounds, seconds for the timed iterations, ratio = Mitochondria / Rust):")
    print(f"{'Workload':<26}{'Mitochondria s':>16}{'Rust (reference) s':>20}{'Mito/Rust':>11}{'Rust, overflow checks s':>25}{'Mito/Rust checked':>19}{'Crystal (original) s':>22}{'Mito/Crystal':>14}  checksums")
    for name, M, R, RC, Y, ok in rows:
        m, r = agg(M), agg(R)
        ratio = lambda a, b: f"{a / b:.2f}x" if b > 0 else "n/a"
        rc = f"{agg(RC):.3f}" if RC and None not in RC else "n/a"
        rcr = ratio(m, agg(RC)) if RC and None not in RC else "n/a"
        cy = f"{agg(Y):.3f}" if Y and None not in Y else "n/a"
        cyr = ratio(m, agg(Y)) if Y and None not in Y else "n/a"
        print(f"{name:<26}{m:>16.3f}{r:>20.3f}{ratio(m, r):>11}{rc:>25}{rcr:>19}{cy:>22}{cyr:>14}  {'verified' if ok else 'MISMATCH'}")
