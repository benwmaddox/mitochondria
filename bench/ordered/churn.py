"""Steady-state churn on the Mito ordered programs, against Rust: does speed or memory drift with use?

usage: python -I churn.py W C ROUNDS exe [exe ...] [-- server flags]
A FIFO window of W live keys. Each round runs C steps; a step deletes the oldest key and inserts the next new one (a random
position in the order). After every round: ns per step, ns per lookup of the live keys, the program's committed and live KB,
and (B-tree builds) leaf and inner node counts. Rust's churn mode runs the same sequence for comparison.
Every program's first line of output is the window filled with no churn yet, so growth over the rounds reads directly."""
import http.client, json, os, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pin import pin

here = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
flags = []
if "--" in args:
    i = args.index("--"); flags = args[i + 1:]; args = args[:i]
w, c, rounds = int(args[0]), int(args[1]), int(args[2])
exes = [os.path.abspath(e) for e in args[3:]]
rust = os.path.join(here, "..", "btree-rust", "target", "release", "btree-rust.exe")
port = 18086
SEED = 20261104


def call(method, path):
    cn = http.client.HTTPConnection("127.0.0.1", port, timeout=3600)
    t = time.perf_counter(); cn.request(method, path); r = cn.getresponse(); body = r.read(); dt = (int(r.getheader("X-Handler-Ns")) / 1e9) if r.getheader("X-Handler-Ns") else time.perf_counter() - t
    if r.status >= 300: raise SystemExit(f"{method} {path}: {r.status} {body[:200]}")
    return dt, json.loads(body)


def step(v): return v * 48271 % 2147483647


def run(exe):
    p = subprocess.Popen([exe, "--port", str(port), "--timing", *flags], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    pin(p)
    rows = []
    try:
        for _ in range(200):
            try: call("GET", "/stats"); break
            except OSError: time.sleep(0.05)
        call("POST", f"/insert/{w}")
        x = SEED
        for _ in range(w): x = step(x)
        y = SEED
        for r in range(0, rounds + 1):
            ns = 0.0
            if r > 0:
                spent, done = 0.0, 0
                while done < c:
                    k = min(8192, c - done)
                    dt, a = call("POST", f"/churn/{x}/{y}/{k}")
                    x, y, spent, done = a["x"], a["y"], spent + dt, done + k
                ns = spent * 1e9 / c
            ops = 100000
            look = min(call("GET", f"/probe/{y}/{ops}")[0] for _ in range(3)) * 1e9 / ops  # best of 3: load from other work comes and goes
            st = call("GET", "/stats")[1]
            shape = ""
            try: sh = call("GET", "/shape")[1]; shape = f"{sh['leaves']}/{sh['inners']}/h{sh['height']}"
            except SystemExit: pass
            rows.append((r, ns, look, st["committed_kb"], st["live_kb"], shape))
        return rows
    finally:
        p.kill(); p.wait()


def show(name, rows, rust_rows=None):
    print(f"\n== {name}   (round 0 = filled, no churn yet)")
    print(f"{'round':>5}{'ns/step':>10}{'ns/lookup':>11}{'committed KB':>14}{'live KB':>9}{'leaves/inner/height':>20}")
    keep = set(range(0, rounds + 1)) if os.environ.get('CHURN_ALL') else {0, 1, 2, 3, 5, 10, 20, 30, 40, rounds}
    for r, ns, look, com, live, shape in rows:
        if r in keep or r == len(rows) - 1: print(f"{r:>5}{ns:>10.0f}{look:>11.0f}{com:>14}{live:>9}{shape:>20}")
    first, last = rows[1], rows[-1]
    print(f"  after {rounds} rounds ({rounds * c} steps, each key replaced {rounds * c // w}x): step {first[1]:.0f} -> {last[1]:.0f} ns, lookup {rows[0][2]:.0f} -> {last[2]:.0f} ns, committed {rows[0][3]} -> {last[3]} KB")


rp = subprocess.Popen([rust, "churn", str(w), str(c), str(rounds)], stdout=subprocess.PIPE, text=True)
pin(rp)
out = rp.communicate()[0]
print(f"== Rust BTreeMap: window {w}, {c} steps per round")
print(f"{'round':>5}{'ns/step':>10}{'ns/lookup':>11}{'private KB':>12}")
rr = [l.split("\t") for l in out.strip().splitlines()]
for r in rr:
    if int(r[0]) in (1, 2, 3, 5, 10, 20, 30, 40, rounds): print(f"{r[0]:>5}{float(r[1]):>10.0f}{float(r[2]):>11.0f}{r[3]:>12}")
for e in exes:
    show(os.path.basename(e), run(e))
