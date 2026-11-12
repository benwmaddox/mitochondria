"""Struct microbenchmark driver: python -I run.py base.mito|new.mito [--steps N] [--rounds N]; MITO_EXE picks the compiler. Prints the handler time per step of each endpoint."""
import http.client, json, os, socket, subprocess, sys, time
here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(here, "..", "ordered"))
from pin import pin
exe = ".exe" if os.name == "nt" else ""
mito = os.environ.get("MITO_EXE", os.path.join(here, "..", "..", "target", "release", "mito" + exe))
src = sys.argv[1]
steps = int(sys.argv[sys.argv.index("--steps") + 1]) if "--steps" in sys.argv else 2000000
rounds = int(sys.argv[sys.argv.index("--rounds") + 1]) if "--rounds" in sys.argv else 5
out = os.environ.get("OUT", os.path.join(here, "build")); os.makedirs(out, exist_ok=True)
server = os.path.join(out, os.path.basename(src) + ".bin" + exe)
subprocess.run([mito, "build", os.path.join(here, src), "-o", server], check=True)
port = 18190
def call(path):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600); c.request("POST", path); r = c.getresponse(); body = r.read()
    if r.status >= 300: raise RuntimeError(f"{path}: {r.status} {body[:200]}")
    return int(r.getheader("X-Handler-Ns")), body.decode()
eps = ["lib", "field"] + (["ret"] if "new" in os.path.basename(src) else [])
best = {e: None for e in eps}; ans = {}
for _ in range(rounds):
    p = subprocess.Popen([server, "--port", str(port), "--timing", "--loop-budget", "1000000000000000"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); pin(p)
    try:
        for _ in range(300):
            try: socket.create_connection(("127.0.0.1", port), 0.2).close(); break
            except OSError: time.sleep(0.05)
        for e in eps:
            call(f"/{e}/{steps // 10}")
            ns, body = call(f"/{e}/{steps}"); ans[e] = body
            best[e] = ns if best[e] is None else min(best[e], ns)
    finally: p.kill(); p.wait()
for e in eps: print(f"{os.path.basename(src)} {e}: {best[e] / steps:.3f} ns/step  answer {ans[e]}")
