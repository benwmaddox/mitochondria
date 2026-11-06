/* ---- cryptography from the operating system's library (runtime/crypto.c) ----
   The primitives the crypto built-ins need, none written here: BCrypt (CNG) on Windows, libcrypto (OpenSSL 3) on Linux, declared
   as the libraries export them (no headers, as in runtime/os.c). Compiled into a program that uses a crypto built-in
   (MITO_USES_CRYPTO); on Linux such a program links libcrypto. Each returns 1, or 0 on failure. */

#ifdef _WIN32
/* ---------------------------------------------------------------- Windows: BCrypt */
long BCryptOpenAlgorithmProvider(void **alg, const u16 *id, const u16 *impl, u32 flags);
long BCryptHash(void *alg, u8 *secret, u32 sn, u8 *in, u32 n, u8 *out, u32 outn);
long BCryptDeriveKeyPBKDF2(void *prf, u8 *pw, u32 pwn, u8 *salt, u32 sn, u64 iters, u8 *out, u32 outn, u32 flags);
long BCryptSetProperty(void *h, const u16 *prop, u8 *in, u32 n, u32 flags);
long BCryptGenerateSymmetricKey(void *alg, void **key, u8 *obj, u32 objn, u8 *secret, u32 sn, u32 flags);
long BCryptDestroyKey(void *key);
long BCryptEncrypt(void *key, u8 *in, u32 n, void *pad, u8 *iv, u32 ivn, u8 *out, u32 outn, u32 *res, u32 flags);
long BCryptDecrypt(void *key, u8 *in, u32 n, void *pad, u8 *iv, u32 ivn, u8 *out, u32 outn, u32 *res, u32 flags);
typedef struct { u32 cbSize, dwInfoVersion; u8 *pbNonce; u32 cbNonce; u8 *pbAuthData; u32 cbAuthData; u8 *pbTag; u32 cbTag;
                 u8 *pbMacContext; u32 cbMacContext, cbAAD; u64 cbData; u32 dwFlags; } CrAuthInfo; /* BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO */
static Lock cr_lk; static int cr_ready;
static void *cr_sha1_, *cr_sha256_, *cr_hmac1_, *cr_hmac256_, *cr_aes_;
/* the algorithm providers, opened once */
static int cr_init(void) {
    if (cr_ready) return cr_ready > 0;
    lk_on(&cr_lk);
    if (!cr_ready) {
        static const u16 sha1[] = { 'S', 'H', 'A', '1', 0 }, sha256[] = { 'S', 'H', 'A', '2', '5', '6', 0 }, aes[] = { 'A', 'E', 'S', 0 };
        static const u16 mode[] = { 'C', 'h', 'a', 'i', 'n', 'i', 'n', 'g', 'M', 'o', 'd', 'e', 0 };
        static const u16 gcm[] = { 'C', 'h', 'a', 'i', 'n', 'i', 'n', 'g', 'M', 'o', 'd', 'e', 'G', 'C', 'M', 0 };
        int ok = BCryptOpenAlgorithmProvider(&cr_sha1_, sha1, 0, 0) == 0 && BCryptOpenAlgorithmProvider(&cr_sha256_, sha256, 0, 0) == 0 &&
                 BCryptOpenAlgorithmProvider(&cr_hmac1_, sha1, 0, 8) == 0 && BCryptOpenAlgorithmProvider(&cr_hmac256_, sha256, 0, 8) == 0 && /* 8: HMAC */
                 BCryptOpenAlgorithmProvider(&cr_aes_, aes, 0, 0) == 0 && BCryptSetProperty(cr_aes_, mode, (u8 *)gcm, sizeof gcm, 0) == 0;
        cr_ready = ok ? 1 : -1;
    }
    lk_off(&cr_lk);
    return cr_ready > 0;
}
static int cr_random(u8 *out, u32 n) { return BCryptGenRandom(0, out, n, 2) == 0; } /* the system's preferred generator */
static int cr_sha(int bits, const u8 *in, u32 n, u8 *out) { return cr_init() && BCryptHash(bits == 1 ? cr_sha1_ : cr_sha256_, 0, 0, (u8 *)in, n, out, bits == 1 ? 20 : 32) == 0; }
static int cr_hmac(int bits, const u8 *key, u32 kn, const u8 *in, u32 n, u8 *out) {
    static u8 empty[1];
    return cr_init() && BCryptHash(bits == 1 ? cr_hmac1_ : cr_hmac256_, kn ? (u8 *)key : empty, kn, (u8 *)in, n, out, bits == 1 ? 20 : 32) == 0;
}
static int cr_pbkdf2_sha256(const u8 *pw, u32 pwn, const u8 *salt, u32 sn, u32 iters, u8 *out, u32 outn) {
    static u8 empty[1];
    return cr_init() && BCryptDeriveKeyPBKDF2(cr_hmac256_, pwn ? (u8 *)pw : empty, pwn, sn ? (u8 *)salt : empty, sn, iters, out, outn, 0) == 0;
}
/* AES-256-GCM with a 12-byte nonce and no associated data: encrypt in[0..n) into out and write the 16-byte tag, or decrypt (dec)
   checking the tag */
static int cr_gcm(int dec, const u8 *key, const u8 *iv, const u8 *in, u32 n, u8 *out, u8 *tag) {
    if (!cr_init()) return 0;
    void *k = 0; u32 got = 0; static u8 none[1];
    if (BCryptGenerateSymmetricKey(cr_aes_, &k, 0, 0, (u8 *)key, 32, 0) != 0) return 0;
    CrAuthInfo ai; memset(&ai, 0, sizeof ai);
    ai.cbSize = sizeof ai; ai.dwInfoVersion = 1; ai.pbNonce = (u8 *)iv; ai.cbNonce = 12; ai.pbTag = tag; ai.cbTag = 16;
    long r = dec ? BCryptDecrypt(k, n ? (u8 *)in : none, n, &ai, 0, 0, n ? out : none, n, &got, 0) : BCryptEncrypt(k, n ? (u8 *)in : none, n, &ai, 0, 0, n ? out : none, n, &got, 0);
    BCryptDestroyKey(k);
    return r == 0 && got == n;
}
#else
/* ---------------------------------------------------------------- Linux: libcrypto (OpenSSL 3) */
const void *EVP_sha1(void);
const void *EVP_sha256(void);
const void *EVP_aes_256_gcm(void);
int EVP_Digest(const void *data, u64 n, u8 *md, u32 *mdn, const void *type, void *impl);
u8 *HMAC(const void *md, const void *key, int kn, const u8 *in, u64 n, u8 *out, u32 *outn);
int PKCS5_PBKDF2_HMAC(const char *pw, int pwn, const u8 *salt, int sn, int iters, const void *md, int keyn, u8 *out);
int RAND_bytes(u8 *buf, int n);
void *EVP_CIPHER_CTX_new(void);
void EVP_CIPHER_CTX_free(void *c);
int EVP_CIPHER_CTX_ctrl(void *c, int type, int arg, void *ptr);
int EVP_EncryptInit_ex(void *c, const void *cipher, void *impl, const u8 *key, const u8 *iv);
int EVP_EncryptUpdate(void *c, u8 *out, int *outn, const u8 *in, int n);
int EVP_EncryptFinal_ex(void *c, u8 *out, int *outn);
int EVP_DecryptInit_ex(void *c, const void *cipher, void *impl, const u8 *key, const u8 *iv);
int EVP_DecryptUpdate(void *c, u8 *out, int *outn, const u8 *in, int n);
int EVP_DecryptFinal_ex(void *c, u8 *out, int *outn);
static int cr_random(u8 *out, u32 n) { return RAND_bytes(out, (int)n) == 1; }
static int cr_sha(int bits, const u8 *in, u32 n, u8 *out) { u32 k = 0; return EVP_Digest(in, n, out, &k, bits == 1 ? EVP_sha1() : EVP_sha256(), 0) == 1; }
static int cr_hmac(int bits, const u8 *key, u32 kn, const u8 *in, u32 n, u8 *out) {
    u32 k = 0; static const u8 empty[1];
    return HMAC(bits == 1 ? EVP_sha1() : EVP_sha256(), kn ? key : empty, (int)kn, n ? in : empty, n, out, &k) != 0;
}
static int cr_pbkdf2_sha256(const u8 *pw, u32 pwn, const u8 *salt, u32 sn, u32 iters, u8 *out, u32 outn) {
    return PKCS5_PBKDF2_HMAC((const char *)pw, (int)pwn, salt, (int)sn, (int)iters, EVP_sha256(), (int)outn, out) == 1;
}
static int cr_gcm(int dec, const u8 *key, const u8 *iv, const u8 *in, u32 n, u8 *out, u8 *tag) {
    void *c = EVP_CIPHER_CTX_new(); int k = 0, f = 0, ok;
    if (!c) return 0;
    if (dec) ok = EVP_DecryptInit_ex(c, EVP_aes_256_gcm(), 0, key, iv) == 1 && (n == 0 || EVP_DecryptUpdate(c, out, &k, in, (int)n) == 1) &&
                  EVP_CIPHER_CTX_ctrl(c, 0x11, 16, tag) == 1 && EVP_DecryptFinal_ex(c, out + k, &f) == 1; /* 0x11: EVP_CTRL_GCM_SET_TAG */
    else ok = EVP_EncryptInit_ex(c, EVP_aes_256_gcm(), 0, key, iv) == 1 && (n == 0 || EVP_EncryptUpdate(c, out, &k, in, (int)n) == 1) &&
              EVP_EncryptFinal_ex(c, out + k, &f) == 1 && EVP_CIPHER_CTX_ctrl(c, 0x10, 16, tag) == 1; /* 0x10: EVP_CTRL_GCM_GET_TAG */
    EVP_CIPHER_CTX_free(c);
    return ok && (u32)(k + f) == n;
}
#endif

/* bcrypt (runtime/bcrypt.c with crypt_blowfish, linked in) */
int mito_bcrypt(const char *key, const char *setting, char *out);
int mito_bcrypt_setting(const u8 *rand16, int cost, char *out);
