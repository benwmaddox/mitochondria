"""usage: python -I probe.py EXE N then METHOD/PATH pairs, e.g. GET:/hit/10000/200000 ; prints seconds per request."""
import http.client, subprocess, sys, time
import os
exe, n = os.path.abspath(sys.argv[1]), int(sys.argv[2])
p = subprocess.Popen([exe, "--port", "18090"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
def call(m, path):
    for _ in range(100):
        try:
            c = http.client.HTTPConnection("127.0.0.1", 18090, timeout=3600); t = time.perf_counter(); c.request(m, path); r = c.getresponse(); b = r.read(); return time.perf_counter() - t, r.status, b[:80]
        except OSError: time.sleep(0.05)
try:
    print("insert", call("POST", f"/insert/{n}"))
    for a in sys.argv[3:]:
        m, path = a.split(":", 1); dt, st, b = call(m, path); print(f"{a:<32}{dt:8.3f}s {st} {b}")
finally: p.kill()
