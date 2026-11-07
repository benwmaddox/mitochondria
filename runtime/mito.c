
/* ---- the C runtime (runtime/mito.c) ----
   Runs a compiled program: one reservation for the declared layout with pages committed where bytes are live, a write
   journal for exact rollback, requests in a fixed text area and response area, and the helper table (`H`) the
   generated endpoints call. It uses no heap: everything comes from reservations made at start and committed as used.
   It reproduces the Rust runtime's behaviour exactly (tests/cases check both against the same records). */

/* The operating system's part (memory, clock, randomness, standard error) is runtime/os.c, included before this file. */
static u64 up(u64 n, u64 to) { return (n + to - 1) / to * to; }

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
/* the first n bytes of a equal those of the lowercase literal b, letters compared without case */
static int b_eq_ci(const u8 *a, u32 n, const char *b) { for (u32 k = 0; k < n; k++) { u8 c = a[k]; if (c >= 'A' && c <= 'Z') c += 32; if (c != (u8)b[k]) return 0; } return 1; }
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
    u8 *scanned;         /* one flag per page: the undo log has looked for a page of zeros there (see save); cleared when it may be zeros again */
    u64 *shrinks;        /* per collection (top level): bumped whenever something in it is freed or cleared */
    Grow jb, ja; int jon;   /* the undo log: jb the saved lines (64 bytes each, in order), ja one address per entry (a "zeros" entry, low bit set, has no line in jb) */
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
   `dirty` marks the lines saved, so later writes to a saved line cost one bit test. Rollback puts the lines back.
   A page still all zeros (state never written there, as an array a request fills for the first time) is not copied: one
   entry, its address with the low bit set, says "zeros", and marks all of its lines saved. A page holding an unlogged line
   never qualifies (its lines stay marked), and the test runs once per page until a rollback zeros it again. */
#define LINE 64ULL
#define PAGE_LINES (PAGE / LINE)
static int zero_page(Mem *m, u64 p) {
    if (m->scanned[p]) return 0;
    u64 d; memcpy(&d, m->dirty + p * (PAGE_LINES / 8), 8);
    if (d) return 0;                                   /* written already in this request, or holding unlogged lines */
    m->scanned[p] = 1;
    if (!m->committed[p]) return 1;                    /* never committed: zeros (and not readable) */
    const u64 *w = (const u64 *)(m->base + p * PAGE);
    u64 acc = 0;
    for (u64 i = 0; i < PAGE / 8; i += 8) { acc |= w[i] | w[i + 1] | w[i + 2] | w[i + 3] | w[i + 4] | w[i + 5] | w[i + 6] | w[i + 7]; if (acc) return 0; }
    return 1;
}
static void save(Mem *m, u64 a, u64 n) {
#ifdef MITO_MEASURE_NO_JOURNAL /* for measuring what journaling costs only: failed requests are then not rolled back */
    return;
#endif
    if (!m->jon || !n) return;
    for (u64 l = a / LINE, last = (a + n - 1) / LINE; l <= last; l++) {
        u8 bit = (u8)(1u << (l & 7));
        if (!(l & 63) && l + 63 <= last) { u64 w; memcpy(&w, m->dirty + (l >> 3), 8); if (w == ~0ULL) { l += 63; continue; } } /* 64 lines already saved */
        if (m->dirty[l >> 3] & bit) continue;
        if (zero_page(m, l / PAGE_LINES)) {
            u64 pl = l / PAGE_LINES * PAGE_LINES;
            memset(m->dirty + (pl >> 3), 0xff, PAGE_LINES / 8);
            u64 apos = m->ja.len, aend = apos + 8;
            if (UNLIKELY(aend > m->ja.committed) && !grow_to(&m->ja, aend)) fatal("journal full");
            u64 at = pl * LINE | 1; memcpy(m->ja.p + apos, &at, 8);
            m->ja.len = aend;
            l = pl + PAGE_LINES - 1;
            continue;
        }
        m->dirty[l >> 3] |= bit;
        u64 pos = m->jb.len, end = pos + LINE, apos = m->ja.len, aend = apos + 8;
        if (UNLIKELY(end > m->jb.committed) && !grow_to(&m->jb, end)) fatal("journal full");
        if (UNLIKELY(aend > m->ja.committed) && !grow_to(&m->ja, aend)) fatal("journal full");
        u64 at = l * LINE; memcpy(m->ja.p + apos, &at, 8); memcpy(m->jb.p + pos, m->base + at, LINE);
        m->ja.len = aend; m->jb.len = end;
    }
}
static void wr(Mem *m, u64 off, u32 n, u64 v) { ensure(m, off, off + n); save(m, off, n); memcpy(m->base + off, &v, n); }
static void put(Mem *m, u64 off, const u8 *b, u64 n) { ensure(m, off, off + n); save(m, off, n); if (n) memcpy(m->base + off, b, n); }
static void zero(Mem *m, u64 off, u64 n) { if (!n) return; ensure(m, off, off + n); save(m, off, n); memset(m->base + off, 0, n); }
static void begin(Mem *m) { m->jon = 1; m->jb.len = 0; m->ja.len = 0; }
static void commit_changes(Mem *m) {
    for (u64 pos = 0; pos < m->ja.len; pos += 8) {
        u64 at; memcpy(&at, m->ja.p + pos, 8); u64 l = at / LINE;
        if (at & 1) memset(m->dirty + (l >> 3), 0, PAGE_LINES / 8); /* a page of zeros: all of its lines */
        else m->dirty[l >> 3] &= (u8)~(1u << (l & 7));
    }
    m->jon = 0; m->jb.len = 0; m->ja.len = 0;
}
static void rollback(Mem *m) {
    u64 dpos = m->jb.len;
    for (u64 pos = m->ja.len; pos > 0;) {
        pos -= 8;
        u64 at; memcpy(&at, m->ja.p + pos, 8);
        if (at & 1) { u64 p = at / PAGE; if (m->committed[p]) memset(m->base + p * PAGE, 0, PAGE); m->scanned[p] = 0; } /* zeros again */
        else { dpos -= LINE; memcpy(m->base + at, m->jb.p + dpos, LINE); }
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
            for (u64 q = s; q < p; q++) { m->committed[q] = 0; m->scanned[q] = 0; } /* zeros again when committed next */
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
#define RESPONSE_CAP (4 * 1024 * 1024) /* an answer: pages with long lists, images an endpoint sends */
/* The declared limits of a request, the same in every runtime (serve.rs MAX_QUERY ...). A route has at most 32 parts
   (checked when the program loads), names at most 128 bytes and fail messages 1024, so messages fit `ebuf`. */
#define MAX_PARAMS 32
#define MAX_QUERY 64
#define MAX_JSON 256
#define MAX_FREES 65536
#define QUOTE 64
#define MAX_SEL (1 << 24)

/* What went wrong in a request: the error kinds of the Rust runtime. */
enum { E_NONE, E_FAIL, E_FULL, E_TOOLONG, E_NOTLIVE, E_STALE, E_BAD, E_DEAD, E_FORWARD, E_WRITES };
#define MAX_FWD 64 /* fields of one forward */
typedef struct { u32 off, len; } T; /* a string in the text area */
typedef struct { T k; Val v; } JField;

typedef struct Mito {
    Mem mem;
    Area out, text;
    u8 *areas;
    i64 clock; int fixed_clock; u64 rng;
    int web_etag; /* the web block says `etag;`: the front end tags every answer, so a literal answer adds no ETag of its own */
    int starting; /* MITO(begin) is running the program's `@start` function */
    int bare; /* the answer leaves out the web block's header lines (`no_default_headers()`) */
    const u8 *target; u32 target_n; /* the request's target (path and query) as sent */
    Area ops; /* what the request asks of sockets (runtime/http.c applies it once the request succeeds): records, see sock_op */
    i64 loop_budget; /* rounds of `while` and `for` loops one request may run (`--loop-budget`) */
    u64 small;       /* bytes of the block holding the page flags (released with the instance) */
    u32 text_base;   /* where a request's text starts: after the literals, or after a running test's own text */
    /* a test's request being made: its body (JSON), its headers, as names and values in the text area */
    u8 rqbuf[16384]; u32 nrqh; struct { T k, v; } rqh[16];
    Pressure pr;
    /* the request being handled */
    u32 ei; const MitoPair *hdrs; u32 nhdrs; const MitoPair *fwd; u32 nfwd; i64 now;
    u32 nparams; struct { u32 seg; T t; } params[MAX_PARAMS];
    u32 nquery; struct { T k, v; } query[MAX_QUERY];
    u32 njson; JField json[MAX_JSON]; int form; /* form: the body was a form (application/x-www-form-urlencoded), its values text */
    int ekind; int ecode; Area emsg; u8 ebuf[4096];
    /* the answer's head: its content type (0 JSON, 1 HTML, 2 plain text) and header lines after it (`Location: ...\r\n`) */
    u32 ctype; Area head; u8 hbuf[16384]; u8 hout[16384 + 64];
#ifdef MITO_USES_SQLITE
    Store store; /* the program's tables, in SQLite (runtime/store.c) */
#endif
    int html_err; /* the endpoint answers HTML: an error is a small page, not JSON */
    Grow frees; /* (collection, item) pairs, u32 each */
    Grow sel;   /* items collected by loops (u32 each) */
    int crashed;
    /* in a system of workers: every kind's name and count, and which worker this is */
    int insys; u32 nkinds, me_kind, me_index; const char *const *kind_names; const u32 *kind_counts;
    u32 nfwd_out; struct { T k, v; } fwd_out[MAX_FWD]; /* the fields of the forward being made */
    int pending; u32 fwd_kind; i64 fwd_key;         /* a forward waiting for MITO(finish_forward) */
    /* a reader (MITO(reader)): an instance that runs requests alongside other readers, the state shared with the primary instance and
       mapped read-only, its own request struct, text, answer and database connection. A request that would change anything else stops
       (wants_write) and runs again alone on the primary. */
    int reader, wants_write;
    u32 pool; /* SQLite's pool for this instance's connection (with db_pools): 0 the primary's, k + 1 reader k's */
    u32 tw; /* table writes of the request (with its undo log, whether it changed anything: the data version) */
    /* the answer being cached (cache_start .. cache_put): where it starts in the answer, and its holes (where each ends, its label);
       the entry cache_get found: its bytes in the text area */
    u32 c_on, c_start, c_nholes; struct { u32 at, end; T label; } c_holes[64]; T c_entry;
    /* the state's shared memory (MITO(share_state) before MITO(start)): the handle readers map */
    i64 shared_fd;
#ifdef MITO_USES_SQLITE
    char db_dir[600], db_file[600]; u64 db_pool; int db_flags; /* how the tables were opened, for readers' connections */
#endif
#ifdef MITO_USES_HTML
    HPool hp; /* the request's HTML nodes (runtime/html.c) */
#endif
} Mito;

#define MITO_TBASE (1u << 24) /* a table is collection MITO_TBASE + its index in generated code */
#ifdef MITO_USES_SQLITE
static int t_token_row(Mito *m, u32 t, const u8 *v, u64 n, u32 *out);
static int t_get(Mito *m, u32 t, u32 rowid, u32 col, Val *out);
#endif
/* ---- errors: kind, status and message ---- */
static int err(Mito *m, int kind, int code) { m->ekind = kind; m->ecode = code; m->emsg.len = 0; return -1; }
static int fail(Mito *m, int code, const char *msg) { err(m, E_FAIL, code); a_str(&m->emsg, msg); return -1; }
static int text_full(Mito *m) { return fail(m, 507, "request text area is full"); }
static int resp_full(Mito *m) { return fail(m, 507, "response too large"); }

static T t_since(Mito *m, u32 start) { T t = { start, m->text.len - start }; return t; }
static const u8 *t_ptr(Mito *m, T t) { return m->text.p + t.off; }
static int t_push(Mito *m, const void *b, u64 n, T *out) { u32 s = m->text.len; if (!a_put(&m->text, b, n)) return text_full(m); *out = t_since(m, s); return 0; }

/* all_emoji's code points (inclusive ranges, sorted), as Ruby 3.4's regexp reads the two properties and U+FE0F */
static const u32 EMOJI_RANGES[] = { 0xA9,0xA9, 0xAE,0xAE, 0x203C,0x203C, 0x2049,0x2049, 0x2122,0x2122, 0x2139,0x2139, 0x2194,0x2199, 0x21A9,0x21AA, 0x231A,0x231B, 0x2328,0x2328, 0x2388,0x2388, 0x23CF,0x23CF, 0x23E9,0x23F3, 0x23F8,0x23FA, 0x24C2,0x24C2, 0x25AA,0x25AB, 0x25B6,0x25B6, 0x25C0,0x25C0, 0x25FB,0x25FE, 0x2600,0x2605, 0x2607,0x2612, 0x2614,0x2685, 0x2690,0x2705, 0x2708,0x2712, 0x2714,0x2714, 0x2716,0x2716, 0x271D,0x271D, 0x2721,0x2721, 0x2728,0x2728, 0x2733,0x2734, 0x2744,0x2744, 0x2747,0x2747, 0x274C,0x274C, 0x274E,0x274E, 0x2753,0x2755, 0x2757,0x2757, 0x2763,0x2767, 0x2795,0x2797, 0x27A1,0x27A1, 0x27B0,0x27B0, 0x27BF,0x27BF, 0x2934,0x2935, 0x2B05,0x2B07, 0x2B1B,0x2B1C, 0x2B50,0x2B50, 0x2B55,0x2B55, 0x3030,0x3030, 0x303D,0x303D, 0x3297,0x3297, 0x3299,0x3299, 0xFE0F,0xFE0F, 0x1F000,0x1F0FF, 0x1F10D,0x1F10F, 0x1F12F,0x1F12F, 0x1F16C,0x1F171, 0x1F17E,0x1F17F, 0x1F18E,0x1F18E, 0x1F191,0x1F19A, 0x1F1AD,0x1F1FF, 0x1F201,0x1F20F, 0x1F21A,0x1F21A, 0x1F22F,0x1F22F, 0x1F232,0x1F23A, 0x1F23C,0x1F23F, 0x1F249,0x1F53D, 0x1F546,0x1F64F, 0x1F680,0x1F6FF, 0x1F774,0x1F77F, 0x1F7D5,0x1F7FF, 0x1F80C,0x1F80F, 0x1F848,0x1F84F, 0x1F85A,0x1F85F, 0x1F888,0x1F88F, 0x1F8AE,0x1F8FF, 0x1F90C,0x1F93A, 0x1F93C,0x1F945, 0x1F947,0x1FAFF, 0x1FC00,0x1FFFD };
/* all_word's: \p{Word} as Ruby 3.4's regexp reads it (letters, marks, numbers, connector punctuation) */
static const u32 WORD_RANGES[] = { 0x30,0x39, 0x41,0x5A, 0x5F,0x5F, 0x61,0x7A, 0xAA,0xAA, 0xB5,0xB5, 0xBA,0xBA, 0xC0,0xD6, 0xD8,0xF6, 0xF8,0x2C1, 0x2C6,0x2D1, 0x2E0,0x2E4, 0x2EC,0x2EC, 0x2EE,0x2EE, 0x300,0x374, 0x376,0x377, 0x37A,0x37D, 0x37F,0x37F, 0x386,0x386, 0x388,0x38A, 0x38C,0x38C, 0x38E,0x3A1, 0x3A3,0x3F5, 0x3F7,0x481, 0x483,0x52F, 0x531,0x556, 0x559,0x559, 0x560,0x588, 0x591,0x5BD, 0x5BF,0x5BF, 0x5C1,0x5C2, 0x5C4,0x5C5, 0x5C7,0x5C7, 0x5D0,0x5EA, 0x5EF,0x5F2, 0x610,0x61A, 0x620,0x669, 0x66E,0x6D3, 0x6D5,0x6DC, 0x6DF,0x6E8, 0x6EA,0x6FC, 0x6FF,0x6FF, 0x710,0x74A, 0x74D,0x7B1, 0x7C0,0x7F5, 0x7FA,0x7FA, 0x7FD,0x7FD, 0x800,0x82D, 0x840,0x85B, 0x860,0x86A, 0x870,0x887, 0x889,0x88E, 0x898,0x8E1, 0x8E3,0x963, 0x966,0x96F, 0x971,0x983, 0x985,0x98C, 0x98F,0x990, 0x993,0x9A8, 0x9AA,0x9B0, 0x9B2,0x9B2, 0x9B6,0x9B9, 0x9BC,0x9C4, 0x9C7,0x9C8, 0x9CB,0x9CE, 0x9D7,0x9D7, 0x9DC,0x9DD, 0x9DF,0x9E3, 0x9E6,0x9F1, 0x9FC,0x9FC, 0x9FE,0x9FE, 0xA01,0xA03, 0xA05,0xA0A, 0xA0F,0xA10, 0xA13,0xA28, 0xA2A,0xA30, 0xA32,0xA33, 0xA35,0xA36, 0xA38,0xA39, 0xA3C,0xA3C, 0xA3E,0xA42, 0xA47,0xA48, 0xA4B,0xA4D, 0xA51,0xA51, 0xA59,0xA5C, 0xA5E,0xA5E, 0xA66,0xA75, 0xA81,0xA83, 0xA85,0xA8D, 0xA8F,0xA91, 0xA93,0xAA8, 0xAAA,0xAB0, 0xAB2,0xAB3, 0xAB5,0xAB9, 0xABC,0xAC5, 0xAC7,0xAC9, 0xACB,0xACD, 0xAD0,0xAD0, 0xAE0,0xAE3, 0xAE6,0xAEF, 0xAF9,0xAFF, 0xB01,0xB03, 0xB05,0xB0C, 0xB0F,0xB10, 0xB13,0xB28, 0xB2A,0xB30, 0xB32,0xB33, 0xB35,0xB39, 0xB3C,0xB44, 0xB47,0xB48, 0xB4B,0xB4D, 0xB55,0xB57, 0xB5C,0xB5D, 0xB5F,0xB63, 0xB66,0xB6F, 0xB71,0xB71, 0xB82,0xB83, 0xB85,0xB8A, 0xB8E,0xB90, 0xB92,0xB95, 0xB99,0xB9A, 0xB9C,0xB9C, 0xB9E,0xB9F, 0xBA3,0xBA4, 0xBA8,0xBAA, 0xBAE,0xBB9, 0xBBE,0xBC2, 0xBC6,0xBC8, 0xBCA,0xBCD, 0xBD0,0xBD0, 0xBD7,0xBD7, 0xBE6,0xBEF, 0xC00,0xC0C, 0xC0E,0xC10, 0xC12,0xC28, 0xC2A,0xC39, 0xC3C,0xC44, 0xC46,0xC48, 0xC4A,0xC4D, 0xC55,0xC56, 0xC58,0xC5A, 0xC5D,0xC5D, 0xC60,0xC63, 0xC66,0xC6F, 0xC80,0xC83, 0xC85,0xC8C, 0xC8E,0xC90, 0xC92,0xCA8, 0xCAA,0xCB3, 0xCB5,0xCB9, 0xCBC,0xCC4, 0xCC6,0xCC8, 0xCCA,0xCCD, 0xCD5,0xCD6, 0xCDD,0xCDE, 0xCE0,0xCE3, 0xCE6,0xCEF, 0xCF1,0xCF3, 0xD00,0xD0C, 0xD0E,0xD10, 0xD12,0xD44, 0xD46,0xD48, 0xD4A,0xD4E, 0xD54,0xD57, 0xD5F,0xD63, 0xD66,0xD6F, 0xD7A,0xD7F, 0xD81,0xD83, 0xD85,0xD96, 0xD9A,0xDB1, 0xDB3,0xDBB, 0xDBD,0xDBD, 0xDC0,0xDC6, 0xDCA,0xDCA, 0xDCF,0xDD4, 0xDD6,0xDD6, 0xDD8,0xDDF, 0xDE6,0xDEF, 0xDF2,0xDF3, 0xE01,0xE3A, 0xE40,0xE4E, 0xE50,0xE59, 0xE81,0xE82, 0xE84,0xE84, 0xE86,0xE8A, 0xE8C,0xEA3, 0xEA5,0xEA5, 0xEA7,0xEBD, 0xEC0,0xEC4, 0xEC6,0xEC6, 0xEC8,0xECE, 0xED0,0xED9, 0xEDC,0xEDF, 0xF00,0xF00, 0xF18,0xF19, 0xF20,0xF29, 0xF35,0xF35, 0xF37,0xF37, 0xF39,0xF39, 0xF3E,0xF47, 0xF49,0xF6C, 0xF71,0xF84, 0xF86,0xF97, 0xF99,0xFBC, 0xFC6,0xFC6, 0x1000,0x1049, 0x1050,0x109D, 0x10A0,0x10C5, 0x10C7,0x10C7, 0x10CD,0x10CD, 0x10D0,0x10FA, 0x10FC,0x1248, 0x124A,0x124D, 0x1250,0x1256, 0x1258,0x1258, 0x125A,0x125D, 0x1260,0x1288, 0x128A,0x128D, 0x1290,0x12B0, 0x12B2,0x12B5, 0x12B8,0x12BE, 0x12C0,0x12C0, 0x12C2,0x12C5, 0x12C8,0x12D6, 0x12D8,0x1310, 0x1312,0x1315, 0x1318,0x135A, 0x135D,0x135F, 0x1380,0x138F, 0x13A0,0x13F5, 0x13F8,0x13FD, 0x1401,0x166C, 0x166F,0x167F, 0x1681,0x169A, 0x16A0,0x16EA, 0x16EE,0x16F8, 0x1700,0x1715, 0x171F,0x1734, 0x1740,0x1753, 0x1760,0x176C, 0x176E,0x1770, 0x1772,0x1773, 0x1780,0x17D3, 0x17D7,0x17D7, 0x17DC,0x17DD, 0x17E0,0x17E9, 0x180B,0x180D, 0x180F,0x1819, 0x1820,0x1878, 0x1880,0x18AA, 0x18B0,0x18F5, 0x1900,0x191E, 0x1920,0x192B, 0x1930,0x193B, 0x1946,0x196D, 0x1970,0x1974, 0x1980,0x19AB, 0x19B0,0x19C9, 0x19D0,0x19D9, 0x1A00,0x1A1B, 0x1A20,0x1A5E, 0x1A60,0x1A7C, 0x1A7F,0x1A89, 0x1A90,0x1A99, 0x1AA7,0x1AA7, 0x1AB0,0x1ACE, 0x1B00,0x1B4C, 0x1B50,0x1B59, 0x1B6B,0x1B73, 0x1B80,0x1BF3, 0x1C00,0x1C37, 0x1C40,0x1C49, 0x1C4D,0x1C7D, 0x1C80,0x1C88, 0x1C90,0x1CBA, 0x1CBD,0x1CBF, 0x1CD0,0x1CD2, 0x1CD4,0x1CFA, 0x1D00,0x1F15, 0x1F18,0x1F1D, 0x1F20,0x1F45, 0x1F48,0x1F4D, 0x1F50,0x1F57, 0x1F59,0x1F59, 0x1F5B,0x1F5B, 0x1F5D,0x1F5D, 0x1F5F,0x1F7D, 0x1F80,0x1FB4, 0x1FB6,0x1FBC, 0x1FBE,0x1FBE, 0x1FC2,0x1FC4, 0x1FC6,0x1FCC, 0x1FD0,0x1FD3, 0x1FD6,0x1FDB, 0x1FE0,0x1FEC, 0x1FF2,0x1FF4, 0x1FF6,0x1FFC, 0x200C,0x200D, 0x203F,0x2040, 0x2054,0x2054, 0x2071,0x2071, 0x207F,0x207F, 0x2090,0x209C, 0x20D0,0x20F0, 0x2102,0x2102, 0x2107,0x2107, 0x210A,0x2113, 0x2115,0x2115, 0x2119,0x211D, 0x2124,0x2124, 0x2126,0x2126, 0x2128,0x2128, 0x212A,0x212D, 0x212F,0x2139, 0x213C,0x213F, 0x2145,0x2149, 0x214E,0x214E, 0x2160,0x2188, 0x24B6,0x24E9, 0x2C00,0x2CE4, 0x2CEB,0x2CF3, 0x2D00,0x2D25, 0x2D27,0x2D27, 0x2D2D,0x2D2D, 0x2D30,0x2D67, 0x2D6F,0x2D6F, 0x2D7F,0x2D96, 0x2DA0,0x2DA6, 0x2DA8,0x2DAE, 0x2DB0,0x2DB6, 0x2DB8,0x2DBE, 0x2DC0,0x2DC6, 0x2DC8,0x2DCE, 0x2DD0,0x2DD6, 0x2DD8,0x2DDE, 0x2DE0,0x2DFF, 0x2E2F,0x2E2F, 0x3005,0x3007, 0x3021,0x302F, 0x3031,0x3035, 0x3038,0x303C, 0x3041,0x3096, 0x3099,0x309A, 0x309D,0x309F, 0x30A1,0x30FA, 0x30FC,0x30FF, 0x3105,0x312F, 0x3131,0x318E, 0x31A0,0x31BF, 0x31F0,0x31FF, 0x3400,0x4DBF, 0x4E00,0xA48C, 0xA4D0,0xA4FD, 0xA500,0xA60C, 0xA610,0xA62B, 0xA640,0xA672, 0xA674,0xA67D, 0xA67F,0xA6F1, 0xA717,0xA71F, 0xA722,0xA788, 0xA78B,0xA7CA, 0xA7D0,0xA7D1, 0xA7D3,0xA7D3, 0xA7D5,0xA7D9, 0xA7F2,0xA827, 0xA82C,0xA82C, 0xA840,0xA873, 0xA880,0xA8C5, 0xA8D0,0xA8D9, 0xA8E0,0xA8F7, 0xA8FB,0xA8FB, 0xA8FD,0xA92D, 0xA930,0xA953, 0xA960,0xA97C, 0xA980,0xA9C0, 0xA9CF,0xA9D9, 0xA9E0,0xA9FE, 0xAA00,0xAA36, 0xAA40,0xAA4D, 0xAA50,0xAA59, 0xAA60,0xAA76, 0xAA7A,0xAAC2, 0xAADB,0xAADD, 0xAAE0,0xAAEF, 0xAAF2,0xAAF6, 0xAB01,0xAB06, 0xAB09,0xAB0E, 0xAB11,0xAB16, 0xAB20,0xAB26, 0xAB28,0xAB2E, 0xAB30,0xAB5A, 0xAB5C,0xAB69, 0xAB70,0xABEA, 0xABEC,0xABED, 0xABF0,0xABF9, 0xAC00,0xD7A3, 0xD7B0,0xD7C6, 0xD7CB,0xD7FB, 0xF900,0xFA6D, 0xFA70,0xFAD9, 0xFB00,0xFB06, 0xFB13,0xFB17, 0xFB1D,0xFB28, 0xFB2A,0xFB36, 0xFB38,0xFB3C, 0xFB3E,0xFB3E, 0xFB40,0xFB41, 0xFB43,0xFB44, 0xFB46,0xFBB1, 0xFBD3,0xFD3D, 0xFD50,0xFD8F, 0xFD92,0xFDC7, 0xFDF0,0xFDFB, 0xFE00,0xFE0F, 0xFE20,0xFE2F, 0xFE33,0xFE34, 0xFE4D,0xFE4F, 0xFE70,0xFE74, 0xFE76,0xFEFC, 0xFF10,0xFF19, 0xFF21,0xFF3A, 0xFF3F,0xFF3F, 0xFF41,0xFF5A, 0xFF66,0xFFBE, 0xFFC2,0xFFC7, 0xFFCA,0xFFCF, 0xFFD2,0xFFD7, 0xFFDA,0xFFDC, 0x10000,0x1000B, 0x1000D,0x10026, 0x10028,0x1003A, 0x1003C,0x1003D, 0x1003F,0x1004D, 0x10050,0x1005D, 0x10080,0x100FA, 0x10140,0x10174, 0x101FD,0x101FD, 0x10280,0x1029C, 0x102A0,0x102D0, 0x102E0,0x102E0, 0x10300,0x1031F, 0x1032D,0x1034A, 0x10350,0x1037A, 0x10380,0x1039D, 0x103A0,0x103C3, 0x103C8,0x103CF, 0x103D1,0x103D5, 0x10400,0x1049D, 0x104A0,0x104A9, 0x104B0,0x104D3, 0x104D8,0x104FB, 0x10500,0x10527, 0x10530,0x10563, 0x10570,0x1057A, 0x1057C,0x1058A, 0x1058C,0x10592, 0x10594,0x10595, 0x10597,0x105A1, 0x105A3,0x105B1, 0x105B3,0x105B9, 0x105BB,0x105BC, 0x10600,0x10736, 0x10740,0x10755, 0x10760,0x10767, 0x10780,0x10785, 0x10787,0x107B0, 0x107B2,0x107BA, 0x10800,0x10805, 0x10808,0x10808, 0x1080A,0x10835, 0x10837,0x10838, 0x1083C,0x1083C, 0x1083F,0x10855, 0x10860,0x10876, 0x10880,0x1089E, 0x108E0,0x108F2, 0x108F4,0x108F5, 0x10900,0x10915, 0x10920,0x10939, 0x10980,0x109B7, 0x109BE,0x109BF, 0x10A00,0x10A03, 0x10A05,0x10A06, 0x10A0C,0x10A13, 0x10A15,0x10A17, 0x10A19,0x10A35, 0x10A38,0x10A3A, 0x10A3F,0x10A3F, 0x10A60,0x10A7C, 0x10A80,0x10A9C, 0x10AC0,0x10AC7, 0x10AC9,0x10AE6, 0x10B00,0x10B35, 0x10B40,0x10B55, 0x10B60,0x10B72, 0x10B80,0x10B91, 0x10C00,0x10C48, 0x10C80,0x10CB2, 0x10CC0,0x10CF2, 0x10D00,0x10D27, 0x10D30,0x10D39, 0x10E80,0x10EA9, 0x10EAB,0x10EAC, 0x10EB0,0x10EB1, 0x10EFD,0x10F1C, 0x10F27,0x10F27, 0x10F30,0x10F50, 0x10F70,0x10F85, 0x10FB0,0x10FC4, 0x10FE0,0x10FF6, 0x11000,0x11046, 0x11066,0x11075, 0x1107F,0x110BA, 0x110C2,0x110C2, 0x110D0,0x110E8, 0x110F0,0x110F9, 0x11100,0x11134, 0x11136,0x1113F, 0x11144,0x11147, 0x11150,0x11173, 0x11176,0x11176, 0x11180,0x111C4, 0x111C9,0x111CC, 0x111CE,0x111DA, 0x111DC,0x111DC, 0x11200,0x11211, 0x11213,0x11237, 0x1123E,0x11241, 0x11280,0x11286, 0x11288,0x11288, 0x1128A,0x1128D, 0x1128F,0x1129D, 0x1129F,0x112A8, 0x112B0,0x112EA, 0x112F0,0x112F9, 0x11300,0x11303, 0x11305,0x1130C, 0x1130F,0x11310, 0x11313,0x11328, 0x1132A,0x11330, 0x11332,0x11333, 0x11335,0x11339, 0x1133B,0x11344, 0x11347,0x11348, 0x1134B,0x1134D, 0x11350,0x11350, 0x11357,0x11357, 0x1135D,0x11363, 0x11366,0x1136C, 0x11370,0x11374, 0x11400,0x1144A, 0x11450,0x11459, 0x1145E,0x11461, 0x11480,0x114C5, 0x114C7,0x114C7, 0x114D0,0x114D9, 0x11580,0x115B5, 0x115B8,0x115C0, 0x115D8,0x115DD, 0x11600,0x11640, 0x11644,0x11644, 0x11650,0x11659, 0x11680,0x116B8, 0x116C0,0x116C9, 0x11700,0x1171A, 0x1171D,0x1172B, 0x11730,0x11739, 0x11740,0x11746, 0x11800,0x1183A, 0x118A0,0x118E9, 0x118FF,0x11906, 0x11909,0x11909, 0x1190C,0x11913, 0x11915,0x11916, 0x11918,0x11935, 0x11937,0x11938, 0x1193B,0x11943, 0x11950,0x11959, 0x119A0,0x119A7, 0x119AA,0x119D7, 0x119DA,0x119E1, 0x119E3,0x119E4, 0x11A00,0x11A3E, 0x11A47,0x11A47, 0x11A50,0x11A99, 0x11A9D,0x11A9D, 0x11AB0,0x11AF8, 0x11C00,0x11C08, 0x11C0A,0x11C36, 0x11C38,0x11C40, 0x11C50,0x11C59, 0x11C72,0x11C8F, 0x11C92,0x11CA7, 0x11CA9,0x11CB6, 0x11D00,0x11D06, 0x11D08,0x11D09, 0x11D0B,0x11D36, 0x11D3A,0x11D3A, 0x11D3C,0x11D3D, 0x11D3F,0x11D47, 0x11D50,0x11D59, 0x11D60,0x11D65, 0x11D67,0x11D68, 0x11D6A,0x11D8E, 0x11D90,0x11D91, 0x11D93,0x11D98, 0x11DA0,0x11DA9, 0x11EE0,0x11EF6, 0x11F00,0x11F10, 0x11F12,0x11F3A, 0x11F3E,0x11F42, 0x11F50,0x11F59, 0x11FB0,0x11FB0, 0x12000,0x12399, 0x12400,0x1246E, 0x12480,0x12543, 0x12F90,0x12FF0, 0x13000,0x1342F, 0x13440,0x13455, 0x14400,0x14646, 0x16800,0x16A38, 0x16A40,0x16A5E, 0x16A60,0x16A69, 0x16A70,0x16ABE, 0x16AC0,0x16AC9, 0x16AD0,0x16AED, 0x16AF0,0x16AF4, 0x16B00,0x16B36, 0x16B40,0x16B43, 0x16B50,0x16B59, 0x16B63,0x16B77, 0x16B7D,0x16B8F, 0x16E40,0x16E7F, 0x16F00,0x16F4A, 0x16F4F,0x16F87, 0x16F8F,0x16F9F, 0x16FE0,0x16FE1, 0x16FE3,0x16FE4, 0x16FF0,0x16FF1, 0x17000,0x187F7, 0x18800,0x18CD5, 0x18D00,0x18D08, 0x1AFF0,0x1AFF3, 0x1AFF5,0x1AFFB, 0x1AFFD,0x1AFFE, 0x1B000,0x1B122, 0x1B132,0x1B132, 0x1B150,0x1B152, 0x1B155,0x1B155, 0x1B164,0x1B167, 0x1B170,0x1B2FB, 0x1BC00,0x1BC6A, 0x1BC70,0x1BC7C, 0x1BC80,0x1BC88, 0x1BC90,0x1BC99, 0x1BC9D,0x1BC9E, 0x1CF00,0x1CF2D, 0x1CF30,0x1CF46, 0x1D165,0x1D169, 0x1D16D,0x1D172, 0x1D17B,0x1D182, 0x1D185,0x1D18B, 0x1D1AA,0x1D1AD, 0x1D242,0x1D244, 0x1D400,0x1D454, 0x1D456,0x1D49C, 0x1D49E,0x1D49F, 0x1D4A2,0x1D4A2, 0x1D4A5,0x1D4A6, 0x1D4A9,0x1D4AC, 0x1D4AE,0x1D4B9, 0x1D4BB,0x1D4BB, 0x1D4BD,0x1D4C3, 0x1D4C5,0x1D505, 0x1D507,0x1D50A, 0x1D50D,0x1D514, 0x1D516,0x1D51C, 0x1D51E,0x1D539, 0x1D53B,0x1D53E, 0x1D540,0x1D544, 0x1D546,0x1D546, 0x1D54A,0x1D550, 0x1D552,0x1D6A5, 0x1D6A8,0x1D6C0, 0x1D6C2,0x1D6DA, 0x1D6DC,0x1D6FA, 0x1D6FC,0x1D714, 0x1D716,0x1D734, 0x1D736,0x1D74E, 0x1D750,0x1D76E, 0x1D770,0x1D788, 0x1D78A,0x1D7A8, 0x1D7AA,0x1D7C2, 0x1D7C4,0x1D7CB, 0x1D7CE,0x1D7FF, 0x1DA00,0x1DA36, 0x1DA3B,0x1DA6C, 0x1DA75,0x1DA75, 0x1DA84,0x1DA84, 0x1DA9B,0x1DA9F, 0x1DAA1,0x1DAAF, 0x1DF00,0x1DF1E, 0x1DF25,0x1DF2A, 0x1E000,0x1E006, 0x1E008,0x1E018, 0x1E01B,0x1E021, 0x1E023,0x1E024, 0x1E026,0x1E02A, 0x1E030,0x1E06D, 0x1E08F,0x1E08F, 0x1E100,0x1E12C, 0x1E130,0x1E13D, 0x1E140,0x1E149, 0x1E14E,0x1E14E, 0x1E290,0x1E2AE, 0x1E2C0,0x1E2F9, 0x1E4D0,0x1E4F9, 0x1E7E0,0x1E7E6, 0x1E7E8,0x1E7EB, 0x1E7ED,0x1E7EE, 0x1E7F0,0x1E7FE, 0x1E800,0x1E8C4, 0x1E8D0,0x1E8D6, 0x1E900,0x1E94B, 0x1E950,0x1E959, 0x1EE00,0x1EE03, 0x1EE05,0x1EE1F, 0x1EE21,0x1EE22, 0x1EE24,0x1EE24, 0x1EE27,0x1EE27, 0x1EE29,0x1EE32, 0x1EE34,0x1EE37, 0x1EE39,0x1EE39, 0x1EE3B,0x1EE3B, 0x1EE42,0x1EE42, 0x1EE47,0x1EE47, 0x1EE49,0x1EE49, 0x1EE4B,0x1EE4B, 0x1EE4D,0x1EE4F, 0x1EE51,0x1EE52, 0x1EE54,0x1EE54, 0x1EE57,0x1EE57, 0x1EE59,0x1EE59, 0x1EE5B,0x1EE5B, 0x1EE5D,0x1EE5D, 0x1EE5F,0x1EE5F, 0x1EE61,0x1EE62, 0x1EE64,0x1EE64, 0x1EE67,0x1EE6A, 0x1EE6C,0x1EE72, 0x1EE74,0x1EE77, 0x1EE79,0x1EE7C, 0x1EE7E,0x1EE7E, 0x1EE80,0x1EE89, 0x1EE8B,0x1EE9B, 0x1EEA1,0x1EEA3, 0x1EEA5,0x1EEA9, 0x1EEAB,0x1EEBB, 0x1F130,0x1F149, 0x1F150,0x1F169, 0x1F170,0x1F189, 0x1FBF0,0x1FBF9, 0x20000,0x2A6DF, 0x2A700,0x2B739, 0x2B740,0x2B81D, 0x2B820,0x2CEA1, 0x2CEB0,0x2EBE0, 0x2F800,0x2FA1D, 0x30000,0x3134A, 0x31350,0x323AF, 0xE0100,0xE01EF };
/* is c in the sorted inclusive ranges r (n pairs) */
static int in_ranges(const u32 *r, u32 n, u32 c) {
    u32 lo = 0, hi = n;
    while (lo < hi) { u32 k = (lo + hi) / 2; if (c < r[2 * k]) hi = k; else if (c > r[2 * k + 1]) lo = k + 1; else return 1; }
    return 0;
}
static int emoji_cp(u32 c) { return in_ranges(EMOJI_RANGES, (u32)(sizeof EMOJI_RANGES / sizeof EMOJI_RANGES[0]) / 2, c); }
static int word_cp(u32 c) { return in_ranges(WORD_RANGES, (u32)(sizeof WORD_RANGES / sizeof WORD_RANGES[0]) / 2, c); }
/* every character of s (UTF-8) in the class (0 emoji, 1 word), and at least one */
static int all_of(const u8 *s, u32 n, int cls) {
    if (n == 0) return 0;
    for (u32 i = 0; i < n; ) {
        u32 c, k; u8 b = s[i];
        if (b < 0x80) { c = b; k = 1; } else if ((b & 0xE0) == 0xC0) { c = b & 0x1F; k = 2; } else if ((b & 0xF0) == 0xE0) { c = b & 0x0F; k = 3; } else if ((b & 0xF8) == 0xF0) { c = b & 0x07; k = 4; } else return 0;
        if (i + k > n) return 0;
        for (u32 j = 1; j < k; j++) { if ((s[i + j] & 0xC0) != 0x80) return 0; c = (c << 6) | (s[i + j] & 0x3F); }
        if (!(cls ? word_cp(c) : emoji_cp(c))) return 0;
        i += k;
    }
    return 1;
}
static Val v_null(void) { Val v = { 0, 0, 0, 0, 0 }; return v; }
static Val v_int(i64 i) { Val v = { 1, 0, 0, i, 0 }; return v; }
static Val v_flt(double f) { Val v = { 2, 0, 0, 0, f }; return v; }
static Val v_bool(int b) { Val v = { 3, 0, 0, b ? 1 : 0, 0 }; return v; }
static Val v_str(T t) { Val v = { 4, t.off, t.len, 0, 0 }; return v; }
static Val v_item(u32 c, u32 i) { Val v = { 5, c, i, 0, 0 }; return v; }
static Mito *M_(Ctx *c) { return (Mito *)c->run; }
/* A reader changes only its own request struct: anything else stops the request (it runs again alone, on the primary instance). */
static int rd_writes(Mito *m) { m->wants_write = 1; err(m, E_WRITES, 503); a_str(&m->emsg, "a reader cannot change the state"); return -1; }
static int in_request(u64 a, u64 n) { return mito_prog.req_size && a >= mito_prog.req_off && a + n <= mito_prog.req_off + mito_prog.req_size; }
#define RD_ANY(m) do { if (UNLIKELY((m)->reader)) return rd_writes(m); } while (0)
#define RD_AT(m, a, n) do { if (UNLIKELY((m)->reader) && !in_request((a), (n))) return rd_writes(m); } while (0)

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
/* `ascii<N>` text holds bytes 0..127 only */
static int not_ascii(Mito *m, const u8 *p, u64 n) {
    for (u64 i = 0; i < n; i++) if (p[i] > 127) { err(m, E_BAD, 400); a_str(&m->emsg, "ascii text cannot hold byte "); a_u64(&m->emsg, p[i]); a_str(&m->emsg, " (at "); a_u64(&m->emsg, i); a_str(&m->emsg, ")"); return 1; }
    return 0;
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
        if (col->scalar == 1 && not_ascii(m, t_ptr(m, (T){ v->a, v->b }), v->b)) return -1; /* an ascii<N> column */
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
/* Copy a path segment, query or form part into the text area, decoding %XX (and + as a space in a query or form). Bytes that
   decode to invalid UTF-8 (`%FF`) are read as U+FFFD, as invalid UTF-8 anywhere in a request is. */
static int push_decoded(Mito *m, const u8 *s, u64 n, int plus, T *out) {
    u32 start = m->text.len; int high = 0;
    for (u64 i = 0; i < n; i++) {
        u8 c = s[i];
        if (c == '%' && i + 2 < n && hexv(s[i + 1]) >= 0 && hexv(s[i + 2]) >= 0) { c = (u8)(hexv(s[i + 1]) * 16 + hexv(s[i + 2])); i += 2; }
        else if (c == '+' && plus) c = ' ';
        high |= c >= 0x80;
        if (!a_put(&m->text, &c, 1)) return 0;
    }
    if (high) {
        /* a checked copy after the decoded bytes, then moved down over them */
        u32 dn = m->text.len - start;
        if (!a_lossy(&m->text, m->text.p + start, dn)) return 0;
        u32 ln = m->text.len - start - dn;
        memmove(m->text.p + start, m->text.p + start + dn, ln);
        m->text.len = start + ln;
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
/* Is the request's Content-Type a form (application/x-www-form-urlencoded, any case, parameters after it allowed)? */
static int is_form(Mito *m) {
    const u8 *v; u64 n; static const char f[] = "application/x-www-form-urlencoded"; const u64 fn = sizeof f - 1;
    if (!header(m, (const u8 *)"content-type", 12, &v, &n)) return 0;
    while (n && (*v == ' ' || *v == '\t')) { v++; n--; }
    if (n < fn) return 0;
    for (u64 i = 0; i < fn; i++) if (lower(v[i]) != f[i]) return 0;
    return n == fn || v[fn] == ';' || v[fn] == ' ';
}
/* A form body, name=value pairs joined by &, %XX and + decoded: the same fields as a JSON body's, each value text. A name given twice
   keeps its first value (as an input finds the first). Returns as parse_body: -2 the text area is full, -3 too many fields. */
static int parse_form(Mito *m, const u8 *s, u64 n) {
    m->njson = 0;
    for (u64 i = 0; i < n; ) {
        u64 b = i; while (i < n && s[i] != '&') i++;
        u64 e = i; i++;
        if (e == b) continue;
        u64 eq = b; while (eq < e && s[eq] != '=') eq++;
        T k, v;
        if (m->njson == MAX_JSON) return -3;
        if (!push_decoded(m, s + b, eq - b, 1, &k) || !push_decoded(m, s + (eq < e ? eq + 1 : e), eq < e ? e - eq - 1 : 0, 1, &v)) return -2;
        m->json[m->njson].k = k; m->json[m->njson].v = v_str(v); m->njson++;
    }
    return 0;
}

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
static int auth_token(Mito *m, u32 cid, const u8 *v, u64 n, u32 *out);
static int h_auth(Ctx *c, u32 cid, u32 *out) {
    Mito *m = M_(c);
    const u8 *v; u64 n;
    if (!header(m, (const u8 *)"x-session", 9, &v, &n)) return fail(m, 401, "missing X-Session header");
    return auth_token(m, cid, v, n, out);
}
/* The item of pool `cid` a session token names (slot.generation.secret, the secret in hex; shard first in a system), checked live and
   against its `secret`: 0, or -1 with the error (401 malformed or ended, 403 wrong secret). */
static int auth_token(Mito *m, u32 cid, const u8 *v, u64 n, u32 *out) {
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
/* The value of cookie `name` in the request's Cookie header ("a=1; b=2"; a value in double quotes without them); the first if repeated. */
static int cookie(Mito *m, const u8 *name, u64 nn, const u8 **v, u64 *vn) {
    for (u32 h = 0; h < m->nhdrs; h++) {
        const MitoPair *p = &m->hdrs[h];
        if (p->kn != 6) continue;
        int is = 1; for (u32 j = 0; j < 6; j++) if (lower(p->k[j]) != "cookie"[j]) is = 0;
        if (!is) continue;
        const u8 *s = (const u8 *)p->v; u64 n = p->vn;
        for (u64 i = 0; i < n; ) {
            while (i < n && (s[i] == ' ' || s[i] == '\t' || s[i] == ';')) i++;
            u64 b = i; while (i < n && s[i] != ';') i++;
            u64 e = i; while (e > b && (s[e - 1] == ' ' || s[e - 1] == '\t')) e--;
            u64 eq = b; while (eq < e && s[eq] != '=') eq++;
            if (eq == e || eq - b != nn || !b_eq(s + b, nn, name, nn)) continue;
            const u8 *x = s + eq + 1; u64 xn = e - eq - 1;
            if (xn >= 2 && x[0] == '"' && x[xn - 1] == '"') { x++; xn -= 2; }
            *v = x; *vn = xn; return 1;
        }
    }
    return 0;
}
/* `set_cookie(name, value, max_age)`: the value must be cookie octets (printable ASCII but space, " , ; \), at most 4000 bytes */
static int h_set_cookie(Ctx *c, const char *name, u32 nn, const Txt *t, i64 age, int http_only) {
    Mito *m = M_(c); const u8 *p = m->text.p + t->off; u32 n = t->present ? t->len : 0;
    if (n > 4000) return fail(m, 500, "set_cookie: a value is at most 4000 bytes");
    for (u32 i = 0; i < n; i++) if (p[i] <= 0x20 || p[i] >= 0x7F || p[i] == '"' || p[i] == ',' || p[i] == ';' || p[i] == '\\') return fail(m, 500, "set_cookie: a value is printable ASCII without spaces, quotes, commas, semicolons or backslashes");
    if (age < 0) return fail(m, 500, "set_cookie: max age is 0 (delete) or more seconds");
    if (!(a_str(&m->head, "Set-Cookie: ") && a_put(&m->head, name, nn) && a_str(&m->head, "=") && a_put(&m->head, p, n) &&
          a_str(&m->head, "; Path=/; Max-Age=") && a_i64(&m->head, age) && a_str(&m->head, http_only ? "; HttpOnly; SameSite=Lax\r\n" : "; SameSite=Lax\r\n"))) return fail(m, 500, "too many header lines on the answer");
    return 0;
}
/* `set_header(name, value)`: the value is printable ASCII (tabs allowed), at most 8000 bytes, so it can add no header line of its own */
static int h_set_header(Ctx *c, const char *name, u32 nn, const Txt *t) {
    Mito *m = M_(c); const u8 *p = m->text.p + t->off; u32 n = t->present ? t->len : 0;
    if (n > 8000) return fail(m, 500, "set_header: a value is at most 8000 bytes");
    for (u32 i = 0; i < n; i++) if ((p[i] < 0x20 && p[i] != '\t') || p[i] >= 0x7F) return fail(m, 500, "set_header: a value is printable ASCII");
    /* a header set again replaces the earlier line of that name (as a headers hash); Set-Cookie lines are set_cookie's, each kept */
    for (u32 i = 0; i < m->head.len;) {
        const u8 *nl = (const u8 *)memchr(m->head.p + i, '\n', m->head.len - i); u32 e = nl ? (u32)(nl - m->head.p) : m->head.len;
        int same = e - i > nn && m->head.p[i + nn] == ':';
        for (u32 k = 0; same && k < nn; k++) { u8 x = m->head.p[i + k], y = (u8)name[k]; if (x >= 'A' && x <= 'Z') x += 32; if (y >= 'A' && y <= 'Z') y += 32; if (x != y) same = 0; }
        if (same) { u32 next = e < m->head.len ? e + 1 : e; memmove(m->head.p + i, m->head.p + next, m->head.len - next); m->head.len -= next - i; continue; }
        i = e < m->head.len ? e + 1 : e;
    }
    if (!(a_put(&m->head, name, nn) && a_str(&m->head, ": ") && a_put(&m->head, p, n) && a_str(&m->head, "\r\n"))) return fail(m, 500, "too many header lines on the answer");
    return 0;
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
static int h_texists(Ctx *c, u32 t, u32 rowid, u32 *out);
static int h_input(Ctx *c, int src, const char *name, u32 nn, int kind, i64 lo, i64 hi, Val *out) {
    Mito *m = M_(c);
    *out = v_null();
    if (kind == 5) { u32 idx; if (h_auth(c, (u32)lo, &idx)) return -1; *out = v_item((u32)lo, idx); return 0; }
    Val v = v_null(); const u8 *tp = 0; u64 tn = 0; int text = 1;
    if (src == 0) {
        text = 0;
        for (u32 i = 0; i < m->njson; i++) if (b_eq(t_ptr(m, m->json[i].k), m->json[i].k.len, (const u8 *)name, nn)) { v = m->json[i].v; break; }
        if (v.tag == 0) return 0;
        if (m->form) { text = 1; tp = t_ptr(m, (T){ v.a, v.b }); tn = v.b; } /* a form's values are text, read as a query's are */
    } else if (src == 1) {
        const MitoRoute *rt = &mito_prog.routes[m->ei]; int found = 0;
        for (u32 sg = 0; sg < rt->nsegs && !found; sg++) if (rt->segs[sg].param && c_eq(rt->segs[sg].text, (const u8 *)name, nn))
            for (u32 i = 0; i < m->nparams; i++) if (m->params[i].seg == sg) { tp = t_ptr(m, m->params[i].t); tn = m->params[i].t.len; found = 1; break; }
        if (!found) return 0;
    } else if (src == 2) {
        int found = 0;
        for (u32 i = 0; i < m->nquery; i++) if (b_eq(t_ptr(m, m->query[i].k), m->query[i].k.len, (const u8 *)name, nn)) { tp = t_ptr(m, m->query[i].v); tn = m->query[i].v.len; found = 1; break; }
        if (!found) return 0;
    } else if (src == 5) {
        const u8 *cv; u64 cn;
        if (!cookie(m, (const u8 *)name, nn, &cv, &cn)) return 0;
        if (kind == 8) { /* an item named by a session token in a cookie: null when it names none (malformed, ended, wrong secret) */
            u32 idx;
#ifdef MITO_USES_SQLITE
            if ((u32)lo >= MITO_TBASE) { if (t_token_row(m, (u32)lo - MITO_TBASE, cv, cn, &idx)) *out = v_item((u32)lo, idx); return 0; }
#endif
            if (auth_token(m, (u32)lo, cv, cn, &idx)) { m->ekind = E_NONE; m->emsg.len = 0; return 0; }
            *out = v_item((u32)lo, idx); return 0;
        }
        tp = cv; tn = cn;
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
            /* a form's ticked checkbox sends `on` (an unticked one sends nothing: the input's default) */
            if (text && src == 0 && b_eq(tp, tn, (const u8 *)"on", 2)) *out = v_bool(1);
            else if (text) { if (b_eq(tp, tn, (const u8 *)"true", 4)) *out = v_bool(1); else if (b_eq(tp, tn, (const u8 *)"false", 5)) *out = v_bool(0); else return in_bad(m, 400, name, nn, ": expected true or false"); }
            else { if (v.tag != 3) return in_bad(m, 400, name, nn, ": expected true or false"); *out = v; }
            return 0;
        case 4: case 7: {
            T t;
            if (text) { if (t_push(m, tp, tn, &t)) return -1; }
            else { if (v.tag != 4) return in_bad(m, 400, name, nn, ": expected a string"); t.off = v.a; t.len = v.b; }
            if ((i64)t.len > hi) { in_bad(m, 413, name, nn, ": "); a_u64(&m->emsg, t.len); a_str(&m->emsg, " bytes, at most "); a_i64(&m->emsg, hi); return -1; }
            if ((i64)t.len < lo) { in_bad(m, 400, name, nn, ": at least "); a_i64(&m->emsg, lo); a_str(&m->emsg, lo == 1 ? " byte" : " bytes"); return -1; }
            if (kind == 7) { const u8 *p = t_ptr(m, t); for (u32 i = 0; i < t.len; i++) if (p[i] > 127) { in_bad(m, 400, name, nn, ": ascii only, byte "); a_u64(&m->emsg, p[i]); a_str(&m->emsg, " at "); a_u64(&m->emsg, i); return -1; } }
            *out = v_str(t); return 0;
        }
        case 6: {
            /* an id: "slot" (a pool without generations), or "slot.generation" (a checked pool) */
            if (!text) { if (v.tag == 4) { tp = t_ptr(m, (T){ v.a, v.b }); tn = v.b; } else if (v.tag == 1 && v.i >= 0) { char d[24]; int dn = 0; u64 x = (u64)v.i; do { d[dn++] = (char)('0' + x % 10); x /= 10; } while (x); T t; u32 st = m->text.len; while (dn) { char ch = d[--dn]; if (!a_put(&m->text, &ch, 1)) return text_full(m); } t = t_since(m, st); tp = t_ptr(m, t); tn = t.len; } else return in_bad(m, 400, name, nn, ": expected an id"); }
#ifdef MITO_USES_SQLITE
            /* a table's row: its id (row ids are never reused), and the row must be there */
            if ((u32)lo >= MITO_TBASE) {
                u64 id; u32 t = (u32)lo - MITO_TBASE, has = 0;
                if (!parse_u64_radix(tp, tn, 10, &id) || id == 0 || id > 0xFFFFFFFFULL) return in_bad(m, 404, name, nn, ": expected a row id");
                if (h_texists(c, t, (u32)id, &has)) return -1;
                if (!has) { in_bad(m, 404, name, nn, ": no row "); a_put(&m->emsg, tp, tn); a_str(&m->emsg, " in "); a_str(&m->emsg, m->store.tabs[t].sql); return -1; }
                *out = v_item((u32)lo, (u32)id); return 0;
            }
#endif
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
/* Two texts compared where they are: each n bytes at an offset in state (k 0) or in the text area (k 1, a literal). */
static int h_str_cmp_at(Ctx *c, int op, int ak, u64 aoff, u32 an, int bk, u64 boff, u32 bn, int *out) {
    Mito *m = M_(c);
    const u8 *p = ak ? m->text.p + aoff : m->mem.base + aoff, *q = bk ? m->text.p + boff : m->mem.base + boff;
    *out = cmp_res(op, bytes_cmp(p, an, q, bn)); return 0;
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
static int h_wr_int(Ctx *c, u64 off, u32 size, i64 v) { RD_AT(M_(c), off, size); wr(&M_(c)->mem, off, size, (u64)v); return 0; }
/* The same for plain state: ensure(0, plain_end) committed it at start and the live-page marking keeps trim from decommitting it, so the page is
   known committed and only the undo log needs a look. */
static int h_clear_coll(Ctx *c, u32 cid, u32 inst) { RD_ANY(M_(c)); reset_instance(&M_(c)->mem, cid, inst); return 0; }
static int h_save_line(Ctx *c, u64 a, u32 size) { if (M_(c)->reader) { RD_AT(M_(c), a, size); return 0; } save(&M_(c)->mem, a, size); return 0; }
static int h_commit_range(Ctx *c, u64 a, u32 size) { ensure(&M_(c)->mem, a, a + size); return 0; }
/* the text area's length now, and cutting it back to an earlier length: a loop body that keeps none of its strings past the iteration gives their space back */
/* `count` bytes of a state array of bytes at a byte offset, copied into the text area as a string */
static int h_arr_txt(Ctx *c, u64 off, u32 count, Txt *out) { Mito *m = M_(c); T t; if (t_push(m, m->mem.base + off, count, &t)) return -1; out->off = t.off; out->len = t.len; out->present = 1; return 0; }
/* save a range to the undo log before a loop stores into it (save skips the lines already saved) */
static int h_save_range(Ctx *c, u64 a, u64 n) { if (M_(c)->reader) { RD_AT(M_(c), a, n); return 0; } save(&M_(c)->mem, a, n); return 0; }
static int h_text_len(Ctx *c, u32 *len) { *len = M_(c)->text.len; return 0; }
static int h_text_cut(Ctx *c, u32 len) { M_(c)->text.len = len; return 0; }
static int h_wr_plain(Ctx *c, u64 off, u32 size, i64 v) { RD_AT(M_(c), off, size); Mem *m = &M_(c)->mem; save(m, off, size); u64 x = (u64)v; memcpy(m->base + off, &x, size); return 0; }
/* `count` elements of `esize` bytes of an array at byte offset `base`, from position `from`, are set to the value whose bits are `bits`: one commit
   (not for plain state, committed at start), one undo-log pass over the range, one fill (the generated code checked the range and that the value fits). */
static int h_fill_elems(Ctx *c, u64 base, u32 esize, i64 from, i64 count, i64 bits, int plain) { if (count > 0) RD_AT(M_(c), base + (u64)from * esize, (u64)count * esize);
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
static int h_move_elems(Ctx *c, u64 base, u32 esize, i64 from, i64 to, i64 count) { if (count > 0) RD_AT(M_(c), base + (u64)(from < to ? from : to) * esize, (u64)(count + (from < to ? to - from : from - to)) * esize);
    if (count <= 0 || from == to) return 0;
    Mem *m = &M_(c)->mem;
    /* only the destination is written: commit and save just that range (a move between far apart ranges does not pay for what lies between them) */
    u64 lo = base + (u64)to * esize, hi = lo + (u64)count * esize;
    ensure(m, lo, hi); save(m, lo, hi - lo);
    memmove(m->base + base + (u64)to * esize, m->base + base + (u64)from * esize, (u64)count * esize);
    return 0;
}
static int h_store(Ctx *c, u32 cid, u32 idx, u32 ci, const Val *v) { RD_ANY(M_(c)); return store_col(M_(c), cid, idx, ci, -1, v); }
static int h_store_elem(Ctx *c, u32 cid, u32 idx, u32 ci, i64 k, const Val *v) { RD_ANY(M_(c)); return store_col(M_(c), cid, idx, ci, k, v); }
/* A plain value at `off`: code 1..11 a scalar, 20 a string<cap>, 30 a reference to top-level pool `cap`. */
static int h_store_plain(Ctx *c, u32 off, u32 code, u32 cap, const Val *v) { RD_AT(M_(c), off, 4);
    Mito *m = M_(c);
    if (v->tag == 4) { if (code != 20 && code != 21) return bad_type(m); if (code == 21 && not_ascii(m, t_ptr(m, (T){ v->a, v->b }), v->b)) return -1; return store_bytes_at(m, off, off + 4, cap, t_ptr(m, (T){ v->a, v->b }), v->b); }
    if (v->tag != 1 && v->tag != 2 && v->tag != 3) return fail(m, 500, "cannot store this value in state");
    if (code == 20 || code == 21) { err(m, E_BAD, 400); a_str(&m->emsg, "expected a string"); return -1; }
    return store_scalar(m, off, code, v);
}
static int h_alloc(Ctx *c, u32 cid, u32 inst, int try_, u32 *out) { RD_ANY(M_(c));
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
static int h_free(Ctx *c, u32 cid, u32 idx) { RD_ANY(M_(c));
    Mito *m = M_(c);
    if (coll(&m->mem, cid)->list) return fail(m, 500, "list items are not freed; lists drop their oldest or pop their newest");
    return add_free(m, cid, idx);
}
static int h_free_val(Ctx *c, const Val *v) { RD_ANY(M_(c));
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
static int h_store_field(Ctx *c, const Val *base, const char *p, u32 n, const Val *v) { RD_ANY(M_(c));
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
#ifdef MITO_USES_HTML
/* the HTML builtins' plumbing: text written into the text area, a node argument checked, the pool's error as the request's */
static int h_put_text(void *c, const u8 *s, u32 n) { return a_put(&((Mito *)c)->text, s, n) ? 0 : -1; }
static int h_node_arg(Mito *m, const Val *a, u32 n, u32 i, u32 *id) {
    if (i >= n || a[i].tag != 1 || !hp_ok(&m->hp, a[i].i)) return fail(m, 500, "html: not a node of this request");
    *id = (u32)a[i].i; return 0;
}
static int h_hp_fail(Mito *m) { return fail(m, 413, m->hp.err ? m->hp.err : "html: failed"); }
#endif
static int arg_txt(Mito *m, const Val *a, u32 n, u32 i, T *out) { if (i >= n || a[i].tag != 4) return fail(m, 500, "expected a string argument"); out->off = a[i].a; out->len = a[i].b; return 0; }
/* a time's text (`2026-03-02 16:00:00`, `.ffffff` to 9 digits, a `T` for the space and a final `Z` allowed) as microseconds since 1970 */
static int time_parse(const u8 *s, u32 n, i64 *out) {
    if (n < 19) return 0;
    for (u32 i = 0; i < 19; i++) { int dig = i != 4 && i != 7 && i != 10 && i != 13 && i != 16; if (dig ? (s[i] < '0' || s[i] > '9') : 0) return 0; }
    if (s[4] != '-' || s[7] != '-' || (s[10] != ' ' && s[10] != 'T') || s[13] != ':' || s[16] != ':') return 0;
    i64 y = (s[0] - '0') * 1000 + (s[1] - '0') * 100 + (s[2] - '0') * 10 + (s[3] - '0'), mo = (s[5] - '0') * 10 + (s[6] - '0'), d = (s[8] - '0') * 10 + (s[9] - '0');
    i64 h = (s[11] - '0') * 10 + (s[12] - '0'), mi = (s[14] - '0') * 10 + (s[15] - '0'), se = (s[17] - '0') * 10 + (s[18] - '0');
    if (mo < 1 || mo > 12 || d < 1 || d > 31 || h > 23 || mi > 59 || se > 60) return 0;
    u32 i = 19; i64 us = 0;
    if (i < n && s[i] == '.') { i++; i64 scale = 100000; u32 k = 0; while (i < n && s[i] >= '0' && s[i] <= '9') { if (k < 6) { us += (s[i] - '0') * scale; scale /= 10; } k++; i++; } if (!k || k > 9) return 0; }
    if (i < n && s[i] == 'Z') i++;
    if (i != n) return 0;
    /* days from civil (Howard Hinnant's algorithm) */
    i64 yy = mo <= 2 ? y - 1 : y, era = (yy >= 0 ? yy : yy - 399) / 400, yoe = yy - era * 400, mp = mo > 2 ? mo - 3 : mo + 9;
    i64 doy = (153 * mp + 2) / 5 + d - 1, doe = yoe * 365 + yoe / 4 - yoe / 100 + doy, days = era * 146097 + doe - 719468;
    *out = ((days * 86400 + h * 3600 + mi * 60 + se) * 1000000) + us;
    return 1;
}
/* microseconds since 1970 as a time's text, as Rails writes one: `YYYY-MM-DD HH:MM:SS`, then `.ffffff` only when there are microseconds */
static u32 time_text(i64 us, char *o) {
    i64 s = us >= 0 ? us / 1000000 : -((-us + 999999) / 1000000), frac = us - s * 1000000, days = s >= 0 ? s / 86400 : -((-s + 86399) / 86400), r = s - days * 86400;
    i64 z = days + 719468, era = (z >= 0 ? z : z - 146096) / 146097, doe = z - era * 146097;
    i64 yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365, y = yoe + era * 400, doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    i64 mp = (5 * doy + 2) / 153, d = doy - (153 * mp + 2) / 5 + 1, mo = mp < 10 ? mp + 3 : mp - 9; if (mo <= 2) y++;
    u32 n = 0;
    o[n++] = (char)('0' + y / 1000 % 10); o[n++] = (char)('0' + y / 100 % 10); o[n++] = (char)('0' + y / 10 % 10); o[n++] = (char)('0' + y % 10); o[n++] = '-';
    o[n++] = (char)('0' + mo / 10); o[n++] = (char)('0' + mo % 10); o[n++] = '-'; o[n++] = (char)('0' + d / 10); o[n++] = (char)('0' + d % 10); o[n++] = ' ';
    o[n++] = (char)('0' + r / 36000); o[n++] = (char)('0' + r / 3600 % 10); o[n++] = ':'; o[n++] = (char)('0' + r % 3600 / 600); o[n++] = (char)('0' + r % 600 / 60); o[n++] = ':';
    o[n++] = (char)('0' + r % 60 / 10); o[n++] = (char)('0' + r % 10);
    if (frac) { o[n++] = '.'; for (i64 dv = 100000; dv; dv /= 10) o[n++] = (char)('0' + frac / dv % 10); }
    return n;
}
/* A socket operation for runtime/http.c: op (1 send, 2 close, 3 subscribe, 4 unsubscribe, 5 broadcast, 6 send to all, 7 tag), the socket's
   id, an integer, then up to three texts, each its length (4 bytes) and bytes. */
static int sock_op(Mito *m, u8 op, u32 id, i64 i, const T *parts, u32 np) {
    u8 hdr[14]; hdr[0] = op; memcpy(hdr + 1, &id, 4); memcpy(hdr + 5, &i, 8); hdr[13] = (u8)np;
    if (!a_put(&m->ops, hdr, 14)) return fail(m, 507, "too many socket operations in one request");
    for (u32 k = 0; k < np; k++) { u32 n = parts[k].len; if (!a_put(&m->ops, &n, 4) || !a_put(&m->ops, t_ptr(m, parts[k]), n)) return fail(m, 507, "too many socket operations in one request"); }
    return 0;
}
MITO_API const u8 *MITO(socket_ops)(void *mp_, u32 *len) { Mito *m = (Mito *)mp_; *len = m->ops.len; return m->ops.p; }
/* the directory read_stored reads (the server's --files DIR), empty when none */
static char g_files_dir[600];
MITO_API void MITO(set_files)(const char *dir) { u32 k = 0; while (dir[k] && k + 1 < sizeof g_files_dir) { g_files_dir[k] = dir[k]; k++; } g_files_dir[k] = 0; }
/* ---- the answer cache (builtins 61 to 67): pieces of answers kept between requests, shared by every instance (readers too),
   outside the state (not rolled back, not journaled; a reader may fill it). An entry is a key, its parts and the labels of the holes
   between them (the request fills each hole, as with a fresh CSRF token). The program keys entries by what they show, with
   data_version() for the data: it moves whenever a request that changed state or tables commits, so an entry from before a change
   is never found again. Full (or its index half full), the cache starts empty again. */
static int a_html(Area *o, const u8 *s, u64 n, u32 attr);
static Lock g_cache_lk; static u8 *g_cache; static u64 g_cache_cap, g_cache_used; static u32 *g_cache_slots; static u32 g_cache_nslots, g_cache_count;
static volatile u64 g_data_version;
static u64 c_hash(const u8 *p, u32 n) { u64 h = 1469598103934665603ULL; for (u32 i = 0; i < n; i++) { h ^= p[i]; h *= 1099511628211ULL; } return h; }
/* an entry: u32 total, key length, parts, then key, each part's length, each hole label's length, then the parts' and labels' bytes */
static u32 c_rd(const u8 *p) { u32 v; memcpy(&v, p, 4); return v; }
MITO_API int MITO(set_cache)(u64 bytes) {
    if (!bytes) return 1;
    lk_init(&g_cache_lk);
    g_cache_cap = bytes; g_cache = (u8 *)os_alloc(bytes);
    g_cache_nslots = 1; while (g_cache_nslots < bytes / 2048 && g_cache_nslots < (1u << 24)) g_cache_nslots <<= 1; if (g_cache_nslots < 1024) g_cache_nslots = 1024;
    g_cache_slots = (u32 *)os_alloc((u64)g_cache_nslots * 8);
    return g_cache && g_cache_slots;
}
static void c_clear(void) { memset(g_cache_slots, 0, (u64)g_cache_nslots * 8); g_cache_used = 0; g_cache_count = 0; }
static int c_find(const u8 *k, u32 kn, u64 h, u32 *slot) {
    for (u32 i = (u32)h & (g_cache_nslots - 1);; i = (i + 1) & (g_cache_nslots - 1)) {
        u32 off1 = g_cache_slots[2 * i], hh = g_cache_slots[2 * i + 1];
        if (!off1) { *slot = i; return 0; }
        const u8 *e = g_cache + (off1 - 1);
        if (hh == (u32)(h >> 32) && c_rd(e + 4) == kn && b_eq(e + 12, kn, k, kn)) { *slot = i; return 1; }
    }
}
/* cache_get(key): the number of holes of the entry (its parts are one more), now this request's; -1 when there is none */
static int c_get(Mito *m, T key, Val *out) {
    *out = v_int(-1); m->c_entry.len = 0;
    if (!g_cache) return 0;
    const u8 *k = t_ptr(m, key); u64 h = c_hash(k, key.len); u32 slot; int r = 0;
    lk_on(&g_cache_lk);
    if (c_find(k, key.len, h, &slot)) {
        const u8 *e = g_cache + (g_cache_slots[2 * slot] - 1); u32 total = c_rd(e);
        u32 at = m->text.len;
        if (!a_put(&m->text, e, total)) r = text_full(m);
        else { m->c_entry.off = at; m->c_entry.len = total; *out = v_int((i64)c_rd(e + 8) - 1); }
    }
    lk_off(&g_cache_lk);
    return r;
}
/* the entry's part i (written into the answer) and hole i's label */
static int c_part(Mito *m, i64 i, Val *out, int label) {
    *out = v_null();
    if (!m->c_entry.len) return fail(m, 500, "cache_part / cache_label: no entry (cache_get found none)");
    const u8 *e = t_ptr(m, m->c_entry); u32 kn = c_rd(e + 4), np = c_rd(e + 8), nh = np - 1;
    if (i < 0 || (u64)i >= (label ? nh : np)) return fail(m, 500, "cache_part / cache_label: no such part or hole");
    u32 at = 12 + kn + 4 * (np + nh), n = 0;
    for (u32 j = 0; j < np + nh; j++) { u32 len = c_rd(e + 12 + kn + 4 * j); if (j == (label ? np + (u32)i : (u32)i)) { n = len; break; } at += len; }
    if (label) { T t = { m->c_entry.off + at, n }; *out = v_str(t); return 0; }
    return a_put(&m->out, e + at, n) ? 0 : resp_full(m);
}
/* cache_put(key): the answer from cache_start to here, split at its holes, kept under the key */
static int c_put(Mito *m, T key) {
    if (!m->c_on) return fail(m, 500, "cache_put without cache_start");
    m->c_on = 0;
    if (!g_cache) return 0;
    u32 np = m->c_nholes + 1, kn = key.len, lb = 0;
    for (u32 j = 0; j < m->c_nholes; j++) lb += m->c_holes[j].label.len;
    u64 kept = m->out.len - m->c_start; for (u32 j = 0; j < m->c_nholes; j++) kept -= m->c_holes[j].end - m->c_holes[j].at;
    u64 total = 12 + (u64)kn + 4ULL * (np + m->c_nholes) + kept + lb;
    if (total > g_cache_cap / 8) return 0; /* too big to keep */
    const u8 *k = t_ptr(m, key); u64 h = c_hash(k, kn); u32 slot;
    lk_on(&g_cache_lk);
    if (c_find(k, kn, h, &slot)) { lk_off(&g_cache_lk); return 0; } /* another request kept it first */
    if (g_cache_used + total > g_cache_cap || (u64)(g_cache_count + 1) * 2 > g_cache_nslots) { c_clear(); c_find(k, kn, h, &slot); }
    u8 *e = g_cache + g_cache_used; u32 v;
    v = (u32)total; memcpy(e, &v, 4); memcpy(e + 4, &kn, 4); memcpy(e + 8, &np, 4); memcpy(e + 12, k, kn);
    u8 *lens = e + 12 + kn, *d = lens + 4 * (np + m->c_nholes); u32 from = m->c_start;
    for (u32 j = 0; j < np; j++) {
        u32 to = j < m->c_nholes ? m->c_holes[j].at : m->out.len; v = to - from; memcpy(lens + 4 * j, &v, 4);
        memcpy(d, m->out.p + from, v); d += v; from = j < m->c_nholes ? m->c_holes[j].end : to; /* past the hole's value */
    }
    for (u32 j = 0; j < m->c_nholes; j++) { v = m->c_holes[j].label.len; memcpy(lens + 4 * (np + j), &v, 4); memcpy(d, t_ptr(m, m->c_holes[j].label), v); d += v; }
    g_cache_slots[2 * slot] = (u32)(g_cache_used + 1); g_cache_slots[2 * slot + 1] = (u32)(h >> 32);
    g_cache_used += (total + 7) & ~7ULL; g_cache_count++;
    lk_off(&g_cache_lk);
    return 0;
}

/* ---- bytes, encodings and cryptography: builtins 17 to 34 (the primitives in runtime/crypto.c) ----
   Texts here are bytes (any bytes; a utf8 field holds them as they are). Results go in the text area. */
/* room for `need` more bytes at the end of the text area, or 0 (the request fails: 507) */
static u8 *t_room(Mito *m, u64 need) { if ((u64)m->text.len + need > m->text.cap) { text_full(m); return 0; } return m->text.p + m->text.len; }
static int t_done(Mito *m, u32 start, u64 wrote, Val *out) { m->text.len = (u32)(start + wrote); *out = v_str(t_since(m, start)); return 0; }
static int b64_val(u8 ch) { return ch >= 'A' && ch <= 'Z' ? ch - 'A' : ch >= 'a' && ch <= 'z' ? ch - 'a' + 26 : ch >= '0' && ch <= '9' ? ch - '0' + 52 : ch == '+' || ch == '-' ? 62 : ch == '/' || ch == '_' ? 63 : -1; }
static int hex_val(u8 ch) { return ch >= '0' && ch <= '9' ? ch - '0' : ch >= 'a' && ch <= 'f' ? ch - 'a' + 10 : ch >= 'A' && ch <= 'F' ? ch - 'A' + 10 : -1; }
/* ASCII white space (\s), for split and the trims */
static int split_ws(u8 b) { return b == ' ' || (b >= 9 && b <= 13); }
static int bytes_builtin(Mito *m, int code, const Val *a, u32 n, Val *out) {
    T x, y, z; u32 start = m->text.len; u8 *w;
    switch (code) {
    case 17: case 18: { /* base64 (standard, padded), base64_url (URL alphabet, no padding) */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        const u8 *s = t_ptr(m, x); const char *al = code == 17 ? "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/" : "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        if (!(w = t_room(m, ((u64)x.len + 2) / 3 * 4))) return -1;
        u64 o = 0, i = 0;
        for (; i + 3 <= x.len; i += 3) { u32 v = (u32)s[i] << 16 | (u32)s[i + 1] << 8 | s[i + 2]; w[o++] = (u8)al[v >> 18]; w[o++] = (u8)al[v >> 12 & 63]; w[o++] = (u8)al[v >> 6 & 63]; w[o++] = (u8)al[v & 63]; }
        if (x.len - i == 1) { u32 v = (u32)s[i] << 16; w[o++] = (u8)al[v >> 18]; w[o++] = (u8)al[v >> 12 & 63]; if (code == 17) { w[o++] = '='; w[o++] = '='; } }
        else if (x.len - i == 2) { u32 v = (u32)s[i] << 16 | (u32)s[i + 1] << 8; w[o++] = (u8)al[v >> 18]; w[o++] = (u8)al[v >> 12 & 63]; w[o++] = (u8)al[v >> 6 & 63]; if (code == 17) w[o++] = '='; }
        return t_done(m, start, o, out); }
    case 19: { /* unbase64: either alphabet, padding optional; null if it is not base64 */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        const u8 *s = t_ptr(m, x); u32 len = x.len;
        while (len && s[len - 1] == '=' && x.len - len < 2) len--;
        if (len % 4 == 1 || (x.len != len && (len + (x.len - len)) % 4)) { *out = v_null(); return 0; }
        if (!(w = t_room(m, (u64)len / 4 * 3 + 3))) return -1;
        u64 o = 0; u32 acc = 0, bits = 0;
        for (u32 i = 0; i < len; i++) {
            int v = b64_val(s[i]); if (v < 0) { *out = v_null(); return 0; }
            acc = acc << 6 | (u32)v; bits += 6;
            if (bits >= 8) { bits -= 8; w[o++] = (u8)(acc >> bits); }
        }
        if (acc & ((1u << bits) - 1)) { *out = v_null(); return 0; } /* bits left over must be zero (canonical) */
        return t_done(m, start, o, out); }
    case 20: { /* hex, lowercase */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        const u8 *s = t_ptr(m, x);
        if (!(w = t_room(m, (u64)x.len * 2))) return -1;
        for (u32 i = 0; i < x.len; i++) { w[2 * i] = (u8)"0123456789abcdef"[s[i] >> 4]; w[2 * i + 1] = (u8)"0123456789abcdef"[s[i] & 15]; }
        return t_done(m, start, (u64)x.len * 2, out); }
    case 27: { /* xor_bytes: two texts of the same length, byte by byte */
        if (arg_txt(m, a, n, 0, &x) || arg_txt(m, a, n, 1, &y)) return -1;
        if (x.len != y.len) return fail(m, 500, "xor_bytes: the texts differ in length");
        if (!(w = t_room(m, x.len))) return -1;
        const u8 *p = t_ptr(m, x), *q = t_ptr(m, y);
        for (u32 i = 0; i < x.len; i++) w[i] = p[i] ^ q[i];
        return t_done(m, start, x.len, out); }
    case 28: { /* secret_equal: the same bytes, in time that depends only on the lengths */
        if (arg_txt(m, a, n, 0, &x) || arg_txt(m, a, n, 1, &y)) return -1;
        const u8 *p = t_ptr(m, x), *q = t_ptr(m, y); u8 d = x.len != y.len;
        u32 k = x.len < y.len ? x.len : y.len;
        for (u32 i = 0; i < k; i++) d |= p[i] ^ q[i];
        *out = v_int(d == 0); return 0; }
    case 33: { /* form_encode: URI.encode_www_form_component (letters, digits and *-._ as they are, space as +, the rest %XX) */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        const u8 *s = t_ptr(m, x);
        if (!(w = t_room(m, (u64)x.len * 3))) return -1;
        u64 o = 0;
        for (u32 i = 0; i < x.len; i++) {
            u8 ch = s[i];
            if ((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9') || ch == '*' || ch == '-' || ch == '.' || ch == '_') w[o++] = ch;
            else if (ch == ' ') w[o++] = '+';
            else { w[o++] = '%'; w[o++] = (u8)"0123456789ABCDEF"[ch >> 4]; w[o++] = (u8)"0123456789ABCDEF"[ch & 15]; }
        }
        return t_done(m, start, o, out); }
    case 34: { /* form_decode: + as space, %XX as its byte; null if a % is not followed by two hex digits */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        const u8 *s = t_ptr(m, x);
        if (!(w = t_room(m, x.len))) return -1;
        u64 o = 0;
        for (u32 i = 0; i < x.len; i++) {
            if (s[i] == '+') w[o++] = ' ';
            else if (s[i] == '%') {
                int hi = i + 2 < x.len ? hex_val(s[i + 1]) : -1, lo = i + 2 < x.len ? hex_val(s[i + 2]) : -1;
                if (hi < 0 || lo < 0) { *out = v_null(); return 0; }
                w[o++] = (u8)(hi << 4 | lo); i += 2;
            }
            else w[o++] = s[i];
        }
        return t_done(m, start, o, out); }
    case 43: { /* add_years(t, n): a time n calendar years later (or earlier), the same day and time of day; Feb 29 becomes Feb 28 in a
                  year without one (as Ruby's `20.years.from_now`) */
        if (n < 2 || a[0].tag != 1 || a[1].tag != 1 || a[1].i < -9000 || a[1].i > 9000) return fail(m, 500, "add_years(time, years)");
        char tx[40]; u32 tn = time_text(a[0].i, tx); i64 y = ((tx[0] - '0') * 1000 + (tx[1] - '0') * 100 + (tx[2] - '0') * 10 + (tx[3] - '0')) + a[1].i, us;
        if (y < 0 || y > 9999) return fail(m, 500, "add_years: past the year 9999");
        tx[0] = (char)('0' + y / 1000); tx[1] = (char)('0' + y / 100 % 10); tx[2] = (char)('0' + y / 10 % 10); tx[3] = (char)('0' + y % 10);
        int leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        if (tx[5] == '0' && tx[6] == '2' && tx[8] == '2' && tx[9] == '9' && !leap) tx[9] = '8';
        if (!time_parse((const u8 *)tx, tn, &us)) return fail(m, 500, "add_years: not a time");
        *out = v_int(us); return 0; }
    case 36: { /* format_time(t, "%Y-%m-%d"): a time (microseconds since 1970, UTC) written by the pattern: %Y %m %d %H %M %S, %L milliseconds,
                  %N microseconds (6 digits), %s seconds since 1970, %%; other characters as they are */
        if (n < 2 || a[0].tag != 1 || arg_txt(m, a, n, 1, &x)) return fail(m, 500, "format_time(time, \"pattern\")");
        const u8 *f = t_ptr(m, x); char tx[40]; time_text(a[0].i, tx); /* YYYY-MM-DD HH:MM:SS[.ffffff] */
        i64 us = a[0].i, frac = ((us % 1000000) + 1000000) % 1000000, secs = (us - frac) / 1000000;
        if (!(w = t_room(m, (u64)x.len * 20 + 1))) return -1;
        u64 o = 0;
        for (u32 i = 0; i < x.len; i++) {
            if (f[i] != '%' || i + 1 == x.len) { w[o++] = f[i]; continue; }
            u8 d = f[++i]; const char *src = 0; u32 len = 0; char num[24];
            switch (d) {
                case 'Y': src = tx; len = 4; break; case 'm': src = tx + 5; len = 2; break; case 'd': src = tx + 8; len = 2; break;
                case 'H': src = tx + 11; len = 2; break; case 'M': src = tx + 14; len = 2; break; case 'S': src = tx + 17; len = 2; break;
                case 'L': num[0] = (char)('0' + frac / 100000); num[1] = (char)('0' + frac / 10000 % 10); num[2] = (char)('0' + frac / 1000 % 10); src = num; len = 3; break;
                case 'N': for (int k = 0; k < 6; k++) { i64 dv = 100000; for (int j = 0; j < k; j++) dv /= 10; num[k] = (char)('0' + frac / dv % 10); } src = num; len = 6; break;
                case 's': { i64 v = secs; int neg = v < 0; u64 uv = neg ? (u64)0 - (u64)v : (u64)v; char r[24]; int k = 0; do { r[k++] = (char)('0' + uv % 10); uv /= 10; } while (uv); if (neg) r[k++] = '-'; for (int j = 0; j < k; j++) num[j] = r[k - 1 - j]; src = num; len = (u32)k; break; }
                case '%': src = "%"; len = 1; break;
                default: return fail(m, 500, "format_time: the pattern's directives are %Y %m %d %H %M %S %L %N %s and %%");
            }
            memcpy(w + o, src, len); o += len;
        }
        return t_done(m, start, o, out); }
    case 51: case 52: case 53: case 54: case 55: case 56: case 57: { /* sockets: socket_send(id, t), socket_close(id), socket_subscribe(id, stream,
        before, after), socket_unsubscribe(id, before), broadcast(stream, t), socket_send_all(t), socket_tag(id, tag): recorded, applied when
        the request succeeds */
        T parts[3]; u32 np = 0; u32 id = 0; i64 iv = 0; u32 first = 0;
        if (code == 51 || code == 52 || code == 53 || code == 54 || code == 57) { if (n < 1 || a[0].tag != 1 || a[0].i < 0 || a[0].i > 0xFFFFFFFFLL) return fail(m, 500, "a socket is named by its id (an integer)"); id = (u32)a[0].i; first = 1; }
        if (code == 57) { if (n < 2 || a[1].tag != 1) return fail(m, 500, "socket_tag(socket, tag): the tag is an integer"); iv = a[1].i; }
        else for (u32 k = first; k < n && np < 3; k++) { if (arg_txt(m, a, n, k, &parts[np])) return -1; np++; }
        if (sock_op(m, (u8)(code - 50), id, iv, parts, np)) return -1;
        *out = v_null(); return 0; }
    case 49: { /* crc32(t): zlib's CRC-32 (IEEE) of the bytes, as Zlib.crc32 */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        const u8 *s = t_ptr(m, x); u32 c = 0xFFFFFFFFu;
        for (u32 i = 0; i < x.len; i++) { c ^= s[i]; for (int k = 0; k < 8; k++) c = c & 1 ? (c >> 1) ^ 0xEDB88320u : c >> 1; }
        *out = v_int((i64)(c ^ 0xFFFFFFFFu)); return 0; }
    case 50: { /* read_stored("xy/zw/key"): a file's bytes under the server's --files directory, or null when it is not there; the path is
                  relative, its parts letters, digits and . _ - (no `..`), at most the request's text area */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        if (!g_files_dir[0]) return fail(m, 500, "read_stored: the server was started without --files DIR");
        const u8 *s = t_ptr(m, x); int ok = x.len > 0 && x.len < 512 && s[0] != '/';
        for (u32 i = 0; ok && i < x.len; i++) { u8 ch = s[i]; ok = (ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9') || ch == '.' || ch == '_' || ch == '-' || ch == '/'; if (ch == '.' && i + 1 < x.len && s[i + 1] == '.') ok = 0; }
        if (!ok) return fail(m, 500, "read_stored: a relative path of letters, digits and . _ - / (no ..)");
        char path[1200]; u32 pn = 0; for (const char *d = g_files_dir; *d && pn < 600; d++) path[pn++] = *d;
        path[pn++] = '/'; memcpy(path + pn, s, x.len); pn += x.len; path[pn] = 0;
        u64 cap = m->text.cap - m->text.len, got = 0;
        int r = os_read_file(path, m->text.p + m->text.len, cap, &got);
        if (r == 0) { *out = v_null(); return 0; }
        if (r == 2) return text_full(m);
        return t_done(m, start, got, out); }
    case 48: { /* request_target(): the request's path and query as the client sent them (`/rooms/7?x=1`), for the address of the page */
        if (!(w = t_room(m, m->target_n))) return -1;
        memcpy(w, m->target, m->target_n); return t_done(m, start, m->target_n, out); }
    case 46: { /* json_quote(t): t as a JSON string, quotes included, escaped as ActiveSupport::JSON does: \" \\ \b \f \n \r \t, other
                  control characters, U+2028 and U+2029 as \uXXXX, and < > & as \u003c \u003e \u0026 (escape_html_entities_in_json) */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        const u8 *s = t_ptr(m, x);
        if (!(w = t_room(m, (u64)x.len * 6 + 2))) return -1;
        u64 o = 0; w[o++] = '"';
        for (u32 i = 0; i < x.len; i++) {
            u8 ch = s[i]; const char *e = 0; char u[7];
            if (ch == '"') e = "\\\""; else if (ch == '\\') e = "\\\\"; else if (ch == '\b') e = "\\b"; else if (ch == '\f') e = "\\f";
            else if (ch == '\n') e = "\\n"; else if (ch == '\r') e = "\\r"; else if (ch == '\t') e = "\\t";
            else if (ch < 0x20 || ch == '<' || ch == '>' || ch == '&') { u[0] = '\\'; u[1] = 'u'; u[2] = '0'; u[3] = '0'; u[4] = "0123456789abcdef"[ch >> 4]; u[5] = "0123456789abcdef"[ch & 15]; u[6] = 0; e = u; }
            else if (ch == 0xE2 && i + 2 < x.len && s[i + 1] == 0x80 && (s[i + 2] == 0xA8 || s[i + 2] == 0xA9)) { e = s[i + 2] == 0xA8 ? "\\u2028" : "\\u2029"; i += 2; }
            if (e) { while (*e) w[o++] = (u8)*e++; } else w[o++] = ch;
        }
        w[o++] = '"';
        return t_done(m, start, o, out); }
    case 47: { /* json_unquote(t): the characters of a JSON string (its quotes optional), escapes decoded (\uXXXX to UTF-8, surrogate pairs
                  joined); null when an escape is not one */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        const u8 *s = t_ptr(m, x); u32 b = 0, e = x.len;
        if (e >= 2 && s[0] == '"' && s[e - 1] == '"') { b = 1; e--; }
        if (!(w = t_room(m, (u64)(e - b) * 2))) return -1;
        u64 o = 0;
        for (u32 i = b; i < e; i++) {
            if (s[i] != '\\') { w[o++] = s[i]; continue; }
            if (++i >= e) { *out = v_null(); return 0; }
            switch (s[i]) {
                case '"': w[o++] = '"'; break; case '\\': w[o++] = '\\'; break; case '/': w[o++] = '/'; break; case 'b': w[o++] = '\b'; break;
                case 'f': w[o++] = '\f'; break; case 'n': w[o++] = '\n'; break; case 'r': w[o++] = '\r'; break; case 't': w[o++] = '\t'; break;
                case 'u': {
                    u32 cp = 0;
                    if (i + 4 >= e) { *out = v_null(); return 0; } /* four hex digits must follow */
                    for (int k = 1; k <= 4; k++) { int h = hex_val(s[i + k]); if (h < 0) { *out = v_null(); return 0; } cp = cp << 4 | (u32)h; }
                    i += 4;
                    if (cp >= 0xD800 && cp < 0xDC00 && i + 6 < e && s[i + 1] == '\\' && s[i + 2] == 'u') {
                        u32 lo = 0; int ok = 1; for (int k = 3; k <= 6; k++) { int h = hex_val(s[i + k]); if (h < 0) ok = 0; else lo = lo << 4 | (u32)h; }
                        if (ok && lo >= 0xDC00 && lo < 0xE000) { cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00); i += 6; }
                    }
                    if (cp >= 0xD800 && cp < 0xE000) cp = 0xFFFD;
                    if (cp < 0x80) w[o++] = (u8)cp;
                    else if (cp < 0x800) { w[o++] = (u8)(0xC0 | cp >> 6); w[o++] = (u8)(0x80 | (cp & 63)); }
                    else if (cp < 0x10000) { w[o++] = (u8)(0xE0 | cp >> 12); w[o++] = (u8)(0x80 | (cp >> 6 & 63)); w[o++] = (u8)(0x80 | (cp & 63)); }
                    else { w[o++] = (u8)(0xF0 | cp >> 18); w[o++] = (u8)(0x80 | (cp >> 12 & 63)); w[o++] = (u8)(0x80 | (cp >> 6 & 63)); w[o++] = (u8)(0x80 | (cp & 63)); }
                    break; }
                default: *out = v_null(); return 0;
            }
        }
        return t_done(m, start, o, out); }
    case 44: case 45: { /* index_of(t, part), last_index_of(t, part): the byte offset of the first (last) place part is in t, or -1 */
        if (arg_txt(m, a, n, 0, &x) || arg_txt(m, a, n, 1, &y)) return -1;
        const u8 *p = t_ptr(m, x), *q = t_ptr(m, y); i64 at = -1;
        if (y.len <= x.len) {
            if (code == 44) { for (u32 i = 0; i + y.len <= x.len; i++) if (b_eq(p + i, y.len, q, y.len)) { at = i; break; } }
            else { for (u32 i = x.len - y.len + 1; i-- > 0;) if (b_eq(p + i, y.len, q, y.len)) { at = i; break; } }
        }
        *out = v_int(at); return 0; }
    case 37: case 38: case 39: { /* contains(t, part), starts_with(t, part), ends_with(t, part): bytes compared */
        if (arg_txt(m, a, n, 0, &x) || arg_txt(m, a, n, 1, &y)) return -1;
        const u8 *p = t_ptr(m, x), *q = t_ptr(m, y); int found = 0;
        if (y.len <= x.len) {
            if (code == 38) found = b_eq(p, y.len, q, y.len);
            else if (code == 39) found = b_eq(p + x.len - y.len, y.len, q, y.len);
            else for (u32 i = 0; i + y.len <= x.len && !found; i++) found = b_eq(p + i, y.len, q, y.len);
        }
        *out = v_int(found); return 0; }
    case 61: /* cache_get(key) */ if (arg_txt(m, a, n, 0, &x)) return -1; return c_get(m, x, out);
    case 62: /* cache_put(key) */ if (arg_txt(m, a, n, 0, &x)) return -1; *out = v_null(); return c_put(m, x);
    case 63: /* cache_start(): what is written from here is what cache_put keeps */ m->c_on = 1; m->c_start = m->out.len; m->c_nholes = 0; *out = v_null(); return 0;
    case 64: /* cache_hole(label, value): the value written here (escaped, as `{value}` writes it), and kept out of the entry: a hole a
               later request fills (its label says with what) */
        if (arg_txt(m, a, n, 0, &x) || arg_txt(m, a, n, 1, &y)) return -1;
        if (!m->c_on) return fail(m, 500, "cache_hole without cache_start");
        if (m->c_nholes == 64) return fail(m, 500, "an answer cached with more than 64 holes");
        m->c_holes[m->c_nholes].at = m->out.len;
        if (!a_html(&m->out, t_ptr(m, y), y.len, 0)) return resp_full(m);
        m->c_holes[m->c_nholes].end = m->out.len; m->c_holes[m->c_nholes].label = x; m->c_nholes++; *out = v_null(); return 0;
    case 65: /* cache_part(i) */ if (n < 1 || a[0].tag != 1) return fail(m, 500, "cache_part(i)"); return c_part(m, a[0].i, out, 0);
    case 66: /* cache_label(i) */ if (n < 1 || a[0].tag != 1) return fail(m, 500, "cache_label(i)"); return c_part(m, a[0].i, out, 1);
    case 67: /* data_version() */ *out = v_int((i64)g_data_version); return 0;
    case 68: { /* replace(t, a, b): every a in t (from the left, not overlapping) as b */
        T z2;
        if (arg_txt(m, a, n, 0, &x) || arg_txt(m, a, n, 1, &y) || arg_txt(m, a, n, 2, &z2)) return -1;
        if (!y.len) return fail(m, 500, "replace: the text to replace is empty");
        u64 most = z2.len > y.len ? (u64)x.len / y.len * z2.len + x.len : x.len;
        if (!(w = t_room(m, most))) return -1;
        const u8 *s = t_ptr(m, x), *f = t_ptr(m, y), *r = t_ptr(m, z2); u64 o = 0;
        for (u32 i = 0; i < x.len;) {
            if (i + y.len <= x.len && b_eq(s + i, y.len, f, y.len)) { for (u32 j = 0; j < z2.len; j++) w[o++] = r[j]; i += y.len; }
            else w[o++] = s[i++];
        }
        return t_done(m, start, o, out); }
    case 69: case 70: { /* trim_start(t), trim_end(t): t without the white space (as trim's) at its start (end): a window onto t */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        const u8 *s = t_ptr(m, x); u32 b0 = 0, e0 = x.len;
        if (code == 69) while (b0 < e0 && split_ws(s[b0])) b0++; else while (e0 > b0 && split_ws(s[e0 - 1])) e0--;
        T r = { x.off + b0, e0 - b0 }; *out = v_str(r); return 0; }
#ifdef MITO_USES_HTML
    /* HTML fragments (runtime/html.c): 72 to 97. A node is an integer, valid in the request that made it; -1 is none. */
    case 72: case 73: { /* html_parse(t), html_parse_in(t, node): a fragment parsed in a body, or in the node's context */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        u32 ctx = HP_NONE, f;
        if (code == 73 && h_node_arg(m, a, n, 1, &ctx)) return -1;
        if (hp_parse(&m->hp, t_ptr(m, x), x.len, ctx, &f)) return h_hp_fail(m);
        *out = v_int(f == HP_NONE ? -1 : (i64)f); return 0; }
    case 74: { u32 id; if (h_node_arg(m, a, n, 0, &id)) return -1; *out = v_int(m->hp.n[id].kind); return 0; } /* node_kind */
    case 75: { /* node_name: an element's name; otherwise libxml2's (text, comment, #cdata-section, #document-fragment) */
        u32 id; if (h_node_arg(m, a, n, 0, &id)) return -1;
        HNode *y = &m->hp.n[id]; u32 start = m->text.len;
        if (y->kind == 1) { if (!a_put(&m->text, m->hp.b + y->name, y->name_n)) return text_full(m); }
        else if (!a_str(&m->text, y->kind == 3 ? "text" : y->kind == 8 ? "comment" : y->kind == 4 ? "#cdata-section" : "#document-fragment")) return text_full(m);
        *out = v_str(t_since(m, start)); return 0; }
    case 76: case 77: case 78: case 79: case 80: { /* node_parent, node_first, node_last, node_next, node_prev */
        u32 id; if (h_node_arg(m, a, n, 0, &id)) return -1;
        HNode *y = &m->hp.n[id];
        u32 r = code == 76 ? y->parent : code == 77 ? y->first : code == 78 ? y->last : code == 79 ? y->next : y->prev;
        *out = v_int(r == HP_NONE ? -1 : (i64)r); return 0; }
    case 81: { /* node_content: a text's (comment's) text, an element's text throughout */
        u32 id; if (h_node_arg(m, a, n, 0, &id)) return -1;
        u32 start = m->text.len;
        if (hp_content(&m->hp, id, h_put_text, m)) { m->text.len = start; return text_full(m); }
        *out = v_str(t_since(m, start)); return 0; }
    case 82: { /* node_attr(n, name): the value, or null */
        u32 id; if (h_node_arg(m, a, n, 0, &id) || arg_txt(m, a, n, 1, &x)) return -1;
        i64 k = m->hp.n[id].kind == 1 ? hp_attr_find(&m->hp, id, t_ptr(m, x), x.len) : -1;
        if (k < 0) { *out = v_null(); return 0; }
        HAttr *at = &m->hp.a[m->hp.n[id].attrs + k]; u32 start = m->text.len;
        if (!a_put(&m->text, m->hp.b + at->value, at->value_n)) return text_full(m);
        *out = v_str(t_since(m, start)); return 0; }
    case 83: { u32 id; if (h_node_arg(m, a, n, 0, &id)) return -1; *out = v_int(m->hp.n[id].kind == 1 ? m->hp.n[id].nattrs : 0); return 0; } /* node_attr_count */
    case 84: case 85: { /* node_attr_name(n, i) (as written: xlink:href), node_attr_value(n, i) */
        u32 id; if (h_node_arg(m, a, n, 0, &id)) return -1;
        if (n < 2 || a[1].tag != 1 || a[1].i < 0 || a[1].i >= (i64)m->hp.n[id].nattrs) return fail(m, 500, "html: no such attribute");
        HAttr *at = &m->hp.a[m->hp.n[id].attrs + a[1].i]; u32 start = m->text.len;
        if (code == 84) {
            const char *pre; u32 pn = hp_attr_prefix(at->ns, &pre);
            if (at->ns == 3 && at->name_n == 5 && memcmp(m->hp.b + at->name, "xmlns", 5) == 0) pn = 0;
            if (!a_put(&m->text, pre, pn) || !a_put(&m->text, m->hp.b + at->name, at->name_n)) return text_full(m);
        } else if (!a_put(&m->text, m->hp.b + at->value, at->value_n)) return text_full(m);
        *out = v_str(t_since(m, start)); return 0; }
    case 86: { /* node_set_attr(n, name, value) */
        u32 id; T v; if (h_node_arg(m, a, n, 0, &id) || arg_txt(m, a, n, 1, &x) || arg_txt(m, a, n, 2, &v)) return -1;
        if (m->hp.n[id].kind != 1) return fail(m, 500, "html: only an element has attributes");
        if (hp_set_attr(&m->hp, id, t_ptr(m, x), x.len, t_ptr(m, v), v.len)) return h_hp_fail(m);
        *out = v_null(); return 0; }
    case 87: { /* node_remove_attr(n, name) */
        u32 id; if (h_node_arg(m, a, n, 0, &id) || arg_txt(m, a, n, 1, &x)) return -1;
        if (m->hp.n[id].kind == 1) hp_remove_attr(&m->hp, id, t_ptr(m, x), x.len);
        *out = v_null(); return 0; }
    case 88: case 89: { /* node_new_element(name) (an HTML element), node_new_text(t): new and detached */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        u32 id;
        if ((code == 88 ? hp_new_element : hp_new_text)(&m->hp, t_ptr(m, x), x.len, &id)) return h_hp_fail(m);
        *out = v_int(id); return 0; }
    case 90: case 91: { /* node_append(parent, child), node_insert_before(ref, node): the node moved there */
        u32 p0, c0; if (h_node_arg(m, a, n, 0, &p0) || h_node_arg(m, a, n, 1, &c0)) return -1;
        if (code == 91) { u32 par = m->hp.n[p0].parent; if (par == HP_NONE) return fail(m, 500, "html: node_insert_before: the node has no parent"); if (hp_within(&m->hp, c0, par)) return fail(m, 500, "html: a node cannot go inside itself"); hp_insert_before(&m->hp, p0, c0); }
        else { if (hp_within(&m->hp, c0, p0)) return fail(m, 500, "html: a node cannot go inside itself"); if (m->hp.n[p0].kind != 1 && m->hp.n[p0].kind != 11) return fail(m, 500, "html: only an element or a fragment has children"); hp_append(&m->hp, p0, c0); }
        *out = v_null(); return 0; }
    case 92: { u32 id; if (h_node_arg(m, a, n, 0, &id)) return -1; hp_detach(&m->hp, id); *out = v_null(); return 0; } /* node_detach */
    case 93: { u32 id, c; if (h_node_arg(m, a, n, 0, &id)) return -1; if (hp_clone(&m->hp, id, &c)) return h_hp_fail(m); *out = v_int(c); return 0; } /* node_clone */
    case 94: case 95: case 97: { /* node_html (outer: a fragment its children), node_inner_html, node_html_escaped (< > escaped in attributes too) */
        u32 id; if (h_node_arg(m, a, n, 0, &id)) return -1;
        u32 start = m->text.len; int r = 0;
        if (code == 95) { for (u32 ch = m->hp.n[id].first; ch != HP_NONE && !r; ch = m->hp.n[ch].next) r = hp_write(&m->hp, ch, 0, h_put_text, m); }
        else r = hp_write(&m->hp, id, code == 97, h_put_text, m);
        if (r) { m->text.len = start; return text_full(m); }
        *out = v_str(t_since(m, start)); return 0; }
    case 96: { /* node_set_text(n, t): a text's (comment's, CDATA's) text */
        u32 id; if (h_node_arg(m, a, n, 0, &id) || arg_txt(m, a, n, 1, &x)) return -1;
        HNode *y = &m->hp.n[id];
        if (y->kind != 3 && y->kind != 4 && y->kind != 8) return fail(m, 500, "html: node_set_text takes a text node");
        if (hp_bytes(&m->hp, t_ptr(m, x), x.len, &y->text)) return h_hp_fail(m);
        y->text_n = x.len; *out = v_null(); return 0; }
#endif
    case 98: { /* chr(cp): the code point's UTF-8 (0 to U+10FFFF, no surrogates) */
        if (n < 1 || a[0].tag != 1) return fail(m, 500, "chr(code point)");
        i64 c = a[0].i; u8 b[4]; u32 k;
        if (c < 0 || c > 0x10FFFF || (c >= 0xD800 && c <= 0xDFFF)) return fail(m, 500, "chr: not a Unicode scalar value");
        if (c < 0x80) { b[0] = (u8)c; k = 1; }
        else if (c < 0x800) { b[0] = (u8)(0xC0 | (c >> 6)); b[1] = (u8)(0x80 | (c & 63)); k = 2; }
        else if (c < 0x10000) { b[0] = (u8)(0xE0 | (c >> 12)); b[1] = (u8)(0x80 | ((c >> 6) & 63)); b[2] = (u8)(0x80 | (c & 63)); k = 3; }
        else { b[0] = (u8)(0xF0 | (c >> 18)); b[1] = (u8)(0x80 | ((c >> 12) & 63)); b[2] = (u8)(0x80 | ((c >> 6) & 63)); b[3] = (u8)(0x80 | (c & 63)); k = 4; }
        if (!(w = t_room(m, k))) return -1;
        memcpy(w, b, k); return t_done(m, start, k, out); }
    case 71: { /* all_emoji(t): t is one or more characters each Emoji_Presentation, Extended_Pictographic or U+FE0F (Ruby's
                  /\A(\p{Emoji_Presentation}|\p{Extended_Pictographic}|\uFE0F)+\z/u; the ranges are Ruby 3.4's, read from it) */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        *out = v_bool(all_of(t_ptr(m, x), x.len, 0)); return 0; }
    case 99: { /* all_word(t): t is one or more characters each \p{Word} (Ruby 3.4's: letters, marks, numbers, connector punctuation) */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        *out = v_bool(all_of(t_ptr(m, x), x.len, 1)); return 0; }
    case 58: { /* byte_at(t, i): the byte at offset i (0 to 255), or -1 past either end */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        if (n < 2 || a[1].tag != 1) return fail(m, 500, "byte_at(text, offset)");
        *out = v_int(a[1].i >= 0 && a[1].i < (i64)x.len ? t_ptr(m, x)[a[1].i] : -1); return 0; }
    case 59: case 60: { /* lower(t), upper(t): Unicode's full case mappings (runtime/unicase.c), as Ruby's downcase and upcase: a
                           character may become several ('ß' upper is "SS"; a text grows at most three times) */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        if (!(w = t_room(m, (u64)x.len * 3))) return -1;
        const u8 *s = t_ptr(m, x); u64 o = 0;
#ifdef MITO_UNICASE
        const u32 *cps = code == 59 ? uc_lower_cp : uc_upper_cp, *offs = code == 59 ? uc_lower_off : uc_upper_off; const u8 *by = code == 59 ? uc_lower_bytes : uc_upper_bytes;
        u32 nc = code == 59 ? UC_LOWER_N : UC_UPPER_N;
#endif
        for (u32 i = 0; i < x.len;) {
            u8 b = s[i];
            if (b < 0x80) { if (code == 59 && b >= 'A' && b <= 'Z') b += 32; else if (code == 60 && b >= 'a' && b <= 'z') b -= 32; w[o++] = b; i++; continue; }
            u32 k = b >= 0xF0 ? 4 : b >= 0xE0 ? 3 : b >= 0xC0 ? 2 : 1; if (i + k > x.len) k = x.len - i;
            u32 cp = k == 2 ? (u32)(b & 0x1F) << 6 | (s[i + 1] & 0x3F) : k == 3 ? (u32)(b & 0x0F) << 12 | (u32)(s[i + 1] & 0x3F) << 6 | (s[i + 2] & 0x3F)
                   : k == 4 ? (u32)(b & 0x07) << 18 | (u32)(s[i + 1] & 0x3F) << 12 | (u32)(s[i + 2] & 0x3F) << 6 | (s[i + 3] & 0x3F) : b;
            int found = 0;
#ifdef MITO_UNICASE
            u32 lo = 0, hi = nc;
            while (lo < hi) { u32 mid = (lo + hi) / 2; if (cps[mid] < cp) lo = mid + 1; else hi = mid; }
            if (lo < nc && cps[lo] == cp) { for (u32 j = offs[lo]; j < offs[lo + 1]; j++) w[o++] = by[j]; found = 1; }
#endif
            if (!found) for (u32 j = 0; j < k; j++) w[o++] = s[i + j];
            i += k;
        }
        return t_done(m, start, o, out); }
    case 41: { /* escape_html(t): & < > " ' as entities (as ERB's <%= %> writes text), for markup or JSON written as text */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        const u8 *s = t_ptr(m, x);
        if (!(w = t_room(m, (u64)x.len * 6))) return -1;
        u64 o = 0;
        for (u32 i = 0; i < x.len; i++) {
            const char *e = s[i] == '&' ? "&amp;" : s[i] == '<' ? "&lt;" : s[i] == '>' ? "&gt;" : s[i] == '"' ? "&quot;" : s[i] == '\'' ? "&#39;" : 0;
            if (e) { while (*e) w[o++] = (u8)*e++; } else w[o++] = s[i];
        }
        return t_done(m, start, o, out); }
    case 40: /* no_default_headers(): the answer goes without the web block's header lines (as Rails' send_file answers) */
        m->bare = 1; *out = v_null(); return 0;
    case 35: { /* env("NAME"): the process's environment variable, or null when it is not set */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        char name[256]; if (x.len == 0 || x.len > 255) return fail(m, 500, "env: a name of 1 to 255 bytes");
        memcpy(name, t_ptr(m, x), x.len); name[x.len] = 0;
        const char *v = getenv(name);
        if (!v) { *out = v_null(); return 0; }
        u64 vn = 0; while (v[vn]) vn++;
        if (!(w = t_room(m, vn))) return -1;
        memcpy(w, v, vn); return t_done(m, start, vn, out); }
#ifdef MITO_USES_CRYPTO
    case 21: case 22: { /* sha1, sha256: the digest's bytes */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        u8 d[32]; u32 dn = code == 21 ? 20 : 32;
        if (!cr_sha(code == 21 ? 1 : 256, t_ptr(m, x), x.len, d)) return fail(m, 500, "the system's hash failed");
        if (!(w = t_room(m, dn))) return -1;
        memcpy(w, d, dn); return t_done(m, start, dn, out); }
    case 23: case 24: { /* hmac_sha1(key, text), hmac_sha256(key, text): the MAC's bytes */
        if (arg_txt(m, a, n, 0, &x) || arg_txt(m, a, n, 1, &y)) return -1;
        u8 d[32]; u32 dn = code == 23 ? 20 : 32;
        if (!cr_hmac(code == 23 ? 1 : 256, t_ptr(m, x), x.len, t_ptr(m, y), y.len, d)) return fail(m, 500, "the system's HMAC failed");
        if (!(w = t_room(m, dn))) return -1;
        memcpy(w, d, dn); return t_done(m, start, dn, out); }
    case 25: { /* pbkdf2_sha256(password, salt, iterations, length) */
        if (arg_txt(m, a, n, 0, &x) || arg_txt(m, a, n, 1, &y)) return -1;
        if (n < 4 || a[2].tag != 1 || a[3].tag != 1 || a[2].i < 1 || a[2].i > 10000000 || a[3].i < 1 || a[3].i > 1024) return fail(m, 500, "pbkdf2_sha256: 1 to 10000000 iterations, a length of 1 to 1024 bytes");
        if (!(w = t_room(m, (u64)a[3].i))) return -1;
        if (!cr_pbkdf2_sha256(t_ptr(m, x), x.len, t_ptr(m, y), y.len, (u32)a[2].i, w, (u32)a[3].i)) return fail(m, 500, "the system's PBKDF2 failed");
        return t_done(m, start, (u64)a[3].i, out); }
    case 26: { /* random_bytes(n): from the operating system's generator, never the program's seeded one */
        if (n < 1 || a[0].tag != 1 || a[0].i < 1 || a[0].i > 65536) return fail(m, 500, "random_bytes: 1 to 65536 bytes");
        if (!(w = t_room(m, (u64)a[0].i))) return -1;
        if (!cr_random(w, (u32)a[0].i)) return fail(m, 500, "the system's random generator failed");
        return t_done(m, start, (u64)a[0].i, out); }
    case 29: case 30: { /* aes256gcm_seal(key, nonce, text): ciphertext and 16-byte tag; aes256gcm_open(key, nonce, sealed): the text, or null */
        if (arg_txt(m, a, n, 0, &x) || arg_txt(m, a, n, 1, &y) || arg_txt(m, a, n, 2, &z)) return -1;
        if (x.len != 32 || y.len != 12) return fail(m, 500, "aes256gcm: a 32-byte key and a 12-byte nonce");
        if (code == 29) {
            if (!(w = t_room(m, (u64)z.len + 16))) return -1;
            if (!cr_gcm(0, t_ptr(m, x), t_ptr(m, y), t_ptr(m, z), z.len, w, w + z.len)) return fail(m, 500, "the system's AES-GCM failed");
            return t_done(m, start, (u64)z.len + 16, out);
        }
        if (z.len < 16) { *out = v_null(); return 0; }
        u8 tag[16]; memcpy(tag, t_ptr(m, z) + z.len - 16, 16);
        if (!(w = t_room(m, z.len - 16))) return -1;
        if (!cr_gcm(1, t_ptr(m, x), t_ptr(m, y), t_ptr(m, z), z.len - 16, w, tag)) { *out = v_null(); return 0; }
        return t_done(m, start, z.len - 16, out); }
    case 42: { /* random_text(n, alphabet): n characters each drawn uniformly from the alphabet (ASCII, 2 to 255 of them) by the system's
                  generator (bytes past the largest multiple of its size are drawn again), as SecureRandom.base58 and alphanumeric do */
        if (n < 2 || a[0].tag != 1 || a[0].i < 1 || a[0].i > 4096 || arg_txt(m, a, n, 1, &x) || x.len < 2 || x.len > 255) return fail(m, 500, "random_text(1 to 4096 characters, an alphabet of 2 to 255)");
        u32 cnt = (u32)a[0].i, lim = 256 - 256 % x.len;
        if (!(w = t_room(m, cnt))) return -1;
        const u8 *al = t_ptr(m, x); u8 buf[64]; u32 have = 0, used = 0, o = 0;
        while (o < cnt) {
            if (used == have) { if (!cr_random(buf, sizeof buf)) return fail(m, 500, "the system's random generator failed"); have = sizeof buf; used = 0; }
            u8 r = buf[used++];
            if (r < lim) w[o++] = al[r % x.len];
        }
        return t_done(m, start, cnt, out); }
    case 31: case 32: { /* bcrypt_hash(password, cost): a `$2a$` digest; bcrypt_check(password, digest): whether it matches */
        if (arg_txt(m, a, n, 0, &x)) return -1;
        char pw[73]; u32 pn = 0; const u8 *p = t_ptr(m, x);
        while (pn < x.len && pn < 72 && p[pn]) { pw[pn] = (char)p[pn]; pn++; } /* as bcrypt-ruby: the first 72 bytes, up to a zero byte */
        pw[pn] = 0;
        char dg[61];
        if (code == 31) {
            if (n < 2 || a[1].tag != 1 || a[1].i < 4 || a[1].i > 31) return fail(m, 500, "bcrypt_hash: a cost of 4 to 31");
            u8 r[16]; char setting[30];
            if (!cr_random(r, 16) || !mito_bcrypt_setting(r, (int)a[1].i, setting) || !mito_bcrypt(pw, setting, dg)) return fail(m, 500, "bcrypt failed");
            if (!(w = t_room(m, 60))) return -1;
            memcpy(w, dg, 60); return t_done(m, start, 60, out);
        }
        if (arg_txt(m, a, n, 1, &y)) return -1;
        char setting[61]; const u8 *d = t_ptr(m, y);
        if (y.len != 60 || d[0] != '$' || d[1] != '2') { *out = v_int(0); return 0; }
        memcpy(setting, d, 60); setting[60] = 0;
        if (!mito_bcrypt(pw, setting, dg)) { *out = v_int(0); return 0; }
        u8 diff = 0; for (int i = 0; i < 60; i++) diff |= (u8)dg[i] ^ d[i];
        *out = v_int(diff == 0); return 0; }
#endif
    }
    return fail(m, 500, "unknown builtin code");
}
static int h_builtin(Ctx *c, int code, const Val *a, u32 n, Val *out) {
    Mito *m = M_(c); T k;
    Val a0 = n > 0 ? a[0] : v_null();
    if (code >= 17) return bytes_builtin(m, code, a, n, out);
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
#ifdef MITO_USES_SQLITE
            if (a0.a >= MITO_TBASE) { /* a row: rowid.secret (row ids are never reused, so no generation) */
                u32 t = a0.a - MITO_TBASE; Val x; int sc = m->store.ts[t].secret;
                if (sc < 0) return fail(m, 500, "token needs a `secret` field");
                if (t_get(m, t, a0.b, (u32)sc, &x)) return -1;
                u32 start = m->text.len;
                if (!(a_u64(&m->text, a0.b) && a_put(&m->text, ".", 1) && a_hex(&m->text, (u64)x.i))) return text_full(m);
                *out = v_str(t_since(m, start)); return 0;
            }
#endif
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
static int h_str_app(Ctx *c, int kind, u64 a, u64 b, u32 cc, const Val *v) { if (kind & 1) RD_ANY(M_(c)); else RD_AT(M_(c), a, 4 + b);
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
    if ((kind & 4) && not_ascii(m, p, n)) { m->text.len = mark; return -1; }
    if (n) put(&m->mem, bytes_at + cur, p, n);
    if (n || (kind & 2)) wr(&m->mem, len_at, 4, cur + n);
    m->text.len = mark;
    return 0;
}

/* Append `n` bytes of state at `src` (a window onto a text field) to a string field in place, as h_str_app (kind, a, b, cc the
   same): one copy from field to field. Overlapping bytes go through the text area first. */
static int h_str_app_at(Ctx *c, int kind, u64 a, u64 b, u32 cc, u64 src, u32 n) { if (kind & 1) RD_ANY(M_(c)); else RD_AT(M_(c), a, 4 + b);
    Mito *m = M_(c);
    u64 len_at, bytes_at, cap;
    if (kind & 1) { const MitoColl *k = coll(&m->mem, (u32)a); len_at = caddr(m, (u32)a, b, cc); bytes_at = caddr(m, (u32)a, b, cc + 1); cap = k->cols[cc + 1].n; }
    else { len_at = a; bytes_at = a + 4; cap = b; }
    u64 cur = (kind & 2) ? 0 : rd(&m->mem, len_at, 4);
    if (cur + n > cap) { err(m, E_TOOLONG, 413); a_str(&m->emsg, "string of "); a_u64(&m->emsg, cur + n); a_str(&m->emsg, " bytes does not fit string<"); a_u64(&m->emsg, cap); a_str(&m->emsg, ">"); return -1; }
    if ((kind & 4) && not_ascii(m, m->mem.base + src, n)) return -1;
    u64 to = bytes_at + cur;
    if (n && src < to + n && to < src + n) {
        u32 mark = m->text.len; T t;
        if (t_push(m, m->mem.base + src, n, &t)) return -1;
        put(&m->mem, to, m->text.p + t.off, n);
        m->text.len = mark;
    } else if (n) put(&m->mem, to, m->mem.base + src, n);
    if (n || (kind & 2)) wr(&m->mem, len_at, 4, cur + n);
    return 0;
}

/* Write pieces (text, integers, floats, bools, formatted as `+` would) to a string field in one call: appended after what it holds,
   or from the start when `clear` (an assignment). kind, a, b, cc as h_str_app. The length is read and written once, the destination
   saved piece by piece; integers are formatted straight into the field. */
static int h_str_set(Ctx *c, int kind, u64 a, u64 b, u32 cc, int clear, const Val *v, u32 nv) { if (kind & 1) RD_ANY(M_(c)); else RD_AT(M_(c), a, 4 + b);
    Mito *m = M_(c);
    u64 len_at, bytes_at, cap;
    if (kind & 1) { const MitoColl *k = coll(&m->mem, (u32)a); len_at = caddr(m, (u32)a, b, cc); bytes_at = caddr(m, (u32)a, b, cc + 1); cap = k->cols[cc + 1].n; }
    else { len_at = a; bytes_at = a + 4; cap = b; }
    u64 start = clear ? 0 : rd(&m->mem, len_at, 4), cur = start;
    u32 mark = m->text.len;
    for (u32 i = 0; i < nv; i++) {
        const u8 *p; u64 n; u8 d[24];
        if (v[i].tag == 4) { p = t_ptr(m, (T){ v[i].a, v[i].b }); n = v[i].b; }
        else if (v[i].tag == 1) {
            /* an integer, written where it goes */
            u64 x = v[i].i < 0 ? (u64)0 - (u64)v[i].i : (u64)v[i].i; int k = 24;
            do { d[--k] = (u8)('0' + x % 10); x /= 10; } while (x);
            if (v[i].i < 0) d[--k] = '-';
            p = d + k; n = (u64)(24 - k);
        }
        else if (v[i].tag == 0) continue;
        else { m->text.len = mark; if (show(m, &v[i])) return -1; p = m->text.p + mark; n = m->text.len - mark; }
        if (cur + n > cap) { m->text.len = mark; err(m, E_TOOLONG, 413); a_str(&m->emsg, "string of "); a_u64(&m->emsg, cur + n); a_str(&m->emsg, " bytes does not fit string<"); a_u64(&m->emsg, cap); a_str(&m->emsg, ">"); return -1; }
        if ((kind & 4) && v[i].tag == 4 && not_ascii(m, p, n)) { m->text.len = mark; return -1; }
        put(&m->mem, bytes_at + cur, p, n);
        cur += n;
    }
    m->text.len = mark;
    if (cur != start || clear) wr(&m->mem, len_at, 4, cur);
    return 0;
}
/* n bytes of state at `at` (a window onto a text field) as a JSON string, read where they are */
static int h_json_at(Ctx *c, u64 at, u32 n) { Mito *m = M_(c); return a_json_str(&m->out, m->mem.base + at, n) ? 0 : resp_full(m); }
/* A stored id (slot + 1 in the low 32 bits, 0 for none; the generation in the high 32) of top-level checked pool `t`, read back:
   *s 1 and the item's index when it is still that item, *s 0 when it is null, no longer live, or its slot was reused. */
static int h_id_get(Ctx *c, u32 t, i64 v, u32 *s, u32 *idx) {
    Mito *m = M_(c); u64 slot1 = (u64)v & 0xFFFFFFFFu, out = 0;
    *s = 0; *idx = 0;
    if (slot1 && deref_quiet(m, t, slot1 - 1, (u32)((u64)v >> 32), &out)) { *s = 1; *idx = (u32)out; }
    return 0;
}
/* The stored id of item `idx` of top-level checked pool `t`. */
static int h_id_of(Ctx *c, u32 t, u32 idx, i64 *out) {
    Mito *m = M_(c); u32 g = (u32)rd(&m->mem, gaddr(&m->mem, t, idx), 4);
    *out = (i64)(((u64)g << 32) | ((u64)idx + 1)); return 0;
}
/* `log(a, b, ...)`: the values, formatted as `+` would and separated by spaces, on one line of standard error with the line they
   were logged from; not part of the answer. */
static int h_log(Ctx *c, u32 line, const Val *v, u32 n) {
    Mito *m = M_(c); u32 mark = m->text.len;
    a_str(&m->text, "log: line "); a_u64(&m->text, line); a_str(&m->text, ":");
    for (u32 i = 0; i < n; i++) { a_str(&m->text, " "); if (v[i].tag == 0) a_str(&m->text, "null"); else if (show(m, &v[i])) { m->text.len = mark; return -1; } }
    a_str(&m->text, "\n");
    os_err(m->text.p + mark, (u32)(m->text.len - mark));
    m->text.len = mark;
    return 0;
}

/* ---- tables (runtime/store.c): rows by id; a table is collection MITO_TBASE + its index in generated code ---- */
#ifdef MITO_USES_SQLITE
static int t_dbfail(Mito *m) {
    Store *st = &m->store; int ec = sqlite3_extended_errcode(st->db);
    if ((ec & 0xFF) == 19) return fail(m, 409, "a row with those values already exists (a unique index)"); /* SQLITE_CONSTRAINT */
    err(m, E_FAIL, 500); a_str(&m->emsg, "database: "); a_str(&m->emsg, sqlite3_errmsg(st->db)); return -1;
}
/* column `col` (as the program declares it: `c`) of the current row of statement s, read as its declared type whatever SQLite holds (another
   program's tables are not STRICT): null for NULL; text over its declared most bytes, or a time that is not one, fails the request (500) */
static int t_val(Mito *m, sqlite3_stmt *s, int col, const StoreCol *c, const char *table, Val *out) {
    int ty = sqlite3_column_type(s, col);
    if (ty == 5) { *out = v_null(); return 0; }
    switch (c->type) {
        case 0: *out = v_int(sqlite3_column_int64(s, col)); return 0;
        case 1: *out = v_flt(sqlite3_column_double(s, col)); return 0;
        case 2: {
            const u8 *p = sqlite3_column_text(s, col); u64 n = (u64)sqlite3_column_bytes(s, col);
            if (n > c->max) { err(m, E_FAIL, 500); a_str(&m->emsg, "a row of "); a_str(&m->emsg, table); a_str(&m->emsg, " holds a "); a_str(&m->emsg, c->name); a_str(&m->emsg, " longer than "); a_u64(&m->emsg, c->max); a_str(&m->emsg, " bytes, the program's limit"); return -1; }
            T t; if (t_push(m, p, n, &t)) return -1; *out = v_str(t); return 0; }
        case 4: {
            const u8 *p = sqlite3_column_text(s, col); u32 n = (u32)sqlite3_column_bytes(s, col); i64 us;
            if (!time_parse(p, n, &us)) { err(m, E_FAIL, 500); a_str(&m->emsg, "a row of "); a_str(&m->emsg, table); a_str(&m->emsg, " holds a "); a_str(&m->emsg, c->name); a_str(&m->emsg, " that is not a time: "); a_lossy(&m->emsg, p, n < 40 ? n : 40); return -1; }
            *out = v_int(us); return 0; }
        default: { T t; if (t_push(m, sqlite3_column_blob(s, col), (u64)sqlite3_column_bytes(s, col), &t)) return -1; *out = v_str(t); return 0; }
    }
}
static int t_bind(Mito *m, sqlite3_stmt *s, int at, const Val *v) {
    switch (v->tag) {
        case 1: case 3: return sqlite3_bind_int64(s, at, v->i);
        case 2: return sqlite3_bind_double(s, at, v->f);
        case 4: return sqlite3_bind_text(s, at, (const char *)m->text.p + v->a, (int)v->b, (void (*)(void *))0);
        case 5: return sqlite3_bind_int64(s, at, (i64)v->b);
        default: return sqlite3_bind_null(s, at); /* a reference set to null */
    }
}
/* column `col` of row `rowid` of table `t`, through the row cache (a loop's cursor or the last row read) */
static int t_get(Mito *m, u32 t, u32 rowid, u32 col, Val *out) {
    Store *st = &m->store;
    if (!(st->row && st->row_t == t && st->row_id == rowid)) {
        store_forget_row(st);
        sqlite3_stmt *s = st->ts[t].row; sqlite3_bind_int64(s, 1, (i64)rowid);
        int r = sqlite3_step(s);
        if (r != SQ_ROW) { sqlite3_reset(s); if (r == SQ_DONE) { err(m, E_FAIL, 404); a_str(&m->emsg, "no row "); a_u64(&m->emsg, rowid); a_str(&m->emsg, " in "); a_str(&m->emsg, st->tabs[t].sql); return -1; } return t_dbfail(m); }
        st->row = s; st->row_t = t; st->row_id = rowid; st->row_off = 0;
    }
    return t_val(m, st->row, (int)(st->row_off + col), &st->tabs[t].cols[col], st->tabs[t].sql, out);
}
static int t_run(Mito *m, sqlite3_stmt *s) { int r = sqlite3_step(s); sqlite3_reset(s); return r == SQ_DONE || r == SQ_ROW ? 0 : t_dbfail(m); }
#endif
#ifdef MITO_USES_SQLITE
#define T_ST(c) (&M_(c)->store)
static int h_tget(Ctx *c, u32 t, u32 rowid, u32 col, Val *out) { return t_get(M_(c), t, rowid, col, out); }
static int h_tset(Ctx *c, u32 t, u32 rowid, u32 col, const Val *v) { RD_ANY(M_(c)); M_(c)->tw++;
    Mito *m = M_(c); Store *st = T_ST(c); store_forget_row(st);
    if (v->tag == 4 && st->tabs[t].cols[col].ascii && not_ascii(m, t_ptr(m, (T){ v->a, v->b }), v->b)) return -1; /* an ascii<N> column */
    sqlite3_stmt *s = st->ts[t].upd[col];
    if (st->tabs[t].cols[col].type == 4 && v->tag == 1) { char tx[40]; u32 n = time_text(v->i, tx); sqlite3_bind_text(s, 1, tx, (int)n, (void (*)(void *))-1); } /* a time, as text (copied) */
    else t_bind(m, s, 1, v);
    sqlite3_bind_int64(s, 2, (i64)rowid); return t_run(m, s);
}
static int h_tadd(Ctx *c, u32 t, u32 parent, u32 *out) { RD_ANY(M_(c)); M_(c)->tw++;
    Mito *m = M_(c); Store *st = T_ST(c); store_forget_row(st);
    if (st->noadd[t]) { err(m, E_FAIL, 500); a_str(&m->emsg, "cannot add a row: "); a_str(&m->emsg, st->noadd_msg[t]); return -1; }
    sqlite3_stmt *s = st->ts[t].ins; if (st->tabs[t].parent >= 0) sqlite3_bind_int64(s, 1, (i64)parent);
    if (t_run(m, s)) return -1;
    i64 id = sqlite3_last_insert_rowid(st->db);
    if (id <= 0 || id > 0xFFFFFFFFLL) return fail(m, 507, "a table's row ids reached 2^32");
    *out = (u32)id; return 0;
}
static int h_tdel(Ctx *c, u32 t, u32 rowid) { RD_ANY(M_(c)); M_(c)->tw++; Mito *m = M_(c); Store *st = T_ST(c); store_forget_row(st); sqlite3_stmt *s = st->ts[t].del; sqlite3_bind_int64(s, 1, (i64)rowid); return t_run(m, s); }
static int h_tclear(Ctx *c, u32 t, u32 parent) { RD_ANY(M_(c)); M_(c)->tw++; Mito *m = M_(c); Store *st = T_ST(c); store_forget_row(st); sqlite3_stmt *s = st->ts[t].clr; if (st->tabs[t].parent >= 0) sqlite3_bind_int64(s, 1, (i64)parent); return t_run(m, s); }
static int h_texists(Ctx *c, u32 t, u32 rowid, u32 *out) {
    Mito *m = M_(c); Store *st = T_ST(c); sqlite3_stmt *s = st->ts[t].has; sqlite3_bind_int64(s, 1, (i64)rowid);
    int r = sqlite3_step(s); sqlite3_reset(s); *out = r == SQ_ROW; return r == SQ_ROW || r == SQ_DONE ? 0 : t_dbfail(m);
}
static int h_tcount(Ctx *c, u32 t, u32 parent, i64 *out) {
    Mito *m = M_(c); Store *st = T_ST(c); sqlite3_stmt *s = st->ts[t].cnt;
    if (st->tabs[t].parent >= 0) sqlite3_bind_int64(s, 1, (i64)parent);
    int r = sqlite3_step(s); *out = r == SQ_ROW ? sqlite3_column_int64(s, 0) : 0; sqlite3_reset(s); return r == SQ_ROW ? 0 : t_dbfail(m);
}
/* `coll[id]`: the row, when it is one of this table's (and this owner's, for a nested table); otherwise 404 */
static int h_tfind(Ctx *c, u32 t, u32 rowid, u32 parent) {
    Mito *m = M_(c); Store *st = T_ST(c); sqlite3_stmt *s = st->ts[t].find;
    sqlite3_bind_int64(s, 1, (i64)rowid); if (st->tabs[t].parent >= 0) sqlite3_bind_int64(s, 2, (i64)parent);
    int r = sqlite3_step(s); sqlite3_reset(s);
    if (r == SQ_ROW) return 0;
    if (r != SQ_DONE) return t_dbfail(m);
    err(m, E_FAIL, 404); a_str(&m->emsg, "no row "); a_u64(&m->emsg, rowid); a_str(&m->emsg, " in "); a_str(&m->emsg, st->tabs[t].sql); return -1;
}
/* a loop over a table: its statement (written by the compiler; column 0 the row id, then the row's columns) with its parameters bound */
/* `coll.add { field: value, ... }` on a table: one INSERT naming those columns (the owner's id first for a nested table), so another
   program's NOT NULL columns get their values at once; each value bound as its column is kept (a time as text). Its statement (`sql`, a
   literal of the program) is prepared once. */
static int h_tinsert(Ctx *c, u32 t, u32 parent, const char *sql, const u32 *cols, const Val *vals, u32 n, u32 *out) { RD_ANY(M_(c)); M_(c)->tw++;
    Mito *m = M_(c); Store *st = T_ST(c); store_forget_row(st);
    SqCached *k = 0;
    for (u32 i = 0; i < st->ncached && !k; i++) if (st->cached[i].sql == sql) k = &st->cached[i];
    if (!k) {
        if (st->ncached == SQ_CACHED) return fail(m, 500, "more than 128 table statements");
        k = &st->cached[st->ncached]; k->sql = sql; k->busy = 0;
        if (sqlite3_prepare_v2(st->db, sql, -1, &k->s, 0) != SQ_OK) return t_dbfail(m);
        st->ncached++;
    }
    int at = 1;
    if (st->tabs[t].parent >= 0) sqlite3_bind_int64(k->s, at++, (i64)parent);
    for (u32 i = 0; i < n; i++, at++) {
        const StoreCol *col = &st->tabs[t].cols[cols[i]]; const Val *v = &vals[i];
        if (v->tag == 4 && col->ascii && not_ascii(m, t_ptr(m, (T){ v->a, v->b }), v->b)) { sqlite3_reset(k->s); return -1; }
        if (col->type == 4 && v->tag == 1) { char tx[40]; u32 tn = time_text(v->i, tx); sqlite3_bind_text(k->s, at, tx, (int)tn, (void (*)(void *))-1); }
        else t_bind(m, k->s, at, v);
    }
    if (t_run(m, k->s)) return -1;
    i64 id = sqlite3_last_insert_rowid(st->db);
    if (id <= 0 || id > 0xFFFFFFFFLL) return fail(m, 507, "a table's row ids reached 2^32");
    *out = (u32)id; return 0;
}
static int h_titer(Ctx *c, const char *sql, u32 t, const Val *p, u32 n, u32 *cur) {
    Mito *m = M_(c); Store *st = T_ST(c);
    if (store_cursor(st, sql, t, cur)) { err(m, E_FAIL, 500); a_str(&m->emsg, st->err); return -1; }
    for (u32 i = 0; i < n; i++) t_bind(m, st->cur[*cur].s, (int)i + 1, &p[i]);
    return 0;
}
static int h_tnext(Ctx *c, u32 cur, u32 *rowid, int *more) {
    Mito *m = M_(c); Store *st = T_ST(c); SqCursor *k = &st->cur[cur];
    int r = sqlite3_step(k->s);
    if (r == SQ_ROW) { *rowid = (u32)sqlite3_column_int64(k->s, 0); *more = 1; if (st->row != k->s) store_forget_row(st); st->row = k->s; st->row_t = k->t; st->row_id = *rowid; st->row_off = 1; return 0; }
    *more = 0;
    if (st->row == k->s) st->row = 0;
    if (r != SQ_DONE) return t_dbfail(m);
    return 0;
}
/* A merged loop over one owner's rows (the inner loop of a loop over the owners): the cursor holds every owner's rows, ordered by owner
   then id (columns: owner id, row id, the row's columns); the next row of owner `parent`, skipping rows of owners before it (ones the
   outer loop filtered out or whose inner loop did not run) and keeping a row of a later owner for that owner's turn. */
static int h_tmerge(Ctx *c, u32 cur, u32 parent, u32 *rowid, int *more) {
    Mito *m = M_(c); Store *st = T_ST(c); SqCursor *k = &st->cur[cur];
    *more = 0;
    for (;;) {
        if (k->done) return 0;
        if (!k->pending) {
            int r = sqlite3_step(k->s);
            if (r == SQ_DONE) { k->done = 1; if (st->row == k->s) st->row = 0; return 0; }
            if (r != SQ_ROW) return t_dbfail(m);
            k->pending = 1;
        }
        i64 owner = sqlite3_column_int64(k->s, 0);
        if (owner < (i64)parent) { k->pending = 0; continue; }
        if (owner > (i64)parent) return 0; /* kept for its owner */
        k->pending = 0;
        *rowid = (u32)sqlite3_column_int64(k->s, 1); *more = 1;
        if (st->row != k->s) store_forget_row(st);
        st->row = k->s; st->row_t = k->t; st->row_id = *rowid; st->row_off = 2;
        return 0;
    }
}
/* the end of a loop (finished or left by `break`): its cursor given back */
static int h_tclose(Ctx *c, u32 cur) {
    Store *st = T_ST(c); SqCursor *k = &st->cur[cur];
    if (!k->on) return 0;
    if (st->row == k->s) st->row = 0;
    if (k->own) sqlite3_finalize(k->s);
    else { sqlite3_reset(k->s); for (u32 i = 0; i < st->ncached; i++) if (st->cached[i].s == k->s) st->cached[i].busy = 0; }
    k->on = 0; return 0;
}
/* a session token naming a row (`rowid.secret`, the secret in hex), checked against its `secret` column: 1 and the row, or 0 */
static int t_token_row(Mito *m, u32 t, const u8 *v, u64 n, u32 *out) {
    Store *st = &m->store; u64 rid = 0, sec = 0; u64 i = 0;
    if (st->ts[t].secret < 0) return 0;
    while (i < n && v[i] != '.') i++;
    if (i == 0 || i == n || !parse_u64_radix(v, i, 10, &rid) || rid > 0xFFFFFFFFULL || !parse_u64_radix(v + i + 1, n - i - 1, 16, &sec)) return 0;
    Val x; u32 mark = m->text.len;
    if (t_get(m, t, (u32)rid, (u32)st->ts[t].secret, &x)) { m->ekind = E_NONE; m->emsg.len = 0; m->text.len = mark; return 0; }
    if (x.tag != 1 || (u64)x.i != sec) return 0;
    *out = (u32)rid; return 1;
}
#else
static int h_tget(Ctx *c, u32 t, u32 rowid, u32 col, Val *out) { (void)t; (void)rowid; (void)col; (void)out; return fail(M_(c), 500, "no tables"); }
static int h_tset(Ctx *c, u32 t, u32 rowid, u32 col, const Val *v) { (void)t; (void)rowid; (void)col; (void)v; return fail(M_(c), 500, "no tables"); }
static int h_tadd(Ctx *c, u32 t, u32 parent, u32 *out) { (void)t; (void)parent; (void)out; return fail(M_(c), 500, "no tables"); }
static int h_tdel(Ctx *c, u32 t, u32 rowid) { (void)t; (void)rowid; return fail(M_(c), 500, "no tables"); }
static int h_tcount(Ctx *c, u32 t, u32 parent, i64 *out) { (void)t; (void)parent; (void)out; return fail(M_(c), 500, "no tables"); }
static int h_tfind(Ctx *c, u32 t, u32 rowid, u32 parent) { (void)t; (void)rowid; (void)parent; return fail(M_(c), 500, "no tables"); }
static int h_titer(Ctx *c, const char *sql, u32 t, const Val *p, u32 n, u32 *cur) { (void)sql; (void)t; (void)p; (void)n; (void)cur; return fail(M_(c), 500, "no tables"); }
static int h_tinsert(Ctx *c, u32 t, u32 parent, const char *sql, const u32 *cols, const Val *vals, u32 n, u32 *out) { (void)t; (void)parent; (void)sql; (void)cols; (void)vals; (void)n; (void)out; return fail(M_(c), 500, "no tables"); }
static int h_tnext(Ctx *c, u32 cur, u32 *rowid, int *more) { (void)cur; (void)rowid; (void)more; return fail(M_(c), 500, "no tables"); }
static int h_tclose(Ctx *c, u32 cur) { (void)c; (void)cur; return 0; }
static int h_tmerge(Ctx *c, u32 cur, u32 parent, u32 *rowid, int *more) { (void)cur; (void)parent; (void)rowid; (void)more; return fail(M_(c), 500, "no tables"); }
static int h_tclear(Ctx *c, u32 t, u32 parent) { (void)t; (void)parent; return fail(M_(c), 500, "no tables"); }
static int h_texists(Ctx *c, u32 t, u32 rowid, u32 *out) { (void)t; (void)rowid; (void)out; return fail(M_(c), 500, "no tables"); }
#endif

/* ---- answers other than JSON ---- */
static int h_answer(Ctx *c, u32 kind) { M_(c)->ctype = kind; return 0; }
/* text sent as it is (an HTML or plain-text answer): from the text area, from state, from a text column */
static int h_raw_txt(Ctx *c, const Txt *t) { Mito *m = M_(c); return a_put(&m->out, m->text.p + t->off, t->len) ? 0 : resp_full(m); }
static int h_raw_at(Ctx *c, u64 at, u32 n) { Mito *m = M_(c); return a_put(&m->out, m->mem.base + at, n) ? 0 : resp_full(m); }
static int h_raw_col(Ctx *c, u32 cid, u32 idx, u32 ci) {
    Mito *m = M_(c); u64 a = caddr(m, cid, idx, ci); u32 n = (u32)rd(&m->mem, a, 4);
    return a_put(&m->out, m->mem.base + caddr(m, cid, idx, ci + 1), n) ? 0 : resp_full(m); /* the bytes are the next column */
}
/* text into markup: & < > as entities, and " inside an attribute's double quotes */
static int a_html(Area *o, const u8 *s, u64 n, u32 attr) {
    u64 b = 0;
    for (u64 i = 0; i < n; i++) {
        const char *e = 0; u8 c = s[i];
        if (c == '&') e = "&amp;"; else if (c == '<') e = "&lt;"; else if (c == '>') e = "&gt;"; else if (c == '"' && attr) e = "&quot;";
        if (!e) continue;
        if (!a_put(o, s + b, i - b) || !a_str(o, e)) return 0;
        b = i + 1;
    }
    return a_put(o, s + b, n - b);
}
static int h_html_txt(Ctx *c, const Txt *t, u32 attr) { Mito *m = M_(c); return a_html(&m->out, m->text.p + t->off, t->len, attr) ? 0 : resp_full(m); }
static int h_html_at(Ctx *c, u64 at, u32 n, u32 attr) { Mito *m = M_(c); return a_html(&m->out, m->mem.base + at, n, attr) ? 0 : resp_full(m); }
static int h_html_col(Ctx *c, u32 cid, u32 idx, u32 ci, u32 attr) {
    Mito *m = M_(c); u64 a = caddr(m, cid, idx, ci); u32 n = (u32)rd(&m->mem, a, 4);
    return a_html(&m->out, m->mem.base + caddr(m, cid, idx, ci + 1), n, attr) ? 0 : resp_full(m); /* the bytes are the next column */
}
/* A static answer's ETag (a quoted hash): `ETag` and `Cache-Control: no-cache` (the browser keeps it and asks again each time);
   *nm 1 when If-None-Match names it (or is *), and the answer is then 304 with no body. */
static int h_etag(Ctx *c, const char *tag, u32 n, int *nm) {
    Mito *m = M_(c); const u8 *v; u64 vn;
    *nm = 0;
    if (m->web_etag) return 0;
    if (header(m, (const u8 *)"if-none-match", 13, &v, &vn)) {
        for (u64 i = 0; i < vn; ) {
            while (i < vn && (v[i] == ' ' || v[i] == ',')) i++;
            u64 b = i; while (i < vn && v[i] != ',') i++;
            u64 e = i; while (e > b && v[e - 1] == ' ') e--;
            const u8 *x = v + b; u64 xn = e - b;
            if (xn >= 2 && x[0] == 'W' && x[1] == '/') { x += 2; xn -= 2; } /* a weak validator compares the same for GET */
            if ((xn == 1 && x[0] == '*') || b_eq(x, xn, (const u8 *)tag, n)) { *nm = 1; break; }
        }
    }
    if (!(a_str(&m->head, "ETag: ") && a_put(&m->head, tag, n) && a_str(&m->head, "\r\nCache-Control: no-cache\r\n"))) return fail(m, 500, "too many header lines on the answer");
    if (*nm) m->out.len = 0;
    return 0;
}
/* `foreach p in split(t, sep)` / `split(t)`: the piece of t from *pos, as Ruby's String#split gives them: by a separator, pieces
   between (a leading empty one kept), and none once only separators are left (trailing empty pieces dropped); by whitespace (no
   separator), the runs of non-whitespace. *more 0 when there is no piece left. */
static int h_split_next(Ctx *c, const Txt *t, const Txt *sep, u32 *pos, Txt *part, int *more) {
    Mito *m = M_(c); const u8 *s = m->text.p + t->off; u32 n = t->present ? t->len : 0, p = *pos;
    *more = 0;
    if (!sep) {
        while (p < n && split_ws(s[p])) p++;
        if (p >= n) { *pos = n; return 0; }
        u32 e = p; while (e < n && !split_ws(s[e])) e++;
        part->off = t->off + p; part->len = e - p; part->present = 1; *pos = e; *more = 1; return 0;
    }
    const u8 *q = m->text.p + sep->off; u32 k = sep->present ? sep->len : 0;
    if (!k) return fail(m, 500, "split: the separator is empty");
    /* only separators (or nothing) left: no more pieces */
    { u32 r = p; while (r + k <= n && b_eq(s + r, k, q, k)) r += k; if (r >= n) { *pos = n; return 0; } }
    u32 e = p; while (e + k <= n && !b_eq(s + e, k, q, k)) e++;
    if (e + k > n) e = n;
    part->off = t->off + p; part->len = e - p; part->present = 1; *pos = e < n ? e + k : n; *more = 1;
    return 0;
}
static int h_trim(Ctx *c, const Txt *in, Txt *out) {
    const u8 *p = M_(c)->text.p + in->off; u32 b = 0, e = in->present ? in->len : 0;
    while (b < e && (p[b] == ' ' || (p[b] >= 9 && p[b] <= 13))) b++;
    while (e > b && (p[e - 1] == ' ' || (p[e - 1] >= 9 && p[e - 1] <= 13))) e--;
    out->off = in->off + b; out->len = e - b; out->present = in->present; return 0;
}
/* `see_other(path)`: 303 to a path on this server. The path starts with one `/` and is printable ASCII (no spaces, no line breaks),
   so the answer can neither send the browser to another site nor add header lines of its own. */
static int h_see_other(Ctx *c, const Txt *t) {
    Mito *m = M_(c); const u8 *p = m->text.p + t->off; u32 n = t->len;
    int ok = n >= 1 && p[0] == '/' && !(n >= 2 && (p[1] == '/' || p[1] == '\\'));
    for (u32 i = 0; ok && i < n; i++) if (p[i] <= 0x20 || p[i] >= 0x7F) ok = 0;
    if (!ok) return fail(m, 500, "see_other takes a path on this server: one leading /, printable ASCII, no spaces");
    m->out.len = 0;
    if (!(a_str(&m->head, "Location: ") && a_put(&m->head, p, n) && a_str(&m->head, "\r\n"))) return fail(m, 500, "redirect path too long");
    return 0;
}
/* `found(path)`: a 302 whose Location is the path made absolute with this request's Host (as Rails' redirect_to writes it); the path is
   checked as see_other's, and the Host must be a host name (letters, digits, - . : [ ]) so it cannot add header lines */
static int h_found(Ctx *c, const Txt *t) {
    Mito *m = M_(c); const u8 *p = m->text.p + t->off; u32 n = t->len;
    int ok = n >= 1 && p[0] == '/' && !(n >= 2 && (p[1] == '/' || p[1] == '\\'));
    for (u32 i = 0; ok && i < n; i++) if (p[i] <= 0x20 || p[i] >= 0x7F) ok = 0;
    if (!ok) return fail(m, 500, "found takes a path on this server: one leading /, printable ASCII, no spaces");
    const u8 *h; u64 hn;
    if (!header(m, (const u8 *)"host", 4, &h, &hn) || hn == 0 || hn > 255) return fail(m, 400, "a redirect needs the request's Host");
    for (u64 i = 0; i < hn; i++) { u8 ch = h[i]; if (!((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9') || ch == '-' || ch == '.' || ch == ':' || ch == '[' || ch == ']')) return fail(m, 400, "the request's Host is not a host name"); }
    m->out.len = 0;
    if (!(a_str(&m->head, "Location: http://") && a_put(&m->head, h, hn) && a_put(&m->head, p, n) && a_str(&m->head, "\r\n"))) return fail(m, 500, "redirect path too long");
    return 0;
}
/* `var acc: text<cap>`: its bytes reserved in the text area, in its scope's region (the scope's end gives them back) */
static int h_txt_cap(Ctx *c, u32 cap, Txt *out) {
    Mito *m = M_(c);
    if ((u64)m->text.len + cap > m->text.cap) return text_full(m);
    out->off = m->text.len; out->len = 0; out->present = 1; m->text.len += cap;
    return 0;
}
/* Write pieces (as h_str_set formats them) to a capped var's reserved bytes: after what it holds, or from the start when `clear`.
   A piece may be the var's own text or a window onto it, so bytes are moved, not copied. */
static int h_txt_set(Ctx *c, Txt *dst, u32 cap, int clear, const Val *v, u32 nv) {
    Mito *m = M_(c);
    u64 cur = clear ? 0 : dst->len;
    u32 mark = m->text.len;
    for (u32 i = 0; i < nv; i++) {
        const u8 *p; u64 n; u8 d[24];
        if (v[i].tag == 4) { p = m->text.p + v[i].a; n = v[i].b; }
        else if (v[i].tag == 1) {
            u64 x = v[i].i < 0 ? (u64)0 - (u64)v[i].i : (u64)v[i].i; int k = 24;
            do { d[--k] = (u8)('0' + x % 10); x /= 10; } while (x);
            if (v[i].i < 0) d[--k] = '-';
            p = d + k; n = (u64)(24 - k);
        }
        else if (v[i].tag == 0) continue;
        else { m->text.len = mark; if (show(m, &v[i])) return -1; p = m->text.p + mark; n = m->text.len - mark; }
        if (cur + n > cap) { m->text.len = mark; err(m, E_TOOLONG, 413); a_str(&m->emsg, "text of "); a_u64(&m->emsg, cur + n); a_str(&m->emsg, " bytes does not fit text<"); a_u64(&m->emsg, cap); a_str(&m->emsg, ">"); return -1; }
        if (n) memmove(m->text.p + dst->off + cur, p, n);
        cur += n;
    }
    m->text.len = mark;
    dst->len = (u32)cur; dst->present = 1;
    return 0;
}

static const H mito_h = {
    h_out, h_json_i64, h_json_col, h_json_txt, h_json_val, h_auth, h_index, h_lit, h_fail, h_dead, h_col_txt, h_plain_txt,
    h_binop, h_truthy, h_store, h_store_plain, h_alloc, h_free, h_fwd_begin, h_fwd_put, h_forward, h_crash, h_builtin,
    h_json_f64, h_elem, h_store_elem, h_sel_mark, h_sel_add, h_sel_limit, h_sel_get, h_sel_end, h_field_val, h_store_field, h_free_val, h_was_dead, h_input, h_fields, h_json_id, h_wr_plain, h_wr_int, h_move_elems, h_fill_elems, h_clear_coll, h_count_below, h_str_cmp_lit, h_str_cmp_col, h_save_line, h_commit_range, h_save_range, h_text_len, h_arr_txt, h_text_cut,
    h_rq_put, h_rq_send, h_json_field, h_str_app, h_str_app_at, h_str_set, h_json_at, h_str_cmp_at, h_id_get, h_id_of, h_log,
    h_answer, h_raw_txt, h_raw_at, h_raw_col, h_see_other, h_set_cookie, h_html_txt, h_html_at, h_html_col, h_etag, h_trim, h_tget, h_tset, h_tadd, h_tdel, h_tcount, h_tfind, h_titer, h_tnext, h_tclose, h_tclear, h_texists, h_tmerge, h_set_header, h_tinsert, h_found, h_split_next,
    h_txt_cap, h_txt_set,
};

/* ---------------------------------------------------------------- the interface: mito_* */
/* Start the program: reserve its layout (refusing if over `budget` bytes), commit plain values and top-level headers.
   `seed` 0 takes random numbers from the OS. Returns 0 if over budget or out of address space. */
static int g_share_state;
/* Before MITO(start): keep the state in memory readers can map too (MITO(reader)). 0 when this system cannot. */
MITO_API int MITO(share_state)(int on) { if (on && !os_can_share()) return 0; g_share_state = on; return 1; }
MITO_API void *MITO(start)(u64 budget, u64 seed) {
    const MitoProg *pg = &mito_prog;
    if (pg->total > budget) return 0;
    Mito *m = (Mito *)os_alloc(sizeof(Mito));
    if (!m) return 0;
    Mem *mem = &m->mem;
    mem->prog = pg;
    mem->reserved = up(pg->total ? pg->total : 1, PAGE);
    m->shared_fd = -1;
#ifdef MITO_USES_CRYPTO
    cr_prepare(); /* the algorithms looked up once, before any request (requests may run at once) */
#endif
    mem->base = (u8 *)(g_share_state ? os_reserve_shared(mem->reserved, &m->shared_fd) : os_reserve(mem->reserved));
    mem->npages = mem->reserved / PAGE;
    mem->dirty = (u8 *)os_reserve(up(mem->npages * (PAGE / 64 / 8), PAGE));
    if (!mem->dirty) return 0;
    u64 small = up(mem->npages * 3 + (pg->ncolls + 1) * 16, PAGE);
    u8 *s = (u8 *)os_alloc(small);
    m->small = small;
    if (!mem->base || !s) return 0;
    mem->committed = s; mem->livepg = s + mem->npages; mem->scanned = s + 2 * mem->npages;
    mem->shrinks = (u64 *)(s + up(mem->npages * 3, 8));
    m->pr.stuck = mem->shrinks + pg->ncolls + 1;
    if (!grow_init(&mem->jb, 1ULL << 32) || !grow_init(&mem->ja, 1ULL << 32) || !grow_init(&m->frees, (u64)MAX_FREES * 8) || !grow_init(&m->sel, (u64)MAX_SEL * 4)) return 0;
    m->areas = (u8 *)os_alloc(TEXT_CAP + RESPONSE_CAP);
    m->ops.p = (u8 *)os_alloc(1 << 20); m->ops.cap = 1 << 20; m->ops.len = 0; /* socket operations: 1 MB a request */
    if (!m->ops.p) return 0;
    if (!m->areas) return 0;
    m->text.p = m->areas; m->text.cap = TEXT_CAP; memcpy(m->areas, mito_lits, MITO_LITS_LEN); m->text.len = MITO_LITS_LEN; m->out.p = m->areas + TEXT_CAP; m->out.cap = RESPONSE_CAP;
    m->emsg.p = m->ebuf; m->emsg.cap = sizeof m->ebuf; m->head.p = m->hbuf; m->head.cap = sizeof m->hbuf;
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
MITO_API void MITO(stop)(void *mp_) { Mito *m = (Mito *)mp_; if (!m) return;
#ifdef MITO_USES_SQLITE
    store_close(&m->store);
#endif
    os_release(m->mem.base, m->mem.reserved); os_release(m->mem.committed, m->small); os_release(m->mem.dirty, up(m->mem.npages * (PAGE / 64 / 8), PAGE)); os_release(m->mem.jb.p, m->mem.jb.reserved); os_release(m->mem.ja.p, m->mem.ja.reserved); os_release(m->frees.p, m->frees.reserved); os_release(m->sel.p, m->sel.reserved); os_release(m->areas, TEXT_CAP + RESPONSE_CAP); os_release(m, sizeof(Mito)); }
/* A reader of the primary instance `mp_` (after its tables are open and its `@start` ran): requests that only read run on readers at
   once, each on its own thread, while no request runs on the primary (runtime/http.c holds a readers-writer lock). The state is the
   primary's, mapped read-only; the request struct, text, answer and database connection are the reader's own. 0 with the reason in
   `why` when the program cannot have readers. */
MITO_API void *MITO(reader)(void *mp_, u32 k, char *why, u32 cap) {
    Mito *p = (Mito *)mp_;
    const char *no = 0;
    u32 own_unlogged = 0; for (u32 r = 0; r < mito_prog.nunlogged; r++) if (!(mito_prog.req_size && mito_prog.unlogged[2 * r] == mito_prog.req_off)) own_unlogged++;
    if (p->shared_fd < 0) no = "the state is not in shared memory (this system cannot share it, or MITO(share_state) was not called)";
    else if (!p->mem.all) no = "readers need all of the state committed at start (no --soft/--hard marks, state of at most 1 GiB)";
    else if (own_unlogged) no = "the program has unlogged state, whose changes skip the undo log a reader is stopped by";
    else if (mito_prog.req_size && (mito_prog.req_off % PAGE || mito_prog.req_size % PAGE)) no = "the request struct is not on whole pages";
    if (no) { u32 n = 0; while (no[n] && n + 1 < cap) { why[n] = no[n]; n++; } if (cap) why[n] = 0; return 0; }
    Mito *m = (Mito *)os_alloc(sizeof(Mito));
    if (!m) return 0;
    m->mem = p->mem;
    Mem *mem = &m->mem;
    mem->base = (u8 *)os_map_shared_ro(p->shared_fd, mem->reserved);
    if (!mem->base) return 0;
    if (mito_prog.req_size && !os_map_private_at(mem->base + mito_prog.req_off, mito_prog.req_size)) return 0;
    /* its own line map, all clear (every write outside its request struct reaches save_line, which stops the request) and no undo log */
    mem->dirty = (u8 *)os_alloc(up(mem->npages * (PAGE / 64 / 8), PAGE));
    if (!mem->dirty) return 0;
    Grow none = { 0, 0, 0, 0 }; mem->jb = none; mem->ja = none; mem->jon = 0;
    if (!grow_init(&m->frees, (u64)MAX_FREES * 8) || !grow_init(&m->sel, (u64)MAX_SEL * 4)) return 0;
    m->areas = (u8 *)os_alloc(TEXT_CAP + RESPONSE_CAP);
    m->ops.p = (u8 *)os_alloc(1 << 20); m->ops.cap = 1 << 20; m->ops.len = 0;
    if (!m->areas || !m->ops.p) return 0;
    m->text.p = m->areas; m->text.cap = TEXT_CAP; memcpy(m->areas, mito_lits, MITO_LITS_LEN); m->text.len = MITO_LITS_LEN; m->out.p = m->areas + TEXT_CAP; m->out.cap = RESPONSE_CAP;
    m->emsg.p = m->ebuf; m->emsg.cap = sizeof m->ebuf; m->head.p = m->hbuf; m->head.cap = sizeof m->hbuf;
    m->rng = p->rng ? p->rng + 0x9E3779B97F4A7C15ULL * (k + 1) : 0;
    m->loop_budget = p->loop_budget; m->text_base = MITO_LITS_LEN; m->web_etag = p->web_etag; m->clock = p->clock; m->fixed_clock = p->fixed_clock;
    m->shared_fd = -1; m->reader = 1;
#ifdef MITO_USES_SQLITE
    m->pool = k + 1;
    if (sq_npools && m->pool >= sq_npools) { const char *e = "more readers than SQLite pools"; u32 n = 0; while (e[n] && n + 1 < cap) { why[n] = e[n]; n++; } if (cap) why[n] = 0; return 0; }
    if (sq_npools) sq_use(m->pool);
    int opened = !p->store.open || store_open(&m->store, mito_tables, MITO_NTABLES, p->db_dir, p->db_file, p->db_pool, p->db_flags | 4);
    if (sq_npools) sq_use(0);
    if (!opened) { u32 n = 0; while (m->store.err[n] && n + 1 < cap) { why[n] = m->store.err[n]; n++; } if (cap) why[n] = 0; return 0; }
#endif
    return m;
}
/* the last request on a reader wanted to change the state (or a socket): it must run again on the primary */
MITO_API int MITO(wants_write)(void *mp_) { return ((Mito *)mp_)->wants_write; }
MITO_API void MITO(set_loop_budget)(void *mp_, i64 n) { ((Mito *)mp_)->loop_budget = n; }
MITO_API void MITO(set_clock)(void *mp_, i64 ms, int fixed) { Mito *m = (Mito *)mp_; m->clock = ms; m->fixed_clock = fixed; }
MITO_API void MITO(set_web_etag)(void *mp_, int on) { ((Mito *)mp_)->web_etag = on; }
MITO_API int MITO(handle)(void *mp_, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *hdrs, u32 nh, const MitoPair *fwd, u32 nf, const char *body, u32 bn);
/* Run the program's `@start` function (if it has one): after its tables are open, before any request. 0, or the failure's status
   (its message then in MITO(response)). */
MITO_API int MITO(begin)(void *mp_) {
    Mito *m = (Mito *)mp_;
    if (!MITO_HAS_START) return 0;
    m->starting = 1;
    int code = MITO(handle)(m, "START", 5, "/", 1, 0, 0, 0, 0, "", 0);
    m->starting = 0;
    return code < 300 ? 0 : code;
}
MITO_API void MITO(set_marks)(void *mp_, u64 soft, u64 hard) { Mito *m = (Mito *)mp_;
    m->pr.on = 1; m->pr.soft = soft; m->pr.hard = hard; m->pr.refusing = 0; m->pr.trims = m->pr.full_trims = m->pr.freed = 0;
    if (m->mem.all) { m->mem.all = 0; trim_all(&m->mem); } /* pressure needs exact accounting: give back what is not live, commit on demand from here */
    for (u32 c = 0; c < m->mem.prog->ncolls; c++) m->pr.stuck[c] = ~0ULL;
}
static void out_err(Mito *m, const u8 *msg, u64 n) {
    m->out.len = 0;
    if (m->html_err) { /* for a browser: a page that says what went wrong */
        m->ctype = 1;
        a_str(&m->out, "<!doctype html><meta charset=\"utf-8\"><title>Error</title><p>"); a_html(&m->out, msg, n, 0); a_str(&m->out, "</p>");
        return;
    }
    a_str(&m->out, "{\"error\":"); a_json_str(&m->out, msg, n); a_str(&m->out, "}");
}
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
            /* a parameter after a literal prefix: the segment starts with it, and holds more */
            if (r->segs[s].param && r->segs[s].pre) { u64 pl = c_len(r->segs[s].pre); if (i - b <= pl || !b_eq(path + b, pl, (const u8 *)r->segs[s].pre, pl)) ok = 0; }
            s++;
        }
        if (ok) return e;
    }
    return 0xFFFFFFFFu;
}
/* Handle one request, with its headers and (in a system) the fields forwarded with it. Returns the status, or 0 when
   the request was forwarded: then MITO(forward_to) says where, and MITO(finish_forward) commits or rolls back. */
MITO_API int MITO(handle)(void *mp_, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *hdrs, u32 nh, const MitoPair *fwd, u32 nf, const char *body, u32 bn) { Mito *m = (Mito *)mp_;
    m->out.len = 0; m->text.len = m->text_base; m->ekind = E_NONE; m->ctype = 0; m->head.len = 0; m->bare = 0;
#ifdef MITO_USES_HTML
    hp_reset(&m->hp);
#endif
    m->target = (const u8 *)target; m->target_n = tn; m->ops.len = 0; m->wants_write = 0; m->tw = 0; m->c_on = 0; m->c_entry.len = 0;
    const u8 *t = (const u8 *)target;
    u64 q = 0; while (q < tn && t[q] != '?') q++;
    /* the `@start` function only through MITO(begin), never as a request */
    u32 ei = c_eq("START", (const u8 *)method, mn) && !m->starting ? 0xFFFFFFFFu : route(m, (const u8 *)method, mn, t, q);
    m->html_err = 0;
    if (ei == 0xFFFFFFFFu) { out_err_c(m, "no such endpoint"); return 404; }
    m->html_err = (int)mito_prog.routes[ei].html;
    if (m->pr.on && m->pr.refusing && !c_eq("GET", (const u8 *)method, mn) && !c_eq("DELETE", (const u8 *)method, mn)) { out_err_c(m, "memory pressure: try again later"); return 503; }
    m->ei = ei; m->hdrs = hdrs; m->nhdrs = nh; m->fwd = fwd; m->nfwd = nf;
    /* a change asked for by a page of another site (a browser sends Origin with every form post): refused. A request without Origin
       (an API client, curl) is not a browser's cross-site one. */
    if (!c_eq("GET", (const u8 *)method, mn) && !c_eq("HEAD", (const u8 *)method, mn)) {
        const u8 *o, *hv; u64 on, hn;
        if (header(m, (const u8 *)"origin", 6, &o, &on)) {
            int same = header(m, (const u8 *)"host", 4, &hv, &hn) && hn > 0 &&
                ((on == 7 + hn && b_eq(o, 7, (const u8 *)"http://", 7) && b_eq(o + 7, hn, hv, hn)) || (on == 8 + hn && b_eq(o, 8, (const u8 *)"https://", 8) && b_eq(o + 8, hn, hv, hn)));
            if (!same) { out_err_c(m, "cross-site request refused: its Origin is not this site"); return 403; }
        }
    }
    const MitoRoute *r = &mito_prog.routes[ei];
    m->nparams = 0; m->nquery = 0; m->njson = 0;
    int fits = 1;
    { u64 i = 0; u32 s = 0;
      while (s < r->nsegs) {
          while (i < q && t[i] == '/') i++;
          u64 b = i; while (i < q && t[i] != '/') i++;
          if (r->segs[s].param && r->segs[s].pre) b += c_len(r->segs[s].pre);
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
        m->form = is_form(m);
        int pr = m->form ? parse_form(m, (const u8 *)body, bn) : parse_body(m, (const u8 *)body, bn);
        if (pr == -2) fits = 0;
        else if (pr == -3) { out_err_c(m, "request body has more than 256 fields"); return 413; }
        else if (pr) { out_err(m, m->ebuf, m->emsg.len); return 400; }
    }
    if (!fits) { out_err_c(m, "request too large for the request text area"); return 413; }
    m->now = m->fixed_clock ? m->clock : os_now_ms();
#ifdef MITO_USES_SQLITE
    if (sq_npools) sq_use(m->pool); /* SQLite allocates for this instance's connection from its pool */
#endif
    if (!m->reader) begin(&m->mem);
    m->frees.len = 0; m->sel.len = 0;
    /* the request struct starts empty: the parts of it the endpoint's code names (all of it when it calls a shared function) */
    if (mito_prog.req_size) {
        const MitoRoute *zr = &mito_prog.routes[ei];
        if (zr->nzero == 0xFFFFFFFFu) memset(m->mem.base + mito_prog.req_off, 0, mito_prog.req_size);
        else for (u32 z = 0; z < zr->nzero; z++) memset(m->mem.base + zr->zero[2 * z], 0, zr->zero[2 * z + 1] - zr->zero[2 * z]);
    }
    /* a reader journals nothing, but its writes go through the journal's test (its line map is clear), so each reaches save_line */
    Ctx ctx = { m->mem.base, m, &mito_h, m->mem.dirty, m->mem.committed, m->reader ? 1 : m->mem.jon, m->text.p, 0, m->loop_budget };
#ifdef MITO_USES_SQLITE
    if (!store_begin(&m->store)) { rollback(&m->mem); out_err_c(m, "the database could not start a transaction"); return 500; }
#endif
    int res = mito_eps[ei](&ctx);
    if (res < 0 && m->ekind == E_FORWARD) { m->out.len = 0; m->pending = 1; return 0; } /* committed or rolled back by MITO(finish_forward) */
    /* a reader's request that would change the state, or asks something of sockets: nothing of it stays, it runs again alone */
    if (m->reader && (m->wants_write || m->ops.len)) {
        m->wants_write = 1;
#ifdef MITO_USES_SQLITE
        store_rollback(&m->store);
#endif
        return 503;
    }
    int code, ok;
    if (res == 0) { m->out.len = 0; code = 204; ok = 1; }
    else if (res > 0) { code = res; ok = 1; }
    else {
        ok = 0;
        m->ctype = 0; m->head.len = 0; /* an error is answered in JSON */
        switch (m->ekind) {
            case E_FULL: { u8 b[4200]; Area x = { b, sizeof b, 0 }; a_str(&x, "capacity: "); a_put(&x, m->ebuf, m->emsg.len); out_err(m, b, x.len); code = 503; break; }
            case E_DEAD: out_err_c(m, "reference is not live"); code = 404; break;
            case E_NONE: out_err_c(m, "compiled code failed without a reason"); code = 500; break;
            default: out_err(m, m->ebuf, m->emsg.len); code = m->ecode; break;
        }
        /* the source line of the statement that failed, when the compiled code knew it */
        if (ctx.at && m->out.len && m->out.p[m->out.len - 1] == '}') { m->out.len--; a_str(&m->out, ",\"line\":"); a_u64(&m->out, ctx.at); a_str(&m->out, "}"); }
    }
#ifdef MITO_USES_SQLITE
    /* the tables first: a request whose commit fails (a full disk) fails, and the working state is rolled back with it */
    if (ok && !store_commit(&m->store)) { ok = 0; m->ctype = 0; m->head.len = 0; out_err(m, (const u8 *)m->store.err, c_len(m->store.err)); code = 500; }
    else if (!ok) store_rollback(&m->store);
#endif
    if (ok) { if (!m->reader && (m->mem.ja.len || m->tw || m->frees.len)) g_data_version++; u32 *f = (u32 *)m->frees.p; for (u64 i = 0; i < m->frees.len; i += 2) free_item(&m->mem, f[i], f[i + 1]); commit_changes(&m->mem); }
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
/* Before MITO(open_data): SQLite's memory as `n` pools of `mb` MB, one for each connection (the primary's and each reader's: see
   store.c), in place of one pool for the process. 1, or 0 when they cannot be made. */
#ifdef MITO_USES_SQLITE
MITO_API int MITO(db_pools)(u32 n, u64 mb) { return sq_pools_init(n, mb); }
#else
MITO_API int MITO(db_pools)(u32 n, u64 mb) { (void)n; (void)mb; return 1; }
#endif
/* Open the program's tables in `dir` (the database file `file`, SQLite's memory pool `pool_mb` MB, `full`: every commit synced to disk
   before its answer; flags: 1 that, 2 drop what does not fit instead of refusing): 1, or 0 with MITO(data_error) saying why. A program
   without tables has nothing to open. */
MITO_API int MITO(open_data)(void *mp_, const char *dir, const char *file, u64 pool_mb, int flags) {
#ifdef MITO_USES_SQLITE
    Mito *m = (Mito *)mp_;
    { u32 n = 0; while (dir[n] && n + 1 < sizeof m->db_dir) { m->db_dir[n] = dir[n]; n++; } m->db_dir[n] = 0; n = 0; while (file[n] && n + 1 < sizeof m->db_file) { m->db_file[n] = file[n]; n++; } m->db_file[n] = 0; }
    m->db_pool = pool_mb; m->db_flags = flags;
    return store_open(&m->store, mito_tables, MITO_NTABLES, dir, file, pool_mb, flags);
#else
    (void)mp_; (void)dir; (void)file; (void)pool_mb; (void)flags; return 1;
#endif
}
MITO_API const char *MITO(data_error)(void *mp_) {
#ifdef MITO_USES_SQLITE
    return ((Mito *)mp_)->store.err;
#else
    (void)mp_; return "";
#endif
}
MITO_API int MITO(has_tables)(void) {
#ifdef MITO_USES_SQLITE
    return 1;
#else
    return 0;
#endif
}
/* Are the program's tables another program's (`extern`)? Then the server opens that database (`--db FILE`) instead of its own. */
MITO_API int MITO(tables_extern)(void) {
#ifdef MITO_USES_SQLITE
    return MITO_NTABLES > 0 && mito_tables[0].ext;
#else
    return 0;
#endif
}
MITO_API const char *MITO(response)(void *mp_, u32 *len) { Mito *m = (Mito *)mp_; *len = m->out.len; return (const char *)m->out.p; }
/* the last answer: bytes (Response<bytes>, sent as they are, not checked as UTF-8), and whether it leaves out the web block's lines */
MITO_API int MITO(response_binary)(void *mp_) { return ((Mito *)mp_)->ctype == 6; }
MITO_API int MITO(response_bare)(void *mp_) { return ((Mito *)mp_)->bare; }
/* The answer's header lines, each ending in CRLF: its content type, then any the program set (Location). */
static const char *const ctypes[] = { "application/json", "text/html; charset=utf-8", "text/plain; charset=utf-8", "text/css; charset=utf-8", "text/javascript; charset=utf-8", "image/svg+xml", "application/octet-stream" };
MITO_API const char *MITO(response_head)(void *mp_, u32 *len) { Mito *m = (Mito *)mp_;
    static const char pre[] = "Content-Type: ";
    u8 *b = m->hout; u32 n = 0;
    const char *ct = ctypes[m->ctype < 7 ? m->ctype : 0];
    /* the content type for the answer's kind, unless the endpoint set its own (`set_header("Content-Type", ...)`) */
    int own = 0; for (u32 i = 0; i + 13 <= m->head.len && !own; i++) if ((i == 0 || m->head.p[i - 1] == '\n') && (m->head.p[i] | 32) == 'c' && b_eq_ci(m->head.p + i, 13, "content-type:")) own = 1;
    if (!own) {
        for (const char *x = pre; *x; x++) b[n++] = (u8)*x;
        for (const char *x = ct; *x; x++) b[n++] = (u8)*x;
        b[n++] = '\r'; b[n++] = '\n';
    }
    u32 k = (u32)m->head.len; /* at most sizeof hbuf, and hout has room for it after the content type */
    memcpy(b + n, m->head.p, k); n += k;
    *len = n; return (const char *)b;
}

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
