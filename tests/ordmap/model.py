"""Random operations on lib/ordmap.mito (tests/ordmap/ordmap_model.mito) against a Python dict.
usage: python -I model.py EXE [OPS] [KEYRANGE] [SEED]"""
import bisect, http.client, json, os, random, subprocess, sys, time
exe = os.path.abspath(sys.argv[1]); ops = int(sys.argv[2]) if len(sys.argv) > 2 else 20000
span = int(sys.argv[3]) if len(sys.argv) > 3 else 400; seed = int(sys.argv[4]) if len(sys.argv) > 4 else 1
port = 18111
p = subprocess.Popen([exe, "--port", str(port)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
def call(m, path):
    for _ in range(100):
        try:
            c = http.client.HTTPConnection("127.0.0.1", port, timeout=60); c.request(m, path); r = c.getresponse(); b = r.read()
            return r.status, (json.loads(b) if b else None)
        except OSError: time.sleep(0.05)
    raise SystemExit("server not reachable")
rnd = random.Random(seed); model = {}
def check():
    for k in [rnd.randint(-3, span + 3) for _ in range(12)]:
        st, a = call("GET", f"/get/{k}"); assert a["v"] == model.get(k, -999999), ("get", k, a, model.get(k))
    keys = sorted(model)
    for _ in range(8):
        k = rnd.randint(-3, span + 3); n = rnd.choice([0, 1, 5, 40, 100, 5000])
        i = bisect.bisect_left(keys, k); exp = sum(model[x] for x in keys[i:i + n])
        st, a = call("GET", f"/range/{k}/{n}"); assert a["sum"] == exp, ("range", k, n, a, exp)
    st, a = call("GET", "/size"); assert a["n"] == len(model), ("size", a, len(model))
try:
    for i in range(ops):
        r = rnd.random(); k = rnd.randint(0, span)
        if r < 0.5 and len(model) < 2800 or len(model) < 20:
            v = rnd.randint(-1000, 1000); st, a = call("POST", f"/put/{k}/{v}"); assert st == 200, (st, a); model[k] = v
        elif r < 0.7:
            d = rnd.randint(-5, 5); st, a = call("POST", f"/addto/{k}/{d}"); assert st == 200 and a["done"] == (k in model), ("addto", k, a)
            if k in model: model[k] += d
        else:
            k = rnd.choice(list(model)) if model and rnd.random() < 0.8 else k
            st, a = call("POST", f"/del/{k}"); assert st == 200 and a["done"] == (k in model), ("del", k, a)
            model.pop(k, None)
        if i % 500 == 499: check(); print(i + 1, "ops ok, size", len(model), flush=True)
    check(); print("PASS", ops, "ops, final size", len(model))
finally:
    p.kill()
