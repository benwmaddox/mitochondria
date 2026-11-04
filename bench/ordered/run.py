"""Alternates the Mito ordered benchmark and the Rust BTreeMap baseline, same n and ops, several rounds.

usage: python -I run.py N OPS ROUNDS [mito.exe]
Each round starts a fresh Mito server, runs the workloads as one request each (timed here; a request's connection cost
is under a millisecond against seconds of work), then runs the Rust baseline. Prints the median ns/op per workload and
memory. The Mito exe is built with `mitochondria build bench/ordered/ordered.mito -o bench/ordered/ordered.exe`.
"""
import http.client, json, statistics, subprocess, sys, time, os

here = os.path.dirname(os.path.abspath(__file__))
n, ops, rounds = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
mito = os.path.abspath(sys.argv[4]) if len(sys.argv) > 4 else os.path.join(here, "ordered.exe")
rust = os.path.join(here, "..", "btree-rust", "target", "release", "btree-rust.exe")
port = 18080


def call(method, path):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600)
    t = time.perf_counter()
    c.request(method, path)
    r = c.getresponse()
    body = r.read()
    dt = (int(r.getheader("X-Handler-Ns")) / 1e9) if r.getheader("X-Handler-Ns") else time.perf_counter() - t
    if r.status >= 300:
        raise SystemExit(f"{method} {path}: {r.status} {body[:200]}")
    return dt, json.loads(body)


def private_kb(pid):
    out = subprocess.run(["powershell", "-NoProfile", "-Command", f"(Get-Process -Id {pid}).PrivateMemorySize64 / 1024"], capture_output=True, text=True).stdout
    return int(float(out.strip()))


def run_mito(extra=()):
    p = subprocess.Popen([mito, "--port", str(port), "--timing", *extra], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            try:
                call("GET", "/stats"); break
            except OSError:
                time.sleep(0.05)
        res = {}
        res["insert"] = call("POST", f"/insert/{n}")[0] * 1e9 / n
        st = call("GET", "/stats")[1]
        res["committed_kb"] = st["committed_kb"]; res["live_kb"] = st["live_kb"]; res["private_kb"] = private_kb(p.pid)
        res["hit"] = call("GET", f"/hit/{n}/{ops}")[0] * 1e9 / ops
        res["miss"] = call("GET", f"/miss/{n}/{ops}")[0] * 1e9 / ops
        res["mixed"] = call("GET", f"/mixed/{n}/{ops}")[0] * 1e9 / ops
        res["increment"] = call("POST", f"/increment/{n}/{ops}")[0] * 1e9 / ops
        res["range100"] = call("GET", f"/range/{n}/{ops // 10}")[0] * 1e9 / (ops // 10)
        x, done, spent = 20261104, 0, 0.0
        while done < n:
            c = min(16384, n - done)
            dt, a = call("DELETE", f"/delete/{x}/{c}")
            x, done, spent = a["x"], done + c, spent + dt
        res["delete"] = spent * 1e9 / n
        if call("GET", f"/hit/{n}/{min(n, 1000)}")[1]["sum"] != 0: raise SystemExit("delete left items behind")
        return res
    finally:
        p.kill(); p.wait()


def run_rust():
    out = subprocess.run([rust, str(n), str(ops)], capture_output=True, text=True, check=True).stdout
    return {k: float(v) for k, v in (l.split("\t") for l in out.strip().splitlines())}


M, R = [], []
for i in range(rounds):
    # alternate who goes first so a drifting machine load hits both alike
    if i % 2 == 0:
        M.append(run_mito()); R.append(run_rust())
    else:
        R.append(run_rust()); M.append(run_mito())
    print(f"round {i + 1} done", file=sys.stderr)

names = ["insert", "hit", "miss", "mixed", "increment", "range100", "delete"]
print(f"n={n} ops={ops} rounds={rounds} (median ns/op; range100 is per scan of up to 100 entries)")
print(f"{'workload':<12}{'Mito':>10}{'Rust':>10}{'Mito/Rust':>11}   rounds (Mito | Rust)")
for k in names:
    m = statistics.median(r[k] for r in M); r_ = statistics.median(r[k] for r in R)
    print(f"{k:<12}{m:>10.1f}{r_:>10.1f}{m / r_:>11.2f}   " + " ".join(f"{r[k]:.0f}" for r in M) + " | " + " ".join(f"{r[k]:.0f}" for r in R))
print(f"memory after insert, KB: Mito committed {statistics.median(r['committed_kb'] for r in M):.0f}, live {statistics.median(r['live_kb'] for r in M):.0f}, process private {statistics.median(r['private_kb'] for r in M):.0f}; Rust private {statistics.median(r['private_kb_after_insert'] for r in R):.0f}")


def memory_phases(extra):
    """Live vs committed KB after inserting n, then after removing all n (logical removal), with or without a trim policy."""
    p = subprocess.Popen([mito, "--port", str(port), "--timing", *extra], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            try:
                call("GET", "/stats"); break
            except OSError:
                time.sleep(0.05)
        call("POST", f"/insert/{n}")
        full = call("GET", "/stats")[1]
        x, done = 20261104, 0
        while done < n:
            c = min(16384, n - done)
            x = call("DELETE", f"/delete/{x}/{c}")[1]["x"]; done += c
        out = [full]
        for _ in range(3):
            out.append(call("GET", "/stats")[1])
        return out
    finally:
        p.kill(); p.wait()


def show(label, rows):
    print(f"  {label}: after insert committed {rows[0]['committed_kb']} / live {rows[0]['live_kb']}; after removing all: " + ", ".join(f"committed {r['committed_kb']} / live {r['live_kb']}" for r in rows[1:]))


if os.environ.get("MITO_MEMORY_PHASES"):
    print("memory (KB), remove = logical (slots free, pages stay committed); trim = pages given back to the OS")
    show("no trim policy", memory_phases(()))
    show("trim policy (--soft 1 MB)", memory_phases(("--soft", "1", "--hard", "100000")))
