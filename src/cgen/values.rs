//! Holding values, text helpers, expressions, loads, fields and table loops of G.
use super::*;

impl<'p> G<'p> {
/// Give a value its own C variable (so it is computed once, and a `var` can be assigned later).
    pub(super) fn hold(&mut self, v: CV, mutable: bool) -> R<CV> {
        // a `let` may share a C name with the value it is given, unless that name is a `var`'s (which a later assignment changes)
        let shared = |g: &Self, e: &str| !mutable && is_name(e) && !g.var_names.contains(e);
        Ok(match v {
            CV::Int(e) if !shared(self, &e) => {
                let t = self.tmp("v");
                self.line(&format!("i64 {} = {};", t, e));
                if mutable { self.var_names.insert(t.clone()); } else if let Some(r) = self.interval(&e) { self.ranges.insert(t.clone(), r); } // a `let` keeps its value's range
                CV::Int(t)
            }
            CV::Flt(e) if !shared(self, &e) => { let t = self.tmp("v"); self.line(&format!("double {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Flt(t) }
            CV::Bool(e) if !shared(self, &e) => { let t = self.tmp("v"); self.line(&format!("int {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Bool(t) }
            CV::Item(c, e) if !shared(self, &e) => { let t = self.tmp("i"); self.line(&format!("u32 {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Item(c, t) }
            CV::Txt(e) if mutable || self.var_names.contains(&e) => { let t = self.tmp("t"); self.line(&format!("Txt {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Txt(t) }
            CV::Any(e) if mutable || self.var_names.contains(&e) => { let t = self.tmp("v"); self.line(&format!("Val {} = {};", t, e)); if mutable { self.var_names.insert(t.clone()); } CV::Any(t) }
            CV::Lit(s) if mutable => { let t = self.txt_of(CV::Lit(s))?; self.var_names.insert(t.clone()); CV::Txt(t) }
            // a `let` of a text field is a window onto it (no copy: each use reads the field as it is then); a `var` holds a copy
            CV::StateStr(c, idx, ci) if !mutable => { let idx = if self.var_names.contains(&idx) { let t = self.tmp("i"); self.line(&format!("u32 {} = {};", t, idx)); t } else { idx }; CV::StateStr(c, idx, ci) }
            CV::PlainStr(off) if !mutable => CV::PlainStr(off),
            v @ CV::Sub(..) if !mutable => v,
            CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) => { let t = self.txt_of(v)?; self.var_names.insert(t.clone()); CV::Txt(t) }
            // `var x = null`: its kind is the first value given it later (a row makes it `T?`, text a text that may be null); the endpoint is
            // generated again with that kind, never a value whose kind is known only at run time
            CV::Null if mutable => CV::Null,
            CV::Ref(c, s, k) if mutable || self.var_names.contains(&s) || self.var_names.contains(&k) => { let (s2, k2) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}, {} = {};", s2, s, k2, k)); if mutable { self.var_names.insert(s2.clone()); self.var_names.insert(k2.clone()); } CV::Ref(c, s2, k2) }
            o => o,
        })
    }
    /// An integer where the language requires one: a value of run-time kind is checked; anything else fails the request
    /// with the interpreter's error (`code`, `msg`).
    pub(super) fn int_or(&mut self, v: CV, code: u16, msg: &str) -> String {
        match v {
            CV::Int(e) => self.hold_int(e),
            CV::Any(x) => { self.open(&format!("if ({}.tag != 1) {{", x)); self.fail(code, msg); self.close("}"); format!("{}.i", x) }
            _ => { self.fail(code, msg); "0LL".into() }
        }
    }
    pub(super) fn hold_int(&mut self, e: String) -> String {
        if is_name(&e) { return e; }
        let t = self.tmp("v");
        self.line(&format!("i64 {} = {};", t, e));
        if let Some(r) = load_interval(&e).or_else(|| if e.starts_with("(i64)") { self.interval(&e) } else { None }) { self.ranges.insert(t.clone(), r); } // a fresh name, never assigned again: it keeps the load's range
        t
    }

    pub(super) fn assign_var(&mut self, cur: &CV, v: CV) -> R<()> {
        match (cur, v) {
            (CV::Int(t), CV::Int(e)) | (CV::Bool(t), CV::Bool(e)) | (CV::Flt(t), CV::Flt(e)) => self.line(&format!("{} = {};", t, e)),
            (CV::Item(a, t), CV::Item(b, e)) if *a == b => self.line(&format!("{} = {};", t, e)),
            (CV::Txt(t), v @ (CV::Txt(_) | CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..))) => { let s = self.txt_of(v)?; self.line(&format!("{} = {};", t, s)); }
            (CV::Any(t), v) => { let s = self.val(v)?; self.line(&format!("{} = {};", t, s)); }
            (CV::Txt(t), CV::Null) => self.line(&format!("{} = (Txt){{ 0, 0, 0 }};", t)),
            (CV::Ref(a, s, k), CV::Ref(b, s2, k2)) if *a == b => self.line(&format!("{} = {}; {} = {};", s, s2, k, k2)),
            (CV::Ref(a, s, k), CV::Item(b, e)) if *a == b => self.line(&format!("{} = 1; {} = {};", s, k, e)),
            (CV::Ref(_, s, _), CV::Null) => self.line(&format!("{} = 0;", s)),
            _ => return no("a `var` assigned a value of another kind"),
        }
        Ok(())
    }

    /// Append pieces to a string field in place (`kind` 0 plain, 1 an item's column; a, b, cc as `str_app` takes them), emptying it first
    /// when `clear` (an assignment). A piece that is not text is formatted as `+` would.
    pub(super) fn str_append(&mut self, kind: i32, a: String, b: String, cc: u32, clear: bool, pieces: &[&'p Expr], env: &mut Vec<Option<CV>>) -> R<()> {
        // no window among the pieces: all of them in one call
        let vals: Vec<CV> = pieces.iter().map(|p| self.expr(p, env, None)).collect::<R<Vec<CV>>>()?;
        if kind & 4 != 0 { for v in &vals { self.ascii_lit(v, "the field")?; } }
        if !vals.iter().any(|v| matches!(v, CV::Sub(..) | CV::PlainStr(_) | CV::StateStr(..))) {
            let mut names = Vec::new();
            for v in vals {
                if matches!(v, CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..)) { return no("only text, numbers and bools can be added to a string"); }
                names.push(self.val(v)?);
            }
            let argv = self.tmp("argv");
            self.line(&format!("Val {}[{}] = {{ {} }};", argv, names.len(), names.join(", ")));
            self.call(&format!("c->h->str_set(c, {}, {}, {}, {}, {}, {}, {})", kind, a, b, cc, clear as i32, argv, names.len()));
            return Ok(());
        }
        for (i, v) in vals.into_iter().enumerate() {
            if matches!(v, CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..)) { return no("only text, numbers and bools can be added to a string"); }
            let k = kind | if clear && i == 0 { 2 } else { 0 };
            // a text field or a window onto one: copied from where it is, field to field
            let window = match v.clone() { CV::Sub(base, from, count) => Some((*base, from, count)), b @ (CV::PlainStr(_) | CV::StateStr(..)) => { let l = self.field_len(&b); Some((b, "0".to_string(), l)) } _ => None };
            if let Some((base, from, count)) = window {
                let n = self.hold_int(count);
                let at = self.sub_bytes_at(&base, &from);
                self.call(&format!("c->h->str_app_at(c, {}, {}, {}, {}, {}, (u32)({}))", k, a, b, cc, at, n));
                continue;
            }
            let vv = self.val(v)?;
            self.call(&format!("c->h->str_app(c, {}, {}, {}, {}, &{})", k, a, b, cc, vv));
        }
        Ok(())
    }

    /// Write pieces to a capped var of text (C name `t`, `cap` bytes), after what it holds or from the start when `clear`: one call. A
    /// piece that is not text is formatted as `+` would.
    pub(super) fn txt_write(&mut self, t: &str, cap: usize, clear: bool, pieces: &[&'p Expr], env: &mut Vec<Option<CV>>) -> R<()> {
        let mut names = Vec::new();
        for p in pieces {
            let v = self.expr(p, env, None)?;
            if matches!(v, CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..)) { return no("only text, numbers and bools can be added to text"); }
            names.push(self.val(v)?);
        }
        let argv = self.tmp("argv");
        self.line(&format!("Val {}[{}] = {{ {} }};", argv, names.len(), names.join(", ")));
        self.call(&format!("c->h->txt_set(c, &{}, {}, {}, {}, {})", t, cap, clear as i32, argv, names.len()));
        Ok(())
    }

    /// A literal put into `ascii` text: refused when compiling if it holds a byte above 127.
    pub(super) fn ascii_lit(&self, v: &CV, f: &str) -> R<()> {
        if let CV::Lit(s) = v { if let Some(b) = s.bytes().find(|b| *b > 127) { return no(format!("`{}` is ascii text: the literal \"{}\" holds byte {}, which is not ASCII", f, s, b)); } }
        Ok(())
    }
    /// Is this text field `ascii<N>`?
    pub(super) fn is_ascii(&self, base: &CV) -> bool {
        match base {
            CV::StateStr(c, _, ci) => self.prog.colls[*c].cols[*ci].range.is_some(),
            CV::PlainStr(off) => self.plain_ascii(*off),
            _ => false,
        }
    }
    /// Whether the plain text field at `off` is ascii (its struct's field range says so).
    pub(super) fn plain_ascii(&self, off: usize) -> bool {
        fn walk(p: &Program, s: usize, base: usize, off: usize) -> Option<bool> {
            let sd = &p.structs[s];
            for (k, (_, t)) in sd.fields.iter().enumerate() {
                let o = base + sd.offs[k];
                match t { Ty::Str(_) if o == off => return Some(sd.ranges[k].is_some()), Ty::Struct(c) => if let Some(r) = walk(p, *c, o, off) { return Some(r); }, _ => {} }
            }
            None
        }
        self.prog.globals.iter().find_map(|g| match g.ty { Ty::Struct(s) => walk(self.prog, s, g.off, off), _ => None }).unwrap_or(false)
    }

    /// The byte offset in state of byte `from` of a text field (a window's base).
    pub(super) fn sub_bytes_at(&self, base: &CV, from: &str) -> String {
        match base {
            CV::PlainStr(off) => format!("{}ULL + (u64)({})", off + 4, from),
            CV::StateStr(c, idx, ci) => { let b = &self.prog.colls[*c].cols[*ci + 1]; let CKind::StrBytes(m) = b.kind else { unreachable!() }; format!("{}ULL + (u64)({}) * {}ULL + (u64)({})", b.off, idx, m, from) }
            _ => unreachable!("a window's base is a text field"),
        }
    }
    /// A stored id (the C expression of its 8 bytes) read back as a `T?` of pool `target`.
    pub(super) fn id_value(&mut self, target: usize, v: String) -> CV {
        if let Some(t) = tab(target) {
            // a row's id kept in working state: null when 0, or when the row has gone since (row ids are never reused)
            let (s, i, x) = (self.tmp("r"), self.tmp("i"), self.tmp("v"));
            self.line(&format!("i64 {} = {}; u32 {} = {} > 0 && {} <= 4294967295LL, {} = (u32){};", x, v, s, x, x, i, x));
            self.line(&format!("if ({}) {{ CALL(c->h->texists(c, {}, {}, &{})); }}", s, t, i, s));
            return CV::Ref(target, s, i);
        }
        let (s, i) = (self.tmp("r"), self.tmp("i"));
        self.line(&format!("u32 {} = 0, {} = 0;", s, i));
        self.call(&format!("c->h->id_get(c, {}, {}, &{}, &{})", target, v, s, i));
        CV::Ref(target, s, i)
    }
    /// A value stored into an id field of pool `target`: an item, a `T?` or null, as its id (0 for null).
    pub(super) fn id_store_value(&mut self, target: usize, v: CV, what: &str) -> R<CV> {
        if tab(target).is_some() {
            // a row's id (0 for null)
            return Ok(match v {
                CV::Null => CV::Int("0LL".into()),
                CV::Item(c, e) if c == target => CV::Int(format!("(i64)({})", e)),
                CV::Ref(c, s, e) if c == target => CV::Int(format!("({} ? (i64)({}) : 0LL)", s, e)),
                _ => return no(format!("`{}` holds a row of {}: give it a row, a value that may be null, or `null`", what, coll_name(self.prog, target))),
            });
        }
        let wrong = |g: &Self, c: usize| no(format!("`{}` holds an id of {}, given an item of {}", what, g.prog.colls[target].name, g.prog.colls[c].name));
        Ok(match v {
            CV::Null => CV::Int("0LL".into()),
            CV::Item(c, e) => { if c != target { return wrong(self, c); } let t = self.tmp("v"); self.line(&format!("i64 {} = 0;", t)); self.call(&format!("c->h->id_of(c, {}, {}, &{})", target, e, t)); CV::Int(t) }
            CV::Ref(c, s, e) => {
                if c != target { return wrong(self, c); }
                let t = self.tmp("v");
                self.line(&format!("i64 {} = 0;", t));
                self.open(&format!("if ({} != 0) {{", s)); self.call(&format!("c->h->id_of(c, {}, {}, &{})", target, e, t)); self.close("}");
                CV::Int(t)
            }
            _ => return no(format!("`{}` holds an id: give it an item, a value that may be null, or `null`", what)),
        })
    }
    /// The byte offset in state of a text field's length (u32).
    pub(super) fn len_at(&self, base: &CV) -> String {
        match base {
            CV::PlainStr(off) => format!("{}ULL", off),
            CV::StateStr(c, idx, ci) => format!("{}ULL + (u64)({}) * 4ULL", self.prog.colls[*c].cols[*ci].off, idx),
            _ => unreachable!("a text field"),
        }
    }
    /// A text field's length now (a C expression).
    pub(super) fn field_len(&self, base: &CV) -> String {
        match base {
            CV::PlainStr(off) => format!("(i64)LD_U32({}, 0)", off),
            CV::StateStr(c, idx, ci) => format!("(i64)LD_U32({}, {})", self.prog.colls[*c].cols[*ci].off, idx),
            _ => unreachable!("a text field"),
        }
    }

    /// The value as a `Val` variable, for helpers that take any kind of value.
    pub(super) fn val(&mut self, v: CV) -> R<String> {
        let t = self.tmp("v");
        match v {
            CV::Any(e) => return Ok(e),
            CV::Int(e) => self.line(&format!("Val {} = {{ 1, 0, 0, {}, 0 }};", t, e)),
            CV::Flt(e) => self.line(&format!("Val {} = {{ 2, 0, 0, 0, {} }};", t, e)),
            CV::Bool(e) => self.line(&format!("Val {} = {{ 3, 0, 0, {}, 0 }};", t, e)),
            CV::Null => self.line(&format!("Val {} = {{ 0, 0, 0, 0, 0 }};", t)),
            CV::Item(c, e) => self.line(&format!("Val {} = {{ 5, {}, {}, 0, 0 }};", t, c, e)),
            CV::Ref(c, s, k) => self.line(&format!("Val {} = {{ {} == 0 ? 0 : 5, {}, {}, 0, 0 }};", t, s, c, k)),
            v @ (CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..)) => { let x = self.txt_of(v)?; self.line(&format!("Val {} = {{ {}.present ? 4 : 0, {}.off, {}.len, 0, 0 }};", t, x, x, x)); }
            CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) => return no("a collection, array or struct used as a value is not compiled here yet (a struct can be copied into a local or a struct field with `let`, `var` or `=`; passing, returning, comparing and answering with struct values come in later steps of \"Struct values\" in DESIGN.md)"),
            CV::Resp(..) => return no("an answer is read through `.status` and `.body`"),
        }
        Ok(t)
    }

    pub(super) fn txt_of(&mut self, v: CV) -> R<String> {
        match v {
            CV::Txt(t) => Ok(t),
            CV::Lit(s) => { let t = self.tmp("t"); let off = intern_lit(s.as_bytes()); self.line(&format!("Txt {} = {{ {}, {}, 1 }};", t, off, s.len())); Ok(t) }
            CV::PlainStr(off) => { let t = self.tmp("t"); self.line(&format!("Txt {};", t)); self.call(&format!("c->h->plain_txt(c, {}, &{})", off, t)); Ok(t) }
            CV::Sub(base, from, count) => {
                let at = self.sub_bytes_at(&base, &from);
                let t = self.tmp("t");
                self.line(&format!("Txt {};", t));
                self.call(&format!("c->h->arr_txt(c, {}, (u32)({}), &{})", at, count, t));
                Ok(t)
            }
            CV::StateStr(c, idx, ci) => { let t = self.tmp("t"); self.line(&format!("Txt {};", t)); self.call(&format!("c->h->col_txt(c, {}, {}, {}, &{})", c, idx, ci, t)); Ok(t) }
            _ => no("expected a string"),
        }
    }

    /// A condition as a C expression, with the interpreter's rules (only booleans and null are conditions).
    pub(super) fn cond(&mut self, v: CV) -> R<String> {
        Ok(match v {
            CV::Bool(e) => e,
            CV::Null => "0".into(),
            CV::Any(e) => { let t = self.tmp("v"); self.line(&format!("int {};", t)); self.call(&format!("c->h->truthy(c, &{}, &{})", e, t)); t }
            CV::Ref(_, s, _) => { self.fail(500, "condition is not a boolean"); let _ = s; "0".into() }
            v => { let vv = self.val(v)?; let t = self.tmp("v"); self.line(&format!("int {};", t)); self.call(&format!("c->h->truthy(c, &{}, &{})", vv, t)); t }
        })
    }

    /// `dead`: where to jump if a reference turns out not to be live (inside `or(...)` and `exists(...)`); otherwise the
    /// request fails as the interpreter's would.
    pub(super) fn expr(&mut self, e: &'p Expr, env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        match e {
            Expr::Int(n) => Ok(CV::Int(if *n == i64::MIN { "I64MIN".into() } else { format!("{}LL", n) })),
            Expr::Bool(b) => Ok(CV::Bool(if *b { "1" } else { "0" }.into())),
            Expr::Str(s) => Ok(CV::Lit(s.clone())),
            Expr::Null => Ok(CV::Null),
            Expr::Float(f) => Ok(CV::Flt(c_double(*f))),
            // a `write` parameter standing for a field or element of the caller's: read where it is, each time
            Expr::Var(_, VarRes::Local(s)) if self.wslots.get(&(self.frame, *s)).map_or(false, |w| w.loc.is_some()) => { let loc = self.wslots[&(self.frame, *s)].loc.unwrap(); self.expr(loc, env, dead) }
            Expr::Var(n, VarRes::Local(s)) => match env.get(*s as usize).cloned().flatten() { Some(CV::Int(name)) if self.u32_vars.contains(&name) => Ok(CV::Int(format!("(i64){}", name))), v => v.ok_or_else(|| format!("`{}` has no compiled value", n)) },
            Expr::Var(n, VarRes::Global(g)) => {
                let gl = &self.prog.globals[*g as usize];
                if let Some(c) = gl.coll { return Ok(CV::Coll(c, "0".into())); }
                self.plain(gl.off, &gl.ty.clone(), dead).map_err(|_| format!("global `{}` of this kind not compiled yet", n))
            }
            Expr::Var(n, VarRes::Unbound) => no(format!("`{}` was not bound", n)),
            Expr::Field(base, f) => { let bv = self.expr(base, env, dead)?; self.field(bv, f, dead) }
            Expr::Index(base, k_expr) => {
                let bv = self.expr(base, env, dead)?;
                let kv = self.expr(k_expr, env, dead)?;
                let k = self.int_or(kv, 400, "an index must be an integer");
                match bv {
                    CV::Coll(c, inst) if tab(c).is_some() => {
                        // `table[id]`: the row with that id, when it is this table's (and this owner's); otherwise 404
                        let k = self.hold_int(k);
                        let r = self.tmp("i");
                        let msg = format!("no such row in {}", coll_name(self.prog, c));
                        self.line(&format!("if ({k} < 0 || {k} > 4294967295LL) FAIL(404, {}, {});", c_lit(msg.as_bytes()), msg.len()));
                        self.line(&format!("u32 {} = (u32){};", r, k));
                        self.call(&format!("c->h->tfind(c, {}, {}, (u32)({}))", c - TBASE, r, inst));
                        Ok(CV::Item(c, r))
                    }
                    CV::Coll(c, inst) => {
                        let t = self.index_of(c, &inst, &k);
                        self.item_value(c, t, dead)
                    }
                    CV::ArrCol(c, idx, ci) => {
                        let (off, CKind::Arr(sc, n)) = (self.prog.colls[c].cols[ci].off, self.prog.colls[c].cols[ci].kind.clone()) else { unreachable!() };
                        let k = self.hold_int(k);
                        self.elem_check(&k, n as u64);
                        self.load(&off.to_string(), &format!("(i64)({}) * {} + {}", idx, n, k), sc)
                    }
                    // one struct of an array of structs (`item.vals[i]`): its fields are read as `.a`
                    CV::Part(c, idx, p) => {
                        let inner = format!("{}.", p);
                        let Some(CKind::Arr(_, n)) = self.prog.colls[c].cols.iter().find(|x| x.path.starts_with(&inner) && matches!(x.kind, CKind::Arr(..))).map(|x| x.kind.clone()) else { return no("only fixed arrays and collections can be indexed") };
                        let k = self.hold_int(k);
                        self.elem_check(&k, n as u64);
                        Ok(CV::Elem(c, idx, p, k))
                    }
                    CV::PlainArr(off, Ty::Scalar(sc), n) => {
                        let k = self.hold_int(k);
                        if !self.fast_sites.contains(&(&**k_expr as *const Expr)) { self.elem_check(&k, n as u64); }
                        let v = self.load(&off.to_string(), &k, sc)?;
                        // every store in the program put a value inside this range there (or none did: the array holds its starting zeros)
                        let range = FIELD_RANGES.with(|fr| fr.borrow().0.get(&off).copied());
                        Ok(self.with_field_range(v, range))
                    }
                    // a string's bytes are read by index (0..len); strings are never written by index
                    v @ (CV::Txt(_) | CV::Lit(_)) => {
                        let t = self.txt_of(v)?;
                        let k = self.hold_int(k);
                        self.line(&format!("if (RANGE_({k} < 0 || (u64){k} >= (u64){t}.len)) FAIL(400, \"string index out of range\", 25);"));
                        Ok(CV::Int(format!("(i64)TX_({}.off + {})", t, k)))
                    }
                    CV::StateStr(c, idx, ci) => {
                        let k = self.hold_int(k);
                        let lenoff = self.prog.colls[c].cols[ci].off;
                        let bytes = &self.prog.colls[c].cols[ci + 1];
                        let CKind::StrBytes(m) = bytes.kind else { return no("string column layout") };
                        let boff = bytes.off;
                        self.line(&format!("if (RANGE_({k} < 0 || (u64){k} >= (u64)LD_U32({lenoff}, {idx}))) FAIL(400, \"string index out of range\", 25);"));
                        Ok(CV::Int(format!("(i64)LD_U8({}, (i64)({}) * {} + {})", boff, idx, m, k)))
                    }
                    CV::Sub(base, from, count) => {
                        let k = self.hold_int(k);
                        self.line(&format!("if (RANGE_({k} < 0 || {k} >= (i64)({count}))) FAIL(400, \"string index out of range\", 25);"));
                        let at = self.sub_bytes_at(&base, &format!("{} + {}", from, k));
                        Ok(CV::Int(format!("(i64)LD_U8({}, 0)", at)))
                    }
                    // a plain string: its length (u32) then its bytes, read where they are
                    CV::PlainStr(off) => {
                        let k = self.hold_int(k);
                        let len = self.str_lens.get(&off).cloned().unwrap_or_else(|| format!("(i64)LD_U32({}, 0)", off));
                        if !self.fast_sites.contains(&(&**k_expr as *const Expr)) { self.line(&format!("if (RANGE_({k} < 0 || {k} >= {len})) FAIL(400, \"string index out of range\", 25);")); }
                        Ok(CV::Int(format!("(i64)LD_U8({}, {})", off + 4, k)))
                    }
                    _ => { self.fail(500, "cannot index this value"); Ok(CV::Null) }
                }
            }
            Expr::Un(op, x) => {
                let v = self.expr(x, env, dead)?;
                if *op == '!' { let c = self.cond(v)?; return Ok(CV::Bool(format!("!({})", c))); }
                match v {
                    CV::Int(e) => { let e = self.hold_int(e); let r = self.tmp("v"); self.line(&format!("NEG({}, {});", r, e)); Ok(CV::Int(r)) }
                    CV::Flt(e) => Ok(CV::Flt(format!("(-{})", e))),
                    CV::Any(x) => {
                        // as the interpreter: an integer negates with an overflow check, a float as a float, nothing else
                        let (r, n) = (self.tmp("v"), self.tmp("v"));
                        self.line(&format!("Val {} = {};", r, x));
                        self.open(&format!("if ({}.tag == 1) {{", x)); self.line(&format!("NEG({}, {}.i);", n, x)); self.line(&format!("{}.i = {};", r, n)); self.close("}");
                        self.open(&format!("else if ({}.tag == 2) {{", x)); self.line(&format!("{}.f = -{}.f;", r, x)); self.close("}");
                        self.open("else {"); self.fail(500, "cannot negate"); self.close("}");
                        Ok(CV::Any(r))
                    }
                    _ => { self.fail(500, "cannot negate"); Ok(CV::Null) }
                }
            }
            Expr::Bin(op, a, b) => self.bin(op, a, b, env, dead),
            Expr::Call(n, args, res) => self.callx(n, args, *res, env, dead),
        }
    }

    /// The most bytes a text value can hold, when known: a literal's, a field's capacity, a number's longest form, a tracked
    /// text's (an input, a table's text column, a join of those).
    pub(super) fn txt_bound(&self, v: &CV) -> Option<usize> {
        match v {
            CV::Lit(s) => Some(s.len()),
            CV::Txt(n) => self.txt_bounds.get(n).copied(),
            CV::StateStr(c, _, ci) => match self.prog.colls[*c].cols.get(ci + 1).map(|x| &x.kind) { Some(crate::layout::CKind::StrBytes(m)) => Some(*m), _ => None },
            CV::PlainStr(off) => self.plain_str_cap(*off),
            CV::Sub(b, ..) => self.txt_bound(b),
            CV::Int(_) => Some(20), CV::Flt(_) => Some(32), CV::Bool(_) => Some(5), CV::Null => Some(0),
            _ => None,
        }
    }
    /// The most bytes of text a builtin gives, from its arguments' bounds (None: not known, as for a header or the environment)
    pub(super) fn builtin_bound(&self, bi: B, a: &[CV]) -> Option<usize> {
        let n = |k: usize| a.get(k).and_then(|v| self.txt_bound(v));
        let int = |k: usize| match a.get(k) { Some(CV::Int(e)) => e.strip_suffix("LL").unwrap_or(e).parse::<usize>().ok(), _ => None };
        match bi {
            B::Str => match a.first() { Some(CV::Int(_)) => Some(20), Some(CV::Flt(_)) => Some(32), Some(CV::Bool(_)) => Some(5), _ => n(0) },
            B::Base64 | B::Base64Url => n(0).map(|x| (x + 2) / 3 * 4),
            B::Unbase64 => n(0).map(|x| (x + 3) / 4 * 3),
            B::Hex => n(0).map(|x| 2 * x),
            B::Sha1 | B::HmacSha1 => Some(20),
            B::Sha256 | B::HmacSha256 => Some(32),
            B::Pbkdf2Sha256 => int(3),
            B::RandomBytes => int(0),
            B::XorBytes => n(0).zip(n(1)).map(|(x, y)| x.max(y)),
            B::GcmSeal => n(2).map(|x| x + 16),
            B::GcmOpen => n(2),
            B::BcryptHash => Some(60),
            B::FormEncode => n(0).map(|x| 3 * x),
            B::FormDecode | B::JsonUnquote | B::TrimStart | B::TrimEnd => n(0),
            // each `%` directive writes at most 32 bytes (a zone or weekday name, a year)
            B::FormatTime => n(1).map(|x| 32 * x),
            B::EscapeHtml => n(0).map(|x| 6 * x),
            B::JsonQuote => n(0).map(|x| 6 * x + 2),
            // a character's other case takes at most three times its bytes
            B::Lower | B::Upper => n(0).map(|x| 3 * x),
            B::Chr => Some(4),
            B::StdinText => int(0),
            B::ReadFile => int(1),
            B::RandomText => int(0).zip(n(1)).map(|(k, _)| 4 * k),
            B::Replace => match (n(0), n(1), n(2)) { (Some(x), Some(_), Some(z)) => match a.get(1) { Some(CV::Lit(s)) if !s.is_empty() => Some(x + x / s.len() * z), _ => None }, _ => None },
            _ => None,
        }
    }
    /// The capacity of the plain text field at `off`, found through the state's structs.
    pub(super) fn plain_str_cap(&self, off: usize) -> Option<usize> {
        fn walk(prog: &Program, ty: &Ty, at: usize, off: usize) -> Option<usize> {
            match ty {
                Ty::Str(m) if at == off => Some(*m as usize),
                Ty::Struct(s) => { let sd = &prog.structs[*s]; sd.fields.iter().zip(sd.offs.iter()).find_map(|(f, o)| walk(prog, &f.1, at + o, off)) }
                _ => None,
            }
        }
        self.prog.globals.iter().find_map(|g| if g.coll.is_none() { walk(self.prog, &g.ty, g.off, off) } else { None })
    }
    /// Field k of the plain struct sid at `off`: when that is the request struct, its range is one the endpoint must find zeroed.
    pub(super) fn touch_request(&mut self, off: usize, sid: usize, k: usize) {
        let Some((ro, _)) = self.prog.request else { return };
        if off != ro { return; }
        let (fo, fty) = (self.prog.structs[sid].offs[k], self.prog.structs[sid].fields[k].1.clone());
        let n = self.prog.plain_size(&fty).0;
        let r = (ro + fo, ro + fo + n.max(1));
        if !self.rq_touched.contains(&r) { self.rq_touched.push(r); }
    }
    /// A plain (record-style) value at offset `off`.
    pub(super) fn plain(&mut self, off: usize, t: &Ty, dead: Option<&str>) -> R<CV> {
        if let (Ty::Scalar(_), Some(target)) = (t, self.prog.id_at(crate::layout::IdAt::Plain(off))) { let v = format!("LD_I64({}, 0)", off); return Ok(self.id_value(target, v)); }
        Ok(match t {
            Ty::Struct(s) => CV::Plain(*s, off),
            Ty::Scalar(s) => self.load(&off.to_string(), "0", *s)?,
            Ty::Str(_) => CV::PlainStr(off),
            Ty::Arr(e, n) => CV::PlainArr(off, (**e).clone(), *n),
            _ => return no("a plain value of this kind"),
        })
    }
    /// A scalar of item `idx` in the column (or plain value) at `off`, through the profile's load macros.
    pub(super) fn load(&mut self, off: &str, idx: &str, s: Scalar) -> R<CV> {
        use Scalar::*;
        let ld = |m: &str| format!("LD_{}({}, {})", m, off, idx);
        Ok(match s {
            I64 => CV::Int(ld("I64")), U64 => CV::Int(format!("(i64){}", ld("U64"))),
            I32 => CV::Int(format!("(i64){}", ld("I32"))), U32 => CV::Int(format!("(i64){}", ld("U32"))),
            I16 => CV::Int(format!("(i64){}", ld("I16"))), U16 => CV::Int(format!("(i64){}", ld("U16"))),
            I8 => CV::Int(format!("(i64){}", ld("I8"))), U8 => CV::Int(format!("(i64){}", ld("U8"))),
            Bool => CV::Bool(format!("({} != 0)", ld("U8"))),
            F32 => CV::Flt(format!("(double){}", ld("F32"))), F64 => CV::Flt(ld("F64")),
        })
    }

    pub(super) fn field(&mut self, base: CV, f: &str, dead: Option<&str>) -> R<CV> {
        if let CV::Resp(st, body) = &base {
            return match f { "status" => Ok(CV::Int(st.clone())), "body" => Ok(CV::Txt(body.clone())), _ => no(format!("an answer has `.status` and `.body`, not `.{}`", f)) };
        }
        let (c, idx, path) = match base {
            CV::Item(c, idx) => (c, idx, f.to_string()),
            CV::Part(c, idx, p) => (c, idx, format!("{}.{}", p, f)),
            CV::Ref(t, s, k) => {
                // a value that may be null is read only inside `or(...)` / `exists(...)`, or after `if exists(x)` made it a plain item
                let Some(l) = dead else { return no(format!("`.{}` of a value that may be null: check it first with `if exists(x) {{ ... }}` (inside, x is the item), or give a fallback with `or(...)`", f)) };
                self.line(&format!("if ({} == 0) goto {};", s, l));
                (t, k, f.to_string())
            }
            CV::Plain(sid, off) => {
                let sd = &self.prog.structs[sid];
                let Some(k) = sd.fields.iter().position(|x| x.0 == f) else { return no(format!("no field `{}`{}", f, crate::lang::suggest(f, sd.fields.iter().map(|x| x.0.as_str())))) };
                self.touch_request(off, sid, k);
                let sd = &self.prog.structs[sid];
                let ty = sd.fields[k].1.clone();
                let range = sd.ranges[k];
                if let Ty::Table(..) = ty { let t = self.prog.plain_tables.iter().find(|x| x.0 == off && x.1 == sid && x.2 == f).map(|x| x.3).ok_or("plain table")?; return Ok(CV::Coll(TBASE + t, "0".into())); }
                if ty.is_coll() { return Ok(CV::Coll(self.prog.plain_coll(off, sid, f).ok_or("plain collection")?, "0".into())); }
                let v = self.plain(off + sd.offs[k], &ty, dead)?;
                let range = range.or_else(|| FIELD_RANGES.with(|fr| fr.borrow().0.get(&(off + sd.offs[k])).copied()));
                return Ok(self.with_field_range(v, range));
            }
            CV::Elem(c, idx, p, k) => {
                let path = format!("{}.{}", p, f);
                let Some(ci) = self.prog.colls[c].cols.iter().position(|x| x.path == path && matches!(x.kind, CKind::Arr(..))) else { return no(format!("no field `{}` in the elements of `{}`", f, p)) };
                let (off, CKind::Arr(sc, n)) = (self.prog.colls[c].cols[ci].off, self.prog.colls[c].cols[ci].kind.clone()) else { unreachable!() };
                return self.load(&off.to_string(), &format!("(i64)({}) * {} + {}", idx, n, k), sc);
            }
            CV::Null => { let Some(l) = dead else { return no(format!("`.{}` of `null`", f)) }; self.line(&format!("goto {};", l)); return Ok(CV::Null); }
            CV::Any(x) => {
                // an item whose collection is known only at run time: the interpreter's own field lookup
                if let Some(l) = dead { self.line(&format!("if ({}.tag == 0) goto {};", x, l)); }
                if dead.is_none() { return no(format!("`.{}` of a value whose kind is only known when the program runs, which may be null: check it first with `if exists(x) {{ ... }}` (inside, x is the item), or give a fallback with `or(...)`", f)); }
                let r = self.tmp("v");
                self.line(&format!("Val {};", r));
                self.call(&format!("c->h->field_val(c, &{}, {}, {}, &{})", x, c_lit(f.as_bytes()), f.len(), r));
                return Ok(CV::Any(r));
            }
            _ => { self.fail(500, &format!("`.{}` needs an item or a state struct", f)); return Ok(CV::Null); }
        };
        if let Some(t) = tab(c) { return self.table_field(t, idx, &path); }
        let coll = &self.prog.colls[c];
        if let Some(k) = coll.children.iter().find(|x| x.0 == path) { return Ok(CV::Coll(k.1, idx)); }
        if let Some(ci) = coll.cols.iter().position(|x| x.path == path && !matches!(x.kind, CKind::StrBytes(_))) { return self.col_value(c, idx, ci, dead); }
        let inner = format!("{}.", path);
        if coll.cols.iter().any(|x| x.path.starts_with(&inner)) || coll.children.iter().any(|x| x.0.starts_with(&inner)) { return Ok(CV::Part(c, idx, path)); }
        let names: Vec<&str> = coll.cols.iter().map(|x| x.path.as_str()).chain(coll.children.iter().map(|x| x.0.as_str())).collect();
        no(format!("no field `{}`{}", path, crate::lang::suggest(&path, names.into_iter())))
    }

    /// The value of column `ci` of item `idx`: scalars are loaded, strings are read where used, references followed.
    pub(super) fn col_value(&mut self, c: usize, idx: String, ci: usize, dead: Option<&str>) -> R<CV> {
        let col = self.prog.colls[c].cols[ci].clone();
        if let Some(target) = self.prog.id_at(crate::layout::IdAt::Col(c, ci)) { let v = format!("LD_I64({}, {})", col.off, idx); return Ok(self.id_value(target, v)); }
        Ok(match col.kind {
            CKind::Scalar(s) => { let v = self.load(&col.off.to_string(), &idx, s)?; self.with_field_range(v, col.range) }
            CKind::StrLen => CV::StateStr(c, idx, ci),
            CKind::Arr(..) => CV::ArrCol(c, idx, ci),
            CKind::StrBytes(_) => unreachable!(),
        })
    }

    /// What an item of collection `c` is as a value: the item, or for a collection of plain values, its value.
    /// A field of a table's row (`idx` its row id): a column, read through the row cache (`tget`); a plain struct inside the row
    /// (its fields are columns `path.x`); or a nested table, the rows this row owns.
    pub(super) fn table_field(&mut self, t: usize, idx: String, path: &str) -> R<CV> {
        use crate::layout::TKind;
        let tb = &self.prog.tables[t];
        // `r.id`: the row's id (its table's own column; fields may not be named `id`)
        if path == "id" { return Ok(CV::Int(format!("(i64)({})", idx))); }
        if let Some(k) = tb.children.iter().find(|x| x.0 == path) { return Ok(CV::Coll(TBASE + k.1, idx)); }
        if let Some(ci) = tb.cols.iter().position(|x| x.path == path) {
            let col = tb.cols[ci].clone();
            if let TKind::Blob(..) = col.kind { return no(format!("`{}` is a fixed array in a table's row: reading it is not compiled yet", path)); }
            let v = self.tmp("v");
            self.line(&format!("Val {};", v));
            self.call(&format!("c->h->tget(c, {}, {}, {}, &{})", t, idx, ci, v));
            return Ok(match col.kind {
                TKind::Int(Scalar::Bool) => CV::Bool(format!("({}.i != 0)", v)),
                TKind::Int(_) => { let x = CV::Int(format!("{}.i", v)); self.with_field_range(x, col.range) }
                TKind::Real(_) => CV::Flt(format!("{}.f", v)),
                // a time: the runtime reads the column's text as microseconds since 1970
                // a nullable time (`time?`): the value, or null
                TKind::Time if col.null && self.narrowed.iter().any(|n| n.0 == TBASE + t && n.1 == idx && n.2 == path) => CV::Int(format!("{}.i", v)),
                TKind::Time if col.null => CV::Any(v),
                TKind::Time => CV::Int(format!("{}.i", v)),
                // a nullable column (`utf8<N>?`): null when the column is NULL
                TKind::Text(n, _) if col.null => { let tt = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, {}.tag == 4 }};", tt, v, v, v)); self.txt_bounds.insert(tt.clone(), n as usize); CV::Txt(tt) }
                TKind::Text(n, _) => { let tt = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, 1 }};", tt, v, v)); self.txt_bounds.insert(tt.clone(), n as usize); CV::Txt(tt) }
                // a reference: null, or a row (SQLite makes it null when its row goes, so it names a live row)
                TKind::Ref(target) if self.narrowed.iter().any(|n| n.0 == TBASE + t && n.1 == idx && n.2 == path) => { let k = self.tmp("i"); self.line(&format!("u32 {} = (u32){}.i;", k, v)); CV::Item(TBASE + target, k) }
                TKind::Ref(target) => { let (s, k) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}.tag != 0, {} = (u32){}.i;", s, v, k, v)); CV::Ref(TBASE + target, s, k) }
                TKind::Blob(..) => unreachable!(),
            });
        }
        let prefix = format!("{}.", path);
        if tb.cols.iter().any(|x| x.path.starts_with(&prefix)) || tb.children.iter().any(|x| x.0.starts_with(&prefix)) { return Ok(CV::Part(TBASE + t, idx, path.to_string())); }
        no(format!("no field `{}` in a row of {}", path, tb.name))
    }
    /// Writing a field of a table's row: the value checked as a field of working state is (an integer's type and range, a text's most
    /// bytes), then one UPDATE.
    pub(super) fn table_set(&mut self, t: usize, idx: &str, path: &str, rhs: &'p Expr, env: &mut Vec<Option<CV>>) -> R<()> {
        let (ci, vv) = self.table_value(t, path, rhs, env)?;
        self.call(&format!("c->h->tset(c, {}, {}, {}, &{})", t, idx, ci, vv));
        Ok(())
    }
    /// The value for a field of a table's row, checked as a field of working state is (an integer's type and range, a text's most bytes,
    /// a reference's table): the column and the C name of the value (a Val).
    pub(super) fn table_value(&mut self, t: usize, path: &str, rhs: &'p Expr, env: &mut Vec<Option<CV>>) -> R<(usize, String)> {
        use crate::layout::TKind;
        let cols = self.prog.tables[t].cols.clone();
        let Some(ci) = cols.iter().position(|x| x.path == path) else {
            let prefix = format!("{}.", path);
            let known = cols.iter().any(|x| x.path.starts_with(&prefix)) || self.prog.tables[t].children.iter().any(|x| x.0 == path);
            return no(if known { format!("assigning `.{}` as a whole is not supported; assign its fields", path) } else { format!("no field `{}` in a row of {}", path, self.prog.tables[t].name) });
        };
        let col = cols[ci].clone();
        let v = self.expr(rhs, env, None)?;
        let v = match &col.kind {
            TKind::Int(Scalar::Bool) => { let b = self.cond(v)?; CV::Bool(b) }
            TKind::Int(sc) => {
                let v = match col.range { Some(r) => self.in_field_range(v, r, path)?, None => v };
                let x = self.int_or(v, 400, "an integer field takes an integer");
                let x = self.hold_int(x);
                let (lo, hi): (i64, i64) = match sc { Scalar::I8 => (-128, 127), Scalar::I16 => (-32768, 32767), Scalar::I32 => (i32::MIN as i64, i32::MAX as i64), Scalar::U8 => (0, 255), Scalar::U16 => (0, 65535), Scalar::U32 => (0, u32::MAX as i64), Scalar::U64 => (0, i64::MAX), _ => (i64::MIN, i64::MAX) };
                if (lo, hi) != (i64::MIN, i64::MAX) && !matches!(self.interval(&x), Some((a, b)) if a >= lo && b <= hi) {
                    let msg = format!("`{}` does not fit {:?}", path, sc).to_lowercase();
                    self.line(&format!("if (UNLIKELY({x} < {} || {x} > {})) FAIL(400, {}, {});", c_i64(lo), c_i64(hi), c_lit(msg.as_bytes()), msg.len()));
                }
                CV::Int(x)
            }
            TKind::Real(_) => match v { CV::Int(e) => CV::Flt(format!("(double)({})", e)), CV::Flt(e) => CV::Flt(e), _ => return no(format!("`{}` takes a number", path)) },
            // a time: microseconds since 1970, written as text by the runtime
            TKind::Time if col.null && matches!(v, CV::Null) => CV::Null,
            TKind::Time => { let x = self.int_or(v, 400, "a time field takes microseconds since 1970 (an integer)"); CV::Int(self.hold_int(x)) }
            TKind::Text(..) if col.null && matches!(v, CV::Null) => CV::Null,
            TKind::Text(max, _) => {
                let tt = self.txt_of(v).map_err(|_| format!("`{}` takes text", path))?;
                let msg = format!("`{}` is at most {} bytes", path, max);
                self.line(&format!("if (UNLIKELY({tt}.len > {max})) FAIL(413, {}, {});", c_lit(msg.as_bytes()), msg.len()));
                if !col.null { let m2 = format!("`{}` cannot be null", path); self.line(&format!("if (UNLIKELY(!{tt}.present)) FAIL(400, {}, {});", c_lit(m2.as_bytes()), m2.len())); }
                CV::Txt(tt)
            }
            TKind::Blob(..) => return no(format!("`{}` is a fixed array in a table's row: writing it is not compiled yet", path)),
            TKind::Ref(target) => match v {
                CV::Null => CV::Null,
                CV::Item(c, _) | CV::Ref(c, ..) if c == TBASE + *target => v,
                v => return no(format!("`{}` names a row of {}, given {}", path, self.prog.tables[*target].name, kind(&v).map_or("a value of another kind".to_string(), |k| kname(self.prog, k)))),
            },
        };
        let vv = self.val(v)?;
        Ok((ci, vv))
    }
    /// A loop over one owner's rows merged into its outer loop: the rows of owner `inst` from the merge cursor `mcur` (its query holds the
    /// `where` that is SQL; the rest is tested here), with `limit` counted here.
    pub(super) fn merged_loop(&mut self, it: &'p Iter, t: usize, inst: String, mcur: String, env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        let mut in_c = None;
        if let Some(w) = &it.wher {
            let (mut c, mut v, mut f) = (Vec::new(), Vec::new(), Vec::new());
            if !self.sql_where(w, it.slot, t, 0, &mut c, &mut v, &mut f) { in_c = Some(w); }
        }
        self.loop_enter();
        self.open("{");
        if it.last { return no("`last n` is for a loop over a table (the last n rows of its order)"); }
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        let (hits, rid, more, k) = (self.tmp("hits"), self.tmp("i"), self.tmp("more"), self.tmp("k"));
        self.line(&format!("u32 {} = 0;", hits));
        self.open(&format!("for (i64 {k} = 0LL; {k} < 9223372036854775807LL; {k}++) {{"));
        self.line(&format!("ROUND_({});", k));
        self.line(&format!("u32 {}; int {};", rid, more));
        self.call(&format!("c->h->tmerge(c, {}, (u32)({}), &{}, &{})", mcur, inst, rid, more));
        self.line(&format!("if (!{}) break;", more));
        env[it.slot as usize] = Some(CV::Item(TBASE + t, rid.clone()));
        if let Some(w) = in_c { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) continue;", wc)); }
        if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); }
        self.line(&format!("{}++;", hits));
        self.round_start(it, env, &hits);
        body(self, env, &hits)?;
        self.loop_round_end();
        self.close("}");
        self.loop_exit();
        self.close("}");
        Ok(())
    }
    /// `where` as SQL, when it is comparisons of the loop's row's fields with values the loop does not change, joined by `&&`
    /// (`t.done == false && t.rank > n`, `!t.done`): its conditions (`?n` for each value, numbered from `first`), the values, and the
    /// columns it uses. False when it is anything else.
    /// A row field's column in SQL, quoted: its path, or for a reference `<path>_id`.
    pub(super) fn csql(&self, t: usize, path: &str) -> String { crate::layout::sql_ident(&self.prog.tables[t].cols.iter().find(|c| c.path == path).map_or(path.to_string(), |c| c.sql.clone())) }
    pub(super) fn sql_where(&self, e: &'p Expr, slot: u32, t: usize, first: usize, conds: &mut Vec<String>, vals: &mut Vec<&'p Expr>, cols: &mut Vec<String>) -> bool {
        fn uses(e: &Expr, slot: u32) -> bool {
            match e {
                Expr::Var(_, VarRes::Local(s)) => *s == slot,
                Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null | Expr::Var(..) => false,
                Expr::Field(b, _) | Expr::Un(_, b) => uses(b, slot),
                Expr::Index(a, b) | Expr::Bin(_, a, b) => uses(a, slot) || uses(b, slot),
                Expr::Call(_, args, _) => args.iter().any(|a| uses(a, slot)),
            }
        }
        let col = |e: &Expr| -> Option<String> {
            let Expr::Field(b, f) = e else { return None };
            let Expr::Var(_, VarRes::Local(s)) = &**b else { return None };
            // a column, or the row's id (the primary key)
            if *s != slot || (f != "id" && !self.prog.tables[t].cols.iter().any(|c| c.path == *f && !matches!(c.kind, crate::layout::TKind::Blob(..)))) { return None; }
            Some(f.clone())
        };
        match e {
            Expr::Bin(op, a, b) if op == "&&" => self.sql_where(a, slot, t, first, conds, vals, cols) && self.sql_where(b, slot, t, first, conds, vals, cols),
            Expr::Bin(op, a, b) if ["==", "!=", "<", "<=", ">", ">="].contains(&op.as_str()) => {
                let (f, v, op) = match (col(a), col(b)) {
                    (Some(f), None) if !uses(b, slot) => (f, &**b, op.as_str()),
                    (None, Some(f)) if !uses(a, slot) => (f, &**a, match op.as_str() { "<" => ">", "<=" => ">=", ">" => "<", ">=" => "<=", o => o }),
                    _ => return false,
                };
                vals.push(v);
                conds.push(format!("{} {} ?{}", self.csql(t, &f), if op == "==" { "=" } else { op }, first + vals.len()));
                cols.push(f);
                true
            }
            Expr::Un('!', x) => match col(x) { Some(f) => { conds.push(format!("{} = 0", self.csql(t, &f))); cols.push(f); true } None => false },
            e => match col(e) { Some(f) => { conds.push(format!("{} != 0", self.csql(t, &f))); cols.push(f); true } None => false },
        }
    }
    /// A loop's order in SQL: `f` (one of the row's fields: also returned), `lower(f)` (SQLite's LOWER: ASCII letters), `f.g` across a
    /// reference (the referenced row's field, through a subquery, NULL when the reference is), `lower(f.g)`.
    pub(super) fn order_sql(&self, t: usize, by: &str) -> R<(String, Option<String>)> {
        let (lower, path) = match by.strip_prefix("lower(").and_then(|x| x.strip_suffix(')')) { Some(p) => (true, p), None => (false, by) };
        let tb = &self.prog.tables[t];
        let plain = |tbl: &crate::layout::Table, f: &str| tbl.cols.iter().any(|c| c.path == f && !matches!(c.kind, crate::layout::TKind::Blob(..)));
        let wrap = |c: String| if lower { format!("LOWER({})", c) } else { c };
        match path.split_once('.') {
            None => {
                if path != "id" && !plain(tb, path) { return no(format!("`by {}`: not a field of a row of {}", by, tb.name)); }
                Ok((wrap(self.csql(t, path)), if lower { None } else { Some(path.to_string()) }))
            }
            Some((f, g)) => {
                let Some(col) = tb.cols.iter().find(|c| c.path == f) else { return no(format!("`by {}`: `{}` is not a field of a row of {}", by, f, tb.name)) };
                let crate::layout::TKind::Ref(target) = col.kind else { return no(format!("`by {}`: `{}` is not a reference to another table's row", by, f)) };
                let tt = &self.prog.tables[target];
                if g.contains('.') || !plain(tt, g) { return no(format!("`by {}`: `{}` is not a field of a row of {}", by, g, tt.name)); }
                let inner = wrap(format!("o.{}", self.csql(target, g)));
                Ok((format!("(SELECT {} FROM {} AS o WHERE o.id = {}.{})", inner, tt.sql, tb.sql, self.csql(t, f)), None))
            }
        }
    }
    /// `foreach r in table [where ...] [reverse] [limit n]`: one query (written here, prepared once) whose cursor gives the rows' ids
    /// and columns; a field of the loop's row is read from the cursor. A `where` that is comparisons with values goes into the query;
    /// over a whole top-level table it must be, on indexed fields (a scan of every row is refused). Over one owner's rows anything goes:
    /// what is not SQL is tested here. A body that adds, removes or writes collects the ids first, so its changes cannot feed the loop.
    pub(super) fn table_loop(&mut self, it: &'p Iter, t: usize, inst: String, env: &mut Vec<Option<CV>>, collect: bool, loop_body: Option<LoopBody<'p>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        // an inner loop merged into its outer loop's query: its rows come from the outer loop's merge cursor, owner by owner
        if let Some(k) = self.merged.iter().position(|m| std::ptr::eq(m.0, it)) {
            let mcur = self.merged.remove(k).1;
            return self.merged_loop(it, t, inst, mcur, env, body);
        }
        let tb = self.prog.tables[t].clone();
        let nested = tb.parent.is_some();
        let mut sql = format!("SELECT id{} FROM {}", tb.cols.iter().map(|c| format!(", {}", crate::layout::sql_ident(&c.sql))).collect::<String>(), tb.sql);
        let mut conds: Vec<String> = Vec::new();
        if nested { conds.push("parent_id = ?1".into()); }
        let (mut wconds, mut wvals, mut wcols) = (Vec::new(), Vec::new(), Vec::new());
        let mut in_c = None; // the `where` tested here, when it is not SQL
        if let Some(w) = &it.wher {
            if self.sql_where(w, it.slot, t, conds.len(), &mut wconds, &mut wvals, &mut wcols) {
                // over a whole table, an index must lead the search: one whose first column the `where` constrains (SQLite then uses it,
                // and the other conditions narrow what it finds)
                // (the id, the primary key, is one)
                let led = wcols.iter().any(|f| f == "id" || tb.index.iter().chain(tb.unique.iter()).any(|ix| ix.first() == Some(f)));
                if !nested && !it.scan && !led {
                    for f in wcols.iter().take(1) {
                        if !tb.index.iter().chain(tb.unique.iter()).any(|ix| ix.first() == Some(f)) {
                            return no(format!("`where` over the whole table {} on `{}`, which has no index: every row would be read; declare one (`{}: table<{}> index({})`), or accept the scan (`scan where ...`) when the schema cannot have one", tb.name, f, tb.name.rsplit('.').next().unwrap_or(""), self.prog.structs[tb.item].name, f));
                        }
                    }
                }
                conds.extend(wconds);
            } else if !nested {
                return no(format!("`where` over the whole table {} is compiled to SQL only when it compares the row's indexed fields with values (joined by `&&`); this one would read every row", tb.name));
            } else { in_c = Some(w); }
        }
        // `after x`: the rows after row x in the loop's order (by id, or by the `by` field then id; backwards with `reverse`); null: all
        let mut after_val = None;
        if let Some(a) = &it.after {
            let n = nested as usize + wvals.len() + 1; // after the owner's and the `where`'s values
            let op = if it.reverse { "<" } else { ">" };
            conds.push(match &it.by {
                Some(f) if f.contains('(') || f.contains('.') => return no(format!("`after` with `by {}`: only an order by one of the row's own fields is compiled", f)),
                Some(f) => format!("(?{n} IS NULL OR ({}, id) {op} ((SELECT {} FROM {} WHERE id = ?{n}), ?{n}))", self.csql(t, f), self.csql(t, f), tb.sql),
                None => format!("(?{n} IS NULL OR id {op} ?{n})"),
            });
            after_val = Some(a);
        }
        if !conds.is_empty() { sql += &format!(" WHERE {}", conds.join(" AND ")); }
        // a constant `limit` with every condition in the query: SQLite stops there too
        let sql_limit = match (&it.limit, in_c.is_none() && !collect) { (Some(Expr::Int(n)), true) if *n >= 0 => Some(*n), _ => None };
        // `last n`: the last n rows in the loop's order, walked in that order (one query: the order turned round, n taken, turned back)
        if it.last && (sql_limit.is_none() || in_c.is_some()) { return no("`last n` takes a constant count, with the whole `where` compiled to SQL (comparisons of the row's fields with values)"); }
        // order: insertion (row ids), or `by` a field: one an index starts with, or any field when an index leads the `where` (SQLite
        // sorts the rows it found, as Rails' `where(room: r).order(:created_at)` does); ties in insertion order
        let led = !nested && wcols.iter().any(|f| f == "id" || tb.index.iter().chain(tb.unique.iter()).any(|ix| ix.first() == Some(f)));
        let rev = it.reverse != it.last;
        let order = match &it.by {
            Some(f) => {
                let (c, own) = self.order_sql(t, f)?;
                match &own {
                    // a field of the row: walked by an index that starts with it, or sorted when an index leads the `where`
                    Some(f) => if !tb.index.iter().chain(tb.unique.iter()).any(|ix| ix.first() == Some(f)) && !led && !nested && !it.scan {
                        return no(format!("`by {}` walks an index, and {} has none on `{}`: declare one (`{}: table<{}> index({})`), narrow the loop with a `where` an index leads, or accept sorting every row (`scan`)", f, tb.name, f, tb.name.rsplit('.').next().unwrap_or(""), self.prog.structs[tb.item].name, f));
                    },
                    // an expression (`lower(f)`, a field across a reference): SQLite sorts the rows the `where` found
                    None => {
                        if !led && !nested && !it.scan { return no(format!("`by {}` sorts the rows found: narrow the loop with a `where` an index leads, or accept sorting every row (`scan`)", f)); }
                        if it.last { return no(format!("`last n` with `by {}`: only an order by one of the row's own fields is compiled", f)); }
                    }
                }
                (format!("{c}{d}, id{d}", d = if rev { " DESC" } else { "" }), format!("{c}{d}, id{d}", d = if it.reverse { " DESC" } else { "" }))
            }
            None => ((if rev { "id DESC" } else { "id" }).to_string(), (if it.reverse { "id DESC" } else { "id" }).to_string()),
        };
        sql += &format!(" ORDER BY {}", order.0);
        if let Some(n) = sql_limit { sql += &format!(" LIMIT {}", n); }
        if it.last { sql = format!("SELECT * FROM ({}) ORDER BY {}", sql, order.1); }
        self.loop_enter();
        self.open("{");
        // the query's values: the owner, then the `where`'s
        let mut names = Vec::new();
        if nested { names.push(format!("{{ 1, 0, 0, (i64)({}), 0 }}", inst)); }
        for e in wvals { let v = self.expr(e, env, None)?; let v = self.hold(v, false)?; names.push(self.val(v)?); }
        if let Some(a) = after_val {
            // a row of this table, a row that may be null, null, or a row id
            let v = self.expr(a, env, None)?;
            let ok = match &v { CV::Item(c, _) | CV::Ref(c, ..) => *c == TBASE + t, CV::Null | CV::Int(_) => true, _ => false };
            if !ok { return no(format!("`after` takes a row of {} (or its id, or null)", tb.name)); }
            let v = self.hold(v, false)?;
            names.push(self.val(v)?);
        }
        let (tp, cur, hits) = (self.tmp("argv"), self.tmp("cur"), self.tmp("hits"));
        if names.is_empty() { self.line(&format!("Val {}[1];", tp)); } else { self.line(&format!("Val {}[{}] = {{ {} }};", tp, names.len(), names.join(", "))); }
        self.line(&format!("u32 {};", cur));
        self.call(&format!("c->h->titer(c, {}, {}, {}, {}, &{})", c_lit(sql.as_bytes()), t, tp, names.len(), cur));
        // one loop over each row's children in the body (`foreach t in b.todos` inside `foreach b in ...`): one query for every row's
        // children, walked in step with this loop (a merge), instead of one query per row
        let mut mcur = None;
        if let (Some(lb), false, None, false) = (loop_body, collect, &it.by, it.reverse) {
            let mut kids = Vec::new();
            child_loops(lb, it.slot, &mut kids);
            let writes_any = match lb { LoopBody::Stmts(ss) => writes(self.api, ss, &mut Vec::new()) || allocates(self.api, ss, &mut Vec::new()), LoopBody::Json(_) => false };
            if let ([inner], false) = (kids.as_slice(), writes_any) {
                let Expr::Field(_, f) = &inner.coll else { unreachable!() };
                if let Some(&(_, ct)) = tb.children.iter().find(|x| x.0 == *f) {
                    let ctb = self.prog.tables[ct].clone();
                    let assigned = match lb { LoopBody::Stmts(ss) => self.assigned_here(ss), LoopBody::Json(_) => Default::default() };
                    let (mut iconds, mut ivals, mut icols) = (Vec::new(), Vec::new(), Vec::new());
                    let inner_ok = match &inner.wher { None => true, Some(w) => self.sql_where(w, inner.slot, ct, names.len(), &mut iconds, &mut ivals, &mut icols) && ivals.iter().all(|e| bound_before(e, env, &assigned)) };
                    if inner_ok {
                        let sub_limit = sql_limit.map_or(String::new(), |n| format!(" ORDER BY id LIMIT {}", n));
                        let outer_where = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
                        let mut msql = format!("SELECT parent_id, id{} FROM {} WHERE parent_id IN (SELECT id FROM {}{}{})", ctb.cols.iter().map(|c| format!(", {}", crate::layout::sql_ident(&c.sql))).collect::<String>(), ctb.sql, tb.sql, outer_where, sub_limit);
                        for c in &iconds { msql += &format!(" AND {}", c); }
                        msql += " ORDER BY parent_id, id";
                        let mut mnames = names.clone();
                        for e in ivals { let v = self.expr(e, env, None)?; let v = self.hold(v, false)?; mnames.push(self.val(v)?); }
                        let (ma, mc) = (self.tmp("argv"), self.tmp("mcur"));
                        if mnames.is_empty() { self.line(&format!("Val {}[1];", ma)); } else { self.line(&format!("Val {}[{}] = {{ {} }};", ma, mnames.len(), mnames.join(", "))); }
                        self.line(&format!("u32 {};", mc));
                        self.call(&format!("c->h->titer(c, {}, {}, {}, {}, &{})", c_lit(msql.as_bytes()), ct, ma, mnames.len(), mc));
                        self.merged.push((*inner as *const Iter, mc.clone()));
                        mcur = Some(mc);
                    }
                }
            }
        }
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        self.line(&format!("u32 {} = 0;", hits));
        let h = self.tmp("h");
        if collect { self.line(&format!("u32 {};", h)); self.call(&format!("c->h->sel_mark(c, &{})", h)); }
        let (rid, more, k) = (self.tmp("i"), self.tmp("more"), self.tmp("k"));
        // rows as long as the cursor gives them, charged to the request's step budget as any loop without a fixed extent
        self.open(&format!("for (i64 {k} = 0LL; {k} < 9223372036854775807LL; {k}++) {{"));
        self.line(&format!("ROUND_({});", k));
        self.line(&format!("u32 {}; int {};", rid, more));
        self.call(&format!("c->h->tnext(c, {}, &{}, &{})", cur, rid, more));
        self.line(&format!("if (!{}) break;", more));
        env[it.slot as usize] = Some(CV::Item(TBASE + t, rid.clone()));
        if let Some(w) = in_c { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) continue;", wc)); }
        if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); }
        self.line(&format!("{}++;", hits));
        if !collect {
            self.round_start(it, env, &hits);
            body(self, env, &hits)?;
            self.loop_round_end();
            self.close("}");
            self.loop_exit();
            self.call(&format!("c->h->tclose(c, {})", cur));
            if let Some(mc) = &mcur { self.call(&format!("c->h->tclose(c, {})", mc)); self.merged.retain(|m| m.1 != *mc); }
            self.close("}");
            return Ok(());
        }
        self.call(&format!("c->h->sel_add(c, {})", rid));
        self.close("}");
        self.call(&format!("c->h->tclose(c, {})", cur));
        let (n, q, m2) = (self.tmp("n"), self.tmp("q"), self.tmp("i"));
        self.line(&format!("u32 {};", n));
        self.call(&format!("c->h->sel_limit(c, {}, -1, &{})", h, n));
        self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", q, q, n, q));
        self.line(&format!("u32 {};", m2));
        self.call(&format!("c->h->sel_get(c, {}, {}, &{})", h, q, m2));
        self.line(&format!("{} = {} + 1;", hits, q));
        env[it.slot as usize] = Some(CV::Item(TBASE + t, m2));
        self.round_start(it, env, &hits);
        body(self, env, &hits)?;
        self.loop_round_end();
        self.close("}");
        self.loop_exit();
        self.call(&format!("c->h->sel_end(c, {})", h));
        self.close("}");
        Ok(())
    }
    pub(super) fn item_value(&mut self, c: usize, idx: String, dead: Option<&str>) -> R<CV> {
        let coll = &self.prog.colls[c];
        if let Some(k) = coll.children.iter().find(|x| x.0.is_empty()) { return Ok(CV::Coll(k.1, idx)); }
        match coll.cols.iter().position(|x| x.path.is_empty()) { Some(ci) => self.col_value(c, idx, ci, dead), None => Ok(CV::Item(c, idx)) }
    }


}
