//! Parse state declarations and compute their exact byte layout (Stasis-style: all sizes fixed).
use std::collections::HashMap;

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

#[derive(Clone, Debug)]
pub enum Ty {
    Scalar(Scalar),
    Str(usize),            // string<N>: u32 length, then N bytes of capacity
    List(Box<Ty>, usize),  // list<T, N>: u32 length, then N elements of capacity
    Arr(Box<Ty>, usize),   // T[N]: exactly N elements, all always present
    Table(Box<Ty>, usize), // table<Struct, N>: same data as list<Struct, N>, stored one column per field
    Struct(usize),
    Named(String),         // only before resolution
}

/// One column of a table. Strings and lists take two: a length column and a data column.
#[derive(Clone, Debug)]
pub enum CKind { Scalar(Scalar), Arr(Scalar, usize), StrLen, StrBytes(usize), ListLen, ListData(Scalar, usize) }
#[derive(Clone, Debug)]
pub struct TCol { pub path: String, pub kind: CKind, pub off: usize } // off is relative to the table start

pub const TABLE_HEADER: usize = 16;

pub struct StructDef { pub name: String, pub fields: Vec<(String, Ty, usize)>, pub size: usize, pub align: usize, done: bool, visiting: bool }
pub struct Program { pub structs: Vec<StructDef>, pub globals: Vec<(String, Ty, usize)>, pub total: usize }

pub fn up(n: usize, to: usize) -> usize { (n + to - 1) / to * to }

impl Program {
    pub fn size_align(&self, t: &Ty) -> (usize, usize) {
        match t {
            Ty::Scalar(s) => (s.size(), s.size()),
            Ty::Str(n) => (up(4 + n, 4), 4),
            Ty::List(e, n) => { let (es, ea) = self.size_align(e); let a = ea.max(4); (up(up(4, ea) + es * n, a), a) }
            Ty::Arr(e, n) => { let (es, ea) = self.size_align(e); (es * n, ea) }
            Ty::Table(row, n) => (self.table_cols(row, *n).unwrap().1, 8),
            Ty::Struct(i) => (self.structs[*i].size, self.structs[*i].align),
            Ty::Named(_) => unreachable!(),
        }
    }

    /// Columns of `table<row, n>` and the table's total size. Rows may hold scalars, scalar arrays,
    /// strings, lists of scalars, and nested structs of those.
    pub fn table_cols(&self, row: &Ty, n: usize) -> Result<(Vec<TCol>, usize), String> {
        let mut fields = Vec::new();
        self.flatten(row, "", &mut fields)?;
        let (mut cols, mut off) = (Vec::new(), TABLE_HEADER);
        let mut place = |path: &str, kind: CKind, align: usize, bytes: usize, off: &mut usize, cols: &mut Vec<TCol>| {
            *off = up(*off, align); cols.push(TCol { path: path.to_string(), kind, off: *off }); *off += bytes;
        };
        for (path, t) in fields {
            match t {
                Ty::Scalar(s) => place(&path, CKind::Scalar(s), s.size(), n * s.size(), &mut off, &mut cols),
                Ty::Arr(e, k) => { let Ty::Scalar(s) = *e else { unreachable!() }; place(&path, CKind::Arr(s, k), s.size(), n * k * s.size(), &mut off, &mut cols) }
                Ty::Str(m) => { place(&path, CKind::StrLen, 4, n * 4, &mut off, &mut cols); place(&path, CKind::StrBytes(m), 1, n * m, &mut off, &mut cols) }
                Ty::List(e, m) => { let Ty::Scalar(s) = *e else { unreachable!() }; place(&path, CKind::ListLen, 4, n * 4, &mut off, &mut cols); place(&path, CKind::ListData(s, m), s.size(), n * m * s.size(), &mut off, &mut cols) }
                _ => unreachable!(),
            }
        }
        Ok((cols, up(off, 8)))
    }
    fn flatten(&self, t: &Ty, prefix: &str, out: &mut Vec<(String, Ty)>) -> Result<(), String> {
        match t {
            Ty::Struct(i) => for f in &self.structs[*i].fields {
                let p = if prefix.is_empty() { f.0.clone() } else { format!("{}.{}", prefix, f.0) };
                self.flatten(&f.1, &p, out)?;
            },
            Ty::Scalar(_) | Ty::Str(_) => out.push((prefix.to_string(), t.clone())),
            Ty::Arr(e, _) | Ty::List(e, _) if matches!(**e, Ty::Scalar(_)) => out.push((prefix.to_string(), t.clone())),
            _ => return Err(format!("table rows cannot hold `{}` at `{}`: use scalars, scalar arrays, strings, lists of scalars, or structs of those", self.show(t), prefix)),
        }
        Ok(())
    }
    /// Bytes before the first element of a list: the length, padded to element alignment.
    pub fn list_header(&self, e: &Ty) -> usize { up(4, self.size_align(e).1) }
    /// True if every byte of this type is always in use (no strings or lists inside).
    pub fn all_live(&self, t: &Ty) -> bool {
        match t {
            Ty::Scalar(_) => true,
            Ty::Str(_) | Ty::List(..) | Ty::Table(..) => false,
            Ty::Arr(e, _) => self.all_live(e),
            Ty::Struct(i) => self.structs[*i].fields.iter().all(|f| self.all_live(&f.1)),
            Ty::Named(_) => unreachable!(),
        }
    }
    pub fn show(&self, t: &Ty) -> String {
        match t {
            Ty::Scalar(s) => format!("{:?}", s).to_lowercase(),
            Ty::Str(n) => format!("string<{}>", n),
            Ty::List(e, n) => format!("list<{}, {}>", self.show(e), n),
            Ty::Arr(e, n) => format!("{}[{}]", self.show(e), n),
            Ty::Table(e, n) => format!("table<{}, {}>", self.show(e), n),
            Ty::Struct(i) => self.structs[*i].name.clone(),
            Ty::Named(n) => n.clone(),
        }
    }

    pub fn parse(src: &str) -> Result<Program, String> {
        let mut p = P { t: tokenize(src), i: 0 };
        let mut structs: Vec<StructDef> = Vec::new();
        let mut globals: Vec<(String, Ty, usize)> = Vec::new();
        while p.i < p.t.len() {
            match p.ident()?.as_str() {
                "struct" => {
                    let name = p.ident()?; p.eat("{")?;
                    let mut fields = Vec::new();
                    while p.peek() != Some("}") { let f = p.ident()?; p.eat(":")?; let ty = p.ty()?; p.eat(";")?; fields.push((f, ty, 0)); }
                    p.eat("}")?;
                    structs.push(StructDef { name, fields, size: 0, align: 1, done: false, visiting: false });
                }
                "global" => { let n = p.ident()?; p.eat(":")?; let ty = p.ty()?; p.eat(";")?; globals.push((n, ty, 0)); }
                o => return Err(format!("expected `struct` or `global`, found `{}`", o)),
            }
        }
        let mut prog = Program { structs, globals: Vec::new(), total: 0 };
        let names: HashMap<String, usize> = prog.structs.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();
        for i in 0..prog.structs.len() { prog.layout_struct(i, &names)?; }
        let mut off = 0;
        for (n, ty, _) in globals {
            let ty = prog.resolve(ty, &names)?;
            let (s, a) = prog.size_align(&ty);
            off = up(off, a.max(8)); // each global starts 8-aligned
            prog.globals.push((n, ty, off));
            off += s;
        }
        prog.total = off;
        Ok(prog)
    }

    fn resolve(&mut self, t: Ty, names: &HashMap<String, usize>) -> Result<Ty, String> {
        Ok(match t {
            Ty::Named(n) => { let i = *names.get(&n).ok_or(format!("unknown type `{}`", n))?; self.layout_struct(i, names)?; Ty::Struct(i) }
            Ty::List(e, n) => Ty::List(Box::new(self.resolve(*e, names)?), n),
            Ty::Arr(e, n) => Ty::Arr(Box::new(self.resolve(*e, names)?), n),
            Ty::Table(e, n) => { let e = self.resolve(*e, names)?; if !matches!(e, Ty::Struct(_)) { return Err("table<T, N> needs a struct T".into()); } self.table_cols(&e, n)?; Ty::Table(Box::new(e), n) }
            o => o,
        })
    }

    fn layout_struct(&mut self, i: usize, names: &HashMap<String, usize>) -> Result<(), String> {
        if self.structs[i].done { return Ok(()); }
        if self.structs[i].visiting { return Err(format!("struct `{}` contains itself: sizes must be fixed", self.structs[i].name)); }
        self.structs[i].visiting = true;
        let mut fields = std::mem::take(&mut self.structs[i].fields);
        let (mut off, mut align) = (0, 1);
        for f in fields.iter_mut() {
            let ty = self.resolve(std::mem::replace(&mut f.1, Ty::Scalar(Scalar::U8)), names)?;
            let (s, a) = self.size_align(&ty);
            off = up(off, a); f.2 = off; f.1 = ty; off += s; align = align.max(a);
        }
        let s = &mut self.structs[i];
        s.fields = fields; s.size = up(off, align); s.align = align; s.done = true; s.visiting = false;
        Ok(())
    }
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
    fn ty(&mut self) -> Result<Ty, String> {
        let n = self.ident()?;
        let mut t = if let Some(s) = Scalar::parse(&n) { Ty::Scalar(s) }
        else if n == "string" { self.eat("<")?; let k = self.num()?; self.eat(">")?; Ty::Str(k) }
        else if n == "list" { self.eat("<")?; let e = self.ty()?; self.eat(",")?; let k = self.num()?; self.eat(">")?; Ty::List(Box::new(e), k) }
        else if n == "table" { self.eat("<")?; let e = self.ty()?; self.eat(",")?; let k = self.num()?; self.eat(">")?; Ty::Table(Box::new(e), k) }
        else { Ty::Named(n) };
        while self.peek() == Some("[") { self.i += 1; let k = self.num()?; self.eat("]")?; t = Ty::Arr(Box::new(t), k); }
        Ok(t)
    }
}
