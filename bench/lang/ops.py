"""ns per operation for the common language operations in ops.mito, from the server's own timer (best of 5), pinned.
usage: python -I ops.py exe"""
import http.client, json, os, socket, subprocess, sys, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "ordered"))
from pin import pin
exe = os.path.abspath(sys.argv[1]); port = 18097
def call(m, path):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=3600); c.request(m, path); r = c.getresponse(); b = r.read()
    if r.status >= 300: raise SystemExit(f"{m} {path}: {r.status} {b[:200]}")
    return int(r.getheader("X-Handler-Ns"))
N = 2_000_000
rows = [  # label, method, path, operations in one request
    ("empty loop (s = s + i)", "GET", f"/empty/{N}", N),
    ("iterate a pool, sum a field", "GET", "/iter_pool/2000", 2000 * 1000),
    ("iterate a pool, where x % 2 == 0", "GET", "/iter_where/2000", 2000 * 1000),
    ("pool[i].x read", "GET", f"/index_pool/{N}", N),
    ("store an i64 in state", "POST", f"/store_int/{N}", N),
    ("store an i64 field of an item", "POST", f"/store_field_int/{N}", N),
    ("store an f64 in state", "POST", f"/store_float/{N}", N),
    ("store an f64 field of an item", "POST", f"/store_field_float/{N}", N),
    ("store a bool field of an item", "POST", f"/store_bool/{N}", N),
    ("store a string<16> field (literal)", "POST", "/store_string/50000", 50000),
    ("read an f64 field and add", "GET", f"/read_float/{N}", N),
    ("compare a string field to a literal", "GET", "/cmp_string/50000", 50000),
    ("  ... 1,000,000 times (text area exhausted?)", "GET", "/cmp_string/1000000", 1000000),
    ("store a ref field", "POST", f"/store_ref/{N}", N),
    ("read through a ref (e.next.x)", "GET", f"/read_ref/{N}", N),
    ("store arr[i % 64] (plain array)", "POST", f"/store_arr/{N}", N),
    ("list.add { at, v } (drop_oldest)", "POST", f"/list_add/{N}", N),
    ("iterate a list of 4096", "GET", "/iter_list/500", 500 * 4096),
    ("pool add + remove (same request)", "POST", "/add_remove/5000", 5000),
    ("answer an array of 1000 objects (per object)", "GET", "/respond_array/1000", 1000),
]
p = subprocess.Popen([exe, "--port", str(port), "--timing"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); pin(p)
try:
    for _ in range(200):
        try: socket.create_connection(("127.0.0.1", port), 0.2).close(); break
        except OSError: time.sleep(0.05)
    call("POST", "/fill/1000")
    call("POST", f"/list_add/4096")
    print(f"{'operation':<48}{'ns/op':>9}")
    for label, m, path, ops in rows:
        best = min(call(m, path) for _ in range(5)) / ops
        print(f"{label:<48}{best:>9.1f}", flush=True)
finally: p.kill()
