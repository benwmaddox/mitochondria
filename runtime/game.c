/* ---- the window driver: `main` of a program whose entry is `@frame` ----
   SDL3 and the system's OpenGL are loaded when the game starts (SDL3.dll, libSDL3.so.0): nothing is linked and no header is used, so a build needs neither,
   and a machine without SDL3 says so. The program's `@shader` functions give fragment shader sources, asked for once; each frame the window's input is
   the `@frame` function's inputs (`:dt` the seconds since the last frame, `:key_w` whether a key is down, `:mouse_dx` and `:mouse_dy` the pointer's movement
   since the last frame) and its answer is the frame: one line for each pass, a shader's name and the shader's uniforms as `name=value` words, `terrain x=1.5 yaw=0.2`.
   A pass is drawn over the whole window (one triangle; the vertex shader is the driver's: it gives `in vec2 vUv` 0..1 across the window) and the driver sets
   `uniform vec2 uResolution`, the size in pixels of what is drawn on. A pass named with a scale, `scene@0.25 ...`, draws into an image of that fraction of the window
   instead (one image, kept between frames, made again when the window or the scale changes); later passes that draw on the window read it as `uniform sampler2D uImage`
   (16-bit floats, so light can be brighter than 1). Fragment shaders are `#version 330 core`.
   Options: --size WxH (1280x720); --seed N; --hold w,shift keys held down in every frame and --mouse DX,DY pointer movement in every frame (to try a game's controls without a person); --frames N runs N frames of 1/60 s and stops (a window that is not shown); --screenshot FILE.ppm writes the
   last of them (with --frames, default 1). Without --frames the window runs until it is closed. */
int mito_handle(void *m, const char *method, u32 mn, const char *target, u32 tn, const MitoPair *h, u32 nh, const MitoPair *f, u32 nf, const char *body, u32 bn);
const char *mito_response(void *m, u32 *len);
void *mito_start(u64 budget, u64 seed);
int mito_begin(void *m);
int mito_has_tables(void);
u64 strlen(const char *s);
int sprintf(char *s, const char *f, ...);
double strtod(const char *s, char **end);
long strtol(const char *s, char **end, int base);
int strcmp(const char *a, const char *b);
void *fopen(const char *path, const char *mode);
u64 fwrite(const void *p, u64 size, u64 n, void *f);
int fclose(void *f);

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
    { "glActiveTexture", (void **)&glActiveTexture }, { "glUniform1i", (void **)&glUniform1i },
};

/* the offscreen image the passes with a scale draw into: one 16-bit float texture on a framebuffer */
static unsigned game_fbo = 0, game_tex = 0;
static int game_tex_w = 0, game_tex_h = 0;
static int game_image(int w, int h) {
    if (game_tex && game_tex_w == w && game_tex_h == h) return 1;
    if (!game_fbo) glGenFramebuffers(1, &game_fbo);
    if (game_tex) glDeleteTextures(1, &game_tex);
    glGenTextures(1, &game_tex);
    glBindTexture(0x0DE1, game_tex);                                  /* GL_TEXTURE_2D */
    glTexImage2D(0x0DE1, 0, 0x881A, w, h, 0, 0x1908, 0x1406, 0);     /* GL_RGBA16F, GL_RGBA, GL_FLOAT */
    glTexParameteri(0x0DE1, 0x2800, 0x2601); glTexParameteri(0x0DE1, 0x2801, 0x2601);   /* linear filtering */
    glTexParameteri(0x0DE1, 0x2802, 0x812F); glTexParameteri(0x0DE1, 0x2803, 0x812F);   /* clamped at the edges */
    glBindFramebuffer(0x8D40, game_fbo);                              /* GL_FRAMEBUFFER */
    glFramebufferTexture2D(0x8D40, 0x8CE0, 0x0DE1, game_tex, 0);     /* GL_COLOR_ATTACHMENT0 */
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

int main(int argc, char **argv) {
    const char *v = 0;
    int width = 1280, height = 720;
    long frames = 0;
    u64 seed = 0;
    const char *shot = 0;
    if (game_flag(argc, argv, "--size", &v)) { char *e; width = (int)strtol(v, &e, 10); height = *e == 'x' ? (int)strtol(e + 1, 0, 10) : 0; if (width < 16 || height < 16 || width > 8192 || height > 8192) { printf("error: --size WxH, from 16x16 to 8192x8192\n"); return 2; } }
    if (game_flag(argc, argv, "--frames", &v)) frames = strtol(v, 0, 10);
    if (game_flag(argc, argv, "--seed", &v)) seed = (u64)strtol(v, 0, 10);
    if (game_flag(argc, argv, "--screenshot", &shot) && frames == 0) frames = 1;
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
    { int sc = mito_begin(app);
      if (sc) { u32 en; const char *e = mito_response(app, &en); printf("error: the @start function failed (%d): %.*s\n", sc, (int)en, e); return 2; } }

    /* the window and its OpenGL context */
    if (!sdl.Init(0x20u)) { printf("error: SDL_Init: %s\n", sdl.GetError()); return 2; }
    sdl.GL_SetAttribute(17, 3); sdl.GL_SetAttribute(18, 3); sdl.GL_SetAttribute(20, 1); sdl.GL_SetAttribute(5, 1); /* 3.3 core, double buffered */
    void *win = sdl.CreateWindow("Mitochondria", width, height, 0x2ull | 0x20ull | (frames ? 0x8ull : 0)); /* OPENGL | RESIZABLE (| HIDDEN when run for a fixed number of frames) */
    if (!win) { printf("error: SDL_CreateWindow: %s\n", sdl.GetError()); return 2; }
    void *ctx = sdl.GL_CreateContext(win);
    if (!ctx) { printf("error: OpenGL 3.3 is not available: %s\n", sdl.GetError()); return 2; }
    sdl.GL_SetSwapInterval(frames ? 0 : 1);
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
    if (!frames) sdl.ShowWindow(win);
    u64 last = sdl.GetTicksNS();
    int quit = 0;
    for (long frame = 0; !quit && (frames == 0 || frame < frames); frame++) {
        u64 ev[16];
        while (sdl.PollEvent(ev)) {
            u32 type = *(u32 *)ev;
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
        if (!frames) { u64 now = sdl.GetTicksNS(); dt = (double)(now - last) / 1e9; last = now; if (dt > 0.25) dt = 0.25; }
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
                glBindTexture(0x0DE1, 0);                      /* not read while it is drawn on */
            } else {
                glBindFramebuffer(0x8D40, 0);
                if (game_tex) { glActiveTexture(0x84C0); glBindTexture(0x0DE1, game_tex); }   /* GL_TEXTURE0 */
            }
            glViewport(0, 0, tw, th);
            glClearColor(0, 0, 0, 1); glClear(0x4000u);
            glUseProgram(programs[shader]);
            glUniform2f(glGetUniformLocation(programs[shader], "uResolution"), (float)tw, (float)th);
            if (scale <= 0 && game_tex) glUniform1i(glGetUniformLocation(programs[shader], "uImage"), 0);
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

        if (shot && frame == frames - 1) {
            u8 *px = (u8 *)os_alloc((u64)pw * (u64)ph * 4);
            if (!px) { printf("error: no memory for the screenshot\n"); return 1; }
            glFinish();
            glReadPixels(0, 0, pw, ph, 0x1908u, 0x1401u, px); /* GL_RGBA, GL_UNSIGNED_BYTE */
            void *f = fopen(shot, "wb");
            if (!f) { printf("error: cannot write %s\n", shot); return 1; }
            char head[64]; int hn = sprintf(head, "P6\n%d %d\n255\n", pw, ph);
            fwrite(head, 1, (u64)hn, f);
            u8 *row = (u8 *)os_alloc((u64)pw * 3);
            for (int y = ph - 1; y >= 0; y--) {              /* OpenGL's rows start at the bottom */
                for (int x = 0; x < pw; x++) { row[x * 3] = px[((u64)y * pw + x) * 4]; row[x * 3 + 1] = px[((u64)y * pw + x) * 4 + 1]; row[x * 3 + 2] = px[((u64)y * pw + x) * 4 + 2]; }
                fwrite(row, 1, (u64)pw * 3, f);
            }
            fclose(f);
        }
        sdl.GL_SwapWindow(win); /* after the screenshot: the back buffer is undefined once swapped */
    }
    sdl.DestroyWindow(win);
    sdl.Quit();
    return 0;
}
