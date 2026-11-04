//! Parse state declarations and compute their exact byte layout.
//!
//! Plain values (scalars, strings, fixed arrays, structs, refs outside any collection) are laid out record-style at fixed
//! offsets, as in Stasis. Every `list` and `pool` is laid out as columns: one array per leaf field, holding `mult * cap`
//! items, where `mult` is how many instances of the collection exist (the product of the capacities of the collections it
//! is nested in). Instance `i` owns items `i*cap .. (i+1)*cap` of every column, so one parent's items are adjacent.
use std::collections::HashMap;

pub const PAGE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scalar { I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, Bool }
impl Scalar {
    pub fn size(self) -> usize {
        use Scalar::*;
        match self { I8 | U8 | Bool => 1, I16 | U16 => 2, I32 | U32 | F32 => 4, I64 | U64 | F64 => 8 }
    }
    pub fn signed(self) -> bool { matches!(self, Scalar::I8 | Scalar::I16 | Scalar::I32 | Scalar::I64) }
    fn parse(s: &str) -> Option<Scalar> {
        use Scalar::*;
        Some(match s { "i8" => I8, "i16" => I16, "i32" => I32, "i64" => I64, "u8" => U8, "u16" => U16, "u32" => U32, "u64" => U64, "f32" => F32, "f64" => F64, "bool" => Bool, _ => return None })
    }
}

/// What appending to a full list does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Full { Fail, DropOldest }

#[derive(Clone, Debug)]
pub enum Ty {
    Scalar(Scalar),
    Str(usize),                 // string<N>: length, then N bytes of capacity
    Arr(Box<Ty>, usize),        // T[N]: exactly N elements
    List(Box<Ty>, usize, Full), // list<T, N> [drop_oldest]
    Pool(Box<Ty>, usize, bool), // pool<T, N> [checked]
    Ref(String),                // ref<pool>: a slot of a top-level pool
    Struct(usize),
    Named(String),              // only before resolution
}
impl Ty { pub fn is_coll(&self) -> bool { matches!(self, Ty::List(..) | Ty::Pool(..)) } }

pub struct StructDef { pub name: String, pub fields: Vec<(String, Ty)>, pub offs: Vec<usize>, pub size: usize, pub align: usize, state: u8 }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind { List(Full), Pool(bool) }

/// One column of a collection. Strings take two (length, bytes).
#[derive(Clone, Debug)]
pub enum CKind { Scalar(Scalar), Arr(Scalar, usize), StrLen, StrBytes(usize), Ref(usize) }
impl CKind {
    pub fn size(&self) -> usize { match self { CKind::Scalar(s) => s.size(), CKind::Arr(s, k) => s.size() * k, CKind::StrLen => 4, CKind::StrBytes(m) => *m, CKind::Ref(_) => 8 } }
    fn align(&self) -> usize { match self { CKind::Scalar(s) | CKind::Arr(s, _) => s.size(), CKind::StrBytes(_) => 1, _ => 4 } }
}
#[derive(Clone, Debug)]
pub struct Col { pub path: String, pub kind: CKind, pub off: usize }

#[derive(Clone, Debug)]
pub struct Coll {
    pub name: String,
    pub kind: Kind,
    pub cap: usize,
    pub mult: usize,
    pub elem: Ty,
    pub parent: Option<usize>, // the collection whose items hold this one
    pub hdr: usize,  // list: len, head; pool: high-water, count, hint, ever. Each a u32 array of `mult` entries
    pub live: usize, // pool: live bits, `words()` u64 per instance
    pub gen: usize,  // checked pool: u32 per item
    pub cols: Vec<Col>,
    pub children: Vec<(String, usize)>,
    pub start: usize,
    pub end: usize, // includes nested collections
}
impl Coll {
    pub fn words(&self) -> usize { self.cap.div_ceil(64) }
    pub fn hdr_fields(&self) -> usize { if matches!(self.kind, Kind::List(_)) { 2 } else { 4 } }
    pub fn checked(&self) -> bool { self.kind == Kind::Pool(true) }
    pub fn is_list(&self) -> bool { matches!(self.kind, Kind::List(_)) }
}

pub struct Global { pub name: String, pub ty: Ty, pub off: usize, pub coll: Option<usize> }

/// A module instance (`global scores: sorted<Score, 100, points>;`, expanded by `modules::expand`): its state is the
/// plain global `name`; `key` is the field of its items it keeps ordered, if any.
#[derive(Clone, Debug)]
pub struct ModInst { pub name: String, pub module: String, pub key: Option<String> }

pub struct Program {
    pub instances: Vec<ModInst>,
    pub structs: Vec<StructDef>,
    pub globals: Vec<Global>,
    pub colls: Vec<Coll>,
    pub plain_colls: Vec<(usize, usize, String, usize)>, // (offset of a plain struct, its struct id, field name, collection)
    pub plain_end: usize,
    pub total: usize,
}

pub fn up(n: usize, to: usize) -> usize { (n + to - 1) / to * to }

impl Program {
    /// Size and alignment of a plain (record-style) value. Collections inside a plain struct take no bytes there.
    pub fn plain_size(&self, t: &Ty) -> (usize, usize) {
        match t {
            Ty::Scalar(s) => (s.size(), s.size()),
            Ty::Str(n) => (up(4 + n, 4), 4),
            Ty::Arr(e, n) => { let (s, a) = self.plain_size(e); (s * n, a) }
            Ty::Ref(_) => (8, 4),
            Ty::Struct(i) => (self.structs[*i].size, self.structs[*i].align),
            Ty::List(..) | Ty::Pool(..) => (0, 1),
            Ty::Named(_) => unreachable!(),
        }
    }
    pub fn show(&self, t: &Ty) -> String {
        match t {
            Ty::Scalar(s) => format!("{:?}", s).to_lowercase(),
            Ty::Str(n) => format!("string<{}>", n),
            Ty::Arr(e, n) => format!("{}[{}]", self.show(e), n),
            Ty::List(e, n, f) => format!("list<{}, {}>{}", self.show(e), n, if *f == Full::DropOldest { " drop_oldest" } else { "" }),
            Ty::Pool(e, n, c) => format!("pool<{}, {}>{}", self.show(e), n, if *c { " checked" } else { "" }),
            Ty::Ref(p) => format!("ref<{}>", p),
            Ty::Struct(i) => self.structs[*i].name.clone(),
            Ty::Named(n) => n.clone(),
        }
    }
    pub fn global(&self, name: &str) -> Option<usize> { self.globals.iter().position(|g| g.name == name) }
    pub fn coll_of(&self, name: &str) -> Option<usize> { self.global(name).and_then(|g| self.globals[g].coll) }
    /// The pool `ref<name>` points into: a top-level pool, or a module instance's items.
    pub fn ref_pool(&self, name: &str) -> Option<usize> {
        let c = self.coll_of(name).or_else(|| self.instances.iter().find(|m| m.name == name).and_then(|m| self.colls.iter().position(|c| c.name == format!("{}.items", m.name))));
        c.filter(|c| matches!(self.colls[*c].kind, Kind::Pool(_)))
    }
    pub fn plain_coll(&self, off: usize, sid: usize, field: &str) -> Option<usize> { self.plain_colls.iter().find(|p| p.0 == off && p.1 == sid && p.2 == field).map(|p| p.3) }

    pub fn parse(src: &str) -> Result<Program, String> {
        let mut p = P { t: tokenize(src), i: 0 };
        let mut structs: Vec<StructDef> = Vec::new();
        let mut globals: Vec<(String, Ty)> = Vec::new();
        let mut instances: Vec<ModInst> = Vec::new();
        while p.i < p.t.len() {
            match p.ident()?.as_str() {
                "struct" => {
                    let name = p.ident()?; p.eat("{")?;
                    let mut fields = Vec::new();
                    while p.peek() != Some("}") { let f = p.ident()?; p.eat(":")?; let ty = p.ty()?; p.eat(";")?; fields.push((f, ty)); }
                    p.eat("}")?;
                    structs.push(StructDef { name, fields, offs: Vec::new(), size: 0, align: 1, state: 0 });
                }
                "global" => { let n = p.ident()?; p.eat(":")?; let ty = p.ty()?; p.eat(";")?; globals.push((n, ty)); }
                // written by module expansion: `instance scores of sorted key points;`
                "instance" => {
                    let name = p.ident()?; p.eat("of")?; let module = p.ident()?;
                    let key = if p.word("key") { Some(p.ident()?) } else { None };
                    p.eat(";")?;
                    instances.push(ModInst { name, module, key });
                }
                o => return Err(format!("expected `struct` or `global`, found `{}`", o)),
            }
        }
        let mut prog = Program { instances, structs, globals: Vec::new(), colls: Vec::new(), plain_colls: Vec::new(), plain_end: 0, total: 0 };
        let names: HashMap<String, usize> = prog.structs.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();
        for i in 0..prog.structs.len() {
            let fs = std::mem::take(&mut prog.structs[i].fields);
            let fs = fs.into_iter().map(|(n, t)| Ok((n, resolve(t, &names)?))).collect::<Result<Vec<_>, String>>()?;
            prog.structs[i].fields = fs;
        }
        for i in 0..prog.structs.len() { prog.layout_struct(i)?; }
        // plain values first
        let mut off = 0;
        for (n, ty) in globals {
            let ty = resolve(ty, &names)?;
            let (s, a) = prog.plain_size(&ty);
            off = up(off, a.max(8));
            prog.globals.push(Global { name: n, ty, off, coll: None });
            off += s;
        }
        prog.plain_end = off;
        // then every collection, each starting on a page so it can be trimmed on its own
        let mut cur = up(off, PAGE);
        let mut refs: Vec<(usize, usize, String)> = Vec::new();
        for g in 0..prog.globals.len() {
            let ty = prog.globals[g].ty.clone();
            let name = prog.globals[g].name.clone();
            if ty.is_coll() {
                let cid = prog.build_coll(&name, &ty, 1, None, &mut cur, &mut refs, 0)?;
                prog.globals[g].coll = Some(cid);
            } else if let Ty::Struct(s) = ty {
                let base = prog.globals[g].off;
                prog.plain_struct_colls(s, base, &name, &mut cur, &mut refs)?;
            } else if contains_coll(&prog, &ty) {
                return Err(format!("`{}`: collections inside fixed arrays are not supported; use a nested list", name));
            }
        }
        for (cid, ci, name) in refs {
            let Some(t) = prog.ref_pool(&name) else { return Err(format!("ref<{}>: `{}` is not a top-level pool", name, name)) };
            prog.colls[cid].cols[ci].kind = CKind::Ref(t);
        }
        for g in &prog.globals { check_plain_refs(&prog, &g.ty)?; }
        prog.total = cur;
        Ok(prog)
    }

    fn layout_struct(&mut self, i: usize) -> Result<(), String> {
        match self.structs[i].state { 2 => return Ok(()), 1 => return Err(format!("struct `{}` contains itself directly: sizes must be fixed (nest it in a list or pool)", self.structs[i].name)), _ => {} }
        self.structs[i].state = 1;
        let fields = self.structs[i].fields.clone();
        for (_, t) in &fields { self.layout_deps(t)?; }
        let (mut off, mut align, mut offs) = (0, 1, Vec::new());
        for (_, t) in &fields {
            if t.is_coll() { offs.push(usize::MAX); continue; }
            let (s, a) = self.plain_size(t);
            off = up(off, a); offs.push(off); off += s; align = align.max(a);
        }
        let s = &mut self.structs[i];
        s.offs = offs; s.size = up(off.max(1), align); // at least one byte, so two plain structs never share an offset
        let _ = 0; s.align = align; s.state = 2;
        Ok(())
    }
    fn layout_deps(&mut self, t: &Ty) -> Result<(), String> {
        match t { Ty::Struct(i) => self.layout_struct(*i), Ty::Arr(e, _) => self.layout_deps(e), _ => Ok(()) } // collections break cycles
    }

    fn plain_struct_colls(&mut self, s: usize, base: usize, path: &str, cur: &mut usize, refs: &mut Vec<(usize, usize, String)>) -> Result<(), String> {
        let fields = self.structs[s].fields.clone();
        for (k, (n, t)) in fields.iter().enumerate() {
            let p = format!("{}.{}", path, n);
            if t.is_coll() {
                let cid = self.build_coll(&p, t, 1, None, cur, refs, 0)?;
                self.plain_colls.push((base, s, n.clone(), cid));
            } else if let Ty::Struct(c) = t {
                let o = base + self.structs[s].offs[k];
                self.plain_struct_colls(*c, o, &p, cur, refs)?;
            } else if contains_coll(self, t) {
                return Err(format!("`{}`: collections inside fixed arrays are not supported; use a nested list", p));
            }
        }
        Ok(())
    }

    fn build_coll(&mut self, name: &str, ty: &Ty, mult: usize, parent: Option<usize>, cur: &mut usize, refs: &mut Vec<(usize, usize, String)>, depth: usize) -> Result<usize, String> {
        if depth > 8 { return Err(format!("`{}`: collections nested more than 8 deep (a struct that contains itself?)", name)); }
        let (elem, cap, kind) = match ty { Ty::List(e, n, f) => ((**e).clone(), *n, Kind::List(*f)), Ty::Pool(e, n, c) => ((**e).clone(), *n, Kind::Pool(*c)), _ => unreachable!() };
        if cap == 0 { return Err(format!("`{}`: capacity must be at least 1", name)); }
        let items = mult.checked_mul(cap).filter(|n| *n <= 1 << 32).ok_or(format!("`{}`: too many items ({} x {})", name, mult, cap))?;
        let cid = self.colls.len();
        let start = up(*cur, PAGE);
        let mut c = Coll { name: name.to_string(), kind, cap, mult, elem: elem.clone(), parent, hdr: start, live: 0, gen: 0, cols: Vec::new(), children: Vec::new(), start, end: 0 };
        let mut o = start + c.hdr_fields() * 4 * mult;
        if let Kind::Pool(checked) = kind {
            o = up(o, 8); c.live = o; o += mult * c.words() * 8;
            if checked { o = up(o, 4); c.gen = o; o += items * 4; }
        }
        let mut leaves = Vec::new();
        let mut kids = Vec::new();
        self.flatten(&elem, "", &mut leaves, &mut kids, name)?;
        if kind == Kind::List(Full::DropOldest) && !kids.is_empty() { return Err(format!("`{}`: a drop_oldest list cannot hold nested collections", name)); }
        for (path, t) in leaves {
            let place = |k: CKind, o: &mut usize| { *o = up(*o, k.align().max(1)); let col = Col { path: path.clone(), kind: k.clone(), off: *o }; *o += items * k.size(); col };
            match t {
                Ty::Scalar(s) => { let col = place(CKind::Scalar(s), &mut o); c.cols.push(col); }
                Ty::Arr(e, k) => { let Ty::Scalar(s) = *e else { unreachable!() }; let col = place(CKind::Arr(s, k), &mut o); c.cols.push(col); }
                Ty::Str(m) => { let a = place(CKind::StrLen, &mut o); c.cols.push(a); let b = place(CKind::StrBytes(m), &mut o); c.cols.push(b); }
                Ty::Ref(p) => { refs.push((cid, c.cols.len(), p)); let col = place(CKind::Ref(usize::MAX), &mut o); c.cols.push(col); }
                _ => unreachable!(),
            }
        }
        self.colls.push(c);
        *cur = o;
        for (path, t) in kids {
            let k = self.build_coll(&format!("{}.{}", name, path), &t, items, Some(cid), cur, refs, depth + 1)?;
            self.colls[cid].children.push((path, k));
        }
        self.colls[cid].end = *cur;
        Ok(cid)
    }

    fn flatten(&self, t: &Ty, prefix: &str, leaves: &mut Vec<(String, Ty)>, kids: &mut Vec<(String, Ty)>, name: &str) -> Result<(), String> {
        let join = |f: &str| if prefix.is_empty() { f.to_string() } else { format!("{}.{}", prefix, f) };
        match t {
            Ty::Struct(i) => for (f, ft) in &self.structs[*i].fields { self.flatten(ft, &join(f), leaves, kids, name)?; },
            Ty::Scalar(_) | Ty::Str(_) | Ty::Ref(_) => leaves.push((prefix.to_string(), t.clone())),
            Ty::Arr(e, _) if matches!(**e, Ty::Scalar(_)) => leaves.push((prefix.to_string(), t.clone())),
            Ty::List(..) | Ty::Pool(..) => kids.push((prefix.to_string(), t.clone())),
            _ => return Err(format!("`{}`: items cannot hold `{}` at `{}`; use a nested list instead of a fixed array of structs", name, self.show(t), prefix)),
        }
        Ok(())
    }
}

fn contains_coll(p: &Program, t: &Ty) -> bool {
    match t { Ty::List(..) | Ty::Pool(..) => true, Ty::Arr(e, _) => contains_coll(p, e), Ty::Struct(i) => p.structs[*i].fields.iter().any(|f| contains_coll(p, &f.1)), _ => false }
}
fn check_plain_refs(p: &Program, t: &Ty) -> Result<(), String> {
    match t {
        Ty::Ref(n) => if p.ref_pool(n).is_some() { Ok(()) } else { Err(format!("ref<{}>: `{}` is not a top-level pool", n, n)) },
        Ty::Arr(e, _) => check_plain_refs(p, e),
        Ty::Struct(i) => { for f in &p.structs[*i].fields { if !f.1.is_coll() { check_plain_refs(p, &f.1)?; } } Ok(()) }
        _ => Ok(()),
    }
}

fn resolve(t: Ty, names: &HashMap<String, usize>) -> Result<Ty, String> {
    Ok(match t {
        Ty::Named(n) => Ty::Struct(*names.get(&n).ok_or(format!("unknown type `{}`", n))?),
        Ty::Arr(e, n) => Ty::Arr(Box::new(resolve(*e, names)?), n),
        Ty::List(e, n, f) => Ty::List(Box::new(resolve(*e, names)?), n, f),
        Ty::Pool(e, n, c) => Ty::Pool(Box::new(resolve(*e, names)?), n, c),
        o => o,
    })
}

fn tokenize(src: &str) -> Vec<String> {
    let c: Vec<char> = src.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    while i < c.len() {
        if c[i].is_whitespace() { i += 1; }
        else if c[i] == '/' && c.get(i + 1) == Some(&'/') { while i < c.len() && c[i] != '\n' { i += 1; } }
        else if c[i].is_alphanumeric() || c[i] == '_' { let s = i; while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') { i += 1; } out.push(c[s..i].iter().collect()); }
        else { out.push(c[i].to_string()); i += 1; }
    }
    out
}

struct P { t: Vec<String>, i: usize }
impl P {
    fn peek(&self) -> Option<&str> { self.t.get(self.i).map(|s| s.as_str()) }
    fn eat(&mut self, s: &str) -> Result<(), String> { if self.peek() == Some(s) { self.i += 1; Ok(()) } else { Err(format!("expected `{}`, found `{}`", s, self.peek().unwrap_or("end of file"))) } }
    fn ident(&mut self) -> Result<String, String> { let s = self.peek().ok_or("unexpected end of file")?.to_string(); self.i += 1; Ok(s) }
    fn num(&mut self) -> Result<usize, String> { let s = self.ident()?; s.parse().map_err(|_| format!("expected a number, found `{}`", s)) }
    fn word(&mut self, w: &str) -> bool { if self.peek() == Some(w) { self.i += 1; true } else { false } }
    /// A size known when the program is compiled: a number, or + - * / ( ) of them (modules size state from parameters).
    fn count(&mut self) -> Result<usize, String> {
        let mut v = self.term()?;
        loop {
            match self.peek() { Some("+") => { self.i += 1; v = v.checked_add(self.term()?).ok_or("size too large")?; } Some("-") => { self.i += 1; v = v.checked_sub(self.term()?).ok_or("size below zero")?; } _ => return Ok(v) }
        }
    }
    fn term(&mut self) -> Result<usize, String> {
        let mut v = self.atom()?;
        loop {
            match self.peek() { Some("*") => { self.i += 1; v = v.checked_mul(self.atom()?).ok_or("size too large")?; } Some("/") => { self.i += 1; let d = self.atom()?; if d == 0 { return Err("size divided by zero".into()); } v /= d; } _ => return Ok(v) }
        }
    }
    fn atom(&mut self) -> Result<usize, String> { if self.peek() == Some("(") { self.i += 1; let v = self.count()?; self.eat(")")?; Ok(v) } else { self.num() } }
    fn ty(&mut self) -> Result<Ty, String> {
        let n = self.ident()?;
        let mut t = if let Some(s) = Scalar::parse(&n) { Ty::Scalar(s) }
        else if n == "string" { self.eat("<")?; let k = self.count()?; self.eat(">")?; Ty::Str(k) }
        else if n == "ref" { self.eat("<")?; let p = self.ident()?; self.eat(">")?; Ty::Ref(p) }
        else if n == "list" { self.eat("<")?; let e = self.ty()?; self.eat(",")?; let k = self.count()?; self.eat(">")?; let f = if self.word("drop_oldest") { Full::DropOldest } else { Full::Fail }; Ty::List(Box::new(e), k, f) }
        else if n == "pool" { self.eat("<")?; let e = self.ty()?; self.eat(",")?; let k = self.count()?; self.eat(">")?; let c = self.word("checked"); Ty::Pool(Box::new(e), k, c) }
        else { Ty::Named(n) };
        while self.peek() == Some("[") { self.i += 1; let k = self.count()?; self.eat("]")?; t = Ty::Arr(Box::new(t), k); }
        Ok(t)
    }
}
