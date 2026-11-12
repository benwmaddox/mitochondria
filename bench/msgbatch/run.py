"""Message batch benchmark: Mitochondria vs Rust vs Go, same pinned core, alternating rounds, checksum verified on every run.
usage: python -I run.py [--rounds N] [--iters N]   (build rust/ with cargo build --release and go/ with go build -o msgbatch.exe first)"""
import http.client, json, os, socket, statistics, subprocess, sys, time
here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(here, "..", "ordered"))
from pin import pin
exe = ".exe" if os.name == "nt" else ""
mito = os.environ.get("MITO_EXE", os.path.join(here, "..", "..", "target", "release", "mito" + exe))
rounds = int(sys.argv[sys.argv.index("--rounds") + 1]) if "--rounds" in sys.argv else 5
iters = int(sys.argv[sys.argv.index("--iters") + 1]) if "--iters" in sys.argv else 500000
port = 18170
out = os.path.join(here, "build"); os.makedirs(out, exist_ok=True)
server = os.path.join(out, "msgbatch" + exe)
subprocess.run([mito, "build", os.path.join(here, "main.mito"), "-o", server], check=True)
# GOOSE_EXE: the Goose compiler (build it from github.com/aardappel/goose); GOOSE_CC: its C compiler (--cc, e.g. clang); RUST_EXE / GO_EXE override the Rust and Go binaries
goose_exe = os.environ.get("GOOSE_EXE")
goose_bin = os.path.join(here, "goose", "msgbatch" + exe)
if goose_exe:
    subprocess.run([goose_exe, "build", os.path.join(here, "goose", "main.goose"), "-o", goose_bin] + (["--cc", os.environ["GOOSE_CC"]] if os.environ.get("GOOSE_CC") else []), check=True)

def call(method, path):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600)
    c.request(method, path); r = c.getresponse(); body = r.read()
    if r.status >= 300: raise RuntimeError(f"{method} {path}: {r.status} {body[:200]}")
    h = r.getheader("X-Handler-Ns")
    return (int(h) / 1e9 if h else None), json.loads(body)

mem = {}
def run_mito():
    p = subprocess.Popen([server, "--port", str(port), "--timing", "--loop-budget", "1000000000000000"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); pin(p)
    try:
        for _ in range(300):
            try: socket.create_connection(("127.0.0.1", port), 0.2).close(); break
            except OSError: time.sleep(0.05)
        call("POST", "/prepare"); call("POST", f"/warmup/{max(iters // 5, 1)}")
        secs, _ = call("POST", f"/run/{iters}")
        r = call("GET", "/checksum")[1]
        mem.setdefault("Mitochondria", []).append(vmhwm(p.pid))
        return secs, r["checksum"], (r["rejected"], r["skipped"], r["copied"])
    finally: p.kill(); p.wait()

def vmhwm(pid):  # Linux: peak resident set (MB) of a running process
    try:
        for l in open(f"/proc/{pid}/status"):
            if l.startswith("VmHWM:"): return int(l.split()[1]) / 1024
    except OSError: pass

def run_ref(path, name):
    p = subprocess.Popen([path, str(iters)], stdout=subprocess.PIPE, text=True); pin(p)
    last = None
    if os.name != "nt":  # Linux: the program allocates once at startup, so the last VmHWM read while it runs is its peak resident set (ru_maxrss would inherit this Python's own peak)
        while p.poll() is None:
            m = vmhwm(p.pid); last = m if m is not None else last; time.sleep(0.02)
        mem.setdefault(name, []).append(last)
    text = p.stdout.read(); p.wait()
    s, c, *n = text.split()
    return float(s), int(c), tuple(int(x) for x in n)

impls = {"Mitochondria": run_mito,
         "Rust": lambda: run_ref(os.environ.get("RUST_EXE", os.path.join(here, "rust", "target", "release", "msgbatch" + exe)), "Rust"),
         "Go": lambda: run_ref(os.environ.get("GO_EXE", os.path.join(here, "go", "msgbatch" + exe)), "Go")}
if os.path.exists(goose_bin):
    impls["Goose"] = lambda: run_ref(goose_bin, "Goose")
times = {k: [] for k in impls}; sums = set()
for r in range(rounds):
    for k, f in impls.items():
        s, c, n = f(); times[k].append(s); sums.add(c)
        assert n == (24, 64, 168), f"{k}: rejected/skipped/copied {n}, expected (24, 64, 168)"
        print(f"round {r + 1} {k}: {s:.3f}s checksum {c}", file=sys.stderr)
print(f"checksums: {sorted(sums)} ({'all equal' if len(sums) == 1 else 'MISMATCH'})")
best = {k: min(v) for k, v in times.items()}
print(f"batches: {iters} (256 messages each, +{max(iters // 5, 1)} warmup), best of {rounds} rounds")
for k, v in times.items():
    print(f"{k:13} best {min(v):.3f}s  median {statistics.median(v):.3f}s  per batch {min(v) / iters * 1e6:.2f} us  vs Rust {min(v) / best['Rust']:.2f}x")
if mem: print("peak RSS MB (max over rounds): " + ", ".join(f"{k} {max(v):.1f}" for k, v in mem.items()))
