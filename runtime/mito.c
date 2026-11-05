
/* ---- the C runtime (runtime/mito.c) ----
   Runs a compiled program: one reservation for the declared layout with pages committed where bytes are live, a write
   journal for exact rollback, requests in a fixed text area and response area, and the helper table (`H`) the
   generated endpoints call. It uses no heap: everything comes from reservations made at start and committed as used.
   It reproduces the Rust runtime's behaviour exactly (tests/cases check both against the same records). */

/* ---------------------------------------------------------------- platform (Windows) */
/* Built as a DLL by clang, the mito_* interface is exported; TinyCC finds it by name in memory. */
#if defined(_MSC_VER) || defined(__clang__)
#define MITO_API __declspec(dllexport)
#else
#define MITO_API
#endif
void *VirtualAlloc(void *a, u64 n, u32 type, u32 prot);
int VirtualFree(void *a, u64 n, u32 type);
void GetSystemTimeAsFileTime(u64 *ft);
int BCryptGenRandom(void *alg, u8 *buf, u32 n, u32 flags);
void *memcpy(void *d, const void *s, u64 n);
void *memmove(void *d, const void *s, u64 n);
void *memset(void *d, int c, u64 n);
#define MEM_COMMIT 0x1000
#define MEM_RESERVE 0x2000
#define MEM_DECOMMIT 0x4000
#define MEM_RELEASE 0x8000
#define PAGE_NOACCESS 1
#define PAGE_READWRITE 4
#define PAGE 4096ULL

static u64 up(u64 n, u64 to) { return (n + to - 1) / to * to; }
static void *os_reserve(u64 n) { return VirtualAlloc(0, n, MEM_RESERVE, PAGE_NOACCESS); }
static int os_commit(void *p, u64 n) { return VirtualAlloc(p, n, MEM_COMMIT, PAGE_READWRITE) != 0; }
static void os_decommit(void *p, u64 n) { VirtualFree(p, n, MEM_DECOMMIT); }
static void os_release(void *p) { if (p) VirtualFree(p, 0, MEM_RELEASE); }
static i64 os_now_ms(void) { u64 ft = 0; GetSystemTimeAsFileTime(&ft); return (i64)((ft - 116444736000000000ULL) / 10000ULL); }
/* Random bits from the OS (BCryptGenRandom with the system's preferred generator). */
static u64 os_random(void) {
    u64 v = 0;
    BCryptGenRandom(0, (u8 *)&v, 8, 2); /* BCRYPT_USE_SYSTEM_PREFERRED_RNG */
    return v;
}

/* A reservation used as a growing array: committed in 64 KB steps as it fills. */
typedef struct { u8 *p; u64 reserved, committed, len; } Grow;
static int grow_init(Grow *g, u64 reserve) { g->p = (u8 *)os_reserve(reserve); g->reserved = g->p ? reserve : 0; g->committed = 0; g->len = 0; return g->p != 0; }
static int grow_to(Grow *g, u64 n) {
    if (n <= g->committed) return 1;
    if (n > g->reserved) return 0;
    u64 c = up(n, 65536); if (c > g->reserved) c = g->reserved;
    if (!os_commit(g->p + g->committed, c - g->committed)) return 0;
    g->committed = c; return 1;
}
static void fatal(const char *why) { (void)why; *(volatile int *)0 = 0; } /* a reservation ran out: cannot continue */

/* ---------------------------------------------------------------- text */
/* A fixed block filled from the start: the request's text area and its response. */
typedef struct { u8 *p; u32 cap, len; } Area;
static int a_put(Area *a, const void *s, u64 n) { if ((u64)a->len + n > a->cap) return 0; if (n) memcpy(a->p + a->len, s, n); a->len += (u32)n; return 1; }
static int a_str(Area *a, const char *s) { u64 n = 0; while (s[n]) n++; return a_put(a, s, n); }
static int a_u64(Area *a, u64 v) { char b[24]; int n = 0; do { b[23 - n++] = (char)('0' + v % 10); v /= 10; } while (v); return a_put(a, b + 24 - n, n); }
static int a_i64(Area *a, i64 v) { if (v < 0) { if (!a_put(a, "-", 1)) return 0; return a_u64(a, (u64)0 - (u64)v); } return a_u64(a, (u64)v); }
static int a_hex(Area *a, u64 v) { char b[16]; int n = 0; do { b[15 - n++] = "0123456789abcdef"[v & 15]; v >>= 4; } while (v); return a_put(a, b + 16 - n, n); }
static u64 c_len(const char *s) { u64 n = 0; while (s[n]) n++; return n; }
static int c_eq(const char *a, const u8 *b, u64 n) { u64 i = 0; for (; i < n; i++) if ((u8)a[i] != b[i] || !a[i]) return 0; return a[n] == 0; }
static int b_eq(const u8 *a, u64 an, const u8 *b, u64 bn) { if (an != bn) return 0; for (u64 i = 0; i < an; i++) if (a[i] != b[i]) return 0; return 1; }
static int lower(int c) { return c >= 'A' && c <= 'Z' ? c + 32 : c; }

/* Bytes as UTF-8 text, each maximal invalid sequence replaced by U+FFFD (as Rust's String::from_utf8_lossy). */
static int a_lossy(Area *o, const u8 *s, u64 n) {
    u64 i = 0;
    while (i < n) {
        u8 b = s[i]; u32 need; u8 lo = 0x80, hi = 0xBF;
        if (b < 0x80) { if (!a_put(o, s + i, 1)) return 0; i++; continue; }
        if (b >= 0xC2 && b <= 0xDF) need = 1;
        else if (b >= 0xE0 && b <= 0xEF) { need = 2; if (b == 0xE0) lo = 0xA0; if (b == 0xED) hi = 0x9F; }
        else if (b >= 0xF0 && b <= 0xF4) { need = 3; if (b == 0xF0) lo = 0x90; if (b == 0xF4) hi = 0x8F; }
        else { if (!a_str(o, "\xEF\xBF\xBD")) return 0; i++; continue; }
        u64 j = i + 1; u32 got = 0;
        while (got < need && j < n && s[j] >= (got == 0 ? lo : 0x80) && s[j] <= (got == 0 ? hi : 0xBF)) { j++; got++; }
        if (got == need) { if (!a_put(o, s + i, j - i)) return 0; } else if (!a_str(o, "\xEF\xBF\xBD")) return 0;
        i = j;
    }
    return 1;
}

/* A JSON string: quotes, backslash escapes, \u00XX for other control bytes. */
static int a_json_str(Area *o, const u8 *s, u64 n) {
    if (!a_put(o, "\"", 1)) return 0;
    u64 start = 0;
    for (u64 i = 0; i < n; i++) {
        u8 c = s[i];
        const char *esc = 0;
        if (c == '"') esc = "\\\""; else if (c == '\\') esc = "\\\\"; else if (c == '\n') esc = "\\n"; else if (c == '\r') esc = "\\r"; else if (c == '\t') esc = "\\t";
        else if (c >= 0x20) continue;
        if (!a_put(o, s + start, i - start)) return 0;
        if (esc) { if (!a_str(o, esc)) return 0; }
        else { char u[6] = { '\\', 'u', '0', '0', "0123456789abcdef"[c >> 4], "0123456789abcdef"[c & 15] }; if (!a_put(o, u, 6)) return 0; }
        start = i + 1;
    }
    return a_put(o, s + start, n - start) && a_put(o, "\"", 1);
}

/* ---------------------------------------------------------------- big integers (for exact float text and parsing) */
#define BN 200
typedef struct { u32 n; u32 d[BN]; } Big; /* little-endian 32-bit limbs */
static void bn_set(Big *a, u64 v) { a->n = 0; while (v) { a->d[a->n++] = (u32)v; v >>= 32; } }
static void bn_trim(Big *a) { while (a->n && !a->d[a->n - 1]) a->n--; }
static void bn_mul_small(Big *a, u32 m) { u64 c = 0; for (u32 i = 0; i < a->n; i++) { u64 t = (u64)a->d[i] * m + c; a->d[i] = (u32)t; c = t >> 32; } if (c) { if (a->n == BN) fatal("big"); a->d[a->n++] = (u32)c; } }
static void bn_add_small(Big *a, u32 v) { u64 c = v; for (u32 i = 0; i < a->n && c; i++) { u64 t = (u64)a->d[i] + c; a->d[i] = (u32)t; c = t >> 32; } if (c) { if (a->n == BN) fatal("big"); a->d[a->n++] = (u32)c; } }
static void bn_shl(Big *a, u32 k) {
    if (!a->n) return;
    u32 w = k / 32, b = k % 32;
    if (a->n + w + 1 > BN) fatal("big");
    a->d[a->n + w] = 0;
    for (i32 i = (i32)a->n - 1; i >= 0; i--) { u32 v = a->d[i]; a->d[i + w + 1] |= b ? v >> (32 - b) : 0; a->d[i + w] = v << b; }
    for (u32 i = 0; i < w; i++) a->d[i] = 0;
    a->n += w + 1; bn_trim(a);
}
static void bn_pow10(Big *a, u32 k) { while (k >= 9) { bn_mul_small(a, 1000000000u); k -= 9; } while (k--) bn_mul_small(a, 10); }
static int bn_cmp(const Big *a, const Big *b) { if (a->n != b->n) return a->n < b->n ? -1 : 1; for (i32 i = (i32)a->n - 1; i >= 0; i--) if (a->d[i] != b->d[i]) return a->d[i] < b->d[i] ? -1 : 1; return 0; }
static void bn_add(Big *r, const Big *a, const Big *b) { /* r = a + b (r may be a) */
    u32 n = a->n > b->n ? a->n : b->n; u64 c = 0;
    for (u32 i = 0; i < n; i++) { u64 t = c + (i < a->n ? a->d[i] : 0) + (i < b->n ? b->d[i] : 0); r->d[i] = (u32)t; c = t >> 32; }
    r->n = n; if (c) { if (n == BN) fatal("big"); r->d[r->n++] = (u32)c; }
}
static void bn_sub(Big *a, const Big *b) { /* a -= b, a >= b */
    i64 br = 0;
    for (u32 i = 0; i < a->n; i++) { i64 t = (i64)a->d[i] - (i < b->n ? b->d[i] : 0) - br; br = t < 0; a->d[i] = (u32)(t + (br ? ((i64)1 << 32) : 0)); }
    bn_trim(a);
}
static u32 bn_bits(const Big *a) { if (!a->n) return 0; u32 top = a->d[a->n - 1], b = 0; while (top) { b++; top >>= 1; } return (a->n - 1) * 32 + b; }
static int bn_cmp_sum(const Big *a, const Big *b, const Big *c) { Big t; bn_add(&t, a, b); return bn_cmp(&t, c); } /* compare a + b with c */

/* The shortest digits that read back as `v` (> 0, finite), and k with v = 0.digits * 10^k (Burger and Dybvig's
   free-format algorithm, exact with big integers; of two shortest candidates the nearer, an exact tie rounding up). */
static int shortest(double v, char *dig, i32 *kout) {
    u64 bits; memcpy(&bits, &v, 8);
    u64 mant = bits & ((1ULL << 52) - 1); i32 ex = (i32)((bits >> 52) & 0x7ff);
    u64 f = ex ? mant | (1ULL << 52) : mant; i32 e = ex ? ex - 1075 : -1074;
    int even = (f & 1) == 0;
    Big r, s, mp, mm;
    if (e >= 0) {
        if (f != (1ULL << 52)) { bn_set(&r, f); bn_shl(&r, (u32)e + 1); bn_set(&s, 2); bn_set(&mp, 1); bn_shl(&mp, (u32)e); bn_set(&mm, 1); bn_shl(&mm, (u32)e); }
        else { bn_set(&r, f); bn_shl(&r, (u32)e + 2); bn_set(&s, 4); bn_set(&mp, 1); bn_shl(&mp, (u32)e + 1); bn_set(&mm, 1); bn_shl(&mm, (u32)e); }
    } else {
        if (ex <= 1 || f != (1ULL << 52)) { bn_set(&r, f); bn_shl(&r, 1); bn_set(&s, 1); bn_shl(&s, (u32)(1 - e)); bn_set(&mp, 1); bn_set(&mm, 1); }
        else { bn_set(&r, f); bn_shl(&r, 2); bn_set(&s, 1); bn_shl(&s, (u32)(2 - e)); bn_set(&mp, 2); bn_set(&mm, 1); }
    }
    /* estimate k = ceil(log10 v) from the binary exponent, then correct it */
    u32 fb = 0; { u64 t = f; while (t) { fb++; t >>= 1; } }
    double lg = ((double)(e + (i32)fb) - 1.0) * 0.30102999566398119521 - 1e-10;
    i32 k = (i32)lg; if ((double)k < lg) k++;
    if (k >= 0) bn_pow10(&s, (u32)k); else { bn_pow10(&r, (u32)-k); bn_pow10(&mp, (u32)-k); bn_pow10(&mm, (u32)-k); }
    for (;;) { int c = bn_cmp_sum(&r, &mp, &s); if (even ? c >= 0 : c > 0) { bn_mul_small(&s, 10); k++; } else break; }
    for (;;) { Big t; bn_add(&t, &r, &mp); bn_mul_small(&t, 10); int c = bn_cmp(&t, &s); if (even ? c < 0 : c <= 0) { bn_mul_small(&r, 10); bn_mul_small(&mp, 10); bn_mul_small(&mm, 10); k--; } else break; }
    int n = 0;
    for (;;) {
        bn_mul_small(&r, 10); bn_mul_small(&mp, 10); bn_mul_small(&mm, 10);
        int d = 0; while (bn_cmp(&r, &s) >= 0) { bn_sub(&r, &s); d++; }
        int c1 = bn_cmp(&r, &mm); int tc1 = even ? c1 <= 0 : c1 < 0;
        int c2 = bn_cmp_sum(&r, &mp, &s); int tc2 = even ? c2 >= 0 : c2 > 0;
        if (!tc1 && !tc2) { dig[n++] = (char)('0' + d); if (n >= 40) break; continue; }
        if (tc1 && tc2) { Big t = r; bn_shl(&t, 1); if (bn_cmp(&t, &s) >= 0) d++; } /* the nearer candidate; an exact tie rounds up, as Rust's formatter does */
        else if (tc2) d++;
        dig[n++] = (char)('0' + d);
        break;
    }
    *kout = k;
    return n;
}

/* A float as text, as JavaScript writes numbers (see serve.rs `float_text`). */
static int a_float(Area *o, double f) {
    if (f != f) return a_str(o, "NaN");
    if (f > 1.7976931348623157e308) return a_str(o, "Infinity");
    if (f < -1.7976931348623157e308) return a_str(o, "-Infinity");
    if (f == 0) return a_put(o, "0", 1);
    if (f < 0) { if (!a_put(o, "-", 1)) return 0; f = -f; }
    char d[48]; i32 n;
    int k = shortest(f, d, &n); /* k digits, value 0.d * 10^n */
    if (k <= n && n <= 21) { if (!a_put(o, d, k)) return 0; for (i32 i = 0; i < n - k; i++) if (!a_put(o, "0", 1)) return 0; return 1; }
    if (0 < n && n <= 21) return a_put(o, d, n) && a_put(o, ".", 1) && a_put(o, d + n, k - n);
    if (-6 < n && n <= 0) { if (!a_put(o, "0.", 2)) return 0; for (i32 i = 0; i < -n; i++) if (!a_put(o, "0", 1)) return 0; return a_put(o, d, k); }
    if (!a_put(o, d, 1)) return 0;
    if (k > 1 && !(a_put(o, ".", 1) && a_put(o, d + 1, k - 1))) return 0;
    return a_put(o, "e", 1) && a_put(o, n - 1 >= 0 ? "+" : "-", 1) && a_i64(o, n - 1 >= 0 ? n - 1 : 1 - n);
}
static int a_json_float(Area *o, double f) { if (f != f || f > 1.7976931348623157e308 || f < -1.7976931348623157e308) return a_str(o, "null"); return a_float(o, f); }

/* The double nearest to digits * 10^exp10 (ties to even), as Rust's `str::parse::<f64>` gives. `digits` are ASCII, at
   most 800 kept (a dropped nonzero digit is kept as a sticky low digit, so rounding stays exact). */
static double exact_double(const u8 *dig, u32 nd, i64 exp10) {
    Big num, den;
    bn_set(&num, 0);
    for (u32 i = 0; i < nd; i++) { bn_mul_small(&num, 10); bn_add_small(&num, (u32)(dig[i] - '0')); }
    if (!num.n) return 0.0;
    if (exp10 > 400) return 1.0 / 0.0;
    if (exp10 < -1200) return 0.0;
    bn_set(&den, 1);
    if (exp10 >= 0) bn_pow10(&num, (u32)exp10); else bn_pow10(&den, (u32)-exp10);
    /* find b with 2^53 <= num / (den * 2^b) < 2^54, to round to 53 bits */
    i32 b = (i32)bn_bits(&num) - (i32)bn_bits(&den) - 54;
    for (;;) {
        Big n2 = num, d2 = den;
        if (b >= 0) bn_shl(&d2, (u32)b); else bn_shl(&n2, (u32)-b);
        Big lo = d2; bn_shl(&lo, 53); Big hi = d2; bn_shl(&hi, 54);
        if (bn_cmp(&n2, &lo) < 0) { b--; continue; }
        if (bn_cmp(&n2, &hi) >= 0) { b++; continue; }
        break;
    }
    /* the value's binary exponent is b + 53; below the normal range the step is fixed at 2^-1074 */
    i32 e2 = b; /* value = q * 2^e2 with q in [2^53, 2^54) */
    i32 shift = 1; /* bits dropped from the 54-bit quotient: keep 53 */
    if (e2 + 1 < -1074) shift = -1074 - e2;
    if (shift > 60) return 0.0;
    Big n2 = num, d2 = den;
    i32 bb = e2 + shift;
    if (bb >= 0) bn_shl(&d2, (u32)bb); else bn_shl(&n2, (u32)-bb);
    /* q = floor(n2 / d2) (fits 54 bits), remainder decides rounding */
    u64 q = 0;
    { Big dd = d2; u32 sh = 0; while (bn_cmp(&dd, &n2) <= 0) { bn_shl(&dd, 1); sh++; } while (sh--) { /* undo one shift */
            Big t = d2; bn_shl(&t, sh); q <<= 1; if (bn_cmp(&n2, &t) >= 0) { bn_sub(&n2, &t); q |= 1; } } }
    Big twice = n2; bn_shl(&twice, 1);
    int c = bn_cmp(&twice, &d2);
    if (c > 0 || (c == 0 && (q & 1))) q++;
    i32 e = bb;
    if (q >> 53) { q >>= 1; e++; }
    if (e + 52 > 1023) return 1.0 / 0.0;
    double r = (double)q;
    /* r * 2^e, exactly (q has at most 53 bits) */
    while (e > 0) { i32 s = e > 60 ? 60 : e; r *= (double)(1ULL << s); e -= s; }
    while (e < 0) { i32 s = -e > 60 ? 60 : -e; r /= (double)(1ULL << s); e += s; }
    return r;
}

/* ---------------------------------------------------------------- the program's memory */
#define LEN 0
#define HEAD 1
#define HW 0
#define COUNT 1
#define HINT 2
#define EVER 3
#define RETRY_AFTER_SHRINKS 256ULL

typedef struct {
    const MitoProg *prog;
    u8 *base; u64 reserved, npages, cpages;
    u8 *committed;       /* one flag per page */
    u8 *livepg;          /* trim's scratch: one flag per page */
    u64 *shrinks;        /* per collection (top level): bumped whenever something in it is freed or cleared */
    Grow jb; int jon;
    u8 *dirty;           /* one bit per 64-byte line of the reservation: set once the request has saved the line */
    int all;             /* all of the state was committed at start (no memory pressure, at most COMMIT_ALL_MAX): nothing needs committing, so ensure() returns at once */
} Mem;

static const MitoColl *coll(Mem *m, u32 c) { return &m->prog->colls[c]; }
#if defined(__clang__) || defined(__GNUC__)
#define NOINLINE_ __attribute__((noinline))
#else
#define NOINLINE_
#endif
static void ensure_pages(Mem *m, u64 a, u64 b);
/* commit [a, b) if it is not yet: nothing when all of the state was committed at start (the test stays in line, the work does not) */
static inline void ensure(Mem *m, u64 a, u64 b) { if (m->all || b <= a) return; ensure_pages(m, a, b); }
static NOINLINE_ void ensure_pages(Mem *m, u64 a, u64 b) {
    u64 p = a / PAGE, last = (b - 1) / PAGE;
    while (p <= last) {
        if (!m->committed[p]) {
            u64 s = p;
            while (p <= last && !m->committed[p]) p++;
            if (!os_commit(m->base + s * PAGE, (p - s) * PAGE)) fatal("commit failed");
            { u64 d0 = s * (PAGE / 64 / 8) & ~(PAGE - 1), d1 = up(p * (PAGE / 64 / 8), PAGE); if (!os_commit(m->dirty + d0, d1 - d0)) fatal("commit failed"); } /* the dirty map: 8 bytes per page */
            for (u64 q = s; q < p; q++) m->committed[q] = 1;
            m->cpages += p - s;
        } else p++;
    }
}
static u64 rd(Mem *m, u64 off, u32 n) { u64 v = 0; memcpy(&v, m->base + off, n); return v; }
/* The undo log of a request: the first write to each 64-byte line saves the whole line (its address, then 64 bytes);
   `dirty` marks the lines saved, so later writes to a saved line cost one bit test. Rollback puts the lines back. */
#define LINE 64ULL
static void save(Mem *m, u64 a, u64 n) {
#ifdef MITO_MEASURE_NO_JOURNAL /* for measuring what journaling costs only: failed requests are then not rolled back */
    return;
#endif
    if (!m->jon || !n) return;
    for (u64 l = a / LINE, last = (a + n - 1) / LINE; l <= last; l++) {
        u8 bit = (u8)(1u << (l & 7));
        if (!(l & 63) && l + 63 <= last) { u64 w; memcpy(&w, m->dirty + (l >> 3), 8); if (w == ~0ULL) { l += 63; continue; } } /* 64 lines already saved */
        if (m->dirty[l >> 3] & bit) continue;
        m->dirty[l >> 3] |= bit;
        u64 pos = m->jb.len, end = pos + 8 + LINE;
        if (UNLIKELY(end > m->jb.committed) && !grow_to(&m->jb, end)) fatal("journal full");
        u64 at = l * LINE; memcpy(m->jb.p + pos, &at, 8); memcpy(m->jb.p + pos + 8, m->base + at, LINE);
        m->jb.len = end;
    }
}
static void wr(Mem *m, u64 off, u32 n, u64 v) { ensure(m, off, off + n); save(m, off, n); memcpy(m->base + off, &v, n); }
static void put(Mem *m, u64 off, const u8 *b, u64 n) { ensure(m, off, off + n); save(m, off, n); if (n) memcpy(m->base + off, b, n); }
static void zero(Mem *m, u64 off, u64 n) { if (!n) return; ensure(m, off, off + n); save(m, off, n); memset(m->base + off, 0, n); }
static void begin(Mem *m) { m->jon = 1; m->jb.len = 0; }
static void commit_changes(Mem *m) {
    for (u64 pos = 0; pos < m->jb.len; pos += 8 + LINE) { u64 at; memcpy(&at, m->jb.p + pos, 8); u64 l = at / LINE; m->dirty[l >> 3] &= (u8)~(1u << (l & 7)); }
    m->jon = 0; m->jb.len = 0;
}
static void rollback(Mem *m) {
    for (u64 pos = m->jb.len; pos > 0;) {
        pos -= 8 + LINE;
        u64 at; memcpy(&at, m->jb.p + pos, 8); memcpy(m->base + at, m->jb.p + pos + 8, LINE);
    }
    commit_changes(m);
}

static u32 hdr_fields(const MitoColl *c) { return c->list ? 2 : 4; }
static u64 words(const MitoColl *c) { return (c->cap + 63) / 64; }
static u64 haddr(Mem *m, u32 c, u32 f, u64 inst) { const MitoColl *k = coll(m, c); return k->hdr + 4 * (f * (u64)k->mult + inst); }
static u64 hget(Mem *m, u32 c, u32 f, u64 inst) { return rd(m, haddr(m, c, f, inst), 4); }
static void hset(Mem *m, u32 c, u32 f, u64 inst, u64 v) { wr(m, haddr(m, c, f, inst), 4, v); }
/* A pool's four header words at once: with one instance they are the first 16 bytes of the header, so one ensure, one
   undo-log check and one store do for all four. */
static void hset4(Mem *m, u32 c, u64 inst, u64 hw, u64 cnt, u64 hint, u64 ever) {
    const MitoColl *k = coll(m, c);
    if (k->mult == 1) {
        u32 v[4] = { (u32)hw, (u32)cnt, (u32)hint, (u32)ever };   /* HW, COUNT, HINT, EVER */
        ensure(m, k->hdr, k->hdr + 16); save(m, k->hdr, 16); memcpy(m->base + k->hdr, v, 16);
    } else { hset(m, c, HW, inst, hw); hset(m, c, COUNT, inst, cnt); hset(m, c, HINT, inst, hint); hset(m, c, EVER, inst, ever); }
}
static u64 waddr(Mem *m, u32 c, u64 inst, u64 w) { const MitoColl *k = coll(m, c); return k->live + 8 * (inst * words(k) + w); }
static int bit(Mem *m, u32 c, u64 inst, u64 s) { return (int)((rd(m, waddr(m, c, inst, s / 64), 8) >> (s % 64)) & 1); }
static u64 gaddr(Mem *m, u32 c, u64 idx) { return coll(m, c)->gen + 4 * idx; }
static u32 gen_of(Mem *m, u32 c, u64 idx) { return coll(m, c)->checked ? (u32)rd(m, gaddr(m, c, idx), 4) : 0; }
static u64 count(Mem *m, u32 c, u64 inst) { return hget(m, c, coll(m, c)->list ? LEN : COUNT, inst); }

static int item_live(Mem *m, u32 c, u64 idx) {
    const MitoColl *k = coll(m, c); u64 cap = k->cap, inst = k->mult == 1 ? 0 : idx / cap, s = k->mult == 1 ? idx : idx % cap; /* one instance: the index is the slot */
    if (inst >= k->mult) return 0;
    if (k->list) return (s + cap - hget(m, c, HEAD, inst)) % cap < hget(m, c, LEN, inst);
    return s < hget(m, c, HW, inst) && bit(m, c, inst, s);
}
/* Live items of an instance in order (lists oldest first, pools by slot): call with i = 0, 1, ... until it returns 0. */
static int item_at(Mem *m, u32 c, u64 inst, u64 *it, u64 *idx) {
    const MitoColl *k = coll(m, c); u64 cap = k->cap;
    if (k->list) { if (*it >= hget(m, c, LEN, inst)) return 0; *idx = inst * cap + (hget(m, c, HEAD, inst) + *it) % cap; (*it)++; return 1; }
    u64 hw = hget(m, c, HW, inst);
    while (*it < hw) { u64 s = (*it)++; if (bit(m, c, inst, s)) { *idx = inst * cap + s; return 1; } }
    return 0;
}
static u32 root(Mem *m, u32 c) { while (coll(m, c)->parent >= 0) c = (u32)coll(m, c)->parent; return c; }
static void shrank(Mem *m, u32 c) { m->shrinks[root(m, c)]++; }

static void reset_instance(Mem *m, u32 c, u64 inst);
static void reset_children(Mem *m, u32 c, u64 idx) { const MitoColl *k = coll(m, c); for (u32 i = 0; i < k->nkids; i++) reset_instance(m, k->kids[i].coll, idx); }
static void zero_item(Mem *m, u32 c, u64 idx) {
    const MitoColl *k = coll(m, c);
    for (u32 i = 0; i < k->ncols; i++) if (k->cols[i].kind != 3) zero(m, k->cols[i].off + idx * k->cols[i].size, k->cols[i].size);
    reset_children(m, c, idx);
}
/* Empty one instance of a collection, including what its items hold. */
static void reset_instance(Mem *m, u32 c, u64 inst) {
    const MitoColl *k = coll(m, c);
    shrank(m, c);
    for (u32 f = 0; f < hdr_fields(k); f++) { u64 h = haddr(m, c, f, inst); ensure(m, h, h + 4); } /* a new parent's header may not be committed yet */
    if (k->nkids) { u64 it = 0, idx; while (item_at(m, c, inst, &it, &idx)) reset_children(m, c, idx); }
    if (k->list) { hset(m, c, LEN, inst, 0); hset(m, c, HEAD, inst, 0); return; }
    u64 hw = hget(m, c, HW, inst);
    if (k->checked) { u64 it = 0, idx; while (item_at(m, c, inst, &it, &idx)) { u64 ga = gaddr(m, c, idx); wr(m, ga, 4, rd(m, ga, 4) + 1); } }
    zero(m, waddr(m, c, inst, 0), (hw + 63) / 64 * 8);
    hset(m, c, HW, inst, 0); hset(m, c, COUNT, inst, 0); hset(m, c, HINT, inst, 0);
    if (!k->checked && !k->nkids) {
        /* zero what the items held now, in one pass per column, and forget that the slots were ever used: the adds that refill the pool then find zeroed memory, as in a fresh pool */
        u64 ever = hget(m, c, EVER, inst);
        for (u32 i = 0; i < k->ncols; i++) if (k->cols[i].kind != 3) zero(m, k->cols[i].off + inst * k->cap * k->cols[i].size, ever * k->cols[i].size);
        hset(m, c, EVER, inst, 0);
    }
}
static int is_full(Mem *m, u32 c, u64 inst) {
    const MitoColl *k = coll(m, c);
    if (k->list) return k->drop_oldest ? 0 : hget(m, c, LEN, inst) >= k->cap;
    return hget(m, c, COUNT, inst) >= k->cap;
}
/* Append to a list (dropping the oldest if declared) or take the lowest free slot of a pool. ~0 when full. */
/* commit and undo-log a small range (within one page and one line is the usual case) for a write that follows: the two checks inline, the general routines only when one fails */
static inline void touch(Mem *m, u64 a, u32 n) {
    if (!m->all && UNLIKELY(!m->committed[a / PAGE] || (a + n - 1) / PAGE != a / PAGE)) ensure(m, a, a + n);
    if (m->jon) { u64 l = a / LINE; if (UNLIKELY((a + n - 1) / LINE != l || !(m->dirty[l >> 3] & (1u << (l & 7))))) save(m, a, n); }
}
static u64 push_item(Mem *m, u32 c, u64 inst) {
    const MitoColl *k = coll(m, c); u64 cap = k->cap;
    if (k->list) {
        u64 len = hget(m, c, LEN, inst), head = hget(m, c, HEAD, inst);
        if (len < cap) { u64 idx = inst * cap + (head + len) % cap; zero_item(m, c, idx); hset(m, c, LEN, inst, len + 1); return idx; }
        if (k->drop_oldest) { u64 idx = inst * cap + head; zero_item(m, c, idx); hset(m, c, HEAD, inst, (head + 1) % cap); return idx; }
        return ~0ULL;
    }
    if (k->mult == 1 && !k->checked && k->nkids == 0) {
        /* the common pool: one instance, no generations, no nested collections. Adding at the high-water mark (nothing freed below it) needs no search */
        u32 h[4]; memcpy(h, m->base + k->hdr, 16); /* HW, COUNT, HINT, EVER: committed at start */
        if (h[2] == h[0] && h[0] < cap) {
            u64 slot = h[0];
            u64 wa = k->live + 8 * (slot / 64);
            u64 word = slot % 64 ? rd(m, wa, 8) : 0; /* a word wholly above the high-water mark may be uncommitted; its bits are zero */
            touch(m, wa, 8); word |= 1ULL << (slot % 64); memcpy(m->base + wa, &word, 8);
            for (u32 i = 0; i < k->ncols; i++) {
                const MitoCol *col = &k->cols[i];
                if (col->kind == 3) continue;
                u64 a = col->off + slot * col->size;
                touch(m, a, col->size);
                if (slot < h[3]) memset(m->base + a, 0, col->size); /* a reused slot is cleared; a never-used one is zero already */
            }
            u32 v[4] = { h[0] + 1, h[1] + 1, h[0] + 1, h[3] > h[0] + 1 ? h[3] : h[0] + 1 };
            touch(m, k->hdr, 16); memcpy(m->base + k->hdr, v, 16);
            return slot;
        }
    }
    u64 hw = hget(m, c, HW, inst), cnt = hget(m, c, COUNT, inst), hint = hget(m, c, HINT, inst);
    u64 slot = ~0ULL, w = hint / 64;
    while (w * 64 < hw) {
        u64 fr = ~rd(m, waddr(m, c, inst, w), 8);
        if ((w + 1) * 64 > hw) fr &= (1ULL << (hw - w * 64)) - 1;
        if (w == hint / 64) fr &= ~0ULL << (hint % 64);
        if (fr) { u64 t = 0; while (!((fr >> t) & 1)) t++; slot = w * 64 + t; break; }
        w++;
    }
    if (slot == ~0ULL) { if (hw == cap) return ~0ULL; slot = hw; }
    u64 wa = waddr(m, c, inst, slot / 64);
    u64 word = (slot / 64) * 64 < hw ? rd(m, wa, 8) : 0; /* words wholly above hw may be uncommitted; their bits are zero */
    wr(m, wa, 8, word | (1ULL << (slot % 64)));
    u64 idx = inst * cap + slot;
    u64 ever = hget(m, c, EVER, inst);
    if (slot >= ever && k->nkids == 0) {
        /* never used: the memory is zero already (fresh pages, or restored to zero by a rollback), it only needs committing */
        for (u32 i = 0; i < k->ncols; i++) if (k->cols[i].kind != 3) ensure(m, k->cols[i].off + idx * k->cols[i].size, k->cols[i].off + (idx + 1) * k->cols[i].size);
    } else zero_item(m, c, idx);
    if (k->checked) { u64 ga = gaddr(m, c, idx); ensure(m, ga, ga + 4); }
    u64 nhw = hw > slot + 1 ? hw : slot + 1;
    if (nhw > ever) ever = nhw;
    hset4(m, c, inst, nhw, cnt + 1, slot + 1, ever);
    return idx;
}
/* Free a pool slot (the caller checked it is a live pool item). */
static void free_item(Mem *m, u32 c, u64 idx) {
    const MitoColl *k = coll(m, c); u64 cap = k->cap, inst = k->mult == 1 ? 0 : idx / cap, s = k->mult == 1 ? idx : idx % cap;
    if (k->list || !item_live(m, c, idx)) return;
    shrank(m, c);
    reset_children(m, c, idx);
    u64 wa = waddr(m, c, inst, s / 64);
    wr(m, wa, 8, rd(m, wa, 8) & ~(1ULL << (s % 64)));
    if (k->checked) { u64 ga = gaddr(m, c, idx); wr(m, ga, 4, rd(m, ga, 4) + 1); }
    u64 cnt = hget(m, c, COUNT, inst), hint = hget(m, c, HINT, inst), hw = hget(m, c, HW, inst), ever = hget(m, c, EVER, inst);
    u64 nhw = hw;
    if (s + 1 == hw) {
        u64 h = s;
        while (h > 0) {
            u64 w = (h - 1) / 64, word = rd(m, waddr(m, c, inst, w), 8), valid = h - w * 64;
            if (valid < 64) word &= (1ULL << valid) - 1;
            if (!word) h = w * 64; else { u64 lz = 0; while (!((word >> (63 - lz)) & 1)) lz++; h = w * 64 + (64 - lz); break; }
        }
        nhw = h;
    }
    hset4(m, c, inst, nhw, cnt - 1, hint < s ? hint : s, ever);
}

/* ---- live ranges: the bytes in use, as trims and the live counts see them ---- */
typedef void (*RangeFn)(Mem *m, u64 a, u64 b, void *ctx);
static void inst_ranges(Mem *m, u32 c, u64 inst, RangeFn f, void *x) {
    const MitoColl *k = coll(m, c); u64 cap = k->cap;
    for (u32 h = 0; h < hdr_fields(k); h++) { u64 a = haddr(m, c, h, inst); f(m, a, a + 4, x); }
    u64 run[2][2]; int nr;
    if (!k->list) {
        u64 hw = hget(m, c, HW, inst);
        if (k->checked) { u64 ever = hget(m, c, EVER, inst); f(m, k->gen + 4 * inst * cap, k->gen + 4 * (inst * cap + ever), x); }
        if (!hw) return;
        f(m, waddr(m, c, inst, 0), waddr(m, c, inst, 0) + (hw + 63) / 64 * 8, x);
        run[0][0] = 0; run[0][1] = hw; nr = 1;
    } else {
        u64 len = hget(m, c, LEN, inst), head = hget(m, c, HEAD, inst);
        if (!len) return;
        run[0][0] = head; run[0][1] = head + len < cap ? head + len : cap; nr = 1;
        if (head + len > cap) { run[1][0] = 0; run[1][1] = head + len - cap; nr = 2; }
    }
    for (u32 ci = 0; ci < k->ncols; ci++) {
        const MitoCol *col = &k->cols[ci]; u64 sz = col->size;
        if (col->kind == 3) {
            const MitoCol *lc = &k->cols[ci - 1];
            for (int r = 0; r < nr; r++) for (u64 s = run[r][0]; s < run[r][1]; s++) {
                if (!k->list && !bit(m, c, inst, s)) continue;
                u64 idx = inst * cap + s, l = rd(m, lc->off + 4 * idx, 4);
                if (l) f(m, col->off + idx * col->n, col->off + idx * col->n + l, x);
            }
        } else for (int r = 0; r < nr; r++) f(m, col->off + (inst * cap + run[r][0]) * sz, col->off + (inst * cap + run[r][1]) * sz, x);
    }
}
/* Every existing instance of a collection: one at top level, one per live item of the parent otherwise. */
static void insts_rec(Mem *m, const u32 *chain, u32 depth, u32 d, u64 inst, RangeFn f, void *x) {
    if (depth == d) { inst_ranges(m, chain[d], inst, f, x); return; }
    u64 it = 0, idx; while (item_at(m, chain[depth], inst, &it, &idx)) insts_rec(m, chain, depth + 1, d, idx, f, x);
}
static void coll_ranges(Mem *m, u32 c, RangeFn f, void *x) {
    u32 chain[32], d = 0, k = c;
    while (coll(m, k)->parent >= 0) k = (u32)coll(m, k)->parent, d++;
    if (d >= 32) fatal("nesting");
    k = c; for (i32 i = (i32)d; i >= 0; i--) { chain[i] = k; if (i) k = (u32)coll(m, k)->parent; }
    insts_rec(m, chain, 0, d, 0, f, x);
}
static void coll_ranges_deep(Mem *m, u32 c, RangeFn f, void *x) { coll_ranges(m, c, f, x); const MitoColl *k = coll(m, c); for (u32 i = 0; i < k->nkids; i++) coll_ranges_deep(m, k->kids[i].coll, f, x); }
static void sum_range(Mem *m, u64 a, u64 b, void *x) { (void)m; *(u64 *)x += b - a; }
static void mark_range(Mem *m, u64 a, u64 b, void *x) { (void)x; if (b > a) for (u64 p = a / PAGE; p <= (b - 1) / PAGE; p++) m->livepg[p] = 1; }
static u64 coll_live(Mem *m, u32 c) { u64 t = 0; coll_ranges(m, c, sum_range, &t); return t; }
static u64 live_total(Mem *m) { u64 t = m->prog->plain_end; for (u32 c = 0; c < m->prog->ncolls; c++) t += coll_live(m, c); return t; }
static u64 coll_committed(Mem *m, u32 c) { const MitoColl *k = coll(m, c); u64 n = 0; for (u64 p = k->start / PAGE; p < up(k->end, PAGE) / PAGE; p++) n += m->committed[p]; return n * PAGE; }
static u64 release_dead(Mem *m, u64 lo, u64 hi) {
    u64 freed = 0, p = lo;
    while (p < hi) {
        if (m->committed[p] && !m->livepg[p]) {
            u64 s = p;
            while (p < hi && m->committed[p] && !m->livepg[p]) p++;
            os_decommit(m->base + s * PAGE, (p - s) * PAGE);
            for (u64 q = s; q < p; q++) m->committed[q] = 0;
            m->cpages -= p - s; freed += (p - s) * PAGE;
        } else p++;
    }
    return freed;
}
static u64 trim_all(Mem *m) {
    memset(m->livepg, 0, m->npages);
    mark_range(m, 0, m->prog->plain_end, 0);
    for (u32 c = 0; c < m->prog->ncolls; c++) coll_ranges(m, c, mark_range, 0);
    return release_dead(m, 0, m->npages);
}
static u64 trim_coll(Mem *m, u32 c) {
    memset(m->livepg, 0, m->npages);
    coll_ranges_deep(m, c, mark_range, 0);
    const MitoColl *k = coll(m, c);
    return release_dead(m, k->start / PAGE, up(k->end, PAGE) / PAGE);
}

/* ---- the memory-pressure policy (DESIGN.md concept 9) ---- */
typedef struct { int on; u64 soft, hard; int refusing; u64 *stuck; u64 trims, full_trims, freed; } Pressure;
static i64 trim_candidate(Mem *m, Pressure *p) {
    i64 best = -1; u64 bestc = 0;
    for (u32 c = 0; c < m->prog->ncolls; c++) {
        if (coll(m, c)->parent >= 0) continue;
        int worth = p->stuck[c] == ~0ULL || m->shrinks[c] >= p->stuck[c] + RETRY_AFTER_SHRINKS;
        if (!worth) continue;
        u64 cc = coll_committed(m, c);
        if (cc > 0 && (best < 0 || cc > bestc)) { best = c; bestc = cc; }
    }
    return best;
}
static void trim_one(Mem *m, Pressure *p, u32 c) { u64 f = trim_coll(m, c); p->trims++; p->freed += f; p->stuck[c] = f == 0 ? m->shrinks[c] : ~0ULL; }
static void relieve(Mem *m, Pressure *p) {
    if (m->cpages * PAGE > p->soft) { i64 c = trim_candidate(m, p); if (c >= 0) trim_one(m, p, (u32)c); }
    if (m->cpages * PAGE > p->hard && !p->refusing) { p->freed += trim_all(m); p->full_trims++; }
    p->refusing = m->cpages * PAGE > p->hard;
}

/* ---------------------------------------------------------------- a running program */
#define TEXT_CAP (4 * 1024 * 1024)
#if defined(MITO_MEASURE_NO_COMMIT) && !defined(COMMIT_ALL_MAX)
#define COMMIT_ALL_MAX (~0ULL) /* measuring only: commit all state at start whatever its size */
#endif
#ifndef COMMIT_ALL_MAX
#define COMMIT_ALL_MAX (1ULL << 30) /* state up to 1 GiB is committed whole at start (see MITO(start)) */
#endif
#define RESPONSE_CAP (256 * 1024)
/* The declared limits of a request, the same in every runtime (serve.rs MAX_QUERY ...). A route has at most 32 parts
   (checked when the program loads), names at most 128 bytes and fail messages 1024, so messages fit `ebuf`. */
#define MAX_PARAMS 32
#define MAX_QUERY 64
#define MAX_JSON 256
#define MAX_FREES 65536
#define QUOTE 64
#define MAX_SEL (1 << 24)

/* What went wrong in a request: the error kinds of the Rust runtime. */
enum { E_NONE, E_FAIL, E_FULL, E_TOOLONG, E_NOTLIVE, E_STALE, E_BAD, E_DEAD, E_FORWARD };
#define MAX_FWD 64 /* fields of one forward */
typedef struct { u32 off, len; } T; /* a string in the text area */
typedef struct { T k; Val v; } JField;

typedef struct Mito {
    Mem mem;
    Area out, text;
    u8 *areas;
    i64 clock; int fixed_clock; u64 rng;
    i64 loop_budget; /* rounds of `while` and `for` loops one request may run (`--loop-budget`) */
    u32 text_base;   /* where a request's text starts: after the literals, or after a running test's own text */
    /* a test's request being made: its body (JSON), its headers, as names and values in the text area */
    u8 rqbuf[16384]; u32 nrqh; struct { T k, v; } rqh[16];
    Pressure pr;
    /* the request being handled */
    u32 ei; const MitoPair *hdrs; u32 nhdrs; const MitoPair *fwd; u32 nfwd; i64 now;
    u32 nparams; struct { u32 seg; T t; } params[MAX_PARAMS];
    u32 nquery; struct { T k, v; } query[MAX_QUERY];
    u32 njson; JField json[MAX_JSON];
    int ekind; int ecode; Area emsg; u8 ebuf[4096];
    Grow frees; /* (collection, item) pairs, u32 each */
    Grow sel;   /* items collected by loops (u32 each) */
    int crashed;
    /* in a system of workers: every kind's name and count, and which worker this is */
    int insys; u32 nkinds, me_kind, me_index; const char *const *kind_names; const u32 *kind_counts;
    u32 nfwd_out; struct { T k, v; } fwd_out[MAX_FWD]; /* the fields of the forward being made */
    int pending; u32 fwd_kind; i64 fwd_key;         /* a forward waiting for MITO(finish_forward) */
} Mito;

/* ---- errors: kind, status and message ---- */
static int err(Mito *m, int kind, int code) { m->ekind = kind; m->ecode = code; m->emsg.len = 0; return -1; }
static int fail(Mito *m, int code, const char *msg) { err(m, E_FAIL, code); a_str(&m->emsg, msg); return -1; }
static int text_full(Mito *m) { return fail(m, 507, "request text area is full"); }
static int resp_full(Mito *m) { return fail(m, 507, "response too large"); }

static T t_since(Mito *m, u32 start) { T t = { start, m->text.len - start }; return t; }
static const u8 *t_ptr(Mito *m, T t) { return m->text.p + t.off; }
static int t_push(Mito *m, const void *b, u64 n, T *out) { u32 s = m->text.len; if (!a_put(&m->text, b, n)) return text_full(m); *out = t_since(m, s); return 0; }

static Val v_null(void) { Val v = { 0, 0, 0, 0, 0 }; return v; }
static Val v_int(i64 i) { Val v = { 1, 0, 0, i, 0 }; return v; }
static Val v_flt(double f) { Val v = { 2, 0, 0, 0, f }; return v; }
static Val v_bool(int b) { Val v = { 3, 0, 0, b ? 1 : 0, 0 }; return v; }
static Val v_str(T t) { Val v = { 4, t.off, t.len, 0, 0 }; return v; }
static Val v_item(u32 c, u32 i) { Val v = { 5, c, i, 0, 0 }; return v; }
static Mito *M_(Ctx *c) { return (Mito *)c->run; }

/* ---- columns ---- */
static i64 col_by_path(Mito *m, u32 c, const u8 *p, u64 n) { const MitoColl *k = coll(&m->mem, c); for (u32 i = 0; i < k->ncols; i++) if (k->cols[i].kind != 3 && c_eq(k->cols[i].path, p, n)) return i; return -1; }
/* Is `p` the path of a struct-valued field (some column or nested collection is below it)? */
static int is_prefix(Mito *m, u32 c, const u8 *p, u64 n) {
    const MitoColl *k = coll(&m->mem, c);
    for (u32 i = 0; i < k->ncols + k->nkids; i++) {
        const char *q = i < k->ncols ? k->cols[i].path : k->kids[i - k->ncols].path;
        u64 j = 0; while (j < n && q[j] && (u8)q[j] == p[j]) j++;
        if (j == n && q[n] == '.') return 1;
    }
    return 0;
}
static i64 child_by_path(Mito *m, u32 c, const u8 *p, u64 n) { const MitoColl *k = coll(&m->mem, c); for (u32 i = 0; i < k->nkids; i++) if (c_eq(k->kids[i].path, p, n)) return k->kids[i].coll; return -1; }
static u64 caddr(Mito *m, u32 c, u64 idx, u32 ci) { const MitoCol *k = &coll(&m->mem, c)->cols[ci]; return k->off + idx * k->size; }
static u32 scalar_size(u32 s) { return s == 1 || s == 5 || s == 11 ? 1 : s == 2 || s == 6 ? 2 : s == 3 || s == 7 || s == 9 ? 4 : 8; }
static const char *scalar_name(u32 s) { const char *n[] = { "", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "bool" }; return n[s]; }
static Val scalar_val(Mito *m, u64 a, u32 s) {
    u64 b = rd(&m->mem, a, scalar_size(s));
    switch (s) {
        case 9: { u32 x = (u32)b; float f; memcpy(&f, &x, 4); return v_flt((double)f); }
        case 10: { double f; memcpy(&f, &b, 8); return v_flt(f); }
        case 11: return v_bool(b != 0);
        case 1: return v_int((i64)(i8)b); case 2: return v_int((i64)(i16)b); case 3: return v_int((i64)(i32)b);
        default: return v_int((i64)b);
    }
}
static int bad_type(Mito *m) { err(m, E_BAD, 400); a_str(&m->emsg, "wrong type for this field"); return -1; }
/* Store a number or bool in a scalar of kind `s`, refusing what does not fit (as the Rust runtime's store_scalar). */
static int store_scalar(Mito *m, u64 a, u32 s, const Val *v) {
    u64 bits;
    if (s == 9 || s == 10) {
        double f; if (v->tag == 2) f = v->f; else if (v->tag == 1) f = (double)v->i; else return bad_type(m);
        if (s == 9) { float x = (float)f; u32 u; memcpy(&u, &x, 4); bits = u; } else memcpy(&bits, &f, 8);
    } else if (s == 11) { if (v->tag != 3) return bad_type(m); bits = v->i != 0; }
    else {
        if (v->tag != 1) return bad_type(m);
        i64 i = v->i, lo, hi;
        switch (s) { case 1: lo = -128; hi = 127; break; case 2: lo = -32768; hi = 32767; break; case 3: lo = -2147483647 - 1; hi = 2147483647; break;
            case 5: lo = 0; hi = 255; break; case 6: lo = 0; hi = 65535; break; case 7: lo = 0; hi = 4294967295LL; break; case 8: lo = 0; hi = I64MAX; break; default: lo = I64MIN; hi = I64MAX; }
        if (i < lo || i > hi) { err(m, E_TOOLONG, 413); a_i64(&m->emsg, i); a_str(&m->emsg, " does not fit "); a_str(&m->emsg, scalar_name(s)); return -1; }
        bits = (u64)i;
    }
    wr(&m->mem, a, scalar_size(s), bits); return 0;
}
static int store_bytes_at(Mito *m, u64 len_at, u64 bytes_at, u64 cap, const u8 *b, u64 n) {
    if (n > cap) { err(m, E_TOOLONG, 413); a_str(&m->emsg, "string of "); a_u64(&m->emsg, n); a_str(&m->emsg, " bytes does not fit string<"); a_u64(&m->emsg, cap); a_str(&m->emsg, ">"); return -1; }
    put(&m->mem, bytes_at, b, n); wr(&m->mem, len_at, 4, n); return 0;
}
static int deref_quiet(Mito *m, u32 t, u64 slot, u32 g, u64 *out) {
    const MitoColl *k = coll(&m->mem, t);
    if (slot >= k->cap || !item_live(&m->mem, t, slot)) return 0;
    if (k->checked && (u32)rd(&m->mem, gaddr(&m->mem, t, slot), 4) != g) return 0;
    *out = slot; return 1;
}
/* Store a value at column `ci` of item `idx` (element `j` of an array column when j >= 0), as the interpreter's `store`. */
static int store_col(Mito *m, u32 c, u64 idx, u32 ci, i64 j, const Val *v) {
    const MitoCol *col = &coll(&m->mem, c)->cols[ci];
    u64 a = caddr(m, c, idx, ci);
    if (v->tag == 4) {
        if (col->kind != 2 || j >= 0) return bad_type(m);
        const MitoCol *bc = &coll(&m->mem, c)->cols[ci + 1];
        return store_bytes_at(m, a, caddr(m, c, idx, ci + 1), bc->n, t_ptr(m, (T){ v->a, v->b }), v->b);
    }
    if (v->tag != 1 && v->tag != 2 && v->tag != 3) return fail(m, 500, "cannot store this value in state");
    if (col->kind == 0) return store_scalar(m, a, col->scalar, v);
    if (col->kind == 1) { if (j < 0) { err(m, E_BAD, 400); a_str(&m->emsg, "an array field needs an index"); return -1; } return store_scalar(m, a + (u64)j * scalar_size(col->scalar), col->scalar, v); }
    if (col->kind == 2) { err(m, E_BAD, 400); a_str(&m->emsg, "expected a string"); return -1; }
    return bad_type(m);
}
/* Copy a string column into the text area. */
static int col_txt(Mito *m, u32 c, u64 idx, u32 ci, T *out) {
    u64 a = caddr(m, c, idx, ci); u32 n = (u32)rd(&m->mem, a, 4);
    return t_push(m, m->mem.base + caddr(m, c, idx, ci + 1), n, out);
}

/* ---- values as text and JSON ---- */
static int show(Mito *m, const Val *v) { /* append a value's text to the text area */
    int ok;
    switch (v->tag) {
        case 4: { u32 n = v->b; if (m->text.len + (u64)n > m->text.cap) return text_full(m); memmove(m->text.p + m->text.len, m->text.p + v->a, n); m->text.len += n; return 0; }
        case 1: ok = a_i64(&m->text, v->i); break;
        case 2: ok = a_float(&m->text, v->f); break;
        case 3: ok = a_str(&m->text, v->i ? "true" : "false"); break;
        case 0: ok = a_str(&m->text, "null"); break;
        default: return fail(m, 500, "cannot show this value");
    }
    return ok ? 0 : text_full(m);
}
static int json_val(Mito *m, const Val *v) {
    int ok;
    switch (v->tag) {
        case 1: ok = a_i64(&m->out, v->i); break;
        case 2: ok = a_json_float(&m->out, v->f); break;
        case 3: ok = a_str(&m->out, v->i ? "true" : "false"); break;
        case 4: ok = a_json_str(&m->out, t_ptr(m, (T){ v->a, v->b }), v->b); break;
        case 0: ok = a_str(&m->out, "null"); break;
        default: return fail(m, 500, "cannot write this value as JSON");
    }
    return ok ? 0 : resp_full(m);
}

/* ---- parsing numbers as Rust's str::parse does ---- */
/* An i64 from text: optional sign, ASCII digits only, no overflow. */
static int parse_i64(const u8 *s, u64 n, i64 *out) {
    u64 i = 0; int neg = 0;
    if (i < n && (s[i] == '+' || s[i] == '-')) { neg = s[i] == '-'; i++; }
    if (i == n) return 0;
    u64 v = 0;
    for (; i < n; i++) { if (s[i] < '0' || s[i] > '9') return 0; u64 d = s[i] - '0'; if (v > (9223372036854775808ULL - d) / 10) return 0; v = v * 10 + d; }
    if (!neg && v > 9223372036854775807ULL) return 0;
    *out = neg ? (i64)(0 - v) : (i64)v; return 1;
}
static int parse_u64_radix(const u8 *s, u64 n, u32 radix, u64 *out) {
    u64 i = 0, v = 0;
    if (i < n && s[i] == '+') i++;
    if (i == n) return 0;
    for (; i < n; i++) {
        u32 c = s[i], d;
        if (c >= '0' && c <= '9') d = c - '0'; else if (lower((int)c) >= 'a' && lower((int)c) <= 'z') d = (u32)lower((int)c) - 'a' + 10; else return 0;
        if (d >= radix) return 0;
        if (v > (~0ULL - d) / radix) return 0;
        v = v * radix + d;
    }
    *out = v; return 1;
}
/* A JSON number (already checked against the grammar) as a double. */
static double parse_json_double(const u8 *s, u64 n) {
    u8 dig[801]; u32 nd = 0; i64 e10 = 0; int neg = 0, sticky = 0, any = 0; u64 i = 0;
    if (s[i] == '-') { neg = 1; i++; }
    for (; i < n && s[i] >= '0' && s[i] <= '9'; i++) { if (s[i] != '0' || any) { any = 1; if (nd < 800) dig[nd++] = s[i]; else { e10++; if (s[i] != '0') sticky = 1; } } }
    if (i < n && s[i] == '.') { i++; for (; i < n && s[i] >= '0' && s[i] <= '9'; i++) { if (s[i] != '0' || any) { any = 1; if (nd < 800) { dig[nd++] = s[i]; e10--; } else if (s[i] != '0') sticky = 1; } else e10--; } }
    if (i < n && (s[i] == 'e' || s[i] == 'E')) {
        i++; int en = 0; if (s[i] == '+' || s[i] == '-') { en = s[i] == '-'; i++; }
        i64 x = 0; for (; i < n; i++) { if (x < 100000) x = x * 10 + (s[i] - '0'); }
        e10 += en ? -x : x;
    }
    if (sticky) { dig[nd++] = '1'; e10--; }
    double r = exact_double(dig, nd, e10);
    return neg ? -r : r;
}

/* ---- request bodies: empty, or one JSON object (see serve.rs `parse_json`) ---- */
typedef struct { const u8 *s; u64 n, i; } J;
static int j_peek(J *p) { return p->i < p->n ? p->s[p->i] : -1; }
static void j_ws(J *p) { while (p->i < p->n && (p->s[p->i] == ' ' || p->s[p->i] == '\t' || p->s[p->i] == '\n' || p->s[p->i] == '\r')) p->i++; }
static int j_bad(Mito *m, const char *msg) { return fail(m, 400, msg); }
static int hexv(int c) { return c >= '0' && c <= '9' ? c - '0' : lower(c) >= 'a' && lower(c) <= 'f' ? lower(c) - 'a' + 10 : -1; }
static int j_hex4(J *p, u64 at, u32 *out) { if (at + 4 > p->n) return 0; u32 v = 0; for (int k = 0; k < 4; k++) { int h = hexv(p->s[at + k]); if (h < 0) return 0; v = v * 16 + (u32)h; } *out = v; return 1; }
static int j_utf8(Mito *m, u32 cp) {
    u8 b[4]; int n;
    if (cp >= 0xD800 && cp < 0xE000) cp = 0xFFFD; /* a lone surrogate */
    if (cp < 0x80) { b[0] = (u8)cp; n = 1; } else if (cp < 0x800) { b[0] = (u8)(0xC0 | cp >> 6); b[1] = (u8)(0x80 | (cp & 63)); n = 2; }
    else if (cp < 0x10000) { b[0] = (u8)(0xE0 | cp >> 12); b[1] = (u8)(0x80 | ((cp >> 6) & 63)); b[2] = (u8)(0x80 | (cp & 63)); n = 3; }
    else { b[0] = (u8)(0xF0 | cp >> 18); b[1] = (u8)(0x80 | ((cp >> 12) & 63)); b[2] = (u8)(0x80 | ((cp >> 6) & 63)); b[3] = (u8)(0x80 | (cp & 63)); n = 4; }
    return a_put(&m->text, b, n);
}
/* 0 ok, -1 invalid (error set), -2 text area full */
static int j_string(Mito *m, J *p, T *out) {
    if (j_peek(p) != '"') return j_bad(m, "invalid JSON body");
    p->i++;
    u32 start = m->text.len;
    for (;;) {
        u64 run = p->i;
        while (p->i < p->n && p->s[p->i] != '"' && p->s[p->i] != '\\' && p->s[p->i] >= 0x20) p->i++;
        if (!a_put(&m->text, p->s + run, p->i - run)) return -2;
        int c = j_peek(p);
        if (c == '"') { p->i++; *out = t_since(m, start); return 0; }
        if (c != '\\') return j_bad(m, "invalid JSON body");
        p->i++;
        int e = j_peek(p); if (e < 0) return j_bad(m, "invalid JSON body");
        p->i++;
        u8 one;
        switch (e) { case '"': one = '"'; break; case '\\': one = '\\'; break; case '/': one = '/'; break; case 'b': one = 8; break; case 'f': one = 12; break;
            case 'n': one = '\n'; break; case 'r': one = '\r'; break; case 't': one = '\t'; break; case 'u': one = 0; break; default: return j_bad(m, "invalid JSON body"); }
        if (e != 'u') { if (!a_put(&m->text, &one, 1)) return -2; continue; }
        u32 cp; if (!j_hex4(p, p->i, &cp)) return j_bad(m, "invalid JSON body");
        p->i += 4;
        if (cp >= 0xD800 && cp < 0xDC00 && p->i + 1 < p->n && p->s[p->i] == '\\' && p->s[p->i + 1] == 'u') {
            u32 lo; if (j_hex4(p, p->i + 2, &lo) && lo >= 0xDC00 && lo < 0xE000) { cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00); p->i += 6; }
        }
        if (!j_utf8(m, cp)) return -2;
    }
}
static int j_scalar(Mito *m, J *p, Val *out) {
    const u8 *s = p->s; u64 n = p->n;
    const char *w[3] = { "true", "false", "null" };
    for (int k = 0; k < 3; k++) { u64 l = c_len(w[k]); if (p->i + l <= n && c_eq(w[k], s + p->i, l)) { p->i += l; *out = k == 0 ? v_bool(1) : k == 1 ? v_bool(0) : v_null(); return 0; } }
    u64 st = p->i;
    if (j_peek(p) == '-') p->i++;
    u64 ia = p->i; while (p->i < n && s[p->i] >= '0' && s[p->i] <= '9') p->i++;
    if (p->i == ia || (s[ia] == '0' && p->i - ia > 1)) return j_bad(m, "invalid JSON body");
    int whole = 1;
    if (j_peek(p) == '.') { p->i++; whole = 0; u64 a = p->i; while (p->i < n && s[p->i] >= '0' && s[p->i] <= '9') p->i++; if (p->i == a) return j_bad(m, "invalid JSON body"); }
    if (j_peek(p) == 'e' || j_peek(p) == 'E') { p->i++; whole = 0; if (j_peek(p) == '+' || j_peek(p) == '-') p->i++; u64 a = p->i; while (p->i < n && s[p->i] >= '0' && s[p->i] <= '9') p->i++; if (p->i == a) return j_bad(m, "invalid JSON body"); }
    i64 iv;
    if (whole && parse_i64(s + st, p->i - st, &iv)) { *out = v_int(iv); return 0; }
    *out = v_flt(parse_json_double(s + st, p->i - st)); return 0;
}
static int j_skip(Mito *m, J *p, int depth) {
    if (depth > 64) return j_bad(m, "JSON body nested too deeply");
    int close = j_peek(p) == '{' ? '}' : ']';
    p->i++; j_ws(p);
    if (j_peek(p) == close) { p->i++; return 0; }
    for (;;) {
        T t; Val v; int r;
        j_ws(p);
        if (close == '}') { if ((r = j_string(m, p, &t))) return r; j_ws(p); if (j_peek(p) != ':') return j_bad(m, "invalid JSON body"); p->i++; j_ws(p); }
        int c = j_peek(p);
        if (c == '"') { if ((r = j_string(m, p, &t))) return r; } else if (c == '{' || c == '[') { if ((r = j_skip(m, p, depth + 1))) return r; } else if ((r = j_scalar(m, p, &v))) return r;
        j_ws(p);
        c = j_peek(p);
        if (c == ',') p->i++; else if (c == close) { p->i++; return 0; } else return j_bad(m, "invalid JSON body");
    }
}
static int parse_body(Mito *m, const u8 *s, u64 n) {
    m->njson = 0;
    J p = { s, n, 0 };
    j_ws(&p);
    if (p.i == n) return 0;
    if (j_peek(&p) != '{') return j_bad(m, "a JSON body must be an object");
    p.i++; j_ws(&p);
    if (j_peek(&p) == '}') p.i++;
    else for (;;) {
        T k; Val v; int r;
        j_ws(&p);
        if ((r = j_string(m, &p, &k))) return r;
        j_ws(&p); if (j_peek(&p) != ':') return j_bad(m, "invalid JSON body"); p.i++; j_ws(&p);
        int c = j_peek(&p);
        if (c == '"') { T t; if ((r = j_string(m, &p, &t))) return r; v = v_str(t); }
        else if (c == '{' || c == '[') { u32 mark = m->text.len; if ((r = j_skip(m, &p, 0))) return r; m->text.len = mark; v = v_null(); }
        else if ((r = j_scalar(m, &p, &v))) return r;
        if (m->njson == MAX_JSON) return -3;
        m->json[m->njson].k = k; m->json[m->njson].v = v; m->njson++;
        j_ws(&p);
        c = j_peek(&p);
        if (c == ',') p.i++; else if (c == '}') { p.i++; break; } else return j_bad(m, "invalid JSON body");
    }
    j_ws(&p);
    if (p.i != n) return j_bad(m, "invalid JSON body");
    return 0;
}
/* Copy a path segment or query part into the text area, decoding %XX (and + as a space in a query). */
static int push_decoded(Mito *m, const u8 *s, u64 n, int plus, T *out) {
    u32 start = m->text.len;
    for (u64 i = 0; i < n; i++) {
        u8 c = s[i];
        if (c == '%' && i + 2 < n && hexv(s[i + 1]) >= 0 && hexv(s[i + 2]) >= 0) { c = (u8)(hexv(s[i + 1]) * 16 + hexv(s[i + 2])); i += 2; }
        else if (c == '+' && plus) c = ' ';
        if (!a_put(&m->text, &c, 1)) return 0;
    }
    *out = t_since(m, start); return 1;
}

/* ---- request headers and forwarded fields: name/value pairs, names compared without case ---- */
static int find_pair(const MitoPair *ps, u32 n, const u8 *name, u64 nn, const u8 **v, u64 *vn) {
    for (u32 i = 0; i < n; i++) {
        if (ps[i].kn != nn) continue;
        u64 k = 0; for (; k < nn; k++) if (lower((u8)ps[i].k[k]) != lower(name[k])) break;
        if (k == nn) { *v = (const u8 *)ps[i].v; *vn = ps[i].vn; return 1; }
    }
    return 0;
}
static int header(Mito *m, const u8 *name, u64 nn, const u8 **v, u64 *vn) { return find_pair(m->hdrs, m->nhdrs, name, nn, v, vn); }

/* ---------------------------------------------------------------- the helpers generated code calls */
static int h_out(Ctx *c, const char *p, u32 n) { Mito *m = M_(c); return a_put(&m->out, p, n) ? 0 : resp_full(m); }
static int h_json_i64(Ctx *c, i64 v) { Mito *m = M_(c); return a_i64(&m->out, v) ? 0 : resp_full(m); }
static int h_json_col(Ctx *c, u32 cid, u32 idx, u32 ci) {
    Mito *m = M_(c); u64 a = caddr(m, cid, idx, ci); u32 n = (u32)rd(&m->mem, a, 4);
    return a_json_str(&m->out, m->mem.base + caddr(m, cid, idx, ci + 1), n) ? 0 : resp_full(m);
}
static int h_json_txt(Ctx *c, const Txt *t) { Mito *m = M_(c); return a_json_str(&m->out, m->text.p + t->off, t->len) ? 0 : resp_full(m); }
static int h_json_val(Ctx *c, const Val *v) { return json_val(M_(c), v); }
static int h_json_f64(Ctx *c, double f) { Mito *m = M_(c); return a_json_float(&m->out, f) ? 0 : resp_full(m); }
/* The session item named by the request's X-Session token (slot.generation.secret in hex), checked against pool `cid`. */
static int h_auth(Ctx *c, u32 cid, u32 *out) {
    Mito *m = M_(c);
    const u8 *v; u64 n;
    if (!header(m, (const u8 *)"x-session", 9, &v, &n)) return fail(m, 401, "missing X-Session header");
    const u8 *part[4]; u64 plen[4]; int np = 0, want = m->insys ? 4 : 3; u64 st = 0;
    for (u64 i = 0; i <= n && np < want; i++) if (i == n || v[i] == '.') { part[np] = v + st; plen[np] = i - st; np++; st = i + 1; }
    u64 slot, gen, sec, w;
    if (m->insys) { /* shard.slot.generation.secret: the first part must be this worker */
        if (np < 1 || !parse_u64_radix(part[0], plen[0], 10, &w) || w != m->me_index) return fail(m, 401, "session token belongs to another shard");
        for (int k = 0; k < 3; k++) { part[k] = part[k + 1]; plen[k] = plen[k + 1]; }
        np--;
    }
    if (np < 3 || !parse_u64_radix(part[0], plen[0], 10, &slot) || !parse_u64_radix(part[1], plen[1], 10, &gen) || gen > 0xFFFFFFFFULL || !parse_u64_radix(part[2], plen[2], 16, &sec)) return fail(m, 401, "malformed session token");
    u64 idx;
    if (!deref_quiet(m, cid, slot, (u32)gen, &idx)) return fail(m, 401, "session ended");
    i64 ci = col_by_path(m, cid, (const u8 *)"secret", 6);
    if (ci < 0) return fail(m, 500, "auth needs a `secret` field");
    Val real = { 0 }; const MitoCol *col = &coll(&m->mem, cid)->cols[ci];
    if (col->kind == 0) real = scalar_val(m, caddr(m, cid, idx, (u32)ci), col->scalar);
    if (real.tag != 1) return fail(m, 500, "`secret` must be an integer");
    if ((u64)real.i != sec) return fail(m, 403, "bad session secret");
    *out = (u32)idx; return 0;
}
/* ---- typed inputs of an endpoint: `points: i64`, `name: string<1..24>`, `{p}` as an item, a session from a header ---- */
/* a 400 (or another code) naming the input: "`name`: <msg>" */
static int in_bad(Mito *m, int code, const char *name, u32 nn, const char *msg) { err(m, E_FAIL, code); a_str(&m->emsg, "`"); a_put(&m->emsg, name, nn); a_str(&m->emsg, "`"); a_str(&m->emsg, msg); return -1; }
/* An integer written as text: optional '-', digits, no overflow. */
static int text_i64(const u8 *s, u64 n, i64 *out) {
    u64 i = 0; int neg = 0; if (n && s[0] == '-') { neg = 1; i = 1; }
    if (i == n) return 0;
    u64 v = 0; for (; i < n; i++) { if (s[i] < '0' || s[i] > '9') return 0; u64 d = (u64)(s[i] - '0'); if (v > (9223372036854775808ULL - d) / 10) return 0; v = v * 10 + d; }
    if (!neg && v > 9223372036854775807ULL) return 0;
    *out = neg ? (i64)(0 - v) : (i64)v; return 1;
}
/* A number written as text, in JSON's grammar. */
static int text_f64(const u8 *s, u64 n, double *out) {
    u64 i = 0; if (i < n && s[i] == '-') i++;
    u64 a = i; while (i < n && s[i] >= '0' && s[i] <= '9') i++; if (i == a) return 0;
    if (i < n && s[i] == '.') { i++; a = i; while (i < n && s[i] >= '0' && s[i] <= '9') i++; if (i == a) return 0; }
    if (i < n && (s[i] == 'e' || s[i] == 'E')) { i++; if (i < n && (s[i] == '+' || s[i] == '-')) i++; a = i; while (i < n && s[i] >= '0' && s[i] <= '9') i++; if (i == a) return 0; }
    if (i != n) return 0;
    *out = parse_json_double(s, n); return 1;
}
/* Input `name` from `src` (0 the JSON body, 1 a path segment, 2 the query, 3 a header, 4 a forwarded field), as `kind`:
   1 an integer in lo..hi, 2 a number, 3 a bool, 4 a string of lo..hi bytes, 5 the item of top-level pool `lo` that the
   X-Session token names (as auth), 6 the item of pool `lo` that an id names ("slot", or "slot.generation" for a checked
   pool). *out is null when the input is absent; the generated code applies its default or answers 400. */
static int h_input(Ctx *c, int src, const char *name, u32 nn, int kind, i64 lo, i64 hi, Val *out) {
    Mito *m = M_(c);
    *out = v_null();
    if (kind == 5) { u32 idx; if (h_auth(c, (u32)lo, &idx)) return -1; *out = v_item((u32)lo, idx); return 0; }
    Val v = v_null(); const u8 *tp = 0; u64 tn = 0; int text = 1;
    if (src == 0) {
        text = 0;
        for (u32 i = 0; i < m->njson; i++) if (b_eq(t_ptr(m, m->json[i].k), m->json[i].k.len, (const u8 *)name, nn)) { v = m->json[i].v; break; }
        if (v.tag == 0) return 0;
    } else if (src == 1) {
        const MitoRoute *rt = &mito_prog.routes[m->ei]; int found = 0;
        for (u32 sg = 0; sg < rt->nsegs && !found; sg++) if (rt->segs[sg].param && c_eq(rt->segs[sg].text, (const u8 *)name, nn))
            for (u32 i = 0; i < m->nparams; i++) if (m->params[i].seg == sg) { tp = t_ptr(m, m->params[i].t); tn = m->params[i].t.len; found = 1; break; }
        if (!found) return 0;
    } else if (src == 2) {
        int found = 0;
        for (u32 i = 0; i < m->nquery; i++) if (b_eq(t_ptr(m, m->query[i].k), m->query[i].k.len, (const u8 *)name, nn)) { tp = t_ptr(m, m->query[i].v); tn = m->query[i].v.len; found = 1; break; }
        if (!found) return 0;
    } else {
        const u8 *hv; u64 hn;
        if (!(src == 3 ? header(m, (const u8 *)name, nn, &hv, &hn) : find_pair(m->fwd, m->nfwd, (const u8 *)name, nn, &hv, &hn))) return 0;
        tp = hv; tn = hn;
    }
    switch (kind) {
        case 1: {
            i64 i;
            if (text) { if (!text_i64(tp, tn, &i)) return in_bad(m, 400, name, nn, ": expected an integer"); }
            else { if (v.tag != 1) return in_bad(m, 400, name, nn, ": expected an integer"); i = v.i; }
            if (i < lo || i > hi) { in_bad(m, 400, name, nn, ": "); a_i64(&m->emsg, i); a_str(&m->emsg, " is outside "); a_i64(&m->emsg, lo); a_str(&m->emsg, ".."); a_i64(&m->emsg, hi); return -1; }
            *out = v_int(i); return 0;
        }
        case 2: {
            double f;
            if (text) { if (!text_f64(tp, tn, &f)) return in_bad(m, 400, name, nn, ": expected a number"); }
            else if (v.tag == 1) f = (double)v.i; else if (v.tag == 2) f = v.f; else return in_bad(m, 400, name, nn, ": expected a number");
            *out = v_flt(f); return 0;
        }
        case 3:
            if (text) { if (b_eq(tp, tn, (const u8 *)"true", 4)) *out = v_bool(1); else if (b_eq(tp, tn, (const u8 *)"false", 5)) *out = v_bool(0); else return in_bad(m, 400, name, nn, ": expected true or false"); }
            else { if (v.tag != 3) return in_bad(m, 400, name, nn, ": expected true or false"); *out = v; }
            return 0;
        case 4: {
            T t;
            if (text) { if (t_push(m, tp, tn, &t)) return -1; }
            else { if (v.tag != 4) return in_bad(m, 400, name, nn, ": expected a string"); t.off = v.a; t.len = v.b; }
            if ((i64)t.len > hi) { in_bad(m, 413, name, nn, ": "); a_u64(&m->emsg, t.len); a_str(&m->emsg, " bytes, at most "); a_i64(&m->emsg, hi); return -1; }
            if ((i64)t.len < lo) { in_bad(m, 400, name, nn, ": at least "); a_i64(&m->emsg, lo); a_str(&m->emsg, lo == 1 ? " byte" : " bytes"); return -1; }
            *out = v_str(t); return 0;
        }
        case 6: {
            /* an id: "slot" (a pool without generations), or "slot.generation" (a checked pool) */
            if (!text) { if (v.tag == 4) { tp = t_ptr(m, (T){ v.a, v.b }); tn = v.b; } else if (v.tag == 1 && v.i >= 0) { char d[24]; int dn = 0; u64 x = (u64)v.i; do { d[dn++] = (char)('0' + x % 10); x /= 10; } while (x); T t; u32 st = m->text.len; while (dn) { char ch = d[--dn]; if (!a_put(&m->text, &ch, 1)) return text_full(m); } t = t_since(m, st); tp = t_ptr(m, t); tn = t.len; } else return in_bad(m, 400, name, nn, ": expected an id"); }
            const MitoColl *k = coll(&m->mem, (u32)lo);
            u64 dot = tn; for (u64 i = 0; i < tn; i++) if (tp[i] == '.') { dot = i; break; }
            u64 slot, gen = 0;
            if (!parse_u64_radix(tp, dot, 10, &slot) || (k->checked ? (dot == tn || !parse_u64_radix(tp + dot + 1, tn - dot - 1, 10, &gen) || gen > 0xFFFFFFFFULL) : dot != tn)) return in_bad(m, 404, name, nn, k->checked ? ": expected an id slot.generation" : ": expected an id");
            u64 idx;
            if (!deref_quiet(m, (u32)lo, slot, (u32)gen, &idx)) { in_bad(m, 404, name, nn, ": no "); a_put(&m->emsg, tp, tn); a_str(&m->emsg, " in "); a_str(&m->emsg, k->name); return -1; }
            *out = v_item((u32)lo, (u32)idx); return 0;
        }
    }
    return fail(m, 500, "unknown input kind");
}
/* An item written as JSON: its id, "slot.generation" (a string) for a checked pool, else the slot (a number). */
static int h_json_id(Ctx *c, u32 cid, u32 idx) {
    Mito *m = M_(c); const MitoColl *k = coll(&m->mem, cid);
    if (!k->checked) return a_i64(&m->out, idx) ? 0 : resp_full(m);
    u32 g = (u32)rd(&m->mem, gaddr(&m->mem, cid, idx), 4);
    return a_put(&m->out, "\"", 1) && a_i64(&m->out, idx) && a_put(&m->out, ".", 1) && a_i64(&m->out, g) && a_put(&m->out, "\"", 1) ? 0 : resp_full(m);
}
/* The JSON body's fields must be among `names` (each followed by a NUL byte): an unknown one is 400. */
static int h_fields(Ctx *c, const char *names, u32 n) {
    Mito *m = M_(c);
    for (u32 i = 0; i < m->njson; i++) {
        const u8 *k = t_ptr(m, m->json[i].k); u32 kn = m->json[i].k.len; int ok = 0;
        for (u32 at = 0; at < n && !ok;) { u32 e = at; while (e < n && names[e]) e++; if (b_eq((const u8 *)names + at, e - at, k, kn)) ok = 1; at = e + 1; }
        if (!ok) { err(m, E_FAIL, 400); a_str(&m->emsg, "unknown field `"); a_put(&m->emsg, k, kn); a_str(&m->emsg, "`"); return -1; }
    }
    return 0;
}
static int not_live(Mito *m, const char *a, i64 k, const char *b) { err(m, E_NOTLIVE, 404); a_str(&m->emsg, a); a_i64(&m->emsg, k); a_str(&m->emsg, b); return -1; }
static int h_index(Ctx *c, u32 cid, u32 inst, i64 k, u32 *out) {
    Mito *m = M_(c); const MitoColl *col = coll(&m->mem, cid);
    if (k < 0) return not_live(m, "index ", k, " is negative");
    if (col->list) {
        if ((u64)k >= hget(&m->mem, cid, LEN, inst)) return not_live(m, "position ", k, " is past the end");
        *out = (u32)((u64)inst * col->cap + (hget(&m->mem, cid, HEAD, inst) + (u64)k) % col->cap); return 0;
    }
    /* a pool: the slot is live when it is below the high-water mark and its bit is set (item_live without the divisions) */
    if ((u64)k >= col->cap || inst >= col->mult || (u64)k >= hget(&m->mem, cid, HW, inst) || !bit(&m->mem, cid, inst, (u64)k)) return not_live(m, "slot ", k, " is not live");
    *out = (u32)((u64)inst * col->cap + (u64)k); return 0;
}
static int h_lit(Ctx *c, const char *p, u32 n, Txt *out) { Mito *m = M_(c); T t; if (t_push(m, p, n, &t)) return -1; out->off = t.off; out->len = t.len; out->present = 1; return 0; }
static int h_fail(Ctx *c, int code, const char *p, u32 n) { Mito *m = M_(c); err(m, E_FAIL, code); a_put(&m->emsg, p, n); return -1; }
static int h_dead(Ctx *c) { return err(M_(c), E_DEAD, 404); }
static int h_col_txt(Ctx *c, u32 cid, u32 idx, u32 ci, Txt *out) { Mito *m = M_(c); T t; if (col_txt(m, cid, idx, ci, &t)) return -1; out->off = t.off; out->len = t.len; out->present = 1; return 0; }
static int h_plain_txt(Ctx *c, u32 off, Txt *out) { Mito *m = M_(c); T t; if (t_push(m, m->mem.base + off + 4, rd(&m->mem, off, 4), &t)) return -1; out->off = t.off; out->len = t.len; out->present = 1; return 0; }

static int num(const Val *v, double *f) { if (v->tag == 1) { *f = (double)v->i; return 1; } if (v->tag == 2) { *f = v->f; return 1; } return 0; }
static int bytes_cmp(const u8 *a, u64 an, const u8 *b, u64 bn) { u64 n = an < bn ? an : bn; for (u64 i = 0; i < n; i++) if (a[i] != b[i]) return a[i] < b[i] ? -1 : 1; return an < bn ? -1 : an > bn ? 1 : 0; }
/* A string column's bytes where they are (its length column, then its bytes). */
static void col_view(Mito *m, u32 cid, u64 idx, u32 ci, const u8 **p, u32 *n) { *n = (u32)rd(&m->mem, caddr(m, cid, idx, ci), 4); *p = m->mem.base + caddr(m, cid, idx, ci + 1); }
static int cmp_res(int op, int c) { switch (op) { case 0: return c == 0; case 1: return c != 0; case 2: return c < 0; case 3: return c <= 0; case 4: return c > 0; default: return c >= 0; } }
/* A string column against a literal, or against another column, compared where they are (op in OPS order). */
/* How many of the first n elements of an integer array (kind code 1..8, at byte offset base) are below target (at most it when
   inclusive). For a sorted array that is the position where target belongs. AVX2 (when the build enables it) compares four
   64-bit keys at a time. */
#ifdef __clang__
__attribute__((always_inline))
#endif
static inline int h_count_below(Ctx *c, u64 base, u32 code, i64 n, i64 target, int inclusive, i64 *out) {
    const u8 *p = M_(c)->mem.base + base; i64 r = 0, i = 0;
#if defined(__AVX2__)
    if (code == 4) {
        __m256i t = _mm256_set1_epi64x(target);
        for (; i + 4 <= n; i += 4) {
            __m256i v = _mm256_loadu_si256((const __m256i *)(p + i * 8));
            int below = _mm256_movemask_pd(_mm256_castsi256_pd(inclusive ? _mm256_cmpgt_epi64(v, t) : _mm256_cmpgt_epi64(t, v)));
            r += inclusive ? 4 - __builtin_popcount((unsigned)below) : __builtin_popcount((unsigned)below);
        }
    }
#endif
    for (; i < n; i++) {
        i64 v;
        switch (code) {
            case 1: { signed char x; memcpy(&x, p + i, 1); v = x; break; }
            case 2: { short x; memcpy(&x, p + i * 2, 2); v = x; break; }
            case 3: { int x; memcpy(&x, p + i * 4, 4); v = x; break; }
            case 5: { u8 x; memcpy(&x, p + i, 1); v = x; break; }
            case 6: { unsigned short x; memcpy(&x, p + i * 2, 2); v = x; break; }
            case 7: { u32 x; memcpy(&x, p + i * 4, 4); v = x; break; }
            case 8: { u64 x; memcpy(&x, p + i * 8, 8); if (x > (u64)I64MAX) { r += 0; continue; } v = (i64)x; break; }
            default: memcpy(&v, p + i * 8, 8); break;
        }
        r += inclusive ? v <= target : v < target;
    }
    *out = r; return 0;
}
static int h_str_cmp_lit(Ctx *c, int op, u32 cid, u32 idx, u32 ci, u32 off, u32 len, int *out) {
    Mito *m = M_(c); const u8 *p; u32 n; col_view(m, cid, idx, ci, &p, &n);
    *out = cmp_res(op, bytes_cmp(p, n, t_ptr(m, (T){ off, len }), len)); return 0;
}
static int h_str_cmp_col(Ctx *c, int op, u32 ca, u32 ia, u32 cia, u32 cb, u32 ib, u32 cib, int *out) {
    Mito *m = M_(c); const u8 *p, *q; u32 n, k; col_view(m, ca, ia, cia, &p, &n); col_view(m, cb, ib, cib, &q, &k);
    *out = cmp_res(op, bytes_cmp(p, n, q, k)); return 0;
}
/* An operator (OPS order: == != < <= > >= + - * / %) on values of run-time kind, as the interpreter's binop. */
static int h_binop(Ctx *c, int op, const Val *l, const Val *r, Val *out) {
    Mito *m = M_(c);
    if (op <= 1) {
        int eq; double a, b;
        if (l->tag == 0 && r->tag == 0) eq = 1;
        else if (l->tag == 0 || r->tag == 0) eq = 0;
        else if (l->tag == 4 && r->tag == 4) eq = b_eq(t_ptr(m, (T){ l->a, l->b }), l->b, t_ptr(m, (T){ r->a, r->b }), r->b);
        else if (l->tag == 3 && r->tag == 3) eq = l->i == r->i;
        else if (l->tag == 5 && r->tag == 5) eq = l->a == r->a && l->b == r->b;
        else if (num(l, &a) && num(r, &b)) eq = a == b;
        else return fail(m, 500, "cannot compare these values");
        *out = v_bool(eq == (op == 0)); return 0;
    }
    if (l->tag == 1 && r->tag == 1) {
        i64 a = l->i, b = r->i;
        switch (op) {
            case 2: *out = v_bool(a < b); return 0; case 3: *out = v_bool(a <= b); return 0; case 4: *out = v_bool(a > b); return 0; case 5: *out = v_bool(a >= b); return 0;
            case 6: if ((b > 0 && a > I64MAX - b) || (b < 0 && a < I64MIN - b)) return fail(m, 500, "integer overflow"); *out = v_int(a + b); return 0;
            case 7: if ((b < 0 && a > I64MAX + b) || (b > 0 && a < I64MIN + b)) return fail(m, 500, "integer overflow"); *out = v_int(a - b); return 0;
            case 8: { if (a > 0 ? (b > 0 ? a > I64MAX / b : b < I64MIN / a) : (b > 0 ? a < I64MIN / b : (a != 0 && b < I64MAX / a))) return fail(m, 500, "integer overflow"); *out = v_int(a * b); return 0; }
            case 9: if (b == 0) return fail(m, 500, "division by zero"); if (a == I64MIN && b == -1) return fail(m, 500, "integer overflow"); *out = v_int(a / b); return 0;
            case 10: if (b == 0) return fail(m, 500, "division by zero"); if (a == I64MIN && b == -1) return fail(m, 500, "integer overflow"); *out = v_int(a % b); return 0;
        }
    }
    if (op == 6 && l->tag == 4) { u32 start = m->text.len; if (show(m, l) || show(m, r)) return -1; *out = v_str(t_since(m, start)); return 0; }
    double a, b;
    if (num(l, &a) && num(r, &b)) {
        switch (op) {
            case 6: *out = v_flt(a + b); return 0; case 7: *out = v_flt(a - b); return 0; case 8: *out = v_flt(a * b); return 0; case 9: *out = v_flt(a / b); return 0;
            case 2: *out = v_bool(a < b); return 0; case 3: *out = v_bool(a <= b); return 0; case 4: *out = v_bool(a > b); return 0; case 5: *out = v_bool(a >= b); return 0;
            default: return fail(m, 500, "bad operator");
        }
    }
    if (l->tag == 4 && r->tag == 4) {
        int k = bytes_cmp(t_ptr(m, (T){ l->a, l->b }), l->b, t_ptr(m, (T){ r->a, r->b }), r->b);
        switch (op) { case 2: *out = v_bool(k < 0); return 0; case 3: *out = v_bool(k <= 0); return 0; case 4: *out = v_bool(k > 0); return 0; case 5: *out = v_bool(k >= 0); return 0; default: return fail(m, 500, "bad operator"); }
    }
    return fail(m, 500, "operator does not fit these values");
}
static int h_truthy(Ctx *c, const Val *v, int *out) { if (v->tag == 3) { *out = v->i != 0; return 0; } if (v->tag == 0) { *out = 0; return 0; } return fail(M_(c), 500, "condition is not a boolean"); }
/* An integer of `size` bytes (the generated code has checked it fits the column) at byte offset `off`. */
static int h_wr_int(Ctx *c, u64 off, u32 size, i64 v) { wr(&M_(c)->mem, off, size, (u64)v); return 0; }
/* The same for plain state: ensure(0, plain_end) committed it at start and the live-page marking keeps trim from decommitting it, so the page is
   known committed and only the undo log needs a look. */
static int h_clear_coll(Ctx *c, u32 cid, u32 inst) { reset_instance(&M_(c)->mem, cid, inst); return 0; }
static int h_save_line(Ctx *c, u64 a, u32 size) { save(&M_(c)->mem, a, size); return 0; }
static int h_commit_range(Ctx *c, u64 a, u32 size) { ensure(&M_(c)->mem, a, a + size); return 0; }
/* the text area's length now, and cutting it back to an earlier length: a loop body that keeps none of its strings past the iteration gives their space back */
/* `count` bytes of a state array of bytes at a byte offset, copied into the text area as a string */
static int h_arr_txt(Ctx *c, u64 off, u32 count, Txt *out) { Mito *m = M_(c); T t; if (t_push(m, m->mem.base + off, count, &t)) return -1; out->off = t.off; out->len = t.len; out->present = 1; return 0; }
/* save a range to the undo log before a loop stores into it (save skips the lines already saved) */
static int h_save_range(Ctx *c, u64 a, u64 n) { save(&M_(c)->mem, a, n); return 0; }
static int h_text_len(Ctx *c, u32 *len) { *len = M_(c)->text.len; return 0; }
static int h_text_cut(Ctx *c, u32 len) { M_(c)->text.len = len; return 0; }
static int h_wr_plain(Ctx *c, u64 off, u32 size, i64 v) { Mem *m = &M_(c)->mem; save(m, off, size); u64 x = (u64)v; memcpy(m->base + off, &x, size); return 0; }
/* `count` elements of `esize` bytes of an array at byte offset `base`, from position `from`, are set to the value whose bits are `bits`: one commit
   (not for plain state, committed at start), one undo-log pass over the range, one fill (the generated code checked the range and that the value fits). */
static int h_fill_elems(Ctx *c, u64 base, u32 esize, i64 from, i64 count, i64 bits, int plain) {
    if (count <= 0) return 0;
    Mem *m = &M_(c)->mem;
    u64 lo = base + (u64)from * esize, n = (u64)count;
    if (!plain) ensure(m, lo, lo + n * esize);
    save(m, lo, n * esize);
    u8 *p = m->base + lo;
    if (esize == 1) memset(p, (int)(bits & 0xFF), n);
    else if (esize == 2) { u16 x = (u16)bits; for (u64 i = 0; i < n; i++) memcpy(p + i * 2, &x, 2); }
    else if (esize == 4) { u32 x = (u32)bits; for (u64 i = 0; i < n; i++) memcpy(p + i * 4, &x, 4); }
    else { u64 x = (u64)bits; for (u64 i = 0; i < n; i++) memcpy(p + i * 8, &x, 8); }
    return 0;
}
/* `count` elements of `esize` bytes move within an array at byte offset `base`, from position `from` to `to`: one commit, one
   undo-log pass over the range they write, one memmove (the generated code checked the range). */
static int h_move_elems(Ctx *c, u64 base, u32 esize, i64 from, i64 to, i64 count) {
    if (count <= 0 || from == to) return 0;
    Mem *m = &M_(c)->mem;
    /* only the destination is written: commit and save just that range (a move between far apart ranges does not pay for what lies between them) */
    u64 lo = base + (u64)to * esize, hi = lo + (u64)count * esize;
    ensure(m, lo, hi); save(m, lo, hi - lo);
    memmove(m->base + base + (u64)to * esize, m->base + base + (u64)from * esize, (u64)count * esize);
    return 0;
}
static int h_store(Ctx *c, u32 cid, u32 idx, u32 ci, const Val *v) { return store_col(M_(c), cid, idx, ci, -1, v); }
static int h_store_elem(Ctx *c, u32 cid, u32 idx, u32 ci, i64 k, const Val *v) { return store_col(M_(c), cid, idx, ci, k, v); }
/* A plain value at `off`: code 1..11 a scalar, 20 a string<cap>, 30 a reference to top-level pool `cap`. */
static int h_store_plain(Ctx *c, u32 off, u32 code, u32 cap, const Val *v) {
    Mito *m = M_(c);
    if (v->tag == 4) { if (code != 20) return bad_type(m); return store_bytes_at(m, off, off + 4, cap, t_ptr(m, (T){ v->a, v->b }), v->b); }
    if (v->tag != 1 && v->tag != 2 && v->tag != 3) return fail(m, 500, "cannot store this value in state");
    if (code == 20) { err(m, E_BAD, 400); a_str(&m->emsg, "expected a string"); return -1; }
    return store_scalar(m, off, code, v);
}
static int h_alloc(Ctx *c, u32 cid, u32 inst, int try_, u32 *out) {
    Mito *m = M_(c);
    if (try_ && is_full(&m->mem, cid, inst)) { *out = 0xFFFFFFFFu; return 0; }
    u64 idx = push_item(&m->mem, cid, inst);
    if (idx == ~0ULL) {
        const MitoColl *k = coll(&m->mem, cid);
        err(m, E_FULL, 503); a_str(&m->emsg, k->name); a_str(&m->emsg, " is full ("); a_u64(&m->emsg, count(&m->mem, cid, inst)); a_str(&m->emsg, " of "); a_u64(&m->emsg, k->cap); a_str(&m->emsg, ")");
        return -1;
    }
    *out = (u32)idx; return 0;
}
/* Record a free for when the request succeeds. Not searched for duplicates: a second free of the same item finds it no
   longer live then and does nothing. */
static int add_free(Mito *m, u32 cid, u32 idx) {
    if (m->frees.len / 2 >= MAX_FREES || !grow_to(&m->frees, (m->frees.len + 2) * 4)) return fail(m, 507, "too many frees in one request");
    u32 *f = (u32 *)m->frees.p; f[m->frees.len] = cid; f[m->frees.len + 1] = idx; m->frees.len += 2; return 0;
}
static int h_free(Ctx *c, u32 cid, u32 idx) {
    Mito *m = M_(c);
    if (coll(&m->mem, cid)->list) return fail(m, 500, "list items are not freed; lists drop their oldest or pop their newest");
    return add_free(m, cid, idx);
}
static int h_free_val(Ctx *c, const Val *v) {
    Mito *m = M_(c);
    if (v->tag == 0) return 0;
    if (v->tag != 5) return fail(m, 500, "free needs an item");
    return h_free(c, v->a, v->b);
}
static i64 kind_of(Mito *m, const char *p, u32 n) { for (u32 k = 0; k < m->nkinds; k++) if (c_eq(m->kind_names[k], (const u8 *)p, n)) return k; return -1; }
static int h_fwd_begin(Ctx *c, const char *p, u32 n) {
    Mito *m = M_(c);
    if (!m->insys) return fail(m, 500, "forward needs a system of workers");
    if (kind_of(m, p, n) < 0) { err(m, E_FAIL, 500); a_str(&m->emsg, "no worker kind `"); a_put(&m->emsg, p, n); a_str(&m->emsg, "`"); return -1; }
    m->nfwd_out = 0; return 0;
}
/* A forwarded field: its name, and its value as text (nothing for null), in the text area. */
static int h_fwd_put(Ctx *c, const char *p, u32 n, const Val *v) {
    Mito *m = M_(c); T k;
    if (m->nfwd_out == MAX_FWD) return fail(m, 500, "a forward has at most 64 fields");
    if (t_push(m, p, n, &k)) return -1;
    u32 start = m->text.len;
    if (v->tag != 0 && show(m, v)) return -1;
    m->fwd_out[m->nfwd_out].k = k; m->fwd_out[m->nfwd_out].v = t_since(m, start); m->nfwd_out++;
    return 0;
}
static int h_forward(Ctx *c, const char *p, u32 n, i64 key) {
    Mito *m = M_(c); i64 k = kind_of(m, p, n);
    m->fwd_kind = k < 0 ? 0 : (u32)k; m->fwd_key = key;
    return err(m, E_FORWARD, 0);
}
static int h_crash(Ctx *c) { Mito *m = M_(c); m->crashed = 1; return fail(m, 500, "worker failed and was restarted"); }
static int h_elem(Ctx *c, i64 k, u32 n) {
    Mito *m = M_(c);
    if (k < 0) return not_live(m, "index ", k, " is negative");
    if (k >= (i64)n) { err(m, E_BAD, 400); a_str(&m->emsg, "index "); a_i64(&m->emsg, k); a_str(&m->emsg, " out of range (fixed size "); a_u64(&m->emsg, n); a_str(&m->emsg, ")"); return -1; }
    return 0;
}
/* Field `name` of an item held as a value (only top-level struct fields of items reach here at run time). */
static int h_field_val(Ctx *c, const Val *base, const char *p, u32 n, Val *out) {
    Mito *m = M_(c);
    if (base->tag == 0) return err(m, E_DEAD, 404);
    if (base->tag != 5) { err(m, E_FAIL, 500); a_str(&m->emsg, "`."); a_put(&m->emsg, p, n); a_str(&m->emsg, "` needs an item or a state struct"); return -1; }
    u32 cid = base->a, idx = base->b;
    i64 ci = col_by_path(m, cid, (const u8 *)p, n);
    if (ci < 0) {
        if (child_by_path(m, cid, (const u8 *)p, n) >= 0 || is_prefix(m, cid, (const u8 *)p, n)) return fail(m, 500, "a collection, struct or array field of an item held as a value cannot be read here; name the item's collection");
        err(m, E_BAD, 400); a_str(&m->emsg, "no field `"); a_put(&m->emsg, p, n); a_str(&m->emsg, "`"); return -1;
    }
    const MitoCol *col = &coll(&m->mem, cid)->cols[ci];
    if (col->kind == 1) return fail(m, 500, "a collection, struct or array field of an item held as a value cannot be read here; name the item's collection");
    if (col->kind == 2) { T t; if (col_txt(m, cid, idx, (u32)ci, &t)) return -1; *out = v_str(t); return 0; }
    *out = scalar_val(m, caddr(m, cid, idx, (u32)ci), col->scalar); return 0;
}
static int h_store_field(Ctx *c, const Val *base, const char *p, u32 n, const Val *v) {
    Mito *m = M_(c);
    if (base->tag != 5) return fail(m, 500, "assignment target is not an item or a state struct");
    i64 ci = col_by_path(m, base->a, (const u8 *)p, n);
    if (ci < 0 && (is_prefix(m, base->a, (const u8 *)p, n) || child_by_path(m, base->a, (const u8 *)p, n) >= 0)) { /* a struct or collection: the interpreter's store refuses it */
        err(m, E_BAD, 400); a_str(&m->emsg, v->tag == 4 ? "wrong type for this field" : "not a value"); return -1;
    }
    if (ci < 0) { err(m, E_BAD, 400); a_str(&m->emsg, "no field `"); a_put(&m->emsg, p, n); a_str(&m->emsg, "`"); return -1; }
    return store_col(m, base->a, base->b, (u32)ci, -1, v);
}
static int h_was_dead(Ctx *c) { Mito *m = M_(c); if (m->ekind == E_DEAD || m->ekind == E_NOTLIVE || m->ekind == E_STALE) { m->ekind = E_NONE; return 1; } return 0; }

/* ---- loops that collect their items first ---- */
static u32 *sel_at(Mito *m, u64 i) { return (u32 *)(m->sel.p + i * 4); }
static int h_sel_mark(Ctx *c, u32 *out) { *out = (u32)M_(c)->sel.len; return 0; }
static int h_sel_add(Ctx *c, u32 idx) {
    Mito *m = M_(c);
    if (!grow_to(&m->sel, (m->sel.len + 1) * 4)) return fail(m, 507, "a loop collected too many items");
    *sel_at(m, m->sel.len) = idx; m->sel.len++; return 0;
}
static int h_sel_limit(Ctx *c, u32 h, i64 limit, u32 *out) {
    Mito *m = M_(c); u64 n = m->sel.len - h;
    if (limit >= 0 && n > (u64)limit) { m->sel.len = h + (u64)limit; n = (u64)limit; }
    *out = (u32)n; return 0;
}
static int h_sel_get(Ctx *c, u32 h, u32 k, u32 *out) { *out = *sel_at(M_(c), (u64)h + k); return 0; }
static int h_sel_end(Ctx *c, u32 h) { M_(c)->sel.len = h; return 0; }

/* ---- builtins (cgen BUILTINS order) ---- */
static int arg_txt(Mito *m, const Val *a, u32 n, u32 i, T *out) { if (i >= n || a[i].tag != 4) return fail(m, 500, "expected a string argument"); out->off = a[i].a; out->len = a[i].b; return 0; }
static int h_builtin(Ctx *c, int code, const Val *a, u32 n, Val *out) {
    Mito *m = M_(c); T k;
    Val a0 = n > 0 ? a[0] : v_null();
    switch (code) {
        case 0: /* body */ if (arg_txt(m, a, n, 0, &k)) return -1;
            for (u32 i = 0; i < m->njson; i++) if (b_eq(t_ptr(m, m->json[i].k), m->json[i].k.len, t_ptr(m, k), k.len)) { *out = m->json[i].v; return 0; }
            *out = v_null(); return 0;
        case 1: { /* param */ if (arg_txt(m, a, n, 0, &k)) return -1;
            const MitoRoute *rt = &mito_prog.routes[m->ei];
            for (u32 s = 0; s < rt->nsegs; s++) if (rt->segs[s].param && c_eq(rt->segs[s].text, t_ptr(m, k), k.len)) {
                for (u32 i = 0; i < m->nparams; i++) if (m->params[i].seg == s) { *out = v_str(m->params[i].t); return 0; }
                break;
            }
            *out = v_null(); return 0; }
        case 2: /* query */ if (arg_txt(m, a, n, 0, &k)) return -1;
            for (u32 i = 0; i < m->nquery; i++) if (b_eq(t_ptr(m, m->query[i].k), m->query[i].k.len, t_ptr(m, k), k.len)) { *out = v_str(m->query[i].v); return 0; }
            *out = v_null(); return 0;
        case 3: { /* header */ if (arg_txt(m, a, n, 0, &k)) return -1;
            const u8 *v; u64 vn; T t;
            if (!header(m, t_ptr(m, k), k.len, &v, &vn)) { *out = v_null(); return 0; }
            if (t_push(m, v, vn, &t)) return -1;
            *out = v_str(t); return 0; }
        case 4: { /* fwd */ if (arg_txt(m, a, n, 0, &k)) return -1;
            const u8 *v; u64 vn; T t;
            if (!find_pair(m->fwd, m->nfwd, t_ptr(m, k), k.len, &v, &vn)) { *out = v_null(); return 0; }
            if (t_push(m, v, vn, &t)) return -1;
            *out = v_str(t); return 0; }
        case 5: /* int */
            if (a0.tag == 1) { *out = a0; return 0; }
            if (a0.tag == 2) { double f = a0.f; i64 i = f != f ? 0 : f >= 9223372036854775807.0 ? I64MAX : f <= -9223372036854775808.0 ? I64MIN : (i64)f; *out = v_int(i); return 0; }
            if (a0.tag == 4) {
                const u8 *s = t_ptr(m, (T){ a0.a, a0.b }); u64 b = 0, e = a0.b; i64 i;
                while (b < e && (s[b] == ' ' || (s[b] >= 9 && s[b] <= 13))) b++;
                while (e > b && (s[e - 1] == ' ' || (s[e - 1] >= 9 && s[e - 1] <= 13))) e--;
                if (parse_i64(s + b, e - b, &i)) { *out = v_int(i); return 0; }
                err(m, E_FAIL, 400); a_str(&m->emsg, "`"); a_lossy(&m->emsg, s, a0.b < QUOTE ? a0.b : QUOTE); if (a0.b > QUOTE) a_str(&m->emsg, "...");
                a_str(&m->emsg, "` is not a number"); return -1;
            }
            if (a0.tag == 0) return fail(m, 400, "missing number");
            return fail(m, 500, "int(x)");
        case 6: { /* str */ u32 start = m->text.len; if (show(m, &a0)) return -1; *out = v_str(t_since(m, start)); return 0; }
        case 7: /* len */ if (a0.tag == 4) { *out = v_int(a0.b); return 0; } if (a0.tag == 0) { *out = v_int(0); return 0; } return fail(m, 500, "len(text)");
        case 8: /* now */ *out = v_int(m->now); return 0;
        case 9: { /* random */ u64 x = m->rng; if (!x) x = os_random(); else { x ^= x << 13; x ^= x >> 7; x ^= x << 17; m->rng = x; } *out = v_int((i64)x); return 0; }
        case 10: { /* token */
            if (a0.tag != 5) return fail(m, 500, "token(item)");
            i64 ci = col_by_path(m, a0.a, (const u8 *)"secret", 6);
            if (ci < 0) return fail(m, 500, "token needs a `secret` field");
            const MitoCol *col = &coll(&m->mem, a0.a)->cols[ci];
            Val sec = col->kind == 0 ? scalar_val(m, caddr(m, a0.a, a0.b, (u32)ci), col->scalar) : v_null();
            if (sec.tag != 1) return fail(m, 500, "`secret` must be an integer");
            u32 start = m->text.len;
            if (m->insys && !(a_u64(&m->text, m->me_index) && a_put(&m->text, ".", 1))) return text_full(m); /* the shard first, in a system */
            if (!(a_u64(&m->text, a0.b) && a_put(&m->text, ".", 1) && a_u64(&m->text, gen_of(&m->mem, a0.a, a0.b)) && a_put(&m->text, ".", 1) && a_hex(&m->text, (u64)sec.i))) return text_full(m);
            *out = v_str(t_since(m, start)); return 0; }
        case 11: { /* stat */ if (arg_txt(m, a, n, 0, &k)) return -1;
            const u8 *s = t_ptr(m, k); u64 v;
            if (c_eq("committed_kb", s, k.len)) v = m->mem.cpages * PAGE / 1024;
            else if (c_eq("live_kb", s, k.len)) v = live_total(&m->mem) / 1024;
            else if (c_eq("reserved_kb", s, k.len)) v = m->mem.reserved / 1024;
            else return fail(m, 500, "unknown stat");
            *out = v_int((i64)v); return 0; }
        case 12: /* worker_index */ *out = v_int(m->insys ? m->me_index : 0); return 0;
        case 13: /* worker_count */
            if (n == 0) { *out = v_int(m->insys ? m->kind_counts[m->me_kind] : 1); return 0; }
            if (a0.tag == 4) { i64 kk = m->insys ? kind_of(m, (const char *)t_ptr(m, (T){ a0.a, a0.b }), a0.b) : -1; if (kk < 0) return fail(m, 500, "no such worker kind"); *out = v_int(m->kind_counts[kk]); return 0; }
            return fail(m, 500, "worker_count(\"kind\")");
        case 14: case 15: { /* min, max */
            Val b = n > 1 ? a[1] : v_null();
            if (a0.tag != 1 || b.tag != 1) return fail(m, 500, "min/max need integers");
            *out = v_int(code == 14 ? (a0.i < b.i ? a0.i : b.i) : (a0.i > b.i ? a0.i : b.i)); return 0; }
        case 16: /* slot */ if (a0.tag != 5) return fail(m, 500, "slot(item)"); *out = v_int((i64)(a0.b % coll(&m->mem, a0.a)->cap)); return 0;
    }
    return fail(m, 500, "unknown builtin code");
}

/* ---- a test's requests: built field by field, then sent through the program's own request handling ---- */
static int h_rq_put(Ctx *c, int hdr, const char *p, u32 n, const Val *v) {
    Mito *m = M_(c);
    if (hdr) {
        if (m->nrqh == 16) return fail(m, 500, "a test's request has at most 16 headers");
        T k; if (t_push(m, p, n, &k)) return -1;
        u32 s = m->text.len;
        if (v->tag != 0 && show(m, v)) return -1;
        m->rqh[m->nrqh].k = k; m->rqh[m->nrqh].v = t_since(m, s); m->nrqh++;
        return 0;
    }
    /* the body is built where the answer goes (a test gives no answer of its own), and moved out before the request */
    if (!a_str(&m->out, m->out.len ? "," : "{") || !a_json_str(&m->out, (const u8 *)p, n) || !a_str(&m->out, ":")) return resp_full(m);
    return json_val(m, v);
}
MITO_API int MITO(handle)(void *mp_, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *hdrs, u32 nh, const MitoPair *fwd, u32 nf, const char *body, u32 bn);
static int h_rq_send(Ctx *c, int method, const Txt *path, i64 *status, Txt *body) {
    Mito *m = M_(c);
    static const char *const verbs[] = { "GET", "POST", "PUT", "PATCH", "DELETE" };
    u32 bn = 0;
    if (m->out.len) {
        if (!a_str(&m->out, "}")) return resp_full(m);
        if (m->out.len > sizeof m->rqbuf) return fail(m, 500, "a test's request body is over 16 KB");
        bn = (u32)m->out.len; memcpy(m->rqbuf, m->out.p, bn); m->out.len = 0;
    }
    MitoPair h[16];
    for (u32 i = 0; i < m->nrqh; i++) { h[i].k = (const char *)t_ptr(m, m->rqh[i].k); h[i].kn = m->rqh[i].k.len; h[i].v = (const char *)t_ptr(m, m->rqh[i].v); h[i].vn = m->rqh[i].v.len; }
    u32 nh = m->nrqh; m->nrqh = 0;
    /* the request's text goes after the test's, which stays as it is */
    u32 keep = m->text.len, base = m->text_base;
    m->text_base = keep;
    int code = MITO(handle)(m, verbs[method], (u32)c_len(verbs[method]), (const char *)t_ptr(m, (T){ path->off, path->len }), path->len, h, nh, 0, 0, (const char *)m->rqbuf, bn);
    m->text_base = base; m->text.len = keep; m->ekind = E_NONE;
    T t; if (t_push(m, m->out.p, m->out.len, &t)) return -1;
    m->out.len = 0;
    *status = code; body->off = t.off; body->len = t.len; body->present = 1;
    return 0;
}
/* `field(text, name)`: a top-level field of a JSON object, as text: a string's characters (escapes kept as written), anything
   else as written; absent (null) when the object has no such field. */
static u32 jf_skip_str(const u8 *p, u32 n, u32 i) { while (i < n && p[i] != '"') { if (p[i] == '\\') i++; i++; } return i; }
static u32 jf_skip_ws(const u8 *p, u32 n, u32 i) { while (i < n && (p[i] == ' ' || p[i] == '\n' || p[i] == '\r' || p[i] == '\t')) i++; return i; }
static int h_json_field(Ctx *c, const Txt *j, const char *name, u32 nn, Txt *out) {
    Mito *m = M_(c);
    const u8 *p = m->text.p + j->off; u32 n = j->len, i = 0;
    out->present = 0; out->off = 0; out->len = 0;
    while (i < n && p[i] != '{') i++;
    i++;
    int depth = 0;
    while (i < n) {
        u8 ch = p[i];
        if (ch == '"') {
            u32 s = i + 1; i = jf_skip_str(p, n, s); u32 e = i++;
            if (depth != 0) continue;
            i = jf_skip_ws(p, n, i);
            if (i >= n || p[i] != ':') continue; /* a string value, not a key */
            i = jf_skip_ws(p, n, i + 1);
            int match = e - s == nn && c_eq(name, p + s, nn);
            u32 vs = i, ve;
            if (i < n && p[i] == '"') { vs = i + 1; i = jf_skip_str(p, n, vs); ve = i++; }
            else {
                int d = 0;
                while (i < n) {
                    u8 x = p[i];
                    if (x == '"') i = jf_skip_str(p, n, i + 1);
                    else if (x == '{' || x == '[') d++;
                    else if (x == '}' || x == ']') { if (d == 0) break; d--; }
                    else if (x == ',' && d == 0) break;
                    i++;
                }
                ve = i; while (ve > vs && (p[ve - 1] == ' ' || p[ve - 1] == '\n' || p[ve - 1] == '\r' || p[ve - 1] == '\t')) ve--;
            }
            if (match) { out->off = j->off + vs; out->len = ve - vs; out->present = !(ve - vs == 4 && c_eq("null", p + vs, 4)); return 0; }
            continue;
        }
        if (ch == '{' || ch == '[') depth++;
        else if (ch == '}' || ch == ']') { if (depth == 0) break; depth--; }
        i++;
    }
    return 0;
}

/* Append a value, formatted as `+` would, to a string field in place: kind bit 0 clear: a plain string (a: its offset, b: its
   capacity); set: an item's string column (a: collection, b: item, cc: its length column). Bit 1: empty it first (an assignment
   written as pieces). Only the appended bytes and the length are written (and saved to the undo log). */
static int h_str_app(Ctx *c, int kind, u64 a, u64 b, u32 cc, const Val *v) {
    Mito *m = M_(c);
    u64 len_at, bytes_at, cap;
    if (kind & 1) { const MitoColl *k = coll(&m->mem, (u32)a); len_at = caddr(m, (u32)a, b, cc); bytes_at = caddr(m, (u32)a, b, cc + 1); cap = k->cols[cc + 1].n; }
    else { len_at = a; bytes_at = a + 4; cap = b; }
    u64 cur = (kind & 2) ? 0 : rd(&m->mem, len_at, 4);
    const u8 *p; u64 n; u32 mark = m->text.len;
    if (v->tag == 4) { p = t_ptr(m, (T){ v->a, v->b }); n = v->b; }
    else if (v->tag == 0) { p = 0; n = 0; }
    else { if (show(m, v)) return -1; p = m->text.p + mark; n = m->text.len - mark; }
    if (cur + n > cap) { m->text.len = mark; err(m, E_TOOLONG, 413); a_str(&m->emsg, "string of "); a_u64(&m->emsg, cur + n); a_str(&m->emsg, " bytes does not fit string<"); a_u64(&m->emsg, cap); a_str(&m->emsg, ">"); return -1; }
    if (n) put(&m->mem, bytes_at + cur, p, n);
    if (n || (kind & 2)) wr(&m->mem, len_at, 4, cur + n);
    m->text.len = mark;
    return 0;
}

static const H mito_h = {
    h_out, h_json_i64, h_json_col, h_json_txt, h_json_val, h_auth, h_index, h_lit, h_fail, h_dead, h_col_txt, h_plain_txt,
    h_binop, h_truthy, h_store, h_store_plain, h_alloc, h_free, h_fwd_begin, h_fwd_put, h_forward, h_crash, h_builtin,
    h_json_f64, h_elem, h_store_elem, h_sel_mark, h_sel_add, h_sel_limit, h_sel_get, h_sel_end, h_field_val, h_store_field, h_free_val, h_was_dead, h_input, h_fields, h_json_id, h_wr_plain, h_wr_int, h_move_elems, h_fill_elems, h_clear_coll, h_count_below, h_str_cmp_lit, h_str_cmp_col, h_save_line, h_commit_range, h_save_range, h_text_len, h_arr_txt, h_text_cut,
    h_rq_put, h_rq_send, h_json_field, h_str_app,
};

/* ---------------------------------------------------------------- the interface: mito_* */
/* Start the program: reserve its layout (refusing if over `budget` bytes), commit plain values and top-level headers.
   `seed` 0 takes random numbers from the OS. Returns 0 if over budget or out of address space. */
MITO_API void *MITO(start)(u64 budget, u64 seed) {
    const MitoProg *pg = &mito_prog;
    if (pg->total > budget) return 0;
    Mito *m = (Mito *)VirtualAlloc(0, sizeof(Mito), MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if (!m) return 0;
    Mem *mem = &m->mem;
    mem->prog = pg;
    mem->reserved = up(pg->total ? pg->total : 1, PAGE);
    mem->base = (u8 *)os_reserve(mem->reserved);
    mem->npages = mem->reserved / PAGE;
    mem->dirty = (u8 *)os_reserve(up(mem->npages * (PAGE / 64 / 8), PAGE));
    if (!mem->dirty) return 0;
    u64 small = up(mem->npages * 2 + (pg->ncolls + 1) * 16, PAGE);
    u8 *s = (u8 *)VirtualAlloc(0, small, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if (!mem->base || !s) return 0;
    mem->committed = s; mem->livepg = s + mem->npages;
    mem->shrinks = (u64 *)(s + up(mem->npages * 2, 8));
    m->pr.stuck = mem->shrinks + pg->ncolls + 1;
    if (!grow_init(&mem->jb, 1ULL << 32) || !grow_init(&m->frees, (u64)MAX_FREES * 8) || !grow_init(&m->sel, (u64)MAX_SEL * 4)) return 0;
    m->areas = (u8 *)VirtualAlloc(0, TEXT_CAP + RESPONSE_CAP, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if (!m->areas) return 0;
    m->text.p = m->areas; m->text.cap = TEXT_CAP; memcpy(m->areas, mito_lits, MITO_LITS_LEN); m->text.len = MITO_LITS_LEN; m->out.p = m->areas + TEXT_CAP; m->out.cap = RESPONSE_CAP;
    m->emsg.p = m->ebuf; m->emsg.cap = sizeof m->ebuf;
    m->rng = seed;
    m->loop_budget = 100000000;
    m->text_base = MITO_LITS_LEN;
    /* State of at most COMMIT_ALL_MAX is committed whole at start: committing reserves the system's commit charge, not physical memory (a page takes
       memory when first written), and then no store, add or helper needs to test or commit pages. Trims happen only under memory pressure; turning
       pressure on (set_marks) gives back what is not live and returns to committing on demand, so its accounting is exact. */
    if (mem->reserved <= COMMIT_ALL_MAX && os_commit(mem->base, mem->reserved) && os_commit(mem->dirty, up(mem->npages * (PAGE / 64 / 8), PAGE))) {
        for (u64 q = 0; q < mem->npages; q++) mem->committed[q] = 1;
        mem->cpages = mem->npages;
        mem->all = 1;
    }
    ensure(mem, 0, pg->plain_end);
    for (u32 c = 0; c < pg->ncolls; c++) { const MitoColl *k = &pg->colls[c]; if (k->parent < 0) ensure(mem, k->hdr, k->hdr + hdr_fields(k) * 4); }
    /* Unlogged bytes: their lines are marked saved once, here, and never journaled, so no store, add or helper saves them and a rollback
       leaves them as written. The undo log clears the mark only of lines it saved, so these stay marked. */
    for (u32 r = 0; r < pg->nunlogged; r++) {
        u64 a = pg->unlogged[2 * r], b = pg->unlogged[2 * r + 1];
        if (b <= a) continue;
        u64 d0 = (a / PAGE) * (PAGE / 64 / 8) & ~(PAGE - 1), d1 = up(((b - 1) / PAGE + 1) * (PAGE / 64 / 8), PAGE);
        if (!os_commit(mem->dirty + d0, d1 - d0)) return 0;
        for (u64 l = a / LINE; l < (b + LINE - 1) / LINE; l++) mem->dirty[l >> 3] |= (u8)(1u << (l & 7));
    }
    return m;
}
MITO_API void MITO(stop)(void *mp_) { Mito *m = (Mito *)mp_; if (!m) return; os_release(m->mem.base); os_release(m->mem.committed); os_release(m->mem.dirty); os_release(m->mem.jb.p); os_release(m->frees.p); os_release(m->sel.p); os_release(m->areas); os_release(m); }
MITO_API void MITO(set_loop_budget)(void *mp_, i64 n) { ((Mito *)mp_)->loop_budget = n; }
MITO_API void MITO(set_clock)(void *mp_, i64 ms, int fixed) { Mito *m = (Mito *)mp_; m->clock = ms; m->fixed_clock = fixed; }
MITO_API void MITO(set_marks)(void *mp_, u64 soft, u64 hard) { Mito *m = (Mito *)mp_;
    m->pr.on = 1; m->pr.soft = soft; m->pr.hard = hard; m->pr.refusing = 0; m->pr.trims = m->pr.full_trims = m->pr.freed = 0;
    if (m->mem.all) { m->mem.all = 0; trim_all(&m->mem); } /* pressure needs exact accounting: give back what is not live, commit on demand from here */
    for (u32 c = 0; c < m->mem.prog->ncolls; c++) m->pr.stuck[c] = ~0ULL;
}
static void out_err(Mito *m, const u8 *msg, u64 n) { m->out.len = 0; a_str(&m->out, "{\"error\":"); a_json_str(&m->out, msg, n); a_str(&m->out, "}"); }
static void out_err_c(Mito *m, const char *msg) { out_err(m, (const u8 *)msg, c_len(msg)); }
static u32 route(Mito *m, const u8 *method, u64 mn, const u8 *path, u64 pn) {
    u64 n = 0; for (u64 i = 0; i < pn; ) { while (i < pn && path[i] == '/') i++; if (i < pn) n++; while (i < pn && path[i] != '/') i++; }
    for (u32 e = 0; e < mito_prog.neps; e++) {
        const MitoRoute *r = &mito_prog.routes[e];
        if (!c_eq(r->method, method, mn) || r->nsegs != n) continue;
        u64 i = 0; u32 s = 0; int ok = 1;
        while (ok && s < r->nsegs) {
            while (i < pn && path[i] == '/') i++;
            u64 b = i; while (i < pn && path[i] != '/') i++;
            if (!r->segs[s].param && !c_eq(r->segs[s].text, path + b, i - b)) ok = 0;
            s++;
        }
        if (ok) return e;
    }
    return 0xFFFFFFFFu;
}
/* Handle one request, with its headers and (in a system) the fields forwarded with it. Returns the status, or 0 when
   the request was forwarded: then MITO(forward_to) says where, and MITO(finish_forward) commits or rolls back. */
MITO_API int MITO(handle)(void *mp_, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *hdrs, u32 nh, const MitoPair *fwd, u32 nf, const char *body, u32 bn) { Mito *m = (Mito *)mp_;
    m->out.len = 0; m->text.len = m->text_base; m->ekind = E_NONE;
    const u8 *t = (const u8 *)target;
    u64 q = 0; while (q < tn && t[q] != '?') q++;
    u32 ei = route(m, (const u8 *)method, mn, t, q);
    if (ei == 0xFFFFFFFFu) { out_err_c(m, "no such endpoint"); return 404; }
    if (m->pr.on && m->pr.refusing && !c_eq("GET", (const u8 *)method, mn) && !c_eq("DELETE", (const u8 *)method, mn)) { out_err_c(m, "memory pressure: try again later"); return 503; }
    m->ei = ei; m->hdrs = hdrs; m->nhdrs = nh; m->fwd = fwd; m->nfwd = nf;
    const MitoRoute *r = &mito_prog.routes[ei];
    m->nparams = 0; m->nquery = 0; m->njson = 0;
    int fits = 1;
    { u64 i = 0; u32 s = 0;
      while (s < r->nsegs) {
          while (i < q && t[i] == '/') i++;
          u64 b = i; while (i < q && t[i] != '/') i++;
          if (r->segs[s].param) { T x; if (m->nparams < MAX_PARAMS && push_decoded(m, t + b, i - b, 0, &x)) { m->params[m->nparams].seg = s; m->params[m->nparams].t = x; m->nparams++; } else fits = 0; }
          s++;
      } }
    for (u64 i = q + 1; i < tn; ) {
        u64 b = i; while (i < tn && t[i] != '&') i++;
        u64 e = i; i++;
        if (e == b) continue;
        u64 eq = b; while (eq < e && t[eq] != '=') eq++;
        T k, v;
        if (m->nquery == MAX_QUERY) { out_err_c(m, "request has more than 64 query parameters"); return 413; }
        if (push_decoded(m, t + b, eq - b, 1, &k) && push_decoded(m, t + (eq < e ? eq + 1 : e), eq < e ? e - eq - 1 : 0, 1, &v)) { m->query[m->nquery].k = k; m->query[m->nquery].v = v; m->nquery++; }
        else fits = 0;
    }
    if (fits) {
        int pr = parse_body(m, (const u8 *)body, bn);
        if (pr == -2) fits = 0;
        else if (pr == -3) { out_err_c(m, "request body has more than 256 fields"); return 413; }
        else if (pr) { out_err(m, m->ebuf, m->emsg.len); return 400; }
    }
    if (!fits) { out_err_c(m, "request too large for the request text area"); return 413; }
    m->now = m->fixed_clock ? m->clock : os_now_ms();
    begin(&m->mem);
    m->frees.len = 0; m->sel.len = 0;
    Ctx ctx = { m->mem.base, m, &mito_h, m->mem.dirty, m->mem.committed, m->mem.jon, m->text.p, 0, m->loop_budget };
    int res = mito_eps[ei](&ctx);
    if (res < 0 && m->ekind == E_FORWARD) { m->out.len = 0; m->pending = 1; return 0; } /* committed or rolled back by MITO(finish_forward) */
    int code, ok;
    if (res == 0) { m->out.len = 0; code = 204; ok = 1; }
    else if (res > 0) { code = res; ok = 1; }
    else {
        ok = 0;
        switch (m->ekind) {
            case E_FULL: { u8 b[4200]; Area x = { b, sizeof b, 0 }; a_str(&x, "capacity: "); a_put(&x, m->ebuf, m->emsg.len); out_err(m, b, x.len); code = 503; break; }
            case E_DEAD: out_err_c(m, "reference is not live"); code = 404; break;
            case E_NONE: out_err_c(m, "compiled code failed without a reason"); code = 500; break;
            default: out_err(m, m->ebuf, m->emsg.len); code = m->ecode; break;
        }
        /* the source line of the statement that failed, when the compiled code knew it */
        if (ctx.at && m->out.len && m->out.p[m->out.len - 1] == '}') { m->out.len--; a_str(&m->out, ",\"line\":"); a_u64(&m->out, ctx.at); a_str(&m->out, "}"); }
    }
    if (ok) { u32 *f = (u32 *)m->frees.p; for (u64 i = 0; i < m->frees.len; i += 2) free_item(&m->mem, f[i], f[i + 1]); commit_changes(&m->mem); }
    else rollback(&m->mem);
    if (m->pr.on) relieve(&m->mem, &m->pr);
    return code;
}
#ifdef MITO_NTESTS
/* Run test k on this instance: 0 passed, 1 failed (its message in msg), 2 its code failed (the error in msg); *line: the line
   of the `return` that failed it, or of the statement that failed. */
MITO_API int MITO(run_test)(void *mp_, u32 k, char *msg, u32 cap, u32 *line) { Mito *m = (Mito *)mp_;
    m->out.len = 0; m->text.len = MITO_LITS_LEN; m->text_base = MITO_LITS_LEN; m->ekind = E_NONE; m->nrqh = 0;
    m->now = m->fixed_clock ? m->clock : os_now_ms();
    m->frees.len = 0; m->sel.len = 0;
    Ctx ctx = { m->mem.base, m, &mito_h, m->mem.dirty, m->mem.committed, m->mem.jon, m->text.p, 0, m->loop_budget };
    Txt r = { 0, 0, 0 };
    int res = mito_tests[k](&ctx, &r);
    *line = ctx.at;
    u32 n = 0;
    if (res == 0) { msg[0] = 0; return 0; }
    if (res == 1) { n = r.len < cap - 1 ? r.len : cap - 1; memcpy(msg, m->text.p + r.off, n); msg[n] = 0; return 1; }
    switch (m->ekind) {
        case E_DEAD: { const char *s = "reference is not live"; n = (u32)c_len(s); memcpy(msg, s, n); break; }
        case E_NONE: { const char *s = "compiled code failed without a reason"; n = (u32)c_len(s); memcpy(msg, s, n); break; }
        default: n = (u32)(m->emsg.len < cap - 1 ? m->emsg.len : cap - 1); memcpy(msg, m->ebuf, n); break;
    }
    msg[n] = 0;
    return 2;
}
MITO_API u32 MITO(test_count)(void) { return MITO_NTESTS; }
MITO_API const char *MITO(test_name)(u32 k) { return mito_test_names[k]; }
#endif
MITO_API const char *MITO(response)(void *mp_, u32 *len) { Mito *m = (Mito *)mp_; *len = m->out.len; return (const char *)m->out.p; }

/* ---- in a system of workers ---- */
MITO_API void MITO(set_system)(void *mp_, u32 nkinds, const char *const *names, const u32 *counts, u32 me_kind, u32 me_index) { Mito *m = (Mito *)mp_;
    m->insys = 1; m->nkinds = nkinds; m->kind_names = names; m->kind_counts = counts; m->me_kind = me_kind; m->me_index = me_index;
}
/* Where the last request was forwarded (worker kind, key) and its fields, pointing into this request's text area. */
MITO_API u32 MITO(forward_to)(void *mp_, u32 *kind, i64 *key, MitoPair *out, u32 cap) { Mito *m = (Mito *)mp_;
    *kind = m->fwd_kind; *key = m->fwd_key;
    u32 n = m->nfwd_out < cap ? m->nfwd_out : cap;
    for (u32 i = 0; i < n; i++) {
        out[i].k = (const char *)t_ptr(m, m->fwd_out[i].k); out[i].kn = m->fwd_out[i].k.len;
        out[i].v = (const char *)t_ptr(m, m->fwd_out[i].v); out[i].vn = m->fwd_out[i].v.len;
    }
    return n;
}
/* Commit the forwarding request's writes (and frees) if it was handed on, or roll them back if it could not be. */
MITO_API void MITO(finish_forward)(void *mp_, int sent) { Mito *m = (Mito *)mp_;
    if (!m->pending) return;
    m->pending = 0;
    if (sent) { u32 *f = (u32 *)m->frees.p; for (u64 i = 0; i < m->frees.len; i += 2) free_item(&m->mem, f[i], f[i + 1]); commit_changes(&m->mem); }
    else rollback(&m->mem);
    if (m->pr.on) relieve(&m->mem, &m->pr);
}
/* Did the last request call the crash test hook? (The system then rebuilds this worker.) */
MITO_API int MITO(crashed)(void *mp_) { Mito *m = (Mito *)mp_; return m->crashed; }
MITO_API u64 MITO(committed)(void *mp_) { Mito *m = (Mito *)mp_; return m->mem.cpages * PAGE; }
/* The same lines the cases record: `state <name> <live bytes>` per collection, then the memory totals. */
MITO_API u32 MITO(state)(void *mp_, char *buf, u32 cap) { Mito *m = (Mito *)mp_;
    Area a = { (u8 *)buf, cap, 0 };
    a_str(&a, "state <plain values> "); a_u64(&a, m->mem.prog->plain_end); a_str(&a, "\n");
    for (u32 c = 0; c < m->mem.prog->ncolls; c++) { a_str(&a, "state "); a_str(&a, m->mem.prog->colls[c].name); a_str(&a, " "); a_u64(&a, coll_live(&m->mem, c)); a_str(&a, "\n"); }
    a_str(&a, "memory reserved="); a_u64(&a, m->mem.reserved); a_str(&a, " committed="); a_u64(&a, m->mem.cpages * PAGE); a_str(&a, " live="); a_u64(&a, live_total(&m->mem)); a_str(&a, "\n");
    return a.len;
}
MITO_API u32 MITO(stats)(void *mp_, char *buf, u32 cap) { Mito *m = (Mito *)mp_;
    Area a = { (u8 *)buf, cap, 0 };
    if (!m->pr.on) a_str(&a, "stats no marks");
    else { a_str(&a, "stats refusing="); a_str(&a, m->pr.refusing ? "true" : "false"); a_str(&a, " full_trims="); a_u64(&a, m->pr.full_trims); }
    return a.len;
}
