"""Does lookup speed drift because of use? Churn a window, then load the same live keys into a fresh process and compare.

usage: python -I fresh.py W C ROUNDS exe     (probes use best of 3)"""
import http.client, json, os, subprocess, sys, time
w, c, rounds, exe = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3]), os.path.abspath(sys.argv[4])
port = 18087
def call(m, p):
    cn = http.client.HTTPConnection("127.0.0.1", port, timeout=3600); t = time.perf_counter(); cn.request(m, p); r = cn.getresponse(); b = r.read(); dt = time.perf_counter() - t
    if r.status >= 300: raise SystemExit(f"{m} {p}: {r.status} {b[:200]}")
    return dt, json.loads(b)
def start():
    p = subprocess.Popen([exe, "--port", str(port)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(200):
        try: call("GET", "/stats"); break
        except OSError: time.sleep(0.05)
    return p
def probe(y, label):
    ops = 100000
    best = min(call("GET", f"/probe/{y}/{ops}")[0] for _ in range(5)) * 1e9 / ops
    print(f"  {label:<46}{best:7.0f} ns/lookup", flush=True)
step = lambda v: v * 48271 % 2147483647
A = start()
try:
    call("POST", f"/insert/{w}")
    x = y = 20261104
    for _ in range(w): x = step(x)
    probe(y, "churned process, before any churn")
    for r in range(1, rounds + 1):
        done = 0
        while done < c:
            k = min(8192, c - done); a = call("POST", f"/churn/{x}/{y}/{k}")[1]; x, y, done = a["x"], a["y"], done + k
        if r in (1, 5, 10, 20, rounds): probe(y, f"churned process, after {r} rounds")
    probe(y, "churned process, same live keys again")
    B = start() if False else None
finally:
    A.kill(); A.wait()
port = 18088
B = start()
try:
    call("POST", f"/insert_from/{y}/{w}")
    probe(y, "FRESH process, the same live keys loaded in order")
finally:
    B.kill(); B.wait()
