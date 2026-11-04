//! Modules (docs/proposals/modules.md): a file in `lib/` with parameters, state and functions, instantiated when the
//! program is compiled, one copy per use. A use in the program's state,
//!
//! ```text
//! global scores: bag<Score, 8>;
//! ```
//!
//! becomes plain declarations: the module's structs renamed for the instance (`scores__Counts`), its state as the fields
//! of a struct the instance's global holds (`scores.items`, `scores.counts`), an `instance` line naming the instance, and
//! its functions, named `scores.add` and so on, appended to the program's api. In the module, `self` is the instance
//! and parameters are replaced by their arguments; nothing else is renamed. Every way of loading a program calls `expand`.
use std::collections::HashMap;

/// The standard library: module name, source.
pub const STDLIB: &[(&str, &str)] = &[("bag", include_str!("../lib/bag.mito"))];

#[derive(Clone, Copy, PartialEq, Debug)]
enum PKind { Struct, Count, Field }

struct Module { name: String, params: Vec<(String, PKind)>, state: String, funcs: String }

fn parse_module(src: &str) -> Result<Module, String> {
    let (mut header, mut state, mut funcs, mut at) = (None, String::new(), String::new(), 0);
    for line in src.lines() {
        let t = line.trim();
        if header.is_none() && t.starts_with("module ") { header = Some(t[7..].to_string()); continue; }
        match t { "--- state" => { at = 1; continue; } "--- functions" => { at = 2; continue; } _ => {} }
        if at == 1 { state += line; state.push('\n'); } else if at == 2 { funcs += line; funcs.push('\n'); }
    }
    let h = header.ok_or("a module file starts with `module name<parameters>`")?;
    let (name, rest) = h.split_once('<').ok_or("module header: expected `<`")?;
    let rest = rest.trim_end().strip_suffix('>').ok_or("module header: expected `>` at the end")?;
    let mut params = Vec::new();
    for p in rest.split(',') {
        let (n, k) = p.split_once(':').ok_or_else(|| format!("module parameter `{}`: expected `name: kind`", p.trim()))?;
        let k = match k.trim() { "struct" => PKind::Struct, "count" => PKind::Count, k if k.starts_with("field of ") => PKind::Field, k => return Err(format!("module parameter kind `{}`: expected struct, count or field of T", k)) };
        params.push((n.trim().to_string(), k));
    }
    Ok(Module { name: name.trim().to_string(), params, state, funcs })
}

/// Replace identifiers by `map` (outside strings and comments). After `fn`, a function's name gets `fn_prefix`.
fn subst(text: &str, map: &HashMap<String, String>, fn_prefix: Option<&str>) -> String {
    let c: Vec<char> = text.chars().collect();
    let (mut o, mut i, mut prev_fn) = (String::new(), 0, false);
    while i < c.len() {
        if c[i] == '/' && c.get(i + 1) == Some(&'/') { while i < c.len() && c[i] != '\n' { o.push(c[i]); i += 1; } continue; }
        if c[i] == '"' {
            o.push('"'); i += 1;
            while i < c.len() && c[i] != '"' { if c[i] == '\\' && i + 1 < c.len() { o.push(c[i]); i += 1; } o.push(c[i]); i += 1; }
            if i < c.len() { o.push('"'); i += 1; }
            continue;
        }
        if c[i].is_alphabetic() || c[i] == '_' {
            let s = i; while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') { i += 1; }
            let w: String = c[s..i].iter().collect();
            let after_dot = o.ends_with('.') && !o.ends_with("..");
            if prev_fn { if let Some(p) = fn_prefix { o += p; o.push('.'); } o += &w; prev_fn = false; continue; }
            prev_fn = w == "fn";
            // a field name after `.` is replaced only if it is a parameter (`x.key`), never `self`
            match map.get(&w) { Some(r) if !(after_dot && w == "self") => o += r, _ => o += &w }
            continue;
        }
        if !c[i].is_whitespace() { prev_fn = false; }
        o.push(c[i]); i += 1;
    }
    o
}

/// Expand every use of a standard-library module in a program's state, returning the state and api to compile.
pub fn expand(state: &str, api: &str) -> Result<(String, String), String> {
    if !state.contains('<') { return Ok((state.to_string(), api.to_string())); }
    let (mut out_state, mut out_api) = (String::new(), api.to_string());
    if !out_api.ends_with('\n') { out_api.push('\n'); }
    // find `global NAME: MODULE<args>;` at the start of a statement
    let mut rest = state;
    loop {
        let Some((pos, inst, module, args, len)) = find_use(rest) else { out_state += rest; break };
        out_state += &rest[..pos];
        let Some(src) = STDLIB.iter().find(|m| m.0 == module).map(|m| m.1) else { unreachable!() };
        let m = parse_module(src).map_err(|e| format!("module {}: {}", module, e))?;
        if args.len() != m.params.len() { return Err(format!("`{}`: {}<...> takes {} arguments, given {}", inst, m.name, m.params.len(), args.len())); }
        let mut map: HashMap<String, String> = HashMap::new();
        let mut key = None;
        for ((p, k), a) in m.params.iter().zip(&args) {
            match k {
                PKind::Count => { a.parse::<u64>().map_err(|_| format!("`{}`: {} must be a number, not `{}`", inst, p, a))?; }
                PKind::Struct | PKind::Field => { if !a.chars().all(|c| c.is_alphanumeric() || c == '_') || a.is_empty() { return Err(format!("`{}`: {} must be a name, not `{}`", inst, p, a)); } }
            }
            if *k == PKind::Field { key = Some(a.clone()); }
            map.insert(p.clone(), a.clone());
        }
        map.insert("self".into(), inst.clone());
        // the module's own structs are renamed for this instance
        let st = subst(&m.state, &map, None);
        let mut names: HashMap<String, String> = HashMap::new();
        for w in st.split_whitespace().collect::<Vec<_>>().windows(2) { if w[0] == "struct" { let n = w[1].trim_end_matches('{'); names.insert(n.to_string(), format!("{}__{}", inst, n)); } }
        let st = subst(&st, &names, None);
        // its globals become fields of the instance's state struct
        let (mut decls, mut fields) = (String::new(), String::new());
        let mut s = st.as_str();
        while let Some(i) = s.find(|c: char| !c.is_whitespace()) {
            s = &s[i..];
            if s.starts_with("//") { s = s.find('\n').map_or("", |e| &s[e..]); continue; }
            if let Some(r) = s.strip_prefix("struct") { let e = r.find('}').ok_or_else(|| format!("module {}: unclosed struct", module))?; decls += "struct"; decls += &r[..=e]; decls.push('\n'); s = &r[e + 1..]; continue; }
            if let Some(r) = s.strip_prefix("global") { let e = r.find(';').ok_or_else(|| format!("module {}: expected `;`", module))?; fields += &r[..e]; fields += ";"; s = &r[e + 1..]; continue; }
            return Err(format!("module {}: its state holds struct and global declarations, found `{}`", module, s.lines().next().unwrap_or("")));
        }
        out_state += &format!("{}struct {}__state {{{} }}\nglobal {}: {}__state;\ninstance {} of {}{};", decls, inst, fields, inst, inst, inst, module, key.map(|k| format!(" key {}", k)).unwrap_or_default());
        out_api += &format!("// module {} as `{}`\n", module, inst);
        out_api += &subst(&m.funcs, &map, Some(&inst));
        rest = &rest[pos + len..];
    }
    Ok((out_state, out_api))
}

/// The first `global NAME: MODULE<args>;` naming a standard-library module: (offset, name, module, args, length).
fn find_use(s: &str) -> Option<(usize, String, String, Vec<String>, usize)> {
    let mut from = 0;
    while let Some(g) = s[from..].find("global") {
        let at = from + g;
        from = at + 6;
        if at > 0 && s[..at].chars().last().map_or(false, |c| c.is_alphanumeric() || c == '_') { continue; }
        let Some(semi) = s[at..].find(';') else { return None };
        let stmt = &s[at + 6..at + semi];
        let Some((name, ty)) = stmt.split_once(':') else { continue };
        let ty = ty.trim();
        let Some((m, args)) = ty.split_once('<') else { continue };
        let m = m.trim();
        if !STDLIB.iter().any(|x| x.0 == m) { continue; }
        let Some(args) = args.trim().strip_suffix('>') else { continue };
        let args = args.split(',').map(|a| a.trim().to_string()).collect();
        return Some((at, name.trim().to_string(), m.to_string(), args, semi + 1));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_use_expands_into_declarations_and_functions() {
        let (st, api) = expand("struct S { n: i64; }\nglobal b: bag<S, 4>;\n", "").unwrap();
        assert!(st.contains("struct b__Counts {") && st.contains("struct b__state {") && st.contains("items: pool<S, 4> checked;") && st.contains("counts: b__Counts;"), "{}", st);
        assert!(st.contains("global b: b__state;") && st.contains("instance b of bag;"), "{}", st);
        assert!(api.contains("pub fn b.add()") && api.contains("alloc b.items") && api.contains("return 4 - count(b.items);"), "{}", api);
        assert!(expand("struct S { n: i64; }\nglobal b: bag<S>;\n", "").unwrap_err().contains("takes 2 arguments"));
    }

    /// A program that changes a module instance's state other than through its functions does not load (interpreted
    /// or compiled: the compiler decides what loads).
    #[test]
    fn module_state_changes_only_through_its_functions() {
        let state = "struct S { n: i64; name: string<8>; }\nglobal b: bag<S, 4>;\n";
        let load = |api: &str| -> Result<(), String> {
            let (st, api) = expand(state, api)?;
            let inst = crate::runtime::Instance::start(crate::layout::Program::parse(&st)?, 1 << 20)?;
            crate::serve::Server::new(inst, &api).map(|_| ())
        };
        assert_eq!(load("endpoint POST /ok { let s = b.add(); s.n = 5; s.name = \"x\"; b.remove(s); respond 200 {}; }"), Ok(()));
        for (api, want) in [
            ("endpoint POST /a { b.counts.added = 9; respond 200 {}; }", "belongs to b (module bag)"),
            ("endpoint POST /a { let s = alloc b.items; respond 200 {}; }", "add items of b with its functions, not `alloc`"),
            ("endpoint POST /a { free b.get(0); respond 200 {}; }", "remove items of b with its functions, not `free`"),
            ("endpoint POST /a { var x = 1; x = b.get(0); free x; respond 200 {}; }", "`free` of a value whose kind is only known at run time"),
            ("endpoint POST /a { each s in b { b.remove(s); } respond 200 {}; }", "a loop over b calls a function that changes b"),
            ("endpoint GET /a { respond 200 { \"x\": [each s in b b.add()] }; }", "a loop over b calls a function that changes b"),
            ("endpoint GET /a { respond 200 { \"x\": [each s in b from 1 order by s.n s.n] }; }", "b keeps its own order"),
        ] {
            let e = load(api).err().unwrap_or_default();
            assert!(e.contains(want), "{} -> {}", api, e);
        }
        // a module's private function cannot be called from outside it
        let (st, mut api) = expand(state, "endpoint GET /a { respond 200 { \"n\": b.secret() }; }").unwrap();
        api += "fn b.secret() { return 1; }\n";
        let inst = crate::runtime::Instance::start(crate::layout::Program::parse(&st).unwrap(), 1 << 20).unwrap();
        let e = crate::serve::Server::new(inst, &api).err().unwrap_or_default();
        assert!(e.contains("`b.secret` is private to b"), "{}", e);
    }
}
