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

/// The open-world game's rules (where the orbs lie, when one is collected) are tested without a window.
#[test]
fn the_world_games_rules_pass() {
    let file = "examples/world/world.mito";
    let text = crate::load_text(file).unwrap();
    crate::lang::set_file_dir(std::path::Path::new(file).parent().unwrap());
    let (code, out) = run_tests("world.mito", &text);
    assert_eq!(code, 0, "{}", out);
    assert!(out.contains("3 passed, 0 failed"), "{}", out);
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
