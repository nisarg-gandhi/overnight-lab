//! # toydb
//!
//! A small but real embedded SQL database, written from scratch for
//! learning. See `README.md` for a tour and `DESIGN.md` for how every layer
//! works.
//!
//! Layers, bottom to top:
//!
//! | module          | role                                                      |
//! |-----------------|-----------------------------------------------------------|
//! | [`storage`]     | pager, LRU buffer pool, write-ahead log, crash recovery   |
//! | [`btree`]       | B+tree over pages: the only index structure               |
//! | [`txn`]         | shared/exclusive locks (strict two-phase locking)         |
//! | [`types`]       | values, row encoding, order-preserving key encoding       |
//! | [`catalog`]     | table definitions, stored in a table of their own         |
//! | [`sql`]         | lexer, parser, planner, Volcano executor                  |
//! | [`db`]          | `Database` and `Session`: the public API                  |

pub mod btree;
pub mod catalog;
pub mod db;
pub mod error;
pub mod format;
pub mod sql;
pub mod storage;
pub mod txn;
pub mod types;
pub mod util;

pub use db::{Database, Options, Session, Stats};
pub use error::{Error, Result};
pub use sql::exec::QueryResult;
pub use types::{DataType, Row, Value};
