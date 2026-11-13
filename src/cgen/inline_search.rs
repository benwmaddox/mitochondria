//! `starts_with`, `ends_with`, `contains`, `index_of` and `last_index_of` with a literal second argument, emitted in line: a compare
//! of a fixed number of bytes (one byte: a byte search), not a call of the runtime's builtin with an argument array.
use super::*;

impl<'p> G<'p> {
    /// Where the bytes of a text are and how many there are, as C expressions (a pointer and a `u64`); `None` for a kind that is not
    /// read in line. A missing text (null) fails the request, as the builtin does: the test is emitted here.
    fn text_bytes(&mut self, t: &CV) -> Option<(String, String)> {
        Some(match t {
            CV::Lit(s) => (format!("TXP_({}ULL)", intern_lit(s.as_bytes())), format!("{}ULL", s.len())),
            CV::Txt(x) => {
                self.line(&format!("if (UNLIKELY(!{x}.present)) FAIL(500, \"expected a string argument\", 26);"));
                (format!("TXP_({x}.off)"), format!("(u64){x}.len"))
            }
            CV::StateStr(c, idx, ci) => {
                let lenoff = self.prog.colls[*c].cols[*ci].off;
                let bytes = &self.prog.colls[*c].cols[*ci + 1];
                let CKind::StrBytes(m) = bytes.kind else { return None };
                (format!("STP_({}ULL + (u64)({idx}) * {m}ULL)", bytes.off), format!("(u64)LD_U32({lenoff}, {idx})"))
            }
            CV::PlainStr(off) => {
                let len = self.str_lens.get(off).cloned().unwrap_or_else(|| format!("(i64)LD_U32({}, 0)", off));
                (format!("STP_({}ULL)", off + 4), format!("(u64)({len})"))
            }
            CV::Sub(base, from, count) => (format!("STP_({})", self.sub_bytes_at(base, from)), format!("(u64)({count})")),
            _ => return None,
        })
    }

    /// The five searches of a text for a literal. `None` when the call is not of that shape (the generic path then runs).
    pub(super) fn inline_search(&mut self, bi: B, a: &[CV]) -> Option<CV> {
        let name = match bi {
            B::StartsWith => "BSTARTS_", B::EndsWith => "BENDS_", B::Contains | B::IndexOf => "BFIND_", B::LastIndexOf => "BRFIND_",
            _ => return None,
        };
        let [t @ (CV::Txt(_) | CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..)), CV::Lit(part)] = a else { return None };
        // a column or field layout that is not a plain byte string is left to the builtin (checked before anything is emitted)
        if let CV::StateStr(c, _, ci) = t { if !matches!(self.prog.colls[*c].cols.get(*ci + 1).map(|b| &b.kind), Some(CKind::StrBytes(_))) { return None; } }
        let (p, n) = self.text_bytes(t)?;
        let q = format!("TXP_({}ULL)", intern_lit(part.as_bytes()));
        let r = self.tmp("v");
        self.line(&format!("i64 {r} = {name}({p}, {n}, {q}, {}ULL);", part.len()));
        Some(match bi {
            B::StartsWith | B::EndsWith => CV::Bool(format!("({r} != 0)")),
            B::Contains => CV::Bool(format!("({r} >= 0)")),
            _ => CV::Int(r),
        })
    }
}
