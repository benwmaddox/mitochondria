"""TodoMVC rendered on the server: Mitochondria (examples/todomvc) against crudspec's build of the same app (apps/todomvc), on Linux.

usage: python3 -I bench.py --mito EXE --crudspec APP --crudspec-assets DIR [--rounds N]   (run under the shared bench lock)
Each server runs alone on one pinned CPU (BENCH_CPU, default 10). The client reads each app's own pages and submits their real
forms (the new-todo form, a todo's toggle form), so neither app's routes are written here: Mitochondria posts url-encoded forms,
crudspec multipart forms with its signed field. Per app and list size (10 and 100 todos): the time of one request on one kept-open
connection (median and 99th percentile of the client's round trip: page render, add a todo, toggle a todo), the pages per second
four client processes get, and the server's peak resident memory (VmHWM) at the end. Writes results.json and RESULTS.md here.
Both keep the lists in SQLite (WAL, on disk): Mitochondria as `table<T>`s, crudspec as its entities."""
import http.client, json, multiprocessing, os, platform, re, shutil, socket, statistics, subprocess, sys, tempfile, time, urllib.parse, uuid
from html.parser import HTMLParser

here = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
def opt(name, default=None):
    if name in args: i = args.index(name); v = args[i + 1]; del args[i:i + 2]; return v
    return default
MITO, CS, CS_ASSETS = opt("--mito"), opt("--crudspec"), opt("--crudspec-assets")
ROUNDS = int(opt("--rounds", "3"))
CPU = int(os.environ.get("BENCH_CPU", "10"))
CLIENT_CPUS = [CPU + 1 + i for i in range(4)]
SIZES = [10, 100]
LAT_N = 2000      # requests per latency measurement
THR_SECONDS = 3   # seconds per throughput measurement


class Forms(HTMLParser):
    """Every form of a page: action, method, enctype, its inputs (name, value, type, class), and the classes of what is inside."""
    def __init__(self):
        super().__init__(); self.forms = []; self.cur = None
    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "form": self.cur = {"action": a.get("action", ""), "enctype": a.get("enctype", ""), "inputs": [], "classes": [], "id": a.get("id", "")}; self.forms.append(self.cur)
        elif self.cur is not None:
            if a.get("class"): self.cur["classes"].append(a["class"])
            if tag in ("input", "button") and a.get("name"): self.cur["inputs"].append((a["name"], a.get("value", ""), a.get("type", "")))
    def handle_endtag(self, tag):
        if tag == "form": self.cur = None


class Client:
    def __init__(self, port):
        self.port = port; self.c = http.client.HTTPConnection("127.0.0.1", port, timeout=30); self.cookies = {}
    def req(self, method, path, body=None, ctype=None):
        h = {"Host": f"127.0.0.1:{self.port}", "Origin": f"http://127.0.0.1:{self.port}"}
        if self.cookies: h["Cookie"] = "; ".join(f"{k}={v}" for k, v in self.cookies.items())
        if ctype: h["Content-Type"] = ctype
        self.c.request(method, path, body=body, headers=h)
        r = self.c.getresponse(); data = r.read()
        for k, v in r.getheaders():
            if k.lower() == "set-cookie":
                kv = v.split(";", 1)[0]
                if "=" in kv: n, val = kv.split("=", 1); self.cookies[n.strip()] = val.strip()
        return r.status, data
    def page(self):
        s, d = self.req("GET", "/"); assert s == 200, s
        f = Forms(); f.feed(d.decode()); return d, f.forms
    def submit(self, form, fields):
        vals = [(n, v) for n, v, t in form["inputs"] if t == "hidden"] + list(fields.items())
        if "multipart" in form["enctype"]:
            b = uuid.uuid4().hex
            body = "".join(f"--{b}\r\nContent-Disposition: form-data; name=\"{n}\"\r\n\r\n{v}\r\n" for n, v in vals) + f"--{b}--\r\n"
            return self.req("POST", form["action"], body.encode(), f"multipart/form-data; boundary={b}")
        return self.req("POST", form["action"], urllib.parse.urlencode(vals).encode(), "application/x-www-form-urlencoded")


def new_todo_form(forms): return next(f for f in forms if any(n == "title" for n, _, _ in f["inputs"]) and "edit" not in f["action"] and "rename" not in f["action"])
def toggle_form(forms): return next(f for f in forms if re.search(r"toggle(?![-_]all)", f["action"]) and "all" not in f["action"])


def wait_up(port):
    for _ in range(200):
        try: socket.create_connection(("127.0.0.1", port), 0.2).close(); return
        except OSError: time.sleep(0.05)
    raise SystemExit(f"port {port}: no server")


def peak_mb(pid):
    for line in open(f"/proc/{pid}/status"):
        if line.startswith("VmHWM:"): return int(line.split()[1]) / 1024
    return None


def lat(f, n=LAT_N):
    ts = []
    for _ in range(n):
        t = time.perf_counter(); f(); ts.append(time.perf_counter() - t)
    ts.sort()
    return {"median_us": ts[len(ts) // 2] * 1e6, "p99_us": ts[int(len(ts) * 0.99)] * 1e6}


def thr_worker(port, cookies, cpu, until, q):
    os.sched_setaffinity(0, {cpu})
    c = Client(port); c.cookies = dict(cookies); n = 0
    while time.time() < until: c.req("GET", "/"); n += 1
    q.put(n)


def throughput(port, cookies):
    q = multiprocessing.Queue(); until = time.time() + THR_SECONDS + 0.5
    ps = [multiprocessing.Process(target=thr_worker, args=(port, cookies, cpu, until, q)) for cpu in CLIENT_CPUS]
    for p in ps: p.start()
    for p in ps: p.join()
    return sum(q.get() for _ in ps) / (THR_SECONDS + 0.5)


def run_app(name, cmd, port, cleanup):
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    os.sched_setaffinity(p.pid, {CPU})
    try:
        wait_up(port)
        c = Client(port); res = {}
        _, forms = c.page()
        have = 0
        for size in SIZES:
            while have < size:
                s, _ = c.submit(new_todo_form(forms), {"title": f"todo number {have}"}); assert s in (200, 303), (name, s)
                have += 1
                if have == 1: _, forms = c.page() # the first todo makes the list (and the cookie)
            body, forms = c.page()
            assert body.count(b"todo number") >= size, (name, size)
            r = {"page_bytes": len(body)}
            r["render"] = lat(lambda: c.req("GET", "/"))
            tf = toggle_form(forms)
            r["toggle"] = lat(lambda: c.submit(tf, {}), 1000)
            r["pages_per_s"] = throughput(port, c.cookies)
            res[str(size)] = r
        # adding: 200 todos onto the largest list
        nf = new_todo_form(forms)
        res["add"] = lat(lambda: c.submit(nf, {"title": "added"}), 200)
        res["peak_mb"] = peak_mb(p.pid)
        return res
    finally:
        p.kill(); p.wait(); cleanup()


def measure():
    out = {}
    for rnd in range(ROUNDS):
        for name in (["mito", "mito_normal", "crudspec"] if rnd % 2 == 0 else ["crudspec", "mito_normal", "mito"]):
            if name.startswith("mito"):
                d = tempfile.mkdtemp(prefix="mito-todomvc-")
                sync = ["--db-sync", "normal"] if name == "mito_normal" else []
                r = run_app(name, [MITO, "--port", "18301", "--data", d] + sync, 18301, lambda: shutil.rmtree(d, ignore_errors=True))
            else:
                d = tempfile.mkdtemp(prefix="cs-todomvc-")
                r = run_app(name, [CS, "--data", d, "--assets", CS_ASSETS, "--port", "18302"], 18302, lambda: shutil.rmtree(d, ignore_errors=True))
            out.setdefault(name, []).append(r)
            print(f"  round {rnd + 1}: {name} done", file=sys.stderr)
    # best of the rounds per measure
    best = {}
    for name, rs in out.items():
        b = {"peak_mb": min(r["peak_mb"] for r in rs), "add": {q: min(r["add"][q] for r in rs) for q in ("median_us", "p99_us")}}
        for size in map(str, SIZES):
            b[size] = {"page_bytes": rs[0][size]["page_bytes"], "pages_per_s": max(r[size]["pages_per_s"] for r in rs)}
            for k in ("render", "toggle"):
                b[size][k] = {m: min(r[size][k][m] for r in rs) for m in ("median_us", "p99_us")}
        best[name] = b
    best["date"] = time.strftime("%Y-%m-%d"); best["machine"] = f"{platform.system()} {platform.release()}"; best["rounds"] = ROUNDS
    json.dump(best, open(os.path.join(here, "results.json"), "w"), indent=1)
    return best


def markdown(b):
    m, n, c = b["mito"], b["mito_normal"], b["crudspec"]
    o = ["# TodoMVC: Mitochondria against crudspec", "",
         f"Written by `bench.py` from `results.json` ({b['date']}, {b['machine']}, best of {b['rounds']} alternating rounds); do not edit by hand.", "",
         "Both are TodoMVC rendered on the server, from this repository's `examples/todomvc` and crudspec's `apps/todomvc`, each server "
         "alone on one pinned CPU. The client submits each app's own forms (url-encoded for Mitochondria, multipart with a signed field for "
         "crudspec). Times are the client's round trip on one kept-open connection, in microseconds (Python's client adds its own share to both). "
         "Both keep the lists in SQLite on disk (WAL): Mitochondria as tables (`table<T>`), crudspec as its entities. By default Mitochondria syncs "
         "every commit to disk before answering (`--db-sync full`: an answered change survives a power cut); `--db-sync normal`, like "
         "crudspec's `synchronous=NORMAL`, syncs WAL commits at checkpoints (a process crash loses nothing, an OS crash or power cut can "
         "lose the last moments): the middle column is the like-for-like comparison. crudspec keeps a rendered page until its data changes, so its page time barely grows "
         "with the list; Mitochondria renders every page.", "",
         "| Measure | Mitochondria (default, `--db-sync full`) | Mitochondria `--db-sync normal` | crudspec | full / crudspec | normal / crudspec |", "|---|---:|---:|---:|---:|---:|"]
    for size in map(str, SIZES):
        for k, label in (("render", "page"), ("toggle", "toggle a todo")):
            for q, ql in (("median_us", "median"), ("p99_us", "99th percentile")):
                x, z, y = m[size][k][q], n[size][k][q], c[size][k][q]
                o.append(f"| {size} todos: {label}, {ql} (us) | {x:.0f} | {z:.0f} | {y:.0f} | {x / y:.2f}x | {z / y:.2f}x |")
        x, z, y = m[size]["pages_per_s"], n[size]["pages_per_s"], c[size]["pages_per_s"]
        o.append(f"| {size} todos: pages per second, 4 clients | {x:,.0f} | {z:,.0f} | {y:,.0f} | {x / y:.2f}x | {z / y:.2f}x |")
        o.append(f"| {size} todos: page size (bytes) | {m[size]['page_bytes']:,} | {n[size]['page_bytes']:,} | {c[size]['page_bytes']:,} | | |")
    for q, ql in (("median_us", "median"), ("p99_us", "99th percentile")):
        x, z, y = m["add"][q], n["add"][q], c["add"][q]
        o.append(f"| add a todo ({SIZES[-1]} to {SIZES[-1] + 200} todos), {ql} (us) | {x:.0f} | {z:.0f} | {y:.0f} | {x / y:.2f}x | {z / y:.2f}x |")
    o.append(f"| peak memory (MB, VmHWM) | {m['peak_mb']:.1f} | {n['peak_mb']:.1f} | {c['peak_mb']:.1f} | {m['peak_mb'] / c['peak_mb']:.2f}x | {n['peak_mb'] / c['peak_mb']:.2f}x |")
    open(os.path.join(here, "RESULTS.md"), "w").write("\n".join(o) + "\n")
    print("\n".join(o))


if __name__ == "__main__":
    if "--write-only" in args: markdown(json.load(open(os.path.join(here, "results.json"))))
    else: markdown(measure())
