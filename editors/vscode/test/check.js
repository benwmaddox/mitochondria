// Checks the TextMate grammar with the real engine VS Code uses (vscode-textmate + vscode-oniguruma).
// Run: node check.js [path to a folder holding node_modules with those two packages]   (see ../README.md)
const fs = require('fs');
const NL = String.fromCharCode(10);
const path = require('path');
const modDir = process.argv[2] || process.env.MITO_TM_MODULES || path.join(__dirname, '..', 'node_modules');
const req = (n) => require(require.resolve(n, { paths: [modDir, __dirname] }));
const vsctm = req('vscode-textmate');
const onig = req('vscode-oniguruma');

const grammarPath = path.join(__dirname, '..', 'syntaxes', 'mitochondria.tmLanguage.json');
const raw = fs.readFileSync(grammarPath, 'utf8');
const grammarJson = JSON.parse(raw); // must parse
const sample = fs.readFileSync(path.join(__dirname, 'sample.mito'), 'utf8');

// [text of the line to find, token text, scope it must have (prefix of one scope), nth occurrence of the token on the line]
// A leading "!" on the scope means: must NOT have it.
const T = (line, tok, scope, nth = 0) => ({ line, tok, scope, nth });
const expectations = [
  T('// A sample', '// A sample that uses every construct of the language; test/check.js checks how it is colored.', 'comment.line'),
  T('platform web;', 'platform', 'storage.type'), T('platform web;', 'web', 'support.constant'),
  T('use "pages.mito"', 'use', 'keyword.control.import'), T('use "pages.mito"', 'pages.mito', 'string.quoted'),
  T('module sorted<', 'module', 'storage.type'), T('module sorted<', 'sorted', 'entity.name.type.module'),
  T('module sorted<', 'struct', 'support.type'), T('module sorted<', 'count', 'support.type'), T('module sorted<', 'field', 'support.type'), T('module sorted<', 'of', 'storage.modifier'),
  T('enum Color', 'enum', 'storage.type'), T('enum Color', 'Color', 'entity.name.type'), T('enum Color', 'u8', 'support.type.primitive'), T('enum Color', '1', 'constant.numeric'),
  T('struct Player', 'struct', 'storage.type'), T('struct Player', 'utf8', 'support.type.primitive'), T('struct Player', 'i32', 'support.type.primitive'),
  T('struct Player', 'f32', 'support.type.primitive'), T('struct Player', 'bool', 'support.type.primitive'), T('struct Player', 'name', '!entity'), T('struct Player', '4', 'constant.numeric'),
  T('players: pool<', 'pool', 'support.type.collection'), T('players: pool<', 'checked', 'storage.modifier'),
  T('log: list<', 'list', 'support.type.collection'), T('log: list<', 'drop_oldest', 'storage.modifier'),
  T('users: table<', 'table', 'support.type.collection'), T('users: table<', 'unique', 'storage.modifier'), T('users: table<', 'index', 'storage.modifier'),
  T('board: sorted<', 'sorted', 'support.type.collection'), T('board: sorted<', 'logged', 'storage.modifier'),
  T('mask: u64', 'u64', 'support.type.primitive'),
  T('state app', 'state', 'storage.type'), T('state app', 'app', 'variable.other.global'),
  T('request seen', 'request', 'storage.type'),
  T('system {', 'system', 'storage.type'), T('worker rooms', 'worker', 'keyword.other.config'), T('worker rooms', 'program', 'keyword.other.config'),
  T('worker rooms', 'rooms.mito', 'string.quoted'), T('worker rooms', '4', 'constant.numeric'), T('budget 8192', 'budget', 'keyword.other.config'), T('budget 8192', '// MB', 'comment.line'),
  T('web {', 'web', 'storage.type'), T('gzip;', 'gzip', 'keyword.other.config'), T('static "/"', 'static', 'keyword.other.config'),
  T('fn hurt', 'fn', 'storage.type.function'), T('fn hurt', 'hurt', 'entity.name.function'), T('fn hurt', 'self', 'variable.language'), T('fn hurt', 'Player', 'entity.name.type'),
  T('self.hp -= n', 'self', 'variable.language'),
  T('pub fn big', 'pub', 'storage.modifier'), T('pub fn big', 'big', 'entity.name.function'),
  T('let a = 0xFF', 'let', 'storage.type'), T('let a = 0xFF', 'a', 'variable.other'), T('let a = 0xFF', '0xFF', 'constant.numeric.hex'),
  T('let a = 0xFF', '12', 'constant.numeric.integer'), T('let a = 0xFF', '1.5', 'constant.numeric.float'),
  T('var b = x as u8', 'var', 'storage.type'), T('var b = x as u8', 'as', 'keyword.operator.word'), T('var b = x as u8', 'u8', 'support.type.primitive'),
  T('for var i', 'for', 'keyword.control'), T('for var i', 'continue', 'keyword.control'), T('for var i', 'break', 'keyword.control'),
  T('for var i', 'if', 'keyword.control'), T('for var i', 'else', 'keyword.control'),
  T('foreach p in', 'foreach', 'keyword.control'), T('foreach p in', 'in', 'keyword.operator.word'), T('foreach p in', 'reverse', 'keyword.control'),
  T('foreach p in', 'where', 'keyword.control'), T('foreach p in', 'limit', 'keyword.control'), T('p.hurt(3)', 'hurt', 'entity.name.function.call'),
  T('foreach i in 0..10', '..', 'keyword.operator.range'), T('foreach i in 0..10', 'while', 'keyword.control'),
  T('foreach i in 0..10', 'wrapping_add', 'support.function.builtin'),
  T('foreach u in', 'by', 'keyword.control'), T('foreach u in', 'after', 'keyword.control'), T('foreach u in', 'null', 'constant.language'),
  T('foreach u in', 'add', 'support.function.builtin'),
  T('match x', 'match', 'keyword.control'), T('match x', '=>', 'keyword.operator'), T('match x', 'return', 'keyword.control'), T('match x', 'fail', 'keyword.control'),
  T('match x', '..', 'keyword.operator.range'), T('match x', '\\n', 'constant.character.escape'), T('match x', '\\t', 'constant.character.escape'),
  T('match x', '\\"', 'constant.character.escape'), T('match x', '\\u{1F600}', 'constant.character.escape'), T('match x', '\\q', 'invalid.illegal.escape'),
  T('return min(abs', 'min', 'support.function.builtin'), T('return min(abs', 'abs', 'support.function.builtin'), T('return min(abs', 'len', 'support.function.builtin'),
  T('return min(abs', 'or', 'support.function.builtin'),
  T('test "big works"', 'test', 'storage.type.test'), T('test "big works"', 'big works', 'string.quoted'),
  T('test "big works"', 'true', 'constant.language'), T('test "big works"', 'false', 'constant.language'), T('test "big works"', '&&', 'keyword.operator'),
  T('@get("/players/{p}")', '@', 'punctuation.definition.annotation'), T('@get("/players/{p}")', 'get', 'entity.other.attribute-name'),
  T('@get("/players/{p}")', '{p}', 'variable.parameter'), T('@get("/players/{p}")', '/players/', 'string.quoted'),
  T('fn show(', 'string', 'support.type.primitive'), T('fn show(', 'from', 'keyword.control'), T('fn show(', 'query', 'support.constant.input-source'),
  T('fn show(', 'header', 'support.function.input-source'), T('fn show(', 'cookie', 'support.function.input-source'), T('fn show(', 'body', 'support.constant.input-source'),
  T('fn show(', 'Response', 'support.type.collection'), 
  T('return ok(p)', 'ok', 'support.function.builtin'),
  T('@post("/players")', 'post', 'entity.other.attribute-name'), T('@post("/players")', 'created', 'support.function.builtin'),
  T('@put("/p")', 'put', 'entity.other.attribute-name'), T('@put("/p")', 'text', 'support.type'), T('@put("/p")', 'no_content', 'support.function.builtin'),
  T('@patch', 'patch', 'entity.other.attribute-name'), T('@delete', 'delete', 'entity.other.attribute-name'),
  T('@socket("/ws")', 'socket', 'entity.other.attribute-name'), T('@socket_message', 'socket_message', 'entity.other.attribute-name'),
  T('@socket_close', 'socket_close', 'entity.other.attribute-name'),
  T('@every(1000)', 'every', 'entity.other.attribute-name'), T('@every(1000)', '1000', 'constant.numeric'),
  T('@start fn init', 'start', 'entity.other.attribute-name'),
  T('@main fn main', 'main', 'entity.other.attribute-name', 0), T('@main fn main', 'arg', 'support.function.input-source'), T('@main fn main', 'stdin', 'support.constant.input-source'),
  T('forward rooms[0]', 'forward', 'keyword.control'),
  T('@frame fn frame', 'frame', 'entity.other.attribute-name'), T('@frame fn frame', 'time', 'support.constant.input-source'),
  T('@frame fn frame', 'key', 'support.function.input-source'), T('@frame fn frame', 'mouse', 'support.function.input-source'),
  T('@shader fn scene', 'shader', 'entity.other.attribute-name'),
  T('-> html {', 'html', 'support.type'),
  // markup
  T('<li class=', '<', 'punctuation.definition.tag.begin.html'), T('<li class=', 'li', 'entity.name.tag.html'),
  T('<li class=', 'class', 'entity.other.attribute-name.html'), T('<li class=', 'item ', 'string.quoted.double.html'),
  T('<li class=', '{', 'punctuation.section.embedded'), T('<li class=', 't.hp', '!string'), T('<li class=', 'checked', 'entity.other.attribute-name.html'),
  T('<li class=', 't.ok', '!string'), T('<li class=', '&amp;', 'constant.character.entity'), T('<li class=', 'br', 'entity.name.tag.html'),
  T('<li class=', 'input', 'entity.name.tag.html'), T('<li class=', 'type', 'entity.other.attribute-name.html'), T('<li class=', 't.name', '!entity.other.attribute-name'),
  T('<li class=', 'if', 'keyword.control'), T('<li class=', 'yes', '!keyword'), T('<li class=', 'i', 'entity.name.tag.html'), T('<li class=', 'b', 'entity.name.tag.html'),
  T('<!doctype html>', '<!', 'punctuation.definition.tag'), T('<!doctype html>', 'doctype', 'meta.tag.metadata.doctype'),
  T('<!doctype html>', 'html', 'entity.name.tag.html'), T('<!doctype html>', 'lang', 'entity.other.attribute-name.html'),
  T('<!doctype html>', 'body', 'entity.name.tag.html'),
  T('<h1>Hello', 'h1', 'entity.name.tag.html'), T('<h1>Hello', 'it', '!string'), T('<h1>Hello', 'not a comment', '!comment'),
  T('<h1>Hello', 'app.mask', '!entity'),
  T('{row(app.users[1]);}', 'row', 'entity.name.function.call'), T('{row(app.users[1]);}', '1', 'constant.numeric'),
];

(async () => {
  const wasm = fs.readFileSync(require.resolve('vscode-oniguruma/release/onig.wasm', { paths: [modDir, __dirname] })).buffer;
  await onig.loadWASM(wasm);
  const registry = new vsctm.Registry({
    onigLib: Promise.resolve({ createOnigScanner: (p) => new onig.OnigScanner(p), createOnigString: (s) => new onig.OnigString(s) }),
    loadGrammar: async (scope) => (scope === 'source.mitochondria' ? vsctm.parseRawGrammar(raw, grammarPath) : null),
  });
  const grammar = await registry.loadGrammar('source.mitochondria');
  if (!grammar) throw new Error('grammar did not load');

  // Tokenize the sample, line by line.
  const lines = sample.split('\n');
  const toks = [];
  let state = vsctm.INITIAL;
  lines.forEach((text, n) => {
    const r = grammar.tokenizeLine(text, state);
    toks[n] = r.tokens.map((t) => ({ text: text.substring(t.startIndex, t.endIndex), scopes: t.scopes }));
    state = r.ruleStack;
  });
  let failed = 0;
  const fail = (m) => { failed++; console.log('FAIL ' + m); };

  // Every line where code resumes at top level must be back at the root scope (no region left open).
  const endScopes = grammar.tokenizeLine('', state).tokens[0].scopes;
  if (endScopes.length !== 1) fail('a region is still open at the end of the sample: ' + endScopes.join(' '));
  lines.forEach((text, n) => {
    if (/^(fn|pub fn|struct|enum|@[a-z_]+|test|state|request|module|platform|use|system|web)\b/.test(text)) {
      const first = toks[n][0];
      if (first && first.scopes.length > 1 && !/^(system|web)\b/.test(text) && !first.scopes.some((s) => /^(meta\.attribute|storage|keyword)/.test(s)))
        fail('line ' + (n + 1) + ' starts inside a region: ' + first.scopes.join(' '));
    }
  });

  // Every sample line has no token of a name that should be plain (identifiers in plain code stay uncolored).
  for (const e of expectations) {
    const idx = [];
    lines.forEach((t, i) => { if (t.includes(e.line)) idx.push(i); });
    if (!idx.length) { fail('no sample line contains "' + e.line + '"'); continue; }
    // search the matching line(s), and the lines that follow it up to the next blank/declaration, for the token
    let found = null;
    for (const i of idx) {
      for (let j = i; j < Math.min(lines.length, i + 8) && !found; j++) {
        const hits = toks[j].filter((t) => t.text === e.tok || t.text.trim() === e.tok || (e.scope.startsWith('!') && t.text.includes(e.tok)));
        if (hits.length > e.nth) found = hits[e.nth];
        if (j > i && lines[j].trim() === '') break;
      }
      if (found) break;
    }
    if (!found) { fail('token "' + e.tok + '" not found for line "' + e.line + '"'); continue; }
    if (!e.scope) continue;
    const neg = e.scope.startsWith('!');
    const want = neg ? e.scope.slice(1) : e.scope;
    const has = found.scopes.some((s) => s.startsWith(want));
    if (neg ? has : !has) fail('"' + e.tok + '" (' + e.line + ') ' + (neg ? 'must not have ' : 'needs ') + want + '; has ' + found.scopes.slice(1).join(' '));
  }
  // The repository's own programs must each end back at the root scope (no unclosed markup, string or brace region).
  const root = path.join(__dirname, '..', '..', '..');
  const progs = [];
  for (const d of ['examples', 'lib']) {
    const walk = (dir) => { if (!fs.existsSync(dir)) return; for (const f of fs.readdirSync(dir, { withFileTypes: true })) { const q = path.join(dir, f.name); if (f.isDirectory()) walk(q); else if (f.name.endsWith('.mito')) progs.push(q); } };
    walk(path.join(root, d));
  }
  for (const f of progs) {
    let st = vsctm.INITIAL;
    for (const text of fs.readFileSync(f, 'utf8').split(NL)) st = grammar.tokenizeLine(text, st).ruleStack;
    if (grammar.tokenizeLine('', st).tokens[0].scopes.length !== 1) fail(path.relative(root, f) + ' ends inside an open region');
  }
  console.log(progs.length + ' repository programs end at the root scope (failures above)');
  console.log(failed ? failed + ' check(s) failed' : 'ok: grammar loads, ' + expectations.length + ' scope checks pass, ' + lines.length + ' sample lines');
  process.exitCode = failed ? 1 : 0;
})().catch((e) => { console.error(e); process.exitCode = 2; });
