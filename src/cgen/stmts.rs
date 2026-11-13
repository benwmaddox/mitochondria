//! Statement lowering of G (stmt, stmt_inner, return, coercions).
use super::*;

impl<'p> G<'p> {
    pub(super) fn stmt(&mut self, s: &'p Stmt, env: &mut Vec<Option<CV>>) -> R<()> {
        if let Some(l) = self.deferred_op(s, env)? {
            // the lowered statement is a collection's own: `Let` named the slot the binder declared, which the allocation takes over
            let l: &'p Stmt = match (s, l) { (Stmt::Let(_, slot, _), Stmt::Alloc { var, coll, try_, .. }) => Box::leak(Box::new(Stmt::Alloc { var: var.clone(), slot: *slot, coll: coll.clone(), try_: *try_ })), _ => l };
            return self.stmt(l, env);
        }
        if let (Stmt::Each(..), None) = (s, &self.presave) {
            // an outermost loop: generate it, then put the saves of the small fields it stores into in front of it
            let start = self.body.len();
            self.presave = Some(Vec::new());
            let r = self.stmt_inner(s, env);
            let list = self.presave.take().unwrap_or_default();
            r?;
            if !list.is_empty() {
                let kept = std::mem::take(&mut self.body);
                let mut lines: Vec<usize> = list.iter().flat_map(|(off, size)| off / 64..(off + size + 63) / 64).collect();
                lines.sort(); lines.dedup();
                for l in lines { self.line(&format!("SAVE_({}ULL, 64);", l * 64)); }
                let head = std::mem::replace(&mut self.body, kept);
                self.body.insert_str(start, &head);
            }
            return Ok(());
        }
        self.stmt_inner(s, env)
    }
    pub(super) fn stmt_inner(&mut self, s: &'p Stmt, env: &mut Vec<Option<CV>>) -> R<()> {
        let rest = std::mem::take(&mut self.rest); // the statements after this one in its block
        if let Stmt::Assign(lhs, _) = s { self.check_assign_target(lhs, env)?; }
        match s {
            Stmt::At(l, c) => { self.at = (*l, *c); self.line("#undef AT_"); self.line(&format!("#define AT_ {}", l)); }
            Stmt::Break => { let l = self.loops.last_mut().ok_or("`break` outside a loop")?; l.2 = true; let g = format!("goto {};", l.0); self.line(&g); }
            Stmt::Continue => { let l = self.loops.last_mut().ok_or("`continue` outside a loop")?; l.3 = true; let g = format!("goto {};", l.1); self.line(&g); }
            Stmt::MatchArms(..) => {}
            Stmt::ContinueHere => { let l = self.loops.last_mut().ok_or("misplaced loop step")?; l.3 = false; let g = format!("{}: ;", l.1); self.line(&g); }
            Stmt::Var(n, slot, Expr::Call(f, args, _)) if f == "__mito__text_cap" => {
                // `var acc: text<N> = ...`: N bytes reserved in its scope's region (block() gives them back at the scope's end), written
                // in place by `=` and `+=`
                let (Some(Expr::Int(cap)), Some(init)) = (args.first(), args.get(1)) else { return no("`var x: text<N> = ...` expected") };
                let cap = *cap as usize;
                let t = self.tmp("t");
                self.line(&format!("Txt {} = {{ 0, 0, 1 }};", t));
                self.call(&format!("c->h->txt_cap(c, {}, &{})", cap, t));
                self.var_names.insert(t.clone());
                self.txt_caps.insert(t.clone(), cap);
                self.txt_bounds.insert(t.clone(), cap);
                self.region_add(Some(cap), n)?;
                self.txt_write(&t, cap, true, &[init], env)?;
                self.own_region = true;
                let place = match self.inlining.last() { Some(k) => format!("fn {}", self.api.fns[*k as usize].name), None => self.ep.text.clone() };
                KINDS.with(|k| k.borrow_mut().push((place, self.at.0, format!("var {}", n), format!("text (its own, up to {} bytes)", cap))));
                env[*slot as usize] = Some(CV::Txt(t));
            }
            // `let w = acc` of a capped var: a window onto it (each use reads the var as it is then), as a `let` of a field is
            Stmt::Let(n, slot, e @ Expr::Var(..)) if matches!(self.expr(e, env, None)?, CV::Txt(t) if self.txt_caps.contains_key(&t)) => {
                let v = self.expr(e, env, None)?;
                let place = match self.inlining.last() { Some(k) => format!("fn {}", self.api.fns[*k as usize].name), None => self.ep.text.clone() };
                KINDS.with(|k| k.borrow_mut().push((place, self.at.0, format!("let {}", n), "text (a window onto a var)".to_string())));
                env[*slot as usize] = Some(v);
            }
            Stmt::Let(_, slot, e) | Stmt::Var(_, slot, e) => {
                let mutable = matches!(s, Stmt::Var(..));
                self.immut.remove(&(self.frame, *slot));
                // a struct bound to a name is a view of the place it names (`V { ..x }` makes a place of its own); a struct literal builds one in the scratch area
                if let Expr::Call(f, args, _) = e { if f == "__mito__struct" { let v = self.struct_new(args)?; env[*slot as usize] = Some(v); return Ok(()); } }
                if matches!(e, Expr::Call(..)) { self.ret_ctx = Some(None); }
                let v = self.expr(e, env, None);
                self.ret_ctx = None;
                let v = v?;
                if let CV::Plain(sid, src) = v {
                    // a struct bound to a name is a view of it, never a copy (`let c = Pos { ..world.pos };` is a copy): a literal's hidden temporary is its own place
                    let v = CV::Plain(sid, src);
                    if !mutable && !matches!(s, Stmt::Let(n, ..) if n.starts_with("__mito__lit")) { self.immut.insert((self.frame, *slot)); }
                    env[*slot as usize] = Some(v);
                    return Ok(());
                }
                let init_iv = match &v { CV::Int(x) => self.interval(x), _ => None };
                // text lives in fields: a local is a window onto text (a field, an input, a literal, a piece of one), never its own copy
                // or text it builds (tests may: their messages are built text)
                if !self.test && matches!(v, CV::Txt(_) | CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..)) {
                    let n = match s { Stmt::Let(n, ..) | Stmt::Var(n, ..) => n.clone(), _ => String::new() };
                    if mutable { return no(format!("`var {}` would hold its own text: declare its cap, `var {}: text<1024> = ...;` (its bytes in its scope's region), or name a field with `let` (a window onto it)", n, n)); }
                    // (text a function returns lives in the request's text area, as a builtin's does: a window onto it)
                    // text built here (`let k = a + ":" + b`): it lives in its scope's region (block() releases it at the scope's end)
                    let built = match e { Expr::Bin(op, ..) => op == "+", Expr::Call(f, _, _) => f == "str", _ => false };
                    if built { let b = self.txt_bound(&v); self.region_add(b, &n)?; self.own_region = true; }
                }
                let v = match self.slot_kind(*slot) { Some(k) => self.coerce(v, k, mutable)?, None => self.hold(v, mutable)? };
                {
                    let place = match self.inlining.last() { Some(k) => format!("fn {}", self.api.fns[*k as usize].name), None => self.ep.text.clone() };
                    let name = match s { Stmt::Let(n, ..) | Stmt::Var(n, ..) => format!("{} {}", if mutable { "var" } else { "let" }, n), _ => String::new() };
                    let kind = match &v { CV::StateStr(..) | CV::PlainStr(_) | CV::Sub(..) => "text (a window onto a field)".to_string(), CV::Lit(_) => "text (a literal)".to_string(), other => type_text(self.prog, other) };
                    if !name.contains("__mito__") { KINDS.with(|k| k.borrow_mut().push((place, self.at.0, name, kind))); }
                }
                // a `var` that starts as a constant and is only ever assigned constants or `x & mask` stays inside one range for its whole life
                // (a hash kept to 32 bits: `h = (h * 33 + b) & 0xFFFFFFFF`): its uses may assume it, which drops overflow tests on its arithmetic
                if mutable {
                    if let (CV::Int(name), Some(r)) = (&v, masked_var_range(rest, *slot, e)) {
                        self.ranges.insert(name.clone(), r);
                        // a range inside 0..2^32-1: the C variable is a `u32` (a sum or hash kept to 32 bits then is a plain 32-bit reduction for the C compiler, which
                        // vectorizes it); every read casts it back to `i64`
                        if r.0 >= 0 && r.1 <= u32::MAX as i64 {
                            let decl = format!("i64 {} = ", name);
                            if let Some(at) = self.body.rfind(&decl) { self.body.replace_range(at..at + 3, "u32"); self.u32_vars.insert(name.clone()); }
                        }
                    }
                }
                if mutable { if let CV::Int(name) = &v { self.var_depth.insert(name.clone(), self.loop_frames.len()); } }
                if mutable { if let (CV::Int(name), Some(iv)) = (&v, init_iv) { if !self.ranges.contains_key(name) { self.var_iv.insert(name.clone(), iv); } } }
                env[*slot as usize] = Some(v);
            }
            Stmt::Assign(target @ Expr::Var(n, VarRes::Local(slot)), rhs) => {
                // a `write` parameter of a number or bool: the caller's place (`wskip`: this is the store itself, into the shared `var`)
                if !std::mem::take(&mut self.wskip) {
                    if let Some(w) = self.wslots.get(&(self.frame, *slot)).cloned() { return self.write_param_assign(w, target, *slot, rhs, env, rest); }
                }
                let cur = env[*slot as usize].clone().ok_or_else(|| format!("`{}` has no compiled value", n))?;
                // a struct local: the whole struct is copied in (its place stays where it is)
                if let CV::Plain(sid, off) = cur {
                    if matches!(rhs, Expr::Call(..)) { self.ret_ctx = Some(Some(off)); }
                    let v = self.expr(rhs, env, None);
                    self.ret_ctx = None;
                    let v = v?;
                    let CV::Plain(s2, src) = v else { return no(format!("`{}` is a `{}`: assign it a struct of that type", n, self.prog.structs[sid].name)) };
                    if s2 != sid { return no(format!("`{}` is a `{}`, not a `{}`", n, self.prog.structs[sid].name, self.prog.structs[s2].name)); }
                    return self.copy_struct(sid, off, src);
                }
                // a `write` text parameter is the caller's text field itself: assigning it stores into that field (`s = "x"`, `s += "x"`), as a field assignment does
                if matches!(cur, CV::PlainStr(_) | CV::StateStr(..)) {
                    let ascii = self.is_ascii(&cur);
                    let (clear, pieces) = match str_pieces(target, rhs) { Some(p) => p, None => (true, vec![rhs]) };
                    return match &cur {
                        CV::PlainStr(off) => {
                            let Some(cap) = self.plain_str_cap(*off) else { return no(format!("`{}` is a text of a size the compiler could not find", n)) };
                            self.may_write_plain(*off)?;
                            self.str_append(if ascii { 4 } else { 0 }, format!("{}ULL", off), format!("{}ULL", cap), 0, clear, &pieces, env)
                        }
                        CV::StateStr(c, idx, ci) => {
                            let path = self.prog.colls[*c].cols[*ci].path.clone();
                            self.may_write_col(*c, &path)?;
                            self.str_append(if ascii { 5 } else { 1 }, format!("{}ULL", c), format!("(u64)({})", idx), *ci as u32, clear, &pieces, env)
                        }
                        _ => unreachable!(),
                    };
                }
                // a capped var of text: written in place (`acc += a + b`: the pieces after what it holds; `acc = ...`: from the start)
                if let CV::Txt(t) = &cur {
                    if let Some(cap) = self.txt_caps.get(t).copied() {
                        let t = t.clone();
                        return match str_pieces(target, rhs) { Some((clear, pieces)) => self.txt_write(&t, cap, clear, &pieces, env), None => self.txt_write(&t, cap, true, &[rhs], env) };
                    }
                }
                let v = self.expr(rhs, env, None)?;
                if matches!(cur, CV::Txt(_) | CV::Any(_)) { self.txt_escapes.push(*slot); }
                if let Err(e) = self.assign_var(&cur, v.clone()) {
                    if let (Some(a), Some(b)) = (kind(&cur), kind(&v)) {
                        if unify(a, b) == K::Any { why(format!("`{}` holds {} and then {}: a var keeps the kind of its first value", n, kname(self.prog, a), kname(self.prog, b))); }
                        self.widen = Some(Widen::Slot(self.frame, *slot, unify(a, b)));
                    }
                    return Err(e);
                }
                // what is now known of the variable: the interval of the value just assigned (nothing when there is none)
                if let (CV::Int(name), CV::Int(c)) = (&cur, &v) {
                    if !self.ranges.contains_key(name) {
                        let now = self.interval(c);
                        match now { Some(iv) => { self.var_iv.insert(name.clone(), iv); } None => { self.var_iv.remove(name); } }
                        // every loop the variable is carried through must keep it inside the interval it assumes at its head: say what this assignment gives
                        if let Some(&d) = self.var_depth.get(name) {
                            for f in d..self.loop_frames.len() {
                                let key = (self.loop_frames[f], *slot);
                                LOOP_INV.with(|l| { let mut l = l.borrow_mut(); let e = l.assigns.entry(key).or_insert(now); *e = match (*e, now) { (Some(a), Some(b)) => Some(iv_union(a, b)), _ => None }; });
                            }
                        }
                    }
                }
            }
            Stmt::Assign(Expr::Field(base, f), rhs) => {
                let bv = self.expr(base, env, None)?;
                let (c, idx, path) = match bv {
                    CV::Item(c, idx) => (c, idx, f.clone()),
                    CV::Part(c, idx, p) => (c, idx, format!("{}.{}", p, f)),
                    CV::Elem(c, idx, p, k) => {
                        // `item.vals[i].a = x`: element i of the array column `vals.a`
                        let path = format!("{}.{}", p, f);
                        let Some(ci) = self.prog.colls[c].cols.iter().position(|x| x.path == path && matches!(x.kind, CKind::Arr(..))) else { return no(format!("no field `{}` in the elements of `{}`", f, p)) };
                        self.may_write_col(c, &path)?;
                        let v = self.expr(rhs, env, None)?;
                        self.skip_save = self.prog.colls[c].unlogged;
                        let inline = self.int_store(c, &idx, ci, Some(&k), &v, &format!("c->h->store_elem(c, {}, {}, {}, {}, &@VAL)", c, idx, ci, k))?;
                        self.skip_save = false;
                        if !inline {
                            let vv = self.val(v)?;
                            self.call(&format!("c->h->store_elem(c, {}, {}, {}, {}, &{})", c, idx, ci, k, vv));
                        }
                        return Ok(());
                    }
                    CV::Ref(..) => return no(format!("assigning `.{}` of a value that may be null: check it first with `if exists(x) {{ ... }}` (inside, x is the item), or give a fallback with `or(...)`", f)),
                    CV::Plain(sid, off) => {
                        let sd = &self.prog.structs[sid];
                        let Some(k) = sd.fields.iter().position(|x| x.0 == *f) else { return no(format!("no field `{}`{}", f, crate::lang::suggest(f, sd.fields.iter().map(|x| x.0.as_str())))) };
                        self.touch_request(off, sid, k);
                        let sd = &self.prog.structs[sid];
                        // a struct-valued field: the whole struct is copied in (`world.pos = v;`)
                        if let Ty::Struct(s2) = sd.fields[k].1 {
                            let at = off + sd.offs[k];
                            if matches!(rhs, Expr::Call(..)) { self.ret_ctx = Some(Some(at)); }
                            let v = self.expr(rhs, env, None);
                            self.ret_ctx = None;
                            let v = v?;
                            let CV::Plain(s3, src) = v else { return no(format!("`.{}` is a `{}`: assign it a struct of that type", f, self.prog.structs[s2].name)) };
                            if s3 != s2 { return no(format!("`.{}` is a `{}`, not a `{}`", f, self.prog.structs[s2].name, self.prog.structs[s3].name)); }
                            self.prog.value_check(s2)?;
                            return self.copy_struct(s2, at, src);
                        }
                        let (code, cap) = plain_code(&sd.fields[k].1).ok_or_else(|| format!("assigning plain `.{}` not compiled yet", f))?;
                        let at = off + sd.offs[k];
                        let range = sd.ranges[k];
                        self.may_write_plain(at)?;
                        if let Some(target) = self.prog.id_at(crate::layout::IdAt::Plain(at)) {
                            self.field_store_seen(at, None);
                            let v = self.expr(rhs, env, None)?;
                            let v = self.id_store_value(target, v, f)?;
                            let vv = self.val(v)?;
                            self.call(&format!("c->h->store_plain(c, {}, {}, {}, &{})", at, code, cap, vv));
                            return Ok(());
                        }
                        let ascii = code == 20 && range.is_some();
                        if code == 20 { if let Some((clear, pieces)) = str_pieces(&Expr::Field(base.clone(), f.clone()), rhs) { return self.str_append(if ascii { 4 } else { 0 }, format!("{}ULL", at), format!("{}ULL", cap), 0, clear, &pieces, env); } }
                        let v = self.expr(rhs, env, None)?;
                        if ascii { self.ascii_lit(&v, f)?; }
                        let mut v = match range { Some(r) if code != 20 => self.in_field_range(v, r, f)?, _ => v };
                        if range.is_none() && (1..=8).contains(&code) {
                            let v2 = match v { CV::Int(e) => { let n = self.hold_int(e); CV::Int(n) } v => v };
                            let r = match &v2 { CV::Int(n) => self.interval(n), _ => None };
                            self.field_store_seen(at, r);
                            v = v2;
                        }
                        let code = if ascii { 21 } else { code };
                        self.skip_save = self.unlogged_at(at) || (code != 21 && self.presaved(at, scalar_bytes(code) as usize));
                        if self.scalar_store(code, format!("{}ULL", at), &v, &format!("c->h->store_plain(c, {}, {}, {}, &@VAL)", at, code, cap), true, at % scalar_bytes(code) as usize == 0)? { return Ok(()); }
                        self.skip_save = false;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store_plain(c, {}, {}, {}, &{})", at, code, cap, vv));
                        return Ok(());
                    }
                    CV::Null => return no(format!("assigning `.{}` of `null`", f)),
                    CV::Any(_) => return no(format!("assigning `.{}` of a value whose kind is only known when the program runs, which may be null: give it a known kind (a plain item)", f)),
                    _ => { self.fail(500, "assignment target is not an item or a state struct"); return Ok(()); }
                };
                if let Some(t) = tab(c) { return self.table_set(t, &idx, &path, rhs, env); }
                let Some(ci) = self.prog.colls[c].cols.iter().position(|x| x.path == path && !matches!(x.kind, CKind::StrBytes(_) | CKind::Arr(..))) else {
                    let prefix = format!("{}.", path);
                    let known = self.prog.colls[c].cols.iter().any(|x| x.path == path || x.path.starts_with(&prefix)) || self.prog.colls[c].children.iter().any(|x| x.0 == path);
                    return no(if known { format!("assigning `.{}` as a whole is not supported yet; assign its fields", path) } else { format!("no field `{}`", path) });
                };
                self.may_write_col(c, &path)?;
                if let Some(target) = self.prog.id_at(crate::layout::IdAt::Col(c, ci)) {
                    let v = self.expr(rhs, env, None)?;
                    let v = self.id_store_value(target, v, &path)?;
                    let vv = self.val(v)?;
                    self.call(&format!("c->h->store(c, {}, {}, {}, &{})", c, idx, ci, vv));
                    return Ok(());
                }
                let is_text = matches!(self.prog.colls[c].cols[ci].kind, CKind::StrLen);
                let ascii = is_text && self.prog.colls[c].cols[ci].range.is_some();
                if is_text {
                    if let Some((clear, pieces)) = str_pieces(&Expr::Field(base.clone(), f.clone()), rhs) { return self.str_append(if ascii { 5 } else { 1 }, format!("{}ULL", c), format!("(u64)({})", idx), ci as u32, clear, &pieces, env); }
                }
                let v = self.expr(rhs, env, None)?;
                if ascii { self.ascii_lit(&v, &path)?; }
                let v = match self.prog.colls[c].cols[ci].range { Some(r) if !is_text => self.in_field_range(v, r, &path)?, _ => v };
                self.skip_save = self.prog.colls[c].unlogged;
                let inline = self.int_store(c, &idx, ci, None, &v, &format!("c->h->store(c, {}, {}, {}, &@VAL)", c, idx, ci))?;
                self.skip_save = false;
                if !inline {
                    let vv = self.val(v)?;
                    self.call(&format!("c->h->store(c, {}, {}, {}, &{})", c, idx, ci, vv));
                }
            }
            Stmt::Assign(Expr::Index(base, k_expr), rhs) => {
                let bv = self.expr(base, env, None)?;
                let kv = self.expr(k_expr, env, None)?;
                let k = self.int_or(kv, 400, "an index must be an integer");
                if let CV::PlainStr(_) | CV::StateStr(..) = &bv {
                    // `s[i] = b` on ascii text: a byte 0..127 at a position below its length
                    if !self.is_ascii(&bv) { return no("utf8 text is not written by index (a character may take several bytes): assign it or append to it, or make the field ascii<N>"); }
                    match &bv { CV::PlainStr(off) => self.may_write_plain(*off)?, CV::StateStr(c, _, ci) => { let p = self.prog.colls[*c].cols[*ci].path.clone(); self.may_write_col(*c, &p)? } _ => {} }
                    let k = self.hold_int(k);
                    let len = self.field_len(&bv);
                    self.line(&format!("if (RANGE_({k} < 0 || {k} >= {len})) FAIL(400, \"string index out of range\", 25);"));
                    let v = self.expr(rhs, env, None)?;
                    let x = self.int_or(v, 400, "a byte of ascii text is an integer 0..127");
                    let x = self.hold_int(x);
                    self.line(&format!("if (RANGE_({x} < 0 || {x} > 127)) FAIL(400, \"not an ASCII byte\", 17);"));
                    let at = self.sub_bytes_at(&bv, &k);
                    self.call(&format!("c->h->wr_int(c, {}, 1, {})", at, x));
                    return Ok(());
                }
                if let CV::Coll(..) | CV::ArrCol(..) | CV::PlainArr(..) = bv {} else { self.fail(500, "only fixed arrays and collections can be assigned by index"); return Ok(()); }
                match bv {
                    CV::ArrCol(c, idx, ci) => {
                        self.may_write_col(c, &self.prog.colls[c].cols[ci].path.clone())?;
                        let CKind::Arr(_, n) = self.prog.colls[c].cols[ci].kind else { unreachable!() };
                        self.elem_check(&k, n as u64);
                        let v = self.expr(rhs, env, None)?;
                        self.skip_save = self.prog.colls[c].unlogged;
                        let inline = self.int_store(c, &idx, ci, Some(&k), &v, &format!("c->h->store_elem(c, {}, {}, {}, {}, &@VAL)", c, idx, ci, k))?;
                        self.skip_save = false;
                        if !inline {
                            let vv = self.val(v)?;
                            self.call(&format!("c->h->store_elem(c, {}, {}, {}, {}, &{})", c, idx, ci, k, vv));
                        }
                    }
                    CV::PlainArr(off, e, n) => {
                        self.may_write_plain(off)?;
                        let Some((code, cap)) = plain_code(&e) else { return no("assigning an element of this array not compiled yet") };
                        let covered = self.fast_sites.contains(&(&**k_expr as *const Expr));
                        if !covered { self.elem_check(&k, n as u64); }
                        let v = self.expr(rhs, env, None)?;
                        // what the whole program stores into this array is what its reads may assume (see `generate_with`)
                        let stored = match &v { CV::Int(x) => self.interval(x), _ => None };
                        self.field_store_seen(off, stored);
                        let esz = self.prog.plain_size(&e).0;
                        self.skip_save = covered || self.unlogged_at(off) || self.presaved(off, n as usize * esz as usize);
                        if self.scalar_store(code, format!("{}ULL + (u64)({}) * {}ULL", off, k, esz), &v, &format!("c->h->store_plain(c, (u64)({} + {} * {}), {}, {}, &@VAL)", off, k, esz, code, cap), true, off % esz as usize == 0)? { self.skip_save = false; return Ok(()); }
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store_plain(c, (u64)({} + {} * {}), {}, {}, &{})", off, k, esz, code, cap, vv));
                        self.skip_save = false;
                    }
                    CV::Coll(c, inst) => {
                        self.may_write_col(c, "")?;
                        let t = self.index_of(c, &inst, &k);
                        let coll = &self.prog.colls[c];
                        let plain = if coll.children.iter().any(|x| x.0.is_empty()) { None } else { coll.cols.iter().position(|x| x.path.is_empty()) };
                        let Some(ci) = plain else { self.fail(500, "only an item that is a plain value can be assigned; assign its fields"); return Ok(()); };
                        let v = self.expr(rhs, env, None)?;
                        let vv = self.val(v)?;
                        self.call(&format!("c->h->store(c, {}, {}, {}, &{})", c, t, ci, vv));
                    }
                    _ => return no("only fixed arrays and collections can be assigned by index"),
                }
            }
            Stmt::Assign(..) => return no("assignment target not compiled yet"),
            Stmt::If(c, t, e) => {
                // `if exists(x) && more { T } else { E }` is `if exists(x) { if more { T } else { E } } else { E }`: `more` and T see x
                // narrowed, and nothing of `more` runs before the test
                if let Some((a, b)) = split_after_narrowing(c) {
                    let inner: &'p Stmt = Box::leak(Box::new(Stmt::If(b, t.clone(), e.clone())));
                    let outer: &'p Stmt = Box::leak(Box::new(Stmt::If(a, vec![inner.clone()], e.clone())));
                    return self.stmt(outer, env);
                }
                // `if exists(m.f)`: in the branch where it holds, m's field f is not null (a nullable time a number, a reference an item)
                let field_key = field_narrow_test(c).and_then(|(slot, f, yes)| match env[slot as usize].clone() { Some(CV::Item(cl, k)) => Some(((cl, k, f), yes)), _ => None });
                // `if exists(x)` / `if x != null`: inside, x is a plain item; `if !exists(x)` / `if x == null`: in the else branch
                let narrowed = narrow_test(c).and_then(|(slot, yes)| match env[slot as usize].clone() { Some(CV::Ref(cl, _, k)) => Some((slot, yes, cl, k)), _ => None });
                let cv = self.expr(c, env, None)?;
                let cond = self.cond(cv)?;
                self.open(&format!("if ({}) {{", cond));
                let narrow = |g: &mut Self, env: &mut Vec<Option<CV>>, branch: &'p [Stmt], want: bool| -> Option<CV> {
                    let (slot, yes, cl, k) = narrowed.clone()?;
                    if yes != want || assigned_slots(branch).contains(&slot) { return None; }
                    let _ = g; env[slot as usize].replace(CV::Item(cl, k))
                };
                let iv_before = self.var_iv.clone();
                self.refine(c, env)?; // inside the branch the condition holds
                let kept = narrow(self, env, t, true);
                let fk = match &field_key { Some((k, true)) => { self.narrowed.push(k.clone()); true } _ => false };
                self.block(t, env)?;
                if fk { self.narrowed.pop(); }
                if let (Some(v), Some((slot, ..))) = (kept, narrowed.clone()) { env[slot as usize] = Some(v); }
                self.close("}");
                let iv_then = std::mem::replace(&mut self.var_iv, iv_before.clone());
                if !e.is_empty() {
                    self.open("else {");
                    let kept = narrow(self, env, e, false);
                    let fk = match &field_key { Some((k, false)) => { self.narrowed.push(k.clone()); true } _ => false };
                    self.block(e, env)?;
                    if fk { self.narrowed.pop(); }
                    if let (Some(v), Some((slot, ..))) = (kept, narrowed.clone()) { env[slot as usize] = Some(v); }
                    self.close("}");
                }
                // after the `if`: what both ways agree on (a way that always leaves counts for nothing)
                let leaves = |b: &[Stmt]| matches!(b.last(), Some(Stmt::Fail(..) | Stmt::Return(..) | Stmt::Respond(..) | Stmt::Break | Stmt::Continue));
                let iv_else = std::mem::take(&mut self.var_iv);
                self.var_iv = match (leaves(t), !e.is_empty() && leaves(e)) {
                    (true, false) => iv_else,
                    (false, true) => iv_then,
                    _ => iv_then.iter().filter_map(|(k, a)| iv_else.get(k).map(|b| (k.clone(), (a.0.min(b.0), a.1.max(b.1))))).collect(),
                };
            }
            Stmt::Each(it, body) => {
                if let (Some(_), Some(n)) = (&it.via, crate::lang::path_of(&it.coll)) {
                    let n = &n;
                    if changes_instance(self.api, n, body) { return no(format!("a loop over {} calls a function that changes {}; change it after the loop", n, n)); }
                }
                let collect = allocates(self.api, body, &mut Vec::new()) || (it.wher.is_some() && writes(self.api, body, &mut Vec::new()));
                // a loop that writes no state cannot change a text field's length: read it once, here, for the reads by index inside
                let mut cached = Vec::new();
                if !assigns_field(self.api, body, &mut Vec::new()) {
                    let mut bases = Vec::new();
                    index_bases(body, &mut bases);
                    for arr in bases {
                        if let CV::PlainStr(off) = self.expr(arr, env, None)? {
                            if self.str_lens.contains_key(&off) { continue; }
                            let t = self.tmp("sl");
                            self.line(&format!("i64 {} = (i64)LD_U32({}, 0);", t, off));
                            self.str_lens.insert(off, t);
                            cached.push(off);
                        }
                    }
                }
                // what is known of the variables the loop assigns is dropped for its body, except what a counted loop can say of a counter (see `counter_heads`); after it,
                // the variables it does not assign keep what they had
                let entry_iv = self.var_iv.clone();
                let mut base_iv = entry_iv.clone();
                for slot in assigned_slots(body) { if let Some(Some(CV::Int(n))) = env.get(slot as usize) { base_iv.remove(n); } }
                let loop_id = it as *const Iter as usize;
                for slot in assigned_slots(body) {
                    if let Some(Some(CV::Int(n))) = env.get(slot as usize) {
                        if let (true, Some(iv)) = (self.var_names.contains(n), entry_iv.get(n)) {
                            LOOP_INV.with(|l| { let mut l = l.borrow_mut(); let e = l.entries.entry((loop_id, slot)).or_insert(*iv); *e = iv_union(*e, *iv); });
                        }
                    }
                }
                let (pre, n_pre) = (self.body.len(), self.n);
                match self.version_guard(it, body, env)? {
                    Some((guard, sites, hints, counter)) => {
                        // the fast copy: the guard proved every listed array access of the loop in range and saved the stored ranges to the undo log
                        self.versioned += 1;
                        let fast_start = self.body.len();
                        self.open(&format!("if ({}) {{", guard));
                        let joined: std::collections::HashSet<*const Expr> = sites.union(&self.fast_sites).copied().collect();
                        let kept = std::mem::replace(&mut self.fast_sites, joined);
                        let ranges_kept = self.ranges.clone();
                        for (name, iv) in hints { self.ranges.insert(name, iv); }
                        self.loop_hint = counter;
                        self.loop_vars(it, body, env, collect, &entry_iv, &base_iv)?;
                        self.loop_hint = None;
                        self.ranges = ranges_kept;
                        self.fast_sites = kept;
                        self.close("}");
                        let slow_start = self.body.len();
                        self.open("else {");
                        self.loop_vars(it, body, env, collect, &entry_iv, &base_iv)?;
                        self.close("}");
                        self.versioned -= 1;
                        // the two copies came out the same (the range analysis had already proved every access, nothing was left to drop):
                        // keep one, without the guard
                        let guard_text = &self.body[pre..fast_start];
                        let fast = self.body[fast_start..slow_start].strip_prefix(&"    ".repeat(self.depth + 1)).and_then(|t| t.strip_prefix(&format!("if ({}) ", guard)));
                        let slow = self.body[slow_start..].strip_prefix(&"    ".repeat(self.depth + 1)).and_then(|t| t.strip_prefix("else "));
                        if let (Some(fast), Some(slow)) = (fast, slow) {
                            if same_code(n_pre, guard_text, fast, slow) {
                                let one = format!("{}{}", "    ".repeat(self.depth + 1), slow);
                                self.body.truncate(pre);
                                self.body.push_str(&one);
                            }
                        }
                    }
                    None => self.loop_vars(it, body, env, collect, &entry_iv, &base_iv)?,
                }
                let mut heads = self.loop_heads.take().unwrap_or_default();
                // a variable with no bound from the loop's rounds has what this build assumes of it at the loop's head, in the body and after (see `generate_with`)
                for slot in assigned_slots(body) {
                    if let Some(Some(CV::Int(n))) = env.get(slot as usize) {
                        if let (false, Some(iv)) = (heads.contains_key(n), LOOP_INV.with(|l| l.borrow().assumed.get(&(loop_id, slot)).copied())) { if entry_iv.contains_key(n) { heads.insert(n.clone(), iv); } }
                    }
                }
                self.var_iv = base_iv;
                self.var_iv.extend(heads);
                for off in cached { self.str_lens.remove(&off); }
            }
            Stmt::Move(arr, from, to, count) => {
                let av = self.expr(arr, env, None)?;
                let mut pos = |g: &mut Self, e: &'p Expr, env: &mut Vec<Option<CV>>| -> R<String> { let v = g.expr(e, env, None)?; let k = g.int_or(v, 400, "a position and a count must be integers"); Ok(g.hold_int(k)) };
                let (f, t2, n) = (pos(self, from, env)?, pos(self, to, env)?, pos(self, count, env)?);
                if let CV::Part(c, idx, p) = &av {
                    // an array of structs: every field's column moves alike
                    let inner = format!("{}.", p);
                    let cols: Vec<usize> = (0..self.prog.colls[*c].cols.len()).filter(|&i| self.prog.colls[*c].cols[i].path.starts_with(&inner) && matches!(self.prog.colls[*c].cols[i].kind, CKind::Arr(..))).collect();
                    if cols.is_empty() { return no("`move` works on a fixed array"); }
                    let CKind::Arr(_, len) = self.prog.colls[*c].cols[cols[0]].kind else { unreachable!() };
                    self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {len}LL || {t2} < 0 || {t2} > {len}LL || {n} < 0 || {n} > {len}LL || {f} + {n} > {len}LL || {t2} + {n} > {len}LL)) FAIL(400, \"move out of range\", 17);"));
                    for ci in cols {
                        let path = self.prog.colls[*c].cols[ci].path.clone();
                        self.may_write_col(*c, &path)?;
                        let col = &self.prog.colls[*c].cols[ci];
                        let (base, esize) = (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), col.kind.size() / len);
                        self.call(&format!("c->h->move_elems(c, {}, {}, {}, {}, {})", base, esize, f, t2, n));
                    }
                    return Ok(());
                }
                let mut unlogged_plain = false;
                let (base, esize, len) = match av {
                    CV::ArrCol(c, idx, ci) => {
                        self.may_write_col(c, &self.prog.colls[c].cols[ci].path.clone())?;
                        let col = &self.prog.colls[c].cols[ci];
                        let CKind::Arr(_, len) = col.kind else { unreachable!() };
                        (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), col.kind.size() / len, len)
                    }
                    CV::PlainArr(off, e, len) => { self.may_write_plain(off)?; unlogged_plain = self.unlogged_at(off); (format!("{}ULL", off), self.prog.plain_size(&e).0, len) }
                    _ => return no("`move` works on a fixed array"),
                };
                self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {len}LL || {t2} < 0 || {t2} > {len}LL || {n} < 0 || {n} > {len}LL || {f} + {n} > {len}LL || {t2} + {n} > {len}LL)) FAIL(400, \"move out of range\", 17);"));
                if unlogged_plain {
                    // an unlogged array of plain state: committed at start and kept out of the undo log, so the move is the copy alone (as an inline store of a scalar there is)
                    self.line(&format!("MOVE_({base} + (u64)({t2}) * {esize}ULL, {base} + (u64)({f}) * {esize}ULL, (u64)({n}) * {esize}ULL);"));
                } else {
                    self.call(&format!("c->h->move_elems(c, {}, {}, {}, {}, {})", base, esize, f, t2, n));
                }
            }
            Stmt::Copy(dst, src, from, to, count) => {
                let dv = self.expr(dst, env, None)?;
                let sv = self.expr(src, env, None)?;
                let mut pos = |g: &mut Self, e: &'p Expr, env: &mut Vec<Option<CV>>| -> R<String> { let v = g.expr(e, env, None)?; let k = g.int_or(v, 400, "a position and a count must be integers"); Ok(g.hold_int(k)) };
                let (f, t2, n) = (pos(self, from, env)?, pos(self, to, env)?, pos(self, count, env)?);
                let (CV::PlainArr(doff, de, dlen), CV::PlainArr(soff, se, slen)) = (&dv, &sv) else { return no("`copy_from` copies between two fixed arrays of plain state: `dst.copy_from(src, from, to, count)`") };
                self.may_write_plain(*doff)?;
                let from_src = FIELD_RANGES.with(|fr| fr.borrow().0.get(soff).copied());
                self.field_store_seen(*doff, from_src);
                let (desz, sesz) = (self.prog.plain_size(de).0, self.prog.plain_size(se).0);
                if desz != sesz { return no("`copy_from` copies between arrays of the same element type"); }
                self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {slen}LL || {t2} < 0 || {t2} > {dlen}LL || {n} < 0 || {n} > {slen}LL || {f} + {n} > {slen}LL || {t2} + {n} > {dlen}LL)) FAIL(400, \"copy out of range\", 17);"));
                // a logged destination first saves the lines it is about to overwrite (an `unlogged` one is scratch the program rebuilds, nothing to restore)
                if !self.unlogged_at(*doff) {
                    let (lo, hi) = (format!("({doff}ULL + (u64)({t2}) * {desz}ULL)"), format!("({doff}ULL + (u64)({t2} + {n}) * {desz}ULL)"));
                    self.open(&format!("if ({n} > 0) for (u64 l_ = {lo} >> 6; l_ <= ({hi} - 1) >> 6; l_++) {{"));
                    self.line("SAVE_(l_ << 6, 64);");
                    self.close("}");
                }
                // two different arrays never overlap: a plain copy
                self.line(&format!("COPY_({doff}ULL + (u64)({t2}) * {desz}ULL, {soff}ULL + (u64)({f}) * {desz}ULL, (u64)({n}) * {desz}ULL);"));
            }
            Stmt::Sort(arr, from, count, scratch) => {
                let av = self.expr(arr, env, None)?;
                let tv = self.expr(scratch, env, None)?;
                let mut pos = |g: &mut Self, e: &'p Expr, env: &mut Vec<Option<CV>>| -> R<String> { let v = g.expr(e, env, None)?; let k = g.int_or(v, 400, "a position and a count must be integers"); Ok(g.hold_int(k)) };
                let (f, n) = (pos(self, from, env)?, pos(self, count, env)?);
                let (CV::PlainArr(aoff, Ty::Scalar(asc), alen), CV::PlainArr(toff, Ty::Scalar(tsc), tlen)) = (&av, &tv) else { return no("`sort` puts an array of integers in order, with another array of the same type to work in: `a.sort(from, count, scratch)`") };
                let (code, tcode) = (plain_code(&Ty::Scalar(*asc)).map_or(0, |x| x.0), plain_code(&Ty::Scalar(*tsc)).map_or(0, |x| x.0));
                if !(1..=8).contains(&code) || code != tcode { return no("`sort` puts an array of integers in order, with another array of the same type to work in"); }
                if aoff == toff { return no("`sort` works in another array than the one it sorts"); }
                self.may_write_plain(*aoff)?;
                self.may_write_plain(*toff)?;
                // the values only move: the array keeps what it was assumed to hold; the scratch array is given a copy of them
                let held = FIELD_RANGES.with(|fr| fr.borrow().0.get(aoff).copied());
                self.field_store_seen(*toff, held);
                self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {alen}LL || {n} < 0 || {n} > {alen}LL || {f} + {n} > {alen}LL || {n} > {tlen}LL)) FAIL(400, \"sort out of range\", 17);"));
                self.call(&format!("c->h->sort_elems(c, {}ULL, {}, {}, {}, {}ULL)", aoff, code, f, n, toff));
            }
            Stmt::Fill(arr, from, count, value) => {
                let av = self.expr(arr, env, None)?;
                let mut pos = |g: &mut Self, e: &'p Expr, env: &mut Vec<Option<CV>>| -> R<String> { let v = g.expr(e, env, None)?; let k = g.int_or(v, 400, "a position and a count must be integers"); Ok(g.hold_int(k)) };
                let (f, n) = (pos(self, from, env)?, pos(self, count, env)?);
                let v = self.expr(value, env, None)?;
                let (base, code, len, plain) = match av {
                    CV::ArrCol(c, idx, ci) => {
                        self.may_write_col(c, &self.prog.colls[c].cols[ci].path.clone())?;
                        let col = &self.prog.colls[c].cols[ci];
                        let CKind::Arr(s, len) = col.kind else { unreachable!() };
                        (format!("{}ULL + (u64)({}) * {}ULL", col.off, idx, col.kind.size()), plain_code(&Ty::Scalar(s)).map_or(0, |x| x.0), len, 0)
                    }
                    CV::PlainArr(off, e, len) => {
                        self.may_write_plain(off)?;
                        let stored = match &v { CV::Int(x) => self.interval(x), _ => None };
                        self.field_store_seen(off, stored);
                        (format!("{}ULL", off), plain_code(&e).map_or(0, |x| x.0), len, 1)
                    }
                    _ => return no("`fill` works on a fixed array"),
                };
                let esize = scalar_bytes(code);
                let w = self.tmp("w");
                self.open("{");
                let bits = match (code, &v) {
                    (1..=8, CV::Int(e)) => {
                        self.line(&format!("i64 {} = {};", w, e));
                        let (lo, hi): (Option<&str>, Option<&str>) = match code {
                            1 => (Some("-128"), Some("127")), 2 => (Some("-32768"), Some("32767")), 3 => (Some("(-2147483647LL - 1)"), Some("2147483647")),
                            4 => (None, None), 5 => (Some("0"), Some("255")), 6 => (Some("0"), Some("65535")), 7 => (Some("0"), Some("4294967295LL")), _ => (Some("0"), None),
                        };
                        let cond = match (lo, hi) { (Some(l), Some(h)) => Some(format!("{w} < {l} || {w} > {h}")), (Some(l), None) => Some(format!("{w} < {l}")), _ => None };
                        if let Some(cond) = cond { self.line(&format!("if (UNLIKELY({})) FAIL(413, \"value does not fit the array's elements\", 39);", cond)); }
                        w.clone()
                    }
                    (11, CV::Bool(e)) => format!("(({}) ? 1 : 0)", e),
                    (10, CV::Flt(e)) => format!("BITS_F64({})", e),
                    _ => return no("`fill` stores an integer, bool or f64 matching the array"),
                };
                self.line(&format!("if (UNLIKELY({f} < 0 || {f} > {len}LL || {n} < 0 || {n} > {len}LL || {f} + {n} > {len}LL)) FAIL(400, \"fill out of range\", 17);"));
                self.call(&format!("c->h->fill_elems(c, {}, {}, {}, {}, {}, {})", base, esize, f, n, bits, plain));
                self.close("}");
            }
            Stmt::Clear(coll) => {
                let v = self.expr(coll, env, None)?;
                if let CV::PlainStr(_) | CV::StateStr(..) = &v {
                    // `s.clear()`: the text becomes empty
                    match &v { CV::PlainStr(off) => self.may_write_plain(*off)?, CV::StateStr(c, _, ci) => { let p = self.prog.colls[*c].cols[*ci].path.clone(); self.may_write_col(*c, &p)? } _ => {} }
                    let at = self.len_at(&v);
                    self.call(&format!("c->h->wr_int(c, {}, 4, 0)", at));
                    return Ok(());
                }
                let CV::Coll(c, inst) = v else { return no("`clear` is a function of a collection or text") };
                if let Some(t) = tab(c) { self.call(&format!("c->h->tclear(c, {}, (u32)({}))", t, inst)); return Ok(()); } // every row (of this owner), and the rows they own
                self.may_add_or_free(c, "clear")?;
                self.call(&format!("c->h->clear_coll(c, {}, {})", c, inst));
            }
            Stmt::Remove(coll, e) => {
                let CV::Coll(want, _) = self.expr(coll, env, None)? else { return no("`remove` is a function of a collection") };
                let wname = coll_name(self.prog, want);
                let other = |c: usize| no(format!("an item of {} cannot be removed from {}", coll_name(self.prog, c), wname));
                match self.expr(e, env, None)? {
                    CV::Item(c, i) if tab(c).is_some() => { if c != want { return no(format!("a row of {} cannot be removed from {}", coll_name(self.prog, c), coll_name(self.prog, want))); } self.call(&format!("c->h->tdel(c, {}, {})", c - TBASE, i)) }
                    CV::Item(c, i) => { if c != want { return other(c); } self.may_add_or_free(c, "remove")?; self.call(&format!("c->h->free_(c, {}, {})", c, i)) }
                    CV::Ref(..) => return no("removing a value that may be null: check it first with `if exists(x) {{ ... }}` (inside, x is the item), or give a fallback with `or(...)`"),
                    CV::Null => {}
                    CV::Any(x) => {
                        // a value whose kind is known only at run time: it must be an item of this collection
                        self.may_add_or_free(want, "remove")?;
                        self.line(&format!("if ({}.tag == 5 && {}.a != {}) FAIL(500, {}, {});", x, x, want, c_lit(format!("not an item of {}", wname).as_bytes()), format!("not an item of {}", wname).len()));
                        self.call(&format!("c->h->free_val(c, &{})", x))
                    }
                    _ => self.fail(500, "remove needs an item"),
                }
            }
            Stmt::Fail(code, m) => self.fail(*code, m),
            Stmt::Respond(303, Json::E(e)) => {
                // `see_other(path)`: from any endpoint, whatever its Response<T>; the runtime checks the path is on this server
                let v = self.expr(e, env, None)?;
                if let CV::Lit(s) = &v {
                    let b = s.as_bytes();
                    if !(b.first() == Some(&b'/') && !matches!(b.get(1), Some(b'/' | b'\\')) && b.iter().all(|c| *c > 0x20 && *c < 0x7F)) {
                        return no(format!("`see_other(\"{}\")`: the address is a path on this server, one leading `/`, printable ASCII, no spaces (encode them as %20)", s));
                    }
                }
                let t = self.txt_of(v).map_err(|_| "`see_other` takes the path as text, as in `see_other(\"/active\")`".to_string())?;
                if let Some((kind, _)) = text_answer(&self.ep.resp) { self.call(&format!("c->h->answer(c, {})", kind)); }
                self.call(&format!("c->h->see_other(c, &{})", t));
                self.line("return 303;");
            }
            Stmt::Respond(302, Json::E(e)) => {
                // `found(path)`: a 302 from any endpoint, its Location the path made absolute with the request's Host (checked by the runtime)
                let v = self.expr(e, env, None)?;
                if let CV::Lit(s) = &v {
                    let b = s.as_bytes();
                    if !(b.first() == Some(&b'/') && !matches!(b.get(1), Some(b'/' | b'\\')) && b.iter().all(|c| *c > 0x20 && *c < 0x7F)) {
                        return no(format!("`found(\"{}\")`: the address is a path on this server, one leading `/`, printable ASCII, no spaces (encode them as %20)", s));
                    }
                }
                let t = self.txt_of(v).map_err(|_| "`found` takes the path as text, as in `found(\"/\")`".to_string())?;
                // the answer's content type is the endpoint's (an HTML endpoint's redirect says text/html, as Rails')
                if let Some((kind, _)) = text_answer(&self.ep.resp) { self.call(&format!("c->h->answer(c, {})", kind)); }
                self.call(&format!("c->h->found(c, &{})", t));
                self.line("return 302;");
            }
            Stmt::Respond(code, Json::Html) => {
                // `ok(<markup>)`: the statements before wrote the body
                if !self.html_answer() { return no("markup is an HTML answer: the endpoint returns `Response<html>`"); }
                self.call("c->h->answer(c, 1)");
                self.line(&format!("return {};", code));
            }
            Stmt::Respond(code, Json::E(e)) if text_answer(&self.ep.resp).is_some() => {
                // an answer of text (HTML, plain text, CSS, JavaScript, SVG): the text sent as it is; one that is a literal (as
                // `file("app.css")`) is the same every time, so it gets an ETag and a browser that has it is answered 304
                let (kind, what) = text_answer(&self.ep.resp).unwrap();
                let v = self.expr(e, env, None)?;
                self.call(&format!("c->h->answer(c, {})", kind));
                if let CV::Lit(s) = &v {
                    let tag = format!("\"{:016x}\"", fnv64(s.as_bytes()));
                    let nm = self.tmp("nm");
                    self.line(&format!("int {} = 0;", nm));
                    self.call(&format!("c->h->etag(c, {}, {}, &{})", c_lit(tag.as_bytes()), tag.len(), nm));
                    self.line(&format!("if ({}) return 304;", nm));
                }
                self.raw(v, what)?;
                self.line(&format!("return {};", code));
            }
            Stmt::Respond(code, j) => {
                let resp = self.ep.resp.clone();
                if text_answer(&resp).is_some() { return no(format!("a Response<{}> answer is text: `ok(page.html)`, `ok(\"...\")`, `ok(file(\"app.css\"))`", show_rtype(&resp))); }
                let shape = self.json(j, env, Some(&resp))?;
                if let Err(e) = fits_shape(self.prog, &shape, &self.ep.resp, "") { return no(format!("the answer is {}, not Response<{}>: {}", shape, show_rtype(&self.ep.resp), e)); }
                self.line(&format!("return {};", code));
            }
            Stmt::Return(e) if self.ret.is_empty() && self.test => {
                // a test's result: `true` passes, `false` fails, a text fails with it as the message (empty: passes)
                let v = match e { Some(e) => self.expr(e, env, None)?, None => CV::Bool("1".into()) };
                match v {
                    CV::Bool(b) => { self.open(&format!("if (!({})) {{", b)); let t = self.txt_of(CV::Lit("returned false".into()))?; self.line(&format!("TEST_FAIL_({});", t)); self.close("}"); }
                    v @ (CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..)) => { let t = self.txt_of(v)?; self.line(&format!("if ({}.present && {}.len > 0) TEST_FAIL_({});", t, t, t)); }
                    _ => return no("a test returns `true` or `false`, or a text (empty when it passed, otherwise what went wrong)"),
                }
                self.line("return 0;");
            }
            Stmt::Return(e) => {
                if self.ret.is_empty() { if let Some(e) = e { self.expr(e, env, None)?; } self.line("return 0;"); return Ok(()); }
                let v = match e { Some(e) => self.expr(e, env, None)?, None => CV::Null };
                self.ret_with(v)?;
            }
            Stmt::Forward { kind, key, fields } => {
                self.call(&format!("c->h->fwd_begin(c, {}, {})", c_lit(kind.as_bytes()), kind.len()));
                let kv = self.expr(key, env, None)?;
                let k = self.int_or(kv, 500, "a forward key must be an integer");
                for (name, e) in fields {
                    let v = self.expr(e, env, None)?;
                    let vv = self.val(v)?;
                    self.call(&format!("c->h->fwd_put(c, {}, {}, &{})", c_lit(name.as_bytes()), name.len(), vv));
                }
                self.line(&format!("TAIL(c->h->forward(c, {}, {}, {}));", c_lit(kind.as_bytes()), kind.len(), k));
            }
            Stmt::Do(e) => { self.expr(e, env, None)?; }
            Stmt::Alloc { slot, coll, try_, .. } => {
                let CV::Coll(c, inst) = self.expr(coll, env, None)? else { return no("`add` is a function of a collection") };
                if let Some(t) = tab(c) {
                    // a new row (a table has no capacity, so `try_add` always gives one)
                    let r = self.tmp("i");
                    self.line(&format!("u32 {};", r));
                    self.call(&format!("c->h->tadd(c, {}, (u32)({}), &{})", t, inst, r));
                    env[*slot as usize] = Some(CV::Item(c, r));
                    return Ok(());
                }
                self.may_add_or_free(c, "add")?;
                let t = self.tmp("i");
                self.line(&format!("u32 {};", t));
                let coll = &self.prog.colls[c];
                if matches!(coll.kind, Kind::Pool(false)) && coll.mult == 1 && coll.children.is_empty() && coll.unlogged {
                    // the common pool (one instance, no generations, no nested collections) kept out of the undo log: adding at the high-water mark, with nothing freed below it, is done here
                    // with the layout as constants, as the runtime does it for any pool; anything else (a freed slot to reuse, a full pool) is left to the runtime's helper
                    let (hdr, live, cap) = (coll.hdr, coll.live, coll.cap);
                    let cols: Vec<(usize, usize)> = coll.cols.iter().filter(|x| !matches!(x.kind, CKind::StrBytes(_))).map(|x| (x.off, x.kind.size())).collect();
                    let (h0, h1, h2, h3, sl, wa, wd) = (self.tmp("h"), self.tmp("h"), self.tmp("h"), self.tmp("h"), self.tmp("s"), self.tmp("a"), self.tmp("w"));
                    self.open("{");
                    self.line(&format!("u32 {h0} = LD_U32({hdr}, 0), {h1} = LD_U32({}, 0), {h2} = LD_U32({}, 0), {h3} = LD_U32({}, 0);", hdr + 4, hdr + 8, hdr + 12));
                    self.line(&format!("if (UNLIKELY(!({h2} == {h0} && {h0} < {cap}u))) {{"));
                    self.depth += 1;
                    self.call(&format!("c->h->alloc(c, {}, {}, {}, &{})", c, inst, *try_ as i32, t));
                    self.depth -= 1;
                    self.line("} else {");
                    self.depth += 1;
                    self.line(&format!("u64 {sl} = {h0};"));
                    self.line(&format!("u64 {wa} = {live}ULL + 8 * ({sl} / 64);"));
                    self.line(&format!("u64 {wd} = {sl} % 64 ? (u64)LD_U64({wa}, 0) : 0; // a word wholly above the high-water mark may be uncommitted: its bits are zero"));
                    self.line(&format!("COMMIT_({wa}, 8);"));
                    self.line(&format!("ST8_({wa}, {wd} | (1ULL << ({sl} % 64)));"));
                    for (off, size) in &cols {
                        self.line(&format!("COMMIT_({off}ULL + {sl} * {size}ULL, {size});"));
                        self.line(&format!("if ({sl} < {h3}) ZERO_({off}ULL + {sl} * {size}ULL, {size}); // a reused slot is cleared; a never-used one is zero already"));
                    }
                    self.line(&format!("COMMIT_({hdr}ULL, 16);"));
                    self.line(&format!("ST4_({hdr}ULL, {h0} + 1); ST4_({}ULL, {h1} + 1); ST4_({}ULL, {h0} + 1); ST4_({}ULL, {h3} > {h0} + 1 ? {h3} : {h0} + 1);", hdr + 4, hdr + 8, hdr + 12));
                    self.line(&format!("{t} = (u32){sl};"));
                    self.depth -= 1;
                    self.line("}");
                    self.close("}");
                } else {
                    self.call(&format!("c->h->alloc(c, {}, {}, {}, &{})", c, inst, *try_ as i32, t));
                }
                env[*slot as usize] = Some(if *try_ {
                    // try_add gives an item or null (a full collection): an item that may be null, narrowed by `if exists(x)` or
                    // `if !exists(x) { fail ...; }` as any other
                    let r = self.tmp("r");
                    self.line(&format!("u32 {} = {} != 0xFFFFFFFFu;", r, t));
                    CV::Ref(c, r, t)
                } else { CV::Item(c, t) });
            }
        }
        Ok(())
    }

    pub(super) fn ret_with(&mut self, v: CV) -> R<()> {
        // a struct goes straight into the destination the call was given
        if let Some((sid, off)) = self.ret.last().and_then(|r| r.dest) {
            let f = *self.inlining.last().unwrap();
            let CV::Plain(s2, src) = v else { return no(format!("`{}` returns a `{}`: return a struct of that type", self.api.fns[f as usize].name, self.prog.structs[sid].name)) };
            if s2 != sid { return no(format!("`{}` returns a `{}`, not a `{}`", self.api.fns[f as usize].name, self.prog.structs[sid].name, self.prog.structs[s2].name)); }
            self.copy_struct(sid, off, src)?;
            let (m, label) = { let r = self.ret.last().unwrap(); (r.mark.clone(), r.label.clone()) };
            self.line(&format!("/*RCUT {}*/", m));
            self.line(&format!("goto {};", label));
            return Ok(());
        }
        let b = self.txt_bound(&v);
        if let Some(r) = self.ret.last_mut() { match b { Some(b) => r.bound = Some(r.bound.unwrap_or(0).max(b)), None => r.unbounded = true } }
        let v = self.hold(v, false)?;
        // what the call can give, for the range analysis in the caller: the union of the ranges of every value returned
        let iv = match &v { CV::Int(e) => self.interval(e), _ => None };
        if let Some(r) = self.ret.last_mut() { r.ivs.push(iv); }
        let Some(kv) = kind(&v) else { return no("a function returning this kind of value is not compiled yet") };
        let r = self.ret.last().unwrap();
        let (var, label, f) = (r.var.clone(), r.label.clone(), *self.inlining.last().unwrap());
        let k = match r.kind { None => { self.ret.last_mut().unwrap().kind = Some(kv); kv } Some(k) => k };
        let declared = self.api.fns[f as usize].ret.clone();
        if let Some(t) = &declared {
            let what = format!("the result of `{}`", self.api.fns[f as usize].name);
            let fname = self.api.fns[f as usize].name.clone();
            let v2 = self.to_declared(v.clone(), t, &fname, &what)?;
            if let (CV::Flt(_), CV::Int(_)) = (&v2, &v) { return self.ret_with(v2); }
        } else if self.api.fns[f as usize].ptys.iter().any(|p| p.is_some()) && kv != K::Null {
            return no(format!("`{}` returns a value: declare its result, `fn {}(...) -> T`", self.api.fns[f as usize].name, self.api.fns[f as usize].name));
        }
        if !fits(kv, k) {
            if unify(k, kv) == K::Any { why(format!("`{}` returns {} and {}: a function returns one kind", self.api.fns[f as usize].name, kname(self.prog, k), kname(self.prog, kv))); }
            self.widen = Some(Widen::Ret(f, unify(k, kv))); return no("a function returns values of different kinds");
        }
        let v = self.coerce(v, k, false)?;
        if self.ret.last().unwrap().func {
            match &v {
                CV::Int(e) | CV::Flt(e) | CV::Bool(e) | CV::Txt(e) | CV::Any(e) | CV::Item(_, e) => self.line(&format!("*ret_v = {};", e)),
                CV::Coll(_, e) => self.line(&format!("*ret_v = (u32)({});", e)),
                CV::Ref(_, s, i) => self.line(&format!("*ret_s = {}; *ret_v = {};", s, i)),
                CV::Null | CV::Plain(..) => {}
                _ => return no("a function returning this kind of value is not compiled yet"),
            }
            if let Some(h) = self.ret.last().unwrap().sel.clone() { self.call(&format!("c->h->sel_end(c, {})", h)); }
            if !matches!(k, K::Txt | K::Any) { let m = self.ret.last().unwrap().mark.clone(); self.line(&format!("/*RCUT {}*/", m)); }
            self.line("return 0;");
            return Ok(());
        }
        let assign = match &v {
            CV::Int(e) | CV::Flt(e) | CV::Bool(e) | CV::Txt(e) | CV::Any(e) | CV::Item(_, e) => format!("{}_v = {};", var, e),
            CV::Coll(_, e) => format!("{}_v = (u32)({});", var, e),
            CV::Ref(_, s, k) => format!("{}_s = {}; {}_v = {};", var, s, var, k),
            CV::Null | CV::Plain(..) => String::new(),
            _ => return no("a function returning this kind of value is not compiled yet"),
        };
        if !assign.is_empty() { self.line(&assign); }
        if !matches!(k, K::Txt | K::Any) { let m = self.ret.last().unwrap().mark.clone(); self.line(&format!("/*RCUT {}*/", m)); }
        self.line(&format!("goto {};", label));
        Ok(())
    }

    pub(super) fn slot_kind(&self, slot: u32) -> Option<K> { self.slot_kinds.iter().find(|x| x.0 == (self.frame, slot)).map(|x| x.1) }

    /// Convert a value to kind `k` (which it fits), in its own variable if `mutable`.
    pub(super) fn coerce(&mut self, v: CV, k: K, mutable: bool) -> R<CV> {
        let held = self.coerce_value(v, k, mutable)?;
        if mutable {
            match &held {
                CV::Int(n) | CV::Flt(n) | CV::Bool(n) | CV::Txt(n) | CV::Any(n) | CV::Item(_, n) => { self.var_names.insert(n.clone()); }
                CV::Ref(_, s, i) => { self.var_names.insert(s.clone()); self.var_names.insert(i.clone()); }
                _ => {}
            }
        }
        Ok(held)
    }
    pub(super) fn coerce_value(&mut self, v: CV, k: K, mutable: bool) -> R<CV> {
        Ok(match (k, v) {
            (K::Any, v) => { let x = self.val(v)?; if mutable || self.var_names.contains(&x) { let t = self.tmp("v"); self.line(&format!("Val {} = {};", t, x)); CV::Any(t) } else { CV::Any(x) } }
            (K::Txt, CV::Null) => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ 0, 0, 0 }};", t)); CV::Txt(t) }
            (K::Txt, v @ (CV::Txt(_) | CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..))) => { let x = self.txt_of(v)?; if mutable || self.var_names.contains(&x) { let t = self.tmp("t"); self.line(&format!("Txt {} = {};", t, x)); CV::Txt(t) } else { CV::Txt(x) } }
            (K::Ref(c), CV::Null) => { let (s, i) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = 0, {} = 0;", s, i)); CV::Ref(c, s, i) }
            (K::Ref(c), CV::Item(_, e)) => { let (s, i) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = 1, {} = {};", s, i, e)); CV::Ref(c, s, i) }
            (K::Ref(_), v @ CV::Ref(..)) => self.hold(v, mutable)?,
            (_, v) => self.hold(v, mutable)?,
        })
    }

}
