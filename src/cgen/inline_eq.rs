//! Text comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=`) read where the texts are, in line: no runtime call. Equality is
//! "same length and `memcmp` equal"; ordering is `BCMP_` (the first differing byte as unsigned, else the shorter first),
//! exactly the runtime's `bytes_cmp`. A literal's length is a constant, so the C compiler folds `memcmp` against it.
use super::*;

impl<'p> G<'p> {
    /// A window onto a text for an in-line compare: (pointer expression, length expression). `None` for any kind of text
    /// that is not a literal, a plain field, a text column or a piece of one (those keep the runtime helper).
    fn cmp_place(&mut self, v: &CV) -> Option<(String, String)> {
        match v {
            CV::Lit(s) => Some((format!("TP_({}ULL)", intern_lit(s.as_bytes())), format!("{}ULL", s.len()))),
            b @ (CV::PlainStr(_) | CV::StateStr(..)) => {
                let len = self.hold_int(self.field_len(b));
                Some((format!("BP_({})", self.sub_bytes_at(b, "0")), format!("(u64)({})", len)))
            }
            CV::Sub(b, from, n) => {
                let from = self.hold_int(from.clone());
                let n = self.hold_int(n.clone());
                Some((format!("BP_({})", self.sub_bytes_at(b, &from)), format!("(u64)(u32)({})", n)))
            }
            _ => None,
        }
    }

    /// The boolean C expression of `l <op> r` (`code` indexes OPS) for two texts compared in line, or `None` to keep the helper.
    pub(super) fn inline_text_cmp(&mut self, l: &CV, r: &CV, code: usize) -> Option<String> {
        if matches!((l, r), (CV::Lit(_), CV::Lit(_))) { return None; }
        let ((p, pn), (q, qn)) = (self.cmp_place(l)?, self.cmp_place(r)?);
        Some(match code {
            0 => format!("({} == {} && BEQ_({}, {}, {}))", pn, qn, p, q, pn),
            1 => format!("({} != {} || !BEQ_({}, {}, {}))", pn, qn, p, q, pn),
            _ => format!("(BCMP_({}, {}, {}, {}) {} 0)", p, pn, q, qn, ["<", "<=", ">", ">="][code - 2]),
        })
    }
}
