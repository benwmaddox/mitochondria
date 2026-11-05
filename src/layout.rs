//! Parse state declarations and compute their exact byte layout.
//!
//! Plain values (scalars, strings, fixed arrays, structs outside any collection) are laid out record-style at fixed
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
    Struct(usize),
    Named(String),              // only before resolution
}
impl Ty { pub fn is_coll(&self) -> bool { matches!(self, Ty::List(..) | Ty::Pool(..)) } }

/// `ranges[k]`: field k's declared range (`limit: i64 in 0..3000000`, both ends included), when it has one: every write is checked against it,
/// and reads give the range analysis that interval.
/// `unlogged[k]`: field k is kept out of the undo log (`scratch: u8[4096] unlogged`): a failed request leaves what it wrote there.
pub struct StructDef { pub name: String, pub fields: Vec<(String, Ty)>, pub ranges: Vec<Option<(i64, i64)>>, pub unlogged: Vec<bool>, pub offs: Vec<usize>, pub size: usize, pub align: usize, state: u8 }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind { List(Full), Pool(bool) }

/// One column of a collection. Strings take two (length, bytes).
#[derive(Clone, Debug)]
pub enum CKind { Scalar(Scalar), Arr(Scalar, usize), StrLen, StrBytes(usize) }
impl CKind {
    pub fn size(&self) -> usize { match self { CKind::Scalar(s) => s.size(), CKind::Arr(s, k) => s.size() * k, CKind::StrLen => 4, CKind::StrBytes(m) => *m } }
    fn align(&self) -> usize { match self { CKind::Scalar(s) | CKind::Arr(s, _) => s.size(), CKind::StrBytes(_) => 1, _ => 4 } }
}
#[derive(Clone, Debug)]
pub struct Col { pub path: String, pub kind: CKind, pub off: usize, pub range: Option<(i64, i64)> }

#[derive(Clone, Debug)]
pub struct Coll {
    pub name: String,
    pub kind: Kind,
    pub cap: usize,
    pub mult: usize,
    /// The struct its items are, when they are structs (an input of type `Player` finds its pool by this).
    pub item: Option<usize>,
    pub parent: Option<usize>, // the collection whose items hold this one
    pub unlogged: bool,        // kept out of the undo log (declared `unlogged`, or inside an unlogged collection's items)
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

/// `enum Op { Plus, Minus }` or `enum Cmd: u8 { Plus = 43, Minus = 45 }`: named integer values, each the one before plus one
/// unless given; stored in the smallest integer that holds them (or the declared one).
#[derive(Clone, Debug)]
pub struct EnumDef { pub name: String, pub base: Scalar, pub values: Vec<(String, i64)> }

pub struct Program {
    pub enums: Vec<EnumDef>,
    /// `request work: Work;`: the per-request struct (offset, bytes on whole 64-byte lines): zeroed before each request, kept out of
    /// the undo log, never kept between requests.
    pub request: Option<(usize, usize)>,
    /// byte ranges kept out of the undo log (unlogged plain fields, unlogged collections), each on whole 64-byte lines
    pub unlogged: Vec<(usize, usize)>,
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
    /// The enums as the binder takes them.
    pub fn enum_table(&self) -> Vec<(String, Vec<(String, i64)>)> { self.enums.iter().map(|e| (e.name.clone(), e.values.clone())).collect() }
    /// Size and alignment of a plain (record-style) value. Collections inside a plain struct take no bytes there.
    pub fn plain_size(&self, t: &Ty) -> (usize, usize) {
        match t {
            Ty::Scalar(s) => (s.size(), s.size()),
            Ty::Str(n) => (up(4 + n, 4), 4),
            Ty::Arr(e, n) => { let (s, a) = self.plain_size(e); (s * n, a) }
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
            Ty::Struct(i) => self.structs[*i].name.clone(),
            Ty::Named(n) => n.clone(),
        }
    }
    pub fn global(&self, name: &str) -> Option<usize> { self.globals.iter().position(|g| g.name == name) }
    /// Where a path of plain structs from the state starts and how many bytes it covers (`game.board`, a module
    /// instance's state).
    pub fn plain_path(&self, path: &str) -> Option<(usize, usize)> {
        let mut parts = path.split('.');
        let g = &self.globals[self.global(parts.next()?)?];
        let (mut off, mut ty) = (g.off, g.ty.clone());
        for f in parts {
            let Ty::Struct(s) = ty else { return None };
            let k = self.structs[s].fields.iter().position(|x| x.0 == f)?;
            off += self.structs[s].offs[k];
            ty = self.structs[s].fields[k].1.clone();
        }
        Some((off, self.plain_size(&ty).0))
    }
    pub fn plain_coll(&self, off: usize, sid: usize, field: &str) -> Option<usize> { self.plain_colls.iter().find(|p| p.0 == off && p.1 == sid && p.2 == field).map(|p| p.3) }

    pub fn parse(src: &str) -> Result<Program, String> {
        let (t, pos) = tokenize(src);
        let mut p = P { t, i: 0, pos };
        let places = declared_places(&p);
        Self::parse_tokens(&mut p).map_err(|e| {
            if crate::lang::starts_with_place(&e) { return e; }
            // stopped while reading: where it stopped (a missing `;` at the end of what came before)
            if p.i < p.t.len() {
                let k = if e.starts_with("expected `;`") { p.i.saturating_sub(1) } else { p.i };
                let (l, c) = p.pos[k];
                let c = if k < p.i { c + p.t[k].chars().count() as u32 } else { c };
                return format!("{}:{}: {}", l, c, e);
            }
            // found later: at the declaration of the first struct or field the message names
            let named = e.split('`').skip(1).step_by(2).find_map(|n| places.iter().find(|x| x.0 == n).map(|x| x.1));
            match named { Some((l, c)) => format!("{}:{}: {}", l, c, e), None => e }
        })
    }
    fn parse_tokens(p: &mut P) -> Result<Program, String> {
        if let Some(t) = p.t.iter().find(|t| t.len() > crate::lang::MAX_NAME) { return Err(format!("a name longer than {} bytes: `{}...`", crate::lang::MAX_NAME, t.chars().take(32).collect::<String>())); }
        let mut structs: Vec<StructDef> = Vec::new();
        let mut globals: Vec<(String, Ty)> = Vec::new();
        let mut instances: Vec<ModInst> = Vec::new();
        let mut enums: Vec<EnumDef> = Vec::new();
        let mut request: Option<(String, Ty)> = None;
        while p.i < p.t.len() {
            match p.ident()?.as_str() {
                "enum" => {
                    let name = p.ident()?;
                    let declared = if p.peek() == Some(":") { p.eat(":")?; let b = p.ident()?; Some(Scalar::parse(&b).filter(|s| !matches!(s, Scalar::F32 | Scalar::F64 | Scalar::Bool)).ok_or(format!("enum `{}`: `{}` is not an integer type", name, b))?) } else { None };
                    p.eat("{")?;
                    let (mut values, mut next): (Vec<(String, i64)>, i64) = (Vec::new(), 0);
                    while p.peek() != Some("}") {
                        let v = p.ident()?;
                        if p.peek() == Some("=") { p.eat("=")?; next = p.signed()?; }
                        if values.iter().any(|x: &(String, i64)| x.0 == v) { return Err(format!("enum `{}` has `{}` twice", name, v)); }
                        if let Some(o) = values.iter().find(|x| x.1 == next) { return Err(format!("enum `{}`: `{}` and `{}` are both {}", name, o.0, v, next)); }
                        values.push((v, next));
                        next += 1;
                        if p.peek() == Some(",") { p.eat(",")?; }
                    }
                    p.eat("}")?;
                    if values.is_empty() { return Err(format!("enum `{}` has no values", name)); }
                    let (lo, hi) = (values.iter().map(|x| x.1).min().unwrap_or(0), values.iter().map(|x| x.1).max().unwrap_or(0));
                    let fits = |s: Scalar| -> bool { let (a, b): (i128, i128) = match s { Scalar::I8 => (-128, 127), Scalar::I16 => (-32768, 32767), Scalar::I32 => (i32::MIN as i128, i32::MAX as i128), Scalar::U8 => (0, 255), Scalar::U16 => (0, 65535), Scalar::U32 => (0, u32::MAX as i128), _ => (i64::MIN as i128, i64::MAX as i128) }; lo as i128 >= a && hi as i128 <= b };
                    let base = match declared {
                        Some(b) => { if !fits(b) { return Err(format!("enum `{}`: its values ({}..{}) do not fit {:?}", name, lo, hi, b).to_lowercase()); } b }
                        None => [Scalar::U8, Scalar::I8, Scalar::U16, Scalar::I16, Scalar::U32, Scalar::I32, Scalar::I64].into_iter().find(|s| fits(*s)).unwrap_or(Scalar::I64),
                    };
                    if enums.iter().any(|e| e.name == name) { return Err(format!("enum `{}` is declared twice", name)); }
                    enums.push(EnumDef { name, base, values });
                }
                "struct" => {
                    let name = p.ident()?; p.eat("{")?;
                    let mut fields = Vec::new();
                    let mut ranges = Vec::new();
                    let mut unlogged = Vec::new();
                    while p.peek() != Some("}") {
                        let f = p.ident()?; p.eat(":")?; let ty = p.ty()?;
                        // `in lo..hi`: a declared range for an integer field, both ends included
                        let range = if p.word("in") { let lo = p.signed()?; p.eat(".")?; p.eat(".")?; let hi = p.signed()?; Some((lo, hi)) } else { None };
                        if let Some((lo, hi)) = range {
                            let Ty::Scalar(sc) = &ty else { return Err(format!("`{}.{}`: only an integer field can have a range", name, f)) };
                            let (tl, th): (i128, i128) = match sc { Scalar::I8 => (-128, 127), Scalar::I16 => (-32768, 32767), Scalar::I32 => (i32::MIN as i128, i32::MAX as i128), Scalar::I64 => (i64::MIN as i128, i64::MAX as i128),
                                Scalar::U8 => (0, 255), Scalar::U16 => (0, 65535), Scalar::U32 => (0, u32::MAX as i128), Scalar::U64 => (0, i64::MAX as i128), _ => return Err(format!("`{}.{}`: only an integer field can have a range", name, f)) };
                            if lo > hi || (lo as i128) < tl || (hi as i128) > th { return Err(format!("`{}.{}`: range {}..{} does not fit its type", name, f, lo, hi)); }
                            if lo > 0 || hi < 0 { return Err(format!("`{}.{}`: range {}..{} must include 0, the value a field starts with", name, f, lo, hi)); }
                        }
                        // `unlogged`: kept out of the undo log (a plain field, or a whole collection with what its items hold)
                        let unl = p.word("unlogged");
                        p.eat(";")?;
                        fields.push((f, ty)); ranges.push(range); unlogged.push(unl);
                    }
                    p.eat("}")?;
                    structs.push(StructDef { name, fields, ranges, unlogged, offs: Vec::new(), size: 0, align: 1, state: 0 });
                }
                "state" => {
                    let n = p.ident()?; p.eat(":")?; let ty = p.ty()?; p.eat(";")?;
                    if let Some(g) = globals.first() { return Err(format!("a program has one state; `{}` is the second (the first is `{}`): put everything in one struct", n, g.0)); }
                    globals.push((n, ty));
                }
                // `request work: Work;`: plain fields for the work of one request (zeroed before it, never kept)
                "request" => {
                    let n = p.ident()?; p.eat(":")?; let ty = p.ty()?; p.eat(";")?;
                    if let Some(r) = &request { return Err(format!("a program has one request struct; `{}` is the second (the first is `{}`)", n, r.0)); }
                    request = Some((n, ty));
                }
                "global" => return Err("`global` was replaced by `state`: a program has one state, a struct holding everything (`state game: Game;`)".into()),
                // written by module expansion: `instance game.board of sorted key points;`
                "instance" => {
                    let name = p.path()?; p.eat("of")?; let module = p.ident()?;
                    let key = if p.word("key") { Some(p.ident()?) } else { None };
                    p.eat(";")?;
                    instances.push(ModInst { name, module, key });
                }
                o => return Err(format!("expected `struct`, `enum`, `state` or `request`, found `{}`", o)),
            }
        }
        if globals.is_empty() { return Err("a program has one state (`state game: Game;`), and this one has none".into()); }
        // a field of an enum type is its integer, checked to hold one of the enum's values' range (which includes 0, the value it starts with)
        for sd in structs.iter_mut() {
            for k in 0..sd.fields.len() {
                let named = match &sd.fields[k].1 { Ty::Named(n) => Some((n.clone(), false)), Ty::Arr(e, _) => match &**e { Ty::Named(n) => Some((n.clone(), true)), _ => None }, _ => None };
                let Some((n, arr)) = named else { continue };
                let Some(en) = enums.iter().find(|e| e.name == n) else { continue };
                let (lo, hi) = (en.values.iter().map(|x| x.1).min().unwrap_or(0).min(0), en.values.iter().map(|x| x.1).max().unwrap_or(0).max(0));
                if arr { if let Ty::Arr(_, len) = sd.fields[k].1 { sd.fields[k].1 = Ty::Arr(Box::new(Ty::Scalar(en.base)), len); } }
                else { sd.fields[k].1 = Ty::Scalar(en.base); if sd.ranges[k].is_none() { sd.ranges[k] = Some((lo, hi)); } }
            }
        }
        let mut prog = Program { enums, request: None, unlogged: Vec::new(), instances, structs, globals: Vec::new(), colls: Vec::new(), plain_colls: Vec::new(), plain_end: 0, total: 0 };
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
            // a state holding nothing but collections takes no plain bytes (its inner structs still get distinct offsets)
            let (s, a) = if prog.has_plain(&ty) { prog.plain_size(&ty) } else { (0, 1) };
            off = up(off, a.max(8));
            prog.globals.push(Global { name: n, ty, off, coll: None });
            off += s;
        }
        // the request struct: its own 64-byte lines after the state's plain values
        if let Some((n, ty)) = request {
            let ty = resolve(ty, &names)?;
            if prog.globals.iter().any(|g| g.name == n) { return Err(format!("`{}` names both the state and the request struct", n)); }
            if contains_coll(&prog, &ty) { return Err(format!("request `{}`: the request struct holds plain fields (text, numbers, arrays, structs); keep collections in the state", n)); }
            let (s, _) = prog.plain_size(&ty);
            off = up(off, 64);
            let size = up(s.max(1), 64);
            prog.globals.push(Global { name: n, ty, off, coll: None });
            prog.request = Some((off, size));
            off += size;
        }
        prog.plain_end = off;
        // then every collection, each starting on a page so it can be trimmed on its own
        let mut cur = up(off, PAGE);
        for g in 0..prog.globals.len() {
            let ty = prog.globals[g].ty.clone();
            let name = prog.globals[g].name.clone();
            if ty.is_coll() {
                let cid = prog.build_coll(&name, &ty, 1, None, false, &mut cur, 0)?;
                prog.globals[g].coll = Some(cid);
            } else if let Ty::Struct(s) = ty {
                let base = prog.globals[g].off;
                prog.plain_struct_colls(s, base, &name, &mut cur)?;
            } else if contains_coll(&prog, &ty) {
                return Err(format!("`{}`: collections inside fixed arrays are not supported; use a nested list", name));
            }
        }
        // the bytes kept out of the undo log (the request struct whole)
        for g in 0..prog.globals.len() { if let Ty::Struct(st) = prog.globals[g].ty.clone() { let base = prog.globals[g].off; if prog.request.map_or(true, |r| r.0 != base) { prog.plain_unlogged(st, base, false); } } }
        for c in 0..prog.colls.len() { if prog.colls[c].unlogged && prog.colls[c].parent.is_none() { let (a, b) = (prog.colls[c].start, prog.colls[c].end); prog.unlogged.push((a, up(b, 64))); } }
        if let Some((a, n)) = prog.request { prog.unlogged.push((a, a + n)); }
        prog.total = cur;
        Ok(prog)
    }

    /// Does a value of this type hold any bytes outside collections?
    fn has_plain(&self, t: &Ty) -> bool {
        match t { Ty::Struct(s) => self.structs[*s].fields.iter().any(|f| self.has_plain(&f.1)), t if t.is_coll() => false, _ => true }
    }

    fn layout_struct(&mut self, i: usize) -> Result<(), String> {
        match self.structs[i].state { 2 => return Ok(()), 1 => return Err(format!("struct `{}` contains itself directly: sizes must be fixed (nest it in a list or pool)", self.structs[i].name)), _ => {} }
        self.structs[i].state = 1;
        let fields = self.structs[i].fields.clone();
        let unlogged = self.structs[i].unlogged.clone();
        for (_, t) in &fields { self.layout_deps(t)?; }
        let (mut off, mut align, mut offs) = (0, 1, Vec::new());
        for (k, (_, t)) in fields.iter().enumerate() {
            if t.is_coll() { offs.push(usize::MAX); continue; }
            let (s, a) = self.plain_size(t);
            // an unlogged field has its own 64-byte lines: the undo log saves and restores whole lines, so no logged byte may share one
            let (s, a) = if unlogged[k] { (up(s, 64), a.max(64)) } else { (s, a) };
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

    /// The byte ranges of unlogged plain fields under struct `s` at `base` (`all`: the whole struct is inside an unlogged field).
    fn plain_unlogged(&mut self, s: usize, base: usize, all: bool) {
        let fields = self.structs[s].fields.clone();
        for (k, (_, t)) in fields.iter().enumerate() {
            if t.is_coll() { continue; }
            let o = base + self.structs[s].offs[k];
            let unl = all || self.structs[s].unlogged[k];
            if let Ty::Struct(c) = t { self.plain_unlogged(*c, o, unl); continue; }
            if unl { let (sz, _) = self.plain_size(t); self.unlogged.push((o, o + up(sz, 64))); }
        }
    }
    fn plain_struct_colls(&mut self, s: usize, base: usize, path: &str, cur: &mut usize) -> Result<(), String> {
        let fields = self.structs[s].fields.clone();
        for (k, (n, t)) in fields.iter().enumerate() {
            let p = format!("{}.{}", path, n);
            if t.is_coll() {
                let cid = self.build_coll(&p, t, 1, None, self.structs[s].unlogged[k], cur, 0)?;
                self.plain_colls.push((base, s, n.clone(), cid));
            } else if let Ty::Struct(c) = t {
                let o = base + self.structs[s].offs[k];
                self.plain_struct_colls(*c, o, &p, cur)?;
            } else if contains_coll(self, t) {
                return Err(format!("`{}`: collections inside fixed arrays are not supported; use a nested list", p));
            }
        }
        Ok(())
    }

    fn build_coll(&mut self, name: &str, ty: &Ty, mult: usize, parent: Option<usize>, unlogged: bool, cur: &mut usize, depth: usize) -> Result<usize, String> {
        if depth > 8 { return Err(format!("`{}`: collections nested more than 8 deep (a struct that contains itself?)", name)); }
        let (elem, cap, kind) = match ty { Ty::List(e, n, f) => ((**e).clone(), *n, Kind::List(*f)), Ty::Pool(e, n, c) => ((**e).clone(), *n, Kind::Pool(*c)), _ => unreachable!() };
        if cap == 0 { return Err(format!("`{}`: capacity must be at least 1", name)); }
        let items = mult.checked_mul(cap).filter(|n| *n <= 1 << 32).ok_or(format!("`{}`: too many items ({} x {})", name, mult, cap))?;
        let cid = self.colls.len();
        let start = up(*cur, PAGE);
        let item = if let Ty::Struct(s) = &elem { Some(*s) } else { None };
        let mut c = Coll { name: name.to_string(), kind, cap, mult, item, parent, unlogged, hdr: start, live: 0, gen: 0, cols: Vec::new(), children: Vec::new(), start, end: 0 };
        let mut o = start + c.hdr_fields() * 4 * mult;
        if let Kind::Pool(checked) = kind {
            o = up(o, 8); c.live = o; o += mult * c.words() * 8;
            if checked { o = up(o, 4); c.gen = o; o += items * 4; }
        }
        let mut leaves = Vec::new();
        let mut kids = Vec::new();
        self.flatten(&elem, "", None, &mut leaves, &mut kids, name)?;
        if kind == Kind::List(Full::DropOldest) && !kids.is_empty() { return Err(format!("`{}`: a drop_oldest list cannot hold nested collections", name)); }
        for (path, t, range) in leaves {
            let place = |k: CKind, o: &mut usize| { *o = up(*o, k.align().max(1)); let col = Col { path: path.clone(), kind: k.clone(), off: *o, range: None }; *o += items * k.size(); col };
            match t {
                Ty::Scalar(s) => { let mut col = place(CKind::Scalar(s), &mut o); col.range = range; c.cols.push(col); }
                Ty::Arr(e, k) => { let Ty::Scalar(s) = *e else { unreachable!() }; let col = place(CKind::Arr(s, k), &mut o); c.cols.push(col); }
                Ty::Str(m) => { let a = place(CKind::StrLen, &mut o); c.cols.push(a); let b = place(CKind::StrBytes(m), &mut o); c.cols.push(b); }
                _ => unreachable!(),
            }
        }
        self.colls.push(c);
        *cur = o;
        for (path, t) in kids {
            let k = self.build_coll(&format!("{}.{}", name, path), &t, items, Some(cid), unlogged, cur, depth + 1)?;
            self.colls[cid].children.push((path, k));
        }
        self.colls[cid].end = *cur;
        Ok(cid)
    }

    fn flatten(&self, t: &Ty, prefix: &str, range: Option<(i64, i64)>, leaves: &mut Vec<(String, Ty, Option<(i64, i64)>)>, kids: &mut Vec<(String, Ty)>, name: &str) -> Result<(), String> {
        let join = |f: &str| if prefix.is_empty() { f.to_string() } else { format!("{}.{}", prefix, f) };
        match t {
            Ty::Struct(i) => for (k, (f, ft)) in self.structs[*i].fields.iter().enumerate() {
                if self.structs[*i].unlogged[k] { return Err(format!("`{}`: `{}` is declared unlogged, but items of a collection are logged or not as a whole: declare the collection `unlogged`", name, join(f))); }
                self.flatten(ft, &join(f), self.structs[*i].ranges[k], leaves, kids, name)?;
            },
            Ty::Scalar(_) | Ty::Str(_) => leaves.push((prefix.to_string(), t.clone(), range)),
            Ty::Arr(e, _) if matches!(**e, Ty::Scalar(_)) => leaves.push((prefix.to_string(), t.clone(), None)),
            Ty::List(..) | Ty::Pool(..) => kids.push((prefix.to_string(), t.clone())),
            _ => return Err(format!("`{}`: items cannot hold `{}` at `{}`; use a nested list instead of a fixed array of structs", name, self.show(t), prefix)),
        }
        Ok(())
    }
}

fn contains_coll(p: &Program, t: &Ty) -> bool {
    match t { Ty::List(..) | Ty::Pool(..) => true, Ty::Arr(e, _) => contains_coll(p, e), Ty::Struct(i) => p.structs[*i].fields.iter().any(|f| contains_coll(p, &f.1)), _ => false }
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

/// Tokens, and the line and column (1-based) each one starts at.
fn tokenize(src: &str) -> (Vec<String>, Vec<(u32, u32)>) {
    let c: Vec<char> = src.chars().collect();
    let (mut i, mut out, mut pos, mut line, mut line_start) = (0, Vec::new(), Vec::new(), 1u32, 0usize);
    while i < c.len() {
        let at = (line, (i - line_start + 1) as u32);
        if c[i] == '\n' { line += 1; line_start = i + 1; i += 1; }
        else if c[i].is_whitespace() { i += 1; }
        else if c[i] == '/' && c.get(i + 1) == Some(&'/') { while i < c.len() && c[i] != '\n' { i += 1; } }
        else if c[i].is_alphanumeric() || c[i] == '_' { let s = i; while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') { i += 1; } out.push(c[s..i].iter().collect()); pos.push(at); }
        else { out.push(c[i].to_string()); pos.push(at); i += 1; }
    }
    (out, pos)
}
/// Where each struct (`Name`), field (`Name.field`) and the state (`name`) is declared, for errors found after reading.
fn declared_places(p: &P) -> Vec<(String, (u32, u32))> {
    let (mut out, mut i, mut current) = (Vec::new(), 0, String::new());
    while i < p.t.len() {
        match p.t[i].as_str() {
            "struct" if i + 1 < p.t.len() => { current = p.t[i + 1].clone(); out.push((current.clone(), p.pos[i + 1])); i += 2; }
            "state" if i + 1 < p.t.len() => { out.push((p.t[i + 1].clone(), p.pos[i + 1])); i += 2; }
            "}" => { current.clear(); i += 1; }
            x if !current.is_empty() && p.t.get(i + 1).map(|s| s.as_str()) == Some(":") && (p.t[i - 1] == "{" || p.t[i - 1] == ";") => {
                out.push((format!("{}.{}", current, x), p.pos[i])); out.push((x.to_string(), p.pos[i]));
                if let Some(ty) = p.t.get(i + 2) { out.push((ty.clone(), p.pos[i + 2])); } // the type it names (an unknown one is reported there)
                i += 2;
            }
            _ => i += 1,
        }
    }
    out
}

struct P { t: Vec<String>, i: usize, pos: Vec<(u32, u32)> }
impl P {
    fn peek(&self) -> Option<&str> { self.t.get(self.i).map(|s| s.as_str()) }
    fn eat(&mut self, s: &str) -> Result<(), String> { if self.peek() == Some(s) { self.i += 1; Ok(()) } else { Err(format!("expected `{}`, found `{}`", s, self.peek().unwrap_or("end of file"))) } }
    fn ident(&mut self) -> Result<String, String> { let s = self.peek().ok_or("unexpected end of file")?.to_string(); self.i += 1; Ok(s) }
    fn num(&mut self) -> Result<usize, String> { let s = self.ident()?; s.parse().map_err(|_| format!("expected a number, found `{}`", s)) }
    fn word(&mut self, w: &str) -> bool { if self.peek() == Some(w) { self.i += 1; true } else { false } }
    /// An integer, with an optional minus sign.
    fn signed(&mut self) -> Result<i64, String> {
        let neg = self.word("-");
        let s = self.ident()?;
        let v: i128 = s.parse().map_err(|_| format!("expected a number, found `{}`", s))?;
        let v = if neg { -v } else { v };
        i64::try_from(v).map_err(|_| format!("`{}` does not fit in an i64", v))
    }
    /// `a.b.c`
    fn path(&mut self) -> Result<String, String> { let mut s = self.ident()?; while self.word(".") { s.push('.'); s += &self.ident()?; } Ok(s) }
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
        else if n == "ref" { return Err("`ref<...>` was removed: keep the item's slot (`slot(item)`) in an integer field and read it back with `pool[slot]` (inside `or(...)` for a fallback when it is gone)".into()); }
        else if n == "list" { self.eat("<")?; let e = self.ty()?; self.eat(",")?; let k = self.count()?; self.eat(">")?; let f = if self.word("drop_oldest") { Full::DropOldest } else { Full::Fail }; Ty::List(Box::new(e), k, f) }
        else if n == "pool" { self.eat("<")?; let e = self.ty()?; self.eat(",")?; let k = self.count()?; self.eat(">")?; let c = self.word("checked"); Ty::Pool(Box::new(e), k, c) }
        else { Ty::Named(n) };
        while self.peek() == Some("[") { self.i += 1; let k = self.count()?; self.eat("]")?; t = Ty::Arr(Box::new(t), k); }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_rejects_what_it_cannot_bound() {
        assert!(Program::parse("struct A { b: B; } struct B { a: A; } state g: A;").is_err());
        assert!(Program::parse("struct N { kids: list<N, 2>; } state g: pool<N, 2>;").is_err()); // nests forever
        assert!(Program::parse("struct M { t: list<i32, 2>; } state g: list<M, 2> drop_oldest;").is_err());
        assert!(Program::parse("struct M { who: ref<nope>; } state g: list<M, 2>;").err().unwrap_or_default().contains("was removed"));
        let p = Program::parse("struct M { t: i32; } struct R { m: list<M, 1000>; } state rooms: pool<R, 64>;").unwrap();
        assert_eq!(p.colls[1].mult, 64);
        // one state: none, two, or the old `global`, do not load
        assert!(Program::parse("struct M { t: i32; }").err().unwrap_or_default().contains("has none"));
        assert!(Program::parse("struct M { t: i32; } state a: M; state b: M;").err().unwrap_or_default().contains("one state"));
        assert!(Program::parse("struct M { t: i32; } global a: M;").err().unwrap_or_default().contains("replaced by `state`"));
    }
}
