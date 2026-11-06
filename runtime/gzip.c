/* ---- gzip for the HTTP front end (runtime/gzip.c), compiled with zlib (third_party/zlib) into one object ----
   A program whose web block says `gzip;` links it. A connection keeps one deflate stream (mito_gzip_new) and resets it for each
   answer, so compressing allocates nothing per request. Level 6, zlib's default (as Go's and Thruster's gzip). */
#include <stdlib.h>
#include "zlib.h"

typedef unsigned long long u64_;

/* room enough for n bytes gzipped: zlib's deflateBound for the default settings, plus the gzip header and trailer */
u64_ mito_gzip_bound(u64_ n) { return n + (n >> 12) + (n >> 14) + (n >> 25) + 13 + 18 + 64; }

/* a deflate stream writing gzip, or 0 */
void *mito_gzip_new(void) {
    z_stream *s = (z_stream *)calloc(1, sizeof(z_stream));
    if (!s) return 0;
    if (deflateInit2(s, 6, Z_DEFLATED, 15 + 16, 8, Z_DEFAULT_STRATEGY) != Z_OK) { free(s); return 0; }
    return s;
}

/* gzip in[0..n) into out (cap bytes, at least mito_gzip_bound(n)) with stream st; the length written, 0 on failure */
u64_ mito_gzip_with(void *st, const unsigned char *in, u64_ n, unsigned char *out, u64_ cap) {
    z_stream *s = (z_stream *)st;
    if (!s || n > 0xFFFFFFFFu || cap > 0xFFFFFFFFu || deflateReset(s) != Z_OK) return 0;
    s->next_in = (Bytef *)in; s->avail_in = (uInt)n; s->next_out = out; s->avail_out = (uInt)cap;
    if (deflate(s, Z_FINISH) != Z_STREAM_END) return 0;
    return (u64_)s->total_out;
}
