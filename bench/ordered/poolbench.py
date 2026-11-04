"""Pool add and remove cost with no tree (pool.mito), builds alternating: python -I poolbench.py N ROUNDS exeA exeB ..."""
import http.client, json, os, socket, statistics, subprocess, sys, time
n, rounds, exes = int(sys.argv[1]), int(sys.argv[2]), [os.path.abspath(e) for e in sys.argv[3:]]
port = 18089
def call(m, p):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600); t = time.perf_counter(); c.request(m, p); r = c.getresponse(); b = r.read(); dt = (int(r.getheader("X-Handler-Ns")) / 1e9) if r.getheader("X-Handler-Ns") else time.perf_counter() - t
    if r.status >= 300: raise SystemExit(f"{m} {p}: {r.status} {b[:200]}")
    return dt
def run(exe):
    p = subprocess.Popen([exe, "--port", str(port), "--timing"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(200):
            try: socket.create_connection(("127.0.0.1", port), 0.2).close(); break
            except OSError: time.sleep(0.05)
        add = call("POST", f"/fill2/{n}") * 1e9 / n
        per, done = [], 0
        while done < n:
            k = min(16384, n - done); per.append(call("DELETE", f"/clear/{done}/{k}") * 1e9 / k); done += k
        return add, statistics.median(per)  # the median batch: a few slow requests (other work on the machine) cannot move it
    finally: p.kill(); p.wait()
res = {e: [] for e in exes}
for i in range(rounds):
    for e in (exes if i % 2 == 0 else exes[::-1]): res[e].append(run(e))
print(f"n={n}, best of {rounds}: ns per (add to two pools + 2 stores) and ns per remove from one pool")
for e in exes: print(f"  {os.path.basename(e):<14}add {min(r[0] for r in res[e]):6.1f}   remove {min(r[1] for r in res[e]):6.1f}")
