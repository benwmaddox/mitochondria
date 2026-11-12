//! Scalar replacement for struct values (docs/design/struct-values.md, "Locals: scratch places first, scalar replacement second").
//!
//! A struct local lives in the request's scratch area and every access is a load or store at a constant offset (`LD_F64(4096, 0)`, `ST8_(a, v)`), with `SCRZERO` and
//! `SCRMOVE` for whole-struct initialization and copies. This pass reads the generated C of one function at a time and, for each struct place that is only ever touched in
//! those forms, keeps every scalar field in a C local variable (a "cell": `u64 sr4096`) instead. The C compiler then keeps the cells in registers and the memmoves vanish.
//!
//! The code generator declares each place with a comment line, `/*SCR offset size width@field_offset ...*/` (or `!` for a struct with text or an array: never replaced).
//! A place is replaced only when every number in the function that falls inside it is one of the recognized forms below; anything else (the offset passed to a function, an
//! index, a variable-size move, text) leaves the place in memory, so the pass can only ever miss an optimization, never change a result. Because the cells mirror the memory
//! byte for byte (a zero or a move copies every cell it covers), no life-time analysis is needed.
//! `MITO_NO_SCALAR=1` turns the pass off (the differential check of the struct cases runs both ways).

struct Place { off: usize, n: usize, cells: Vec<(usize, usize)>, ok: bool }

/// Replace what can be replaced in the generated code `src` (the part after the prelude).
pub fn replace(src: &str) -> String {
    if std::env::var_os("MITO_NO_SCALAR").is_some() || !src.contains("/*SCR ") { return src.to_string(); }
    let lines: Vec<&str> = src.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let is_start = l.ends_with(") {") && l.chars().next().map_or(false, |c| c.is_ascii_alphabetic());
        if !is_start { out.push(l.to_string()); i += 1; continue; }
        let Some(end) = (i + 1..lines.len()).find(|&j| lines[j] == "}") else { out.push(l.to_string()); i += 1; continue; };
        let body: Vec<&str> = lines[i..=end].to_vec();
        if body.iter().any(|b| b.trim_start().starts_with("/*SCR ")) { out.extend(function(&body)); } else { out.extend(body.iter().map(|s| s.to_string())); }
        i = end + 1;
    }
    out.join("\n")
}

fn function(lines: &[&str]) -> Vec<String> {
    if lines.iter().any(|l| l.trim() == "/*SCRALL*/") { return lines.iter().map(|s| s.to_string()).collect(); }
    let mut places = parse_places(lines);
    let aliases = find_aliases(lines, &mut places);
    let orig = |lines: &[&str]| lines.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    if places.iter().all(|p| !p.ok) { return orig(lines); }
    loop {
        let mut bad = Vec::new();
        let mut out: Vec<String> = Vec::with_capacity(lines.len() + 8);
        out.push(lines[0].to_string());
        let decl_at = out.len();
        for l in &lines[1..] {
            match line(l, &places, &aliases, &mut bad) { Some(s) => out.push(s), None => {} }
        }
        if bad.is_empty() {
            let mut decls = Vec::new();
            for p in places.iter().filter(|p| p.ok) {
                for &(a, w) in &p.cells { decls.push(format!("    u{} sr{} = 0; (void)sr{};", w * 8, a, a)); }
            }
            if decls.is_empty() { return orig(lines); }
            out.splice(decl_at..decl_at, decls);
            return out;
        }
        for b in bad { places[b].ok = false; }
        if places.iter().all(|p| !p.ok) { return orig(lines); }
    }
}

fn parse_places(lines: &[&str]) -> Vec<Place> {
    let mut places: Vec<Place> = Vec::new();
    for l in lines {
        let Some(r) = l.trim().strip_prefix("/*SCR ").and_then(|r| r.strip_suffix("*/")) else { continue };
        let mut it = r.split_whitespace();
        let (Some(off), Some(n)) = (it.next().and_then(|x| x.parse::<usize>().ok()), it.next().and_then(|x| x.parse::<usize>().ok())) else { continue };
        let mut cells = Vec::new();
        let mut ok = true;
        for c in it {
            match c.split_once('@').and_then(|(w, o)| Some((w.parse::<usize>().ok()?, o.parse::<usize>().ok()?))) {
                Some((w, o)) if matches!(w, 1 | 2 | 4 | 8) => cells.push((off + o, w)),
                _ => ok = false,
            }
        }
        cells.sort();
        // the cells must tile the place exactly (no padding, nothing left over)
        let mut at = off;
        for &(a, w) in &cells { if a != at { ok = false; } at = a + w; }
        if at != off + n || cells.is_empty() { ok = false; }
        if places.iter().any(|p| p.off == off && p.n == n && p.cells == cells && p.ok == ok) { continue; }
        places.push(Place { off, n, cells, ok });
    }
    // a place handed to a function compiled apart (which has its offset built in) stays in memory
    for l in lines {
        let Some(r) = l.trim().strip_prefix("/*SCRX ").and_then(|r| r.strip_suffix("*/")) else { continue };
        let mut it = r.split_whitespace().map(|x| x.parse::<usize>().ok());
        let (Some(Some(off)), Some(Some(n))) = (it.next(), it.next()) else { for p in places.iter_mut() { p.ok = false; } continue };
        for p in places.iter_mut() { if off < p.off + p.n && p.off < off + n.max(1) { p.ok = false; } }
    }
    // places that overlap without being the same place cannot share cells
    for i in 0..places.len() {
        for j in 0..places.len() {
            if i != j && places[i].off < places[j].off + places[j].n && places[j].off < places[i].off + places[i].n { places[i].ok = false; }
        }
    }
    places
}

fn find(places: &[Place], addr: usize) -> Option<usize> { places.iter().position(|p| addr >= p.off && addr < p.off + p.n) }

/// `u64 a56 = (u64)(48ULL);` at the start of a store: name -> address (only names defined once; the places of a name defined twice are given up).
fn find_aliases(lines: &[&str], places: &mut Vec<Place>) -> std::collections::HashMap<String, usize> {
    let mut seen: std::collections::HashMap<String, Vec<usize>> = Default::default();
    for l in lines {
        if let Some((n, a)) = alias_def(l.trim()) { seen.entry(n).or_default().push(a); }
    }
    let mut m = std::collections::HashMap::new();
    for (n, v) in seen {
        if v.len() == 1 { m.insert(n, v[0]); } else { for a in v { if let Some(p) = find(places, a) { places[p].ok = false; } } }
    }
    m
}

fn alias_def(t: &str) -> Option<(String, usize)> {
    let r = t.strip_prefix("u64 a")?;
    let d = r.find(|c: char| !c.is_ascii_digit())?;
    if d == 0 { return None; }
    let name = format!("a{}", &r[..d]);
    let r = r[d..].strip_prefix(" = (u64)(")?.strip_suffix("ULL);")?;
    Some((name, r.parse().ok()?))
}

fn ctype(t: &str) -> Option<(&'static str, usize)> {
    Some(match t { "I64" => ("i64", 8), "U64" => ("u64", 8), "I32" => ("int", 4), "U32" => ("u32", 4), "I16" => ("short", 2), "U16" => ("unsigned short", 2), "I8" => ("signed char", 1), "U8" => ("u8", 1), "F32" => ("float", 4), "F64" => ("double", 8), _ => return None })
}

/// Numbers outside strings, comments and identifiers: (start, end, value).
fn numbers(s: &str) -> (Vec<(usize, usize, Option<u64>)>, Vec<(usize, usize)>) {
    let b = s.as_bytes();
    let (mut nums, mut ids) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' || c == b'\'' { i += 1; while i < b.len() && b[i] != c { if b[i] == b'\\' { i += 1; } i += 1; } i += 1; continue; }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' { break; }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' { match s[i + 2..].find("*/") { Some(e) => { i += e + 4; continue; } None => break } }
        if c.is_ascii_alphabetic() || c == b'_' { let s0 = i; while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; } ids.push((s0, i)); continue; }
        if c.is_ascii_digit() {
            let s0 = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || ((b[i] == b'+' || b[i] == b'-') && i > s0 && matches!(b[i - 1], b'e' | b'E') && !s[s0..i].starts_with("0x"))) { i += 1; }
            let t = &s[s0..i];
            let digits = t.trim_end_matches(|c: char| matches!(c, 'u' | 'U' | 'l' | 'L'));
            let v = if let Some(h) = digits.strip_prefix("0x") { u64::from_str_radix(h, 16).ok() } else { digits.parse::<u64>().ok() };
            nums.push((s0, i, v));
            continue;
        }
        if c == b'.' { i += 1; while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') { i += 1; } continue; }
        i += 1;
    }
    (nums, ids)
}

fn lit(s: &str) -> Option<usize> { s.trim().strip_suffix("ULL").unwrap_or(s.trim()).parse().ok() }

/// Split `a, b(c, d), e` at the top-level commas.
fn args(s: &str) -> Vec<&str> {
    let (mut v, mut depth, mut from, b) = (Vec::new(), 0i32, 0, s.as_bytes());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => { let q = b[i]; i += 1; while i < b.len() && b[i] != q { if b[i] == b'\\' { i += 1; } i += 1; } }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => { v.push(&s[from..i]); from = i + 1; }
            _ => {}
        }
        i += 1;
    }
    v.push(&s[from..]);
    v
}

/// The cells of place `p` inside `[a, a + n)`, when the range starts and ends on cell boundaries.
fn cells_in(p: &Place, a: usize, n: usize) -> Option<Vec<(usize, usize)>> {
    if a < p.off || a + n > p.off + p.n { return None; }
    let v: Vec<(usize, usize)> = p.cells.iter().copied().filter(|&(c, w)| c >= a && c + w <= a + n).collect();
    if v.iter().map(|c| c.1).sum::<usize>() == n { Some(v) } else { None }
}

/// One line of the function: the line rewritten (None: dropped). `bad` collects the places that turned out not to be replaceable.
fn line(l: &str, places: &[Place], aliases: &std::collections::HashMap<String, usize>, bad: &mut Vec<usize>) -> Option<String> {
    let t = l.trim_start();
    let indent = &l[..l.len() - t.len()];
    if t.starts_with('#') || t.starts_with("//") || t.starts_with("/*") { return Some(l.to_string()); }
    let mut cur = l.to_string();
    let mut dropped = false;
    // an address variable's definition
    if let Some((_, a)) = alias_def(t) {
        if let Some(p) = find(places, a) { if places[p].ok { return None; } }
    }
    let ok_at = |a: usize| find(places, a).filter(|&p| places[p].ok);
    // a store through an address variable
    let st = (|| {
        let w = match t.get(..5)? { "ST1_(" => 1, "ST2_(" => 2, "ST4_(" => 4, "ST8_(" => 8, _ => return None };
        let inner = t[5..].strip_suffix(");")?;
        let (name, rest) = inner.split_once(',')?;
        let a = *aliases.get(name.trim())?;
        Some((w, a, rest.trim().to_string()))
    })();
    if let Some((w, a, rest)) = st {
        if let Some(p) = ok_at(a) {
            if places[p].cells.contains(&(a, w)) { cur = format!("{}sr{} = (u{})({});", indent, a, w * 8, rest); } else { bad.push(p); }
        }
    } else if let Some(op) = ["SCRZERO(", "SCRMOVE(", "MOVE_("].iter().find(|o| t.starts_with(**o)) {
        if let Some(inner) = t[op.len()..].strip_suffix(");") {
            let a = args(inner);
            let zero = *op == "SCRZERO(";
            if (zero && a.len() == 2) || (!zero && a.len() == 3) {
                let n: Option<usize> = a[a.len() - 1].trim().parse().ok();
                let (to, from) = (lit(a[0]), if zero { None } else { lit(a[1]) });
                if let Some(n) = n {
                    let (tp, fp) = (to.and_then(&ok_at), from.and_then(&ok_at));
                    if tp.is_some() || fp.is_some() {
                        let tc = tp.and_then(|p| cells_in(&places[p], to.unwrap(), n));
                        let fc = fp.and_then(|p| cells_in(&places[p], from.unwrap(), n));
                        let mut stmts: Vec<String> = Vec::new();
                        let mut fail = false;
                        if tp.is_some() && tc.is_none() { bad.push(tp.unwrap()); fail = true; }
                        if fp.is_some() && fc.is_none() { bad.push(fp.unwrap()); fail = true; }
                        if !fail {
                            if let (Some(tc), Some(fc)) = (&tc, &fc) {
                                let (ta, fa) = (to.unwrap(), from.unwrap());
                                if ta == fa { dropped = true; }
                                else if tc.len() != fc.len() || tc.iter().zip(fc).any(|(x, y)| x.0 - ta != y.0 - fa || x.1 != y.1) || tp == fp { bad.push(tp.unwrap()); bad.push(fp.unwrap()); fail = true; }
                                else { for (x, y) in tc.iter().zip(fc) { stmts.push(format!("sr{} = sr{};", x.0, y.0)); } }
                            } else if let Some(tc) = &tc {
                                let ta = to.unwrap();
                                for &(c, w) in tc {
                                    if zero { stmts.push(format!("sr{} = 0;", c)); } else { stmts.push(format!("sr{} = LD_U{}(({}) + {}, 0);", c, w * 8, a[1].trim(), c - ta)); }
                                }
                            } else if let Some(fc) = &fc {
                                let fa = from.unwrap();
                                for &(c, w) in fc { stmts.push(format!("ST{}_(({}) + {}, sr{});", w, a[0].trim(), c - fa, c)); }
                            }
                        }
                        if !fail {
                            if dropped { return None; }
                            cur = if stmts.len() == 1 { format!("{}{}", indent, stmts[0]) } else { format!("{}{{ {} }}", indent, stmts.join(" ")) };
                        }
                    }
                }
            }
        }
    }
    // loads at constant offsets
    let mut o = String::with_capacity(cur.len());
    let mut rest = cur.as_str();
    while let Some(k) = rest.find("LD_") {
        let (head, tail) = rest.split_at(k);
        o.push_str(head);
        let prev_ok = !head.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
        let parsed = (|| {
            if !prev_ok { return None; }
            let pc = tail.find('(')?;
            let (c, w) = ctype(&tail[3..pc])?;
            let close = tail.find(')')?;
            let ar = args(&tail[pc + 1..close]);
            if ar.len() != 2 { return None; }
            let a = lit(ar[0])?;
            let z = ar[1].trim().trim_end_matches(|c: char| matches!(c, 'u' | 'U' | 'l' | 'L'));
            if z != "0" { return None; }
            Some((c, w, a, close))
        })();
        match parsed {
            Some((c, w, a, close)) => match ok_at(a) {
                Some(p) if places[p].cells.contains(&(a, w)) => { o.push_str(&format!("CELL_({}, sr{})", c, a)); rest = &tail[close + 1..]; }
                Some(p) => { bad.push(p); o.push_str("LD_"); rest = &tail[3..]; }
                None => { o.push_str("LD_"); rest = &tail[3..]; }
            },
            None => { o.push_str("LD_"); rest = &tail[3..]; }
        }
    }
    o.push_str(rest);
    // whatever number is left in a place that is being replaced stops the replacement
    let (nums, ids) = numbers(&o);
    for (_, _, v) in nums { if let Some(p) = v.and_then(|v| ok_at(v as usize)) { bad.push(p); } }
    for (s, e) in ids { if let Some(&a) = aliases.get(&o[s..e]) { if let Some(p) = ok_at(a) { bad.push(p); } } }
    if bad.is_empty() { Some(o) } else { Some(l.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::replace;
    fn f(body: &str) -> String { format!("static int ep0(Ctx *c) {{
{}
}}", body.lines().map(|l| format!("    {}", l)).collect::<Vec<_>>().join("
")) }
    #[test]
    fn a_place_touched_only_by_constant_fields_becomes_locals() {
        let src = f("/*SCR 4096 16 8@0 8@8*/\nSCRZERO(4096ULL, 16);\n{\nu64 a1 = (u64)(4096ULL);\nST8_(a1, BITS_F64(1.5));\n}\nMOVE_(0ULL, 4096ULL, 16);\nCALL(x(c, LD_F64(4096, 0)));");
        let out = replace(&src);
        assert!(out.contains("u64 sr4096 = 0;") && out.contains("sr4096 = (u64)(BITS_F64(1.5));") && out.contains("CELL_(double, sr4096)"), "{}", out);
        assert!(out.contains("ST8_((0ULL) + 0, sr4096); ST8_((0ULL) + 8, sr4104);"), "{}", out);
        assert!(!out.contains("SCRZERO") && !out.contains("a1"), "{}", out);
    }
    #[test]
    fn a_place_with_another_use_stays_in_memory() {
        // the offset is passed on, a run-time index, a place handed to a function compiled apart, text, a returned place
        for bad in ["CALL(g(c, 4096ULL));", "CALL(x(c, LD_F64(4096, i)));", "/*SCRX 4096 16*/", "CALL(x(c, LD_F64(4096 + k, 0)));"] {
            let src = f(&format!("/*SCR 4096 16 8@0 8@8*/\nSCRZERO(4096ULL, 16);\n{}", bad));
            assert_eq!(replace(&src), src, "{}", bad);
        }
        let src = f("/*SCR 4096 16 !*/\nSCRZERO(4096ULL, 16);");
        assert_eq!(replace(&src), src);
        let src = f("/*SCRALL*/\n/*SCR 4096 16 8@0 8@8*/\nSCRZERO(4096ULL, 16);");
        assert_eq!(replace(&src), src);
    }
}
