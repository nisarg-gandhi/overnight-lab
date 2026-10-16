//! The public API: [`Database`] and [`Session`].
//!
//! ```no_run
//! use toydb::{Database, QueryResult};
//!
//! let db = Database::open("example.db")?;
//! let mut s = db.session();
//! s.execute("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v INTEGER)")?;
//! s.execute("INSERT INTO kv VALUES ('a', 1), ('b', 2)")?;
//! if let QueryResult::Rows { rows, .. } = s.execute("SELECT v FROM kv WHERE k = 'b'")? {
//!     assert_eq!(rows[0][0], toydb::Value::Integer(2));
//! }
//! # Ok::<(), toydb::Error>(())
//! ```
//!
//! A `Database` may be shared between threads; each thread uses its own
//! `Session` (connection). Sessions coordinate through the lock manager:
//!
//! ```text
//!  Session A ──┐  1. take S/X lock (may wait)      +--------------+
//!  Session B ──┼────────────────────────────────>  | LockManager  |
//!  Session C ──┘                                   +--------------+
//!              │  2. lock the engine mutex for     +--------------+
//!              └───────── one statement ────────>  | Engine       |
//!                                                  | (Storage +   |
//!                                                  |  Catalog)    |
//!                                                  +--------------+
//! ```
//!
//! **Transactions.** Outside `BEGIN ... COMMIT`, every statement runs in its
//! own transaction (autocommit). Inside one, locks accumulate until COMMIT
//! or ROLLBACK. If a statement inside a transaction fails, only *that
//! statement's* changes are undone (via a statement savepoint) and the
//! transaction stays open — the same behaviour as SQLite and MySQL.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::sql::ast::Statement;
use crate::sql::exec::{Engine, QueryResult};
use crate::sql::parser::{parse, parse_script};
use crate::storage::file::{OsVfs, Vfs};
use crate::storage::{Storage, StorageOptions};
use crate::txn::{LockManager, LockMode, SessionId};

/// Options for opening a database.
#[derive(Clone, Debug)]
pub struct Options {
    pub storage: StorageOptions,
    /// How long a statement waits for a lock before failing with `Busy`.
    pub lock_timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            storage: StorageOptions::default(),
            lock_timeout: Duration::from_secs(5),
        }
    }
}

struct Shared {
    engine: Mutex<Engine>,
    locks: LockManager,
    next_session: AtomicU64,
    lock_timeout: Duration,
}

impl Shared {
    fn engine(&self) -> MutexGuard<'_, Engine> {
        // A panic while holding the lock leaves the guard poisoned. The
        // storage layer has its own poisoning for I/O failures, so we can
        // safely keep going here.
        self.engine.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// An open database. Cheap to clone; all clones share the same engine.
#[derive(Clone)]
pub struct Database {
    shared: Arc<Shared>,
}

impl Database {
    /// Open (creating if necessary) a database file with default options.
    pub fn open(path: impl AsRef<Path>) -> Result<Database> {
        Self::open_with(path, Options::default())
    }

    pub fn open_with(path: impl AsRef<Path>, opts: Options) -> Result<Database> {
        Self::open_vfs(&OsVfs, path, opts)
    }

    /// Open on an arbitrary file system (e.g. [`MemVfs`] for tests).
    ///
    /// [`MemVfs`]: crate::storage::file::MemVfs
    pub fn open_vfs(vfs: &dyn Vfs, path: impl AsRef<Path>, opts: Options) -> Result<Database> {
        let storage = Storage::open(vfs, path.as_ref(), opts.storage)?;
        Ok(Database {
            shared: Arc::new(Shared {
                engine: Mutex::new(Engine::new(storage)?),
                locks: LockManager::new(),
                next_session: AtomicU64::new(1),
                lock_timeout: opts.lock_timeout,
            }),
        })
    }

    /// Open a new session (connection).
    pub fn session(&self) -> Session {
        Session {
            shared: self.shared.clone(),
            id: self.shared.next_session.fetch_add(1, Ordering::Relaxed),
            in_txn: false,
        }
    }

    /// Convenience: run one statement in a fresh autocommit session.
    pub fn execute(&self, sql: &str) -> Result<QueryResult> {
        self.session().execute(sql)
    }

    /// Force a checkpoint: flush the database file and truncate the WAL.
    pub fn checkpoint(&self) -> Result<()> {
        self.shared.engine().storage.checkpoint()
    }

    /// Engine statistics, for the REPL's `.stats` and benchmarks.
    pub fn stats(&self) -> Stats {
        let mut e = self.shared.engine();
        let pool = e.storage.pool_stats();
        Stats {
            page_count: e.storage.page_count().unwrap_or(0),
            free_pages: e.storage.free_count().unwrap_or(0),
            wal_bytes: e.storage.wal_size(),
            cache_hits: pool.hits,
            cache_misses: pool.misses,
            evictions: pool.evictions,
        }
    }

    /// See [`Session::check_integrity`].
    pub fn check_integrity(&self) -> Result<Vec<String>> {
        self.session().check_integrity()
    }

    /// Names of all user tables, read through a fresh session (so it takes
    /// a shared lock and never sees another session's uncommitted DDL).
    pub fn table_names(&self) -> Result<Vec<String>> {
        self.session().table_names()
    }
}

/// Counters reported by [`Database::stats`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub page_count: u32,
    pub free_pages: u32,
    pub wal_bytes: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub evictions: u64,
}

/// A connection to a [`Database`], with its own transaction state.
pub struct Session {
    shared: Arc<Shared>,
    id: SessionId,
    in_txn: bool,
}

impl Session {
    /// Is an explicit transaction (BEGIN ...) open?
    pub fn in_transaction(&self) -> bool {
        self.in_txn
    }

    /// Check the database's internal consistency (B+tree invariants, and
    /// that every index has exactly the right entry for every row). Returns
    /// the problems found; an empty list means all is well.
    pub fn check_integrity(&mut self) -> Result<Vec<String>> {
        self.shared
            .locks
            .acquire(self.id, LockMode::Shared, self.shared.lock_timeout)?;
        let r = self.shared.engine().check_integrity();
        if !self.in_txn {
            self.shared.locks.release(self.id);
        }
        r
    }

    /// Names of all user tables, as seen by this session.
    pub fn table_names(&mut self) -> Result<Vec<String>> {
        match self.execute("SELECT name FROM toydb_schema WHERE type = 'table' ORDER BY name")? {
            QueryResult::Rows { rows, .. } => Ok(rows
                .into_iter()
                .map(|mut r| match r.remove(0) {
                    crate::Value::Text(t) => t,
                    v => v.to_string(),
                })
                .collect()),
            _ => unreachable!("SELECT returns rows"),
        }
    }

    /// Execute a single SQL statement.
    pub fn execute(&mut self, sql: &str) -> Result<QueryResult> {
        let stmt = parse(sql)?;
        self.execute_statement(&stmt)
    }

    /// Execute a `;`-separated script, stopping at the first error.
    pub fn execute_script(&mut self, sql: &str) -> Result<Vec<QueryResult>> {
        parse_script(sql)?
            .iter()
            .map(|s| self.execute_statement(s))
            .collect()
    }

    /// Execute an already-parsed statement.
    pub fn execute_statement(&mut self, stmt: &Statement) -> Result<QueryResult> {
        match stmt {
            Statement::Begin => {
                if self.in_txn {
                    return Err(Error::Txn("a transaction is already open".into()));
                }
                self.in_txn = true;
                Ok(QueryResult::Done("BEGIN".into()))
            }
            Statement::Commit => {
                if !self.in_txn {
                    return Err(Error::Txn("COMMIT without BEGIN".into()));
                }
                self.in_txn = false;
                self.finish(true)?;
                Ok(QueryResult::Done("COMMIT".into()))
            }
            Statement::Rollback => {
                if !self.in_txn {
                    return Err(Error::Txn("ROLLBACK without BEGIN".into()));
                }
                self.in_txn = false;
                self.finish(false)?;
                Ok(QueryResult::Done("ROLLBACK".into()))
            }
            stmt => self.run(stmt),
        }
    }

    fn run(&mut self, stmt: &Statement) -> Result<QueryResult> {
        let writes = !matches!(stmt, Statement::Select(_) | Statement::Explain(_));
        let mode = if writes { LockMode::Exclusive } else { LockMode::Shared };
        // Lock *before* touching the engine, so waiting doesn't block anyone.
        if let Err(e) = self.shared.locks.acquire(self.id, mode, self.shared.lock_timeout) {
            if !self.in_txn {
                self.shared.locks.release(self.id);
            }
            return Err(e);
        }
        let result = {
            let mut engine = self.shared.engine();
            if writes {
                engine.storage.begin_statement();
            }
            let r = engine.execute(stmt);
            if writes {
                match &r {
                    Ok(_) => engine.storage.end_statement(),
                    Err(_) => {
                        // Undo just this statement; an enclosing explicit
                        // transaction stays open with its earlier work.
                        let _ = engine.storage.rollback_statement();
                        engine.invalidate_catalog();
                    }
                }
            }
            r
        };
        if self.in_txn {
            return result;
        }
        // Autocommit: this statement was the whole transaction.
        let commit_ok = result.is_ok();
        let finished = self.finish(commit_ok);
        let r = result?;
        finished?;
        Ok(r)
    }

    /// End the current transaction: commit or roll back the storage (only
    /// if we hold the write lock — otherwise there is nothing of ours to
    /// commit) and release our locks.
    fn finish(&mut self, commit: bool) -> Result<()> {
        let mut result = Ok(());
        if self.shared.locks.held(self.id) == Some(LockMode::Exclusive) {
            let mut engine = self.shared.engine();
            if commit {
                result = engine.storage.commit();
            }
            if !commit || result.is_err() {
                engine.storage.rollback();
                engine.invalidate_catalog();
            }
        }
        self.shared.locks.release(self.id);
        result
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.in_txn {
            self.in_txn = false;
            let _ = self.finish(false);
        }
    }
}
