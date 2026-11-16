# SIMD vector types (design; step 1 built)

**Status.** Step 1 is built (see "Step 1 findings" at the end). Step 2 (`--cpu auto` with two compiled paths and startup dispatch) is planned in section 8 and is not built.

**The question first.** Mitochondria already reaches SIMD speed for a few hand-written C kernels (the base64 example in `tests/simd_base64`), but the language cannot express byte-lane work itself. This slice adds three byte-lane vector types and a small set of operations that map one-to-one onto x86 instructions, so a program can scan bytes 16, 32 or 64 at a time without dropping to C. Vectors are values, not storage: they live in locals, parameters and results only.

## 1. Types

| type | lanes | width |
|------|-------|-------|
| `u8x16` | 16 | 128 bits |
| `u8x32` | 32 | 256 bits |
| `u8x64` | 64 | 512 bits |

Lane element type is `u8`. Vectors are allowed only in locals, parameters and results. A vector in a state field, an array or a collection is refused at compile time with a "not yet" error. (Storage in state comes after the layout work is designed.)

## 2. Construction

- `u8x16(x)`, `u8x32(x)`, `u8x64(x)`: splat. Every lane equals `x` (an `i64` truncated to `u8`, so the value must be 0..255 to be meaningful; values outside 0..255 are a compile error).
- A 16-literal constant table: `u8x16(b0, b1, ..., b15)`. Lane k is `bk`. Each literal must be a constant 0..255. Only the 16-lane form has a literal constructor; wider vectors are built from splat, load and arithmetic.
- `t.load_u8x16(i)`, `t.load_u8x32(i)`, `t.load_u8x64(i)` load N consecutive bytes of text `t` starting at byte `i`. Lane k is byte `i + k`. Lanes whose byte index is `>= t.len` (or `i + k` is negative) are zero. The load never fails. An in-range load is a single unaligned vector load with no runtime call (profile-tested).

## 3. Lane order and masks

- Lane k is byte `i + k` of the source. Lane 0 is the lowest address.
- Comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=` against a vector or a splat scalar) yield a vector whose lanes are `0xFF` (true) or `0x00` (false).
- `v.mask()` returns an `i64` whose bit k is set iff lane k's high bit (bit 7) is set. This matches `pmovmskb`. For `u8x64` the result uses all 64 bits, so bit 63 is lane 63. For a comparison result, the mask therefore has bit k set iff lane k compared true.
- `v.lane(k)` returns lane k as an `i64` in 0..255. `k` must be in 0..N-1 (N is the lane count). It is checked: an out-of-range `k` fails with the same code as other index faults. (Pinned: `k` is checked against the lane count, not against 255.)

## 4. Operators

| operator | meaning |
|----------|---------|
| `&`, `\|`, `^` | lane-wise bitwise and, or, xor |
| `+`, `-` | lane-wise add and subtract, wrapping modulo 256 |
| `v ^ 255` | lane-wise bitwise not (there is no unary `~`) |
| `==`, `!=`, `<`, `<=`, `>`, `>=` | lane-wise compare, unsigned, result `0xFF`/`0x00` |
| `v >> k` | lane-wise logical right shift by a scalar count k |

### Shift rule (pinned, mirrors scalar)

Scalar `SHL`, `SHR` and `SHRL` (`src/cgen/mod.rs` lines 314-316) FAIL with `"shift count out of range"` and code 500 when the count is outside 0..63. A byte lane has 8 bits, so the per-lane shift must mirror the same failure mode with the lane width:

- A per-lane shift count must be in 0..7. A count outside 0..7 FAILs with `"shift count out of range"` and code 500, the same message and code as the scalar shifts.
- The vector shift therefore never silently produces 0 for a count of 8 or more. Decision: mirror the scalar failure. The alternative (shift to zero) was rejected because the scalar operators fail, and two different rules for one language would be surprising.

## 5. Scalar helpers (i64)

| helper | value for x = 0 | notes |
|--------|-----------------|-------|
| `popcount(x)` | 0 | `__builtin_popcountll` |
| `trailing_zeros(x)` | 64 | codegen `x ? ctz(x) : 64`, avoiding UB |
| `leading_zeros(x)` | 64 | codegen `x ? clz(x) : 64`, avoiding UB |
| `prefix_xor(x)` | 0 | bit k of the result is the xor of bits 0..k of x |

On clang with the matching `-march` level, `trailing_zeros` and `leading_zeros` fold to `tzcnt` and `lzcnt`. `popcount` folds to `popcnt` at x86-64-v2 and above.

`prefix_xor` is a carry-less multiply by all-ones (PCLMUL). The C code uses `#if defined(__PCLMUL__)` for that instruction and a shift-xor cascade otherwise, including under tcc.

### PCLMUL finding (verified, clang 18.1.3)

`x86-64-v3` does **not** define `__PCLMUL__`. Only `native` on this machine defines it. Consequence: under `--cpu x86-64-v3`, `prefix_xor` uses the shift-xor cascade. The cascade is correct, just slower. Users who want the PCLMUL path must pass `--cpu native` (or add `--cflag -mpclmul`).

## 6. `table.lookup(idx)`

Define `table` as a `u8x16` and `idx` as a `u8xN`:

    lane_k = (idx_k & 0x80) ? 0 : table[idx_k & 15]

This is the `pshufb` rule, with the 16-byte table broadcast to each 128-bit half for `u8x32` and `u8x64`. It is defined for all three widths, so the same program means the same thing at every width. Codegen uses `_mm_shuffle_epi8` under `__SSSE3__`, and a portable loop otherwise.

## 7. `--cpu` flag

    mito build <program> [--cpu baseline|x86-64-v2|x86-64-v3|native] [--cflag ...]

- Maps to `-march=` placed before user `--cflag` values, so a user `--cflag -march=...` still wins.
- `baseline` is `-march=x86-64` (SSE2 only). `x86-64-v2` adds SSSE3, SSE4.2, POPCNT. `x86-64-v3` adds AVX2 and BMI, without PCLMUL. `native` is the host.
- Absent `--cpu`, no `-march` is passed, so existing builds are unchanged. (`src/` contains no existing `-march` or `-Werror`.)
- `--cpu` combined with `--tcc` is an error that names both flags. TinyCC does not take `-march` for these levels, and silently ignoring it would be a silent fallback.
- Gating: 256-bit movemask needs `__AVX2__`, pshufb needs `__SSSE3__`. Without the gate, the helper uses a portable path, so baseline builds still compile.

## 8. Step 2 (planned, not built): `--cpu auto`

The default would become `auto`. The compiler emits two object files: one for `x86-64-v3` and one for baseline. At startup the program checks CPU features once (cpuid) and calls the matching entry point. The cost is two compiled copies of hot code and one dispatch branch per entry point. Open questions for step 2: code-size budget, whether dispatch happens per program or per kernel, and how `--tcc` participates (likely baseline only).

## 9. Ownership and refusals

- Helpers are `static inline __attribute__((always_inline))`. Each has a clang body (vector typedefs and intrinsics) and a tcc body (byte-array struct and loops). Codegen never emits raw C operators on vector values, so the two backends share one lowering.
- Vector values are never stored in a state struct, array or collection. The refusal is a compile error with the text "not yet".

## 10. Tests required

- `tests/cases/*.case` for each operator and constructor.
- A clang-vs-tcc comparison modeled on `tests/simd_base64`.
- A `tests/errors` entry for a vector in a state field.
- A profile test: an in-range `load_u8x16/32/64` is a single vector load with no runtime call.
- Example: count `"` not preceded by `\` in stdin, using `u8x64` compare, `mask`, `popcount`, with the backslash state carried across 64-byte blocks and chunk boundaries. Cross-checked against a `byte_at` loop and timed on about 200 MB with `--cpu x86-64-v3`.

## Step 1 findings (2026-11-15)

Built as above, with these differences from the plan:

- **No `~v`.** The language has no unary `~`; write `v ^ 255` (an integer beside a vector is that byte in every lane), which clang folds the same way.
- **Shift counts** follow the plan: a constant outside 0..7 is a compile error (`shifts the lanes of a u8x32 by 0 to 7, given 8`), and a computed one is tested at run time and fails with `shift count out of range` (500), as the scalar shifts do.
- **Lane bytes.** A byte given to `u8x32(b)` or beside a vector in an operator must be 0..255: a constant outside is a compile error, a computed one fails with `a lane holds 0 to 255` (500).
- **Where vectors go.** Locals, parameters and results, also of functions compiled apart (recursive or outlined: `V32` parameters and `V32 *ret_v`). A vector cannot be written into an answer or turned into a value of any kind (`.lane(k)` and `.mask()` read it); a state field of a vector type is refused (tests/errors/vector_*.mito).
- **The prelude** (`VEC_C` in src/cgen/vectors.rs) is emitted only into a program whose generated code declares a vector, so other programs are built exactly as before.
- **Speed.** examples/vectors/quotes.mito counts the `"` not after a `\` in 2.9 GB of GH Archive JSON (WSL, Core Ultra 9 185H, best of 5, minus the 1.7 s a program that only reads its input takes): about 0.37 s with `u8x64` (7.9 GB/s; baseline SSE2 and `native` builds within 5%), 3.5 s with a `byte_at` loop (0.83 GB/s).
- **Reading input is now the larger cost** in that program: 1.7 s for 2.9 GB, most of it first touches of fresh pages of the text area (a later step could map files instead of reading them).
