# Learning databases with toydb

A reading list mapped to the code. The idea: pick a layer, read the material
next to it, then read (and break, and fix) the module. Every module has a
long doc comment at the top explaining its design, and DESIGN.md goes deeper.

## The core resources

* **CMU 15-445/645 Database Systems** (Andy Pavlo, Carnegie Mellon). Free
  lecture videos, slides and notes at <https://15445.courses.cs.cmu.edu>.
  The course project, *BusTub*, implements many of the same components in
  C++; toydb is a good companion because it makes different, simpler choices
  that you can compare against. Lecture numbers change between semesters, so
  the table below refers to lectures by topic.
* **Alex Petrov, *Database Internals* (O'Reilly, 2019).** Part I (storage
  engines) is the best book-length treatment of B-trees, file formats and
  recovery for practitioners.
* **The SQLite documentation** (<https://sqlite.org/docs.html>). SQLite is
  the closest real-world relative of toydb: embedded, single file, B-trees,
  WAL, a VFS, a self-hosted schema table. Its design documents are short,
  precise and free.

## Reading map

| toydb module | what it does | CMU 15-445 lecture topic | *Database Internals* | SQLite docs |
|---|---|---|---|---|
| `storage/file.rs` | VFS, crash simulator | Database Storage I | Ch. 3 *File Formats* | [The OS Interface Object (VFS)](https://sqlite.org/vfs.html); [How SQLite Is Tested](https://sqlite.org/testing.html) (crash testing) |
| `storage/pager.rs`, `storage/mod.rs` | pages, header, free list | Database Storage I & II | Ch. 3 *File Formats* (page structure) | [Database File Format](https://sqlite.org/fileformat2.html) §1 (pages, header, freelist) |
| `storage/buffer.rs` | buffer pool, LRU | Memory Management / Buffer Pools | Ch. 5 *Transaction Processing and Recovery* (buffer management section) | [Architecture of SQLite](https://sqlite.org/arch.html) (the pager) |
| `btree/` | B+tree, splits, merges, overflow pages, cursors | Tree Indexes (B+Trees) | Ch. 2 *B-Tree Basics*, Ch. 4 *Implementing B-Trees* | [Database File Format](https://sqlite.org/fileformat2.html) §1.6 (b-tree pages, overflow) |
| `storage/wal.rs` | write-ahead log, recovery, checkpoints | Database Logging; Database Recovery | Ch. 5 *Transaction Processing and Recovery* (recovery section) | [Write-Ahead Logging](https://sqlite.org/wal.html); [Atomic Commit In SQLite](https://sqlite.org/atomiccommit.html) |
| `txn.rs`, `db.rs` | 2PL locks, sessions, savepoints | Concurrency Control Theory; Two-Phase Locking | Ch. 5 (concurrency control section) | [File Locking And Concurrency](https://sqlite.org/lockingv3.html); [Isolation In SQLite](https://sqlite.org/isolation.html) |
| `types.rs` | row format, memcomparable keys | Database Storage II (tuple layout) | Ch. 3 (*Binary Encoding*) | [Database File Format](https://sqlite.org/fileformat2.html) §2.1 (record format) |
| `catalog.rs` | self-hosted schema, secondary indexes | Database Storage (system catalogs) | — | [Database File Format](https://sqlite.org/fileformat2.html) §2.6 (the schema table) |
| `sql/lexer.rs`, `sql/parser.rs` | tokens, recursive descent | Modern SQL (for the language) | — | [SQL As Understood By SQLite](https://sqlite.org/lang.html) |
| `sql/planner.rs` | binding, aggregation rewrite, access paths | Query Planning & Optimization | — | [The SQLite Query Optimizer Overview](https://sqlite.org/optoverview.html) |
| `sql/exec.rs`, `sql/expr.rs` | Volcano iterators, joins, aggregation, sorting | Query Execution; Join Algorithms; Sorting & Aggregation Algorithms | — | [Architecture of SQLite](https://sqlite.org/arch.html) (the VDBE, for contrast: bytecode instead of iterators) |

## Suggested path

1. **Storage first.** Watch the CMU lectures on database storage and buffer
   pools, read *Database Internals* chapter 3, then read
   `storage/pager.rs` and `storage/buffer.rs`. Exercise: replace LRU with
   CLOCK or LRU-K (the course's buffer-pool project uses LRU-K) and measure
   the hit rate with `.stats`.
2. **B+trees.** CMU's tree-index lecture and *Database Internals* chapters 2
   and 4. Read `btree/node.rs` then `btree/mod.rs`. Exercise: implement
   redistribution (borrowing from a sibling) on delete and check the
   property tests still pass; or add suffix truncation of separator keys.
3. **Recovery.** Read SQLite's *Atomic Commit* (a beautifully illustrated
   walk through rollback journals) and *Write-Ahead Logging*, then CMU's
   logging and recovery lectures, then `storage/wal.rs`. Run
   `cargo run --example crash_recovery`. Exercise: delete the `sync()` call
   in `Wal::append_commit` and watch `tests/crash.rs` fail.
4. **Transactions.** CMU's concurrency-control lectures (theory, then 2PL),
   then `txn.rs` and `db.rs`. Run `cargo run --release --example bank`.
   Exercise: add `BEGIN IMMEDIATE` (take the exclusive lock at BEGIN) and
   see how the bank example's retry count changes.
5. **Query processing.** CMU's query execution and planning lectures, then
   `sql/` in pipeline order: lexer → parser → planner → exec. Use `EXPLAIN`
   constantly. Exercises: add a hash join for joins without a usable
   index; make the planner use an index's second column after an equality
   on its first.

## Papers and other classics

* Joseph M. Hellerstein, Michael Stonebraker, James Hamilton.
  **"Architecture of a Database System"** (Foundations and Trends in
  Databases, 2007). The map of the whole territory; read first or last.
* Goetz Graefe. **"Volcano — An Extensible and Parallel Query Evaluation
  System"** (IEEE TKDE, 1994). The iterator model used by `sql/exec.rs`.
* Goetz Graefe. **"Modern B-Tree Techniques"** (Foundations and Trends in
  Databases, 2011). Everything toydb's B+tree leaves out.
* Douglas Comer. **"The Ubiquitous B-Tree"** (ACM Computing Surveys, 1979).
* C. Mohan et al. **"ARIES: A Transaction Recovery Method Supporting
  Fine-Granularity Locking and Partial Rollbacks Using Write-Ahead
  Logging"** (ACM TODS, 1992). What toydb's redo-only, no-steal design
  avoids — and why real systems need it.
* Jim Gray, Andreas Reuter. ***Transaction Processing: Concepts and
  Techniques*** (1992). The reference on locking and recovery.
* Hal Berenson et al. **"A Critique of ANSI SQL Isolation Levels"**
  (SIGMOD 1995). Why "serializable" and "snapshot isolation" differ.
* Patricia Selinger et al. **"Access Path Selection in a Relational
  Database Management System"** (SIGMOD 1979). The origin of cost-based
  optimization; toydb's `choose_access` is its tiniest possible ancestor.
* Robert Nystrom. ***Crafting Interpreters*** (free online). Its chapters on
  scanning and parsing explain the same recursive-descent technique as
  `sql/lexer.rs` and `sql/parser.rs`.

## Other small databases worth reading

* **SQLite** source, especially `btree.c`, `pager.c` and `wal.c`.
* **Erik Grinaker's toydb** (<https://github.com/erikgrinaker/toydb>): a
  distributed SQL database in Rust with Raft and MVCC — the natural next
  step after this project (and no relation despite the name).
* **BusTub** (<https://github.com/cmu-db/bustub>): the CMU course's
  educational DBMS in C++.
* **BoltDB / bbolt** (Go): a single-file B+tree key-value store with
  copy-on-write pages instead of a WAL — an instructive alternative design.
