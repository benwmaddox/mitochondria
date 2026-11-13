/* ---- the operating system (runtime/os.c) ----
   Everything the runtime asks of the operating system, for Windows (_WIN32) and Linux (glibc, x86-64): address space (reserve,
   commit, decommit, release), the clock and a timer, random bits, standard error, threads, locks and condition variables,
   and sockets (TCP on 127.0.0.1, poll, a wake-up channel for the poller). No C library headers are included: each function
   is declared as the system exports it, with types of the same size (the runtime's u64 and friends would clash with the
   headers' size_t). Every translation unit of a build includes this file first; its functions are static. */

#ifndef MITO_API
#if defined(_WIN32) && (defined(_MSC_VER) || defined(__clang__))
#define MITO_API __declspec(dllexport) /* built as a DLL by clang, the interface is exported; TinyCC finds it by name */
#elif defined(__clang__) || defined(__GNUC__)
#define MITO_API __attribute__((visibility("default")))
#else
#define MITO_API
#endif
#endif
void *memcpy(void *d, const void *s, u64 n);
void *memmove(void *d, const void *s, u64 n);
void *memset(void *d, int c, u64 n);
int memcmp(const void *a, const void *b, u64 n);
void *memchr(const void *s, int c, u64 n);
int printf(const char *f, ...);
char *getenv(const char *name);
int fflush(void *f);
#define PAGE 4096ULL
#define OS_FOREVER 0xFFFFFFFFu

#ifdef _WIN32
/* ---------------------------------------------------------------- Windows */
void *VirtualAlloc(void *a, u64 n, u32 type, u32 prot);
int VirtualFree(void *a, u64 n, u32 type);
void GetSystemTimeAsFileTime(u64 *ft);
u64 GetTickCount64(void);
int QueryPerformanceCounter(i64 *t);
int QueryPerformanceFrequency(i64 *f);
int BCryptGenRandom(void *alg, u8 *buf, u32 n, u32 flags);
void *GetStdHandle(u32 which);
int WriteFile(void *h, const void *b, u32 n, u32 *done, void *ov);
void *CreateThread(void *attr, u64 stack, u32 (*fn)(void *), void *arg, u32 flags, u32 *id);
u32 WaitForSingleObject(void *h, u32 ms);
int CloseHandle(void *h);
int CreateDirectoryA(const char *path, void *attr);
u32 GetLastError(void);
static void *os_reserve(u64 n) { return VirtualAlloc(0, n, 0x2000, 1); }                 /* MEM_RESERVE, PAGE_NOACCESS */
static int os_commit(void *p, u64 n) { return VirtualAlloc(p, n, 0x1000, 4) != 0; }      /* MEM_COMMIT, PAGE_READWRITE */
static void os_decommit(void *p, u64 n) { VirtualFree(p, n, 0x4000); }                   /* MEM_DECOMMIT */
static void os_release(void *p, u64 n) { (void)n; if (p) VirtualFree(p, 0, 0x8000); }    /* MEM_RELEASE */
static void *os_alloc(u64 n) { return VirtualAlloc(0, n, 0x3000, 4); }                   /* reserved and committed, zeroed */
/* Address space that commits itself when first touched (Windows has no lazy commit: a handler for access violations commits the 256 KB around the
   address, in ranges that os_lazy_add registered, and the access runs again). Linux commits whole ranges with mprotect, and the kernel backs a page only when it
   is written, so it needs no handler: OS_FAULT_COMMIT is defined only here. */
#define OS_FAULT_COMMIT 1
typedef struct { u64 lo, hi; } LazyRange;
static LazyRange g_lazy[8]; static u32 g_nlazy; static int g_lazy_on;
void *AddVectoredExceptionHandler(u32 first, long (*h)(void **ep));
static long os_lazy_fault(void **ep) {
    u8 *rec = (u8 *)ep[0];
    if (*(u32 *)rec != 0xC0000005u || *(u32 *)(rec + 24) < 2 || (*(u64 *)(rec + 32) != 0 && *(u64 *)(rec + 32) != 1)) return 0; /* not a read or write of an uncommitted page */
    u64 a = *(u64 *)(rec + 40);
    for (u32 i = 0; i < g_nlazy; i++) if (a >= g_lazy[i].lo && a < g_lazy[i].hi) {
        u64 s = a & ~(u64)(256 * 1024 - 1), e = s + 256 * 1024; if (s < g_lazy[i].lo) s = g_lazy[i].lo; if (e > g_lazy[i].hi) e = g_lazy[i].hi;
        if (!VirtualAlloc((void *)s, e - s, 0x1000, 4)) return 0;
        return -1; /* EXCEPTION_CONTINUE_EXECUTION */
    }
    return 0;
}
static int os_lazy_add(void *p, u64 n) {
    if (g_nlazy == 8) return 0;
    if (!g_lazy_on) { if (!AddVectoredExceptionHandler(1, os_lazy_fault)) return 0; g_lazy_on = 1; }
    g_lazy[g_nlazy].lo = (u64)p; g_lazy[g_nlazy].hi = (u64)p + n; g_nlazy++; return 1;
}
/* shared state for readers (os_reserve_shared, os_map_shared_ro, os_map_private_at): Linux only for now */
static int os_can_share(void) { return 0; }
static u32 os_cpu_count(void) { return 1; } /* readers are Linux only for now: one */
static void *os_reserve_shared(u64 n, i64 *fd) { (void)n; *fd = -1; return 0; }
static void *os_map_shared_ro(i64 fd, u64 n) { (void)fd; (void)n; return 0; }
static int os_map_private_at(void *p, u64 n) { (void)p; (void)n; return 0; }
static i64 os_now_ms(void) { u64 ft = 0; GetSystemTimeAsFileTime(&ft); return (i64)((ft - 116444736000000000ULL) / 10000ULL); }
static u64 os_ticks_ms(void) { return GetTickCount64(); }
static u64 os_perf_ns(void) { i64 t = 0, f = 1; QueryPerformanceCounter(&t); QueryPerformanceFrequency(&f); return (u64)((double)t * 1e9 / (double)f); }
static u64 os_random(void) { u64 v = 0; BCryptGenRandom(0, (u8 *)&v, 8, 2); return v; } /* BCRYPT_USE_SYSTEM_PREFERRED_RNG */
static void os_err(const void *p, u32 n) { u32 done = 0; WriteFile(GetStdHandle((u32)-12), p, n, &done, 0); } /* STD_ERROR_HANDLE */
int ReadFile(void *h, void *b, u32 n, u32 *done, void *ov);
/* standard output and input for command-line programs (runtime/cli.c): bytes as they are; os_in reads up to n bytes, 0 at the end of the input */
/* one write to standard output until all of it is out: 0 done, 1 the reader has gone (a broken pipe), 2 any other error. A handle is binary: nothing is translated */
static int os_out_raw(const void *p, u32 n) { while (n) { u32 done = 0; if (!WriteFile(GetStdHandle((u32)-11), p, n, &done, 0)) { u32 e = GetLastError(); return e == 109 || e == 232 ? 1 : 2; } if (!done) return 2; p = (const u8 *)p + done; n -= done; } return 0; } /* ERROR_BROKEN_PIPE, ERROR_NO_DATA */
static u32 os_in(void *p, u32 n) { u32 done = 0; if (!ReadFile(GetStdHandle((u32)-10), p, n, &done, 0)) return 0; return done; }
/* make a directory (one level); 1 when it exists afterwards */
static int os_mkdir(const char *path) { return CreateDirectoryA(path, 0) || GetLastError() == 183; } /* ERROR_ALREADY_EXISTS */
void *CreateFileA(const char *path, u32 access, u32 share, void *sec, u32 disp, u32 flags, void *tmpl);
int ReadFile(void *h, void *b, u32 n, u32 *done, void *ov);
/* a whole file into buf (at most cap bytes): 1 and its length, 0 when it cannot be read, 2 when it is larger than cap */
static int os_read_file(const char *path, u8 *buf, u64 cap, u64 *len) {
    void *h = CreateFileA(path, 0x80000000u, 1, 0, 3, 0, 0); /* GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING */
    if (h == (void *)(i64)-1) return 0;
    u64 n = 0; int r = 1;
    for (;;) { u32 got = 0; u64 want = cap - n < (1u << 30) ? cap - n : (1u << 30); if (!want) { u8 x; if (ReadFile(h, &x, 1, &got, 0) && got) r = 2; break; }
               if (!ReadFile(h, buf + n, (u32)want, &got, 0)) { r = 0; break; } if (!got) break; n += got; }
    CloseHandle(h); *len = n; return r;
}
/* a file to read in steps (a command-line program's read_file): a handle, or 0 when it cannot be opened (a directory cannot) */
static void *os_file_open(const char *path) { void *h = CreateFileA(path, 0x80000000u, 3, 0, 3, 0, 0); return h == (void *)(i64)-1 ? 0 : h; } /* GENERIC_READ, FILE_SHARE_READ | WRITE, OPEN_EXISTING */
static u32 os_file_read(void *h, void *p, u32 n) { u32 done = 0; if (!ReadFile(h, p, n, &done, 0)) return 0; return done; }
static void os_file_close(void *h) { CloseHandle(h); }
/* the process's environment as `NAME=value` entries each followed by a 0 byte (UTF-8; Windows' hidden `=C:=...` entries left out), at most cap bytes: the length, or 0xFFFFFFFF when it does not fit */
unsigned short *GetEnvironmentStringsW(void);
int FreeEnvironmentStringsW(unsigned short *p);
int WideCharToMultiByte(u32 cp, u32 flags, const unsigned short *w, int nw, char *out, int nout, const char *def, int *used);
static u32 os_environ(char *buf, u32 cap) {
    unsigned short *blk = GetEnvironmentStringsW(); u32 n = 0;
    if (!blk) return 0;
    for (unsigned short *p = blk; *p;) {
        u32 wl = 0; while (p[wl]) wl++;
        if (p[0] != '=') {
            if (n >= cap) { n = 0xFFFFFFFFu; break; }
            int k = WideCharToMultiByte(65001, 0, p, (int)wl + 1, buf + n, (int)(cap - n), 0, 0); /* CP_UTF8, with the 0 */
            if (k <= 0) { n = 0xFFFFFFFFu; break; }
            n += (u32)k;
        }
        p += wl + 1;
    }
    FreeEnvironmentStringsW(blk); return n;
}

/* threads: a record that lives as long as the thread (the caller's) */
typedef struct { void *h; } OsThread;
/* a thread's stack: 16 MB reserved (STACK_SIZE_PARAM_IS_A_RESERVATION), committed as it is used. An unoptimized build (TinyCC) gives a
   function each of its inlined calls' temporaries, so a recursive one can take far more than the executable's default 1 MB. */
#define OS_THREAD_STACK (16u << 20)
static int os_thread_start(OsThread *t, u32 (*fn)(void *), void *arg) { t->h = CreateThread(0, OS_THREAD_STACK, fn, arg, 0x10000, 0); return t->h != 0; }
u32 TlsAlloc(void); void *TlsGetValue(u32 k); int TlsSetValue(u32 k, void *v);
/* a value of the calling thread's own (one key for the process) */
static u32 g_os_tls = 0xFFFFFFFFu;
static int os_tls_init(void) { if (g_os_tls == 0xFFFFFFFFu) g_os_tls = TlsAlloc(); return g_os_tls != 0xFFFFFFFFu; }
static void *os_tls_get(void) { return g_os_tls == 0xFFFFFFFFu ? 0 : TlsGetValue(g_os_tls); }
static void os_tls_set(void *v) { if (g_os_tls != 0xFFFFFFFFu) TlsSetValue(g_os_tls, v); }
static void os_thread_join(OsThread *t) { WaitForSingleObject(t->h, OS_FOREVER); CloseHandle(t->h); }

/* SRW locks and condition variables */
typedef struct { void *p; } Lock;
typedef struct { void *p; } Cond;
void InitializeSRWLock(Lock *l);
void AcquireSRWLockExclusive(Lock *l);
void ReleaseSRWLockExclusive(Lock *l);
void InitializeConditionVariable(Cond *c);
void WakeConditionVariable(Cond *c);
void WakeAllConditionVariable(Cond *c);
int SleepConditionVariableSRW(Cond *c, Lock *l, u32 ms, u32 flags);
static void lk_init(Lock *l) { InitializeSRWLock(l); }
static void lk_on(Lock *l) { AcquireSRWLockExclusive(l); }
static void lk_off(Lock *l) { ReleaseSRWLockExclusive(l); }
static void cv_init(Cond *c) { InitializeConditionVariable(c); }
static void cv_one(Cond *c) { WakeConditionVariable(c); }
static void cv_all(Cond *c) { WakeAllConditionVariable(c); }
static void cv_wait(Cond *c, Lock *l, u32 ms) { SleepConditionVariableSRW(c, l, ms, 0); }

/* sockets (Winsock): a socket is a 64-bit handle */
typedef u64 Sock;
#define OS_BAD_SOCK (~0ULL)
typedef struct { u32 len; char *buf; } OsBuf;
typedef struct { u64 fd; short events, revents; } OsPoll;
#define OS_POLLIN 0x0100 /* POLLRDNORM */
int WSAStartup(unsigned short version, void *data);
u64 socket(int af, int type, int proto);
int bind(u64 s, const void *addr, int n);
int listen(u64 s, int backlog);
u64 accept(u64 s, void *addr, int *n);
int recv(u64 s, char *b, int n, int flags);
int send(u64 s, const char *b, int n, int flags);
int closesocket(u64 s);
int setsockopt(u64 s, int level, int name, const char *v, int n);
int WSASend(u64 s, OsBuf *bufs, u32 n, u32 *sent, u32 flags, void *overlapped, void *done);
int WSAPoll(OsPoll *fds, u32 n, int timeout);
int getsockname(u64 s, void *addr, int *n);
int sendto(u64 s, const char *b, int n, int flags, const void *to, int tn);
int ioctlsocket(u64 s, long cmd, u32 *arg);
static int os_net_init(void) { u8 wsa[512]; return WSAStartup(0x0202, wsa) == 0; }
static Sock os_listen(u32 port) {
    Sock s = socket(2, 1, 6); /* AF_INET, SOCK_STREAM, TCP */
    u8 addr[16] = { 2, 0, (u8)(port >> 8), (u8)port, 127, 0, 0, 1 };
    if (s == OS_BAD_SOCK) return s;
    if (bind(s, addr, 16) != 0 || listen(s, 128) != 0) { closesocket(s); return OS_BAD_SOCK; }
    return s;
}
static Sock os_accept(Sock l) { return accept(l, 0, 0); }
static void os_sock_setup(Sock s) {
    u32 to = 5000, one = 1;
    setsockopt(s, 0xffff, 0x1006, (const char *)&to, 4); /* SO_RCVTIMEO: 5 s */
    setsockopt(s, 6, 1, (const char *)&one, 4);         /* TCP_NODELAY: an answer goes out as soon as it is written */
}
static int os_recv(Sock s, u8 *b, u32 n) { return recv(s, (char *)b, (int)n, 0); }
static int os_send(Sock s, const u8 *b, u32 n) { return send(s, (const char *)b, (int)n, 0); }
/* two buffers in one call (two small sends can wait on the client's delayed acknowledgement of the first): the bytes sent (0 on error) */
static u64 os_send2(Sock s, const u8 *a, u32 an, const u8 *b, u32 bn) { OsBuf v[2] = { { an, (char *)a }, { bn, (char *)b } }; u32 sent = 0; return WSASend(s, v, bn ? 2 : 1, &sent, 0, 0, 0) == 0 ? sent : 0; }
static void os_close(Sock s) { closesocket(s); }
int shutdown(u64 s, int how);
/* both directions ended (the peer sees the end; the socket is closed later by its reader) */
static void os_shutdown(Sock s) { shutdown(s, 2); } /* SD_BOTH */
static int os_poll(OsPoll *p, u32 n, int ms) { return WSAPoll(p, n, ms); }
/* the poller's wake-up channel: a UDP socket on 127.0.0.1 that sends to itself */
static Sock g_os_wake = OS_BAD_SOCK; static u8 g_os_wake_addr[16];
static Sock os_wake_open(void) {
    g_os_wake = socket(2, 2, 17); /* AF_INET, SOCK_DGRAM, UDP */
    if (g_os_wake == OS_BAD_SOCK) return OS_BAD_SOCK;
    u8 a[16] = { 2, 0, 0, 0, 127, 0, 0, 1 }; int an = 16;
    if (bind(g_os_wake, a, 16) != 0 || getsockname(g_os_wake, g_os_wake_addr, &an) != 0) return OS_BAD_SOCK;
    u32 nb = 1; ioctlsocket(g_os_wake, (long)0x8004667EL, &nb); /* FIONBIO: drained without blocking */
    return g_os_wake;
}
static void os_wake_ping(void) { sendto(g_os_wake, "w", 1, 0, g_os_wake_addr, 16); }
static void os_wake_drain(void) { char b[64]; while (recv(g_os_wake, b, sizeof b, 0) > 0) {} }

#else
/* ---------------------------------------------------------------- Linux (glibc, x86-64) */
void *mmap(void *a, u64 n, int prot, int flags, int fd, i64 off);
int munmap(void *a, u64 n);
int mprotect(void *a, u64 n, int prot);
int madvise(void *a, u64 n, int advice);
typedef struct { i64 sec, nsec; } OsTime;
int clock_gettime(int clock, OsTime *t);
i64 getrandom(void *b, u64 n, u32 flags);
i64 write(int fd, const void *b, u64 n);
i64 read(int fd, void *b, u64 n);
int close(int fd);
int mkdir(const char *path, u32 mode);
int *__errno_location(void);
#define OS_MAP (0x02 | 0x20) /* MAP_PRIVATE | MAP_ANONYMOUS */
static void *os_reserve(u64 n) { void *p = mmap(0, n, 0, OS_MAP | 0x4000, -1, 0); return p == (void *)-1 ? 0 : p; } /* PROT_NONE, MAP_NORESERVE */
static int os_commit(void *p, u64 n) { return mprotect(p, n, 3) == 0; }                                             /* PROT_READ | PROT_WRITE */
static void os_decommit(void *p, u64 n) { madvise(p, n, 4); mprotect(p, n, 0); }                                     /* MADV_DONTNEED: the pages go, and read as zero when committed again */
static void os_release(void *p, u64 n) { if (p) munmap(p, n); }
static void *os_alloc(u64 n) { void *p = mmap(0, n, 3, OS_MAP, -1, 0); return p == (void *)-1 ? 0 : p; }
int memfd_create(const char *name, unsigned flags);
int ftruncate(int fd, i64 n);
static int os_can_share(void) { return 1; }
int sched_getaffinity(int pid, u64 size, void *mask);
/* the CPUs this process may run on (its affinity mask: a server pinned with taskset counts those) */
static u32 os_cpu_count(void) { u64 m[16] = { 0 }; if (sched_getaffinity(0, sizeof m, m) != 0) return 1; u32 n = 0; for (u32 i = 0; i < 16; i++) for (u64 v = m[i]; v; v &= v - 1) n++; return n ? n : 1; }
/* State that readers map too: an anonymous shared memory file (memfd), reserved like os_reserve (no access until committed) */
static void *os_reserve_shared(u64 n, i64 *fd) {
    int f = memfd_create("mito-state", 1); /* MFD_CLOEXEC */
    *fd = -1;
    if (f < 0) return 0;
    if (ftruncate(f, (i64)n) != 0) { close(f); return 0; }
    void *p = mmap(0, n, 0, 0x01 | 0x4000, f, 0); /* PROT_NONE, MAP_SHARED | MAP_NORESERVE */
    if (p == (void *)-1) { close(f); return 0; }
    *fd = f; return p;
}
/* a reader's view of the shared state: the same pages, read-only (a change it should not make faults instead of landing) */
static void *os_map_shared_ro(i64 fd, u64 n) { void *p = mmap(0, n, 1, 0x01 | 0x4000, (int)fd, 0); return p == (void *)-1 ? 0 : p; } /* PROT_READ, MAP_SHARED | MAP_NORESERVE */
/* the reader's own pages over [p, p + n) of that view (its request struct): private, zeroed, writable */
static int os_map_private_at(void *p, u64 n) { void *q = mmap(p, n, 3, OS_MAP | 0x10, -1, 0); return q == p; } /* MAP_FIXED */
static i64 os_now_ms(void) { OsTime t; clock_gettime(0, &t); return t.sec * 1000 + t.nsec / 1000000; }               /* CLOCK_REALTIME */
static u64 os_ticks_ms(void) { OsTime t; clock_gettime(1, &t); return (u64)t.sec * 1000 + (u64)t.nsec / 1000000; }   /* CLOCK_MONOTONIC */
static u64 os_perf_ns(void) { OsTime t; clock_gettime(1, &t); return (u64)t.sec * 1000000000ULL + (u64)t.nsec; }
static u64 os_random(void) { u64 v = 0; while (getrandom(&v, 8, 0) != 8) {} return v; }
static void os_err(const void *p, u32 n) { while (n) { i64 k = write(2, p, n); if (k <= 0) return; p = (const u8 *)p + k; n -= (u32)k; } }
/* one write to standard output until all of it is out: 0 done, 1 the reader has gone (EPIPE), 2 any other error */
static int os_out_raw(const void *p, u32 n) { while (n) { i64 k = write(1, p, n); if (k < 0) { int e = *__errno_location(); if (e == 4) continue; return e == 32 ? 1 : 2; } if (k == 0) return 2; p = (const u8 *)p + k; n -= (u32)k; } return 0; } /* EINTR, EPIPE */
static u32 os_in(void *p, u32 n) { i64 k = read(0, p, n); return k > 0 ? (u32)k : 0; }
static int os_mkdir(const char *path) { return mkdir(path, 0755) == 0 || *__errno_location() == 17; } /* EEXIST */
int open(const char *path, int flags, ...);
static int os_read_file(const char *path, u8 *buf, u64 cap, u64 *len) {
    int fd = open(path, 0x80000); /* O_RDONLY | O_CLOEXEC */
    if (fd < 0) return 0;
    u64 n = 0; int r = 1;
    for (;;) { if (n == cap) { u8 x; if (read(fd, &x, 1) == 1) r = 2; break; } i64 k = read(fd, buf + n, cap - n); if (k < 0) { r = 0; break; } if (!k) break; n += (u64)k; }
    close(fd); *len = n; return r;
}
/* a file to read in steps (a command-line program's read_file): a handle (the descriptor plus one), or 0 when it cannot be opened (a directory cannot) */
static void *os_file_open(const char *path) {
    int d = open(path, 0x90000); /* O_RDONLY | O_CLOEXEC | O_DIRECTORY: this opens only a directory */
    if (d >= 0) { close(d); return 0; }
    int fd = open(path, 0x80000); /* O_RDONLY | O_CLOEXEC */
    return fd < 0 ? 0 : (void *)(u64)(fd + 1);
}
static u32 os_file_read(void *h, void *p, u32 n) { i64 k = read((int)(u64)h - 1, p, n); return k > 0 ? (u32)k : 0; }
static void os_file_close(void *h) { close((int)(u64)h - 1); }
/* the process's environment as `NAME=value` entries each followed by a 0 byte, at most cap bytes: the length, or 0xFFFFFFFF when it does not fit */
extern char **environ;
static u32 os_environ(char *buf, u32 cap) {
    u32 n = 0;
    for (char **e = environ; e && *e; e++) {
        u64 l = 0; while ((*e)[l]) l++;
        if (n + l + 1 > cap) return 0xFFFFFFFFu;
        memcpy(buf + n, *e, l + 1); n += (u32)(l + 1);
    }
    return n;
}

/* threads, locks and condition variables (pthreads; the types are opaque blocks of glibc's sizes) */
typedef struct { u64 id; u32 (*fn)(void *); void *arg; } OsThread; /* lives as long as the thread: the thread reads fn and arg from it */
typedef struct { u64 w[5]; } Lock;  /* pthread_mutex_t: 40 bytes */
typedef struct { u64 w[6]; } Cond;  /* pthread_cond_t: 48 bytes */
int pthread_create(u64 *id, const void *attr, void *(*fn)(void *), void *arg);
int pthread_join(u64 id, void **ret);
int pthread_mutex_init(Lock *l, const void *attr);
int pthread_mutex_lock(Lock *l);
int pthread_mutex_unlock(Lock *l);
int pthread_condattr_init(void *a);
int pthread_condattr_setclock(void *a, int clock);
int pthread_cond_init(Cond *c, const void *attr);
int pthread_cond_signal(Cond *c);
int pthread_cond_broadcast(Cond *c);
int pthread_cond_wait(Cond *c, Lock *l);
int pthread_cond_timedwait(Cond *c, Lock *l, const OsTime *at);
static void *os_thread_main(void *p) { OsThread *t = (OsThread *)p; t->fn(t->arg); return 0; }
/* a thread's stack: 16 MB (mapped as it is used), as on Windows: not whatever RLIMIT_STACK says */
int pthread_attr_init(void *a); int pthread_attr_setstacksize(void *a, u64 n); int pthread_attr_destroy(void *a);
static int os_thread_start(OsThread *t, u32 (*fn)(void *), void *arg) {
    t->fn = fn; t->arg = arg;
    u64 attr[16]; /* pthread_attr_t: 56 bytes on glibc's x86-64 and aarch64 */
    if (pthread_attr_init(attr) != 0) return 0;
    pthread_attr_setstacksize(attr, 16u << 20);
    int r = pthread_create(&t->id, attr, os_thread_main, t);
    pthread_attr_destroy(attr);
    return r == 0;
}
int pthread_key_create(u32 *k, void (*dtor)(void *)); void *pthread_getspecific(u32 k); int pthread_setspecific(u32 k, const void *v);
/* a value of the calling thread's own (one key for the process) */
static u32 g_os_tls; static int g_os_tls_on;
static int os_tls_init(void) { if (!g_os_tls_on) g_os_tls_on = pthread_key_create(&g_os_tls, 0) == 0; return g_os_tls_on; }
static void *os_tls_get(void) { return g_os_tls_on ? pthread_getspecific(g_os_tls) : 0; }
static void os_tls_set(void *v) { if (g_os_tls_on) pthread_setspecific(g_os_tls, v); }
static void os_thread_join(OsThread *t) { pthread_join(t->id, 0); }
static void lk_init(Lock *l) { pthread_mutex_init(l, 0); }
static void lk_on(Lock *l) { pthread_mutex_lock(l); }
static void lk_off(Lock *l) { pthread_mutex_unlock(l); }
static void cv_init(Cond *c) { u64 a[1] = { 0 }; pthread_condattr_init(a); pthread_condattr_setclock(a, 1); pthread_cond_init(c, a); } /* waits measured on CLOCK_MONOTONIC */
static void cv_one(Cond *c) { pthread_cond_signal(c); }
static void cv_all(Cond *c) { pthread_cond_broadcast(c); }
static void cv_wait(Cond *c, Lock *l, u32 ms) {
    if (ms == OS_FOREVER) { pthread_cond_wait(c, l); return; }
    OsTime t; clock_gettime(1, &t);
    t.sec += ms / 1000; t.nsec += (i64)(ms % 1000) * 1000000;
    if (t.nsec >= 1000000000) { t.sec++; t.nsec -= 1000000000; }
    pthread_cond_timedwait(c, l, &t);
}

/* sockets: a socket is a file descriptor (zero-extended) */
typedef u64 Sock;
#define OS_BAD_SOCK (~0ULL)
typedef struct { int fd; short events, revents; } OsPoll;
#define OS_POLLIN 0x0001 /* POLLIN */
typedef struct { const void *base; u64 len; } OsIov;
typedef struct { void *name; u32 namelen; u32 pad; OsIov *iov; u64 iovlen; void *control; u64 controllen; int flags; } OsMsg;
int socket(int af, int type, int proto);
int bind(int s, const void *addr, u32 n);
int listen(int s, int backlog);
int accept4(int s, void *addr, u32 *n, int flags);
i64 recv(int s, void *b, u64 n, int flags);
i64 send(int s, const void *b, u64 n, int flags);
i64 sendmsg(int s, const OsMsg *m, int flags);
int setsockopt(int s, int level, int name, const void *v, u32 n);
int poll(OsPoll *fds, u64 n, int timeout);
int pipe2(int *fds, int flags);
void *signal(int sig, void *handler);
#define OS_NOSIGNAL 0x4000 /* MSG_NOSIGNAL: a write to a closed connection fails instead of ending the process */
static int os_net_init(void) { signal(13, (void *)1); return 1; } /* SIGPIPE ignored */
static Sock os_listen(u32 port) {
    int s = socket(2, 1 | 0x80000, 6); /* AF_INET, SOCK_STREAM | SOCK_CLOEXEC, TCP */
    u8 addr[16] = { 2, 0, (u8)(port >> 8), (u8)port, 127, 0, 0, 1 };
    int one = 1;
    if (s < 0) return OS_BAD_SOCK;
    setsockopt(s, 1, 2, &one, 4); /* SO_REUSEADDR: a restarted server takes its port back at once */
    if (bind(s, addr, 16) != 0 || listen(s, 128) != 0) { close(s); return OS_BAD_SOCK; }
    return (Sock)(u32)s;
}
static Sock os_accept(Sock l) { int c = accept4((int)l, 0, 0, 0x80000); return c < 0 ? OS_BAD_SOCK : (Sock)(u32)c; } /* SOCK_CLOEXEC */
static void os_sock_setup(Sock s) {
    i64 to[2] = { 5, 0 }; int one = 1;
    setsockopt((int)s, 1, 20, to, 16); /* SO_RCVTIMEO: 5 s */
    setsockopt((int)s, 6, 1, &one, 4); /* TCP_NODELAY */
}
static int os_recv(Sock s, u8 *b, u32 n) { return (int)recv((int)s, b, n, 0); }
static int os_send(Sock s, const u8 *b, u32 n) { return (int)send((int)s, b, n, OS_NOSIGNAL); }
static u64 os_send2(Sock s, const u8 *a, u32 an, const u8 *b, u32 bn) {
    OsIov v[2] = { { a, an }, { b, bn } };
    OsMsg m = { 0, 0, 0, v, bn ? 2 : 1, 0, 0, 0 };
    i64 k = sendmsg((int)s, &m, OS_NOSIGNAL);
    return k > 0 ? (u64)k : 0;
}
static void os_close(Sock s) { close((int)s); }
int shutdown(int fd, int how);
static void os_shutdown(Sock s) { shutdown((int)s, 2); } /* SHUT_RDWR */
static int os_poll(OsPoll *p, u32 n, int ms) { return poll(p, n, ms); }
/* the poller's wake-up channel: a pipe; its read end is polled */
static int g_os_wake[2] = { -1, -1 };
static Sock os_wake_open(void) { if (pipe2(g_os_wake, 0x800 | 0x80000) != 0) return OS_BAD_SOCK; return (Sock)(u32)g_os_wake[0]; } /* O_NONBLOCK | O_CLOEXEC */
static void os_wake_ping(void) { (void)write(g_os_wake[1], "w", 1); }
static void os_wake_drain(void) { char b[64]; while (read(g_os_wake[0], b, sizeof b) > 0) {} }
#endif

/* ---- a command-line program's standard output, buffered (print, flush, the answer's body: runtime/cli.c, mito.c sys_builtin) ----
   Bytes go into a fixed block of OS_OBUF (64 KiB) and are written when it fills, on flush(), before standard input is read (stdin_text), before anything is
   written to standard error (eprint, an error answer) and when the program ends: so output appears as the program runs, in large writes, in the order it was
   written. A text of OS_OBUF bytes or more is written straight from where it lives (after what is buffered), never copied. When the reader has gone (a closed
   pipe: `prog | head -1`) the program ends quietly with exit code 2 (on Linux the kernel's SIGPIPE usually ends it first, as it does any program); any other
   write error ends it with exit code 2 and a short message on standard error. */
#define OS_OBUF 65536u
void exit(int code);
static u8 g_os_obuf[OS_OBUF]; static u32 g_os_on;
static void os_out_end(int r) { if (r == 2) os_err("error: cannot write to standard output\n", 40); exit(2); }
static void os_out_flush(void) { if (g_os_on) { u32 n = g_os_on; g_os_on = 0; int r = os_out_raw(g_os_obuf, n); if (r) os_out_end(r); } }
static void os_out(const void *p, u32 n) {
    if (n >= OS_OBUF) { os_out_flush(); int r = os_out_raw(p, n); if (r) os_out_end(r); return; }
    if (g_os_on + n > OS_OBUF) os_out_flush();
    memcpy(g_os_obuf + g_os_on, p, n); g_os_on += n;
}
