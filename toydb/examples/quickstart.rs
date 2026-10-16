//! The smallest useful toydb program: open a database, create a table,
//! insert some rows, and query them.
//!
//! Run with: cargo run --example quickstart

use toydb::{format, Database, QueryResult, Value};

fn main() -> toydb::Result<()> {
    let path = std::env::temp_dir().join("toydb-quickstart.db");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(toydb::storage::wal_path(&path));

    let db = Database::open(&path)?;
    let mut s = db.session();

    s.execute_script(
        "CREATE TABLE books (
             id     INTEGER PRIMARY KEY,
             title  TEXT NOT NULL,
             author TEXT NOT NULL,
             year   INTEGER,
             rating REAL
         );
         INSERT INTO books VALUES
             (1, 'Database Internals', 'Petrov', 2019, 4.6),
             (2, 'Designing Data-Intensive Applications', 'Kleppmann', 2017, 4.8),
             (3, 'Architecture of a Database System', 'Hellerstein', 2007, 4.4),
             (4, 'Readings in Database Systems', 'Bailis', 2015, NULL);",
    )?;

    // Query results come back as rows of `Value`s...
    let result = s.execute("SELECT title, year FROM books WHERE year >= 2015 ORDER BY year")?;
    print!("{}", format::result(&result));

    // ...which you can also pattern-match on directly.
    if let QueryResult::Rows { rows, .. } = s.execute("SELECT AVG(rating) FROM books")? {
        if let Value::Real(avg) = rows[0][0] {
            println!("average rating: {avg:.2}");
        }
    }

    // Transactions: nothing below is visible to anyone until COMMIT.
    s.execute("BEGIN")?;
    s.execute("UPDATE books SET rating = 5.0 WHERE author = 'Petrov'")?;
    s.execute("DELETE FROM books WHERE rating IS NULL")?;
    s.execute("ROLLBACK")?; // changed our mind
    print!("{}", format::result(&s.execute("SELECT COUNT(*) AS books FROM books")?));

    // EXPLAIN shows how a query will run. A primary-key range becomes a
    // B+tree range scan instead of a full table scan.
    print!("{}", format::result(&s.execute("EXPLAIN SELECT title FROM books WHERE id BETWEEN 2 AND 3")?));
    Ok(())
}
