//! Library surface for the percolator cranker.
//!
//! The binary (`src/main.rs`) still declares these modules itself, so this
//! `lib.rs` exists purely so integration tests under `tests/` can import the
//! same code (`use percolator_cranker::submit::submit_crank;` etc.). A
//! bin-only crate has no library target, and Rust integration tests link
//! against the library target, not the binary.
//!
//! Optional cleanup (not required for the tests to work): switch `main.rs`
//! from its own `mod submit;` declarations to `use percolator_cranker::...`
//! so each module compiles once instead of twice. Left as-is here to keep
//! this change non-destructive to the working binary.

pub mod binding;
pub mod discovery;
pub mod observation;
pub mod plan_gate;
pub mod setup;
pub mod submit;
