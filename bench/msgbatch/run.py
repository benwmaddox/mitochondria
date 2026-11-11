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

def call(method, path):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600)
    c.request(method, path); r = c.getresponse(); body = r.read()
    if r.status >= 300: raise RuntimeError(f"{method} {path}: {r.status} {body[:200]}")
    h = r.getheader("X-Handler-Ns")
    return (int(h) / 1e9 if h else None), json.loads(body)

def run_mito():
    p = subprocess.Popen([server, "--port", str(port), "--timing", "--loop-budget", "1000000000000000"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); pin(p)
    try:
        for _ in range(300):
            try: socket.create_connection(("127.0.0.1", port), 0.2).close(); break
            except OSError: time.sleep(0.05)
        call("POST", "/prepare"); call("POST", f"/warmup/{max(iters // 5, 1)}")
        secs, _ = call("POST", f"/run/{iters}")
        r = call("GET", "/checksum")[1]
        return secs, r["checksum"], (r["rejected"], r["skipped"], r["copied"])
    finally: p.kill(); p.wait()

def run_ref(path):
    p = subprocess.Popen([path, str(iters)], stdout=subprocess.PIPE, text=True); pin(p)
    s, c, *n = p.communicate()[0].split()
    return float(s), int(c), tuple(int(x) for x in n)

impls = {"Mitochondria": run_mito,
         "Rust": lambda: run_ref(os.path.join(here, "rust", "target", "release", "msgbatch" + exe)),
         "Go": lambda: run_ref(os.path.join(here, "go", "msgbatch" + exe))}
# Goose: `goose build goose/main.goose -o goose/msgbatch` (GOOSE_EXE names the compiler); left out until that binary exists
if os.path.exists(os.path.join(here, "goose", "msgbatch" + exe)):
    impls["Goose"] = lambda: run_ref(os.path.join(here, "goose", "msgbatch" + exe))
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
