# toydb design

This document explains how toydb works, layer by layer, from bytes on disk
up to SQL. It is written as a learning resource: each section explains the
*problem* a layer solves, the *approach* toydb takes, and the *tradeoffs*
against what production databases do. Code references point at the module
that implements each idea; the source is heavily commented too.

Contents

1. [The big picture](#1-the-big-picture)
2. [Files, the VFS and the crash simulator](#2-files-the-vfs-and-the-crash-simulator)
3. [The pager and the header page](#3-the-pager-and-the-header-page)
4. [The buffer pool](#4-the-buffer-pool)
5. [The B+tree](#5-the-btree)
6. [The write-ahead log and crash recovery](#6-the-write-ahead-log-and-crash-recovery)
7. [Transactions and locking](#7-transactions-and-locking)
8. [Values, rows and keys](#8-values-rows-and-keys)
9. [The catalog](#9-the-catalog)
10. [The query pipeline](#10-the-query-pipeline)
11. [Testing strategy](#11-testing-strategy)
12. [Benchmarks](#benchmarks)
13. [Decision log](#13-decision-log)
14. [Limitations and where to go next](#14-limitations-and-where-to-go-next)

---

## 1. The big picture

```text
                 SQL text
                    │
   ┌────────────────▼─────────────────┐
   │ db.rs        Database / Session  │  transactions, autocommit, savepoints
   ├──────────────────────────────────┤
   │ sql/lexer.rs   tokens            │
   │ sql/parser.rs  AST               │  recursive descent
   │ sql/planner.rs physical plan     │  binding, aggregates, access paths
   │ sql/exec.rs    Volcano iterators │  scan/filter/project/sort/aggregate
   ├──────────────────────────────────┤
   │ catalog.rs   toydb_schema table  │  table definitions as SQL text
   │ types.rs     values & encodings  │  row format, order-preserving keys
   ├──────────────────────────────────┤        ┌────────────────────────┐
   │ btree/       B+tree              │        │ txn.rs  LockManager    │
   ├──────────────────────────────────┤        │ shared/exclusive, 2PL  │
   │ storage/mod.rs  Storage          │        └────────────────────────┘
   │   header page, free list,        │
   │   commit / rollback / checkpoint │
   │ storage/buffer.rs  LRU cache     │
   │ storage/wal.rs     redo log      │
   │ storage/pager.rs   page I/O      │
   │ storage/file.rs    VFS           │
   └───────────────┬──────────────────┘
                   │
       path.db (pages)    path.db-wal (log)
```

Each layer only talks to the one below it. The B+tree, for example, sees
storage only through the four-method `PageStore` trait (`read`, `write`,
`allocate`, `free`), which is also what lets its tests run on a toy
in-memory store.

A statement's life: the session takes a lock, the parser builds an AST, the
planner turns it into a tree of operators, the executor pulls rows through
that tree, the operators read B+tree pages through the buffer pool, and at
commit the dirty pages go to the WAL (fsync) and then to the database file.

---

## 2. Files, the VFS and the crash simulator

*Module: `storage/file.rs`*

All I/O goes through two small traits, borrowed in spirit from SQLite's VFS:

```rust
trait Vfs  { fn open(&self, path) -> Box<dyn File>; }
trait File { read_at, write_at, size, set_len, sync, try_lock }
```

`OsVfs` maps them onto `std::fs`. The interesting one is **`MemVfs`**, an
in-memory file system that models what a disk is allowed to do when the
power fails. For each file it keeps:

```text
 current : bytes that reads see (like the OS page cache)
 durable : bytes guaranteed to survive a crash
 pending : list of writes / truncates since the last sync()
```

`sync()` makes `durable = current`. `crash(seed)` replays the pending
operations onto `durable` with a random fate for each: **lost**, **fully
persisted**, or **torn** (each 512-byte sector independently persisted or
not). `fail_after(n)` makes the *n*-th next mutating operation, and every
operation after it, fail — the "power cut" — so tests can interrupt the
engine in the middle of a commit or a checkpoint.

This is the same idea as SQLite's crash-test VFS and it is the most valuable
testing tool in the project: it turns "is recovery correct?" from a matter of
careful reasoning into something hundreds of randomized tests check.

**Process locking.** `File::try_lock` takes an advisory exclusive lock on
the database file (`std::fs::File::try_lock`, i.e. `flock` on Unix), so a
second process — or a second `Database::open` of the same path — gets a
`Busy` error instead of corrupting the file.

---

## 3. The pager and the header page

*Modules: `storage/pager.rs`, `storage/mod.rs`*

The pager treats the database file as an array of 4 KiB pages: page *n* is
at byte offset *n × 4096*. 4 KiB matches the page size of operating systems
and most SSDs, so writing one database page is one device write. Reading past
the end of the file yields zeroes (a page may be allocated and committed via
the WAL before it ever reaches the main file).

Page 0 is the **header**:

```text
offset  size  field
0       8     magic "TOYDB\0v1"       detects "this isn't a toydb file"
8       4     page size (4096)
12      4     page count              pages in use (file may be longer)
16      4     free list head          0 = empty
20      4     free list length
24      4     catalog root page       root of the toydb_schema B+tree
28      8     last committed txn id
36..          zero
```

The header is an ordinary page as far as the buffer pool and WAL are
concerned. That design choice pays off repeatedly: allocating a page changes
the header, so the header is dirty in that transaction, so it is logged in
the WAL and rolled back by ROLLBACK like any other page — allocation becomes
transactional for free.

**Free list.** Freed pages form a singly linked list threaded through the
pages themselves (`[0xFE marker][...][next u32 at offset 4]`), with the head
in the header. `allocate()` pops the head or, if the list is empty, extends
the file by bumping the page count. The file never shrinks (SQLite's
`VACUUM` would be the analogue).

---

## 4. The buffer pool

*Module: `storage/buffer.rs`*

Disk is slow; the buffer pool keeps recently used pages in memory.

```text
             page_table: HashMap<PageId, FrameId>
                  │
   frames:  [ f0: page 7 | f1: page 0 (dirty) | f2: page 3 | ... ]

   LRU list (intrusive, by frame index):
     head (most recent) ⇄ f2 ⇄ f0 ⇄ f1 ⇄ tail (least recent)
```

* **Lookup** is a hash-map probe. A hit moves the frame to the head of the
  list; a miss loads the page into a free frame or evicts one.
* **LRU in O(1).** The list is a doubly linked list stored as `prev`/`next`
  indices inside the frames themselves (no allocation per access). Moving a
  frame to the front is four index updates.
* **Eviction** walks from the tail looking for a *clean* frame (see next
  point). Evicting a clean page needs no I/O.

**No-steal.** A page modified by an uncommitted transaction is never written
to the database file — the "no-steal" policy in recovery jargon. That is why
dirty frames are never evicted: they are effectively pinned until commit or
rollback. Rollback is then trivial (`discard_dirty()` drops the frames and
the next access re-reads the committed version from disk) and the WAL never
needs undo records. The cost: a transaction can't dirty more pages than fit
in memory. toydb lets the pool *grow* past its capacity rather than fail,
so huge transactions work but use RAM. ARIES-style engines ("steal") instead
write uncommitted pages under memory pressure and log *undo* information to
take them back out.

**Where are the pin counts?** Textbook buffer pools (and CMU 15-445's
BusTub) hand out pointers into frames and maintain pin counts so a frame
isn't evicted while someone holds a pointer. In toydb, `read()`/`write()`
return references borrowed from `&mut self`, so the borrow checker proves
at compile time that no reference survives the next pool call — the only
moment eviction can happen. The B+tree copies what it needs (it decodes nodes
into owned structs), so it never needs a long-lived reference. This is a
nice example of Rust ownership replacing a runtime protocol.

---

## 5. The B+tree

*Modules: `btree/node.rs` (format), `btree/mod.rs` (algorithms)*

Every table is a B+tree mapping an encoded primary key to an encoded row.
The catalog is one too.

```text
                         root (page 3 — never moves)
                        [  "m"  ]
                       /         \
              [ "d" | "h" ]      [ "t" ]               internal nodes: keys + child ids
              /    |     \        /    \
           [a,c]->[d,f]->[h,k]->[m,p]->[t,z]           leaves: keys + values, linked
```

### Node format

```text
Common header (7 bytes)
+--------+-----------+-------------------------------+
| kind   | num_keys  | link                          |
| u8     | u16       | u32                           |
+--------+-----------+-------------------------------+
  1 = leaf      link = next leaf (0 = last leaf)
  2 = internal  link = leftmost child

Leaf entries, packed after the header, sorted by key:
+---------+---------+---------+-----------+
| key_len | val_len | key ... | value ... |
| u16     | u16     |         |           |
+---------+---------+---------+-----------+
  val_len bit 15 set => the value lives in overflow pages and the
  8 value bytes are [total_len u32][first overflow page u32]

Internal entries:
+---------+---------+-------------+
| key_len | key ... | right child |     child[i+1] holds keys >= key[i]
| u16     |         | u32         |     link (= child[0]) holds keys < key[0]
+---------+---------+-------------+

Overflow page:
+-----------+-----------+--------------------+
| next u32  | used u16  | up to 4090 bytes   |
+-----------+-----------+--------------------+
```

**Decode/modify/encode.** Each visit decodes a page into a `Node` struct
(`Vec`s of keys, values, children), the algorithm edits it with ordinary
`Vec` operations, and it is encoded back. Production engines use *slotted
pages* and edit bytes in place, which avoids copying but makes every
operation fiddly. For a learning project, readable algorithms win. The cost
shows up in the benchmarks: point reads copy a whole node.

**Size limits that make splitting always work.** Keys are at most 512
bytes, and a leaf entry stored inline is at most a quarter page (1022
bytes); bigger values go to an overflow chain. Consequently any overfull
node has enough entries that splitting it at its byte midpoint yields two
halves that each fit in a page, and internal nodes always have at least
seven keys when they split.

### Search

From the root, at each internal node pick child `i` = the number of keys
`<= search key` (`partition_point`), and descend. At the leaf, binary search.
Cost: one page per level; with ~100-200 children per internal node a tree of
depth 3 holds millions of rows.

### Insert and split

Insert is recursive: insert into the right child; if the child split, it
returns a *separator key* and the id of its new right sibling, which the
parent inserts into itself — possibly overflowing and splitting in turn.

```text
leaf overflows                   split at the byte midpoint
[a b c d e f g]          =>      [a b c d] -> [e f g]      separator "e" goes up
                                 (left keeps the page id, right is new,
                                  leaf links fixed: left.next = right)

internal node overflows          middle key moves up and appears in neither half
[c f j m q]              =>      [c f] [m q]   separator "j" goes up
```

**The root never moves.** When the root splits, its contents are copied to
a newly allocated page and the root page is rewritten as an internal node
with two children. Because the root page id is stable, the catalog never
needs updating when a table's tree grows or shrinks.

### Delete and merge

Delete removes the entry from its leaf. On the way back up, if a child has
fallen below a quarter full, the parent tries to **merge** it with a
sibling: if the two (plus the separator, for internal nodes) fit in one
page, the right node is appended to the left, its page is freed, and the
separator is removed from the parent. When the root is left with a single
child, that child is pulled up into the root page.

toydb does **not** redistribute (borrow entries from a fuller sibling), so a
node can stay under-full when its siblings are too full to merge with. That
wastes some space but never affects correctness — search doesn't care how
full nodes are. An empty leaf always merges (its size adds nothing), so the
tree doesn't accumulate empty leaves in practice.

### Range scans and cursors

`range(lo, hi)` descends to the leaf that would contain `lo`, then a
`Cursor` walks the leaf chain. The cursor holds a decoded copy of the
current leaf, so it never holds a buffer-pool reference between calls (see
§4). The flip side: modifying a tree while a cursor is open over it would
let the cursor see stale data, so UPDATE and DELETE first collect the
matching keys and then modify (§10).

### Invariants and testing

`BTree::check` verifies: keys sorted in every node; every key within the
bounds given by its ancestors' separators; all leaves at the same depth; the
leaf chain visits leaves in order and ends in 0; every node fits a page.
Property tests (proptest) run random sequences of insert/delete/get/range
against `BTreeMap`, call `check`, and assert that the number of live pages
equals the number of reachable pages — so a page leak on any code path fails
the test.

---

## 6. The write-ahead log and crash recovery

*Modules: `storage/wal.rs`, `storage/mod.rs`*

### The problem

A transaction typically modifies several pages. If the machine crashes
after some of them reach the disk but not others, the B+tree is corrupt. A
single page write can also be **torn**: partly old, partly new. We need
commits to be *atomic* and *durable*.

### The approach: physical redo logging

Before any page of a transaction touches the database file, its new contents
(the *after-image* of the whole page) are appended to a separate log file,
followed by a commit record, and the log is fsynced. The fsync returning is
the **commit point**.

```text
WAL file
+------------------------+
| header (24 bytes)      |  magic "TOYDBWAL", version, page size, salt, crc32
+------------------------+
| PAGE   txn=7  page=0   |  \
| PAGE   txn=7  page=12  |   } transaction 7: committed
| COMMIT txn=7  n=2      |  /
| PAGE   txn=8  page=0   |  \
| PAGE   txn=8  page=3   |   } transaction 8: no COMMIT record => ignored
| (torn garbage)         |  /
+------------------------+

Record
+------+--------+---------+---------+------------------+--------+
| type | txn id | page id | length  | payload          | crc32  |
| u8   | u64    | u32     | u32     | `length` bytes   | u32    |
+------+--------+---------+---------+------------------+--------+
  type 1 = PAGE    payload = full 4096-byte page image
  type 2 = COMMIT  payload empty; the page-id field holds the number of
                   PAGE records, as a cross-check
  crc32 covers the salt and every preceding byte of the record
```

### Commit protocol (`Storage::commit`)

```text
1. bump txn id in the header page (so page 0 is always in the txn)
2. WAL: append PAGE records for every dirty page + COMMIT record
        (one write() call), then fsync              <── commit point
3. write the same pages into the database file (no fsync)
4. mark frames clean
5. if WAL > checkpoint threshold (4 MiB): checkpoint
```

Step 3 without an fsync is safe because the WAL can always redo those
writes. Writing them eagerly (rather than only at checkpoint) keeps the
database file current, so the buffer pool can simply drop clean pages and
rollback can re-read the committed version from the file.

### Recovery algorithm (`Wal::recover`, run by every `open`)

```text
if the WAL header is missing or its checksum is wrong:
    nothing can have committed since the last reset → skip to reset
pending ← []                          # pages of the txn being read
for each record, in order:
    if the record is truncated, has a bad type/length, or its CRC fails:
        stop                          # end of the valid log (torn tail)
    if PAGE:   pending.push(page id, image)
    if COMMIT: if count matches: write every pending page to the db file
               pending ← []
fsync the database file
reset the WAL (truncate, new salt, fsync)
```

Why this is correct:

* A transaction is applied only if its COMMIT record is intact, and the
  COMMIT record is written *after* its pages, so all its pages are intact
  too. Records are checksummed, so a torn record is detected and ends the
  scan. Transactions after a damaged record can't exist: the WAL was synced
  before each commit returned, and a commit only starts after the previous
  one finished.
* Replay is **idempotent**: writing a full page image twice equals writing
  it once. Recovery can therefore crash and be re-run any number of times,
  and it doesn't matter which of the step-3 writes made it before the crash.
* Uncommitted transactions never wrote to the database file (no-steal), so
  there is nothing to undo. This is why toydb needs only *redo*.

**The salt.** Each log generation (between resets) gets a fresh random salt
that is mixed into every record's checksum. If an old record ever sat beyond
the end of the current log (say a truncate was lost), it would fail the
checksum instead of being replayed. SQLite's WAL uses the same trick.

### Checkpointing

The WAL would grow forever, and recovery would get slower. A **checkpoint**
fsyncs the database file (making step-3 writes durable) and then truncates
the WAL and fsyncs it. After a checkpoint the database file alone holds
everything. toydb checkpoints when the log exceeds 4 MiB, on clean shutdown,
and on demand (`.checkpoint`, `Database::checkpoint`).

### Failure during commit

If any I/O fails during a commit we can't know which writes landed. Instead
of guessing, the engine is **poisoned**: every later operation returns
`Error::Poisoned`, and reopening the database runs recovery, which sorts it
out from what is actually on disk. (PostgreSQL makes the same choice after
an fsync failure: it panics and recovers.)

### Durability knob

`StorageOptions::fsync = false` skips the per-commit fsync, like SQLite's
`synchronous=OFF`: a process crash is still safe (the OS has the writes), but
an OS crash or power loss may lose recent commits or even corrupt the
database. Checkpoints always sync.

### Physical vs logical logging

toydb logs whole pages (*physical* logging). It is simple and makes replay
idempotent, but logs 4 KiB for a one-byte change. *Logical* or
*physiological* logging (ARIES; "insert key K into page P") is far more
compact but needs per-page LSNs to know whether a record was already applied,
plus undo records if pages may be stolen. Full-page images are not unusual in
practice: SQLite's WAL is exactly this, and PostgreSQL writes full page
images the first time a page is touched after each checkpoint to defend
against torn pages.

---

## 7. Transactions and locking

*Modules: `txn.rs` (lock manager), `db.rs` (sessions)*

### Locks

toydb uses **strict two-phase locking at database granularity**:

* Before a transaction's first read it takes a **shared** lock; before its
  first write, an **exclusive** lock (upgrading if it holds a shared one).
* Locks are released only at COMMIT/ROLLBACK ("strict" 2PL).

```text
                 held by others:  none     shared    exclusive
   request shared                 grant    grant     wait
   request exclusive              grant    wait      wait
```

Strict 2PL guarantees *serializability*: the result is as if transactions
ran one after another. It also gives us isolation from uncommitted data for
free: a writer's dirty pages are in the shared buffer pool, but nobody else
can read while the writer holds its exclusive lock.

**Upgrade deadlock.** Two transactions that both read (shared) and then both
try to write would wait for each other forever. The lock manager allows only
one session to *wait for an upgrade* at a time; a second one gets `Busy`
immediately and should roll back and retry (SQLite does the same). All other
waits time out (5 s by default), turning any other deadlock into an error.
**Writer preference**: while a writer waits, new readers queue behind it so
writers can't be starved.

The `bank` example shows the practical consequence: under contention a
read-then-write transaction sometimes gets `Busy` and must retry, and the
invariant (total money) always holds.

### Sessions, autocommit and savepoints

`Session::execute` does, for each statement:

```text
lock (S for SELECT/EXPLAIN, X otherwise) ── may wait / fail with Busy
lock the engine mutex (one statement at a time inside the engine)
if writing: begin statement savepoint
execute
on error:   roll back to the savepoint (just this statement)
if autocommit: commit (or roll back on error) and release locks
```

**Statement savepoints.** Inside `BEGIN ... COMMIT`, a failing statement
(say, a duplicate key on the third row of a multi-row INSERT) must not leave
half its work behind, but it also shouldn't destroy the transaction's earlier
statements. `Storage` keeps, per statement, the *pre-image* of each page the
first time the statement modifies it; on failure those images are restored.
That gives SQLite/MySQL semantics: the statement is atomic, the transaction
stays open.

**Only the writer commits.** Storage has one set of dirty pages, owned by
whichever session holds the exclusive lock. `finish()` therefore commits or
rolls back storage only if *this* session holds the exclusive lock — a
read-only session's COMMIT must never commit someone else's work.

Dropping a `Session` with an open transaction rolls it back.

### Isolation level and what's missing

Serializable, but with very coarse locks: one writer blocks all readers.
The next steps would be (a) **MVCC/snapshot isolation** — readers see a
consistent snapshot while a writer works, e.g. by keeping the WAL frames of
recent transactions readable like SQLite's WAL mode does, and (b) finer
grained (table/page/row) locks, which require intention locks and deadlock
detection over a wait-for graph.

---

## 8. Values, rows and keys

*Module: `types.rs`*

Values: `NULL`, `INTEGER` (i64), `REAL` (f64), `TEXT` (UTF-8), `BOOLEAN`.
The only implicit conversion is INTEGER → REAL when storing into a REAL
column; anything else is a type error (toydb is stricter than SQLite).

**Row encoding** (a B+tree *value*): self-describing and compact enough.

```text
[ncols u16] [tag u8][payload] [tag u8][payload] ...
  tag 0 NULL     -
  tag 1 INTEGER  i64 little-endian
  tag 2 REAL     f64 bits little-endian
  tag 3 TEXT     u32 length + UTF-8 bytes
  tag 4 BOOLEAN  u8
```

**Key encoding** (a B+tree *key*) must be **order-preserving**: comparing two
encoded keys with `memcmp` must give the same answer as comparing the values,
because the B+tree only compares bytes.

```text
INTEGER  tag 0x01, then (value XOR 0x8000_0000_0000_0000) big-endian
         -1 → 01 7F FF FF FF FF FF FF FF
          0 → 01 80 00 00 00 00 00 00 00
          1 → 01 80 00 00 00 00 00 00 01
REAL     tag 0x02, then: positive → bits with the sign bit flipped,
         negative → all bits inverted; big-endian (-0.0 normalized to 0.0)
TEXT     tag 0x03, bytes with 0x00 escaped as 00 FF, terminated by 00 00
         (so "a" < "a\0" < "ab" holds byte-wise, and keys could be
          concatenated into composite keys without ambiguity)
BOOLEAN  tag 0x04, 00 or 01
```

Flipping the sign bit makes two's-complement integers sort correctly as
unsigned bytes; big-endian puts the most significant byte first so `memcmp`
looks at it first. Property tests check `encode(a).cmp(encode(b)) == a.cmp(b)`
for random values of each type.

Tables without a primary key get a hidden **rowid** key: one more than the
largest existing key (`BTree::last`), like SQLite's rowid tables.

---

## 9. The catalog

*Module: `catalog.rs`*

Where do table definitions live? In a table, of course. `toydb_schema` is a
B+tree whose root page is recorded in the header, with rows:

```text
+-------+---------+----------+----------+------------------------------+
| type  | name    | tbl_name | rootpage | sql                          |
+-------+---------+----------+----------+------------------------------+
| table | orders  | orders   | 7        | CREATE TABLE orders (...)    |
| table | users   | users    | 2        | CREATE TABLE users (...)     |
+-------+---------+----------+----------+------------------------------+
```

The schema is stored as **canonical SQL text** — the AST printed back out —
and re-parsed when the catalog is loaded. This is exactly SQLite's
`sqlite_schema` design. It means the schema "file format" is just SQL, it is
human-readable (`SELECT sql FROM toydb_schema`), and DDL is transactional
with no extra machinery: CREATE/DROP TABLE are B+tree inserts/deletes in the
current transaction. The in-memory catalog is a cache that is invalidated
whenever a transaction or statement is rolled back.

---

## 10. The query pipeline

*Modules: `sql/lexer.rs`, `sql/parser.rs`, `sql/planner.rs`, `sql/expr.rs`,
`sql/exec.rs`*

```text
"SELECT dept, COUNT(*) AS n FROM emp WHERE id > 10 GROUP BY dept ORDER BY n DESC"
   │ lexer: characters → tokens (with byte positions for error messages)
   ▼
[SELECT] [ident dept] [,] [ident count] [(] [*] [)] [AS] [ident n] [FROM] ...
   │ parser: recursive descent → AST
   ▼
Select { items: [Column(dept), Function(count, star) AS n], from: emp,
         filter: Binary(>, Column(id), 10), group_by: [Column(dept)], ... }
   │ planner: bind names to positions, rewrite aggregates, choose access path
   ▼
Sort #1 DESC
  Project dept, n
    Aggregate group by [dept] aggregates [COUNT(*)]
      Filter id > 10
        Scan emp (range id > 10)
   │ executor: one iterator per node, rows pulled from the top
   ▼
rows
```

### Lexer and parser

The lexer is a single loop over bytes producing tokens; keywords are
recognized case-insensitively, unquoted identifiers are lower-cased, `'...'`
strings and `"..."` identifiers use doubled quotes as escapes, and `--` and
`/* */` comments are skipped.

The parser is **recursive descent**: one method per grammar rule. Expression
precedence is handled by *precedence climbing* — one method per level, each
calling the next-tighter level for its operands:

```text
or → and → not → predicate (=, <, IS NULL, LIKE, IN, BETWEEN)
   → concat (||) → additive (+ -) → term (* / %) → unary (- +) → primary
```

The grammar is documented at the top of `parser.rs`. Errors say what was
expected, what was found, and where (`expected an identifier, found , at
line 2, column 9`). Every AST node implements `Display`, which is used to
name result columns and to store canonical CREATE TABLE text; tests check
that printing and re-parsing round-trips.

### Binding

The AST refers to columns by name. The planner's `Binder` resolves them to
positions in the input row, producing `BExpr` trees that evaluate with no
lookups (`sql/expr.rs`). `BETWEEN` is desugared to `>= AND <=` here.

### Aggregation

If there is a GROUP BY or any aggregate call, the planner:

1. collects the distinct aggregate calls appearing in the SELECT list,
   HAVING and ORDER BY (nested aggregates are an error);
2. inserts an `Aggregate` operator whose output rows are
   `[group values..., aggregate results...]`;
3. re-binds everything above it against that shape: an expression equal to
   a GROUP BY expression becomes "column i", an aggregate call becomes
   "column (groups + j)", and any other bare column reference is an error
   ("must appear in the GROUP BY clause"), as in PostgreSQL.

So `HAVING COUNT(*) > 1` becomes `Filter #1 > 1` over the Aggregate's
output. The Aggregate executor keeps one set of accumulators per group in a
`BTreeMap` (so output is ordered by group), supports `DISTINCT` aggregates
with a per-group seen-set, and emits a single row for an ungrouped aggregate
over empty input (`COUNT(*)` = 0, others NULL).

### ORDER BY

An ORDER BY term may be a position (`ORDER BY 2`), an alias, an expression
from the SELECT list, or any other expression. The planner projects the
SELECT list plus *hidden* columns for terms not in it, sorts on column
indexes, and finally projects the hidden columns away. With DISTINCT, hidden
columns would change the meaning, so they're an error (as in PostgreSQL).

### Access path selection

The only index is the primary key, so the only choice is "full scan" vs
"use the B+tree". The planner splits WHERE into AND-ed conjuncts and looks
for `pk = const`, `pk < const` (either side), and `pk BETWEEN const AND
const`. Equalities become a point lookup; inequalities are intersected into
the tightest range. The full predicate is still evaluated afterwards, so the
access path only has to be a superset — which keeps this optimization simple
and obviously safe. Constants must be of the key's exact type (the key
encoding orders values within a type, so `id < 2.5` on an INTEGER key falls
back to a full scan).

### The Volcano executor

Every operator implements `next() -> Option<Row>` and pulls from its child
(Graefe's *Volcano* / iterator model):

| operator    | behavior                                                   |
|-------------|------------------------------------------------------------|
| `Scan`      | B+tree cursor over the chosen key range, decodes rows      |
| `Filter`    | passes rows whose predicate is TRUE (not FALSE or NULL)    |
| `Project`   | evaluates the SELECT list                                  |
| `Aggregate` | pipeline breaker: consumes input, emits one row per group  |
| `Distinct`  | remembers rows already emitted                             |
| `Sort`      | pipeline breaker: in-memory stable sort                    |
| `Limit`     | skips OFFSET rows, then stops pulling after LIMIT rows     |

`Limit` stopping early is the iterator model's nice property: `SELECT * FROM
t LIMIT 1` reads one leaf, not the table.

### NULL and three-valued logic

Comparisons with NULL yield NULL; `AND`/`OR` follow the SQL truth tables
(`NULL AND FALSE` is FALSE, `NULL OR TRUE` is TRUE); WHERE keeps a row only
if the predicate is TRUE. `x IN (1, NULL)` is TRUE if x = 1 and NULL
otherwise. Aggregates skip NULLs. In ORDER BY, GROUP BY and DISTINCT, values
need a *total* order, so `Value::total_cmp` sorts NULL first.

### DML

* **INSERT** evaluates each VALUES row (constants only), coerces types,
  checks NOT NULL and primary-key uniqueness, and inserts.
* **UPDATE/DELETE** first collect all matching `(key, row)` pairs through
  the same access-path logic as SELECT, *then* modify. Collect-then-modify
  avoids mutating a B+tree under an open cursor and the **Halloween
  problem** (an UPDATE that moves rows forward in the scan seeing them
  again). UPDATE computes all new rows from old values, deletes the rows
  whose key changes, then inserts — so `UPDATE t SET id = id + 1` doesn't
  collide with not-yet-updated rows, while a real duplicate still fails
  (and the statement savepoint undoes the partial work).

### EXPLAIN

`EXPLAIN` prints the operator tree, including the access path, which makes
the planner's decisions visible:

```text
toydb> EXPLAIN SELECT name FROM employees WHERE id BETWEEN 2 AND 4 ORDER BY name;
Sort name
  Project name
    Filter id BETWEEN 2 AND 4
      Scan employees (range id >= 2 AND id <= 4)
```

---

## 11. Testing strategy

| layer        | technique                                                                                     |
|--------------|-----------------------------------------------------------------------------------------------|
| VFS          | unit tests of crash semantics (`MemVfs`)                                                      |
| buffer pool  | LRU order, no eviction of dirty pages, rollback via discard                                   |
| B+tree       | property tests vs `BTreeMap`, invariant checker, page-leak detection, overflow & edge sizes   |
| WAL          | torn tail, corrupted record, stale-salt records                                               |
| storage      | randomized power failures at any I/O operation, torn writes, repeated crash/recover cycles    |
| encodings    | property tests: round trips, order preservation                                               |
| SQL          | end-to-end tests per feature; differential property test vs an in-memory model               |
| transactions | rollback/commit/savepoints/DDL rollback; multi-threaded lost-update test; deadlock detection |
| durability   | SQL-level bank-transfer invariant across power failures                                       |
| REPL         | drives the real binary through stdin                                                          |

The crash tests were themselves validated by **mutation**: disabling WAL
replay, or the fsync in the commit path, makes them fail. A test suite that
can't detect a missing fsync isn't testing durability.

---

## Benchmarks

`cargo bench --bench compare` runs a small, dependency-light harness that
performs the same workloads on toydb and on SQLite (via `rusqlite`, a
dev-dependency used only here). Both use a WAL with fsync on commit unless
noted. See `benches/compare.rs` for exactly what is measured; numbers below
are from the development container and only meaningful relative to each
other.

<!-- BENCHMARK RESULTS -->
*(Results are filled in by running the benchmark; see the "Benchmark
results" subsection below.)*

---

## 13. Decision log

Decisions made while building toydb, with the reasoning. (The project was
built in milestones: pager & buffer pool → B+tree → WAL → transactions →
SQL → REPL → benchmarks.)

| # | decision | why | alternatives considered |
|---|----------|-----|-------------------------|
| 1 | Zero runtime dependencies; `proptest` and `rusqlite` only as dev-dependencies | The point is to build everything; also keeps builds fast | `thiserror`, `parking_lot`, `rustyline` — small conveniences not worth it |
| 2 | 4 KiB pages | Matches OS/SSD pages; one page write = one device write | 8/16 KiB (fewer levels, more write amplification) |
| 3 | VFS trait with an in-memory crash-simulating implementation | Makes durability testable and deterministic | Testing only with real files and `kill -9` (can't model lost/torn writes) |
| 4 | Decode nodes into structs instead of slotted pages edited in place | Readable algorithms; borrow-checker friendly | Slotted pages (faster, much fiddlier) |
| 5 | Header is an ordinary page in the buffer pool and WAL | Allocation and catalog root changes become transactional for free | Separate in-memory metadata with its own persistence |
| 6 | No-steal buffer pool, dirty pages pinned until commit | Rollback = discard; WAL needs no undo | Steal + undo logging (ARIES): supports huge transactions, much more complex |
| 7 | Redo-only physical WAL of full page images | Idempotent replay; simple, robust recovery | Logical/physiological logging with LSNs |
| 8 | Write committed pages to the db file at commit (no fsync), fsync at checkpoint | Keeps the file current so clean pages can be dropped and rollback can re-read | Keep committed pages only in WAL until checkpoint (SQLite WAL mode; needs a WAL index for reads) |
| 9 | Per-generation salt in WAL checksums | Defends against replaying stale records | Relying on truncation alone |
| 10 | Poison the engine on I/O error during commit | Unknown on-disk state; recovery is the only trustworthy repair | Retrying writes (can silently lose data after fsync failure) |
| 11 | Database-level S/X locks, strict 2PL | Serializable with a tiny lock manager | MVCC, row locks with deadlock detection |
| 12 | Fail the second upgrader immediately with `Busy` | Breaks the common read-then-write deadlock without a wait-for graph | Timeouts only (slow), deadlock detection |
| 13 | Statement savepoints via page pre-images | Statement atomicity inside transactions with ~40 lines | Abort the whole transaction on any error (PostgreSQL-style) |
| 14 | Catalog stored as canonical CREATE TABLE text in `toydb_schema` | Self-hosted, human-readable, transactional DDL | Binary schema format |
| 15 | Single-column primary key as the B+tree key; hidden rowid otherwise | Clustered storage, simple access-path selection | Always rowid + secondary unique index |
| 16 | Order-preserving (memcomparable) key encoding | B+tree compares bytes only; enables range scans | Type-aware comparator callbacks in the tree |
| 17 | Strict typing (only INTEGER→REAL coercion), errors on cross-type comparison | Catches mistakes; simpler semantics | SQLite-style type affinity |
| 18 | PostgreSQL-style GROUP BY rule (no bare ungrouped columns) | Unambiguous results | SQLite's "any row of the group" |
| 19 | Hand-written lexer + recursive-descent parser with precedence climbing | Classic, readable, great error messages | Parser generators/combinators |
| 20 | Volcano iterators; fully materialized results in the API | Textbook model; simple API | Vectorized/push-based execution; streaming result iterator |
| 21 | Collect-then-modify for UPDATE/DELETE | Avoids cursor invalidation and the Halloween problem | Cursors robust to concurrent modification |
| 22 | REPL without line-editing library | Zero deps; works fine with `rlwrap toydb` | `rustyline` |
| 23 | Benchmarks as a plain harness (`harness = false`) | Few dependencies, readable output table | `criterion` |

---

## 14. Limitations and where to go next

Roughly in order of how much each would teach:

1. **Secondary indexes** (`CREATE INDEX`): a second B+tree mapping
   `encode(indexed value) ++ encode(pk)` → empty; maintained by DML; used by
   the planner. The key encoding was designed to concatenate for this.
2. **Joins**: nested-loop join as a new operator, then index nested-loop
   using the primary key, then hash join. The binder would need multi-table
   scopes.
3. **MVCC / snapshot isolation** so readers don't block the writer.
4. **Streaming results**: return an iterator that holds the engine lock.
5. **Slotted pages** and in-place node edits, prefix compression, suffix
   truncation of separators.
6. **Steal + undo logging** (ARIES) to support transactions larger than RAM.
7. **External merge sort** for ORDER BY on large inputs.
8. **A cost-based optimizer** once there is more than one access path.
9. Composite primary keys, UNIQUE/CHECK/DEFAULT, ALTER TABLE.
10. `VACUUM` to shrink the file.
