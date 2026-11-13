//! Versioned loops, stores, calls, blocks and statement helpers of G.
use super::*;

impl<'p> G<'p> {
/// Generate a loop statement (the loop, its body, and the text-area mark and cut when the body can build a string).
    pub(super) fn loop_stmt(&mut self, it: &'p Iter, body: &'p [Stmt], env: &mut Vec<Option<CV>>, collect: bool) -> R<()> {
        self.loop_body = Some(LoopBody::Stmts(body));
        self.each(it, env, collect, |g, env, _| {
            // strings built in an iteration are dropped at its end unless the body hands one to a variable declared before the loop;
            // the mark and the cut are only emitted for a body whose code can build a string
            let (start, first) = (g.body.len(), g.txt_escapes.len());
            g.block(body, env)?;
            let code = &g.body[start..];
            let builds = ["->lit(", "->col_txt(", "->plain_txt(", "->arr_txt(", "->builtin(", "->binop(", "->input(", "->field_val("].iter().any(|h| code.contains(h))
                || g.specs.borrow().iter().any(|sp| code.contains(&format!("{}(c", sp.name)));
            if builds && !g.txt_escapes[first..].iter().any(|slot| *slot < it.slot) {
                let mark = g.tmp("tm");
                let kept = std::mem::take(&mut g.body);
                g.line(&format!("u32 {};", mark));
                g.call(&format!("c->h->text_len(c, &{})", mark));
                let head = std::mem::replace(&mut g.body, kept);
                g.body.insert_str(start, &head);
                g.call(&format!("c->h->text_cut(c, {})", mark));
            }
            Ok(())
        })
    }

    /// Loop versioning. For a range loop (`foreach v in lo..hi`, internally lo up to hi excluded), and for a loop that steps a variable (`while j <= limit` with one
    /// `j = j + step` at the top level of the body), find the accesses to plain-state arrays whose index is the counter (or the
    /// stepped variable) plus or minus a value the loop does not change. Emit, before the loop, a test that every such index
    /// stays inside its array for the whole loop, and save the ranges the loop stores into to the undo log in one pass when it
    /// does. Returns the test's C name and the accesses the fast copy may do without their own range test and undo-log test;
    /// None when the loop has no such access. The checked copy runs when the test fails, so behavior is unchanged.
    pub(super) fn version_guard(&mut self, it: &'p Iter, body: &'p [Stmt], env: &mut Vec<Option<CV>>) -> R<Option<(String, std::collections::HashSet<*const Expr>, Vec<(String, (i64, i64))>, Option<(i64, i64)>)>> {
        let (Some(hi), None) = (&it.hi, &it.via) else { return Ok(None) };
        if self.versioned >= 3 { return Ok(None); }
        let assigned = self.assigned_here(body);
        // the counter's values: lo..hi (evaluated again by the loop itself, so they must be free of effects)
        let mut plans: Vec<(u32, Vec<Site<'p>>, Bound<'p>)> = Vec::new();
        if pure(&it.coll) && pure(hi) {
            let mut sites = Vec::new();
            for st in body { stmt_sites(st, it.slot, &assigned, &mut sites); }
            if !sites.is_empty() { plans.push((it.slot, sites, Bound::Range(&it.coll, hi))); }
        }
        // a stepped variable: `while ... && j <= limit && ...`, one top-level `j = j + step`, the accesses in the statements before it
        if let Some(w) = &it.whil {
            for (slot, limit, inclusive) in upper_bounds(w) {
                if assigned.contains(&slot) && invariant(limit, &assigned) {
                    let steps: Vec<usize> = body.iter().enumerate().filter(|(_, st)| assigns_slot(st, slot)).map(|(i, _)| i).collect();
                    if steps.len() != 1 { continue; }
                    let Stmt::Assign(Expr::Var(_, VarRes::Local(s2)), Expr::Bin(op, a, step)) = &body[steps[0]] else { continue };
                    if *s2 != slot || op != "+" || !matches!(&**a, Expr::Var(_, VarRes::Local(s3)) if *s3 == slot) || !invariant(step, &assigned) { continue; }
                    if body.iter().enumerate().any(|(i, st)| i != steps[0] && assigns_slot(st, slot)) { continue; }
                    let mut sites = Vec::new();
                    for st in &body[..steps[0]] { stmt_sites(st, slot, &assigned, &mut sites); }
                    if !sites.is_empty() { plans.push((slot, sites, Bound::Stepped(limit, inclusive, step))); }
                }
            }
        }
        // a variable stepped once per iteration (`k = k + 1` at the top level of the body, nowhere else assigned), bounded by the number of
        // iterations of the range (as the merge loop's output position): the accesses before the step and those after it
        if pure(&it.coll) && pure(hi) {
            let stepped: Vec<u32> = plans.iter().map(|p| p.0).collect();
            for (si, st) in body.iter().enumerate() {
                let Stmt::Assign(Expr::Var(_, VarRes::Local(slot)), Expr::Bin(op, a, step)) = st else { continue };
                let slot = *slot;
                if op != "+" || !matches!(&**a, Expr::Var(_, VarRes::Local(s3)) if *s3 == slot) || !invariant(step, &assigned) || stepped.contains(&slot) || slot == it.slot { continue; }
                if body.iter().enumerate().any(|(i, other)| i != si && assigns_slot(other, slot)) { continue; }
                let (mut before, mut after) = (Vec::new(), Vec::new());
                for other in &body[..si] { stmt_sites(other, slot, &assigned, &mut before); }
                for other in &body[si + 1..] { stmt_sites(other, slot, &assigned, &mut after); }
                if !before.is_empty() { plans.push((slot, before, Bound::Counted(&it.coll, hi, step, false))); }
                if !after.is_empty() { plans.push((slot, after, Bound::Counted(&it.coll, hi, step, true))); }
            }
        }
        if plans.is_empty() { return Ok(None); }
        let guard = self.tmp("fast");
        self.line(&format!("int {} = 1;", guard));
        const BIG: &str = "1099511627776LL"; // 2^40: every value in the test is checked against it first, so no sum in it overflows
        let mut fast = std::collections::HashSet::new();
        let mut saves: Vec<(String, String, String)> = Vec::new(); // (nonempty test, byte offset, byte count)
        // what the guard proves about values, for the fast copy's range analysis: every bound is within 2^40, so the counter and a stepped
        // variable (which stays below its limit plus one step) are within 2^41, and arithmetic on them that cannot overflow needs no test
        const BOUND: i64 = 1 << 40;
        let mut hints: Vec<(String, (i64, i64))> = Vec::new();
        let mut counter: Option<(i64, i64)> = None;
        for (slot, sites, bound) in plans {
            // the smallest and largest value of the counter or stepped variable in the loop, and whether the loop runs at all
            let (vmin, vmax) = match bound {
                Bound::Range(lo, hi) => {
                    let lo = { let v = self.expr(lo, env, None)?; let k = self.int_or(v, 500, "a range needs integers"); self.hold_int(k) };
                    let hi = { let v = self.expr(hi, env, None)?; let k = self.int_or(v, 500, "a range needs integers"); self.hold_int(k) };
                    counter = Some((-BOUND, BOUND));
                    self.line(&format!("if (!({lo} >= -{BIG} && {lo} <= {BIG} && {hi} >= -{BIG} && {hi} <= {BIG})) {guard} = 0;"));
                    (lo, format!("({} - 1)", hi))
                }
                Bound::Counted(lo, hi, step, after) => {
                    let Some(CV::Int(start)) = env[slot as usize].clone() else { continue };
                    let start = self.hold_int(start);
                    let lo = { let v = self.expr(lo, env, None)?; let k = self.int_or(v, 500, "a range needs integers"); self.hold_int(k) };
                    let hi = { let v = self.expr(hi, env, None)?; let k = self.int_or(v, 500, "a range needs integers"); self.hold_int(k) };
                    let stepv = { let v = self.expr(step, env, None)?; let k = self.int_or(v, 500, "a step must be an integer"); self.hold_int(k) };
                    // steps times iterations within 2^40, so the bound below cannot overflow
                    self.line(&format!("if (!({start} >= -{BIG} && {start} <= {BIG} && {lo} >= -{BIG} && {lo} <= {BIG} && {hi} >= -{BIG} && {hi} <= {BIG} && {stepv} > 0 && {stepv} <= {BIG} && ({hi} <= {lo} || {stepv} <= {BIG} / ({hi} - {lo})))) {guard} = 0;"));
                    if let Some(CV::Int(name)) = env[slot as usize].clone() { hints.push((name, (-2 * BOUND, 2 * BOUND))); }
                    if let Expr::Var(_, VarRes::Local(ss)) = step { if let Some(CV::Int(name)) = env[*ss as usize].clone() { hints.push((name, (1, BOUND))); } }
                    let extra = if after { "" } else { " - 1" };
                    (start.clone(), format!("({} + ({} - {}{}) * {})", start, hi, lo, extra, stepv))
                }
                Bound::Stepped(limit, inclusive, step) => {
                    let step_expr = step;
                    let Some(CV::Int(start)) = env[slot as usize].clone() else { continue };
                    let start = self.hold_int(start);
                    let limit = { let v = self.expr(limit, env, None)?; let k = self.int_or(v, 500, "a bound must be an integer"); self.hold_int(k) };
                    let step = { let v = self.expr(step, env, None)?; let k = self.int_or(v, 500, "a step must be an integer"); self.hold_int(k) };
                    self.line(&format!("if (!({step} > 0 && {step} <= {BIG} && {start} >= -{BIG} && {start} <= {BIG} && {limit} >= -{BIG} && {limit} <= {BIG})) {guard} = 0;"));
                    if let Some(CV::Int(name)) = env[slot as usize].clone() { hints.push((name, (-2 * BOUND, 2 * BOUND))); }
                    if let Expr::Var(_, VarRes::Local(ss)) = step_expr { if let Some(CV::Int(name)) = env[*ss as usize].clone() { hints.push((name, (1, BOUND))); } }
                    (start, if inclusive { limit } else { format!("({} - 1)", limit) })
                }
            };
            self.line(&format!("if ({guard} && !({vmin} >= -{BIG} && {vmin} <= {BIG} && {vmax} >= -{BIG} && {vmax} <= {BIG})) {guard} = 0;"));
            // a short loop keeps its per-access tests: the test and the bulk save would cost more than they save
            self.line(&format!("if ({guard} && {vmax} - {vmin} < 32) {guard} = 0;"));
            // a loop that writes no state cannot change a text field's length: its reads by index are bounded by the length read here
            let reads_only = !writes(self.api, body, &mut Vec::new());
            for site in sites {
                let (off, n, esz) = match self.expr(site.arr, env, None)? {
                    CV::PlainArr(off, e, n) => (off, format!("{}LL", n), self.prog.plain_size(&e).0),
                    CV::PlainStr(off) if reads_only && !site.store => (off + 4, format!("(i64)LD_U32({}, 0)", off), 1),
                    _ => continue,
                };
                let (smin, smax) = match site.off {
                    Off::Zero => (vmin.clone(), vmax.clone()),
                    Off::Plus(c) | Off::Minus(c) | Off::From(c) => {
                        let c = { let v = self.expr(c, env, None)?; let k = self.int_or(v, 500, "an index must be an integer"); self.hold_int(k) };
                        self.line(&format!("if (!({c} >= -{BIG} && {c} <= {BIG})) {guard} = 0;"));
                        match site.off {
                            Off::Plus(_) => (format!("({} + {})", vmin, c), format!("({} + {})", vmax, c)),
                            Off::Minus(_) => (format!("({} - {})", vmin, c), format!("({} - {})", vmax, c)),
                            _ => (format!("({} - {})", c, vmax), format!("({} - {})", c, vmin)),
                        }
                    }
                };
                let nonempty = format!("{} >= {}", vmax, vmin);
                self.line(&format!("if ({guard} && {nonempty} && !({smin} >= 0 && {smax} < {n})) {guard} = 0;"));
                // an array kept out of the undo log has nothing to save before the loop stores into it
                if site.store && !self.unlogged_at(off) { saves.push((nonempty, format!("{}ULL + (u64)({}) * {}ULL", off, smin, esz), format!("(u64)({} - {} + 1) * {}ULL", smax, smin, esz))); }
                fast.insert(site.idx as *const Expr);
            }
        }
        if fast.is_empty() { return Ok(None); }
        // a counter whose loop has no array access to version still gets nothing: only plans with sites reach here
        for (nonempty, at, count) in saves {
            self.line(&format!("if ({guard} && {nonempty}) {{"));
            self.depth += 1;
            self.call(&format!("c->h->save_range(c, {}, {})", at, count));
            self.depth -= 1;
            self.line("}");
        }
        Ok(Some((guard, fast, hints, counter)))
    }

    /// One store of `size` bytes (the bits `bits`) at byte offset `addr`. A naturally aligned one (`aligned`: it cannot cross a 64-byte line or a
    /// page) is inline: the undo-log test and the store. No commit test: plain state is committed at start, and a store to an item is to a live
    /// item (from `add`, a checked index or a checked reference), whose memory `add` committed and which trimming (only between requests, only
    /// outside the live ranges) never gives back. Any other store goes to the runtime's helper, which does the checks for any alignment.
    pub(super) fn emit_wr(&mut self, plain: bool, aligned: bool, addr: &str, size: u64, bits: &str) {
        if !aligned {
            let wr = if plain { "wr_plain" } else { "wr_int" };
            self.call(&format!("c->h->{}(c, {}, {}, {})", wr, addr, size, bits));
            return;
        }
        let a = self.tmp("a");
        self.open("{");
        self.line(&format!("u64 {} = (u64)({});", a, addr));
        if !std::mem::take(&mut self.skip_save) { self.line(&format!("SAVE_({}, {});", a, size)); }
        self.line(&format!("ST{}_({}, {});", size, a, bits));
        self.close("}");
    }
    /// Store `v` in a scalar column of item `idx` (element `k` of an array column when `k` is given) as a typed store in
    /// line, when its kind is known here: see `scalar_store`. Returns false when it does not apply.
    pub(super) fn int_store(&mut self, c: usize, idx: &str, ci: usize, k: Option<&str>, v: &CV, slow: &str) -> R<bool> {
        let col = &self.prog.colls[c].cols[ci];
        let s = match &col.kind { CKind::Scalar(s) => *s, CKind::Arr(s, _) => *s, _ => return Ok(false) };
        let code = plain_code(&Ty::Scalar(s)).map_or(0, |x| x.0);
        let (off, whole) = (col.off, col.kind.size());
        let size = scalar_bytes(code);
        let addr = match k { Some(k) => format!("{}ULL + (u64)({}) * {}ULL + (u64)({}) * {}ULL", off, idx, whole, k, size), None => format!("{}ULL + (u64)({}) * {}ULL", off, idx, whole) };
        let aligned = off % size as usize == 0 && whole % size as usize == 0;
        self.scalar_store(code, addr, v, slow, false, aligned)
    }
    /// A typed store in line of an integer, bool or f64 `v` into a scalar of kind `code` (see `plain_code`) at byte
    /// offset `addr`: the generated code checks the value fits (an integer in a narrower column), and the generic helper
    /// (`slow`, with `@VAL` for the value) runs only for one that does not, to make the error. False if it does not apply.
    pub(super) fn scalar_store(&mut self, code: u32, addr: String, v: &CV, slow: &str, plain: bool, aligned: bool) -> R<bool> {
        let w = self.tmp("w");
        let size = scalar_bytes(code);
        match (code, v) {
            (1..=8, CV::Int(e)) => {
                let (lo, hi): (Option<&str>, Option<&str>) = match code {
                    1 => (Some("-128"), Some("127")), 2 => (Some("-32768"), Some("32767")), 3 => (Some("(-2147483647LL - 1)"), Some("2147483647")),
                    4 => (None, None), 5 => (Some("0"), Some("255")), 6 => (Some("0"), Some("65535")), 7 => (Some("0"), Some("4294967295LL")), _ => (Some("0"), None),
                };
                self.open("{");
                self.line(&format!("i64 {} = {};", w, e));
                let cond = match (lo, hi) {
                    (Some(l), Some(h)) => Some(format!("{w} < {l} || {w} > {h}")),
                    (Some(l), None) => Some(format!("{w} < {l}")),
                    _ => None,
                };
                // no test when the value is proven to fit (as `x & 255` for a u8)
                let bounds: Option<(i64, i64)> = match code { 1 => Some((-128, 127)), 2 => Some((-32768, 32767)), 3 => Some((i32::MIN as i64, i32::MAX as i64)), 5 => Some((0, 255)), 6 => Some((0, 65535)), 7 => Some((0, u32::MAX as i64)), 8 => Some((0, i64::MAX)), _ => None };
                let cond = match (cond, bounds, self.interval(e)) { (Some(_), Some((bl, bh)), Some((vl, vh))) if vl >= bl && vh <= bh => None, (c, _, _) => c };
                match cond {
                    Some(cond) => {
                        self.line(&format!("if (UNLIKELY({})) {{", cond));
                        self.depth += 1;
                        let vv = self.val(CV::Int(w.clone()))?;
                        self.call(&slow.replace("@VAL", &vv));
                        self.depth -= 1;
                        self.line("} else {");
                        self.depth += 1;
                        self.emit_wr(plain, aligned, &addr, size, &w);
                        self.depth -= 1;
                        self.line("}");
                    }
                    None => self.emit_wr(plain, aligned, &addr, size, &w),
                }
                self.close("}");
            }
            (11, CV::Bool(e)) => self.emit_wr(plain, aligned, &addr, 1, &format!("(({}) ? 1 : 0)", e)),
            (10, CV::Flt(e)) => self.emit_wr(plain, aligned, &addr, 8, &format!("BITS_F64({})", e)),
            _ => return Ok(false),
        }
        Ok(true)
    }
    /// The item index of position `k` in collection `c`, instance `inst`, in a new `u32` (its name is returned). A pool
    /// is checked in line from its layout (in range, below the high-water mark, live bit set: what `h_index` checks); a
    /// failure, or a list, goes through the helper, which finds the position not live and says so.
    pub(super) fn index_of(&mut self, c: usize, inst: &str, k: &str) -> String {
        let t = self.tmp("i");
        self.line(&format!("u32 {};", t));
        let coll = &self.prog.colls[c];
        if let Kind::Pool(_) = coll.kind {
            let (cap, hdr, live, words) = (coll.cap, coll.hdr, coll.live, coll.words());
            let q = self.tmp("q");
            self.open("{");
            self.line(&format!("i64 {} = {};", q, k));
            // below the high-water mark every slot is live when nothing in the program removes from the pool (a removal frees a slot; `clear` lowers the mark)
            if REMOVAL.with(|r| r.borrow().0.contains(&c)) {
                self.line(&format!("if (RANGE_({q} < 0 || {q} >= {cap}LL || {q} >= (i64)LD_U32({hdr}, {inst}))) {{"));
            } else {
                self.line(&format!("if (RANGE_({q} < 0 || {q} >= {cap}LL || {q} >= (i64)LD_U32({hdr}, {inst}) || !((LD_U64({live}, (i64)({inst}) * {words} + ({q} >> 6)) >> ({q} & 63)) & 1))) {{"));
            }
            self.depth += 1;
            self.call(&format!("c->h->index(c, {}, {}, {}, &{})", c, inst, q, t));
            self.depth -= 1;
            self.line("}");
            self.line(&format!("{} = (u32)((u64)({}) * {}ULL + (u64){});", t, inst, cap, q));
            self.close("}");
        } else {
            self.call(&format!("c->h->index(c, {}, {}, {}, &{})", c, inst, k, t));
        }
        t
    }
    /// Call a helper that returns 0 or -1.
    pub(super) fn call(&mut self, s: &str) {
        // a table write or a hand-off to another worker: not seen by the offset-aware checks of plain state and collections
        if let Some(h) = s.strip_prefix("c->h->") {
            if ["tset(", "tadd(", "tdel(", "tclear(", "tinsert(", "forward"].iter().any(|w| h.starts_with(w)) { self.wrote_state.set(true); }
        }
        match self.or_dead.clone() { Some(l) => self.line(&format!("CALL_OR_DEAD({}, {});", s, l)), None => self.line(&format!("CALL({});", s)) }
    }
    pub(super) fn fail(&mut self, code: u16, m: &str) { self.line(&format!("FAIL({}, {}, {});", code, c_lit(m.as_bytes()), m.len())); }
    pub(super) fn open(&mut self, head: &str) { self.checked.clear(); self.line(head); self.depth += 1; }
    pub(super) fn close(&mut self, tail: &str) { self.checked.clear(); self.depth -= 1; self.line(tail); }

    pub(super) fn block(&mut self, stmts: &'p [Stmt], env: &mut Vec<Option<CV>>) -> R<()> {
        let (start, lets0, esc0, live0) = (self.body.len(), self.region_lets.len(), self.txt_escapes.len(), self.region_live);
        let bound_before: Vec<bool> = env.iter().map(|x| x.is_some()).collect();
        let mut live_from: Option<usize> = None; // where the first `let` of built text in this block was compiled
        let scr0 = self.scr_top; // struct locals of this block are given back at its end
        self.block_inner(stmts, env, &mut live_from)?;
        self.scr_top = scr0;
        if self.region_lets.len() > lets0 {
            let code = self.body[start..].to_string();
            // a text built in a `let` is not live at a recursive call (so each endpoint's worst case is known when compiling)
            if let Some(at) = live_from {
                let after = &self.body[at..];
                if self.specs.borrow().iter().any(|sp| sp.rec && after.contains(&format!("{}(c", sp.name))) {
                    return no("text a `let` built or a capped `var` holds is live at a recursive call here: copy it into a field first, or build it after the call");
                }
            }
            // the scope's region released at its end, unless text built in it may outlive it: an outer variable given text here, or the
            // answer cache (which keeps places in the text area between statements). A `return` leaves past the cut, so what it returns
            // stays (the caller's scope gives it back; until then its bytes count as live)
            let returns = stmts_return_value(stmts);
            let escapes = self.txt_escapes[esc0..].iter().any(|s| bound_before.get(*s as usize).copied().unwrap_or(false));
            let cached = code.contains("builtin(c, 61,") || code.contains("builtin(c, 64,");
            if !escapes && !cached {
                let mark = self.tmp("tm");
                let kept = std::mem::take(&mut self.body);
                self.line(&format!("u32 {};", mark));
                self.call(&format!("c->h->text_len(c, &{})", mark));
                let head = std::mem::replace(&mut self.body, kept);
                self.body.insert_str(start, &head);
                self.call(&format!("c->h->text_cut(c, {})", mark));
                // what a `return` leaves in the region outlives the scope only when the function returns text (or a value of a kind
                // known at run time); a number, a bool or an item leaves nothing behind
                let texty = self.ret.last().map_or(true, |r| matches!(r.kind, None | Some(K::Txt) | Some(K::Any)));
                if !returns || !texty { self.region_live = live0; }
            } else if !self.loops.is_empty() {
                // kept past its scope inside a loop: it may add up over the iterations
                let at = self.here(); self.region_unbounded.push(format!("{} (kept across a loop's rounds)", at));
            }
        }
        Ok(())
    }
    /// A `let` or capped `var` of region text, of at most `b` bytes: its size must be known when compiling (from its parts' sizes), and
    /// the endpoint's region text must fit the request's text area
    /// How much text one request may build: 4 MB, or for a command-line program (`@main`) its text area of 4 GiB - 1 (address space, committed as used)
    pub(super) fn text_area(&self) -> usize { if self.api.eps.iter().any(|e| e.method == "MAIN") { 0xFFFF_FFFF } else { TEXT_AREA } }
    pub(super) fn region_add(&mut self, b: Option<usize>, name: &str) -> R<()> {
        self.region_lets.push(b);
        let Some(b) = b else { return no(format!("`let {}` builds text whose size is not known when compiling (a part from a header, the environment, stored data or a recursive function): give it a cap, `var {}: text<N> = ...;`", name, name)) };
        self.region_live += b;
        self.region_peak = self.region_peak.max(self.region_live);
        if self.region_peak > self.text_area() { return no(format!("`{}`: the text in this endpoint's scopes can reach {} bytes, more than the request's text area holds ({} bytes): build less at once, or keep it in a field", name, self.region_peak, self.text_area())); }
        Ok(())
    }
    /// Where code is being generated, for messages: the function being inlined (or the endpoint) and the line
    pub(super) fn here(&self) -> String {
        match self.inlining.last() { Some(k) => format!("fn {} line {}", self.api.fns[*k as usize].name, self.at.0), None => format!("line {}", self.at.0) }
    }
    pub(super) fn block_inner(&mut self, stmts: &'p [Stmt], env: &mut Vec<Option<CV>>, live_from: &mut Option<usize>) -> R<()> {
        let mut restore: Vec<(u32, CV)> = Vec::new();
        let mut fields_kept = 0usize;
        let mut skip = 0usize;
        for (i, s) in stmts.iter().enumerate() {
            if skip > 0 { skip -= 1; continue; }
            // a test of a plain C variable stays good into the next statement until a line writes that variable (see `line`)
            self.checked.retain(|(x, _)| is_ident(x));
            // `let r = table.add { a: x, b: y }`: the assignments to the new row that follow its `add` (as `add { ... }` is written) become
            // one INSERT with those columns
            if let Stmt::Alloc { slot, coll, .. } = s {
                let mut inits: Vec<(&'p str, &'p Expr)> = Vec::new();
                let mut k = i + 1;
                while k < stmts.len() {
                    match &stmts[k] {
                        Stmt::At(..) => {}
                        Stmt::Assign(Expr::Field(b, f), rhs) if matches!(&**b, Expr::Var(_, VarRes::Local(x)) if x == slot) && !expr_uses_slot(rhs, *slot) && !inits.iter().any(|x| x.0 == f) => inits.push((f.as_str(), rhs)),
                        _ => break,
                    }
                    k += 1;
                }
                if !inits.is_empty() {
                    if let CV::Coll(c, inst) = self.expr(coll, env, None)? {
                        if let Some(t) = tab(c) {
                            // the line markers among them are kept for errors' places; the assignments are compiled here
                            let tb = self.prog.tables[t].clone();
                            let mut cols = Vec::new(); let mut vals = Vec::new();
                            for (f, rhs) in &inits { let (ci, vv) = self.table_value(t, f, rhs, env)?; cols.push(ci); vals.push(vv); }
                            let names: Vec<String> = cols.iter().map(|ci| crate::layout::sql_ident(&tb.cols[*ci].sql)).collect();
                            let parent = tb.parent.is_some();
                            let marks: Vec<String> = (1..=names.len() + parent as usize).map(|n| format!("?{}", n)).collect();
                            let sql = format!("INSERT INTO {} ({}{}) VALUES ({})", tb.sql, if parent { "parent_id, " } else { "" }, names.join(", "), marks.join(", "));
                            let (cs, vs, r) = (self.tmp("argv"), self.tmp("argv"), self.tmp("i")); // argument arrays (the safety profile's name for them)
                            self.line(&format!("const u32 {}[{}] = {{ {} }};", cs, cols.len(), cols.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")));
                            self.line(&format!("Val {}[{}] = {{ {} }};", vs, vals.len(), vals.join(", ")));
                            self.line(&format!("u32 {};", r));
                            self.call(&format!("c->h->tinsert(c, {}, (u32)({}), {}, {}, {}, {}, &{})", t, inst, c_lit(sql.as_bytes()), cs, vs, cols.len(), r));
                            env[*slot as usize] = Some(CV::Item(c, r));
                            skip = k - i - 1;
                            continue;
                        }
                    }
                }
            }
            self.own_region = false;
            self.rest = &stmts[i + 1..];
            let (depth0, inl0) = (self.depth, self.inlining.len());
            if let Err(e) = self.stmt(s, env) {
                // a statement that fails is noted and skipped, so one run reports every mistake (a retry for a wider kind is not a mistake)
                if self.widen.is_some() || self.spec_key.is_some() { return Err(e); }
                self.errs.push(place_err(self.at, e));
                self.depth = depth0; self.inlining.truncate(inl0);
                // what follows a failed `let` or `var` most likely uses it: the rest of this block is skipped (its other mistakes show once this one is fixed)
                if matches!(s, Stmt::Let(..) | Stmt::Var(..)) { break; }
                continue;
            }
            if matches!(s, Stmt::Let(..) | Stmt::Var(..)) && self.own_region && live_from.is_none() { *live_from = Some(self.body.len()); }
            self.own_region = false;
            // `if !exists(x) { ... }` (or `if x == null`) whose branch always leaves (return, fail, break, continue), or `if exists(x)
            // { ... } else { ... }` whose else always leaves: x is a plain item for the rest of this block; the same for a field `m.f`
            if let Stmt::If(c, t, e) = s {
                let leaves = |b: &[Stmt]| matches!(b.last(), Some(Stmt::Fail(..) | Stmt::Return(..) | Stmt::Respond(..) | Stmt::Break | Stmt::Continue));
                let after = |yes: bool| (!yes && leaves(t) && e.is_empty()) || (yes && !e.is_empty() && leaves(e));
                if let Some((slot, yes)) = narrow_test(c) {
                    if after(yes) && !assigned_slots(&stmts[i + 1..]).contains(&slot) {
                        if let Some(CV::Ref(cl, _, k)) = env[slot as usize].clone() { restore.push((slot, env[slot as usize].replace(CV::Item(cl, k)).unwrap())); }
                    }
                }
                if let Some((slot, f, yes)) = field_narrow_test(c) {
                    if after(yes) && !assigned_slots(&stmts[i + 1..]).contains(&slot) {
                        if let Some(CV::Item(cl, k)) = env[slot as usize].clone() { self.narrowed.push((cl, k, f)); fields_kept += 1; }
                    }
                }
            }
        }
        for _ in 0..fields_kept { self.narrowed.pop(); }
        for (slot, v) in restore.into_iter().rev() { env[slot as usize] = Some(v); }
        Ok(())
    }

    /// A whole plain field or array of at most PRESAVE_MAX bytes stored into inside a loop: saved to the undo log before the loop (whatever
    /// the loop then writes there is restored from that copy), so its stores skip the per-store test. False outside loops. The save is the
    /// stores' own inline test, once per line: a loop entered very often (a small loop in a recursive function) pays no more than before.
    pub(super) fn presaved(&mut self, off: usize, size: usize) -> bool {
        const PRESAVE_MAX: usize = 512; // eight 64-byte lines: each is one inline test before the loop, about what one store's test costs
        let Some(list) = &mut self.presave else { return false };
        if size > PRESAVE_MAX { return false; }
        if !list.contains(&(off, size)) { list.push((off, size)); }
        true
    }
    /// Is this receiver one a function of a type was declared for (`fn clear(self: write Grid)`)? A name that is also a collection operation means the function
    /// when the receiver's type has one declared, and the collection operation otherwise.
    pub(super) fn user_method_for(&mut self, op: &str, recv: &'p Expr, env: &mut Vec<Option<CV>>) -> R<bool> {
        let v = self.expr(recv, env, None)?;
        Ok(match self.receiver_type(&v) {
            Some(ty) => {
                let want = format!("{}.{}", ty, op);
                let declared = self.api.fns.iter().any(|f| (f.recv && f.name == want) || (!f.recv && f.test.is_none() && f.name == op && f.ptys.first().map_or(false, |t| t.is_some())));
                // an instance of a generic struct whose function of this name has no copy yet: while the program is probed this asks for the copy, and the call is the function's
                declared || (crate::generics::is_template_method(op) && crate::generics::note_missing(&ty, op))
            }
            None => false,
        })
    }
    /// A statement the binder left as a call because its name is also declared for some type: where the receiver is a collection it becomes the collection statement.
    pub(super) fn deferred_op(&mut self, s: &'p Stmt, env: &mut Vec<Option<CV>>) -> R<Option<&'p Stmt>> {
        let (Stmt::Let(_, _, Expr::Call(op, args, CallRes::Unbound)) | Stmt::Do(Expr::Call(op, args, CallRes::Unbound))) = s else { return Ok(None) };
        if !op.starts_with('.') || !crate::lang::AMBIGUOUS_OPS.contains(&&op[1..]) || args.is_empty() { return Ok(None); }
        if self.user_method_for(&op[1..], &args[0], env)? { return Ok(None); }
        Ok(crate::lang::lower_op(s, 0).map(|l| &*Box::leak(Box::new(l))))
    }
}
