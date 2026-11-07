
/* ---- the HTTP front end and main (runtime/http.c) ----
   Serves one program (HTTP_SINGLE) or a system of workers (HTTP_SYSTEM) on 127.0.0.1: headers up to 64 KB, a body of
   Content-Length bytes (at most 1 MB), invalid UTF-8 in the request read as U+FFFD. Connections are kept open (HTTP/1.1
   unless the client sends `Connection: close`; HTTP/1.0 only with `Connection: keep-alive`), and requests sent back to
   back on one connection are answered in order. A request that is malformed or cut short closes its connection.
   `--conns N` connection threads (16) serve connections that have a request waiting; one more thread waits on the idle
   ones (poll) and hands a connection back to the threads when its next request arrives; an idle connection is closed
   after 10 s. One program's handler runs for one request at a time (a lock); a system has its own workers.
   Sockets, threads, locks and timers are runtime/os.c's (Windows and Linux). Flags: --port N, --conns N (a system's connection threads), --budget MB, --soft MB,
   --hard MB (one program's memory), --data DIR, --db-memory MB, --db-sync full|normal and --db-fit refuse|drop (a program's tables, in SQLite), and for tests --seed N and --clock MS. */

static int g_timing; /* --timing: every answer carries X-Handler-Ns, the time the handler took (for benchmarks; off, the answers are byte for byte as recorded) */

#ifdef HTTP_SINGLE
void *mito_start(u64 budget, u64 seed);
void mito_set_clock(void *m, i64 ms, int fixed);
void mito_set_loop_budget(void *m, i64 n);
void mito_set_marks(void *m, u64 soft, u64 hard);
void mito_set_web_etag(void *m, int on);
int mito_begin(void *m);
int mito_response_binary(void *m);
int mito_response_bare(void *m);
void mito_set_files(const char *dir);
void *mito_reader(void *m, u32 k, char *why, u32 cap); int mito_wants_write(void *m); int mito_share_state(int on); int mito_db_pools(u32 n, u64 mb); int mito_set_cache(u64 bytes);
int mito_handle(void *m, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *h, u32 nh, const MitoPair *f, u32 nf, const char *body, u32 bn);
const char *mito_response(void *m, u32 *len);
const char *mito_response_head(void *m, u32 *len);
int mito_open_data(void *m, const char *dir, const char *file, u64 pool_mb, int full);
const char *mito_data_error(void *m);
int mito_has_tables(void);
int mito_tables_extern(void);
void mito_stop(void *m);
#else
void *mito_system_start(u64 seed, int seeded);
void mito_system_set_data(const char *dir, u64 pool_mb, int full);
const char *mito_system_data_error(void);
void mito_system_set_clock(void *s, i64 ms, int fixed);
int mito_system_request(void *s, const char *m, u32 mn, const char *t, u32 tn, const MitoPair *h, u32 nh, const char *body, u32 bn, u32 timeout_ms, char *out, u32 cap, u32 *outlen, char *head, u32 *headlen);
#endif


static const char *reason(int code) {
    switch (code) {
        case 200: return "OK"; case 201: return "Created"; case 204: return "No Content"; case 302: return "Found"; case 303: return "See Other"; case 304: return "Not Modified"; case 400: return "Bad Request"; case 401: return "Unauthorized";
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
        /* ASCII, 8 bytes at a time (most of a page) */
        while (i + 8 <= n) { u64 w; memcpy(&w, s + i, 8); if (w & 0x8080808080808080ULL) break; i += 8; }
        if (i >= n) break;
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
typedef struct { void *app; Sock listener; Res data, head, body, pairs; u8 *out, *gz; void *zs; u32 carry; u32 rhn; char rh[16448]; char hd[32768]; } Conn;
/* the head search stops past 64 KB (at most one more 4 KB read), the body read past Content-Length (at most 1 MB) */
#define HEAD_MAX (64 * 1024 + 4096)
#define DATA_CAP (HEAD_MAX + 4 + (1 << 20) + 4096)
#define TEXT_CAP_ (3 * DATA_CAP)
#define MAX_PAIRS (HEAD_MAX / 2 + 1)
#define OUT_CAP (RESPONSE_CAP_ + 64 * 1024) /* the program's answer (at most 4 MB) and room to spare */
#define RESPONSE_CAP_ (4 * 1024 * 1024)

static void send_all(Sock c, const char *p, u32 n) { while (n) { int k = os_send(c, (const u8 *)p, n); if (k <= 0) return; p += k; n -= (u32)k; } }
/* head and body in one call: two small sends can wait on the client's delayed acknowledgement of the first */
static void send_two(Sock c, const char *a, u32 an, const char *b, u32 bn) {
    u64 sent = os_send2(c, (const u8 *)a, an, (const u8 *)b, bn);
    if (sent < an) { send_all(c, a + sent, an - (u32)sent); send_all(c, b, bn); } else if (sent < (u64)an + bn) send_all(c, b + (sent - an), (u32)(an + bn - sent));
}
/* ---- the program's `web` block (src/web.rs): its header lines, ETags, gzip and static files ---- */
static char lower_(char ch) { return ch >= 'A' && ch <= 'Z' ? (char)(ch + 32) : ch; }
/* header lines s[0..n) (each ending CRLF): the value of the line named `name` (lowercase), and its length, or 0 */
/* where the line starting at i ends (its CR), or n */
static u32 line_end(const char *s, u32 n, u32 i) { const char *r = i < n ? (const char *)memchr(s + i, '\r', n - i) : 0; return r ? (u32)(r - s) : n; }
static const char *line_value(const char *s, u32 n, const char *name, u32 *vn) {
    u32 nl = 0; while (name[nl]) nl++;
    for (u32 i = 0; i < n;) {
        u32 e = line_end(s, n, i);
        if (e - i > nl && s[i + nl] == ':') {
            u32 k = 0; while (k < nl && lower_(s[i + k]) == name[k]) k++;
            if (k == nl) { u32 v = i + nl + 1; while (v < e && s[v] == ' ') v++; *vn = e - v; return s + v; }
        }
        i = e + 2;
    }
    return 0;
}
/* the request's header `name` (lowercase), or 0 */
static const MitoPair *req_header(const MitoPair *p, u32 np, const char *name) {
    u32 nl = 0; while (name[nl]) nl++;
    for (u32 i = 0; i < np; i++) {
        if (p[i].kn != nl) continue;
        u32 k = 0; while (k < nl && lower_(p[i].k[k]) == name[k]) k++;
        if (k == nl) return &p[i];
    }
    return 0;
}
#ifdef MITO_WEB
/* Accept-Encoding names gzip (or *) without q=0 */
static int accepts_gzip(const MitoPair *p, u32 np) {
    const MitoPair *a = req_header(p, np, "accept-encoding"); if (!a) return 0;
    for (u32 i = 0; i < a->vn;) {
        while (i < a->vn && (a->v[i] == ' ' || a->v[i] == ',')) i++;
        u32 w = i; while (i < a->vn && a->v[i] != ',' && a->v[i] != ';' && a->v[i] != ' ') i++;
        int gz = (i - w == 4 && lower_(a->v[w]) == 'g' && lower_(a->v[w + 1]) == 'z' && lower_(a->v[w + 2]) == 'i' && lower_(a->v[w + 3]) == 'p') || (i - w == 1 && a->v[w] == '*');
        /* parameters up to the next comma: q=0 (0, 0.0, 0.00, 0.000) refuses it */
        int zero = 0;
        while (i < a->vn && a->v[i] != ',') {
            if (a->v[i] == 'q' && i + 2 < a->vn && a->v[i + 1] == '=' && a->v[i + 2] == '0') {
                u32 j = i + 3; if (j < a->vn && a->v[j] == '.') { j++; while (j < a->vn && a->v[j] == '0') j++; }
                zero = j >= a->vn || a->v[j] == ',' || a->v[j] == ' ' || a->v[j] == ';';
            }
            i++;
        }
        if (gz && !zero) return 1;
    }
    return 0;
}
/* the static file whose path is the target's (up to `?`), or -1: a binary search of the sorted table */
static int static_find(const char *t, u32 tn) {
    u32 n = 0; while (n < tn && t[n] != '?' && t[n] != '#') n++;
    int lo = 0, hi = MITO_NSTATIC - 1;
    while (lo <= hi) {
        int mid = (lo + hi) / 2; const MitoStatic *s = &mito_static[mid];
        if (!s->path) return -1;
        u32 k = 0, m = s->pn < n ? s->pn : n; int d = 0;
        while (k < m && !(d = (int)(u8)s->path[k] - (int)(u8)t[k])) k++;
        if (!d) d = (int)s->pn - (int)n;
        if (!d) return mid;
        if (d < 0) lo = mid + 1; else hi = mid - 1;
    }
    return -1;
}
static char g_last_modified[48]; /* the static files' Last-Modified: when the server started (by its clock) */
/* an HTTP date (RFC 9110 IMF-fixdate) for ms since 1970 */
static void http_date(i64 ms, char *o) {
    static const char *dn[] = { "Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed" }, *mn[] = { "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec" };
    i64 s = ms / 1000, days = s / 86400, r = s % 86400; if (r < 0) { r += 86400; days--; }
    const char *wd = dn[((days % 7) + 7) % 7];
    /* civil from days (Howard Hinnant's algorithm) */
    i64 z = days + 719468, era = (z >= 0 ? z : z - 146096) / 146097; i64 doe = z - era * 146097;
    i64 yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365, y = yoe + era * 400, doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    i64 mp = (5 * doy + 2) / 153, d = doy - (153 * mp + 2) / 5 + 1, m = mp < 10 ? mp + 3 : mp - 9; if (m <= 2) y++;
    u32 n = 0; const char *p;
    for (p = wd; *p; p++) o[n++] = *p;
    o[n++] = ','; o[n++] = ' '; o[n++] = (char)('0' + d / 10); o[n++] = (char)('0' + d % 10); o[n++] = ' ';
    for (p = mn[m - 1]; *p; p++) o[n++] = *p;
    o[n++] = ' '; o[n++] = (char)('0' + y / 1000 % 10); o[n++] = (char)('0' + y / 100 % 10); o[n++] = (char)('0' + y / 10 % 10); o[n++] = (char)('0' + y % 10);
    o[n++] = ' '; o[n++] = (char)('0' + r / 36000); o[n++] = (char)('0' + r / 3600 % 10); o[n++] = ':'; o[n++] = (char)('0' + r % 3600 / 600); o[n++] = (char)('0' + r % 600 / 60);
    o[n++] = ':'; o[n++] = (char)('0' + r % 60 / 10); o[n++] = (char)('0' + r % 10); o[n++] = ' '; o[n++] = 'G'; o[n++] = 'M'; o[n++] = 'T'; o[n] = 0;
}
#ifdef MITO_WEB_GZIP
/* runtime/gzip.c with zlib (third_party/zlib), linked in: a deflate stream, and gzip `in` into `out` (room for mito_gzip_bound(n) bytes)
   with it; the length written, 0 on failure */
void *mito_gzip_new(void);
u64 mito_gzip_with(void *st, const u8 *in, u64 n, u8 *out, u64 cap);
u64 mito_gzip_bound(u64 n);
static u8 *g_static_gz[MITO_NSTATIC]; static u64 g_static_gzn[MITO_NSTATIC];
#endif
#ifdef MITO_WEB_ETAG
/* a weak ETag for an answer's bytes: W/"<32 hex digits>", two 64-bit hashes of the body */
static void body_tag(const u8 *p, u32 n, char *o) {
    u64 a = 0x9E3779B97F4A7C15ULL ^ n, b = 0xC2B2AE3D27D4EB4FULL + n;
    u32 i = 0;
    /* 32 bytes at a time in four lanes (independent, so they overlap), folded into the two */
    if (n >= 64) {
        u64 l[4] = { a, b, a ^ 0x165667B19E3779F9ULL, b ^ 0x27D4EB2F165667C5ULL };
        for (; i + 32 <= n; i += 32) for (int k = 0; k < 4; k++) { u64 w; memcpy(&w, p + i + 8 * k, 8); l[k] = (l[k] ^ w) * 0x100000001B3ULL; l[k] ^= l[k] >> 29; }
        a = l[0] ^ (l[2] * 0xFF51AFD7ED558CCDULL); b = l[1] ^ (l[3] * 0xC4CEB9FE1A85EC53ULL);
    }
    for (; i + 8 <= n; i += 8) { u64 w; memcpy(&w, p + i, 8); a = (a ^ w) * 0x100000001B3ULL; a ^= a >> 29; b = (b + w) * 0xFF51AFD7ED558CCDULL; b ^= b >> 32; }
    for (; i < n; i++) { a = (a ^ p[i]) * 0x100000001B3ULL; b = (b + p[i]) * 0xFF51AFD7ED558CCDULL; }
    a ^= a >> 33; a *= 0xC4CEB9FE1A85EC53ULL; a ^= a >> 33; b ^= b >> 31; b *= 0x94D049BB133111EBULL; b ^= b >> 29;
    static const char hx[] = "0123456789abcdef";
    o[0] = 'W'; o[1] = '/'; o[2] = '"';
    for (int k = 0; k < 16; k++) { o[3 + k] = hx[(a >> (60 - 4 * k)) & 15]; o[19 + k] = hx[(b >> (60 - 4 * k)) & 15]; }
    o[35] = '"'; o[36] = 0;
}
/* If-None-Match names the tag (weakly: W/ ignored on both sides) or is * */
static int none_match(const MitoPair *p, u32 np, const char *tag) {
    const MitoPair *h = req_header(p, np, "if-none-match"); if (!h) return 0;
    for (u32 i = 0; i < h->vn;) {
        while (i < h->vn && (h->v[i] == ' ' || h->v[i] == ',')) i++;
        u32 b = i; while (i < h->vn && h->v[i] != ',') i++;
        u32 e = i; while (e > b && h->v[e - 1] == ' ') e--;
        const char *v = h->v + b; u32 vn = e - b;
        if (vn == 1 && v[0] == '*') return 1;
        if (vn >= 2 && v[0] == 'W' && v[1] == '/') { v += 2; vn -= 2; }
        if (vn == 34) { u32 k = 0; while (k < 34 && v[k] == tag[2 + k]) k++; if (k == 34) return 1; }
    }
    return 0;
}
#endif
#endif

/* the lock around one program's handler (HTTP_SINGLE): several connection threads, one request in the program at a time */
/* The program's lock, a readers-writer one: requests that only read run at once, each on a reader instance (MITO(reader)); anything
   else runs alone on the primary instance. A writer waiting holds back new readers, so writes are not starved. */
static Lock g_rw; static Cond g_rw_cv; static u32 g_rw_readers, g_rw_writer, g_rw_waiting;
static void app_lock(void) { lk_on(&g_rw); g_rw_waiting++; while (g_rw_writer || g_rw_readers) cv_wait(&g_rw_cv, &g_rw, OS_FOREVER); g_rw_waiting--; g_rw_writer = 1; lk_off(&g_rw); }
static void app_unlock(void) { lk_on(&g_rw); g_rw_writer = 0; cv_all(&g_rw_cv); lk_off(&g_rw); }
static void read_lock(void) { lk_on(&g_rw); while (g_rw_writer || g_rw_waiting) cv_wait(&g_rw_cv, &g_rw, OS_FOREVER); g_rw_readers++; lk_off(&g_rw); }
static void read_unlock(void) { lk_on(&g_rw); if (--g_rw_readers == 0 && g_rw_waiting) cv_all(&g_rw_cv); lk_off(&g_rw); }
/* the readers, and which are free: a thread takes one for a request (waiting when all are busy) */
#define READERS_CAP 256
static void *g_readers[READERS_CAP]; static u32 g_nreaders, g_free_readers[READERS_CAP], g_nfree; static Lock g_rd_lk; static Cond g_rd_cv;
static u64 g_reruns; /* reader requests that wanted to write and ran again alone (counted under g_rd_lk) */
static void *reader_take(void) { lk_on(&g_rd_lk); while (!g_nfree) cv_wait(&g_rd_cv, &g_rd_lk, OS_FOREVER); void *r = g_readers[g_free_readers[--g_nfree]]; lk_off(&g_rd_lk); return r; }
static void reader_give(void *r, int rerun) {
    lk_on(&g_rd_lk); u32 k = 0; while (g_readers[k] != r) k++; g_free_readers[g_nfree++] = k; if (rerun) g_reruns++; cv_one(&g_rd_cv); lk_off(&g_rd_lk);
}

#ifdef MITO_SOCKETS
#ifndef HTTP_SINGLE
#error "sockets are served by one program, not yet by a system of workers"
#endif
int mito_handle(void *m, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *h, u32 nh, const MitoPair *f, u32 nf, const char *body, u32 bn);
const u8 *mito_socket_ops(void *m, u32 *len);
/* ---- sockets (WebSocket, RFC 6455): a connection upgraded by an `@socket` endpoint stays open; each text message it sends runs the
   `@socket_message` endpoint, its end `@socket_close`; what a request asks of sockets (sends, subscriptions, broadcasts) is applied here
   once it succeeds, under the program's lock, so every socket sees the same order. A socket is named by its id: generation * WS_CAP +
   slot + 1, so an id of a closed socket names no later one. */
#define WS_CAP 4096
typedef struct { Sock fd; u32 gen; int live; i64 tag; Lock wlk; char path[256]; u32 pathn;
    Res in; u32 inlen;     /* bytes read that are not yet a whole frame */
    Res msg; u32 msgn;     /* a message in fragments being put together */
    Res subs; u32 subsn;   /* subscriptions: [u32 n][stream][u32 n][before][u32 n][after] */
} Ws;
static Ws g_ws[WS_CAP]; static Lock g_ws_lk; static int g_ws_ready;
static Res g_bcast; /* a broadcast's frame being built (under the program's lock) */
static void ws_init(void) {
    lk_init(&g_ws_lk);
    for (u32 i = 0; i < WS_CAP; i++) lk_init(&g_ws[i].wlk);
    res_init(&g_bcast, (u64)RESPONSE_CAP_ + 1024 * 1024);
    g_ws_ready = 1;
}
static u32 ws_id(u32 slot) { return g_ws[slot].gen * WS_CAP + slot + 1; }
static Ws *ws_by_id(u32 id) { if (!id) return 0; u32 slot = (id - 1) % WS_CAP; Ws *w = &g_ws[slot]; return w->live && ws_id(slot) == id ? w : 0; }
/* a free slot, reserved (not yet live), or WS_CAP when all are taken */
static u32 ws_reserve(void) {
    lk_on(&g_ws_lk);
    u32 k = 0; while (k < WS_CAP && (g_ws[k].live || g_ws[k].pathn == 0xFFFFFFFFu)) k++;
    if (k < WS_CAP) { g_ws[k].pathn = 0xFFFFFFFFu; g_ws[k].gen++; }
    lk_off(&g_ws_lk);
    if (k < WS_CAP && !g_ws[k].in.p && !(res_init(&g_ws[k].in, (u64)1 << 21) && res_init(&g_ws[k].msg, (u64)1 << 21) && res_init(&g_ws[k].subs, (u64)1 << 20))) return WS_CAP;
    return k;
}
static void ws_release(u32 slot) { lk_on(&g_ws_lk); g_ws[slot].live = 0; g_ws[slot].pathn = 0; g_ws[slot].subsn = 0; g_ws[slot].inlen = 0; g_ws[slot].msgn = 0; lk_off(&g_ws_lk); }
/* one frame from the server (never masked): FIN, the opcode, the length, the payload */
static void ws_frame(Ws *w, u8 op, const u8 *a, u32 an, const u8 *b, u32 bn, const u8 *c3, u32 cn) {
    u64 n = (u64)an + bn + cn; u8 h[10]; u32 hn = 2; h[0] = (u8)(0x80 | op);
    if (n < 126) h[1] = (u8)n; else if (n < 65536) { h[1] = 126; h[2] = (u8)(n >> 8); h[3] = (u8)n; hn = 4; }
    else { h[1] = 127; for (int k = 0; k < 8; k++) h[2 + k] = (u8)(n >> (56 - 8 * k)); hn = 10; }
    lk_on(&w->wlk);
    send_all(w->fd, (const char *)h, hn); if (an) send_all(w->fd, (const char *)a, an); if (bn) send_all(w->fd, (const char *)b, bn); if (cn) send_all(w->fd, (const char *)c3, cn);
    lk_off(&w->wlk);
}
static u32 rd32(const u8 *p) { u32 v; memcpy(&v, p, 4); return v; }
/* apply what a request asked of sockets (MITO(socket_ops)); `self` is the socket the request came from (its close is the reader's) */
static void ws_apply(void *app) {
    u32 len; const u8 *p = mito_socket_ops(app, &len), *e = p + len;
    while (p + 14 <= e) {
        u8 op = p[0]; u32 id = rd32(p + 1); i64 iv; memcpy(&iv, p + 5, 8); u32 np = p[13]; p += 14;
        const u8 *part[3] = { 0, 0, 0 }; u32 pn[3] = { 0, 0, 0 };
        for (u32 k = 0; k < np && k < 3; k++) { pn[k] = rd32(p); part[k] = p + 4; p += 4 + pn[k]; }
        Ws *w = (op == 5 || op == 6) ? 0 : ws_by_id(id);
        switch (op) {
            case 1: if (w) ws_frame(w, 1, part[0], pn[0], 0, 0, 0, 0); break;
            case 2: if (w) { u8 code[2] = { 0x03, 0xE8 }; ws_frame(w, 8, code, 2, 0, 0, 0, 0); os_shutdown(w->fd); } break; /* 1000: normal */
            case 3: if (w) { u64 need = (u64)w->subsn + 12 + pn[0] + pn[1] + pn[2];
                        if (res_ensure(&w->subs, need)) { u8 *q = w->subs.p + w->subsn; for (u32 k = 0; k < 3; k++) { memcpy(q, &pn[k], 4); memcpy(q + 4, part[k], pn[k]); q += 4 + pn[k]; } w->subsn = (u32)need; } }
                    break;
            case 4: if (w) { /* the subscriptions whose `before` is this */
                        u32 i = 0;
                        while (i < w->subsn) {
                            u8 *q = w->subs.p + i; u32 a0 = rd32(q), b0 = rd32(q + 4 + a0), c0 = rd32(q + 8 + a0 + b0), sz = 12 + a0 + b0 + c0;
                            if (b0 == pn[0] && !memcmp(q + 8 + a0, part[0], b0)) { memmove(q, q + sz, w->subsn - i - sz); w->subsn -= sz; } else i += sz;
                        } }
                    break;
            case 5: case 6: /* a broadcast to a stream's subscribers (each its own before and after), or a text to every socket */
                for (u32 k = 0; k < WS_CAP; k++) {
                    Ws *x = &g_ws[k]; if (!x->live) continue;
                    if (op == 6) { ws_frame(x, 1, part[0], pn[0], 0, 0, 0, 0); continue; }
                    for (u32 i = 0; i < x->subsn;) {
                        u8 *q = x->subs.p + i; u32 a0 = rd32(q), b0 = rd32(q + 4 + a0), c0 = rd32(q + 8 + a0 + b0);
                        if (a0 == pn[0] && !memcmp(q + 4, part[0], a0)) ws_frame(x, 1, q + 8 + a0, b0, part[1], pn[1], q + 12 + a0 + b0, c0);
                        i += 12 + a0 + b0 + c0;
                    }
                }
                break;
            case 7: if (w) w->tag = iv; break;
        }
    }
}
/* run a socket endpoint (SOCKET, SOCKETMSG, SOCKETCLOSE) for socket `slot`: its id, tag and the message as the headers :socket, :tag,
   :message beside the request's own (an upgrade's); what it asks of sockets applied when it succeeds. The answer's status. */
static int ws_call(void *app, const char *method, u32 slot, const char *path, u32 pathn, MitoPair *pairs, u32 np, const u8 *msg, u32 msgn) {
    char ids[24], tags[24]; u32 idn = 0, tagn = 0; u64 v = ws_id(slot); i64 tg = g_ws[slot].tag;
    { char r[24]; int k = 0; do { r[k++] = (char)('0' + v % 10); v /= 10; } while (v); while (k) ids[idn++] = r[--k]; }
    { u64 t = tg < 0 ? (u64)0 - (u64)tg : (u64)tg; char r[24]; int k = 0; do { r[k++] = (char)('0' + t % 10); t /= 10; } while (t); if (tg < 0) tags[tagn++] = '-'; while (k) tags[tagn++] = r[--k]; }
    pairs[np].k = ":socket"; pairs[np].kn = 7; pairs[np].v = ids; pairs[np].vn = idn;
    pairs[np + 1].k = ":tag"; pairs[np + 1].kn = 4; pairs[np + 1].v = tags; pairs[np + 1].vn = tagn;
    pairs[np + 2].k = ":message"; pairs[np + 2].kn = 8; pairs[np + 2].v = (const char *)msg; pairs[np + 2].vn = msgn;
    u32 mn = 0; while (method[mn]) mn++;
    app_lock();
    int code = mito_handle(app, method, mn, path, pathn, pairs, np + 3, 0, 0, "", 0);
    if (code < 300) ws_apply(app);
    app_unlock();
    return code;
}
static const char ws_b64[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/* An upgrade request (GET with Upgrade: websocket): the program's `@socket` endpoint decides; accepted, the 101 answer (Sec-WebSocket-Accept:
   base64 of SHA-1 of the key and RFC 6455's GUID, and the program's own lines such as Sec-WebSocket-Protocol) and the socket is live: 2.
   Refused, its answer and the connection closed: 0. */
static int ws_upgrade(Conn *x, Sock c, const char *target, u32 tn, u32 np, const MitoPair *key) {
    if (!g_ws_ready) ws_init();
    u32 slot = ws_reserve();
    const char *full = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    if (slot == WS_CAP || !res_ensure(&x->pairs, (u64)(np + 3) * sizeof(MitoPair))) { u32 n = 0; while (full[n]) n++; send_all(c, full, n); return 0; }
    u32 pn = 0; while (pn < tn && target[pn] != '?' && pn < sizeof g_ws[slot].path) pn++;
    memcpy(g_ws[slot].path, target, pn);
    g_ws[slot].tag = 0;
    MitoPair *pairs = (MitoPair *)x->pairs.p;
    int code = ws_call(x->app, "SOCKET", slot, target, tn, pairs, np, (const u8 *)"", 0);
    char *hd = x->hd; u32 n = 0;
    #define WPUT(s) do { const char *s_ = (s); while (*s_ && n < sizeof x->hd) hd[n++] = *s_++; } while (0)
    if (code >= 300) {
        u32 rn; const char *resp = mito_response(x->app, &rn);
        WPUT("HTTP/1.1 "); { char d[8]; int k = 0; u32 v = (u32)code; do { d[k++] = (char)('0' + v % 10); v /= 10; } while (v); while (k) hd[n++] = d[--k]; }
        WPUT(" "); WPUT(reason(code)); WPUT("\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: ");
        { char d[16]; int k = 0; u32 v = rn; do { d[k++] = (char)('0' + v % 10); v /= 10; } while (v); while (k) hd[n++] = d[--k]; }
        WPUT("\r\n\r\n"); send_two(c, hd, n, resp, rn);
        ws_release(slot); return 0;
    }
    /* the accept key */
    char kb[128]; u32 kn = key->vn < 64 ? key->vn : 64; memcpy(kb, key->v, kn); memcpy(kb + kn, "258EAFA5-E914-47DA-95CA-C5AB0DC85B11", 36); kn += 36;
    u8 dg[20]; char acc[32]; u32 an = 0;
    if (!cr_sha(1, (const u8 *)kb, kn, dg)) { ws_release(slot); return 0; }
    for (u32 i = 0; i < 21; i += 3) { u32 v = (u32)dg[i] << 16 | (i + 1 < 20 ? (u32)dg[i + 1] << 8 : 0) | (i + 2 < 20 ? dg[i + 2] : 0);
        acc[an++] = ws_b64[v >> 18]; acc[an++] = ws_b64[v >> 12 & 63]; acc[an++] = i + 1 < 20 ? ws_b64[v >> 6 & 63] : '='; acc[an++] = i + 2 < 20 ? ws_b64[v & 63] : '='; }
    acc[an] = 0;
    WPUT("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: "); WPUT(acc); WPUT("\r\n");
    /* the program's lines, but its content type (an upgrade has no body) */
    { u32 hn; const char *h = mito_response_head(x->app, &hn);
      for (u32 i = 0; i < hn;) { u32 e = i; while (e < hn && h[e] != '\r') e++; int ct = e - i > 13 && lower_(h[i]) == 'c' && lower_(h[i + 8]) == 't' && h[i + 12] == ':'; if (!ct) for (u32 k = i; k < e + 2 && k < hn && n < sizeof x->hd; k++) hd[n++] = h[k]; i = e + 2; } }
    WPUT("\r\n");
    #undef WPUT
    /* live before what the endpoint asked is applied (its welcome goes out after the 101) */
    app_lock();
    g_ws[slot].fd = c; g_ws[slot].pathn = pn; g_ws[slot].inlen = 0; g_ws[slot].msgn = 0;
    send_all(c, hd, n);
    g_ws[slot].live = 1;
    ws_apply(x->app);
    app_unlock();
    return 2;
}
/* A socket the poller found readable: its bytes read, each whole frame handled (a text message to the program, a ping answered, a close
   ended). 1 while it stays open, 0 when it ended (the program told, the slot freed, the connection closed). */
static int ws_read(Conn *x, u32 slot) {
    Ws *w = &g_ws[slot];
    if (!res_ensure(&w->in, (u64)w->inlen + 65536) || !res_ensure(&x->pairs, 3 * (u64)sizeof(MitoPair))) return 0; /* room for :socket, :tag, :message */
    int k = os_recv(w->fd, w->in.p + w->inlen, 65536);
    int ended = k <= 0;
    if (k > 0) w->inlen += (u32)k;
    u32 at = 0;
    while (!ended) {
        u8 *f = w->in.p + at; u32 have = w->inlen - at;
        if (have < 2) break;
        int fin = f[0] & 0x80, op = f[0] & 0x0F, masked = f[1] & 0x80; u64 n = f[1] & 0x7F; u32 h = 2;
        if (n == 126) { if (have < 4) break; n = (u64)f[2] << 8 | f[3]; h = 4; }
        else if (n == 127) { if (have < 10) break; n = 0; for (int i = 0; i < 8; i++) n = n << 8 | f[2 + i]; h = 10; }
        if (!masked || n > (1u << 20)) { ended = 1; break; } /* a client's frames are masked, and at most 1 MB */
        if (have < h + 4 + n) break;
        u8 *mk = f + h, *data = f + h + 4;
        for (u64 i = 0; i < n; i++) data[i] ^= mk[i & 3];
        at += h + 4 + (u32)n;
        if (op == 8) { ws_frame(w, 8, data, n >= 2 ? 2 : 0, 0, 0, 0, 0); ended = 1; break; }
        if (op == 9) { ws_frame(w, 10, data, (u32)n, 0, 0, 0, 0); continue; }
        if (op == 10) continue;
        if (op == 1 || op == 2 || op == 0) {
            if (op != 0) w->msgn = 0;
            if (!res_ensure(&w->msg, (u64)w->msgn + n) || (u64)w->msgn + n > (1u << 20)) { ended = 1; break; }
            memcpy(w->msg.p + w->msgn, data, n); w->msgn += (u32)n;
            if (fin) { ws_call(x->app, "SOCKETMSG", slot, w->path, w->pathn, (MitoPair *)x->pairs.p, 0, w->msg.p, w->msgn); w->msgn = 0; }
        }
    }
    if (at) { memmove(w->in.p, w->in.p + at, w->inlen - at); w->inlen -= at; }
    if (ended) {
        ws_call(x->app, "SOCKETCLOSE", slot, w->path, w->pathn, (MitoPair *)x->pairs.p, 0, (const u8 *)"", 0);
        app_lock(); os_close(w->fd); ws_release(slot); app_unlock();
        return 0;
    }
    return 1;
}
#endif
#ifdef MITO_EVERY_MS
/* the program's `@every` function, on its clock, under the program's lock (with what it asks of sockets) */
static u32 every_loop(void *app) {
    Lock lk; Cond cv; lk_init(&lk); cv_init(&cv);
    for (;;) {
        lk_on(&lk); cv_wait(&cv, &lk, MITO_EVERY_MS); lk_off(&lk);
        app_lock();
        int code = mito_handle(app, "EVERY", 5, "/", 1, 0, 0, 0, 0, "", 0);
#ifdef MITO_SOCKETS
        if (code < 300 && g_ws_ready) ws_apply(app);
#endif
        (void)code;
        app_unlock();
    }
    return 0;
}
#endif

/* Read one request from connection c and answer it. `first`: the connection's first request (an empty one is answered 400;
   later, the client closing the connection between requests ends it quietly). Returns 1 when the connection stays open. */
#ifdef HTTP_SINGLE
/* the answer an instance has just made, copied to the connection (the instance's response area is reused by the next request, which
   may run while this one is sent): its body (a text body that is not UTF-8 is sent empty; Response<bytes> as it is) and head lines */
static int take_answer(Conn *x, void *app, u32 *rn) {
    const char *resp = mito_response(app, rn);
    int bare = mito_response_bare(app);
    if (!mito_response_binary(app) && !utf8_valid((const u8 *)resp, *rn)) *rn = 0;
    if (*rn > OUT_CAP) *rn = 0;
    memcpy(x->out, resp, *rn);
    { const char *h = mito_response_head(app, &x->rhn); if (x->rhn > sizeof x->rh) x->rhn = 0; memcpy(x->rh, h, x->rhn); }
    return bare;
}
#endif
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
    /* HEAD: answered as GET, with the same head and no body */
    int head_only = mn == 4 && method[0] == 'H' && method[1] == 'E' && method[2] == 'A' && method[3] == 'D';
    if (head_only) { method = "GET"; mn = 3; }
    /* the program's own requests (its start, its clock, its sockets' messages) never come from the network */
    { static const char *const inner[] = { "START", "EVERY", "SOCKET", "SOCKETMSG", "SOCKETCLOSE" };
      for (u32 k = 0; k < 5; k++) { u32 q = 0; while (inner[k][q] && q < mn && method[q] == inner[k][q]) q++; if (!inner[k][q] && q == mn) {
          const char *r = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"; u32 rn = 0; while (r[rn]) rn++; send_all(c, r, rn); return 0; } } }
#ifdef MITO_SOCKETS
    /* a WebSocket upgrade: the program's `@socket` endpoint for the path decides */
    if (mn == 3 && method[0] == 'G' && !head_only) {
        const MitoPair *up = req_header((const MitoPair *)x->pairs.p, np, "upgrade"), *key = req_header((const MitoPair *)x->pairs.p, np, "sec-websocket-key");
        if (up && key && up->vn == 9) {
            u32 q = 0; while (q < 9 && lower_(up->v[q]) == "websocket"[q]) q++;
            if (q == 9) { (void)rest; return ws_upgrade(x, c, target, tn, np, key); }
        }
    }
#endif
    char *hd = x->hd; u32 n = 0; char num[24]; int dn;
    #define HPUT(s) do { const char *s_ = (s); while (*s_ && n < sizeof x->hd) hd[n++] = *s_++; } while (0)
    #define HPUTN(s, k) do { const char *s_ = (s); u32 k_ = (k); for (u32 i_ = 0; i_ < k_ && n < sizeof x->hd; i_++) hd[n++] = s_[i_]; } while (0)
    #define HNUM(x_) do { u64 v_ = (u64)(x_); dn = 0; do { num[dn++] = (char)('0' + v_ % 10); v_ /= 10; } while (v_); while (dn && n < sizeof x->hd) hd[n++] = num[--dn]; } while (0)
#ifdef MITO_WEB
    /* a static file: answered before the endpoints, for GET and HEAD, whatever the query */
    if (mn == 3 && method[0] == 'G' && method[1] == 'E' && method[2] == 'T') {
        int si = static_find(target, tn);
        if (si >= 0) {
            const MitoStatic *s = &mito_static[si];
            const char *bp = (const char *)s->data; u64 bn = s->len; int gz = 0;
#ifdef MITO_WEB_GZIP
            if (g_static_gz[si] && accepts_gzip((MitoPair *)x->pairs.p, np)) { bp = (const char *)g_static_gz[si]; bn = g_static_gzn[si]; gz = 1; }
#endif
            HPUT("HTTP/1.1 200 OK\r\nContent-Type: "); HPUT(s->type); HPUT("\r\n"); HPUT(s->headers);
            HPUT("Last-Modified: "); HPUT(g_last_modified); HPUT("\r\n");
#ifdef MITO_WEB_GZIP
            HPUT("Vary: Accept-Encoding\r\n");
#endif
            if (gz) HPUT("Content-Encoding: gzip\r\n");
            HPUT("Content-Length: "); HNUM(bn);
            HPUT(keep ? "\r\nConnection: keep-alive\r\n\r\n" : "\r\nConnection: close\r\n\r\n");
            send_two(c, hd, n, bp, head_only ? 0 : (u32)bn);
            if (keep && rest) { memmove(x->data.p, x->data.p + be, rest); x->carry = rest; }
            return keep;
        }
    }
#endif
    if (g_timing) t_start = os_perf_ns();
#ifdef HTTP_SINGLE
    int bare = 0, done = 0;
    if (g_nreaders && ((mn == 3 && method[0] == 'G' && method[1] == 'E' && method[2] == 'T') || (mn == 4 && method[0] == 'H' && method[1] == 'E' && method[2] == 'A' && method[3] == 'D'))) {
        void *rd = reader_take();
        read_lock();
        code = mito_handle(rd, method, mn, target, tn, (MitoPair *)x->pairs.p, np, 0, 0, (const char *)body.p, body.len);
        int rerun = mito_wants_write(rd);
        if (!rerun) { bare = take_answer(x, rd, &rn); done = 1; }
        read_unlock();
        reader_give(rd, rerun);
    }
    if (!done) {
        app_lock();
        code = mito_handle(x->app, method, mn, target, tn, (MitoPair *)x->pairs.p, np, 0, 0, (const char *)body.p, body.len);
        bare = take_answer(x, x->app, &rn);
        app_unlock();
    }
    resp = (const char *)x->out;
#else
    code = mito_system_request(x->app, method, mn, target, tn, (MitoPair *)x->pairs.p, np, (const char *)body.p, body.len, 10000, (char *)x->out, OUT_CAP, &rn, x->rh, &x->rhn);
    resp = (const char *)x->out;
    int bare = 0;
#endif
    if (g_timing) t_end = os_perf_ns();
#ifndef MITO_WEB
    HPUT("HTTP/1.1 "); HNUM(code); HPUT(" "); HPUT(reason(code)); HPUT("\r\n");
    HPUTN(x->rh, x->rhn); /* Content-Type and any other lines, each ending CRLF */
    /* every answer: the browser takes the content type as given; a page: scripts, styles and form targets only from this site */
    HPUT("X-Content-Type-Options: nosniff\r\n");
    { static const char html[] = "Content-Type: text/html"; u32 k = 0; while (html[k] && k < x->rhn && x->rh[k] == html[k]) k++;
      if (!html[k]) HPUT("Content-Security-Policy: default-src 'self'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; base-uri 'none'\r\nReferrer-Policy: same-origin\r\n"); }
#else
    {
        /* the web block's answer: the program's lines, the block's lines it did not give, an ETag, Vary and gzip */
        u32 vn; const MitoPair *pairs = (const MitoPair *)x->pairs.p;
        int get = mn == 3 && method[0] == 'G';
#ifdef MITO_WEB_ETAG
        char tag[40]; int tagged = 0;
        if (code == 200 && rn > 0 && get && !line_value(x->rh, x->rhn, "etag", &vn)) {
            body_tag((const u8 *)resp, rn, tag); tagged = 1;
            if (none_match(pairs, np, tag)) { code = 304; rn = 0; }
        }
#endif
        HPUT("HTTP/1.1 "); HNUM(code); HPUT(" "); HPUT(reason(code)); HPUT("\r\n");
        /* the program's lines, but Vary (merged below) and, on a 304, the content type */
        for (u32 i = 0; i < x->rhn;) {
            u32 e = line_end(x->rh, x->rhn, i);
            int vary = e - i > 5 && lower_(x->rh[i]) == 'v' && lower_(x->rh[i + 1]) == 'a' && lower_(x->rh[i + 2]) == 'r' && lower_(x->rh[i + 3]) == 'y' && x->rh[i + 4] == ':';
            int ctype = code == 304 && e - i > 13 && lower_(x->rh[i]) == 'c' && lower_(x->rh[i + 8]) == 't' && x->rh[i + 12] == ':';
            if (!vary && !ctype) HPUTN(x->rh + i, e - i + 2);
            i = e + 2;
        }
#ifdef MITO_WEB_HEADERS
        /* the block's lines whose names the program did not give (Vary merged below; Cache-Control left to the ETag's when tagged) */
        { const char *w = mito_web_headers; u32 wn = sizeof mito_web_headers - 1;
          for (u32 i = 0; i < wn;) {
              u32 e = i; while (e < wn && w[e] != '\r') e++;
              u32 col = i; while (col < e && w[col] != ':') col++;
              char name[72]; u32 nl = col - i < 71 ? col - i : 71; for (u32 k = 0; k < nl; k++) name[k] = lower_(w[i + k]); name[nl] = 0;
              int skip = line_value(x->rh, x->rhn, name, &vn) != 0 || (nl == 4 && name[0] == 'v' && name[1] == 'a' && name[2] == 'r' && name[3] == 'y');
#ifdef MITO_WEB_ETAG
              { static const char cc[] = "cache-control"; u32 k = 0; while (k < 14 && name[k] == cc[k]) k++; if (tagged && mito_web_etag_cc[0] && k == 14) skip = 1; }
#endif
              if (!skip && !bare) HPUTN(w + i, e - i + 2);
              i = e + 2;
          }
        }
#endif
#ifdef MITO_WEB_ETAG
        if (tagged) {
            HPUT("ETag: "); HPUT(tag); HPUT("\r\n");
            if (mito_web_etag_cc[0] && !line_value(x->rh, x->rhn, "cache-control", &vn)) { HPUT("Cache-Control: "); HPUT(mito_web_etag_cc); HPUT("\r\n"); }
        }
#endif
        /* Vary: the program's, else the block's, with Accept-Encoding added when a body may be compressed */
        { const char *v = line_value(x->rh, x->rhn, "vary", &vn);
#ifdef MITO_WEB_HEADERS
          if (!v && !bare) v = line_value(mito_web_headers, sizeof mito_web_headers - 1, "vary", &vn);
#endif
          int ae = 0;
#ifdef MITO_WEB_GZIP
          ae = code != 204 && code != 304; /* as Thruster: every answer that may have a body, an empty one too */
#endif
          if (v || ae) {
              HPUT("Vary: "); if (v) HPUTN(v, vn);
              if (ae) { if (v && vn) HPUT(","); HPUT("Accept-Encoding"); }
              HPUT("\r\n");
          }
        }
#ifdef MITO_WEB_WRITES
        /* a request that changes state: the block's `writes` lines, as they are (a proxy's lines on answers it does not cache) */
        if (!get && !bare) HPUT(mito_web_writes);
#endif
#ifdef MITO_WEB_GZIP
        if (code != 204 && code != 304 && accepts_gzip(pairs, np)) { /* as Thruster: every answer that may have a body, an empty one too */
            u64 zn = mito_gzip_with(x->zs, (const u8 *)resp, rn, x->gz, mito_gzip_bound(OUT_CAP));
            if (zn) { resp = (const char *)x->gz; rn = (u32)zn; HPUT("Content-Encoding: gzip\r\n"); }
        }
#endif
        (void)get; (void)vn;
    }
#endif
    HPUT("Content-Length: "); HNUM(rn);
    if (g_timing) { HPUT("\r\nX-Handler-Ns: "); HNUM(t_end - t_start); }
    HPUT(keep ? "\r\nConnection: keep-alive\r\n\r\n" : "\r\nConnection: close\r\n\r\n");
    send_two(c, hd, n, resp, head_only ? 0 : rn);
    #undef HPUT
    #undef HPUTN
    #undef HNUM
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
#ifdef MITO_WEB_GZIP
    x->gz = (u8 *)os_alloc(mito_gzip_bound(OUT_CAP)); x->zs = mito_gzip_new(); /* a compressed answer, and the stream that writes it */
    if (!x->gz || !x->zs) return 0;
#endif
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
    if (!ok) os_close(c >> 14);
}
static u64 ready_pop(void) {
    lk_on(&g_q_lock);
    while (!g_ready_len) cv_wait(&g_q_cv, &g_q_lock, OS_FOREVER);
    u64 c = g_ready[g_ready_head]; g_ready_head = (g_ready_head + 1) % READY_CAP; g_ready_len--;
    lk_off(&g_q_lock);
    return c;
}
static u32 g_back_ws[IDLE_CAP]; /* a socket's slot + 1, 0 for an HTTP connection */
static void give_back_ws(Sock c, u32 ws) {
    lk_on(&g_q_lock);
    int ok = g_back_len < IDLE_CAP; if (ok) { g_back_ws[g_back_len] = ws; g_back[g_back_len++] = c; }
    lk_off(&g_q_lock);
    if (!ok) { os_close(c); return; }
    os_wake_ping();
}
static void give_back(Sock c) { give_back_ws(c, 0); }
/* a connection thread: answer the requests waiting on a connection (back to back ones too), then give it to the poller */
static u32 conn_loop(void *arg) {
    Conn *x = (Conn *)arg;
    for (;;) {
        u64 rc = ready_pop();
        Sock c = rc >> 14; int first = (int)(rc & 1); u32 wslot = (u32)(rc >> 1 & 0x1FFF); /* a new connection's first request; a socket's slot + 1 */
        x->carry = 0;
#ifdef MITO_SOCKETS
        if (wslot) { if (ws_read(x, wslot - 1)) give_back_ws(c, wslot); continue; }
#endif
        (void)wslot;
        int keep = serve_one(x, c, first);
#ifdef MITO_SOCKETS
        if (keep == 2) { u32 sl = 0; while (sl < WS_CAP && !(g_ws[sl].live && g_ws[sl].fd == c)) sl++; if (sl < WS_CAP) give_back_ws(c, sl + 1); else os_close(c); continue; }
#endif
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
static Sock g_idle[IDLE_CAP]; static u64 g_since[IDLE_CAP]; static u8 g_new[IDLE_CAP]; static u32 g_iws[IDLE_CAP]; static OsPoll g_fds[IDLE_CAP + 2];
static u32 poll_loop(void *arg) {
    Sock listener = *(Sock *)arg; u32 ni = 0;
    for (;;) {
        lk_on(&g_q_lock);
        u64 now = os_ticks_ms();
        for (u32 i = 0; i < g_back_len; i++) { if (ni < IDLE_CAP) { g_idle[ni] = g_back[i]; g_since[ni] = now; g_new[ni] = 0; g_iws[ni] = g_back_ws[i]; ni++; } else os_close(g_back[i]); }
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
            if (c != OS_BAD_SOCK) { if (ni < IDLE_CAP) { g_idle[ni] = c; g_since[ni] = os_ticks_ms(); g_new[ni] = 1; g_iws[ni] = 0; ni++; } else os_close(c); }
        }
        now = os_ticks_ms();
        u32 kept = 0;
        for (u32 i = 0; i < ni; i++) {
            short ev = i < polled ? g_fds[i + 2].revents : 0;
            /* a request (or the client closed: the thread finds out); + 1 marks a first request; a socket carries its slot and never idles out */
            if (ev) ready_push(g_idle[i] << 14 | (u64)g_iws[i] << 1 | g_new[i]);
            else if (!g_iws[i] && now - g_since[i] > IDLE_MS) os_close(g_idle[i]);
            else { g_idle[kept] = g_idle[i]; g_since[kept] = g_since[i]; g_new[kept] = g_new[i]; g_iws[kept] = g_iws[i]; kept++; }
        }
        ni = kept;
    }
    return 0;
}
static OsThread g_threads[257]; static Sock g_listener;
static int start_threads(void *app, Sock listener, u32 nt) {
    lk_init(&g_q_lock); cv_init(&g_q_cv); lk_init(&g_rw); cv_init(&g_rw_cv); lk_init(&g_rd_lk); cv_init(&g_rd_cv);
    g_wake = os_wake_open();
    if (g_wake == OS_BAD_SOCK) return 0;
    g_listener = listener;
    for (u32 i = 0; i < nt; i++) { Conn *x = conn_new(app, listener); if (!x) return 0; if (!os_thread_start(&g_threads[i], conn_loop, x)) return 0; }
#ifdef MITO_EVERY_MS
    { static OsThread every; if (!os_thread_start(&every, every_loop, app)) return 0; }
#endif
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
/* a flag's text value (`--data ./state`), or `def` */
static const char *sflag(int argc, char **argv, const char *name, const char *def) {
    for (int i = 1; i + 1 < argc; i++) { const char *a = argv[i]; u32 k = 0; while (name[k] && a[k] == name[k]) k++; if (!name[k] && !a[k]) return argv[i + 1]; }
    return def;
}
static int same(const char *a, const char *b) { while (*a && *a == *b) { a++; b++; } return *a == *b; }
/* the tables' flags: --db-sync full (default: an answered change is on disk) or normal (synced at checkpoints) gives 1 or 0; --db-fit refuse
   (default: a database that does not fit the program stops the server) or drop (drop what does not fit, saying each) adds 2; -1 for anything else */
static int db_sync(int argc, char **argv) {
    const char *v = sflag(argc, argv, "--db-sync", "full"), *f = sflag(argc, argv, "--db-fit", "refuse");
    int s = same(v, "full") ? 1 : same(v, "normal") ? 0 : -1, d = same(f, "refuse") ? 0 : same(f, "drop") ? 2 : -1;
    return s < 0 || d < 0 ? -1 : s | d;
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
    /* --workers N: requests that only read run on N readers at once; 1, one request at a time. Default: twice the CPUs this process may
       use (a connection thread holding a reader is sometimes descheduled, doing its I/O, while another waits for one: as many readers as
       CPUs left a fifth of them idle) */
    u64 workers = 0; int asked = flag(argc, argv, "--workers", &workers);
    if (!asked) { workers = os_cpu_count(); if (workers > 1) workers *= 2; if (workers > conns) workers = conns; } /* no more than threads to use them */
    if (workers > READERS_CAP) workers = READERS_CAP;
    if (workers > 1 && marks) { if (asked) { printf("error: --workers above 1 needs all of the state committed at start, which --soft/--hard marks turn off\n"); return 1; } workers = 1; }
    if (workers > 1 && !mito_share_state(1)) { if (asked) { printf("error: --workers above 1: this system cannot share the state with readers (Linux only for now)\n"); return 1; } workers = 1; }
    /* --cache-mb N: the answer cache (cache_get / cache_put) holds N MB, shared by the program's instances (default 64; 0: none) */
    { u64 cmb = 64; flag(argc, argv, "--cache-mb", &cmb); if (!mito_set_cache(cmb << 20)) { printf("error: no memory for a %u MB answer cache\n", (u32)cmb); return 1; } }
    void *app = mito_start(budget << 20, seeded ? seed : 0);
    if (!app) { printf("error: over budget (%u MB) or out of address space\n", (u32)budget); return 1; }
    if (marks) mito_set_marks(app, soft << 20, hard << 20);
    if (clocked) mito_set_clock(app, (i64)clock, 1);
    if (flag(argc, argv, "--loop-budget", &loop_budget)) mito_set_loop_budget(app, (i64)loop_budget);
    /* --files DIR: the directory read_stored reads (an app's stored files) */
    { const char *fd = sflag(argc, argv, "--files", 0); if (fd) mito_set_files(fd); }
#ifdef MITO_WEB
    /* the web block: static files dated when the server starts (by its clock), every answer tagged, static files compressed once */
    http_date(clocked ? (i64)clock : os_now_ms(), g_last_modified);
#ifdef MITO_WEB_ETAG
    mito_set_web_etag(app, 1);
#endif
#ifdef MITO_WEB_GZIP
    void *zs = mito_gzip_new();
    if (!zs) { printf("error: no memory for gzip\n"); return 1; }
    for (u32 i = 0; i < MITO_NSTATIC; i++) {
        if (!mito_static[i].path) continue;
        u64 cap = mito_gzip_bound(mito_static[i].len); u8 *b = (u8 *)os_alloc(cap);
        if (!b) { printf("error: no memory to compress the static files\n"); return 1; }
        g_static_gzn[i] = mito_gzip_with(zs, (const u8 *)mito_static[i].data, mito_static[i].len, b, cap);
        g_static_gz[i] = g_static_gzn[i] ? b : 0;
    }
#endif
#endif
    /* the program's tables: in --data (default ./data), data.db, SQLite's memory --db-memory MB (default 64); a database that does not fit
       the program stops the server here, saying why */
    if (mito_has_tables()) {
        u64 dbmem = 64; flag(argc, argv, "--db-memory", &dbmem);
        /* connections at once: SQLite's memory as a pool of --db-memory MB for each (see store.c) */
        if (workers > 1 && !mito_db_pools((u32)workers + 1, dbmem)) { printf("error: no address space for %u SQLite pools of %u MB\n", (u32)workers + 1, (u32)dbmem); mito_stop(app); return 1; }
        const char *dir = sflag(argc, argv, "--data", "data");
        int full = db_sync(argc, argv);
        if (full < 0) { printf("error: --db-sync is full or normal, --db-fit refuse or drop\n"); mito_stop(app); return 1; }
        if (mito_tables_extern()) {
            /* another program's database (its tables are `extern`): named by --db, checked against the program, never changed in shape */
            const char *db = sflag(argc, argv, "--db", 0);
            if (!db) { printf("error: the program's tables are another program's (`extern`): name its database with --db FILE\n"); mito_stop(app); return 1; }
            if (!mito_open_data(app, "", db, dbmem, full)) { printf("error: %s\n", mito_data_error(app)); mito_stop(app); return 1; }
            printf("tables in %s\n", db);
        } else {
            if (!mito_open_data(app, dir, "data.db", dbmem, full)) { printf("error: %s\n", mito_data_error(app)); mito_stop(app); return 1; }
            printf("tables in %s/data.db\n", dir);
        }
    }
    /* the program's `@start` function, once, before any request: a failure stops the server, saying why */
    { int sc = mito_begin(app);
      if (sc) { u32 en; const char *e = mito_response(app, &en); printf("error: the @start function failed (%d): %.*s\n", sc, (int)en, e); mito_stop(app); return 1; } }
    u32 nt = conns > 256 ? 256 : conns < 1 ? 1 : (u32)conns;
    if (workers > 1) {
        char why[512] = { 0 };
        for (u32 k = 0; k < (u32)workers; k++) {
            void *r = mito_reader(app, k, why, sizeof why);
            if (!r) {
                if (asked || k > 0) { printf("error: reader %u: %s\n", k, why[0] ? why : "out of memory"); mito_stop(app); return 1; }
                printf("note: requests run one at a time (%s)\n", why); g_nreaders = 0; break;
            }
            g_readers[k] = r; g_free_readers[k] = k; g_nreaders = g_nfree = k + 1;
        }
    }
    if (g_nreaders) printf("listening on http://127.0.0.1:%u (reads on %u readers at once, other requests one at a time; %u connection threads; Ctrl-C to stop)\n", (u32)port, g_nreaders, nt);
    else printf("listening on http://127.0.0.1:%u (one request at a time in the program, %u connection threads; Ctrl-C to stop)\n", (u32)port, nt);
    fflush(0);
    if (!start_threads(app, l, nt)) return 1;
#else
    (void)marks; (void)budget;
    { u64 dbmem = 64; flag(argc, argv, "--db-memory", &dbmem); int full = db_sync(argc, argv);
      if (full < 0) { printf("error: --db-sync is full or normal, --db-fit refuse or drop\n"); return 1; }
      mito_system_set_data(sflag(argc, argv, "--data", "data"), dbmem, full); }
    void *app = mito_system_start(seed, seeded);
    if (!app) { const char *e = mito_system_data_error(); printf("error: %s\n", e[0] ? e : "the system did not start"); return 1; }
    if (clocked) mito_system_set_clock(app, (i64)clock, 1);
    printf("listening on http://127.0.0.1:%u (%u connection threads)\n", (u32)port, (u32)conns); fflush(0);
    u32 nt = conns > 256 ? 256 : conns < 1 ? 1 : (u32)conns;
    if (!start_threads(app, l, nt)) return 1;
#endif
    return 0;
}
