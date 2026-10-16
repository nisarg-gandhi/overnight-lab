//! Secondary indexes: planning, maintenance, constraints, persistence.

use toydb::storage::file::MemVfs;
use toydb::{Database, Error, Options, QueryResult, Session};

fn mem_db() -> (MemVfs, Database) {
    let vfs = MemVfs::new();
    let db = Database::open_vfs(&vfs, "idx.db", Options::default()).unwrap();
    (vfs, db)
}

fn q(s: &mut Session, sql: &str) -> Vec<Vec<String>> {
    match s.execute(sql) {
        Ok(QueryResult::Rows { rows, .. }) => rows
            .into_iter()
            .map(|r| r.into_iter().map(|v| v.to_string()).collect())
            .collect(),
        other => panic!("{sql}: expected rows, got {other:?}"),
    }
}

fn plan(s: &mut Session, sql: &str) -> String {
    q(s, &format!("EXPLAIN {sql}")).into_iter().map(|r| r[0].clone()).collect::<Vec<_>>().join("\n")
}

fn ok(s: &mut Session, sql: &str) {
    if let Err(e) = s.execute_script(sql) {
        panic!("{sql}: {e}");
    }
}

fn assert_consistent(s: &mut Session) {
    let problems = s.check_integrity().unwrap();
    assert!(problems.is_empty(), "integrity problems: {problems:?}");
}

fn setup(s: &mut Session) {
    ok(
        s,
        "CREATE TABLE emp (id INTEGER PRIMARY KEY, name TEXT, dept TEXT, salary INTEGER);
         INSERT INTO emp VALUES (1, 'Ada', 'eng', 120), (2, 'Grace', 'eng', 135), (3, 'Alan', 'research', 110),
                                (4, 'Barbara', 'research', 125), (5, 'Edsger', 'eng', 99), (6, 'Donald', NULL, 90),
                                (7, 'Linus', 'ops', NULL);
         CREATE INDEX by_dept ON emp (dept);
         CREATE INDEX by_salary ON emp (salary);",
    );
}

#[test]
fn index_is_used_and_agrees_with_full_scan() {
    let (_, db) = mem_db();
    let mut s = db.session();
    setup(&mut s);
    assert_consistent(&mut s);
    let p = plan(&mut s, "SELECT name FROM emp WHERE dept = 'eng'");
    assert!(p.contains("IndexScan emp using by_dept (dept = 'eng')"), "{p}");
    let p = plan(&mut s, "SELECT name FROM emp WHERE salary > 100 AND salary <= 125");
    assert!(p.contains("IndexScan emp using by_salary (salary > 100 AND salary <= 125)"), "{p}");
    // A primary-key equality still beats any index.
    let p = plan(&mut s, "SELECT name FROM emp WHERE id = 3 AND dept = 'eng'");
    assert!(p.contains("point lookup id = 3"), "{p}");
    // Index equality beats a primary-key range.
    let p = plan(&mut s, "SELECT name FROM emp WHERE id > 1 AND dept = 'eng'");
    assert!(p.contains("using by_dept"), "{p}");

    // Each indexed query must return the same rows as an unindexable
    // rewrite of it (`x || ''` hides the column from the planner).
    for (indexed, scanned) in [
        ("dept = 'eng'", "dept || '' = 'eng'"),
        ("dept = 'nope'", "dept || '' = 'nope'"),
        ("salary > 100 AND salary <= 125", "salary + 0 > 100 AND salary + 0 <= 125"),
        ("salary >= 120", "salary + 0 >= 120"),
        ("salary < 110", "salary + 0 < 110"),
        ("salary BETWEEN 90 AND 99", "salary + 0 BETWEEN 90 AND 99"),
        ("dept > 'eng'", "dept || '' > 'eng'"),
    ] {
        let a = q(&mut s, &format!("SELECT id FROM emp WHERE {indexed} ORDER BY id"));
        let b = q(&mut s, &format!("SELECT id FROM emp WHERE {scanned} ORDER BY id"));
        assert_eq!(a, b, "{indexed}");
    }
    // Index scans return rows in index order.
    assert_eq!(
        q(&mut s, "SELECT salary FROM emp WHERE salary >= 110"),
        vec![vec!["110"], vec!["120"], vec!["125"], vec!["135"]]
    );
}

#[test]
fn dml_keeps_indexes_consistent() {
    let (_, db) = mem_db();
    let mut s = db.session();
    setup(&mut s);
    ok(&mut s, "UPDATE emp SET dept = 'ops' WHERE dept = 'research'");
    assert_consistent(&mut s);
    assert_eq!(q(&mut s, "SELECT COUNT(*) FROM emp WHERE dept = 'ops'"), vec![vec!["3"]]);
    assert_eq!(q(&mut s, "SELECT COUNT(*) FROM emp WHERE dept = 'research'"), vec![vec!["0"]]);
    // Changing the primary key moves index entries to the new key.
    ok(&mut s, "UPDATE emp SET id = id + 100");
    assert_consistent(&mut s);
    assert_eq!(q(&mut s, "SELECT id FROM emp WHERE dept = 'eng' ORDER BY id"), vec![vec!["101"], vec!["102"], vec!["105"]]);
    ok(&mut s, "DELETE FROM emp WHERE salary < 100");
    assert_consistent(&mut s);
    assert_eq!(q(&mut s, "SELECT COUNT(*) FROM emp WHERE salary < 1000"), vec![vec!["4"]]);
    ok(&mut s, "DELETE FROM emp WHERE dept = 'eng'");
    assert_consistent(&mut s);
    ok(&mut s, "DELETE FROM emp");
    assert_consistent(&mut s);
}

#[test]
fn unique_indexes() {
    let (_, db) = mem_db();
    let mut s = db.session();
    ok(&mut s, "CREATE TABLE u (id INTEGER PRIMARY KEY, email TEXT UNIQUE, n INTEGER)");
    ok(&mut s, "INSERT INTO u VALUES (1, 'a@x', 1), (2, 'b@x', 2), (3, NULL, 3), (4, NULL, 4)");
    let e = s.execute("INSERT INTO u VALUES (5, 'a@x', 5)").unwrap_err();
    assert!(matches!(e, Error::Constraint(ref m) if m.contains("UNIQUE")), "{e}");
    // A failing multi-row insert leaves nothing behind (statement atomicity).
    assert!(s.execute("INSERT INTO u VALUES (5, 'c@x', 5), (6, 'c@x', 6)").is_err());
    assert_eq!(q(&mut s, "SELECT COUNT(*) FROM u"), vec![vec!["4"]]);
    assert!(matches!(s.execute("UPDATE u SET email = 'a@x' WHERE id = 2"), Err(Error::Constraint(_))));
    // Swapping two unique values in one statement is fine.
    ok(&mut s, "UPDATE u SET id = 3 - id WHERE id IN (1, 2)");
    assert_eq!(q(&mut s, "SELECT id, email FROM u WHERE email = 'a@x'"), vec![vec!["2", "a@x"]]);
    assert_consistent(&mut s);
    // The UNIQUE constraint's index can't be dropped.
    let auto = q(&mut s, "SELECT name FROM toydb_schema WHERE type = 'index'");
    assert_eq!(auto, vec![vec!["toydb_autoindex_u_1"]]);
    assert!(s.execute("DROP INDEX toydb_autoindex_u_1").is_err());
    // A unique index can't be built over duplicates, and leaves no trace.
    ok(&mut s, "INSERT INTO u VALUES (10, 'q', 1)");
    assert!(matches!(s.execute("CREATE UNIQUE INDEX un ON u (n)"), Err(Error::Constraint(_))));
    assert_eq!(q(&mut s, "SELECT COUNT(*) FROM toydb_schema WHERE name = 'un'"), vec![vec!["0"]]);
    assert_consistent(&mut s);
    // Multi-column unique index.
    ok(&mut s, "CREATE TABLE pair (a INTEGER, b TEXT); CREATE UNIQUE INDEX ab ON pair (a, b)");
    ok(&mut s, "INSERT INTO pair VALUES (1, 'x'), (1, 'y'), (2, 'x')");
    assert!(s.execute("INSERT INTO pair VALUES (1, 'y')").is_err());
    let p = plan(&mut s, "SELECT b FROM pair WHERE a = 1");
    assert!(p.contains("IndexScan pair using ab (a = 1)"), "{p}");
    assert_eq!(q(&mut s, "SELECT b FROM pair WHERE a = 1"), vec![vec!["x"], vec!["y"]]);
}

#[test]
fn ddl_errors_drop_and_rollback() {
    let (_, db) = mem_db();
    let mut s = db.session();
    setup(&mut s);
    assert!(s.execute("CREATE INDEX by_dept ON emp (name)").is_err());
    ok(&mut s, "CREATE INDEX IF NOT EXISTS by_dept ON emp (name)");
    assert!(s.execute("CREATE INDEX emp ON emp (name)").is_err()); // name taken by a table
    assert!(s.execute("CREATE INDEX x ON emp (nope)").is_err());
    assert!(s.execute("CREATE INDEX x ON nope (a)").is_err());
    assert!(s.execute("CREATE INDEX x ON emp (name, name)").is_err());
    assert!(s.execute("DROP INDEX nope").is_err());
    ok(&mut s, "DROP INDEX IF EXISTS nope");

    ok(&mut s, "BEGIN; CREATE INDEX by_name ON emp (name); ROLLBACK");
    assert!(!plan(&mut s, "SELECT * FROM emp WHERE name = 'Ada'").contains("IndexScan"));
    ok(&mut s, "DROP INDEX by_dept");
    assert!(!plan(&mut s, "SELECT * FROM emp WHERE dept = 'eng'").contains("IndexScan"));
    let before = db.stats().free_pages;
    ok(&mut s, "DROP TABLE emp");
    assert!(db.stats().free_pages > before);
    assert_eq!(q(&mut s, "SELECT COUNT(*) FROM toydb_schema"), vec![vec!["0"]]);
    assert_consistent(&mut s);
}

#[test]
fn indexes_persist() {
    let vfs = MemVfs::new();
    {
        let db = Database::open_vfs(&vfs, "p.db", Options::default()).unwrap();
        let mut s = db.session();
        setup(&mut s);
        ok(&mut s, "CREATE TABLE t (k TEXT PRIMARY KEY, v TEXT UNIQUE)");
        ok(&mut s, "INSERT INTO t VALUES ('a', 'x')");
    }
    let db = Database::open_vfs(&vfs, "p.db", Options::default()).unwrap();
    let mut s = db.session();
    assert!(plan(&mut s, "SELECT * FROM emp WHERE dept = 'eng'").contains("IndexScan"));
    assert_eq!(q(&mut s, "SELECT name FROM emp WHERE dept = 'research' ORDER BY name"), vec![vec!["Alan"], vec!["Barbara"]]);
    assert!(s.execute("INSERT INTO t VALUES ('b', 'x')").is_err());
    assert_consistent(&mut s);
}

/// Larger randomized workload: every query through the index must agree
/// with a model, and the integrity check must pass throughout.
mod differential {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    #[derive(Debug, Clone)]
    enum Op {
        Insert(i64, Option<i64>),
        SetV(i64, i64, Option<i64>),
        Delete(i64),
        QueryEq(i64),
        QueryRange(i64, i64),
    }

    fn arb_op() -> impl Strategy<Value = Op> {
        let id = 0i64..80;
        let v = prop::option::weighted(0.9, -10i64..10);
        prop_oneof![
            4 => (id.clone(), v.clone()).prop_map(|(i, v)| Op::Insert(i, v)),
            2 => (id.clone(), id.clone(), v).prop_map(|(a, b, v)| Op::SetV(a, b, v)),
            2 => id.prop_map(Op::Delete),
            2 => (-11i64..11).prop_map(Op::QueryEq),
            2 => (-11i64..11, -11i64..11).prop_map(|(a, b)| Op::QueryRange(a, b)),
        ]
    }

    fn sql(v: Option<i64>) -> String {
        v.map_or("NULL".into(), |v| v.to_string())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]
        #[test]
        fn index_matches_model(ops in prop::collection::vec(arb_op(), 1..120)) {
            let (_, db) = mem_db();
            let mut s = db.session();
            ok(&mut s, "CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER, pad TEXT); CREATE INDEX tv ON t (v)");
            let mut model: BTreeMap<i64, Option<i64>> = BTreeMap::new();
            for op in ops {
                match op {
                    Op::Insert(id, v) => {
                        let r = s.execute(&format!("INSERT INTO t VALUES ({id}, {}, '{}')", sql(v), "p".repeat(id as usize * 3)));
                        prop_assert_eq!(r.is_ok(), !model.contains_key(&id));
                        model.entry(id).or_insert(v);
                    }
                    Op::SetV(a, b, v) => {
                        ok(&mut s, &format!("UPDATE t SET v = {} WHERE id BETWEEN {a} AND {b}", sql(v)));
                        if a <= b {
                            for (_, x) in model.range_mut(a..=b) {
                                *x = v;
                            }
                        }
                    }
                    Op::Delete(id) => {
                        ok(&mut s, &format!("DELETE FROM t WHERE id = {id}"));
                        model.remove(&id);
                    }
                    Op::QueryEq(v) => {
                        let got = q(&mut s, &format!("SELECT id FROM t WHERE v = {v} ORDER BY id"));
                        let want: Vec<Vec<String>> = model.iter().filter(|(_, x)| **x == Some(v)).map(|(k, _)| vec![k.to_string()]).collect();
                        prop_assert_eq!(got, want);
                    }
                    Op::QueryRange(a, b) => {
                        let got = q(&mut s, &format!("SELECT id FROM t WHERE v >= {a} AND v < {b} ORDER BY id"));
                        let want: Vec<Vec<String>> = model.iter().filter(|(_, x)| x.is_some_and(|x| x >= a && x < b)).map(|(k, _)| vec![k.to_string()]).collect();
                        prop_assert_eq!(got, want);
                    }
                }
            }
            let problems = s.check_integrity().unwrap();
            prop_assert!(problems.is_empty(), "{:?}", problems);
        }
    }
}
