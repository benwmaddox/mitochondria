//! Binary operators, calls and the built-ins (builtin) of G.
use super::*;

impl<'p> G<'p> {
/// Follow a reference stored at `off` + item `idx` * 8 (slot+1, then generation) to pool `t`.
    pub(super) fn bin(&mut self, op: &str, a: &'p Expr, b: &'p Expr, env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        if op == "&&" || op == "||" {
            let l = self.expr(a, env, dead)?;
            let lc = self.cond(l)?;
            let t = self.tmp("v");
            self.line(&format!("int {} = {};", t, lc));
            self.open(&format!("if ({}{}) {{", if op == "&&" { "" } else { "!" }, t));
            let r = self.expr(b, env, dead)?;
            let rc = self.cond(r)?;
            self.line(&format!("{} = {};", t, rc));
            self.close("}");
            return Ok(CV::Bool(t));
        }
        let (l, r) = (self.expr(a, env, dead)?, self.expr(b, env, dead)?);
        if op == "==" || op == "!=" {
            let neg = if op == "!=" { "!" } else { "" };
            let inline = match (&l, &r) {
                (CV::Int(x), CV::Int(y)) | (CV::Bool(x), CV::Bool(y)) => Some(format!("({} == {})", x, y)),
                (CV::Flt(x), CV::Flt(y)) => Some(format!("({} == {})", x, y)),
                (CV::Flt(x), CV::Int(y)) | (CV::Int(y), CV::Flt(x)) => Some(format!("({} == (double)({}))", x, y)),
                (CV::Item(c1, x), CV::Item(c2, y)) => Some(if c1 == c2 { format!("({} == {})", x, y) } else { "0".into() }),
                (CV::Ref(c1, s, k), CV::Item(c2, y)) | (CV::Item(c2, y), CV::Ref(c1, s, k)) => Some(if c1 == c2 { format!("({} != 0 && {} == {})", s, k, y) } else { "0".into() }),
                (CV::Null, CV::Null) => Some("1".into()),
                (CV::Null, CV::Txt(t)) | (CV::Txt(t), CV::Null) => Some(format!("(!{}.present)", t)),
                (CV::Null, CV::Any(v)) | (CV::Any(v), CV::Null) => Some(format!("({}.tag == 0)", v)),
                (CV::Null, CV::Ref(_, s, _)) | (CV::Ref(_, s, _), CV::Null) => Some(format!("({} == 0)", s)),
                (CV::Null, CV::Int(_) | CV::Flt(_) | CV::Bool(_) | CV::Lit(_) | CV::Item(..) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..)) | (CV::Int(_) | CV::Flt(_) | CV::Bool(_) | CV::Lit(_) | CV::Item(..) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..), CV::Null) => Some("0".into()),
                _ => None,
            };
            if let Some(e) = inline { return Ok(CV::Bool(format!("{}{}", neg, e))); }
        }
        if let (CV::Int(x), CV::Int(y)) = (&l, &r) {
            let (x, y) = (self.hold_int(x.clone()), self.hold_int(y.clone()));
            let r = self.tmp("v");
            let mac = match op { "+" => "ADD", "-" => "SUB", "*" => "MUL", "/" => "DIV", "%" => "MOD", _ => "" };
            match op {
                "<" | "<=" | ">" | ">=" => return Ok(CV::Bool(format!("({} {} {})", x, op, y))),
                "&" | "|" | "^" | "<<" | ">>" | ">>>" => {
                    let (ix, iy) = (self.interval(&x), self.interval(&y));
                    let nonneg = |i: Option<(i64, i64)>| i.filter(|(l, _)| *l >= 0);
                    match op {
                        "&" => {
                            // with a non-negative operand the result is between 0 and that operand's largest value, whatever the other is
                            let hi = [nonneg(ix), nonneg(iy)].iter().flatten().map(|(_, h)| *h).min();
                            self.line(&format!("BAND{}({}, {}, {});", width_of(hi), r, x, y));
                            if let Some(h) = hi { self.ranges.insert(r.clone(), (0, h)); }
                        }
                        "|" | "^" => {
                            let hi = match (nonneg(ix), nonneg(iy)) { (Some((_, xh)), Some((_, yh))) => Some(xh.max(yh)), _ => None };
                            self.line(&format!("{}{}({}, {}, {});", if op == "|" { "BOR" } else { "BXOR" }, width_of(hi), r, x, y));
                            if let (Some((_, xh)), Some((_, yh))) = (nonneg(ix), nonneg(iy)) {
                                let m = xh.max(yh) as u64;
                                let top = if m == 0 { 0 } else { u64::MAX >> m.leading_zeros() };
                                if top <= i64::MAX as u64 { self.ranges.insert(r.clone(), (0, top as i64)); }
                            }
                        }
                        _ => {
                            // a count proven inside 0..63 needs no test
                            let counted = iy.map_or(false, |(l, h)| l >= 0 && h <= 63);
                            let left = op == "<<";
                            let mac = match op { "<<" => "SHL", ">>>" => "SHRL", _ => "SHR" };
                            self.line(&format!("{}{}({}, {}, {});", if counted { "N" } else { "" }, mac, r, x, y));
                            if let (Some((xl, xh)), Some((sl, sh))) = (ix, iy) {
                                if counted && op == ">>>" && xl < 0 {
                                    // a negative operand becomes a large positive one unless the count is 0
                                    if sl >= 1 { self.ranges.insert(r.clone(), (0, i64::MAX >> (sl - 1))); }
                                } else if counted {
                                    let mut lo = i128::MAX; let mut hi = i128::MIN;
                                    for &xv in &[xl as i128, xh as i128] { for &sv in &[sl, sh] {
                                        let v = if left { xv * (1i128 << sv) } else { xv >> sv };
                                        lo = lo.min(v); hi = hi.max(v);
                                    } }
                                    if lo >= i64::MIN as i128 && hi <= i64::MAX as i128 { self.ranges.insert(r.clone(), (lo as i64, hi as i64)); }
                                }
                            }
                        }
                    }
                    return Ok(CV::Int(r));
                }
                "+" | "-" | "*" | "/" | "%" => {
                    // with intervals for both operands, arithmetic that provably cannot overflow (or divide by zero) has no check; the
                    // result's interval is recorded for what is computed from it
                    let (ix, iy) = (self.interval(&x), self.interval(&y));
                    let mut proven: Option<(i128, i128)> = None;
                    if let (Some((xl, xh)), Some((yl, yh))) = (ix, iy) {
                        let (xl, xh, yl, yh) = (xl as i128, xh as i128, yl as i128, yh as i128);
                        proven = match op {
                            "+" => Some((xl + yl, xh + yh)),
                            "-" => Some((xl - yh, xh - yl)),
                            "*" => { let c = [xl * yl, xl * yh, xh * yl, xh * yh]; Some((*c.iter().min().unwrap(), *c.iter().max().unwrap())) }
                            "%" if yl > 0 => Some((if xl >= 0 { 0 } else { -(yh - 1) }, yh - 1)),
                            // a positive divisor: the quotient (truncated toward zero, as the C division is) is monotone in each operand, so the corners bound it
                            "/" if yl > 0 => { let c = [xl / yl, xl / yh, xh / yl, xh / yh]; Some((*c.iter().min().unwrap(), *c.iter().max().unwrap())) }
                            _ => None,
                        };
                    } else if let (None, Some((yl, yh))) = (ix, iy) {
                        // a positive divisor cannot be zero or -1 whatever the dividend is
                        if yl > 0 && op == "%" { proven = Some((-((yh - 1) as i128), (yh - 1) as i128)); }
                    }
                    let fits = proven.filter(|(l, h)| *l >= i64::MIN as i128 && *h <= i64::MAX as i128);
                    let safe_div = matches!(op, "/" | "%") && iy.map_or(false, |(yl, _)| yl > 0);
                    if let Some((l, h)) = fits.filter(|_| matches!(op, "+" | "-" | "*" | "%")) {
                        self.line(&format!("N{}({}, {}, {});", mac, r, x, y));
                        self.ranges.insert(r.clone(), (l as i64, h as i64));
                    } else if safe_div {
                        self.line(&format!("N{}({}, {}, {});", mac, r, x, y));
                        if let (Some((l, h)), "/") = (proven, op) { self.ranges.insert(r.clone(), (l as i64, h as i64)); }
                    } else {
                        self.line(&format!("{}({}, {}, {});", mac, r, x, y));
                        // a checked operation that passed: its result is inside what its operands allow, cut to the range of an i64
                        if let Some((l, h)) = proven.filter(|_| matches!(op, "+" | "-" | "*")) { self.ranges.insert(r.clone(), (l.max(i64::MIN as i128) as i64, h.min(i64::MAX as i128) as i64)); }
                    }
                    return Ok(CV::Int(r));
                }
                _ => {}
            }
        }
        let num = |v: &CV| match v { CV::Int(e) => Some(format!("(double)({})", e)), CV::Flt(e) => Some(e.clone()), _ => None };
        if let (Some(x), Some(y), true) = (num(&l), num(&r), matches!(l, CV::Flt(_)) || matches!(r, CV::Flt(_))) {
            // as the interpreter: a float on either side makes it float arithmetic (no overflow checks; IEEE results)
            match op {
                "+" | "-" | "*" | "/" => return Ok(CV::Flt(format!("({} {} {})", x, op, y))),
                "<" | "<=" | ">" | ">=" => return Ok(CV::Bool(format!("({} {} {})", x, op, y))),
                _ => {}
            }
        }
        // strings (and a value joined to one): the runtime's operator, with the kind of its result known here
        let kind = |v: &CV| match v { CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) => "string", CV::Int(_) => "i64", CV::Flt(_) => "f64", CV::Bool(_) => "bool", CV::Null => "null", CV::Any(_) => "?", _ => "an item or collection" };
        let (lk, rk) = (kind(&l), kind(&r));
        let joined = op == "+" && (lk == "string" || rk == "string") && [lk, rk].iter().all(|k| matches!(*k, "string" | "i64" | "f64" | "bool" | "null"));
        let compared = matches!(op, "<" | "<=" | ">" | ">=" | "==" | "!=") && lk == "string" && rk == "string";
        // text compared is text that lives somewhere: built on the spot it would be a temporary copy (tests may: they report)
        if compared && !self.test && [a, b].iter().any(|e| matches!(e, Expr::Bin(o, ..) if o == "+")) {
            return no("text compared is text that lives somewhere: build it in a `let` first (`let key = a + b;`), then compare that");
        }
        if !joined && !compared && lk != "?" && rk != "?" {
            // a null here comes from a copy of a function made for a null argument, in a branch that cannot run with it
            // (a walk with no `from`): it fails if reached, as it always has; any other mix does not load
            if lk == "null" || rk == "null" { self.fail(500, "operator does not fit these values"); return Ok(CV::Bool("0".into())); }
            return no(format!("operator `{}` does not fit {} and {}", op, lk, rk));
        }
        let code = OPS.iter().position(|o| *o == op).ok_or_else(|| format!("operator `{}`", op))?;
        if compared {
            // a string column against a literal or another column is compared where it is, without copying it into the text area
            let flip = [0, 1, 4, 5, 2, 3][code];
            // a window onto a plain field or a piece of a field: compared where it is, against a literal or another field
            let place = |g: &mut Self, v: &CV| -> Option<(i32, String, String)> { match v {
                CV::Lit(s) => Some((1, format!("{}ULL", intern_lit(s.as_bytes())), format!("{}", s.len()))),
                b @ (CV::PlainStr(_) | CV::StateStr(..)) => Some((0, g.sub_bytes_at(b, "0"), format!("(u32)({})", g.field_len(b)))),
                CV::Sub(b, from, n) => Some((0, g.sub_bytes_at(b, from), format!("(u32)({})", n))),
                _ => None,
            } };
            if matches!(l, CV::PlainStr(_) | CV::Sub(..)) || matches!(r, CV::PlainStr(_) | CV::Sub(..)) {
                if let (Some((ak, ao, an)), Some((bk, bo, bn))) = (place(self, &l), place(self, &r)) {
                    let t = self.tmp("r");
                    self.line(&format!("int {};", t));
                    self.call(&format!("c->h->str_cmp_at(c, {}, {}, {}, {}, {}, {}, {}, &{})", code, ak, ao, an, bk, bo, bn, t));
                    return Ok(CV::Bool(format!("({} != 0)", t)));
                }
            }
            let res = match (&l, &r) {
                (CV::StateStr(c, i, ci), CV::Lit(s)) => Some((format!("c->h->str_cmp_lit(c, {}, {}, {}, {}, {}, {}, &@)", code, c, i, ci, intern_lit(s.as_bytes()), s.len()), ())),
                (CV::Lit(s), CV::StateStr(c, i, ci)) => Some((format!("c->h->str_cmp_lit(c, {}, {}, {}, {}, {}, {}, &@)", flip, c, i, ci, intern_lit(s.as_bytes()), s.len()), ())),
                (CV::StateStr(c, i, ci), CV::StateStr(d, j, cj)) => Some((format!("c->h->str_cmp_col(c, {}, {}, {}, {}, {}, {}, {}, &@)", code, c, i, ci, d, j, cj), ())),
                _ => None,
            };
            if let Some((call, ())) = res {
                let t = self.tmp("r");
                self.line(&format!("int {};", t));
                self.call(&call.replace('@', &t));
                return Ok(CV::Bool(format!("({} != 0)", t)));
            }
        }
        let (lb, rb) = (self.txt_bound(&l), self.txt_bound(&r));
        let (lv, rv) = (self.val(l)?, self.val(r)?);
        let t = self.tmp("v");
        self.line(&format!("Val {};", t));
        self.call(&format!("c->h->binop(c, {}, &{}, &{}, &{})", code, lv, rv, t));
        if joined {
            let x = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, 1 }};", x, t, t));
            if let (Some(a), Some(b)) = (lb, rb) { self.txt_bounds.insert(x.clone(), a + b); }
            return Ok(CV::Txt(x));
        }
        if compared { return Ok(CV::Bool(format!("({}.i != 0)", t))); }
        Ok(CV::Any(t))
    }

    /// The type a function declared for a receiver is looked up by: a struct's name (an item of a pool, list or table, or a plain struct), or `i64`, `f64`, `bool`, `string`.
    pub(super) fn receiver_type(&self, v: &CV) -> Option<String> {
        let item = |c: usize| if let Some(t) = tab(c) { Some(self.prog.structs[self.prog.tables[t].item].name.clone()) } else { self.prog.colls[c].item.map(|s| self.prog.structs[s].name.clone()) };
        match v {
            CV::Int(_) => Some("i64".into()), CV::Flt(_) => Some("f64".into()), CV::Bool(_) => Some("bool".into()),
            CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) => Some("string".into()),
            CV::Item(c, _) => item(*c),
            CV::Plain(sd, _) => Some(self.prog.structs[*sd].name.clone()),
            _ => None,
        }
    }
    pub(super) fn callx(&mut self, n: &str, args: &'p [Expr], res: CallRes, env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        match res {
            CallRes::User(k) => self.inline(k, args, env, dead),
            // `x.f(args)` on a receiver whose function depends on its type: the receiver is evaluated, its type names the function (`Type.f`), and it is called with the receiver first
            CallRes::Unbound if n.starts_with('.') && !args.is_empty() => {
                // a name that is also a collection operation (`count`, `capacity`, `truncate`): the collection's own unless the receiver's type declares the function
                let own = match (&n[1..], args.len()) { ("count", 1) => Some(B::Count), ("capacity", 1) => Some(B::Capacity), ("truncate", 2) => Some(B::Truncate), _ => None };
                if let Some(b) = own { if !self.user_method_for(&n[1..], &args[0], env)? { return self.callx(n, args, CallRes::Builtin(b), env, dead); } }
                let ctx = self.ret_ctx.take();
                // the receiver is evaluated first only to learn its type; a `write` receiver is then resolved as a place (see `write_place`), anything else is held
                let v0 = self.expr(&args[0], env, dead)?;
                let Some(ty) = self.receiver_type(&v0) else { return no(format!("`{}` is called on something that is not a struct, a number, a string or a bool, and only those have functions of their own", &n[1..])) };
                let want = format!("{}.{}", ty, &n[1..]);
                let Some(k) = self.api.fns.iter().position(|f| f.recv && f.name == want).or_else(|| self.api.fns.iter().position(|f| !f.recv && f.test.is_none() && f.name == n[1..] && f.ptys.first().map_or(false, |t| t.is_some()))) else {
                    if crate::generics::note_missing(&ty, &n[1..]) { return Ok(CV::Int("0LL".into())); } // a probe of `generics::expand`: go on to find the next missing copy
                    let has: Vec<String> = self.api.fns.iter().filter(|f| f.recv).filter_map(|f| f.name.rsplit_once('.').filter(|(_, m)| *m == &n[1..]).map(|(t, _)| t.to_string())).collect();
                    if has.is_empty() { return no(format!("`{}` has no function `{}` (declare `fn {}(self: {}, ...)`)", ty, &n[1..], &n[1..], ty)); }
                    return no(format!("`{}` has no function `{}` (declare `fn {}(self: {}, ...)`; `{}` is declared for {})", ty, &n[1..], &n[1..], ty, &n[1..], has.join(", ")));
                };
                crate::generics::check_call(&want, self.at.0)?;
                let k = k as u32;
                let mut vals = Vec::new();
                let mut places: Vec<Option<WPlace<'p>>> = Vec::new();
                for (j, a) in args.iter().enumerate() {
                    if scalar_write_at(k, j) {
                        if j == 0 && !no_calls(a) { return no(format!("`{}: write` in `{}`: `write` needs a place to write to, not a value computed by a call", self.api.fns[k as usize].params[0], want)); }
                        let (v, pl) = self.write_place(a, k, j, env, dead)?; vals.push(v); places.push(Some(pl));
                    } else {
                        let v = if j == 0 { v0.clone() } else { self.expr(a, env, dead)? };
                        vals.push(self.hold(v, false)?); places.push(None);
                    }
                }
                self.ret_ctx = ctx;
                self.wargs = places;
                self.call_values(k, vals, dead)
            }
            CallRes::Unbound if n == "split" => no("`split` is what a loop walks: `foreach part in split(text, \", \") { ... }` (or `split(text)` for words)"),
            CallRes::Unbound => no("unbound call"),
            CallRes::Builtin(bi) => {
                let lit = |a: Option<&Expr>| match a { Some(Expr::Str(s)) => Some(s.clone()), _ => None };
                let int_out = |g: &mut Self, call: String| { let r = g.tmp("v"); g.line(&format!("i64 {};", r)); g.call(&call.replace("@", &format!("&{}", r))); CV::Int(r) };
                let txt_out = |g: &mut Self, call: String| { let r = g.tmp("t"); g.line(&format!("Txt {};", r)); g.call(&call.replace("@", &format!("&{}", r))); CV::Txt(r) };
                let val_out = |g: &mut Self, call: String| { let r = g.tmp("v"); g.line(&format!("Val {};", r)); g.call(&call.replace("@", &format!("&{}", r))); CV::Any(r) };
                match bi {
                    B::Or => {
                        let (l, end) = (self.tmp("dead"), self.tmp("done"));
                        let res = self.tmp("v");
                        self.line(&format!("Val {};", res));
                        self.line("{");
                        let saved = self.or_dead.replace(l.clone());
                        let a = self.expr(&args[0], env, Some(&l));
                        self.or_dead = saved;
                        let a = a?;
                        let ka = kind(&a);
                        let av = self.val(a)?;
                        self.line(&format!("if ({}.tag == 0) goto {};", av, l)); // null also takes the fallback
                        self.line(&format!("{} = {}; goto {};", res, av, end));
                        self.line("}");
                        self.line(&format!("{}: {{", l));
                        let fb = self.expr(&args[1], env, dead)?;
                        let kb = kind(&fb);
                        let fv = self.val(fb)?;
                        self.line(&format!("{} = {};", res, fv));
                        self.line("}");
                        self.line(&format!("{}: ;", end));
                        Ok(match (ka, kb) {
                            (Some(K::Item(c) | K::Ref(c)), Some(K::Item(d))) if c == d => CV::Item(c, format!("{}.b", res)),
                            (Some(K::Item(c) | K::Ref(c)), Some(K::Ref(d))) if c == d => { let (sv, kv) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}.tag != 0, {} = {}.b;", sv, res, kv, res)); CV::Ref(c, sv, kv) }
                            // text or a fallback text, a number or a fallback number: that kind (the result is never null)
                            (Some(K::Txt), Some(K::Txt)) => { let tt = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, 1 }};", tt, res, res)); CV::Txt(tt) }
                            (Some(K::Int), Some(K::Int)) => CV::Int(format!("{}.i", res)),
                            (Some(K::Flt), Some(K::Flt | K::Int)) => CV::Flt(format!("({}.tag == 2 ? {}.f : (double){}.i)", res, res, res)),
                            (Some(K::Bool), Some(K::Bool)) => CV::Bool(format!("({}.i != 0)", res)),
                            (Some(K::Item(c) | K::Ref(c)), Some(K::Null)) => { let (sv, kv) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}.tag != 0, {} = {}.b;", sv, res, kv, res)); CV::Ref(c, sv, kv) }
                            _ => CV::Any(res),
                        })
                    }
                    B::Exists => {
                        let (l, end, res) = (self.tmp("dead"), self.tmp("done"), self.tmp("v"));
                        self.line(&format!("int {} = 1;", res));
                        self.line("{");
                        let saved = self.or_dead.replace(l.clone());
                        let a = self.expr(&args[0], env, Some(&l));
                        self.or_dead = saved;
                        let av = self.val(a?)?;
                        self.line(&format!("if ({}.tag == 0) goto {};", av, l));
                        self.line(&format!("goto {};", end));
                        self.line("}");
                        self.line(&format!("{}: {} = 0;", l, res));
                        self.line(&format!("{}: ;", end));
                        Ok(CV::Bool(res))
                    }
                    B::MaxLen => {
                        let Some(Expr::Field(b, f)) = args.first() else { return no("maxlen(item.field)") };
                        match self.expr(b, env, dead)? {
                            CV::Item(c, _) | CV::Ref(c, _, _) => match self.prog.colls[c].cols.iter().position(|x| x.path == **f).and_then(|ci| self.prog.colls[c].cols.get(ci + 1)).map(|x| x.kind.clone()) {
                                Some(CKind::StrBytes(m)) => Ok(CV::Int(format!("{}LL", m))), _ => no("maxlen needs a string field") },
                            CV::Plain(sid, _) => { let sd = &self.prog.structs[sid]; match sd.fields.iter().find(|x| x.0 == **f).map(|x| &x.1) { Some(Ty::Str(m)) => Ok(CV::Int(format!("{}LL", m))), _ => no("maxlen needs a string field") } }
                            _ => no("maxlen(item.field)"),
                        }
                    }
                    _ => {
                        let mut a = Vec::new();
                        for x in args { a.push(self.expr(x, env, dead)?); }
                        self.builtin(bi, n, args, a, lit, int_out, txt_out, val_out)
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn builtin(&mut self, bi: B, n: &str, _args: &'p [Expr], a: Vec<CV>, lit: impl Fn(Option<&Expr>) -> Option<String>,
        int_out: impl Fn(&mut Self, String) -> CV, txt_out: impl Fn(&mut Self, String) -> CV, val_out: impl Fn(&mut Self, String) -> CV) -> R<CV> {
        let _ = (&lit, &txt_out, &val_out);
        if bi == B::Log {
            // `log(...)`: for debugging, to standard error; not part of the answer
            let mut names = Vec::new();
            for v in a {
                let v = match v { w @ (CV::PlainStr(_) | CV::StateStr(..) | CV::Sub(..)) => CV::Txt(self.txt_of(w)?), o => o };
                if matches!(v, CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..)) { return no("`log` takes text, numbers, bools and items"); }
                names.push(self.val(v)?);
            }
            let argv = self.tmp("argv");
            if names.is_empty() { self.line(&format!("Val {}[1];", argv)); } else { self.line(&format!("Val {}[{}] = {{ {} }};", argv, names.len(), names.join(", "))); }
            self.call(&format!("c->h->log(c, AT_, {}, {})", argv, names.len()));
            return Ok(CV::Null);
        }
        if matches!(bi, B::HtmlLit | B::HtmlEsc | B::HtmlEscAttr | B::HtmlAttr | B::HtmlRaw) {
            // markup written into the answer (see lang.rs `html`): only in an endpoint that answers HTML, and the `-> html` functions it calls
            if !self.html_answer() { return no("markup is written only into an HTML answer: an endpoint returning `Response<html>`, or a `-> html` function it calls"); }
            let mut a = a.into_iter();
            match bi {
                B::HtmlLit => { let Some(CV::Lit(s)) = a.next() else { return no("bad markup") }; self.out(&s); }
                // `html_raw(t)`: text the program trusts as markup, written unescaped (a stylesheet an administrator wrote, as Rails' html_safe)
                B::HtmlRaw => match a.next() {
                    Some(CV::Lit(s)) => self.out(&s),
                    Some(CV::Txt(t)) => self.line(&format!("if ({}.present) CALL(c->h->raw_txt(c, &{}));", t, t)),
                    Some(CV::StateStr(cc, idx, ci)) => self.call(&format!("c->h->raw_col(c, {}, {}, {})", cc, idx, ci)),
                    Some(CV::PlainStr(off)) => { let at = self.sub_bytes_at(&CV::PlainStr(off), "0"); let n = self.field_len(&CV::PlainStr(off)); self.call(&format!("c->h->raw_at(c, {}, (u32)({}))", at, n)); }
                    Some(CV::Null) => {}
                    _ => return no("`html_raw` takes text: `{html_raw(styles)}`"),
                },
                B::HtmlEsc | B::HtmlEscAttr => { let v = a.next().unwrap_or(CV::Null); self.html_esc(v, bi == B::HtmlEscAttr)?; }
                _ => {
                    let (Some(CV::Lit(name)), Some(v)) = (a.next(), a.next()) else { return no("bad markup") };
                    match v {
                        CV::Bool(b) if b == "1" => self.out(&format!(" {}", name)),
                        CV::Bool(b) if b == "0" => {}
                        CV::Bool(b) => { let o = format!(" {}", name); self.line(&format!("if ({}) CALL(c->h->out(c, {}, {}));", b, c_lit(o.as_bytes()), o.len())); }
                        v => { self.out(&format!(" {}=\"", name)); self.html_esc(v, true)?; self.out("\""); }
                    }
                }
            }
            return Ok(CV::Null);
        }
        if bi == B::SetCookie {
            // `set_cookie("name", value, max_age_seconds)`: a Set-Cookie line on the answer (Path=/, HttpOnly, SameSite=Lax); 0 deletes it
            // an optional fourth argument: whether the cookie is HttpOnly (true unless given)
            let mut a = a;
            let http_only = if a.len() == 4 { let h = a.pop().unwrap(); match h { CV::Bool(b) => b, _ => return no("`set_cookie`'s fourth argument says whether the cookie is HttpOnly: true or false") } } else { "1".to_string() };
            let [name, value, age] = <[CV; 3]>::try_from(a).map_err(|_| "`set_cookie` takes the name, the value, how long it lasts in seconds and (optionally) whether it is HttpOnly: `set_cookie(\"list\", token(b), 31536000)`".to_string())?;
            let CV::Lit(name) = name else { return no("`set_cookie`: the name is a literal, as in `set_cookie(\"list\", ...)`") };
            if !crate::lang::cookie_name_ok(&name) { return no(format!("`set_cookie`: `{}` is not a cookie name (letters, digits and !#$%&'*+-.^_`|~)", name)); }
            let t = self.txt_of(value).map_err(|_| "`set_cookie` takes the value as text".to_string())?;
            let age = self.int_or(age, 500, "`set_cookie` takes the number of seconds as an integer");
            self.call(&format!("c->h->set_cookie(c, {}, {}, &{}, {}, {})", c_lit(name.as_bytes()), name.len(), t, age, http_only));
            return Ok(CV::Null);
        }
        if bi == B::SetHeader {
            // `set_header("Link", value)`: a header line on the answer, in place of the `web` block's line of that name
            let [name, value] = <[CV; 2]>::try_from(a).map_err(|_| "`set_header` takes the name and the value: `set_header(\"Link\", links)`".to_string())?;
            let CV::Lit(name) = name else { return no("`set_header`: the name is a literal, as in `set_header(\"Link\", ...)`") };
            if let Err(e) = crate::web::header_name_ok(&name) { return no(format!("`set_header`: {}", e)); }
            let t = self.txt_of(value).map_err(|_| "`set_header` takes the value as text".to_string())?;
            self.call(&format!("c->h->set_header(c, {}, {}, &{})", c_lit(name.as_bytes()), name.len(), t));
            return Ok(CV::Null);
        }
        if bi == B::Truncate {
            // `s.truncate(n)`: keep the first n bytes (0 <= n <= length); in utf8 text, n must not split a character
            let [s, n] = <[CV; 2]>::try_from(a).map_err(|_| "`truncate` takes the number of bytes to keep: `s.truncate(n)`".to_string())?;
            if !matches!(s, CV::PlainStr(_) | CV::StateStr(..)) { return no("`truncate` is a function of a text field"); }
            match &s { CV::PlainStr(off) => self.may_write_plain(*off)?, CV::StateStr(c, _, ci) => { let p = self.prog.colls[*c].cols[*ci].path.clone(); self.may_write_col(*c, &p)? } _ => {} }
            let n = self.int_or(n, 400, "`truncate` takes an integer");
            let n = self.hold_int(n);
            let len = self.field_len(&s);
            let len = self.hold_int(len);
            self.line(&format!("if (UNLIKELY({n} < 0 || {n} > {len})) FAIL(400, \"truncate past the end\", 21);"));
            if !self.is_ascii(&s) {
                let at = self.sub_bytes_at(&s, &n);
                self.line(&format!("if (UNLIKELY({n} < {len} && (LD_U8({at}, 0) & 0xC0) == 0x80)) FAIL(400, \"truncate would split a character\", 32);"));
            }
            let at = self.len_at(&s);
            self.call(&format!("c->h->wr_int(c, {}, 4, {})", at, n));
            return Ok(CV::Null);
        }
        if bi == B::JsonField {
            // `json_field(text, "name")` (`field` in a test): the field of a JSON object (a string's characters as written, escapes kept; anything else
            // as written), or null
            let [j, k] = <[CV; 2]>::try_from(a).map_err(|_| "`field` takes a JSON text and a field name: `field(r.body, \"id\")`".to_string())?;
            let CV::Lit(key) = k else { return no("`field`'s name is a string written in the test") };
            let jb = self.txt_bound(&j);
            let j = self.txt_of(j)?;
            let r = self.tmp("t");
            self.line(&format!("Txt {} = {{ 0, 0, 0 }};", r));
            self.call(&format!("c->h->json_field(c, &{}, {}, {}, &{})", j, c_lit(key.as_bytes()), key.len(), r));
            if let Some(b) = jb { self.txt_bounds.insert(r.clone(), b); }
            return Ok(CV::Txt(r));
        }
        if let B::ReqGet | B::ReqPost | B::ReqPut | B::ReqPatch | B::ReqDelete = bi {
            // a test's request: path, the number of body fields, then (name, value) pairs for the body and the headers
            let method = match bi { B::ReqGet => 0, B::ReqPost => 1, B::ReqPut => 2, B::ReqPatch => 3, _ => 4 };
            let nb = match _args.get(1) { Some(Expr::Int(n)) => *n as usize, _ => 0 };
            let mut it = a.into_iter();
            let path = it.next().ok_or("a request needs its path")?;
            let path = self.txt_of(path).map_err(|_| "a request's path is a string, as in `post(\"/players\", { name: \"ann\" })`".to_string())?;
            let _ = it.next();
            let rest: Vec<CV> = it.collect();
            for (i, pair) in rest.chunks(2).enumerate() {
                let CV::Lit(key) = &pair[0] else { return no("a request's field names are names") };
                let vv = self.val(pair[1].clone())?;
                self.call(&format!("c->h->rq_put(c, {}, {}, {}, &{})", (i >= nb) as i32, c_lit(key.as_bytes()), key.len(), vv));
            }
            let (st, bd) = (self.tmp("st"), self.tmp("rb"));
            self.line(&format!("i64 {} = 0; Txt {} = {{ 0, 0, 0 }};", st, bd));
            self.call(&format!("c->h->rq_send(c, {}, &{}, &{}, &{})", method, path, st, bd));
            self.checked.clear(); // the request may have changed anything a test before it proved
            return Ok(CV::Resp(st, bd));
        }
        // `len` of a text field: its stored length, read where it is (no copy)
        if bi == B::Len {
            match a.first() {
                Some(CV::StateStr(c, idx, ci)) => return Ok(CV::Int(format!("(i64)LD_U32({}, {})", self.prog.colls[*c].cols[*ci].off, idx))),
                Some(CV::PlainStr(off)) => return Ok(CV::Int(format!("(i64)LD_U32({}, 0)", off))),
                Some(CV::Sub(_, _, n)) => return Ok(CV::Int(format!("(i64)({})", n))),
                Some(CV::Lit(s)) => return Ok(CV::Int(format!("{}LL", s.len()))),
                Some(CV::Txt(t)) => return Ok(CV::Int(format!("(i64){}.len", t))),
                _ => {}
            }
        }
        // `byte_at(t, i)`: the byte at i, or -1 past either end, read in line (as the runtime's builtin 58 does; no call, no copy of a field)
        if bi == B::ByteAt {
            if let [t @ (CV::Txt(_) | CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..)), CV::Int(k)] = a.as_slice() {
                let (t, k) = (t.clone(), k.clone());
                let iv = self.interval(&k);
                let k = self.hold_int(k);
                let r = match t {
                    CV::Lit(s) => {
                        let off = intern_lit(s.as_bytes());
                        match iv.or_else(|| self.interval(&k)) {
                            // the range analysis proves the index inside the literal: no test
                            Some((lo, hi)) if lo >= 0 && hi < s.len() as i64 => format!("(i64)TX_({}ULL + (u64)({}))", off, k),
                            _ => format!("((u64)({k}) < {}ULL ? (i64)TX_({}ULL + (u64)({k})) : -1LL)", s.len(), off),
                        }
                    }
                    CV::Txt(x) => {
                        // a missing text is an error, as the builtin's (the test is the same for every byte, so it leaves the loop)
                        self.line(&format!("if (UNLIKELY(!{x}.present)) FAIL(500, \"expected a string argument\", 26);"));
                        format!("((u64)({k}) < (u64){x}.len ? (i64)TX_({x}.off + (u64)({k})) : -1LL)")
                    }
                    CV::StateStr(c, idx, ci) => {
                        let lenoff = self.prog.colls[c].cols[ci].off;
                        let bytes = &self.prog.colls[c].cols[ci + 1];
                        let CKind::StrBytes(m) = bytes.kind else { return no("string column layout") };
                        let boff = bytes.off;
                        format!("((u64)({k}) < (u64)LD_U32({lenoff}, {idx}) ? (i64)LD_U8({boff}, (i64)({idx}) * {m} + {k}) : -1LL)")
                    }
                    CV::PlainStr(off) => {
                        let len = self.str_lens.get(&off).cloned().unwrap_or_else(|| format!("(i64)LD_U32({}, 0)", off));
                        format!("((u64)({k}) < (u64)({len}) ? (i64)LD_U8({}, {k}) : -1LL)", off + 4)
                    }
                    CV::Sub(base, from, count) => {
                        let at = self.sub_bytes_at(&base, &format!("{} + {}", from, k));
                        format!("((u64)({k}) < (u64)({count}) ? (i64)LD_U8({at}, 0) : -1LL)")
                    }
                    _ => unreachable!(),
                };
                let r = self.hold_int(r);
                return Ok(CV::Int(r));
            }
        }
        // `int` of a number: an integer as it is; a float truncated in line, as the runtime's builtin does (NaN is 0, beyond i64 the nearest end)
        if bi == B::Int {
            match a.first() {
                Some(CV::Int(e)) => return Ok(CV::Int(e.clone())),
                Some(CV::Flt(e)) => {
                    let (f, r) = (self.tmp("f"), self.tmp("v"));
                    self.line(&format!("double {f} = {e}; i64 {r} = {f} != {f} ? 0 : {f} >= 9223372036854775807.0 ? I64MAX : {f} <= -9223372036854775808.0 ? I64MIN : (i64){f};"));
                    return Ok(CV::Int(r));
                }
                _ => {}
            }
        }
        let generic = match bi {
            B::Min | B::Max => !matches!(a.as_slice(), [CV::Int(_) | CV::Flt(_), CV::Int(_) | CV::Flt(_)]),
            B::Slot => !matches!(a.first(), Some(CV::Item(..) | CV::Ref(..))),
            _ => BUILTINS[..14].contains(&bi) || BUILTINS[17..].contains(&bi),
        };
        // arity of the byte and crypto built-ins, checked when compiling
        let want = match bi {
            B::Base64 | B::Base64Url | B::Unbase64 | B::Hex | B::Sha1 | B::Sha256 | B::RandomBytes | B::FormEncode | B::FormDecode | B::Env | B::EscapeHtml | B::JsonQuote | B::JsonUnquote | B::Crc32 | B::ReadStored | B::Lower | B::Upper | B::CacheGet | B::CachePut | B::CachePart | B::CacheLabel | B::TrimStart | B::TrimEnd | B::AllEmoji | B::HtmlParse | B::NodeKind | B::NodeName | B::NodeParent | B::NodeFirst | B::NodeLast | B::NodeNext | B::NodePrev | B::NodeContent | B::NodeAttrCount | B::NodeNewElement | B::NodeNewText | B::NodeDetach | B::NodeClone | B::NodeHtml | B::NodeInnerHtml | B::NodeHtmlEscaped | B::Chr | B::AllWord => Some((1, "one argument")),
            B::HtmlParseIn | B::NodeAttr | B::NodeAttrName | B::NodeAttrValue | B::NodeRemoveAttr | B::NodeAppend | B::NodeInsertBefore | B::NodeSetText => Some((2, "two arguments")),
            B::NodeSetAttr => Some((3, "a node, a name and a value")),
            B::RequestTarget | B::CacheStart | B::DataVersion => Some((0, "no arguments")),
            B::SocketClose | B::SocketSendAll => Some((1, "one argument")),
            B::SocketSend | B::SocketUnsubscribe | B::Broadcast | B::SocketTag => Some((2, "two arguments")),
            B::SocketSubscribe => Some((4, "the socket, the stream, and the text before and after each message broadcast to it")),
            B::HmacSha1 | B::HmacSha256 | B::XorBytes | B::SecretEqual | B::BcryptHash | B::BcryptCheck | B::FormatTime | B::Contains | B::StartsWith | B::EndsWith | B::RandomText | B::AddYears | B::IndexOf | B::LastIndexOf | B::ByteAt | B::CacheHole => Some((2, "two arguments")),
            B::GcmSeal | B::GcmOpen => Some((3, "the key, the 12-byte nonce and the text")),
            B::Eprint | B::Print | B::FileExists | B::StdinText => Some((1, "one argument")),
            B::Flush => Some((0, "no arguments")),
            B::ReadFile => Some((2, "a path and the most bytes the file may have")),
            B::Replace => Some((3, "the text, what to replace and what with")),
            B::Pbkdf2Sha256 => Some((4, "the password, the salt, the iterations and the length")),
            _ => None,
        };
        if let Some((k, what)) = want {
            if a.len() != k {
                let name = match bi { B::Base64 => "base64", B::Base64Url => "base64_url", B::Unbase64 => "unbase64", B::Hex => "hex", B::Sha1 => "sha1", B::Sha256 => "sha256",
                    B::RandomBytes => "random_bytes", B::Env => "env", B::EscapeHtml => "escape_html", B::JsonQuote => "json_quote", B::JsonUnquote => "json_unquote", B::RequestTarget => "request_target", B::Crc32 => "crc32", B::ByteAt => "byte_at", B::Lower => "lower", B::Upper => "upper", B::CacheGet => "cache_get", B::CachePut => "cache_put", B::CacheStart => "cache_start", B::CacheHole => "cache_hole", B::CachePart => "cache_part", B::CacheLabel => "cache_label", B::DataVersion => "data_version", B::Replace => "replace", B::TrimStart => "trim_start", B::TrimEnd => "trim_end", B::AllEmoji => "all_emoji", B::HtmlParse => "html_parse", B::HtmlParseIn => "html_parse_in", B::NodeKind => "node_kind", B::NodeName => "node_name", B::NodeParent => "node_parent", B::NodeFirst => "node_first", B::NodeLast => "node_last", B::NodeNext => "node_next", B::NodePrev => "node_prev", B::NodeContent => "node_content", B::NodeAttr => "node_attr", B::NodeAttrCount => "node_attr_count", B::NodeAttrName => "node_attr_name", B::NodeAttrValue => "node_attr_value", B::NodeSetAttr => "node_set_attr", B::NodeRemoveAttr => "node_remove_attr", B::NodeNewElement => "node_new_element", B::NodeNewText => "node_new_text", B::NodeAppend => "node_append", B::NodeInsertBefore => "node_insert_before", B::NodeDetach => "node_detach", B::NodeClone => "node_clone", B::NodeHtml => "node_html", B::NodeInnerHtml => "node_inner_html", B::NodeSetText => "node_set_text", B::NodeHtmlEscaped => "node_html_escaped", B::Chr => "chr", B::AllWord => "all_word", B::ReadStored => "read_stored", B::SocketSend => "socket_send", B::SocketClose => "socket_close", B::SocketSubscribe => "socket_subscribe", B::SocketUnsubscribe => "socket_unsubscribe", B::Broadcast => "broadcast", B::SocketSendAll => "socket_send_all", B::SocketTag => "socket_tag", B::RandomText => "random_text", B::AddYears => "add_years", B::IndexOf => "index_of", B::LastIndexOf => "last_index_of", B::FormEncode => "form_encode", B::FormDecode => "form_decode", B::HmacSha1 => "hmac_sha1", B::HmacSha256 => "hmac_sha256",
                    B::XorBytes => "xor_bytes", B::SecretEqual => "secret_equal", B::BcryptHash => "bcrypt_hash", B::BcryptCheck => "bcrypt_check", B::FormatTime => "format_time", B::Contains => "contains", B::StartsWith => "starts_with", B::EndsWith => "ends_with",
                    B::Eprint => "eprint", B::Print => "print", B::Flush => "flush", B::StdinText => "stdin_text", B::ReadFile => "read_file", B::FileExists => "file_exists", B::GcmSeal => "aes256gcm_seal", B::GcmOpen => "aes256gcm_open", _ => "pbkdf2_sha256" };
                return no(format!("`{}` takes {}", name, what));
            }
        }
        // `str(n)` of a number or bool: its longest text, for the bounds of lets of built text
        let str_bound = self.builtin_bound(bi, &a);
        if let (true, Some(code)) = (generic, BUILTINS.iter().position(|x| *x == bi)) {
            // arguments as one Val array; the interpreter's builtin computes the result
            let mut names = Vec::new();
            for v in a { names.push(self.val(v)?); }
            let argv = self.tmp("argv");
            if names.is_empty() { self.line(&format!("Val {}[1];", argv)); } else { self.line(&format!("Val {}[{}] = {{ {} }};", argv, names.len(), names.join(", "))); }
            let r = self.tmp("v");
            self.line(&format!("Val {};", r));
            // a crypto built-in is marked, so the unit gets runtime/crypto.c and its library (program_c_full)
            let mark = if CRYPTO_BUILTINS.contains(&bi) { "/*crypto*/" } else if HTML_BUILTINS.contains(&bi) { "/*html*/" } else { "" };
            self.call(&format!("c->h->builtin(c, {}, {}, {}, &{}){}", code, argv, names.len(), r, mark));
            let _ = n;
            let _ = int_out;
            return Ok(match bi {
                B::Int | B::Len | B::Now | B::Random | B::Stat | B::WorkerIndex | B::WorkerCount | B::Min | B::Max | B::Slot | B::AddYears | B::IndexOf | B::LastIndexOf | B::Crc32 | B::ByteAt | B::CacheGet | B::DataVersion
                    | B::HtmlParse | B::HtmlParseIn | B::NodeKind | B::NodeParent | B::NodeFirst | B::NodeLast | B::NodeNext | B::NodePrev | B::NodeAttrCount | B::NodeNewElement | B::NodeNewText | B::NodeClone => CV::Int(format!("{}.i", r)),
                B::SecretEqual | B::FileExists | B::BcryptCheck | B::Contains | B::StartsWith | B::EndsWith | B::AllEmoji | B::AllWord => CV::Bool(format!("({}.i != 0)", r)),
                B::Str | B::Token | B::Base64 | B::Base64Url | B::Hex | B::Sha1 | B::Sha256 | B::HmacSha1 | B::HmacSha256 | B::Pbkdf2Sha256 | B::RandomBytes
                    | B::XorBytes | B::GcmSeal | B::BcryptHash | B::FormEncode | B::FormatTime | B::EscapeHtml | B::RandomText | B::JsonQuote | B::RequestTarget | B::Lower | B::Upper | B::CacheLabel | B::Replace | B::TrimStart | B::TrimEnd
                    | B::NodeName | B::NodeContent | B::NodeAttrName | B::NodeAttrValue | B::NodeHtml | B::NodeInnerHtml | B::NodeHtmlEscaped | B::Chr | B::StdinText => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, 1 }};", t, r, r)); if let Some(b) = str_bound { self.txt_bounds.insert(t.clone(), b); } CV::Txt(t) }
                // a text, or null: decoding or opening that failed
                B::Param | B::Query | B::Header | B::Unbase64 | B::GcmOpen | B::FormDecode | B::Env | B::JsonUnquote | B::ReadStored | B::ReadFile | B::NodeAttr => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, {}.tag == 4 }};", t, r, r, r)); if let Some(b) = str_bound { self.txt_bounds.insert(t.clone(), b); } CV::Txt(t) }
                B::Eprint | B::Print | B::Flush => CV::Null,
                _ => CV::Any(r),
            });
        }
        Ok(match bi {
            B::Auth => {
                let Some(CV::Coll(c, inst)) = a.first() else { return no("auth(pool)") };
                if inst != "0" { return no("auth of a nested pool"); }
                let t = self.tmp("i"); self.line(&format!("u32 {};", t)); self.call(&format!("c->h->auth(c, {}, &{})", c, t));
                CV::Item(*c, t)
            }
            B::Min | B::Max => {
                let mut it = a.into_iter();
                let (Some(x), Some(y)) = (it.next(), it.next()) else { return no("min/max take two numbers") };
                let op = if bi == B::Min { "<" } else { ">" };
                match (x, y) {
                    (CV::Int(x), CV::Int(y)) => { let (x, y) = (self.hold_int(x), self.hold_int(y)); CV::Int(format!("({} {} {} ? {} : {})", x, op, y, x, y)) }
                    // a float and an integer give a float, as the arithmetic operators do
                    (x @ (CV::Flt(_) | CV::Int(_)), y @ (CV::Flt(_) | CV::Int(_))) => {
                        let f = |v: CV| match v { CV::Flt(e) => e, CV::Int(e) => format!("(double)({})", e), _ => unreachable!() };
                        let (x, y) = (f(x), f(y));
                        let (tx, ty) = (self.tmp("f"), self.tmp("f"));
                        self.line(&format!("double {} = {}, {} = {};", tx, x, ty, y));
                        CV::Flt(format!("({} {} {} ? {} : {})", tx, op, ty, tx, ty))
                    }
                    _ => return no("min/max of something other than numbers"),
                }
            }
            B::Count => {
                let Some(CV::Coll(c, inst)) = a.first() else { return no("count(collection)") };
                if let Some(t) = tab(*c) { let r = self.tmp("n"); self.line(&format!("i64 {};", r)); self.call(&format!("c->h->tcount(c, {}, (u32)({}), &{})", t, inst, r)); return Ok(CV::Int(r)); }
                let col = &self.prog.colls[*c];
                let f = if col.is_list() { 0 } else { 1 };
                CV::Int(format!("(i64)LD_U32({}, {})", col.hdr + 4 * f * col.mult, inst))
            }
            B::CountBelow => {
                // a null argument comes from a copy of a function made for a null argument, in a branch that cannot run with it
                // (a walk with no `from`): it fails if reached, as an operator on null does
                if a.iter().any(|v| matches!(v, CV::Null)) { self.fail(500, "operator does not fit these values"); return Ok(CV::Int("0LL".into())); }
                let mut it = a.into_iter();
                let (Some(arr), Some(CV::Int(n)), Some(CV::Int(target)), Some(incl)) = (it.next(), it.next(), it.next(), it.next()) else { return no("`count_below(n, target, inclusive)`: n and target are integers, inclusive a bool") };
                let incl = match incl { CV::Bool(e) => format!("(({}) ? 1 : 0)", e), CV::Int(e) => format!("(({}) != 0)", e), _ => return no("`count_below`: inclusive is a bool") };
                let (base, code, len) = match arr {
                    CV::ArrCol(c, idx, ci) => {
                        let col = &self.prog.colls[c].cols[ci];
                        let CKind::Arr(s, len) = col.kind else { unreachable!() };
                        (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), plain_code(&Ty::Scalar(s)).map_or(0, |x| x.0), len)
                    }
                    CV::PlainArr(off, e, len) => (format!("{}ULL", off), plain_code(&e).map_or(0, |x| x.0), len),
                    _ => return no("`count_below` works on a fixed array"),
                };
                if !(1..=8).contains(&code) { return no("`count_below` works on an array of integers"); }
                let (n, target) = (self.hold_int(n), self.hold_int(target));
                self.line(&format!("if (UNLIKELY({n} < 0 || {n} > {len}LL)) FAIL(400, \"count_below out of range\", 24);"));
                let r = self.tmp("r");
                self.line(&format!("i64 {};", r));
                self.call(&format!("c->h->count_below(c, {}, {}, {}, {}, {}, &{})", base, code, n, target, incl, r));
                CV::Int(r)
            }
            B::Capacity => { let Some(CV::Coll(c, _)) = a.first() else { return no("`capacity()` is a function of a collection") }; CV::Int(format!("{}LL", self.prog.colls[*c].cap)) }
            B::Slot => match a.first() {
                Some(CV::Item(c, i)) if tab(*c).is_some() => CV::Int(format!("(i64)({})", i)),
                Some(CV::Item(c, i)) => CV::Int(format!("(i64)({} % {})", i, self.prog.colls[*c].cap)),
                Some(CV::Ref(..)) => return no("`slot(x)` of a value that may be null: check it first with `if exists(x) {{ ... }}` (inside, x is the item), or give a fallback with `or(...)`"),
                _ => unreachable!("generic above"),
            },
            B::Atan2 | B::Pow | B::Hypot | B::Fmod => {
                let name = match bi { B::Atan2 => "atan2", B::Hypot => "hypot", B::Fmod => "fmod", _ => "pow" };
                let num = |v: &CV| match v { CV::Flt(e) => Some(e.clone()), CV::Int(e) => Some(format!("(double)({})", e)), _ => None };
                let (x, y) = match a.as_slice() { [p, q] => match (num(p), num(q)) { (Some(x), Some(y)) => (x, y), _ => return no(format!("`{}` takes two numbers", name)) }, _ => return no(format!("`{}` takes two numbers", name)) };
                CV::Flt(format!("{}_({}, {})", name.to_uppercase(), x, y))
            }
            B::Math(k) => {
                // jq's other math functions (lang::MATH): the C macro NAME_ over doubles; an integer is taken as a float
                let (name, arity, kind) = crate::lang::MATH[k as usize];
                let num = |v: &CV| match v { CV::Flt(e) => Some(e.clone()), CV::Int(e) => Some(format!("(double)({})", e)), _ => None };
                let xs: Vec<String> = a.iter().filter_map(num).collect();
                if a.len() != arity || xs.len() != arity { return no(format!("`{}` takes {} number{}", name, arity, if arity == 1 { "" } else { "s" })); }
                let call = format!("{}_({})", name.to_uppercase(), xs.join(", "));
                if kind == 2 { CV::Int(call) } else { CV::Flt(call) }
            }
            B::Sqrt | B::Exp | B::Floor | B::Sin | B::Cos | B::Tan | B::Asin | B::Acos | B::Atan | B::Sinh | B::Cosh | B::Tanh | B::Ln | B::Log2 | B::Log10 | B::Ceil | B::Round | B::Trunc | B::Cbrt => {
                // IEEE results, as the platform's math library gives them (a negative square root is NaN)
                let name = match bi { B::Sqrt => "sqrt", B::Exp => "exp", B::Sin => "sin", B::Cos => "cos", B::Tan => "tan", B::Asin => "asin", B::Acos => "acos", B::Atan => "atan", B::Sinh => "sinh", B::Cosh => "cosh", B::Tanh => "tanh", B::Ln => "ln", B::Log2 => "log2", B::Log10 => "log10", B::Ceil => "ceil", B::Round => "round", B::Trunc => "trunc", B::Cbrt => "cbrt", _ => "floor" };
                let x = match a.as_slice() { [CV::Flt(e)] => e.clone(), [CV::Int(e)] => format!("(double)({})", e), _ => return no(format!("`{}` takes one number", name)) };
                let mac = if name == "round" { "ROUNDF_".to_string() } else { format!("{}_", name.to_uppercase()) };
                CV::Flt(format!("{}({})", mac, x))
            }
            B::Substr => {
                // a piece of a string, `substr(text, from, count)`: it shares the text's bytes (no copy), the range is checked
                let [text, from, count] = <[CV; 3]>::try_from(a).map_err(|_| "`substr` takes a string, a start and a count".to_string())?;
                if matches!(text, CV::PlainStr(_) | CV::StateStr(..) | CV::Sub(..)) {
                    let (from, count) = (self.int_or(from, 400, "a start and a count must be integers"), self.int_or(count, 400, "a start and a count must be integers"));
                    let (from, count) = (self.hold_int(from), self.hold_int(count));
                    let (base, start, have) = match text { CV::Sub(b, s0, n0) => (*b, s0, n0), b => { let l = self.field_len(&b); let l = self.hold_int(l); (b, "0".to_string(), l) } };
                    self.line(&format!("if (UNLIKELY({from} < 0 || {count} < 0 || {from} + {count} > (i64)({have}))) FAIL(400, \"substring out of range\", 22);"));
                    let s2 = if start == "0" { from.clone() } else { let t = self.tmp("v"); self.line(&format!("i64 {} = {} + {};", t, start, from)); t };
                    if !self.is_ascii(&base) {
                        // utf8: a piece starts and ends on a character's first byte (or the end)
                        let flen = self.field_len(&base);
                        let (a0, a1) = (self.sub_bytes_at(&base, &s2), self.sub_bytes_at(&base, &format!("{} + {}", s2, count)));
                        self.line(&format!("if (UNLIKELY(({s2} < {flen} && (LD_U8({a0}, 0) & 0xC0) == 0x80) || ({s2} + {count} < {flen} && (LD_U8({a1}, 0) & 0xC0) == 0x80))) FAIL(400, \"substring would split a character\", 33);"));
                    }
                    return Ok(CV::Sub(Box::new(base), s2, count));
                }
                let tb = self.txt_bound(&text);
                let t = self.txt_of(text)?;
                let (from, count) = (self.int_or(from, 400, "a start and a count must be integers"), self.int_or(count, 400, "a start and a count must be integers"));
                let (from, count) = (self.hold_int(from), self.hold_int(count));
                self.line(&format!("if (UNLIKELY({from} < 0 || {count} < 0 || {from} + {count} > (i64){t}.len)) FAIL(400, \"substring out of range\", 22);"));
                let r = self.tmp("t");
                self.line(&format!("Txt {r} = {{ (u32)({t}.off + {from}), (u32){count}, 1 }};"));
                if let Some(b) = tb { self.txt_bounds.insert(r.clone(), b); }
                CV::Txt(r)
            }
            B::Trim => {
                // `trim(text)`: the text without the spaces, tabs and line breaks at either end, a window onto the same bytes
                let [text] = <[CV; 1]>::try_from(a).map_err(|_| "`trim` takes one text".to_string())?;
                let tb = self.txt_bound(&text);
                let t = self.txt_of(text).map_err(|_| "`trim` takes a text".to_string())?;
                let r = self.tmp("t");
                self.line(&format!("Txt {} = {{ 0, 0, 0 }};", r));
                self.call(&format!("c->h->trim(c, &{}, &{})", t, r));
                if let Some(b) = tb { self.txt_bounds.insert(r.clone(), b); }
                CV::Txt(r)
            }
            B::WrapAdd | B::WrapSub | B::WrapMul => {
                // two's-complement wrapping arithmetic (the ordinary operators fail the request on overflow)
                let name = match bi { B::WrapAdd => "wrapping_add", B::WrapSub => "wrapping_sub", _ => "wrapping_mul" };
                let op = match bi { B::WrapAdd => '+', B::WrapSub => '-', _ => '*' };
                let [x, y] = <[CV; 2]>::try_from(a).map_err(|_| format!("`{}` takes two integers", name))?;
                let (x, y) = (self.int_or(x, 400, "wrapping arithmetic takes integers"), self.int_or(y, 400, "wrapping arithmetic takes integers"));
                CV::Int(format!("(i64)((u64)({}) {} (u64)({}))", x, op, y))
            }
            B::TextFrom => {
                // `text_from(bytes, from, count)`: the bytes of a state array of u8 as a string (copied into the request's text area), range checked
                let [arr, from, count] = <[CV; 3]>::try_from(a).map_err(|_| "`text_from` takes a byte array, a start and a count".to_string())?;
                let (base, len) = match arr {
                    CV::ArrCol(c, idx, ci) => {
                        let col = &self.prog.colls[c].cols[ci];
                        let CKind::Arr(_, len) = col.kind else { unreachable!() };
                        if col.kind.size() != len { return no("`text_from` reads an array of u8"); }
                        (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), len)
                    }
                    CV::PlainArr(off, e, len) => {
                        if self.prog.plain_size(&e).0 != 1 { return no("`text_from` reads an array of u8"); }
                        (format!("{}ULL", off), len)
                    }
                    _ => return no("`text_from` reads a fixed array of u8"),
                };
                let (from, count) = (self.int_or(from, 400, "a start and a count must be integers"), self.int_or(count, 400, "a start and a count must be integers"));
                let (from, count) = (self.hold_int(from), self.hold_int(count));
                self.line(&format!("if (UNLIKELY({from} < 0 || {count} < 0 || {from} + {count} > {len}LL)) FAIL(400, \"text_from out of range\", 22);"));
                let r = self.tmp("t");
                self.line(&format!("Txt {r};"));
                self.call(&format!("c->h->arr_txt(c, {base} + (u64){from}, (u32){count}, &{r})"));
                CV::Txt(r)
            }
            B::Abs => match a.as_slice() {
                [CV::Flt(e)] => CV::Flt(format!("FABS_({})", e)),
                [CV::Int(e)] => { let e = self.hold_int(e.clone()); let r = self.tmp("v"); self.line(&format!("ABS_({}, {});", r, e)); CV::Int(r) }
                _ => return no("`abs` takes one number"),
            },
            // a value kept in a `let`, `var`, parameter or result declared with a sized number type (`var h: u32`): tested against the type's range, as a
            // store into a field with that range is (the parser writes the call; `a` is the value, the type's code and the name for the message)
            B::InRange => {
                let [v, code, name] = <[CV; 3]>::try_from(a).map_err(|_| "internal: a typed value takes three arguments".to_string())?;
                let code = match &code { CV::Int(s) => s.trim_end_matches("LL").parse::<usize>().unwrap_or(0), _ => 0 };
                let name = match &name { CV::Lit(s) => s.clone(), _ => "value".to_string() };
                let (_, lo, hi) = NUM_TYPES[code.min(NUM_TYPES.len() - 1)];
                let v = self.in_field_range(v, (lo, hi), &name)?;
                self.with_field_range(v, Some((lo, hi)))
            }
            // an `f32`: a number rounded to the nearest float
            B::F32 => match a.as_slice() {
                [CV::Flt(e)] => CV::Flt(format!("((double)(float)({}))", e)),
                [CV::Int(e)] => CV::Flt(format!("((double)(float)({}))", e)),
                _ => return no("an f32 takes a number"),
            },
            B::Crash => { self.line("TAIL(c->h->crash(c));"); CV::Null }
            _ => unreachable!("handled above"),
        })
    }

}
