/* ---------------------------------------------------------------- stored data: tables in SQLite (docs/persistence.md)
   Included (after the layout, before the runtime) in a program that has a `table<T>`; the bundled SQLite (third_party/sqlite) is linked
   in. The database is `<data dir>/data.db` (a system's workers: `<kind>-<index>.db`), in WAL mode, synced fully on every commit. At start
   the program's tables are created, or migrated by name: a column added, a column removed dropped; a column whose type changed, or rows
   that no longer fit a declared limit (text too long, a number outside its range), refuse to start, naming it. Every request is one
   transaction, committed before its answer (or rolled back with the working state's undo log). SQLite's memory is one fixed pool. */
typedef struct sqlite3 sqlite3;
typedef struct sqlite3_stmt sqlite3_stmt;
int sqlite3_config(int, ...);
int sqlite3_open_v2(const char *, sqlite3 **, int, const char *);
int sqlite3_close(sqlite3 *);
int sqlite3_exec(sqlite3 *, const char *, int (*)(void *, int, char **, char **), void *, char **);
int sqlite3_prepare_v2(sqlite3 *, const char *, int, sqlite3_stmt **, const char **);
int sqlite3_step(sqlite3_stmt *);
int sqlite3_reset(sqlite3_stmt *);
int sqlite3_finalize(sqlite3_stmt *);
int sqlite3_bind_text(sqlite3_stmt *, int, const char *, int, void (*)(void *));
i64 sqlite3_column_int64(sqlite3_stmt *, int);
const unsigned char *sqlite3_column_text(sqlite3_stmt *, int);
int sqlite3_column_bytes(sqlite3_stmt *, int);
const char *sqlite3_errmsg(sqlite3 *);
#define SQ_OK 0
#define SQ_ROW 100
#define SQ_DONE 101
#define SQ_CONFIG_HEAP 8

/* A column as the program declares it: SQLite type (0 INTEGER, 1 REAL, 2 TEXT, 3 BLOB), a range to check (when has_range), the most
   bytes of text, or the exact bytes of a fixed array (a BLOB of 0 bytes is the array's zeros). */
typedef struct { const char *name; u32 type; int has_range; i64 lo, hi; u32 max; } StoreCol;
/* A table: its quoted SQL name, its CREATE TABLE, its owner (-1 none), its columns, its indexes (CREATE INDEX statements) */
typedef struct { const char *sql; const char *create; int parent; u32 ncols; const StoreCol *cols; const char *indexes; } StoreTable;

typedef struct { sqlite3 *db; sqlite3_stmt *begin, *commit, *rollback; int open; char err[1024]; } Store;

static const char *const sq_types[] = { "INTEGER", "REAL", "TEXT", "BLOB" };
static const char *const sq_defaults[] = { "0", "0.0", "''", "x''" };

/* a bounded text builder for SQL and messages */
typedef struct { char *p; u32 cap, n; } SqBuf;
static void sb_s(SqBuf *b, const char *s) { while (*s && b->n + 1 < b->cap) b->p[b->n++] = *s++; b->p[b->n] = 0; }
static void sb_i(SqBuf *b, i64 v) { char t[24]; int k = 0; u64 u = v < 0 ? (u64)0 - (u64)v : (u64)v; do { t[k++] = (char)('0' + u % 10); u /= 10; } while (u); if (v < 0) t[k++] = '-'; while (k) { char c[2] = { t[--k], 0 }; sb_s(b, c); } }
static void sb_q(SqBuf *b, const char *s) { sb_s(b, "\""); for (; *s; s++) { char c[2] = { *s, 0 }; if (*s == '"') sb_s(b, "\""); sb_s(b, c); } sb_s(b, "\""); }

static int st_fail(Store *st, const char *what, const char *detail) {
    SqBuf b = { st->err, sizeof st->err, 0 }; sb_s(&b, what); if (detail) { sb_s(&b, ": "); sb_s(&b, detail); }
    return 0;
}
static int st_exec(Store *st, const char *sql) { if (sqlite3_exec(st->db, sql, 0, 0, 0) != SQ_OK) return st_fail(st, sql, sqlite3_errmsg(st->db)); return 1; }
/* one integer from a query (-1 when it gives no row or fails) */
static i64 st_int(Store *st, const char *sql, const char *arg) {
    sqlite3_stmt *q; i64 v = -1;
    if (sqlite3_prepare_v2(st->db, sql, -1, &q, 0) != SQ_OK) return -1;
    if (arg) sqlite3_bind_text(q, 1, arg, -1, (void (*)(void *))0);
    if (sqlite3_step(q) == SQ_ROW) v = sqlite3_column_int64(q, 0);
    sqlite3_finalize(q); return v;
}
static int sq_streq(const char *a, const char *b) { while (*a && *a == *b) { a++; b++; } return *a == *b; }
/* a table's name without its quotes, as sqlite_schema keeps it */
static void sq_bare(const char *q, char *out, u32 cap) { u32 n = 0; for (q++; *q && n + 1 < cap; q++) { if (*q == '"') { if (q[1] == '"') q++; else break; } out[n++] = *q; } out[n] = 0; }

static int sq_configured;
/* Open (create) the database and bring its tables to the program's declarations. 1, or 0 with the reason in st->err. */
static int store_open(Store *st, const StoreTable *tabs, u32 ntabs, const char *dir, const char *file, u64 pool_mb) {
    char path[1200]; SqBuf p = { path, sizeof path, 0 }; sb_s(&p, dir);
    if (!os_mkdir(path)) return st_fail(st, "cannot make the data directory", path);
    sb_s(&p, "/"); sb_s(&p, file);
    if (!sq_configured) {
        /* SQLite's memory: one fixed pool for the process, given before anything opens */
        u64 n = (pool_mb ? pool_mb : 64) << 20;
        void *pool = os_alloc(n);
        if (!pool || sqlite3_config(SQ_CONFIG_HEAP, pool, (int)n, 64) != SQ_OK) return st_fail(st, "cannot give SQLite its memory pool", 0);
        sq_configured = 1;
    }
    if (sqlite3_open_v2(path, &st->db, 0x2 | 0x4, 0) != SQ_OK) return st_fail(st, path, st->db ? sqlite3_errmsg(st->db) : "cannot open"); /* READWRITE | CREATE */
    if (!st_exec(st, "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=OFF;") || !st_exec(st, "BEGIN IMMEDIATE")) return 0;
    char sqlb[4096], msg[1024], bare[300];
    for (u32 t = 0; t < ntabs; t++) {
        const StoreTable *tb = &tabs[t];
        sq_bare(tb->sql, bare, sizeof bare);
        if (st_int(st, "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1", bare) <= 0) { if (!st_exec(st, tb->create)) return 0; continue; }
        /* an existing table: its columns against the declared ones */
        SqBuf q = { sqlb, sizeof sqlb, 0 }; sb_s(&q, "SELECT name, type FROM pragma_table_info("); sb_s(&q, "'"); sb_s(&q, bare); sb_s(&q, "')");
        sqlite3_stmt *ti; if (sqlite3_prepare_v2(st->db, sqlb, -1, &ti, 0) != SQ_OK) return st_fail(st, sqlb, sqlite3_errmsg(st->db));
        int has_parent = 0; char drop[32][128]; u32 ndrop = 0; u32 seen[256] = { 0 };
        while (sqlite3_step(ti) == SQ_ROW) {
            const char *cn = (const char *)sqlite3_column_text(ti, 0), *ct = (const char *)sqlite3_column_text(ti, 1);
            if (sq_streq(cn, "id")) continue;
            if (sq_streq(cn, "parent_id")) { has_parent = 1; continue; }
            u32 k = 0; while (k < tb->ncols && !sq_streq(tb->cols[k].name, cn)) k++;
            if (k == tb->ncols) { if (ndrop < 32) { SqBuf d = { drop[ndrop], 128, 0 }; sb_s(&d, cn); ndrop++; } continue; }
            if (k < 256) seen[k] = 1;
            if (!sq_streq(ct, sq_types[tb->cols[k].type])) {
                SqBuf m = { msg, sizeof msg, 0 }; sb_s(&m, "column "); sb_s(&m, tb->sql); sb_s(&m, "."); sb_s(&m, cn); sb_s(&m, " is "); sb_s(&m, ct); sb_s(&m, " in the database, ");
                sb_s(&m, sq_types[tb->cols[k].type]); sb_s(&m, " in the program");
                sqlite3_finalize(ti); return st_fail(st, "the database does not fit the program", msg);
            }
        }
        sqlite3_finalize(ti);
        if (has_parent != (tb->parent >= 0)) { SqBuf m = { msg, sizeof msg, 0 }; sb_s(&m, "table "); sb_s(&m, tb->sql); sb_s(&m, has_parent ? " belongs to another table's rows in the database, not in the program" : " belongs to another table's rows in the program, not in the database"); return st_fail(st, "the database does not fit the program", msg); }
        /* our indexes go (they are made again below); removed columns are dropped, new ones added */
        { SqBuf d = { sqlb, sizeof sqlb, 0 }; sb_s(&d, "SELECT group_concat('DROP INDEX \"' || replace(name, '\"', '\"\"') || '\";', ' ') FROM sqlite_schema WHERE type = 'index' AND sql IS NOT NULL AND tbl_name = ?1");
          sqlite3_stmt *di; if (sqlite3_prepare_v2(st->db, sqlb, -1, &di, 0) == SQ_OK) { sqlite3_bind_text(di, 1, bare, -1, (void (*)(void *))0);
            if (sqlite3_step(di) == SQ_ROW && sqlite3_column_bytes(di, 0) > 0) { char drops[4096]; SqBuf x = { drops, sizeof drops, 0 }; sb_s(&x, (const char *)sqlite3_column_text(di, 0)); sqlite3_finalize(di); if (!st_exec(st, drops)) return 0; } else sqlite3_finalize(di); } }
        for (u32 k = 0; k < ndrop; k++) { SqBuf a = { sqlb, sizeof sqlb, 0 }; sb_s(&a, "ALTER TABLE "); sb_s(&a, tb->sql); sb_s(&a, " DROP COLUMN "); sb_q(&a, drop[k]); if (!st_exec(st, sqlb)) return 0; }
        for (u32 k = 0; k < tb->ncols; k++) if (k >= 256 || !seen[k]) {
            SqBuf a = { sqlb, sizeof sqlb, 0 }; sb_s(&a, "ALTER TABLE "); sb_s(&a, tb->sql); sb_s(&a, " ADD COLUMN "); sb_q(&a, tb->cols[k].name);
            sb_s(&a, " "); sb_s(&a, sq_types[tb->cols[k].type]); sb_s(&a, " NOT NULL DEFAULT "); sb_s(&a, sq_defaults[tb->cols[k].type]);
            if (!st_exec(st, sqlb)) return 0;
        }
    }
    /* tables in the database that the program no longer has: dropped */
    { sqlite3_stmt *lt; char names[64][300]; u32 nn = 0;
      if (sqlite3_prepare_v2(st->db, "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'", -1, &lt, 0) == SQ_OK) {
          while (sqlite3_step(lt) == SQ_ROW && nn < 64) { const char *n = (const char *)sqlite3_column_text(lt, 0); int ours = 0;
              for (u32 t = 0; t < ntabs && !ours; t++) { sq_bare(tabs[t].sql, bare, sizeof bare); ours = sq_streq(bare, n); }
              if (!ours) { SqBuf c = { names[nn], 300, 0 }; sb_s(&c, n); nn++; } }
          sqlite3_finalize(lt); }
      for (u32 k = 0; k < nn; k++) { SqBuf a = { sqlb, sizeof sqlb, 0 }; sb_s(&a, "DROP TABLE "); sb_q(&a, names[k]); if (!st_exec(st, sqlb)) return 0; } }
    /* rows written while the server was stopped: each column within the program's limits */
    for (u32 t = 0; t < ntabs; t++) for (u32 k = 0; k < tabs[t].ncols; k++) {
        const StoreCol *c = &tabs[t].cols[k];
        SqBuf a = { sqlb, sizeof sqlb, 0 }; sb_s(&a, "SELECT count(*) FROM "); sb_s(&a, tabs[t].sql); sb_s(&a, " WHERE ");
        if (c->has_range) { sb_q(&a, c->name); sb_s(&a, " < "); sb_i(&a, c->lo); sb_s(&a, " OR "); sb_q(&a, c->name); sb_s(&a, " > "); sb_i(&a, c->hi); }
        else if (c->type == 2) { sb_s(&a, "length(CAST("); sb_q(&a, c->name); sb_s(&a, " AS BLOB)) > "); sb_i(&a, c->max); }
        else if (c->type == 3) { sb_s(&a, "length("); sb_q(&a, c->name); sb_s(&a, ") NOT IN (0, "); sb_i(&a, c->max); sb_s(&a, ")"); }
        else continue;
        i64 bad = st_int(st, sqlb, 0);
        if (bad != 0) {
            SqBuf m = { msg, sizeof msg, 0 }; sb_i(&m, bad); sb_s(&m, bad == 1 ? " row of " : " rows of "); sb_s(&m, tabs[t].sql); sb_s(&m, bad == 1 ? " holds a " : " hold a "); sb_s(&m, c->name);
            if (c->has_range) { sb_s(&m, " outside "); sb_i(&m, c->lo); sb_s(&m, ".."); sb_i(&m, c->hi); }
            else if (c->type == 2) { sb_s(&m, " longer than "); sb_i(&m, c->max); sb_s(&m, " bytes"); }
            else { sb_s(&m, " that is not "); sb_i(&m, c->max); sb_s(&m, " bytes"); }
            return st_fail(st, "the database does not fit the program", msg);
        }
    }
    for (u32 t = 0; t < ntabs; t++) if (tabs[t].indexes[0] && !st_exec(st, tabs[t].indexes)) return 0;
    if (!st_exec(st, "COMMIT") || !st_exec(st, "PRAGMA foreign_keys=ON")) return 0;
    if (sqlite3_prepare_v2(st->db, "BEGIN", -1, &st->begin, 0) != SQ_OK || sqlite3_prepare_v2(st->db, "COMMIT", -1, &st->commit, 0) != SQ_OK ||
        sqlite3_prepare_v2(st->db, "ROLLBACK", -1, &st->rollback, 0) != SQ_OK) return st_fail(st, "cannot prepare", sqlite3_errmsg(st->db));
    st->open = 1;
    return 1;
}
static void store_close(Store *st) {
    if (!st->db) return;
    if (st->begin) sqlite3_finalize(st->begin); if (st->commit) sqlite3_finalize(st->commit); if (st->rollback) sqlite3_finalize(st->rollback);
    sqlite3_close(st->db); st->db = 0; st->open = 0;
}
/* a request's transaction: begin before it runs; commit before its answer (0 failed: the request fails); roll back when it failed */
static int store_begin(Store *st) { if (!st->open) return 1; int r = sqlite3_step(st->begin); sqlite3_reset(st->begin); return r == SQ_DONE; }
static int store_commit(Store *st) { if (!st->open) return 1; int r = sqlite3_step(st->commit); sqlite3_reset(st->commit); if (r != SQ_DONE) { st_fail(st, "commit failed", sqlite3_errmsg(st->db)); sqlite3_step(st->rollback); sqlite3_reset(st->rollback); return 0; } return 1; }
static void store_rollback(Store *st) { if (!st->open) return; sqlite3_step(st->rollback); sqlite3_reset(st->rollback); }
