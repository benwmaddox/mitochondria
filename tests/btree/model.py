"""Random operations on lib/btree.mito (tests/btree/btree_model.mito) against a Python model.
usage: python -I model.py EXE [OPS] [KEYRANGE] [SEED]
Items with equal keys must stay in the order added (a key change counts as a new add)."""
import http.client, json, os, random, subprocess, sys, time
exe = os.path.abspath(sys.argv[1]); ops = int(sys.argv[2]) if len(sys.argv) > 2 else 20000
span = int(sys.argv[3]) if len(sys.argv) > 3 else 400; seed = int(sys.argv[4]) if len(sys.argv) > 4 else 1
port = 18091
p = subprocess.Popen([exe, "--port", str(port)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
def call(m, path):
    for _ in range(100):
        try:
            c = http.client.HTTPConnection("127.0.0.1", port, timeout=60); c.request(m, path); r = c.getresponse(); b = r.read()
            return r.status, (json.loads(b) if b else None)
        except OSError: time.sleep(0.05)
    raise SystemExit("server not reachable")
rnd = random.Random(seed)
model = []            # [(key, order, slot)] sorted by (key, order)
order = 0
def check(full):
    exp = [k * 100000 + s for k, _, s in model]
    st, a = call("GET", "/all"); assert a["x"] == exp, ("all", len(exp), a["x"][:20], exp[:20])
    st, a = call("GET", "/rev"); assert a["x"] == exp[::-1], ("rev",)
    st, a = call("GET", "/ends"); assert a["size"] == len(model)
    assert (a["first"], a["last"]) == ((model[0][0], model[-1][0]) if model else (-1, -1)), ("ends", a)
    for k in ([rnd.randint(-5, span + 5) for _ in range(6)] if not full else range(-2, span + 3, max(1, span // 40))):
        st, a = call("GET", f"/from/{k}")
        up = [kk * 100000 + s for kk, _, s in model if kk >= k][:40]
        down = [kk * 100000 + s for kk, _, s in model if kk <= k][::-1][:40]
        assert a["up"] == up, ("from", k, a["up"], up); assert a["down"] == down, ("from rev", k, a["down"], down)
try:
    for i in range(ops):
        r = rnd.random()
        grow = len(model) < 2800 and (r < 0.55 or len(model) < 20)
        if grow:
            k = rnd.randint(0, span); st, a = call("POST", f"/add/{k}"); assert st == 200, (st, a)
            order += 1; model.append((k, order, a["slot"])); model.sort(key=lambda x: (x[0], x[1]))
        elif r < 0.85 and model:
            j = rnd.randrange(len(model)); _, _, s = model.pop(j); st, a = call("POST", f"/del/{s}"); assert st == 200, (st, a)
        elif model:
            j = rnd.randrange(len(model)); k0, _, s = model.pop(j); k = rnd.randint(0, span)
            st, a = call("POST", f"/set/{s}/{k}"); assert st == 200, (st, a)
            order += 1; model.append((k, order, s)); model.sort(key=lambda x: (x[0], x[1]))
        if i % 500 == 499: check(False); print(i + 1, "ops ok, size", len(model), flush=True)
    check(True); print("PASS", ops, "ops, final size", len(model))
finally:
    p.kill()
