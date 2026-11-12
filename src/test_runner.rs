//! `mito test`: tests written in programs (`test "name" { ... }`), built into a runner with TinyCC and run. The scoreboard
//! example's tests must pass; a program with failing tests must report each failure with its message and line.

fn run_tests(file: &str, text: &str) -> (i32, String) {
    let (s, code) = crate::source::load(text).unwrap();
    let prog = crate::layout::Program::parse(&s).unwrap();
    let api = crate::cgen::check(&prog, &code).unwrap_or_else(|e| panic!("{}: {}", file, e));
    let units = crate::cgen::program_test_exe(&prog, &api, Some((file, &code))).unwrap();
    let exe = std::env::temp_dir().join(format!("mito-test-check-{}-{}{}", file.replace('.', "_"), std::process::id(), std::env::consts::EXE_SUFFIX));
    crate::build::exe(&units, &exe, true, &[]).unwrap();
    let out = std::process::Command::new(&exe).arg(file).output().unwrap();
    let _ = std::fs::remove_file(&exe);
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"))
}

#[test]
fn the_examples_tests_pass() {
    let text = std::fs::read_to_string("examples/scoreboard/score.mito").unwrap();
    let (code, out) = run_tests("score.mito", &text);
    assert_eq!(code, 0, "{}", out);
    assert!(out.contains("5 passed, 0 failed"), "{}", out);
}

#[test]
fn failing_tests_name_their_line() {
    let text = "struct App { n: i64; } state app: App;
@post(\"/n/{k}\") fn set_n(k: i64) -> Response<{ n: i64 }> { app.n = k; return ok({ n: app.n }); }
test \"passes\" { post(\"/n/3\"); return app.n == 3; }
test \"returns false\" {
    post(\"/n/3\");
    return app.n == 4;
}
test \"returns a message\" {
    let r = post(\"/n/5\");
    if r.status == 200 { return \"n is \" + field(r.body, \"n\"); }
    return true;
}
test \"code that fails\" {
    post(\"/n/1\");
    let x = 9223372036854775807;
    return x + app.n > 0;
}
test \"each test starts empty\" { return app.n == 0 && get(\"/nope\").status == 404; }
";
    let (code, out) = run_tests("fails.mito", text);
    assert_eq!(code, 1, "{}", out);
    for want in ["ok    passes", "FAIL  returns false\n      fails.mito:6: returned false", "FAIL  returns a message\n      fails.mito:10: n is 5",
        "FAIL  code that fails\n      fails.mito:16: error: integer overflow", "ok    each test starts empty", "2 passed, 3 failed"] {
        assert!(out.contains(want), "missing {:?} in\n{}", want, out);
    }
}

/// A test may hold struct values: locals copied from the state, literals, copies between locals, fields written through them; none of it touches the state, which a test still changes
/// only through requests. A local that is not a struct value cannot be written.
#[test]
fn tests_can_use_struct_values() {
    let text = "struct V { x: i64; y: i64 in 0..10; } struct A { p: V; n: i64; } state app: A;
@post(\"/p/{x}\") fn setp(x: i64) -> Response<{ x: i64 }> { app.p.x = x; return ok({ x: app.p.x }); }
test \"copy is independent\" { post(\"/p/3\"); var c = app.p; c.x = 9; c.x += 1; return app.p.x == 3 && c.x == 10; }
test \"literals\" { let v = V { x: 1, y: 2 }; var w = V { ..v, y: 5 }; w.x = w.y + v.x; var z = V {}; z = w; return w.x == 6 && v.y == 2 && z.x == 6 && z.y == 5; }
test \"state starts empty\" { return app.p.x == 0; }
test \"a range holds in a local\" { var c = app.p; c.y = 11; return true; }
test \"a literal range\" { let c = V { x: 1, y: 11 }; return true; }
";
    let (code, out) = run_tests("values.mito", text);
    assert_eq!(code, 1, "{}", out);
    for want in ["ok    copy is independent", "ok    literals", "ok    state starts empty", "FAIL  a range holds in a local", "FAIL  a literal range", "3 passed, 2 failed"] {
        assert!(out.contains(want), "missing {:?} in\n{}", want, out);
    }
    assert!(out.matches("`y` must be in 0..10").count() == 2, "{}", out);
    // the state is not assignable from a test, through a local name or not
    for bad in ["test \"t\" { app.p.x = 1; return true; }", "test \"t\" { let q = app.p; app.n = 1; return true; }"] {
        let src = format!("struct V {{ x: i64; }} struct A {{ p: V; n: i64; }} state app: A;\n{}\n", bad);
        let (s, code) = crate::source::load(&src).unwrap();
        let prog = crate::layout::Program::parse(&s).unwrap();
        let e = crate::cgen::check(&prog, &code).err().unwrap_or_default();
        assert!(e.contains("a test changes the state only through requests"), "{}: {}", bad, e);
    }
}
