//! # toydb
//!
//! A small but real embedded SQL database, written from scratch for
//! learning. See `README.md` for a tour and `DESIGN.md` for how every layer
//! works.

pub mod btree;
pub mod error;
pub mod storage;
pub mod util;

pub use error::{Error, Result};
