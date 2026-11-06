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
static i64 os_now_ms(void) { u64 ft = 0; GetSystemTimeAsFileTime(&ft); return (i64)((ft - 116444736000000000ULL) / 10000ULL); }
static u64 os_ticks_ms(void) { return GetTickCount64(); }
static u64 os_perf_ns(void) { i64 t = 0, f = 1; QueryPerformanceCounter(&t); QueryPerformanceFrequency(&f); return (u64)((double)t * 1e9 / (double)f); }
static u64 os_random(void) { u64 v = 0; BCryptGenRandom(0, (u8 *)&v, 8, 2); return v; } /* BCRYPT_USE_SYSTEM_PREFERRED_RNG */
static void os_err(const void *p, u32 n) { u32 done = 0; WriteFile(GetStdHandle((u32)-12), p, n, &done, 0); } /* STD_ERROR_HANDLE */
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

/* threads: a record that lives as long as the thread (the caller's) */
typedef struct { void *h; } OsThread;
static int os_thread_start(OsThread *t, u32 (*fn)(void *), void *arg) { t->h = CreateThread(0, 0, fn, arg, 0, 0); return t->h != 0; }
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
static i64 os_now_ms(void) { OsTime t; clock_gettime(0, &t); return t.sec * 1000 + t.nsec / 1000000; }               /* CLOCK_REALTIME */
static u64 os_ticks_ms(void) { OsTime t; clock_gettime(1, &t); return (u64)t.sec * 1000 + (u64)t.nsec / 1000000; }   /* CLOCK_MONOTONIC */
static u64 os_perf_ns(void) { OsTime t; clock_gettime(1, &t); return (u64)t.sec * 1000000000ULL + (u64)t.nsec; }
static u64 os_random(void) { u64 v = 0; while (getrandom(&v, 8, 0) != 8) {} return v; }
static void os_err(const void *p, u32 n) { while (n) { i64 k = write(2, p, n); if (k <= 0) return; p = (const u8 *)p + k; n -= (u32)k; } }
static int os_mkdir(const char *path) { return mkdir(path, 0755) == 0 || *__errno_location() == 17; } /* EEXIST */
int open(const char *path, int flags, ...);
static int os_read_file(const char *path, u8 *buf, u64 cap, u64 *len) {
    int fd = open(path, 0x80000); /* O_RDONLY | O_CLOEXEC */
    if (fd < 0) return 0;
    u64 n = 0; int r = 1;
    for (;;) { if (n == cap) { u8 x; if (read(fd, &x, 1) == 1) r = 2; break; } i64 k = read(fd, buf + n, cap - n); if (k < 0) { r = 0; break; } if (!k) break; n += (u64)k; }
    close(fd); *len = n; return r;
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
static int os_thread_start(OsThread *t, u32 (*fn)(void *), void *arg) { t->fn = fn; t->arg = arg; return pthread_create(&t->id, 0, os_thread_main, t) == 0; }
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
static int os_poll(OsPoll *p, u32 n, int ms) { return poll(p, n, ms); }
/* the poller's wake-up channel: a pipe; its read end is polled */
static int g_os_wake[2] = { -1, -1 };
static Sock os_wake_open(void) { if (pipe2(g_os_wake, 0x800 | 0x80000) != 0) return OS_BAD_SOCK; return (Sock)(u32)g_os_wake[0]; } /* O_NONBLOCK | O_CLOEXEC */
static void os_wake_ping(void) { (void)write(g_os_wake[1], "w", 1); }
static void os_wake_drain(void) { char b[64]; while (read(g_os_wake[0], b, sizeof b) > 0) {} }
#endif
