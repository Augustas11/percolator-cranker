//! Off-chain urgency and conservative quiet-refresh cadence. The engine
//! remains the authority on what a submitted crank does.
//!
//! Funding (engine #227, spec "Funding accrual and F settlement carry
//! sub-unit value"): F settlement pays `floor(|basis| · (F_target − f_snap) /
//! (a_basis · POS_SCALE))` and leaves a remainder worth less than one atom.
//! A leg in that state is **F-settled**. Counting it as pending work would
//! make the cranker loop on an account the engine considers done, so
//! `leg_f_settled` is the only F test used here. There is no standalone
//! funding apply/settle entry point upstream; funding settles inside refresh.
//!
//! Caveat: the sub-atom rule is engine #227, which is in `68859b94` (preview)
//! but not in the default pin `4db11a8c`, where some close paths still need
//! `f_snap == F_target` exactly. That only matters for ordering here: quiet
//! accounts are still periodically refreshed.

use percolator::{PortfolioAccountV16Account, PortfolioLegV16Account, POS_SCALE};
use percolator_prog::state::MarketGroupV16;

/// Scheduling priority, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Certified liquidation deficit or liquidation lock held.
    Liquidatable = 0,
    /// Stale / B-stale flags, or a leg whose K/F snapshot trails its asset.
    Pending = 1,
    /// Nothing visible off-chain; still probed if budget allows.
    Quiet = 2,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Priority::Liquidatable => "liquidatable",
            Priority::Pending => "pending",
            Priority::Quiet => "quiet",
        }
    }
}

/// `true` when a leg's unpaid funding remainder is worth less than one atom.
///
/// Overflow handling is exact, not a guess:
/// * the gap (`f_target − f_snap`) or the numerator `|basis| · gap`
///   overflowing means the payment is huge → **not settled**;
/// * the denominator `a_basis · POS_SCALE` overflowing means it exceeds any
///   u128 numerator, so the floor is exactly 0 → **settled**.
///
/// Only a non-negative gap can floor to zero: the engine floors F deltas
/// toward −∞, so any negative gap is at least a one-atom debit and is work.
pub fn leg_f_settled(basis_pos_q: i128, a_basis: u128, f_snap: i128, f_target: i128) -> bool {
    if basis_pos_q == 0 || a_basis == 0 {
        return true;
    }
    let Some(gap) = f_target.checked_sub(f_snap) else {
        return false;
    };
    if gap == 0 {
        return true;
    }
    if gap < 0 {
        return false;
    }
    let Some(num) = basis_pos_q.unsigned_abs().checked_mul(gap as u128) else {
        return false;
    };
    // Overflowing denominator: the true value exceeds u128::MAX >= num, so
    // floor(num / den) is exactly 0. Settled is the exact answer, not a guess.
    let Some(den) = a_basis.checked_mul(POS_SCALE) else {
        return true;
    };
    num / den == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortfolioWork {
    pub priority: Priority,
    pub structural: bool,
    /// Active legs with K/epoch drift or ≥ 1 atom of unpaid funding.
    pub pending_legs: usize,
    /// Active legs whose only gap is a sub-atom F remainder (not work).
    pub f_subatom_legs: usize,
}

fn leg_pending(leg: &PortfolioLegV16Account, group: &MarketGroupV16) -> (bool, bool) {
    let Some(asset) = group.assets.get(leg.asset_index.get() as usize) else {
        return (true, false);
    };
    let long = leg.side == 0;
    let (k, f, kf_epoch) = if long {
        (asset.k_long, asset.f_long_num, asset.kf_epoch_long)
    } else {
        (asset.k_short, asset.f_short_num, asset.kf_epoch_short)
    };
    if leg.stale != 0 || leg.b_stale != 0 || leg.kf_epoch_snap.get() != kf_epoch {
        return (true, false);
    }
    let f_settled = leg_f_settled(leg.basis_pos_q.get(), leg.a_basis.get(), leg.f_snap.get(), f);
    let k_settled = leg.k_snap.get() == k;
    let subatom_only = k_settled && f_settled && leg.f_snap.get() != f;
    (!(k_settled && f_settled), subatom_only)
}

pub fn assess(portfolio: &PortfolioAccountV16Account, group: &MarketGroupV16) -> PortfolioWork {
    let mut structural = portfolio.stale_state != 0 || portfolio.b_stale_state != 0;
    let mut pending_legs = 0;
    let mut f_subatom_legs = 0;
    for leg in portfolio.legs.iter().filter(|l| l.active != 0) {
        structural |= group
            .assets
            .get(leg.asset_index.get() as usize)
            .is_none_or(|asset| {
                let (b, epoch) = if leg.side == 0 {
                    (asset.b_long_num, asset.epoch_long)
                } else {
                    (asset.b_short_num, asset.epoch_short)
                };
                leg.stale != 0
                    || leg.b_stale != 0
                    || leg.b_snap.get() != b
                    || leg.epoch_snap.get() != epoch
                    || leg.b_epoch_snap.get() != epoch
            });
        let (pending, subatom) = leg_pending(leg, group);
        pending_legs += pending as usize;
        f_subatom_legs += subatom as usize;
    }
    let priority = if portfolio.liquidation_lock != 0
        || portfolio.health_cert.certified_liq_deficit.get() != 0
    {
        Priority::Liquidatable
    } else if portfolio.stale_state != 0 || portfolio.b_stale_state != 0 || pending_legs > 0 {
        Priority::Pending
    } else {
        Priority::Quiet
    };
    PortfolioWork {
        priority,
        structural,
        pending_legs,
        f_subatom_legs,
    }
}

/// Prices are indexed by asset, from the market snapshot used for landed progress.
#[derive(Debug, Clone)]
pub struct RefreshRecord {
    pub at: std::time::Instant,
    pub slot: u64,
    pub prices: Vec<(u64, u64)>,
}

impl RefreshRecord {
    pub fn new(at: std::time::Instant, slot: u64, group: &MarketGroupV16) -> Self {
        Self {
            at,
            slot,
            prices: group
                .assets
                .iter()
                .map(|a| (a.raw_oracle_target_price, a.effective_price))
                .collect(),
        }
    }
}

pub fn refresh_due(
    portfolio: &PortfolioAccountV16Account,
    group: &MarketGroupV16,
    work: PortfolioWork,
    previous: Option<&RefreshRecord>,
    now: std::time::Instant,
    now_slot: u64,
    quiet_refresh_secs: u64,
) -> bool {
    if group.mode != percolator::MarketModeV16::Live
        || quiet_refresh_secs == 0
        || work.priority == Priority::Liquidatable
        || work.structural
    {
        return true;
    }
    let Some(previous) = previous else {
        return true;
    };
    if now.saturating_duration_since(previous.at).as_secs() >= quiet_refresh_secs {
        return true;
    }
    if (group.config.backing_fee_base_rate_e9_per_slot != 0
        || group.config.backing_fee_slope_at_kink_e9_per_slot != 0
        || group.config.backing_fee_slope_above_kink_e9_per_slot != 0)
        && portfolio.source_domains.iter().any(|source| {
            source.source_lien_counterparty_backing_num.get() != 0
        })
    {
        return true;
    }
    if group.source_backing_buckets.iter().any(|bucket| {
        bucket.expiry_slot > previous.slot && bucket.expiry_slot <= now_slot
    }) {
        return true;
    }
    if !portfolio.legs.iter().any(|leg| leg.active != 0) {
        return false;
    }
    let cert = &portfolio.health_cert;
    // Oracle/funding epochs advance on ordinary accrual; risk/asset-set epochs do not.
    if cert.cert_risk_epoch.get() != group.risk_epoch
        || cert.cert_asset_set_epoch.get() != group.asset_set_epoch
        || cert.active_bitmap_at_cert.map(|word| word.get())
            != portfolio.active_bitmap.map(|word| word.get())
    {
        return true;
    }
    let mm = cert.certified_maintenance_req.get();
    let Some(headroom) = i128::try_from(mm)
        .ok()
        .and_then(|mm| cert.certified_equity.get().checked_sub(mm))
    else {
        return true;
    };
    let bps = group.config.maintenance_margin_bps;
    if cert.valid == 0 || headroom <= 0 || mm == 0 || bps == 0 {
        return true;
    }
    let Some(notional) = mm
        .checked_mul(10_000)
        .and_then(|n| n.checked_add(u128::from(bps) - 1))
        .map(|n| n / u128::from(bps))
    else {
        return true;
    };
    let mut movement = 0.0f64;
    let mut drift_atoms = 0u128;
    for leg in portfolio.legs.iter().filter(|l| l.active != 0) {
        let index = leg.asset_index.get() as usize;
        let (Some(asset), Some(&(raw, effective))) =
            (group.assets.get(index), previous.prices.get(index))
        else {
            return true;
        };
        let (k, f) = if leg.side == 0 {
            (asset.k_long, asset.f_long_num)
        } else {
            (asset.k_short, asset.f_short_num)
        };
        if leg.basis_pos_q.get() != 0 {
            let Some(den) = leg
                .a_basis
                .get()
                .checked_mul(POS_SCALE)
                .filter(|&den| den != 0)
            else {
                return true;
            };
            for (target, snap) in [(k, leg.k_snap.get()), (f, leg.f_snap.get())] {
                let Some(atoms) = target
                    .checked_sub(snap)
                    .and_then(|gap| {
                        leg.basis_pos_q
                            .get()
                            .unsigned_abs()
                            .checked_mul(gap.unsigned_abs())
                    })
                    .and_then(|num| (num / den).checked_add((num % den != 0) as u128))
                else {
                    return true;
                };
                let Some(total) = drift_atoms.checked_add(atoms) else {
                    return true;
                };
                drift_atoms = total;
            }
        }
        for (before, after) in [
            (raw, asset.raw_oracle_target_price),
            (effective, asset.effective_price),
        ] {
            if before == 0 || after == 0 {
                return true;
            }
            movement = movement.max(before.abs_diff(after) as f64 / before as f64);
        }
    }
    let Some(drift_atoms) = drift_atoms.checked_mul(2) else {
        return true;
    };
    let consumed = 2.0 * movement * notional as f64 + drift_atoms as f64;
    !consumed.is_finite() || consumed >= headroom as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet_fixture() -> (
        PortfolioAccountV16Account,
        MarketGroupV16,
        RefreshRecord,
        std::time::Instant,
    ) {
        let mut group =
            MarketGroupV16::new([7; 32], percolator::V16Config::public_user_fund(4, 1, 8)).unwrap();
        group.config.maintenance_margin_bps = 1000;
        group.assets[0].raw_oracle_target_price = 100_000;
        group.assets[0].effective_price = 100_000;
        let mut pf: PortfolioAccountV16Account = bytemuck::Zeroable::zeroed();
        pf.legs[0].active = 1;
        pf.legs[0].a_basis = percolator::V16PodU128::new(1);
        pf.active_bitmap[0] = percolator::V16PodU64::new(1);
        pf.health_cert.valid = 1;
        pf.health_cert.cert_risk_epoch = percolator::V16PodU64::new(group.risk_epoch);
        pf.health_cert.cert_asset_set_epoch = percolator::V16PodU64::new(group.asset_set_epoch);
        pf.health_cert.active_bitmap_at_cert = pf.active_bitmap;
        pf.health_cert.certified_equity = percolator::V16PodI128::new(300);
        pf.health_cert.certified_maintenance_req = percolator::V16PodU128::new(100);
        let now = std::time::Instant::now();
        let record = RefreshRecord::new(now - std::time::Duration::from_secs(10), 90, &group);
        (pf, group, record, now)
    }

    #[test]
    fn quiet_refresh_backing_fees_require_a_lien() {
        let (mut pf, mut group, record, now) = quiet_fixture();
        pf.legs[0].active = 0;
        for rate in 0..3 {
            group.config.backing_fee_base_rate_e9_per_slot = u64::from(rate == 0);
            group.config.backing_fee_slope_at_kink_e9_per_slot = u64::from(rate == 1);
            group.config.backing_fee_slope_above_kink_e9_per_slot = u64::from(rate == 2);
            pf.source_domains[0].source_lien_counterparty_backing_num = percolator::V16PodU128::new(0);
            assert!(!refresh_due(&pf, &group, assess(&pf, &group), Some(&record), now, 100, 300));
            pf.source_domains[0].source_lien_counterparty_backing_num = percolator::V16PodU128::new(1);
            assert!(refresh_due(&pf, &group, assess(&pf, &group), Some(&record), now, 100, 300));
        }
        group.config.backing_fee_slope_above_kink_e9_per_slot = 0;
        assert!(!refresh_due(&pf, &group, assess(&pf, &group), Some(&record), now, 100, 300));
    }

    #[test]
    fn quiet_refresh_backing_expiry_is_due_once() {
        let (mut pf, mut group, record, now) = quiet_fixture();
        pf.legs[0].active = 0;
        for expiry in [0, 89, 90, 91, 100, 101] {
            group.source_backing_buckets[0].expiry_slot = expiry;
            assert_eq!(
                refresh_due(&pf, &group, assess(&pf, &group), Some(&record), now, 100, 300),
                expiry > record.slot && expiry <= 100,
                "expiry={expiry}",
            );
        }
        group.source_backing_buckets[0].expiry_slot = 100;
        let record = RefreshRecord::new(now, 100, &group);
        assert!(!refresh_due(&pf, &group, assess(&pf, &group), Some(&record), now, 101, 300));
    }

    #[test]
    fn quiet_refresh_rounds_fractional_k_and_f_drift_up() {
        let (mut pf, mut group, record, now) = quiet_fixture();
        pf.health_cert.certified_equity = percolator::V16PodI128::new(101);
        pf.legs[0].basis_pos_q = percolator::V16PodI128::new(POS_SCALE as i128 / 2);
        assert!(!refresh_due(&pf, &group, assess(&pf, &group), Some(&record), now, 100, 300));
        group.assets[0].k_long = -1;
        group.assets[0].f_long_num = -1;
        assert!(refresh_due(&pf, &group, assess(&pf, &group), Some(&record), now, 100, 300));
    }

    #[test]
    fn quiet_refresh_preserves_immediate_liquidation() {
        let (mut pf, mut group, record, now) = quiet_fixture();
        let due = |pf: &PortfolioAccountV16Account, group: &MarketGroupV16, now, interval| {
            refresh_due(pf, group, assess(pf, group), Some(&record), now, 100, interval)
        };
        assert_eq!(assess(&pf, &group).priority, Priority::Quiet);
        assert!(!due(&pf, &group, now, 300));
        pf.health_cert.certified_liq_deficit = percolator::V16PodU128::new(1);
        assert_eq!(assess(&pf, &group).priority, Priority::Liquidatable);
        assert!(due(&pf, &group, now, 300));
        pf.health_cert.certified_liq_deficit = percolator::V16PodU128::new(0);
        group.assets[0].raw_oracle_target_price = 110_000;
        assert!(due(&pf, &group, now, 300));
        group.assets[0].raw_oracle_target_price = 100_000;
        group.assets[0].effective_price = 90_000;
        assert!(due(&pf, &group, now, 300));
        group.assets[0].effective_price = 100_000;
        assert!(due(
            &pf,
            &group,
            now + std::time::Duration::from_secs(290),
            300
        ));
        pf.stale_state = 1;
        assert!(due(&pf, &group, now, 300));
        pf.stale_state = 0;
        assert!(due(&pf, &group, now, 0));
    }

    #[test]
    fn quiet_refresh_fails_safe_and_distinguishes_drift() {
        let (mut pf, mut group, mut record, now) = quiet_fixture();
        group.assets[0].k_long += 1;
        group.assets[0].kf_epoch_long += 1;
        let work = assess(&pf, &group);
        assert_eq!(work.priority, Priority::Pending);
        assert!(!work.structural);
        assert!(!refresh_due(&pf, &group, work, Some(&record), now, 100, 300));
        assert!(refresh_due(&pf, &group, work, None, now, 100, 300));
        for (valid, equity, mm, bps) in [
            (0, 300, 100, 1000),
            (1, 100, 100, 1000),
            (1, 300, 0, 1000),
            (1, 300, 100, 0),
            (1, 300, u128::MAX, 1000),
        ] {
            pf.health_cert.valid = valid;
            pf.health_cert.certified_equity = percolator::V16PodI128::new(equity);
            pf.health_cert.certified_maintenance_req = percolator::V16PodU128::new(mm);
            group.config.maintenance_margin_bps = bps;
            assert!(refresh_due(
                &pf,
                &group,
                assess(&pf, &group),
                Some(&record),
                now,
                100,
                300
            ));
        }
        pf.legs[0].active = 0;
        assert!(!refresh_due(
            &pf,
            &group,
            assess(&pf, &group),
            Some(&record),
            now,
            100,
            300
        ));
        pf.legs[0].active = 1;
        pf.health_cert.valid = 1;
        pf.health_cert.certified_equity = percolator::V16PodI128::new(300);
        pf.health_cert.certified_maintenance_req = percolator::V16PodU128::new(100);
        group.config.maintenance_margin_bps = 1000;
        record.prices.clear();
        assert!(refresh_due(
            &pf,
            &group,
            assess(&pf, &group),
            Some(&record),
            now,
            100,
            300
        ));
        record = RefreshRecord::new(now, 100, &group);
        for mode in [
            percolator::MarketModeV16::Recovery,
            percolator::MarketModeV16::Resolved,
        ] {
            group.mode = mode;
            assert!(refresh_due(
                &pf,
                &group,
                assess(&pf, &group),
                Some(&record),
                now,
                100,
                300
            ));
        }
    }

    #[test]
    fn quiet_refresh_structural_side_snapshots() {
        for side in [0, 1] {
            for field in 0..3 {
                let (mut pf, mut group, record, now) = quiet_fixture();
                pf.legs[0].side = side;
                let asset = &mut group.assets[0];
                if field == 0 {
                    if side == 0 {
                        asset.b_long_num += 1;
                    } else {
                        asset.b_short_num += 1;
                    }
                } else if side == 0 {
                    asset.epoch_long += 1;
                } else {
                    asset.epoch_short += 1;
                }
                // Exercise each epoch snapshot independently.
                if field == 1 {
                    pf.legs[0].epoch_snap = percolator::V16PodU64::new(1);
                }
                if field == 2 {
                    pf.legs[0].b_epoch_snap = percolator::V16PodU64::new(1);
                }
                let work = assess(&pf, &group);
                assert!(work.structural, "side={side}, field={field}");
                assert!(refresh_due(&pf, &group, work, Some(&record), now, 100, 300));
            }
        }
    }

    #[test]
    fn quiet_refresh_requires_current_risk_asset_set_and_bitmap() {
        let (mut pf, mut group, record, now) = quiet_fixture();
        group.oracle_epoch += 1;
        group.funding_epoch += 1;
        assert!(!refresh_due(
            &pf,
            &group,
            assess(&pf, &group),
            Some(&record),
            now,
            100,
            300
        ));
        group.risk_epoch += 1;
        assert!(refresh_due(
            &pf,
            &group,
            assess(&pf, &group),
            Some(&record),
            now,
            100,
            300
        ));
        pf.health_cert.cert_risk_epoch = percolator::V16PodU64::new(group.risk_epoch);
        group.asset_set_epoch += 1;
        assert!(refresh_due(
            &pf,
            &group,
            assess(&pf, &group),
            Some(&record),
            now,
            100,
            300
        ));
        pf.health_cert.cert_asset_set_epoch = percolator::V16PodU64::new(group.asset_set_epoch);
        pf.active_bitmap[0] = percolator::V16PodU64::new(3);
        assert!(refresh_due(
            &pf,
            &group,
            assess(&pf, &group),
            Some(&record),
            now,
            100,
            300
        ));
    }

    #[test]
    fn quiet_refresh_counts_k_and_f_drift_as_adverse_without_price_moves() {
        for side in [0, 1] {
            for sign in [-1, 1] {
                let (mut pf, mut group, record, now) = quiet_fixture();
                pf.legs[0].side = side;
                pf.legs[0].basis_pos_q = percolator::V16PodI128::new(if side == 0 {
                    POS_SCALE as i128
                } else {
                    -(POS_SCALE as i128)
                });
                let asset = &mut group.assets[0];
                if side == 0 {
                    asset.f_long_num = sign * 99;
                    asset.kf_epoch_long += 1;
                } else {
                    asset.f_short_num = sign * 99;
                    asset.kf_epoch_short += 1;
                }
                let work = assess(&pf, &group);
                assert!(!work.structural);
                assert!(!refresh_due(&pf, &group, work, Some(&record), now, 100, 300));
                // F alone consumes half the 200-atom headroom before the 2x buffer.
                if side == 0 {
                    group.assets[0].f_long_num = sign * 100;
                } else {
                    group.assets[0].f_short_num = sign * 100;
                }
                assert!(refresh_due(
                    &pf,
                    &group,
                    assess(&pf, &group),
                    Some(&record),
                    now,
                    100,
                    300
                ));
                if side == 0 {
                    group.assets[0].f_long_num = sign * 50;
                    group.assets[0].k_long = sign * 50;
                } else {
                    group.assets[0].f_short_num = sign * 50;
                    group.assets[0].k_short = sign * 50;
                }
                assert!(refresh_due(
                    &pf,
                    &group,
                    assess(&pf, &group),
                    Some(&record),
                    now,
                    100,
                    300
                ));
            }
        }
    }

    #[test]
    fn quiet_refresh_drift_overflow_is_due() {
        let (mut pf, mut group, record, now) = quiet_fixture();
        pf.legs[0].basis_pos_q = percolator::V16PodI128::new(i128::MAX);
        group.assets[0].f_long_num = 3;
        assert!(refresh_due(
            &pf,
            &group,
            assess(&pf, &group),
            Some(&record),
            now,
            100,
            300
        ));
        pf.legs[0].basis_pos_q = percolator::V16PodI128::new(1);
        pf.legs[0].f_snap = percolator::V16PodI128::new(i128::MIN);
        assert!(refresh_due(
            &pf,
            &group,
            assess(&pf, &group),
            Some(&record),
            now,
            100,
            300
        ));
        pf.legs[0].f_snap = percolator::V16PodI128::new(0);
        pf.legs[0].a_basis = percolator::V16PodU128::new(u128::MAX);
        assert!(refresh_due(
            &pf,
            &group,
            assess(&pf, &group),
            Some(&record),
            now,
            100,
            300
        ));
    }

    #[test]
    fn newly_liquidatable_quiet_account_reaches_submit_this_cycle() {
        use crate::{
            quarantine::{Quarantine, QuarantineConfig},
            scheduler::{self, Attempt, Candidate, CycleBudget},
        };
        let (mut pf, group, record, now) = quiet_fixture();
        let key = solana_sdk::pubkey::Pubkey::new_unique();
        let mut quarantine = Quarantine::new(QuarantineConfig::default());
        quarantine.schedule_state_mut().refresh.insert(key, record);
        assert!(!refresh_due(
            &pf,
            &group,
            assess(&pf, &group),
            quarantine.schedule_state().refresh.get(&key),
            now,
            100,
            300
        ));
        pf.health_cert.certified_liq_deficit = percolator::V16PodU128::new(1);
        let work = assess(&pf, &group);
        let candidates = if refresh_due(
            &pf,
            &group,
            work,
            quarantine.schedule_state().refresh.get(&key),
            now,
            100,
            300,
        ) {
            vec![Candidate {
                pubkey: key,
                priority: work.priority,
                plan_class: "live:liquidatable".into(),
                pending_work: true,
            }]
        } else {
            vec![]
        };
        let mut submitted = Vec::new();
        let summary = scheduler::run_cycle(
            1,
            1,
            now,
            candidates,
            &CycleBudget::default(),
            &mut quarantine,
            |c, budget| {
                assert!(budget.begin_tx());
                budget.settle_tx(Some(1));
                submitted.push(c.pubkey);
                Ok(Attempt::Progress { cu: 1, steps: 1 })
            },
        );
        assert_eq!(submitted, vec![key]);
        assert_eq!(summary.progressed, 1);
    }

    #[test]
    fn subatom_f_gap_is_settled() {
        // |basis| = 1 unit (POS_SCALE), a_basis = 1e15 (ADL_ONE scale-ish):
        // gap < a_basis pays floor(<1) = 0 atoms → settled.
        let a = 1_000_000_000_000_000u128;
        let basis = POS_SCALE as i128;
        assert!(leg_f_settled(basis, a, 0, (a - 1) as i128));
        assert!(!leg_f_settled(basis, a, 0, a as i128));
        assert!(!leg_f_settled(-basis, a, 0, a as i128), "short basis uses |basis|");
    }

    #[test]
    fn zero_gap_and_flat_legs_are_settled_negative_gap_is_not() {
        assert!(leg_f_settled(5, 7, 10, 10));
        assert!(!leg_f_settled(5, 7, 10, 9), "floor toward -inf: a debit");
        assert!(leg_f_settled(0, 7, 0, i128::MAX));
        assert!(leg_f_settled(5, 0, 0, i128::MAX));
    }

    #[test]
    fn overflowing_gap_is_not_settled() {
        assert!(!leg_f_settled(i128::MAX, 1, i128::MIN, i128::MAX));
    }

    #[test]
    fn priority_orders_liquidatable_first() {
        assert!(Priority::Liquidatable < Priority::Pending);
        assert!(Priority::Pending < Priority::Quiet);
    }
}
