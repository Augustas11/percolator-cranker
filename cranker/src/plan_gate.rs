//! Plan-classification gate for the cranker submission loop.
//!
//! Delegates to the upstream free function
//! `percolator::auto_crank_plan_requires_caller_observation`
//! (`repo/src/v16.rs:187`, re-exported in `repo/src/lib.rs:54`).
//!
//! **Why we don't hand-roll the match:** upstream's predicate is the only
//! place the engine's truth-table is pinned by proof. A local copy silently
//! rots when a new `AutoCrankPlanV16` variant lands.

use percolator::{auto_crank_plan_requires_caller_observation, AutoCrankPlanV16};

/// `true`  → cranker MUST fetch a fresh authenticated observation
///           (oracle accounts in the tx tail) before submitting.
///           Only `AutoCrankPlanV16::RefreshAccount { asset_index: None }`
///           lives in this bucket today.
/// `false` → engine synthesizes an observation from committed
///           `asset_state` via `obs_or_current_asset` (funding_rate_e9: 0).
///           Submission may proceed without an oracle tail, but the call
///           **WILL** mutate state on retry — NOT idempotent.
#[inline]
pub fn needs_observation(plan: &AutoCrankPlanV16) -> bool {
    auto_crank_plan_requires_caller_observation(plan)
}

/// `true`  → safe to blindly resubmit (only `RefreshAccount{None}` is
///           idempotent-clean in the absent-observation, NonProgress-no-mutation
///           sense).
/// `false` → state-mutating progress on retry; the cranker MUST re-read the
///           target portfolio post-state and re-decide before resubmitting.
#[inline]
#[allow(dead_code)] // callable helper reserved for M2.f pre-retry decisions;
                    // kept alongside `needs_observation` so the intent reads clearly.
pub fn blind_retry_is_safe(plan: &AutoCrankPlanV16) -> bool {
    // Identical to needs_observation today because the no-observation =
    // NonProgress invariant only survives for one variant. Keeping a
    // separate accessor so call sites read at the right altitude.
    auto_crank_plan_requires_caller_observation(plan)
}
