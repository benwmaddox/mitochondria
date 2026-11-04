//! Modules (docs/proposals/modules.md): a file in `lib/` with parameters, state and functions, instantiated when the
//! program is compiled, one copy per use. A use is a field of a struct in the program's state,
//!
//! ```text
//! struct Game { scores: bag<Score, 8>; }
//! state game: Game;
//! ```
//!
//! and becomes plain declarations: the module's structs renamed for the instance (`__mito__game__scores__Counts`:
//! generated names start with `__mito__`, which programs do not use), its state as the fields of a struct the field
//! now holds (`game.scores.items`, `game.scores.counts`), an `instance` line naming the instance by its path, and its
//! functions, named `game.scores.add` and so on, appended to the program's api. Parameters are replaced by their
//! arguments; nothing else is renamed. `self` stays `self`: the binder reads it as the instance whose function it is
//! in (its path), so no local of the module can shadow it. Every way of loading a program calls `expand`.
use std::collections::HashMap;

/// The standard library: module name, source.
pub const STDLIB: &[(&str, &str)] = &[("bag", include_str!("../lib/bag.mito")), ("sorted", include_str!("../lib/sorted.mito")), ("btree", include_str!("../lib/btree.mito")), ("ordmap", include_str!("../lib/ordmap.mito"))];

#[derive(Clone, Copy, PartialEq, Debug)]
enum PKind { Struct, Count, Field }

/// A module: its parameters, its structs, the fields an instance holds (its `state { ... }` block) and its functions.
struct Module { name: String, params: Vec<(String, PKind)>, structs: String, fields: String, funcs: String }

/// A module file, in the program syntax: `module name<parameters>`, its structs, `state { fields }` (what an instance
/// holds), and its functions.
fn parse_module(src: &str) -> Result<Module, String> {
    let at = src.match_indices("module ").map(|m| m.0).find(|&i| i == 0 || src.as_bytes()[i - 1] == b'\n').ok_or("a module file starts with `module name<parameters>`")?;
    let line_end = src[at..].find('\n').map_or(src.len(), |e| at + e);
    let header = Some(src[at + 7..line_end].trim().to_string());
    let body = &src[line_end..];
    // `state { ... }`: the instance's fields
    let st = body.find("\nstate {").ok_or("a module has a `state { ... }` block: what an instance holds")? + 1;
    let close = body[st..].find('}').map(|e| st + e).ok_or("module state block: expected `}`")?;
    let fields = body[st + 7..close].to_string();
    let (structs, funcs) = crate::source::split(&format!("{}{}", &body[..st], &body[close + 1..])).map_err(|e| format!("module: {}", e))?;
    let h = header.ok_or("a module file starts with `module name<parameters>`")?;
    let (name, rest) = h.split_once('<').ok_or("module header: expected `<`")?;
    let rest = rest.trim_end().strip_suffix('>').ok_or("module header: expected `>` at the end")?;
    let mut params = Vec::new();
    for p in rest.split(',') {
        let (n, k) = p.split_once(':').ok_or_else(|| format!("module parameter `{}`: expected `name: kind`", p.trim()))?;
        let k = match k.trim() { "struct" => PKind::Struct, "count" => PKind::Count, k if k.starts_with("field of ") => PKind::Field, k => return Err(format!("module parameter kind `{}`: expected struct, count or field of T", k)) };
        params.push((n.trim().to_string(), k));
    }
    Ok(Module { name: name.trim().to_string(), params, structs, fields, funcs })
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
            let _ = after_dot;
            match map.get(&w) { Some(r) => o += r, _ => o += &w }
            continue;
        }
        if !c[i].is_whitespace() { prev_fn = false; }
        o.push(c[i]); i += 1;
    }
    o
}

/// One instance of `module` named `inst`, the path of the struct field holding it (`game.board`). Returns its
/// declarations (the module's structs renamed for the instance, and the struct holding its state, named by `id`), its
/// `instance` line, and its functions, named `inst.add` and so on.
fn instantiate(inst: &str, module: &str, args: &[String]) -> Result<(String, String, String), String> {
    let Some(src) = STDLIB.iter().find(|m| m.0 == module).map(|m| m.1) else { return Err(format!("no module `{}`", module)) };
    let m = parse_module(src).map_err(|e| format!("module {}: {}", module, e))?;
    if args.len() != m.params.len() { return Err(format!("`{}`: {}<...> takes {} arguments, given {}", inst, m.name, m.params.len(), args.len())); }
    let id = inst.replace('.', "__");
    let mut map: HashMap<String, String> = HashMap::new();
    let mut key = None;
    for ((p, k), a) in m.params.iter().zip(args) {
        match k {
            PKind::Count => { a.parse::<u64>().map_err(|_| format!("`{}`: {} must be a number, not `{}`", inst, p, a))?; }
            PKind::Struct | PKind::Field => { if !a.chars().all(|c| c.is_alphanumeric() || c == '_') || a.is_empty() { return Err(format!("`{}`: {} must be a name, not `{}`", inst, p, a)); } }
        }
        if *k == PKind::Field { key = Some(a.clone()); }
        map.insert(p.clone(), a.clone());
    }
    // the module's own structs are renamed for this instance; its state block becomes the instance's struct
    let (st, fields) = (subst(&m.structs, &map, None), subst(&m.fields, &map, None));
    let mut names: HashMap<String, String> = HashMap::new();
    for w in st.split_whitespace().collect::<Vec<_>>().windows(2) { if w[0] == "struct" { let n = w[1].trim_end_matches('{'); names.insert(n.to_string(), format!("__mito__{}__{}", id, n)); } }
    let (mut decls, fields) = (subst(&st, &names, None), subst(&fields, &names, None));
    let fields: String = fields.lines().map(|l| l.split("//").next().unwrap_or("").trim()).filter(|l| !l.is_empty()).map(|l| format!(" {}", l)).collect();
    decls += &format!("struct __mito__{}__state {{{} }}\n", id, fields);
    let line = format!("instance {} of {}{};\n", inst, module, key.map(|k| format!(" key {}", k)).unwrap_or_default());
    let funcs = format!("// module {} as `{}`\n{}", module, inst, subst(&m.funcs, &map, Some(inst)));
    Ok((decls, line, funcs))
}

/// Expand every use of a standard-library module in a program's state, returning the state and api to compile. A use
/// is a field of a struct (`board: sorted<...>;` in `struct Game`), and a struct holding one must be reached exactly
/// once, through plain structs from the state, so the instance exists once.
pub fn expand(state: &str, api: &str) -> Result<(String, String), String> {
    if !state.contains('<') { return Ok((state.to_string(), api.to_string())); }
    let mut out_api = api.to_string();
    if !out_api.ends_with('\n') { out_api.push('\n'); }
    let (_, states) = decls(state);
    if let Some((n, t)) = states.iter().find(|g| STDLIB.iter().any(|m| g.1.starts_with(&format!("{}<", m.0)))) {
        return Err(format!("`state {}: {}`: the state is a struct; make the module instance one of its fields", n, t));
    }
    let state = expand_fields(state, &mut out_api)?;
    Ok((state, out_api))
}

/// A struct declaration found in the program's state text: its name, and each field's name, type text and where the
/// type text is.
struct SDecl { name: String, fields: Vec<(String, String, std::ops::Range<usize>)> }

/// The struct declarations and the `state` declaration of a state text (comments skipped).
fn decls(state: &str) -> (Vec<SDecl>, Vec<(String, String)>) {
    let b = state.as_bytes();
    let (mut structs, mut globals) = (Vec::new(), Vec::new());
    let mut i = 0;
    let word = |i: usize, w: &str| state[i..].starts_with(w) && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) && state[i + w.len()..].starts_with(|c: char| c.is_whitespace());
    while i < b.len() {
        if state[i..].starts_with("//") { i = state[i..].find('\n').map_or(b.len(), |e| i + e); continue; }
        if word(i, "struct") {
            let Some(open) = state[i..].find('{').map(|e| i + e) else { break };
            let Some(close) = state[open..].find('}').map(|e| open + e) else { break };
            let name = state[i + 6..open].trim().to_string();
            let mut fields = Vec::new();
            let mut at = open + 1;
            for part in state[open + 1..close].split(';') {
                let start = at; at += part.len() + 1;
                // a field may follow a comment line
                let mut text = part; let mut off = start;
                while let Some(k) = text.find("//") { let e = text[k..].find('\n').map_or(text.len(), |e| k + e); off += e; text = &text[e..]; }
                if let Some((n, t)) = text.split_once(':') {
                    let tstart = off + n.len() + 1;
                    let lead = t.len() - t.trim_start().len();
                    fields.push((n.trim().to_string(), t.trim().to_string(), tstart + lead..tstart + lead + t.trim().len()));
                }
            }
            structs.push(SDecl { name, fields });
            i = close + 1; continue;
        }
        if word(i, "state") {
            let Some(semi) = state[i..].find(';').map(|e| i + e) else { break };
            if let Some((n, t)) = state[i + 5..semi].split_once(':') { globals.push((n.trim().to_string(), t.trim().to_string())); }
            i = semi + 1; continue;
        }
        i += 1;
    }
    (structs, globals)
}

/// Expand module instances that are fields of structs: the field's type becomes the instance's state struct, named by
/// the one path that reaches it (`game.board`).
fn expand_fields(state: &str, api: &mut String) -> Result<String, String> {
    let (structs, globals) = decls(state);
    let is_struct = |t: &str| structs.iter().any(|s| s.name == t);
    // every path from the state to a struct through plain struct fields, and whether the struct is used any other way
    fn paths(target: &str, structs: &[SDecl], globals: &[(String, String)], depth: usize) -> Vec<String> {
        if depth > 64 { return Vec::new(); }
        let mut v: Vec<String> = globals.iter().filter(|g| g.1 == target).map(|g| g.0.clone()).collect();
        for s in structs { for f in &s.fields { if f.1 == target { for p in paths(&s.name, structs, globals, depth + 1) { v.push(format!("{}.{}", p, f.0)); } } } }
        v
    }
    let mentions = |t: &str, name: &str| t != name && t.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w == name);
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut added = String::new();
    for s in &structs {
        for (fname, ty, range) in &s.fields {
            let Some((m, args)) = ty.split_once('<') else { continue };
            if !STDLIB.iter().any(|x| x.0 == m.trim()) { continue; }
            let Some(args) = args.trim().strip_suffix('>') else { continue };
            let args: Vec<String> = args.split(',').map(|a| a.trim().to_string()).collect();
            let elsewhere = globals.iter().map(|g| &g.1).chain(structs.iter().flat_map(|x| x.fields.iter().map(|f| &f.1))).any(|t| mentions(t, &s.name));
            let ps = paths(&s.name, &structs, &globals, 0);
            if elsewhere || ps.len() != 1 {
                let how = if elsewhere { "is also used inside a collection or array".to_string() } else if ps.is_empty() { "is not held by the state".to_string() } else { format!("is reached {} ways ({})", ps.len(), ps.join(", ")) };
                return Err(format!("struct {} holds a module instance (`{}`), so it must exist once, reached through plain structs from the state; it {}", s.name, fname, how.replace("global", "the state")));
            }
            let inst = format!("{}.{}", ps[0], fname);
            let (d, line, funcs) = instantiate(&inst, m.trim(), &args)?;
            edits.push((range.clone(), format!("__mito__{}__state", inst.replace('.', "__"))));
            added += &d; added += &line;
            *api += &funcs;
        }
    }
    let _ = is_struct;
    if edits.is_empty() { return Ok(state.to_string()); }
    let mut out = state.to_string();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
    for (r, t) in edits { out.replace_range(r, &t); }
    if !out.ends_with('\n') { out.push('\n'); }
    Ok(out + &added)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_use_expands_into_declarations_and_functions() {
        let (st, api) = expand("struct S { n: i64; }\nstruct App { b: bag<S, 4>; }\nstate app: App;\n", "").unwrap();
        assert!(st.contains("struct __mito__app__b__Counts {") && st.contains("struct __mito__app__b__state {") && st.contains("items: pool<S, 4> checked;") && st.contains("counts: __mito__app__b__Counts;"), "{}", st);
        assert!(st.contains("b: __mito__app__b__state;") && st.contains("instance app.b of bag;"), "{}", st);
        assert!(api.contains("pub fn app.b.add()") && api.contains("self.items.add()") && api.contains("return 4 - self.items.count();"), "{}", api);
        assert!(expand("struct S { n: i64; }\nstruct App { b: bag<S>; }\nstate app: App;\n", "").unwrap_err().contains("takes 2 arguments"));
        // an instance exists once: its struct is reached through plain structs from the state, one way only
        let twice = expand("struct S { n: i64; }\nstruct H { b: bag<S, 4>; }\nstruct App { x: H; y: H; }\nstate app: App;\n", "").unwrap_err();
        assert!(twice.contains("reached 2 ways (app.x, app.y)"), "{}", twice);
        let inside = expand("struct S { n: i64; }\nstruct H { b: bag<S, 4>; }\nstruct App { hs: pool<H, 2>; }\nstate app: App;\n", "").unwrap_err();
        assert!(inside.contains("inside a collection"), "{}", inside);
        assert!(expand("struct S { n: i64; }\nstate b: bag<S, 4>;\n", "").unwrap_err().contains("make the module instance one of its fields"));
        // nested a level deeper, the path names it
        let (st, api) = expand("struct S { n: i64; }\nstruct Inner { b: bag<S, 4>; }\nstruct App { inner: Inner; }\nstate app: App;\n", "").unwrap();
        assert!(st.contains("instance app.inner.b of bag;") && api.contains("pub fn app.inner.b.add()"), "{}\n{}", st, api);
    }

    /// A program that changes a module instance's state other than through its functions does not load.
    #[test]
    fn module_state_changes_only_through_its_functions() {
        let state = "struct S { n: i64; name: string<8>; }\nstruct App { b: bag<S, 4>; }\nstate app: App;\n";
        let load = |api: &str| -> Result<(), String> {
            let (st, api) = expand(state, api)?;
            crate::cgen::check(&crate::layout::Program::parse(&st)?, &api).map(|_| ())
        };
        assert_eq!(load("@post(\"/ok\") fn post_ok() -> Response<{}> { let s = app.b.add(); s.n = 5; s.name = \"x\"; app.b.remove(s); return ok({}); }"), Ok(()));
        for (api, want) in [
            ("@post(\"/a\") fn post_a() -> Response<{}> { app.b.counts.added = 9; return ok({}); }", "belongs to app.b (module bag)"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { let s = app.b.items.add(); return ok({}); }", "add items of app.b with its functions, not `app.b.items.add()`"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { app.b.items.remove(app.b.get(0)); return ok({}); }", "remove items of app.b with its functions, not `app.b.items.remove()`"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { let x = app.b.get(0); app.b.items.remove(x); return ok({}); }", "remove items of app.b with its functions"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { each s in app.b { app.b.remove(s); } return ok({}); }", "a loop over app.b calls a function that changes app.b"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ \"x\": [each s in app.b app.b.add()] }); }", "a loop over app.b calls a function that changes app.b"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ \"x\": [each s in app.b from 1 order by s.n s.n] }); }", "`order by` was removed"),
        ] {
            let e = load(api).err().unwrap_or_default();
            assert!(e.contains(want), "{} -> {}", api, e);
        }
        // a module's private function cannot be called from outside it
        let (st, mut api) = expand(state, "@get(\"/a\") fn get_a() -> Response<{}> { return ok({ \"n\": app.b.secret() }); }").unwrap();
        api += "fn app.b.secret() { return 1; }\n";
        let e = crate::cgen::check(&crate::layout::Program::parse(&st).unwrap(), &api).err().unwrap_or_default();
        assert!(e.contains("`app.b.secret` is private to app.b"), "{}", e);
    }
}
