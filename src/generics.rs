//! Generics (docs/design/generics.md): generic structs and the functions of a generic type, expanded into ordinary
//! declarations before the program is parsed, bound or laid out, so layout and code generation never see a generic.
//!
//! ```text
//! struct Stack<T, N: count> { items: T[N]; len: i64 in 0..N; }
//! fn push(self: Stack<T, N>, x: T) { self.items[self.len] = x; self.len += 1; }
//! struct Game { undo: Stack<i64, 64>; names: Stack<string<12>, 8>; }
//! ```
//!
//! Every distinct use (`Stack<i64, 64>`: the template and its evaluated arguments) becomes one struct named
//! `__mito__Stack__i64__64` (generated names start with `__mito__`, which programs do not use) and one copy of each
//! function of `Stack<T, N>` for it, a plain function of a type (`fn push(self: __mito__Stack__i64__64, x: i64)`), found
//! by the receiver's type like any other. Copies are made by replacing the parameters' identifiers token by token (outside
//! strings and comments, and not after a `.`), then expanded again, to a fixed point: a copy may use further generics.
//! Limits: instances nest 64 deep and there are at most 1000, so a recursive generic type is an error, not a hang.
//! `name_instances` turns the generated names back into `Stack<i64, 64>` in a message.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

const MAX_DEPTH: usize = 64;
const MAX_INSTANCES: usize = 1000;
const MAX_NAME: usize = 100;
const INT_TYPES: [&str; 8] = ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"];

#[derive(Clone, PartialEq, Debug)]
enum Kind { Any, Struct, Number, Int, Text, Count, Field(String) }

struct Template { name: String, params: Vec<(String, Kind)>, body: String }
/// A function of a generic type: the names its receiver gives the parameters (`Stack<U, M>`), and its text.
struct Method { template: usize, name: String, names: Vec<String>, text: String, line: usize }
struct Inst { name: String, display: String, depth: usize, args: Vec<String>, body: String }

thread_local! { static NAMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new()); }

/// A message with the generated names of generic instances written as the program wrote them: `Stack<i64, 16>.push`.
pub fn name_instances(msg: &str) -> String {
    if !msg.contains("__mito__") { return msg.to_string(); }
    let mut names: Vec<(String, String)> = NAMES.with(|n| n.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    names.sort_by_key(|n| std::cmp::Reverse(n.0.len()));
    let mut out = msg.to_string();
    for (k, v) in names { out = out.replace(&k, &v); }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum K { Id, Num, Str, Comment, Space, Other }
#[derive(Clone, Copy)]
struct Tk { k: K, s: usize, e: usize }

fn lex(c: &[char]) -> Vec<Tk> {
    let (mut v, mut i) = (Vec::new(), 0);
    while i < c.len() {
        let s = i;
        let k = if c[i] == '/' && c.get(i + 1) == Some(&'/') { while i < c.len() && c[i] != '\n' { i += 1; } K::Comment }
            else if c[i] == '"' { i += 1; while i < c.len() && c[i] != '"' { if c[i] == '\\' { i += 1; } i += 1; } i = (i + 1).min(c.len()); K::Str }
            else if c[i].is_alphabetic() || c[i] == '_' { while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') { i += 1; } K::Id }
            else if c[i].is_ascii_digit() { while i < c.len() && c[i].is_ascii_digit() { i += 1; } K::Num }
            else if c[i].is_whitespace() { while i < c.len() && c[i].is_whitespace() { i += 1; } K::Space }
            else { i += 1; K::Other };
        v.push(Tk { k, s, e: i });
    }
    v
}

struct Src { c: Vec<char>, t: Vec<Tk> }
impl Src {
    fn new(text: &str) -> Src { let c: Vec<char> = text.chars().collect(); let t = lex(&c); Src { c, t } }
    fn txt(&self, i: usize) -> String { self.c[self.t[i].s..self.t[i].e].iter().collect() }
    fn is(&self, i: usize, ch: char) -> bool { self.t.get(i).map_or(false, |t| t.k == K::Other && self.c[t.s] == ch) }
    fn is_id(&self, i: usize, w: &str) -> bool { self.t.get(i).map_or(false, |t| t.k == K::Id && self.txt(i) == w) }
    /// The next token from `i` that is not space or a comment.
    fn sig(&self, mut i: usize) -> usize { while i < self.t.len() && matches!(self.t[i].k, K::Space | K::Comment) { i += 1; } i }
    /// The last token before `i` that is not space or a comment.
    fn prev(&self, i: usize) -> Option<usize> { (0..i).rev().find(|&j| !matches!(self.t[j].k, K::Space | K::Comment)) }
    fn after_dot(&self, i: usize) -> bool { self.prev(i).map_or(false, |p| self.is(p, '.') && !self.prev(p).map_or(false, |q| self.is(q, '.'))) }
    /// The list at the `<` or `(` at token `j`: its top-level comma-separated pieces (trimmed) and the token after the closing bracket.
    fn list(&self, j: usize) -> Result<(Vec<String>, usize), String> {
        let (mut depth, mut cur, mut out, mut i) = (0i32, String::new(), Vec::new(), j);
        while i < self.t.len() {
            let ch = if self.t[i].k == K::Other { self.c[self.t[i].s] } else { ' ' };
            match ch { '<' | '(' | '[' => depth += 1, '>' | ')' | ']' => depth -= 1, _ => {} }
            if depth == 0 { out.push(cur.trim().to_string()); return Ok((out, i + 1)); }
            if depth == 1 && ch == ',' { out.push(cur.trim().to_string()); cur.clear(); i += 1; continue; }
            if i > j { cur += &self.txt(i); }
            i += 1;
        }
        Err("generic arguments are never closed: expected `>`".into())
    }
    /// The token after the `}` matching the first `{` at or after `i`.
    fn block_end(&self, mut i: usize) -> Option<usize> {
        let mut depth = 0;
        while i < self.t.len() { if self.is(i, '{') { depth += 1; } if self.is(i, '}') { depth -= 1; if depth == 0 { return Some(i + 1); } } i += 1; }
        None
    }
}

/// `text` with the character ranges removed, their newlines kept (so every line number stays).
fn blank(text: &str, ranges: &[(usize, usize)]) -> String {
    let c: Vec<char> = text.chars().collect();
    let (mut out, mut at) = (String::new(), 0);
    for &(s, e) in ranges {
        out.extend(&c[at..s]);
        out.extend(c[s..e].iter().filter(|&&ch| ch == '\n'));
        at = e;
    }
    out.extend(&c[at..]);
    out
}

/// `text` with the identifiers in `map` replaced (outside strings and comments, and not a field after a `.`, except the
/// parameters that name a field, `key: field of T`: `self.items[i].key` reads the field the argument names).
fn subst(text: &str, map: &HashMap<String, String>, fields: &HashSet<String>) -> String {
    let s = Src::new(text);
    let mut out = String::new();
    for i in 0..s.t.len() {
        let w = s.txt(i);
        match map.get(&w) { Some(r) if s.t[i].k == K::Id && (!s.after_dot(i) || fields.contains(&w)) => out += r, _ => out += &w }
    }
    out
}

fn sanitize(a: &str) -> String {
    let s: String = a.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    let mut o = String::new();
    for c in s.chars() { if !(c == '_' && o.ends_with('_')) { o.push(c); } }
    o.trim_matches('_').to_string()
}

struct Ctx { templates: Vec<Template>, methods: Vec<Method>, plain: HashMap<String, String>, insts: Vec<Inst>, by_key: HashMap<String, usize>, taken: HashSet<String>, out_state: String, out_api: String, asked: HashSet<(String, String)>, pending: Vec<(usize, usize)> }

impl Ctx {
    fn template(&self, name: &str) -> Option<usize> { self.templates.iter().position(|t| t.name == name) }

    /// `text` with every use of a generic struct (`Stack<i64, 16>`) replaced by the name of its instance, made if new.
    fn rewrite(&mut self, text: &str) -> Result<String, String> {
        let s = Src::new(text);
        let (mut out, mut i) = (String::new(), 0);
        while i < s.t.len() {
            if s.t[i].k == K::Id && !s.after_dot(i) {
                if let Some(ti) = self.template(&s.txt(i)) {
                    let j = s.sig(i + 1);
                    if !s.is(j, '<') {
                        let t = &self.templates[ti];
                        return Err(format!("`{}` is a generic struct: write its arguments, `{}<{}>`", t.name, t.name, t.params.iter().map(|p| p.0.clone()).collect::<Vec<_>>().join(", ")));
                    }
                    let (args, end) = s.list(j)?;
                    let args = args.iter().map(|a| self.rewrite(a)).collect::<Result<Vec<_>, _>>()?;
                    out += &self.instantiate(ti, args)?;
                    i = end;
                    continue;
                }
            }
            out += &s.txt(i);
            i += 1;
        }
        Ok(out)
    }

    /// A copy of each function of a generic struct that a call asked for on an instance (`asked`: the instance as written,
    /// `Stack<i64, 16>`, and the function's name). Copies name further instances, whose functions are copied only when
    /// a call asks for them in turn (see `expand`).
    fn copy_asked_functions(&mut self) -> Result<(), String> {
        loop {
            let Some(at) = self.pending.iter().position(|&(k, mi)| self.asked.contains(&(self.insts[k].display.clone(), self.methods[mi].name.clone()))) else { return Ok(()) };
            let (k, mi) = self.pending.remove(at);
            let map: HashMap<String, String> = self.methods[mi].names.iter().cloned().zip(self.insts[k].args.iter().cloned()).collect();
            let fields = self.field_params(self.methods[mi].template, &self.methods[mi].names);
            let f = subst(&self.methods[mi].text, &map, &fields);
            let f = self.rewrite(&f)?;
            // the copy's lines are the template's lines in the program's own source (see `//# line` in src/lang.rs)
            self.out_api += &format!("//# line {} {}\n{}\n", self.methods[mi].line, self.insts[k].display, f);
        }
    }

    /// Of the names a function's receiver gives a template's parameters (`self: Sorted<T, N, k>`), those of parameters that name a field.
    fn field_params(&self, ti: usize, names: &[String]) -> HashSet<String> {
        names.iter().zip(&self.templates[ti].params).filter(|(_, p)| matches!(p.1, Kind::Field(_))).map(|(n, _)| n.clone()).collect()
    }

    /// The text between the braces of a struct given by name: a plain one, or an instance of a generic one.
    fn struct_body(&self, name: &str) -> Option<String> {
        self.plain.get(name).cloned().or_else(|| self.by_key.get(&format!("name:{}", name)).map(|&k| self.insts[k].body.clone())).filter(|b| !b.is_empty())
    }

    /// The display form of an argument (its instances named as written).
    fn show(&self, arg: &str) -> String {
        let s = Src::new(arg);
        (0..s.t.len()).map(|i| { let w = s.txt(i); self.by_key.get(&format!("name:{}", w)).map_or(w, |&k| self.insts[k].display.clone()) }).collect()
    }

    /// Is `a` an acceptable argument for parameter `p` of `t`? The error says what was written and what the parameter is.
    fn check_arg(&self, display: &str, t: &Template, p: &(String, Kind), args: &[String], a: &str) -> Result<(), String> {
        let params = t.params.iter().map(|q| q.0.clone()).collect::<Vec<_>>().join(", ");
        let is_num = a.chars().next().map_or(false, |c| c.is_ascii_digit());
        let is_name = !a.is_empty() && a.chars().all(|c| c.is_alphanumeric() || c == '_');
        let is_struct = self.plain.contains_key(a) || self.by_key.contains_key(&format!("name:{}", a));
        let (what, ok, got) = match &p.1 {
            Kind::Field(of) => return self.check_field_arg(display, t, p, of, args, a),
            Kind::Count => {
                if is_num && a.parse::<u64>().map_or(false, |n| n >= 1) { return Ok(()); }
                let got = if is_name && !is_num { format!("{} is a name; counts are whole-number literals for now", a) } else { format!("{} is not a whole number of at least 1", a) };
                ("count", false, got)
            }
            _ if is_num || a.is_empty() => {
                return Err(format!("`{}`: {} is a type in {}<{}>, but {} is a number", display, p.0, t.name, params, a));
            }
            Kind::Any => return Ok(()),
            Kind::Struct => ("struct", is_struct, format!("{} is not a struct", self.show(a))),
            Kind::Number => ("number", INT_TYPES.contains(&a) || a == "f32" || a == "f64", String::new()),
            Kind::Int => ("int", INT_TYPES.contains(&a), String::new()),
            Kind::Text => ("text", ["string<", "ascii<", "utf8<"].iter().any(|x| a.starts_with(x)), String::new()),
        };
        if ok { return Ok(()); }
        let got = if !got.is_empty() { got } else if is_struct { format!("{} is a struct", self.show(a)) } else { format!("{} is not that kind of type", self.show(a)) };
        if what == "count" { return Err(format!("`{}`: {} is a `count` in {}<{}>, but {}", display, p.0, t.name, params, got)); }
        Err(format!("`{}`: {} is `{}` in {}<{}>, but {}", display, p.0, what, t.name, params, got))
    }

    /// `key: field of T` given `a`: a field of the struct `T` stands for, and an integer.
    fn check_field_arg(&self, display: &str, t: &Template, p: &(String, Kind), of: &str, args: &[String], a: &str) -> Result<(), String> {
        let params = t.params.iter().map(|q| q.0.clone()).collect::<Vec<_>>().join(", ");
        let head = format!("`{}`: {} is a field of {} in {}<{}>", display, p.0, of, t.name, params);
        if a.is_empty() || !a.chars().all(|c| c.is_alphanumeric() || c == '_') || a.starts_with(|c: char| c.is_ascii_digit()) { return Err(format!("{}, but {} is not a field name", head, a)); }
        let sarg = match t.params.iter().position(|q| q.0 == of) { Some(i) => args[i].clone(), None => of.to_string() };
        // not a struct: the parameter that stands for it says so
        let Some(body) = self.struct_body(&sarg) else { return Ok(()) };
        let sname = self.show(&sarg);
        match field_type(&body, a) {
            None => Err(format!("{}, but {} has no field `{}`", head, sname, a)),
            Some(ty) if !is_int_type(&ty) => Err(format!("{}, but {}.{} is {}, not an integer", head, sname, a, ty)),
            Some(_) => Ok(()),
        }
    }

    /// The name of the instance of template `ti` with these arguments (already expanded), making it, its struct and its functions if new.
    fn instantiate(&mut self, ti: usize, args: Vec<String>) -> Result<String, String> {
        let key = format!("{}<{}>", self.templates[ti].name, args.iter().map(|a| a.split_whitespace().collect::<String>()).collect::<Vec<_>>().join(","));
        if let Some(&k) = self.by_key.get(&key) { return Ok(self.insts[k].name.clone()); }
        let tname = self.templates[ti].name.clone();
        let shown: Vec<String> = args.iter().map(|a| self.show(a)).collect();
        let display = format!("{}<{}>", tname, shown.join(", "));
        let np = self.templates[ti].params.len();
        if args.len() != np { return Err(format!("`{}`: {} takes {} argument{}, given {}", display, tname, np, if np == 1 { "" } else { "s" }, args.len())); }
        // the parameters that name a field are checked last: they read the struct another parameter stands for
        for fields in [false, true] {
            for (p, a) in self.templates[ti].params.iter().zip(&args) { if matches!(p.1, Kind::Field(_)) == fields { self.check_arg(&display, &self.templates[ti], p, &args, a)?; } }
        }
        // the nesting depth: one more than the deepest instance among the arguments
        let mut depth = 1;
        for a in &args { let s = Src::new(a); for i in 0..s.t.len() { if let Some(&k) = self.by_key.get(&format!("name:{}", s.txt(i))) { depth = depth.max(self.insts[k].depth + 1); } } }
        if depth > MAX_DEPTH { return Err(format!("generic instance `{}` nests {} levels deep (more than {}): a recursive generic type?", if display.len() > 80 { format!("{}...", &display[..80]) } else { display }, depth, MAX_DEPTH)); }
        if self.insts.len() >= MAX_INSTANCES { return Err(format!("more than {} generic instances (the last: `{}`)", MAX_INSTANCES, display)); }
        let mut name = format!("__mito__{}__{}", tname, args.iter().map(|a| sanitize(a)).collect::<Vec<_>>().join("__"));
        if name.len() > MAX_NAME || self.taken.contains(&name) { name = format!("__mito__{}__{}", tname, self.insts.len()); }
        self.taken.insert(name.clone());
        NAMES.with(|n| { n.borrow_mut().insert(name.clone(), display.clone()); });
        let k = self.insts.len();
        self.insts.push(Inst { name: name.clone(), display, depth, args: args.clone(), body: String::new() });
        self.by_key.insert(key, k);
        self.by_key.insert(format!("name:{}", name), k);
        // the struct, then a copy of each function of the template
        let map = |names: &[String]| -> HashMap<String, String> { names.iter().cloned().zip(args.iter().cloned()).collect() };
        let pnames: Vec<String> = self.templates[ti].params.iter().map(|p| p.0.clone()).collect();
        let fields: HashSet<String> = self.templates[ti].params.iter().filter(|p| matches!(p.1, Kind::Field(_))).map(|p| p.0.clone()).collect();
        let body = subst(&self.templates[ti].body, &map(&pnames), &fields);
        let body = self.rewrite(&body)?;
        self.insts[k].body = body.clone();
        self.out_state += &format!("struct {} {{{}}}\n", name, body);
        for (mi, m) in self.methods.iter().enumerate() { if m.template == ti { self.pending.push((k, mi)); } }
        Ok(name)
    }
}

/// The `struct Name<params> { body }` declarations of a state text: the templates, and the character ranges to blank.
fn collect_structs(state: &str) -> Result<(Vec<Template>, HashMap<String, String>, Vec<(usize, usize)>), String> {
    let s = Src::new(state);
    let (mut templates, mut plain, mut cut) = (Vec::new(), HashMap::new(), Vec::new());
    let mut i = 0;
    while i < s.t.len() {
        if s.is_id(i, "struct") && !s.after_dot(i) {
            let n = s.sig(i + 1);
            if s.t.get(n).map_or(false, |t| t.k == K::Id) {
                let name = s.txt(n);
                let j = s.sig(n + 1);
                if s.is(j, '<') {
                    let (ps, after) = s.list(j)?;
                    let mut params: Vec<(String, Kind)> = Vec::new();
                    for p in ps {
                        let (pn, kind) = match p.split_once(':') { Some((a, b)) => (a.trim().to_string(), Some(b.trim().to_string())), None => (p.trim().to_string(), None) };
                        if pn.is_empty() || !pn.chars().all(|c| c.is_alphanumeric() || c == '_') || pn.chars().next().map_or(false, |c| c.is_ascii_digit()) { return Err(format!("struct {}: generic parameter `{}`: expected a name, `T`, or `N: count`", name, p)); }
                        if params.iter().any(|q| q.0 == pn) { return Err(format!("struct {}: generic parameter `{}` is declared twice", name, pn)); }
                        let k = match kind.as_deref() {
                            None => Kind::Any, Some("struct") => Kind::Struct, Some("number") => Kind::Number, Some("int") => Kind::Int, Some("text") => Kind::Text, Some("count") => Kind::Count,
                            Some(k) if k.starts_with("field of ") => {
                                let of = k[9..].trim();
                                if of.is_empty() || !of.chars().all(|c| c.is_alphanumeric() || c == '_') { return Err(format!("struct {}: `{}: {}`: expected `field of T`, a field of the struct T stands for", name, pn, k)); }
                                Kind::Field(of.to_string())
                            }
                            Some(k) => return Err(format!("struct {}: generic parameter kind `{}`: expected struct, number, int, text, count or nothing (any type)", name, k)),
                        };
                        params.push((pn, k));
                    }
                    if params.is_empty() { return Err(format!("struct {}<>: a generic struct has at least one parameter", name)); }
                    let open = s.sig(after);
                    let end = if s.is(open, '{') { s.block_end(open) } else { None }.ok_or_else(|| format!("struct {}<...>: expected `{{` and its fields", name))?;
                    if templates.iter().any(|t: &Template| t.name == name) { return Err(format!("struct {} is declared twice", name)); }
                    let body: String = s.c[s.t[open].e..s.t[end - 1].s].iter().collect();
                    templates.push(Template { name, params, body });
                    cut.push((s.t[i].s, s.t[end - 1].e));
                    i = end;
                    continue;
                }
                let open = s.sig(n + 1);
                let body: String = if s.is(open, '{') { s.block_end(open).map(|end| s.c[s.t[open].e..s.t[end - 1].s].iter().collect()).unwrap_or_default() } else { String::new() };
                plain.insert(name, body);
            }
        }
        i += 1;
    }
    for t in &templates { if plain.contains_key(&t.name) { return Err(format!("struct {} is declared twice", t.name)); } }
    for t in &templates { for p in &t.params { if plain.contains_key(&p.0) || templates.iter().any(|u| u.name == p.0) { return Err(format!("struct {}<{}>: the parameter `{}` has the name of a struct; a parameter shadows nothing, so rename it", t.name, p.0, p.0)); } } }
    // a parameter that names a field says which struct it is a field of: a `struct` parameter of its own, or a struct
    for t in &templates {
        for p in &t.params {
            let Kind::Field(of) = &p.1 else { continue };
            match t.params.iter().find(|q| q.0 == *of) {
                Some(q) if q.1 != Kind::Struct => return Err(format!("struct {}: `{}: field of {}`: {} must be a struct, `{}: struct`", t.name, p.0, of, of, of)),
                Some(_) => {}
                None if plain.contains_key(of) => {}
                None => return Err(format!("struct {}: `{}: field of {}`: {} is neither a parameter of {}<...> nor a struct", t.name, p.0, of, of, t.name)),
            }
        }
    }
    Ok((templates, plain, cut))
}

/// The type text of the field `name` in a struct body (`items: T[N]; len: i64 in 0..N;`), comments skipped.
fn field_type(body: &str, name: &str) -> Option<String> {
    let s = Src::new(body);
    let text: String = (0..s.t.len()).filter(|&i| s.t[i].k != K::Comment).map(|i| s.txt(i)).collect();
    let (mut depth, mut cur, mut parts) = (0i32, String::new(), Vec::new());
    for ch in text.chars() {
        match ch { '<' | '(' | '[' | '{' => depth += 1, '>' | ')' | ']' | '}' => depth -= 1, _ => {} }
        if ch == ';' && depth == 0 { parts.push(std::mem::take(&mut cur)); } else { cur.push(ch); }
    }
    parts.push(cur);
    parts.iter().find_map(|p| p.split_once(':').filter(|(n, _)| n.trim() == name).map(|(_, t)| t.split_whitespace().collect::<Vec<_>>().join(" ")))
}

/// Is a field's type text an integer type, with or without a range (`i64`, `u8 in 0..9`)?
fn is_int_type(ty: &str) -> bool {
    let word: String = ty.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    INT_TYPES.contains(&word.as_str()) && { let rest = ty[word.len()..].trim_start(); rest.is_empty() || rest.starts_with("in ") }
}

/// The functions of generic types in a code text (`fn push(self: Stack<T, N>, x: T) {...}`), and the ranges to blank.
fn collect_methods(api: &str, templates: &[Template]) -> Result<(Vec<Method>, Vec<(usize, usize)>), String> {
    let s = Src::new(api);
    let (mut methods, mut cut) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < s.t.len() {
        if !(s.is_id(i, "fn") && !s.after_dot(i)) { i += 1; continue; }
        let start = match s.prev(i) { Some(p) if s.is_id(p, "pub") => s.t[p].s, _ => s.t[i].s };
        let n = s.sig(i + 1);
        let name = s.txt(n.min(s.t.len() - 1));
        let mut j = s.sig(n + 1);
        if s.is(j, '<') { return Err(format!("`fn {}<...>`: generic functions of no type are not supported yet; a function of a generic type, `fn {}(self: Stack<T, N>, ...)`, binds its parameters from its receiver", name, name)); }
        if !s.is(j, '(') { i += 1; continue; }
        let p0 = s.sig(j + 1);
        let (colon, rt) = (s.sig(p0 + 1), s.sig(s.sig(p0 + 1) + 1));
        let Some(ti) = (if s.is_id(p0, "self") && s.is(colon, ':') && s.t.get(rt).map_or(false, |t| t.k == K::Id) { templates.iter().position(|t| t.name == s.txt(rt)) } else { None }) else { i = j; continue };
        let lt = s.sig(rt + 1);
        let t = &templates[ti];
        if !s.is(lt, '<') { return Err(format!("`fn {}(self: {})`: {} is a generic struct, so the receiver names its parameters: `self: {}<{}>`", name, t.name, t.name, t.name, t.params.iter().map(|p| p.0.clone()).collect::<Vec<_>>().join(", "))); }
        let (names, _) = s.list(lt)?;
        if names.len() != t.params.len() { return Err(format!("`fn {}(self: {}<...>)`: {} takes {} parameter{}, the receiver names {}", name, t.name, t.name, t.params.len(), if t.params.len() == 1 { "" } else { "s" }, names.len())); }
        for (k, nm) in names.iter().enumerate() {
            if nm.is_empty() || !nm.chars().all(|c| c.is_alphanumeric() || c == '_') || nm.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                return Err(format!("`fn {}(self: {}<...>)`: `{}` is not a parameter name; the receiver of a function of a generic struct names its parameters, `{}<{}>`", name, t.name, nm, t.name, t.params.iter().map(|p| p.0.clone()).collect::<Vec<_>>().join(", ")));
            }
            if names[..k].contains(nm) { return Err(format!("`fn {}(self: {}<...>)`: parameter `{}` is named twice", name, t.name, nm)); }
        }
        // the body: past the parameters and the return type, the first `{` outside angle brackets
        let (mut depth, mut angle) = (0i32, 0i32);
        let mut b = j;
        while b < s.t.len() {
            if s.is(b, '(') { depth += 1; }
            if s.is(b, ')') { depth -= 1; if depth == 0 { b += 1; break; } }
            b += 1;
        }
        while b < s.t.len() {
            if s.is(b, '<') { angle += 1; }
            if s.is(b, '>') && !s.prev(b).map_or(false, |p| s.is(p, '-')) { angle -= 1; }
            if s.is(b, '{') && angle == 0 { break; }
            b += 1;
        }
        let end = s.block_end(b).ok_or_else(|| format!("`fn {}`: expected a body", name))?;
        j = end;
        let text: String = s.c[start..s.t[end - 1].e].iter().collect();
        let line = 1 + s.c[..start].iter().filter(|&&c| c == '\n').count();
        methods.push(Method { template: ti, name, names, text, line });
        cut.push((start, s.t[end - 1].e));
        i = j;
    }
    Ok((methods, cut))
}

// The calls found to name a function that is not there yet: (instance as written, function name).
thread_local! { static MISSING: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new()); }

thread_local! { static TEMPLATE_METHODS: RefCell<HashSet<String>> = RefCell::new(HashSet::new()); }

/// Is `name` the name of a function of a generic struct in the program being compiled? Its copies are made where a call
/// names them, so the binder cannot find the function itself and takes this as "declared".
pub fn is_template_method(name: &str) -> bool { TEMPLATE_METHODS.with(|m| m.borrow().contains(name)) }

/// The code generator found `x.f(...)` on a receiver of type `ty` and no function `f` for it. If `ty` is an instance of a
/// generic struct, that is a request for the copy of the template's `f` for this instance (`expand` answers it).
pub fn note_missing(ty: &str, method: &str) {
    if let Some(shown) = NAMES.with(|n| n.borrow().get(ty).cloned()) { MISSING.with(|m| m.borrow_mut().push((shown, method.to_string()))); }
}

/// One expansion of the program with the copies `asked` for; the text, and whether any function of a generic type exists
/// (then calls may ask for more).
fn expand_once(state: &str, api: &str, asked: &HashSet<(String, String)>) -> Result<(String, String, bool), String> {
    TEMPLATE_METHODS.with(|m| m.borrow_mut().clear());
    if !state.contains('<') { return Ok((state.to_string(), api.to_string(), false)); }
    let (templates, plain, cut) = collect_structs(state)?;
    if templates.is_empty() { return Ok((state.to_string(), api.to_string(), false)); }
    let (methods, api_cut) = collect_methods(api, &templates)?;
    let calls = !methods.is_empty();
    TEMPLATE_METHODS.with(|m| m.borrow_mut().extend(methods.iter().map(|x| x.name.clone())));
    let (state, api) = (blank(state, &cut), blank(api, &api_cut));
    let mut cx = Ctx { templates, methods, plain, insts: Vec::new(), by_key: HashMap::new(), taken: HashSet::new(), out_state: String::new(), out_api: String::new(), asked: asked.clone(), pending: Vec::new() };
    let (mut state, mut api) = (cx.rewrite(&state)?, cx.rewrite(&api)?);
    cx.copy_asked_functions()?;
    if !state.ends_with('\n') { state.push('\n'); }
    if !api.ends_with('\n') { api.push('\n'); }
    state += &cx.out_state;
    api += &cx.out_api;
    Ok((state, api, calls))
}

/// Expand every generic struct and function of a generic type in a program's state and code, returning plain text.
/// `finish` turns the expanded text into what the compiler reads (the module expansion).
///
/// A function of a generic type is copied only for the instances a call names, and the receiver's type of a call is known
/// only to the code generator. So the program is expanded with the copies asked for so far (none at first), compiled once
/// to see which calls name a function no instance has (`note_missing`), and expanded again with those added, until a
/// compile asks for nothing new. The last text is returned; its errors are the caller's to report, as for any program.
pub fn expand(state: &str, api: &str, finish: &dyn Fn(&str, &str) -> Result<(String, String), String>) -> Result<(String, String), String> {
    let mut asked: HashSet<(String, String)> = HashSet::new();
    loop {
        let (s, a, calls) = expand_once(state, api, &asked)?;
        let (s, a) = finish(&s, &a)?;
        if !calls { return Ok((s, a)); }
        MISSING.with(|m| m.borrow_mut().clear());
        if let Ok(prog) = crate::layout::Program::parse(&s) { let _ = crate::cgen::check(&prog, &a); }
        let found: Vec<(String, String)> = MISSING.with(|m| m.borrow_mut().drain(..).collect());
        let before = asked.len();
        asked.extend(found);
        if asked.len() == before { return Ok((s, a)); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plain(state: &str, api: &str) -> Result<(String, String), String> { expand(state, api, &|s, a| Ok((s.to_string(), a.to_string()))) }
    const STACK: &str = "struct Stack<T, N: count> { items: T[N]; len: i64 in 0..N; }\nstruct Game { undo: Stack<i64, 16>; names: Stack<string<12>, 8>; again: Stack<i64, 16>; }\nstate game: Game;\n";
    const PUSH: &str = "fn push(self: Stack<T, N>, x: T) { self.items[self.len] = x; self.len = self.len + 1; }\nfn full(self: Stack<T, N>) -> bool { return self.len == N; }\n";

    #[test]
    fn a_use_becomes_a_struct_and_functions_of_a_type() {
        let asked: HashSet<(String, String)> = [("Stack<i64, 16>", "push"), ("Stack<string<12>, 8>", "push"), ("Stack<string<12>, 8>", "full")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let (st, api, _) = expand_once(STACK, &format!("{}fn go() {{ game.undo.push(1); game.names.push(\"a\"); game.undo.full(); game.names.full(); }}
", PUSH), &asked).unwrap();
        assert!(st.contains("struct __mito__Stack__i64__16 { items: i64[16]; len: i64 in 0..16; }"), "{}", st);
        assert!(st.contains("struct __mito__Stack__string_12__8 { items: string<12>[8]; len: i64 in 0..8; }"), "{}", st);
        assert!(st.contains("undo: __mito__Stack__i64__16;") && st.contains("again: __mito__Stack__i64__16;") && !st.contains("struct Stack<"), "{}", st);
        // one struct per distinct use; a copy of a function only where a call asked for it
        assert!(!api.contains("fn full(self: __mito__Stack__i64__16"), "{}", api);
        assert_eq!(st.matches("struct __mito__Stack__i64__16 ").count(), 1);
        assert!(api.contains("fn push(self: __mito__Stack__i64__16, x: i64) {") && api.contains("fn push(self: __mito__Stack__string_12__8, x: string<12>) {"), "{}", api);
        assert!(api.contains("return self.len == 8;") && api.contains("//# line 2 Stack<string<12>, 8>"), "{}", api);
        // line numbers are kept: the templates are blanked, not removed
        assert_eq!(st.lines().take(3).count(), 3);
        assert!(!st.lines().next().unwrap().contains("Stack<T"));
    }

    #[test]
    fn instances_nest_and_the_expansion_parses_and_lays_out() {
        let state = "struct Pair<A, B> { a: A; b: B; }\nstruct Stack<T, N: count> { items: T[N]; len: i64 in 0..N; }\nstruct G { s: Stack<Pair<i64, string<8>>, 4>; p: pool<Pair<i64, i64>, 2>; }\nstate g: G;\n";
        let asked: HashSet<(String, String)> = [("Stack<Pair<i64, string<8>>, 4>".to_string(), "first".to_string())].into_iter().collect();
        let (st, api, _) = expand_once(state, "fn never(self: Stack<T, N>) -> T { return self.items[0] + 1; }
fn go() { g.s.first(); }
fn first(self: Stack<T, N>) -> T { return self.items[0]; }\nfn sum(p: Pair<i64, i64>) -> i64 { return p.a + p.b; }\n", &asked).unwrap();
        assert!(st.contains("s: __mito__Stack__"), "{}", st);
        assert!(api.contains("fn sum(p: __mito__Pair__i64__i64)"), "{}", api);
        assert!(crate::layout::Program::parse(&st).is_ok(), "{}", st);
        // `never` is named nowhere, so it is not copied (a string element could not be added to)
        assert!(!api.contains("never"), "{}", api);
        assert_eq!(crate::lang::parse(&api).map(|a| a.fns.len()), Ok(3));
        assert_eq!(name_instances(&st.lines().find(|l| l.contains("struct __mito__Stack__")).unwrap().to_string()).contains("Stack<Pair<i64, string<8>>, 4>"), true);
    }

    #[test]
    fn a_program_without_generics_is_unchanged() {
        let st = "struct S { a: i64[4]; }\nstate s: S;\n";
        assert_eq!(plain(st, "fn f() {}").unwrap(), (st.to_string(), "fn f() {}".to_string()));
    }

    #[test]
    fn wrong_arguments_are_errors_that_name_the_instance() {
        let go = |uses: &str| plain(&format!("struct Stack<T, N: count> {{ items: T[N]; len: i64; }}\nstruct E {{ a: i64; }}\nstruct Num<T: number> {{ v: T; }}\nstruct Int<T: int> {{ v: T; }}\nstruct Txt<T: text> {{ v: T; }}\nstruct Rec<T: struct> {{ v: T[2]; }}\nstruct G {{ {} }}\nstate g: G;\n", uses), "").unwrap_err();
        assert_eq!(go("a: Stack<i64, 0>;"), "`Stack<i64, 0>`: N is a `count` in Stack<T, N>, but 0 is not a whole number of at least 1");
        assert!(go("a: Stack<i64>;").contains("`Stack<i64>`: Stack takes 2 arguments, given 1"), "{}", go("a: Stack<i64>;"));
        assert!(go("a: Stack<i64, 4, 5>;").contains("takes 2 arguments, given 3"));
        assert!(go("a: Stack<i64, n>;").contains("n is a name; counts are whole-number literals"), "{}", go("a: Stack<i64, n>;"));
        assert!(go("a: Stack<16, 4>;").contains("T is a type in Stack<T, N>, but 16 is a number"));
        assert_eq!(go("a: Num<E>;"), "`Num<E>`: T is `number` in Num<T>, but E is a struct");
        assert!(go("a: Int<f64>;").contains("T is `int` in Int<T>, but f64 is not that kind of type"), "{}", go("a: Int<f64>;"));
        assert!(go("a: Txt<i64>;").contains("T is `text`"));
        assert!(go("a: Rec<i64>;").contains("T is `struct` in Rec<T>, but i64 is not a struct") || go("a: Rec<i64>;").contains("T is `struct`"), "{}", go("a: Rec<i64>;"));
        assert!(go("a: Stack;").contains("`Stack` is a generic struct: write its arguments, `Stack<T, N>`"));
        // the right kinds are fine
        assert!(plain("struct E { a: i64; }\nstruct Num<T: number> { v: T; }\nstruct Rec<T: struct> { v: T[2]; }\nstruct Txt<T: text> { v: T; }\nstruct G { a: Num<f32>; b: Rec<E>; c: Txt<string<4>>; }\nstate g: G;\n", "").is_ok());
    }

    #[test]
    fn declaration_errors() {
        assert!(plain("struct P<T, T> { a: T; }\nstruct G { a: P<i64, i64>; }\nstate g: G;\n", "").unwrap_err().contains("`T` is declared twice"));
        assert!(plain("struct E { a: i64; }\nstruct P<E> { a: E; }\nstate g: E;\n", "").unwrap_err().contains("has the name of a struct"));
        assert!(plain("struct P<K: field of T, T> { a: T; }\nstate g: P<i64, i64>;\n", "").unwrap_err().contains("T must be a struct"));
        assert!(plain("struct P<T> { a: T; }\nstate g: P<i64>;\n", "fn f<T>(x: T) {}").unwrap_err().contains("generic functions of no type are not supported yet"));
        assert!(plain("struct P<T, N: count> { a: T[N]; }\nstate g: P<i64, 2>;\n", "fn f(self: P<T>) {}").unwrap_err().contains("takes 2 parameters, the receiver names 1"));
        assert!(plain("struct P<T, N: count> { a: T[N]; }\nstate g: P<i64, 2>;\n", "fn f(self: P<T, 4>) {}").unwrap_err().contains("is not a parameter name"));
        assert!(plain("struct P<T, N: count> { a: T[N]; }\nstate g: P<i64, 2>;\n", "fn f(self: P) {}").unwrap_err().contains("names its parameters"));
    }

    #[test]
    fn a_recursive_generic_type_hits_the_depth_limit() {
        let e = plain("struct R<T> { r: R<R<T>>; }\nstate g: R<i64>;\n", "").unwrap_err();
        assert!(e.contains("nests 65 levels deep (more than 64)") && e.contains("recursive generic type"), "{}", e);
        // a type that holds itself by the same arguments is one instance, not a loop
        let (st, _) = plain("struct L<T> { v: T; next: L<T>?; }\nstate g: L<i64>;\n", "").unwrap();
        assert_eq!(st.matches("struct __mito__L__i64").count(), 1, "{}", st);
    }

    #[test]
    fn the_instance_cap_stops_a_runaway() {
        let mut state = "struct B<N: count> { a: i64[N]; }\nstruct G {".to_string();
        for n in 1..=1001 { state += &format!(" f{}: B<{}>;", n, n); }
        state += " }\nstate g: G;\n";
        assert!(plain(&state, "").unwrap_err().contains("more than 1000 generic instances"));
    }

    #[test]
    fn fields_named_like_parameters_and_strings_are_not_replaced() {
        let asked: HashSet<(String, String)> = [("P<i64>".to_string(), "show".to_string())].into_iter().collect();
        let (_, api, _) = expand_once("struct P<T> { v: T; }
state g: P<i64>;
", "fn show(self: P<U>) -> text { let a = self.U; return \"U \" + a; }
fn go() { g.show(); }
", &asked).unwrap();
        assert!(api.contains("self.U") && api.contains("\"U \""), "{}", api);
    }

    fn load(src: &str) -> Result<(), String> {
        let (st, api) = crate::source::load(src)?;
        crate::cgen::check(&crate::layout::Program::parse(&st)?, &api).map(|_| ())
    }

    #[test]
    fn errors_inside_an_instance_name_it() {
        let head = "struct Stack<T, N: count> { items: T[N]; len: i64 in 0..N; }\nstruct G { a: Stack<i64, 16>; b: Stack<string<12>, 8>; }\nstate g: G;\nfn push(self: Stack<T, N>, x: T) { self.items[self.len] = x; self.len = self.len + 1; }\n";
        assert_eq!(load(&format!("{}@post(\"/p\") fn p() -> Response<{{}}> {{ g.a.push(3); return ok({{}}); }}\n", head)), Ok(()));
        // the wrong element type: the error is in the copy for this instance, and says which
        let e = load(&format!("{}@post(\"/p\") fn p() -> Response<{{}}> {{ g.a.push(\"hi\"); return ok({{}}); }}\n", head)).unwrap_err();
        eprintln!("{}", e);
        assert!(e.contains("Stack<i64, 16>.push") && !e.contains("__mito__"), "{}", e);
        // an error in a body names the instance whose copy it is, and only the instances a call names have a copy
        let body = "fn loud(self: Stack<T, N>) -> i64 { return self.items[0].nothing(); }\n";
        let e = load(&format!("{}{}@post(\"/p\") fn p() -> Response<{{}}> {{ let s = g.b.loud(); return ok({{}}); }}\n", head, body)).unwrap_err();
        assert!(e.contains("fn Stack<string<12>, 8>.loud:") && !e.contains("Stack<i64, 16>.loud") && !e.contains("__mito__"), "{}", e);
        // the error is at the template's own line (5), not a line after the end of the source
        assert!(e.starts_with("5:"), "{}", e);
        // a function the program never names is not copied
        assert_eq!(load(&format!("{}{}@post(\"/p\") fn p() -> Response<{{}}> {{ g.b.push(\"x\"); return ok({{}}); }}\n", head, body)), Ok(()));
        // a struct whose parameters are wrong fails before any body is looked at
        let e = load("struct Stack<T: number, N: count> { items: T[N]; }\nstruct E { a: i64; }\nstruct G { a: Stack<E, 4>; }\nstate g: G;\n").unwrap_err();
        assert!(e.contains("`Stack<E, 4>`: T is `number` in Stack<T, N>, but E is a struct"), "{}", e);
        // an instance a program names twice, each time written differently, is one
        let (st, _) = plain("struct S<T, N: count> { a: T[N]; }\nstruct G { x: S<i64,4>; y: S< i64 , 4 >; }\nstate g: G;\n", "").unwrap();
        assert_eq!(st.matches("struct __mito__S__").count(), 1, "{}", st);
    }

    const POP_HEAD: &str = "struct Stack<T, N: count> { items: T[N]; len: i64 in 0..N; }\nstruct G { a: Stack<i64, 8>; b: Stack<string<12>, 8>; }\nstate g: G;\nfn push(self: Stack<T, N>, x: T) { self.items[self.len] = x; self.len = self.len + 1; }\nfn pop(self: Stack<T, N>) -> T {\n    if self.len == 0 { fail 409 \"empty\"; }\n    self.len = self.len - 1;\n    return self.items[self.len];\n}\n";

    #[test]
    fn a_function_is_copied_only_for_the_instances_a_call_names() {
        // `pop` returns T: fine for the number instance, impossible for the text one, which nothing pops
        let ok = format!("{}@post(\"/p\") fn p() -> Response<{{ x: i64 }}> {{ g.b.push(\"hi\"); g.a.push(3); return ok({{ x: g.a.pop() }}); }}\n", POP_HEAD);
        assert_eq!(load(&ok), Ok(()));
        let (_, api) = crate::source::load(&ok).unwrap();
        assert!(api.contains("fn pop(self: __mito__Stack__i64__8)") && !api.contains("fn pop(self: __mito__Stack__string_12__8"), "{}", api);
        assert!(api.contains("fn push(self: __mito__Stack__i64__8, x: i64)") && api.contains("fn push(self: __mito__Stack__string_12__8, x: string<12>)"), "{}", api);
        // calling it on the text instance is an error that names the instance, at the template's line in the program's own source
        let bad = format!("{}@post(\"/p\") fn p() -> Response<{{}}> {{ let s = g.b.pop(); return ok({{}}); }}\n", POP_HEAD);
        let e = load(&bad).unwrap_err();
        assert!(e.contains("Stack<string<12>, 8>.pop") && !e.contains("Stack<i64, 8>") && !e.contains("__mito__"), "{}", e);
        let place: u32 = e.split(':').next().unwrap().parse().unwrap();
        assert!((5..=9).contains(&place), "line {} is not in the template of pop: {}", place, e);
        // a call of a function no generic struct has is the ordinary error
        let e = load(&format!("{}@post(\"/p\") fn p() -> Response<{{}}> {{ g.a.nothing(); return ok({{}}); }}\n", POP_HEAD)).unwrap_err();
        assert!(e.contains("unknown function `g.a.nothing`"), "{}", e);
        // a copy asks for another function: the copies follow the calls, however deep
        let deep = "struct Stack<T, N: count> { items: T[N]; len: i64 in 0..N; }\nstruct G { a: Stack<i64, 8>; b: Stack<string<12>, 8>; }\nstate g: G;\nfn size(self: Stack<T, N>) -> i64 { return self.len; }\nfn twice(self: Stack<T, N>) -> i64 { return self.size() + self.size(); }\nfn bad(self: Stack<T, N>) -> T { return self.items[0]; }\n@post(\"/p\") fn p() -> Response<{ n: i64 }> { return ok({ n: g.a.twice() }); }\n";
        assert_eq!(load(deep), Ok(()));
        let (_, api) = crate::source::load(deep).unwrap();
        assert!(api.contains("fn size(self: __mito__Stack__i64__8)") && !api.contains("fn size(self: __mito__Stack__string_12__8") && !api.contains("fn bad"), "{}", api);
    }

    #[test]
    fn a_runtime_failure_in_a_copy_reports_the_templates_line() {
        let src = format!("{}@post(\"/p\") fn p() -> Response<{{ x: i64 }}> {{ return ok({{ x: g.a.pop() }}); }}\n", POP_HEAD);
        let (st, api) = crate::source::load(&src).unwrap();
        let prog = crate::layout::Program::parse(&st).unwrap();
        let a = crate::cgen::check(&prog, &api).unwrap();
        let c = crate::cgen::program_c(&prog, &a, None).unwrap();
        // the fail is on line 6 of the program (the line of `if self.len == 0`), as AT_ says
        assert!(c.contains("#define AT_ 6"), "no AT_ 6 in the C");
    }

    const SORTED: &str = "struct Sorted<T: struct, N: count, key: field of T> { items: pool<T, N>; n: i64 in 0..N; }\nstruct Entry { name: string<8>; pts: i64; small: u8 in 0..9; id: i64; }\nstruct Game { board: Sorted<Entry, 4, pts>; byid: Sorted<Entry, 3, id>; }\nstate game: Game;\nfn put(self: Sorted<T, N, key>, v: T.key) { let it = self.items.add(); it.key = v; self.n = self.n + 1; }\nfn best(self: Sorted<T, N, key>) -> T.key { var top = 0; var i = 0; while i < self.n { if self.items[i].key > top { top = self.items[i].key; } i += 1; } return top; }\n@post(\"/p/{v}\") fn p(v: i64 in 0..1000) -> Response<{ a: i64, b: i64 }> { game.board.put(v); game.byid.put(v + 1); return ok({ a: game.board.best(), b: game.byid.best() }); }\n";

    #[test]
    fn a_parameter_naming_a_field_reads_that_field() {
        assert_eq!(load(SORTED), Ok(()));
        let (st, api) = crate::source::load(SORTED).unwrap();
        // each instance reads its own field: `key` is replaced after a `.` too
        assert!(api.contains("it.pts = v;") && api.contains("it.id = v;"), "{}", api);
        assert!(api.contains("self.items[i].pts > top") && api.contains("self.items[i].id > top") && !api.contains(".key"), "{}", api);
        assert!(st.contains("__mito__Sorted__Entry__4__pts"), "{}", st);
    }

    #[test]
    fn field_of_constraint_errors_name_the_instance() {
        let go = |arg: &str| load(&SORTED.replace("Sorted<Entry, 3, id>", arg)).unwrap_err();
        assert_eq!(go("Sorted<Entry, 3, nope>"), "`Sorted<Entry, 3, nope>`: key is a field of T in Sorted<T, N, key>, but Entry has no field `nope`");
        assert_eq!(go("Sorted<Entry, 3, name>"), "`Sorted<Entry, 3, name>`: key is a field of T in Sorted<T, N, key>, but Entry.name is string<8>, not an integer");
        assert!(go("Sorted<Entry, 3, 5>").contains("key is a field of T in Sorted<T, N, key>, but 5 is not a field name"), "{}", go("Sorted<Entry, 3, 5>"));
        assert!(go("Sorted<i64, 3, id>").contains("T is `struct` in Sorted<T, N, key>, but i64 is not a struct"), "{}", go("Sorted<i64, 3, id>"));
        // a range on an integer field is still an integer
        assert_eq!(load(&SORTED.replace("Sorted<Entry, 3, id>", "Sorted<Entry, 3, small>")), Ok(()));
        // declarations: the struct the field belongs to is a `struct` parameter or a struct
        assert!(plain("struct P<K: field of T, T> { a: T; }\nstate g: P<i64, i64>;\n", "").unwrap_err().contains("T must be a struct, `T: struct`"));
        assert!(plain("struct P<K: field of Q, T: struct> { a: T; }\nstate g: P<i64, i64>;\n", "").unwrap_err().contains("Q is neither a parameter of P<...> nor a struct"));
        // a field of a named struct (not a parameter)
        let src = "struct E { a: i64; b: string<4>; }\nstruct P<K: field of E, N: count> { v: i64[N]; }\nstruct G { x: P<a, 2>; }\nstate g: G;\n";
        assert!(plain(src, "").is_ok());
        assert!(plain(&src.replace("P<a, 2>", "P<b, 2>"), "").unwrap_err().contains("E.b is string<4>, not an integer"));
        // an instance of a generic struct can be the element: its fields are the instance's, as written in the template
        let nested = "struct Pair<A, B> { a: A; b: B; }\nstruct S<T: struct, key: field of T> { v: T; }\nstruct G { x: S<Pair<i64, i64>, a>; }\nstate g: G;\n";
        assert!(plain(nested, "").is_ok(), "{:?}", plain(nested, ""));
        assert!(plain(&nested.replace("Pair<i64, i64>, a", "Pair<i64, i64>, c"), "").unwrap_err().contains("Pair<i64, i64> has no field `c`"));
    }
}
