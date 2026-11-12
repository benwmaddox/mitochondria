/* ---- the window driver: `main` of a program whose entry is `@frame` ----
   SDL3 and the system's OpenGL are loaded when the game starts (SDL3.dll, libSDL3.so.0): nothing is linked and no header is used, so a build needs neither,
   and a machine without SDL3 says so. The program's `@shader` functions give fragment shader sources, asked for once; each frame the window's input is
   the `@frame` function's inputs (`:dt` the seconds since the last frame, `:key_w` whether a key is down, `:mouse_dx` and `:mouse_dy` the pointer's movement
   since the last frame) and its answer is the frame: one line for each pass, a shader's name and the shader's uniforms as `name=value` words, `terrain x=1.5 yaw=0.2`.
   A pass is drawn over the whole window (one triangle; the vertex shader is the driver's: it gives `in vec2 vUv` 0..1 across the window) and the driver sets
   `uniform vec2 uResolution`, the size in pixels of what is drawn on. A pass named with a scale, `scene@0.25 ...`, draws into an image of that fraction of the window
   instead (one image, kept between frames, made again when the window or the scale changes) and may write two outputs, `layout(location = 0) out vec4` and
   `layout(location = 1) out vec4`: later passes that draw on the window read them as `uniform sampler2D uImage` and `uImage1` (16-bit floats, so light can be
   brighter than 1, and a second output can hold what the picture is made of: how far, which way a surface faces, which thing it is). Fragment shaders are `#version 330 core`.
   Options: --size WxH (1280x720); --seed N; --hold w,shift keys held down in every frame and --mouse DX,DY pointer movement in every frame (to try a game's controls without a person); --frames N runs N frames of 1/60 s and stops (a window that is not shown); --screenshot FILE.ppm writes the
   last of them (with --frames, default 1). Without --frames the window runs until it is closed.
   Record and replay: --record FILE writes each frame's inputs (dt, the keys the program reads, the pointer's movement) and every --checkpoint N frames (60) a hash of the
   declared state (mito_state_hash); --replay FILE feeds them back (a window that is not shown) and stops at the first checkpoint that differs; --capture DIR writes a PNG
   of every --capture-every N frames (1). The recording and replay use a fixed clock and a seed, so `now()` and `random()` repeat. The format is in DESIGN.md. */
int mito_handle(void *m, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *h, u32 nh, const MitoPair *f, u32 nf, const char *body, u32 bn);
const char *mito_response(void *m, u32 *len);
void *mito_start(u64 budget, u64 seed);
int mito_begin(void *m);
int mito_has_tables(void);
u64 strlen(const char *s);
/* The few printf formats the driver uses, done here so that it needs no C library formatting: on Windows `sprintf` is an inline function of the headers, not a symbol a build
   with clang can link, and one formatter on every platform makes the text of a recorded input (`%.6f` of dt) identical everywhere. Supported: literal text, `%%`, `%s`, `%d`/`%u`
   with `l` or `ll`, an optional zero-padded width (`%06ld`), and `%.Nf` for N 0 to 9. `sprintf(buffer, ...)` is `game_snprintf(buffer, sizeof buffer, ...)`: every call gives an array. */
#ifdef __TINYC__
#include <stdarg.h> /* TinyCC 0.9.27 has no __builtin_va_list: its own header */
#define GAME_VA va_list
#define GAME_VA_START va_start
#define GAME_VA_ARG va_arg
#define GAME_VA_END va_end
#else
#define GAME_VA __builtin_va_list
#define GAME_VA_START __builtin_va_start
#define GAME_VA_ARG __builtin_va_arg
#define GAME_VA_END __builtin_va_end
#endif
static int game_snprintf(char *out, u64 cap, const char *f, ...) {
    GAME_VA ap;
    GAME_VA_START(ap, f);
    u64 n = 0;
#define GAME_PUT(ch) do { if (n + 1 < cap) out[n] = (char)(ch); n++; } while (0)
    for (; *f; f++) {
        if (*f != '%') { GAME_PUT(*f); continue; }
        f++;
        if (*f == '%') { GAME_PUT('%'); continue; }
        int zero = 0, width = 0, prec = -1, lng = 0;
        if (*f == '0') { zero = 1; f++; }
        while (*f >= '0' && *f <= '9') { width = width * 10 + (*f - '0'); f++; }
        if (*f == '.') { f++; prec = 0; while (*f >= '0' && *f <= '9') { prec = prec * 10 + (*f - '0'); f++; } }
        while (*f == 'l') { lng++; f++; }
        if (*f == 's') {
            const char *t = GAME_VA_ARG(ap, const char *);
            for (; *t; t++) GAME_PUT(*t);
        } else if (*f == 'd' || *f == 'u') {
            unsigned long long u; int neg = 0;
            if (*f == 'd') {
                long long v = lng == 0 ? (long long)GAME_VA_ARG(ap, int) : lng == 1 ? (long long)GAME_VA_ARG(ap, long) : GAME_VA_ARG(ap, long long);
                if (v < 0) { neg = 1; u = 0ULL - (unsigned long long)v; } else u = (unsigned long long)v;
            } else u = lng == 0 ? (unsigned long long)GAME_VA_ARG(ap, unsigned) : lng == 1 ? (unsigned long long)GAME_VA_ARG(ap, unsigned long) : GAME_VA_ARG(ap, unsigned long long);
            char digits[24]; int nd = 0;
            do { digits[nd++] = (char)('0' + u % 10); u /= 10; } while (u);
            int len = nd + neg;
            if (neg && zero) { GAME_PUT('-'); neg = 0; }
            for (int k = len; k < width; k++) GAME_PUT(zero ? '0' : ' ');
            if (neg) GAME_PUT('-');
            while (nd) GAME_PUT(digits[--nd]);
        } else if (*f == 'f') {
            double v = GAME_VA_ARG(ap, double);
            if (prec < 0) prec = 6;
            if (v != v) { const char *t = "nan"; for (; *t; t++) GAME_PUT(*t); continue; }
            if (v < 0) { GAME_PUT('-'); v = -v; }
            if (v > 9e18) { const char *t = "inf"; for (; *t; t++) GAME_PUT(*t); continue; }
            unsigned long long scale = 1; for (int k = 0; k < prec; k++) scale *= 10;
            unsigned long long whole = (unsigned long long)v;
            double rest = (v - (double)whole) * (double)scale;
            unsigned long long frac = (unsigned long long)(rest + 0.5);
            if (frac >= scale) { whole++; frac -= scale; }
            char digits[24]; int nd = 0;
            do { digits[nd++] = (char)('0' + whole % 10); whole /= 10; } while (whole);
            while (nd) GAME_PUT(digits[--nd]);
            if (prec > 0) {
                GAME_PUT('.');
                char fr[12]; for (int k = prec - 1; k >= 0; k--) { fr[k] = (char)('0' + frac % 10); frac /= 10; }
                for (int k = 0; k < prec; k++) GAME_PUT(fr[k]);
            }
        }
    }
#undef GAME_PUT
    GAME_VA_END(ap);
    if (cap) out[n < cap ? n : cap - 1] = 0;
    return (int)n;
}
#define sprintf(buf, ...) game_snprintf(buf, sizeof(buf), __VA_ARGS__)
double strtod(const char *s, char **end);
long strtol(const char *s, char **end, int base);
int strcmp(const char *a, const char *b);
void *fopen(const char *path, const char *mode);
u64 fwrite(const void *p, u64 size, u64 n, void *f);
int fclose(void *f);
int fflush(void *f);
u64 fread(void *p, u64 size, u64 n, void *f);
int fseek(void *f, long off, int whence);
long ftell(void *f);
u64 mito_state_hash(void *m);
void mito_set_clock(void *m, i64 ms, int fixed);
#ifdef _WIN32
int _mkdir(const char *path);
#define game_mkdir(p) _mkdir(p)
#else
int mkdir(const char *path, unsigned mode);
#define game_mkdir(p) mkdir(p, 0777)
#endif

#ifdef _WIN32
void *LoadLibraryA(const char *name);
void *GetProcAddress(void *lib, const char *name);
static void *dl_open(const char *name) { return LoadLibraryA(name); }
static void *dl_sym(void *lib, const char *name) { return GetProcAddress(lib, name); }
#define GAME_SDL_LIB "SDL3.dll"
#else
void *dlopen(const char *name, int flags);
void *dlsym(void *lib, const char *name);
static void *dl_open(const char *name) { return dlopen(name, 2); } /* RTLD_NOW */
static void *dl_sym(void *lib, const char *name) { return dlsym(lib, name); }
#define GAME_SDL_LIB "libSDL3.so.0"
#endif

/* SDL3 (3.2): the functions used, as the library exports them (`bool` is one byte) */
static struct {
    u8 (*Init)(u32 flags);
    void (*Quit)(void);
    const char *(*GetError)(void);
    void *(*CreateWindow)(const char *title, int w, int h, u64 flags);
    void (*DestroyWindow)(void *window);
    u8 (*ShowWindow)(void *window);
    u8 (*GL_SetAttribute)(int attr, int value);
    void *(*GL_CreateContext)(void *window);
    u8 (*GL_SetSwapInterval)(int interval);
    u8 (*GL_SwapWindow)(void *window);
    void *(*GL_GetProcAddress)(const char *name);
    u8 (*GetWindowSizeInPixels)(void *window, int *w, int *h);
    u8 (*SetWindowRelativeMouseMode)(void *window, u8 enabled);
    u8 (*PollEvent)(void *event);
    u64 (*GetTicksNS)(void);
} sdl;
static const struct { const char *name; void **at; } sdl_table[] = {
    { "SDL_Init", (void **)&sdl.Init }, { "SDL_Quit", (void **)&sdl.Quit }, { "SDL_GetError", (void **)&sdl.GetError },
    { "SDL_CreateWindow", (void **)&sdl.CreateWindow }, { "SDL_DestroyWindow", (void **)&sdl.DestroyWindow }, { "SDL_ShowWindow", (void **)&sdl.ShowWindow },
    { "SDL_GL_SetAttribute", (void **)&sdl.GL_SetAttribute }, { "SDL_GL_CreateContext", (void **)&sdl.GL_CreateContext },
    { "SDL_GL_SetSwapInterval", (void **)&sdl.GL_SetSwapInterval }, { "SDL_GL_SwapWindow", (void **)&sdl.GL_SwapWindow },
    { "SDL_GL_GetProcAddress", (void **)&sdl.GL_GetProcAddress }, { "SDL_GetWindowSizeInPixels", (void **)&sdl.GetWindowSizeInPixels },
    { "SDL_SetWindowRelativeMouseMode", (void **)&sdl.SetWindowRelativeMouseMode }, { "SDL_PollEvent", (void **)&sdl.PollEvent },
    { "SDL_GetTicksNS", (void **)&sdl.GetTicksNS },
};

/* OpenGL 3.3 core: the functions used */
static unsigned (*glCreateShader)(unsigned type);
static void (*glShaderSource)(unsigned shader, int count, const char *const *src, const int *len);
static void (*glCompileShader)(unsigned shader);
static void (*glGetShaderiv)(unsigned shader, unsigned what, int *out);
static void (*glGetShaderInfoLog)(unsigned shader, int max, int *len, char *log);
static unsigned (*glCreateProgram)(void);
static void (*glAttachShader)(unsigned program, unsigned shader);
static void (*glLinkProgram)(unsigned program);
static void (*glGetProgramiv)(unsigned program, unsigned what, int *out);
static void (*glGetProgramInfoLog)(unsigned program, int max, int *len, char *log);
static void (*glUseProgram)(unsigned program);
static int (*glGetUniformLocation)(unsigned program, const char *name);
static void (*glUniform1f)(int at, float x);
static void (*glUniform2f)(int at, float x, float y);
static void (*glGenVertexArrays)(int n, unsigned *out);
static void (*glBindVertexArray)(unsigned vao);
static void (*glDrawArrays)(unsigned mode, int first, int count);
static void (*glViewport)(int x, int y, int w, int h);
static void (*glClear)(unsigned mask);
static void (*glClearColor)(float r, float g, float b, float a);
static void (*glReadPixels)(int x, int y, int w, int h, unsigned format, unsigned type, void *out);
static void (*glFinish)(void);
static void (*glGenFramebuffers)(int n, unsigned *out);
static void (*glBindFramebuffer)(unsigned target, unsigned fb);
static void (*glFramebufferTexture2D)(unsigned target, unsigned attachment, unsigned textarget, unsigned tex, int level);
static unsigned (*glCheckFramebufferStatus)(unsigned target);
static void (*glGenTextures)(int n, unsigned *out);
static void (*glDeleteTextures)(int n, const unsigned *tex);
static void (*glBindTexture)(unsigned target, unsigned tex);
static void (*glTexImage2D)(unsigned target, int level, int internal, int w, int h, int border, unsigned format, unsigned type, const void *data);
static void (*glTexParameteri)(unsigned target, unsigned name, int value);
static void (*glActiveTexture)(unsigned unit);
static void (*glDrawBuffers)(int n, const unsigned *bufs);
static void (*glUniform1i)(int at, int x);
static const struct { const char *name; void **at; } gl_table[] = {
    { "glCreateShader", (void **)&glCreateShader }, { "glShaderSource", (void **)&glShaderSource }, { "glCompileShader", (void **)&glCompileShader },
    { "glGetShaderiv", (void **)&glGetShaderiv }, { "glGetShaderInfoLog", (void **)&glGetShaderInfoLog }, { "glCreateProgram", (void **)&glCreateProgram },
    { "glAttachShader", (void **)&glAttachShader }, { "glLinkProgram", (void **)&glLinkProgram }, { "glGetProgramiv", (void **)&glGetProgramiv },
    { "glGetProgramInfoLog", (void **)&glGetProgramInfoLog }, { "glUseProgram", (void **)&glUseProgram }, { "glGetUniformLocation", (void **)&glGetUniformLocation },
    { "glUniform1f", (void **)&glUniform1f }, { "glUniform2f", (void **)&glUniform2f }, { "glGenVertexArrays", (void **)&glGenVertexArrays },
    { "glBindVertexArray", (void **)&glBindVertexArray }, { "glDrawArrays", (void **)&glDrawArrays }, { "glViewport", (void **)&glViewport },
    { "glClear", (void **)&glClear }, { "glClearColor", (void **)&glClearColor }, { "glReadPixels", (void **)&glReadPixels }, { "glFinish", (void **)&glFinish },
    { "glGenFramebuffers", (void **)&glGenFramebuffers }, { "glBindFramebuffer", (void **)&glBindFramebuffer }, { "glFramebufferTexture2D", (void **)&glFramebufferTexture2D },
    { "glCheckFramebufferStatus", (void **)&glCheckFramebufferStatus }, { "glGenTextures", (void **)&glGenTextures }, { "glDeleteTextures", (void **)&glDeleteTextures },
    { "glBindTexture", (void **)&glBindTexture }, { "glTexImage2D", (void **)&glTexImage2D }, { "glTexParameteri", (void **)&glTexParameteri },
    { "glActiveTexture", (void **)&glActiveTexture }, { "glUniform1i", (void **)&glUniform1i }, { "glDrawBuffers", (void **)&glDrawBuffers },
};

/* the offscreen image the passes with a scale draw into: two 16-bit float textures on one framebuffer (color attachments 0 and 1) */
static unsigned game_fbo = 0, game_tex[2] = { 0, 0 };
static int game_tex_w = 0, game_tex_h = 0;
static int game_image(int w, int h) {
    if (game_tex[0] && game_tex_w == w && game_tex_h == h) return 1;
    if (!game_fbo) glGenFramebuffers(1, &game_fbo);
    glBindFramebuffer(0x8D40, game_fbo);                              /* GL_FRAMEBUFFER */
    for (int k = 0; k < 2; k++) {
        if (game_tex[k]) glDeleteTextures(1, &game_tex[k]);
        glGenTextures(1, &game_tex[k]);
        glBindTexture(0x0DE1, game_tex[k]);                           /* GL_TEXTURE_2D */
        glTexImage2D(0x0DE1, 0, 0x881A, w, h, 0, 0x1908, 0x1406, 0); /* GL_RGBA16F, GL_RGBA, GL_FLOAT */
        glTexParameteri(0x0DE1, 0x2800, 0x2601); glTexParameteri(0x0DE1, 0x2801, 0x2601);   /* linear filtering */
        glTexParameteri(0x0DE1, 0x2802, 0x812F); glTexParameteri(0x0DE1, 0x2803, 0x812F);   /* clamped at the edges */
        glFramebufferTexture2D(0x8D40, 0x8CE0 + (unsigned)k, 0x0DE1, game_tex[k], 0);       /* GL_COLOR_ATTACHMENT0 + k */
    }
    static const unsigned both[2] = { 0x8CE0, 0x8CE1 };
    glDrawBuffers(2, both);
    game_tex_w = w; game_tex_h = h;
    return glCheckFramebufferStatus(0x8D40) == 0x8CD5;               /* GL_FRAMEBUFFER_COMPLETE */
}

/* the vertex shader: one triangle that covers the window, `vUv` 0..1 across it */
static const char game_vertex[] =
    "#version 330 core\n"
    "out vec2 vUv;\n"
    "void main() { vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)); vUv = p; gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0); }\n";

static u32 game_keycode(const char *name) {
    if (strlen(name) == 1) return (u32)(u8)name[0];
    if (!strcmp(name, "space")) return 0x20u;
    if (!strcmp(name, "escape")) return 0x1bu;
    if (!strcmp(name, "right")) return 0x4000004fu;
    if (!strcmp(name, "left")) return 0x40000050u;
    if (!strcmp(name, "down")) return 0x40000051u;
    if (!strcmp(name, "up")) return 0x40000052u;
    if (!strcmp(name, "shift")) return 0x400000e1u;
    return 0;
}

static unsigned game_compile(unsigned type, const char *src, const char *what) {
    unsigned s = glCreateShader(type);
    const char *text[1] = { src };
    glShaderSource(s, 1, text, 0);
    glCompileShader(s);
    int ok = 0;
    glGetShaderiv(s, 0x8B81, &ok); /* GL_COMPILE_STATUS */
    if (!ok) { char log[2048]; int n = 0; glGetShaderInfoLog(s, (int)sizeof log - 1, &n, log); log[n > 0 ? n : 0] = 0; printf("error: shader %s does not compile:\n%s\n", what, log); return 0; }
    return s;
}

static int game_flag(int argc, char **argv, const char *name, const char **value) {
    for (int i = 1; i + 1 < argc; i++) if (!strcmp(argv[i], name)) { *value = argv[i + 1]; return 1; }
    return 0;
}

/* ---- record and replay: the file (little-endian; DESIGN.md "Record and replay") ----
   header: "MITOREP1", u32 version (1), u32 checkpoint interval, u64 seed, u32 width, u32 height, u64 frames (all ones until the recording is finished),
   u64 hash of the state after the last frame, u64 program hash, u32 number of keys, each u16 length + name, u32 number of shaders, each u16 length + name.
   then per frame: f64 dt, the key bits (one byte per 8 keys, the program's key order, bit 0 first), f32 mouse dx, f32 mouse dy, and after every frame whose
   number + 1 is a multiple of the interval a u64 hash of the state. */
#define REP_FIXED 56
static void rep_put32(u8 **p, u32 v) { memcpy(*p, &v, 4); *p += 4; }
static void rep_put64(u8 **p, u64 v) { memcpy(*p, &v, 8); *p += 8; }
static void rep_put16(u8 **p, u32 v) { u16 x = (u16)v; memcpy(*p, &x, 2); *p += 2; }
static u32 rep_get32(const u8 *p) { u32 v; memcpy(&v, p, 4); return v; }
static u64 rep_get64(const u8 *p) { u64 v; memcpy(&v, p, 8); return v; }
static u32 rep_get16(const u8 *p) { u16 v; memcpy(&v, p, 2); return v; }
static u64 rep_fnv(u64 h, const void *p, u64 n) { const u8 *b = (const u8 *)p; for (u64 i = 0; i < n; i++) h = (h ^ b[i]) * 0x100000001b3ULL; return h; }
static u64 rep_fnv64(u64 h, u64 v) { return rep_fnv(h, &v, 8); }
/* what a recording is tied to: the keys and shaders (names, in order) and the state's layout */
static u64 game_program_hash(void) {
    u64 h = 0xcbf29ce484222325ULL;
    for (int k = 0; k < MITO_NKEYS; k++) h = rep_fnv(h, mito_key_names[k], strlen(mito_key_names[k]) + 1);
    h = rep_fnv64(h, 0xFFFF);
    for (int k = 0; k < MITO_NSHADERS; k++) h = rep_fnv(h, mito_shader_names[k], strlen(mito_shader_names[k]) + 1);
    h = rep_fnv64(rep_fnv64(rep_fnv64(h, mito_prog.plain_end), mito_prog.total), mito_prog.ncolls);
    for (u32 c = 0; c < mito_prog.ncolls; c++) {
        const MitoColl *k = &mito_prog.colls[c];
        h = rep_fnv64(rep_fnv64(rep_fnv64(rep_fnv64(h, k->cap), k->mult), k->start), k->ncols);
        for (u32 i = 0; i < k->ncols; i++) h = rep_fnv64(rep_fnv64(rep_fnv64(h, k->cols[i].off), k->cols[i].size), (u64)k->cols[i].kind * 64 + k->cols[i].scalar);
    }
    return h;
}
static u64 rep_names(u8 **p, const char *const *names, int n) { /* a count, then each name with its u16 length */
    rep_put32(p, (u32)n);
    for (int k = 0; k < n; k++) { u64 l = strlen(names[k]); rep_put16(p, (u32)l); memcpy(*p, names[k], l); *p += l; }
    return 0;
}
/* a list of names in a recording, as text for a message: `w shift` */
static const u8 *rep_names_text(const u8 *p, const u8 *end, char *out, int cap) {
    if (end - p < 4) return 0;
    u32 n = rep_get32(p); p += 4; int o = 0; out[0] = 0;
    for (u32 k = 0; k < n; k++) {
        if (end - p < 2) return 0;
        u32 l = rep_get16(p); p += 2;
        if ((u64)(end - p) < l) return 0;
        if (o + (int)l + 2 < cap) { if (o) out[o++] = ' '; memcpy(out + o, p, l); o += (int)l; out[o] = 0; }
        p += l;
    }
    return p;
}

/* ---- PNG without zlib: the deflate stream is stored blocks ---- */
static u32 png_crc_table[256]; static int png_crc_ready;
static u32 png_crc(u32 c, const u8 *p, u64 n) {
    if (!png_crc_ready) { for (u32 i = 0; i < 256; i++) { u32 x = i; for (int j = 0; j < 8; j++) x = (x & 1) ? 0xEDB88320u ^ (x >> 1) : x >> 1; png_crc_table[i] = x; } png_crc_ready = 1; }
    for (u64 i = 0; i < n; i++) c = png_crc_table[(c ^ p[i]) & 0xFF] ^ (c >> 8);
    return c;
}
static void png_be32(u8 *p, u32 v) { p[0] = (u8)(v >> 24); p[1] = (u8)(v >> 16); p[2] = (u8)(v >> 8); p[3] = (u8)v; }
static void png_piece(void *f, u32 *crc, const u8 *p, u64 n) { fwrite(p, 1, n, f); *crc = png_crc(*crc, p, n); }
static u8 *game_raw; static u64 game_raw_cap;
/* px: RGBA rows from the bottom, as glReadPixels gives them */
static int game_write_png(const char *path, const u8 *px, int w, int h) {
    u64 rn = (u64)h * (1 + (u64)w * 3);
    if (rn > game_raw_cap) { game_raw = (u8 *)os_alloc(rn); game_raw_cap = game_raw ? rn : 0; if (!game_raw) return 0; }
    u8 *o = game_raw;
    for (int y = h - 1; y >= 0; y--) {
        *o++ = 0; /* filter: none */
        for (int x = 0; x < w; x++) { const u8 *s = px + ((u64)y * (u64)w + (u64)x) * 4; *o++ = s[0]; *o++ = s[1]; *o++ = s[2]; }
    }
    u32 a = 1, b = 0;                                           /* Adler-32 */
    for (u64 i = 0; i < rn;) { u64 e = i + 5552 < rn ? i + 5552 : rn; for (; i < e; i++) { a += game_raw[i]; b += a; } a %= 65521; b %= 65521; }
    void *f = fopen(path, "wb");
    if (!f) return 0;
    static const u8 sig[8] = { 0x89, 'P', 'N', 'G', 13, 10, 26, 10 };
    fwrite(sig, 1, 8, f);
    u8 t[16]; u32 crc;
    png_be32(t, 13); fwrite(t, 1, 4, f);                        /* IHDR */
    u8 ih[17] = { 'I', 'H', 'D', 'R' }; png_be32(ih + 4, (u32)w); png_be32(ih + 8, (u32)h); ih[12] = 8; ih[13] = 2; ih[14] = 0; ih[15] = 0; ih[16] = 0;
    crc = 0xFFFFFFFFu; png_piece(f, &crc, ih, 17); png_be32(t, ~crc); fwrite(t, 1, 4, f);
    u64 nb = (rn + 65534) / 65535; if (nb == 0) nb = 1;
    png_be32(t, (u32)(2 + nb * 5 + rn + 4)); fwrite(t, 1, 4, f); /* IDAT: zlib header, stored blocks, Adler-32 */
    crc = 0xFFFFFFFFu; png_piece(f, &crc, (const u8 *)"IDAT", 4);
    static const u8 zh[2] = { 0x78, 0x01 }; png_piece(f, &crc, zh, 2);
    u64 at = 0;
    do {
        u64 n = rn - at < 65535 ? rn - at : 65535;
        u8 bh[5] = { (u8)(at + n >= rn), (u8)n, (u8)(n >> 8), (u8)~n, (u8)(~n >> 8) };
        png_piece(f, &crc, bh, 5); png_piece(f, &crc, game_raw + at, n); at += n;
    } while (at < rn);
    png_be32(t, (b << 16) | a); png_piece(f, &crc, t, 4);
    png_be32(t, ~crc); fwrite(t, 1, 4, f);
    png_be32(t, 0); fwrite(t, 1, 4, f);                         /* IEND */
    crc = 0xFFFFFFFFu; png_piece(f, &crc, (const u8 *)"IEND", 4); png_be32(t, ~crc); fwrite(t, 1, 4, f);
    fclose(f);
    return 1;
}
static int game_write_ppm(const char *path, const u8 *px, int pw, int ph) {
    void *f = fopen(path, "wb");
    if (!f) return 0;
    char head[64]; int hn = sprintf(head, "P6\n%d %d\n255\n", pw, ph);
    fwrite(head, 1, (u64)hn, f);
    u64 rn = (u64)pw * 3;
    if (rn > game_raw_cap) { game_raw = (u8 *)os_alloc(rn); game_raw_cap = game_raw ? rn : 0; if (!game_raw) { fclose(f); return 0; } }
    for (int y = ph - 1; y >= 0; y--) {              /* OpenGL's rows start at the bottom */
        for (int x = 0; x < pw; x++) { game_raw[x * 3] = px[((u64)y * pw + x) * 4]; game_raw[x * 3 + 1] = px[((u64)y * pw + x) * 4 + 1]; game_raw[x * 3 + 2] = px[((u64)y * pw + x) * 4 + 2]; }
        fwrite(game_raw, 1, rn, f);
    }
    fclose(f);
    return 1;
}
static u8 *game_px; static u64 game_px_cap;
static u8 *game_pixels(int pw, int ph) {
    u64 n = (u64)pw * (u64)ph * 4;
    if (n > game_px_cap) { game_px = (u8 *)os_alloc(n); game_px_cap = game_px ? n : 0; if (!game_px) return 0; }
    glFinish();
    glReadPixels(0, 0, pw, ph, 0x1908u, 0x1401u, game_px); /* GL_RGBA, GL_UNSIGNED_BYTE */
    return game_px;
}

int main(int argc, char **argv) {
    const char *v = 0;
    int width = 1280, height = 720, size_given = 0;
    long frames = 0;
    u64 seed = 0;
    const char *shot = 0;
    if (game_flag(argc, argv, "--size", &v)) { char *e; size_given = 1; width = (int)strtol(v, &e, 10); height = *e == 'x' ? (int)strtol(e + 1, 0, 10) : 0; if (width < 16 || height < 16 || width > 8192 || height > 8192) { printf("error: --size WxH, from 16x16 to 8192x8192\n"); return 2; } }
    int frames_given = game_flag(argc, argv, "--frames", &v);
    if (frames_given) frames = strtol(v, 0, 10);
    int seed_given = game_flag(argc, argv, "--seed", &v);
    if (seed_given) seed = (u64)strtol(v, 0, 10);
    if (game_flag(argc, argv, "--screenshot", &shot) && frames == 0) frames = 1;
    const char *rec_path = 0, *rep_path = 0, *cap_dir = 0;
    game_flag(argc, argv, "--record", &rec_path); game_flag(argc, argv, "--replay", &rep_path); game_flag(argc, argv, "--capture", &cap_dir);
    long ckpt = 60, cap_every = 1;
    if (game_flag(argc, argv, "--checkpoint", &v)) { ckpt = strtol(v, 0, 10); if (ckpt < 0) { printf("error: --checkpoint N, N frames between two state checkpoints (0: only the last)\n"); return 2; } }
    if (game_flag(argc, argv, "--capture-every", &v)) { cap_every = strtol(v, 0, 10); if (cap_every < 1) { printf("error: --capture-every N, N at least 1\n"); return 2; } }
    if (rec_path && rep_path) { printf("error: --record and --replay cannot be used together\n"); return 2; }
    if (cap_dir && strlen(cap_dir) > 900) { printf("error: --capture directory name too long\n"); return 2; }
    int hold_given = 0, mouse_given = 0;
    { const char *t; hold_given = game_flag(argc, argv, "--hold", &t); mouse_given = game_flag(argc, argv, "--mouse", &t); }
    if (rep_path && (hold_given || mouse_given || seed_given)) { printf("error: with --replay the inputs and the seed come from the recording: --hold, --mouse and --seed are not used\n"); return 2; }

    /* a recording to replay: read and checked before anything is opened */
    const u8 *rep = 0; u64 rep_len = 0, rep_frames = 0, rep_final = 0, rep_at = 0; u32 keybytes = (u32)(MITO_NKEYS + 7) / 8;
    if (rep_path) {
        void *f = fopen(rep_path, "rb");
        if (!f) { printf("error: cannot read %s\n", rep_path); return 2; }
        fseek(f, 0, 2); long fl = ftell(f); fseek(f, 0, 0);
        if (fl < 0) { printf("error: cannot read %s\n", rep_path); fclose(f); return 2; }
        u8 *buf = (u8 *)os_alloc((u64)fl + 1);
        if (!buf || fread(buf, 1, (u64)fl, f) != (u64)fl) { printf("error: cannot read %s\n", rep_path); fclose(f); return 2; }
        fclose(f);
        rep = buf; rep_len = (u64)fl;
        if (rep_len < REP_FIXED + 8 || memcmp(rep, "MITOREP1", 8)) { printf("error: %s is not a Mitochondria recording (its first bytes are not MITOREP1)\n", rep_path); return 2; }
        if (rep_get32(rep + 8) != 1) { printf("error: %s is recording format version %u; this build replays version 1\n", rep_path, rep_get32(rep + 8)); return 2; }
        ckpt = (long)rep_get32(rep + 12); seed = rep_get64(rep + 16);
        if (!size_given) { width = (int)rep_get32(rep + 24); height = (int)rep_get32(rep + 28); }
        rep_frames = rep_get64(rep + 32); rep_final = rep_get64(rep + 40);
        char kl[512], sl[512]; kl[0] = sl[0] = 0;
        const u8 *p = rep_names_text(rep + REP_FIXED, rep + rep_len, kl, sizeof kl);
        p = p ? rep_names_text(p, rep + rep_len, sl, sizeof sl) : 0;
        if (!p) { printf("error: %s is damaged: its header is cut short\n", rep_path); return 2; }
        if (rep_get64(rep + 48) != game_program_hash()) {
            char mk[512], ms[512]; int a = 0, b = 0; mk[0] = ms[0] = 0;
            for (int k = 0; k < MITO_NKEYS && a < 400; k++) a += sprintf(mk + a, "%s%s", a ? " " : "", mito_key_names[k]);
            for (int k = 0; k < MITO_NSHADERS && b < 400; k++) b += sprintf(ms + b, "%s%s", b ? " " : "", mito_shader_names[k]);
            printf("error: %s was recorded by another program (or another version of this one): it reads keys [%s] and has shaders [%s]; this program reads keys [%s] and has shaders [%s], or its state layout differs\n", rep_path, kl, sl, mk, ms);
            return 2;
        }
        if (rep_frames == ~0ULL) { printf("error: %s is not finished: the recording stopped before its end was written\n", rep_path); return 2; }
        u64 rs = 8 + keybytes + 8;
        u64 want = (u64)(p - rep) + rep_frames * rs + (ckpt ? (rep_frames / (u64)ckpt) * 8 : 0);
        if (rep_frames > (1ULL << 40) || rep_len != want) { printf("error: %s is damaged: it holds %llu bytes, %llu frames need %llu\n", rep_path, (unsigned long long)rep_len, (unsigned long long)rep_frames, (unsigned long long)want); return 2; }
        rep_at = (u64)(p - rep);
        if (frames_given && frames > (long)rep_frames) { printf("error: --frames %ld is more than the %llu frames recorded\n", frames, (unsigned long long)rep_frames); return 2; }
        if (!frames_given) frames = (long)rep_frames;
    }
    int fixed = frames != 0 || rep_path != 0;                  /* a run of a fixed number of frames: a window that is not shown, frames of 1/60 s unless replayed */
    int deterministic = rec_path != 0 || rep_path != 0;        /* a fixed clock and a seed */
    if (rec_path && MITO_NKEYS > 512) { printf("error: a recording holds at most 512 keys\n"); return 2; }
    if (rec_path && seed == 0) seed = os_random() | 1;
    void *rec = 0;
    if (rec_path) {
        rec = fopen(rec_path, "wb");
        if (!rec) { printf("error: cannot write %s\n", rec_path); return 2; }
        static u8 hb[65536]; u8 *p = hb;
        memcpy(p, "MITOREP1", 8); p += 8; rep_put32(&p, 1); rep_put32(&p, (u32)ckpt); rep_put64(&p, seed); rep_put32(&p, (u32)width); rep_put32(&p, (u32)height);
        rep_put64(&p, ~0ULL); rep_put64(&p, 0); rep_put64(&p, game_program_hash());
        u64 names = 0; for (int k = 0; k < MITO_NKEYS; k++) names += strlen(mito_key_names[k]) + 2; for (int k = 0; k < MITO_NSHADERS; k++) names += strlen(mito_shader_names[k]) + 2;
        if (names > sizeof hb - 100) { printf("error: too many key and shader names to record\n"); return 2; }
        rep_names(&p, mito_key_names, MITO_NKEYS); rep_names(&p, mito_shader_names, MITO_NSHADERS);
        fwrite(hb, 1, (u64)(p - hb), rec);
    }
    if (cap_dir) game_mkdir(cap_dir);
    const char *hold = 0, *mouse_arg = 0;
    game_flag(argc, argv, "--hold", &hold);
    float hold_dx = 0, hold_dy = 0;
    if (game_flag(argc, argv, "--mouse", &mouse_arg)) { char *e; hold_dx = (float)strtod(mouse_arg, &e); hold_dy = *e == ',' ? (float)strtod(e + 1, 0) : 0; }

    /* SDL3 */
    void *lib = dl_open(GAME_SDL_LIB);
    if (!lib) { printf("error: %s not found: a game needs SDL3 (3.2 or newer) beside it or on the system's library path\n", GAME_SDL_LIB); return 2; }
    for (u32 i = 0; i < sizeof sdl_table / sizeof sdl_table[0]; i++) {
        *sdl_table[i].at = dl_sym(lib, sdl_table[i].name);
        if (!*sdl_table[i].at) { printf("error: %s has no %s: SDL 3.2 or newer is needed\n", GAME_SDL_LIB, sdl_table[i].name); return 2; }
    }

    /* the program */
    if (mito_has_tables()) { printf("error: a game has no tables yet\n"); return 2; }
    void *app = mito_start(256ULL << 20, seed);
    if (!app) { printf("error: out of address space\n"); return 2; }
    /* record and replay run on a fixed clock (2026-01-01 as `mito test` does, then moving on by the frames' dt) and a seed, so `@start`, `now()` and `random()` repeat */
    const i64 clock_base = 1767225600000LL;
    double sim_time = 0;
    if (deterministic) mito_set_clock(app, clock_base, 1);
    { int sc = mito_begin(app);
      if (sc) { u32 en; const char *e = mito_response(app, &en); printf("error: the @start function failed (%d): %.*s\n", sc, (int)en, e); return 2; } }

    /* the window and its OpenGL context */
    if (!sdl.Init(0x20u)) { printf("error: SDL_Init: %s\n", sdl.GetError()); return 2; }
    sdl.GL_SetAttribute(17, 3); sdl.GL_SetAttribute(18, 3); sdl.GL_SetAttribute(20, 1); sdl.GL_SetAttribute(5, 1); /* 3.3 core, double buffered */
    void *win = sdl.CreateWindow("Mitochondria", width, height, 0x2ull | 0x20ull | (fixed ? 0x8ull : 0)); /* OPENGL | RESIZABLE (| HIDDEN when run for a fixed number of frames) */
    if (!win) { printf("error: SDL_CreateWindow: %s\n", sdl.GetError()); return 2; }
    void *ctx = sdl.GL_CreateContext(win);
    if (!ctx) { printf("error: OpenGL 3.3 is not available: %s\n", sdl.GetError()); return 2; }
    sdl.GL_SetSwapInterval(fixed ? 0 : 1);
    for (u32 i = 0; i < sizeof gl_table / sizeof gl_table[0]; i++) {
        *gl_table[i].at = (void *)sdl.GL_GetProcAddress(gl_table[i].name);
        if (!*gl_table[i].at) { printf("error: OpenGL has no %s\n", gl_table[i].name); return 2; }
    }
    unsigned vao = 0; glGenVertexArrays(1, &vao); glBindVertexArray(vao);

    /* the shaders: each `@shader` function gives its source once */
    unsigned programs[MITO_NSHADERS > 0 ? MITO_NSHADERS : 1];
    unsigned vs = game_compile(0x8B31, game_vertex, "(the driver's vertex shader)");
    if (!vs) return 1;
    for (int k = 0; k < MITO_NSHADERS; k++) {
        char target[96]; int tn = sprintf(target, "/%s", mito_shader_names[k]);
        int code = mito_handle(app, "SHADER", 6, target, (u32)tn, 0, 0, 0, 0, "", 0);
        u32 sn; const char *src = mito_response(app, &sn);
        if (code >= 400) { printf("error: shader %s: %.*s\n", mito_shader_names[k], (int)sn, src); return 1; }
        char *own = (char *)os_alloc((u64)sn + 1);
        if (!own) { printf("error: no memory for a shader\n"); return 1; }
        memcpy(own, src, sn); own[sn] = 0;
        unsigned fs = game_compile(0x8B30, own, mito_shader_names[k]);
        if (!fs) return 1;
        unsigned p = glCreateProgram();
        glAttachShader(p, vs); glAttachShader(p, fs); glLinkProgram(p);
        int ok = 0; glGetProgramiv(p, 0x8B82, &ok); /* GL_LINK_STATUS */
        if (!ok) { char log[2048]; int n = 0; glGetProgramInfoLog(p, (int)sizeof log - 1, &n, log); log[n > 0 ? n : 0] = 0; printf("error: shader %s does not link:\n%s\n", mito_shader_names[k], log); return 1; }
        programs[k] = p;
    }

    /* what the window sees */
    u8 down[MITO_NKEYS > 0 ? MITO_NKEYS : 1] = { 0 };
    u32 codes[MITO_NKEYS > 0 ? MITO_NKEYS : 1] = { 0 };
    for (int k = 0; k < MITO_NKEYS; k++) codes[k] = game_keycode(mito_key_names[k]);
    float mdx = 0, mdy = 0;
    if (hold) {                                      /* --hold w,shift: these keys are down in every frame */
        for (int k = 0; k < MITO_NKEYS; k++) {
            const char *q = hold; u32 kn = (u32)strlen(mito_key_names[k]);
            while (*q) {
                const char *e = q; while (*e && *e != ',') e++;
                if ((u32)(e - q) == kn && !memcmp(q, mito_key_names[k], kn)) down[k] = 1;
                q = *e ? e + 1 : e;
            }
        }
    }
    static char names[MITO_NKEYS + 3][24];
    static char vals[MITO_NKEYS + 3][32];
    static MitoPair pairs[MITO_NKEYS + 3];
#ifdef MITO_MOUSE
    sdl.SetWindowRelativeMouseMode(win, 1);
#endif
    if (!fixed) sdl.ShowWindow(win);
    u64 last = sdl.GetTicksNS();
    int quit = 0;
    long frame = 0;
    for (; !quit && (!fixed || frame < frames); frame++) {
        u64 ev[16];
        while (sdl.PollEvent(ev)) {
            u32 type = *(u32 *)ev;
            if (rep) continue;                                                                              /* a replay takes its inputs from the recording */
            if (type == 0x100u || type == 0x210u) quit = 1;                                                 /* QUIT, WINDOW_CLOSE_REQUESTED */
            else if (type == 0x300u || type == 0x301u) {                                                   /* KEY_DOWN, KEY_UP */
                u32 key = *(u32 *)((u8 *)ev + 28); u8 d = *((u8 *)ev + 36);
                for (int k = 0; k < MITO_NKEYS; k++) if (codes[k] == key) down[k] = d;
                if (key == 0x1bu && d) quit = 1;                                                            /* Escape leaves unless the game reads it */
                for (int k = 0; k < MITO_NKEYS; k++) if (codes[k] == 0x1bu) quit = 0;
            }
            else if (type == 0x400u) { mdx += *(float *)((u8 *)ev + 36); mdy += *(float *)((u8 *)ev + 40); } /* MOUSE_MOTION */
        }
        mdx += hold_dx; mdy += hold_dy;
        double dt = 1.0 / 60.0;
        if (!fixed) { u64 now = sdl.GetTicksNS(); dt = (double)(now - last) / 1e9; last = now; if (dt > 0.25) dt = 0.25; }
        if (rep) {                                                                                          /* the recorded inputs of this frame */
            const u8 *r = rep + rep_at;
            memcpy(&dt, r, 8); r += 8;
            for (int k = 0; k < MITO_NKEYS; k++) down[k] = (u8)((r[k >> 3] >> (k & 7)) & 1);
            r += keybytes; memcpy(&mdx, r, 4); memcpy(&mdy, r + 4, 4);
            rep_at += 8 + keybytes + 8;
        } else if (rec) {
            u8 rb[8 + 64 + 8 + 8]; u8 *p = rb; u8 kb[64];
            memset(kb, 0, sizeof kb); for (int k = 0; k < MITO_NKEYS && k < 512; k++) if (down[k]) kb[k >> 3] |= (u8)(1u << (k & 7));
            memcpy(p, &dt, 8); p += 8; memcpy(p, kb, keybytes); p += keybytes; memcpy(p, &mdx, 4); memcpy(p + 4, &mdy, 4); p += 8;
            fwrite(rb, 1, (u64)(p - rb), rec);
        }
        if (deterministic) mito_set_clock(app, clock_base + (i64)(sim_time * 1000.0), 1);
        sim_time += dt;
        u32 np = 0;
        names[np][0] = ':'; names[np][1] = 'd'; names[np][2] = 't'; pairs[np].k = names[np]; pairs[np].kn = 3; pairs[np].v = vals[np]; pairs[np].vn = (u32)sprintf(vals[np], "%.6f", dt); np++;
        for (int k = 0; k < MITO_NKEYS; k++) {
            pairs[np].kn = (u32)sprintf(names[np], ":key_%s", mito_key_names[k]); pairs[np].k = names[np];
            pairs[np].vn = (u32)sprintf(vals[np], "%s", down[k] ? "true" : "false"); pairs[np].v = vals[np]; np++;
        }
        pairs[np].kn = (u32)sprintf(names[np], ":mouse_dx"); pairs[np].k = names[np]; pairs[np].vn = (u32)sprintf(vals[np], "%.3f", (double)mdx); pairs[np].v = vals[np]; np++;
        pairs[np].kn = (u32)sprintf(names[np], ":mouse_dy"); pairs[np].k = names[np]; pairs[np].vn = (u32)sprintf(vals[np], "%.3f", (double)mdy); pairs[np].v = vals[np]; np++;
        mdx = mdy = 0;

        int code = mito_handle(app, "FRAME", 5, "/", 1, pairs, np, 0, 0, "", 0);
        u32 bn; const char *body = mito_response(app, &bn);
        if (code >= 400) { printf("error: the frame function failed (%d): %.*s\n", code, (int)bn, body); return 1; }

        /* the frame: a line for each pass, a shader's name (with `@scale` to draw into the offscreen image) and then its uniforms */
        static char line[8192]; u32 ln = bn < sizeof line - 1 ? bn : (u32)sizeof line - 1;
        memcpy(line, body, ln); line[ln] = 0;
        int pw = 0, ph = 0; sdl.GetWindowSizeInPixels(win, &pw, &ph);
        char *p = line;
        while (*p) {
            while (*p == ' ' || *p == '\n') p++;
            if (!*p) break;
            char *word = p; while (*p && *p != ' ' && *p != '\n' && *p != '@') p++;
            double scale = 0;
            if (*p == '@') { *p++ = 0; char *e; scale = strtod(p, &e); p = e; if (scale <= 0 || scale > 1) { printf("error: a pass's scale is from 0 to 1: `%s@%g`\n", word, scale); return 1; } }
            char endc = *p; if (*p) *p++ = 0;
            int shader = -1;
            for (int k = 0; k < MITO_NSHADERS; k++) if (!strcmp(word, mito_shader_names[k])) shader = k;
            if (shader < 0) { printf("error: the frame names a shader that does not exist: `%s`\n", word); return 1; }
            int tw = pw, th = ph;
            if (scale > 0) {
                tw = (int)(pw * scale); th = (int)(ph * scale); if (tw < 1) tw = 1; if (th < 1) th = 1;
                if (!game_image(tw, th)) { printf("error: the offscreen image of %dx%d is not complete\n", tw, th); return 1; }
                glBindFramebuffer(0x8D40, game_fbo);
                for (int k = 0; k < 2; k++) { glActiveTexture(0x84C0 + (unsigned)k); glBindTexture(0x0DE1, 0); }   /* not read while it is drawn on */
            } else {
                glBindFramebuffer(0x8D40, 0);
                if (game_tex[0]) for (int k = 0; k < 2; k++) { glActiveTexture(0x84C0 + (unsigned)k); glBindTexture(0x0DE1, game_tex[k]); }   /* GL_TEXTURE0, 1 */
            }
            glViewport(0, 0, tw, th);
            glClearColor(0, 0, 0, 1); glClear(0x4000u);
            glUseProgram(programs[shader]);
            glUniform2f(glGetUniformLocation(programs[shader], "uResolution"), (float)tw, (float)th);
            if (scale <= 0 && game_tex[0]) { glUniform1i(glGetUniformLocation(programs[shader], "uImage"), 0); glUniform1i(glGetUniformLocation(programs[shader], "uImage1"), 1); }
            /* the pass's uniforms, up to the end of its line */
            char *lineend = p; if (endc == ' ') while (*lineend && *lineend != '\n') lineend++;   /* a pass with no uniforms ends where its name did */
            char lc = *lineend; *lineend = 0;
            if (endc && endc != '\n') {
                while (*p) {
                    while (*p == ' ') p++;
                    char *nm = p; while (*p && *p != '=' && *p != ' ') p++;
                    if (*p != '=') break;
                    *p++ = 0;
                    char *end; double x = strtod(p, &end);
                    if (end == p) { printf("error: the frame gives `%s` no number\n", nm); return 1; }
                    glUniform1f(glGetUniformLocation(programs[shader], nm), (float)x);
                    p = end;
                }
            }
            *lineend = lc; p = lineend;
            glDrawArrays(4, 0, 3); /* GL_TRIANGLES */
        }
        glBindFramebuffer(0x8D40, 0);

        int do_shot = shot && frame == frames - 1, do_cap = cap_dir && frame % cap_every == 0;
        if (do_shot || do_cap) {
            u8 *px = game_pixels(pw, ph);
            if (!px) { printf("error: no memory for the screenshot\n"); return 1; }
            if (do_shot && !game_write_ppm(shot, px, pw, ph)) { printf("error: cannot write %s\n", shot); return 1; }
            if (do_cap) {
                char path[1024]; sprintf(path, "%s/frame_%06ld.png", cap_dir, frame);
                if (!game_write_png(path, px, pw, ph)) { printf("error: cannot write %s\n", path); return 1; }
            }
        }
        sdl.GL_SwapWindow(win); /* after the screenshot: the back buffer is undefined once swapped */

        /* the checkpoint after this frame: recorded, or compared with the recording (the first that differs ends the replay) */
        if (ckpt && (frame + 1) % ckpt == 0 && (rec || rep)) {
            u64 got = mito_state_hash(app);
            if (rec) fwrite(&got, 1, 8, rec);
            else {
                u64 want = rep_get64(rep + rep_at); rep_at += 8;
                if (want != got) { printf("replay diverged at frame %ld: expected %016llx, got %016llx\n", frame, (unsigned long long)want, (unsigned long long)got); fflush(0); return 1; }
            }
        }
    }
    if (rec || rep) {
        u64 got = mito_state_hash(app);
        if (rec) {                                           /* finish the header: the number of frames and the last state's hash */
            u8 tail[16]; u64 fr = (u64)frame; memcpy(tail, &fr, 8); memcpy(tail + 8, &got, 8);
            fseek(rec, 32, 0); fwrite(tail, 1, 16, rec); fclose(rec);
        } else if ((u64)frame == rep_frames && got != rep_final) {
            printf("replay diverged at frame %ld: expected %016llx, got %016llx\n", frame - 1, (unsigned long long)rep_final, (unsigned long long)got); fflush(0); return 1;
        }
    }
    sdl.DestroyWindow(win);
    sdl.Quit();
    return 0;
}
