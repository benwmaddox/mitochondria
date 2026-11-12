//! `mito fmt`: a conservative source formatter. It works on tokens, not on the compiler's tree (which drops comments and layout), and
//! it only ever changes white space: indentation (four spaces per open `{`, `(`, `[` or markup element, counted per line), spaces
//! around commas, colons and the common operators, trailing white space, blank lines. Strings, comments and the text and spacing
//! inside markup are never touched, and no line is joined to or split from another. See DESIGN.md, Formatting.
use crate::lang::VOID;

#[derive(Clone, Copy, PartialEq, Debug)]
enum K { Word, Num, Str, Sym, Comment, Other }
#[derive(Clone, Copy, PartialEq, Debug)]
enum Ent { Brace, Paren, Elem }
#[derive(Clone, Debug)]
enum Mode { Code { depth: u32, html: bool }, Content { depth: u32 }, Tag(String), AttrQ }
/// What the compiler's lexer sees as the last token (it decides where markup may start).
#[derive(Clone, Copy, PartialEq)]
enum Last { Other, Open(char), Mk }

struct Tk { s: usize, e: usize, k: K, plain: bool }

struct Scan {
    toks: Vec<Tk>,
    /// byte ranges where white space is part of the program's meaning (multi-line texts, quoted attribute values, `<!-- -->`)
    prot: Vec<(usize, usize)>,
    ls: Vec<usize>,
    base: Vec<usize>,
    top_paren: Vec<Option<usize>>,
    lead: Vec<Option<usize>>,
    lead_open: Vec<bool>,
    first: Vec<Option<usize>>,
    stack: Vec<(usize, Ent)>,
    filled: usize,
}

impl Scan {
    fn line_of(&self, off: usize) -> usize { self.ls.partition_point(|&x| x <= off) - 1 }
    /// How many lines have an open bracket or element on them (one level of indentation each).
    fn distinct(&self) -> usize {
        let (mut n, mut prev) = (0, usize::MAX);
        for &(l, _) in &self.stack { if l != prev { n += 1; prev = l; } }
        n
    }
    fn fill(&mut self, upto: usize) {
        while self.filled <= upto && self.filled < self.base.len() {
            let f = self.filled;
            self.base[f] = self.distinct();
            self.top_paren[f] = self.stack.last().filter(|e| e.1 == Ent::Paren).map(|e| e.0);
            self.filled += 1;
        }
    }
    fn pop(&mut self, ent: Ent) { if self.stack.last().map_or(false, |e| e.1 == ent) { self.stack.pop(); } }
    fn emit(&mut self, src: &str, s: usize, e: usize, k: K, plain: bool, closer: Option<Ent>, opener: Option<Ent>) {
        let l = self.line_of(s);
        self.fill(l);
        let idx = self.toks.len();
        if self.first[l].is_none() && src[self.ls[l]..s].trim().is_empty() { self.first[l] = Some(idx); self.lead_open[l] = closer.is_some(); }
        else if !(self.lead_open[l] && closer.is_some()) { self.lead_open[l] = false; }
        if let Some(ent) = closer { self.pop(ent); if self.lead_open[l] { self.lead[l] = Some(self.distinct()); } }
        if let Some(ent) = opener { self.stack.push((l, ent)); }
        self.toks.push(Tk { s, e, k, plain });
    }
}

const TWO: [&str; 15] = ["==", "!=", "<=", ">=", "&&", "||", "->", "..", "+=", "-=", "*=", "/=", "<<", ">>", "=>"];

fn scan(src: &str, raw_all: bool) -> Result<Scan, String> {
    let c: Vec<char> = src.chars().collect();
    let mut offs: Vec<usize> = src.char_indices().map(|x| x.0).collect();
    offs.push(src.len());
    let n = c.len();
    let mut ls = vec![0];
    for (i, b) in src.bytes().enumerate() { if b == b'\n' { ls.push(i + 1); } }
    let nl = ls.len();
    let mut sc = Scan { toks: Vec::new(), prot: Vec::new(), ls, base: vec![0; nl], top_paren: vec![None; nl], lead: vec![None; nl], lead_open: vec![false; nl], first: vec![None; nl], stack: Vec::new(), filled: 0 };
    let place = |sc: &Scan, i: usize| { let l = sc.line_of(offs[i]); format!("{}:{}", l + 1, src[sc.ls[l]..offs[i]].chars().count() + 1) };
    let mut modes = vec![Mode::Code { depth: 0, html: false }];
    let mut last = Last::Other;
    let (mut raw_pending, mut raw_len): (bool, Option<usize>) = (false, None);
    let mut i = 0;
    while i < n {
        let ch = c[i];
        let mode = modes.last().cloned().unwrap();
        match mode {
            Mode::Content { depth } => {
                if ch == '<' && c.get(i + 1) == Some(&'/') {
                    let mut k = i + 2;
                    while k < n && (c[k].is_alphanumeric() || c[k] == '-') { k += 1; }
                    let empty = k == i + 2;
                    while k < n && c[k] == ' ' { k += 1; }
                    if c.get(k) != Some(&'>') || empty || depth == 0 { return Err(format!("{}: a closing tag is `</name>`", place(&sc, i))); }
                    sc.emit(src, offs[i], offs[k + 1], K::Other, false, Some(Ent::Elem), None);
                    i = k + 1; last = Last::Mk;
                    if depth == 1 { modes.pop(); } else { *modes.last_mut().unwrap() = Mode::Content { depth: depth - 1 }; }
                } else if ch == '<' && src[offs[i]..].starts_with("<!--") {
                    let Some(r) = src[offs[i] + 4..].find("-->") else { return Err(format!("{}: a comment that is never closed", place(&sc, i))) };
                    let (s, e) = (offs[i], offs[i] + 4 + r + 3);
                    sc.prot.push((s, e));
                    sc.emit(src, s, e, K::Other, false, None, None);
                    while i < n && offs[i] < e { i += 1; }
                } else if ch == '<' && c.get(i + 1) == Some(&'!') {
                    let mut k = i; while k < n && c[k] != '>' { k += 1; }
                    if k >= n || depth != 0 { return Err(format!("{}: `<!doctype html>` comes first", place(&sc, i))); }
                    sc.emit(src, offs[i], offs[k + 1], K::Other, false, None, None);
                    i = k + 1; last = Last::Other;
                } else if ch == '<' && c.get(i + 1).map_or(false, |x| x.is_ascii_alphabetic()) {
                    let mut k = i + 1;
                    while k < n && (c[k].is_alphanumeric() || c[k] == '-') { k += 1; }
                    let name: String = c[i + 1..k].iter().collect();
                    sc.emit(src, offs[i], offs[k], K::Other, false, None, Some(Ent::Elem));
                    i = k; last = Last::Other; modes.push(Mode::Tag(name));
                } else if ch == '<' {
                    return Err(format!("{}: a `<` in text", place(&sc, i)));
                } else if ch == '{' {
                    sc.emit(src, offs[i], offs[i + 1], K::Other, false, None, Some(Ent::Brace));
                    i += 1; last = Last::Open('{'); modes.push(Mode::Code { depth: 0, html: true });
                } else if ch == '}' {
                    return Err(format!("{}: a `}}` in text", place(&sc, i)));
                } else {
                    let s = i;
                    while i < n && c[i] != '<' && c[i] != '{' && c[i] != '}' { i += 1; }
                    if let Some(f) = (s..i).find(|&k| !c[k].is_whitespace()) {
                        if depth == 0 { return Err(format!("{}: text outside the root element", place(&sc, f))); }
                        sc.emit(src, offs[f], offs[i], K::Other, false, None, None);
                        last = Last::Other;
                    }
                }
                continue;
            }
            Mode::Tag(name) => {
                if ch.is_whitespace() { i += 1; continue; }
                let selfc = ch == '/' && c.get(i + 1) == Some(&'>');
                if ch == '>' || selfc {
                    let e = i + if selfc { 2 } else { 1 };
                    let void = selfc || VOID.contains(&name.to_ascii_lowercase().as_str());
                    sc.emit(src, offs[i], offs[e], K::Other, false, None, None);
                    if void { sc.pop(Ent::Elem); }
                    i = e; last = Last::Mk;
                    modes.pop();
                    if let Some(Mode::Content { depth }) = modes.last().cloned() {
                        if !void { *modes.last_mut().unwrap() = Mode::Content { depth: depth + 1 }; } else if depth == 0 { modes.pop(); }
                    }
                    continue;
                }
                if ch.is_alphanumeric() || matches!(ch, '-' | ':' | '_' | '@') {
                    let s = i;
                    while i < n && (c[i].is_alphanumeric() || matches!(c[i], '-' | ':' | '_' | '@' | '.')) { i += 1; }
                    last = Last::Other;
                    if c.get(i) == Some(&'=') {
                        i += 1;
                        if c.get(i) == Some(&'"') { i += 1; sc.emit(src, offs[s], offs[i], K::Other, false, None, None); modes.push(Mode::AttrQ); }
                        else if c.get(i) == Some(&'{') {
                            sc.emit(src, offs[s], offs[i], K::Other, false, None, None);
                            sc.emit(src, offs[i], offs[i + 1], K::Other, false, None, Some(Ent::Brace));
                            i += 1; last = Last::Open('{'); modes.push(Mode::Code { depth: 0, html: true });
                        } else { return Err(format!("{}: an attribute's value is in double quotes or braces", place(&sc, i))); }
                    } else { sc.emit(src, offs[s], offs[i], K::Other, false, None, None); }
                    continue;
                }
                return Err(format!("{}: unexpected `{}` in a tag", place(&sc, i), ch));
            }
            Mode::AttrQ => {
                if ch == '"' { sc.emit(src, offs[i], offs[i + 1], K::Other, false, None, None); i += 1; modes.pop(); last = Last::Other; }
                else if ch == '{' {
                    sc.emit(src, offs[i], offs[i + 1], K::Other, false, None, Some(Ent::Brace));
                    i += 1; last = Last::Open('{'); modes.push(Mode::Code { depth: 0, html: true });
                } else {
                    let s = i;
                    while i < n && c[i] != '"' && c[i] != '{' { i += 1; }
                    sc.prot.push((offs[s], offs[i]));
                    sc.emit(src, offs[s], offs[i], K::Other, false, None, None);
                }
                continue;
            }
            Mode::Code { depth, html } => {
                let raw = raw_all || raw_len.is_some();
                if ch == '{' {
                    if raw_pending { raw_pending = false; raw_len = Some(sc.stack.len()); }
                    let plain = !(raw_all || raw_len.is_some());
                    *modes.last_mut().unwrap() = Mode::Code { depth: depth + 1, html };
                    sc.emit(src, offs[i], offs[i + 1], K::Sym, plain, None, Some(Ent::Brace));
                    i += 1; last = Last::Open('{'); continue;
                }
                if ch == '}' {
                    let boundary = depth == 0 && html;
                    sc.emit(src, offs[i], offs[i + 1], K::Sym, !raw && !boundary, Some(Ent::Brace), None);
                    i += 1; last = Last::Open('}');
                    if boundary { modes.pop(); } else { *modes.last_mut().unwrap() = Mode::Code { depth: depth.saturating_sub(1), html }; }
                    if let Some(len) = raw_len { if sc.stack.len() <= len { raw_len = None; } }
                    continue;
                }
                let nx = c.get(i + 1).copied();
                let starts = nx.map_or(false, |x| x.is_ascii_alphabetic() || x == '!') && matches!(last, Last::Open(_))
                    || nx.map_or(false, |x| x.is_ascii_alphabetic()) && last == Last::Mk;
                if !raw && ch == '<' && starts {
                    modes.push(Mode::Content { depth: 0 });
                    continue;
                }
                if ch.is_whitespace() { i += 1; continue; }
                if ch == '/' && nx == Some('/') {
                    let s = i;
                    while i < n && c[i] != '\n' { i += 1; }
                    let mut e = i;
                    while e > s && c[e - 1].is_whitespace() { e -= 1; }
                    sc.emit(src, offs[s], offs[e], K::Comment, false, None, None);
                    continue;
                }
                let plain = !raw;
                if ch.is_alphabetic() || ch == '_' {
                    let s = i;
                    while i < n && (c[i].is_alphanumeric() || c[i] == '_') { i += 1; }
                    if &src[offs[s]..offs[i]] == "web" && sc.stack.is_empty() && modes.len() == 1 {
                        let mut j = i; while j < n && c[j].is_whitespace() { j += 1; }
                        if c.get(j) == Some(&'{') { raw_pending = true; }
                    }
                    sc.emit(src, offs[s], offs[i], K::Word, plain, None, None);
                    last = Last::Other;
                } else if ch.is_ascii_digit() {
                    let s = i;
                    if ch == '0' && matches!(nx, Some('x') | Some('X')) && c.get(i + 2).map_or(false, |d| d.is_ascii_hexdigit()) {
                        i += 2; while i < n && c[i].is_ascii_hexdigit() { i += 1; }
                    } else {
                        while i < n && c[i].is_ascii_digit() { i += 1; }
                        if c.get(i) == Some(&'.') && c.get(i + 1).map_or(false, |d| d.is_ascii_digit()) { i += 1; while i < n && c[i].is_ascii_digit() { i += 1; } }
                    }
                    sc.emit(src, offs[s], offs[i], K::Num, plain, None, None);
                    last = Last::Other;
                } else if ch == '"' {
                    let s = i;
                    i += 1;
                    while i < n && c[i] != '"' { if c[i] == '\\' && i + 1 < n { i += 1; } i += 1; }
                    if i >= n { return Err(format!("{}: a text that is never closed", place(&sc, s))); }
                    i += 1;
                    if src[offs[s]..offs[i]].contains('\n') { sc.prot.push((offs[s], offs[i])); }
                    sc.emit(src, offs[s], offs[i], K::Str, plain, None, None);
                    last = Last::Other;
                } else {
                    let two: String = c[i..(i + 2).min(n)].iter().collect();
                    let len = if TWO.contains(&two.as_str()) { 2 } else { 1 };
                    let (opener, closer) = match ch { '(' | '[' => (Some(Ent::Paren), None), ')' | ']' => (None, Some(Ent::Paren)), _ => (None, None) };
                    sc.emit(src, offs[i], offs[i + len], K::Sym, plain, closer, opener);
                    last = if len == 1 && matches!(ch, '(' | ',' | ';') { Last::Open(ch) } else { Last::Other };
                    i += len;
                }
            }
        }
    }
    match modes.last() { Some(Mode::Code { html: false, .. }) => {} _ => return Err(format!("{}: markup that is never closed (a closing tag, `\"` or `}}` is missing)", place(&sc, n.saturating_sub(1)))) }
    sc.fill(nl);
    Ok(sc)
}

const SPACED: [&str; 13] = ["==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "=>", "->", "="];
const TRAIL: [&str; 13] = ["&&", "||", "+", "*", "/", "%", "=", "==", "!=", "<=", ">=", "=>", "->"];
const LEAD: [&str; 9] = ["&&", "||", "+", "*", "/", "%", "==", "!=", "=>"];

/// What the white space between two code tokens on one line should be: Some(text), or None to keep it (one space at most).
fn want(a: &str, ak: K, b: &str, a_bin: bool, b_bin: bool) -> Option<&'static str> {
    let op = |t: &str, bin: bool| SPACED.contains(&t) || (bin && matches!(t, "+" | "*" | "/" | "%"));
    if matches!(b, "," | ";" | ")" | "]") { return Some(""); }
    if matches!(a, "(" | "[") { return Some(""); }
    if b == ":" { return Some(""); }
    if a == ":" || a == "," { return Some(" "); }
    if op(a, a_bin) || op(b, b_bin) { return Some(" "); }
    if a == "{" && b == "}" { return None; }
    if a == "{" || b == "}" { return Some("*"); }
    if b == "{" && (a == ")" || a == ">" || ak == K::Word) { return Some("*"); }
    None
}

pub struct Opts { pub raw_all: bool, pub keep_blank: bool }

/// The formatted text of one program file (`Err`: it does not lex as a program, so it is left alone).
pub fn format(src: &str) -> Result<String, String> { format_with(src, &Opts { raw_all: crate::source::is_system(src), keep_blank: false }) }

pub fn format_with(src: &str, o: &Opts) -> Result<String, String> {
    let sc = scan(src, o.raw_all)?;
    let nl = sc.ls.len();
    let le = |l: usize| if l + 1 < nl { sc.ls[l + 1] - 1 } else { src.len() };
    let mut prot_start = vec![false; nl];
    for &(s, e) in &sc.prot {
        let mut l = sc.line_of(s) + 1;
        while l < nl && sc.ls[l] <= e { prot_start[l] = true; l += 1; }
    }
    let prot_end = |l: usize| l + 1 < nl && prot_start[l + 1];
    let text = |l: usize| &src[sc.ls[l]..le(l)];
    let blank = |l: usize| !prot_start[l] && !prot_end(l) && text(l).trim().is_empty();
    let sym = |t: &Tk| if t.k == K::Sym { &src[t.s..t.e] } else { "" };
    // indentation of each line
    let mut ind: Vec<Option<usize>> = vec![None; nl];
    for l in 0..nl {
        if prot_start[l] || blank(l) { continue; }
        let Some(f) = sc.first[l] else { ind[l] = Some(sc.base[l] * 4); continue };
        let ft = &sc.toks[f];
        if ft.k == K::Comment { continue; }
        let mut v = sc.lead[l].unwrap_or(sc.base[l]) * 4;
        // inside a bracket that opened on an earlier line: keep the line's offset from that line (aligned or hanging, as written)
        if let (None, Some(ol), true) = (sc.lead[l], sc.top_paren[l], ft.plain) {
            if ol < l { if let Some(oi) = ind[ol] {
                let old = |m: usize| { let t = text(m); (t.len() - t.trim_start().len()) as isize };
                ind[l] = Some((oi as isize + old(l) - old(ol)).max(0) as usize);
                continue;
            } }
        }
        if sc.lead[l].is_none() && ft.plain && sc.top_paren[l].is_none() {
            let cont_lead = LEAD.contains(&sym(ft));
            let cont_trail = sc.toks[..f].iter().rev().find(|t| t.k != K::Comment).map_or(false, |t| t.plain && TRAIL.contains(&sym(t)));
            if cont_lead || cont_trail { v += 4; }
        }
        ind[l] = Some(v);
    }
    // an own-line comment is indented like the line after it (like the block's inside, before a closing line)
    for l in (0..nl).rev() {
        if prot_start[l] || blank(l) { continue; }
        let Some(f) = sc.first[l] else { continue };
        if sc.toks[f].k != K::Comment { continue; }
        let next = (l + 1..nl).find(|&m| !blank(m));
        ind[l] = Some(match next {
            None => sc.base[l] * 4,
            Some(m) if sc.lead[m].is_some() => sc.base[l] * 4,
            Some(m) => ind[m].unwrap_or(sc.base[l] * 4),
        });
    }
    let mut last_on: Vec<Option<usize>> = vec![None; nl];
    for (k, t) in sc.toks.iter().enumerate() { last_on[sc.line_of(t.s)] = Some(k); }
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let (mut seen, mut prev_blank, mut prev_open) = (false, false, false);
    let last_nb = (0..nl).rev().find(|&l| !blank(l));
    for l in 0..nl {
        let (s, e) = (sc.ls[l], le(l));
        let whole_end = if l + 1 < nl { sc.ls[l + 1] } else { src.len() };
        if blank(l) {
            let tail = last_nb.map_or(true, |m| l > m);
            if tail || (!o.keep_blank && (!seen || prev_blank || prev_open)) { edits.push((s, whole_end, String::new())); }
            else { if e > s { edits.push((s, e, String::new())); } prev_blank = true; }
            continue;
        }
        seen = true; prev_blank = false;
        prev_open = last_on[l].map_or(false, |k| sc.toks[k].plain && sym(&sc.toks[k]) == "{");
        let t = text(l);
        if !prot_start[l] {
            let lead_len = t.len() - t.trim_start().len();
            let want_ind = " ".repeat(ind[l].unwrap_or(0));
            if &t[..lead_len] != want_ind { edits.push((s, s + lead_len, want_ind)); }
        }
        if !prot_end(l) {
            let tl = t.trim_end().len();
            if tl < t.len() { edits.push((s + tl, e, String::new())); }
        }
    }
    // spaces between tokens
    for i in 1..sc.toks.len() {
        let (a, b) = (&sc.toks[i - 1], &sc.toks[i]);
        let gap = &src[a.e..b.s];
        if gap.contains('\n') || !gap.chars().all(|c| c == ' ' || c == '\t') { continue; }
        if b.k == K::Comment { if gap.is_empty() { edits.push((a.e, a.e, "  ".into())); } continue; }
        if a.k == K::Comment || !a.plain || !b.plain { continue; }
        // `+ * / %` is an operator between two operands (not a sign or a doubled `++`)
        let bin = |k: usize| k > 0 && { let t = &sc.toks[k - 1]; t.plain && (matches!(t.k, K::Word | K::Num | K::Str) || matches!(sym(t), ")" | "]")) }
            && sc.toks.get(k + 1).map_or(true, |t| t.k != K::Sym || matches!(sym(t), "(" | "[" | "{"));
        let Some(w) = want(sym(a), a.k, sym(b), bin(i - 1), bin(i)) else { continue };
        let w = if w == "*" { if gap.is_empty() { " " } else { continue } } else { w }; // "*": at least one space
        if gap != w { edits.push((a.e, b.s, w.to_string())); }
    }
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    for (s, e, r) in &edits {
        if *s < at { return Err("internal: overlapping edits".into()); }
        out.push_str(&src[at..*s]); out.push_str(r); at = *e;
    }
    out.push_str(&src[at..]);
    if !out.is_empty() && !out.ends_with('\n') { out.push('\n'); }
    Ok(out)
}

/// `mito fmt <file.mito>... [--check] [--write]`
pub fn run(args: &[String]) {
    let files: Vec<&String> = args.iter().take_while(|a| !a.starts_with('-')).collect();
    let (mut check, mut write) = (false, false);
    for a in &args[files.len()..] {
        match a.as_str() { "--check" => check = true, "--write" => write = true, o => { eprintln!("fmt: unknown option `{}` (usage: mito fmt <file.mito>... [--check] [--write])", o); std::process::exit(2) } }
    }
    if files.is_empty() || (check && write) || (!check && !write && files.len() != 1) {
        eprintln!("usage: mito fmt <file.mito>... [--check|--write]   (without an option, one file, printed formatted)");
        std::process::exit(2);
    }
    let mut code = 0;
    for f in files {
        let text = match std::fs::read_to_string(f) { Ok(t) => t, Err(e) => { eprintln!("{}: {}", f, e); code = 2; continue } };
        let out = match format(&text) { Ok(o) => o, Err(e) => { eprintln!("{}:{}", f, e); code = 2; continue } };
        if check { if out != text { println!("{}", f); if code == 0 { code = 1; } } }
        else if write { if out != text { if let Err(e) = std::fs::write(f, &out) { eprintln!("{}: {}", f, e); code = 2; } } }
        else { print!("{}", out); }
    }
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")) }

    fn f(s: &str) -> String { format(s).unwrap_or_else(|e| panic!("{}: {}", e, s)) }

    #[test]
    fn small_programs_are_formatted() {
        let table: &[(&str, &str)] = &[
            // spaces, braces, operators, commas
            ("fn f(x:i64,y:i64)->i64{return x+y;}", "fn f(x: i64, y: i64) -> i64 { return x + y; }\n"),
            ("fn f( a ,b ) { let x=a==b; }", "fn f(a, b) { let x = a == b; }\n"),
            ("fn f() {\n  x +=1 ;\n}\n", "fn f() {\n    x += 1;\n}\n"),
            // an already compact one-line function stays as it is
            ("fn f(x: i64) -> i64 { return x + 1; }\n", "fn f(x: i64) -> i64 { return x + 1; }\n"),
            // blank lines: one at most, none at the start of a block or the file, none at the end
            ("\n\nfn f() {\n\n\n    a();\n\n\n\n    b();\n}\n\n\n", "fn f() {\n    a();\n\n    b();\n}\n"),
            // line endings and trailing white space
            ("fn f() {\r\n    a();   \r\n}", "fn f() {\n    a();\n}\n"),
            // else if chains
            ("fn f() {\nif a {\nx();\n}\nelse if b {\ny();\n} else {\nz();\n}\n}\n", "fn f() {\n    if a {\n        x();\n    }\n    else if b {\n        y();\n    } else {\n        z();\n    }\n}\n"),
            ("fn f() {\n        if a {\n            x();\n        } else if b {\n  y();\n }\n}\n", "fn f() {\n    if a {\n        x();\n    } else if b {\n        y();\n    }\n}\n"),
            // comments are kept; own-line comments take the next line's indent (the block's inside before a closing brace)
            ("fn f() {\n// before\n        a(); // trailing\n        b();// tight\n    x();      // aligned\n  // last\n}\n", "fn f() {\n    // before\n    a(); // trailing\n    b();  // tight\n    x();      // aligned\n    // last\n}\n"),
            // strings with braces, slashes and commas are not touched
            ("fn f() {\n  let s = \"a,b {  } // x ;\";\n  let t=\"\\\"{\";\n}\n", "fn f() {\n    let s = \"a,b {  } // x ;\";\n    let t = \"\\\"{\";\n}\n"),
            // a text that spans lines keeps its inside
            ("fn f() {\n  let s = \"one\n   two  \nthree\";\n  g();\n}\n", "fn f() {\n    let s = \"one\n   two  \nthree\";\n    g();\n}\n"),
            // attributes, foreach clauses, match arms
            ("@get(\"/x\")\nfn x() -> Response<html> {\nreturn ok(<p>hi</p>);\n}\n", "@get(\"/x\")\nfn x() -> Response<html> {\n    return ok(<p>hi</p>);\n}\n"),
            ("fn f() {\nforeach (let x,i in arr) {\nsum+=x*i;\n}\n}\n", "fn f() {\n    foreach (let x, i in arr) {\n        sum += x * i;\n    }\n}\n"),
            ("fn f(n: i64) -> i64 {\nmatch n {\n0=>1,\n_=>n*2,\n}\n}\n", "fn f(n: i64) -> i64 {\n    match n {\n        0 => 1,\n        _ => n * 2,\n    }\n}\n"),
            // unary minus and generics are left alone
            ("fn f(x: table<Todo>) -> i64 { return -1; }\n", "fn f(x: table<Todo>) -> i64 { return -1; }\n"),
            // continuation lines of one statement
            ("fn f() {\nlet x = a &&\nb;\nlet y = c\n|| d;\n}\n", "fn f() {\n    let x = a &&\n        b;\n    let y = c\n        || d;\n}\n"),
            // arguments over several lines
            ("fn f() {\ng(a,\nb);\n}\n", "fn f() {\n    g(a,\n    b);\n}\n"),
            ("fn f() {\n   g(a,\n       b);\n}\n", "fn f() {\n    g(a,\n        b);\n}\n"),
            ("fn f() { var i = 0; i++; }\n", "fn f() { var i = 0; i++; }\n"),
            // markup: only line indentation and trailing white space; text and spacing between tags are kept
            ("fn p() -> html {\n<ul class=\"a  b\">\n<li>  one   two </li>\n  <li><b>x</b> <i>y</i></li>\n</ul>\n}\n", "fn p() -> html {\n    <ul class=\"a  b\">\n        <li>  one   two </li>\n        <li><b>x</b> <i>y</i></li>\n    </ul>\n}\n"),
            ("fn p(l: i64) -> html {\n<div>\n{if l > 1 {\n<p>many</p>\n}}\n  text across\n      lines\n</div>\n}\n", "fn p(l: i64) -> html {\n    <div>\n        {if l > 1 {\n            <p>many</p>\n        }}\n        text across\n        lines\n    </div>\n}\n"),
            // code inside markup: spacing is normalized in the code only
            ("fn p(l: i64) -> html {\n<p class=\"{if l==1 { \"a\" }}\"  >{l}  and {if l!=1 {\"s\"}}</p>\n}\n", "fn p(l: i64) -> html {\n    <p class=\"{if l == 1 { \"a\" }}\"  >{l}  and {if l != 1 { \"s\" }}</p>\n}\n"),
            // a quoted attribute over several lines, and an HTML comment, are left as they are
            ("fn p() -> html {\n<div>\n<p class=\"a\n     b  \">x</p>\n<!-- one\n   two -->\n</div>\n}\n", "fn p() -> html {\n    <div>\n        <p class=\"a\n     b  \">x</p>\n        <!-- one\n   two -->\n    </div>\n}\n"),
            // `//` in markup text is text, not a comment
            ("fn p() -> html {\n<a href=\"http://x\">\n// not a comment  \n</a>\n}\n", "fn p() -> html {\n    <a href=\"http://x\">\n        // not a comment\n    </a>\n}\n"),
            // a tag over several lines
            ("fn p() -> html {\n<input class=\"a\"\n  type=\"text\"\n  checked={x}>\n}\n", "fn p() -> html {\n    <input class=\"a\"\n        type=\"text\"\n        checked={x}>\n}\n"),
            // a system file: indentation only
            ("system {\n  worker auth  { program \"a.mito\"; count 2; }\n   entry auth;\n}\n", "system {\n    worker auth  { program \"a.mito\"; count 2; }\n    entry auth;\n}\n"),
        ];
        for (before, after) in table {
            assert_eq!(&f(before), after, "formatting {:?}", before);
            assert_eq!(&f(after), after, "idempotence of {:?}", after);
        }
    }

    #[test]
    fn unlexable_text_is_refused() {
        assert!(format("fn f() { let s = \"open; }").is_err());
        assert!(format("fn f() -> html { <p>x }").is_err());
    }

    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        v.sort();
        for p in v {
            if p.is_dir() { walk(&p, out); } else if p.extension().map_or(false, |x| x == "mito") { out.push(p); }
        }
    }

    /// Every program text in the repository: its files, and the programs inside the cases. (name, text, path if it is a file)
    fn programs() -> Vec<(String, String, Option<PathBuf>)> {
        let mut files = Vec::new();
        for d in ["examples", "lib", "bench", "tests", "tools"] { walk(&root().join(d), &mut files); }
        let mut out: Vec<(String, String, Option<PathBuf>)> = files.into_iter().map(|p| {
            let t = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {}", p.display(), e));
            (p.strip_prefix(root()).unwrap().to_string_lossy().replace('\\', "/"), t, Some(p))
        }).collect();
        let mut cases: Vec<PathBuf> = std::fs::read_dir(root().join("tests/cases")).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "case")).collect();
        cases.sort();
        for p in cases {
            let text = std::fs::read_to_string(&p).unwrap();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let mut cur: Option<(String, String)> = None;
            let mut done: Vec<(String, String)> = Vec::new();
            for line in text.lines() {
                if let Some(s) = line.trim_end().strip_prefix("--- ") { if let Some(c) = cur.take() { done.push(c); } cur = Some((s.to_string(), String::new())); continue; }
                if let Some(c) = cur.as_mut() { c.1.push_str(line); c.1.push('\n'); }
            }
            if let Some(c) = cur.take() { done.push(c); }
            for (sec, body) in done {
                if sec == "program" || sec == "system" || (sec.starts_with("file ") && sec.ends_with(".mito")) { out.push((format!("{} [{}]", name, sec), body, None)); }
            }
        }
        out
    }

    /// The generated C with its `//` comment lines (which quote the source, white space and all) stripped of white space: the only
    /// normalization; code lines must be identical.
    fn norm_c(c: &str) -> String {
        c.lines().map(|l| if l.trim_start().starts_with("//") { l.split_whitespace().collect::<String>() } else { l.to_string() }).collect::<Vec<_>>().join("\n")
    }

    fn squeeze(s: &str) -> String { s.chars().filter(|c| !c.is_whitespace()).collect() }

    /// The generated C of a program (its `use`d files expanded in place, formatted first when `fmt_keep_blank`), or None when it does not compile.
    fn c_of(text: &str, dir: Option<&Path>, fmt_keep_blank: bool) -> Option<String> {
        fn read(t: &str, fmt: bool) -> String {
            if !fmt { return t.to_string(); }
            format_with(t, &Opts { raw_all: crate::source::is_system(t), keep_blank: true }).unwrap_or_else(|_| t.to_string())
        }
        fn expand(dir: Option<&Path>, text: &str, fmt: bool, depth: u32, out: &mut String) -> Option<()> {
            if depth > 8 { return None; }
            for line in read(text, fmt).split('\n') {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("use \"") {
                    let name = rest.split('"').next()?;
                    let p = dir?.join(name);
                    let inner = std::fs::read_to_string(&p).ok()?;
                    expand(p.parent(), &inner, fmt, depth + 1, out)?;
                    out.push('\n');
                } else if t.starts_with("include ") { return None; }
                else { out.push_str(line); out.push('\n'); }
            }
            Some(())
        }
        let mut full = String::new();
        expand(dir, text, fmt_keep_blank, 0, &mut full)?;
        if crate::source::is_system(&full) || crate::web::find(&full).is_some() { return None; }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (state, code) = crate::source::load(&full).ok()?;
            let prog = crate::layout::Program::parse(&state).ok()?;
            let api = crate::cgen::check(&prog, &code).ok()?;
            crate::cgen::emit(&prog, &api, ("prog.mito", &code)).ok()
        })).ok().flatten()
    }

    #[test]
    fn every_program_in_the_repository_formats_idempotently_and_keeps_its_meaning() {
        let progs = programs();
        let (mut refused, mut changed, mut tokens_same, mut c_same, mut c_total, mut mangled_n) = (0, 0, 0, 0, 0, 0);
        for (name, text, path) in &progs {
            let raw = crate::source::is_system(text);
            let Ok(out) = format(text) else { refused += 1; eprintln!("refused: {}", name); continue };
            if out != *text {
                changed += 1;
                if let Some((a, b)) = text.lines().zip(out.lines()).find(|(a, b)| a != b) { eprintln!("would change {}: {:?} -> {:?}", name, a.chars().take(90).collect::<String>(), b.chars().take(90).collect::<String>()); }
                else { eprintln!("would change {}: line count or end of file", name); }
            }
            // idempotent
            assert_eq!(format(&out).as_deref(), Ok(out.as_str()), "{}: not idempotent", name);
            // the same again from a harsher start: every line's indentation removed, then every gap of two or more spaces made one
            for mangled in [text.lines().map(|l| l.trim_start()).collect::<Vec<_>>().join("\n") + "\n", text.replace("  ", " ")] {
                let Ok(o2) = format(&mangled) else { continue };
                assert_eq!(format(&o2).as_deref(), Ok(o2.as_str()), "{}: not idempotent after mangling", name);
                assert_eq!(squeeze(&o2), squeeze(&mangled), "{}: more than white space changed after mangling", name);
                if !raw { if let (Ok(a), Ok(b)) = (crate::lang::tokens(&mangled), crate::lang::tokens(&o2)) { assert!(a == b, "{}: tokens differ after mangling", name); } }
                mangled_n += 1;
            }
            // only white space differs
            assert_eq!(squeeze(&out), squeeze(text), "{}: more than white space changed", name);
            // the lexer sees the same tokens (markup text included, as the compiler joins it)
            if !raw {
                match (crate::lang::tokens(text), crate::lang::tokens(&out)) {
                    (Ok(a), Ok(b)) => { assert!(a == b, "{}: tokens differ", name); tokens_same += 1; }
                    (Err(_), Err(_)) => {}
                    (a, b) => panic!("{}: lexes before: {}, after: {}", name, a.is_ok(), b.is_ok()),
                }
            }
            // the generated C is the same byte for byte (blank lines kept, so line numbers in it stay)
            let dir = path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()));
            if let Some(before) = c_of(text, dir.as_deref(), false) {
                c_total += 1;
                let after = c_of(text, dir.as_deref(), true).unwrap_or_else(|| panic!("{}: compiled before formatting, not after", name));
                let (before, after) = (norm_c(&before), norm_c(&after));
                assert!(before == after, "{}: generated C differs at {:?}", name, before.lines().zip(after.lines()).find(|(a, b)| a != b));
                c_same += 1;
            }
        }
        eprintln!("fmt: {} programs ({} refused as not lexable, {} would change), tokens equal in {}, generated C equal in {} of {} that compile, {} mangled variants idempotent", progs.len(), refused, changed, tokens_same, c_same, c_total, mangled_n);
        assert!(progs.len() > 200 && c_total > 100, "the walk found too little");
    }
}
