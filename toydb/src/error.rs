//! The single error type used throughout toydb.
//!
//! Every layer (storage, B+tree, SQL front end, executor) reports failures
//! through [`Error`]. Keeping one enum makes `?` work everywhere and lets the
//! REPL print a useful category prefix for each failure.

use std::fmt;
use std::io;

/// Convenience alias used by every fallible function in the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// All the ways a toydb operation can fail.
#[derive(Debug)]
pub enum Error {
    /// An operating-system level I/O failure (disk full, permission, ...).
    Io(io::Error),
    /// On-disk data did not have the shape we expected (bad magic number,
    /// impossible node header, checksum mismatch outside the WAL tail, ...).
    Corrupt(String),
    /// The SQL text could not be tokenized or parsed.
    Parse(String),
    /// The SQL was well-formed but meaningless: unknown table or column,
    /// wrong number of values, type mismatch, ...
    Query(String),
    /// A constraint was violated (duplicate primary key, NOT NULL, ...).
    Constraint(String),
    /// A lock could not be acquired before the timeout expired.
    Busy(String),
    /// Misuse of transactions, e.g. `COMMIT` without `BEGIN`.
    Txn(String),
    /// A hard limit was exceeded (key or row too large, ...).
    Limit(String),
    /// A previous I/O failure during commit left the in-memory state
    /// untrustworthy. The database must be closed and reopened, which runs
    /// crash recovery.
    Poisoned,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Corrupt(m) => write!(f, "database corrupt: {m}"),
            Error::Parse(m) => write!(f, "parse error: {m}"),
            Error::Query(m) => write!(f, "query error: {m}"),
            Error::Constraint(m) => write!(f, "constraint violation: {m}"),
            Error::Busy(m) => write!(f, "database is locked: {m}"),
            Error::Txn(m) => write!(f, "transaction error: {m}"),
            Error::Limit(m) => write!(f, "limit exceeded: {m}"),
            Error::Poisoned => write!(
                f,
                "database poisoned by an earlier I/O failure; reopen it to run recovery"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// Shorthand constructors, used like `return Err(query_err!("no such table {t}"))`.
#[allow(unused_macros)]
macro_rules! corrupt {
    ($($arg:tt)*) => { $crate::error::Error::Corrupt(format!($($arg)*)) };
}
#[allow(unused_macros)]
macro_rules! query_err {
    ($($arg:tt)*) => { $crate::error::Error::Query(format!($($arg)*)) };
}
#[allow(unused_imports)]
pub(crate) use corrupt;
#[allow(unused_imports)]
pub(crate) use query_err;
