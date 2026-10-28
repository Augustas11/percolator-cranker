//! Resolved-mode crank hints.
//!
//! Since wrapper `5cb331dd`, a `PermissionlessCrank` (tag 5) on a Resolved
//! market may carry **at most one** hint, with `oracle_accounts == 0` and
//! `asset_index < max_market_slots` (`repo-prog/src/v16_program.rs`,
//! `handle_permissionless_crank` → `handle_close_resolved`). The hint is
//! discovery-only: the wrapper turns it into
//! `AutoCrankObservationV16 { asset_index, effective_price: 0, funding_rate_e9: 0 }`
//! and the engine either advances that asset's backing bucket (expiry) or
//! returns `EngineNonProgress` atomically. `CloseResolved` is a compatibility
//! alias for the same crank.
//!
//! So the cranker's job is only to name the asset whose backing bucket is
//! due. A wrong guess costs a fee and comes back as `EngineNonProgress`,
//! which `submit::OutcomeClass` treats as idle, never as a failure.

use percolator::{BackingBucketStatusV16, MarketModeV16};
use percolator_prog::state::MarketGroupV16;

use crate::observation::FetchedObservation;

/// Effective expiry of a backing bucket once the market is Resolved.
///
/// Engine #224 (in `68859b94`, not yet in the wrapper-main pin `4db11a8c`)
/// caps every bucket at `resolved_slot + H` with
/// `H = max(max_accrual_dt_slots, h_max, max_bankrupt_close_lifetime_slots, 1)`.
/// Taking the min is safe on both pins: on `4db11a8c` an early hint just
/// returns NonProgress.
pub fn resolved_bucket_deadline(group: &MarketGroupV16, expiry_slot: u64) -> u64 {
    let c = &group.config;
    let horizon = c
        .max_accrual_dt_slots
        .max(c.h_max)
        .max(c.max_bankrupt_close_lifetime_slots)
        .max(1);
    expiry_slot.min(group.resolved_slot.saturating_add(horizon))
}

/// The asset whose backing bucket is due to expire at `now_slot`, if any.
///
/// Domains are `asset_index * 2 + side`. Only `Fresh` buckets qualify: the
/// engine's hint path (`first_lapsed_source_backing_for_asset_at_slot`) acts
/// on lapsed Fresh buckets only. Expiry turns a liened Fresh bucket into
/// `Impaired` and keeps its old `expiry_slot`, so counting Impaired here
/// would pin the single hint on a bucket the engine has already handled and
/// starve the one that is actually due. Among due buckets the earliest
/// deadline wins, ties broken by lower asset index.
pub fn resolved_backing_hint(group: &MarketGroupV16, now_slot: u64) -> Option<u16> {
    if group.mode != MarketModeV16::Resolved {
        return None;
    }
    let slots = group.config.max_market_slots as usize;
    group
        .source_backing_buckets
        .iter()
        .enumerate()
        .filter(|(domain, _)| domain / 2 < slots)
        .filter(|(_, b)| b.status == BackingBucketStatusV16::Fresh)
        .map(|(domain, b)| (resolved_bucket_deadline(group, b.expiry_slot), domain / 2))
        .filter(|(deadline, _)| *deadline <= now_slot)
        .min()
        .and_then(|(_, asset)| u16::try_from(asset).ok())
}

/// Hints to attach to a crank on a Resolved market: zero or one
/// oracle-less observation.
pub fn resolved_crank_hints(group: &MarketGroupV16, now_slot: u64) -> Vec<FetchedObservation> {
    resolved_backing_hint(group, now_slot)
        .map(|asset_index| FetchedObservation {
            asset_index,
            oracle_accounts: Vec::new(),
        })
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use percolator::BackingBucketV16;
    use percolator_prog::ix::{CrankObservationHint, Instruction as ProgInstruction};

    fn group_with(buckets: &[(usize, BackingBucketStatusV16, u64)]) -> MarketGroupV16 {
        let mut config = percolator::V16Config::public_user_fund(4, 1, 8);
        config.max_bankrupt_close_lifetime_slots = 16;
        let mut g = MarketGroupV16::new([7u8; 32], config).expect("group");
        g.mode = MarketModeV16::Resolved;
        g.resolved_slot = 1_000;
        for &(domain, status, expiry_slot) in buckets {
            g.source_backing_buckets[domain] = BackingBucketV16 {
                market_id: 1,
                fresh_unliened_backing_num: 1,
                status,
                expiry_slot,
                ..BackingBucketV16::EMPTY
            };
        }
        g
    }

    #[test]
    fn no_hint_outside_resolved() {
        let mut g = group_with(&[(2, BackingBucketStatusV16::Fresh, 10)]);
        g.mode = MarketModeV16::Live;
        assert_eq!(resolved_backing_hint(&g, 1_000_000), None);
    }

    #[test]
    fn earliest_due_bucket_wins() {
        // asset 1 short (domain 3) due at 50, asset 2 long (domain 4) due at 40,
        // asset 0 long (domain 0) not due yet.
        let g = group_with(&[
            (0, BackingBucketStatusV16::Fresh, 5_000_000),
            (3, BackingBucketStatusV16::Fresh, 50),
            (4, BackingBucketStatusV16::Fresh, 40),
        ]);
        assert_eq!(resolved_backing_hint(&g, 60), Some(2));
        assert_eq!(resolved_backing_hint(&g, 45), Some(2));
        assert_eq!(resolved_backing_hint(&g, 39), None);
    }

    #[test]
    fn expired_and_empty_buckets_are_ignored() {
        let g = group_with(&[
            (0, BackingBucketStatusV16::Expired, 10),
            (2, BackingBucketStatusV16::Empty, 10),
        ]);
        assert_eq!(resolved_backing_hint(&g, 1_000_000), None);
    }

    /// Review finding: an Impaired bucket keeps its old (earliest) expiry.
    /// It must not shadow a Fresh bucket that is actually due.
    #[test]
    fn impaired_bucket_does_not_shadow_due_fresh_bucket() {
        let g = group_with(&[
            (1, BackingBucketStatusV16::Impaired, 10),
            (7, BackingBucketStatusV16::Fresh, 50),
        ]);
        assert_eq!(resolved_backing_hint(&g, 60), Some(3));
    }

    #[test]
    fn resolved_horizon_caps_far_expiry() {
        let g = group_with(&[(6, BackingBucketStatusV16::Fresh, u64::MAX)]);
        let deadline = resolved_bucket_deadline(&g, u64::MAX);
        assert!(deadline > g.resolved_slot && deadline < u64::MAX);
        assert_eq!(resolved_backing_hint(&g, deadline - 1), None);
        assert_eq!(resolved_backing_hint(&g, deadline), Some(3));
    }

    /// Wire: the resolved hint is an ordinary tag-5 hint with zero oracle
    /// accounts, which is exactly what the wrapper's resolved branch accepts.
    #[test]
    fn resolved_hint_encodes_as_single_oracle_less_tag5_hint() {
        let g = group_with(&[(3, BackingBucketStatusV16::Fresh, 10)]);
        let hints = resolved_crank_hints(&g, 20);
        assert_eq!(hints.len(), 1);
        let ix = ProgInstruction::PermissionlessCrank {
            now_slot: 20,
            observations: hints
                .iter()
                .map(|h| CrankObservationHint {
                    asset_index: h.asset_index,
                    oracle_accounts: h.oracle_account_count(),
                })
                .collect(),
        };
        let bytes = ix.encode();
        // tag | now_slot | n=1 | asset_index u16 = 1 | oracle_accounts = 0
        assert_eq!(bytes[0], 5);
        assert_eq!(bytes[9], 1);
        assert_eq!(u16::from_le_bytes([bytes[10], bytes[11]]), 1);
        assert_eq!(bytes[12], 0);
        assert_eq!(bytes.len(), 13);
    }
}
