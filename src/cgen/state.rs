//! G's setup, scopes, range facts, specialization, declared kinds and loop-body helpers.
use super::*;

impl<'p> G<'p> {
    pub(super) fn new(prog: &'p Program, api: &'p Api, ep: &'p Endpoint, slot_kinds: &'p [((Option<u32>, u32), K)], ret_kinds: &'p [(u32, K)], recursive: &'p [bool], specs: &'p std::cell::RefCell<Vec<Spec>>) -> G<'p> {
        G { ret_ctx: None, pending_ctx: None, var_iv: std::collections::HashMap::new(), wslots: Default::default(), wargs: Vec::new(), wskip: false, loop_frames: Vec::new(), var_depth: std::collections::HashMap::new(), loop_stmts: None, loop_entry: None, loop_heads: None, rq_touched: Vec::new(), uses_spec: false, wrote_state: std::cell::Cell::new(false), narrowed: Vec::new(), txt_bounds: std::collections::HashMap::new(), region_lets: Vec::new(), own_region: false, region_live: 0, region_peak: 0, region_unbounded: Vec::new(), txt_caps: std::collections::HashMap::new(), prog, api, ep, body: String::new(), n: 0, depth: 0, inlining: Vec::new(), ret: Vec::new(), frame: None, slot_kinds, ret_kinds, widen: None, u32_vars: std::collections::HashSet::new(), rest: &[], var_names: std::collections::HashSet::new(), fast_sites: std::collections::HashSet::new(), skip_save: false, presave: None, checked: Vec::new(), at: (0, 0), test: false, str_lens: std::collections::HashMap::new(), loop_n: 0, loops: Vec::new(), loop_hint: None, versioned: 0, txt_escapes: Vec::new(), recursive, specs, dep: false, inl: 0, ranges: std::collections::HashMap::new(), or_dead: None, loop_body: None, merged: Vec::new(), scr_top: 0, spec_key: None, scr_n: 0, spec_rec: false, immut: std::collections::HashSet::new() }
    }

    /// An endpoint's typed inputs, read and checked before its body, into slots 0.. in order. For a request with a body,
    /// its fields must be among the body inputs: an unknown field is 400.
    pub(super) fn inputs(&mut self, ep: &'p Endpoint, env: &mut Vec<Option<CV>>) -> R<()> {
        if matches!(ep.method.as_str(), "POST" | "PUT" | "PATCH") || ep.inputs.iter().any(|i| i.src == Src::Body) {
            let names: String = ep.inputs.iter().filter(|i| i.src == Src::Body).map(|i| format!("{}\0", i.name)).collect();
            self.call(&format!("c->h->fields(c, {}, {})", c_lit(names.as_bytes()), names.len()));
        }
        for (k, inp) in ep.inputs.iter().enumerate() {
            let src = match &inp.src { Src::Body => 0, Src::Path => 1, Src::Query => 2, Src::Header(_) => 3, Src::Fwd => 4, Src::Cookie(_) => 5 };
            let name = match &inp.src { Src::Header(h) | Src::Cookie(h) => h.clone(), _ => inp.name.clone() };
            let (kind, lo, hi, pool) = match &inp.ty {
                InTy::Int(a, b) => (1, *a, *b, 0),
                InTy::Flt => (2, 0, 0, 0),
                InTy::Bool => (3, 0, 0, 0),
                InTy::Str(a, b, ascii) => (if *ascii { 7 } else { 4 }, *a, *b, 0),
                InTy::Item(t, p) => { let c = self.item_pool(&inp.name, t, p.as_deref())?; (match inp.src { Src::Header(_) => 5, Src::Cookie(_) => 8, _ => 6 }, c as i64, 0, c) }
            };
            let v = self.tmp("in");
            self.line(&format!("Val {};", v));
            self.call(&format!("c->h->input(c, {}, {}, {}, {}, {}, {}, &{})", src, c_lit(name.as_bytes()), name.len(), kind, i64_lit(lo), i64_lit(hi), v));
            // absent: the default, null when optional, else 400
            match (&inp.default, inp.optional) {
                (Some(d), _) => { let dv = self.expr(d, env, None)?; let dv = self.val(dv)?; self.line(&format!("if ({}.tag == 0) {} = {};", v, v, dv)); }
                (None, true) => {}
                (None, false) if matches!(inp.src, Src::Cookie(_)) && matches!(inp.ty, InTy::Item(..)) => { let msg = format!("no valid `{}` cookie", name); self.line(&format!("if ({}.tag == 0) FAIL(401, {}, {});", v, c_lit(msg.as_bytes()), msg.len())); }
                (None, false) => { let msg = format!("`{}` is required", inp.name); self.line(&format!("if ({}.tag == 0) FAIL(400, {}, {});", v, c_lit(msg.as_bytes()), msg.len())); }
            }
            let cv = match &inp.ty {
                InTy::Int(a, b) => {
                    let x = self.tmp("i");
                    self.line(&format!("i64 {} = {}.i;", x, v));
                    // a value the runtime range-checked, or a default that is an integer literal inside the range (an optional input
                    // with no default can be absent, and then it is 0)
                    let inside = match &inp.default { None => !inp.optional, Some(Expr::Int(d)) => d >= a && d <= b, _ => false };
                    if inside { self.ranges.insert(x.clone(), (*a, *b)); }
                    CV::Int(x)
                }
                InTy::Flt => { let x = self.tmp("f"); self.line(&format!("double {} = {}.f;", x, v)); CV::Flt(x) }
                InTy::Bool => { let x = self.tmp("b"); self.line(&format!("int {} = {}.i != 0;", x, v)); CV::Bool(x) }
                InTy::Str(_, hi, _) => { let t = self.tmp("t"); self.line(&format!("Txt {} = {{ {}.a, {}.b, {}.tag == 4 }};", t, v, v, v)); self.txt_bounds.insert(t.clone(), *hi as usize); CV::Txt(t) }
                InTy::Item(..) if inp.optional => { let (s, x) = (self.tmp("r"), self.tmp("i")); self.line(&format!("u32 {} = {}.tag != 0, {} = {}.b;", s, v, x, v)); CV::Ref(pool, s, x) }
                InTy::Item(..) => { let x = self.tmp("i"); self.line(&format!("u32 {} = {}.b;", x, v)); CV::Item(pool, x) }
            };
            env[k] = Some(cv);
        }
        Ok(())
    }
    /// The pool an input of item type `t` names: the one top-level pool of `t` items, or `in` the one given.
    pub(super) fn item_pool(&self, input: &str, t: &str, pool: Option<&str>) -> R<usize> {
        let Some(sid) = self.prog.structs.iter().position(|s| s.name == t) else { return no(format!("input `{}`: `{}` is not a type", input, t)) };
        let mut found: Vec<usize> = self.prog.colls.iter().enumerate().filter(|(_, c)| c.item == Some(sid) && c.parent.is_none() && matches!(c.kind, Kind::Pool(_)) && pool.map_or(true, |p| c.name == p)).map(|(i, _)| i).collect();
        found.extend(self.prog.tables.iter().enumerate().filter(|(_, x)| x.item == sid && x.parent.is_none() && pool.map_or(true, |p| x.name == p)).map(|(i, _)| TBASE + i));
        match found.as_slice() {
            [c] => Ok(*c),
            [] => no(format!("input `{}`: no pool that exists once holds `{}`{}", input, t, pool.map_or(String::new(), |p| format!(" as {}", p)))),
            _ => no(format!("input `{}`: `{}` is kept in more than one pool; say which: `{}: {} in {}`", input, t, input, t, coll_name(self.prog, found[0]))),
        }
    }

    /// The module instance whose function is being generated (None: the program's own code).
    pub(super) fn module_code(&self) -> Option<String> { let k = *self.inlining.last()?; let n = &self.api.fns[k as usize].name; n.rfind('.').map(|i| n[..i].to_string()) }
    /// The instance a collection belongs to: (instance, its items or nested in them (the program's data), its items exactly).
    pub(super) fn coll_owner(&self, c: usize) -> Option<(&'p crate::layout::ModInst, bool, bool)> {
        if tab(c).is_some() { return None; }
        let name = &self.prog.colls[c].name;
        let m = self.prog.instances.iter().find(|m| name.starts_with(&format!("{}.", m.name)))?;
        let items = format!("{}.items", m.name);
        Some((m, *name == items || name.starts_with(&format!("{}.", items)), *name == items))
    }
    pub(super) fn module_err(&self, m: &crate::layout::ModInst, what: &str) -> String { format!("{} belongs to {} (module {}); change it through {}'s functions", what, m.name, m.module, m.name) }
    /// The locals assigned in these statements, and the `write` parameters of the function being inlined (each stands for a place something else may write: not invariant in a loop).
    pub(super) fn assigned_here(&self, body: &[Stmt]) -> std::collections::HashSet<u32> {
        let mut a = assigned_slots(body);
        for ((fr, s), _) in &self.wslots { if *fr == self.frame { a.insert(*s); } }
        a
    }
    /// Writing column `path` of collection `c` from here: refused outside a module for its own state, and for its key.
    /// `loop_stmt` for a loop whose body is `body`, with what is known of the variables set up as the handler of `Stmt::Each` says: `base` (the ones the loop assigns dropped),
    /// plus the counters `range_loop` can bound from the `entry` state.
    pub(super) fn loop_vars(&mut self, it: &'p Iter, body: &'p [Stmt], env: &mut Vec<Option<CV>>, collect: bool, entry: &std::collections::HashMap<String, (i64, i64)>, base: &std::collections::HashMap<String, (i64, i64)>) -> R<()> {
        self.var_iv = base.clone();
        let loop_id = it as *const Iter as usize;
        for slot in assigned_slots(body) {
            if let Some(Some(CV::Int(n))) = env.get(slot as usize) {
                if let (true, Some(iv)) = (entry.contains_key(n), LOOP_INV.with(|l| l.borrow().assumed.get(&(loop_id, slot)).copied())) { self.var_iv.insert(n.clone(), iv); }
            }
        }
        self.loop_stmts = Some(body);
        self.loop_entry = Some(entry.clone());
        self.loop_heads = None;
        self.loop_frames.push(loop_id);
        let r = self.loop_stmt(it, body, env, collect);
        self.loop_frames.pop();
        self.loop_stmts = None;
        self.loop_entry = None;
        r
    }
    /// The variables a counted loop only steps up or down by constants (`n = n + 1`, `n = n - 2`; nowhere else assigned in the body, nor inside a loop of the body): each stays
    /// within its entry interval widened by (the steps of one round) x (the most rounds), in the body and after the loop. `trips`: the most rounds, when known.
    pub(super) fn counter_heads(&self, stmts: &[Stmt], entry: &std::collections::HashMap<String, (i64, i64)>, trips: Option<i128>, env: &[Option<CV>]) -> std::collections::HashMap<String, (i64, i64)> {
        let mut heads = std::collections::HashMap::new();
        let Some(trips) = trips else { return heads };
        for slot in assigned_slots(stmts) {
            let Some(Some(CV::Int(name))) = env.get(slot as usize) else { continue };
            let Some((lo, hi)) = entry.get(name).copied() else { continue };
            let Some((up, down)) = step_sums(stmts, slot) else { continue };
            let (l, h) = (lo as i128 - down * trips, hi as i128 + up * trips);
            if l >= i64::MIN as i128 && h <= i64::MAX as i128 { heads.insert(name.clone(), (l as i64, h as i64)); }
        }
        heads
    }
    /// What a condition that holds tells of a variable: `x < e` and `x <= e` (and `e > x`, `e >= x`), joined by `&&`, give `x` the upper bound of `e`'s interval (below it).
    pub(super) fn refine(&mut self, c: &'p Expr, env: &mut Vec<Option<CV>>) -> R<()> {
        let Expr::Bin(op, a, b) = c else { return Ok(()) };
        if op == "&&" { self.refine(a, env)?; return self.refine(b, env); }
        let (var, bound, strict) = match op.as_str() { "<" => (a, b, true), "<=" => (a, b, false), ">" => (b, a, true), ">=" => (b, a, false), _ => return Ok(()) };
        let Expr::Var(_, VarRes::Local(slot)) = &**var else { return Ok(()) };
        let Some(Some(CV::Int(name))) = env.get(*slot as usize).cloned() else { return Ok(()) };
        if !self.var_names.contains(&name) || self.ranges.contains_key(&name) || !pure(bound) { return Ok(()); }
        let bv = self.expr(bound, env, None)?;
        let CV::Int(bc) = bv else { return Ok(()) };
        let Some((_, bh)) = self.interval(&bc) else { return Ok(()) };
        let h = if strict { bh.saturating_sub(1) } else { bh };
        let (lo, hi) = self.var_iv.get(&name).copied().unwrap_or((i64::MIN, i64::MAX));
        self.var_iv.insert(name, (lo, hi.min(h)));
        Ok(())
    }
    pub(super) fn may_write_col(&self, c: usize, path: &str) -> R<()> {
        self.wrote_state.set(true);
        let Some((m, data, items)) = self.coll_owner(c) else { return Ok(()) };
        if self.module_code().as_deref() == Some(m.name.as_str()) { return Ok(()); }
        if !data { return Err(self.module_err(m, &format!("`{}`", self.prog.colls[c].name))); }
        if items && m.key.as_deref() == Some(path) { return Err(format!("`{}` is the key of {}; change it with {}.set_key(item, value)", path, m.name, m.name)); }
        Ok(())
    }
    // ---- struct values: locals are places in the request region's scratch area ("Struct values" in DESIGN.md) ----
    pub(super) fn in_scratch(&self, off: usize) -> bool { let (a, n) = self.prog.scratch; off >= a && off < a + n }
    /// A place of `size` bytes in the scratch area for a struct value: from the area's start in the code of an endpoint or an inlined function (given back at the end of the
    /// block, so locals of blocks that do not overlap share it); in a C function generated apart from the area's top end, one place for each local, kept for the whole program
    /// (such a function is never running twice at once, and its caller's places are lower).
    pub(super) fn scratch_alloc(&mut self, size: usize) -> R<usize> {
        if self.spec_rec { return no("a struct value inside a recursive function is not supported yet (each call would need its own place: a frame); keep the struct in the state and pass it, or make the function not recursive"); }
        let (so, cap) = self.prog.scratch;
        let size = (size.max(1) + 7) / 8 * 8;
        let full = format!("the struct values live at once here need more than the {} bytes of scratch a request has: use fewer or smaller struct locals", cap);
        if let Some(key) = self.spec_key.clone() {
            let n = self.scr_n;
            self.scr_n += 1;
            return SCRATCH.with(|s| {
                let mut s = s.borrow_mut();
                if let Some((o, sz)) = s.unique.get(&(key.clone(), n)).copied() { if sz == size { return Ok(so + o); } }
                if s.top + size + s.bottom > cap { return Err(full.clone()); }
                s.top += size;
                let o = cap - s.top;
                s.unique.insert((key, n), (o, size));
                Ok(so + o)
            });
        }
        let o = self.scr_top;
        self.scr_top += size;
        let top = self.scr_top;
        SCRATCH.with(|s| { let mut s = s.borrow_mut(); s.bottom = s.bottom.max(top); if s.top + s.bottom > cap { Err(full.clone()) } else { Ok(()) } })?;
        Ok(so + o)
    }
    /// Declares the struct `sid` at scratch `off` for the scalar replacement pass (src/scalar.rs): a comment line listing its scalar cells (`width@offset`), or `!` when it
    /// holds text or an array (never replaced).
    pub(super) fn scratch_marker(&mut self, sid: usize, off: usize) {
        fn walk(prog: &Program, sid: usize, base: usize, out: &mut Vec<String>) -> bool {
            let sd = &prog.structs[sid];
            for (k, (_, t)) in sd.fields.iter().enumerate() {
                match t {
                    Ty::Scalar(sc) => out.push(format!("{}@{}", sc.size(), base + sd.offs[k])),
                    Ty::Struct(s2) => { if !walk(prog, *s2, base + sd.offs[k], out) { return false; } }
                    _ => return false,
                }
            }
            true
        }
        let mut cells = Vec::new();
        let ok = walk(self.prog, sid, 0, &mut cells);
        let n = self.prog.structs[sid].size;
        self.line(&format!("/*SCR {} {} {}*/", off, n, if ok { cells.join(" ") } else { "!".to_string() }));
    }
    /// The bytes of the request struct the code reads or writes here are ones the endpoint must find zeroed first.
    pub(super) fn touch_range(&mut self, off: usize, n: usize) {
        let Some((ro, rn)) = self.prog.request else { return };
        if off < ro || off >= ro + rn { return; }
        let r = (off, off + n.max(1));
        if !self.rq_touched.contains(&r) { self.rq_touched.push(r); }
    }
    /// A store into every number leaf of struct `sid` at `off` that no range is proven for (the whole-program ranges of what each field holds: see `generate_with`).
    pub(super) fn seen_struct(&mut self, sid: usize, off: usize) {
        for k in 0..self.prog.structs[sid].fields.len() {
            let (t, at) = (self.prog.structs[sid].fields[k].1.clone(), off + self.prog.structs[sid].offs[k]);
            self.seen_ty(&t, at);
        }
    }
    pub(super) fn seen_ty(&mut self, t: &Ty, at: usize) {
        match t {
            Ty::Scalar(_) => self.field_store_seen(at, None),
            Ty::Arr(e, n) => match &**e { Ty::Scalar(_) => self.field_store_seen(at, None), e => { let sz = self.prog.plain_size(e).0; for i in 0..*n { self.seen_ty(e, at + i * sz); } } },
            Ty::Struct(s) => self.seen_struct(*s, at),
            _ => {}
        }
    }
    /// Copy struct `sid` from the place at `src` to the one at `dst` (a whole copy, as a memmove: the places may be the same or overlap). A destination in the state is a state
    /// write: it saves the logged lines it covers for the undo log first, and counts for the endpoint's reads and the whole-program ranges; one in the request region (the
    /// scratch area, the request struct) is the request's own and is not.
    pub(super) fn copy_struct(&mut self, sid: usize, dst: usize, src: usize) -> R<()> {
        let n = self.prog.structs[sid].size;
        if dst == src || n == 0 { return Ok(()); }
        self.touch_range(src, n);
        let (ro, rn) = self.prog.request.unwrap_or((0, 0));
        let own = (dst >= ro && dst < ro + rn) || self.in_scratch(dst);
        if own { self.touch_range(dst, n); } else {
            self.may_write_plain(dst)?;
            self.may_write_plain(dst + n - 1)?;
            self.seen_struct(sid, dst);
            for l in dst / 64..=(dst + n - 1) / 64 {
                if !self.unlogged_at((l * 64).max(dst)) { self.line(&format!("SAVE_({}ULL, 64);", l * 64)); }
            }
        }
        self.line(&format!("{}({}ULL, {}ULL, {});", if own { "SCRMOVE" } else { "MOVE_" }, dst, src, n));
        Ok(())
    }
    /// `Name { f: e, ..base }` (the parser's `__mito__struct("Name", mode, fields...)`): a new struct place, all zero: its fields are set by the assignments that follow.
    /// `mode`: "full" (every field given), "zero" (`Name {}`), "update" (`..base` and the fields listed, copied over a copy of `base`).
    pub(super) fn struct_new(&mut self, args: &[Expr]) -> R<CV> {
        let strs: Vec<&str> = args.iter().filter_map(|a| if let Expr::Str(s) = a { Some(s.as_str()) } else { None }).collect();
        let (name, mode) = (strs[0], strs[1]);
        let Some(sid) = self.prog.structs.iter().position(|s| s.name == name) else { return no(format!("`{} {{ ... }}`: no struct `{}`{}", name, name, crate::lang::suggest(name, self.prog.structs.iter().map(|s| s.name.as_str())))) };
        let sd = &self.prog.structs[sid];
        for f in &strs[2..] {
            if !sd.fields.iter().any(|x| x.0 == *f) { return no(format!("`{} {{ ... }}`: no field `{}`{}", name, f, crate::lang::suggest(f, sd.fields.iter().map(|x| x.0.as_str())))); }
        }
        if mode == "full" {
            let missing: Vec<&str> = sd.fields.iter().filter(|x| !strs[2..].contains(&x.0.as_str())).map(|x| x.0.as_str()).collect();
            if !missing.is_empty() {
                let arr = sd.fields.iter().any(|x| missing.contains(&x.0.as_str()) && matches!(x.1, Ty::Arr(..)));
                return no(format!("`{} {{ ... }}` is missing {}: a struct literal gives every field{}", name, missing.iter().map(|m| format!("`{}`", m)).collect::<Vec<_>>().join(", "), if arr { " (an array field cannot be given in a literal: start from `Name {}` or `Name { ..base }` and set its elements)" } else { "; `Name {}` is all zero, `Name { ..base, f: e }` changes some" }));
            }
        }
        self.prog.value_check(sid)?;
        let off = self.scratch_alloc(self.prog.structs[sid].size)?;
        let n = self.prog.structs[sid].size;
        self.scratch_marker(sid, off);
        self.line(&format!("SCRZERO({}ULL, {});", off, n));
        Ok(CV::Plain(sid, off))
    }
    /// The slot of the local at the root of `e` (`v`, `v.pos.x`, `v.xs[i]`), and its name.
    pub(super) fn root_local(e: &Expr) -> Option<(&str, u32)> {
        match e { Expr::Var(n, VarRes::Local(s)) => Some((n.as_str(), *s)), Expr::Field(b, _) | Expr::Index(b, _) => Self::root_local(b), _ => None }
    }
    /// Before an assignment to `lhs` (not a bare variable): a `let` struct local's fields are not assigned; a test changes only its own struct locals.
    pub(super) fn check_assign_target(&self, lhs: &Expr, env: &[Option<CV>]) -> R<()> {
        if matches!(lhs, Expr::Var(..)) { return Ok(()); }
        let root = Self::root_local(lhs);
        if let Some((n, slot)) = root { if self.immut.contains(&(self.frame, slot)) { return no(format!("`{}` is a `let`: declare it with `var` to change its fields", n)); } }
        if self.test && self.frame.is_none() {
            let mine = root.map_or(false, |(_, s)| matches!(env.get(s as usize), Some(Some(CV::Plain(_, o))) if self.in_scratch(*o)));
            if !mine { return no("a test changes the state only through requests (`post(...)` and the others); it can read the state directly"); }
        }
        Ok(())
    }
    /// Writing the plain value at `off` from here: an instance's plain state is its own.
    pub(super) fn may_write_plain(&self, off: usize) -> R<()> {
        if !self.in_scratch(off) && self.prog.request.map_or(true, |(a, n)| off < a || off >= a + n) { self.wrote_state.set(true); } // the request struct is the request's own
        for m in &self.prog.instances {
            let Some((start, size)) = self.prog.plain_path(&m.name) else { continue };
            if off >= start && off < start + size && self.module_code().as_deref() != Some(m.name.as_str()) { return Err(self.module_err(m, &format!("the state of {}", m.name))); }
        }
        Ok(())
    }
    /// Adding or freeing items of collection `c` from here: an instance's items and state change only through its functions.
    pub(super) fn may_add_or_free(&self, c: usize, verb: &str) -> R<()> {
        self.wrote_state.set(true);
        if verb == "remove" { REMOVAL.with(|r| { r.borrow_mut().1.insert(c); }); }
        let Some((m, data, items)) = self.coll_owner(c) else { return Ok(()) };
        if self.module_code().as_deref() == Some(m.name.as_str()) || (data && !items) { return Ok(()); }
        Err(format!("{} items of {} with its functions, not `{}.{}()`", verb, m.name, self.prog.colls[c].name, verb))
    }
    /// A field store or free through a value whose kind is known only at run time: refused outside module code when it
    /// could reach a module's key or private state.
    pub(super) fn may_write_dynamic(&self, field: Option<&str>) -> R<()> {
        self.wrote_state.set(true);
        if self.prog.instances.is_empty() || self.module_code().is_some() { return Ok(()); }
        match field {
            None => Ok(()),
            Some(f) => {
                for m in &self.prog.instances { if m.key.as_deref() == Some(f) { return Err(format!("`.{}` through a value whose kind is only known at run time could change the key of {}", f, m.name)); } }
                for (c, coll) in self.prog.colls.iter().enumerate() {
                    if let Some((m, false, _)) = self.coll_owner(c) { if coll.cols.iter().any(|x| x.path == f) { return Err(format!("`.{}` through a value whose kind is only known at run time could change the state of {}", f, m.name)); } }
                }
                Ok(())
            }
        }
    }

    /// A call is about to be made: refuse it, as the interpreter does, if it would nest more than MAX_DEPTH calls deep.
    pub(super) fn depth_check(&mut self) {
        let n = self.inl + 1;
        if self.dep { self.open(&format!("if (dep + {} > {}) {{", n, MAX_DEPTH)); self.fail(500, DEPTH_MSG); self.close("}"); }
        else if n > MAX_DEPTH { self.fail(500, DEPTH_MSG); }
    }

    /// Call the C function for recursive or outlined function `k` with these argument values, generating it if needed.
    pub(super) fn call_spec(&mut self, k: u32, vals: Vec<CV>) -> R<CV> {
        let rec = self.recursive[k as usize];
        let (mut pks, mut cargs) = (Vec::new(), Vec::new());
        for v in vals {
            match v {
                CV::Coll(c, inst) => { pks.push(PK::Coll(c)); cargs.push(format!("(u32)({})", inst)); }
                v @ (CV::Lit(_) | CV::StateStr(..) | CV::PlainStr(..) | CV::Sub(..) | CV::Txt(_)) => {
                    // an outlined function's text parameter keeps its size bound (rounded up to a power of two: fewer copies), which
                    // the sizes of the text its body builds are worked out from
                    let b = if rec { None } else { self.txt_bound(&v).map(|b| b.max(1).next_power_of_two()) };
                    let t = self.txt_of(v)?;
                    pks.push(match b { Some(b) => PK::TxtB(b), None => PK::V(K::Txt) });
                    cargs.push(t);
                }
                CV::Int(e) => { pks.push(PK::V(K::Int)); cargs.push(e); }
                CV::Flt(e) => { pks.push(PK::V(K::Flt)); cargs.push(e); }
                CV::Bool(e) => { pks.push(PK::V(K::Bool)); cargs.push(e); }
                CV::Any(e) => { pks.push(PK::V(K::Any)); cargs.push(e); }
                CV::Item(c, e) => { pks.push(PK::V(K::Item(c))); cargs.push(e); }
                CV::Ref(c, s, i) => { pks.push(PK::V(K::Ref(c))); cargs.push(s); cargs.push(i); }
                CV::Null => pks.push(PK::V(K::Null)),
                CV::Plain(sd, o) => { let n = self.prog.structs[sd].size; self.line(&format!("/*SCRX {} {}*/", o, n)); pks.push(PK::Plain(sd, o)) } // the callee has this place's offset built in: src/scalar.rs must keep it in memory
                _ => return no("passing a struct field or array of an item to a recursive function is not compiled yet"),
            }
        }
        let key = format!("{}{:?}", k, pks);
        let found = self.specs.borrow().iter().position(|s| s.key == key);
        let si = match found { Some(i) => i, None => self.gen_spec(k, key, &pks)? };
        let (name, ret, peak, kept, ret_bound) = { let sp = &self.specs.borrow()[si]; (sp.name.clone(), sp.ret, sp.peak, sp.kept, sp.ret_bound) };
        if !rec {
            // its region text on top of the caller's; what it returns stays until the caller's scope ends, as an inlined call's does
            self.region_peak = self.region_peak.max(self.region_live + peak);
            if self.region_peak > self.text_area() { return no(format!("`{}`: the text in this endpoint's scopes can reach {} bytes, more than the request's text area holds ({} bytes): build less at once, or keep it in a field", self.api.fns[k as usize].name, self.region_peak, self.text_area())); }
            if kept > 0 { self.region_lets.push(Some(kept)); self.region_live += kept; }
        }
        self.uses_spec = true;
        let level = if self.dep { format!("dep + {}", self.inl + 1) } else { format!("{}", self.inl + 1) };
        let mut a = vec!["c".to_string(), level];
        a.extend(cargs);
        let r = self.tmp("rv");
        let (decl, out) = match ret {
            K::Null => (None, CV::Null),
            K::Int => (Some(format!("i64 {} = 0;", r)), CV::Int(r.clone())),
            K::Flt => (Some(format!("double {} = 0;", r)), CV::Flt(r.clone())),
            K::Bool => (Some(format!("int {} = 0;", r)), CV::Bool(r.clone())),
            K::Txt => (Some(format!("Txt {} = {{ 0, 0, 0 }};", r)), CV::Txt(r.clone())),
            K::Any => (Some(format!("Val {} = {{ 0, 0, 0, 0, 0 }};", r)), CV::Any(r.clone())),
            K::Item(c) => (Some(format!("u32 {} = 0;", r)), CV::Item(c, r.clone())),
            K::Ref(c) => (Some(format!("u32 {}_s = 0, {} = 0;", r, r)), CV::Ref(c, format!("{}_s", r), r.clone())),
            K::Coll(c) => (Some(format!("u32 {} = 0;", r)), CV::Coll(c, r.clone())),
            K::Plain(sd, o) => (None, CV::Plain(sd, o)),
        };
        if let Some(d) = decl { self.line(&d); }
        if let K::Ref(_) = ret { a.push(format!("&{}_s", r)); }
        if !matches!(ret, K::Null | K::Plain(..)) { a.push(format!("&{}", r)); }
        self.call(&format!("{}({})", name, a.join(", ")));
        if let (CV::Txt(t), Some(b)) = (&out, ret_bound) { self.txt_bounds.insert(t.clone(), b); }
        Ok(out)
    }

    /// Generate the C function for recursive function `k` with argument kinds `pks`; its result kind is its signature's.
    pub(super) fn gen_spec(&mut self, k: u32, key: String, pks: &[PK]) -> R<usize> {
        let si = self.specs.borrow().len();
        let name = format!("fn_{}_{}", k, si);
        let mut proto = String::from("(Ctx *c, u32 dep");
        for (i, pk) in pks.iter().enumerate() {
            match pk {
                PK::Coll(_) | PK::V(K::Item(_)) => proto += &format!(", u32 p{}", i),
                PK::V(K::Ref(_)) => proto += &format!(", u32 p{}s, u32 p{}k", i, i),
                PK::V(K::Int) => proto += &format!(", i64 p{}", i),
                PK::V(K::Flt) => proto += &format!(", double p{}", i),
                PK::V(K::Bool) => proto += &format!(", int p{}", i),
                PK::V(K::Txt) | PK::TxtB(_) => proto += &format!(", Txt p{}", i),
                PK::V(K::Any) => proto += &format!(", Val p{}", i),
                PK::V(K::Null) | PK::Plain(..) => {}
                PK::V(K::Coll(_)) => proto += &format!(", u32 p{}", i),
                PK::V(K::Plain(..)) => {}
            }
        }
        let rec = self.recursive[k as usize];
        self.specs.borrow_mut().push(Spec { key, name, ret: K::Null, proto: String::new(), body: None, rec, peak: 0, kept: 0, ret_bound: None });
        // the declared result (a function without one gives no value)
        let cand = match &self.api.fns[k as usize].ret { Some(t) => self.ret_kind(t, &self.api.fns[k as usize].name)?, None => K::Null };
        {
            self.specs.borrow_mut()[si].ret = cand;
            match self.spec_body(k, pks, cand) {
                Ok((body, peak, kept, ret_bound)) => {
                    let rp = match cand { K::Null | K::Plain(..) => "", K::Int => ", i64 *ret_v", K::Flt => ", double *ret_v", K::Bool => ", int *ret_v", K::Txt => ", Txt *ret_v", K::Any => ", Val *ret_v", K::Item(_) | K::Coll(_) => ", u32 *ret_v", K::Ref(_) => ", u32 *ret_s, u32 *ret_v" };
                    let mut sp = self.specs.borrow_mut();
                    sp[si].proto = format!("{}{})", proto, rp);
                    sp[si].body = Some(if matches!(cand, K::Plain(..)) { format!("/*SCRALL*/
{}", body) } else { body }); // a returned place is read by the caller: all of this function's places stay in memory
                    (sp[si].peak, sp[si].kept, sp[si].ret_bound) = (peak, kept, ret_bound);
                    return Ok(si);
                }
                Err(SpecErr::Conflict(_)) => { self.specs.borrow_mut().truncate(si); no(format!("`{}` returns a value of another kind than it declares", self.api.fns[k as usize].name)) }
                Err(SpecErr::Other(e)) => { self.specs.borrow_mut().truncate(si); Err(e) }
            }
        }
    }

    /// The body of a C function for `k`, assuming its result kind is `cand`; widens its own locals as endpoints do.
    /// With the body: (for an outlined function) its region text's peak, what stays after it returns, its text result's bound.
    pub(super) fn spec_body(&mut self, k: u32, pks: &[PK], cand: K) -> Result<(String, usize, usize, Option<usize>), SpecErr> {
        let rec = self.recursive[k as usize];
        let f = &self.api.fns[k as usize];
        let (mut slot_kinds, mut ret_kinds): (Vec<((Option<u32>, u32), K)>, Vec<(u32, K)>) = (Vec::new(), Vec::new());
        for _ in 0..32 {
            let mut g = G::new(self.prog, self.api, self.ep, &slot_kinds, &ret_kinds, self.recursive, self.specs);
            g.dep = true;
            g.spec_key = Some(format!("{}{:?}", k, pks));
            g.spec_rec = rec;
            g.frame = Some(k);
            g.inlining.push(k);
            let sel = if has_collecting_loop(self.api, &f.body) { Some("hm".to_string()) } else { None };
            if sel.is_some() { g.line("u32 hm;"); g.call("c->h->sel_mark(c, &hm)"); }
            g.ret.push(Ret { dest: None, var: String::new(), label: String::new(), kind: Some(cand), func: true, sel: sel.clone(), bound: None, unbounded: rec, mark: "rmark".into(), ivs: Vec::new() });
            let mut fenv: Vec<Option<CV>> = vec![None; f.slots as usize];
            for (i, pk) in pks.iter().enumerate() {
                fenv[i] = Some(match pk {
                    PK::Coll(c) => CV::Coll(*c, format!("p{}", i)),
                    PK::V(K::Item(c)) => CV::Item(*c, format!("p{}", i)),
                    PK::V(K::Ref(c)) => CV::Ref(*c, format!("p{}s", i), format!("p{}k", i)),
                    PK::V(K::Int) => CV::Int(format!("p{}", i)),
                    PK::V(K::Flt) => CV::Flt(format!("p{}", i)),
                    PK::V(K::Bool) => CV::Bool(format!("p{}", i)),
                    PK::V(K::Txt) => CV::Txt(format!("p{}", i)),
                    PK::TxtB(b) => { g.txt_bounds.insert(format!("p{}", i), *b); CV::Txt(format!("p{}", i)) }
                    PK::V(K::Any) => CV::Any(format!("p{}", i)),
                    PK::V(K::Null) => CV::Null,
                    PK::Plain(sd, o) | PK::V(K::Plain(sd, o)) => CV::Plain(*sd, *o),
                    PK::V(K::Coll(c)) => CV::Coll(*c, format!("p{}", i)),
                });
            }
            let r = g.block(&f.body, &mut fenv);
            if sel.is_some() && r.is_ok() { g.call("c->h->sel_end(c, hm)"); } // falling off the end
            let ends_by_itself = matches!(f.body.last(), Some(Stmt::Return(_)) | Some(Stmt::Fail(..)) | Some(Stmt::Respond(..)) | Some(Stmt::Forward { .. }));
            match (r, g.widen) {
                (Ok(()), _) if !ends_by_itself && !fits(K::Null, cand) => return Err(SpecErr::Other(format!("`{}` can end without returning its `{}`: return a value on every path", f.name, f.ret.as_ref().map_or(String::new(), |t| t.to_string())))),
                // the function's parameters (p0, p1, ...: in its prototype, not its body) are its locals too
                (Ok(()), _) => {
                    if !rec && !g.region_unbounded.is_empty() { return Err(SpecErr::Other(format!("region text kept across a loop's rounds: {}", g.region_unbounded[0]))); }
                    let body = resolve_return_cuts(&g.body, "rmark");
                    let ps: Vec<String> = (0..f.params.len()).map(|i| format!("p{}", i)).collect();
                    let ret_bound = match g.ret.first() { Some(r) if !r.unbounded => r.bound, _ => None };
                    let texty = matches!(cand, K::Txt | K::Any);
                    let kept = if texty { g.region_live.max(ret_bound.unwrap_or(0)) } else { 0 };
                    return check_profile_in(&body, &ps).map(|_| (body, g.region_peak, kept, ret_bound)).map_err(|e| SpecErr::Other(format!("C safety profile: {}", e)))
                }
                (Err(_), Some(Widen::Ret(f2, u))) if f2 == k => return Err(SpecErr::Conflict(u)),
                (Err(_), Some(Widen::Slot(fr, sl, kk))) => match slot_kinds.iter_mut().find(|x| x.0 == (fr, sl)) { Some(x) => x.1 = unify(x.1, kk), None => slot_kinds.push(((fr, sl), kk)) },
                (Err(_), Some(Widen::Ret(f2, kk))) => match ret_kinds.iter_mut().find(|x| x.0 == f2) { Some(x) => x.1 = unify(x.1, kk), None => ret_kinds.push((f2, kk)) },
                (Err(e), None) => return Err(SpecErr::Other(place_err(g.at, e))),
            }
        }
        Err(SpecErr::Other("kinds did not settle".into()))
    }

    /// Is struct `sid` a field of the state, directly or inside plain structs (so that a function can be given it as a view)?
    pub(super) fn plain_used(&self, sid: usize) -> bool {
        fn walk(prog: &Program, ty: &Ty, sid: usize, seen: &mut Vec<usize>) -> bool {
            match ty {
                Ty::Struct(s) if *s == sid => true,
                Ty::Struct(s) => { if seen.contains(s) { return false; } seen.push(*s); prog.structs[*s].fields.iter().any(|f| walk(prog, &f.1, sid, seen)) }
                _ => false,
            }
        }
        let mut seen = Vec::new();
        self.prog.globals.iter().any(|g| walk(self.prog, &g.ty, sid, &mut seen))
    }
    /// The kind a function's declared result stands for (a struct result is written into the place the call was given).
    pub(super) fn ret_kind(&self, t: &crate::lang::PTy, f: &str) -> R<K> { self.decl_kind(t, f) }
    /// The kind a signature's type stands for (an item type `T?` is a reference; a scalar `T?` is its scalar, and also takes null).
    /// `f`: the function whose signature it is; in a module instance's function (`inst.f`), an item type names the instance's own items.
    pub(super) fn decl_kind(&self, t: &crate::lang::PTy, f: &str) -> R<K> {
        let inst = f.rsplit_once('.').map(|x| format!("{}.", x.0));
        let scalar = |n: &str| -> Option<K> { Some(match n {
            "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" => K::Int, "f32" | "f64" => K::Flt, "bool" => K::Bool,
            "string" | "utf8" | "ascii" | "text" => K::Txt, _ => return None }) };
        if let Some(fld) = &t.field {
            // `T.key`: the type of a struct's field
            let Some(sd) = self.prog.structs.iter().find(|x| x.name == t.name) else { return no(format!("`{}` is not a struct", t.name)) };
            let Some((_, ty)) = sd.fields.iter().find(|x| x.0 == *fld) else { return no(format!("`{}` has no field `{}`", t.name, fld)) };
            let tn = self.prog.show(ty);
            let base = tn.split(|c: char| c == '<' || c == ' ' || c == '[').next().unwrap_or("").to_string();
            return scalar(&base).ok_or_else(|| format!("`{}.{}` is {}: a signature takes a field's type only when it is a number, bool or string", t.name, fld, tn));
        }
        if let Some(k) = scalar(&t.name) { return Ok(k); }
        if self.prog.enums.iter().any(|e| e.name == t.name) { return Ok(K::Int); }
        if t.name == "pool" || t.name == "list" {
            // a collection: `pool<T>` / `list<T>` (`in path` when several hold T)
            let Some(item) = &t.arg else { return no(format!("`{}` needs its item type: `{}<T>`", t.name, t.name)) };
            let Some(sid) = self.prog.structs.iter().position(|x| x.name == *item) else { return no(format!("`{}` is not a struct", item)) };
            let found: Vec<usize> = (0..self.prog.colls.len()).filter(|&c| { let x = &self.prog.colls[c]; x.item == Some(sid) && x.is_list() == (t.name == "list") && t.coll.as_ref().map_or(true, |p| x.name == *p) }).collect();
            return match found.as_slice() {
                [c] => Ok(K::Coll(*c)),
                [] => no(format!("no {} of `{}` items{}", t.name, item, t.coll.as_ref().map_or(String::new(), |p| format!(" at `{}`", p)))),
                _ => no(format!("several collections are `{}`: write `{} in path` ({})", t, t, found.iter().map(|&c| self.prog.colls[c].name.clone()).collect::<Vec<_>>().join(", "))),
            };
        }
        let Some(sid) = self.prog.structs.iter().position(|x| x.name == t.name) else {
            return no(format!("`{}` is not a type: a parameter is a number (i64, f64), bool, string, or an item (a struct held by a collection)", t.name));
        };
        let mut holders: Vec<usize> = (0..self.prog.colls.len()).filter(|&c| self.prog.colls[c].item == Some(sid) && t.coll.as_ref().map_or(true, |p| self.prog.colls[c].name == *p)).collect();
        holders.extend((0..self.prog.tables.len()).filter(|&k| self.prog.tables[k].item == sid && t.coll.as_ref().map_or(true, |p| self.prog.tables[k].name == *p)).map(|k| TBASE + k));
        if holders.len() > 1 { if let Some(i) = &inst { let own: Vec<usize> = holders.iter().copied().filter(|&c| coll_name(self.prog, c).starts_with(i.as_str())).collect(); if own.len() == 1 { holders = own; } } }
        let c = match holders.as_slice() {
            [c] => *c,
            // a struct that is a field of the state (not an item) is passed as a view of where it lives: the argument's own place decides, so this only stands for "that struct"
            [] if t.coll.is_none() && self.plain_used(sid) => {
                if t.opt { return no(format!("`{}?`: a plain struct is part of the state, it cannot be null", t.name)); }
                return Ok(K::Plain(sid, usize::MAX));
            }
            [] => return no(match &t.coll { Some(p) => format!("`{}` is not held by `{}`", t.name, p), None => format!("no collection holds `{}` items", t.name) }),
            _ => return no(format!("several collections hold `{}` items: write `{} in path` with the one this takes ({})", t.name, t.name, holders.iter().map(|&c| coll_name(self.prog, c)).collect::<Vec<_>>().join(", "))),
        };
        Ok(if t.opt { K::Ref(c) } else { K::Item(c) })
    }
    /// A value given where a signature declares type `t` (`what`: "parameter `x` of `f`" or "result of `f`"): checked, an integer
    /// widened to a number for a `f64`, an item kept as an item for `T?`.
    pub(super) fn to_declared(&mut self, v: CV, t: &crate::lang::PTy, f: &str, what: &str) -> R<CV> {
        // a plain struct (a field of the state) given for its own type: a view, whether or not the same struct is also an item type somewhere
        if let CV::Plain(sd, _) = &v {
            if t.name == self.prog.structs[*sd].name && t.arg.is_none() && t.field.is_none() && t.coll.is_none() && !t.opt { return Ok(v); }
        }
        let want = self.decl_kind(t, f)?;
        let got = kind(&v);
        let ok = match (got, want) {
            (Some(K::Int), K::Flt) => return Ok(match v { CV::Int(e) => CV::Flt(format!("((double)({}))", e)), v => v }),
            (Some(K::Null), _) => t.opt,
            (Some(K::Item(c)), K::Ref(d)) => c == d,
            (Some(K::Ref(c)), K::Ref(d)) => c == d,
            (Some(K::Ref(_)), K::Item(_)) => return no(format!("{} is `{}`, given a value that may be null: check it first with `if exists(x)`, or declare `{}?`", what, t, t)),
            (Some(a), b) => a == b,
            (None, _) => false,
        };
        if !ok { return no(format!("{} is `{}`, given {}", what, t, got.map_or("a struct field or array".to_string(), |k| kname(self.prog, k)))); }
        Ok(v)
    }

    pub(super) fn tmp(&mut self, p: &str) -> String {
        self.n += 1;
        let t = format!("{}{}", p, self.n);
        if ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"].contains(&t.as_str()) { format!("{}_{}", p, self.n) } else { t } // never a type's name
    }
    pub(super) fn line(&mut self, s: &str) {
        if s.ends_with(": ;") || s.contains("goto ") { self.checked.clear(); } // control joins or leaves here: a test seen before may not have run
        else if !self.checked.is_empty() { self.checked.retain(|(x, _)| !writes_name(s, x)); } // an index given a new value must be tested again
        for _ in 0..self.depth + 1 { self.body.push_str("    "); } self.body.push_str(s); self.body.push('\n');
    }
    /// The interval a C integer is proven to lie in: a literal, or a name recorded in `ranges` (names that are never assigned again).
    pub(super) fn interval(&self, s: &str) -> Option<(i64, i64)> {
        if let Some(d) = s.strip_suffix("LL") { if let Ok(v) = d.parse::<i64>() { return Some((v, v)); } }
        if let Some(r) = load_interval(s) { return Some(r); }
        let name = s.strip_prefix("(i64)").unwrap_or(s);
        let known = self.ranges.get(name).or_else(|| self.var_iv.get(name)).copied();
        // an index whose bounds test has passed in this straight run of code (`checked`: dropped at a branch and when the index is assigned) is inside that array, as the test said
        let tested = self.checked.iter().filter(|(x, n)| x == name && *n > 0).map(|(_, n)| (0i64, (*n).min(i64::MAX as u64) as i64 - 1)).fold(None, |acc: Option<(i64, i64)>, r| Some(acc.map_or(r, |a| (a.0.max(r.0), a.1.min(r.1)))));
        match (known, tested) { (Some(k), Some(t)) => Some((k.0.max(t.0), k.1.min(t.1))), (k, t) => k.or(t) }
    }
    /// A value stored into a field declared with a range (`n: i64 in 0..100`): an integer, tested against the range unless the range analysis
    /// proves it inside; a value outside fails the request (400), naming the field.
    pub(super) fn in_field_range(&mut self, v: CV, (lo, hi): (i64, i64), field: &str) -> R<CV> {
        let x = self.int_or(v, 400, "a field with a range takes an integer");
        let x = self.hold_int(x);
        if let Some((a, b)) = self.interval(&x) { if a >= lo && b <= hi { return Ok(CV::Int(x)); } }
        let msg = format!("`{}` must be in {}..{}", field, lo, hi);
        self.line(&format!("if (UNLIKELY({x} < {} || {x} > {})) FAIL(400, {}, {});", c_i64(lo), c_i64(hi), c_lit(msg.as_bytes()), msg.len()));
        Ok(CV::Int(x))
    }
    /// Is the byte at plain-state offset `off` kept out of the undo log (an unlogged field)?
    pub(super) fn unlogged_at(&self, off: usize) -> bool { self.in_scratch(off) || self.prog.unlogged.iter().any(|(a, b)| off >= *a && off < *b) }
    /// A store of `range` (None: not proven) into the plain integer field at `at`, for the whole-program field ranges.
    pub(super) fn field_store_seen(&mut self, at: usize, range: Option<(i64, i64)>) {
        if self.in_scratch(at) || self.prog.request.map_or(false, |(a, n)| at >= a && at < a + n) { return; } // the request struct starts zeroed every request; left alone
        FIELD_RANGES.with(|fr| {
            let mut fr = fr.borrow_mut();
            let e = fr.1.entry(at).or_insert(range);
            *e = match (*e, range) { (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))), _ => None };
        });
    }
    /// A value read from a field declared with a range: the range analysis may assume it.
    pub(super) fn with_field_range(&mut self, v: CV, range: Option<(i64, i64)>) -> CV {
        match (v, range) {
            (CV::Int(e), Some(r)) => {
                // a range recorded from the stores seen may be looser than what the load itself guarantees (a `u8` element is 0..255 whatever
                // was stored: a store of -255..255 is checked, and a failed store changes nothing): keep the narrower of the two ends
                let r = match self.interval(&e) { Some((a, b)) if a.max(r.0) <= b.min(r.1) => (a.max(r.0), b.min(r.1)), _ => r };
                let n = self.hold_int(e); if !self.var_names.contains(&n) { self.ranges.insert(n.clone(), r); } CV::Int(n)
            }
            (v, _) => v,
        }
    }
    /// Check that array position `k` is in 0..n: nothing for a constant in range, else a comparison in line, with the
    /// helper (which builds the error) only when it fails.
    pub(super) fn elem_check(&mut self, k: &str, n: u64) {
        if let Some(v) = k.strip_suffix("LL").and_then(|d| d.parse::<u64>().ok()) { if v < n { return; } }
        if let Some((lo, hi)) = self.interval(k) { if lo >= 0 && (hi as u64) < n { return; } }
        // the same index against the same size, tested earlier in this straight run of code (as in `a[i] = a[i] + 1`)
        if self.checked.iter().any(|(x, m)| x == k && *m == n) { return; }
        // MITO_EXPLAIN_INDEX=1: say which index tests are kept and what is known of each index (to see what a proof is missing)
        if std::env::var_os("MITO_EXPLAIN_INDEX").is_some() { eprintln!("index test kept: line {}, index {} (known: {:?}), array of {}", self.at.0, k, self.interval(k), n); }
        self.line(&format!("if (RANGE_({k} < 0 || {k} >= {n}LL)) {{"));
        self.depth += 1;
        self.call(&format!("c->h->elem(c, {}, {})", k, n));
        self.depth -= 1;
        self.line("}");
        self.checked.push((k.to_string(), n));
    }
}
