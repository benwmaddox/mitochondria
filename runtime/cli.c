/* ---- the command-line driver: `main` of a program whose entry is `@main` ----
   The command line is the request. A positional argument N is the input `from arg(N)` (header ":argN"), what is piped in is the input
   `from stdin` (header ":stdin"; read only when the program has one), and `--name value`, `--name=value` or a bare `--name` (true)
   is the input called `name` (a short flag, `-c`, is the input `c`, true). The answer's body is written to standard output and the exit code is 0; an answer of status 400 or
   more goes to standard error with exit code 1; `exit(n, body)` answers with exit code n (body on standard output), `exit_error(n, body)` with exit code n and the body on
   standard error (the body as it is, no line break added); a program that fails to start exits 2. Memory is the program's own declared state. */
int mito_handle(void *m, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *h, u32 nh, const MitoPair *f, u32 nf, const char *body, u32 bn);
const char *mito_response(void *m, u32 *len);
void *mito_start(u64 budget, u64 seed);
int mito_begin(void *m);
int mito_has_tables(void);
u64 strlen(const char *s);

#define CLI_STDIN_MAX 0xFFFFFFFFu /* a length is 32 bits */
#define CLI_READ_CHUNK (16u << 20)
void mito_set_areas(u64 text_max, u64 out_max);
void mito_set_loop_budget(void *m, i64 n);
u8 *mito_text_room(void *m, u64 n);
void mito_text_input(void *m, u64 n);
void mito_set_cli(void *m);
#define CLI_TARGET_MAX 8192

/* a flag's name or value, percent-encoded into the target; 0 when it does not fit */
static int cli_enc(char *t, u32 *n, const char *s, u32 len) {
    static const char hex[] = "0123456789ABCDEF";
    for (u32 i = 0; i < len; i++) {
        u8 c = (u8)s[i];
        int plain = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '-' || c == '_' || c == '.' || c == '~';
        if (*n + 3 >= CLI_TARGET_MAX) return 0;
        if (plain) t[(*n)++] = (char)c;
        else { t[(*n)++] = '%'; t[(*n)++] = hex[c >> 4]; t[(*n)++] = hex[c & 15]; }
    }
    return 1;
}

int main(int argc, char **argv) {
    static MitoPair pairs[35];
    static char names[32][8];
    static char target[CLI_TARGET_MAX];
    u32 np = 0, tn = 0, npos = 0;
    int flags = 0;
#ifdef MITO_CLI_ARGS
    const int raw = 1; /* the program reads `from args`: it parses its own flags, so every argument is a plain one */
    static char argbuf[1 << 20];
    u32 an = 0;
#else
    const int raw = 0;
#endif
    target[tn++] = '/';
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
        if (!raw && a[0] == '-' && ((a[1] >= 'a' && a[1] <= 'z') || (a[1] >= 'A' && a[1] <= 'Z')) && !a[2]) {
            /* a short flag, `-c`: the input called `c`, true */
            target[tn++] = flags ? '&' : '?';
            flags = 1;
            target[tn++] = a[1]; target[tn++] = '='; target[tn++] = 't'; target[tn++] = 'r'; target[tn++] = 'u'; target[tn++] = 'e';
        } else if (!raw && a[0] == '-' && a[1] == '-' && a[2]) {
            const char *name = a + 2, *value = "true";
            u32 nl = (u32)strlen(name);
            const char *eq = (const char *)memchr(name, '=', nl);
            if (eq) { value = eq + 1; nl = (u32)(eq - name); }
            else if (i + 1 < argc && !(argv[i + 1][0] == '-' && argv[i + 1][1] == '-')) value = argv[++i];
            target[tn++] = flags ? '&' : '?';
            flags = 1;
            if (!cli_enc(target, &tn, name, nl) || (target[tn++] = '=', !cli_enc(target, &tn, value, (u32)strlen(value)))) { os_err("error: the flags are too long\n", 30); return 2; }
        } else {
#ifdef MITO_CLI_ARGS
            { u32 l = (u32)strlen(a) + 1; if (an + l > sizeof argbuf) { os_err("error: the arguments are too long\n", 34); return 2; } memcpy(argbuf + an, a, l); an += l; }
            if (npos > 31) continue; /* only the first 32 are also `from arg(N)` */
#endif
            if (npos > 31) { os_err("error: at most 32 arguments\n", 28); return 2; }
            u32 k = npos++;
            names[k][0] = ':'; names[k][1] = 'a'; names[k][2] = 'r'; names[k][3] = 'g';
            u32 d = 4;
            if (k >= 10) names[k][d++] = (char)('0' + k / 10);
            names[k][d++] = (char)('0' + k % 10);
            pairs[np].k = names[k]; pairs[np].kn = d; pairs[np].v = a; pairs[np].vn = (u32)strlen(a);
            np++;
        }
    }
#ifdef MITO_CLI_ARGS
    pairs[np].k = ":args"; pairs[np].kn = 5; pairs[np].v = argbuf; pairs[np].vn = an; np++; /* every argument, each followed by a 0 byte */
#endif
    if (mito_has_tables()) { os_err("error: a command-line program has no tables yet\n", 48); return 2; }
    mito_set_areas(0xFFFFFFFFULL, 0xFFFFFFFFULL); /* the request text and the answer: 4 GiB - 1 of address space each, committed as used */
    void *app = mito_start(1ULL << 40, 0);
    if (!app) { os_err("error: out of address space\n", 28); return 2; }
    { int sc = mito_begin(app);
      if (sc) { u32 en; const char *e = mito_response(app, &en); os_err("error: ", 7); os_err(e, en); os_err("\n", 1); return 2; } }
#ifdef MITO_CLI_STDIN
    /* everything piped in, as the value of ":stdin" (a program that takes no input from stdin never waits for it). It is read straight into the request's
       text area (address space, committed as the input arrives: a small input costs a small amount), where the program's text input then points at it:
       the input is held once. The most it can hold is 4 GiB - 1 including the program's literals (a text offset is 32 bits). */
    u64 inn = 0;
    u8 *in = mito_text_room(app, 0);
    if (!in) { os_err("error: no memory for the input\n", 31); return 2; }
    for (;;) {
        u64 room = CLI_READ_CHUNK;
        while (room > 1 && !mito_text_room(app, inn + room)) room >>= 1;
        if (!mito_text_room(app, inn + room)) { /* the text area is full: more input is too much */
            u8 probe[1];
            if (os_in(probe, 1)) { os_err("error: the input is larger than 4294967295 bytes (4 GiB - 1)\n", 63); return 2; }
            break;
        }
        u32 k = os_in(in + inn, (u32)room);
        if (!k) break;
        inn += k;
    }
    mito_text_input(app, inn);
    pairs[np].k = ":stdin"; pairs[np].kn = 6; pairs[np].v = (const char *)in; pairs[np].vn = (u32)inn;
    np++;
    mito_set_loop_budget(app, 100000000 + 64 * (i64)inn); /* the step budget grows with the input: a pass over every byte of it takes several steps */
#endif
    mito_set_cli(app); /* from here the built-ins that touch the system (eprint, stdin_text, read_file, file_exists) may run */
    int code = mito_handle(app, "MAIN", 4, target, tn, pairs, np, 0, 0, "", 0);
    u32 bn; const char *body = mito_response(app, &bn);
    int exit_code = 0;
    if (code >= 1256 && code <= 1511) { os_err(body, bn); return code - 1256; } /* `exit_error(n, body)`: the body as it is on standard error, exit code n */
    if (code >= 1000 && code <= 1255) exit_code = code - 1000;                  /* `exit(n, body)`: the body on standard output, exit code n */
    else if (code >= 400) { os_err(body, bn); os_err("\n", 1); return 1; }
    while (bn) { u32 c = bn > CLI_READ_CHUNK ? CLI_READ_CHUNK : bn; os_out(body, c); body += c; bn -= c; }
    return exit_code;
}
