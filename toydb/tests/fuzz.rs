//! Robustness: malformed input must produce errors, never panics.

use proptest::prelude::*;
use toydb::btree::node::{page_find_child, page_leaf_lookup, page_node_size, Node};
use toydb::sql::parser::parse_script;
use toydb::storage::file::MemVfs;
use toydb::types::decode_row;
use toydb::{Database, Options};

const WORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
    "TABLE", "INDEX", "UNIQUE", "ON", "JOIN", "LEFT", "GROUP", "BY", "ORDER", "HAVING", "LIMIT",
    "AND", "OR", "NOT", "NULL", "IS", "IN", "LIKE", "BETWEEN", "(", ")", ",", ";", "*", "+", "-",
    "/", "=", "<", ">=", "||", "t", "a", "b", "1", "2.5", "'x'", "\"q\"", "COUNT", "sum", ".",
    "PRIMARY", "KEY", "INTEGER", "TEXT", "BEGIN", "COMMIT", "ROLLBACK", "DISTINCT", "AS", "DROP",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn parser_never_panics_on_arbitrary_text(s in ".{0,200}") {
        let _ = parse_script(&s);
    }

    #[test]
    fn parser_never_panics_on_token_soup(words in prop::collection::vec(prop::sample::select(WORDS), 0..40)) {
        let _ = parse_script(&words.join(" "));
    }

    #[test]
    fn page_decoding_never_panics(bytes in prop::collection::vec(any::<u8>(), 4096), kind in 0u8..4) {
        let mut page = [0u8; 4096];
        page.copy_from_slice(&bytes);
        page[0] = kind; // make the node kinds likely
        let _ = Node::decode(&page);
        let _ = page_node_size(&page);
        let _ = page_find_child(&page, Some(b"key"));
        let _ = page_leaf_lookup(&page, b"key");
    }

    #[test]
    fn row_decoding_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
        let _ = decode_row(&bytes);
    }
}

/// Run random statements — a mix of plausible ones (many of which violate
/// constraints or types) and pure token soup — against a real database.
/// Every one must return Ok or Err, and the database must stay consistent.
#[test]
fn random_statements_keep_database_consistent() {
    let db = Database::open_vfs(&MemVfs::new(), "f.db", Options::default()).unwrap();
    let mut s = db.session();
    s.execute_script(
        "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT UNIQUE, c INTEGER NOT NULL); CREATE INDEX tc ON t (c);
         INSERT INTO t VALUES (1, 'x', 1), (2, 'y', 2);",
    )
    .unwrap();
    let mut rng = toydb::util::Rng::new(5);
    let pick = |rng: &mut toydb::util::Rng, options: &[&'static str]| {
        options[rng.below(options.len() as u64) as usize]
    };
    let mut ok_count = 0;
    for _ in 0..3000 {
        let int = pick(&mut rng, &["0", "1", "2", "3", "-1", "NULL", "'2'", "2.5"]);
        let text = pick(&mut rng, &["'x'", "'y'", "'z'", "NULL", "1", "b", "'w' || b"]);
        let op = pick(&mut rng, &["=", "<", ">=", "!=", "<="]);
        let sql = match pick(&mut rng, &["ins", "ins", "upd", "del", "sel", "txn", "soup"]) {
            "ins" => format!("INSERT INTO t VALUES ({int}, {text}, {int}), ({int}, {text}, 5)"),
            "upd" => format!("UPDATE t SET b = {text}, c = c + {int} WHERE a {op} {int}"),
            "del" => format!("DELETE FROM t WHERE c {op} {int} OR b = {text}"),
            "sel" => format!("SELECT a, COUNT(*) FROM t WHERE c {op} {int} GROUP BY a ORDER BY 2 LIMIT {int}"),
            "txn" => pick(&mut rng, &["BEGIN", "COMMIT", "ROLLBACK"]).to_string(),
            _ => {
                let n = 1 + rng.below(12) as usize;
                (0..n).map(|_| WORDS[rng.below(WORDS.len() as u64) as usize]).collect::<Vec<_>>().join(" ")
            }
        };
        if s.execute_script(&sql).is_ok() {
            ok_count += 1;
        }
    }
    // Plenty of statements must have succeeded, or this tests nothing.
    assert!(ok_count > 500, "only {ok_count} statements succeeded");
    if s.in_transaction() {
        s.execute("ROLLBACK").unwrap();
    }
    let problems = s.check_integrity().unwrap();
    assert!(problems.is_empty(), "{problems:?}");
}
