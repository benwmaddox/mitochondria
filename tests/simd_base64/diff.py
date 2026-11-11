"""Differential test of the base64 builtins' AVX2 loops (runtime/mito.c, MITO_B64_SIMD) against a Python reference of their meaning, with the SIMD build and a scalar build.
Build first:  mito build tests/simd_base64/b64.mito -o D:/tmp/probe/b64_simd.exe
              mito build tests/simd_base64/b64.mito -o D:/tmp/probe/b64_scalar.exe --cflag -O3 --cflag -DMITO_NO_SIMD
then `python tests/simd_base64/diff.py`. Any mismatch fails (exit 1)."""
import base64, binascii, http.client, json, random, socket, subprocess, sys, time

STD = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
URL = STD[:62] + "-_"

def ref_decode(t):
    """The builtin's meaning: either alphabet, up to two '=' at the end (then the length is a multiple of 4), else no padding; None when it is not base64 or has leftover non-zero bits."""
    n = len(t); ln = n
    while ln and t[ln - 1] == "=" and n - ln < 2: ln -= 1
    if ln % 4 == 1 or (n != ln and n % 4): return None
    acc = 0; bits = 0; out = bytearray()
    for ch in t[:ln]:
        if ch in STD: v = STD.index(ch)
        elif ch == "-": v = 62
        elif ch == "_": v = 63
        else: return None
        acc = (acc << 6 | v) & 0xFFFFFFFF; bits += 6
        if bits >= 8: bits -= 8; out.append((acc >> bits) & 255)
    if acc & ((1 << bits) - 1): return None
    return bytes(out)

def run(exe, port):
    p = subprocess.Popen([exe, "--port", str(port)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(200):
        try: socket.create_connection(("127.0.0.1", port), 0.2).close(); break
        except OSError: time.sleep(0.05)
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
    def call(path, t):
        conn.request("POST", path, body=json.dumps({"t": t}), headers={"Content-Type": "application/json"})
        r = conn.getresponse(); b = r.read()
        if r.status != 200: raise RuntimeError(f"{path} {t[:30]!r}: {r.status} {b[:100]}")
        return json.loads(b)["r"]
    return p, call

random.seed(7)
enc_cases = ["", "a", "ab", "abc"] + ["".join(random.choice("abcdefghijklmnopqrstuvwxyz0123456789 +/=-_~!") for _ in range(n)) for n in list(range(0, 130)) + [255, 256, 257, 1000, 4097, 9000, 16000]]
dec_cases = []
for n in list(range(0, 140)) + [191, 192, 193, 400, 1000, 4096, 12000]:
    raw = bytes(random.randrange(256) for _ in range(n))
    good = base64.b64encode(raw).decode()
    dec_cases += [good, good.rstrip("="), base64.urlsafe_b64encode(raw).decode(), base64.urlsafe_b64encode(raw).decode().rstrip("=")]
    if good:
        i = random.randrange(len(good))
        dec_cases.append(good[:i] + random.choice("*!@ \n.") + good[i + 1:])           # an invalid character somewhere
        dec_cases.append(good[:i] + random.choice(STD) + good[i + 1:])                  # a different valid character (may be non-canonical at the end)
        dec_cases.append(good + random.choice(["=", "==", "===", "A", "AA"]))           # bad or odd tails
dec_cases += ["A" * k for k in range(0, 70)] + ["A" * 31 + "-", "A" * 32 + "_", "/" * 64, "+" * 33, "=" * 5, "A" * 63 + "="]
bad = 0
for name, exe, port in (("simd", "D:/tmp/probe/b64_simd.exe", 18270), ("scalar", "D:/tmp/probe/b64_scalar.exe", 18271)):
    p, call = run(exe, port)
    try:
        for t in enc_cases:
            got = call("/enc", t); want = base64.b64encode(t.encode()).decode()
            if got != want: bad += 1; print(name, "ENC mismatch", len(t), got[:40], want[:40])
        for t in dec_cases:
            got = call("/dec", t); w = ref_decode(t); want = None if w is None else w.hex()
            if got != want: bad += 1; print(name, "DEC mismatch", len(t), repr(t[:40]), "got", None if got is None else got[:30], "want", None if want is None else want[:30])
    finally: p.kill(); p.wait()
    print(name, "checked", len(enc_cases), "encodes and", len(dec_cases), "decodes")
print("mismatches:", bad)
sys.exit(1 if bad else 0)
