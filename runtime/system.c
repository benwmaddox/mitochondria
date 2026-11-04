
/* ---- the C system of workers (runtime/system.c) ----
   Several programs (worker kinds), each built with its interface under its own prefix, run as one system: a thread
   per worker, bounded mailboxes carrying requests between them, and reply slots through which callers get answers.
   It reproduces the Rust system (src/system.rs): routing by session token or in turn, the message encoding (so the
   same requests fit a mailbox slot), forwarding with the forwarding request committed only when handed on, restarts,
   and every reply's text. The generated part before this file declares SYS_KINDS (one SysKind per worker kind),
   SYS_NKINDS, SYS_ENTRY and SYS_INFLIGHT. No heap: everything is a reservation made at start. */

void *VirtualAlloc(void *a, u64 n, u32 type, u32 prot);
int VirtualFree(void *a, u64 n, u32 type);
void *CreateThread(void *attr, u64 stack, u32 (*fn)(void *), void *arg, u32 flags, u32 *id);
u32 WaitForSingleObject(void *h, u32 ms);
int CloseHandle(void *h);
void *memcpy(void *d, const void *s, u64 n);
#define S_RESERVE 0x2000
#define S_COMMIT 0x1000
#define S_RELEASE 0x8000
#define S_RW 4
#define REPLY_CAP (256 * 1024)

/* locks and condition variables (kernel32: Windows' SRW locks and condition variables) */
typedef struct { void *p; } Lock;
typedef struct { void *p; } Cond;
void InitializeSRWLock(Lock *l);
void AcquireSRWLockExclusive(Lock *l);
void ReleaseSRWLockExclusive(Lock *l);
void InitializeConditionVariable(Cond *c);
void WakeConditionVariable(Cond *c);
void WakeAllConditionVariable(Cond *c);
int SleepConditionVariableSRW(Cond *c, Lock *l, u32 ms, u32 flags);
u64 GetTickCount64(void);
#define lk_init InitializeSRWLock
#define lk_on AcquireSRWLockExclusive
#define lk_off ReleaseSRWLockExclusive
#define cv_init InitializeConditionVariable
#define cv_one WakeConditionVariable
#define cv_all WakeAllConditionVariable
#define cv_wait SleepConditionVariableSRW
#define ticks GetTickCount64
static int sys_os(void) { return 1; }
static void *sys_mem(u64 n) { return VirtualAlloc(0, n, S_RESERVE | S_COMMIT, S_RW); }

/* ---- a mailbox: a ring of `cap` slots of `slot` bytes, each a u32 length and an encoded message ---- */
typedef struct { Lock lk; Cond cv; u8 *buf; u32 cap, slot, head, len; int closed, waiting; u64 refused; } Box;
enum { SENT, FULL, TOOBIG, CLOSED };
/* The encoding, as src/system.rs `encode`: u32 reply id; method, target, body as u32 length + bytes; then headers and
   forwarded fields, each a u16 count of (name, value) strings. */
typedef struct { u8 *b; u32 cap, n; int ok; } W;
static void w_raw(W *w, const void *p, u32 n) { if (!w->ok || w->n + (u64)n > w->cap) { w->ok = 0; return; } memcpy(w->b + w->n, p, n); w->n += n; }
static void w_s(W *w, const char *p, u32 n) { w_raw(w, &n, 4); w_raw(w, p, n); }
static void w_pairs(W *w, const MitoPair *ps, u32 n) { u16 k = (u16)n; w_raw(w, &k, 2); for (u32 i = 0; i < n; i++) { w_s(w, ps[i].k, ps[i].kn); w_s(w, ps[i].v, ps[i].vn); } }
static int box_send(Box *b, u32 reply, const char *m, u32 mn, const char *t, u32 tn, const char *body, u32 bn, const MitoPair *h, u32 nh, const MitoPair *f, u32 nf) {
    lk_on(&b->lk);
    if (b->closed) { lk_off(&b->lk); return CLOSED; }
    if (b->len == b->cap) { b->refused++; lk_off(&b->lk); return FULL; }
    u8 *at = b->buf + (u64)((b->head + b->len) % b->cap) * b->slot;
    W w = { at + 4, b->slot - 4, 0, 1 };
    w_raw(&w, &reply, 4); w_s(&w, m, mn); w_s(&w, t, tn); w_s(&w, body, bn); w_pairs(&w, h, nh); w_pairs(&w, f, nf);
    if (!w.ok) { lk_off(&b->lk); return TOOBIG; }
    memcpy(at, &w.n, 4);
    b->len++;
    int wake = b->waiting;
    lk_off(&b->lk);
    if (wake) cv_one(&b->cv);
    return SENT;
}
/* The next message, copied into `out` (at least `slot` bytes); 0 when the mailbox is closed and empty. */
static u32 box_recv(Box *b, u8 *out) {
    lk_on(&b->lk);
    while (b->len == 0) {
        if (b->closed) { lk_off(&b->lk); return 0; }
        b->waiting = 1; cv_wait(&b->cv, &b->lk, 500, 0); b->waiting = 0;
    }
    u8 *at = b->buf + (u64)b->head * b->slot;
    u32 n; memcpy(&n, at, 4); memcpy(out, at + 4, n);
    b->head = (b->head + 1) % b->cap; b->len--;
    lk_off(&b->lk);
    return n ? n : 1;
}
static void box_close(Box *b) { lk_on(&b->lk); b->closed = 1; lk_off(&b->lk); cv_all(&b->cv); }

/* ---- reply slots: 0 free, 1 waiting for an answer, 2 answered, 3 abandoned by a caller that gave up ---- */
typedef struct { Lock lk; Cond cv; u32 state, code, len; u8 *body; u64 committed; } Reply;
typedef struct { Lock lk; u32 *free; u32 nfree; } FreeList;
typedef struct Sys Sys;
typedef struct { Sys *sys; u32 kind, index, me; void *prog; u8 *msg; MitoPair *pairs; u32 maxpairs; void *thread; u64 handled, forwarded, restarts; } Worker;
struct Sys {
    Lock lk; i64 clock; int fixed; u64 seed; int seeded; u64 rr;
    u32 nworkers; Box *boxes; Worker *workers; u32 base[64], counts[64];
    const char *names[64];
    Reply *replies; FreeList fl;
};
static int reply_take(Sys *s, u32 *id) {
    lk_on(&s->fl.lk);
    if (!s->fl.nfree) { lk_off(&s->fl.lk); return 0; }
    *id = s->fl.free[--s->fl.nfree];
    lk_off(&s->fl.lk);
    Reply *r = &s->replies[*id];
    lk_on(&r->lk); r->state = 1; lk_off(&r->lk);
    return 1;
}
static void reply_free(Sys *s, u32 id) { lk_on(&s->fl.lk); s->fl.free[s->fl.nfree++] = id; lk_off(&s->fl.lk); }
static void reply_put(Sys *s, u32 id, u32 code, const char *body, u32 n) {
    if (id >= SYS_INFLIGHT) return;
    Reply *r = &s->replies[id];
    lk_on(&r->lk);
    if (r->state == 1) {
        if (n > REPLY_CAP) n = REPLY_CAP;
        if (n > r->committed) { u64 c = (n + 65535) / 65536 * 65536; if (c > REPLY_CAP) c = REPLY_CAP; VirtualAlloc(r->body, c, S_COMMIT, S_RW); r->committed = c; }
        memcpy(r->body, body, n); r->len = n; r->code = code; r->state = 2;
        lk_off(&r->lk); cv_all(&r->cv);
        return;
    }
    if (r->state == 3) { r->state = 0; lk_off(&r->lk); reply_free(s, id); return; }
    lk_off(&r->lk);
}
static void reply_c(Sys *s, u32 id, u32 code, const char *msg) { u32 n = 0; while (msg[n]) n++; reply_put(s, id, code, msg, n); }

/* ---- a worker ---- */
static void *make(Worker *w) {
    const SysKind *k = &SYS_KINDS[w->kind];
    u64 seed = w->sys->seeded ? (w->sys->seed ^ ((u64)w->me + 1) * 0x9E3779B97F4A7C15ULL) | 1 : 0; /* as the Rust system, per worker */
    void *p = k->start(~0ULL, seed);
    if (!p) return 0;
    k->set_system(p, SYS_NKINDS, w->sys->names, w->sys->counts, w->kind, w->index);
    if (k->marks) k->set_marks(p, k->soft, k->hard);
    return p;
}
/* Decode a message (src/system.rs `Rd`); 0 if malformed. Strings must be UTF-8, as there. */
typedef struct { const u8 *b; u32 n, i; } Rd;
static int rd_u32(Rd *r, u32 *v) { if (r->i + 4 > r->n) return 0; memcpy(v, r->b + r->i, 4); r->i += 4; return 1; }
static int utf8_ok(const u8 *s, u32 n) {
    for (u32 i = 0; i < n;) {
        u8 c = s[i]; u32 need; u8 lo = 0x80, hi = 0xBF;
        if (c < 0x80) { i++; continue; }
        if (c >= 0xC2 && c <= 0xDF) need = 1; else if (c >= 0xE0 && c <= 0xEF) { need = 2; if (c == 0xE0) lo = 0xA0; if (c == 0xED) hi = 0x9F; }
        else if (c >= 0xF0 && c <= 0xF4) { need = 3; if (c == 0xF0) lo = 0x90; if (c == 0xF4) hi = 0x8F; } else return 0;
        for (u32 k = 1; k <= need; k++) { if (i + k >= n) return 0; u8 d = s[i + k]; if (d < (k == 1 ? lo : 0x80) || d > (k == 1 ? hi : 0xBF)) return 0; }
        i += need + 1;
    }
    return 1;
}
static int rd_s(Rd *r, const char **p, u32 *n) { u32 k; if (!rd_u32(r, &k) || r->i + (u64)k > r->n || !utf8_ok(r->b + r->i, k)) return 0; *p = (const char *)r->b + r->i; *n = k; r->i += k; return 1; }
static int rd_pairs(Rd *r, MitoPair *ps, u32 max, u32 *n) {
    if (r->i + 2 > r->n) return 0;
    u16 k; memcpy(&k, r->b + r->i, 2); r->i += 2;
    if (k > max) return 0;
    for (u32 j = 0; j < k; j++) if (!rd_s(r, &ps[j].k, &ps[j].kn) || !rd_s(r, &ps[j].v, &ps[j].vn)) return 0;
    *n = k; return 1;
}
static u32 worker_main(void *arg) {
    Worker *w = (Worker *)arg; Sys *s = w->sys; const SysKind *k = &SYS_KINDS[w->kind];
    MitoPair out[64];
    for (;;) {
        u32 n = box_recv(&s->boxes[w->me], w->msg);
        if (!n) break;
        Rd r = { w->msg, n, 0 };
        u32 reply, mn, tn, bn, nh, nf; const char *m, *t, *body;
        u32 half = w->maxpairs / 2;
        if (!(rd_u32(&r, &reply) && rd_s(&r, &m, &mn) && rd_s(&r, &t, &tn) && rd_s(&r, &body, &bn) && rd_pairs(&r, w->pairs, half, &nh) && rd_pairs(&r, w->pairs + half, half, &nf))) {
            lk_on(&s->lk); w->handled++; lk_off(&s->lk);
            if (n >= 4) { memcpy(&reply, w->msg, 4); reply_c(s, reply, 400, "{\"error\":\"malformed message\"}"); }
            continue;
        }
        lk_on(&s->lk); i64 clock = s->clock; int fixed = s->fixed; lk_off(&s->lk);
        k->set_clock(w->prog, clock, fixed);
        int code = k->handle(w->prog, m, mn, t, tn, w->pairs, nh, w->pairs + half, nf, body, bn);
        if (k->crashed(w->prog)) {
            /* supervision: the worker's state is discarded and rebuilt from its declarations (a fresh reservation) */
            k->stop(w->prog); w->prog = make(w);
            lk_on(&s->lk); w->restarts++; w->handled++; lk_off(&s->lk);
            reply_c(s, reply, 500, "{\"error\":\"worker failed and was restarted\"}");
            continue;
        }
        if (code != 0) {
            u32 len; const char *resp = k->response(w->prog, &len);
            lk_on(&s->lk); w->handled++; lk_off(&s->lk); /* counted before answering: a caller with its answer sees it */
            reply_put(s, reply, (u32)code, resp, len);
            continue;
        }
        u32 fk; i64 key;
        u32 nfo = k->forward_to(w->prog, &fk, &key, out, 64);
        i64 c = s->counts[fk], rem = key % c; if (rem < 0) rem += c;
        u32 dst = s->base[fk] + (u32)rem;
        lk_on(&s->lk); w->handled++; w->forwarded++; lk_off(&s->lk);
        int sent = box_send(&s->boxes[dst], reply, m, mn, t, tn, body, bn, w->pairs, nh, out, nfo);
        if (sent != SENT) { lk_on(&s->lk); w->forwarded--; lk_off(&s->lk); }
        k->finish_forward(w->prog, sent == SENT); /* commit only if the request was handed on */
        if (sent == TOOBIG) reply_c(s, reply, 413, "{\"error\":\"forwarded request does not fit a mailbox slot\"}");
        else if (sent == FULL) reply_c(s, reply, 503, "{\"error\":\"the next worker is busy\"}");
        else if (sent == CLOSED) reply_c(s, reply, 503, "{\"error\":\"shutting down\"}");
    }
    return 0;
}

/* ---- the interface ---- */
/* Start every worker. `seeded`: random numbers start from `seed` (mixed with each worker's position; tests) instead of
   the OS. Returns 0 if a program could not start. */
MITO_API void *mito_system_start(u64 seed, int seeded) {
    if (!sys_os()) return 0;
    Sys *s = (Sys *)sys_mem(sizeof(Sys));
    if (!s || SYS_NKINDS > 64) return 0;
    lk_init(&s->lk); s->seed = seed; s->seeded = seeded;
    u32 n = 0;
    for (u32 k = 0; k < SYS_NKINDS; k++) { s->base[k] = n; s->counts[k] = SYS_KINDS[k].count; s->names[k] = SYS_KINDS[k].name; n += SYS_KINDS[k].count; }
    s->nworkers = n;
    s->boxes = (Box *)sys_mem(sizeof(Box) * n); s->workers = (Worker *)sys_mem(sizeof(Worker) * n);
    s->replies = (Reply *)sys_mem(sizeof(Reply) * SYS_INFLIGHT); s->fl.free = (u32 *)sys_mem(4ULL * SYS_INFLIGHT);
    if (!s->boxes || !s->workers || !s->replies || !s->fl.free) return 0;
    lk_init(&s->fl.lk);
    for (u32 i = 0; i < SYS_INFLIGHT; i++) {
        Reply *r = &s->replies[i]; lk_init(&r->lk); cv_init(&r->cv);
        r->body = (u8 *)VirtualAlloc(0, REPLY_CAP, S_RESERVE, 1);
        if (!r->body) return 0;
        s->fl.free[s->fl.nfree++] = SYS_INFLIGHT - 1 - i;
    }
    for (u32 k = 0, me = 0; k < SYS_NKINDS; k++) {
        const SysKind *kd = &SYS_KINDS[k];
        for (u32 i = 0; i < kd->count; i++, me++) {
            Box *b = &s->boxes[me]; lk_init(&b->lk); cv_init(&b->cv); b->cap = kd->mailbox; b->slot = kd->slot;
            b->buf = (u8 *)sys_mem((u64)kd->mailbox * kd->slot);
            Worker *w = &s->workers[me]; w->sys = s; w->kind = k; w->index = i; w->me = me;
            w->msg = (u8 *)sys_mem(kd->slot); w->maxpairs = kd->slot / 4 + 2; w->pairs = (MitoPair *)sys_mem(sizeof(MitoPair) * w->maxpairs);
            if (!b->buf || !w->msg || !w->pairs) return 0;
            w->prog = make(w);
            if (!w->prog) return 0;
        }
    }
    for (u32 i = 0; i < n; i++) { s->workers[i].thread = CreateThread(0, 0, worker_main, &s->workers[i], 0, 0); if (!s->workers[i].thread) return 0; }
    return s;
}
MITO_API void mito_system_set_clock(void *sp, i64 ms, int fixed) { Sys *s = (Sys *)sp; lk_on(&s->lk); s->clock = ms; s->fixed = fixed; lk_off(&s->lk); }
/* Send a request in at the entry worker and wait for its answer (copied into `out`). A session token with four parts
   (shard.slot.generation.secret) goes to its shard; other requests are spread in turn. */
MITO_API int mito_system_request(void *sp, const char *m, u32 mn, const char *t, u32 tn, const MitoPair *h, u32 nh, const char *body, u32 bn, u32 timeout_ms, char *out, u32 cap, u32 *outlen) {
    Sys *s = (Sys *)sp;
    const char *err = 0; int code = 0;
    u32 id;
    #define ANSWER(c, msg) do { u32 l_ = 0; while (msg[l_]) l_++; if (l_ > cap) l_ = cap; memcpy(out, msg, l_); *outlen = l_; return (c); } while (0)
    if (!reply_take(s, &id)) ANSWER(503, "{\"error\":\"too many requests in flight\"}");
    u32 n = s->counts[SYS_ENTRY];
    u64 shard = ~0ULL;
    for (u32 i = 0; i < nh; i++) {
        const char *k = h[i].k; u32 kn = h[i].kn;
        const char *x = "x-session"; u32 j = 0;
        if (kn != 9) continue;
        for (; j < 9; j++) { char c = k[j]; if (c >= 'A' && c <= 'Z') c += 32; if (c != x[j]) break; }
        if (j < 9) continue;
        u32 dots = 0, first = 0; for (u32 q = 0; q < h[i].vn; q++) if (h[i].v[q] == '.') { if (!dots) first = q; dots++; }
        if (dots == 3) { u64 v = 0; int ok = first > 0; for (u32 q = 0; q < first && ok; q++) { char c = h[i].v[q]; if (c == '+' && q == 0 && first > 1) continue; if (c < '0' || c > '9' || v > (~0ULL - (u64)(c - '0')) / 10) ok = 0; else v = v * 10 + (u64)(c - '0'); } if (ok) shard = v; }
        break;
    }
    if (shard == ~0ULL) { lk_on(&s->lk); shard = s->rr++; lk_off(&s->lk); }
    shard %= n;
    int sent = box_send(&s->boxes[s->base[SYS_ENTRY] + (u32)shard], id, m, mn, t, tn, body, bn, h, nh, 0, 0);
    if (sent != SENT) {
        Reply *r = &s->replies[id]; lk_on(&r->lk); r->state = 0; lk_off(&r->lk); reply_free(s, id);
        if (sent == TOOBIG) ANSWER(413, "{\"error\":\"request does not fit a mailbox slot\"}");
        ANSWER(503, "{\"error\":\"entry worker is busy\"}");
    }
    Reply *r = &s->replies[id];
    u64 deadline = ticks() + timeout_ms;
    lk_on(&r->lk);
    while (r->state == 1) {
        u64 now = ticks();
        if (now >= deadline) { r->state = 3; lk_off(&r->lk); ANSWER(504, "{\"error\":\"no answer in time\"}"); }
        cv_wait(&r->cv, &r->lk, (u32)(deadline - now), 0);
    }
    code = (int)r->code;
    u32 l = r->len < cap ? r->len : cap;
    memcpy(out, r->body, l); *outlen = l;
    r->state = 0;
    lk_off(&r->lk);
    reply_free(s, id);
    (void)err;
    return code;
    #undef ANSWER
}
/* One line per worker: kind index handled forwarded restarts. */
MITO_API u32 mito_system_report(void *sp, char *buf, u32 cap) {
    Sys *s = (Sys *)sp; u32 o = 0;
    lk_on(&s->lk);
    for (u32 i = 0; i < s->nworkers; i++) {
        Worker *w = &s->workers[i];
        u64 v[5] = { w->index, w->handled, w->forwarded, w->restarts, 0 };
        const char *nm = s->names[w->kind];
        for (u32 j = 0; nm[j] && o < cap; j++) buf[o++] = nm[j];
        for (int f = 0; f < 4; f++) {
            char d[24]; int dn = 0; u64 x = v[f];
            do { d[dn++] = (char)('0' + x % 10); x /= 10; } while (x);
            if (o < cap) buf[o++] = ' ';
            while (dn && o < cap) buf[o++] = d[--dn];
        }
        if (o < cap) buf[o++] = '\n';
    }
    lk_off(&s->lk);
    return o;
}
/* Close every mailbox; workers answer what is already queued, then stop. */
MITO_API void mito_system_stop(void *sp) {
    Sys *s = (Sys *)sp;
    for (u32 i = 0; i < s->nworkers; i++) box_close(&s->boxes[i]);
    for (u32 i = 0; i < s->nworkers; i++) { WaitForSingleObject(s->workers[i].thread, 0xFFFFFFFFu); CloseHandle(s->workers[i].thread); }
    for (u32 i = 0; i < s->nworkers; i++) SYS_KINDS[s->workers[i].kind].stop(s->workers[i].prog);
}
