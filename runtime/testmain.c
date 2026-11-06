/* ---- the test runner (`mito test`): every test of the program on a fresh instance, time and randomness fixed ----
   argv[1]: the program's file name, for the places of failures. Prints one line per test (and where a failing one failed);
   exits 1 if any failed. */
int printf(const char *f, ...);
int fflush(void *f);
int main(int argc, char **argv) {
    const char *file = argc > 1 ? argv[1] : "program";
    u32 n = mito_test_count(), failed = 0;
    static char msg[4400];
    for (u32 k = 0; k < n; k++) {
        void *m = mito_start(1024ULL << 20, 1);
        if (!m) { printf("error: the program did not start (over budget or out of address space)\n"); return 2; }
        mito_set_clock(m, 1767225600000LL, 1); /* 2026-01-01 */
        { int sc = mito_begin(m); /* the program's `@start` function, as on a server */
          if (sc) { u32 en; const char *e = mito_response(m, &en); printf("error: the @start function failed (%d): %.*s\n", sc, (int)en, e); return 2; } }
        u32 line = 0;
        int r = mito_run_test(m, k, msg, sizeof msg, &line);
        if (r == 0) printf("ok    %s\n", mito_test_name(k));
        else { failed++; printf("FAIL  %s\n      %s:%u: %s%s\n", mito_test_name(k), file, line, r == 2 ? "error: " : "", msg); }
        mito_stop(m);
    }
    printf("%u passed, %u failed\n", n - failed, failed);
    fflush(0);
    return failed ? 1 : 0;
}
