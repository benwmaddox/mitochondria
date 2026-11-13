//! Tests of the generator and the C safety profile.
    use super::*;

    /// A macro name defined twice in the prelude where both definitions can be in force (the same `#if` branch, or one
    /// of them outside every branch) silently replaces the first: a built-in's `ROUND_` once replaced the loop budget's.
    #[test]
    fn the_prelude_defines_each_macro_once_per_branch() {
        let (mut stack, mut next, mut defs): (Vec<String>, usize, Vec<(String, Vec<String>)>) = (vec![], 0, vec![]);
        for l in PRELUDE.lines() {
            let t = l.trim_start();
            if t.starts_with("#if") { next += 1; stack.push(format!("if{}", next)); }
            else if t.starts_with("#else") || t.starts_with("#elif") { if let Some(top) = stack.last_mut() { *top = top.replace("if", "else"); } }
            else if t.starts_with("#endif") { stack.pop(); }
            else if let Some(rest) = t.strip_prefix("#define ") {
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                for (n, st) in &defs {
                    assert!(*n != name || !(st.starts_with(&stack) || stack.starts_with(st)), "the prelude defines `{}` twice where both can be in force", name);
                }
                defs.push((name, stack.clone()));
            }
        }
    }

    #[test]
    fn each_endpoint_zeroes_only_the_request_fields_it_names() {
        // `small` names work.a (through a function, and by assignment), `large` reads work.big (an array, 8000 bytes): each finds
        // zeroed only what it names
        let src = "struct Work { a: i64; big: i64[1000]; s: utf8<64>; }\nrequest work: Work;\nstruct A { n: i64; }\nstate app: A;\n\
            fn bump() { work.a += 1; }\n\
            @get(\"/small\") fn small() -> Response<{ a: i64 }> { bump(); work.a += 1; return ok({ a: work.a }); }\n\
            @get(\"/large\") fn large() -> Response<{ n: i64 }> { work.big[3] = 7; work.s = \"x\"; return ok({ n: work.big[3] }); }\n";
        let (state, api) = crate::source::load(src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        let (ro, _) = prog.request.unwrap();
        let w = prog.structs.iter().position(|x| x.name == "Work").unwrap();
        let at = |f: &str| { let k = prog.structs[w].fields.iter().position(|x| x.0 == f).unwrap(); ro + prog.structs[w].offs[k] };
        assert_eq!(g.zeros[0], Some(vec![(at("a"), at("a") + 8)]));
        let z = g.zeros[1].clone().unwrap();
        let total: usize = z.iter().map(|(a, b)| b - a).sum();
        assert!(total >= 8000 + 68 && total < 8200 && z.iter().all(|(a, _)| *a >= at("big")), "{:?}", z);
    }

    /// An endpoint is read-only (a reader may run it) when nothing in its code changes state: plain state, a collection and a table do; the request struct
    /// and the text area are the request's own. `reads` is what the route table tells the runtime.
    #[test]
    fn endpoints_that_only_read_are_marked_for_readers() {
        let src = "struct Work { a: i64; t: utf8<32>; }
request work: Work;
struct Node { v: i64; }
struct A { n: i64; buf: u8[8]; xs: pool<Node, 4>; s: utf8<16>; }
state app: A;
            fn helper() -> i64 { return app.n + 1; }
            @get(\"/read\") fn read() -> Response<{ n: i64 }> { return ok({ n: app.n + app.buf[2] + app.xs.count() }); }
            @get(\"/call\") fn call() -> Response<{ n: i64 }> { return ok({ n: helper() }); }
            @get(\"/scratch\") fn scratch() -> Response<{ n: i64 }> { work.a = app.n; work.t = \"x\"; return ok({ n: work.a }); }
            @get(\"/plain\") fn plain() -> Response<{ n: i64 }> { app.n = 1; return ok({ n: app.n }); }
            @get(\"/arr\") fn arr() -> Response<{ n: i64 }> { app.buf[1] = 3; return ok({ n: 1 }); }
            @get(\"/add\") fn add() -> Response<{ n: i64 }> { let p = app.xs.add(); p.v = 1; return ok({ n: 1 }); }
            @get(\"/text\") fn text() -> Response<{ n: i64 }> { app.s = \"hi\"; return ok({ n: 1 }); }
            @get(\"/item\") fn item() -> Response<{ n: i64 }> { foreach p in app.xs { p.v = 2; } return ok({ n: 1 }); }
";
        let (state, api) = crate::source::load(src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        let want = [("read", true), ("call", true), ("scratch", true), ("plain", false), ("arr", false), ("add", false), ("text", false), ("item", false)];
        for (k, (name, reads)) in want.iter().enumerate() { assert_eq!(g.reads[k], *reads, "endpoint {}", name); }
    }

    /// A call of a function declared for a type counts as what the function it reaches does: a GET that calls one that writes is not a reader, one that calls only readers is, whichever types share the name.
    #[test]
    fn receiver_functions_count_as_what_they_do_for_readers() {
        let src = "struct Node { v: i64; }
struct Leaf { w: i64; }
struct A { xs: pool<Node, 4>; ls: pool<Leaf, 4>; }
state app: A;
            fn bump(self: write Node) { self.v += 1; }
            fn bump(self: write Leaf) -> i64 { return self.w; }
            fn peek(self: Node) -> i64 { return self.v; }
            @get(\"/peek\") fn peek() -> Response<{ n: i64 }> { var t = 0; foreach n in app.xs { t += n.peek(); } return ok({ n: t }); }
            @get(\"/leaf\") fn leaf() -> Response<{ n: i64 }> { var t = 0; foreach l in app.ls { t += l.bump(); } return ok({ n: t }); }
            @get(\"/bump\") fn bump() -> Response<{ n: i64 }> { foreach n in app.xs { n.bump(); } return ok({ n: 1 }); }
";
        let (state, api) = crate::source::load(src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        // `bump` writes for a Node and only reads for a Leaf: what a GET does is what its generated code does, so the call follows the function its receiver's type names
        let want = [("peek", true), ("leaf", true), ("bump", false)];
        for (k, (name, reads)) in want.iter().enumerate() { assert_eq!(g.reads[k], *reads, "endpoint {}", name); }
    }

    /// Struct locals live in the request's scratch area, which is the request's own: an endpoint that only builds locals (from state, from other locals, through a function that writes
    /// them) is still a reader; one that copies a struct (or a literal) into the state, or writes the state's struct through a function, is not. Both directions.
    #[test]
    fn struct_locals_are_the_requests_own_for_readers() {
        let src = "struct V { x: i64; y: i64; }
struct A { p: V; q: V logged; n: i64; }
state app: A;
            fn bump(p: write V) { p.x += 1; }
            fn shift(self: write V, d: i64) { self.x += d; self.y += d; }
            @get(\"/locals\") fn locals() -> Response<{ n: i64 }> { var a = V { ..app.p }; var b = V { ..app.q }; b = a; a.x += 2; let c = V { ..b, y: 4 }; return ok({ n: a.x + c.y }); }
            @get(\"/through\") fn through() -> Response<{ n: i64 }> { var a = V { ..app.p }; bump(a); a.shift(3); return ok({ n: a.x }); }
            @get(\"/into\") fn into() -> Response<{ n: i64 }> { var a = V { ..app.p }; app.q = a; return ok({ n: 1 }); }
            @get(\"/literal\") fn literal() -> Response<{ n: i64 }> { app.p = V { x: 1, y: 2 }; return ok({ n: 1 }); }
            @get(\"/state_through\") fn state_through() -> Response<{ n: i64 }> { var a = V { ..app.p }; bump(app.p); return ok({ n: a.x }); }
            @get(\"/plain\") fn plain() -> Response<{ n: i64 }> { return ok({ n: app.p.x }); }
";
        let g = gen_of(src);
        assert!(g.scratch, "an endpoint with struct locals uses the scratch area");
        let want = [("locals", true), ("through", true), ("into", false), ("literal", false), ("state_through", false), ("plain", true)];
        for (k, (name, reads)) in want.iter().enumerate() { assert_eq!(g.reads[k], *reads, "endpoint {}", name); }
        // a program with no struct value has no scratch area: its layout and totals are what they were
        let g = gen_of("struct V { x: i64; }\nstruct A { p: V; }\nstate app: A;\n@get(\"/a\") fn a() -> Response<{ n: i64 }> { return ok({ n: app.p.x }); }\n");
        assert!(!g.scratch);
    }

    /// A copy into the state counts as the stores it is for the whole-program ranges of fields (a read through an index may drop its test only when every store is proven small);
    /// stores into a local never count; a copy into one place does not widen another's field. Both directions.
    #[test]
    fn struct_copies_count_as_stores_for_field_ranges() {
        let head = "struct P { v: i64; }\nstruct A { p: P; q: P; buf: i64[8]; }\nstate app: A;\n";
        let read = "@get(\"/r\") fn r() -> Response<{ n: i64 }> { return ok({ n: app.buf[app.p.v] }); }\n";
        let small = "@post(\"/w\") fn w() -> Response<{}> { app.p.v = 3; return ok({}); }\n";
        let tested = |more: &str| gen_of(&format!("{}{}{}{}", head, small, more, read)).src.contains("c->h->elem(");
        assert!(!tested(""), "every store to p.v is 3: the index is inside buf");
        // a copy of a local, of a state struct, or of a literal into app.p could hold anything: the test stays
        assert!(tested("@post(\"/c\") fn c(x: i64) -> Response<{}> { var l = P { ..app.q }; l.v = x; app.p = l; return ok({}); }\n"), "a local copied into p");
        assert!(tested("@post(\"/c\") fn c() -> Response<{}> { app.p = app.q; return ok({}); }\n"), "a state struct copied into p (q is never proven small)");
        assert!(tested("@post(\"/c\") fn c(x: i64) -> Response<{}> { app.p = P { v: x }; return ok({}); }\n"), "a literal copied into p");
        // writes to locals, and copies into another place, leave p.v at its proven range
        assert!(!tested("@post(\"/c\") fn c(x: i64) -> Response<{}> { var l = P { ..app.p }; l.v = x; l.v = 1000; return ok({}); }\n"), "stores into a local are not stores into the state");
        assert!(!tested("@post(\"/c\") fn c(x: i64) -> Response<{}> { var l = P { ..app.p }; l.v = x; app.q = l; return ok({}); }\n"), "a copy into q does not change what p.v holds");
        assert!(!tested("@post(\"/c\") fn c() -> Response<{}> { let l = P { ..app.p }; var m = P { ..l }; m = l; return ok({}); }\n"), "copies between locals store nothing in the state");
    }

    /// A range-declared field keeps its check when a literal or a field store of a local writes it, unless the value is proven inside; a copy needs none (the source holds the range).
    #[test]
    fn struct_locals_keep_declared_ranges() {
        let head = "struct G { n: i64 in 0..100; }\nstruct A { g: G; }\nstate app: A;\n";
        let msg = "`n` must be in 0..100";
        let lit = |x: &str| gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in {}) -> Response<{{}}> {{ let l = G {{ n: x }}; app.g = l; return ok({{}}); }}\n", head, x)).src.contains(msg);
        assert!(lit("0..500"), "a literal field outside the range is tested");
        assert!(!lit("0..50"), "a literal field inside the range is proven");
        let store = |x: &str| gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in {}) -> Response<{{}}> {{ var l = app.g; l.n = x; app.g = l; return ok({{}}); }}\n", head, x)).src.contains(msg);
        assert!(store("0..500") && !store("0..50"));
        let copy_only = gen_of(&format!("{}@post(\"/a\") fn a() -> Response<{{}}> {{ var l = app.g; app.g = l; let m = app.g; return ok({{}}); }}\n", head));
        assert!(!copy_only.src.contains(msg), "a copy cannot leave the range: no test");
    }

    /// Locals of blocks that do not overlap share their scratch place; locals alive together do not; a C function generated apart has places of its own above everything an endpoint uses.
    #[test]
    fn struct_locals_get_places_by_scope() {
        let zeros = |s: &str| -> Vec<usize> {
            let mut v = Vec::new();
            for l in s.lines() { if let Some(r) = l.trim().strip_prefix("SCRZERO(") { v.push(r.split("ULL").next().unwrap().parse::<usize>().unwrap()); } }
            v
        };
        let head = "struct V { x: i64; y: i64; }\nstruct A { p: V; n: i64; }\nstate app: A;\n";
        let g = gen_of(&format!("{}@post(\"/a\") fn a() -> Response<{{ n: i64 }}> {{ var t = V {{}}; if app.n > 0 {{ let u = V {{ x: 1, y: 2 }}; t.x = u.x; }} else {{ let w = V {{ x: 3, y: 4 }}; t.y = w.y; }} return ok({{ n: t.x }}); }}\n", head));
        let z = zeros(&g.src);
        // t, then u and w (one place for both), then the hidden temporaries of the literals (also given back at the block's end)
        let t = z[0];
        assert!(z.iter().all(|o| (*o - t) % 8 == 0) && z.iter().filter(|o| **o == t).count() == 1, "t's place is its own: {:?}", z);
        let mut uniq = z.clone(); uniq.sort(); uniq.dedup();
        assert!(uniq.len() < z.len(), "blocks that do not overlap reuse places: {:?}", z);
    }

    /// A function too big to inline is a C function of its own: its struct locals have places of their own, apart from its caller's and from every other such function's, so a call in the
    /// middle of the caller's locals (or one such function calling another) cannot overwrite them; a caller's local passes to it as a view.
    #[test]
    fn struct_locals_of_functions_compiled_apart_have_places_of_their_own() {
        let body: String = (0..400).map(|_| "    p.x += 1.0;\n    t.y += 0.5;\n").collect();
        let src = format!("struct V {{ x: f64; y: f64; z: f64; }}\nstruct A {{ pos: V; b: V; }}\nstate app: A;\nfn big(p: write V, n: f64) -> f64 {{ var t = V {{ x: 1.0, y: 2.0, z: 3.0 }};\n{}    return t.x + t.y + n; }}\n\
            fn big2(p: write V) -> f64 {{ var t = V {{ ..p }};\n{}    let r = big(t, 1.0); return t.z + r; }}\n\
            @post(\"/a\") fn a() -> Response<{{ n: f64 }}> {{ var l = V {{ ..app.pos }}; let k = V {{ x: 5.0, y: 6.0, z: 7.0 }}; let a = big(l, 1.0); let c = big2(l); return ok({{ n: a + c + k.z }}); }}\n", body, body);
        let g = gen_of(&src);
        let mut places: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
        let mut cur = String::new();
        for l in g.src.lines() {
            if (l.starts_with("static int ") || l.starts_with("int ")) && l.ends_with('{') { cur = l.split('(').next().unwrap().to_string(); }
            if let Some(r) = l.trim().strip_prefix("SCRZERO(") { places.entry(cur.clone()).or_default().push(r.split("ULL").next().unwrap().parse().unwrap()); }
        }
        let apart: Vec<&Vec<usize>> = places.iter().filter(|(k, _)| k.contains("fn_")).map(|x| x.1).collect();
        assert!(apart.len() >= 2, "the big functions are compiled apart: {:?}", places);
        let own: Vec<usize> = places.iter().filter(|(k, _)| k.contains(" ep")).flat_map(|x| x.1.clone()).collect();
        let mut all: Vec<usize> = apart.iter().flat_map(|x| x.iter().copied()).collect();
        let n = all.len();
        all.sort(); all.dedup();
        assert_eq!(all.len(), n, "no two C functions share a place: {:?}", places);
        assert!(!own.is_empty() && own.iter().all(|o| *o < *all.first().unwrap()), "the endpoint's places are below those of the functions: {:?}", places);
    }

    fn gen_of(src: &str) -> Generated {
        let (state, api) = crate::source::load(src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        g
    }

    /// A function given a plain struct counts as what its body does: one that writes through the parameter makes a GET not a reader, one that only reads leaves it one.
    #[test]
    fn plain_struct_parameters_count_as_what_they_do_for_readers() {
        let head = "struct P { v: i64; }\nstruct A { p: P; q: P; }\nstate app: A;\nfn peek(p: P) -> i64 { return p.v; }\nfn bump(p: write P) { p.v += 1; }\nfn look(self: P) -> i64 { return self.v; }\nfn poke(self: write P) { self.v = 2; }\n";
        let g = gen_of(&format!("{}@get(\"/a\") fn a() -> Response<{{ n: i64 }}> {{ return ok({{ n: peek(app.p) + app.q.look() }}); }}\n@get(\"/b\") fn b() -> Response<{{ n: i64 }}> {{ bump(app.p); return ok({{ n: 1 }}); }}\n@get(\"/c\") fn c() -> Response<{{ n: i64 }}> {{ app.q.poke(); return ok({{ n: 1 }}); }}\n", head));
        let want = [("a", true), ("b", false), ("c", false)];
        for (k, (name, reads)) in want.iter().enumerate() { assert_eq!(g.reads[k], *reads, "endpoint {}", name); }
    }

    /// A range-declared field written through a parameter keeps its check unless the value is proven inside the range; a field without a range that is only ever
    /// given small values lets an index into a larger array go unchecked, until a function writes it through a parameter with a value nothing is known of.
    #[test]
    fn plain_struct_parameters_are_seen_by_the_range_analyses() {
        let head = "struct P { n: i64 in 0..100; v: i64; }\nstruct A { p: P; buf: i64[8]; }\nstate app: A;\nfn set_n(p: write P, x: i64) { p.n = x; }\nfn set_v(p: write P, x: i64) { p.v = x; }\n";
        let msg = "`n` must be in 0..100";
        // a value the endpoint's own range proves inside: no check; an unbounded one: checked
        let g = gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in 0..50) -> Response<{{}}> {{ set_n(app.p, x); return ok({{}}); }}\n", head));
        assert!(!g.src.contains(msg), "a value inside the range needs no check");
        let g = gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in 0..500) -> Response<{{}}> {{ set_n(app.p, x); return ok({{}}); }}\n", head));
        assert!(g.src.contains(msg), "a value outside the range is still checked through a parameter");
        // the whole-program range of `v`: stores of 0..3 only, so `buf[app.p.v]` needs no index test ...
        let read = "@get(\"/r\") fn r() -> Response<{ n: i64 }> { return ok({ n: app.buf[app.p.v] }); }\n";
        let g = gen_of(&format!("{}@post(\"/w\") fn w() -> Response<{{}}> {{ app.p.v = 3; return ok({{}}); }}\n{}", head, read));
        assert!(!g.src.contains("c->h->elem("), "every store to `v` is 3, so the index is inside buf");
        // ... and the same program with a store through a parameter of an unbounded value keeps the test
        let g = gen_of(&format!("{}@post(\"/w\") fn w() -> Response<{{}}> {{ app.p.v = 3; return ok({{}}); }}\n@post(\"/s/{{x}}\") fn s(x: i64) -> Response<{{}}> {{ set_v(app.p, x); return ok({{}}); }}\n{}", head, read));
        assert!(g.src.contains("c->h->elem("), "a store through a parameter widens the range of the field");
        // a proven small value written through a parameter keeps the range small
        let g = gen_of(&format!("{}@post(\"/w\") fn w() -> Response<{{}}> {{ app.p.v = 3; return ok({{}}); }}\n@post(\"/s/{{x}}\") fn s(x: i64 in 0..7) -> Response<{{}}> {{ set_v(app.p, x); return ok({{}}); }}\n{}", head, read));
        assert!(!g.src.contains("c->h->elem("), "stores of 0..7 through a parameter keep the index inside buf");
    }

    /// Generic instances are ordinary state to the range analyses, in both directions: a declared range is its own per instance (`in 0..N`), and the whole-program
    /// range of a field is made of the stores into that instance only.
    #[test]
    fn generic_instances_are_separate_state_to_the_range_analyses() {
        // `n: i64 in 0..N`: the same function copied for Box<100> and Box<20>; a value in 0..50 is proven inside 0..100 and not inside 0..20
        let head = "struct Box<N: count> { n: i64 in 0..N; }\nstruct A { big: Box<100>; small: Box<20>; }\nstate app: A;\nfn set_n(self: write Box<N>, x: i64) { self.n = x; }\n";
        let both = gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in 0..50) -> Response<{{}}> {{ app.big.set_n(x); app.small.set_n(x); return ok({{}}); }}\n", head));
        assert!(both.src.contains("`n` must be in 0..20") && !both.src.contains("`n` must be in 0..100"), "Box<100> proves 0..50 inside, Box<20> keeps its test");
        let big = gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in 0..50) -> Response<{{}}> {{ app.big.set_n(x); return ok({{}}); }}\n", head));
        assert!(!big.src.contains("`n` must be in 0..100"));
        let wide = gen_of(&format!("{}@post(\"/a/{{x}}\") fn a(x: i64 in 0..500) -> Response<{{}}> {{ app.big.set_n(x); return ok({{}}); }}\n", head));
        assert!(wide.src.contains("`n` must be in 0..100"), "a value outside the instance's range is checked");
        // whole-program field ranges: `big.v` only ever holds 3, `small.v` takes any value; each read is judged by its own instance's stores
        let head = "struct Cell<N: count> { v: i64; buf: i64[N]; }\nstruct A { big: Cell<8>; small: Cell<4>; }\nstate app: A;\nfn put(self: write Cell<N>, x: i64) { self.v = x; }\n";
        let w = "@post(\"/w\") fn w() -> Response<{}> { app.big.put(3); return ok({}); }\n";
        let ws = "@post(\"/s/{x}\") fn s(x: i64) -> Response<{}> { app.small.put(x); return ok({}); }\n";
        let rb = "@get(\"/rb\") fn rb() -> Response<{ n: i64 }> { return ok({ n: app.big.buf[app.big.v] }); }\n";
        let rs = "@get(\"/rs\") fn rs() -> Response<{ n: i64 }> { return ok({ n: app.small.buf[app.small.v] }); }\n";
        assert!(!gen_of(&format!("{}{}{}", head, w, rb)).src.contains("c->h->elem("), "every store to big.v is 3: inside buf[8]");
        assert!(!gen_of(&format!("{}{}{}{}", head, w, ws, rb)).src.contains("c->h->elem("), "an unbounded store to small.v does not widen big.v");
        assert!(gen_of(&format!("{}{}{}{}", head, w, ws, rs)).src.contains("c->h->elem("), "small.v is unbounded: its index is tested");
    }

    /// `byte_at` is read in line (the byte, or -1 past either end): no runtime call for any kind of text it takes, and no test where the
    /// range analysis proves the index inside a literal.
    #[test]
    fn byte_at_is_read_in_line() {
        let gen = |body: &str| {
            let src = format!("struct A {{ s: string<16>; n: i64 in 0..2; }}\nstate app: A;\n@post(\"/t\") fn t(x: utf8<16>) -> Response<{{ v: i64 }}> {{ {} }}\n", body);
            let (state, api) = crate::source::load(&src).unwrap();
            let prog = Program::parse(&state).unwrap();
            let mut api = crate::lang::parse(&api).unwrap();
            crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
            let g = generate(&prog, &api);
            assert!(g.notes.is_empty(), "{:?}", g.notes);
            g.src[g.src.find("int ep0(Ctx").unwrap()..].to_string()
        };
        let looped = gen("var t = 0; var i = 0; while i < len(x) { t += byte_at(x, i); i += 1; } return ok({ v: t });");
        assert!(!looped.contains("builtin(c, 58"), "an input text's byte is a load: {}", looped);
        assert!(looped.contains("TX_("), "{}", looped);
        let kinds = gen("return ok({ v: byte_at(app.s, 3) + byte_at(substr(app.s, 1, 2), 1) + byte_at(substr(x, 1, 2), 0) });");
        assert!(!kinds.contains("builtin(c, 58"), "a field's and a window's bytes are loads: {}", kinds);
        // inside the literal (the field `n` is 0..2 and "abc" has 3 bytes): no length test; one that may be outside keeps it
        let proven = gen("return ok({ v: byte_at(\"abc\", app.n) });");
        assert!(proven.contains("TX_(") && !proven.contains("< 3ULL"), "{}", proven);
        let open = gen("return ok({ v: byte_at(\"abc\", app.n + 5) });");
        assert!(open.contains("< 3ULL"), "{}", open);
    }

    /// Text compared (a field, a column, a piece of one, a literal) is read in line: no `str_cmp_*` helper call.
    #[test]
    fn text_compare_is_read_in_line() {
        let gen = |body: &str| {
            let src = format!("struct A {{ s: string<16>; t: string<16>; }}\nstate app: A;\n@get(\"/t\") fn t() -> Response<{{ v: bool }}> {{ {} }}\n", body);
            let (state, api) = crate::source::load(&src).unwrap();
            let prog = Program::parse(&state).unwrap();
            let mut api = crate::lang::parse(&api).unwrap();
            crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
            let g = generate(&prog, &api);
            assert!(g.notes.is_empty(), "{:?}", g.notes);
            g.src[g.src.find("int ep0(Ctx").unwrap()..].to_string()
        };
        for body in ["return ok({ v: app.s == \"abc\" });", "return ok({ v: \"abc\" != app.s });", "return ok({ v: app.s == app.t });",
                     "return ok({ v: app.s < app.t });", "return ok({ v: app.s >= \"abc\" });", "return ok({ v: substr(app.s, 0, 2) == \"ab\" });"] {
            let c = gen(body);
            assert!(!c.contains("str_cmp_"), "{}: {}", body, c);
            assert!(c.contains("BEQ_(") || c.contains("BCMP_("), "{}: {}", body, c);
        }
    }

    /// How many overflow-checked operations (`ADD`, `SUB`, `MUL`) an endpoint's generated code keeps, for a body written in a program with one range-typed field and one free field.
    fn checked_ops(body: &str) -> usize {
        let src = format!("struct A {{ n: i64 in 0..1000; big: i64; }}\nstate app: A;\n@get(\"/t\") fn t() -> Response<{{ x: i64 }}> {{ {} }}\n", body);
        let (state, api) = crate::source::load(&src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        let code = &g.src[g.src.find("int ep0(Ctx").unwrap()..];
        code.lines().map(|l| l.trim_start()).filter(|l| ["ADD(", "SUB(", "MUL("].iter().any(|m| l.starts_with(m))).count()
    }

    /// What the compiler knows of a variable as the code runs (its initial value, its assignments, `if` joins, a counted loop's counter, a `while` condition) decides which
    /// overflow tests remain. Each case that must keep its test is the way a wrong proof would show.
    #[test]
    fn variable_ranges_drop_only_tests_that_cannot_fire() {
        // gone: a counter stepped by 1 in a loop of at most 1000 rounds; in a `while` (its rounds are budgeted); an `if` whose arms both give small values; a bound from a condition
        assert_eq!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { c = c + 1; } return ok({ x: c });"), 0);
        assert_eq!(checked_ops("var c = 0; var i = 0; while i < app.n { c = c + 1; i = i + 1; } return ok({ x: c });"), 0);
        assert_eq!(checked_ops("var c = 0; if app.n > 5 { c = 100; } else { c = 7; } let y = c + 1; return ok({ x: y });"), 0);
        assert_eq!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { if i > 3 { c = c + 1; } else { c = c - 1; } } return ok({ x: c });"), 0);
        // kept: the loop's rounds are not bounded; the step is large; the counter is also set or scaled; it is stepped inside an inner loop; the value is unknown on one way
        assert!(checked_ops("var c = 0; foreach i in 0..app.big { c = c + 1; } return ok({ x: c });") > 0);
        assert!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { c = c + 2097152; } return ok({ x: c });") > 0);
        assert!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { c = c + 1; if i == 5 { c = app.big; } } return ok({ x: c });") > 0);
        assert!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { c = c * 2 + 1; } return ok({ x: c });") > 0);
        assert!(checked_ops("var c = 0; foreach i in 0..app.n - 1 { foreach j in 0..app.n - 1 { c = c + 1; } } return ok({ x: c });") > 0);
        assert!(checked_ops("var c = 0; if app.n > 5 { c = app.big; } let y = c + 1; return ok({ x: y });") > 0);
        // a start close to the end of the i64 range stays checked: ten rounds can pass it
        assert!(checked_ops("var c = 9223372036854775800; foreach i in 0..9 { c = c + 1; } return ok({ x: c });") > 0);
    }

    /// The endpoint code generated for `api` over `state`, from its first endpoint on.
    fn endpoint_code(state: &str, api: &str) -> String {
        let src = format!("{}\n{}\n", state, api);
        let (state, api) = crate::source::load(&src).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut api = crate::lang::parse(&api).unwrap();
        crate::lang::bind_with(&mut api, &|n| prog.global(n), &prog.enum_table(), &prog.const_table()).unwrap();
        let g = generate(&prog, &api);
        assert!(g.notes.is_empty(), "{:?}", g.notes);
        g.src[g.src.find("int ep0(Ctx").unwrap()..].to_string()
    }

    /// What the compiler can show of an index decides whether its test stays; a quotient has the range its operands allow, so an index computed with a division is proven or not by them.
    #[test]
    fn index_tests_follow_the_ranges_of_what_is_computed() {
        let state = "struct A { n: i64 in 0..1000; big: i64; xs: u8[10]; }\nstate app: A;";
        let tests = |body: &str| endpoint_code(state, &format!("@get(\"/t\") fn t() -> Response<{{ x: i64 }}> {{ {} }}", body)).matches("RANGE_(").count();
        assert_eq!(tests("let i = app.n / 200; return ok({ x: app.xs[i] });"), 0);        // 0..5
        assert_eq!(tests("let i = (app.n + 199) / 200; return ok({ x: app.xs[i] });"), 0); // 0..5
        assert!(tests("let i = app.n / 50; return ok({ x: app.xs[i] });") > 0);             // 0..20: can be out
        assert!(tests("let i = app.big / 2; return ok({ x: app.xs[i] });") > 0);           // nothing known
        assert!(tests("let i = (app.n - 500) / 100; return ok({ x: app.xs[i] });") > 0);   // -5..5: can be negative
    }

    /// A pool nothing in the program removes from has no dead slot below its high-water mark, so reaching an item by number tests the range and the mark only; any removal anywhere
    /// brings the live-bit test back.
    #[test]
    fn a_pool_nothing_removes_from_is_reached_without_a_live_bit_test() {
        let state = "struct N { v: i64; }\nstruct A { n: i64 in 0..7; nodes: pool<N, 8>; }\nstate app: A;";
        let reach = "@get(\"/t\") fn t() -> Response<{ x: i64 }> { let p = app.nodes[app.n]; return ok({ x: p.v }); }\n";
        let add = "@post(\"/add\") fn add() -> Response<{ x: i64 }> { let p = app.nodes.add(); p.v = 1; return ok({ x: 1 }); }\n";
        let remove = "@post(\"/rm\") fn rm() -> Response<{ x: i64 }> { foreach p in app.nodes { app.nodes.remove(p); } return ok({ x: 1 }); }\n";
        let live_tests = |api: &str| endpoint_code(state, api).matches(">> 6)) >> ").count();
        assert_eq!(live_tests(&format!("{}{}", reach, add)), 0);
        assert!(live_tests(&format!("{}{}{}", reach, add, remove)) > 0);
        assert!(live_tests(&format!("{}{}", remove, reach)) > 0); // the removal is compiled before the reach: still seen
    }

    /// `add` to a plain pool kept out of the undo log is done in the endpoint (the runtime's helper stays as the fallback for a freed slot or a full pool); a pool in the undo log, a
    /// checked pool (generations) and a pool with nested collections keep the helper alone.
    #[test]
    fn adding_to_a_plain_unlogged_pool_is_done_in_line() {
        let add = "@post(\"/add\") fn add() -> Response<{ x: i64 }> { let p = app.nodes.add(); p.v = 1; return ok({ x: 1 }); }";
        let inline = |state: &str| { let c = endpoint_code(state, add); (c.contains("ZERO_("), c.contains("->alloc(")) };
        assert_eq!(inline("struct N { v: i64; }\nstruct A { nodes: pool<N, 8>; }\nstate app: A;"), (true, true));          // in line, helper as the fallback
        assert_eq!(inline("struct N { v: i64; }\nstruct A { nodes: pool<N, 8>; }\nstate app: A logged;"), (false, true));   // in the undo log: the helper
        assert_eq!(inline("struct N { v: i64; }\nstruct A { nodes: pool<N, 8> checked; }\nstate app: A;"), (false, true));  // generations: the helper
        assert_eq!(inline("struct K { w: i64; }\nstruct N { v: i64; ks: list<K, 2>; }\nstruct A { nodes: pool<N, 8>; }\nstate app: A;"), (false, true)); // nested collections: the helper
    }

    /// A variable that a loop moves by amounts that are not constants still has an interval at the head of the loop: its first one, widened by what the loop's assignments give, until
    /// the guess holds. A walking offset that only grows (`off = off + step` with `step` from an unsigned byte) and is bounded by the loop's condition needs no index test; one that can
    /// go down, or grow by an unknown amount, keeps it.
    #[test]
    fn loop_carried_variables_have_intervals_from_a_guess_that_holds() {
        let state = "struct A { n: i64 in 0..1000; step: u8; big: i64; xs: u8[2000]; }\nstate app: A;";
        let tests = |body: &str| endpoint_code(state, &format!("@get(\"/t\") fn t() -> Response<{{ x: i64 }}> {{ {} }}", body)).matches("RANGE_(").count();
        assert_eq!(tests("var off = 0; var s = 0; while off < app.n { s = s + app.xs[off + 1]; off = off + app.step; } return ok({ x: s });"), 0);
        assert!(tests("var off = 0; var s = 0; while off < app.n { s = s + app.xs[off + 1]; off = off - app.step; } return ok({ x: s });") > 0);
        assert!(tests("var off = 0; var s = 0; while off < app.n { s = s + app.xs[off + 1]; off = off + app.big; } return ok({ x: s });") > 0);
        assert!(tests("var off = 0; var s = 0; while off < app.big { s = s + app.xs[off + 1]; off = off + app.step; } return ok({ x: s });") > 0);
    }

    /// A bounds test that passes tells the compiler where the index is for the rest of that code: stepping it afterwards cannot overflow. Without an access there is nothing to tell.
    #[test]
    fn a_passed_bounds_test_bounds_the_index() {
        let state = "struct A { n: i64 in 0..1000; big: i64; xs: u8[2000]; }\nstate app: A;";
        let checked = |body: &str| endpoint_code(state, &format!("@get(\"/t\") fn t() -> Response<{{ x: i64 }}> {{ {} }}", body)).lines().map(|l| l.trim_start()).filter(|l| ["ADD(", "SUB(", "MUL("].iter().any(|m| l.starts_with(m))).count();
        assert_eq!(checked("var i = app.big; let x = app.xs[i]; i = i + 1; i = i - 3; return ok({ x: i + x });"), 0);
        assert!(checked("var i = app.big; i = i + 1; return ok({ x: i });") > 0);
        assert!(checked("var i = app.big; i = i + 1; let x = app.xs[i]; return ok({ x: x });") > 0); // the step came before the access
        assert!(checked("var i = app.big; let h = app.n > 5 && app.xs[i] == 1; i = i + 1; return ok({ x: i });") > 0); // the test may not have run: it says nothing of the code after
        assert!(checked("var i = app.big; if app.n > 5 { let x = app.xs[i]; } i = i + 1; return ok({ x: i });") > 0);
    }

    /// What every store into an array put there is assumed of a read, but never wider than what the element type itself gives: a `u8` array that was fed values
    /// of -255..255 (each store is tested, so what is read is still 0..255) keeps the 16-bit shape of `lo | (hi << 8)`; an `i16` array fed only 0..100 keeps that, a narrower range than its type.
    #[test]
    fn a_range_seen_in_stores_never_widens_the_element_type() {
        let reader = "@get(\"/r\") fn r() -> Response<{ x: i64 }> { return ok({ x: app.xs[0] | (app.xs[1] << 8) }); }";
        let code = |state: &str, store: &str| endpoint_code(state, &format!("@post(\"/w\") fn w() -> Response<{{ x: i64 }}> {{ {} return ok({{ x: 0 }}); }}\n{}", store, reader));
        let wide_state = "struct A { n: i64 in -255..255; xs: u8[2]; }\nstate app: A;";
        assert!(code(wide_state, "app.xs[0] = app.n; app.xs[1] = app.n;").contains("BOR16("), "a u8 element read stays 0..255");
        let i16_state = "struct A { n: i64 in 0..100; m: i64 in -5..5; xs: i16[2]; }\nstate app: A;";
        assert!(code(i16_state, "app.xs[0] = app.n; app.xs[1] = app.n;").contains("BOR16("), "an i16 array that only ever held 0..100 reads as 0..100");
        assert!(!code(i16_state, "app.xs[0] = app.m; app.xs[1] = app.m;").contains("BOR16("), "an i16 array that held -5..5 may be negative: no 16-bit or");
    }

    /// A constant is exactly its value: code that uses one is the code written with the number (so a loop over a constant range is bounded, and its checks removed, as for a literal).
    #[test]
    fn a_constant_is_its_value_to_the_analyses() {
        let ep = |range: &str, bound: &str| format!("@get(\"/t\") fn t() -> Response<{{ x: i64 }}> {{ var s = 0; foreach i in 0..{} {{ app.xs[i] = 1; s += app.xs[i] + {}; }} return ok({{ x: s }}); }}", range, bound);
        let lit = |range: &str| endpoint_code("struct A { xs: u8[4]; } state app: A;", &ep(range, "7"));
        let cst = |range: &str| endpoint_code("const L { N = 4; K: i64 = L.N + 3; } struct A { xs: u8[L.N]; } state app: A;", &ep(range, "L.K"));
        assert_eq!(lit("3"), cst("L.N - 1"));
        assert_eq!(lit("4"), cst("L.N")); // one too many: the index test stays, as with the literal
        assert_ne!(lit("3"), lit("4"));
        let (a, b) = (cst("L.N - 1"), cst("L.N"));
        assert!(b.len() > a.len(), "the unbounded loop keeps a test the bounded one lost");
    }

    #[test]
    fn generated_chat_code_meets_the_profile() {
        for file in [include_str!("../../examples/chat/chat.mito"), include_str!("../../examples/chat-sharded/rooms.mito"), include_str!("../../examples/chat-sharded/auth.mito")] {
            let (state, api) = crate::source::load(file).unwrap();
            let prog = Program::parse(&state).unwrap();
            let mut api = crate::lang::parse(&api).unwrap();
            crate::lang::bind(&mut api, &|n| prog.global(n)).unwrap();
            let g = generate(&prog, &api);
            assert!(!g.notes.iter().any(|n| n.contains("profile")), "{:#?}", g.notes);
        }
    }

    #[test]
    fn emitted_files_compile_on_their_own_and_cite_their_source() {
        let (state, api) = crate::source::load(include_str!("../../examples/chat/chat.mito")).unwrap();
        let prog = Program::parse(&state).unwrap();
        let mut a = crate::lang::parse(&api).unwrap();
        crate::lang::bind(&mut a, &|n| prog.global(n)).unwrap();
        let c = emit(&prog, &a, ("chat.mito", &api)).unwrap();
        crate::tcc::Unit::check(&c).unwrap();
        assert!(c.contains("rooms.items.messages: list cap 1000 drop_oldest"), "{}", &c[..1500]);
        for ep in &a.eps { assert!(c.contains(&format!("// chat.mito:{}-{}", ep.lines.0, ep.lines.1))); }
        assert!(c.contains("//   | @delete(\"/session\")"));
    }

    #[test]
    fn the_binder_rejects_bad_names_and_calls_at_load() {
        let prog = Program::parse("struct C { n: i64; } state c: C;").unwrap();
        for (api, want) in [
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ \"x\": nope }); }", "unknown name `nope`"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ \"x\": frob(1) }); }", "unknown function `frob`"),
            ("fn f(a: i64, b: i64) -> i64 { return a; } @get(\"/a\") fn get_a() -> Response<{}> { return ok({ \"x\": f(1) }); }", "`f` takes 2 arguments, given 1"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { x = 1; return ok({}); }", "unknown variable `x`"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { if true { let y = 1; } return ok({ \"y\": y }); }", "unknown name `y`"), // block scope
            ("fn len(s: string) -> i64 { return 1; }", "name of a builtin"),
            (&format!("@get(\"{}\") fn a() -> Response<{{}}> {{ return ok({{}}); }}", "/a".repeat(33)), "at most 32 parts"),
            (&format!("@get(\"/a\") fn a() -> Response<{{}}> {{ let {} = 1; return ok({{}}); }}", "n".repeat(129)), "a name longer than 128 bytes"),
            (&format!("@get(\"/a\") fn a() -> Response<{{}}> {{ fail 400 \"{}\"; }}", "m".repeat(1025)), "a fail message longer than 1024 bytes"),
        ] {
            let e = super::check(&prog, api).err().unwrap_or_default();
            assert!(e.contains(want), "{} -> {}", api, e);
        }
    }

    /// Collections are used through their functions; the old keywords, and operations in the wrong place, do not load.
    #[test]
    fn collection_functions_replace_the_old_keywords() {
        let st = "struct I { n: i64; } struct E { who: i64; n: i64; } struct App { items: pool<I, 4>; board: sorted<E, 4, n>; } state app: App;";
        let (st, _) = crate::modules::expand(st, "").unwrap();
        let load = |api: &str| -> String {
            let (_, api) = crate::modules::expand("struct I { n: i64; } struct E { who: i64; n: i64; } struct App { items: pool<I, 4>; board: sorted<E, 4, n>; } state app: App;", api).unwrap();
            super::check(&Program::parse(&st).unwrap(), &api).err().unwrap_or_default()
        };
        for (api, want) in [
            ("@post(\"/a\") fn post_a() -> Response<{}> { let p = alloc app.items; return ok({}); }", "replaced by collection functions"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { let p = app.items[0]; free p; return ok({}); }", "replaced by the collection's function"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ n: count(app.items) }); }", "`count(x)` was replaced by `x.count()`"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ n: capacity(app.items) }); }", "`capacity(x)` was replaced by `x.capacity()`"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { return ok({ n: slot(app.items.add()) }); }", "makes an item"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { let p = app.items[0]; app.board.items.remove(p); return ok({}); }", "cannot be removed from"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { let p = app.items[0]; let n = app.items.remove(p); return ok({}); }", "is a statement"),
            ("@get(\"/a\") fn get_a() -> Response<{}> { return ok({ n: app.items.count(1) }); }", "takes no arguments, given 1"),
            ("@post(\"/a\") fn post_a() -> Response<{}> { if app.items.add { n: 1 } { } return ok({}); }", "expected"),
        ] {
            let e = load(api);
            assert!(e.contains(want), "{} -> {}", api, e);
        }
        assert_eq!(load("@post(\"/a\") fn post_a() -> Response<{ n: i64, c: i64 }> { let p = app.items.add { n: 2 }; p.n += 1; app.items.remove(p); return ok({ n: p.n, c: app.items.count() }); }"), "");
    }

    /// `T?`: a value that may be null is used as an item only after `if exists(x)` (or a guard that leaves when it is null), or inside
    /// `or(...)` / `exists(...)`; a function that can return null says `-> T?`. A plain item is always live, so nothing here can fail at run time.
    #[test]
    fn values_that_may_be_null_are_checked_before_use() {
        let st = "struct I { n: i64; name: string<8>; } struct App { items: pool<I, 4>; } state app: App;";
        let load = |api: &str| -> String { super::check(&Program::parse(st).unwrap(), api).err().unwrap_or_default() };
        let find = "fn find(n: i64) -> I? { foreach it in app.items { if it.n == n { return it; } } return null; }\n";
        for (api, want) in [
            // reading, writing, removing and slot() of a value that may be null
            (format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = find(k); return ok({{ n: it.n }}); }}", find), "`.n` of a value that may be null"),
            (format!("{}@post(\"/a\") fn a(k: i64) -> Response<{{}}> {{ let it = find(k); it.n = 1; return ok({{}}); }}", find), "assigning `.n` of a value that may be null"),
            (format!("{}@post(\"/a\") fn a(k: i64) -> Response<{{}}> {{ let it = find(k); app.items.remove(it); return ok({{}}); }}", find), "removing a value that may be null"),
            (format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ s: i64 }}> {{ let it = find(k); return ok({{ s: slot(it) }}); }}", find), "`slot(x)` of a value that may be null"),
            // try_add gives a value that may be null
            ("@post(\"/a\") fn a() -> Response<{}> { let it = app.items.try_add(); it.n = 1; return ok({}); }".to_string(), "may be null"),
            // a function that can return null must say so
            ("fn find(n: i64) -> I { foreach it in app.items { if it.n == n { return it; } } return null; }\n@get(\"/a\") fn a(k: i64) -> Response<{ n: i64 }> { return ok({ n: or(find(k).n, 0) }); }".to_string(), "the result of `find` is `I`, given null"),
            // the narrowing ends with its branch, and does not hold when the branch assigns the variable
            (format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = find(k); if exists(it) {{ }} return ok({{ n: it.n }}); }}", find), "may be null"),
            (format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ var it = find(k); if exists(it) {{ it = find(k + 1); return ok({{ n: it.n }}); }} return ok({{ n: 0 }}); }}", find), "may be null"),
            // writing through null
            ("@post(\"/a\") fn a() -> Response<{}> { let it = null; it.n = 1; return ok({}); }".to_string(), "of `null`"),
        ] {
            let e = load(&api);
            assert!(e.contains(want), "{} -> {}", api, e);
        }
        // what is accepted: `if exists(x)`, `if x != null`, a guard that leaves, the else branch of `if x == null`, `or(...)`, `exists(...)`
        for api in [
            format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = find(k); if exists(it) {{ return ok({{ n: it.n }}); }} return ok({{ n: 0 }}); }}", find),
            format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = find(k); if it != null {{ it.n = 2; return ok({{ n: it.n }}); }} return ok({{ n: 0 }}); }}", find),
            format!("{}@post(\"/a\") fn a(k: i64) -> Response<{{ s: i64 }}> {{ let it = find(k); if !exists(it) {{ fail 404 \"no such item\"; }} it.n = 3; app.items.remove(it); return ok({{ s: slot(it) }}); }}", find),
            format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = find(k); if it == null {{ return ok({{ n: 0 }}); }} else {{ return ok({{ n: it.n }}); }} }}", find),
            format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64, there: bool }}> {{ return ok({{ n: or(find(k).n, -1), there: exists(find(k)) }}); }}", find),
            // `or` with a plain item as the fallback gives a plain item
            format!("{}@get(\"/a\") fn a(k: i64) -> Response<{{ n: i64 }}> {{ let it = or(find(k), app.items[0]); return ok({{ n: it.n }}); }}", find),
        ] {
            assert_eq!(load(&api), "", "{}", api);
        }
    }

    /// Every answer is checked against the endpoint's `Response<T>` when the program is compiled.
    #[test]
    fn answers_are_checked_against_their_response_type() {
        let st = "struct Row { name: string<8>; n: i64; } struct I { name: string<8>; n: i64; } struct App { items: pool<I, 4>; total: i64; } state app: App;";
        let load = |api: &str| -> String { super::check(&Program::parse(st).unwrap(), api).err().unwrap_or_default() };
        for (api, want) in [
            ("@get(\"/a\") fn a() -> Response<{ n: i64 }> { return ok({ n: 1, m: 2 }); }", "has `m`, which { n: i64 } does not declare"),
            ("@get(\"/a\") fn a() -> Response<{ n: i64, m: i64 }> { return ok({ n: 1 }); }", "has no `m`"),
            ("@get(\"/a\") fn a() -> Response<{ n: string }> { return ok({ n: 1 }); }", "`n` is i64, declared string"),
            ("@get(\"/a\") fn a() -> Response<{ x: [i64] }> { return ok({ x: [foreach it in app.items it.name] }); }", "`x[]` is string, declared i64"),
            ("@get(\"/a\") fn a() -> Response<[Row]> { return ok([foreach it in app.items { name: it.name }]); }", "has no `n`"),
            ("@get(\"/a\") fn a() -> Response<null> { return ok({}); }", "declared null"),
            ("@get(\"/a\") fn a() -> Response<{}> { return status(302, {}); }", "an answer is 2xx, 4xx or 5xx"),
            ("@get(\"/a\") fn a() -> Response<{}> { return {}; }", "an endpoint returns its answer"),
            ("@get(\"/a\") fn a() -> Response<{}> { respond 200 {}; }", "`respond` was replaced"),
            ("endpoint GET /a { respond 200 {}; }", "endpoints are functions with an attribute"),
            ("@get(\"/a\") fn a() { return ok({}); }", "returns a `Response<T>`"),
            ("@fetch(\"/a\") fn a() -> Response<{}> { return ok({}); }", "`@fetch`"),
            ("@get(\"/a\") fn a() -> Response<{}> { if app.total > 1 { return ok({}); } }", "can finish without an answer"),
            // every value's kind is known when the program is compiled
            ("@get(\"/a\") fn a() -> Response<{ n: i64 }> { var v = 1; v = \"x\"; return ok({ n: 1 }); }", "`v` holds an integer and then a string: a var keeps the kind of its first value"),
            ("fn f(k: i64) -> i64 { if k > 0 { return 1; } return \"x\"; } @get(\"/a\") fn a() -> Response<{}> { let x = f(1); return ok({}); }", "the result of `f` is `i64`, given a string"),
            ("fn f(k: i64) { if k > 0 { return 1; } } @get(\"/a\") fn a() -> Response<{}> { f(1); return ok({}); }", "`f` returns a value: declare its result"),
            ("fn f(k) -> i64 { return k; } @get(\"/a\") fn a() -> Response<{}> { let x = f(1); return ok({}); }", "parameter `k` needs a type"),
            ("fn f(k: i64) -> i64 { return k; } @get(\"/a\") fn a() -> Response<{}> { let x = f(\"one\"); return ok({}); }", "parameter `k` of `f` is `i64`, given a string"),
            ("fn f(k: Nope) -> i64 { return 1; } @get(\"/a\") fn a() -> Response<{}> { let x = f(1); return ok({}); }", "`Nope` is not a type"),
            ("fn f(k: i64) -> i64 { if k > 0 { return 1; } } @get(\"/a\") fn a() -> Response<{}> { let x = f(1); return ok({}); }", "`f` can end without returning its `i64`"),
            ("@get(\"/a\") fn a() -> Response<{ n: string }> { return ok({ n: \"x\" - 1 }); }", "operator `-` does not fit string and i64"),
        ] {
            let e = load(api);
            assert!(e.contains(want), "{} -> {}", api, e);
        }
        // what fits: a declared struct, optional values, run-time kinds, no body, a status of choice
        for api in [
            "@get(\"/a\") fn a() -> Response<[Row]> { return ok([foreach it in app.items { name: it.name, n: it.n }]); }",
            "@get(\"/a\") fn a(x: string<8>?) -> Response<{ n: string? }> { return ok({ n: or(x, null) }); }",
            "@delete(\"/a\") fn a() -> Response<null> { return no_content(); }",
            "@post(\"/a\") fn a() -> Response<{ total: i64 }> { app.total += 1; if app.total > 2 { return status(202, { total: app.total }); } return created({ total: app.total }); }",
        ] {
            assert_eq!(load(api), "", "{}", api);
        }
    }

    #[test]
    fn the_checker_rejects_each_kind_of_violation() {
        let ok = "    i64 v1 = LD_I64(64, i2);\n    ADD(v3, v1, 1LL);\n    CALL(c->h->json_i64(c, v3));\n    return 200;\n";
        assert_eq!(check_profile(ok), Ok(()));
        for (bad, why) in [
            ("    *(i64 *)(b + 8) = 1;\n", "pointer dereference"),
            ("    i64 v = *(const i64 *)(b + 8);\n", "pointer dereference"),
            ("    while (1) { }\n", "forbidden word"),
            ("    for (u32 k = 0;; k++) { }\n", "without a bound"),
            ("    c->h->out(c, \"x\", 1);\n", "without CALL"),
            ("    CALL(ep1(c));\n", "call of something other"),
            ("    CALL(memcpy(&v, b, 8));\n", "forbidden word"),
            ("    u32 k = 0;\nL1: ;\n    goto L1;\n", "backward goto"),
            ("    return 5 + 3;\n", "status"),
            ("    CALL(c->h->lit(c, \"x\", 1, &b));\n    CALL(c->h->lit(c, \"x\", 1, &zz));\n", "address of"),
            ("    i64 v = b[3];\n", "indexing"),
            ("    i64 v = 1; v += 2;\n", "compound assignment"),
            ("#include <stdio.h>\n", "preprocessor"),
            ("    static i64 g;\n", "forbidden word"),
            ("    i64 v = c->base;\n", "member access"),
        ] {
            let e = check_profile(bad).err().unwrap_or_default();
            assert!(e.contains(why), "{:?}: {}", bad, e);
        }
    }
