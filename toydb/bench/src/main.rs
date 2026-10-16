//! toydb vs SQLite on a handful of simple workloads.
//!
//! Both engines receive *identical SQL text* for every operation, so both
//! pay for parsing and planning each statement (SQLite's prepared-statement
//! cache isn't used). Both run with a write-ahead log and fsync on commit:
//! SQLite with `journal_mode=WAL, synchronous=FULL`, and the same 4 MiB page
//! cache size as toydb's default buffer pool.
//!
//! Usage: cargo run --release [-- N]      (N rows, default 20000)

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use toydb::util::Rng;

trait Engine {
    fn exec(&mut self, sql: &str);
    /// Run a query returning a single integer.
    fn query_int(&mut self, sql: &str) -> i64;
    /// Run a query and return the number of rows (consuming them all).
    fn query_rows(&mut self, sql: &str) -> usize;
}

struct Toy {
    session: toydb::Session,
    _db: toydb::Database,
}

impl Engine for Toy {
    fn exec(&mut self, sql: &str) {
        self.session.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    fn query_int(&mut self, sql: &str) -> i64 {
        match self.session.execute(sql).unwrap() {
            toydb::QueryResult::Rows { rows, .. } => match &rows[0][0] {
                toydb::Value::Integer(i) => *i,
                toydb::Value::Null => 0,
                v => panic!("{v:?}"),
            },
            r => panic!("{r:?}"),
        }
    }
    fn query_rows(&mut self, sql: &str) -> usize {
        match self.session.execute(sql).unwrap() {
            toydb::QueryResult::Rows { rows, .. } => rows.len(),
            r => panic!("{r:?}"),
        }
    }
}

struct Lite(rusqlite::Connection);

impl Engine for Lite {
    fn exec(&mut self, sql: &str) {
        self.0.execute_batch(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    fn query_int(&mut self, sql: &str) -> i64 {
        self.0
            .query_row(sql, [], |r| r.get::<_, Option<i64>>(0))
            .unwrap()
            .unwrap_or(0)
    }
    fn query_rows(&mut self, sql: &str) -> usize {
        let mut stmt = self.0.prepare(sql).unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut n = 0;
        while rows.next().unwrap().is_some() {
            n += 1;
        }
        n
    }
}

fn fresh(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", p.display()));
    }
    p
}

fn open_toy(dir: &Path) -> Toy {
    let db = toydb::Database::open(fresh(dir, "bench.toydb")).unwrap();
    Toy {
        session: db.session(),
        _db: db,
    }
}

fn open_lite(dir: &Path) -> Lite {
    let c = rusqlite::Connection::open(fresh(dir, "bench.sqlite")).unwrap();
    c.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA cache_size=-4096;",
    )
    .unwrap();
    Lite(c)
}

/// A benchmark body, run once against each engine.
type RunFn = Box<dyn Fn(&mut dyn Engine)>;

struct Workload {
    name: String,
    ops: usize,
    run: RunFn,
}

fn workloads(n: usize) -> Vec<Workload> {
    let mut w: Vec<Workload> = vec![];
    let row = |i: usize| format!("({i}, 'name-{i}', {}, 'some payload text for row {i}')", (i * 7919) % 1000);
    w.push(Workload {
        name: format!("insert {n} rows, sequential keys, 1 txn"),
        ops: n,
        run: Box::new(move |e| {
            e.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, v INTEGER, payload TEXT)");
            e.exec("BEGIN");
            for chunk in (0..n).collect::<Vec<_>>().chunks(100) {
                let values: Vec<String> = chunk.iter().map(|&i| row(i)).collect();
                e.exec(&format!("INSERT INTO t VALUES {}", values.join(", ")));
            }
            e.exec("COMMIT");
        }),
    });
    w.push(Workload {
        name: format!("insert {n} rows, random keys, 1 txn"),
        ops: n,
        run: Box::new(move |e| {
            e.exec("CREATE TABLE r (id INTEGER PRIMARY KEY, name TEXT, v INTEGER, payload TEXT)");
            let mut rng = Rng::new(7);
            let mut keys: Vec<usize> = (0..n).collect();
            for i in (1..keys.len()).rev() {
                keys.swap(i, rng.below(i as u64 + 1) as usize);
            }
            e.exec("BEGIN");
            for chunk in keys.chunks(100) {
                let values: Vec<String> = chunk.iter().map(|&i| row(i)).collect();
                e.exec(&format!("INSERT INTO r VALUES {}", values.join(", ")));
            }
            e.exec("COMMIT");
        }),
    });
    let lookups = n / 2;
    w.push(Workload {
        name: format!("{lookups} point lookups by primary key"),
        ops: lookups,
        run: Box::new(move |e| {
            let mut rng = Rng::new(1);
            let mut found = 0;
            for _ in 0..lookups {
                let id = rng.below(n as u64);
                found += e.query_rows(&format!("SELECT name, v FROM t WHERE id = {id}"));
            }
            assert_eq!(found, lookups);
        }),
    });
    let ranges = 500;
    w.push(Workload {
        name: format!("{ranges} range scans of 100 rows"),
        ops: ranges,
        run: Box::new(move |e| {
            let mut rng = Rng::new(2);
            for _ in 0..ranges {
                let lo = rng.below((n - 100) as u64);
                let got = e.query_rows(&format!(
                    "SELECT id, payload FROM t WHERE id >= {lo} AND id < {}",
                    lo + 100
                ));
                assert_eq!(got, 100);
            }
        }),
    });
    let scans = 20;
    w.push(Workload {
        name: format!("{scans} full scans with filter + aggregate"),
        ops: scans,
        run: Box::new(move |e| {
            for _ in 0..scans {
                e.query_int("SELECT SUM(v) FROM t WHERE v % 7 = 0 AND name LIKE 'name-1%'");
            }
        }),
    });
    w.push(Workload {
        name: "10 GROUP BY + ORDER BY queries".into(),
        ops: 10,
        run: Box::new(move |e| {
            for _ in 0..10 {
                e.query_rows("SELECT v % 100 AS bucket, COUNT(*), AVG(v) FROM t GROUP BY v % 100 ORDER BY 2 DESC LIMIT 5");
            }
        }),
    });
    let joins = 20;
    w.push(Workload {
        name: format!("{joins} joins of 1000 rows via primary key"),
        ops: joins,
        run: Box::new(move |e| {
            for j in 0..joins {
                let lo = j * 500;
                let got = e.query_int(&format!(
                    "SELECT COUNT(*) FROM t JOIN r ON r.id = t.id WHERE t.id >= {lo} AND t.id < {}",
                    lo + 1000
                ));
                assert_eq!(got, 1000);
            }
        }),
    });
    w.push(Workload {
        name: format!("update {} rows by range, 1 txn", n / 2),
        ops: n / 2,
        run: Box::new(move |e| {
            e.exec("BEGIN");
            e.exec(&format!("UPDATE t SET v = v + 1 WHERE id < {}", n / 2));
            e.exec("COMMIT");
        }),
    });
    let small = 200;
    w.push(Workload {
        name: format!("{small} single-row autocommit inserts (fsync each)"),
        ops: small,
        run: Box::new(move |e| {
            for i in 0..small {
                e.exec(&format!("INSERT INTO t VALUES ({}, 'x', 1, 'y')", n + i));
            }
        }),
    });
    w.push(Workload {
        name: format!("delete {} rows, 1 txn", n / 2),
        ops: n / 2,
        run: Box::new(move |e| {
            e.exec("BEGIN");
            e.exec(&format!("DELETE FROM t WHERE id >= {}", n / 2));
            e.exec("COMMIT");
        }),
    });
    w
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .map(|a| a.parse().expect("N must be a number"))
        .unwrap_or(20_000);
    let dir = std::env::temp_dir().join(format!("toydb-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut toy = open_toy(&dir);
    let mut lite = open_lite(&dir);
    println!("toydb vs SQLite {} — {n} rows\n", rusqlite::version());
    println!(
        "| {:<48} | {:>12} | {:>12} | {:>8} |",
        "workload", "toydb", "sqlite", "ratio"
    );
    println!("|{:-<50}|{:->14}|{:->14}|{:->10}|", "", "", "", "");
    for w in workloads(n) {
        let mut times = [Duration::ZERO; 2];
        for (i, engine) in [&mut toy as &mut dyn Engine, &mut lite].into_iter().enumerate() {
            let start = Instant::now();
            (w.run)(engine);
            times[i] = start.elapsed();
        }
        let per = |d: Duration| {
            let us = d.as_secs_f64() * 1e6 / w.ops as f64;
            if us >= 1000.0 {
                format!("{:.2} ms/op", us / 1000.0)
            } else {
                format!("{us:.1} µs/op")
            }
        };
        println!(
            "| {:<48} | {:>12} | {:>12} | {:>7.1}x |",
            w.name,
            per(times[0]),
            per(times[1]),
            times[0].as_secs_f64() / times[1].as_secs_f64()
        );
    }
    let stats = toy._db.stats();
    println!(
        "\ntoydb: {} pages ({} KiB), buffer pool hit rate {:.1}%",
        stats.page_count,
        stats.page_count as u64 * 4,
        100.0 * stats.cache_hits as f64 / (stats.cache_hits + stats.cache_misses).max(1) as f64
    );
    drop(toy);
    drop(lite);
    let _ = std::fs::remove_dir_all(&dir);
}
