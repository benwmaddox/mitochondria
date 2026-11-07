/* ---- HTML fragments: the node pool (Phase 8 of docs/roadmap.md) ----
   A program that uses `html_parse` gets this file in its unit, with runtime/html_parse.c and gumbo (third_party/gumbo) linked beside
   it. A request's nodes live in a pool of the instance's own (each reader has one: never shared state), reserved once and committed as
   it fills, emptied when the next request starts. A node is an index: elements (with attributes), text, CDATA, comments and fragments
   (a parse's result: its nodes are the fragment's children), linked to their parent and siblings; names, text and attribute values in
   the pool's bytes. Parsing is Nokogiri::HTML5's, as Rails gets it; writing a node back is Nokogiri's html_standard_serialize
   (ext/nokogiri/xml_node.c), so a program edits markup and writes it as Rails would. More than the pool's caps fails the request
   (413), naming the cap; nothing is cut. */

#define HP_NODES 131072u     /* nodes a request's documents may hold */
#define HP_ATTRS 262144u     /* attributes */
#define HP_BYTES (16u << 20) /* bytes of names, text and values */
#define HP_NONE 0xFFFFFFFFu

/* kind: 1 element, 3 text, 4 CDATA, 8 comment, 11 fragment (the DOM's node types); ns (an element's): 0 HTML, 1 SVG, 2 MathML */
typedef struct { u8 kind, ns; u32 parent, first, last, next, prev, name, name_n, text, text_n, attrs, nattrs; } HNode;
/* ns (an attribute's): 0 none, 1 xlink, 2 xml, 3 xmlns */
typedef struct { u32 name, name_n, value, value_n; u8 ns; } HAttr;
typedef struct { HNode *n; HAttr *a; u8 *b; u32 nn, na, nb; u64 cn, ca, cb; int ready; const char *err; } HPool;

/* the parser's side (runtime/html_parse.c) */
void *mg_parse(const char *s, unsigned long long n, const char *ctx, int ns, int form, int *status);
void mg_free(void *out);
void *mg_root(void *out);
int mg_type(void *node);
const char *mg_name(void *node);
int mg_ns(void *node);
const char *mg_text(void *node);
unsigned long long mg_nchildren(void *node);
void *mg_child(void *node, unsigned long long i);
unsigned long long mg_nattrs(void *node);
const char *mg_attr_name(void *node, unsigned long long i);
const char *mg_attr_value(void *node, unsigned long long i);
int mg_attr_ns(void *node, unsigned long long i);

static u32 hp_strlen(const char *s) { u32 n = 0; while (s[n]) n++; return n; }

/* the pool reserved (on the instance's first parse), then committed in 64 KB steps as it fills */
static int hp_ready(HPool *p) {
    if (p->ready) return 0;
    p->n = (HNode *)os_reserve((u64)HP_NODES * sizeof(HNode));
    p->a = (HAttr *)os_reserve((u64)HP_ATTRS * sizeof(HAttr));
    p->b = (u8 *)os_reserve(HP_BYTES);
    if (!p->n || !p->a || !p->b) { p->err = "html: the node pool could not be reserved"; return -1; }
    p->ready = 1; p->nn = p->na = p->nb = 0; p->cn = p->ca = p->cb = 0;
    return 0;
}
static void hp_reset(HPool *p) { p->nn = p->na = p->nb = 0; }
static int hp_commit(u8 *base, u64 *committed, u64 need) {
    if (need <= *committed) return 1;
    u64 c = (need + 65535) / 65536 * 65536;
    if (!os_commit(base + *committed, c - *committed)) return 0;
    *committed = c; return 1;
}
static int hp_node(HPool *p, u8 kind, u32 *id) {
    if (p->nn >= HP_NODES) { p->err = "html: a request's documents hold more than 131072 nodes"; return -1; }
    if (!hp_commit((u8 *)p->n, &p->cn, (u64)(p->nn + 1) * sizeof(HNode))) { p->err = "html: out of memory"; return -1; }
    HNode *x = &p->n[p->nn];
    x->kind = kind; x->ns = 0; x->parent = x->first = x->last = x->next = x->prev = HP_NONE;
    x->name = x->name_n = x->text = x->text_n = x->attrs = x->nattrs = 0;
    *id = p->nn++;
    return 0;
}
static int hp_bytes(HPool *p, const u8 *s, u32 n, u32 *off) {
    if ((u64)p->nb + n > HP_BYTES) { p->err = "html: a request's documents hold more than 16 MiB of text"; return -1; }
    if (!hp_commit(p->b, &p->cb, (u64)p->nb + n)) { p->err = "html: out of memory"; return -1; }
    if (n) memcpy(p->b + p->nb, s, n);
    *off = p->nb; p->nb += n;
    return 0;
}
static int hp_attr_room(HPool *p, u32 k) {
    if ((u64)p->na + k > HP_ATTRS) { p->err = "html: a request's documents hold more than 262144 attributes"; return -1; }
    if (!hp_commit((u8 *)p->a, &p->ca, (u64)(p->na + k) * sizeof(HAttr))) { p->err = "html: out of memory"; return -1; }
    return 0;
}
static int hp_ok(HPool *p, i64 id) { return id >= 0 && id < (i64)p->nn; }

/* ---- the tree's links ---- */
static void hp_detach(HPool *p, u32 id) {
    HNode *x = &p->n[id];
    if (x->parent == HP_NONE) return;
    HNode *par = &p->n[x->parent];
    if (x->prev != HP_NONE) p->n[x->prev].next = x->next; else par->first = x->next;
    if (x->next != HP_NONE) p->n[x->next].prev = x->prev; else par->last = x->prev;
    x->parent = x->prev = x->next = HP_NONE;
}
static void hp_append(HPool *p, u32 parent, u32 child) {
    hp_detach(p, child);
    HNode *par = &p->n[parent], *c = &p->n[child];
    c->parent = parent; c->prev = par->last; c->next = HP_NONE;
    if (par->last != HP_NONE) p->n[par->last].next = child; else par->first = child;
    par->last = child;
}
static void hp_insert_before(HPool *p, u32 ref, u32 node) {
    hp_detach(p, node);
    HNode *r = &p->n[ref], *x = &p->n[node];
    x->parent = r->parent; x->next = ref; x->prev = r->prev;
    if (r->prev != HP_NONE) p->n[r->prev].next = node; else p->n[r->parent].first = node;
    r->prev = node;
}
/* is `a` the node `b` or one of its ancestors (appending b's ancestor into b would make a loop) */
static int hp_within(HPool *p, u32 a, u32 b) { for (u32 x = b; x != HP_NONE; x = p->n[x].parent) if (x == a) return 1; return 0; }

/* ---- parsing ---- */
/* gumbo's node `g` copied as children of `parent` (adjacent text merged, as libxml2's xmlAddChild does in Nokogiri's build_tree) */
static int hp_build(HPool *p, u32 parent, void *g) {
    u64 nc = mg_nchildren(g);
    for (u64 i = 0; i < nc; i++) {
        void *c = mg_child(g, i);
        int t = mg_type(c);
        u32 id;
        if (t == 2 || t == 5) {
            const char *s = mg_text(c); u32 n = hp_strlen(s);
            u32 last = p->n[parent].last;
            if (last != HP_NONE && p->n[last].kind == 3) {
                /* merged: the two texts, one after the other */
                u32 off, a0 = p->n[last].text, an = p->n[last].text_n;
                if ((u64)p->nb + an + n > HP_BYTES) { p->err = "html: a request's documents hold more than 16 MiB of text"; return -1; }
                if (hp_bytes(p, p->b + a0, an, &off) || hp_bytes(p, (const u8 *)s, n, &a0)) return -1;
                p->n[last].text = off; p->n[last].text_n = an + n;
                continue;
            }
            if (hp_node(p, 3, &id) || hp_bytes(p, (const u8 *)s, n, &p->n[id].text)) return -1;
            p->n[id].text_n = n;
        } else if (t == 3 || t == 4) {
            const char *s = mg_text(c); u32 n = hp_strlen(s);
            if (hp_node(p, t == 3 ? 4 : 8, &id) || hp_bytes(p, (const u8 *)s, n, &p->n[id].text)) return -1;
            p->n[id].text_n = n;
        } else if (t == 1) {
            const char *nm = mg_name(c); u32 nn = hp_strlen(nm);
            if (hp_node(p, 1, &id) || hp_bytes(p, (const u8 *)nm, nn, &p->n[id].name)) return -1;
            p->n[id].name_n = nn; p->n[id].ns = (u8)mg_ns(c);
            u64 na = mg_nattrs(c);
            if (na) {
                if (hp_attr_room(p, (u32)na)) return -1;
                p->n[id].attrs = p->na; p->n[id].nattrs = (u32)na;
                for (u64 k = 0; k < na; k++) {
                    HAttr *x = &p->a[p->na++];
                    const char *an = mg_attr_name(c, k), *av = mg_attr_value(c, k);
                    x->ns = (u8)mg_attr_ns(c, k); x->name_n = hp_strlen(an); x->value_n = hp_strlen(av);
                    if (hp_bytes(p, (const u8 *)an, x->name_n, &x->name) || hp_bytes(p, (const u8 *)av, x->value_n, &x->value)) return -1;
                }
            }
            hp_append(p, parent, id);
            if (hp_build(p, id, c)) return -1;
            continue;
        } else continue;
        hp_append(p, parent, id);
    }
    return 0;
}
static int hp_name_is(HPool *p, u32 id, const char *s) {
    HNode *x = &p->n[id]; u32 n = hp_strlen(s);
    return x->kind == 1 && x->name_n == n && memcmp(p->b + x->name, s, n) == 0;
}
/* `s` parsed as a fragment in the context of node `ctx` (an element: its name and namespace, and whether it or an ancestor is an
   HTML form; anything else, or none, is a body), as Nokogiri's Node#fragment; *frag the new fragment, its nodes the children */
static int hp_parse(HPool *p, const u8 *s, u32 n, u32 ctx, u32 *frag) {
    if (hp_ready(p)) return -1;
    char tag[256] = "body"; int ns = 0, form = 0;
    if (ctx != HP_NONE && p->n[ctx].kind == 1) {
        HNode *x = &p->n[ctx];
        if (x->name_n < sizeof tag) { memcpy(tag, p->b + x->name, x->name_n); tag[x->name_n] = 0; }
        ns = x->ns;
        for (u32 a = ctx; a != HP_NONE; a = p->n[a].parent) if (p->n[a].ns == 0 && hp_name_is(p, a, "form")) { form = 1; break; }
    }
    int status;
    void *out = mg_parse((const char *)s, n, tag, ns, form, &status);
    if (!out) { p->err = status == 1 ? "html: an element has more than 400 attributes" : status == 2 ? "html: the markup is nested more than 400 deep" : "html: the parser ran out of memory"; return -1; }
    int r = hp_node(p, 11, frag);
    if (!r) r = hp_build(p, *frag, mg_root(out));
    mg_free(out);
    return r;
}

/* ---- attributes ---- */
/* an attribute's name as written: its namespace's prefix (xlink:, xml:, xmlns:) and its name */
static u32 hp_attr_prefix(u8 ns, const char **pre) { *pre = ns == 1 ? "xlink:" : ns == 2 ? "xml:" : ns == 3 ? "xmlns:" : ""; return hp_strlen(*pre); }
static int hp_attr_named(HPool *p, HAttr *x, const u8 *s, u32 n) {
    const char *pre; u32 pn = hp_attr_prefix(x->ns, &pre);
    /* xmlns:xmlns is written xmlns */
    if (x->ns == 3 && x->name_n == 5 && memcmp(p->b + x->name, "xmlns", 5) == 0) return n == 5 && memcmp(s, "xmlns", 5) == 0;
    return n == pn + x->name_n && memcmp(s, pre, pn) == 0 && memcmp(s + pn, p->b + x->name, x->name_n) == 0;
}
static i64 hp_attr_find(HPool *p, u32 id, const u8 *s, u32 n) {
    HNode *x = &p->n[id];
    for (u32 k = 0; k < x->nattrs; k++) if (hp_attr_named(p, &p->a[x->attrs + k], s, n)) return k;
    return -1;
}
static int hp_set_attr(HPool *p, u32 id, const u8 *s, u32 n, const u8 *v, u32 vn) {
    HNode *x = &p->n[id];
    i64 k = hp_attr_find(p, id, s, n);
    u32 off;
    if (hp_bytes(p, v, vn, &off)) return -1;
    if (k >= 0) { p->a[x->attrs + k].value = off; p->a[x->attrs + k].value_n = vn; return 0; }
    /* a new one at the end of the list: the list moved to the end of the attributes first unless it is there already */
    if (x->attrs + x->nattrs != p->na || x->nattrs == 0) {
        if (hp_attr_room(p, x->nattrs + 1)) return -1;
        memcpy(&p->a[p->na], &p->a[x->attrs], x->nattrs * sizeof(HAttr));
        x->attrs = p->na; p->na += x->nattrs;
    } else if (hp_attr_room(p, 1)) return -1;
    HAttr *a = &p->a[p->na++];
    a->ns = 0; a->value = off; a->value_n = vn;
    if (hp_bytes(p, s, n, &a->name)) return -1;
    a->name_n = n; x->nattrs++;
    return 0;
}
static void hp_remove_attr(HPool *p, u32 id, const u8 *s, u32 n) {
    HNode *x = &p->n[id];
    i64 k = hp_attr_find(p, id, s, n);
    if (k < 0) return;
    memmove(&p->a[x->attrs + k], &p->a[x->attrs + k + 1], (x->nattrs - (u32)k - 1) * sizeof(HAttr));
    x->nattrs--;
}

/* ---- new nodes, clones ---- */
static int hp_new_element(HPool *p, const u8 *s, u32 n, u32 *id) {
    if (hp_ready(p) || hp_node(p, 1, id) || hp_bytes(p, s, n, &p->n[*id].name)) return -1;
    p->n[*id].name_n = n; return 0;
}
static int hp_new_text(HPool *p, const u8 *s, u32 n, u32 *id) {
    if (hp_ready(p) || hp_node(p, 3, id) || hp_bytes(p, s, n, &p->n[*id].text)) return -1;
    p->n[*id].text_n = n; return 0;
}
/* a deep copy, detached (its strings shared: they are never changed in place) */
static int hp_clone(HPool *p, u32 id, u32 *out) {
    if (hp_node(p, p->n[id].kind, out)) return -1;
    HNode *x = &p->n[*out], *o = &p->n[id];
    x->ns = o->ns; x->name = o->name; x->name_n = o->name_n; x->text = o->text; x->text_n = o->text_n;
    if (o->nattrs) {
        if (hp_attr_room(p, o->nattrs)) return -1;
        memcpy(&p->a[p->na], &p->a[o->attrs], o->nattrs * sizeof(HAttr));
        x->attrs = p->na; x->nattrs = o->nattrs; p->na += o->nattrs;
    }
    for (u32 c = p->n[id].first; c != HP_NONE; c = p->n[c].next) {
        u32 cc;
        if (hp_clone(p, c, &cc)) return -1;
        hp_append(p, *out, cc);
    }
    return 0;
}

/* ---- writing back (Nokogiri's html_standard_serialize) ---- */
typedef int (*HPut)(void *ctx, const u8 *s, u32 n);
static int hp_is(HPool *p, u32 id, const char *const *names, u32 k) {
    HNode *x = &p->n[id];
    if (x->kind != 1 || x->ns != 0) return 0; /* a foreign element is none of these */
    for (u32 i = 0; i < k; i++) if (hp_name_is(p, id, names[i])) return 1;
    return 0;
}
/* text escaped: & always, U+00A0 as &nbsp;, " in an attribute, < > outside one (and in one too with `brackets`: the current HTML
   serialization algorithm's, which Nokogiri does not do) */
static int hp_escaped(const u8 *s, u32 n, int attr, int brackets, HPut put, void *ctx) {
    u32 start = 0;
    for (u32 i = 0; i < n; ) {
        const char *rep = 0; u32 step = 1;
        if (s[i] == '&') rep = "&amp;";
        else if (s[i] == 0xC2 && i + 1 < n && s[i + 1] == 0xA0) { rep = "&nbsp;"; step = 2; }
        else if (attr && s[i] == '"') rep = "&quot;";
        else if ((!attr || brackets) && s[i] == '<') rep = "&lt;";
        else if ((!attr || brackets) && s[i] == '>') rep = "&gt;";
        if (!rep) { i++; continue; }
        if (put(ctx, s + start, i - start) || put(ctx, (const u8 *)rep, hp_strlen(rep))) return -1;
        i += step; start = i;
    }
    return put(ctx, s + start, n - start);
}
static int hp_write(HPool *p, u32 id, int brackets, HPut put, void *ctx) {
    static const char *const VOIDS[] = { "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input", "keygen", "link", "meta", "param", "source", "track", "wbr" };
    static const char *const RAW[] = { "style", "script", "xmp", "iframe", "noembed", "noframes", "plaintext", "noscript" };
    HNode *x = &p->n[id];
    switch (x->kind) {
        case 1: {
            if (put(ctx, (const u8 *)"<", 1) || put(ctx, p->b + x->name, x->name_n)) return -1;
            for (u32 k = 0; k < x->nattrs; k++) {
                HAttr *a = &p->a[x->attrs + k];
                const char *pre; u32 pn = hp_attr_prefix(a->ns, &pre);
                if (a->ns == 3 && a->name_n == 5 && memcmp(p->b + a->name, "xmlns", 5) == 0) pn = 0; /* xmlns:xmlns -> xmlns */
                if (put(ctx, (const u8 *)" ", 1) || put(ctx, (const u8 *)pre, pn) || put(ctx, p->b + a->name, a->name_n) || put(ctx, (const u8 *)"=\"", 2)
                    || hp_escaped(p->b + a->value, a->value_n, 1, brackets, put, ctx) || put(ctx, (const u8 *)"\"", 1)) return -1;
            }
            if (put(ctx, (const u8 *)">", 1)) return -1;
            if (hp_is(p, id, VOIDS, sizeof VOIDS / sizeof VOIDS[0])) return 0;
            for (u32 c = x->first; c != HP_NONE; c = p->n[c].next) if (hp_write(p, c, brackets, put, ctx)) return -1;
            return put(ctx, (const u8 *)"</", 2) || put(ctx, p->b + x->name, x->name_n) || put(ctx, (const u8 *)">", 1) ? -1 : 0;
        }
        case 3:
            if (x->parent != HP_NONE && hp_is(p, x->parent, RAW, sizeof RAW / sizeof RAW[0])) return put(ctx, p->b + x->text, x->text_n);
            return hp_escaped(p->b + x->text, x->text_n, 0, 0, put, ctx);
        case 4: return put(ctx, (const u8 *)"<![CDATA[", 9) || put(ctx, p->b + x->text, x->text_n) || put(ctx, (const u8 *)"]]>", 3) ? -1 : 0;
        case 8: return put(ctx, (const u8 *)"<!--", 4) || put(ctx, p->b + x->text, x->text_n) || put(ctx, (const u8 *)"-->", 3) ? -1 : 0;
        case 11:
            for (u32 c = x->first; c != HP_NONE; c = p->n[c].next) if (hp_write(p, c, brackets, put, ctx)) return -1;
            return 0;
    }
    return 0;
}
/* the text of a node (xmlNodeGetContent: a text, CDATA or comment its own; an element or fragment its descendants' text and CDATA) */
static int hp_content(HPool *p, u32 id, HPut put, void *ctx) {
    HNode *x = &p->n[id];
    if (x->kind == 3 || x->kind == 4 || x->kind == 8) return put(ctx, p->b + x->text, x->text_n);
    for (u32 c = x->first; c != HP_NONE; c = p->n[c].next) {
        HNode *y = &p->n[c];
        if (y->kind == 8) continue;
        if (hp_content(p, c, put, ctx)) return -1;
    }
    return 0;
}
