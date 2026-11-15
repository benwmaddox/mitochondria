//! Byte vectors (docs/design/vectors.md): `u8x16`, `u8x32` and `u8x64` values, their operators and the vector built-ins.
//!
//! Generated code never applies a C operator to a vector: every operation is one of the `V...` macros of `VEC_C`, which clang
//! compiles to vector extensions and SSE/AVX2 instructions (whichever the build's `-march` has) and TinyCC to loops over a
//! struct of bytes. Both give the same bytes. A vector is held in a C local of type `V16`, `V32` or `V64`.
use super::*;

/// The C for vectors, emitted only when a program uses one.
pub const VEC_C: &str = r#"/* byte vectors (docs/design/vectors.md) */
#ifndef __TINYC__
#if defined(__SSE2__)
#include <immintrin.h>
#endif
typedef u8 V16 __attribute__((vector_size(16)));
typedef u8 V32 __attribute__((vector_size(32)));
typedef u8 V64 __attribute__((vector_size(64)));
#define VI_ static inline __attribute__((always_inline))
#define VSPLAT16(x) ((V16){0} + (u8)(x))
#define VSPLAT32(x) ((V32){0} + (u8)(x))
#define VSPLAT64(x) ((V64){0} + (u8)(x))
#define VTAB16(...) ((V16){__VA_ARGS__})
#define VOP_(op, a, b) ((a) op (b))
#define VCMP_(T, op, a, b) ((T)((a) op (b)))
#define VSHR_(a, k) ((a) >> (u8)(k))
#define VLANE_(v, k) ((i64)(v)[k])
VI_ V16 vlo32_(V32 v) { return __builtin_shufflevector(v, v, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15); }
VI_ V16 vhi32_(V32 v) { return __builtin_shufflevector(v, v, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31); }
VI_ V32 vlo64_(V64 v) { return __builtin_shufflevector(v, v, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31); }
VI_ V32 vhi64_(V64 v) { return __builtin_shufflevector(v, v, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63); }
VI_ V32 vcat16_(V16 a, V16 b) { return __builtin_shufflevector(a, b, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31); }
VI_ V64 vcat32_(V32 a, V32 b) { return __builtin_shufflevector(a, b, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63); }
VI_ i64 vmask16_(V16 v) {
#if defined(__SSE2__)
    return (i64)(u32)_mm_movemask_epi8((__m128i)v);
#else
    i64 m = 0; for (int k = 0; k < 16; k++) m |= (i64)(v[k] >> 7) << k; return m;
#endif
}
VI_ i64 vmask32_(V32 v) {
#if defined(__AVX2__)
    return (i64)(u32)_mm256_movemask_epi8((__m256i)v);
#else
    return vmask16_(vlo32_(v)) | vmask16_(vhi32_(v)) << 16;
#endif
}
VI_ i64 vmask64_(V64 v) { return (i64)((u64)vmask32_(vlo64_(v)) | (u64)vmask32_(vhi64_(v)) << 32); }
VI_ V16 vlook16_(V16 t, V16 i) {
#if defined(__SSSE3__)
    return (V16)_mm_shuffle_epi8((__m128i)t, (__m128i)i);
#else
    V16 r; for (int k = 0; k < 16; k++) r[k] = (i[k] & 0x80) ? 0 : t[i[k] & 15]; return r;
#endif
}
VI_ V32 vlook32_(V16 t, V32 i) {
#if defined(__AVX2__)
    return (V32)_mm256_shuffle_epi8(_mm256_broadcastsi128_si256((__m128i)t), (__m256i)i);
#else
    return vcat16_(vlook16_(t, vlo32_(i)), vlook16_(t, vhi32_(i)));
#endif
}
VI_ V64 vlook64_(V16 t, V64 i) { return vcat32_(vlook32_(t, vlo64_(i)), vlook32_(t, vhi64_(i))); }
#define VLDSLOW_(T, N) static __attribute__((noinline)) T vld##N##s_(const u8 *p, u64 n, i64 i) { T v = (T){0}; for (int k = 0; k < N; k++) { i64 j = i + k; if (j >= 0 && (u64)j < n) v[k] = p[j]; } return v; }
VLDSLOW_(V16, 16) VLDSLOW_(V32, 32) VLDSLOW_(V64, 64)
#define VLD_(T, N) VI_ T vld##N##_(const u8 *p, u64 n, i64 i) { if (__builtin_expect((u64)i <= n && n - (u64)i >= N, 1)) { T v; __builtin_memcpy(&v, p + i, N); return v; } return vld##N##s_(p, n, i); }
VLD_(V16, 16) VLD_(V32, 32) VLD_(V64, 64)
VI_ i64 vpopcount_(i64 x) { return __builtin_popcountll((u64)x); }
VI_ i64 vtzcnt_(i64 x) { return x ? __builtin_ctzll((u64)x) : 64; }
VI_ i64 vlzcnt_(i64 x) { return x ? __builtin_clzll((u64)x) : 64; }
VI_ i64 vpxor_(i64 x) {
#if defined(__PCLMUL__)
    return _mm_cvtsi128_si64(_mm_clmulepi64_si128(_mm_set_epi64x(0, x), _mm_set1_epi8((char)0xFF), 0));
#else
    u64 y = (u64)x; y ^= y << 1; y ^= y << 2; y ^= y << 4; y ^= y << 8; y ^= y << 16; y ^= y << 32; return (i64)y;
#endif
}
#define VAND16(a, b) VOP_(&, a, b)
#define VAND32(a, b) VOP_(&, a, b)
#define VAND64(a, b) VOP_(&, a, b)
#define VOR16(a, b) VOP_(|, a, b)
#define VOR32(a, b) VOP_(|, a, b)
#define VOR64(a, b) VOP_(|, a, b)
#define VXOR16(a, b) VOP_(^, a, b)
#define VXOR32(a, b) VOP_(^, a, b)
#define VXOR64(a, b) VOP_(^, a, b)
#define VADD16(a, b) VOP_(+, a, b)
#define VADD32(a, b) VOP_(+, a, b)
#define VADD64(a, b) VOP_(+, a, b)
#define VSUB16(a, b) VOP_(-, a, b)
#define VSUB32(a, b) VOP_(-, a, b)
#define VSUB64(a, b) VOP_(-, a, b)
#define VEQ16(a, b) VCMP_(V16, ==, a, b)
#define VEQ32(a, b) VCMP_(V32, ==, a, b)
#define VEQ64(a, b) VCMP_(V64, ==, a, b)
#define VNE16(a, b) VCMP_(V16, !=, a, b)
#define VNE32(a, b) VCMP_(V32, !=, a, b)
#define VNE64(a, b) VCMP_(V64, !=, a, b)
#define VLT16(a, b) VCMP_(V16, <, a, b)
#define VLT32(a, b) VCMP_(V32, <, a, b)
#define VLT64(a, b) VCMP_(V64, <, a, b)
#define VLE16(a, b) VCMP_(V16, <=, a, b)
#define VLE32(a, b) VCMP_(V32, <=, a, b)
#define VLE64(a, b) VCMP_(V64, <=, a, b)
#define VGT16(a, b) VCMP_(V16, >, a, b)
#define VGT32(a, b) VCMP_(V32, >, a, b)
#define VGT64(a, b) VCMP_(V64, >, a, b)
#define VGE16(a, b) VCMP_(V16, >=, a, b)
#define VGE32(a, b) VCMP_(V32, >=, a, b)
#define VGE64(a, b) VCMP_(V64, >=, a, b)
#define VSHR16(a, k) VSHR_(a, k)
#define VSHR32(a, k) VSHR_(a, k)
#define VSHR64(a, k) VSHR_(a, k)
#define VLANE16(v, k) VLANE_(v, k)
#define VLANE32(v, k) VLANE_(v, k)
#define VLANE64(v, k) VLANE_(v, k)
#else
/* TinyCC: a vector is a struct of bytes, and every operation a loop over its lanes (the same bytes as the clang build) */
typedef struct { u8 l[16]; } V16;
typedef struct { u8 l[32]; } V32;
typedef struct { u8 l[64]; } V64;
#define VTAB16(...) ((V16){{__VA_ARGS__}})
#define VDEF_(N) \
static V##N VSPLAT##N(i64 x) { V##N r; int k; for (k = 0; k < N; k++) r.l[k] = (u8)x; return r; } \
static V##N VAND##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] &= b.l[k]; return a; } \
static V##N VOR##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] |= b.l[k]; return a; } \
static V##N VXOR##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] ^= b.l[k]; return a; } \
static V##N VADD##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] = (u8)(a.l[k] + b.l[k]); return a; } \
static V##N VSUB##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] = (u8)(a.l[k] - b.l[k]); return a; } \
static V##N VEQ##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] = a.l[k] == b.l[k] ? 255 : 0; return a; } \
static V##N VNE##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] = a.l[k] != b.l[k] ? 255 : 0; return a; } \
static V##N VLT##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] = a.l[k] < b.l[k] ? 255 : 0; return a; } \
static V##N VLE##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] = a.l[k] <= b.l[k] ? 255 : 0; return a; } \
static V##N VGT##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] = a.l[k] > b.l[k] ? 255 : 0; return a; } \
static V##N VGE##N(V##N a, V##N b) { int k; for (k = 0; k < N; k++) a.l[k] = a.l[k] >= b.l[k] ? 255 : 0; return a; } \
static V##N VSHR##N(V##N a, i64 s) { int k; for (k = 0; k < N; k++) a.l[k] = (u8)(a.l[k] >> s); return a; } \
static i64 VLANE##N(V##N v, i64 k) { return v.l[k]; } \
static i64 vmask##N##_(V##N v) { u64 m = 0; int k; for (k = 0; k < N; k++) m |= (u64)(v.l[k] >> 7) << k; return (i64)m; } \
static V##N vlook##N##_(V16 t, V##N i) { int k; for (k = 0; k < N; k++) i.l[k] = (i.l[k] & 0x80) ? 0 : t.l[i.l[k] & 15]; return i; } \
static V##N vld##N##_(const u8 *p, u64 n, i64 i) { V##N v; int k; for (k = 0; k < N; k++) { i64 j = i + k; v.l[k] = j >= 0 && (u64)j < n ? p[j] : 0; } return v; }
VDEF_(16) VDEF_(32) VDEF_(64)
static i64 vpopcount_(i64 x) { u64 y = (u64)x; i64 n = 0; while (y) { y &= y - 1; n++; } return n; }
static i64 vtzcnt_(i64 x) { u64 y = (u64)x; i64 n = 0; if (!y) return 64; while (!(y & 1)) { y >>= 1; n++; } return n; }
static i64 vlzcnt_(i64 x) { u64 y = (u64)x; i64 n = 0; if (!y) return 64; while (!(y >> 63)) { y <<= 1; n++; } return n; }
static i64 vpxor_(i64 x) { u64 y = (u64)x; y ^= y << 1; y ^= y << 2; y ^= y << 4; y ^= y << 8; y ^= y << 16; y ^= y << 32; return (i64)y; }
#endif
#define VLD16(p, n, i) vld16_((p), (u64)(n), (i64)(i))
#define VLD32(p, n, i) vld32_((p), (u64)(n), (i64)(i))
#define VLD64(p, n, i) vld64_((p), (u64)(n), (i64)(i))
#define VMASK16(v) vmask16_(v)
#define VMASK32(v) vmask32_(v)
#define VMASK64(v) vmask64_(v)
#define VLOOK16(t, i) vlook16_((t), (i))
#define VLOOK32(t, i) vlook32_((t), (i))
#define VLOOK64(t, i) vlook64_((t), (i))
#define VPOPCNT(x) vpopcount_(x)
#define VTZCNT(x) vtzcnt_(x)
#define VLZCNT(x) vlzcnt_(x)
#define VPXOR(x) vpxor_(x)
"#;

/// The names generated code may call for vectors (the safety profile's list): each is a macro of `VEC_C`.
pub const VEC_CALLS: &[&str] = &["VSPLAT16", "VSPLAT32", "VSPLAT64", "VTAB16", "VAND16", "VAND32", "VAND64", "VOR16", "VOR32", "VOR64", "VXOR16", "VXOR32", "VXOR64",
    "VADD16", "VADD32", "VADD64", "VSUB16", "VSUB32", "VSUB64", "VEQ16", "VEQ32", "VEQ64", "VNE16", "VNE32", "VNE64", "VLT16", "VLT32", "VLT64", "VLE16", "VLE32", "VLE64",
    "VGT16", "VGT32", "VGT64", "VGE16", "VGE32", "VGE64", "VSHR16", "VSHR32", "VSHR64", "VLANE16", "VLANE32", "VLANE64", "VLD16", "VLD32", "VLD64", "VMASK16", "VMASK32", "VMASK64",
    "VLOOK16", "VLOOK32", "VLOOK64", "VPOPCNT", "VTZCNT", "VLZCNT", "VPXOR"];
/// The C types of vectors (declarations the safety profile allows).
pub const VEC_TYPES: [&str; 3] = ["V16", "V32", "V64"];

/// A vector type's name in the language, for errors.
pub fn vec_name(n: u8) -> String { format!("u8x{}", n) }

impl<'p> G<'p> {
    /// A vector value held in a C local of its type (so an expression is computed once).
    fn hold_vec(&mut self, n: u8, e: String) -> CV {
        if is_name(&e) { return CV::Vec(n, e); }
        let t = self.tmp("w");
        self.line(&format!("V{} {} = {};", n, t, e));
        CV::Vec(n, t)
    }

    /// A number as a byte for a lane (an integer that must be 0 to 255; a constant outside is an error, a value outside fails the request).
    fn lane_byte(&mut self, v: &CV, what: &str) -> R<String> {
        let CV::Int(x) = v else { return no(format!("{} takes an integer 0 to 255", what)) };
        let iv = self.interval(x);
        let x = self.hold_int(x.clone());
        match iv.or_else(|| self.interval(&x)) {
            Some((lo, hi)) if lo >= 0 && hi <= 255 => {}
            Some((lo, hi)) if lo == hi => return no(format!("{} takes an integer 0 to 255, given {}", what, lo)),
            _ => self.line(&format!("if (UNLIKELY((u64)({}) > 255)) FAIL(500, \"a lane holds 0 to 255\", 22);", x)),
        }
        Ok(x)
    }

    /// `a op b` where either side is a vector: lane by lane. An integer on one side is that byte in every lane.
    pub(super) fn vec_bin(&mut self, op: &str, l: CV, r: CV) -> R<CV> {
        let n = match (&l, &r) { (CV::Vec(n, _), _) | (_, CV::Vec(n, _)) => *n, _ => unreachable!() };
        if op == ">>" {
            let CV::Vec(_, a) = &l else { return no(format!("`>>` shifts a vector's lanes by an integer, not an integer by a {}", vec_name(n))) };
            let CV::Int(k) = &r else { return no(format!("`>>` shifts the lanes of a {} by an integer 0 to 7", vec_name(n))) };
            let iv = self.interval(k);
            let k = self.hold_int(k.clone());
            match iv.or_else(|| self.interval(&k)) {
                Some((lo, hi)) if lo >= 0 && hi <= 7 => {}
                Some((lo, hi)) if lo == hi => return no(format!("`>>` shifts the lanes of a {} by 0 to 7, given {}", vec_name(n), lo)),
                _ => self.line(&format!("if (UNLIKELY((u64)({}) > 7)) FAIL(500, \"shift count out of range\", 24);", k)),
            }
            let e = format!("VSHR{}({}, {})", n, a, k);
            return Ok(self.hold_vec(n, e));
        }
        let mac = match op { "&" => "VAND", "|" => "VOR", "^" => "VXOR", "+" => "VADD", "-" => "VSUB", "==" => "VEQ", "!=" => "VNE", "<" => "VLT", "<=" => "VLE", ">" => "VGT", ">=" => "VGE",
            _ => return no(format!("`{}` is not an operator of {} (lane by lane: & | ^ + - == != < <= > >=, and >> by a constant)", op, vec_name(n))) };
        let side = |g: &mut Self, v: &CV| -> R<String> {
            match v {
                CV::Vec(m, e) if *m == n => Ok(e.clone()),
                CV::Vec(m, _) => no(format!("`{}` of a {} and a {}: both sides must have the same number of lanes", op, vec_name(n), vec_name(*m))),
                CV::Int(_) => { let b = g.lane_byte(v, &format!("`{}` with a {}", op, vec_name(n)))?; let t = g.tmp("w"); g.line(&format!("V{} {} = VSPLAT{}({});", n, t, n, b)); Ok(t) }
                _ => no(format!("`{}` of a {} takes another {} or an integer 0 to 255", op, vec_name(n), vec_name(n))),
            }
        };
        let (a, b) = (side(self, &l)?, side(self, &r)?);
        let e = format!("{}{}({}, {})", mac, n, a, b);
        Ok(self.hold_vec(n, e))
    }

    /// The vector built-in `SIMD[code]` of `a` (arity is checked here).
    pub(super) fn simd_builtin(&mut self, code: u8, a: &[CV]) -> R<CV> {
        let name = crate::lang::SIMD[code as usize].0;
        let arity = |k: usize| if a.len() == k { Ok(()) } else { no(format!("`{}` takes {} argument{}, given {}", name, k, if k == 1 { "" } else { "s" }, a.len())) };
        let int_of = |v: &CV| -> R<String> { match v { CV::Int(x) => Ok(x.clone()), _ => no(format!("`{}` takes an integer", name)) } };
        match name {
            "u8x16" | "u8x32" | "u8x64" => {
                let n: u8 = name[3..].parse().unwrap();
                if a.len() == 1 {
                    let b = self.lane_byte(&a[0], &format!("`{}(x)`", name))?;
                    return Ok(self.hold_vec(n, format!("VSPLAT{}({})", n, b)));
                }
                if n != 16 || a.len() != 16 { return no(format!("`{}` takes one byte (every lane){}, given {} arguments", name, if n == 16 { " or 16 (lane by lane)" } else { "" }, a.len())); }
                let mut bs = Vec::new();
                for v in a {
                    let x = int_of(v)?;
                    match self.interval(&x) { Some((lo, hi)) if lo == hi && (0..=255).contains(&lo) => bs.push(lo.to_string()), _ => return no("`u8x16(b0, ..., b15)` takes 16 constants 0 to 255".to_string()) }
                }
                Ok(self.hold_vec(16, format!("VTAB16({})", bs.join(", "))))
            }
            "load_u8x16" | "load_u8x32" | "load_u8x64" => {
                arity(2)?;
                let n: u8 = name[8..].parse().unwrap();
                let i = self.hold_int(int_of(&a[1])?);
                let (p, len) = match &a[0] {
                    CV::Lit(s) => (format!("TP_({}ULL)", intern_lit(s.as_bytes())), format!("{}ULL", s.len())),
                    CV::Txt(x) => {
                        self.line(&format!("if (UNLIKELY(!{x}.present)) FAIL(500, \"expected a string argument\", 26);"));
                        (format!("TP_({x}.off)"), format!("{x}.len"))
                    }
                    CV::StateStr(c, idx, ci) => {
                        let lenoff = self.prog.colls[*c].cols[*ci].off;
                        let bytes = &self.prog.colls[*c].cols[*ci + 1];
                        let CKind::StrBytes(m) = bytes.kind else { return no("string column layout") };
                        (format!("BP_({} + (i64)({}) * {})", bytes.off, idx, m), format!("LD_U32({}, {})", lenoff, idx))
                    }
                    CV::PlainStr(off) => {
                        let len = self.str_lens.get(off).cloned().unwrap_or_else(|| format!("(i64)LD_U32({}, 0)", off));
                        (format!("BP_({})", off + 4), len)
                    }
                    CV::Sub(base, from, count) => (format!("BP_({})", self.sub_bytes_at(base, from)), count.clone()),
                    _ => return no(format!("`t.{}(i)` loads from a text", name)),
                };
                Ok(self.hold_vec(n, format!("VLD{}({}, {}, {})", n, p, len, i)))
            }
            "mask" => {
                arity(1)?;
                let CV::Vec(n, v) = &a[0] else { return no("`v.mask()` is for a vector (u8x16, u8x32, u8x64)") };
                let r = self.hold_int(format!("VMASK{}({})", n, v));
                if *n < 64 { self.ranges.insert(r.clone(), (0, (1i64 << *n) - 1)); }
                Ok(CV::Int(r))
            }
            "lane" => {
                arity(2)?;
                let CV::Vec(n, v) = &a[0] else { return no("`v.lane(k)` is for a vector (u8x16, u8x32, u8x64)") };
                let (n, v) = (*n, v.clone());
                let k = int_of(&a[1])?;
                let iv = self.interval(&k);
                let k = self.hold_int(k);
                match iv.or_else(|| self.interval(&k)) {
                    Some((lo, hi)) if lo >= 0 && hi < n as i64 => {}
                    Some((lo, hi)) if lo == hi => return no(format!("`v.lane(k)` of a {} takes 0 to {}, given {}", vec_name(n), n - 1, lo)),
                    _ => self.line(&format!("if (UNLIKELY((u64)({}) >= {})) FAIL(500, \"lane out of range\", 17);", k, n)),
                }
                let r = self.hold_int(format!("VLANE{}({}, {})", n, v, k));
                self.ranges.insert(r.clone(), (0, 255));
                Ok(CV::Int(r))
            }
            "lookup" => {
                arity(2)?;
                let CV::Vec(16, t) = &a[0] else { return no("`table.lookup(idx)`: the table is a u8x16") };
                let CV::Vec(n, i) = &a[1] else { return no("`table.lookup(idx)`: the indices are a vector (u8x16, u8x32, u8x64)") };
                let e = format!("VLOOK{}({}, {})", n, t, i);
                Ok(self.hold_vec(*n, e))
            }
            "popcount" | "trailing_zeros" | "leading_zeros" | "prefix_xor" => {
                arity(1)?;
                let x = int_of(&a[0])?;
                let mac = match name { "popcount" => "VPOPCNT", "trailing_zeros" => "VTZCNT", "leading_zeros" => "VLZCNT", _ => "VPXOR" };
                let r = self.hold_int(format!("{}({})", mac, x));
                if name != "prefix_xor" { self.ranges.insert(r.clone(), (0, 64)); }
                Ok(CV::Int(r))
            }
            _ => unreachable!(),
        }
    }
}
