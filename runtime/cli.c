/* ---- the command-line driver: `main` of a program whose entry is `@main` ----
   The command line is the request. A positional argument N is the input `from arg(N)` (header ":argN"), what is piped in is the input
   `from stdin` (header ":stdin"; read only when the program has one), and `--name value`, `--name=value` or a bare `--name` (true)
   is the input called `name` (a short flag, `-c`, is the input `c`, true). The answer's body is written to standard output and the exit code is 0; an answer of status 400 or
   more goes to standard error with exit code 1; a program that fails to start exits 2. Memory is the program's own declared state. */
int mito_handle(void *m, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *h, u32 nh, const MitoPair *f, u32 nf, const char *body, u32 bn);
const char *mito_response(void *m, u32 *len);
void *mito_start(u64 budget, u64 seed);
int mito_begin(void *m);
int mito_has_tables(void);
u64 strlen(const char *s);

#define CLI_STDIN_MAX (64u << 20)
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
    static MitoPair pairs[33];
    static char names[32][8];
    static char target[CLI_TARGET_MAX];
    u32 np = 0, tn = 0, npos = 0;
    int flags = 0;
    target[tn++] = '/';
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
        if (a[0] == '-' && ((a[1] >= 'a' && a[1] <= 'z') || (a[1] >= 'A' && a[1] <= 'Z')) && !a[2]) {
            /* a short flag, `-c`: the input called `c`, true */
            target[tn++] = flags ? '&' : '?';
            flags = 1;
            target[tn++] = a[1]; target[tn++] = '='; target[tn++] = 't'; target[tn++] = 'r'; target[tn++] = 'u'; target[tn++] = 'e';
        } else if (a[0] == '-' && a[1] == '-' && a[2]) {
            const char *name = a + 2, *value = "true";
            u32 nl = (u32)strlen(name);
            const char *eq = (const char *)memchr(name, '=', nl);
            if (eq) { value = eq + 1; nl = (u32)(eq - name); }
            else if (i + 1 < argc && !(argv[i + 1][0] == '-' && argv[i + 1][1] == '-')) value = argv[++i];
            target[tn++] = flags ? '&' : '?';
            flags = 1;
            if (!cli_enc(target, &tn, name, nl) || (target[tn++] = '=', !cli_enc(target, &tn, value, (u32)strlen(value)))) { os_err("error: the flags are too long\n", 30); return 2; }
        } else {
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
#ifdef MITO_CLI_STDIN
    /* everything piped in, as the value of ":stdin" (a program that takes no input from stdin never waits for it) */
    u8 *in = (u8 *)os_alloc(CLI_STDIN_MAX + 1);
    u32 inn = 0;
    if (!in) { os_err("error: no memory for the input\n", 31); return 2; }
    for (;;) {
        u32 k = os_in(in + inn, CLI_STDIN_MAX + 1 - inn);
        if (!k) break;
        inn += k;
        if (inn > CLI_STDIN_MAX) { os_err("error: the input is larger than 64 MB\n", 39); return 2; }
    }
    pairs[np].k = ":stdin"; pairs[np].kn = 6; pairs[np].v = (const char *)in; pairs[np].vn = inn;
    np++;
#endif
    if (mito_has_tables()) { os_err("error: a command-line program has no tables yet\n", 48); return 2; }
    void *app = mito_start(256ULL << 20, 0);
    if (!app) { os_err("error: out of address space\n", 28); return 2; }
    { int sc = mito_begin(app);
      if (sc) { u32 en; const char *e = mito_response(app, &en); os_err("error: ", 7); os_err(e, en); os_err("\n", 1); return 2; } }
    int code = mito_handle(app, "MAIN", 4, target, tn, pairs, np, 0, 0, "", 0);
    u32 bn; const char *body = mito_response(app, &bn);
    if (code >= 400) { os_err(body, bn); os_err("\n", 1); return 1; }
    os_out(body, bn);
    return 0;
}
