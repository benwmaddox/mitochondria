//! Parse state declarations and compute their exact byte layout.
//!
//! Plain values (scalars, strings, fixed arrays, structs outside any collection) are laid out record-style at fixed
//! offsets, as in Stasis. Every `list` and `pool` is laid out as columns: one array per leaf field, holding `mult * cap`
//! items, where `mult` is how many instances of the collection exist (the product of the capacities of the collections it
//! is nested in). Instance `i` owns items `i*cap .. (i+1)*cap` of every column, so one parent's items are adjacent.
use std::collections::HashMap;

pub const PAGE: usize = 4096;
/// Bytes of scratch area for struct values (locals) in every request region; a struct value is at most `MAX_VALUE` bytes.
pub const SCRATCH_BYTES: usize = 4 * PAGE;
pub const MAX_VALUE: usize = 256;

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
    Table(Box<Ty>, Vec<Vec<String>>, Vec<Vec<String>>, bool), // table<T> [index(f, ...)]... [unique(f, ...)]... [extern]: rows in SQLite, no capacity
    Struct(usize),
    Named(String),              // only before resolution
}
impl Ty { pub fn is_coll(&self) -> bool { matches!(self, Ty::List(..) | Ty::Pool(..)) } }

/// (For a text field, `ranges[k]` is the range of its bytes: Some((0, 127)) for `ascii<N>`.)
/// `ranges[k]`: field k's declared range (`limit: i64 in 0..3000000`, both ends included), when it has one: every write is checked against it,
/// and reads give the range analysis that interval.
/// `unlogged[k]`: field k is kept out of the undo log: a failed request leaves what it wrote there. This is the default; `logged` (`count: i64 logged;`) puts a field in
/// the undo log (`false` here), and on a struct-typed field or on the state it covers everything inside.
/// `ids[k]`: field k holds a stored id (`best: Player?`, or `Player in game.players?`): the struct and the collection named, if any.
/// `cols[k]`: how field k is kept in a table's column, beyond its type: a `time` (an i64 of microseconds since 1970 in the program, text in
/// the column), and nullable (`utf8<N>?`: the column may be NULL, read as null; table rows only).
pub struct StructDef { pub name: String, pub fields: Vec<(String, Ty)>, pub ranges: Vec<Option<(i64, i64)>>, pub ids: Vec<Option<(String, Option<String>)>>, pub unlogged: Vec<bool>, pub cols: Vec<ColOpt>, pub offs: Vec<usize>, pub size: usize, pub align: usize, state: u8 }
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColOpt { pub time: bool, pub null: bool }

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

pub struct Global { pub name: String, pub ty: Ty, pub off: usize, pub coll: Option<usize>, pub logged: bool }

/// A table's column: an item field (a plain struct inside the item flattened to `inner.x`), stored as SQLite's INTEGER (integers, bools,
/// enums, stored ids), REAL, TEXT (with its maximum bytes, and whether it is ascii) or BLOB (a fixed array of `n` bytes).
#[derive(Clone, Debug, PartialEq)]
pub enum TKind { Int(Scalar), Real(Scalar), Text(usize, bool), Blob(Scalar, usize), Ref(usize), Time }
/// A table is collection TABLE_BASE + its index where collections are numbered together (a stored id naming a row; the code generator).
pub const TABLE_BASE: usize = 1 << 24;
#[derive(Clone, Debug)]
/// `refers`: a `T?` field (the struct named, and `in path` if given), resolved to `TKind::Ref` once every table is known.
/// `sql`: the column's name in SQLite (the path; a reference's is `<path>_id`); `null`: it may be NULL (a `T?` reference, a `utf8<N>?`).
pub struct TCol { pub path: String, pub sql: String, pub kind: TKind, pub range: Option<(i64, i64)>, pub refers: Option<(String, Option<String>)>, pub null: bool }
/// A table: its path from the state (`app.boards`, `app.boards.todos`), its name in SQLite (the path after the state's name, quoted:
/// `"boards"`, `"boards.todos"`), the struct of its rows, the table whose rows own its rows (a `parent_id` column names one), its
/// columns, the nested tables of its rows, and its indexes.
#[derive(Clone, Debug)]
/// `ext`: the table belongs to another program's database (`table<T> extern`): checked at start, never created or changed in shape.
pub struct Table { pub name: String, pub sql: String, pub item: usize, pub parent: Option<usize>, pub cols: Vec<TCol>, pub children: Vec<(String, usize)>, pub index: Vec<Vec<String>>, pub unique: Vec<Vec<String>>, pub ext: bool }

/// A module instance (`global scores: sorted<Score, 100, points>;`, expanded by `modules::expand`): its state is the
/// plain global `name`; `key` is the field of its items it keeps ordered, if any.
#[derive(Clone, Debug)]
pub struct ModInst { pub name: String, pub module: String, pub key: Option<String> }

/// `enum Op { Plus, Minus }` or `enum Cmd: u8 { Plus = 43, Minus = 45 }`: named integer values, each the one before plus one
/// unless given; stored in the smallest integer that holds them (or the declared one).
#[derive(Clone, Debug)]
pub struct EnumDef { pub name: String, pub base: Scalar, pub values: Vec<(String, i64)> }

/// A compile-time constant's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CVal { Int(i64), Flt(f64) }
/// `const Limits { MAX_HP: i64 = 100; MAX_NAME = 16; }`: a named group of constants, used as `Limits.MAX_HP`.
#[derive(Clone, Debug)]
pub struct ConstGroup { pub name: String, pub items: Vec<(String, CVal)> }

/// Where a stored id lives: an item's column (collection, column) or a plain value (its offset).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IdAt { Col(usize, usize), Plain(usize) }

pub struct Program {
    /// The stored-id fields and the pool each names (a top-level `checked` pool).
    pub ids: Vec<(IdAt, usize)>,
    pub enums: Vec<EnumDef>,
    pub consts: Vec<ConstGroup>,
    /// `request work: Work;`: the per-request struct (offset, bytes on whole 64-byte lines): zeroed before each request, kept out of
    /// the undo log, never kept between requests.
    pub request: Option<(usize, usize)>,
    /// where struct values live (offset on a page after everything else, bytes): the request's own memory like the request struct (zeroed by nobody, kept out of the undo log, private to a
    /// reader), but only part of the program when the code uses struct values: `cgen::generate` says so (`Generated::scratch`) and then `total` and the table of ranges include it
    pub scratch: (usize, usize),
    /// byte ranges kept out of the undo log (unlogged plain fields, unlogged collections), each on whole 64-byte lines
    pub unlogged: Vec<(usize, usize)>,
    pub instances: Vec<ModInst>,
    pub structs: Vec<StructDef>,
    pub globals: Vec<Global>,
    pub colls: Vec<Coll>,
    pub plain_colls: Vec<(usize, usize, String, usize)>, // (offset of a plain struct, its struct id, field name, collection)
    /// The tables (`table<T>`): kept in SQLite, outside the fixed reservation; a nested table's rows each name a row of its parent.
    pub tables: Vec<Table>,
    pub plain_tables: Vec<(usize, usize, String, usize)>, // (offset of a plain struct, its struct id, field name, table)
    pub plain_end: usize,
    pub total: usize,
}

pub fn up(n: usize, to: usize) -> usize { (n + to - 1) / to * to }

impl Program {
    /// The enums as the binder takes them.
    /// The constants as the binder takes them.
    pub fn const_table(&self) -> Vec<(String, Vec<(String, CVal)>)> { self.consts.iter().map(|g| (g.name.clone(), g.items.clone())).collect() }
    pub fn enum_table(&self) -> Vec<(String, Vec<(String, i64)>)> { self.enums.iter().map(|e| (e.name.clone(), e.values.clone())).collect() }
    /// Size and alignment of a plain (record-style) value. Collections inside a plain struct take no bytes there.
    pub fn plain_size(&self, t: &Ty) -> (usize, usize) {
        match t {
            Ty::Scalar(s) => (s.size(), s.size()),
            Ty::Str(n) => (up(4 + n, 4), 4),
            Ty::Arr(e, n) => { let (s, a) = self.plain_size(e); (s * n, a) }
            Ty::Struct(i) => (self.structs[*i].size, self.structs[*i].align),
            Ty::List(..) | Ty::Pool(..) | Ty::Table(..) => (0, 1),
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
            Ty::Table(e, ix, un, x) => format!("table<{}>{}{}{}", self.show(e), ix.iter().map(|f| format!(" index({})", f.join(", "))).collect::<String>(), un.iter().map(|f| format!(" unique({})", f.join(", "))).collect::<String>(), if *x { " extern" } else { "" }),
            Ty::Struct(i) => self.structs[*i].name.clone(),
            Ty::Named(n) => n.clone(),
        }
    }
    /// Can struct `sid` be a value (a local, a copy)? At most `MAX_VALUE` bytes; no collection, no stored id inside (a copy must be a bounded plain copy that means the same anywhere).
    pub fn value_check(&self, sid: usize) -> Result<(), String> {
        fn walk(p: &Program, sid: usize, path: &str) -> Result<(), String> {
            let sd = &p.structs[sid];
            for (k, (f, t)) in sd.fields.iter().enumerate() {
                let here = if path.is_empty() { f.clone() } else { format!("{}.{}", path, f) };
                if sd.ids[k].is_some() { return Err(format!("`{}` holds a stored id (`{}`): a struct with an id inside is part of the state; it cannot be a value", p.structs[sid].name, here)); }
                if contains_coll(p, t) { return Err(format!("`{}` contains a collection (`{}`): a struct with a pool, list or table inside is part of the state; it cannot be a value", p.structs[sid].name, here)); }
                if let Ty::Struct(s2) = t { walk(p, *s2, &here)?; }
            }
            Ok(())
        }
        let sd = &self.structs[sid];
        if sd.size > MAX_VALUE {
            let n = sd.size.to_string(); let mut digits = String::new();
            for (i, c) in n.chars().enumerate() { if i > 0 && (n.len() - i) % 3 == 0 { digits.push(','); } digits.push(c); }
            return Err(format!("`{}` is {} bytes: a struct used as a value can be at most {} bytes (use it in place in the state, or split it)", sd.name, digits, MAX_VALUE));
        }
        walk(self, sid, "")
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
        let mut p = P { t, i: 0, pos, consts: Vec::new() };
        let places = declared_places(&p);
        Self::parse_tokens(&mut p).map_err(|e| crate::generics::name_instances(&e)).map_err(|e| {
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
        let mut globals: Vec<(String, Ty, bool)> = Vec::new();
        let mut instances: Vec<ModInst> = Vec::new();
        let mut enums: Vec<EnumDef> = Vec::new();
        let mut request: Option<(String, Ty)> = None;
        p.read_consts()?;
        while p.i < p.t.len() {
            match p.ident()?.as_str() {
                // read before everything else (so a size or range can use them): skipped here
                "const" => { p.ident()?; p.skip_block()?; }
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
                    let mut ids = Vec::new();
                    let mut cols = Vec::new();
                    while p.peek() != Some("}") {
                        let f = p.ident()?; p.eat(":")?;
                        let ascii = p.peek() == Some("ascii");
                        // `time`: an i64 of microseconds since 1970 (UTC); in a table's row, text in the column (`2026-03-02 16:00:00.123456`)
                        let time = p.peek() == Some("time");
                        let ty = if time { p.i += 1; Ty::Scalar(Scalar::I64) } else { p.ty()? };
                        // `utf8<N>?`, `time?`: a nullable text or time column (table rows)
                        let null = (matches!(ty, Ty::Str(_)) || time) && p.peek() == Some("?");
                        if null { p.i += 1; }
                        // `T?` / `T in path?`: a stored id of an item, read back as a `T?` (8 bytes: slot + 1, then its generation)
                        let mut idref = None;
                        if let Ty::Named(n) = &ty {
                            if p.peek() == Some("?") { p.i += 1; idref = Some((n.clone(), None)); }
                            else if p.peek() == Some("in") && p.t.get(p.i + 1).map_or(false, |x| x.chars().next().map_or(false, |c| c.is_alphabetic())) {
                                p.i += 1; let path = p.path()?; p.eat("?")?; idref = Some((n.clone(), Some(path)));
                            }
                        }
                        let ty = if idref.is_some() { Ty::Scalar(Scalar::I64) } else { ty };
                        // `in lo..hi`: a declared range for an integer field, both ends included
                        let range = if p.word("in") { let lo = p.signed()?; p.eat(".")?; p.eat(".")?; let hi = p.signed()?; Some((lo, hi)) } else { None };
                        if let Some((lo, hi)) = range {
                            let Ty::Scalar(sc) = &ty else { return Err(format!("`{}.{}`: only an integer field can have a range", name, f)) };
                            let (tl, th): (i128, i128) = match sc { Scalar::I8 => (-128, 127), Scalar::I16 => (-32768, 32767), Scalar::I32 => (i32::MIN as i128, i32::MAX as i128), Scalar::I64 => (i64::MIN as i128, i64::MAX as i128),
                                Scalar::U8 => (0, 255), Scalar::U16 => (0, 65535), Scalar::U32 => (0, u32::MAX as i128), Scalar::U64 => (0, i64::MAX as i128), _ => return Err(format!("`{}.{}`: only an integer field can have a range", name, f)) };
                            if lo > hi || (lo as i128) < tl || (hi as i128) > th { return Err(format!("`{}.{}`: range {}..{} does not fit its type", name, f, lo, hi)); }
                            if lo > 0 || hi < 0 { return Err(format!("`{}.{}`: range {}..{} must include 0, the value a field starts with", name, f, lo, hi)); }
                        }
                        // `logged`: kept in the undo log, so a failed request restores it (a plain field, or a whole collection with what its items hold); the default is unlogged
                        if p.word("unlogged") { return Err(format!("`{}.{}`: state is unlogged by default now; write `logged` on what a failed request must restore", name, f)); }
                        let unl = !p.word("logged");
                        p.eat(";")?;
                        // an `ascii<N>` field: its bytes are 0..127 (the range of a text field is the range of its bytes)
                        fields.push((f, ty)); ranges.push(if ascii { Some((0, 127)) } else { range }); unlogged.push(unl); ids.push(idref); cols.push(ColOpt { time, null });
                    }
                    p.eat("}")?;
                    structs.push(StructDef { name, fields, ranges, ids, unlogged, cols, offs: Vec::new(), size: 0, align: 1, state: 0 });
                }
                "state" => {
                    let n = p.ident()?; p.eat(":")?; let ty = p.ty()?;
                    if p.word("unlogged") { return Err(format!("`state {}`: state is unlogged by default now; write `logged` on what a failed request must restore", n)); }
                    let logged = p.word("logged"); p.eat(";")?;
                    if let Some(g) = globals.first() { return Err(format!("a program has one state; `{}` is the second (the first is `{}`): put everything in one struct", n, g.0)); }
                    globals.push((n, ty, logged));
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
                o => return Err(format!("expected `struct`, `enum`, `const`, `state` or `request`, found `{}`", o)),
            }
        }
        for g in &p.consts {
            if enums.iter().any(|e| e.name == g.name) { return Err(format!("const group `{}` has the name of an enum", g.name)); }
            if structs.iter().any(|x| x.name == g.name) { return Err(format!("const group `{}` has the name of a struct", g.name)); }
            if globals.iter().any(|x| x.0 == g.name) { return Err(format!("const group `{}` has the name of the state", g.name)); }
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
        let mut prog = Program { ids: Vec::new(), enums, consts: std::mem::take(&mut p.consts), request: None, scratch: (0, 0), unlogged: Vec::new(), instances, structs, globals: Vec::new(), colls: Vec::new(), plain_colls: Vec::new(), tables: Vec::new(), plain_tables: Vec::new(), plain_end: 0, total: 0 };
        let names: HashMap<String, usize> = prog.structs.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();
        for i in 0..prog.structs.len() {
            let fs = std::mem::take(&mut prog.structs[i].fields);
            let fs = fs.into_iter().map(|(n, t)| Ok((n, resolve(t, &names)?))).collect::<Result<Vec<_>, String>>()?;
            prog.structs[i].fields = fs;
        }
        for i in 0..prog.structs.len() { prog.layout_struct(i)?; }
        // plain values first
        let mut off = 0;
        for (n, ty, logged) in globals {
            let ty = resolve(ty, &names)?;
            // a state holding nothing but collections takes no plain bytes (its inner structs still get distinct offsets)
            let (s, a) = if prog.has_plain(&ty) { prog.plain_size(&ty) } else { (0, 1) };
            off = up(off, a.max(8));
            prog.globals.push(Global { name: n, ty, off, coll: None, logged });
            off += s;
        }
        // the request struct: its own pages after the state's plain values
        if let Some((n, ty)) = request {
            let ty = resolve(ty, &names)?;
            if prog.globals.iter().any(|g| g.name == n) { return Err(format!("`{}` names both the state and the request struct", n)); }
            if contains_coll(&prog, &ty) { return Err(format!("request `{}`: the request struct holds plain fields (text, numbers, arrays, structs); keep collections in the state", n)); }
            // on whole pages of its own: a reader (runtime MITO(reader)) maps its own pages over it
            let (s, _) = prog.plain_size(&ty);
            off = up(off, PAGE);
            let size = up(s.max(1), PAGE);
            prog.globals.push(Global { name: n, ty, off, coll: None, logged: false });
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
                let cid = prog.build_coll(&name, &ty, 1, None, !prog.globals[g].logged, &mut cur, 0)?;
                prog.globals[g].coll = Some(cid);
            } else if let Ty::Struct(s) = ty {
                let base = prog.globals[g].off;
                let logged = prog.globals[g].logged;
                prog.plain_struct_colls(s, base, &name, &mut cur, logged)?;
            } else if contains_coll(&prog, &ty) {
                return Err(format!("`{}`: collections inside fixed arrays are not supported; use a nested list", name));
            }
        }
        // the bytes kept out of the undo log (the request struct whole)
        for g in 0..prog.globals.len() { if let Ty::Struct(st) = prog.globals[g].ty.clone() { let base = prog.globals[g].off; if prog.request.map_or(true, |r| r.0 != base) { let logged = prog.globals[g].logged; prog.plain_unlogged(st, base, logged); } } }
        for c in 0..prog.colls.len() { if prog.colls[c].unlogged && prog.colls[c].parent.is_none() { let (a, b) = (prog.colls[c].start, prog.colls[c].end); prog.unlogged.push((a, up(b, 64))); } }
        if let Some((a, n)) = prog.request { prog.unlogged.push((a, a + n)); }
        prog.total = cur;
        prog.scratch = (up(cur, PAGE), SCRATCH_BYTES);
        // a table's references, now that every table is known: each to a table (a pool's item is not kept, so a row cannot name one)
        for t in 0..prog.tables.len() {
            for ci in 0..prog.tables[t].cols.len() {
                let Some((st, path)) = prog.tables[t].cols[ci].refers.clone() else { continue };
                let owner = prog.structs[prog.tables[t].item].name.clone();
                let target = prog.id_target(&owner, &prog.tables[t].cols[ci].path.clone(), &st, &path)?;
                if target < TABLE_BASE { return Err(format!("`{}.{}` (`{}?`): a table's row names rows of a table; `{}` items are kept in {}, which is not stored", owner, prog.tables[t].cols[ci].path, st, st, prog.colls[target].name)); }
                prog.tables[t].cols[ci].kind = TKind::Ref(target - TABLE_BASE);
            }
        }
        // one database per program: the program's own (created and migrated), or another program's (every table `extern`), never both
        if let (Some(e), Some(o)) = (prog.tables.iter().find(|t| t.ext), prog.tables.iter().find(|t| !t.ext)) {
            return Err(format!("`{}` is another program's table (`extern`) and `{}` is the program's own: a program's tables are all in one database, its own or another's", e.name, o.name));
        }
        // a nullable field (`utf8<N>?`) is a column that may be NULL: only in a table's row
        let in_rows: Vec<usize> = prog.tables.iter().map(|t| t.item).collect();
        for (i, sd) in prog.structs.iter().enumerate() {
            if let Some(k) = sd.cols.iter().position(|c| c.null) {
                if !in_rows.contains(&i) { return Err(format!("`{}.{}`: a nullable field (`utf8<N>?`, `time?`) is a column that may be NULL, so it belongs in a table's rows; elsewhere use the empty text", sd.name, sd.fields[k].0)); }
            }
        }
        prog.resolve_ids()?;
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
            // a logged field has its own 64-byte lines: the undo log saves and restores whole lines, so no unlogged byte may share one
            let (s, a) = if !unlogged[k] { (up(s, 64), a.max(64)) } else { (s, a) };
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
    /// The pool a stored id names: the one top-level `checked` pool of its struct (or the collection written).
    fn id_target(&self, owner: &str, field: &str, st: &str, path: &Option<String>) -> Result<usize, String> {
        let Some(sid) = self.structs.iter().position(|s| s.name == st) else { return Err(format!("`{}.{}`: `{}` is not a struct", owner, field, st)) };
        // a table holding `st`: its rows' ids (never reused, so no generation is needed)
        let tabs: Vec<usize> = (0..self.tables.len()).filter(|&t| self.tables[t].item == sid && path.as_ref().map_or(true, |p| self.tables[t].name == *p)).collect();
        let pools = (0..self.colls.len()).filter(|&c| { let k = &self.colls[c]; k.item == Some(sid) && path.as_ref().map_or(true, |p| k.name == *p) }).count();
        match (tabs.as_slice(), pools) {
            ([t], 0) => return Ok(TABLE_BASE + *t),
            ([_, ..], _) => return Err(format!("`{}.{}` (`{}?`): several collections hold `{}` items: write `{} in path?`", owner, field, st, st, st)),
            _ => {}
        }
        let found: Vec<usize> = (0..self.colls.len()).filter(|&c| { let k = &self.colls[c]; k.item == Some(sid) && path.as_ref().map_or(true, |p| k.name == *p) }).collect();
        let c = match found.as_slice() {
            [c] => *c,
            [] => return Err(format!("`{}.{}` (`{}?`): no collection holds `{}` items{}", owner, field, st, st, path.as_ref().map_or(String::new(), |p| format!(" at `{}`", p)))),
            _ => return Err(format!("`{}.{}` (`{}?`): several collections hold `{}` items: write `{} in path?` ({})", owner, field, st, st, st, found.iter().map(|&c| self.colls[c].name.clone()).collect::<Vec<_>>().join(", "))),
        };
        let k = &self.colls[c];
        if k.parent.is_some() { return Err(format!("`{}.{}` (`{}?`): an id of an item of a nested collection ({}) cannot be stored yet", owner, field, st, k.name)); }
        if !k.checked() { return Err(format!("`{}.{}` (`{}?`): a stored id needs generations to tell a reused slot from its item: declare `{}` `checked`", owner, field, st, k.name)); }
        Ok(c)
    }
    /// Every stored-id field, in items' columns and in plain values, with the pool it names.
    fn resolve_ids(&mut self) -> Result<(), String> {
        let mut out = Vec::new();
        // items' columns: a field of the item struct, or of a plain struct inside it (`a.best`)
        fn paths(p: &Program, s: usize, prefix: &str, out: &mut Vec<(String, String, String, Option<String>)>) {
            for (k, (f, t)) in p.structs[s].fields.iter().enumerate() {
                let path = if prefix.is_empty() { f.clone() } else { format!("{}.{}", prefix, f) };
                if let Some((st, c)) = &p.structs[s].ids[k] { out.push((path.clone(), p.structs[s].name.clone(), st.clone(), c.clone())); }
                if let Ty::Struct(i) = t { paths(p, *i, &path, out); }
            }
        }
        for c in 0..self.colls.len() {
            let Some(s) = self.colls[c].item else { continue };
            let mut ps = Vec::new();
            paths(self, s, "", &mut ps);
            for (path, owner, st, cp) in ps {
                let target = self.id_target(&owner, &path, &st, &cp)?;
                if let Some(ci) = self.colls[c].cols.iter().position(|x| x.path == path) { out.push((IdAt::Col(c, ci), target)); }
            }
        }
        // plain values
        fn plain(p: &Program, s: usize, base: usize, out: &mut Vec<(usize, String, String, String, Option<String>)>) {
            for (k, (f, t)) in p.structs[s].fields.iter().enumerate() {
                let o = base + p.structs[s].offs[k];
                if let Some((st, c)) = &p.structs[s].ids[k] { out.push((o, p.structs[s].name.clone(), f.clone(), st.clone(), c.clone())); }
                if let Ty::Struct(i) = t { plain(p, *i, o, out); }
            }
        }
        let mut ps = Vec::new();
        for g in &self.globals { if let Ty::Struct(s) = g.ty { plain(self, s, g.off, &mut ps); } }
        for (o, owner, f, st, cp) in ps { let target = self.id_target(&owner, &f, &st, &cp)?; out.push((IdAt::Plain(o), target)); }
        self.ids = out;
        Ok(())
    }
    /// The pool a stored id at this place names, if the place holds one.
    pub fn id_at(&self, at: IdAt) -> Option<usize> { self.ids.iter().find(|x| x.0 == at).map(|x| x.1) }
    fn plain_unlogged(&mut self, s: usize, base: usize, all_logged: bool) {
        let fields = self.structs[s].fields.clone();
        for (k, (_, t)) in fields.iter().enumerate() {
            if t.is_coll() { continue; }
            let o = base + self.structs[s].offs[k];
            let logged = all_logged || !self.structs[s].unlogged[k];
            if let Ty::Struct(c) = t { self.plain_unlogged(*c, o, logged); continue; }
            if !logged { let (sz, _) = self.plain_size(t); self.unlogged.push((o, o + sz)); } // the bytes alone: a logged field starts on a line of its own
        }
    }
    /// `all_logged`: the struct is inside a `logged` field (or the state is `logged`), so its collections are logged too.
    fn plain_struct_colls(&mut self, s: usize, base: usize, path: &str, cur: &mut usize, all_logged: bool) -> Result<(), String> {
        let fields = self.structs[s].fields.clone();
        for (k, (n, t)) in fields.iter().enumerate() {
            let p = format!("{}.{}", path, n);
            let logged = all_logged || !self.structs[s].unlogged[k];
            if let Ty::Table(..) = t {
                // a table is kept in its database, which keeps it atomic whatever the field says
                let tid = self.build_table(&p, t, None, 0)?;
                self.plain_tables.push((base, s, n.clone(), tid));
            } else if t.is_coll() {
                let cid = self.build_coll(&p, t, 1, None, !logged, cur, 0)?;
                self.plain_colls.push((base, s, n.clone(), cid));
            } else if let Ty::Struct(c) = t {
                let o = base + self.structs[s].offs[k];
                self.plain_struct_colls(*c, o, &p, cur, logged)?;
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
                Ty::Str(m) => { let mut a = place(CKind::StrLen, &mut o); a.range = range; c.cols.push(a); let b = place(CKind::StrBytes(m), &mut o); c.cols.push(b); }
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
                if !self.structs[*i].unlogged[k] { return Err(format!("`{}`: `{}` is declared `logged`, but items of a collection are logged or not as a whole: declare the collection `logged`", name, join(f))); }
                self.flatten(ft, &join(f), self.structs[*i].ranges[k], leaves, kids, name)?;
            },
            Ty::Scalar(_) | Ty::Str(_) => leaves.push((prefix.to_string(), t.clone(), range)),
            Ty::Arr(e, _) if matches!(**e, Ty::Scalar(_)) => leaves.push((prefix.to_string(), t.clone(), None)),
            // a fixed array of structs of numbers and bools: one array column per field (`vals.a`, `vals.b`), read and written as `item.vals[i].a`
            Ty::Arr(e, k) if matches!(**e, Ty::Struct(_)) => {
                let Ty::Struct(i) = **e else { unreachable!() };
                for (fk, (f, ft)) in self.structs[i].fields.iter().enumerate() {
                    if !self.structs[i].unlogged[fk] { return Err(format!("`{}`: `{}` is declared `logged`, but items of a collection are logged or not as a whole: declare the collection `logged`", name, join(f))); }
                    if !matches!(ft, Ty::Scalar(_)) { return Err(format!("`{}`: the structs in the array at `{}` hold numbers and bools only (`{}` is `{}`)", name, prefix, f, self.show(ft))); }
                    leaves.push((join(f), Ty::Arr(Box::new(ft.clone()), *k), None));
                }
            }
            Ty::List(..) | Ty::Pool(..) => kids.push((prefix.to_string(), t.clone())),
            Ty::Table(..) => return Err(format!("`{}`: a table inside an item of a pool or list is not supported (its rows would belong to memory that is not kept); put the table in the state or in another table's rows", name)),
            _ => return Err(format!("`{}`: items cannot hold `{}` at `{}`; use a nested list instead of a fixed array of structs", name, self.show(t), prefix)),
        }
        Ok(())
    }
}

impl Program {
    /// A table and the tables nested in its rows. Rows hold plain fields (numbers, bools, text, enums, fixed arrays, plain structs
    /// flattened) and nested tables; a pool or list there would be memory per row, so it is refused.
    fn build_table(&mut self, path: &str, ty: &Ty, parent: Option<usize>, depth: usize) -> Result<usize, String> {
        if depth > 8 { return Err(format!("`{}`: tables nested more than 8 deep (a struct that contains itself?)", path)); }
        let Ty::Table(elem, index, unique, ext) = ty else { unreachable!() };
        let ext = *ext;
        let Ty::Struct(item) = **elem else { return Err(format!("`{}`: a table's rows are a struct (`table<Todo>`), not `{}`", path, self.show(elem))) };
        let sql = format!("\"{}\"", path.split_once('.').map_or(path, |x| x.1));
        let t = self.tables.len();
        if parent.is_some() && (ext || self.tables[parent.unwrap_or(0)].ext) { return Err(format!("`{}`: a table of another program's database is a table of its own, not nested in another's rows (refer to the owner with a `T?` field)", path)); }
        self.tables.push(Table { name: path.to_string(), sql, item, parent, cols: Vec::new(), children: Vec::new(), index: index.clone(), unique: unique.clone(), ext });
        let (mut leaves, mut kids) = (Vec::new(), Vec::new());
        self.table_fields(item, "", &mut leaves, &mut kids, path)?;
        for (p, kind, range, refers, null) in leaves {
            if p == "id" || p == "parent_id" { return Err(format!("`{}`: a row's `{}` is its table's own column (the row id{}); name the field otherwise", path, p, if p == "id" { "" } else { " of the row that owns it" })); }
            // a reference is kept as `<field>_id`, as Rails and most schemas name it
            let sql = if refers.is_some() { format!("{}_id", p) } else { p.clone() };
            if self.tables[t].cols.iter().any(|c| c.sql == sql) { return Err(format!("`{}`: two fields are kept in the column `{}`", path, sql)); }
            self.tables[t].cols.push(TCol { path: p, sql, kind, range, refers, null });
        }
        for fs in index.iter().chain(unique.iter()) {
            for f in fs { if !self.tables[t].cols.iter().any(|c| c.path == *f) { return Err(format!("`{}`: an index names `{}`, which is not a column of its rows", path, f)); } }
        }
        for (f, kt) in kids {
            let k = self.build_table(&format!("{}.{}", path, f), &kt, Some(t), depth + 1)?;
            self.tables[t].children.push((f, k));
        }
        Ok(t)
    }
    fn table_fields(&self, s: usize, prefix: &str, leaves: &mut Vec<(String, TKind, Option<(i64, i64)>, Option<(String, Option<String>)>, bool)>, kids: &mut Vec<(String, Ty)>, name: &str) -> Result<(), String> {
        for (k, (f, t)) in self.structs[s].fields.iter().enumerate() {
            let p = if prefix.is_empty() { f.clone() } else { format!("{}.{}", prefix, f) };
            let range = self.structs[s].ranges[k];
            // `owner: User?`: a reference to a row of the table that holds `User` (resolved once every table is known)
            if let Some(r) = &self.structs[s].ids[k] { leaves.push((p, TKind::Ref(usize::MAX), None, Some(r.clone()), true)); continue; }
            let opt = self.structs[s].cols[k];
            match t {
                Ty::Scalar(_) if opt.time => leaves.push((p, TKind::Time, None, None, opt.null)),
                Ty::Scalar(sc) if matches!(sc, Scalar::F32 | Scalar::F64) => leaves.push((p, TKind::Real(*sc), None, None, false)),
                Ty::Scalar(sc) => leaves.push((p, TKind::Int(*sc), range, None, false)),
                Ty::Str(m) => leaves.push((p, TKind::Text(*m, range == Some((0, 127))), None, None, opt.null)),
                Ty::Arr(e, n) => match **e { Ty::Scalar(sc) => leaves.push((p, TKind::Blob(sc, *n), None, None, false)), _ => return Err(format!("`{}`: a row holds a fixed array of numbers at `{}`, not of `{}`; use a nested table", name, p, self.show(e))) },
                Ty::Struct(i) => { if prefix.matches('.').count() > 6 { return Err(format!("`{}`: plain structs nested too deep at `{}`", name, p)); } self.table_fields(*i, &p, leaves, kids, name)?; }
                Ty::Table(..) => kids.push((p, t.clone())),
                Ty::List(..) | Ty::Pool(..) => return Err(format!("`{}`: a row holds plain fields and nested tables; `{}` is a {} (memory for every row): make it a `table<...>`", name, p, if matches!(t, Ty::List(..)) { "list" } else { "pool" })),
                Ty::Named(_) => unreachable!(),
            }
        }
        Ok(())
    }
    /// The SQL that creates the tables (if absent) and their indexes: STRICT tables, so SQLite checks each value's type; a row id that is
    /// never reused (AUTOINCREMENT); a nested table's rows deleted with the row that owns them.
    pub fn schema_sql(&self) -> String { (0..self.tables.len()).map(|t| format!("{}\n{}", self.table_create_sql(t), self.table_index_sql(t))).collect() }
    /// One table's CREATE TABLE.
    pub fn table_create_sql(&self, t: usize) -> String {
        let tb = &self.tables[t];
        let mut o = format!("CREATE TABLE IF NOT EXISTS {} (id INTEGER PRIMARY KEY AUTOINCREMENT", tb.sql);
        if let Some(p) = tb.parent { o += &format!(", parent_id INTEGER NOT NULL REFERENCES {}(id) ON DELETE CASCADE", self.tables[p].sql); }
        for c in &tb.cols { o += &format!(", {} {}", sql_ident(&c.sql), self.col_decl(c)); }
        o + ") STRICT;"
    }
    /// A column's declaration after its name: its type and first value; a reference may be null, and becomes null when its row goes.
    pub fn col_decl(&self, c: &TCol) -> String {
        match c.kind { TKind::Ref(t) => format!("INTEGER REFERENCES {}(id) ON DELETE SET NULL", self.tables[t].sql), _ if c.null => format!("{} DEFAULT NULL", c.kind.sql().0), _ => { let (ty, def) = c.kind.sql(); format!("{} NOT NULL DEFAULT {}", ty, def) } }
    }
    /// One table's indexes (one statement per line): by owner for a nested table, then the declared `index(...)` and `unique(...)`.
    pub fn table_index_sql(&self, t: usize) -> String {
        let tb = &self.tables[t];
        let base = tb.sql.trim_matches('"');
        let cols = |fs: &Vec<String>| fs.iter().map(|f| sql_ident(&tb.cols.iter().find(|c| c.path == *f).map_or(f.clone(), |c| c.sql.clone()))).collect::<Vec<_>>().join(", ");
        let mut o = String::new();
        if tb.parent.is_some() { o += &format!("CREATE INDEX IF NOT EXISTS {} ON {}(parent_id, id);\n", sql_ident(&format!("{} by parent", base)), tb.sql); }
        for (k, fs) in tb.index.iter().enumerate() { o += &format!("CREATE INDEX IF NOT EXISTS {} ON {}({});\n", sql_ident(&format!("{} index {}", base, k)), tb.sql, cols(fs)); }
        for (k, fs) in tb.unique.iter().enumerate() { o += &format!("CREATE UNIQUE INDEX IF NOT EXISTS {} ON {}({});\n", sql_ident(&format!("{} unique {}", base, k)), tb.sql, cols(fs)); }
        o
    }
}

/// A name quoted for SQL ("a.b", a quote doubled).
pub fn sql_ident(s: &str) -> String { format!("\"{}\"", s.replace('"', "\"\"")) }
impl TKind {
    /// SQLite's type for the column and the value a new row (or a column added later) starts with.
    pub fn sql(&self) -> (&'static str, &'static str) { match self { TKind::Int(_) | TKind::Ref(_) => ("INTEGER", "0"), TKind::Real(_) => ("REAL", "0.0"), TKind::Text(..) => ("TEXT", "''"), TKind::Blob(..) => ("BLOB", "x''"), TKind::Time => ("TEXT", "'1970-01-01 00:00:00'") } }
    /// The runtime's type code (0 INTEGER, 1 REAL, 2 TEXT, 3 BLOB, 4 a time: TEXT in SQLite, microseconds in the program).
    pub fn code(&self) -> u32 { match self { TKind::Int(_) | TKind::Ref(_) => 0, TKind::Real(_) => 1, TKind::Text(..) => 2, TKind::Blob(..) => 3, TKind::Time => 4 } }
}

fn contains_coll(p: &Program, t: &Ty) -> bool {
    match t { Ty::List(..) | Ty::Pool(..) | Ty::Table(..) => true, Ty::Arr(e, _) => contains_coll(p, e), Ty::Struct(i) => p.structs[*i].fields.iter().any(|f| contains_coll(p, &f.1)), _ => false }
}

fn resolve(t: Ty, names: &HashMap<String, usize>) -> Result<Ty, String> {
    Ok(match t {
        Ty::Named(n) => Ty::Struct(*names.get(&n).ok_or(format!("unknown type `{}`", n))?),
        Ty::Arr(e, n) => Ty::Arr(Box::new(resolve(*e, names)?), n),
        Ty::List(e, n, f) => Ty::List(Box::new(resolve(*e, names)?), n, f),
        Ty::Pool(e, n, c) => Ty::Pool(Box::new(resolve(*e, names)?), n, c),
        Ty::Table(e, ix, un, x) => Ty::Table(Box::new(resolve(*e, names)?), ix, un, x),
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

struct P { t: Vec<String>, i: usize, pos: Vec<(u32, u32)>, consts: Vec<ConstGroup> }
impl P {
    fn peek(&self) -> Option<&str> { self.t.get(self.i).map(|s| s.as_str()) }
    fn eat(&mut self, s: &str) -> Result<(), String> { if self.peek() == Some(s) { self.i += 1; Ok(()) } else { Err(format!("expected `{}`, found `{}`", s, self.peek().unwrap_or("end of file"))) } }
    fn ident(&mut self) -> Result<String, String> { let s = self.peek().ok_or("unexpected end of file")?.to_string(); self.i += 1; Ok(s) }
    fn num(&mut self) -> Result<usize, String> { let s = self.ident()?; s.parse().map_err(|_| format!("expected a number, found `{}`", s)) }
    fn word(&mut self, w: &str) -> bool { if self.peek() == Some(w) { self.i += 1; true } else { false } }
    /// An integer, with an optional minus sign.
    fn signed(&mut self) -> Result<i64, String> {
        let neg = self.word("-");
        if self.is_const_path() {
            let (path, v) = self.const_path()?;
            let CVal::Int(v) = v else { return Err(format!("`{}` is a float: a range, size or enum value takes an integer", path)) };
            return if neg { v.checked_neg() } else { Some(v) }.ok_or(format!("`{}` does not fit in an i64", path));
        }
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
    fn atom(&mut self) -> Result<usize, String> {
        if self.peek() == Some("(") { self.i += 1; let v = self.count()?; self.eat(")")?; Ok(v) }
        else if self.is_const_path() {
            let (path, v) = self.const_path()?;
            match v { CVal::Int(v) if v >= 0 => Ok(v as usize), CVal::Int(_) => Err(format!("`{}` is negative: a size is not", path)), CVal::Flt(_) => Err(format!("`{}` is a float: a size takes an integer", path)) }
        } else { self.num() }
    }
    /// Is the next thing `Group.NAME`, a path into a const group?
    fn is_const_path(&self) -> bool {
        let word = |x: &String| x.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_');
        self.t.get(self.i).map_or(false, word) && self.t.get(self.i + 1).map(|s| s.as_str()) == Some(".") && self.t.get(self.i + 2).map_or(false, word)
    }
    /// `Group.NAME`: the path as written and its value.
    fn const_path(&mut self) -> Result<(String, CVal), String> {
        let g = self.ident()?; self.eat(".")?; let n = self.ident()?;
        let path = format!("{}.{}", g, n);
        let Some(grp) = self.consts.iter().find(|x| x.name == g) else { return Err(format!("`{}` is not a constant: there is no const group `{}`", path, g)) };
        match grp.items.iter().find(|x| x.0 == n) { Some(x) => Ok((path, x.1)), None => Err(format!("const group `{}` has no `{}`: it has {}", g, n, grp.items.iter().map(|x| format!("`{}`", x.0)).collect::<Vec<_>>().join(", "))) }
    }
    /// Skips a `{ ... }` block (nested braces included).
    fn skip_block(&mut self) -> Result<(), String> {
        self.eat("{")?;
        let mut d = 1;
        while d > 0 { match self.ident()?.as_str() { "{" => d += 1, "}" => d -= 1, _ => {} } }
        Ok(())
    }
    /// Reads every top-level `const Name { NAME [: type] = value; ... }` in file order (a value may use the constants before it).
    fn read_consts(&mut self) -> Result<(), String> {
        let (mut i, mut depth) = (0, 0);
        while i < self.t.len() {
            match self.t[i].as_str() {
                "{" => depth += 1,
                "}" => depth -= 1,
                "const" if depth == 0 && i + 2 < self.t.len() && self.t[i + 2] == "{" => {
                    self.i = i + 1;
                    let name = self.ident()?;
                    if self.consts.iter().any(|g| g.name == name) { return Err(format!("const group `{}` is declared twice", name)); }
                    self.eat("{")?;
                    let mut items: Vec<(String, CVal)> = Vec::new();
                    // the group is visible inside itself too: its own earlier names
                    self.consts.push(ConstGroup { name: name.clone(), items: Vec::new() });
                    let at = self.consts.len() - 1;
                    while self.peek() != Some("}") {
                        let n = self.ident()?;
                        if self.peek().is_none() { return Err(format!("const group `{}` is not closed", name)); }
                        if items.iter().any(|x| x.0 == n) { return Err(format!("const group `{}` has `{}` twice", name, n)); }
                        let ty = if self.word(":") { let t = self.ident()?; Some((t.clone(), Scalar::parse(&t).filter(|s| *s != Scalar::Bool).ok_or(format!("constant `{}.{}`: `{}` is not a number type (i8..i64, u8..u64, f32, f64)", name, n, t))?)) } else { None };
                        self.eat("=").map_err(|e| format!("constant `{}.{}`: {}", name, n, e))?;
                        let v = self.cexpr().map_err(|e| format!("constant `{}.{}`: {}", name, n, e))?;
                        self.eat(";").map_err(|e| format!("constant `{}.{}`: {}", name, n, e))?;
                        let v = match (ty, v) {
                            (None, CVal::Flt(_)) => return Err(format!("constant `{}.{}` is a float: give it a type, `{}: f64 = ...`", name, n, n)),
                            (None, v) => v,
                            (Some((t, s)), v) => match (s, v) {
                                (Scalar::F64, CVal::Int(v)) => CVal::Flt(v as f64),
                                (Scalar::F32, CVal::Int(v)) => CVal::Flt(v as f32 as f64),
                                (Scalar::F32, CVal::Flt(v)) => CVal::Flt(v as f32 as f64),
                                (Scalar::F64, v) => v,
                                (_, CVal::Flt(_)) => return Err(format!("constant `{}.{}` is a float but declared `{}`", name, n, t)),
                                (s, CVal::Int(v)) => {
                                    let (lo, hi): (i128, i128) = match s { Scalar::I8 => (-128, 127), Scalar::I16 => (-32768, 32767), Scalar::I32 => (i32::MIN as i128, i32::MAX as i128), Scalar::U8 => (0, 255), Scalar::U16 => (0, 65535), Scalar::U32 => (0, u32::MAX as i128), Scalar::U64 => (0, i64::MAX as i128), _ => (i64::MIN as i128, i64::MAX as i128) };
                                    if (v as i128) < lo || (v as i128) > hi { return Err(format!("constant `{}.{}` is {}, which does not fit {}", name, n, v, t)); }
                                    CVal::Int(v)
                                }
                            },
                        };
                        items.push((n.clone(), v));
                        self.consts[at].items = items.clone();
                    }
                    self.eat("}")?;
                    if items.is_empty() { return Err(format!("const group `{}` has no constants", name)); }
                    i = self.i;
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
        self.i = 0;
        Ok(())
    }
    /// A constant expression: numbers, `Group.NAME`, parentheses and `- | ^ & << >> + - * / %`, folded when read.
    fn cexpr(&mut self) -> Result<CVal, String> { self.clevel(0) }
    fn clevel(&mut self, lv: usize) -> Result<CVal, String> {
        const LV: [&[&str]; 6] = [&["|"], &["^"], &["&"], &["<", ">"], &["+", "-"], &["*", "/", "%"]];
        if lv == LV.len() { return self.cunary(); }
        let mut a = self.clevel(lv + 1)?;
        loop {
            let Some(op) = self.peek().map(|s| s.to_string()) else { return Ok(a) };
            // `<<` and `>>` arrive as two tokens
            let shift = lv == 3 && (op == "<" || op == ">") && self.t.get(self.i + 1) == Some(&op);
            if !LV[lv].contains(&op.as_str()) || (lv == 3 && !shift) { return Ok(a); }
            self.i += if shift { 2 } else { 1 };
            let b = self.clevel(lv + 1)?;
            let name = if shift { format!("{0}{0}", op) } else { op.clone() };
            a = cbin(&name, a, b)?;
        }
    }
    fn cunary(&mut self) -> Result<CVal, String> {
        if self.word("-") { return match self.cunary()? { CVal::Int(v) => v.checked_neg().map(CVal::Int).ok_or("overflow".to_string()), CVal::Flt(v) => Ok(CVal::Flt(-v)) }; }
        if self.word("(") { let v = self.cexpr()?; self.eat(")")?; return Ok(v); }
        if self.is_const_path() { return Ok(self.const_path()?.1); }
        let s = self.ident()?;
        if !s.chars().next().map_or(false, |c| c.is_ascii_digit()) { return Err(format!("`{}` is not a constant (a number or `Group.NAME` of an earlier constant)", s)); }
        // `9.8`: the digits, a `.` and digits, written together
        if self.peek() == Some(".") && self.t.get(self.i + 1).map_or(false, |x| x.chars().next().map_or(false, |c| c.is_ascii_digit())) {
            let (a, d, f) = (self.pos[self.i - 1], self.pos[self.i], self.pos[self.i + 1]);
            if a.0 == d.0 && a.1 + s.chars().count() as u32 == d.1 && d.0 == f.0 && d.1 + 1 == f.1 {
                self.i += 1; let frac = self.ident()?;
                return format!("{}.{}", s, frac).parse::<f64>().map(CVal::Flt).map_err(|_| format!("`{}.{}` is not a number", s, frac));
            }
        }
        let v = if let Some(h) = s.strip_prefix("0x") { i64::from_str_radix(&h.replace('_', ""), 16).ok() } else { s.replace('_', "").parse::<i64>().ok() };
        v.map(CVal::Int).ok_or(format!("`{}` is not an integer that fits an i64", s))
    }
    fn ty(&mut self) -> Result<Ty, String> {
        let n = self.ident()?;
        let mut t = if let Some(s) = Scalar::parse(&n) { Ty::Scalar(s) }
        else if n == "string" || n == "utf8" || n == "ascii" { self.eat("<")?; let k = self.count()?; self.eat(">")?; Ty::Str(k) }
        else if n == "ref" { return Err("`ref<...>` was removed: keep the item's slot (`slot(item)`) in an integer field and read it back with `pool[slot]` (inside `or(...)` for a fallback when it is gone)".into()); }
        else if n == "list" { self.eat("<")?; let e = self.ty()?; self.eat(",")?; let k = self.count()?; self.eat(">")?; let f = if self.word("drop_oldest") { Full::DropOldest } else { Full::Fail }; Ty::List(Box::new(e), k, f) }
        else if n == "table" {
            // `table<T> index(f, ...) unique(f, ...)`: rows in SQLite, as many as are added (no capacity)
            self.eat("<")?; let e = self.ty()?;
            if self.peek() == Some(",") { return Err("a table has no capacity: it holds as many rows as are added (`table<T>`)".into()); }
            self.eat(">")?;
            let (mut ix, mut un) = (Vec::new(), Vec::new());
            loop {
                let which = if self.word("index") { 0 } else if self.word("unique") { 1 } else { break };
                self.eat("(")?;
                let mut fs = vec![self.ident()?];
                while self.peek() == Some(",") { self.eat(",")?; fs.push(self.ident()?); }
                self.eat(")")?;
                if which == 0 { ix.push(fs) } else { un.push(fs) }
            }
            // `extern`: the table is another program's (its database named by `--db FILE`), checked at start and never changed in shape
            let ext = self.word("extern");
            Ty::Table(Box::new(e), ix, un, ext)
        }
        else if n == "pool" { self.eat("<")?; let e = self.ty()?; self.eat(",")?; let k = self.count()?; self.eat(">")?; let c = self.word("checked"); Ty::Pool(Box::new(e), k, c) }
        else { Ty::Named(n) };
        while self.peek() == Some("[") { self.i += 1; let k = self.count()?; self.eat("]")?; t = Ty::Arr(Box::new(t), k); }
        Ok(t)
    }
}

/// One folded operation of a constant expression: integers stay integers (overflow is an error), a float with an integer is a float.
fn cbin(op: &str, a: CVal, b: CVal) -> Result<CVal, String> {
    match (a, b) {
        (CVal::Int(x), CVal::Int(y)) => {
            let r = match op {
                "+" => x.checked_add(y), "-" => x.checked_sub(y), "*" => x.checked_mul(y),
                "/" | "%" => { if y == 0 { return Err("division by zero".into()); } if op == "/" { x.checked_div(y) } else { x.checked_rem(y) } }
                "|" => Some(x | y), "^" => Some(x ^ y), "&" => Some(x & y),
                "<<" => if (0..63).contains(&y) { x.checked_mul(1i64 << y) } else { None },
                ">>>" => if (0..64).contains(&y) { Some(((x as u64) >> y) as i64) } else { None },
                ">>" => if (0..64).contains(&y) { Some(x >> y) } else { None },
                _ => None,
            };
            r.map(CVal::Int).ok_or(format!("`{} {} {}` overflows an i64 (or shifts by a bad amount)", x, op, y))
        }
        (a, b) => {
            let f = |v: CVal| match v { CVal::Int(i) => i as f64, CVal::Flt(f) => f };
            let (x, y) = (f(a), f(b));
            let r = match op { "+" => x + y, "-" => x - y, "*" => x * y, "/" => { if y == 0.0 { return Err("division by zero".into()); } x / y }, _ => return Err(format!("`{}` takes integers, not floats", op)) };
            if r.is_finite() { Ok(CVal::Flt(r)) } else { Err("not a finite number".into()) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn const_err(consts: &str) -> String { Program::parse(&format!("{} struct A {{ n: i64; }} state a: A;", consts)).err().unwrap_or_else(|| "no error".into()) }

    #[test]
    fn const_groups_fold_and_name_their_mistakes() {
        let p = Program::parse("const L { N: i64 = 6; M = L.N * 2 + 1; NEG = -L.M; SH = 1 << 4; } const T { G: f64 = 9.8; H: f64 = T.G / 2; I: f32 = 3; } struct A { s: string<L.N>; v: i64 in L.NEG..L.M; xs: u8[L.N + 2]; } state a: A;").unwrap();
        let v = |g: &str, n: &str| p.consts.iter().find(|x| x.name == g).and_then(|x| x.items.iter().find(|y| y.0 == n)).map(|y| y.1);
        assert_eq!(v("L", "M"), Some(CVal::Int(13)));
        assert_eq!(v("L", "NEG"), Some(CVal::Int(-13)));
        assert_eq!(v("L", "SH"), Some(CVal::Int(16)));
        assert_eq!(v("T", "G"), Some(CVal::Flt(9.8)));
        assert_eq!(v("T", "H"), Some(CVal::Flt(4.9)));
        assert_eq!(v("T", "I"), Some(CVal::Flt(3.0)));
        let s = &p.structs[0];
        assert!(matches!(s.fields[0].1, Ty::Str(6)) && s.ranges[1] == Some((-13, 13)) && matches!(&s.fields[2].1, Ty::Arr(_, 8)));
        // mistakes name the constant
        assert!(const_err("const L { A = 9223372036854775807 + 1; }").contains("constant `L.A`") && const_err("const L { A = 9223372036854775807 + 1; }").contains("overflows"));
        assert!(const_err("const L { A: u8 = 256; }").contains("constant `L.A` is 256, which does not fit u8"));
        assert!(const_err("const L { A = 1.5; }").contains("constant `L.A` is a float: give it a type"));
        assert!(const_err("const L { A: i64 = 1.5; }").contains("is a float but declared"));
        assert!(const_err("const L { A = x; }").contains("constant `L.A`: `x` is not a constant"));
        assert!(const_err("const L { A = M.B; }").contains("constant `L.A`") && const_err("const L { A = M.B; }").contains("not a constant"));
        assert!(const_err("const L { A = 1; B = L.C; }").contains("constant `L.B`") && const_err("const L { A = 1; B = L.C; }").contains("has no `C`: it has `A`"));
        assert!(const_err("const L { A = 1; A = 2; }").contains("`A` twice"));
        assert!(const_err("const L { A = 1; } const L { B = 2; }").contains("const group `L` is declared twice"));
        assert!(const_err("const A { X = 1; }").contains("has the name of a struct"));
        assert!(const_err("enum E { X } const E { Y = 1; }").contains("has the name of an enum"));
        assert!(const_err("const a { Y = 1; }").contains("has the name of the state"));
        assert!(const_err("const L { A = 1 / 0; }").contains("division by zero"));
        assert!(const_err("const L { }").contains("has no constants"));
        assert!(Program::parse("const L { N: f64 = 2; } struct A { s: string<L.N>; } state a: A;").err().unwrap_or_default().contains("is a float"));
        assert!(Program::parse("const L { N = 2; } struct A { s: string<L.M>; } state a: A;").err().unwrap_or_default().contains("has no `M`: it has `N`"));
    }

    #[test]
    fn code_names_the_constants_it_misses() {
        let src = "const L { N = 4; G: f64 = 1.5; } struct A { n: i64; } state a: A;\n@get(\"/t\") fn t() -> Response<{ x: i64 }> { return ok({ x: L.NOPE }); }";
        let (state, api) = crate::source::load(src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        let e = crate::lang::bind_with(&mut api, &|n| prog.global(n), &prog.enum_table(), &prog.const_table()).unwrap_err();
        assert!(e.contains("const group `L` has no `NOPE`: it has `N`, `G`"), "{}", e);
    }

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

    /// Tables: kept outside the reservation, with the schema SQLite is given; what a row may not hold is refused.
    #[test]
    fn tables_and_their_schema() {
        let p = Program::parse("enum Kind { A, B } struct Todo { title: utf8<1000>; done: bool; rank: i64 in 0..9; kind: Kind; }
            struct Board { secret: i64; name: ascii<32>; todos: table<Todo> index(done); }
            struct App { boards: table<Board> unique(name) index(secret, name); n: i64; }
state app: App;").unwrap();
        assert_eq!(p.tables.len(), 2);
        assert_eq!((p.tables[0].sql.as_str(), p.tables[1].sql.as_str(), p.tables[1].parent), ("\"boards\"", "\"boards.todos\"", Some(0)));
        assert_eq!(p.total, 4096); // the plain `n` only: no table takes room in the reservation
        let sql = p.schema_sql();
        for want in ["CREATE TABLE IF NOT EXISTS \"boards\" (id INTEGER PRIMARY KEY AUTOINCREMENT, \"secret\" INTEGER NOT NULL DEFAULT 0, \"name\" TEXT NOT NULL DEFAULT '') STRICT;",
                     "CREATE TABLE IF NOT EXISTS \"boards.todos\" (id INTEGER PRIMARY KEY AUTOINCREMENT, parent_id INTEGER NOT NULL REFERENCES \"boards\"(id) ON DELETE CASCADE, \"title\" TEXT NOT NULL DEFAULT '', \"done\" INTEGER NOT NULL DEFAULT 0, \"rank\" INTEGER NOT NULL DEFAULT 0, \"kind\" INTEGER NOT NULL DEFAULT 0) STRICT;",
                     "CREATE INDEX IF NOT EXISTS \"boards.todos by parent\" ON \"boards.todos\"(parent_id, id);",
                     "CREATE INDEX IF NOT EXISTS \"boards index 0\" ON \"boards\"(\"secret\", \"name\");",
                     "CREATE UNIQUE INDEX IF NOT EXISTS \"boards unique 0\" ON \"boards\"(\"name\");",
                     "CREATE INDEX IF NOT EXISTS \"boards.todos index 0\" ON \"boards.todos\"(\"done\");"] {
            assert!(sql.contains(want), "missing {}
in
{}", want, sql);
        }
        assert_eq!(p.tables[1].cols[2].range, Some((0, 9)));
        assert_eq!(p.tables[0].cols[1].kind, TKind::Text(32, true));
        let err = |src: &str| Program::parse(src).err().unwrap_or_default();
        assert!(err("struct T { a: i64; } struct S { t: table<T, 10>; } state s: S;").contains("no capacity"));
        assert!(err("struct I { a: i64; } struct T { xs: list<I, 4>; } struct S { t: table<T>; } state s: S;").contains("make it a `table<...>`"));
        assert!(err("struct T { a: i64; } struct I { t: table<T>; } struct S { p: pool<I, 4>; } state s: S;").contains("inside an item of a pool or list"));
        assert!(err("struct T { a: i64; } struct S { t: table<T> index(b); } state s: S;").contains("not a column"));
        assert!(err("struct T { id: i64; } struct S { t: table<T>; } state s: S;").contains("own column"));
        // `unlogged` was the opt-out; it is the default now and `logged` the opt-in (a table is atomic in its database whatever it says)
        assert!(err("struct T { a: i64; } struct S { t: table<T> unlogged; } state s: S;").contains("unlogged by default"));
        assert!(err("struct T { a: i64; } struct S { n: i64; } state s: S unlogged;").contains("unlogged by default"));
        assert!(err("struct I { a: i64 logged; } struct S { p: pool<I, 4>; } state s: S;").contains("declared `logged`, but items of a collection"));
        assert!(Program::parse("struct T { a: i64; } struct S { t: table<T>; n: i64 logged; } state s: S logged;").is_ok());
    }
}
