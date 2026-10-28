//! Prometheus text exposition (format 0.0.4) for a [`Snapshot`].
//!
//! Every family is emitted once with HELP/TYPE, samples grouped under it.
//! Values are exact integers where the source is an integer (Prometheus
//! stores f64, so values above 2^53 lose precision on ingestion; the JSON
//! snapshot keeps them exact).

use std::fmt::{Display, Write as _};

use super::snapshot::{keeper_derived, Snapshot, BUCKET_STATUSES, LIFECYCLES, MODES};

/// Escape a label value per the exposition format.
pub fn escape_label(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[derive(Default)]
pub struct Prom {
    out: String,
}

impl Prom {
    /// Start a family. Samples for it must follow before the next family.
    pub fn family(&mut self, name: &str, kind: &str, help: &str) -> &mut Self {
        let _ = writeln!(
            self.out,
            "# HELP {name} {}",
            help.replace('\\', "\\\\").replace('\n', "\\n")
        );
        let _ = writeln!(self.out, "# TYPE {name} {kind}");
        self
    }

    pub fn sample(
        &mut self,
        name: &str,
        labels: &[(&str, &str)],
        value: impl Display,
    ) -> &mut Self {
        self.out.push_str(name);
        if !labels.is_empty() {
            let parts: Vec<String> = labels
                .iter()
                .map(|(k, v)| format!("{k}=\"{}\"", escape_label(v)))
                .collect();
            let _ = write!(self.out, "{{{}}}", parts.join(","));
        }
        let _ = writeln!(self.out, " {value}");
        self
    }

    /// One-sample family.
    pub fn gauge(&mut self, name: &str, help: &str, labels: &[(&str, &str)], value: impl Display) {
        self.family(name, "gauge", help).sample(name, labels, value);
    }

    pub fn finish(self) -> String {
        self.out
    }
}

pub fn render(s: &Snapshot) -> String {
    let mut p = Prom::default();
    let market = s.market.to_string();
    let m: &[(&str, &str)] = &[("market", market.as_str())];

    p.gauge(
        "percolator_monitor_snapshot_unix_seconds",
        "Wall-clock time the monitor took this snapshot.",
        m,
        format!("{:.3}", s.taken_unix_ms as f64 / 1000.0),
    );
    p.gauge(
        "percolator_monitor_chain_read_ok",
        "1 if the market and its portfolios were read and decoded this poll.",
        m,
        s.chain.is_ok() as u8,
    );

    if let Ok(c) = &s.chain {
        p.gauge(
            "percolator_chain_slot",
            "RPC slot at snapshot time (confirmed).",
            m,
            c.slot,
        );
        p.gauge(
            "percolator_engine_current_slot",
            "Engine header current_slot (last slot an instruction advanced the market to).",
            m,
            c.header_current_slot,
        );
        p.family(
            "percolator_market_mode",
            "gauge",
            "Market mode (1 for the current mode).",
        );
        for mode in MODES {
            p.sample(
                "percolator_market_mode",
                &[("market", &market), ("mode", mode)],
                (c.mode == mode) as u8,
            );
        }

        // ---- accounts
        let a = &c.accounts;
        p.gauge(
            "percolator_accounts_total",
            "Portfolios of this market found by discovery.",
            m,
            a.total,
        );
        p.gauge(
            "percolator_accounts_decode_errors",
            "Discovered portfolios that read_portfolio could not decode.",
            m,
            a.decode_errors,
        );
        p.gauge(
            "percolator_engine_materialized_portfolio_count",
            "Engine header materialized_portfolio_count.",
            m,
            a.materialized_portfolio_count,
        );
        let ood = "percolator_accounts_out_of_date";
        p.family(
            ood,
            "gauge",
            "Out-of-date accounts by the engine's own fields, per field (kind) and source (portfolio scan vs engine counter).",
        );
        for (kind, source, v) in [
            ("stale_state", "portfolio", a.stale_state),
            ("stale_state", "engine", a.stale_certificate_count),
            ("b_stale_state", "portfolio", a.b_stale_state),
            ("b_stale_state", "engine", a.b_stale_account_count),
            ("kf_stale_cohort", "portfolio", a.kf_stale_cohort_observed),
            ("kf_stale_cohort", "engine", a.kf_stale_cohort_engine),
            ("any", "portfolio", a.out_of_date),
        ] {
            p.sample(
                ood,
                &[("market", &market), ("kind", kind), ("source", source)],
                v,
            );
        }
        p.family(
            "percolator_accounts_out_of_date_mismatch",
            "gauge",
            "|portfolio scan − engine counter| per kind. Nonzero means the two disagree (or the scan raced a write).",
        );
        for (kind, x, y) in [
            ("stale_state", a.stale_state, a.stale_certificate_count),
            ("b_stale_state", a.b_stale_state, a.b_stale_account_count),
            (
                "kf_stale_cohort",
                a.kf_stale_cohort_observed,
                a.kf_stale_cohort_engine,
            ),
        ] {
            p.sample(
                "percolator_accounts_out_of_date_mismatch",
                &[("market", &market), ("kind", kind)],
                x.abs_diff(y),
            );
        }
        p.family(
            "percolator_accounts_keeper_estimate",
            "gauge",
            "Keeper's off-chain priority estimate (work::assess). Heuristic, not an engine definition.",
        );
        p.sample(
            "percolator_accounts_keeper_estimate",
            &[("market", &market), ("priority", "liquidatable")],
            a.liquidatable_estimate,
        );
        p.sample(
            "percolator_accounts_keeper_estimate",
            &[("market", &market), ("priority", "pending")],
            a.pending_estimate,
        );
        p.gauge(
            "percolator_legs_f_subatom_remainder",
            "Active legs whose only funding gap is worth less than one atom. NOT pending work: the engine considers them settled.",
            m,
            a.f_subatom_legs,
        );

        // ---- assets
        p.family(
            "percolator_asset_lifecycle",
            "gauge",
            "Asset lifecycle (1 for the current state).",
        );
        for x in &c.assets {
            let asset = x.asset.to_string();
            for l in LIFECYCLES {
                p.sample(
                    "percolator_asset_lifecycle",
                    &[("market", &market), ("asset", &asset), ("lifecycle", l)],
                    (x.lifecycle == l) as u8,
                );
            }
        }
        p.family(
            "percolator_asset_funding_last_accrual_slot",
            "gauge",
            "AssetStateV16.slot_last: last slot this asset accrued (funding/mark).",
        );
        for x in &c.assets {
            p.sample(
                "percolator_asset_funding_last_accrual_slot",
                &[("market", &market), ("asset", &x.asset.to_string())],
                x.slot_last,
            );
        }
        p.family(
            "percolator_asset_slots_since_accrual",
            "gauge",
            "Chain slot − AssetStateV16.slot_last.",
        );
        for x in &c.assets {
            p.sample(
                "percolator_asset_slots_since_accrual",
                &[("market", &market), ("asset", &x.asset.to_string())],
                c.slot.saturating_sub(x.slot_last),
            );
        }
        p.family(
            "percolator_asset_kf_stale_accounts",
            "gauge",
            "Per asset side: engine stale_account_count_{long,short} (source=engine) vs active legs with kf_epoch_snap < kf_epoch (source=portfolio).",
        );
        for x in &c.assets {
            let asset = x.asset.to_string();
            for (side, source, v) in [
                ("long", "engine", x.stale_account_count_long),
                ("short", "engine", x.stale_account_count_short),
                ("long", "portfolio", x.stale_legs_observed_long),
                ("short", "portfolio", x.stale_legs_observed_short),
            ] {
                p.sample(
                    "percolator_asset_kf_stale_accounts",
                    &[
                        ("market", &market),
                        ("asset", &asset),
                        ("side", side),
                        ("source", source),
                    ],
                    v,
                );
            }
        }

        // ---- backing buckets
        let bc = &c.bucket_counts;
        p.family(
            "percolator_backing_buckets",
            "gauge",
            "Backing buckets by status.",
        );
        for (st, v) in BUCKET_STATUSES
            .iter()
            .zip([bc.fresh, bc.impaired, bc.expired, bc.empty])
        {
            p.sample(
                "percolator_backing_buckets",
                &[("market", &market), ("status", st)],
                v,
            );
        }
        let warn = s.expiry_warn_slots.to_string();
        p.gauge(
            "percolator_backing_buckets_fresh_expiring_soon",
            "Fresh buckets with 0 < expiry_slot − slot <= window slots.",
            &[("market", &market), ("window_slots", &warn)],
            bc.fresh_expiring_soon,
        );
        p.gauge(
            "percolator_backing_buckets_fresh_past_expiry",
            "Fresh buckets with expiry_slot <= slot (any mode).",
            m,
            bc.fresh_past_expiry,
        );
        p.gauge(
            "percolator_backing_buckets_fresh_past_expiry_in_live",
            "Fresh buckets past expiry while the market is Live: the known engine-side wedge condition (see RUNBOOK, Known limitations).",
            m,
            bc.fresh_past_expiry_in_live,
        );
        p.gauge(
            "percolator_backing_buckets_fresh_past_expiry_in_live_unreferenced",
            "Subset of the above whose domain no portfolio references: no Live instruction can expire it.",
            m,
            bc.fresh_past_expiry_in_live_unreferenced,
        );
        p.gauge(
            "percolator_backing_buckets_expiring_with_liens",
            "Fresh buckets with valid_liened_backing_num > 0 and expiry_slot − slot <= window (including past expiry).",
            &[("market", &market), ("window_slots", &warn)],
            bc.expiring_with_liens,
        );
        p.family(
            "percolator_backing_bucket_slots_to_expiry",
            "gauge",
            "Per Fresh bucket: expiry_slot − chain slot (negative = past expiry).",
        );
        for b in c.buckets.iter().filter(|b| b.fresh()) {
            p.sample(
                "percolator_backing_bucket_slots_to_expiry",
                &[
                    ("market", &market),
                    ("asset", &b.asset.to_string()),
                    ("side", b.side),
                ],
                b.slots_to_expiry,
            );
        }
        p.family(
            "percolator_backing_bucket_valid_liened_num",
            "gauge",
            "Per non-empty bucket: valid_liened_backing_num (BOUND_SCALE units).",
        );
        for b in c.buckets.iter().filter(|b| b.status != "empty") {
            p.sample(
                "percolator_backing_bucket_valid_liened_num",
                &[
                    ("market", &market),
                    ("asset", &b.asset.to_string()),
                    ("side", b.side),
                ],
                b.valid_liened_backing_num,
            );
        }

        // ---- unowned collateral
        let u = &c.unowned;
        let t = &u.terms;
        p.family(
            "percolator_engine_header_atoms",
            "gauge",
            "Engine header terms of residual() and the junior claims, in collateral atoms (exact).",
        );
        for (term, v) in [
            ("vault", t.vault),
            ("c_tot", t.c_tot),
            ("insurance", t.insurance),
            (
                "backing_provider_earnings_total",
                t.backing_provider_earnings_total,
            ),
            (
                "source_fresh_backing_total",
                u.source_fresh_backing_total_atoms,
            ),
            ("pnl_pos_tot", t.pnl_pos_tot),
            ("pnl_pos_bound_tot", t.pnl_pos_bound_tot),
        ] {
            p.sample(
                "percolator_engine_header_atoms",
                &[("market", &market), ("term", term)],
                v,
            );
        }
        p.gauge(
            "percolator_engine_source_fresh_backing_total_num",
            "Engine header source_fresh_backing_total_num (BOUND_SCALE units, raw header read).",
            m,
            t.source_fresh_backing_total_num,
        );
        p.gauge(
            "percolator_residual_atoms",
            "Engine residual(): vault − (c_tot + insurance + backing_provider_earnings_total + source_fresh_backing_total_num / BOUND_SCALE), saturating at 0.",
            m,
            u.residual,
        );
        p.gauge(
            "percolator_senior_shortfall_atoms",
            "Senior claims above vault (residual saturated). Should be 0.",
            m,
            u.senior_shortfall,
        );
        p.gauge(
            "percolator_unowned_collateral_atoms",
            "Unowned collateral: residual − pnl_pos_tot (signed).",
            m,
            u.unowned,
        );
        p.gauge(
            "percolator_unowned_collateral_vs_bound_atoms",
            "Variant with the junior claim bound: residual − pnl_pos_bound_tot (signed).",
            m,
            u.unowned_vs_bound,
        );
        if let Some(bal) = c.spl_vault_balance {
            p.gauge(
                "percolator_spl_vault_balance_atoms",
                "SPL token balance of the market vault account.",
                &[("market", &market), ("vault", &c.spl_vault.to_string())],
                bal,
            );
            p.gauge(
                "percolator_spl_vault_minus_engine_vault_atoms",
                "SPL vault balance − engine header vault (signed).",
                m,
                bal as i128 - t.vault as i128,
            );
        }

        // ---- resolved claims
        if let Some(r) = &c.resolved {
            p.gauge(
                "percolator_resolved_claimants_unpaid",
                "Resolved market: portfolios with capital > 0, positive PnL, or an open payout receipt.",
                m,
                r.claimants_unpaid,
            );
            p.family(
                "percolator_resolved_receipts",
                "gauge",
                "Resolved payout receipts by state.",
            );
            p.sample(
                "percolator_resolved_receipts",
                &[("market", &market), ("state", "open")],
                r.receipts_open,
            );
            p.sample(
                "percolator_resolved_receipts",
                &[("market", &market), ("state", "finalized")],
                r.receipts_finalized,
            );
            p.family(
                "percolator_resolved_owed_atoms",
                "gauge",
                "Resolved market atoms by component. receipt_face_remaining, receipt_claimable_now and capital are exact; unreceipted_positive_pnl is face value, not payout.",
            );
            for (component, v) in [
                ("receipt_face_remaining", r.receipt_face_remaining_atoms),
                ("receipt_claimable_now", r.receipt_claimable_now_atoms),
                ("capital", r.capital_atoms),
                ("unreceipted_positive_pnl", r.unreceipted_positive_pnl_atoms),
                ("negative_pnl", r.negative_pnl_atoms),
            ] {
                p.sample(
                    "percolator_resolved_owed_atoms",
                    &[("market", &market), ("component", component)],
                    v,
                );
            }
            p.gauge(
                "percolator_resolved_payout_halted",
                "Resolved payout ledger payout_halted flag.",
                m,
                r.ledger_payout_halted as u8,
            );
        }
    }

    // ---- keeper status file
    if s.keeper.is_some() {
        p.gauge(
            "percolator_keeper_status_ok",
            "1 if the configured keeper status file was read and parsed, 0 if missing or unparseable. Absent when no status file is configured.",
            m,
            s.keeper_ok().is_some() as u8,
        );
    }
    if let Some(age) = s.keeper_age_seconds {
        p.gauge(
            "percolator_keeper_status_age_seconds",
            "Seconds since the keeper last wrote its status file.",
            m,
            format!("{age:.3}"),
        );
    }
    if let Some(k) = s.keeper_ok() {
        let d = keeper_derived(s);
        p.gauge(
            "percolator_keeper_cycle",
            "Keeper cycle number of the last written status.",
            m,
            k.cycle,
        );
        p.gauge(
            "percolator_keeper_cycle_slot",
            "Slot the last keeper cycle was planned at.",
            m,
            k.slot,
        );
        p.family(
            "percolator_keeper_last_cycle_attempts",
            "gauge",
            "Per-account attempts in the last cycle, by outcome.",
        );
        let lc = &k.last_cycle;
        for (o, v) in [
            ("progress", lc.progressed),
            ("idle", lc.idle),
            ("failed", lc.failed),
            ("quarantined_skipped", lc.quarantined_skipped),
            ("budget_deferred", lc.budget_deferred),
        ] {
            p.sample(
                "percolator_keeper_last_cycle_attempts",
                &[("market", &market), ("outcome", o)],
                v,
            );
        }
        let t = &k.totals;
        p.family(
            "percolator_keeper_attempts_total",
            "counter",
            "Per-account attempts since keeper start, by outcome.",
        );
        for (o, v) in [
            ("progress", t.attempts_progress),
            ("idle", t.attempts_idle),
            ("failed", t.attempts_failed),
        ] {
            p.sample(
                "percolator_keeper_attempts_total",
                &[("market", &market), ("outcome", o)],
                v,
            );
        }
        p.family(
            "percolator_keeper_cycles_total",
            "counter",
            "Cycles since keeper start by class: progress (>= 1 progressed), failed (none progressed, >= 1 failed), idle.",
        );
        for (o, v) in [
            ("progress", t.cycles_progress),
            ("idle", t.cycles_idle),
            ("failed", t.cycles_failed),
        ] {
            p.sample(
                "percolator_keeper_cycles_total",
                &[("market", &market), ("outcome", o)],
                v,
            );
        }
        p.gauge(
            "percolator_keeper_crank_cu_samples",
            "Program-CU samples in the keeper's recent window.",
            m,
            k.cu_window.len(),
        );
        if let (Some(p50), Some(max)) = (d.cu_p50, d.cu_max) {
            p.family(
                "percolator_keeper_crank_cu",
                "gauge",
                "Program CU per submitted crank over the recent window.",
            );
            p.sample(
                "percolator_keeper_crank_cu",
                &[("market", &market), ("stat", "p50")],
                p50,
            );
            p.sample(
                "percolator_keeper_crank_cu",
                &[("market", &market), ("stat", "max")],
                max,
            );
        }
        p.gauge(
            "percolator_keeper_quarantined_accounts",
            "Accounts the keeper holds struck (quarantined/unrefreshable, not yet cleared by a success).",
            m,
            k.quarantined.len(),
        );
        p.family(
            "percolator_keeper_account_quarantined_seconds",
            "gauge",
            "Per struck account: seconds since the keeper first struck it (labels carry last error, strike).",
        );
        for q in &k.quarantined {
            let age = s.taken_unix_ms.saturating_sub(q.since_unix_ms) as f64 / 1000.0;
            p.sample(
                "percolator_keeper_account_quarantined_seconds",
                &[
                    ("market", &market),
                    ("account", &q.account),
                    ("error", &q.error),
                    ("strike", &q.strike.to_string()),
                    ("until_cycle", &q.until_cycle.to_string()),
                ],
                format!("{age:.3}"),
            );
        }
        if let Some(ps) = k.last_progress_slot {
            p.gauge(
                "percolator_keeper_last_progress_slot",
                "Slot of the last keeper cycle with >= 1 Progress crank.",
                m,
                ps,
            );
        }
        if let Some(v) = d.slots_since_last_progress {
            p.gauge(
                "percolator_keeper_slots_since_last_progress",
                "Chain slot − last_progress_slot.",
                m,
                v,
            );
        }
    }
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposition_lines_are_well_formed() {
        let mut p = Prom::default();
        p.family("x_total", "counter", "Help with \\ and\nnewline.");
        p.sample("x_total", &[("a", "q\"v\\"), ("b", "line\nbreak")], 7);
        p.gauge("y", "Y.", &[], -3);
        let out = p.finish();
        assert_eq!(
            out,
            "# HELP x_total Help with \\\\ and\\nnewline.\n# TYPE x_total counter\n\
             x_total{a=\"q\\\"v\\\\\",b=\"line\\nbreak\"} 7\n\
             # HELP y Y.\n# TYPE y gauge\ny -3\n"
        );
    }

    #[test]
    fn every_family_is_declared_once_and_samples_follow_it() {
        use crate::monitor::snapshot::Snapshot;
        use solana_sdk::pubkey::Pubkey;
        let s = Snapshot {
            taken_unix_ms: 1_000,
            market: Pubkey::new_unique(),
            program: Pubkey::new_unique(),
            expiry_warn_slots: 100,
            chain: Err("down".into()),
            keeper: None,
            keeper_age_seconds: None,
        };
        let out = render(&s);
        assert!(out.contains("percolator_monitor_chain_read_ok{market="));
        assert!(
            !out.contains("percolator_keeper_status_ok"),
            "absent when not configured"
        );
        let mut seen = std::collections::HashSet::new();
        let mut current = String::new();
        for line in out.lines() {
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                let name = rest.split(' ').next().unwrap().to_string();
                assert!(seen.insert(name.clone()), "duplicate family {name}");
                current = name;
            } else if !line.starts_with('#') {
                let name = line.split(['{', ' ']).next().unwrap();
                assert_eq!(name, current, "sample outside its family: {line}");
            }
        }
    }
}
