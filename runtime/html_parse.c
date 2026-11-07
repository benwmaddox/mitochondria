/* ---- HTML fragments: the parser's side (Phase 8; third_party/gumbo, Nokogiri's fork at 1.19.4) ----
   Compiled with gumbo into one object (src/build.rs, gumbo_object) for a program that uses `html_parse`. It parses a fragment as
   Nokogiri::HTML5's DocumentFragment does (ext/nokogiri/gumbo.c, noko_gumbo_s_fragment: a context element, its namespace, whether a
   form is around it, no quirks, Nokogiri's limits) and lets runtime/html.c walk gumbo's tree to copy it into the instance's node pool;
   then the tree is freed. Gumbo's own working memory is the C heap for the length of one parse. */
#include <stddef.h>
#include "nokogiri_gumbo.h"

/* A fragment of `n` bytes parsed in the context `ctx` (an element's name; namespace 0 HTML, 1 SVG, 2 MathML; `form`: a form element
   is the context or around it). Returns gumbo's output (free it with mg_free), or 0 with *status: 1 too many attributes on an element,
   2 the tree too deep, 3 out of memory. */
void *mg_parse(const char *s, unsigned long long n, const char *ctx, int ns, int form, int *status) {
    GumboOptions o = kGumboDefaultOptions;
    /* Nokogiri::Gumbo's defaults: DEFAULT_MAX_ATTRIBUTES 400, DEFAULT_MAX_ERRORS 0, DEFAULT_MAX_TREE_DEPTH 400 (plus one for the html
       element a fragment's nodes sit in) */
    o.max_attributes = 400;
    o.max_errors = 0;
    o.max_tree_depth = 400 + 1;
    o.fragment_context = ctx;
    o.fragment_namespace = ns == 1 ? GUMBO_NAMESPACE_SVG : ns == 2 ? GUMBO_NAMESPACE_MATHML : GUMBO_NAMESPACE_HTML;
    o.fragment_encoding = NULL;
    o.quirks_mode = GUMBO_DOCTYPE_NO_QUIRKS;
    o.fragment_context_has_form_ancestor = form != 0;
    GumboOutput *out = gumbo_parse_with_options(&o, s, n);
    *status = 0;
    switch (out->status) {
        case GUMBO_STATUS_OK: return out;
        case GUMBO_STATUS_TOO_MANY_ATTRIBUTES: *status = 1; break;
        case GUMBO_STATUS_TREE_TOO_DEEP: *status = 2; break;
        default: *status = 3; break;
    }
    gumbo_destroy_output(out);
    return 0;
}
void mg_free(void *out) { gumbo_destroy_output((GumboOutput *)out); }
/* the node a fragment's nodes are the children of (the html element) */
void *mg_root(void *out) { return ((GumboOutput *)out)->root; }

/* a node: 1 element (or template), 2 text, 3 CDATA, 4 comment, 5 white space (text) */
int mg_type(void *node) {
    switch (((GumboNode *)node)->type) {
        case GUMBO_NODE_ELEMENT: case GUMBO_NODE_TEMPLATE: return 1;
        case GUMBO_NODE_TEXT: return 2;
        case GUMBO_NODE_CDATA: return 3;
        case GUMBO_NODE_COMMENT: return 4;
        case GUMBO_NODE_WHITESPACE: return 5;
        default: return 0;
    }
}
const char *mg_name(void *node) { return ((GumboNode *)node)->v.element.name; }
/* an element's namespace: 0 HTML, 1 SVG, 2 MathML */
int mg_ns(void *node) {
    switch (((GumboNode *)node)->v.element.tag_namespace) { case GUMBO_NAMESPACE_SVG: return 1; case GUMBO_NAMESPACE_MATHML: return 2; default: return 0; }
}
const char *mg_text(void *node) { return ((GumboNode *)node)->v.text.text; }
unsigned long long mg_nchildren(void *node) {
    GumboNode *g = (GumboNode *)node;
    return g->type == GUMBO_NODE_DOCUMENT ? g->v.document.children.length : g->v.element.children.length;
}
void *mg_child(void *node, unsigned long long i) {
    GumboNode *g = (GumboNode *)node;
    return g->type == GUMBO_NODE_DOCUMENT ? g->v.document.children.data[i] : g->v.element.children.data[i];
}
unsigned long long mg_nattrs(void *node) { return ((GumboNode *)node)->v.element.attributes.length; }
const char *mg_attr_name(void *node, unsigned long long i) { return ((GumboAttribute *)((GumboNode *)node)->v.element.attributes.data[i])->name; }
const char *mg_attr_value(void *node, unsigned long long i) { return ((GumboAttribute *)((GumboNode *)node)->v.element.attributes.data[i])->value; }
/* an attribute's namespace: 0 none, 1 xlink, 2 xml, 3 xmlns */
int mg_attr_ns(void *node, unsigned long long i) {
    switch (((GumboAttribute *)((GumboNode *)node)->v.element.attributes.data[i])->attr_namespace) {
        case GUMBO_ATTR_NAMESPACE_XLINK: return 1; case GUMBO_ATTR_NAMESPACE_XML: return 2; case GUMBO_ATTR_NAMESPACE_XMLNS: return 3; default: return 0;
    }
}
