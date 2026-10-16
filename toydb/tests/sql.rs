//! End-to-end SQL tests through the public API.

use std::path::Path;

use toydb::storage::file::MemVfs;
use toydb::{Database, Error, Options, QueryResult, Session, Value};

fn mem_db() -> (MemVfs, Database) {
    let vfs = MemVfs::new();
    let db = Database::open_vfs(&vfs, "test.db", Options::default()).unwrap();
    (vfs, db)
}

/// Run a query and render every value as a string, for compact asserts.
fn q(s: &mut Session, sql: &str) -> Vec<Vec<String>> {
    match s.execute(sql) {
        Ok(QueryResult::Rows { rows, .. }) => rows
            .into_iter()
            .map(|r| r.into_iter().map(|v| v.to_string()).collect())
            .collect(),
        other => panic!("{sql}: expected rows, got {other:?}"),
    }
}

/// Single-column query flattened to a vector.
fn col(s: &mut Session, sql: &str) -> Vec<String> {
    q(s, sql).into_iter().map(|mut r| r.remove(0)).collect()
}

fn one(s: &mut Session, sql: &str) -> String {
    let rows = col(s, sql);
    assert_eq!(rows.len(), 1, "{sql}: {rows:?}");
    rows.into_iter().next().unwrap()
}

fn ok(s: &mut Session, sql: &str) {
    if let Err(e) = s.execute_script(sql) {
        panic!("{sql}: {e}");
    }
}

fn err(s: &mut Session, sql: &str) -> Error {
    s.execute(sql).expect_err(sql)
}

fn columns(s: &mut Session, sql: &str) -> Vec<String> {
    match s.execute(sql).unwrap() {
        QueryResult::Rows { columns, .. } => columns,
        r => panic!("{r:?}"),
    }
}

fn setup_people(s: &mut Session) {
    ok(
        s,
        "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER, city TEXT, score REAL);
         INSERT INTO people VALUES
            (1, 'Ada', 36, 'London', 9.5),
            (2, 'Grace', 45, 'New York', 8.0),
            (3, 'Alan', 41, 'London', NULL),
            (4, 'Edsger', 72, 'Austin', 7.5),
            (5, 'Barbara', NULL, 'Boston', 9.0),
            (6, 'Donald', 84, 'Stanford', 10);",
    );
}

#[test]
fn create_insert_select() {
    let (_, db) = mem_db();
    let mut s = db.session();
    setup_people(&mut s);
    assert_eq!(q(&mut s, "SELECT * FROM people WHERE id = 1"), vec![vec!["1", "Ada", "36", "London", "9.5"]]);
    // INTEGER is widened to REAL on insert into a REAL column.
    assert_eq!(one(&mut s, "SELECT score FROM people WHERE id = 6"), "10.0");
    assert_eq!(columns(&mut s, "SELECT id, name AS who, age + 1 FROM people"), vec!["id", "who", "age + 1"]);
    assert_eq!(one(&mut s, "SELECT 1 + 2 * 3"), "7");
    assert_eq!(one(&mut s, "SELECT 'a' || 'b' || 1"), "ab1");
    assert_eq!(one(&mut s, "SELECT 7 / 2"), "3");
    assert_eq!(one(&mut s, "SELECT 7 / 2.0"), "3.5");
    assert_eq!(one(&mut s, "SELECT -(-3) % 2"), "1");
    // Explicit column list, missing columns become NULL.
    ok(&mut s, "INSERT INTO people (name, id) VALUES ('Linus', 7)");
    assert_eq!(q(&mut s, "SELECT age, city FROM people WHERE id = 7"), vec![vec!["NULL", "NULL"]]);
    assert_eq!(
        s.execute("INSERT INTO people (id, name) VALUES (8, 'x'), (9, 'y')").unwrap(),
        QueryResult::Modified { verb: "INSERT", count: 2 }
    );
}

#[test]
fn where_clauses_and_null_logic() {
    let (_, db) = mem_db();
    let mut s = db.session();
    setup_people(&mut s);
    let names = |s: &mut Session, w: &str| col(s, &format!("SELECT name FROM people WHERE {w} ORDER BY id"));
    assert_eq!(names(&mut s, "age > 40 AND city = 'London'"), vec!["Alan"]);
    assert_eq!(names(&mut s, "age < 40 OR city = 'Boston'"), vec!["Ada", "Barbara"]);
    // NULL age: neither > 50 nor NOT > 50.
    assert_eq!(names(&mut s, "NOT age > 50"), vec!["Ada", "Grace", "Alan"]);
    assert_eq!(names(&mut s, "age IS NULL"), vec!["Barbara"]);
    assert_eq!(names(&mut s, "score IS NOT NULL AND score >= 9"), vec!["Ada", "Barbara", "Donald"]);
    assert_eq!(names(&mut s, "name LIKE 'A%'"), vec!["Ada", "Alan"]);
    assert_eq!(names(&mut s, "name NOT LIKE '%a%'"), vec!["Edsger"]);
    assert_eq!(names(&mut s, "name LIKE '_r%'"), vec!["Grace"]);
    assert_eq!(names(&mut s, "city IN ('Austin', 'Boston')"), vec!["Edsger", "Barbara"]);
    assert_eq!(names(&mut s, "id NOT IN (1, 2, 3, 4)"), vec!["Barbara", "Donald"]);
    assert_eq!(names(&mut s, "age BETWEEN 40 AND 72"), vec!["Grace", "Alan", "Edsger"]);
    assert_eq!(names(&mut s, "age NOT BETWEEN 40 AND 72"), vec!["Ada", "Donald"]);
    assert_eq!(names(&mut s, "NULL = NULL"), Vec::<String>::new());
    assert_eq!(names(&mut s, "TRUE"), vec!["Ada", "Grace", "Alan", "Edsger", "Barbara", "Donald"]);
    assert_eq!(one(&mut s, "SELECT NULL OR TRUE"), "TRUE");
    assert_eq!(one(&mut s, "SELECT NULL AND TRUE"), "NULL");
    assert_eq!(one(&mut s, "SELECT 1 IN (2, NULL)"), "NULL");
    assert_eq!(one(&mut s, "SELECT 1 < 1.5"), "TRUE");
}

#[test]
fn order_by_limit_offset() {
    let (_, db) = mem_db();
    let mut s = db.session();
    setup_people(&mut s);
    assert_eq!(col(&mut s, "SELECT name FROM people ORDER BY age DESC LIMIT 3"), vec!["Donald", "Edsger", "Grace"]);
    // NULLs sort first ascending.
    assert_eq!(col(&mut s, "SELECT name FROM people ORDER BY age LIMIT 2"), vec!["Barbara", "Ada"]);
    assert_eq!(col(&mut s, "SELECT name FROM people ORDER BY city, name DESC"), vec!["Edsger", "Barbara", "Alan", "Ada", "Grace", "Donald"]);
    // By alias, by position, and by an expression not in the select list.
    assert_eq!(col(&mut s, "SELECT name, age * 2 AS dbl FROM people WHERE age IS NOT NULL ORDER BY dbl LIMIT 1"), vec!["Ada"]);
    assert_eq!(col(&mut s, "SELECT name, age FROM people ORDER BY 2 DESC LIMIT 1"), vec!["Donald"]);
    assert_eq!(col(&mut s, "SELECT name FROM people ORDER BY length(name), name"), vec!["Ada", "Alan", "Grace", "Donald", "Edsger", "Barbara"]);
    assert_eq!(columns(&mut s, "SELECT name FROM people ORDER BY age"), vec!["name"]);
    assert_eq!(col(&mut s, "SELECT id FROM people ORDER BY id LIMIT 2 OFFSET 3"), vec!["4", "5"]);
    assert_eq!(col(&mut s, "SELECT id FROM people LIMIT 0"), Vec::<String>::new());
    assert_eq!(col(&mut s, "SELECT id FROM people ORDER BY id LIMIT 10 OFFSET 5"), vec!["6"]);
    assert!(matches!(err(&mut s, "SELECT id FROM people LIMIT -1"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT id FROM people ORDER BY 3"), Error::Query(_)));
}

#[test]
fn aggregates_and_group_by() {
    let (_, db) = mem_db();
    let mut s = db.session();
    setup_people(&mut s);
    assert_eq!(q(&mut s, "SELECT COUNT(*), COUNT(age), MIN(age), MAX(age), SUM(age) FROM people"), vec![vec!["6", "5", "36", "84", "278"]]);
    assert_eq!(one(&mut s, "SELECT AVG(score) FROM people"), "8.8");
    assert_eq!(one(&mut s, "SELECT SUM(score) FROM people WHERE id < 3"), "17.5");
    assert_eq!(one(&mut s, "SELECT MAX(name) FROM people"), "Grace");
    assert_eq!(
        q(&mut s, "SELECT city, COUNT(*) AS n, AVG(age) FROM people GROUP BY city HAVING COUNT(*) >= 1 ORDER BY n DESC, city"),
        vec![
            vec!["London", "2", "38.5"],
            vec!["Austin", "1", "72.0"],
            vec!["Boston", "1", "NULL"],
            vec!["New York", "1", "45.0"],
            vec!["Stanford", "1", "84.0"],
        ]
    );
    assert_eq!(col(&mut s, "SELECT city FROM people GROUP BY city HAVING COUNT(*) > 1"), vec!["London"]);
    assert_eq!(col(&mut s, "SELECT COUNT(DISTINCT city) FROM people"), vec!["5"]);
    assert_eq!(col(&mut s, "SELECT DISTINCT city FROM people ORDER BY city"), vec!["Austin", "Boston", "London", "New York", "Stanford"]);
    assert_eq!(col(&mut s, "SELECT age / 10 * 10 AS decade FROM people WHERE age IS NOT NULL GROUP BY age / 10 * 10 ORDER BY decade"), vec!["30", "40", "70", "80"]);
    // Aliases and positions in GROUP BY / HAVING.
    assert_eq!(
        q(&mut s, "SELECT age / 10 * 10 AS decade, COUNT(*) AS n FROM people WHERE age IS NOT NULL GROUP BY decade HAVING n >= 1 ORDER BY decade"),
        vec![vec!["30", "1"], vec!["40", "2"], vec!["70", "1"], vec!["80", "1"]]
    );
    assert_eq!(col(&mut s, "SELECT city, COUNT(*) FROM people GROUP BY 1 HAVING COUNT(*) = 2"), vec!["London"]);
    // A real column wins over an alias of the same name.
    assert_eq!(col(&mut s, "SELECT COUNT(*) AS age FROM people GROUP BY age ORDER BY 1 DESC LIMIT 1"), vec!["1"]);
    assert!(matches!(err(&mut s, "SELECT city FROM people GROUP BY 2"), Error::Query(_)));
    // Aggregates over an empty input.
    assert_eq!(q(&mut s, "SELECT COUNT(*), SUM(age), MAX(age) FROM people WHERE id > 100"), vec![vec!["0", "NULL", "NULL"]]);
    assert!(q(&mut s, "SELECT city, COUNT(*) FROM people WHERE id > 100 GROUP BY city").is_empty());
    // Ungrouped column use is an error.
    assert!(matches!(err(&mut s, "SELECT name, COUNT(*) FROM people"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT id FROM people WHERE COUNT(*) > 1"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT SUM(COUNT(*)) FROM people"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT SUM(name) FROM people"), Error::Query(_)));
}

#[test]
fn scalar_functions() {
    let (_, db) = mem_db();
    let mut s = db.session();
    assert_eq!(one(&mut s, "SELECT upper('abc') || lower('DEF')"), "ABCdef");
    assert_eq!(one(&mut s, "SELECT length('héllo')"), "5");
    assert_eq!(one(&mut s, "SELECT abs(-5) + abs(2)"), "7");
    assert_eq!(one(&mut s, "SELECT coalesce(NULL, NULL, 3)"), "3");
    assert_eq!(one(&mut s, "SELECT typeof(1.5)"), "real");
    assert_eq!(one(&mut s, "SELECT round(2.567, 2)"), "2.57");
    assert_eq!(one(&mut s, "SELECT substr('database', 5)"), "base");
    assert_eq!(one(&mut s, "SELECT substr('database', 1, 4)"), "data");
    assert!(matches!(err(&mut s, "SELECT nosuch(1)"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT upper(1, 2)"), Error::Query(_)));
}

#[test]
fn update_and_delete() {
    let (_, db) = mem_db();
    let mut s = db.session();
    setup_people(&mut s);
    assert_eq!(
        s.execute("UPDATE people SET age = age + 1, city = 'Cambridge' WHERE city = 'London'").unwrap(),
        QueryResult::Modified { verb: "UPDATE", count: 2 }
    );
    assert_eq!(q(&mut s, "SELECT age, city FROM people WHERE id = 3"), vec![vec!["42", "Cambridge"]]);
    // Shift every primary key: must not collide with not-yet-updated rows.
    ok(&mut s, "UPDATE people SET id = id + 1");
    assert_eq!(col(&mut s, "SELECT id FROM people"), vec!["2", "3", "4", "5", "6", "7"]);
    assert_eq!(one(&mut s, "SELECT name FROM people WHERE id = 2"), "Ada");
    // Changing a key onto an existing one is a constraint violation, and the
    // whole statement is undone.
    assert!(matches!(err(&mut s, "UPDATE people SET id = 2 WHERE id = 3"), Error::Constraint(_)));
    assert_eq!(one(&mut s, "SELECT name FROM people WHERE id = 3"), "Grace");
    assert!(matches!(err(&mut s, "UPDATE people SET name = NULL"), Error::Constraint(_)));
    assert!(matches!(err(&mut s, "UPDATE people SET age = 'old'"), Error::Query(_)));
    assert_eq!(
        s.execute("DELETE FROM people WHERE age > 50 OR age IS NULL").unwrap(),
        QueryResult::Modified { verb: "DELETE", count: 3 }
    );
    assert_eq!(one(&mut s, "SELECT COUNT(*) FROM people"), "3");
    ok(&mut s, "DELETE FROM people");
    assert_eq!(one(&mut s, "SELECT COUNT(*) FROM people"), "0");
}

#[test]
fn constraints_and_errors() {
    let (_, db) = mem_db();
    let mut s = db.session();
    setup_people(&mut s);
    assert!(matches!(err(&mut s, "INSERT INTO people VALUES (1, 'dup', 1, 'x', 1.0)"), Error::Constraint(_)));
    assert!(matches!(err(&mut s, "INSERT INTO people (id) VALUES (10)"), Error::Constraint(_)));
    assert!(matches!(err(&mut s, "INSERT INTO people (name) VALUES ('no id')"), Error::Constraint(_)));
    assert!(matches!(err(&mut s, "INSERT INTO people VALUES (10, 'x')"), Error::Query(_)));
    assert!(matches!(err(&mut s, "INSERT INTO people (id, nope) VALUES (10, 1)"), Error::Query(_)));
    assert!(matches!(err(&mut s, "INSERT INTO people (id, name, age) VALUES (10, 'x', 'old')"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT * FROM nope"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT nope FROM people"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT other.id FROM people"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT 1 / 0"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT name + 1 FROM people"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT * FROM people WHERE name"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELECT 'a' < 1"), Error::Query(_)));
    assert!(matches!(err(&mut s, "CREATE TABLE people (x INTEGER)"), Error::Query(_)));
    assert!(matches!(err(&mut s, "CREATE TABLE t (a INTEGER, a TEXT)"), Error::Query(_)));
    assert!(matches!(err(&mut s, "CREATE TABLE t (a INTEGER PRIMARY KEY, b INTEGER PRIMARY KEY)"), Error::Query(_)));
    assert!(matches!(err(&mut s, "CREATE TABLE toydb_x (a INTEGER)"), Error::Query(_)));
    assert!(matches!(err(&mut s, "DELETE FROM toydb_schema"), Error::Query(_)));
    assert!(matches!(err(&mut s, "SELEC 1"), Error::Parse(_)));
    // Failed statements changed nothing.
    assert_eq!(one(&mut s, "SELECT COUNT(*) FROM people"), "6");
    ok(&mut s, "CREATE TABLE IF NOT EXISTS people (x INTEGER); DROP TABLE IF EXISTS nope;");
}

#[test]
fn text_primary_keys_and_rowid_tables() {
    let (_, db) = mem_db();
    let mut s = db.session();
    ok(&mut s, "CREATE TABLE kv (k TEXT PRIMARY KEY, v INTEGER); CREATE TABLE log (msg TEXT)");
    ok(&mut s, "INSERT INTO kv VALUES ('b', 2), ('a', 1), ('c', 3), ('a\u{0}b', 4)");
    // Rows come back in key order.
    assert_eq!(col(&mut s, "SELECT v FROM kv"), vec!["1", "4", "2", "3"]);
    assert_eq!(col(&mut s, "SELECT v FROM kv WHERE k >= 'b'"), vec!["2", "3"]);
    for m in ["first", "second", "third"] {
        ok(&mut s, &format!("INSERT INTO log VALUES ('{m}')"));
    }
    ok(&mut s, "DELETE FROM log WHERE msg = 'third'");
    ok(&mut s, "INSERT INTO log VALUES ('fourth')");
    // Rowid tables keep insertion order.
    assert_eq!(col(&mut s, "SELECT msg FROM log"), vec!["first", "second", "fourth"]);
}

#[test]
fn explain_shows_access_paths() {
    let (_, db) = mem_db();
    let mut s = db.session();
    setup_people(&mut s);
    let plan = |s: &mut Session, sql: &str| col(s, &format!("EXPLAIN {sql}")).join("\n");
    let p = plan(&mut s, "SELECT name FROM people WHERE id = 3");
    assert!(p.contains("Scan people (point lookup id = 3)"), "{p}");
    let p = plan(&mut s, "SELECT name FROM people WHERE id > 2 AND id <= 4 AND age > 1");
    assert!(p.contains("range id > 2 AND id <= 4"), "{p}");
    let p = plan(&mut s, "SELECT name FROM people WHERE 3 < id");
    assert!(p.contains("range id > 3"), "{p}");
    let p = plan(&mut s, "SELECT name FROM people WHERE id BETWEEN 2 AND 4");
    assert!(p.contains("range id >= 2 AND id <= 4"), "{p}");
    let p = plan(&mut s, "SELECT name FROM people WHERE age = 3 OR id = 1");
    assert!(p.contains("(full)"), "{p}");
    let p = plan(&mut s, "SELECT city, COUNT(*) FROM people GROUP BY city ORDER BY 2 DESC LIMIT 1");
    for op in ["Limit 1", "Sort", "Project", "Aggregate", "Scan people (full)"] {
        assert!(p.contains(op), "{p}");
    }
    let p = plan(&mut s, "DELETE FROM people WHERE id < 3");
    assert!(p.contains("Delete people") && p.contains("range id < 3"), "{p}");
    // Range results agree with full-scan results.
    assert_eq!(col(&mut s, "SELECT id FROM people WHERE id > 2 AND id <= 4"), vec!["3", "4"]);
    assert_eq!(col(&mut s, "SELECT id FROM people WHERE id >= 2 AND id > 4"), vec!["5", "6"]);
    assert_eq!(col(&mut s, "SELECT id FROM people WHERE id < 2.5"), vec!["1", "2"]);
}

#[test]
fn schema_persists_across_reopen() {
    let vfs = MemVfs::new();
    {
        let db = Database::open_vfs(&vfs, "p.db", Options::default()).unwrap();
        let mut s = db.session();
        setup_people(&mut s);
        ok(&mut s, "CREATE TABLE \"Odd Name\" (\"key\" TEXT, text TEXT); INSERT INTO \"Odd Name\" VALUES ('k', 't')");
        ok(&mut s, "CREATE TABLE gone (x INTEGER); DROP TABLE gone");
    }
    let db = Database::open_vfs(&vfs, "p.db", Options::default()).unwrap();
    let mut s = db.session();
    assert_eq!(one(&mut s, "SELECT name FROM people WHERE id = 4"), "Edsger");
    assert_eq!(q(&mut s, "SELECT \"key\", text FROM \"Odd Name\""), vec![vec!["k", "t"]]);
    assert_eq!(db.table_names().unwrap(), vec!["Odd Name", "people"]);
    assert_eq!(
        col(&mut s, "SELECT sql FROM toydb_schema WHERE name = 'people'"),
        vec!["CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER, city TEXT, score REAL)"]
    );
}

#[test]
fn drop_table_frees_pages() {
    let (_, db) = mem_db();
    let mut s = db.session();
    ok(&mut s, "CREATE TABLE big (id INTEGER PRIMARY KEY, data TEXT)");
    for i in 0..300 {
        ok(&mut s, &format!("INSERT INTO big VALUES ({i}, '{}')", "x".repeat(500)));
    }
    let before = db.stats();
    ok(&mut s, "DROP TABLE big");
    let after = db.stats();
    assert_eq!(after.page_count, before.page_count);
    assert!(after.free_pages >= 30, "{after:?}");
    // Freed pages are reused.
    ok(&mut s, "CREATE TABLE big2 (id INTEGER PRIMARY KEY, data TEXT)");
    for i in 0..100 {
        ok(&mut s, &format!("INSERT INTO big2 VALUES ({i}, '{}')", "y".repeat(500)));
    }
    assert_eq!(db.stats().page_count, before.page_count);
}

#[test]
fn large_rows_and_many_rows() {
    let (_, db) = mem_db();
    let mut s = db.session();
    ok(&mut s, "CREATE TABLE docs (id INTEGER PRIMARY KEY, body TEXT)");
    let big = "lorem ipsum ".repeat(5000); // ~60 KB: overflow pages
    ok(&mut s, &format!("INSERT INTO docs VALUES (1, '{big}')"));
    assert_eq!(one(&mut s, "SELECT length(body) FROM docs WHERE id = 1"), big.len().to_string());
    s.execute("BEGIN").unwrap();
    for i in 2..5000 {
        ok(&mut s, &format!("INSERT INTO docs VALUES ({i}, 'doc {i}')"));
    }
    s.execute("COMMIT").unwrap();
    assert_eq!(one(&mut s, "SELECT COUNT(*) FROM docs"), "4999");
    assert_eq!(one(&mut s, "SELECT body FROM docs WHERE id = 4321"), "doc 4321");
    assert_eq!(one(&mut s, "SELECT COUNT(*) FROM docs WHERE id >= 1000 AND id < 2000"), "1000");
}

#[test]
fn value_api() {
    let (_, db) = mem_db();
    match db.execute("SELECT 1, 2.5, 'x', TRUE, NULL").unwrap() {
        QueryResult::Rows { rows, .. } => assert_eq!(
            rows[0],
            vec![Value::Integer(1), Value::Real(2.5), Value::Text("x".into()), Value::Boolean(true), Value::Null]
        ),
        r => panic!("{r:?}"),
    }
}

#[test]
fn real_files() {
    let dir = std::env::temp_dir().join(format!("toydb-sql-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("real.db");
    {
        let db = Database::open(&path).unwrap();
        let mut s = db.session();
        ok(&mut s, "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t VALUES (1, 'one')");
        // A second open of the same file in the same process is refused.
        assert!(matches!(Database::open(&path), Err(Error::Busy(_))));
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(one(&mut db.session(), "SELECT v FROM t"), "one");
    drop(db);
    assert!(Path::new(&path).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

// --- differential testing against a simple model ----------------------------------------

mod differential {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    #[derive(Debug, Clone)]
    enum Op {
        Insert(i64, Option<i64>),
        Update(i64, i64, i64),
        Delete(i64, i64),
        Query(i64, i64),
        Begin,
        Commit,
        Rollback,
    }

    fn arb_op() -> impl Strategy<Value = Op> {
        let id = -20i64..60;
        prop_oneof![
            6 => (id.clone(), prop::option::of(-100i64..100)).prop_map(|(i, v)| Op::Insert(i, v)),
            2 => (id.clone(), id.clone(), -5i64..5).prop_map(|(a, b, d)| Op::Update(a, b, d)),
            2 => (id.clone(), id.clone()).prop_map(|(a, b)| Op::Delete(a, b)),
            3 => (id.clone(), id).prop_map(|(a, b)| Op::Query(a, b)),
            1 => Just(Op::Begin),
            1 => Just(Op::Commit),
            1 => Just(Op::Rollback),
        ]
    }

    fn sql_val(v: Option<i64>) -> String {
        v.map_or("NULL".into(), |v| v.to_string())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn sql_matches_model(ops in prop::collection::vec(arb_op(), 1..150)) {
            let (_, db) = mem_db();
            let mut s = db.session();
            ok(&mut s, "CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER, label TEXT)");
            let mut committed: BTreeMap<i64, Option<i64>> = BTreeMap::new();
            let mut cur = committed.clone();
            let mut in_txn = false;
            for op in ops {
                match op {
                    Op::Insert(id, v) => {
                        let r = s.execute(&format!("INSERT INTO t VALUES ({id}, {}, 'row {id}')", sql_val(v)));
                        if let std::collections::btree_map::Entry::Vacant(e) = cur.entry(id) {
                            r.unwrap();
                            e.insert(v);
                        } else {
                            prop_assert!(matches!(r, Err(Error::Constraint(_))));
                        }
                    }
                    Op::Update(a, b, d) => {
                        s.execute(&format!("UPDATE t SET v = v + {d} WHERE id >= {a} AND id < {b}")).unwrap();
                        for (_, v) in cur.range_mut(a..b.max(a)) {
                            *v = v.map(|x| x + d);
                        }
                    }
                    Op::Delete(a, b) => {
                        s.execute(&format!("DELETE FROM t WHERE id BETWEEN {a} AND {b}")).unwrap();
                        cur.retain(|k, _| !(a..=b).contains(k));
                    }
                    Op::Query(a, b) => {
                        let got = q(&mut s, &format!("SELECT id, v FROM t WHERE id > {a} AND id <= {b} ORDER BY id"));
                        let want: Vec<Vec<String>> = cur
                            .iter()
                            .filter(|(k, _)| **k > a && **k <= b)
                            .map(|(k, v)| vec![k.to_string(), sql_val(*v)])
                            .collect();
                        prop_assert_eq!(got, want);
                        let sum: Option<i64> = cur.values().flatten().copied().reduce(|x, y| x + y);
                        prop_assert_eq!(one(&mut s, "SELECT SUM(v) FROM t"), sql_val(sum));
                    }
                    Op::Begin if !in_txn => { ok(&mut s, "BEGIN"); in_txn = true; }
                    Op::Commit if in_txn => { ok(&mut s, "COMMIT"); in_txn = false; committed = cur.clone(); }
                    Op::Rollback if in_txn => { ok(&mut s, "ROLLBACK"); in_txn = false; cur = committed.clone(); }
                    _ => {}
                }
                if !in_txn {
                    committed = cur.clone();
                }
            }
            let n = one(&mut s, "SELECT COUNT(*) FROM t");
            prop_assert_eq!(n, cur.len().to_string());
        }
    }
}
