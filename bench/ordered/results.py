"""Ordered maps against Rust's BTreeMap: writes RESULTS.md (time and peak memory per workload) from results.json.

usage: python -I results.py [--rounds N] [--ops N] [--mito-only] [--write-only]
Builds `ordered_btree.mito` (lib/btree.mito: items in their own pool, stable slots) and `ordered_omap.mito` (lib/ordmap.mito: values
inside the leaves) with the current compiler, then for 10,000 and 1,000,000 keys runs them and the Rust baseline (bench/btree-rust,
`BTreeMap<u64,u64>`) in alternating rounds on one pinned core, each timed inside its own process. Best of the rounds per workload.
`--mito-only` measures only the Mitochondria builds and keeps Rust's last numbers; `--write-only` rewrites RESULTS.md from results.json."""
import ctypes, datetime, http.client, json, math, os, platform, socket, subprocess, sys, time
here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, here)
from pin import pin

args = sys.argv[1:]
def opt(name, default):
    if name in args: i = args.index(name); v = args[i + 1]; del args[i:i + 2]; return int(v)
    return default
rounds, ops = opt("--rounds", 5), opt("--ops", 1000000)
mito_only = "--mito-only" in args
write_only = "--write-only" in args
mito = os.environ.get("MITO_EXE", os.path.join(here, "..", "..", "target", "release", "mito.exe"))
rust = os.path.join(here, "..", "btree-rust", "target", "release", "btree-rust.exe")
builds = {"btree": "ordered_btree.mito", "ordmap": "ordered_omap.mito"}
sizes = [10000, 1000000]
names = ["insert", "hit", "miss", "mixed", "increment", "range100", "delete"]
labels = {"insert": "Insert new keys", "hit": "Lookup: hit", "miss": "Lookup: miss", "mixed": "Lookup: 50% hits, 50% misses",
          "increment": "Increment existing value", "range100": "Range scan, up to 100 entries", "delete": "Delete existing key"}
port = 18086
results_path, markdown_path = os.path.join(here, "results.json"), os.path.join(here, "RESULTS.md")
results = json.load(open(results_path, encoding="utf-8")) if os.path.exists(results_path) else {}


class _PMC(ctypes.Structure):
    _fields_ = [("cb", ctypes.c_uint32), ("PageFaultCount", ctypes.c_uint32), ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t), ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t), ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t)]
def peak_mb(p):
    """A process's peak working set (the most physical memory it held at once), from its handle (still open after it ends)."""
    if os.name != "nt": return None
    m = _PMC(); m.cb = ctypes.sizeof(_PMC); k32 = ctypes.windll.kernel32
    k32.K32GetProcessMemoryInfo.argtypes = [ctypes.c_void_p, ctypes.POINTER(_PMC), ctypes.c_uint32]
    return m.PeakWorkingSetSize / (1 << 20) if k32.K32GetProcessMemoryInfo(ctypes.c_void_p(int(p._handle)), ctypes.byref(m), m.cb) else None


def call(method, path):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600)
    c.request(method, path); r = c.getresponse(); body = r.read()
    if r.status >= 300: raise SystemExit(f"{method} {path}: {r.status} {body[:200]}")
    return int(r.getheader("X-Handler-Ns")) / 1e9, json.loads(body)


def build(key):
    out = os.path.join(here, "build", key + (".exe" if os.name == "nt" else ""))
    os.makedirs(os.path.dirname(out), exist_ok=True)
    r = subprocess.run([mito, "build", os.path.join(here, builds[key]), "-o", out], capture_output=True, text=True)
    if r.returncode: raise SystemExit(f"{key}: build failed\n{r.stdout}{r.stderr}")
    return out


def run_mito(exe, n):
    try: socket.create_connection(("127.0.0.1", port), 0.2).close(); raise SystemExit(f"port {port} is in use: stop the process listening there first")
    except OSError: pass
    p = subprocess.Popen([exe, "--port", str(port), "--timing"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); pin(p)
    try:
        for _ in range(200):
            try: call("GET", "/stats"); break
            except OSError: time.sleep(0.05)
        r = {"insert": call("POST", f"/insert/{n}")[0] * 1e9 / n}
        for w in ("hit", "miss", "mixed"): r[w] = call("GET", f"/{w}/{n}/{ops}")[0] * 1e9 / ops
        r["increment"] = call("POST", f"/increment/{n}/{ops}")[0] * 1e9 / ops
        r["range100"] = call("GET", f"/range/{n}/{ops // 10}")[0] * 1e9 / (ops // 10)
        x, done, spent = 20261104, 0, 0.0
        while done < n:
            c = min(16384, n - done); dt, a = call("DELETE", f"/delete/{x}/{c}"); x, done, spent = a["x"], done + c, spent + dt
        r["delete"] = spent * 1e9 / n
        if call("GET", f"/hit/{n}/{min(n, 1000)}")[1]["sum"] != 0: raise SystemExit("delete left items behind")
        r["mb"] = peak_mb(p)
        return r
    finally:
        p.kill(); p.wait()


def run_rust(n):
    p = subprocess.Popen([rust, str(n), str(ops)], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True); pin(p)
    out = p.communicate()[0]
    r = {k: float(v) for k, v in (l.split("\t") for l in out.strip().splitlines()) if k in names}
    r["mb"] = peak_mb(p)
    return r


def measure():
    exes = {k: build(k) for k in builds}
    for n in sizes:
        got = {k: [] for k in list(builds) + ([] if mito_only else ["rust"])}
        for i in range(rounds):
            order = list(got) if i % 2 == 0 else list(reversed(list(got)))  # alternate who goes first: a drifting machine load hits all alike
            for k in order: got[k].append(run_rust(n) if k == "rust" else run_mito(exes[k], n))
            print(f"  n={n}: round {i + 1} done", file=sys.stderr)
        stamp = datetime.date.today().isoformat()
        for k, rs in got.items():
            mbs = [r["mb"] for r in rs if r.get("mb") is not None]
            results.setdefault(str(n), {})[k] = {"ns": {w: min(r[w] for r in rs) for w in names}, "mb": min(mbs) if mbs else None, "date": stamp, "ops": ops, "rounds": rounds}
        with open(results_path, "w", encoding="utf-8") as f: json.dump(results, f, indent=1, sort_keys=True)


def write_markdown():
    def num(x, d=1): return "" if x is None else f"{x:.{d}f}"
    def ratio(a, b): return "" if a is None or not b else f"{a / b:.2f}x"
    dates = sorted({v["date"] for s in results.values() for v in s.values()})
    o = ["# Ordered maps: Mitochondria against Rust's BTreeMap", "",
         "Written by `results.py` (`python -I bench/ordered/results.py`) from `results.json`; do not edit by hand. Each implementation's numbers are from its latest run ("
         + (dates[0] + " to " + dates[-1] if len(dates) > 1 else (dates[0] if dates else "no runs")) + "; `--mito-only` keeps Rust's last numbers).", "",
         f"Machine: {platform.system()} {platform.release()}, {platform.processor() or platform.machine()}. The same workloads on the same key sequence (MINSTD; present keys 2r, "
         "absent keys 2r+1), i64 keys to i64 values. `btree` is lib/btree.mito (items in their own pool with stable slots, each found by key), `ordmap` lib/ordmap.mito "
         "(values inside the leaves); Rust is `BTreeMap<u64,u64>` (opt-level 3, fat LTO, one codegen unit). Both Mitochondria maps are declared `unlogged` "
         "(`m: ordmap<1048576> unlogged;`): kept out of the undo log, so a failed request does not roll the map back, as a Rust map has no rollback; their arithmetic "
         "is still overflow-checked. Both run on one pinned core, timed inside their own process, alternating rounds; best of the rounds. Ratios are Mitochondria / Rust; "
         "rows are sorted by the better of the two ratios, best first.", ""]
    for n in sorted(results, key=int):
        s = results[n]
        if "rust" not in s: continue
        def ns(k, w): return s[k]["ns"][w] if k in s else None
        o += [f"## {int(n):,} keys: time (ns per operation; a range scan is per scan)", "",
              f"Operations per lookup workload: {s['rust'].get('ops', ''):,}.", "",
              "| Workload | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |", "|---|---:|---:|---:|---:|---:|"]
        def best(w): return min(x for x in [(ns(k, w) / ns("rust", w)) if ns(k, w) else None for k in builds] + [float("inf")] if x is not None)
        for w in sorted(names, key=best):
            o.append(f"| {labels[w]} | {num(ns('btree', w))} | {num(ns('ordmap', w))} | {num(ns('rust', w))} | {ratio(ns('btree', w), ns('rust', w))} | {ratio(ns('ordmap', w), ns('rust', w))} |")
        o += ["", f"## {int(n):,} keys: memory (peak working set, MB)", "",
              "The most physical memory each process held at once (Windows' peak working set), over the whole run: insert, lookups, scans and deletes. "
              "Mitochondria's includes its server (connection threads, request areas); reserved address space never touched is not counted.", "",
              "| | Mitochondria `lib/btree.mito` | Mitochondria `lib/ordmap.mito` | Rust `std::collections::BTreeMap` | btree.mito / BTreeMap | ordmap.mito / BTreeMap |", "|---|---:|---:|---:|---:|---:|"]
        mb = lambda k: s[k]["mb"] if k in s else None
        o += [f"| Peak memory | {num(mb('btree'))} | {num(mb('ordmap'))} | {num(mb('rust'))} | {ratio(mb('btree'), mb('rust'))} | {ratio(mb('ordmap'), mb('rust'))} |", ""]
    with open(markdown_path, "w", encoding="utf-8") as f: f.write("\n".join(o))
    print(f"wrote {markdown_path}", file=sys.stderr)


if not write_only: measure()
write_markdown()
