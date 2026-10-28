//! Deterministic load scenarios that drive the real crank cycle
//! (`cycle::run_one_cycle`) against a local validator.
//!
//! Scenario code owns everything a cranker does not: market setup, trades,
//! mark pushes, lifecycle and exits. The cranker itself is not special-cased.

pub mod chain;
pub mod live_shaped;
pub mod rng;
