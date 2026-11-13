//! Inlining, places, call values, loops, enums, JSON output and writes of G.
use super::*;

impl<'p> G<'p> {
/// Inline a user function: its parameters become values in a fresh frame; `return` jumps to the end of the inlined block.
    pub(super) fn inline(&mut self, k: u32, args: &'p [Expr], env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        let mut vals = Vec::new();
        // where a struct this call returns goes is meant for the call, not for a call among its arguments
        let ctx = self.ret_ctx.take();
        let mut places: Vec<Option<WPlace<'p>>> = Vec::new();
        for (j, a) in args.iter().enumerate() {
            // a `write` parameter of a number or bool takes a place, which is not evaluated to a value here
            if scalar_write_at(k, j) { let (v, pl) = self.write_place(a, k, j, env, dead)?; vals.push(v); places.push(Some(pl)); }
            else { let v = self.expr(a, env, dead)?; vals.push(self.hold(v, false)?); places.push(None); }
        }
        self.ret_ctx = ctx;
        self.wargs = places;
        self.call_values(k, vals, dead)
    }

    /// The place an argument names for `write` parameter `j` of callee `k`: a `var` local, or a field or element of the state or of an item. Returns the place's value now, and the
    /// place. Everything in the place that could change before the callee stores (an item's number, an index) is copied into a variable of its own here.
    pub(super) fn write_place(&mut self, a: &'p Expr, k: u32, j: usize, env: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<(CV, WPlace<'p>)> {
        let f = &self.api.fns[k as usize];
        let (fname, pname) = (f.name.clone(), f.params[j].clone());
        let pty = f.ptys[j].as_ref().ok_or("a `write` parameter has a type")?;
        let want = self.decl_kind(pty, &fname)?;
        let bad = |what: &str| format!("`{}: write` in `{}`: `write` needs a place to write to (a `var`, a field of the state or of an item, or an element of an array), not {}", pname, fname, what);
        let same_kind = |g: &Self, v: &CV| -> R<()> {
            if kind(v) == Some(want) { Ok(()) } else { no(format!("`{}: write {}` in `{}` takes a place of its own type, given {}", pname, pty, fname, type_text(g.prog, v))) }
        };
        match a {
            Expr::Var(n, VarRes::Local(slot)) => {
                // a `write` parameter of this function handed on: the same place
                if let Some(w) = self.wslots.get(&(self.frame, *slot)).cloned() {
                    let v = self.expr(a, env, dead)?;
                    same_kind(self, &v)?;
                    return Ok((v, w));
                }
                let v = env.get(*slot as usize).cloned().flatten();
                let name = match &v { Some(CV::Int(x)) | Some(CV::Flt(x)) | Some(CV::Bool(x)) => x.clone(), _ => return no(bad(&format!("`{}`", n))) };
                if !self.var_names.contains(&name) { return no(bad(&format!("`{}`, which is a `let` or a parameter (declare it with `var` to change it)", n))); }
                if self.u32_vars.contains(&name) { return no(format!("`{}: write` in `{}`: this `var` is kept in 32 bits: not supported yet", pname, fname)); }
                let v = v.unwrap();
                same_kind(self, &v)?;
                Ok((v, WPlace { shape: WShape::Var(name, Some(*slot)), hidden: Vec::new(), hb: 0, loc: None }))
            }
            Expr::Field(base, fld) => {
                self.check_assign_target(a, env)?;
                let bv = self.expr(base, env, dead)?;
                let bv = self.freeze_place(bv, &bad)?;
                let pl = WPlace { shape: WShape::Field(fld.clone()), hidden: vec![bv], hb: 0, loc: None };
                let v = self.read_place(&pl, dead)?;
                same_kind(self, &v)?;
                Ok((v, pl))
            }
            Expr::Index(base, k_expr) => {
                self.check_assign_target(a, env)?;
                let bv = self.expr(base, env, dead)?;
                if !matches!(bv, CV::ArrCol(..) | CV::PlainArr(_, Ty::Scalar(_), _)) { return no(bad("an element of something that is not an array of numbers or bool")); }
                let kv = self.expr(k_expr, env, dead)?;
                let kk = self.int_or(kv, 400, "an index must be an integer");
                let bv = self.freeze_place(bv, &bad)?;
                let kt = self.tmp("k");
                self.line(&format!("i64 {} = {};", kt, kk));
                let pl = WPlace { shape: WShape::Index, hidden: vec![bv, CV::Int(kt)], hb: 0, loc: None };
                let v = self.read_place(&pl, dead)?;
                same_kind(self, &v)?;
                Ok((v, pl))
            }
            Expr::Var(..) => no(bad("a state variable of its own (give it a field of a struct)")),
            _ => no(bad("a literal or a computed value")),
        }
    }
    /// The names a place holds, copied into variables of their own (an item's number, an element's index), so nothing assigned later moves it.
    pub(super) fn freeze_place(&mut self, bv: CV, bad: &dyn Fn(&str) -> String) -> R<CV> {
        let copy = |g: &mut Self, ty: &str, e: &str| { let t = g.tmp("w"); g.line(&format!("{} {} = {};", ty, t, e)); t };
        Ok(match bv {
            CV::Item(c, idx) => CV::Item(c, copy(self, "u32", &idx)),
            CV::Part(c, idx, p) => CV::Part(c, copy(self, "u32", &idx), p),
            CV::Elem(c, idx, p, k) => { let i2 = copy(self, "u32", &idx); CV::Elem(c, i2, p, copy(self, "i64", &k)) }
            CV::ArrCol(c, idx, ci) => CV::ArrCol(c, copy(self, "u32", &idx), ci),
            b @ (CV::Plain(..) | CV::PlainArr(..)) => b,
            CV::Ref(..) => return no(bad("a field of a value that may be null: check it first with `if exists(x)`")),
            _ => return no(bad("a field or element of something else")),
        })
    }
    /// The expression that names `pl`, written over the slots `hb` (the base) and `hb + 1` (the index).
    pub(super) fn place_loc(pl: &WPlace<'p>, hb: u32) -> &'p Expr {
        let base = Expr::Var("__w".into(), VarRes::Local(hb));
        let e = match &pl.shape {
            WShape::Field(f) => Expr::Field(Box::new(base), f.clone()),
            WShape::Index => Expr::Index(Box::new(base), Box::new(Expr::Var("__wk".into(), VarRes::Local(hb + 1)))),
            WShape::Var(..) => base,
        };
        Box::leak(Box::new(e))
    }
    /// The value now in a field or element place (not yet bound to a callee).
    pub(super) fn read_place(&mut self, pl: &WPlace<'p>, dead: Option<&str>) -> R<CV> {
        let loc = Self::place_loc(pl, 0);
        let mut tmp: Vec<Option<CV>> = pl.hidden.iter().cloned().map(Some).collect();
        self.expr(loc, &mut tmp, dead)
    }
    /// Assigning `write` parameter `slot` (a place of the caller's): the value is checked as the parameter's declared type, then stored the way an assignment to that place is (a `var`: its
    /// own C variable; a field or element: the same code as `field = value`, with its range and type checks, undo log and whole-program facts).
    pub(super) fn write_param_assign(&mut self, w: WPlace<'p>, target: &'p Expr, slot: u32, rhs: &'p Expr, env: &mut Vec<Option<CV>>, rest: &'p [Stmt]) -> R<()> {
        let k = self.frame.ok_or("a `write` parameter outside a function")?;
        let f = &self.api.fns[k as usize];
        let (fname, pname) = (f.name.clone(), f.params[slot as usize].clone());
        let pty = f.ptys[slot as usize].as_ref().ok_or("a `write` parameter has a type")?;
        let v = self.expr(rhs, env, None)?;
        let mut v = self.to_declared(v, pty, &fname, &format!("`{}`", pname))?;
        let range = match pty.name.as_str() { "i8" => Some((-128, 127)), "i16" => Some((-32768, 32767)), "i32" => Some((i32::MIN as i64, i32::MAX as i64)), "u8" => Some((0, 255)), "u16" => Some((0, 65535)), "u32" => Some((0, u32::MAX as i64)), "u64" => Some((0, i64::MAX)), _ => None };
        if let (CV::Int(_), Some(r)) = (&v, range) { v = self.in_field_range(v, r, &pname)?; }
        let v = self.hold(v, false)?;
        let need = (w.hb + 3) as usize;
        if env.len() < need { env.resize(need, None); }
        env[(w.hb + 2) as usize] = Some(v);
        let val = Expr::Var("__wv".into(), VarRes::Local(w.hb + 2));
        let stmt: &'p Stmt = match (&w.shape, w.loc) {
            (WShape::Var(..), _) => { self.wskip = true; Box::leak(Box::new(Stmt::Assign(target.clone(), val))) }
            (_, Some(loc)) => Box::leak(Box::new(Stmt::Assign(loc.clone(), val))),
            _ => return no("a `write` parameter with no place"),
        };
        self.rest = rest;
        self.stmt_inner(stmt, env)
    }

    /// Call user function `k` with argument values already computed: inlined, or a generated C function if recursive.
    pub(super) fn call_values(&mut self, k: u32, vals: Vec<CV>, dead: Option<&str>) -> R<CV> {
        self.pending_ctx = self.ret_ctx.take();
        let places = std::mem::take(&mut self.wargs);
        let f = &self.api.fns[k as usize];
        if vals.len() != f.params.len() {
            // a function of a type is called `p.hit(1)`: the receiver is not counted among its arguments
            let (shown, skip) = (f.name.clone(), if f.recv { 1 } else { 0 });
            let (want, given) = (f.params.len() - skip, vals.len().saturating_sub(skip));
            return no(format!("`{}` takes {} argument{}, given {}", shown, want, if want == 1 { "" } else { "s" }, given));
        }
        if let Some(t) = &f.ret { self.ret_kind(t, &f.name)?; } // (a one-line function never reaches its result kind otherwise)
        let mut checked = Vec::with_capacity(vals.len());
        for (i, v) in vals.into_iter().enumerate() {
            let v = match &f.ptys[i] { Some(t) => self.to_declared(v, t, &f.name, &format!("parameter `{}` of `{}`", f.params[i], f.name))?, None => v };
            // a struct parameter is read-only unless declared `write` (it may change the caller's struct, and may be handed to other `write` parameters) or `copy` (the function gets a copy
            // of its own, which it may change). A read-only one is passed as it is, which no one can tell from a copy, unless the function writes something else (it might write the very
            // place the argument is): then the copy is made.
            let mode = f.modes.get(i).copied().unwrap_or(0);
            if mode != 0 {
                let (word, pn) = ("write", f.params[i].as_str());
                let why = match &v {
                    CV::Plain(..) => None,
                    CV::Int(_) | CV::Flt(_) | CV::Bool(_) if places.get(i).map_or(false, |p| p.is_some()) => None, // a place the caller named (see `write_place`)
                    CV::Int(_) | CV::Flt(_) | CV::Bool(_) => Some("`write` needs a place to write to (a `var`, a field of the state or of an item, or an element of an array), not a computed value (and a function called as `x.f(...)` cannot take one yet)".to_string()),
                    CV::StateStr(..) | CV::PlainStr(..) if mode == 1 => None,
                    CV::Lit(_) | CV::Txt(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) if mode == 1 => Some("`write` needs a place to write to (a text field of the state or of an item), not a literal or computed text (a `var` text local is not supported yet)".to_string()),
                    CV::Item(..) | CV::Ref(..) => None, // an item is already a handle to its row: `write` is allowed (so one signature serves a state struct and an item) and changes nothing
                    _ => Some("only a plain struct (with its text and array fields) can be a `write` parameter".to_string()),
                };
                if let Some(why) = why { return no(format!("`{}: {}` in `{}`: {}", pn, word, f.name, why)); }
            }
            if let CV::Plain(sid, _) = v {
                if mode == 0 && f.test.is_none() {
                    if param_written(self.api, &f.body, i as u32) {
                        let ty = self.prog.structs[sid].name.clone();
                        return no(format!("`{}` is read-only here, and this changes it (or hands it to a `write` parameter): declare `{}: write {}` to change the caller's struct, or work on `var c = {} {{ ..{} }}`", f.params[i], f.params[i], ty, ty, f.params[i]));
                    }
                }
            }
            checked.push(v);
        }
        let vals = checked;
        let args: Vec<String> = vals.iter().map(|v| type_text(self.prog, v)).collect();
        let r = self.call_values_inner(k, vals, dead, places);
        self.pending_ctx = None;
        let r = r?;
        let rt = if returns_value(&self.api.fns[k as usize].body) { type_text(self.prog, &r) } else { "-".into() };
        SIGS.with(|s| s.borrow_mut().push((k, args, rt)));
        let (opt, name) = (self.api.fns[k as usize].ret_opt, self.api.fns[k as usize].name.clone());
        let gives = returns_value(&self.api.fns[k as usize].body); // a function without `return value` gives nothing, which is not null
        Ok(match r {
            // (a recursive function's result reads as null until its kind is known: only a definite item-or-null counts there)
            CV::Ref(..) if !opt && gives => return no(format!("`{}` can return null (it returns `null`, or a value that may be null, on some path): declare it `fn {}(...) -> T?` so that callers check it", name, name)),
            CV::Null if !opt && gives && !self.recursive[k as usize] => return no(format!("`{}` can return null (it returns `null`, or a value that may be null, on some path): declare it `fn {}(...) -> T?` so that callers check it", name, name)),
            CV::Item(c, e) if opt => { let s = self.tmp("r"); self.line(&format!("u32 {} = 1;", s)); CV::Ref(c, s, e) }
            r => r,
        })
    }
    pub(super) fn call_values_inner(&mut self, k: u32, vals: Vec<CV>, dead: Option<&str>, places: Vec<Option<WPlace<'p>>>) -> R<CV> {
        let f = &self.api.fns[k as usize];
        self.depth_check();
        let has_places = places.iter().any(|p| p.is_some());
        // a function that returns a struct is always inlined (its result is written into the caller's destination)
        let returns_plain = matches!(f.ret.as_ref().map(|t| self.decl_kind(t, &f.name)), Some(Ok(K::Plain(..))));
        if self.recursive[k as usize] {
            if has_places { return no(format!("`{}` is recursive and has a `write` parameter of a number or bool: not supported yet (each call would need its own place)", f.name)); }
            if returns_plain { return no(format!("`{}` is recursive and returns a struct: not supported yet (a recursive function cannot hold struct values)", f.name)); }
            self.pending_ctx = None;
            return self.call_spec(k, vals);
        }
        // an outlined function: called, when its arguments are of kinds a C function takes and its body compiles as one; inlined otherwise
        if !returns_plain && !has_places && OUTLINED.with(|o| o.borrow().get(k as usize).copied().unwrap_or(false)) && vals.iter().all(spec_arg) {
            let (n0, body0, specs0) = (self.n, self.body.len(), self.specs.borrow().len());
            match self.call_spec(k, vals.clone()) {
                Ok(v) => return Ok(v),
                Err(_) => { self.n = n0; self.body.truncate(body0); self.specs.borrow_mut().truncate(specs0); self.widen = None; WHY.with(|w| w.borrow_mut().clear()); }
            }
        }
        let mut fenv: Vec<Option<CV>> = vec![None; f.slots as usize];
        for (i, v) in vals.into_iter().enumerate() { fenv[i] = Some(v); }
        // a `write` parameter of a number or bool: a `var` of the caller is shared as it is; a field or element is read and assigned through its place, kept in the function's hidden slots
        let mut bound: Vec<u32> = Vec::new();
        let mut shared_vars: Vec<(String, Option<u32>)> = Vec::new();
        if has_places {
            let mut hb = f.slots;
            fenv.resize((f.slots as usize) + 3 * places.iter().filter(|p| p.is_some()).count(), None);
            for (i, pl) in places.into_iter().enumerate() {
                let Some(mut pl) = pl else { continue };
                pl.hb = hb;
                match &pl.shape {
                    WShape::Var(name, cslot) => shared_vars.push((name.clone(), *cslot)),
                    _ => {
                        fenv[i] = None; // read through the place on every use
                        for (h, v) in pl.hidden.iter().enumerate() { fenv[hb as usize + h] = Some(v.clone()); }
                        pl.loc = Some(Self::place_loc(&pl, hb));
                    }
                }
                self.wslots.insert((Some(k), i as u32), pl);
                bound.push(i as u32);
                hb += 3;
            }
        }
        self.inl += 1;
        let r = self.inline_body(k, f, &mut fenv, dead);
        self.inl -= 1;
        for i in bound { self.wslots.remove(&(Some(k), i)); }
        // the callee may have assigned a `var` of the caller: what was known of it is gone, in the straight code after the call and for the loops around it
        for (name, cslot) in shared_vars {
            self.var_iv.remove(&name);
            if let (Some(cs), Some(&d)) = (cslot, self.var_depth.get(&name)) {
                for fi in d..self.loop_frames.len() {
                    let key = (self.loop_frames[fi], cs);
                    LOOP_INV.with(|l| { l.borrow_mut().assigns.insert(key, None); });
                }
            }
        }
        r
    }

    /// An error inside a function names the function's statement, not the call (the endpoint is named beside it).
    pub(super) fn inline_body(&mut self, k: u32, f: &'p Func, fenv: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        let caller_at = self.at;
        if let Some(Stmt::At(l, c)) = f.body.first() { self.at = (*l, *c); }
        let r = self.inline_body_at(k, f, fenv, dead).map_err(|e| place_err(self.at, e));
        self.at = caller_at;
        r
    }
    pub(super) fn inline_body_at(&mut self, k: u32, f: &'p Func, fenv: &mut Vec<Option<CV>>, dead: Option<&str>) -> R<CV> {
        // a function that returns a struct writes it into the place the call was given (`target = f(x);`: the target itself; `let v = f(x);`: a new place that `v` names)
        let ctx = self.pending_ctx.take();
        let struct_dest = match f.ret.as_ref().map(|t| self.decl_kind(t, &f.name)) {
            Some(Ok(K::Plain(sid, _))) => match ctx {
                None => {
                    let short = f.name.rsplit('.').next().unwrap_or(&f.name).to_string();
                    return no(format!("`{}` returns a struct, which has to go somewhere: write `target = {}(...);`, `let x = {}(...);` or `var x = {}(...);` (name the middle of a chain: `let t = a.plus(b); world.pos = t.scaled(2.0);`)", f.name, short, short, short));
                }
                Some(Some(off)) => Some((sid, off)),
                Some(None) => { self.prog.value_check(sid)?; let off = self.scratch_alloc(self.prog.structs[sid].size)?; self.scratch_marker(sid, off); Some((sid, off)) }
            },
            _ => None,
        };
        // a function that is just `return e;` needs no block
        if let Some(Stmt::Return(Some(body))) = only_stmt(&f.body) {
            self.inlining.push(k);
            let r = self.expr(body, fenv, dead);
            self.inlining.pop();
            if let Some((sid, off)) = struct_dest {
                let CV::Plain(s2, src) = r? else { return no(format!("`{}` returns a `{}`: return a struct of that type", f.name, self.prog.structs[sid].name)) };
                if s2 != sid { return no(format!("`{}` returns a `{}`, not a `{}`", f.name, self.prog.structs[sid].name, self.prog.structs[s2].name)); }
                self.copy_struct(sid, off, src)?;
                return Ok(CV::Plain(sid, off));
            }
            return r;
        }
        let (var, label) = (self.tmp("ret"), self.tmp("end"));
        let saved = std::mem::take(&mut self.body);
        self.inlining.push(k);
        let forced = match &f.ret { Some(_) if struct_dest.is_some() => struct_dest.map(|(s, o)| K::Plain(s, o)), Some(t) => Some(self.ret_kind(t, &f.name)?), None => self.ret_kinds.iter().find(|x| x.0 == k).map(|x| x.1) };
        let sel = if has_collecting_loop(self.api, &f.body) { Some(self.tmp("hm")) } else { None };
        let rmark = self.tmp("rm");
        self.ret.push(Ret { dest: struct_dest, var: var.clone(), label: label.clone(), kind: forced, func: false, sel: sel.clone(), bound: None, unbounded: false, mark: rmark.clone(), ivs: Vec::new() });
        self.depth += 1;
        let frame = std::mem::replace(&mut self.frame, Some(k));
        let r = self.block(&f.body, fenv);
        self.frame = frame;
        self.depth -= 1;
        let ret = self.ret.pop().unwrap();
        self.inlining.pop();
        let inner = std::mem::replace(&mut self.body, saved);
        r?;
        let inner = resolve_return_cuts(&inner, &ret.mark);
        let ends_by_itself = matches!(f.body.last(), Some(Stmt::Return(_)) | Some(Stmt::Fail(..)) | Some(Stmt::Respond(..)) | Some(Stmt::Forward { .. }));
        let kind = ret.kind.unwrap_or(K::Null);
        if !ends_by_itself && kind != K::Null {
            if let Some(t) = &f.ret { if !t.opt { return no(format!("`{}` can end without returning its `{}`: return a value on every path", f.name, t)); } }
            // falling off the end returns null, as in the interpreter
            if !fits(K::Null, kind) {
            if unify(kind, K::Null) == K::Any { why(format!("`{}` returns {} on one path and nothing on another: return a value on every path", self.api.fns[k as usize].name, kname(self.prog, kind))); }
            self.widen = Some(Widen::Ret(k, unify(kind, K::Null))); return no("a function that can end without returning its value");
        }
        }
        let (decl, out) = match kind {
            K::Int => {
                // every value returned had a range: the call's result is inside their union
                if !ret.ivs.is_empty() && ret.ivs.iter().all(|x| x.is_some()) {
                    let (lo, hi) = ret.ivs.iter().flatten().fold((i64::MAX, i64::MIN), |a, r| (a.0.min(r.0), a.1.max(r.1)));
                    self.ranges.insert(format!("{}_v", var), (lo, hi));
                }
                (format!("i64 {}_v = 0;", var), CV::Int(format!("{}_v", var)))
            }
            K::Flt => (format!("double {}_v = 0;", var), CV::Flt(format!("{}_v", var))),
            K::Bool => (format!("int {}_v = 0;", var), CV::Bool(format!("{}_v", var))),
            K::Txt => {
                // the most any of its returns gives
                if let (false, Some(b)) = (ret.unbounded, ret.bound) { self.txt_bounds.insert(format!("{}_v", var), b); }
                (format!("Txt {}_v = {{ 0, 0, 0 }};", var), CV::Txt(format!("{}_v", var)))
            }
            K::Any => (format!("Val {}_v = {{ 0, 0, 0, 0, 0 }};", var), CV::Any(format!("{}_v", var))),
            K::Item(c) => (format!("u32 {}_v = 0;", var), CV::Item(c, format!("{}_v", var))),
            K::Ref(c) => (format!("u32 {}_s = 0, {}_v = 0;", var, var), CV::Ref(c, format!("{}_s", var), format!("{}_v", var))),
            K::Coll(c) => (format!("u32 {}_v = 0;", var), CV::Coll(c, format!("{}_v", var))),
            K::Plain(sd, o) => (String::new(), CV::Plain(sd, o)),
            K::Null => (String::new(), CV::Null),
        };
        if !decl.is_empty() { self.line(&decl); }
        if let Some(h) = &ret.sel { self.line(&format!("u32 {};", h)); self.call(&format!("c->h->sel_mark(c, &{})", h)); }
        self.line("{");
        self.body.push_str(&inner);
        self.line("}");
        self.line(&format!("{}: ;", label));
        if let Some(h) = &ret.sel { self.call(&format!("c->h->sel_end(c, {})", h)); }
        Ok(out)
    }

    /// Emit a loop over a list or pool instance (with `reverse`, `where` and `limit`), running `body` with the item bound.
    /// With `collect`, the items passing `where` are collected before any body runs, then limited, as the interpreter's
    /// `select` does; otherwise filtering and the body run in one pass.
    /// A loop's labels for `break` and `continue` (placed only when used).
    pub(super) fn loop_enter(&mut self) { self.loop_n += 1; let (b, c) = (format!("brk_{}", self.loop_n), format!("cnt_{}", self.loop_n)); self.loops.push((b, c, false, false)); }
    /// The end of a round's body: where `continue` lands, unless a `for` loop's step placed it already.
    pub(super) fn loop_round_end(&mut self) { if let Some(l) = self.loops.last_mut() { if l.3 { l.3 = false; let t = format!("{}: ;", l.1); self.line(&t); } } }
    /// After the loop: where `break` lands.
    pub(super) fn loop_exit(&mut self) { if let Some(l) = self.loops.pop() { if l.2 { self.line(&format!("{}: ;", l.0)); } } }
    /// Each round: the index (`foreach item, i in ...`: the rounds before this one), and for a loop without a fixed extent its
    /// count against the request's step budget.
    pub(super) fn round_start(&mut self, it: &'p Iter, env: &mut Vec<Option<CV>>, hits: &str) {
        if let Some((_, slot)) = &it.index { let ix = self.tmp("ix"); self.line(&format!("i64 {} = (i64){} - 1;", ix, hits)); env[*slot as usize] = Some(CV::Int(ix)); }
    }

    /// `foreach p in split(t, sep)` (or `split(t)`, by whitespace): each piece a window onto the text, as Ruby's split gives them
    /// (h_split_next); `where` and `limit` as over a collection, `p, i` the piece's number.
    pub(super) fn split_loop(&mut self, it: &'p Iter, args: &'p [Expr], env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        if args.is_empty() || args.len() > 2 { return no("`split(text, separator)` or `split(text)` (by white space)"); }
        if it.by.is_some() || it.after.is_some() || it.reverse || it.last || it.from.is_some() || it.whil.is_some() { return no("a loop over `split` takes `where` and `limit`"); }
        let t = self.expr(&args[0], env, None)?; let t = self.txt_of(t).map_err(|_| "`split` splits a text".to_string())?;
        let sep = match args.get(1) { Some(e) => { let v = self.expr(e, env, None)?; Some(self.txt_of(v).map_err(|_| "`split`'s separator is a text".to_string())?) } None => None };
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        self.loop_enter();
        let (pos, part, more, hits) = (self.tmp("pos"), self.tmp("t"), self.tmp("more"), self.tmp("hits"));
        self.open("{");
        self.line(&format!("u32 {} = 0, {} = 0; int {} = 0; Txt {};", pos, hits, more, part));
        // a text of n bytes has at most n + 1 pieces
        let round = self.tmp("k");
        self.open(&format!("for (u32 {r} = 0; {r} <= {}.len; {r}++) {{", t, r = round));
        if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); }
        self.call(&format!("c->h->split_next(c, &{}, {}, &{}, &{}, &{})", t, sep.as_ref().map_or("0".to_string(), |s| format!("&{}", s)), pos, part, more));
        self.line(&format!("if (!{}) break;", more));
        if let Some(b) = self.txt_bounds.get(&t).copied() { self.txt_bounds.insert(part.clone(), b); }
        env[it.slot as usize] = Some(CV::Txt(part.clone()));
        if let Some((_, s)) = it.index { let ix = self.tmp("ix"); self.line(&format!("i64 {} = (i64){};", ix, hits)); env[s as usize] = Some(CV::Int(ix)); }
        if let Some(w) = &it.wher { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) continue;", wc)); }
        self.line(&format!("{}++;", hits));
        body(self, env, &hits)?;
        self.close("}");
        self.close("}");
        self.loop_exit();
        Ok(())
    }
    pub(super) fn each(&mut self, it: &'p Iter, env: &mut Vec<Option<CV>>, collect: bool, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        let loop_body = self.loop_body.take();
        if let Some(v) = it.via { return self.walk(it, v, env, body); }
        if let Some(hi) = &it.hi { return self.range_loop(it, hi, env, body); }
        if let Expr::Call(f, args, _) = &it.coll { if f == "split" { return self.split_loop(it, args, env, body); } }
        let CV::Coll(c, inst) = self.expr(&it.coll, env, None)? else { return no("each over something other than a collection") };
        if let Some(t) = tab(c) { return self.table_loop(it, t, inst, env, collect, loop_body, body); }
        if let Some(f) = &it.by { return no(format!("`by {}`: only a table is walked in a field's order (a list keeps insertion order; keep other orders in the data)", f)); }
        if it.after.is_some() { return no("`after`: only a table's rows are paged this way"); }
        self.loop_enter();
        let coll = self.prog.colls[c].clone();
        if it.last { return no("`last n` is for a loop over a table (the last n rows of its order)"); }
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        let (k, m, hits, h) = (self.tmp("k"), self.tmp("m"), self.tmp("hits"), self.tmp("h"));
        self.open("{");
        self.line(&format!("u32 {} = 0;", hits));
        if collect { self.line(&format!("u32 {};", h)); self.call(&format!("c->h->sel_mark(c, &{})", h)); }
        if coll.is_list() {
            let (len, head) = (self.tmp("len"), self.tmp("head"));
            self.line(&format!("u32 {} = LD_U32({}, {}), {} = LD_U32({}, {});", len, coll.hdr, inst, head, coll.hdr + 4 * coll.mult, inst));
            self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", k, k, len, k));
            let pos = if it.reverse { format!("({} + {} - 1 - {})", head, len, k) } else { format!("({} + {})", head, k) };
            self.line(&format!("u32 {} = (u32)((i64)({}) * {} + {} % {});", m, inst, coll.cap, pos, coll.cap));
        } else {
            let hw = self.tmp("hw");
            self.line(&format!("u32 {} = LD_U32({}, {});", hw, coll.hdr, inst));
            if it.reverse { self.open(&format!("for (u32 {} = {}; {}-- > 0;) {{", k, hw, k)); } else { self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", k, k, hw, k)); }
            self.line(&format!("if (!((LD_U64({}, (i64)({}) * {} + {} / 64) >> ({} % 64)) & 1)) continue;", coll.live, inst, coll.words(), k, k));
            self.line(&format!("u32 {} = (u32)((i64)({}) * {} + {});", m, inst, coll.cap, k));
        }
        if !collect { if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); } }
        let v = self.item_value(c, m.clone(), None)?;
        env[it.slot as usize] = Some(v);
        if let Some(w) = &it.wher {
            let wv = self.expr(w, env, None)?;
            let wc = self.cond(wv)?;
            self.line(&format!("if (!({})) continue;", wc));
        }
        self.line(&format!("{}++;", hits));
        if !collect {
            self.round_start(it, env, &hits);
            body(self, env, &hits)?;
            self.loop_round_end();
            self.close("}");
            self.close("}");
            self.loop_exit();
            return Ok(());
        }
        self.call(&format!("c->h->sel_add(c, {})", m));
        if let Some(l) = &lim { self.line(&format!("if ((i64){} >= {}) break;", hits, l)); }
        self.close("}");
        let (n, q, m2) = (self.tmp("n"), self.tmp("q"), self.tmp("m"));
        self.line(&format!("u32 {};", n));
        self.call(&format!("c->h->sel_limit(c, {}, {}, &{})", h, lim.clone().unwrap_or("-1".into()), n));
        self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", q, q, n, q));
        self.line(&format!("u32 {};", m2));
        self.call(&format!("c->h->sel_get(c, {}, {}, &{})", h, q, m2));
        self.line(&format!("{} = {} + 1;", hits, q));
        let v = self.item_value(c, m2, None)?;
        env[it.slot as usize] = Some(v);
        self.round_start(it, env, &hits);
        body(self, env, &hits)?;
        self.loop_round_end();
        self.close("}");
        self.loop_exit(); // `break` ends the selection too
        self.call(&format!("c->h->sel_end(c, {})", h));
        self.close("}");
        Ok(())
    }

    /// `foreach s in instance [from k] [reverse] ...`: walk a module instance with its iteration functions, at most its
    /// capacity of steps, stopping at a null cursor or the limit. Order of evaluation as in the interpreter: `from`,
    /// `limit`, then the first cursor.
    pub(super) fn walk(&mut self, it: &'p Iter, v: crate::lang::Via, env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        let Some(n) = crate::lang::path_of(&it.coll) else { return no("walking something other than a module instance") };
        let steps = self.prog.colls.iter().find(|c| c.name == format!("{}.items", n)).map_or(0, |c| c.cap);
        let from = match &it.from { Some(e) => { let x = self.expr(e, env, None)?; self.hold(x, false)? } None => CV::Null };
        if it.last { return no("`last n` is for a loop over a table (the last n rows of its order)"); }
        let lim = match &it.limit { Some(e) => { let x = self.expr(e, env, None)?; Some(self.int_or(x, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        // a cursor is an integer, -1 when there is none
        let c0 = self.call_values(if it.reverse { v.last.ok_or("no reverse walk")? } else { v.first }, vec![from], None)?;
        let c0 = self.int_or(c0, 500, "a walk's cursor is an integer (-1 for none)");
        let (cur, k, hits) = (self.tmp("cur"), self.tmp("k"), self.tmp("hits"));
        self.loop_enter();
        self.open("{");
        self.line(&format!("i64 {} = {};", cur, c0));
        self.line(&format!("i64 {} = 0;", hits));
        self.open(&format!("for (u32 {} = 0; {} < {}; {}++) {{", k, k, steps, k));
        self.line(&format!("if ({} < 0) break;", cur));
        if let Some(l) = &lim { self.line(&format!("if ({} >= {}) break;", hits, l)); }
        let item = self.call_values(v.item, vec![CV::Int(cur.clone())], None)?;
        let item = self.hold(item, false)?;
        env[it.slot as usize] = Some(item);
        if let Some(w) = &it.whil { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) break;", wc)); }
        let filtered = if let Some(w) = &it.wher { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.open(&format!("if ({}) {{", wc)); true } else { false };
        self.line(&format!("{}++;", hits));
        self.round_start(it, env, &hits);
        body(self, env, &hits)?;
        if filtered { self.close("}"); }
        self.loop_round_end(); // before the step to the next cursor
        let nx = self.call_values(if it.reverse { v.prev.ok_or("no reverse walk")? } else { v.next }, vec![CV::Int(cur.clone())], None)?;
        let nx = self.int_or(nx, 500, "a walk's cursor is an integer (-1 for none)");
        self.line(&format!("{} = {};", cur, nx));
        self.close("}");
        self.close("}");
        self.loop_exit();
        Ok(())
    }

    /// `foreach i in lo..hi`: the bounds are read once, values are computed as the loop goes (where is tested just before
    /// each body), and `limit` counts the bodies run.
    pub(super) fn range_loop(&mut self, it: &'p Iter, hi: &'p Expr, env: &mut Vec<Option<CV>>, body: impl FnOnce(&mut Self, &mut Vec<Option<CV>>, &str) -> R<()>) -> R<()> {
        let v = self.expr(&it.coll, env, None)?;
        let lo = self.int_or(v, 500, "a range needs integers");
        let lo = self.hold_int(lo);
        let v = self.expr(hi, env, None)?;
        let hi = self.int_or(v, 500, "a range needs integers");
        let hi = self.hold_int(hi);
        if it.last { return no("`last n` is for a loop over a table (the last n rows of its order)"); }
        let lim = match &it.limit { Some(e) => { let v = self.expr(e, env, None)?; Some(self.int_or(v, 400, "bad limit")) } None => None };
        let lim = lim.map(|l| self.hold_int(l));
        if let Some(l) = &lim { self.line(&format!("if ({} < 0) FAIL(400, \"bad limit\", 9);", l)); }
        let (k, hits) = (self.tmp("k"), self.tmp("hits"));
        self.loop_enter();
        self.open("{");
        self.line(&format!("i64 {} = 0;", hits));
        if it.reverse {
            let kk = self.tmp("k");
            self.open(&format!("for (i64 {} = {}; {} > {};) {{", kk, hi, kk, lo));
            self.line(&format!("i64 {} = --{};", k, kk));
        } else {
            self.open(&format!("for (i64 {} = {}; {} < {}; {}++) {{", k, lo, k, hi, k));
        }
        if let Some(l) = &lim { self.line(&format!("if ({} >= {}) break;", hits, l)); }
        // a loop without a fixed extent counts its rounds against the request's budget, so a mistake cannot hold the program
        if it.budget { self.line(&format!("ROUND_({});", k)); }
        env[it.slot as usize] = Some(CV::Int(k.clone()));
        // the counter is never assigned in the body: between the smallest lower bound and one below the largest upper bound
        if let (Some((lo_min, _)), Some((_, hi_max))) = (self.interval(&lo), self.interval(&hi)) { if hi_max > lo_min { self.ranges.insert(k.clone(), (lo_min, hi_max - 1)); } }
        if let Some(h) = self.loop_hint.take() { self.ranges.entry(k.clone()).or_insert(h); } // a versioning guard proved the bounds within 2^40
        // a counter this loop only steps by constants has, in the body and after it, the interval its entry value plus (steps per round x most rounds) gives
        if let (Some(stmts), Some(entry)) = (self.loop_stmts.take(), self.loop_entry.take()) {
            let by_range = match (self.interval(&lo), self.interval(&hi)) { (Some((lo_min, _)), Some((_, hi_max))) => Some((hi_max as i128 - lo_min as i128).max(0)), _ => None };
            // a loop without a fixed extent charges its rounds to the request's budget, which the runtime never allows above 2^50 (the rounds it can run before failing: a little more)
            let by_budget = if it.budget { Some(1i128 << 51) } else { None };
            let trips = match (by_range, by_budget) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) };
            let heads = self.counter_heads(stmts, &entry, trips, env);
            for (n, iv) in &heads { self.var_iv.insert(n.clone(), *iv); }
            self.loop_heads = Some(heads);
        }
        if let Some(w) = &it.whil { let wv = self.expr(w, env, None)?; let wc = self.cond(wv)?; self.line(&format!("if (!({})) break;", wc)); self.refine(w, env)?; }
        if let Some(w) = &it.wher {
            let wv = self.expr(w, env, None)?;
            let wc = self.cond(wv)?;
            self.line(&format!("if (!({})) continue;", wc));
        }
        self.line(&format!("{}++;", hits));
        self.round_start(it, env, &hits);
        body(self, env, &hits)?;
        self.loop_round_end();
        self.close("}");
        self.close("}");
        self.loop_exit();
        Ok(())
    }

    /// Write `j` as JSON; returns the shape it has (known while generating), for checking against `Response<T>`.
    /// An enum value (an integer) as the name of its value; a number that is none of them is written as the number.
    pub(super) fn write_enum(&mut self, en: usize, e: &str) -> R<()> {
        let x = self.tmp("ev");
        self.line(&format!("i64 {} = {};", x, e));
        let vals = self.prog.enums[en].values.clone();
        for (i, (n, v)) in vals.iter().enumerate() {
            self.open(&format!("{}if ({} == {}) {{", if i > 0 { "else " } else { "" }, x, c_i64(*v)));
            let t = json_lit(n);
            self.out(&t);
            self.close("}");
        }
        self.open("else {");
        self.call(&format!("c->h->json_i64(c, {})", x));
        self.close("}");
        Ok(())
    }
    /// The enum a declared answer type names, if it is one (`Op` or `Op?`).
    pub(super) fn enum_of(&self, want: Option<&crate::lang::RType>) -> Option<usize> {
        match want {
            Some(crate::lang::RType::Name(n)) => self.prog.enums.iter().position(|e| e.name == *n),
            Some(crate::lang::RType::Opt(t)) => self.enum_of(Some(t)),
            _ => None,
        }
    }

    /// Write `j` as JSON (`want`: its declared type, for enums written by name).
    pub(super) fn json(&mut self, j: &'p Json, env: &mut Vec<Option<CV>>, want: Option<&crate::lang::RType>) -> R<Shape> {
        let want = match want { Some(crate::lang::RType::Opt(t)) if !matches!(**t, crate::lang::RType::Name(_)) => Some(&**t), w => w };
        match j {
            Json::Obj(fields) => {
                let mut text = String::from("{");
                let mut shape = Vec::new();
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 { text.push(','); }
                    text += &json_lit(k); text.push(':');
                    self.out(&text); text.clear();
                    let fw = match want { Some(crate::lang::RType::Obj(fs)) => fs.iter().find(|x| x.0 == *k).map(|x| x.1.clone()), _ => None };
                    shape.push((k.clone(), self.json(v, env, fw.as_ref())?));
                }
                text.push('}'); self.out(&text);
                Ok(Shape::Obj(shape))
            }
            Json::Arr(it, inner) => {
                self.out("[");
                if let (Some(_), Some(n)) = (&it.via, crate::lang::path_of(&it.coll)) {
                    let n = &n;
                    if changes_instance(self.api, n, &[Stmt::Respond(200, (**inner).clone())]) { return no(format!("a loop over {} calls a function that changes {}; change it after the loop", n, n)); }
                }
                let mut elem = Shape::Any;
                let ew = match want { Some(crate::lang::RType::Arr(t)) => Some((**t).clone()), _ => None };
                self.loop_body = Some(LoopBody::Json(inner));
                self.each(it, env, false, |g, env, hits| {
                    g.line(&format!("if ({} > 1) {{ CALL(c->h->out(c, \",\", 1)); }}", hits));
                    elem = g.json(inner, env, ew.as_ref())?;
                    Ok(())
                })?;
                self.out("]");
                Ok(Shape::Arr(Box::new(elem)))
            }
            Json::E(Expr::Call(_, args, CallRes::Builtin(B::Or))) => {
                // or(value that may be a dead reference or null, fallback): write the value, or the fallback
                let (l, end) = (self.tmp("dead"), self.tmp("done"));
                self.line("{");
                let saved = self.or_dead.replace(l.clone());
                let v = self.expr(&args[0], env, Some(&l));
                self.or_dead = saved;
                let v = v?;
                match &v { CV::Txt(t) => self.line(&format!("if (!{}.present) goto {};", t, l)), CV::Any(t) => self.line(&format!("if ({}.tag == 0) goto {};", t, l)), CV::Ref(_, s, _) => self.line(&format!("if ({} == 0) goto {};", s, l)), CV::Null => self.line(&format!("goto {};", l)), _ => {} }
                let a = Shape::of(&v);
                self.write(v)?;
                self.line(&format!("goto {};", end));
                self.line("}");
                self.line(&format!("{}: {{", l));
                let fb = self.expr(&args[1], env, None)?;
                let b = Shape::of(&fb);
                self.write(fb)?;
                self.line("}");
                self.line(&format!("{}: ;", end));
                Ok(Shape::either(a, b))
            }
            Json::E(e) => {
                let v = self.expr(e, env, None)?;
                let s = Shape::of(&v);
                match (self.enum_of(want), &v) { (Some(en), CV::Int(x)) => { let x = x.clone(); self.write_enum(en, &x)?; } _ => self.write(v)? }
                Ok(s)
            }
            Json::Empty | Json::Html => Ok(Shape::Nothing),
        }
    }

    /// Is this endpoint's answer HTML (markup may be written)?
    pub(super) fn html_answer(&self) -> bool { matches!(&self.ep.resp, crate::lang::RType::Name(n) if n == "html") }
    /// A value written into markup, escaped (`attr`: inside an attribute's double quotes): text with & < > (and ") as entities,
    /// numbers as digits, bools as true / false, null as nothing; the result of a `-> html` call has already written itself.
    pub(super) fn html_esc(&mut self, v: CV, attr: bool) -> R<()> {
        let mode = attr as u32;
        match v {
            CV::Null => {}
            CV::Lit(s) => { let e = html_escape(&s, attr); self.out(&e); }
            CV::Txt(t) => self.line(&format!("if ({}.present) CALL(c->h->html_txt(c, &{}, {}));", t, t, mode)),
            CV::PlainStr(off) => { let at = self.sub_bytes_at(&CV::PlainStr(off), "0"); let n = self.field_len(&CV::PlainStr(off)); self.call(&format!("c->h->html_at(c, {}, (u32)({}), {})", at, n, mode)); }
            CV::Sub(base, from, count) => { let at = self.sub_bytes_at(&base, &from); self.call(&format!("c->h->html_at(c, {}, (u32)({}), {})", at, count, mode)); }
            CV::StateStr(c, idx, ci) => self.call(&format!("c->h->html_col(c, {}, {}, {}, {})", c, idx, ci, mode)),
            CV::Int(e) => self.call(&format!("c->h->json_i64(c, {})", e)),
            CV::Flt(e) => self.call(&format!("c->h->json_f64(c, {})", e)),
            CV::Bool(e) if e == "1" || e == "0" => self.out(if e == "1" { "true" } else { "false" }),
            CV::Bool(e) => self.line(&format!("CALL({} ? c->h->out(c, \"true\", 4) : c->h->out(c, \"false\", 5));", e)),
            _ => return no("markup takes text, numbers and bools in `{ }` (or a `-> html` function's call)"),
        }
        Ok(())
    }
    /// Text added to the answer as it is (an HTML or plain-text answer).
    pub(super) fn raw(&mut self, v: CV, what: &str) -> R<()> {
        match v {
            CV::Lit(s) => self.out(&s),
            CV::Txt(t) => self.line(&format!("if ({}.present) CALL(c->h->raw_txt(c, &{}));", t, t)),
            CV::PlainStr(off) => { let at = self.sub_bytes_at(&CV::PlainStr(off), "0"); let n = self.field_len(&CV::PlainStr(off)); self.call(&format!("c->h->raw_at(c, {}, (u32)({}))", at, n)); }
            CV::Sub(base, from, count) => { let at = self.sub_bytes_at(&base, &from); self.call(&format!("c->h->raw_at(c, {}, (u32)({}))", at, count)); }
            CV::StateStr(c, idx, ci) => self.call(&format!("c->h->raw_col(c, {}, {}, {})", c, idx, ci)),
            _ => return no(format!("a Response<{}> answer is text", what)),
        }
        Ok(())
    }
    pub(super) fn out(&mut self, text: &str) { if !text.is_empty() { self.call(&format!("c->h->out(c, {}, {})", c_lit(text.as_bytes()), text.len())); } }
    pub(super) fn write(&mut self, v: CV) -> R<()> {
        match v {
            CV::Int(e) => self.call(&format!("c->h->json_i64(c, {})", e)),
            CV::Flt(e) => self.call(&format!("c->h->json_f64(c, {})", e)),
            CV::Bool(e) if e == "1" || e == "0" => self.out(if e == "1" { "true" } else { "false" }),
            CV::Bool(e) => self.line(&format!("CALL({} ? c->h->out(c, \"true\", 4) : c->h->out(c, \"false\", 5));", e)),
            CV::Lit(s) => { let j = json_lit(&s); self.out(&j); }
            CV::Txt(t) => self.line(&format!("CALL({}.present ? c->h->json_txt(c, &{}) : c->h->out(c, \"null\", 4));", t, t)),
            CV::PlainStr(off) => { let at = self.sub_bytes_at(&CV::PlainStr(off), "0"); let n = self.field_len(&CV::PlainStr(off)); self.call(&format!("c->h->json_at(c, {}, (u32)({}))", at, n)); }
            CV::Sub(base, from, count) => { let at = self.sub_bytes_at(&base, &from); self.call(&format!("c->h->json_at(c, {}, (u32)({}))", at, count)); }
            CV::Any(v) => self.call(&format!("c->h->json_val(c, &{})", v)),
            CV::StateStr(c, idx, ci) => self.call(&format!("c->h->json_col(c, {}, {}, {})", c, idx, ci)),
            CV::Null => self.out("null"),
            CV::Ref(t, s, k) if tab(t).is_some() => { self.line(&format!("if ({} == 0) {{ CALL(c->h->out(c, \"null\", 4)); }}", s)); self.line(&format!("else CALL(c->h->json_i64(c, (i64)({})));", k)); }
            CV::Ref(t, s, k) => {
                if self.prog.colls[t].parent.is_some() { return no("an item of a collection inside another's items has no id; write `slot(x)`"); }
                self.line(&format!("if ({} == 0) {{ CALL(c->h->out(c, \"null\", 4)); }}", s));
                self.line(&format!("else CALL(c->h->json_id(c, {}, {}));", t, k));
            }
            CV::Item(c, i) if tab(c).is_some() => self.call(&format!("c->h->json_i64(c, (i64)({}))", i)),
            CV::Item(c, i) if self.prog.colls[c].parent.is_none() && matches!(self.prog.colls[c].kind, Kind::Pool(_)) => self.call(&format!("c->h->json_id(c, {}, {})", c, i)),
            CV::Item(..) | CV::Coll(..) | CV::Plain(..) | CV::Part(..) | CV::ArrCol(..) | CV::Elem(..) | CV::PlainArr(..) | CV::Resp(..) => self.fail(500, "cannot write this value as JSON"),
        }
        Ok(())
    }
}
