
/* ---- the HTTP front end and main (runtime/http.c) ----
   Serves one program (HTTP_SINGLE) or a system of workers (HTTP_SYSTEM) on 127.0.0.1: headers up to 64 KB, a body of
   Content-Length bytes (at most 1 MB), invalid UTF-8 in the request read as U+FFFD. Connections are kept open (HTTP/1.1
   unless the client sends `Connection: close`; HTTP/1.0 only with `Connection: keep-alive`), and requests sent back to
   back on one connection are answered in order. A request that is malformed or cut short closes its connection.
   `--conns N` connection threads (16) serve connections that have a request waiting; one more thread waits on the idle
   ones (poll) and hands a connection back to the threads when its next request arrives; an idle connection is closed
   after 10 s. One program's handler runs for one request at a time (a lock); a system has its own workers.
   Sockets, threads, locks and timers are runtime/os.c's (Windows and Linux). Flags: --port N, --conns N (a system's connection threads), --budget MB, --soft MB,
   --hard MB (one program's memory), and for tests --seed N and --clock MS. */

static int g_timing; /* --timing: every answer carries X-Handler-Ns, the time the handler took (for benchmarks; off, the answers are byte for byte as recorded) */

#ifdef HTTP_SINGLE
void *mito_start(u64 budget, u64 seed);
void mito_set_clock(void *m, i64 ms, int fixed);
void mito_set_loop_budget(void *m, i64 n);
void mito_set_marks(void *m, u64 soft, u64 hard);
int mito_handle(void *m, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *h, u32 nh, const MitoPair *f, u32 nf, const char *body, u32 bn);
const char *mito_response(void *m, u32 *len);
const char *mito_response_head(void *m, u32 *len);
#else
void *mito_system_start(u64 seed, int seeded);
void mito_system_set_clock(void *s, i64 ms, int fixed);
int mito_system_request(void *s, const char *m, u32 mn, const char *t, u32 tn, const MitoPair *h, u32 nh, const char *body, u32 bn, u32 timeout_ms, char *out, u32 cap, u32 *outlen, char *head, u32 *headlen);
#endif


static const char *reason(int code) {
    switch (code) {
        case 200: return "OK"; case 201: return "Created"; case 204: return "No Content"; case 303: return "See Other"; case 304: return "Not Modified"; case 400: return "Bad Request"; case 401: return "Unauthorized";
        case 403: return "Forbidden"; case 404: return "Not Found"; case 409: return "Conflict"; case 413: return "Payload Too Large"; case 429: return "Too Many Requests";
        case 500: return "Internal Server Error"; case 503: return "Service Unavailable"; case 504: return "Gateway Timeout"; case 507: return "Insufficient Storage";
    }
    return "Error";
}

/* text: bytes to UTF-8 with each maximal invalid sequence as U+FFFD (Rust's from_utf8_lossy) */
typedef struct { u8 *p; u32 cap, len; } Buf;
static void b_put(Buf *b, const void *s, u32 n) { if (b->len + (u64)n > b->cap) n = b->cap - b->len; memcpy(b->p + b->len, s, n); b->len += n; }
static void b_lossy(Buf *o, const u8 *s, u32 n) {
    for (u32 i = 0; i < n;) {
        u8 c = s[i]; u32 need; u8 lo = 0x80, hi = 0xBF;
        if (c < 0x80) { b_put(o, s + i, 1); i++; continue; }
        if (c >= 0xC2 && c <= 0xDF) need = 1; else if (c >= 0xE0 && c <= 0xEF) { need = 2; if (c == 0xE0) lo = 0xA0; if (c == 0xED) hi = 0x9F; }
        else if (c >= 0xF0 && c <= 0xF4) { need = 3; if (c == 0xF0) lo = 0x90; if (c == 0xF4) hi = 0x8F; }
        else { b_put(o, "\xEF\xBF\xBD", 3); i++; continue; }
        u32 j = i + 1, got = 0;
        while (got < need && j < n && s[j] >= (got == 0 ? lo : 0x80) && s[j] <= (got == 0 ? hi : 0xBF)) { j++; got++; }
        if (got == need) b_put(o, s + i, j - i); else b_put(o, "\xEF\xBF\xBD", 3);
        i = j;
    }
}
static int utf8_valid(const u8 *s, u32 n) {
    for (u32 i = 0; i < n;) {
        u8 c = s[i]; u32 need; u8 lo = 0x80, hi = 0xBF;
        if (c < 0x80) { i++; continue; }
        if (c >= 0xC2 && c <= 0xDF) need = 1; else if (c >= 0xE0 && c <= 0xEF) { need = 2; if (c == 0xE0) lo = 0xA0; if (c == 0xED) hi = 0x9F; }
        else if (c >= 0xF0 && c <= 0xF4) { need = 3; if (c == 0xF0) lo = 0x90; if (c == 0xF4) hi = 0x8F; } else return 0;
        for (u32 k = 1; k <= need; k++) { if (i + k >= n) return 0; if (s[i + k] < (k == 1 ? lo : 0x80) || s[i + k] > (k == 1 ? hi : 0xBF)) return 0; }
        i += need + 1;
    }
    return 1;
}
/* the length of the Unicode white space (Rust's char::is_whitespace) starting at s[i], or 0 */
static u32 ws_at(const u8 *s, u32 n, u32 i) {
    u8 c = s[i];
    if (c == ' ' || (c >= 9 && c <= 13)) return 1;
    if (c == 0xC2 && i + 1 < n && (s[i + 1] == 0x85 || s[i + 1] == 0xA0)) return 2;
    if (c == 0xE1 && i + 2 < n && s[i + 1] == 0x9A && s[i + 2] == 0x80) return 3;
    if (c == 0xE2 && i + 2 < n && s[i + 1] == 0x80 && (s[i + 2] <= 0x8A || s[i + 2] == 0xA8 || s[i + 2] == 0xA9 || s[i + 2] == 0xAF)) return 3;
    if (c == 0xE2 && i + 2 < n && s[i + 1] == 0x81 && s[i + 2] == 0x9F) return 3;
    if (c == 0xE3 && i + 2 < n && s[i + 1] == 0x80 && s[i + 2] == 0x80) return 3;
    return 0;
}
/* trim white space at both ends of s[*a..*b] (UTF-8) */
static void trim(const u8 *s, u32 *a, u32 *b) {
    for (u32 k; *a < *b && (k = ws_at(s, *b, *a)); ) *a += k;
    for (;;) {
        u32 hit = 0;
        for (u32 k = 1; k <= 3 && k <= *b - *a; k++) if (ws_at(s, *b, *b - k) == k) { hit = k; break; }
        if (!hit) break;
        *b -= hit;
    }
}

/* A connection's buffer: reserved at its largest when the server starts, committed in 64 KB steps as requests reach
   further into it, and kept at that high-water mark. */
typedef struct { u8 *p; u64 committed, cap; } Res;
static int res_init(Res *r, u64 cap) { r->p = (u8 *)os_reserve(cap); r->committed = 0; r->cap = cap; return r->p != 0; }
static int res_ensure(Res *r, u64 n) {
    if (n <= r->committed) return 1;
    if (n > r->cap) return 0;
    u64 c = (n + 65535) / 65536 * 65536; if (c > r->cap) c = r->cap;
    if (!os_commit(r->p, c)) return 0;
    r->committed = c; return 1;
}
/* rh: the answer's header lines from the program (content type, Location); hd: the head sent, status line to blank line */
typedef struct { void *app; Sock listener; Res data, head, body, pairs; u8 *out; u32 carry; u32 rhn; char rh[4160]; char hd[4160 + 256]; } Conn;
/* the head search stops past 64 KB (at most one more 4 KB read), the body read past Content-Length (at most 1 MB) */
#define HEAD_MAX (64 * 1024 + 4096)
#define DATA_CAP (HEAD_MAX + 4 + (1 << 20) + 4096)
#define TEXT_CAP_ (3 * DATA_CAP)
#define MAX_PAIRS (HEAD_MAX / 2 + 1)
#define OUT_CAP (300 * 1024)

static void send_all(Sock c, const char *p, u32 n) { while (n) { int k = os_send(c, (const u8 *)p, n); if (k <= 0) return; p += k; n -= (u32)k; } }
/* head and body in one call: two small sends can wait on the client's delayed acknowledgement of the first */
static void send_two(Sock c, const char *a, u32 an, const char *b, u32 bn) {
    u64 sent = os_send2(c, (const u8 *)a, an, (const u8 *)b, bn);
    if (sent < an) { send_all(c, a + sent, an - (u32)sent); send_all(c, b, bn); } else if (sent < (u64)an + bn) send_all(c, b + (sent - an), (u32)(an + bn - sent));
}
/* the lock around one program's handler (HTTP_SINGLE): several connection threads, one request in the program at a time */
static Lock g_app_lock;
/* Read one request from connection c and answer it. `first`: the connection's first request (an empty one is answered 400;
   later, the client closing the connection between requests ends it quietly). Returns 1 when the connection stays open. */
static int serve_one(Conn *x, Sock c, int first) {
    os_sock_setup(c); /* a read waits at most 5 s; an answer goes out as soon as it is written */
    u32 len = x->carry, he = 0xFFFFFFFFu, from = 0, eof = 0;
    x->carry = 0;
    for (;;) {
        for (u32 i = from; i + 4 <= len; i++) if (x->data.p[i] == '\r' && x->data.p[i + 1] == '\n' && x->data.p[i + 2] == '\r' && x->data.p[i + 3] == '\n') { he = i; break; }
        if (he != 0xFFFFFFFFu) break;
        if (len > 64 * 1024) break;
        from = len >= 3 ? len - 3 : 0;
        if (!res_ensure(&x->data, (u64)len + 4096)) break;
        int k = os_recv(c, x->data.p + len, 4096);
        if (k <= 0) { eof = 1; break; }
        len += (u32)k;
    }
    if (he == 0xFFFFFFFFu) {
        if (!first && len == 0) return 0; /* the client closed a kept connection: nothing to answer */
        const char *r = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"; u32 n = 0; while (r[n]) n++; send_all(c, r, n); return 0;
    }
    if (!res_ensure(&x->head, 3ULL * he)) return 0; /* each byte is at most 3 bytes of text */
    Buf head = { x->head.p, (u32)x->head.committed, 0 };
    b_lossy(&head, x->data.p, he);
    /* lines (a trailing \r dropped), the request line split on spaces */
    const u8 *h = head.p; u32 hn = head.len, ls = 0, le = 0;
    while (le < hn && h[le] != '\n') le++;
    u32 rle = le < hn && le > 0 && h[le - 1] == '\r' ? le - 1 : le; /* as Rust's lines(): \r goes only with a \n */
    u32 m0 = 0, m1 = 0; while (m1 < rle && h[m1] != ' ') m1++;
    u32 t0, t1;
    if (m1 < rle) { t0 = m1 + 1; t1 = t0; while (t1 < rle && h[t1] != ' ') t1++; } else { t0 = t1 = 0; }
    const char *method = (const char *)h + m0, *target = (const char *)h + t0; u32 mn = m1 - m0, tn = t1 - t0;
    if (m1 >= rle) { target = "/"; tn = 1; }
    /* keep the connection: HTTP/1.1 unless `Connection: close`, HTTP/1.0 only with `Connection: keep-alive`; not for a request line without a version */
    int vok = m1 < rle && t1 + 9 == rle && h[t1] == ' ' && h[t1 + 1] == 'H' && h[t1 + 2] == 'T' && h[t1 + 3] == 'T' && h[t1 + 4] == 'P' && h[t1 + 5] == '/' && h[t1 + 6] == '1' && h[t1 + 7] == '.';
    int v11 = vok && h[t1 + 8] == '1', v10 = vok && h[t1 + 8] == '0';
    int keep = v11;
    u32 np = 0; u64 want = 0; int have_len = 0;
    ls = le < hn ? le + 1 : hn;
    while (ls < hn) {
        u32 e = ls; while (e < hn && h[e] != '\n') e++;
        u32 ee = e < hn && e > ls && h[e - 1] == '\r' ? e - 1 : e;
        u32 col = ls; while (col < ee && h[col] != ':') col++;
        if (col < ee && np < MAX_PAIRS && res_ensure(&x->pairs, (np + 1) * (u64)sizeof(MitoPair))) {
            u32 ka = ls, kb = col, va = col + 1, vb = ee;
            trim(h, &ka, &kb); trim(h, &va, &vb);
            MitoPair *p = &((MitoPair *)x->pairs.p)[np++]; p->k = (const char *)h + ka; p->kn = kb - ka; p->v = (const char *)h + va; p->vn = vb - va;
            if (p->kn == 10) {
                const char *cn = "connection"; u32 q = 0; for (; q < 10; q++) { char ch = p->k[q]; if (ch >= 'A' && ch <= 'Z') ch += 32; if (ch != cn[q]) break; }
                if (q == 10) {
                    /* the value's words: `close` or `keep-alive`, any case */
                    for (u32 i = 0; i < p->vn;) {
                        while (i < p->vn && (p->v[i] == ' ' || p->v[i] == ',' || p->v[i] == '\t')) i++;
                        u32 w = i; while (i < p->vn && p->v[i] != ',' && p->v[i] != ' ' && p->v[i] != '\t') i++;
                        char lw[11]; u32 wn = i - w; if (wn > 10) continue;
                        for (u32 j = 0; j < wn; j++) { char ch = p->v[w + j]; lw[j] = ch >= 'A' && ch <= 'Z' ? (char)(ch + 32) : ch; }
                        if (wn == 5 && lw[0] == 'c' && lw[1] == 'l' && lw[2] == 'o' && lw[3] == 's' && lw[4] == 'e') keep = 0;
                        if (wn == 10 && v10 && lw[0] == 'k' && lw[1] == 'e' && lw[2] == 'e' && lw[3] == 'p' && lw[4] == '-' && lw[5] == 'a' && lw[9] == 'e') keep = 1;
                    }
                }
            }
            const char *cl = "content-length";
            if (!have_len && p->kn == 14) {
                u32 q = 0; for (; q < 14; q++) { char ch = p->k[q]; if (ch >= 'A' && ch <= 'Z') ch += 32; if (ch != cl[q]) break; }
                if (q == 14) {
                    have_len = 1; u64 v = 0; u32 i = 0; int ok = p->vn > 0;
                    if (ok && p->v[0] == '+') { i = 1; ok = p->vn > 1; }
                    for (; i < p->vn && ok; i++) { char ch = p->v[i]; if (ch < '0' || ch > '9' || v > (~0ULL - (u64)(ch - '0')) / 10) ok = 0; else v = v * 10 + (u64)(ch - '0'); }
                    want = ok ? v : 0;
                    if (!ok) keep = 0; /* a length that is not a number: the end of the body is not known */
                }
            }
        }
        ls = e < hn ? e + 1 : hn;
    }
    if (want > (1 << 20)) { want = 1 << 20; keep = 0; }
    while ((u64)len < he + 4 + want && res_ensure(&x->data, (u64)len + 4096)) { int k = os_recv(c, x->data.p + len, 4096); if (k <= 0) { eof = 1; break; } len += (u32)k; }
    u32 bs = he + 4 < len ? he + 4 : len;
    /* the body is Content-Length bytes; what follows is the next request on this connection, kept for it */
    u32 be = (u64)len > he + 4 + want ? (u32)(he + 4 + want) : len;
    if ((u64)be < he + 4 + want) keep = 0; /* cut short: the client stopped sending */
    if (eof) keep = 0;
    if (!res_ensure(&x->body, 3ULL * (be - bs) + 1)) return 0;
    Buf body = { x->body.p, (u32)x->body.committed, 0 };
    b_lossy(&body, x->data.p + bs, be - bs);
    u32 rest = len - be;
    int code; const char *resp; u32 rn;
    u64 t_start = 0, t_end = 0;
    if (g_timing) t_start = os_perf_ns();
#ifdef HTTP_SINGLE
    lk_on(&g_app_lock);
    code = mito_handle(x->app, method, mn, target, tn, (MitoPair *)x->pairs.p, np, 0, 0, (const char *)body.p, body.len);
    resp = mito_response(x->app, &rn);
    if (!utf8_valid((const u8 *)resp, rn)) rn = 0; /* a body that is not UTF-8 is sent empty */
    if (rn > OUT_CAP) rn = 0;
    memcpy(x->out, resp, rn); /* the program's response area is reused by the next request, which may run while this one is sent */
    { const char *h = mito_response_head(x->app, &x->rhn); if (x->rhn > sizeof x->rh) x->rhn = 0; memcpy(x->rh, h, x->rhn); }
    lk_off(&g_app_lock);
    resp = (const char *)x->out;
#else
    code = mito_system_request(x->app, method, mn, target, tn, (MitoPair *)x->pairs.p, np, (const char *)body.p, body.len, 10000, (char *)x->out, OUT_CAP, &rn, x->rh, &x->rhn);
    resp = (const char *)x->out;
#endif
    if (g_timing) t_end = os_perf_ns();
    char *hd = x->hd; u32 n = 0; const char *r = reason(code);
    #define HPUT(s) do { const char *s_ = (s); while (*s_ && n < sizeof x->hd) hd[n++] = *s_++; } while (0)
    char num[24]; int dn;
    HPUT("HTTP/1.1 "); { u64 v = (u64)code; dn = 0; do { num[dn++] = (char)('0' + v % 10); v /= 10; } while (v); while (dn) hd[n++] = num[--dn]; }
    HPUT(" "); HPUT(r); HPUT("\r\n");
    for (u32 i = 0; i < x->rhn && n < sizeof x->hd; i++) hd[n++] = x->rh[i]; /* Content-Type and any other lines, each ending CRLF */
    HPUT("Content-Length: ");
    { u64 v = rn; dn = 0; do { num[dn++] = (char)('0' + v % 10); v /= 10; } while (v); while (dn) hd[n++] = num[--dn]; }
    if (g_timing) {
        u64 v = t_end - t_start;
        HPUT("\r\nX-Handler-Ns: "); dn = 0; do { num[dn++] = (char)('0' + v % 10); v /= 10; } while (v); while (dn) hd[n++] = num[--dn];
    }
    HPUT(keep ? "\r\nConnection: keep-alive\r\n\r\n" : "\r\nConnection: close\r\n\r\n");
    send_two(c, hd, n, resp, rn);
    #undef HPUT
    if (keep && rest) { memmove(x->data.p, x->data.p + be, rest); x->carry = rest; }
    return keep;
}
static Conn *conn_new(void *app, Sock listener) {
    Conn *x = (Conn *)os_alloc(sizeof(Conn));
    if (!x) return 0;
    x->app = app; x->listener = listener;
    if (!res_init(&x->data, DATA_CAP) || !res_init(&x->head, TEXT_CAP_) || !res_init(&x->body, TEXT_CAP_) || !res_init(&x->pairs, sizeof(MitoPair) * (u64)MAX_PAIRS)) return 0;
    x->out = (u8 *)os_alloc(OUT_CAP); /* the answer is copied here, so the program can take the next request while it is sent */
    if (!x->out) return 0;
    return x;
}
/* Connections with a request waiting: a ring the poller fills and the connection threads empty. */
#define READY_CAP 4096
#define IDLE_CAP 4096
#define IDLE_MS 10000
static Lock g_q_lock; static Cond g_q_cv;
static u64 g_ready[READY_CAP]; static u32 g_ready_head, g_ready_len; /* a connection waiting: its socket * 2, + 1 for its first request */
/* connections a thread has finished a request on, for the poller to watch (it takes them under the same lock) */
static Sock g_back[IDLE_CAP]; static u32 g_back_len;
static Sock g_wake; /* the poller's wake-up channel (os_wake_open) */
static void ready_push(u64 c) {
    lk_on(&g_q_lock);
    int ok = g_ready_len < READY_CAP;
    if (ok) { g_ready[(g_ready_head + g_ready_len) % READY_CAP] = c; g_ready_len++; cv_one(&g_q_cv); }
    lk_off(&g_q_lock);
    if (!ok) os_close(c >> 1);
}
static u64 ready_pop(void) {
    lk_on(&g_q_lock);
    while (!g_ready_len) cv_wait(&g_q_cv, &g_q_lock, OS_FOREVER);
    u64 c = g_ready[g_ready_head]; g_ready_head = (g_ready_head + 1) % READY_CAP; g_ready_len--;
    lk_off(&g_q_lock);
    return c;
}
static void give_back(Sock c) {
    lk_on(&g_q_lock);
    int ok = g_back_len < IDLE_CAP; if (ok) g_back[g_back_len++] = c;
    lk_off(&g_q_lock);
    if (!ok) { os_close(c); return; }
    os_wake_ping();
}
/* a connection thread: answer the requests waiting on a connection (back to back ones too), then give it to the poller */
static u32 conn_loop(void *arg) {
    Conn *x = (Conn *)arg;
    for (;;) {
        u64 rc = ready_pop();
        Sock c = rc >> 1; int first = (int)(rc & 1); /* a new connection's first request */
        x->carry = 0;
        int keep = serve_one(x, c, first);
        for (;;) {
            while (keep && x->carry) keep = serve_one(x, c, 0);
            if (!keep) break;
            /* a client that sends its next request as soon as it has the answer: wait for it here a little (1 ms) rather than
               hand the connection to the poller and back, which costs two thread switches */
            OsPoll one; one.fd = c; one.events = OS_POLLIN; one.revents = 0;
            if (os_poll(&one, 1, 1) <= 0) break;
            keep = serve_one(x, c, 0);
        }
        if (keep) give_back(c); else os_close(c);
    }
    return 0;
}
/* the poller: accepts new connections and watches idle ones; one with data (or closed) goes to the connection threads */
static Sock g_idle[IDLE_CAP]; static u64 g_since[IDLE_CAP]; static u8 g_new[IDLE_CAP]; static OsPoll g_fds[IDLE_CAP + 2];
static u32 poll_loop(void *arg) {
    Sock listener = *(Sock *)arg; u32 ni = 0;
    for (;;) {
        lk_on(&g_q_lock);
        u64 now = os_ticks_ms();
        for (u32 i = 0; i < g_back_len; i++) { if (ni < IDLE_CAP) { g_idle[ni] = g_back[i]; g_since[ni] = now; g_new[ni] = 0; ni++; } else os_close(g_back[i]); }
        g_back_len = 0;
        lk_off(&g_q_lock);
        g_fds[0].fd = listener; g_fds[0].events = OS_POLLIN; g_fds[0].revents = 0;
        g_fds[1].fd = g_wake; g_fds[1].events = OS_POLLIN; g_fds[1].revents = 0;
        for (u32 i = 0; i < ni; i++) { g_fds[i + 2].fd = g_idle[i]; g_fds[i + 2].events = OS_POLLIN; g_fds[i + 2].revents = 0; }
        int r = os_poll(g_fds, ni + 2, 1000);
        if (r < 0) continue;
        u32 polled = ni; /* the connections in this poll (one accepted below is not) */
        if (g_fds[1].revents) os_wake_drain();
        if (g_fds[0].revents & OS_POLLIN) {
            /* a new connection waits here, like an idle one, until its first request arrives: no thread blocks on a client that has not sent anything */
            Sock c = os_accept(listener);
            if (c != OS_BAD_SOCK) { if (ni < IDLE_CAP) { g_idle[ni] = c; g_since[ni] = os_ticks_ms(); g_new[ni] = 1; ni++; } else os_close(c); }
        }
        now = os_ticks_ms();
        u32 kept = 0;
        for (u32 i = 0; i < ni; i++) {
            short ev = i < polled ? g_fds[i + 2].revents : 0;
            if (ev) ready_push(g_idle[i] * 2 + g_new[i]); /* a request (or the client closed: the thread finds out); + 1 marks a first request */
            else if (now - g_since[i] > IDLE_MS) os_close(g_idle[i]);
            else { g_idle[kept] = g_idle[i]; g_since[kept] = g_since[i]; g_new[kept] = g_new[i]; kept++; }
        }
        ni = kept;
    }
    return 0;
}
static OsThread g_threads[257]; static Sock g_listener;
static int start_threads(void *app, Sock listener, u32 nt) {
    lk_init(&g_q_lock); cv_init(&g_q_cv); lk_init(&g_app_lock);
    g_wake = os_wake_open();
    if (g_wake == OS_BAD_SOCK) return 0;
    g_listener = listener;
    for (u32 i = 0; i < nt; i++) { Conn *x = conn_new(app, listener); if (!x) return 0; if (!os_thread_start(&g_threads[i], conn_loop, x)) return 0; }
    if (!os_thread_start(&g_threads[256], poll_loop, &g_listener)) return 0;
    os_thread_join(&g_threads[256]);
    return 1;
}

static int has_flag(int argc, char **argv, const char *name) {
    for (int i = 1; i < argc; i++) { const char *a = argv[i]; u32 k = 0; while (name[k] && a[k] == name[k]) k++; if (!name[k] && !a[k]) return 1; }
    return 0;
}
static int flag(int argc, char **argv, const char *name, u64 *out) {
    for (int i = 1; i + 1 < argc; i++) {
        const char *a = argv[i]; u32 k = 0; while (name[k] && a[k] == name[k]) k++;
        if (!name[k] && !a[k]) { u64 v = 0; for (const char *p = argv[i + 1]; *p >= '0' && *p <= '9'; p++) v = v * 10 + (u64)(*p - '0'); *out = v; return 1; }
    }
    return 0;
}
int main(int argc, char **argv) {
    u64 port = 8080, conns = 16, budget = 1024, soft = 0, hard = 0, seed = 0, clock = 0, loop_budget = 0;
    g_timing = has_flag(argc, argv, "--timing");
    flag(argc, argv, "--port", &port); flag(argc, argv, "--conns", &conns); flag(argc, argv, "--budget", &budget);
    int marks = flag(argc, argv, "--soft", &soft); if (!flag(argc, argv, "--hard", &hard)) hard = ~0ULL >> 21;
    int seeded = flag(argc, argv, "--seed", &seed), clocked = flag(argc, argv, "--clock", &clock);
    if (!os_net_init()) { printf("error: sockets did not start\n"); return 1; }
    Sock l = os_listen((u32)port);
    if (l == OS_BAD_SOCK) { printf("error: cannot listen on port %u\n", (u32)port); return 1; }
#ifdef HTTP_SINGLE
    void *app = mito_start(budget << 20, seeded ? seed : 0);
    if (!app) { printf("error: over budget (%u MB) or out of address space\n", (u32)budget); return 1; }
    if (marks) mito_set_marks(app, soft << 20, hard << 20);
    if (clocked) mito_set_clock(app, (i64)clock, 1);
    if (flag(argc, argv, "--loop-budget", &loop_budget)) mito_set_loop_budget(app, (i64)loop_budget);
    u32 nt = conns > 256 ? 256 : conns < 1 ? 1 : (u32)conns;
    printf("listening on http://127.0.0.1:%u (one request at a time in the program, %u connection threads; Ctrl-C to stop)\n", (u32)port, nt); fflush(0);
    if (!start_threads(app, l, nt)) return 1;
#else
    (void)marks; (void)budget;
    void *app = mito_system_start(seed, seeded);
    if (!app) { printf("error: the system did not start\n"); return 1; }
    if (clocked) mito_system_set_clock(app, (i64)clock, 1);
    printf("listening on http://127.0.0.1:%u (%u connection threads)\n", (u32)port, (u32)conns); fflush(0);
    u32 nt = conns > 256 ? 256 : conns < 1 ? 1 : (u32)conns;
    if (!start_threads(app, l, nt)) return 1;
#endif
    return 0;
}
