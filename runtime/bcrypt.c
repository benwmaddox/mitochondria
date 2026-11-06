/* ---- bcrypt for password digests (runtime/bcrypt.c), compiled with crypt_blowfish (third_party/crypt_blowfish) into one object ----
   Linked into a program that uses `bcrypt_hash` or `bcrypt_check`. The password is a C string, as bcrypt-ruby and OpenBSD's bcrypt take
   it: the caller cuts it at 72 bytes (only those count) and at a zero byte. */
#include "crypt_blowfish.h"

/* bcrypt of `key` under `setting` (a whole digest, or `$2a$NN$` and 22 salt characters): the 60-character digest into out (61 bytes,
   NUL-terminated); 1, or 0 when the setting is not a bcrypt one */
int mito_bcrypt(const char *key, const char *setting, char *out) { return _crypt_blowfish_rn(key, setting, out, 61) != 0; }

/* a setting `$2a$NN$...` (30 bytes with its NUL) for cost NN from 16 random bytes; 1, or 0 for a cost outside 4..31 */
int mito_bcrypt_setting(const unsigned char *rand16, int cost, char *out) {
    return _crypt_gensalt_blowfish_rn("$2a$", (unsigned long)cost, (const char *)rand16, 16, out, 30) != 0;
}
