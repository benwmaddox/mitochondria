//! Built-ins emitted as C in line, where the runtime's generic `c->h->builtin(...)` call would give the same answer:
//! `chr` of a code point the range analysis proves ascii, and `trim_start`/`trim_end` (a window onto the text: no bytes are written).
//! `min`/`max` already inline for every kind the generator produces in practice (probes with typed inputs, fields, `or`, recursion
//! never give a run-time-kind argument), so nothing is added for them.
//!
//! Not here, on purpose: `worker_index`, `worker_count` and `data_version` (the generated code has no way to read the
//! runtime's worker identity or its version counter: `Ctx` has no such fields, and the safety profile allows no member access
//! but `c->h->`), and `lower`/`upper` of an ascii text (they write new bytes into the text area, and the profile lets nothing
//! but a helper write memory).
use super::*;

/// The bytes `trim_start`/`trim_end` remove, as the runtime's `split_ws`: a space and bytes 9 to 13.
fn is_ws(c: char) -> bool { c == ' ' || ('\t'..='\r').contains(&c) }

impl<'p> G<'p> {
    /// The built-in `bi` of `a` emitted in line, or None to leave it to the generic call. Run after the arity checks.
    pub(super) fn inline_misc(&mut self, bi: B, a: &[CV]) -> R<Option<CV>> {
        match (bi, a) {
            // a code point 0 to 127 is one byte of a table of the 128 ascii characters: a window onto it, nothing written
            (B::Chr, [CV::Int(k)]) => {
                let Some((lo, hi)) = self.interval(k) else { return Ok(None) };
                if lo < 0 || hi > 127 { return Ok(None); }
                let k = self.hold_int(k.clone());
                let off = intern_lit(&(0u8..128).collect::<Vec<u8>>());
                let t = self.tmp("t");
                self.line(&format!("Txt {} = {{ {} + (u32)({}), 1, 1 }};", t, off, k));
                self.txt_bounds.insert(t.clone(), 1);
                Ok(Some(CV::Txt(t)))
            }
            (B::TrimStart | B::TrimEnd, [v @ (CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..))]) => {
                if let CV::Lit(s) = v {
                    let r = if bi == B::TrimStart { s.trim_start_matches(is_ws) } else { s.trim_end_matches(is_ws) };
                    return Ok(Some(CV::Lit(r.to_string())));
                }
                let bound = self.txt_bound(v);
                let present = matches!(v, CV::Txt(_));
                let t = self.txt_of(v.clone())?;
                // a missing text is an error, as the builtin's
                if present { self.line(&format!("if (UNLIKELY(!{t}.present)) FAIL(500, \"expected a string argument\", 26);")); }
                let (s, k, ch, r) = (self.tmp("n"), self.tmp("k"), self.tmp("ch"), self.tmp("t"));
                let ws = format!("{ch} == 32 || ({ch} >= 9 && {ch} <= 13)");
                if bi == B::TrimStart {
                    self.line(&format!("u32 {s} = 0; for (u32 {k} = 0; {k} < {t}.len; {k}++) {{ i64 {ch} = TX_({t}.off + {k}); if (!({ws})) break; {s}++; }}"));
                    self.line(&format!("Txt {r} = {{ {t}.off + {s}, {t}.len - {s}, 1 }};"));
                } else {
                    self.line(&format!("u32 {s} = {t}.len; for (u32 {k} = 0; {k} < {t}.len; {k}++) {{ i64 {ch} = TX_({t}.off + {t}.len - 1 - {k}); if (!({ws})) break; {s}--; }}"));
                    self.line(&format!("Txt {r} = {{ {t}.off, {s}, 1 }};"));
                }
                if let Some(b) = bound { self.txt_bounds.insert(r.clone(), b); }
                Ok(Some(CV::Txt(r)))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen(body: &str) -> String {
        let src = format!("struct A {{ s: string<16>; n: i64 in 0..2; m: i64; }}\nstate app: A;\n@post(\"/t\") fn t(x: utf8<16>, k: i64 from body) -> Response<{{ v: i64 }}> {{ {} }}\n", body);
        let (state, api) = crate::source::load(&src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        g.src[g.src.find("int ep0(Ctx").unwrap()..].to_string()
    }

    /// `chr` of a code point proven 0..127 is a window onto a table; any other keeps the runtime call (number 98).
    #[test]
    fn chr_of_ascii_is_a_window() {
        let proven = gen("return ok({ v: len(chr(app.n + 65)) });");
        assert!(!proven.contains("builtin(c, 98"), "{}", proven);
        let open = gen("return ok({ v: len(chr(k)) });");
        assert!(open.contains("builtin(c, 98"), "{}", open);
    }

    /// `trim_start` / `trim_end` of text are a loop over bytes and a window: no runtime call (numbers 69 and 70).
    #[test]
    fn trims_are_windows() {
        let t = gen("return ok({ v: len(trim_start(x)) + len(trim_end(x)) + len(trim_start(app.s)) });");
        assert!(!t.contains("builtin(c, 69") && !t.contains("builtin(c, 70"), "{}", t);
        assert!(t.contains("TX_("), "{}", t);
        let lit = gen("return ok({ v: len(trim_end(\"  ab  \")) });");
        assert!(!lit.contains("builtin(c, 70") && lit.contains("4LL"), "{}", lit);
    }
}
