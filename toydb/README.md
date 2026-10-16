# toydb

A small but real embedded SQL database, written from scratch in Rust as a
learning project. It stores data in a single file of 4 KiB pages, indexes it
with a B+tree, survives power failures thanks to a write-ahead log, supports
transactions with locking, and speaks a useful subset of SQL through a
hand-written parser and an iterator-based executor.

The library has **zero dependencies** (only `std`). Every layer is
documented for readers who want to learn how databases work:

* [`DESIGN.md`](DESIGN.md) — how each layer works, with diagrams of the
  on-disk formats, the recovery algorithm and the query pipeline, plus the
  log of design decisions and their tradeoffs.
* [`LEARNING.md`](LEARNING.md) — a reading list (CMU 15-445, *Database
  Internals*, the SQLite docs, classic papers) mapped to the modules here.

> Not to be confused with Erik Grinaker's
> [toydb](https://github.com/erikgrinaker/toydb), a distributed SQL database
> in Rust that is also excellent reading.

## Quickstart

```sh
cd toydb
cargo build --release
./target/release/toydb my.db            # interactive shell
```

```text
toydb> CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score REAL);
CREATE TABLE
toydb> INSERT INTO users VALUES (1, 'Ada', 9.5), (2, 'Grace', NULL), (3, 'Linus', 7);
INSERT 3 rows
toydb> SELECT * FROM users WHERE score IS NOT NULL ORDER BY score DESC;
+----+-------+-------+
| id | name  | score |
+----+-------+-------+
|  1 | Ada   |   9.5 |
|  3 | Linus |   7.0 |
+----+-------+-------+
(2 rows)
toydb> EXPLAIN SELECT name FROM users WHERE id >= 2;
+--------------------------------+
| plan                           |
+--------------------------------+
| Project name                   |
|   Filter id >= 2               |
|     Scan users (range id >= 2) |
+--------------------------------+
(3 rows)
```

Other ways to run the shell:

```sh
./target/release/toydb                           # throwaway in-memory database
./target/release/toydb my.db "SELECT * FROM users"   # one-shot
./target/release/toydb my.db < examples/demo.sql     # run a script
```

Shell commands: `.help`, `.tables`, `.schema [table]`, `.stats`,
`.timer on|off`, `.read FILE`, `.checkpoint`, `.quit`.

### As a library

```rust
use toydb::{Database, QueryResult, Value};

let db = Database::open("app.db")?;
let mut s = db.session();                 // one session per thread
s.execute("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v INTEGER)")?;
s.execute("BEGIN")?;
s.execute("INSERT INTO kv VALUES ('a', 1), ('b', 2)")?;
s.execute("COMMIT")?;
if let QueryResult::Rows { rows, .. } = s.execute("SELECT v FROM kv WHERE k = 'b'")? {
    assert_eq!(rows[0][0], Value::Integer(2));
}
```

Examples (`cargo run --example <name>`):

| example          | shows                                                                 |
|------------------|-----------------------------------------------------------------------|
| `quickstart`     | the API: tables, queries, transactions, EXPLAIN                       |
| `bank`           | threads doing concurrent transfers; serializability keeps the total   |
| `crash_recovery` | simulated power cuts; acknowledged commits survive, others vanish     |
| `demo.sql`       | a tour of the SQL dialect (`toydb < examples/demo.sql`)               |

## Features

**Storage**
* Single database file of fixed 4 KiB pages, plus a `-wal` file.
* Buffer pool with O(1) LRU eviction (intrusive linked list).
* Free-page list; dropped tables and merged B+tree nodes are reused.

**Indexing**
* B+tree with node splits, merges on delete, linked leaves for range
  scans, and overflow pages for large values (rows can be megabytes).
* Order-preserving key encoding so `WHERE pk < 10` is a range scan.

**Durability**
* Write-ahead log of full page images with CRC-32 and a per-generation
  salt; fsync on commit.
* Crash recovery by redo replay; automatic checkpointing.
* Tested by a power-failure simulator that drops and tears unsynced writes.

**Transactions**
* `BEGIN` / `COMMIT` / `ROLLBACK`, autocommit otherwise.
* Strict two-phase locking at database granularity (many readers or one
  writer): serializable isolation.
* Upgrade-deadlock detection and lock timeouts (`Busy` errors).
* Statement-level atomicity: a failing statement inside a transaction is
  undone without aborting the transaction. DDL is transactional too.

**SQL**
* `CREATE TABLE [IF NOT EXISTS]`, `DROP TABLE [IF EXISTS]`
* `INSERT INTO t [(cols)] VALUES (...), (...)`
* `SELECT [DISTINCT] ... FROM t WHERE ... GROUP BY ... HAVING ...
  ORDER BY ... [ASC|DESC] LIMIT n OFFSET m`
* `UPDATE t SET ... WHERE ...`, `DELETE FROM t WHERE ...`
* `EXPLAIN <statement>`
* Types: `INTEGER`, `REAL`, `TEXT`, `BOOLEAN` (+ `NULL`); constraints
  `PRIMARY KEY`, `NOT NULL`.
* Operators: `+ - * / %`, `||`, comparisons, `AND OR NOT` (three-valued
  logic), `IS [NOT] NULL`, `[NOT] LIKE`, `[NOT] IN (...)`,
  `[NOT] BETWEEN`.
* Aggregates: `COUNT(*)`, `COUNT/SUM/AVG/MIN/MAX([DISTINCT] x)`.
* Scalar functions: `abs`, `lower`, `upper`, `length`, `substr`, `round`,
  `coalesce`, `typeof`.
* Self-hosted catalog: `SELECT * FROM toydb_schema`.
* Error messages point at the line and column of syntax errors.

## Limitations

toydb is a learning project; these are deliberate simplifications (see
DESIGN.md for why and how each could be lifted):

* **No joins or subqueries.** A query reads one table.
* **No secondary indexes.** Only the primary key is indexed; other
  predicates are evaluated by scanning.
* **Single-column primary keys only**; no `UNIQUE`, `CHECK`, `DEFAULT`,
  foreign keys or `ALTER TABLE`.
* **Coarse locking.** One writer *or* many readers for the whole database;
  readers block writers (no MVCC/snapshot isolation).
* **No-steal buffer management.** A transaction's dirty pages stay in
  memory until commit, so a single transaction can't modify more data than
  fits in RAM.
* **Results are materialized.** `execute` returns all rows at once rather
  than streaming them; sorting and grouping are in-memory.
* **One process at a time** per database file (enforced with a file lock).
* Keys are limited to 512 bytes; `fsync=false` mode trades crash safety for
  speed like SQLite's `synchronous=OFF`.

## Project layout

```text
toydb/
├── src/
│   ├── storage/       file.rs (VFS + crash simulator), pager.rs, buffer.rs (LRU),
│   │                  wal.rs, mod.rs (Storage: header, free list, commit)
│   ├── btree/         node.rs (page format), mod.rs (search/insert/delete/cursor)
│   ├── txn.rs         lock manager
│   ├── types.rs       values, row encoding, order-preserving key encoding
│   ├── catalog.rs     toydb_schema
│   ├── sql/           lexer, parser, ast, planner, expr (evaluation), exec (Volcano)
│   ├── db.rs          Database / Session (public API, transactions)
│   ├── format.rs      ASCII tables
│   └── bin/toydb.rs   the REPL
├── tests/             crash.rs, sql.rs, txn.rs, repl.rs
├── examples/          quickstart, bank, crash_recovery, demo.sql
└── bench/             separate crate: benchmark against SQLite
```

## Testing

```sh
cargo test              # unit, property, crash-simulation and end-to-end tests
cargo clippy --all-targets
```

(The benchmark crate in `bench/` is separate so these never compile SQLite.)

Highlights of the test suite:

* **B+tree property tests** compare random operation sequences against
  `std::collections::BTreeMap` and check structural invariants (ordering,
  separator bounds, equal leaf depth, leaf chain) and that no page leaks.
* **Power-failure simulation**: `MemVfs` cuts the power after a random
  number of I/O operations, then randomly loses or tears (at 512-byte sector
  granularity) every write that wasn't fsynced. After recovery the database
  must contain exactly the committed transactions — plus, possibly, the one
  whose commit was in flight. A bank-transfer workload checks that money is
  never created or destroyed.
* **Differential SQL tests** run random INSERT/UPDATE/DELETE/SELECT and
  BEGIN/COMMIT/ROLLBACK sequences against both toydb and an in-memory model.
* **Concurrency tests** run threads doing read-modify-write transactions and
  check that no update is lost.
* **Mutation checks**: breaking WAL replay or the commit fsync makes the
  crash tests fail (verified while developing).

## Benchmarks

`toydb/bench` compares toydb with SQLite (via `rusqlite`) on identical SQL:

```sh
cd bench && cargo run --release -- 20000
```

Typical ratios (toydb time / SQLite time) on the development VM: durable
single-row commits **1.1×** (both are fsync-bound), point lookups **1.2×**,
GROUP BY queries **1.2×**, scans **~4×**, bulk inserts **3–7×**, bulk deletes
**~45×**. DESIGN.md has the full table, the methodology, and an explanation of
where the time goes (mostly: decoding/encoding whole nodes instead of editing
slotted pages in place).

## License

MIT
