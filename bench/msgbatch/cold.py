"""Cold vs warm: a fresh process per trial, no warmup. Time of the first batches one by one, then the steady per-batch time, heap allocations during the batches
(Rust: counting allocator, build with --features allocs into rust/target-allocs; Go: runtime.MemStats; Mitochondria: no allocator, see README) and peak memory.
usage: python -I cold.py [--trials N] [--iters N]"""
import ctypes, http.client, json, os, re, socket, statistics, subprocess, sys, time
here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(here, "..", "ordered"))
from pin import pin
exe = ".exe" if os.name == "nt" else ""
mito = os.environ.get("MITO_EXE", os.path.join(here, "..", "..", "target", "release", "mito" + exe))
trials = int(sys.argv[sys.argv.index("--trials") + 1]) if "--trials" in sys.argv else 5
iters = int(sys.argv[sys.argv.index("--iters") + 1]) if "--iters" in sys.argv else 100000
port = 18173
server = os.path.join(here, "build", "msgbatch" + exe)
subprocess.run([mito, "build", os.path.join(here, "main.mito"), "-o", server], check=True, capture_output=True)

class _PMC(ctypes.Structure):
    _fields_ = [("cb", ctypes.c_uint32), ("PageFaultCount", ctypes.c_uint32), ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t), ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t), ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t)]
def mem_mb(p):  # (peak working set, page faults) of a Windows process, from its handle
    m = _PMC(); m.cb = ctypes.sizeof(_PMC); k = ctypes.windll.kernel32
    k.K32GetProcessMemoryInfo.argtypes = [ctypes.c_void_p, ctypes.POINTER(_PMC), ctypes.c_uint32]
    k.K32GetProcessMemoryInfo(ctypes.c_void_p(int(p._handle)), ctypes.byref(m), m.cb)
    return m.PeakWorkingSetSize / (1 << 20), m.PageFaultCount

def call(method, path):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600); c.request(method, path); r = c.getresponse(); body = r.read()
    return int(r.getheader("X-Handler-Ns") or 0), json.loads(body)

def cold_mito():
    p = subprocess.Popen([server, "--port", str(port), "--timing", "--loop-budget", "1000000000000000"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); pin(p)
    try:
        for _ in range(300):
            try: socket.create_connection(("127.0.0.1", port), 0.2).close(); break
            except OSError: time.sleep(0.05)
        idle = mem_mb(p)
        prep = call("POST", "/prepare")[0]
        first = [call("POST", "/run/1")[0] for _ in range(5)]
        total = call("POST", f"/run/{iters - 5}")[0]
        peak, faults = mem_mb(p)
        return {"prepare_ns": prep, "first_ns": first, "per_batch_ns": total / (iters - 5), "allocs": 0, "allocs_first5": 0, "peak_mb": peak, "idle_mb": idle[0]}
    finally: p.kill(); p.wait()

def cold_ref(path):
    p = subprocess.Popen([path, str(iters), "cold"], stdout=subprocess.PIPE, text=True); pin(p)
    out = p.communicate()[0]
    m = re.search(r"first batches ns \[([^\]]*)\] total \S+ per batch \(after the first 5\) (\d+) ns allocations during batches (-?\d+)", out)
    first = [int(x) for x in re.split(r"[ ,]+", m.group(1).strip())]
    peak = mem_mb(p)[0]
    a = subprocess.run([path, str(iters), "allocs"], capture_output=True, text=True).stdout
    m2 = re.search(r"allocations first5 (\d+) steady (\d+)", a)
    return {"first_ns": first, "per_batch_ns": int(m.group(2)), "allocs": int(m2.group(1)) + int(m2.group(2)), "allocs_first5": int(m2.group(1)), "peak_mb": peak}

impls = {"Mitochondria": cold_mito,
         "Rust": lambda: cold_ref(os.path.join(here, "rust", "target-allocs", "release", "msgbatch" + exe)),
         "Go": lambda: cold_ref(os.path.join(here, "go", "msgbatch" + exe))}
res = {k: [] for k in impls}
for t in range(trials):
    for k, f in impls.items(): res[k].append(f())
for k, rs in res.items():
    first = [statistics.median(r["first_ns"][i] for r in rs) for i in range(5)]
    extra = f", prepare {statistics.median(r['prepare_ns'] for r in rs) / 1000:.1f} us (builds the input)" if k == "Mitochondria" else ""
    print(f"{k:13} first 5 batches (median ns) {[int(x) for x in first]}; steady {statistics.median(r['per_batch_ns'] for r in rs):.0f} ns/batch; "
          f"heap allocations in all batches {max(r['allocs'] for r in rs)} (first 5: {max(r['allocs_first5'] for r in rs)}); peak memory {statistics.median(r['peak_mb'] for r in rs):.1f} MB{extra}")
