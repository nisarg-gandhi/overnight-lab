//! Joins: inner, left, cross; aliases; lookup strategies; agreement with a
//! brute-force model.

use toydb::storage::file::MemVfs;
use toydb::{Database, Error, Options, QueryResult, Session};

fn mem_db() -> Database {
    Database::open_vfs(&MemVfs::new(), "join.db", Options::default()).unwrap()
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

fn rows(v: &[&[&str]]) -> Vec<Vec<String>> {
    v.iter().map(|r| r.iter().map(|x| x.to_string()).collect()).collect()
}

fn setup(s: &mut Session) {
    ok(
        s,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, city TEXT);
         CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, total INTEGER);
         CREATE INDEX orders_by_user ON orders (user_id);
         INSERT INTO users VALUES (1, 'Ada', 'London'), (2, 'Grace', 'NYC'), (3, 'Alan', 'London'), (4, 'Edsger', NULL);
         INSERT INTO orders VALUES (10, 1, 50), (11, 1, 70), (12, 2, 20), (13, 3, 90), (14, NULL, 5), (15, 99, 1);",
    );
}

#[test]
fn inner_join() {
    let db = mem_db();
    let mut s = db.session();
    setup(&mut s);
    let sql = "SELECT u.name, o.total FROM users u JOIN orders o ON o.user_id = u.id ORDER BY o.id";
    assert_eq!(q(&mut s, sql), rows(&[&["Ada", "50"], &["Ada", "70"], &["Grace", "20"], &["Alan", "90"]]));
    let p = plan(&mut s, sql);
    assert!(p.contains("NestedLoopJoin JOIN o ON o.user_id = u.id"), "{p}");
    assert!(p.contains("IndexLookup orders using orders_by_user (user_id = u.id)"), "{p}");
    // Joining the other way round uses the users primary key.
    let sql = "SELECT u.name, o.total FROM orders o INNER JOIN users u ON u.id = o.user_id ORDER BY o.id";
    assert_eq!(q(&mut s, sql), rows(&[&["Ada", "50"], &["Ada", "70"], &["Grace", "20"], &["Alan", "90"]]));
    assert!(plan(&mut s, sql).contains("Lookup users by primary key (id = o.user_id)"));
    // Extra ON conditions and WHERE filters.
    assert_eq!(
        q(&mut s, "SELECT o.id FROM users u JOIN orders o ON o.user_id = u.id AND o.total > 60 WHERE u.city = 'London' ORDER BY 1"),
        rows(&[&["11"], &["13"]])
    );
    // Unqualified names work when unambiguous; ambiguous ones are errors.
    assert_eq!(q(&mut s, "SELECT name, total FROM users JOIN orders ON user_id = users.id WHERE total = 20"), rows(&[&["Grace", "20"]]));
    assert!(matches!(s.execute("SELECT id FROM users JOIN orders ON user_id = users.id"), Err(Error::Query(m)) if m.contains("ambiguous")));
    assert!(s.execute("SELECT * FROM users JOIN users ON 1 = 1").is_err()); // needs aliases
}

#[test]
fn left_join_and_anti_join() {
    let db = mem_db();
    let mut s = db.session();
    setup(&mut s);
    let sql = "SELECT u.name, o.id FROM users u LEFT JOIN orders o ON o.user_id = u.id ORDER BY u.id, o.id";
    assert_eq!(
        q(&mut s, sql),
        rows(&[&["Ada", "10"], &["Ada", "11"], &["Grace", "12"], &["Alan", "13"], &["Edsger", "NULL"]])
    );
    // Users with no orders: WHERE runs after the join, on the padded row.
    assert_eq!(
        q(&mut s, "SELECT u.name FROM users u LEFT OUTER JOIN orders o ON o.user_id = u.id WHERE o.id IS NULL"),
        rows(&[&["Edsger"]])
    );
    // A condition in ON (unlike WHERE) keeps unmatched left rows.
    assert_eq!(
        q(&mut s, "SELECT u.name, o.total FROM users u LEFT JOIN orders o ON o.user_id = u.id AND o.total > 60 ORDER BY u.id"),
        rows(&[&["Ada", "70"], &["Grace", "NULL"], &["Alan", "90"], &["Edsger", "NULL"]])
    );
}

#[test]
fn cross_join_wildcards_and_self_join() {
    let db = mem_db();
    let mut s = db.session();
    setup(&mut s);
    assert_eq!(q(&mut s, "SELECT COUNT(*) FROM users, orders"), rows(&[&["24"]]));
    assert_eq!(q(&mut s, "SELECT COUNT(*) FROM users CROSS JOIN orders WHERE users.id = orders.user_id"), rows(&[&["4"]]));
    let cols = match s.execute("SELECT * FROM users u JOIN orders o ON o.user_id = u.id").unwrap() {
        QueryResult::Rows { columns, .. } => columns,
        _ => unreachable!(),
    };
    assert_eq!(cols, vec!["id", "name", "city", "id", "user_id", "total"]);
    assert_eq!(q(&mut s, "SELECT o.* FROM users u JOIN orders o ON o.user_id = u.id WHERE u.id = 2"), rows(&[&["12", "2", "20"]]));
    // Self join: pairs of users in the same city.
    assert_eq!(
        q(&mut s, "SELECT a.name, b.name FROM users a JOIN users b ON a.city = b.city AND a.id < b.id"),
        rows(&[&["Ada", "Alan"]])
    );
    let p = plan(&mut s, "SELECT a.name FROM users a JOIN users b ON a.city = b.city");
    assert!(p.contains("Scan users (full, materialized once)"), "{p}");
}

#[test]
fn joins_with_aggregates_and_three_tables() {
    let db = mem_db();
    let mut s = db.session();
    setup(&mut s);
    ok(&mut s, "CREATE TABLE items (order_id INTEGER, sku TEXT, qty INTEGER); CREATE INDEX items_by_order ON items (order_id);
                INSERT INTO items VALUES (10, 'a', 1), (10, 'b', 2), (11, 'a', 5), (13, 'c', 1);");
    assert_eq!(
        q(&mut s, "SELECT u.name, COUNT(*) AS orders, SUM(o.total) AS spent FROM users u JOIN orders o ON o.user_id = u.id
                   GROUP BY u.name ORDER BY spent DESC"),
        rows(&[&["Ada", "2", "120"], &["Alan", "1", "90"], &["Grace", "1", "20"]])
    );
    let sql = "SELECT u.name, SUM(i.qty) FROM users u JOIN orders o ON o.user_id = u.id JOIN items i ON i.order_id = o.id
               GROUP BY u.name ORDER BY 1";
    assert_eq!(q(&mut s, sql), rows(&[&["Ada", "8"], &["Alan", "1"]]));
    let p = plan(&mut s, sql);
    assert!(p.contains("IndexLookup items using items_by_order (order_id = o.id)"), "{p}");
    assert_eq!(
        q(&mut s, "SELECT u.name FROM users u LEFT JOIN orders o ON o.user_id = u.id LEFT JOIN items i ON i.order_id = o.id
                   WHERE i.sku = 'a' ORDER BY 1"),
        rows(&[&["Ada"], &["Ada"]])
    );
}

/// Joins must give the same answer whichever strategy the planner picks,
/// and the same answer as a brute-force nested loop in Rust.
mod differential {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]
        #[test]
        fn join_strategies_agree(
            left in prop::collection::vec(prop::option::of(0i64..8), 0..25),
            right in prop::collection::vec(prop::option::of(0i64..8), 0..25),
        ) {
            let db = mem_db();
            let mut s = db.session();
            ok(&mut s, "CREATE TABLE l (id INTEGER PRIMARY KEY, k INTEGER); CREATE TABLE r (id INTEGER PRIMARY KEY, k INTEGER)");
            for (i, k) in left.iter().enumerate() {
                ok(&mut s, &format!("INSERT INTO l VALUES ({i}, {})", k.map_or("NULL".into(), |k| k.to_string())));
            }
            for (i, k) in right.iter().enumerate() {
                ok(&mut s, &format!("INSERT INTO r VALUES ({i}, {})", k.map_or("NULL".into(), |k| k.to_string())));
            }
            // Brute force: inner and left joins on equal non-NULL keys.
            let mut inner = vec![];
            let mut outer = vec![];
            for (i, lk) in left.iter().enumerate() {
                let mut any = false;
                for (j, rk) in right.iter().enumerate() {
                    if lk.is_some() && lk == rk {
                        inner.push(vec![i.to_string(), j.to_string()]);
                        outer.push(vec![i.to_string(), j.to_string()]);
                        any = true;
                    }
                }
                if !any {
                    outer.push(vec![i.to_string(), "NULL".to_string()]);
                }
            }
            let inner_sql = "SELECT l.id, r.id FROM l JOIN r ON r.k = l.k ORDER BY 1, 2";
            let outer_sql = "SELECT l.id, r.id FROM l LEFT JOIN r ON l.k = r.k ORDER BY 1, 2";
            prop_assert!(plan(&mut s, inner_sql).contains("materialized"));
            prop_assert_eq!(q(&mut s, inner_sql), inner.clone());
            prop_assert_eq!(q(&mut s, outer_sql), outer.clone());
            ok(&mut s, "CREATE INDEX rk ON r (k)");
            prop_assert!(plan(&mut s, inner_sql).contains("IndexLookup"));
            prop_assert_eq!(q(&mut s, inner_sql), inner);
            prop_assert_eq!(q(&mut s, outer_sql), outer);
        }
    }
}
