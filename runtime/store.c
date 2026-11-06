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
int sqlite3_bind_int64(sqlite3_stmt *, int, i64);
int sqlite3_bind_null(sqlite3_stmt *, int);
int sqlite3_bind_double(sqlite3_stmt *, int, double);
int sqlite3_column_type(sqlite3_stmt *, int);
double sqlite3_column_double(sqlite3_stmt *, int);
i64 sqlite3_last_insert_rowid(sqlite3 *);
int sqlite3_extended_errcode(sqlite3 *);
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
/* ascii: text of bytes 0..127 only; decl: the column's declaration after its name; refers: the table a reference names (-1 none) */
typedef struct { const char *name; u32 type; int has_range; i64 lo, hi; u32 max; int ascii; const char *decl; int refers; } StoreCol;
/* A table: its quoted SQL name, its CREATE TABLE, its owner (-1 none), its columns, its indexes (CREATE INDEX statements) */
typedef struct { const char *sql; const char *create; int parent; u32 ncols; const StoreCol *cols; const char *indexes; } StoreTable;

/* A table's statements, prepared once after migration: its row's columns by id, insert (with the owner's id for a nested table), delete,
   count (of one owner's rows), find (a row of one owner), and one update per column. `secret`: the column a session token checks, or -1. */
typedef struct { sqlite3_stmt *row, *ins, *del, *cnt, *find, *clr, *has, **upd; int secret; } TStmts;
/* A loop's cursor: its statement (a cached one, or its own when the cached one is in use by an enclosing loop), the table of its rows. */
typedef struct { sqlite3_stmt *s; int own, on; u32 t; } SqCursor;
typedef struct { const char *sql; sqlite3_stmt *s; int busy; } SqCached;
#define SQ_CURSORS 32
#define SQ_CACHED 128
/* The row cache: the last row read (table, id) and the statement holding it, `off` the column of its first field (1 in a cursor's
   statement, whose column 0 is the id); any write forgets it. */
typedef struct { sqlite3 *db; sqlite3_stmt *begin, *commit, *rollback; int open; char err[1024];
    const StoreTable *tabs; u32 ntabs; TStmts *ts; SqCursor cur[SQ_CURSORS]; SqCached cached[SQ_CACHED]; u32 ncached;
    sqlite3_stmt *row; u32 row_t, row_id, row_off; } Store;

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

static u32 c_strlen_sq(const char *s) { u32 n = 0; while (s[n]) n++; return n; }
static int sq_configured;
/* Open (create) the database and bring its tables to the program's declarations. 1, or 0 with the reason in st->err. */
/* What does not fit, dropped deliberately (`--db-fit drop`): said on standard error, one line each. */
static void st_dropped(const char *what) { os_err("dropped: ", 9); u32 n = 0; while (what[n]) n++; os_err(what, n); os_err("\n", 1); }
/* flags: 1 `full`: each commit synced to disk before the request is answered (synchronous=FULL; an answered change survives a power cut),
   else WAL commits synced at checkpoints (NORMAL: a process crash loses nothing, an OS crash or power cut can lose the last moments, never
   corrupts); 2 `drop`: what does not fit the program is dropped (a column whose type changed loses its values, a table whose owner changed
   its rows, rows past a declared limit are deleted with the rows they own), each said on standard error; without it the server refuses. */
static int store_open(Store *st, const StoreTable *tabs, u32 ntabs, const char *dir, const char *file, u64 pool_mb, int flags) {
    int full = flags & 1, dropmis = (flags & 2) != 0;
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
    if (!st_exec(st, full ? "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=OFF;" : "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=OFF;") || !st_exec(st, "BEGIN IMMEDIATE")) return 0;
    char sqlb[4096], msg[1024], bare[300];
    for (u32 t = 0; t < ntabs; t++) {
        const StoreTable *tb = &tabs[t];
        sq_bare(tb->sql, bare, sizeof bare);
        if (st_int(st, "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1", bare) <= 0) { if (!st_exec(st, tb->create)) return 0; continue; }
        /* the owner changed (a table now nested, or no longer): refused, or (dropping) the table made again, empty */
        { SqBuf q = { sqlb, sizeof sqlb, 0 }; sb_s(&q, "SELECT count(*) FROM pragma_table_info('"); sb_s(&q, bare); sb_s(&q, "') WHERE name = 'parent_id'");
          int had = st_int(st, sqlb, 0) > 0;
          if (had != (tb->parent >= 0)) {
              SqBuf m = { msg, sizeof msg, 0 }; sb_s(&m, "table "); sb_s(&m, tb->sql); sb_s(&m, had ? " belongs to another table's rows in the database, not in the program" : " belongs to another table's rows in the program, not in the database");
              if (!dropmis) return st_fail(st, "the database does not fit the program (--db-fit drop drops what does not fit)", msg);
              st_dropped(msg);
              SqBuf a = { sqlb, sizeof sqlb, 0 }; sb_s(&a, "DROP TABLE "); sb_s(&a, tb->sql); if (!st_exec(st, sqlb) || !st_exec(st, tb->create)) return 0;
              continue;
          } }
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
                if (!dropmis) { sqlite3_finalize(ti); return st_fail(st, "the database does not fit the program (--db-fit drop drops what does not fit)", msg); }
                /* dropping: the column goes and comes back with its new type, every row with its first value */
                st_dropped(msg);
                if (k < 256) seen[k] = 0;
                if (ndrop < 32) { SqBuf d = { drop[ndrop], 128, 0 }; sb_s(&d, cn); ndrop++; }
            }
        }
        sqlite3_finalize(ti);
        (void)has_parent; /* checked above */
        /* our indexes go (they are made again below); removed columns are dropped, new ones added */
        { SqBuf d = { sqlb, sizeof sqlb, 0 }; sb_s(&d, "SELECT group_concat('DROP INDEX \"' || replace(name, '\"', '\"\"') || '\";', ' ') FROM sqlite_schema WHERE type = 'index' AND sql IS NOT NULL AND tbl_name = ?1");
          sqlite3_stmt *di; if (sqlite3_prepare_v2(st->db, sqlb, -1, &di, 0) == SQ_OK) { sqlite3_bind_text(di, 1, bare, -1, (void (*)(void *))0);
            if (sqlite3_step(di) == SQ_ROW && sqlite3_column_bytes(di, 0) > 0) { char drops[4096]; SqBuf x = { drops, sizeof drops, 0 }; sb_s(&x, (const char *)sqlite3_column_text(di, 0)); sqlite3_finalize(di); if (!st_exec(st, drops)) return 0; } else sqlite3_finalize(di); } }
        for (u32 k = 0; k < ndrop; k++) { SqBuf a = { sqlb, sizeof sqlb, 0 }; sb_s(&a, "ALTER TABLE "); sb_s(&a, tb->sql); sb_s(&a, " DROP COLUMN "); sb_q(&a, drop[k]); if (!st_exec(st, sqlb)) return 0; }
        for (u32 k = 0; k < tb->ncols; k++) if (k >= 256 || !seen[k]) {
            SqBuf a = { sqlb, sizeof sqlb, 0 }; sb_s(&a, "ALTER TABLE "); sb_s(&a, tb->sql); sb_s(&a, " ADD COLUMN "); sb_q(&a, tb->cols[k].name);
            sb_s(&a, " "); sb_s(&a, tb->cols[k].decl);
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
        else if (c->type == 2) {
            sb_s(&a, "length(CAST("); sb_q(&a, c->name); sb_s(&a, " AS BLOB)) > "); sb_i(&a, c->max);
            /* ascii: every character one byte (a text's byte length is its character length only when all are ASCII) */
            if (c->ascii) { sb_s(&a, " OR length(CAST("); sb_q(&a, c->name); sb_s(&a, " AS BLOB)) != length("); sb_q(&a, c->name); sb_s(&a, ")"); }
        }
        else if (c->type == 3) { sb_s(&a, "length("); sb_q(&a, c->name); sb_s(&a, ") NOT IN (0, "); sb_i(&a, c->max); sb_s(&a, ")"); }
        else continue;
        i64 bad = st_int(st, sqlb, 0);
        if (bad > 0 && dropmis) {
            /* dropping: those rows deleted (and below, the rows they owned) */
            SqBuf m = { msg, sizeof msg, 0 }; sb_i(&m, bad); sb_s(&m, bad == 1 ? " row of " : " rows of "); sb_s(&m, tabs[t].sql); sb_s(&m, " past the limit of "); sb_s(&m, c->name);
            st_dropped(msg);
            char del[4200]; SqBuf d = { del, sizeof del, 0 }; sb_s(&d, "DELETE FROM "); sb_s(&d, tabs[t].sql); sb_s(&d, sqlb + 21 + (u32)c_strlen_sq(tabs[t].sql));
            if (!st_exec(st, del)) return 0;
            continue;
        }
        if (bad != 0) {
            SqBuf m = { msg, sizeof msg, 0 }; sb_i(&m, bad); sb_s(&m, bad == 1 ? " row of " : " rows of "); sb_s(&m, tabs[t].sql); sb_s(&m, bad == 1 ? " holds a " : " hold a "); sb_s(&m, c->name);
            if (c->has_range) { sb_s(&m, " outside "); sb_i(&m, c->lo); sb_s(&m, ".."); sb_i(&m, c->hi); }
            else if (c->type == 2) { sb_s(&m, " longer than "); sb_i(&m, c->max); sb_s(&m, c->ascii ? " bytes or not ASCII" : " bytes"); }
            else { sb_s(&m, " that is not "); sb_i(&m, c->max); sb_s(&m, " bytes"); }
            return st_fail(st, "the database does not fit the program (--db-fit drop drops what does not fit)", msg);
        }
    }
    /* references to rows that are not there (written while the server was stopped): refused, or (dropping) set to null */
    for (u32 t = 0; t < ntabs; t++) for (u32 k = 0; k < tabs[t].ncols; k++) {
        const StoreCol *c = &tabs[t].cols[k]; if (c->refers < 0) continue;
        SqBuf a = { sqlb, sizeof sqlb, 0 }; sb_s(&a, " WHERE "); sb_q(&a, c->name); sb_s(&a, " IS NOT NULL AND "); sb_q(&a, c->name); sb_s(&a, " NOT IN (SELECT id FROM "); sb_s(&a, tabs[c->refers].sql); sb_s(&a, ")");
        char q[4400]; SqBuf b2 = { q, sizeof q, 0 }; sb_s(&b2, "SELECT count(*) FROM "); sb_s(&b2, tabs[t].sql); sb_s(&b2, sqlb);
        i64 bad = st_int(st, q, 0);
        if (bad <= 0) continue;
        SqBuf m = { msg, sizeof msg, 0 }; sb_i(&m, bad); sb_s(&m, bad == 1 ? " row of " : " rows of "); sb_s(&m, tabs[t].sql); sb_s(&m, bad == 1 ? " names a row of " : " name a row of "); sb_s(&m, tabs[c->refers].sql); sb_s(&m, " that is not there ("); sb_s(&m, c->name); sb_s(&m, ")");
        if (!dropmis) return st_fail(st, "the database does not fit the program (--db-fit drop drops what does not fit)", msg);
        st_dropped(msg);
        b2.n = 0; sb_s(&b2, "UPDATE "); sb_s(&b2, tabs[t].sql); sb_s(&b2, " SET "); sb_q(&b2, c->name); sb_s(&b2, " = NULL"); sb_s(&b2, sqlb);
        if (!st_exec(st, q)) return 0;
    }
    /* rows whose owner is gone (deleted above while foreign keys are off during migration): deleted, owners before what they own */
    if (dropmis) for (u32 t = 0; t < ntabs; t++) if (tabs[t].parent >= 0) {
        SqBuf a = { sqlb, sizeof sqlb, 0 }; sb_s(&a, "DELETE FROM "); sb_s(&a, tabs[t].sql); sb_s(&a, " WHERE parent_id NOT IN (SELECT id FROM "); sb_s(&a, tabs[tabs[t].parent].sql); sb_s(&a, ")");
        if (!st_exec(st, sqlb)) return 0;
    }
    for (u32 t = 0; t < ntabs; t++) if (tabs[t].indexes[0] && !st_exec(st, tabs[t].indexes)) return 0;
    if (!st_exec(st, "COMMIT") || !st_exec(st, "PRAGMA foreign_keys=ON")) return 0;
    if (sqlite3_prepare_v2(st->db, "BEGIN", -1, &st->begin, 0) != SQ_OK || sqlite3_prepare_v2(st->db, "COMMIT", -1, &st->commit, 0) != SQ_OK ||
        sqlite3_prepare_v2(st->db, "ROLLBACK", -1, &st->rollback, 0) != SQ_OK) return st_fail(st, "cannot prepare", sqlite3_errmsg(st->db));
    /* each table's statements */
    st->tabs = tabs; st->ntabs = ntabs;
    st->ts = (TStmts *)os_alloc(sizeof(TStmts) * (ntabs ? ntabs : 1));
    if (!st->ts) return st_fail(st, "out of memory for statements", 0);
    for (u32 t = 0; t < ntabs; t++) {
        const StoreTable *tb = &tabs[t]; TStmts *x = &st->ts[t];
        x->upd = (sqlite3_stmt **)os_alloc(sizeof(sqlite3_stmt *) * (tb->ncols ? tb->ncols : 1));
        x->secret = -1;
        SqBuf a = { sqlb, sizeof sqlb, 0 }; sb_s(&a, "SELECT ");
        if (!tb->ncols) sb_s(&a, "1");
        for (u32 k = 0; k < tb->ncols; k++) { if (k) sb_s(&a, ", "); sb_q(&a, tb->cols[k].name); if (sq_streq(tb->cols[k].name, "secret") && tb->cols[k].type == 0) x->secret = (int)k; }
        sb_s(&a, " FROM "); sb_s(&a, tb->sql); sb_s(&a, " WHERE id = ?1");
        if (sqlite3_prepare_v2(st->db, sqlb, -1, &x->row, 0) != SQ_OK) return st_fail(st, sqlb, sqlite3_errmsg(st->db));
        a.n = 0; sb_s(&a, "INSERT INTO "); sb_s(&a, tb->sql); sb_s(&a, tb->parent >= 0 ? "(parent_id) VALUES (?1)" : " DEFAULT VALUES");
        if (sqlite3_prepare_v2(st->db, sqlb, -1, &x->ins, 0) != SQ_OK) return st_fail(st, sqlb, sqlite3_errmsg(st->db));
        a.n = 0; sb_s(&a, "DELETE FROM "); sb_s(&a, tb->sql); sb_s(&a, " WHERE id = ?1");
        if (sqlite3_prepare_v2(st->db, sqlb, -1, &x->del, 0) != SQ_OK) return st_fail(st, sqlb, sqlite3_errmsg(st->db));
        a.n = 0; sb_s(&a, "SELECT count(*) FROM "); sb_s(&a, tb->sql); if (tb->parent >= 0) sb_s(&a, " WHERE parent_id = ?1");
        if (sqlite3_prepare_v2(st->db, sqlb, -1, &x->cnt, 0) != SQ_OK) return st_fail(st, sqlb, sqlite3_errmsg(st->db));
        a.n = 0; sb_s(&a, "SELECT 1 FROM "); sb_s(&a, tb->sql); sb_s(&a, " WHERE id = ?1"); if (tb->parent >= 0) sb_s(&a, " AND parent_id = ?2");
        if (sqlite3_prepare_v2(st->db, sqlb, -1, &x->find, 0) != SQ_OK) return st_fail(st, sqlb, sqlite3_errmsg(st->db));
        a.n = 0; sb_s(&a, "DELETE FROM "); sb_s(&a, tb->sql); if (tb->parent >= 0) sb_s(&a, " WHERE parent_id = ?1");
        if (sqlite3_prepare_v2(st->db, sqlb, -1, &x->clr, 0) != SQ_OK) return st_fail(st, sqlb, sqlite3_errmsg(st->db));
        a.n = 0; sb_s(&a, "SELECT 1 FROM "); sb_s(&a, tb->sql); sb_s(&a, " WHERE id = ?1");
        if (sqlite3_prepare_v2(st->db, sqlb, -1, &x->has, 0) != SQ_OK) return st_fail(st, sqlb, sqlite3_errmsg(st->db));
        for (u32 k = 0; k < tb->ncols; k++) {
            a.n = 0; sb_s(&a, "UPDATE "); sb_s(&a, tb->sql); sb_s(&a, " SET "); sb_q(&a, tb->cols[k].name); sb_s(&a, " = ?1 WHERE id = ?2");
            if (sqlite3_prepare_v2(st->db, sqlb, -1, &x->upd[k], 0) != SQ_OK) return st_fail(st, sqlb, sqlite3_errmsg(st->db));
        }
    }
    st->open = 1;
    return 1;
}
/* the end of a request: every statement reset (a transaction cannot end with one in progress), cursors' own statements finalized */
static void store_settle(Store *st) {
    for (u32 k = 0; k < SQ_CURSORS; k++) if (st->cur[k].on) { if (st->cur[k].own) sqlite3_finalize(st->cur[k].s); else sqlite3_reset(st->cur[k].s); st->cur[k].on = 0; }
    for (u32 k = 0; k < st->ncached; k++) st->cached[k].busy = 0;
    if (st->row) { sqlite3_reset(st->row); st->row = 0; }
}
static void store_forget_row(Store *st) { if (st->row) { if (st->row_off == 0) sqlite3_reset(st->row); st->row = 0; } }
/* A loop's statement (`sql`, a literal of the program: kept by its address), ready to bind: the cached one, or its own when an enclosing
   loop is using that one. 0, or -1 when there are too many loops open at once or it does not prepare. */
static int store_cursor(Store *st, const char *sql, u32 t, u32 *out) {
    u32 k = 0; while (k < SQ_CURSORS && st->cur[k].on) k++;
    if (k == SQ_CURSORS) { st_fail(st, "more than 32 loops over tables open at once", 0); return -1; }
    SqCached *c = 0;
    for (u32 i = 0; i < st->ncached && !c; i++) if (st->cached[i].sql == sql) c = &st->cached[i];
    if (!c && st->ncached < SQ_CACHED) { c = &st->cached[st->ncached]; c->sql = sql; c->busy = 0; if (sqlite3_prepare_v2(st->db, sql, -1, &c->s, 0) != SQ_OK) { st_fail(st, sql, sqlite3_errmsg(st->db)); return -1; } st->ncached++; }
    if (c && !c->busy) { c->busy = 1; st->cur[k].s = c->s; st->cur[k].own = 0; }
    else { if (sqlite3_prepare_v2(st->db, sql, -1, &st->cur[k].s, 0) != SQ_OK) { st_fail(st, sql, sqlite3_errmsg(st->db)); return -1; } st->cur[k].own = 1; }
    st->cur[k].on = 1; st->cur[k].t = t; *out = k;
    return 0;
}
static void store_close(Store *st) {
    if (!st->db) return;
    if (st->begin) sqlite3_finalize(st->begin); if (st->commit) sqlite3_finalize(st->commit); if (st->rollback) sqlite3_finalize(st->rollback);
    sqlite3_close(st->db); st->db = 0; st->open = 0;
}
/* a request's transaction: begin before it runs; commit before its answer (0 failed: the request fails); roll back when it failed */
static int store_begin(Store *st) { if (!st->open) return 1; int r = sqlite3_step(st->begin); sqlite3_reset(st->begin); return r == SQ_DONE; }
static int store_commit(Store *st) { if (!st->open) return 1; store_settle(st); int r = sqlite3_step(st->commit); sqlite3_reset(st->commit); if (r != SQ_DONE) { st_fail(st, "commit failed", sqlite3_errmsg(st->db)); sqlite3_step(st->rollback); sqlite3_reset(st->rollback); return 0; } return 1; }
static void store_rollback(Store *st) { if (!st->open) return; store_settle(st); sqlite3_step(st->rollback); sqlite3_reset(st->rollback); }
