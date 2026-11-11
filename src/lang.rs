//! The endpoint language: tokens, syntax tree, parser, and the binder.
//! A file holds `fn` declarations and `endpoint` handlers. Types written in `fn` signatures are documentation for now.
//!
//! The binder runs once when a server loads the file. It gives every local a fixed slot in its function's frame,
//! resolves every name to a local, a global or a function, checks argument counts and assignments, and turns builtin
//! calls into an enum. Errors that used to surface as a 500 at run time (an unknown name, assigning to a `let`)
//! now stop the server from starting.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok { Id(String), Int(i64), Flt(f64), Str(String), Sym(String),
    /// markup (HTML literals): `<tag`, an attribute's name, `>` or `/>` (true), `</tag>`, text, a `"` around an attribute's value
    HOpen(String), HAttr(String), HEnd(bool), HClose(String), HText(String), HQ }

/// HTML's void elements: no closing tag, no content.
pub const VOID: [&str; 14] = ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];

/// Text between tags as written in source: on one line it is kept as it is; across lines, each line is trimmed, empty lines dropped, and
/// the rest joined by one space (so indentation and line breaks between tags cost nothing in the page).
fn markup_text(raw: &str) -> String {
    if !raw.contains('\n') { return raw.to_string(); }
    let lines: Vec<&str> = raw.split('\n').collect();
    let n = lines.len();
    lines.iter().enumerate().map(|(k, l)| if k == 0 { l.trim_end() } else if k == n - 1 { l.trim_start() } else { l.trim() }).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
}
/// Where the lexer is: code (`depth` braces open since it began; `html`: it began at a `{` inside markup and ends at its `}`), the
/// content of markup (`depth` elements open), inside a start tag (its name), inside a quoted attribute value.
enum Mode { Code { depth: u32, html: bool }, Content { depth: u32 }, Tag(String), AttrQ }

/// What a name refers to, filled in by the binder.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum VarRes { #[default] Unbound, Local(u32), Global(u32) }

thread_local! {
    /// Where `file("...")` reads: the program's directory, and files given by name first (a test case's `--- file` sections).
    static FILES: std::cell::RefCell<(Option<std::path::PathBuf>, Vec<(String, String)>)> = std::cell::RefCell::new((None, Vec::new()));
}
thread_local! {
    /// `asset("name")`: the web block's manifest, each asset's name and URL.
    static ASSETS: std::cell::RefCell<Vec<(String, String)>> = std::cell::RefCell::new(Vec::new());
}
/// The assets `asset("name")` resolves (the web block's `manifest`).
pub fn set_assets(a: Vec<(String, String)>) { ASSETS.with(|x| *x.borrow_mut() = a); }
/// `file("...")` reads relative to `dir` (the program's directory).
pub fn set_file_dir(dir: &std::path::Path) { FILES.with(|f| f.borrow_mut().0 = Some(dir.to_path_buf())); }
/// `file("...")` finds these by name before looking on disk.
pub fn set_files(files: Vec<(String, String)>) { FILES.with(|f| f.borrow_mut().1 = files); }
/// The bytes of a file named in the program (`file_base64("logos/icon.png")`), read when compiling, relative to the program's directory.
fn read_file_bytes(name: &str) -> R<Vec<u8>> {
    if std::path::Path::new(name).is_absolute() || name.is_empty() { return Err(format!("`file_base64(\"{}\")`: a path relative to the program's directory", name)); }
    FILES.with(|f| {
        let f = f.borrow();
        if let Some(x) = f.1.iter().find(|x| x.0 == name) { return Ok(x.1.clone().into_bytes()); }
        let Some(dir) = &f.0 else { return Err(format!("`file_base64(\"{}\")`: no program directory to read from", name)) };
        std::fs::read(dir.join(name)).map_err(|e| format!("`file_base64(\"{}\")`: {}", name, e))
    })
}
/// Standard base64, padded.
fn base64(b: &[u8]) -> String {
    const AL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let v = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        o.push(AL[(v >> 18) as usize] as char); o.push(AL[(v >> 12 & 63) as usize] as char);
        o.push(if c.len() > 1 { AL[(v >> 6 & 63) as usize] as char } else { '=' }); o.push(if c.len() > 2 { AL[(v & 63) as usize] as char } else { '=' });
    }
    o
}
/// The text of a file named in the program (`file("assets/app.css")`), read when compiling: relative to the program's directory, UTF-8.
fn read_file(name: &str) -> R<String> {
    if std::path::Path::new(name).is_absolute() || name.is_empty() { return Err(format!("`file(\"{}\")`: a path relative to the program's directory", name)); }
    FILES.with(|f| {
        let f = f.borrow();
        if let Some(x) = f.1.iter().find(|x| x.0 == name) { return Ok(x.1.clone()); }
        let Some(dir) = &f.0 else { return Err(format!("`file(\"{}\")`: no program directory to read from", name)) };
        let p = dir.join(name);
        let b = std::fs::read(&p).map_err(|e| format!("`file(\"{}\")`: {}", name, e))?;
        String::from_utf8(b).map_err(|_| format!("`file(\"{}\")`: not UTF-8 text (only text files are embedded)", name))
    })
}

/// Markup's fixed text so far, as one statement that writes it.
fn flush_lit(out: &mut Vec<Stmt>, lit: &mut String) {
    if !lit.is_empty() { out.push(Stmt::Do(Expr::Call("__html_lit".into(), vec![Expr::Str(std::mem::take(lit))], CallRes::Unbound))); }
}

/// A cookie name: an HTTP token (RFC 6265 cookie-name).
pub fn cookie_name_ok(c: &str) -> bool { !c.is_empty() && c.len() <= 64 && c.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) }

/// Built-in functions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum B { Sin, Cos, Atan2, Pow, InRange, F32, Body, Param, Query, Header, Fwd, Int, Str, Len, Now, Random, Min, Max, Count, Capacity, Slot, Stat, Token, Auth, WorkerIndex, WorkerCount, Crash, Or, Exists, MaxLen, CountBelow, Sqrt, Exp, Floor, Abs, Substr, TextFrom, WrapAdd, WrapSub, WrapMul, ReqGet, ReqPost, ReqPut, ReqPatch, ReqDelete, JsonField, Truncate, Log, SetCookie, SetHeader, HtmlLit, HtmlEsc, HtmlEscAttr, HtmlAttr, HtmlRaw, Trim,
    Base64, Base64Url, Unbase64, Hex, Sha1, Sha256, HmacSha1, HmacSha256, Pbkdf2Sha256, RandomBytes, XorBytes, SecretEqual, GcmSeal, GcmOpen, BcryptHash, BcryptCheck, FormEncode, FormDecode, Env, FormatTime, Contains, StartsWith, EndsWith, NoDefaultHeaders, EscapeHtml, RandomText, AddYears, IndexOf, LastIndexOf, JsonQuote, JsonUnquote, RequestTarget, Crc32, ReadStored, SocketSend, SocketClose, SocketSubscribe, SocketUnsubscribe, Broadcast, SocketSendAll, SocketTag, ByteAt, Lower, Upper, CacheGet, CachePut, CacheStart, CacheHole, CachePart, CacheLabel, DataVersion, Replace, TrimStart, TrimEnd, AllEmoji,
    HtmlParse, HtmlParseIn, NodeKind, NodeName, NodeParent, NodeFirst, NodeLast, NodeNext, NodePrev, NodeContent, NodeAttr, NodeAttrCount, NodeAttrName, NodeAttrValue,
    NodeSetAttr, NodeRemoveAttr, NodeNewElement, NodeNewText, NodeAppend, NodeInsertBefore, NodeDetach, NodeClone, NodeHtml, NodeInnerHtml, NodeSetText, NodeHtmlEscaped, Chr, AllWord }
impl B {
    fn from(n: &str) -> Option<B> {
        use B::*;
        Some(match n {
            "body" => Body, "param" => Param, "query" => Query, "header" => Header, "fwd" => Fwd, "int" => Int, "str" => Str, "len" => Len, "now" => Now,
            "random" => Random, "min" => Min, "max" => Max, "count" => Count, "capacity" => Capacity, "slot" => Slot, "stat" => Stat, "token" => Token,
            "auth" => Auth, "worker_index" => WorkerIndex, "worker_count" => WorkerCount, "__crash" => Crash, "__mito__in" => InRange, "__mito__f32" => F32, "sin" => Sin, "cos" => Cos, "atan2" => Atan2, "pow" => Pow, "or" => Or, "exists" => Exists, "maxlen" => MaxLen, "sqrt" => Sqrt, "exp" => Exp, "floor" => Floor, "abs" => Abs, "substr" => Substr, "text_from" => TextFrom, "wrapping_add" => WrapAdd, "wrapping_sub" => WrapSub, "wrapping_mul" => WrapMul, "log" => Log, "set_cookie" => SetCookie, "set_header" => SetHeader, "__html_lit" => HtmlLit, "__html_esc" => HtmlEsc, "__html_esc_attr" => HtmlEscAttr, "__html_attr" => HtmlAttr, "html_raw" => HtmlRaw, "html_text" => HtmlEsc, "trim" => Trim,
            "base64" => Base64, "base64_url" => Base64Url, "unbase64" => Unbase64, "hex" => Hex, "sha1" => Sha1, "sha256" => Sha256,
            "hmac_sha1" => HmacSha1, "hmac_sha256" => HmacSha256, "pbkdf2_sha256" => Pbkdf2Sha256, "random_bytes" => RandomBytes,
            "xor_bytes" => XorBytes, "secret_equal" => SecretEqual, "aes256gcm_seal" => GcmSeal, "aes256gcm_open" => GcmOpen,
            "bcrypt_hash" => BcryptHash, "bcrypt_check" => BcryptCheck, "form_encode" => FormEncode, "form_decode" => FormDecode, "env" => Env, "json_field" => JsonField, "format_time" => FormatTime, "contains" => Contains, "starts_with" => StartsWith, "ends_with" => EndsWith, "no_default_headers" => NoDefaultHeaders, "escape_html" => EscapeHtml, "random_text" => RandomText, "add_years" => AddYears, "index_of" => IndexOf, "last_index_of" => LastIndexOf, "json_quote" => JsonQuote, "json_unquote" => JsonUnquote, "request_target" => RequestTarget, "crc32" => Crc32, "read_stored" => ReadStored, "socket_send" => SocketSend, "socket_close" => SocketClose, "socket_subscribe" => SocketSubscribe,
            "socket_unsubscribe" => SocketUnsubscribe, "broadcast" => Broadcast, "socket_send_all" => SocketSendAll, "socket_tag" => SocketTag, "byte_at" => ByteAt, "lower" => Lower, "upper" => Upper, "cache_get" => CacheGet, "cache_put" => CachePut, "cache_start" => CacheStart, "cache_hole" => CacheHole, "cache_part" => CachePart, "cache_label" => CacheLabel, "data_version" => DataVersion, "replace" => Replace, "trim_start" => TrimStart, "trim_end" => TrimEnd, "all_emoji" => AllEmoji, "html_parse" => HtmlParse, "html_parse_in" => HtmlParseIn, "node_kind" => NodeKind, "node_name" => NodeName, "node_parent" => NodeParent, "node_first" => NodeFirst, "node_last" => NodeLast, "node_next" => NodeNext, "node_prev" => NodePrev, "node_content" => NodeContent, "node_attr" => NodeAttr, "node_attr_count" => NodeAttrCount, "node_attr_name" => NodeAttrName, "node_attr_value" => NodeAttrValue, "node_set_attr" => NodeSetAttr, "node_remove_attr" => NodeRemoveAttr, "node_new_element" => NodeNewElement, "node_new_text" => NodeNewText, "node_append" => NodeAppend, "node_insert_before" => NodeInsertBefore, "node_detach" => NodeDetach, "node_clone" => NodeClone, "node_html" => NodeHtml, "node_inner_html" => NodeInnerHtml, "node_set_text" => NodeSetText, "node_html_escaped" => NodeHtmlEscaped, "chr" => Chr, "all_word" => AllWord,
            _ => return None,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CallRes { #[default] Unbound, Builtin(B), User(u32) }
impl Default for B { fn default() -> B { B::Now } }

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64), Float(f64), Str(String), Bool(bool), Null,
    Var(String, VarRes),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>, CallRes),
    Un(char, Box<Expr>),
    Bin(String, Box<Expr>, Box<Expr>),
}

/// `foreach m in room.messages reverse where ... limit n`
#[derive(Debug, Clone)]
/// `foreach var in coll ...`, or with `hi`, a range of integers `foreach var in coll..hi ...` (from `coll`, the low bound,
/// up to `hi` included; `hi` here is one past it, as the parser excludes the end internally).
/// `from`: where a loop over a module instance starts. `via`: that instance's iteration functions (set by the binder).
/// `whil`: `while cond` ends the loop as soon as `cond` is false (tested before `where` and the body).
/// `scan`: a `where` over a whole table that reads every row, accepted as such (a schema the program cannot add an index to).
/// `by`: a table walked in the order of one of its rows' fields (an indexed one). `after`: a table's rows after this row (or id) in the
/// loop's order, for paging (null: from the start).
pub struct Iter { pub index: Option<(String, u32)>, pub budget: bool, pub var: String, pub slot: u32, pub coll: Expr, pub hi: Option<Expr>, pub from: Option<Expr>, pub reverse: bool, pub by: Option<String>, pub after: Option<Expr>, pub whil: Option<Expr>, pub wher: Option<Expr>, pub limit: Option<Expr>, pub via: Option<Via>, pub scan: bool, pub last: bool }

/// A module instance's iteration functions: a cursor from `first(from)` (or `last(from)` for reverse), the next from
/// `next(cur)` (or `prev`), the item at it from `item(cur)`; a null cursor ends the loop.
#[derive(Debug, Clone, Copy)]
pub struct Via { pub first: u32, pub next: u32, pub item: u32, pub last: Option<u32>, pub prev: Option<u32> }

#[derive(Debug, Clone)]
/// An answer's JSON: an object, an array made by a loop, a value, or nothing (`no_content()`).
pub enum Json { Obj(Vec<(String, Json)>), Arr(Iter, Box<Json>), E(Expr), Empty,
    /// the answer's body was written by the markup before (`ok(<p>...</p>)`)
    Html }

/// The `T` of an endpoint's `Response<T>`: the shape of every answer it gives. `Name` is a scalar (`i64`, `bool`,
/// `string`, ...), `none` (no body), a declared struct (an object with its fields), or an item type (written as an id).
#[derive(Debug, Clone, PartialEq)]
pub enum RType { Name(String), Opt(Box<RType>), Arr(Box<RType>), Obj(Vec<(String, RType)>) }

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(String, u32, Expr),
    Var(String, u32, Expr),
    Alloc { var: String, slot: u32, coll: Expr, try_: bool },
    Assign(Expr, Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    Each(Iter, Vec<Stmt>),
    /// `coll.remove(item)`: the item leaves the collection when the request succeeds.
    Remove(Expr, Expr),
    /// `coll.clear()`: every item leaves the collection at once (and what the items hold), undone with the request if it fails.
    Clear(Expr),
    /// `array.move(from, to, count)`: `count` elements of a fixed array move from position `from` to position `to`, as
    /// if through a temporary (the array, from, to, count).
    Move(Expr, Expr, Expr, Expr),
    /// `dst.copy_from(src, from, to, count)`: `count` elements of the fixed array `src` from position `from` are copied to position `to` of the fixed array
    /// `dst` (another array of the same element type; the destination array, from, to, count, and the source first after it: dst, src, from, to, count).
    Copy(Expr, Expr, Expr, Expr, Expr),
    /// `array.sort(from, count, scratch)`: `count` integer elements of a fixed array from position `from` put in ascending order (a stable sort); `scratch` is another fixed array of the same
    /// element type, with room for `count` elements, the sort uses (the array, from, count, scratch).
    Sort(Expr, Expr, Expr, Expr),
    /// `array.fill(from, count, value)`: `count` elements of a fixed array from position `from` are set to `value` (the array, from, count, value).
    Fill(Expr, Expr, Expr, Expr),
    Fail(u16, String),
    Respond(u16, Json),
    Return(Option<Expr>),
    /// Hand the request on to another worker: `forward rooms[key] { "user": expr };`
    Forward { kind: String, key: Expr, fields: Vec<(String, Expr)> },
    Do(Expr),
    /// `break;` / `continue;`: leave the innermost loop / go on with its next round.
    Break,
    Continue,
    /// Where `continue` lands in a `for` loop: before its step (the end of the body elsewhere).
    ContinueHere,
    /// A `match`'s arms as written, for the binder's checks (each value once; on an enum, every value or `else`); the
    /// `match` itself runs as the `if` chain after it. Generates nothing.
    MatchArms(Vec<Pat>, bool),
    /// Where the next statement starts in the source (line, column): kept for error messages; generates nothing.
    At(u32, u32),
}

/// A `match` arm's pattern: a constant (a number, `-n`, an enum value) or a range of them, both ends included.
#[derive(Debug, Clone)]
pub enum Pat { One(Expr), Range(Expr, Expr) }

#[derive(Debug, Clone)]
/// A path segment: literal text, or a parameter (after a literal prefix, possibly empty: `@{message}` takes `@933` as 933)
pub enum Seg { Lit(String), Param(String, String) }
#[derive(Debug, Clone)]
/// `lines`: first and last source line (1-based) of the endpoint, for comments in generated code.
/// An endpoint: a function with an attribute (`@post("/players") fn add_player(name: string<1..24>) -> Response<T>`).
/// Its parameters are its typed inputs, checked before the body runs.
pub struct Endpoint { pub method: String, pub segs: Vec<Seg>, pub body: Vec<Stmt>, pub text: String, pub slots: u32, pub lines: (u32, u32), pub name: String, pub resp: RType, pub inputs: Vec<Input> }

/// A typed input: where it comes from, its type, and what an absent one is (`= default`, or `?`: null).
#[derive(Debug, Clone)]
pub struct Input { pub name: String, pub src: Src, pub ty: InTy, pub default: Option<Expr>, pub optional: bool }
/// Where an input comes from: a path segment, the JSON body, the query string, a header, or a field forwarded by
/// another worker.
#[derive(Debug, Clone, PartialEq)]
pub enum Src { Path, Body, Query, Header(String), Fwd, Cookie(String) }
/// An input's type: an integer in lo..hi, a number, a bool, a string of lo..hi bytes, or an item (`Player`, or
/// `Player in game.players`): from the path an id, from a header a session token.
#[derive(Debug, Clone, PartialEq)]
pub enum InTy { Int(i64, i64), Flt, Bool, Str(i64, i64, bool), Item(String, Option<String>) }
#[derive(Debug, Clone)]
/// A function. A module instance's functions are named `instance.name` (written by `modules::expand`); `public` ones
/// (`pub fn`) can be called from outside the module.
/// `ret_opt`: declared `-> T?`: the result may be null, and a caller must check it (`if exists(x)`) or give a fallback (`or`).
/// `test`: a test (`test "name" { ... }`): no parameters, a result of `bool` or text, run by `mito test` only.
/// `html`: declared `-> html`, a function that writes markup into the answer (used inside markup, as `{item(t)}`).
pub struct Func { pub test: Option<String>, pub name: String, pub params: Vec<String>, pub ptys: Vec<Option<PTy>>, pub ret: Option<PTy>, pub body: Vec<Stmt>, pub slots: u32, pub public: bool, pub ret_opt: bool, pub html: bool }
/// A type in a function's signature, as written: `i64`, `f64`, `bool`, `string`, a struct whose items the function takes
/// (`Player`, or `Player in game.players` when several collections hold it), the type of a struct's field (`T.key`, for
/// modules), each with `?` for "or null".
#[derive(Debug, Clone, PartialEq)]
pub struct PTy { pub name: String, pub arg: Option<String>, pub field: Option<String>, pub coll: Option<String>, pub opt: bool }
impl std::fmt::Display for PTy {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        if let Some(a) = &self.arg { write!(f, "<{}>", a)?; }
        if let Some(x) = &self.field { write!(f, ".{}", x)?; }
        if let Some(c) = &self.coll { write!(f, " in {}", c)?; }
        if self.opt { write!(f, "?")?; }
        Ok(())
    }
}
#[derive(Debug, Clone, Default)]
pub struct Api { pub eps: Vec<Endpoint>, pub fns: Vec<Func> }

/// Names (of fields, collections, functions, workers) are at most this long, and `fail` messages at most MAX_FAIL
/// bytes, so error messages fit every runtime's buffer.
pub const MAX_NAME: usize = 128;
pub const MAX_FAIL: usize = 1024;

/// Tokens, and the source line and column (1-based, in characters) each one starts on.
fn lex_lines(src: &str) -> Result<(Vec<Tok>, Vec<(u32, u32)>), String> {
    let c: Vec<char> = src.chars().collect();
    let (mut i, mut out, mut at, mut line, mut tl, mut line_start, mut tc) = (0, Vec::new(), Vec::new(), 1u32, 1u32, 0usize, 1u32);
    let mut modes = vec![Mode::Code { depth: 0, html: false }];
    while i < c.len() {
        let ch = c[i];
        while at.len() < out.len() { at.push((tl, tc)); } // tokens pushed by the last step started where it started
        tl = line;
        tc = (i - line_start + 1) as u32;
        // markup: content, a start tag, a quoted attribute value
        match modes.last_mut().unwrap() {
            Mode::Content { depth } => {
                if ch == '<' && c.get(i + 1) == Some(&'/') {
                    let s = i + 2; let mut k = s; while k < c.len() && (c[k].is_alphanumeric() || c[k] == '-') { k += 1; }
                    let name: String = c[s..k].iter().collect();
                    while k < c.len() && c[k] == ' ' { k += 1; }
                    if c.get(k) != Some(&'>') || name.is_empty() { return Err(format!("{}:{}: a closing tag is `</name>`", tl, tc)); }
                    i = k + 1; out.push(Tok::HClose(name));
                    *depth -= 1;
                    if *depth == 0 { modes.pop(); }
                } else if ch == '<' && c[i..].iter().take(4).collect::<String>() == "<!--" {
                    let mut k = i + 4; while k < c.len() && c[k..].iter().take(3).collect::<String>() != "-->" { if c[k] == '\n' { line += 1; line_start = k + 1; } k += 1; }
                    if k >= c.len() { return Err(format!("{}:{}: a comment that is never closed (`-->` is missing)", tl, tc)); }
                    i = k + 3; // comments are not sent
                } else if ch == '<' && c.get(i + 1) == Some(&'!') {
                    let s = i; let mut k = i; while k < c.len() && c[k] != '>' { k += 1; }
                    if k >= c.len() || *depth != 0 { return Err(format!("{}:{}: `<!doctype html>` comes first, before the root element", tl, tc)); }
                    i = k + 1; out.push(Tok::HText(c[s..i].iter().collect()));
                } else if ch == '<' && c.get(i + 1).map_or(false, |x| x.is_ascii_alphabetic()) {
                    let s = i + 1; let mut k = s; while k < c.len() && (c[k].is_alphanumeric() || c[k] == '-') { k += 1; }
                    let name: String = c[s..k].iter().collect();
                    i = k; out.push(Tok::HOpen(name.clone())); modes.push(Mode::Tag(name));
                } else if ch == '<' {
                    return Err(format!("{}:{}: a `<` in text: write `&lt;`", tl, tc));
                } else if ch == '{' {
                    i += 1; out.push(Tok::Sym("{".into())); modes.push(Mode::Code { depth: 0, html: true });
                } else if ch == '}' {
                    return Err(format!("{}:{}: a `}}` in text: write `&#125;` (code in markup is inside `{{ }}`)", tl, tc));
                } else {
                    let s = i;
                    while i < c.len() && c[i] != '<' && c[i] != '{' && c[i] != '}' { if c[i] == '\n' { line += 1; line_start = i + 1; } i += 1; }
                    let raw: String = c[s..i].iter().collect();
                    let t = markup_text(&raw);
                    if *depth == 0 { if !t.trim().is_empty() { return Err(format!("{}:{}: text outside the root element", tl, tc)); } }
                    else if !t.is_empty() { out.push(Tok::HText(t)); }
                }
                continue;
            }
            Mode::Tag(name) => {
                if ch.is_whitespace() { if ch == '\n' { line += 1; line_start = i + 1; } i += 1; continue; }
                let closes = ch == '>' || (ch == '/' && c.get(i + 1) == Some(&'>'));
                if closes {
                    let selfc = ch == '/';
                    i += if selfc { 2 } else { 1 };
                    let void = selfc || VOID.contains(&name.to_ascii_lowercase().as_str());
                    out.push(Tok::HEnd(selfc));
                    modes.pop();
                    if let Some(Mode::Content { depth }) = modes.last_mut() {
                        if !void { *depth += 1; } else if *depth == 0 { modes.pop(); }
                    }
                    continue;
                }
                if ch.is_alphanumeric() || ch == '-' || ch == ':' || ch == '_' || ch == '@' {
                    let s = i; while i < c.len() && (c[i].is_alphanumeric() || matches!(c[i], '-' | ':' | '_' | '@' | '.')) { i += 1; }
                    out.push(Tok::HAttr(c[s..i].iter().collect()));
                    if c.get(i) == Some(&'=') {
                        i += 1;
                        if c.get(i) == Some(&'"') { i += 1; out.push(Tok::HQ); modes.push(Mode::AttrQ); }
                        else if c.get(i) == Some(&'{') { i += 1; out.push(Tok::Sym("{".into())); modes.push(Mode::Code { depth: 0, html: true }); }
                        else { return Err(format!("{}:{}: an attribute's value is in double quotes or braces: `class=\"x\"`, `checked={{done}}`", tl, tc)); }
                    }
                    continue;
                }
                return Err(format!("{}:{}: unexpected `{}` in a tag", tl, tc, ch));
            }
            Mode::AttrQ => {
                if ch == '"' { i += 1; out.push(Tok::HQ); modes.pop(); }
                else if ch == '{' { i += 1; out.push(Tok::Sym("{".into())); modes.push(Mode::Code { depth: 0, html: true }); }
                else {
                    let s = i;
                    while i < c.len() && c[i] != '"' && c[i] != '{' { if c[i] == '\n' { line += 1; line_start = i + 1; } i += 1; }
                    out.push(Tok::HText(c[s..i].iter().collect()));
                }
                continue;
            }
            Mode::Code { .. } => {}
        }
        // code: braces counted, so the `}` that ends code inside markup returns to the markup
        if ch == '{' { if let Some(Mode::Code { depth, .. }) = modes.last_mut() { *depth += 1; } out.push(Tok::Sym("{".into())); i += 1; continue; }
        if ch == '}' {
            out.push(Tok::Sym("}".into())); i += 1;
            if let Some(Mode::Code { depth, html }) = modes.last_mut() { if *depth == 0 && *html { modes.pop(); } else { *depth = depth.saturating_sub(1); } }
            continue;
        }
        // markup starts at `<` and a letter (or `<!doctype`) where no expression can be followed by `<`: after `(`, `,`, `{`, `;`, `}` (a
        // block's end: `if !exists(c) { return; } <div>...`), or other markup
        if ch == '<' && c.get(i + 1).map_or(false, |x| x.is_ascii_alphabetic() || *x == '!')
            && matches!(out.last(), Some(Tok::Sym(s)) if s == "(" || s == "," || s == "{" || s == ";" || s == "}") || (ch == '<' && c.get(i + 1).map_or(false, |x| x.is_ascii_alphabetic()) && matches!(out.last(), Some(Tok::HClose(_) | Tok::HEnd(_)))) {
            modes.push(Mode::Content { depth: 0 });
            continue;
        }
        if ch == '\n' { line += 1; line_start = i + 1; }
        if ch.is_whitespace() { i += 1; }
        else if ch == '/' && c.get(i + 1) == Some(&'/') { while i < c.len() && c[i] != '\n' { i += 1; } }
        else if ch.is_alphabetic() || ch == '_' {
            let s = i; while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') { i += 1; }
            let id: String = c[s..i].iter().collect();
            if id.len() > MAX_NAME { return Err(format!("{}:{}: a name longer than {} bytes", tl, tc, MAX_NAME)); }
            out.push(Tok::Id(id));
        }
        else if ch == '0' && matches!(c.get(i + 1), Some('x') | Some('X')) && c.get(i + 2).map_or(false, |d| d.is_ascii_hexdigit()) {
            // 0x...: a hexadecimal integer, at most i64::MAX
            i += 2; let s = i; while i < c.len() && c[i].is_ascii_hexdigit() { i += 1; }
            let digits: String = c[s..i].iter().collect();
            out.push(Tok::Int(i64::from_str_radix(&digits, 16).map_err(|_| "number too large".to_string())?));
        }
        else if ch.is_ascii_digit() {
            let s = i; while i < c.len() && c[i].is_ascii_digit() { i += 1; }
            if c.get(i) == Some(&'.') && c.get(i + 1).map_or(false, |d| d.is_ascii_digit()) { i += 1; while i < c.len() && c[i].is_ascii_digit() { i += 1; } out.push(Tok::Flt(c[s..i].iter().collect::<String>().parse().unwrap())); }
            else { out.push(Tok::Int(c[s..i].iter().collect::<String>().parse().map_err(|_| "number too large".to_string())?)); }
        }
        else if ch == '"' {
            i += 1; let mut s = String::new(); let open = i;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' && i + 1 < c.len() {
                    // the escapes: \n \t \r \0 \" \\ \' and \u{HEX} (a code point); any other is an error, not its letter
                    i += 1;
                    match c[i] {
                        'n' => s.push('\n'), 't' => s.push('\t'), 'r' => s.push('\r'), '0' => s.push('\0'), '"' | '\\' | '\'' => s.push(c[i]),
                        'u' if c.get(i + 1) == Some(&'{') => {
                            let close = c[i + 2..].iter().position(|x| *x == '}').map(|k| i + 2 + k);
                            let hex: String = match close { Some(e) => c[i + 2..e].iter().collect(), None => String::new() };
                            let ch = u32::from_str_radix(&hex, 16).ok().filter(|_| !hex.is_empty() && hex.len() <= 6).and_then(char::from_u32);
                            match (close, ch) { (Some(e), Some(ch)) => { s.push(ch); i = e; } _ => return Err(format!("{}:{}: `\\u{{...}}` in a text takes 1 to 6 hex digits of a Unicode scalar value", tl, tc)) }
                        }
                        o => return Err(format!("{}:{}: `\\{}` is no escape in a text: write `\\n`, `\\t`, `\\r`, `\\0`, `\\\"`, `\\\\`, or `\\u{{HEX}}`", tl, tc, o)),
                    }
                } else { s.push(c[i]); }
                i += 1;
            }
            if i >= c.len() { return Err(format!("{}:{}: a text that is never closed (a `\"` is missing)", tl, tc)); }
            if let Some(k) = c[..i].iter().rposition(|x| *x == '\n') { if k + 1 > line_start { line_start = k + 1; } }
            line += c[open..i].iter().filter(|x| **x == '\n').count() as u32; // the source's lines, not a `\n` escape's
            i += 1; out.push(Tok::Str(s));
        }
        else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            if ["==", "!=", "<=", ">=", "&&", "||", "->", "..", "+=", "-=", "*=", "/=", "<<", ">>", "=>"].contains(&two.as_str()) { out.push(Tok::Sym(two)); i += 2; }
            else { out.push(Tok::Sym(ch.to_string())); i += 1; }
        }
    }
    while at.len() < out.len() { at.push((tl, tc)); }
    match modes.last() {
        Some(Mode::Code { html: false, .. }) => {}
        _ => return Err(format!("{}:{}: markup that is never closed (a closing tag, `\"` or `}}` is missing)", tl, tc)),
    }
    Ok((out, at))
}

/// `pre`: statements an initializer (`coll.add() { f: v }`) puts before the statement it is in; `init_ok`: whether
/// an initializer may appear here (not in a condition, a loop's clauses or a JSON array, where it would run at a
/// different time); `tmp`: hidden locals made so far.
/// `markup`: inside code in markup (`{if ...}`), where a bare string statement writes itself: Some(false) in content, Some(true) in an attribute.
/// Keys besides a letter or a digit that `from key("...")` names (the game platform's).
pub const GAME_KEYS: [&str; 7] = ["space", "up", "down", "left", "right", "shift", "escape"];
/// The sized number types of code, by the code `__mito__in` is given: a value kept in a `let`, `var`, parameter or result of such a type is tested against its range
/// (a value outside fails the request with 400; the range analysis drops the test where it proves the value inside). 6 is `f32`: a value rounded to a float. `i64` and `f64` are the
/// plain kinds and need no test; `u64` is not a value in code yet.
pub const NUM_TYPES: [(&str, i64, i64); 6] = [("i8", -128, 127), ("i16", -32768, 32767), ("i32", -2147483648, 2147483647), ("u8", 0, 255), ("u16", 0, 65535), ("u32", 0, 4294967295)];
fn num_code(t: &str) -> Option<u8> {
    if t == "f32" { return Some(6); }
    NUM_TYPES.iter().position(|x| x.0 == t).map(|k| k as u8)
}
/// `e` as a value of the typed kind `code` (a name for the message): the check, or the rounding.
fn typed_expr(e: Expr, code: u8, name: &str) -> Expr {
    if code == 6 { return Expr::Call("__mito__f32".into(), vec![e], CallRes::Unbound); }
    Expr::Call("__mito__in".into(), vec![e, Expr::Int(code as i64), Expr::Str(name.to_string())], CallRes::Unbound)
}
struct P { t: Vec<Tok>, i: usize, lines: Vec<(u32, u32)>, pre: Vec<Stmt>, init_ok: bool, tmp: u32, in_endpoint: bool, in_test: bool, markup: Option<bool>, typed: Vec<(String, u8)>, ret_code: Option<u8> }
type R<T> = Result<T, String>;

pub fn parse(src: &str) -> R<Api> {
    let (t, lines) = lex_lines(src)?;
    let mut p = P { t, i: 0, lines, pre: Vec::new(), init_ok: true, tmp: 0, in_endpoint: false, in_test: false, markup: None, typed: Vec::new(), ret_code: None };
    let mut api = Api::default();
    parse_items(&mut p, &mut api).map_err(|e| {
        if starts_with_place(&e) { return e; }
        // a missing `;` or `}` belongs at the end of what came before, not at the next token (often on the next line)
        let k = if e.starts_with("expected `;`") || e.starts_with("missing `}`") { p.i.saturating_sub(1) } else { p.i.min(p.t.len().saturating_sub(1)) };
        let (l, c) = p.lines.get(k).copied().unwrap_or((1, 1));
        let c = if k < p.i { c + tok_width(&p.t[k]) } else { c };
        format!("{}:{}: {}", l, c, e)
    })?;
    Ok(api)
}
/// Does an error already start with `line:col: `?
pub fn starts_with_place(e: &str) -> bool {
    let mut parts = e.splitn(3, ':');
    matches!((parts.next(), parts.next()), (Some(a), Some(b)) if !a.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && !b.is_empty() && b.chars().all(|c| c.is_ascii_digit()))
}
/// The characters a token takes in the source (a text's escapes counted as written once).
fn tok_width(t: &Tok) -> u32 {
    match t { Tok::Id(x) | Tok::Sym(x) => x.chars().count() as u32, Tok::Str(x) => x.chars().count() as u32 + 2, Tok::Int(n) => n.to_string().len() as u32, Tok::Flt(f) => f.to_string().len() as u32,
        Tok::HOpen(x) => x.chars().count() as u32 + 1, Tok::HAttr(x) | Tok::HText(x) => x.chars().count() as u32, Tok::HClose(x) => x.chars().count() as u32 + 3, Tok::HEnd(s) => 1 + *s as u32, Tok::HQ => 1 }
}
fn parse_items(p: &mut P, api: &mut Api) -> R<()> {
    while p.i < p.t.len() {
        let public = p.is_kw("pub");
        if public { p.i += 1; if !p.is_kw("fn") { return Err(format!("expected `fn` after `pub`, found {}", p.here())); } }
        if p.is_kw("fn") {
            p.i += 1;
            let mut name = p.id()?;
            while p.sym(".") { name = format!("{}.{}", name, p.id()?); } // instance.function, from module expansion
            p.eat("(")?;
            let (mut params, mut ptys) = (Vec::new(), Vec::new());
            while !p.sym(")") { params.push(p.id()?); ptys.push(if p.sym(":") { Some(p.pty()?) } else { None }); p.sym(","); }
            let ret = if p.sym("->") { Some(p.pty()?) } else { None };
            let html = ret.as_ref().map_or(false, |t| t.name == "html" && t.arg.is_none() && !t.opt);
            let ret = if html { None } else { ret };
            let ret_opt = ret.as_ref().map_or(false, |t| t.opt);
            // sized number types in the signature: an integer parameter is tested on entry (a value outside its range fails), a result on return; an f32 is rounded where it is stored or returned
            let mark = p.typed.len();
            let mut entry = Vec::new();
            for (n, t) in params.iter().zip(ptys.iter()) {
                let c = t.as_ref().filter(|t| t.arg.is_none() && t.field.is_none() && t.coll.is_none() && !t.opt).and_then(|t| num_code(&t.name));
                p.typed.push((n.clone(), c.unwrap_or(255)));
                if let Some(c) = c.filter(|c| *c != 6) { entry.push(Stmt::Do(typed_expr(Expr::Var(n.clone(), VarRes::Unbound), c, n))); }
            }
            p.ret_code = ret.as_ref().filter(|t| t.arg.is_none() && t.field.is_none() && t.coll.is_none() && !t.opt).and_then(|t| num_code(&t.name));
            let body = p.block();
            p.ret_code = None;
            p.typed.truncate(mark);
            let mut body = body?;
            if !entry.is_empty() { entry.append(&mut body); body = entry; }
            api.fns.push(Func { test: None, name, params, ptys, ret, body, slots: 0, public, ret_opt, html });
            continue;
        }
        // `test "name" { ... }`: a function the test runner calls; inside, `get`, `post`, `put`, `patch`, `delete` make requests
        if p.is_kw("test") {
            p.i += 1;
            let title = match p.t.get(p.i) { Some(Tok::Str(s)) => { p.i += 1; s.clone() } _ => return Err(format!("a test is named by a string: `test \"what it checks\" {{ ... }}`, found {}", p.here())) };
            if api.fns.iter().any(|f| f.test.as_deref() == Some(title.as_str())) { return Err(format!("two tests named \"{}\"", title)); }
            p.in_test = true;
            let body = p.block();
            p.in_test = false;
            let n = api.fns.iter().filter(|f| f.test.is_some()).count();
            api.fns.push(Func { test: Some(title), name: format!("__mito__test{}", n), params: Vec::new(), ptys: Vec::new(), ret: None, body: body?, slots: 0, public: false, ret_opt: false, html: false });
            continue;
        }
        if p.is_kw("endpoint") { return Err("endpoints are functions with an attribute: `@post(\"/path\") fn name() -> Response<T> { ... }`".into()); }
        // `@get("/path") fn name() -> Response<T> { ... }`
        let first = p.lines.get(p.i).map_or(0, |x| x.0);
        p.eat("@")?;
        let verb = p.id()?;
        // `@start fn name() { ... }`: run once when the program starts (after its tables open, before any request; in tests, before
        // each test), as a request with no inputs whose writes are kept; for what the program derives from its environment
        if verb == "start" {
            if api.eps.iter().any(|e| e.method == "START") { return Err("a program has one `@start` function".into()); }
            p.kw("fn")?;
            let name = p.id()?;
            p.eat("(")?;
            if !p.sym(")") { return Err(format!("`@start fn {}()` takes no inputs", name)); }
            if p.sym("->") { return Err(format!("`@start fn {}()` returns nothing: it is not an answer to a request", name)); }
            p.in_endpoint = true;
            let body = p.block();
            p.in_endpoint = false;
            let mut body = body?;
            body.push(Stmt::Respond(204, Json::Empty));
            let last = p.lines.get(p.i.saturating_sub(1)).map_or(first, |x| x.0);
            api.eps.push(Endpoint { method: "START".into(), segs: Vec::new(), body, text: "START".into(), slots: 0, lines: (first, last), name, resp: RType::Name("null".into()), inputs: Vec::new() });
            continue;
        }
        // `@every(3000) fn tick() { ... }`: run every 3000 ms while the server runs (one such function), as a request without inputs; for
        // what is sent to sockets on a clock (Action Cable's pings)
        if verb == "every" {
            if api.eps.iter().any(|e| e.method == "EVERY") { return Err("a program has one `@every` function".into()); }
            p.eat("(")?;
            let ms = p.int()?;
            if !(10..=86_400_000).contains(&ms) { return Err(format!("`@every({})`: from 10 ms to a day", ms)); }
            p.eat(")")?;
            p.kw("fn")?;
            let name = p.id()?;
            p.eat("(")?;
            if !p.sym(")") { return Err(format!("`@every fn {}()` takes no inputs", name)); }
            if p.sym("->") { return Err(format!("`@every fn {}()` returns nothing", name)); }
            p.in_endpoint = true;
            let body = p.block();
            p.in_endpoint = false;
            let mut body = body?;
            body.push(Stmt::Respond(204, Json::Empty));
            let last = p.lines.get(p.i.saturating_sub(1)).map_or(first, |x| x.0);
            api.eps.push(Endpoint { method: "EVERY".into(), segs: Vec::new(), body, text: format!("EVERY {}", ms), slots: 0, lines: (first, last), name, resp: RType::Name("null".into()), inputs: Vec::new() });
            continue;
        }
        // `@socket("/cable")`: a WebSocket upgrade to that path (a 2xx answer accepts it); `@socket_message("/cable")` each text message on
        // such a socket, `@socket_close("/cable")` its end; they read `:socket`, `:tag` and `:message` as headers (docs: DESIGN.md, Sockets)
        let method = match verb.as_str() {
            "get" | "post" | "put" | "patch" | "delete" => verb.to_uppercase(),
            "socket" => "SOCKET".into(), "socket_message" => "SOCKETMSG".into(), "socket_close" => "SOCKETCLOSE".into(),
            // `@main fn name(filter: ascii<1..200> from arg(0), json: utf8<1000000> from stdin) -> Response<text>`: a command-line program's one
            // function. The command line is its request: `from arg(N)` the Nth argument, `from stdin` what was piped in, any other input
            // the flag `--name value`; the answer's body goes to stdout and a status of 400 or more makes the exit code 1 (runtime/cli.c)
            "main" => "MAIN".into(),
            // `@frame fn frame(dt: f64 from time, fwd: bool from key("w"), turn: f64 from mouse("dx")) -> Response<text>`: a game's function, called every frame with
            // what the window saw (`from time` the seconds since the last frame, `from key("w")` whether a key is down, `from mouse("dx")` the pointer's movement since the
            // last frame); its answer is the frame: the name of a shader and its uniforms as `name=value` words (runtime/game.c). `@shader fn terrain() -> Response<text>` gives
            // one shader's source (a fragment shader drawn over the whole window), asked for once when the game starts.
            "frame" => "FRAME".into(), "shader" => "SHADER".into(),
            v => return Err(format!("`@{}`: an endpoint is `@get`, `@post`, `@put`, `@patch`, `@delete`, `@socket`, `@socket_message`, `@socket_close`, `@main`, `@frame` or `@shader`", v)),
        };
        let mut segs = if matches!(method.as_str(), "MAIN" | "FRAME" | "SHADER") {
            if method != "SHADER" && api.eps.iter().any(|e| e.method == method) { return Err(format!("a program has one `@{}` function", verb)); }
            Vec::new()
        } else {
            p.eat("(")?;
            let path = match p.t.get(p.i) { Some(Tok::Str(s)) => { p.i += 1; s.clone() } _ => return Err(format!("`@{}(...)` takes the path as a string, as in `@{}(\"/players/{{p}}\")`", verb, verb)) };
            p.eat(")")?;
            path_segs(&path)?
        };
        p.kw("fn")?;
        let name = p.id()?;
        // a shader is asked for by its name
        if method == "SHADER" {
            if api.eps.iter().any(|e| e.method == "SHADER" && e.name == name) { return Err(format!("two shaders named `{}`", name)); }
            segs = vec![Seg::Lit(name.clone())];
        }
        p.eat("(")?;
        let mut inputs: Vec<Input> = Vec::new();
        while !p.sym(")") { let i = p.input(&method, &segs)?; if inputs.iter().any(|x| x.name == i.name) { return Err(format!("endpoint `{}`: two inputs named `{}`", name, i.name)); } inputs.push(i); p.sym(","); }
        for s in &segs { if let Seg::Param(_, n) = s { if !inputs.iter().any(|i| i.name == *n && i.src == Src::Path) { return Err(format!("endpoint `{}`: the path's `{{{}}}` is not one of its inputs: `fn {}({}: ...)`", name, n, name, n)); } } }
        if !(p.sym("->") && p.is_kw("Response")) { return Err(format!("endpoint `{}` returns a `Response<T>`: `fn {}() -> Response<T>`", name, name)); }
        p.i += 1; p.eat("<")?; let resp = p.rtype()?; p.eat(">")?;
        p.in_endpoint = true;
        let body = p.block();
        p.in_endpoint = false;
        let body = body?;
        let text = format!("{} {}", method, segs.iter().map(|s| match s { Seg::Lit(l) => format!("/{}", l), Seg::Param(pre, n) => format!("/{}{{{}}}", pre, n) }).collect::<String>());
        let last = p.lines.get(p.i.saturating_sub(1)).map_or(first, |x| x.0);
        api.eps.push(Endpoint { method, segs, body, text, slots: 0, lines: (first, last), name, resp, inputs });
    }
    Ok(())
}

/// `/rooms/{room}/messages` as segments.
fn path_segs(path: &str) -> R<Vec<Seg>> {
    let Some(rest) = path.strip_prefix('/') else { return Err(format!("an endpoint path starts with `/`: `{}`", path)) };
    let mut segs = Vec::new();
    for part in rest.split('/').filter(|x| !x.is_empty()) {
        let ok = |n: &str| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.');
        // a parameter, after a literal prefix of name characters or `@`, `~`, `+` (`/rooms/{room}/@{message}`)
        let open = part.find('{');
        let pre = open.map_or("", |k| &part[..k]);
        let pre_ok = pre.chars().all(|c| c.is_alphanumeric() || "_-.@~+".contains(c));
        match open.and_then(|k| part[k + 1..].strip_suffix('}')) {
            Some(n) if ok(n) && pre_ok => segs.push(Seg::Param(pre.to_string(), n.to_string())),
            None if open.is_none() && ok(part) => segs.push(Seg::Lit(part.to_string())),
            _ => return Err(format!("endpoint path `{}`: `{}` is not a name or a `{{parameter}}`", path, part)),
        }
        if segs.len() > 32 { return Err("an endpoint path has at most 32 parts".into()); }
    }
    Ok(segs)
}

impl P {
    /// An endpoint input: `[forwarded] name: TYPE [?] [= default] [from header("H") | from query | from body]`.
    fn input(&mut self, method: &str, segs: &[Seg]) -> R<Input> {
        let forwarded = if self.is_kw("forwarded") { self.i += 1; true } else { false };
        let name = self.id()?; self.eat(":")?;
        let tn = self.id()?;
        let range = |p: &mut Self, lo: i64, hi: i64| -> R<(i64, i64)> {
            if !p.is_kw("in") { return Ok((lo, hi)); }
            p.i += 1;
            let a = p.signed()?; p.eat("..")?; let b = p.signed()?;
            if a > b || a < lo || b > hi { return Err(format!("input `{}`: range {}..{} does not fit its type", name, a, b)); }
            Ok((a, b))
        };
        let ty = match tn.as_str() {
            "i8" => { let (a, b) = range(self, i8::MIN as i64, i8::MAX as i64)?; InTy::Int(a, b) }
            "i16" => { let (a, b) = range(self, i16::MIN as i64, i16::MAX as i64)?; InTy::Int(a, b) }
            "i32" => { let (a, b) = range(self, i32::MIN as i64, i32::MAX as i64)?; InTy::Int(a, b) }
            "i64" => { let (a, b) = range(self, i64::MIN, i64::MAX)?; InTy::Int(a, b) }
            "u8" => { let (a, b) = range(self, 0, u8::MAX as i64)?; InTy::Int(a, b) }
            "u16" => { let (a, b) = range(self, 0, u16::MAX as i64)?; InTy::Int(a, b) }
            "u32" => { let (a, b) = range(self, 0, u32::MAX as i64)?; InTy::Int(a, b) }
            "u64" => { let (a, b) = range(self, 0, i64::MAX)?; InTy::Int(a, b) }
            "f32" | "f64" => InTy::Flt,
            "bool" => InTy::Bool,
            "string" | "utf8" | "ascii" => {
                let ascii = tn == "ascii";
                self.eat("<")?;
                let a = self.int()?;
                let (lo, hi) = if self.sym("..") { (a, self.int()?) } else { (0, a) };
                self.eat(">")?;
                if lo > hi || lo < 0 { return Err(format!("input `{}`: string<{}..{}> is not a length range", name, lo, hi)); }
                InTy::Str(lo, hi, ascii)
            }
            t => { let pool = if self.is_kw("in") { self.i += 1; let mut p = self.id()?; while self.sym(".") { p = format!("{}.{}", p, self.id()?); } Some(p) } else { None }; InTy::Item(t.to_string(), pool) }
        };
        let optional = self.sym("?");
        let default = if self.sym("=") { Some(match self.t.get(self.i).cloned() { Some(Tok::Int(n)) => { self.i += 1; Expr::Int(n) } Some(Tok::Sym(m)) if m == "-" => { self.i += 1; Expr::Int(-self.int()?) } Some(Tok::Flt(f)) => { self.i += 1; Expr::Float(f) } Some(Tok::Str(s)) => { self.i += 1; Expr::Str(s) } Some(Tok::Id(b)) if b == "true" || b == "false" => { self.i += 1; Expr::Bool(b == "true") } _ => return Err(format!("input `{}`: a default is a literal", name)) }) } else { None };
        let in_path = segs.iter().any(|s| matches!(s, Seg::Param(_, n) if *n == name));
        let src = if self.is_kw("from") {
            self.i += 1;
            if self.is_kw("query") { self.i += 1; Src::Query }
            else if self.is_kw("body") { self.i += 1; Src::Body }
            // a command-line program's input: the Nth argument, or what was piped in (named as headers, as `:socket` is)
            else if self.is_kw("stdin") { self.i += 1; Src::Header(":stdin".into()) }
            // a game's inputs: the seconds since the last frame, whether a key is down, how far the pointer moved
            else if self.is_kw("time") { self.i += 1; Src::Header(":dt".into()) }
            else if self.is_kw("key") {
                self.i += 1; self.eat("(")?;
                let k = match self.t.get(self.i) { Some(Tok::Str(s)) => { self.i += 1; s.clone() } _ => return Err(format!("input `{}`: `from key(\"w\")`", name)) };
                self.eat(")")?;
                if !(k.len() == 1 && k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())) && !GAME_KEYS.contains(&k.as_str()) { return Err(format!("input `{}`: `{}` is not a key the game platform reads: a letter, a digit, {}", name, k, GAME_KEYS.join(", "))); }
                Src::Header(format!(":key_{}", k))
            }
            else if self.is_kw("mouse") {
                self.i += 1; self.eat("(")?;
                let k = match self.t.get(self.i) { Some(Tok::Str(s)) => { self.i += 1; s.clone() } _ => return Err(format!("input `{}`: `from mouse(\"dx\")` (dx or dy)", name)) };
                self.eat(")")?;
                if k != "dx" && k != "dy" { return Err(format!("input `{}`: `from mouse(\"dx\")` or `\"dy\"`, the pointer's movement since the last frame", name)); }
                Src::Header(format!(":mouse_{}", k))
            }
            else if self.is_kw("arg") {
                self.i += 1; self.eat("(")?;
                let n = self.int()?;
                self.eat(")")?;
                if !(0..=31).contains(&n) { return Err(format!("input `{}`: `from arg(N)` takes N from 0 to 31", name)); }
                Src::Header(format!(":arg{}", n))
            }
            else if self.is_kw("cookie") {
                self.i += 1; self.eat("(")?;
                let c = match self.t.get(self.i) { Some(Tok::Str(s)) => { self.i += 1; s.clone() } _ => return Err(format!("input `{}`: `from cookie(\"name\")`", name)) };
                self.eat(")")?;
                if !cookie_name_ok(&c) { return Err(format!("input `{}`: `{}` is not a cookie name (letters, digits and !#$%&'*+-.^_`|~)", name, c)); }
                Src::Cookie(c)
            }
            else { self.kw("header")?; self.eat("(")?; let h = match self.t.get(self.i) { Some(Tok::Str(s)) => { self.i += 1; s.clone() } _ => return Err(format!("input `{}`: `from header(\"Name\")`, `from cookie(\"name\")`, `from query` or `from body`", name)) }; self.eat(")")?; Src::Header(h) }
        } else if forwarded { Src::Fwd } else if in_path { Src::Path } else if matches!(method, "GET" | "DELETE" | "MAIN" | "FRAME" | "SHADER") { Src::Query } else { Src::Body };
        if in_path && src != Src::Path { return Err(format!("input `{}` is named in the path, so it comes from the path", name)); }
        if let InTy::Item(..) = ty { if default.is_some() || (optional && !matches!(src, Src::Cookie(_))) { return Err(format!("input `{}`: an item cannot have a default, and only one from a cookie can be optional (null when absent or not valid)", name)); } }
        if optional && !matches!(ty, InTy::Str(..) | InTy::Item(..)) { return Err(format!("input `{}`: only a string can be optional (null when absent); give it a default instead", name)); }
        match (&ty, &default) {
            (_, None) | (InTy::Int(..), Some(Expr::Int(_))) | (InTy::Flt, Some(Expr::Float(_) | Expr::Int(_))) | (InTy::Bool, Some(Expr::Bool(_))) | (InTy::Str(..), Some(Expr::Str(_))) => {}
            _ => return Err(format!("input `{}`: the default is not of its type", name)),
        }
        if let (InTy::Int(lo, hi), Some(Expr::Int(d))) = (&ty, &default) { if d < lo || d > hi { return Err(format!("input `{}`: the default {} is outside {}..{}", name, d, lo, hi)); } }
        if matches!(ty, InTy::Item(..)) && !matches!(src, Src::Path | Src::Header(_) | Src::Cookie(_)) { return Err(format!("input `{}`: an item comes from the path (an id), a header or a cookie (a session token)", name)); }
        if let Src::Header(h) = &src { if matches!(ty, InTy::Item(..)) && !h.eq_ignore_ascii_case("X-Session") { return Err(format!("input `{}`: a session item comes from `header(\"X-Session\")`", name)); } }
        Ok(Input { name, src, ty, default, optional })
    }
    fn signed(&mut self) -> R<i64> { if self.sym("-") { Ok(-self.int()?) } else { self.int() } }

    /// The `T` of `Response<T>`: `{ name: T, ... }`, `[T]`, `T?`, or a name (`i64`, `string<24>`, `Row`, `Player`, `none`).
    fn rtype(&mut self) -> R<RType> {
        let t = if self.sym("{") {
            let mut f = Vec::new();
            while !self.sym("}") { let k = self.key()?; self.eat(":")?; f.push((k, self.rtype()?)); if !self.sym(",") { self.sym(";"); } }
            RType::Obj(f)
        } else if self.sym("[") { let e = self.rtype()?; self.eat("]")?; RType::Arr(Box::new(e)) }
        else {
            let n = self.id()?;
            if self.sym("<") { let mut depth = 1; while depth > 0 { if self.sym("<") { depth += 1; } else if self.sym(">") { depth -= 1; } else if self.peek().is_none() { return Err("unfinished type".into()); } else { self.i += 1; } } }
            RType::Name(n)
        };
        Ok(if self.sym("?") { RType::Opt(Box::new(t)) } else { t })
    }
    fn peek(&self) -> Option<&Tok> { self.t.get(self.i) }
    fn here(&self) -> String { describe(self.peek()) }
    fn sym(&mut self, s: &str) -> bool { if matches!(self.peek(), Some(Tok::Sym(x)) if x == s) { self.i += 1; true } else { false } }
    fn eat(&mut self, s: &str) -> R<()> { if self.sym(s) { Ok(()) } else { Err(format!("expected `{}`, found {}", s, self.here())) } }
    fn is_kw(&self, k: &str) -> bool { matches!(self.peek(), Some(Tok::Id(x)) if x == k) }
    fn kw(&mut self, k: &str) -> R<()> { if self.is_kw(k) { self.i += 1; Ok(()) } else { Err(format!("expected `{}`, found {}", k, self.here())) } }
    fn id(&mut self) -> R<String> { match self.t.get(self.i) { Some(Tok::Id(x)) => { self.i += 1; Ok(x.clone()) } _ => Err(format!("expected a name, found {}", self.here())) } }
    fn int(&mut self) -> R<i64> { match self.t.get(self.i) { Some(Tok::Int(n)) => { self.i += 1; Ok(*n) } _ => Err(format!("expected a number, found {}", self.here())) } }

    /// A signature's type: `name`, `name.field`, `name in a.b`, `name<...>` (read, not used yet), then `?`.
    fn pty(&mut self) -> R<PTy> {
        let name = self.id()?;
        let field = if self.sym(".") { Some(self.id()?) } else { None };
        let mut arg = None;
        if self.sym("<") { if let Some(Tok::Id(a)) = self.peek() { arg = Some(a.clone()); } let mut depth = 1; while depth > 0 { if self.sym("<") { depth += 1; } else if self.sym(">") { depth -= 1; } else if self.peek().is_none() { return Err("unfinished type".into()); } else { self.i += 1; } } }
        if self.sym("[") { return Err(format!("a fixed array as a parameter or result (`{}[...]`) is not supported: pass the item or plain struct that holds it", name)); }
        let coll = if self.is_kw("in") { self.i += 1; let mut path = self.id()?; while self.sym(".") { path = format!("{}.{}", path, self.id()?); } Some(path) } else { None };
        let opt = self.sym("?");
        Ok(PTy { name, arg, field, coll, opt })
    }

    fn block(&mut self) -> R<Vec<Stmt>> {
        self.eat("{")?;
        let mut v = Vec::new();
        let typed_mark = self.typed.len();
        let r = self.block_items(&mut v);
        self.typed.truncate(typed_mark);
        r?;
        Ok(v)
    }
    fn block_items(&mut self, v: &mut Vec<Stmt>) -> R<()> {
        while !self.sym("}") {
            if self.peek().is_none() { return Err("missing `}`".into()); }
            let (l, c) = self.lines.get(self.i).copied().unwrap_or((0, 0));
            let st = self.stmt()?;
            v.push(Stmt::At(l, c));
            v.append(&mut self.pre);
            v.push(st);
        }
        Ok(())
    }

    /// Parse with initializers refused (they would run at another time than they are written).
    fn no_init<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> { let was = self.init_ok; self.init_ok = false; let r = f(self); self.init_ok = was; r }

    fn iter(&mut self) -> R<Iter> { self.no_init(|p| p.iter_inner()) }
    /// `item[, i] in coll [from k] [by field] [after x] [reverse] [while c] [scan] [where c] [limit n | last n]`, or `i in lo..hi` (both ends included).
    fn iter_inner(&mut self) -> R<Iter> {
        let var = self.id()?;
        let index = if self.sym(",") { Some((self.id()?, 0)) } else { None };
        self.kw("in")?; let coll = self.expr()?;
        // internally the end is excluded: `lo..e - 1` is lo..e, a number n is n + 1, anything else e + 1
        let hi = if self.sym("..") { Some(match self.expr()? {
            Expr::Bin(op, a, b) if op == "-" && matches!(*b, Expr::Int(1)) => *a,
            Expr::Int(n) => Expr::Int(n + 1),
            e => Expr::Bin("+".into(), Box::new(e), Box::new(Expr::Int(1))),
        }) } else { None };
        let from = if self.is_kw("from") { self.i += 1; Some(self.expr()?) } else { None };
        // `by f`, `by lower(f)` (ASCII letters lowered, as SQLite's LOWER), `by f.g` across a reference (and `by lower(f.g)`)
        let by = if self.is_kw("by") {
            self.i += 1;
            let lower = matches!(self.peek(), Some(Tok::Id(n)) if n == "lower") && matches!(self.t.get(self.i + 1), Some(Tok::Sym(s)) if s == "(");
            if lower { self.i += 2; }
            let mut path = self.id()?;
            while self.sym(".") { path = format!("{}.{}", path, self.id()?); }
            if lower { self.eat(")")?; path = format!("lower({})", path); }
            Some(path)
        } else { None };
        let after = if self.is_kw("after") { self.i += 1; Some(self.expr()?) } else { None };
        let reverse = if self.is_kw("reverse") { self.i += 1; true } else { false };
        let whil = if self.is_kw("while") { self.i += 1; Some(self.expr()?) } else { None };
        let scan = if self.is_kw("scan") { self.i += 1; true } else { false };
        let wher = if self.is_kw("where") { self.i += 1; Some(self.expr()?) } else { None };
        if scan && wher.is_none() { return Err("`scan` accepts that a `where` reads every row of a table: `foreach u in app.users scan where u.role == 1`".into()); }
        if self.is_kw("order") { return Err("`order by` was removed: keep the data in order with a sorted collection (lib/sorted.mito)".into()); }
        // `limit n`: the first n; `last n`: the last n, in the same order (a table loop: the newest page of messages)
        let last = self.is_kw("last");
        let limit = if self.is_kw("limit") || last { self.i += 1; Some(self.expr()?) } else { None };
        Ok(Iter { index, budget: false, var, slot: 0, coll, hi, from, reverse, by, after, whil, wher, limit, via: None, scan, last })
    }

    /// Markup (an HTML literal), lowered to ordinary statements that write it into the answer: its fixed text as `__html_lit("...")`
    /// (each run merged), `{x}` as `__html_esc(x)` (escaped), an attribute's `{x}` parts as `__html_esc_attr(x)`, `name={x}` as
    /// `__html_attr("name", x)` (a bool writes the bare attribute when true), and `{if ...}` / `{foreach ...}` as the statements they are.
    fn html(&mut self) -> R<Vec<Stmt>> {
        let (mut out, mut lit) = (Vec::new(), String::new());
        if let Some(Tok::HText(t)) = self.peek() { let t = t.clone(); self.i += 1; lit.push_str(&t); }
        self.html_elem(&mut out, &mut lit)?;
        flush_lit(&mut out, &mut lit);
        Ok(out)
    }
    fn html_elem(&mut self, out: &mut Vec<Stmt>, lit: &mut String) -> R<()> {
        let call = |n: &str, a: Vec<Expr>| Stmt::Do(Expr::Call(n.into(), a, CallRes::Unbound));
        let Some(Tok::HOpen(tag)) = self.peek().cloned() else { return Err(format!("expected markup (`<tag`), found {}", self.here())) };
        self.i += 1;
        lit.push('<'); lit.push_str(&tag);
        loop {
            match self.peek().cloned() {
                Some(Tok::HAttr(name)) => {
                    self.i += 1;
                    if matches!(self.peek(), Some(Tok::HQ)) {
                        self.i += 1;
                        lit.push(' '); lit.push_str(&name); lit.push_str("=\"");
                        loop {
                            match self.peek().cloned() {
                                Some(Tok::HQ) => { self.i += 1; break; }
                                Some(Tok::HText(t)) => { self.i += 1; lit.push_str(&t); }
                                Some(Tok::Sym(s)) if s == "{" => { flush_lit(out, lit); self.markup_code(out, true)?; }
                                _ => return Err(format!("unfinished attribute value, found {}", self.here())),
                            }
                        }
                        lit.push('"');
                    } else if self.sym("{") {
                        let e = self.expr()?; self.eat("}")?;
                        flush_lit(out, lit); out.push(call("__html_attr", vec![Expr::Str(name), e]));
                    } else { lit.push(' '); lit.push_str(&name); }
                }
                Some(Tok::HEnd(selfc)) => {
                    self.i += 1;
                    lit.push('>');
                    if VOID.contains(&tag.to_ascii_lowercase().as_str()) { return Ok(()); }
                    if selfc { lit.push_str("</"); lit.push_str(&tag); lit.push('>'); return Ok(()); }
                    break;
                }
                _ => return Err(format!("expected an attribute or the end of `<{}`, found {}", tag, self.here())),
            }
        }
        loop {
            match self.peek().cloned() {
                Some(Tok::HClose(t)) => {
                    self.i += 1;
                    if t != tag { return Err(format!("`</{}>` closes `<{}>`: the tags must match", t, tag)); }
                    lit.push_str("</"); lit.push_str(&tag); lit.push('>');
                    return Ok(());
                }
                Some(Tok::HText(t)) => { self.i += 1; lit.push_str(&t); }
                Some(Tok::HOpen(_)) => self.html_elem(out, lit)?,
                Some(Tok::Sym(s)) if s == "{" => { flush_lit(out, lit); self.markup_code(out, false)?; }
                None => return Err(format!("`<{}>` is never closed", tag)),
                _ => return Err(format!("unexpected {} inside `<{}>`", self.here(), tag)),
            }
        }
    }

    /// `{ ... }` in markup (`attr`: inside an attribute's quotes): statements (`{if ...}`, `{foreach ...}`, `{item(t);}`), in which a bare
    /// string writes itself; or one expression, written escaped.
    /// Is the `{` here a block of statements (`{if ...}`, `{item(t);}`) rather than a value (`{x}`, or a JSON object `{ a: 1 }`)?
    fn code_block_ahead(&self) -> bool {
        // `{ name: ...`: a field, whatever the name (`{ log: app.log }`)
        if matches!((self.t.get(self.i + 1), self.t.get(self.i + 2)), (Some(Tok::Id(_)), Some(Tok::Sym(c))) if c == ":") { return false; }
        match self.t.get(self.i + 1) {
            Some(Tok::Id(k)) if ["if", "foreach", "for", "while", "let", "var", "match", "log"].contains(&k.as_str()) => true,
            // a call as a statement: `{item(t);}`
            _ => { let mut k = self.i + 1; let mut depth = 0i32; let mut semi = false;
                   while let Some(t) = self.t.get(k) { match t { Tok::Sym(s) if s == "(" || s == "{" || s == "[" => depth += 1, Tok::Sym(s) if s == ")" || s == "]" => depth -= 1,
                       Tok::Sym(s) if s == "}" => { if depth == 0 { break; } depth -= 1; } Tok::Sym(s) if s == ";" && depth == 0 => { semi = true; break; } _ => {} } k += 1; }
                   semi }
        }
    }
    fn markup_code(&mut self, out: &mut Vec<Stmt>, attr: bool) -> R<()> {
        let code = self.code_block_ahead();
        if code {
            let was = self.markup.replace(attr);
            let b = self.block();
            self.markup = was;
            out.push(Stmt::If(Expr::Bool(true), b?, Vec::new()));
        } else {
            self.i += 1; let e = self.expr()?; self.eat("}")?;
            out.push(Stmt::Do(Expr::Call(if attr { "__html_esc_attr" } else { "__html_esc" }.into(), vec![e], CallRes::Unbound)));
        }
        Ok(())
    }

    fn stmt(&mut self) -> R<Stmt> {
        // `include_html("fragments/head.html");` in markup: a file of the program's (read when compiling, relative to the program) written as
        // it is, for fixed markup that is not worth writing as code (generated asset lists, another app's templates' output)
        if self.markup != Some(true) && matches!(self.peek(), Some(Tok::Id(n)) if n == "include_html") {
            if let (Some(Tok::Sym(o)), Some(Tok::Str(name)), Some(Tok::Sym(c))) = (self.t.get(self.i + 1).cloned(), self.t.get(self.i + 2).cloned(), self.t.get(self.i + 3).cloned()) {
                if o == "(" && c == ")" { self.i += 4; self.sym(";"); return Ok(Stmt::Do(Expr::Call("__html_lit".into(), vec![Expr::Str(read_file(&name)?)], CallRes::Unbound))); }
            }
            return Err("`include_html` takes the file's path as a literal: `include_html(\"fragments/head.html\");`".into());
        }
        // a bare string writes itself, in code inside markup (`{if left == 1 { "item" } else { "items" }}`) and in a `-> html`
        // function's code (`"/rooms/"; html_text(r.id);`); anywhere else it is an error, as markup is (it would do nothing)
        if let Some(Tok::Str(s)) = self.peek().cloned() {
            if matches!(self.t.get(self.i + 1), Some(Tok::Sym(x)) if x == ";" || (x == "}" && self.markup.is_some())) {
                self.i += 1; self.sym(";");
                let attr = self.markup == Some(true);
                return Ok(Stmt::Do(Expr::Call(if attr { "__html_esc_attr" } else { "__html_esc" }.into(), vec![Expr::Str(s)], CallRes::Unbound)));
            }
        }
        // `{...}` between markup in code inside markup: the same as in the markup, `{if p { <h2>..</h2> {steps(p);} }}`
        if let (Some(attr), Some(Tok::Sym(s))) = (self.markup, self.peek()) {
            if s == "{" {
                let mut v = Vec::new();
                self.markup_code(&mut v, attr)?;
                let last = v.pop().unwrap_or(Stmt::Do(Expr::Null));
                self.pre.extend(v);
                return Ok(last);
            }
        }
        if self.markup.is_none() && matches!(self.peek(), Some(Tok::Sym(s)) if s == "{") {
            return Err("`{` here is code, not markup: write the statement itself (`bell(r);`, `html_text(x);`); `{...}` is for code inside an element".into());
        }
        // markup as a statement (in a `-> html` function, or in `{if ...}` / `{foreach ...}` inside markup): it writes itself
        if matches!(self.peek(), Some(Tok::HOpen(_) | Tok::HText(_))) {
            let was = self.markup.take(); // markup inside resets: its own code says where it is
            let v = self.html();
            self.markup = was;
            let mut v = v?;
            let last = v.pop().unwrap_or(Stmt::Do(Expr::Null));
            self.pre.extend(v);
            return Ok(last);
        }
        if self.is_kw("each") { return Err("`each` is now `foreach`, and a range includes its end: `foreach i in 0..n - 1 { ... }`, `foreach item, i in coll { ... }`".into()); }
        if self.is_kw("foreach") {
            self.i += 1;
            // the head may be in parentheses: `foreach (item in coll) { ... }`
            let paren = self.sym("(");
            let it = self.iter()?;
            if paren { self.eat(")")?; }
            let b = self.block()?;
            return Ok(Stmt::Each(it, b));
        }
        if self.is_kw("while") { self.i += 1; let c = self.no_init(|p| p.expr())?; let b = self.block()?; return Ok(Stmt::Each(unbounded(Some(c)), b)); }
        if self.is_kw("for") { return self.for_stmt(); }
        if self.is_kw("match") { return self.match_stmt(); }
        if self.is_kw("break") { self.i += 1; self.eat(";")?; return Ok(Stmt::Break); }
        if self.is_kw("continue") { self.i += 1; self.eat(";")?; return Ok(Stmt::Continue); }
        if self.is_kw("let") || self.is_kw("var") {
            let is_var = self.is_kw("var");
            self.i += 1; let n = self.id()?;
            // `var acc: text<8192> = "";`: a var holding text of its own, up to its cap (its bytes in its scope's region)
            let mut cap = None;
            let mut num: Option<u8> = None;
            if self.sym(":") {
                let t = self.id()?;
                if t == "u64" { return Err(format!("`{} {}: u64`: u64 is not a value in code yet (it is stored in a field; compute in i64, or in u32 pieces)", if is_var { "var" } else { "let" }, n)); }
                if t == "i64" || t == "f64" { num = Some(255); }
                else if let Some(c) = num_code(&t) { num = Some(c); }
                else if !is_var || t != "text" { return Err(format!("`{} {}: {}`: only a `var` declares a type, and only `text<N>` (its cap in bytes: `var {}: text<1024> = \"\";`)", if is_var { "var" } else { "let" }, n, t, n)); }
                if num.is_none() {
                    self.eat("<")?; let k = self.int()?; self.eat(">")?;
                    if k < 1 || k > 1 << 24 { return Err(format!("`var {}: text<{}>`: the cap is 1 to 16777216 bytes", n, k)); }
                    cap = Some(k);
                }
            }
            self.eat("=")?;
            if self.is_kw("alloc") || self.is_kw("try_alloc") { return Err(format!("`{}` was replaced by collection functions: `let {} = coll.add();` (or `coll.try_add()`)", self.here_word(), n)); }
            let e = self.expr()?; self.eat(";")?;
            let e = match cap { Some(k) => Expr::Call("__mito__text_cap".into(), vec![Expr::Int(k), e], CallRes::Unbound), None => e };
            // a typed number: the first value is tested (or rounded), and so is every later assignment (see the assignment below)
            let e = match num { Some(c) if c != 255 => typed_expr(e, c, &n), _ => e };
            self.typed.push((n.clone(), num.unwrap_or(255)));
            return Ok(if is_var { Stmt::Var(n, 0, e) } else { Stmt::Let(n, 0, e) });
        }
        if self.is_kw("if") { return self.if_stmt(); }
        if self.is_kw("each") { self.i += 1; let it = self.iter()?; let b = self.block()?; return Ok(Stmt::Each(it, b)); }
        if self.is_kw("free") { return Err("`free x` was replaced by the collection's function: `coll.remove(x);`".into()); }
        if self.is_kw("forward") {
            self.i += 1;
            let kind = self.id()?; self.eat("[")?; let key = self.expr()?; self.eat("]")?;
            let mut fields = Vec::new();
            if self.sym("{") {
                while !self.sym("}") {
                    let k = self.key()?;
                    self.eat(":")?; fields.push((k, self.expr()?)); self.sym(",");
                }
            }
            self.eat(";")?;
            return Ok(Stmt::Forward { kind, key, fields });
        }
        if self.is_kw("return") {
            self.i += 1;
            // in an endpoint, the answer: ok(...) 200, created(...) 201, no_content() 204, status(code, ...), see_other(path) 303, found(path) 302
            if self.in_endpoint {
                let how = self.here_word();
                let code: u16 = match how.as_str() { "ok" => 200, "created" => 201, "no_content" => 204, "status" => 0, "see_other" => 303, "found" => 302, _ => return Err("an endpoint returns its answer: `return ok(...)`, `created(...)`, `no_content()`, `status(code, ...)`, `see_other(path)` or `found(path)`".into()) };
                self.i += 1; self.eat("(")?;
                let code = if code == 0 {
                    let c = self.int()?;
                    // a page may be answered with an error status (a form shown again 422, a sign-in refused 401); redirects are `see_other`/`found`
                    if !((200..300).contains(&c) || (400..600).contains(&c)) { return Err(format!("`status({}, ...)`: an answer is 2xx, 4xx or 5xx (a redirect is `see_other(path)` or `found(path)`)", c)); }
                    self.eat(",")?; c as u16
                } else { code };
                let j = if code == 204 { Json::Empty } else if code == 303 || code == 302 { Json::E(self.expr()?) }
                    else if matches!(self.peek(), Some(Tok::HOpen(_) | Tok::HText(_))) { let body = self.html()?; self.pre.extend(body); Json::Html }
                    // an HTML answer written by code alone, for a fragment with no single element around it: `ok({foreach t in ts { item(t); }})`
                    else if matches!(self.peek(), Some(Tok::Sym(x)) if x == "{") && self.code_block_ahead() { let mut body = Vec::new(); self.markup_code(&mut body, false)?; self.pre.extend(body); Json::Html }
                    else { self.json()? };
                self.eat(")")?; self.eat(";")?;
                return Ok(Stmt::Respond(code, j));
            }
            if self.sym(";") { return Ok(Stmt::Return(None)); }
            let e = self.expr()?; self.eat(";")?;
            let e = match self.ret_code { Some(c) if !self.in_endpoint && !self.in_test => typed_expr(e, c, "the result"), _ => e };
            return Ok(Stmt::Return(Some(e)));
        }
        if self.is_kw("fail") {
            self.i += 1; let code = self.int()? as u16;
            let msg = match self.t.get(self.i) { Some(Tok::Str(s)) => { self.i += 1; s.clone() } _ => return Err("fail needs a message string".into()) };
            if msg.len() > MAX_FAIL { return Err(format!("a fail message longer than {} bytes", MAX_FAIL)); }
            self.eat(";")?; return Ok(Stmt::Fail(code, msg));
        }
        if self.is_kw("respond") { return Err("`respond` was replaced by returning the answer: `return ok(...);` (or `created`, `no_content`, `status`)".into()); }
        let st = self.simple()?;
        self.eat(";")?;
        Ok(st)
    }

    /// An expression, `x = e` or `x op= e`, without its `;` (a statement, or a `for` loop's step).
    fn simple(&mut self) -> R<Stmt> {
        if matches!(self.t.get(self.i), Some(Tok::Id(_))) && matches!((self.t.get(self.i + 1), self.t.get(self.i + 2)), (Some(Tok::Sym(a)), Some(Tok::Sym(b))) if a == b && (a == "+" || a == "-")) {
            let (n, op) = (self.here_word(), match self.t.get(self.i + 1) { Some(Tok::Sym(a)) => a.clone(), _ => String::new() });
            return Err(format!("`{}{}{}` does not exist: write `{} {}= 1`", n, op, op, n, op));
        }
        let lhs = self.expr()?;
        // a variable declared with a sized number type keeps its kind: what is assigned to it is tested (or rounded)
        let typed = match &lhs { Expr::Var(n, _) => self.typed.iter().rev().find(|x| x.0 == *n).filter(|x| x.1 != 255).map(|x| (x.0.clone(), x.1)), _ => None };
        let wrap = |e: Expr| match &typed { Some((n, c)) => typed_expr(e, *c, n), None => e };
        // `x += e` is `x = x + e`
        for op in ["+=", "-=", "*=", "/="] {
            if self.sym(op) { let rhs = self.expr()?; return Ok(Stmt::Assign(lhs.clone(), wrap(Expr::Bin(op[..1].to_string(), Box::new(lhs), Box::new(rhs))))); }
        }
        if !self.sym("=") { return Ok(Stmt::Do(lhs)); }
        let rhs = self.expr()?;
        Ok(Stmt::Assign(lhs, wrap(rhs)))
    }

    /// `for [var] i = e; cond; step { ... }` (the head may be in parentheses). `i` is a new variable of the loop. A counted loop
    /// (`i < n` or `i <= n`, step `i += 1`, neither `i` nor `n` assigned in the body, `n` a number or a local) is a range loop;
    /// any other runs as `while cond { body; step }` with `continue` landing before the step.
    fn for_stmt(&mut self) -> R<Stmt> {
        self.kw("for")?;
        let paren = self.sym("(");
        self.sym_kw("var");
        let v = self.id()?;
        self.eat("=")?;
        let init = self.no_init(|p| p.expr())?;
        self.eat(";")?;
        let cond = self.no_init(|p| p.expr())?;
        self.eat(";")?;
        let step = self.no_init(|p| p.simple())?;
        if paren { self.eat(")")?; }
        let body = self.block()?;
        let is_v = |e: &Expr| matches!(e, Expr::Var(n, _) if *n == v);
        let counted = match (&cond, &step) {
            (Expr::Bin(op, a, n), Stmt::Assign(t, Expr::Bin(plus, x, one)))
                if (op == "<" || op == "<=") && is_v(a) && is_v(t) && plus == "+" && is_v(x) && matches!(**one, Expr::Int(1))
                && !assigns_name(&body, &v) && match &**n { Expr::Int(_) => true, Expr::Var(m, _) => *m != v && !assigns_name(&body, m), _ => false } =>
                Some(if op == "<" { (**n).clone() } else if let Expr::Int(k) = **n { Expr::Int(k + 1) } else { Expr::Bin("+".into(), n.clone(), Box::new(Expr::Int(1))) }),
            _ => None,
        };
        if let Some(hi) = counted {
            let it = Iter { index: None, budget: false, var: v, slot: 0, coll: init, hi: Some(hi), from: None, reverse: false, by: None, after: None, whil: None, wher: None, limit: None, via: None, scan: false, last: false };
            return Ok(Stmt::Each(it, body));
        }
        let mut b = body;
        b.push(Stmt::ContinueHere);
        b.push(step);
        // a block of its own (an `if` that always holds) keeps `i` inside the loop
        Ok(Stmt::If(Expr::Bool(true), vec![Stmt::Var(v, 0, init), Stmt::Each(unbounded(Some(cond)), b)], Vec::new()))
    }
    /// `match x { A.b => { } A.c, A.d => { } 0..9 => { } else => { } }`: the subject is read once (a hidden local), each arm is
    /// tested in order, no fall-through. Runs as an `if` chain (clang makes a jump table of it); `MatchArms` keeps the arms for
    /// the binder's checks.
    fn match_stmt(&mut self) -> R<Stmt> {
        self.kw("match")?;
        let subject = self.no_init(|p| p.expr())?;
        self.eat("{")?;
        let (mut arms, mut all, mut otherwise): (Vec<(Vec<Pat>, Vec<Stmt>)>, Vec<Pat>, Option<Vec<Stmt>>) = (Vec::new(), Vec::new(), None);
        while !self.sym("}") {
            if self.peek().is_none() { return Err("missing `}` at the end of a `match`".into()); }
            if otherwise.is_some() { return Err("`else` is the last arm of a `match`".into()); }
            if self.is_kw("else") { self.i += 1; self.eat("=>")?; otherwise = Some(self.block()?); continue; }
            let mut pats = Vec::new();
            loop {
                let a = self.no_init(|p| p.expr())?;
                pats.push(if self.sym("..") { let b = self.no_init(|p| p.expr())?; Pat::Range(a, b) } else { Pat::One(a) });
                if !self.sym(",") { break; }
            }
            self.eat("=>")?;
            let body = self.block()?;
            all.extend(pats.iter().cloned());
            arms.push((pats, body));
        }
        let m = || Expr::Var("__mito__match".into(), VarRes::Unbound);
        let test = |p: &Pat| -> Expr { match p {
            Pat::One(e) => Expr::Bin("==".into(), Box::new(m()), Box::new(e.clone())),
            Pat::Range(a, b) => Expr::Bin("&&".into(), Box::new(Expr::Bin(">=".into(), Box::new(m()), Box::new(a.clone()))), Box::new(Expr::Bin("<=".into(), Box::new(m()), Box::new(b.clone())))),
        } };
        let mut chain: Vec<Stmt> = otherwise.clone().unwrap_or_default();
        for (pats, body) in arms.into_iter().rev() {
            let mut cond = test(&pats[0]);
            for p in &pats[1..] { cond = Expr::Bin("||".into(), Box::new(cond), Box::new(test(p))); }
            chain = vec![Stmt::If(cond, body, chain)];
        }
        let mut block = vec![Stmt::Let("__mito__match".into(), 0, subject), Stmt::MatchArms(all, otherwise.is_some())];
        block.extend(chain);
        Ok(Stmt::If(Expr::Bool(true), block, Vec::new()))
    }
    fn sym_kw(&mut self, k: &str) -> bool { if self.is_kw(k) { self.i += 1; true } else { false } }

    fn if_stmt(&mut self) -> R<Stmt> {
        self.kw("if")?; let c = self.no_init(|p| p.expr())?; let t = self.block()?;
        let e = if self.is_kw("else") { self.i += 1; if self.is_kw("if") { vec![self.if_stmt()?] } else { self.block()? } } else { vec![] };
        Ok(Stmt::If(c, t, e))
    }

    /// A key of a JSON object or a forwarded field: a name, or a string for one that is not a name.
    fn key(&mut self) -> R<String> {
        match self.t.get(self.i) { Some(Tok::Str(s)) | Some(Tok::Id(s)) => { let s = s.clone(); self.i += 1; Ok(s) } _ => Err(format!("expected a key, found {}", self.here())) }
    }
    fn here_word(&self) -> String { match self.peek() { Some(Tok::Id(x)) => x.clone(), _ => self.here() } }

    fn json(&mut self) -> R<Json> {
        if self.sym("{") {
            let mut f = Vec::new();
            while !self.sym("}") {
                let k = self.key()?;
                self.eat(":")?; f.push((k, self.json()?)); self.sym(",");
            }
            return Ok(Json::Obj(f));
        }
        if self.sym("[") {
            if self.is_kw("each") { return Err("`[each ...]` is now `[foreach ...]`, and a range includes its end".into()); }
            self.kw("foreach")?; let it = self.iter()?; let v = self.no_init(|p| p.json())?; self.eat("]")?; return Ok(Json::Arr(it, Box::new(v)));
        }
        Ok(Json::E(self.expr()?))
    }

    fn expr(&mut self) -> R<Expr> { self.bin(0) }
    fn bin(&mut self, level: usize) -> R<Expr> {
        // Rust's precedence, loosest first: `||`, `&&`, comparisons, `|`, `^`, `&`, shifts, `+ -`, `* / %` (comparisons bind looser than the
        // bit operators, unlike C)
        const LV: [&[&str]; 9] = [&["||"], &["&&"], &["==", "!=", "<", "<=", ">", ">="], &["|"], &["^"], &["&"], &["<<", ">>"], &["+", "-"], &["*", "/", "%"]];
        if level == LV.len() { return self.unary(); }
        let mut l = self.bin(level + 1)?;
        loop {
            let op = match self.peek() { Some(Tok::Sym(s)) if LV[level].contains(&s.as_str()) => s.clone(), _ => break };
            self.i += 1;
            let r = self.bin(level + 1)?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    /// `e as TYPE`, binding tighter than `*` and looser than a prefix `-` (as in Rust): a conversion that always succeeds. To a narrower integer
    /// it keeps the low bits (`x as u8` is `x & 255`; to a signed type the top bit is the sign), so the compiler's ranges for masks apply to it;
    /// `as i64` leaves an integer as it is, `as f64` makes a number a float. A float becomes an integer by truncation, as `int(x)` does.
    fn unary(&mut self) -> R<Expr> {
        let mut e = self.unary_operand()?;
        while self.is_kw("as") {
            self.i += 1;
            let ty = self.id()?;
            let bits = match ty.as_str() { "u8" | "i8" => 8, "u16" | "i16" => 16, "u32" | "i32" => 32, "i64" => 64, "f64" => 0, _ => return Err(format!("`as {}`: a number converts `as` u8, u16, u32, i8, i16, i32, i64 or f64 (u64 and f32 are not values in code yet)", ty)) };
            let int = |x: Expr| Expr::Call("int".into(), vec![x], CallRes::Unbound);
            e = if bits == 0 { Expr::Bin("*".into(), Box::new(e), Box::new(Expr::Float(1.0))) }
                else if bits == 64 { int(e) }
                else {
                    let mask = (1i64 << bits) - 1;
                    let low = Expr::Bin("&".into(), Box::new(int(e)), Box::new(Expr::Int(mask)));
                    if ty.starts_with('u') { low } else {
                        // sign: flip the top bit, then take it away
                        let half = 1i64 << (bits - 1);
                        Expr::Bin("-".into(), Box::new(Expr::Bin("^".into(), Box::new(low), Box::new(Expr::Int(half)))), Box::new(Expr::Int(half)))
                    }
                };
        }
        Ok(e)
    }
    fn unary_operand(&mut self) -> R<Expr> {
        if self.sym("!") { return Ok(Expr::Un('!', Box::new(self.unary_operand()?))); }
        if self.sym("-") { return Ok(Expr::Un('-', Box::new(self.unary_operand()?))); }
        self.postfix()
    }
    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.sym(".") {
                let f = self.id()?;
                // `game.board.add(args)`: named by the receiver's path (a module instance's function, or the binder makes it
                // a collection operation); on any other value, `.f(value, args)`
                // `coll.add { ... }`: an initializer may follow `add` without parentheses
                let bare_init = (f == "add" || f == "try_add") && self.init_ok && matches!(self.peek(), Some(Tok::Sym(s)) if s == "{");
                if bare_init || matches!(self.peek(), Some(Tok::Sym(s)) if s == "(") {
                    let mut a = Vec::new();
                    if !bare_init { self.i += 1; while !self.sym(")") { a.push(self.expr()?); self.sym(","); } }
                    e = match path_of(&e) { Some(n) => Expr::Call(format!("{}.{}", n, f), a, CallRes::Unbound), None => { a.insert(0, e); Expr::Call(format!(".{}", f), a, CallRes::Unbound) } };
                    // `coll.add() { field: value, ... }`: made into a hidden local and assignments before this statement
                    if self.init_ok && matches!(self.peek(), Some(Tok::Sym(s)) if s == "{") {
                        self.i += 1;
                        let t = format!("__mito__new{}", self.tmp); self.tmp += 1;
                        let mut sets = Vec::new();
                        while !self.sym("}") { let k = self.id()?; self.eat(":")?; let v = self.expr()?; sets.push(Stmt::Assign(Expr::Field(Box::new(Expr::Var(t.clone(), VarRes::Unbound)), k), v)); self.sym(","); }
                        self.pre.push(Stmt::Let(t.clone(), 0, e));
                        self.pre.extend(sets);
                        e = Expr::Var(t, VarRes::Unbound);
                    }
                    continue;
                }
                e = Expr::Field(Box::new(e), f);
            }
            else if self.sym("[") { let i = self.expr()?; self.eat("]")?; e = Expr::Index(Box::new(e), Box::new(i)); }
            else { break; }
        }
        Ok(e)
    }
    fn primary(&mut self) -> R<Expr> {
        match self.t.get(self.i).cloned() {
            Some(Tok::Int(n)) => { self.i += 1; Ok(Expr::Int(n)) }
            Some(Tok::Flt(f)) => { self.i += 1; Ok(Expr::Float(f)) }
            Some(Tok::Str(s)) => { self.i += 1; Ok(Expr::Str(s)) }
            Some(Tok::Sym(s)) if s == "(" => { self.i += 1; let e = self.expr()?; self.eat(")")?; Ok(e) }
            Some(Tok::Id(n)) => {
                self.i += 1;
                match n.as_str() { "true" => return Ok(Expr::Bool(true)), "false" => return Ok(Expr::Bool(false)), "null" => return Ok(Expr::Null), _ => {} }
                // `first u in app.users where u.email == x`: the first item the loop would visit, or null; a hidden `var` and a loop with
                // `limit 1` before this statement (for a table, one SELECT ... LIMIT 1)
                if n == "first" && matches!(self.peek(), Some(Tok::Id(_))) && matches!(self.t.get(self.i + 1), Some(Tok::Id(k)) if k == "in") {
                    if !self.init_ok { return Err("`first ... in ...` cannot be used here (a condition or a loop's clauses run at another time): name it first, `let x = first ...;`".into()); }
                    let mut it = self.iter()?;
                    if it.hi.is_some() { return Err("`first` takes a collection (`first u in app.users where ...`), not a range".into()); }
                    if it.index.is_some() { return Err("`first x, i in ...`: the index of the first is 0".into()); }
                    if it.limit.is_none() { it.limit = Some(Expr::Int(1)); }
                    let t = format!("__mito__first{}", self.tmp); self.tmp += 1;
                    let var = it.var.clone();
                    self.pre.push(Stmt::Var(t.clone(), 0, Expr::Null));
                    self.pre.push(Stmt::Each(it, vec![Stmt::Assign(Expr::Var(t.clone(), VarRes::Unbound), Expr::Var(var, VarRes::Unbound))]));
                    return Ok(Expr::Var(t, VarRes::Unbound));
                }
                // `asset("email.svg")`: the asset's digested URL from the web block's manifest (`/assets/email-6c595bc5.svg`); a name the
                // manifest does not have stops the build
                if n == "asset" && matches!(self.peek(), Some(Tok::Sym(s)) if s == "(") {
                    if let (Some(Tok::Str(name)), Some(Tok::Sym(close))) = (self.t.get(self.i + 1).cloned(), self.t.get(self.i + 2).cloned()) {
                        if close == ")" {
                            self.i += 3;
                            return ASSETS.with(|a| a.borrow().iter().find(|x| x.0 == name).map(|x| Expr::Str(x.1.clone())))
                                .ok_or_else(|| format!("`asset(\"{}\")`: the web block's manifest has no such asset", name));
                        }
                    }
                    return Err("`asset` takes the asset's name as a literal: `asset(\"email.svg\")`".into());
                }
                // `file_base64("logos/icon.png")`: any file's bytes, read when compiling, as a base64 literal (`unbase64` gives the bytes)
                if n == "file_base64" && matches!(self.peek(), Some(Tok::Sym(s)) if s == "(") {
                    if let (Some(Tok::Str(name)), Some(Tok::Sym(close))) = (self.t.get(self.i + 1).cloned(), self.t.get(self.i + 2).cloned()) {
                        if close == ")" { self.i += 3; return Ok(Expr::Str(base64(&read_file_bytes(&name)?))); }
                    }
                    return Err("`file_base64` takes the path as a literal: `file_base64(\"logos/icon.png\")`".into());
                }
                // `file("assets/app.css")`: the file's text, read when compiling, as a literal
                if n == "file" && matches!(self.peek(), Some(Tok::Sym(s)) if s == "(") {
                    if let (Some(Tok::Str(name)), Some(Tok::Sym(close))) = (self.t.get(self.i + 1).cloned(), self.t.get(self.i + 2).cloned()) {
                        if close == ")" { self.i += 3; return Ok(Expr::Str(read_file(&name)?)); }
                    }
                    return Err("`file` takes the path as a literal: `file(\"assets/app.css\")`".into());
                }
                if self.in_test && matches!(n.as_str(), "get" | "post" | "put" | "patch" | "delete") && self.sym("(") {
                    // arguments: the path, how many body fields, the body's (name, value) pairs, then the headers'
                    let path = self.expr()?;
                    let mut pairs = |p: &mut Self| -> R<Vec<Expr>> {
                        let mut v = Vec::new();
                        p.eat("{")?;
                        while !p.sym("}") { let k = p.key()?; p.eat(":")?; v.push(Expr::Str(k)); v.push(p.expr()?); p.sym(","); }
                        Ok(v)
                    };
                    let body = if self.sym(",") { pairs(self)? } else { Vec::new() };
                    let hdrs = if self.sym(",") { pairs(self)? } else { Vec::new() };
                    self.eat(")")?;
                    let mut a = vec![path, Expr::Int(body.len() as i64 / 2)];
                    a.extend(body); a.extend(hdrs);
                    return Ok(Expr::Call(n, a, CallRes::Unbound));
                }
                if self.sym("(") {
                    let mut a = Vec::new();
                    while !self.sym(")") { a.push(self.expr()?); self.sym(","); }
                    return Ok(Expr::Call(n, a, CallRes::Unbound));
                }
                Ok(Expr::Var(n, VarRes::Unbound))
            }
            _ => Err(format!("unexpected {}", self.here())),
        }
    }
}

// ---------------------------------------------------------------- binder

/// `a.b.c` when `e` is a name followed by fields, else None.
pub fn path_of(e: &Expr) -> Option<String> {
    match e { Expr::Var(n, _) => Some(n.clone()), Expr::Field(b, f) => path_of(b).map(|p| format!("{}.{}", p, f)), _ => None }
}

/// `owner`: the module instance whose function is being bound (its private functions may be called from it).
struct Binder<'g> { names: Vec<(String, u32, bool)>, next: u32, max: u32, globals: &'g dyn Fn(&str) -> Option<usize>, fns: Vec<(String, usize, bool)>, place: String, owner: Option<String>, hidden: u32, at: (u32, u32), loops: u32, enums: &'g Enums, test: bool, coll_loops: u32 }

/// Resolve every name in `api`. `globals` answers whether a name is a declared global (and which).
pub fn bind(api: &mut Api, globals: &dyn Fn(&str) -> Option<usize>) -> R<()> { bind_with(api, globals, &[]) }
/// The enums' names and values, for `Name.Value` in code.
pub type Enums = [(String, Vec<(String, i64)>)];
pub fn bind_with(api: &mut Api, globals: &dyn Fn(&str) -> Option<usize>, enums: &Enums) -> R<()> {
    let fns: Vec<(String, usize, bool)> = api.fns.iter().map(|f| (f.name.clone(), f.params.len(), f.public)).collect();
    for (k, f) in fns.iter().enumerate() { if fns[..k].iter().any(|g| g.0 == f.0) { return Err(format!("fn `{}` is declared twice", f.0)); } if B::from(&f.0).is_some() { return Err(format!("fn `{}` has the name of a builtin", f.0)); } }
    let mut errs = Vec::new();
    for f in &mut api.fns {
        let mut b = Binder { names: Vec::new(), next: 0, max: 0, globals, fns: fns.clone(), place: format!("fn {}", f.name), owner: f.name.rfind('.').map(|i| f.name[..i].to_string()), hidden: 0, at: (0, 0), loops: 0, enums, test: false, coll_loops: 0 };
        for p in &f.params { b.declare(p, false); }
        // a full signature: every parameter's type, and the result's when the function returns a value
        if let Some(Stmt::At(l, c)) = f.body.first() { b.at = (*l, *c); }
        if f.test.is_some() { b.test = true; b.place = format!("test \"{}\"", f.test.as_deref().unwrap_or("")); }
        if f.test.is_none() { if let Some(i) = f.ptys.iter().position(|t| t.is_none()) { errs.push(b.err::<()>(format!("parameter `{}` needs a type: `{}: i64` (or f64, bool, string, an item type, ...)", f.params[i], f.params[i])).unwrap_err()); continue; }
        // text a function returns: `-> text`, which lives in the request's text area (no size to declare: it is built, not stored)
        if let Some(t) = &f.ret { if matches!(t.name.as_str(), "string" | "utf8" | "ascii") && t.field.is_none() { errs.push(b.err::<()>(format!("`{}` returns text: declare it `-> text` (text built for the request; a field's size is declared where it is kept)", f.name)).unwrap_err()); continue; } }
        if f.ret.is_none() && gives_value(&f.body) { errs.push(b.err::<()>(format!("`{}` returns a value: declare its result, `fn {}(...) -> T`", f.name, f.name)).unwrap_err()); continue; } }
        if let Err(e) = b.block(&mut f.body) { errs.push(e); }
        f.slots = b.max;
    }
    for e in &mut api.eps {
        let mut b = Binder { names: Vec::new(), next: 0, max: 0, globals, fns: fns.clone(), place: format!("endpoint {}", e.text), owner: None, hidden: 0, at: (0, 0), loops: 0, enums, test: false, coll_loops: 0 };
        for i in &e.inputs { b.declare(&i.name, false); } // slots 0.. in order: the inputs
        if let Err(e) = b.block(&mut e.body) { errs.push(e); continue; }
        e.slots = b.max;
        if !answers(&e.body) { errs.push(format!("endpoint {} ({}) can finish without an answer: end every path with `return ok(...)` (or created, no_content, status), `fail` or `forward`", e.name, e.text)); }
    }
    // every function and endpoint is bound on its own: their errors are independent, one per line
    if errs.is_empty() { Ok(()) } else { Err(errs.join("\n")) }
}

/// Does every path through `body` end in an answer, a `fail`, a `forward` (or the test hook `__crash()`)?
fn answers(body: &[Stmt]) -> bool {
    match body.last() {
        Some(Stmt::Respond(..) | Stmt::Fail(..) | Stmt::Forward { .. }) => true,
        Some(Stmt::Do(Expr::Call(n, _, _))) => n == "__crash",
        Some(Stmt::If(_, t, e)) => answers(t) && answers(e),
        _ => false,
    }
}

impl Binder<'_> {
    fn err<T>(&self, m: String) -> R<T> { if self.at.0 > 0 { Err(format!("{}:{}: {}: {}", self.at.0, self.at.1, self.place, m)) } else { Err(format!("{}: {}", self.place, m)) } }
    fn declare(&mut self, n: &str, mutable: bool) -> u32 {
        let s = self.next; self.next += 1; self.max = self.max.max(self.next);
        self.names.push((n.to_string(), s, mutable)); s
    }
    fn block(&mut self, stmts: &mut [Stmt]) -> R<()> {
        let (mark, next) = (self.names.len(), self.next);
        for s in stmts.iter_mut() { self.stmt(s)?; }
        self.names.truncate(mark); self.next = next; // slots of a finished block are reused
        Ok(())
    }
    fn iter_head(&mut self, it: &mut Iter) -> R<()> {
        self.expr(&mut it.coll)?;
        if let Some(h) = &mut it.hi { self.expr(h)?; }
        // a loop over a module instance walks it with the instance's iteration functions
        if let Some(n) = path_of(&it.coll) {
            let f = |x: &str| self.fns.iter().position(|g| g.0 == format!("{}.{}", n, x)).map(|k| k as u32);
            if let (Some(first), Some(next), Some(item)) = (f("iter_first"), f("iter_next"), f("iter_item")) {
                if it.reverse && (f("iter_last").is_none() || f("iter_prev").is_none()) { return self.err(format!("{} cannot be walked in reverse", n)); }
                it.via = Some(Via { first, next, item, last: f("iter_last"), prev: f("iter_prev") });
            }
        }
        if let Some(fr) = &mut it.from { if it.via.is_none() { return self.err(format!("`foreach {} in ... from`: only a module instance can start a loop from a key", it.var)); } self.expr(fr)?; }
        if let Some(l) = &mut it.limit { self.expr(l)?; }
        if let Some(a) = &mut it.after { self.expr(a)?; }
        it.slot = self.declare(&it.var, false);
        if let Some((n, slot)) = &mut it.index { *slot = self.declare(n, false); }
        if let Some(w) = &mut it.whil { if it.hi.is_none() && it.via.is_none() { return self.err(format!("`foreach {} in ... while`: only ranges and module instances stop early; a list or pool is walked whole", it.var)); } self.expr(w)?; }
        if let Some(w) = &mut it.wher { self.expr(w)?; }
        Ok(())
    }
    /// A `match`'s arms: constants only, each value once; arms naming an enum's values name one enum, and cover all of it
    /// unless there is an `else`.
    fn match_arms(&mut self, pats: &mut [Pat], has_else: bool) -> R<()> {
        let mut used_enum: Option<String> = None;
        let mut seen: Vec<(i64, i64)> = Vec::new();
        for p in pats.iter_mut() {
            let (a, b) = match p { Pat::One(e) => (e.clone(), e.clone()), Pat::Range(a, b) => (a.clone(), b.clone()) };
            let mut ends = Vec::new();
            for mut e in [a, b] {
                if let Expr::Field(base, _) = &e { if let Expr::Var(n, _) = &**base { if self.enums.iter().any(|x| x.0 == *n) {
                    if let Some(u) = &used_enum { if u != n { return self.err(format!("a `match` on values of `{}` has an arm of `{}`", u, n)); } }
                    used_enum = Some(n.clone());
                } } }
                self.expr(&mut e)?;
                let v = match &e { Expr::Int(v) => *v, Expr::Un('-', x) => match **x { Expr::Int(v) => -v, _ => return self.err("a `match` arm is a constant: a number, an enum value, or a range of them".into()) }, _ => return self.err("a `match` arm is a constant: a number, an enum value, or a range of them".into()) };
                ends.push(v);
            }
            let (lo, hi) = (ends[0], ends[1]);
            if lo > hi { return self.err(format!("the range {}..{} in a `match` is empty", lo, hi)); }
            if let Some(o) = seen.iter().find(|o| lo <= o.1 && o.0 <= hi) { let (a, b) = (lo.max(o.0), hi.min(o.1)); return self.err(format!("a `match` covers {} twice", if a == b { a.to_string() } else { format!("{}..{}", a, b) })); }
            seen.push((lo, hi));
        }
        if let (Some(n), false) = (&used_enum, has_else) {
            let vals: Vec<(String, i64)> = self.enums.iter().find(|x| x.0 == *n).map(|x| x.1.clone()).unwrap_or_default();
            let missing: Vec<&str> = vals.iter().filter(|v| !seen.iter().any(|s| s.0 <= v.1 && v.1 <= s.1)).map(|v| v.0.as_str()).collect();
            if !missing.is_empty() { return self.err(format!("this `match` on `{}` does not handle {}: add {} or an `else` arm", n, missing.iter().map(|m| format!("`{}.{}`", n, m)).collect::<Vec<_>>().join(", "), if missing.len() == 1 { "it" } else { "them" })); }
        }
        Ok(())
    }
    /// `x.f(args)` naming no function of a module instance: a collection operation, `.f(x, args)`.
    fn operation(&self, e: &mut Expr) {
        let Expr::Call(n, args, _) = e else { return };
        if n.starts_with('.') || self.fns.iter().any(|f| f.0 == *n) { return; }
        let Some((recv, op)) = n.rsplit_once('.') else { return };
        if !matches!(op, "add" | "try_add" | "remove" | "move" | "copy_from" | "sort" | "fill" | "clear" | "truncate" | "count_below" | "count" | "capacity") { return; }
        let mut parts = recv.split('.');
        let mut r = Expr::Var(parts.next().unwrap_or_default().to_string(), VarRes::Unbound);
        for f in parts { r = Expr::Field(Box::new(r), f.to_string()); }
        args.insert(0, r);
        *n = format!(".{}", op);
    }
    fn stmt(&mut self, s: &mut Stmt) -> R<()> {
        if let Stmt::At(l, c) = s { self.at = (*l, *c); return Ok(()); }
        // collection operations that are statements: `let p = coll.add();`, `coll.add();`, `coll.remove(p);`
        match s {
            Stmt::Let(_, _, e) | Stmt::Do(e) => self.operation(e),
            _ => {}
        }
        let lowered = match s {
            Stmt::Let(n, _, Expr::Call(op, args, _)) if (op == ".add" || op == ".try_add") && args.len() == 1 => Some(Stmt::Alloc { var: n.clone(), slot: 0, coll: args.remove(0), try_: op == ".try_add" }),
            Stmt::Do(Expr::Call(op, args, _)) if (op == ".add" || op == ".try_add") && args.len() == 1 => { self.hidden += 1; Some(Stmt::Alloc { var: format!("__mito__added{}", self.hidden), slot: 0, coll: args.remove(0), try_: op == ".try_add" }) }
            Stmt::Do(Expr::Call(op, args, _)) if op == ".clear" && args.len() == 1 => Some(Stmt::Clear(args.remove(0))),
            Stmt::Do(Expr::Call(op, args, _)) if op == ".remove" && args.len() == 2 => { let item = args.remove(1); Some(Stmt::Remove(args.remove(0), item)) }
            Stmt::Do(Expr::Call(op, args, _)) if op == ".fill" && args.len() == 4 => {
                let value = args.remove(3); let count = args.remove(2); let from = args.remove(1);
                Some(Stmt::Fill(args.remove(0), from, count, value))
            }
            Stmt::Do(Expr::Call(op, args, _)) if op == ".move" && args.len() == 4 => {
                let count = args.remove(3); let to = args.remove(2); let from = args.remove(1);
                Some(Stmt::Move(args.remove(0), from, to, count))
            }
            Stmt::Do(Expr::Call(op, args, _)) if op == ".sort" && args.len() == 4 => {
                let scratch = args.remove(3); let count = args.remove(2); let from = args.remove(1);
                Some(Stmt::Sort(args.remove(0), from, count, scratch))
            }
            Stmt::Do(Expr::Call(op, args, _)) if op == ".copy_from" && args.len() == 5 => {
                let count = args.remove(4); let to = args.remove(3); let from = args.remove(2); let src = args.remove(1);
                Some(Stmt::Copy(args.remove(0), src, from, to, count))
            }
            _ => None,
        };
        if let Some(l) = lowered { *s = l; }
        match s {
            Stmt::At(..) => {}
            Stmt::Let(n, slot, e) => { self.expr(e)?; *slot = self.declare(n, false); }
            Stmt::Var(n, slot, e) => { self.expr(e)?; *slot = self.declare(n, true); }
            Stmt::Alloc { .. } | Stmt::Remove(..) | Stmt::Clear(..) | Stmt::Move(..) | Stmt::Copy(..) | Stmt::Sort(..) | Stmt::Fill(..) if self.test => return self.err("a test changes the state only through requests (`post(...)` and the others); it can read the state directly".into()),
            Stmt::Assign(lhs, _) if self.test && !matches!(lhs, Expr::Var(..)) => return self.err("a test changes the state only through requests (`post(...)` and the others); it can read the state directly".into()),
            Stmt::Alloc { var, slot, coll, .. } => { self.expr(coll)?; *slot = self.declare(var, false); }
            Stmt::Assign(lhs, rhs) => {
                self.expr(rhs)?;
                if let Expr::Var(n, res) = lhs {
                    match self.names.iter().rev().find(|x| x.0 == *n) {
                        Some(x) if x.2 => *res = VarRes::Local(x.1),
                        Some(_) => return self.err(format!("`{}` is a `let`; declare it with `var` to change it", n)),
                        None => return self.err(format!("unknown variable `{}`", n)),
                    }
                } else { self.expr(lhs)?; }
            }
            Stmt::If(c, t, e) => { self.expr(c)?; self.block(t)?; self.block(e)?; }
            Stmt::Each(it, body) => {
                let (mark, next) = (self.names.len(), self.next);
                self.iter_head(it)?;
                // a loop over a collection (not a range) reads its extent once: a request inside could change it
                let over_coll = it.hi.is_none();
                self.loops += 1; if over_coll { self.coll_loops += 1; }
                let r = self.block(body);
                self.loops -= 1; if over_coll { self.coll_loops -= 1; }
                r?; self.names.truncate(mark); self.next = next;
            }
            Stmt::Break | Stmt::Continue => if self.loops == 0 { return self.err(format!("`{}` outside a loop", if matches!(s, Stmt::Break) { "break" } else { "continue" })); },
            Stmt::ContinueHere => {}
            Stmt::MatchArms(pats, has_else) => self.match_arms(pats, *has_else)?,
            Stmt::Remove(c, e) => { self.expr(c)?; self.expr(e)?; }
            Stmt::Clear(c) => { self.expr(c)?; }
            Stmt::Move(a, f, t2, n) => { self.expr(a)?; self.expr(f)?; self.expr(t2)?; self.expr(n)?; }
            Stmt::Copy(d, src, f, t2, n) => { self.expr(d)?; self.expr(src)?; self.expr(f)?; self.expr(t2)?; self.expr(n)?; }
            Stmt::Fill(a, f, n, v) => { self.expr(a)?; self.expr(f)?; self.expr(n)?; self.expr(v)?; }
            Stmt::Sort(a, f, n, t) => { self.expr(a)?; self.expr(f)?; self.expr(n)?; self.expr(t)?; }
            Stmt::Do(e) => self.expr(e)?,
            Stmt::Return(e) => { if let Some(e) = e { self.expr(e)?; } }
            Stmt::Respond(_, j) => self.json(j)?,
            Stmt::Forward { key, fields, .. } => { self.expr(key)?; for f in fields { self.expr(&mut f.1)?; } }
            Stmt::Fail(..) => {}
        }
        Ok(())
    }
    fn json(&mut self, j: &mut Json) -> R<()> {
        match j {
            Json::Obj(f) => { for (_, v) in f { self.json(v)?; } Ok(()) }
            Json::Arr(it, inner) => { let (mark, next) = (self.names.len(), self.next); self.iter_head(it)?; self.json(inner)?; self.names.truncate(mark); self.next = next; Ok(()) }
            Json::E(e) => self.expr(e),
            Json::Empty | Json::Html => Ok(()),
        }
    }
    fn expr(&mut self, e: &mut Expr) -> R<()> {
        // in a module instance's function, `self` is the instance, written as its path (`game.board`); never a local,
        // so nothing shadows it
        if matches!(e, Expr::Var(n, _) if n == "self") {
            let Some(o) = self.owner.clone() else { return self.err("`self` is only for module code".into()) };
            let mut parts = o.split('.');
            let mut p = Expr::Var(parts.next().unwrap_or_default().to_string(), VarRes::Unbound);
            for f in parts { p = Expr::Field(Box::new(p), f.to_string()); }
            *e = p;
        }
        // `Op.Plus`: an enum's value is its integer (unless `Op` is a local)
        if let Expr::Field(b, v) = e {
            if let Expr::Var(n, _) = &**b {
                if !self.names.iter().any(|x| x.0 == *n) {
                    if let Some((en, vals)) = self.enums.iter().find(|x| x.0 == *n) {
                        let Some(x) = vals.iter().find(|x| x.0 == *v) else { let hint = suggest(v, vals.iter().map(|x| x.0.as_str())); return self.err(format!("enum `{}` has no value `{}`{}", en, v, hint)) };
                        *e = Expr::Int(x.1);
                        return Ok(());
                    }
                }
            }
        }
        self.operation(e);
        match e {
            Expr::Var(n, res) => {
                *res = match self.names.iter().rev().find(|x| x.0 == *n) {
                    Some(x) => VarRes::Local(x.1),
                    None => match (self.globals)(n) { Some(g) => VarRes::Global(g as u32), None => {
                        let hint = if ["nil", "none", "None", "NULL", "Null", "undefined", "nullptr"].contains(&n.as_str()) { " (the value that is not there is written `null`)".to_string() }
                            else { suggest(n, self.names.iter().map(|x| x.0.as_str())) };
                        return self.err(format!("unknown name `{}`{}", n, hint)) } },
                };
            }
            Expr::Field(b, _) => self.expr(b)?,
            Expr::Index(b, i) => { self.expr(b)?; self.expr(i)?; }
            Expr::Un(_, x) => self.expr(x)?,
            Expr::Bin(_, a, b) => { self.expr(a)?; self.expr(b)?; }
            Expr::Call(n, ..) if n.starts_with('.') => {
                let Expr::Call(n, args, res) = e else { unreachable!() };
                for a in args.iter_mut() { self.expr(a)?; }
                *res = match (n.as_str(), args.len()) {
                    (".count", 1) => CallRes::Builtin(B::Count),
                    (".capacity", 1) => CallRes::Builtin(B::Capacity),
                    (".add" | ".try_add", 1) => return self.err(format!("`{}()` makes an item: write `let x = coll{}();` or `coll{}();` as a statement", &n[1..], n, n)),
                    (".remove", 2) => return self.err("`remove(item)` is a statement: `coll.remove(item);`".into()),
                    (".clear", _) => return self.err("`coll.clear()` is a statement".into()),
                    (".count" | ".capacity" | ".add" | ".try_add", k) => return self.err(format!("`{}()` takes no arguments, given {}", &n[1..], k - 1)),
                    (".remove", k) => return self.err(format!("`remove` takes one item, given {}", k - 1)),
                    (".truncate", 2) => CallRes::Builtin(B::Truncate),
                    (".truncate", k) => return self.err(format!("`text.truncate(n)` takes the number of bytes to keep, given {} arguments", k - 1)),
                    (".count_below", 4) => CallRes::Builtin(B::CountBelow),
                    (".count_below", k) => return self.err(format!("`array.count_below(n, target, inclusive)` takes three arguments, given {}", k - 1)),
                    (".move", _) => return self.err("`array.move(from, to, count)` is a statement".into()),
                    (".fill", _) => return self.err("`array.fill(from, count, value)` is a statement".into()),
                    _ => return self.err(format!("unknown function `{}`", &n[1..])),
                };
            }
            Expr::Call(n, args, res) => {
                if n == "count" || n == "capacity" { return self.err(format!("`{}(x)` was replaced by `x.{}()`", n, n)); }
                if matches!(n.as_str(), "body" | "param" | "query" | "header" | "fwd") { return self.err(format!("`{}(...)` was replaced by typed inputs: the endpoint names what it takes, as in `fn name(points: i64)`", n)); }
                if n == "auth" { return self.err("`auth(pool)` was replaced by a session input: `who: Session from header(\"X-Session\")`".into()); }
                for a in args.iter_mut() { self.expr(a)?; }
                if let (Some(rest), Some(o)) = (n.strip_prefix("self."), self.owner.as_ref()) { *n = format!("{}.{}", o, rest); }
                // a test's requests (only in tests: elsewhere `get` and the others are ordinary names)
                if self.test {
                    let verb = match n.as_str() { "get" => Some(B::ReqGet), "post" => Some(B::ReqPost), "put" => Some(B::ReqPut), "patch" => Some(B::ReqPatch), "delete" => Some(B::ReqDelete), "field" => Some(B::JsonField), _ => None };
                    if let Some(b) = verb {
                        if self.coll_loops > 0 && b != B::JsonField { return self.err(format!("`{}(...)` inside a loop over a collection: a request may change the collection; loop over a range, or collect what to send first", n)); }
                        *res = CallRes::Builtin(b);
                        return Ok(());
                    }
                }
                *res = if let Some(k) = self.fns.iter().position(|f| f.0 == *n) {
                    if self.fns[k].1 != args.len() { return self.err(format!("`{}` takes {} arguments, given {}", n, self.fns[k].1, args.len())); }
                    if let Some(i) = n.rfind('.') { if !self.fns[k].2 && self.owner.as_deref() != Some(&n[..i]) { return self.err(format!("`{}` is private to {}", n, &n[..i])); } }
                    CallRes::User(k as u32)
                } else if let Some(b) = B::from(n) { CallRes::Builtin(b) }
                // `split(...)`: what a loop walks (`foreach p in split(t, ", ")`); compiled there
                else if n == "split" || n == "__mito__text_cap" { CallRes::Unbound }
                else { let hint = suggest(n, self.fns.iter().map(|f| f.0.as_str()).chain(BUILTIN_NAMES.iter().copied())); return self.err(format!("unknown function `{}`{}", n, hint)); };
            }
            _ => {}
        }
        Ok(())
    }
}

/// A token as an error message names it.
fn describe(t: Option<&Tok>) -> String {
    match t {
        None => "the end of the file".into(),
        Some(Tok::Id(x)) => format!("`{}`", x),
        Some(Tok::Sym(x)) => format!("`{}`", x),
        Some(Tok::Int(n)) => format!("the number {}", n),
        Some(Tok::Flt(f)) => format!("the number {}", f),
        Some(Tok::Str(x)) => format!("the text \"{}\"", x.chars().take(20).collect::<String>()),
        Some(Tok::HOpen(x)) => format!("`<{}`", x),
        Some(Tok::HAttr(x)) => format!("the attribute `{}`", x),
        Some(Tok::HEnd(s)) => if *s { "`/>`".into() } else { "`>`".into() },
        Some(Tok::HClose(x)) => format!("`</{}>`", x),
        Some(Tok::HText(x)) => format!("the text \"{}\"", x.chars().take(20).collect::<String>()),
        Some(Tok::HQ) => "`\"`".into(),
    }
}

/// " (did you mean `x`?)" when one of `candidates` is within two edits of `name`, else "".
pub fn suggest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> String {
    fn dist(a: &str, b: &str) -> usize {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut cur = vec![i; b.len() + 1];
            for j in 1..=b.len() { cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1])); }
            prev = cur;
        }
        prev[b.len()]
    }
    // one edit turns a short name into an unrelated one: none below three letters, one edit up to three
    let limit = match name.chars().count() { 0..=2 => return String::new(), 3 => 1, _ => 2 };
    let best = candidates.filter(|c| *c != name && !c.starts_with("__mito__")).map(|c| (dist(name, c), c)).filter(|(d, _)| *d <= limit).min_by_key(|(d, _)| *d);
    best.map_or(String::new(), |(_, c)| format!(" (did you mean `{}`?)", c))
}
/// The builtin functions, for suggestions.
pub const BUILTIN_NAMES: &[&str] = &["int", "str", "len", "now", "random", "min", "max", "slot", "stat", "token", "or", "exists", "maxlen", "sqrt", "exp", "sin", "cos", "atan2", "pow", "floor", "abs", "substr", "text_from", "wrapping_add", "wrapping_sub", "wrapping_mul", "log", "ok", "created", "no_content", "status"];

/// A loop without a fixed extent (`while`, a `for` that is not counted): a range as long as it needs, stopped by `cond`, and
/// counted against the request's step budget.
fn unbounded(cond: Option<Expr>) -> Iter {
    Iter { index: None, budget: true, var: "__mito__round".into(), slot: 0, coll: Expr::Int(0), hi: Some(Expr::Int(i64::MAX)), from: None, reverse: false, by: None, after: None, whil: cond, wher: None, limit: None, via: None, scan: false, last: false }
}
/// Is the variable `name` assigned anywhere in these statements?
fn assigns_name(stmts: &[Stmt], name: &str) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Assign(Expr::Var(n, _), _) => n == name,
        Stmt::If(_, t, e) => assigns_name(t, name) || assigns_name(e, name),
        Stmt::Each(_, b) => assigns_name(b, name),
        _ => false,
    })
}

/// Does any `return` in these statements give a value?
fn gives_value(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Return(Some(_)) => true,
        Stmt::If(_, t, e) => gives_value(t) || gives_value(e),
        Stmt::Each(_, b) => gives_value(b),
        _ => false,
    })
}
