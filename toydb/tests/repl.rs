//! Drive the `toydb` binary the way a user would.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str], stdin: &str) -> (String, String, bool) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_toydb"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
        out.status.success(),
    )
}

#[test]
fn script_on_stdin_and_persistence() {
    let dir = std::env::temp_dir().join(format!("toydb-repl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("repl.db");
    let db = db.to_str().unwrap();
    let (out, err, ok) = run(
        &[db],
        "CREATE TABLE t (id INTEGER PRIMARY KEY,\n name TEXT);\n\
         INSERT INTO t VALUES (1, 'semi;colon'), (2, 'two');\n\
         SELECT name FROM t\n  ORDER BY id DESC;\n\
         SELECT nope FROM t;\n\
         .tables\n.schema t\n.bogus\n",
    );
    assert!(ok);
    assert!(out.contains("CREATE TABLE\nINSERT 2 rows\n"), "{out}");
    assert!(out.contains("| two        |\n| semi;colon |"), "{out}");
    assert!(out.contains("\nt\nCREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT);"), "{out}");
    assert!(err.contains("no such column: nope"), "{err}");
    assert!(err.contains("Unknown command .bogus"), "{err}");

    // One-shot mode, on the same (persisted) file.
    let (out, _, ok) = run(&[db, "SELECT COUNT(*) FROM t"], "");
    assert!(ok);
    assert!(out.contains("|        2 |"), "{out}");
    let (_, err, ok) = run(&[db, "SELECT broken FROM"], "");
    assert!(!ok);
    assert!(err.contains("parse error"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn in_memory_and_demo_script() {
    let demo = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/demo.sql")).unwrap();
    let (out, err, ok) = run(&[], &demo);
    assert!(ok);
    assert!(err.contains("in-memory"), "{err}");
    assert!(!err.contains("Error"), "demo.sql should run cleanly: {err}");
    assert!(out.contains("point lookup id = 4"), "{out}");
    assert!(out.contains("| after_rollback |"), "{out}");
}
