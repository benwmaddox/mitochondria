//! `.system` files: the worker kinds of a system, how many of each, their mailboxes, and the system's limits.

const MB: usize = 1 << 20;

pub struct KindSpec { pub name: String, pub state: String, pub api: String, pub count: usize, pub mailbox: usize, pub slot: usize, pub soft: Option<usize>, pub hard: Option<usize> }
/// `budget`: the most memory every worker's declared state may reserve together (checked when the system is built).
pub struct Spec { pub kinds: Vec<KindSpec>, pub entry: usize, pub inflight: usize, pub budget: usize }

/// Parse a `.system` file. Paths in it are relative to `dir`.
///
/// ```text
/// worker auth  { state "auth.mito";  api "auth.api";  count 2; mailbox 1024; }
/// worker rooms { state "rooms.mito"; api "rooms.api"; count 4; mailbox 1024; slot 8192; soft 64; hard 256; }
/// entry auth;
/// inflight 4096;
/// budget 4096;
/// ```
pub fn parse_spec(src: &str, dir: &std::path::Path) -> Result<Spec, String> {
    parse_spec_with(src, &|f| std::fs::read_to_string(dir.join(f)).map_err(|e| format!("{}: {}", f, e)))
}

/// Parse a `.system` file, reading the files it names with `read`.
pub fn parse_spec_with(src: &str, read: &dyn Fn(&str) -> Result<String, String>) -> Result<Spec, String> {
    let toks = tokens(src);
    let mut i = 0;
    let mut spec = Spec { kinds: Vec::new(), entry: usize::MAX, inflight: 4096, budget: 4096 * MB };
    let mut entry = String::new();
    let next = |i: &mut usize| -> Result<String, String> { let t = toks.get(*i).cloned().ok_or("unexpected end of system file")?; *i += 1; Ok(t) };
    let num = |s: String| -> Result<usize, String> { s.parse().map_err(|_| format!("expected a number, found `{}`", s)) };
    while i < toks.len() {
        match next(&mut i)?.as_str() {
            "worker" => {
                let name = next(&mut i)?;
                if next(&mut i)? != "{" { return Err("expected `{`".into()); }
                let mut k = KindSpec { name, state: String::new(), api: String::new(), count: 1, mailbox: 1024, slot: 8192, soft: None, hard: None };
                loop {
                    let key = next(&mut i)?;
                    if key == "}" { break; }
                    let v = next(&mut i)?;
                    match key.as_str() {
                        "state" => k.state = read(&v)?, "api" => k.api = read(&v)?,
                        "count" => k.count = num(v)?.max(1), "mailbox" => k.mailbox = num(v)?.max(1), "slot" => k.slot = num(v)?.max(256),
                        "soft" => k.soft = Some(num(v)? * MB), "hard" => k.hard = Some(num(v)? * MB),
                        o => return Err(format!("unknown worker setting `{}`", o)),
                    }
                    if next(&mut i)? != ";" { return Err(format!("expected `;` after `{}`", key)); }
                }
                let (st, api) = crate::modules::expand(&k.state, &k.api).map_err(|e| format!("worker {}: {}", k.name, e))?;
                k.state = st; k.api = api;
                spec.kinds.push(k);
            }
            "entry" => { entry = next(&mut i)?; next(&mut i)?; }
            "inflight" => { spec.inflight = num(next(&mut i)?)?.max(1); next(&mut i)?; }
            "budget" => { spec.budget = num(next(&mut i)?)? * MB; next(&mut i)?; }
            o @ ("spin" | "batch" | "compile") => return Err(format!("`{}` was a setting of the Rust system, which was removed; the C system has no such setting", o)),
            o => return Err(format!("expected `worker`, `entry`, `inflight` or `budget`, found `{}`", o)),
        }
    }
    spec.entry = spec.kinds.iter().position(|k| k.name == entry).ok_or(format!("entry `{}` is not a declared worker", entry))?;
    Ok(spec)
}
fn tokens(src: &str) -> Vec<String> {
    let c: Vec<char> = src.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    while i < c.len() {
        if c[i].is_whitespace() { i += 1; }
        else if c[i] == '/' && c.get(i + 1) == Some(&'/') { while i < c.len() && c[i] != '\n' { i += 1; } }
        else if c[i] == '"' { let s = i + 1; i += 1; while i < c.len() && c[i] != '"' { i += 1; } out.push(c[s..i].iter().collect()); i += 1; }
        else if c[i].is_alphanumeric() || c[i] == '_' || c[i] == '.' { let s = i; while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '.') { i += 1; } out.push(c[s..i].iter().collect()); }
        else { out.push(c[i].to_string()); i += 1; }
    }
    out
}
