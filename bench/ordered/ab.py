"""Alternates two Mito builds of the ordered benchmark (and Rust), round by round, so shared-machine load hits all alike.

usage: python -I ab.py N OPS ROUNDS exeA exeB [exeC ...]
Prints the median ns/op per workload for every exe and for the Rust baseline, and each exe's ratio to Rust."""
import http.client, json, os, statistics, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pin import pin

here = os.path.dirname(os.path.abspath(__file__))
n, ops, rounds = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
exes = [os.path.abspath(e) for e in sys.argv[4:]]
rust = os.path.join(here, "..", "btree-rust", "target", "release", "btree-rust.exe")
port = 18085
names = ["insert", "hit", "miss", "mixed", "increment", "range100", "delete"]


def call(method, path):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600)
    t = time.perf_counter(); c.request(method, path); r = c.getresponse(); body = r.read(); dt = (int(r.getheader("X-Handler-Ns")) / 1e9) if r.getheader("X-Handler-Ns") else time.perf_counter() - t
    if r.status >= 300: raise SystemExit(f"{method} {path}: {r.status} {body[:200]}")
    return dt, json.loads(body)


def run_mito(exe):
    p = subprocess.Popen([exe, "--port", str(port), "--timing"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    pin(p)
    try:
        for _ in range(200):
            try: call("GET", "/stats"); break
            except OSError: time.sleep(0.05)
        r = {}
        r["insert"] = call("POST", f"/insert/{n}")[0] * 1e9 / n
        for name, path in (("hit", "hit"), ("miss", "miss"), ("mixed", "mixed")):
            r[name] = call("GET", f"/{path}/{n}/{ops}")[0] * 1e9 / ops
        r["increment"] = call("POST", f"/increment/{n}/{ops}")[0] * 1e9 / ops
        r["range100"] = call("GET", f"/range/{n}/{ops // 10}")[0] * 1e9 / (ops // 10)
        x, done, spent = 20261104, 0, 0.0
        while done < n:
            c = min(16384, n - done); dt, a = call("DELETE", f"/delete/{x}/{c}"); x, done, spent = a["x"], done + c, spent + dt
        r["delete"] = spent * 1e9 / n
        if call("GET", f"/hit/{n}/{min(n, 1000)}")[1]["sum"] != 0: raise SystemExit("delete left items behind")
        return r
    finally:
        p.kill(); p.wait()


def run_rust():
    rp = subprocess.Popen([rust, str(n), str(ops)], stdout=subprocess.PIPE, text=True)
    pin(rp)
    out = rp.communicate()[0]
    return {k: float(v) for k, v in (l.split("\t") for l in out.strip().splitlines())}


res = {e: [] for e in exes}; R = []
for i in range(rounds):
    order = exes if i % 2 == 0 else exes[::-1]
    for e in order: res[e].append(run_mito(e))
    R.append(run_rust())
    print(f"round {i + 1} done", file=sys.stderr)
def table(title, agg):
    print(title)
    print(f"{'workload':<11}{'Rust':>8}" + "".join(f"{os.path.basename(e)[:14]:>20}" for e in exes))
    for k in names:
        rv = agg(R, k)
        print(f"{k:<11}{rv:>8.1f}" + "".join(f"{agg(res[e], k):>12.1f} ({agg(res[e], k) / rv:4.2f}x)" for e in exes))
print(f"n={n} ops={ops} rounds={rounds}; ns/op, and the ratio to Rust in brackets (range100 is per scan of up to 100 entries)")
table("best of rounds (the minimum filters load from other work on the machine):", lambda rows, k: min(r[k] for r in rows))
table("median of rounds:", lambda rows, k: statistics.median(r[k] for r in rows))
