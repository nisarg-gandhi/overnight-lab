//! The SQL front end and executor.
//!
//! ```text
//!  SQL text ──> lexer ──> tokens ──> parser ──> AST ──> planner ──> Plan
//!                                                                    │
//!          rows <── executor (Volcano iterators over the B+trees) <──┘
//! ```

pub mod ast;
pub mod exec;
pub mod expr;
pub mod lexer;
pub mod parser;
pub mod planner;
