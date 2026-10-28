//! One discover → rank → hint → schedule → submit cycle, shared by the
//! binary (`main.rs`) and the `live_shaped` load scenario.
//!
//! Per candidate:
//! * **Live / Recovery market:** one hint per active leg asset (falling back
//!   to asset 0), each with the oracle accounts that asset's profile needs.
//! * **Resolved market:** zero or one oracle-less hint naming the asset whose
//!   backing bucket is due (`resolved::resolved_crank_hints`).
//!
//! The A6 drain (one source domain per call) runs inside one scheduler
//! attempt, bounded by `SOURCE_LIEN_DRAIN_MAX_STEPS`.

use anyhow::{anyhow, Context, Result};
use percolator::MarketModeV16;
use percolator_prog::state::{read_market, read_portfolio, MarketGroupV16};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{pubkey::Pubkey, signature::Keypair};

use crate::binding::SOURCE_LIEN_DRAIN_MAX_STEPS;
use crate::discovery;
use crate::observation::{self, FetchedObservation, PythFeedResolver};
use crate::quarantine::Quarantine;
use crate::resolved;
use crate::scheduler::{self, Attempt, Candidate, CycleBudget, CycleSummary};
use crate::submit::{self, CrankOutcome, OutcomeClass};
use crate::work::{self, Priority};

/// Wrapper cap: `CRANK_OBSERVATION_DECODE_MAX = 16`.
pub const HINT_BUDGET: usize = 16;

pub struct CrankCtx<'a> {
    pub rpc: &'a RpcClient,
    pub program_id: &'a Pubkey,
    pub payer: &'a Keypair,
    pub market: &'a Pubkey,
    pub reward_portfolio: Option<&'a Pubkey>,
    pub pyth: &'a PythFeedResolver,
    /// Accounts never cranked (e.g. our own reward portfolio).
    pub exclude: &'a [Pubkey],
}

/// One submission, for the mirror-custody table and CU stats.
#[derive(Debug)]
pub struct SubmissionRecord {
    pub cycle: u64,
    pub target: Pubkey,
    pub plan_class: String,
    pub outcome: CrankOutcome,
}

/// CU observations across a run.
#[derive(Debug, Default, Clone, Copy)]
pub struct CuStats {
    /// Smallest program CU seen on an `Ok(no-mut)` crank.
    pub noop_min: Option<u64>,
    /// Whole-tx CU for that same smallest no-op crank.
    pub noop_tx: Option<u64>,
    /// Largest program CU seen on any crank that landed.
    pub worst_ok: Option<u64>,
    /// Largest program CU seen on a reverted crank (preflight logs).
    pub worst_reject: Option<u64>,
}

impl CuStats {
    pub fn observe(&mut self, o: &CrankOutcome) {
        let Some(cu) = o.program_cu() else { return };
        match o {
            CrankOutcome::Ok { mutated, tx_cu, .. } => {
                if !mutated && self.noop_min.map_or(true, |m| cu < m) {
                    self.noop_min = Some(cu);
                    self.noop_tx = *tx_cu;
                }
                self.worst_ok = Some(self.worst_ok.map_or(cu, |m| m.max(cu)));
            }
            CrankOutcome::HardReject { .. } => {
                self.worst_reject = Some(self.worst_reject.map_or(cu, |m| m.max(cu)));
            }
        }
    }
}

fn plan_hints(
    group: &MarketGroupV16,
    market_data: &[u8],
    portfolio_data: &[u8],
    pyth: &PythFeedResolver,
    now_slot: u64,
) -> Result<(Vec<FetchedObservation>, &'static str)> {
    if group.mode == MarketModeV16::Resolved {
        let hints = resolved::resolved_crank_hints(group, now_slot);
        let class = if hints.is_empty() {
            "resolved:close"
        } else {
            "resolved:backing-hint"
        };
        return Ok((hints, class));
    }
    let mut assets: Vec<u16> = read_portfolio(portfolio_data)
        .map(|p| {
            p.legs
                .iter()
                .filter(|l| l.active != 0)
                .map(|l| l.asset_index.get() as u16)
                .collect()
        })
        .unwrap_or_default();
    assets.sort_unstable();
    assets.dedup();
    if assets.is_empty() {
        assets.push(0);
    }
    assets.truncate(HINT_BUDGET);
    let hints = assets
        .into_iter()
        .map(|a| observation::fetch_observation_for_asset(market_data, a, pyth))
        .collect::<Result<Vec<_>>>()?;
    let class = if group.mode == MarketModeV16::Recovery {
        "recovery"
    } else {
        "live"
    };
    Ok((hints, class))
}

fn attempt_from(outcomes: &[CrankOutcome]) -> Attempt {
    let cu: u64 = outcomes.iter().filter_map(|o| o.program_cu()).sum();
    let Some(last) = outcomes.last() else {
        return Attempt::Idle { cu };
    };
    let progressed = outcomes.iter().filter(|o| o.class() == OutcomeClass::Progress).count();
    match last.class() {
        OutcomeClass::Failed if progressed == 0 => Attempt::Failed {
            error: last.error_key().unwrap_or_else(|| "unknown".into()),
            cu,
        },
        _ if progressed > 0 => Attempt::Progress {
            cu,
            steps: progressed,
        },
        _ => Attempt::Idle { cu },
    }
}

/// Run one full cycle against `ctx.market`.
pub fn run_one_cycle(
    ctx: &CrankCtx<'_>,
    cycle: u64,
    budget: &CycleBudget,
    quarantine: &mut Quarantine,
    records: &mut Vec<SubmissionRecord>,
    cu: &mut CuStats,
) -> Result<CycleSummary> {
    let all = discovery::discover_portfolios(ctx.rpc, ctx.program_id)?;
    let reward = ctx.reward_portfolio.copied().unwrap_or_default();
    let ranked: Vec<_> = discovery::rank_by_staleness(all, ctx.market, &reward)
        .into_iter()
        .filter(|p| !ctx.exclude.contains(&p.pubkey))
        .collect();

    let market_data = ctx
        .rpc
        .get_account_data(ctx.market)
        .context("read market slab")?;
    let (_, group) = read_market(&market_data).map_err(|e| anyhow!("read_market: {e:?}"))?;
    let now_slot = ctx.rpc.get_slot()?;

    let mut plans = std::collections::HashMap::new();
    let mut candidates = Vec::with_capacity(ranked.len());
    for p in &ranked {
        let priority = read_portfolio(&p.account.data)
            .map(|pf| work::assess(&pf, &group).priority)
            .unwrap_or(Priority::Pending);
        let (hints, class) = plan_hints(&group, &market_data, &p.account.data, ctx.pyth, now_slot)?;
        let plan_class = match (class, hints.first()) {
            ("resolved:backing-hint", Some(h)) => format!("{class}({})", h.asset_index),
            ("live" | "recovery", _) => format!("{class}:{}", priority.label()),
            _ => class.to_string(),
        };
        plans.insert(p.pubkey, hints);
        candidates.push(Candidate {
            pubkey: p.pubkey,
            priority,
            plan_class,
        });
    }

    let summary = scheduler::run_cycle(cycle, now_slot, candidates, budget, quarantine, |c| {
        let hints = plans.get(&c.pubkey).map(Vec::as_slice).unwrap_or(&[]);
        let mut outcomes = Vec::new();
        let mut slot = now_slot;
        for _ in 0..SOURCE_LIEN_DRAIN_MAX_STEPS {
            let o = submit::submit_crank(
                ctx.rpc,
                ctx.program_id,
                ctx.payer,
                ctx.market,
                &c.pubkey,
                ctx.reward_portfolio,
                hints,
                slot,
            )?;
            cu.observe(&o);
            let keep_draining = o.class() == OutcomeClass::Progress;
            outcomes.push(o);
            if !keep_draining {
                break;
            }
            slot = ctx.rpc.get_slot()?;
        }
        let attempt = attempt_from(&outcomes);
        for o in outcomes {
            records.push(SubmissionRecord {
                cycle,
                target: c.pubkey,
                plan_class: c.plan_class.clone(),
                outcome: o,
            });
        }
        Ok(attempt)
    });
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signature::Signature;

    fn ok(mutated: bool, cu: u64) -> CrankOutcome {
        CrankOutcome::Ok {
            signature: Signature::default(),
            pre_state_hash: 0,
            post_state_hash: 0,
            mutated,
            target_bytes_diff: 0,
            reward_bytes_diff: None,
            program_cu: Some(cu),
            tx_cu: None,
        }
    }

    fn rej(name: &'static str) -> CrankOutcome {
        CrankOutcome::HardReject {
            error: String::new(),
            error_name: Some(name),
            program_cu: Some(10),
        }
    }

    #[test]
    fn drain_then_idle_is_progress() {
        let a = attempt_from(&[ok(true, 100), ok(true, 100), ok(false, 18)]);
        assert_eq!(a, Attempt::Progress { cu: 218, steps: 2 });
    }

    #[test]
    fn non_progress_revert_alone_is_idle() {
        assert_eq!(attempt_from(&[rej("EngineNonProgress")]), Attempt::Idle { cu: 10 });
    }

    #[test]
    fn engine_error_alone_is_failed() {
        assert_eq!(
            attempt_from(&[rej("EngineCounterUnderflow")]),
            Attempt::Failed {
                error: "EngineCounterUnderflow".into(),
                cu: 10
            }
        );
    }

    #[test]
    fn cu_stats_split_noop_and_worst() {
        let mut s = CuStats::default();
        s.observe(&ok(false, 18_000));
        s.observe(&ok(true, 300_000));
        s.observe(&ok(false, 17_975));
        s.observe(&rej("EngineStale"));
        assert_eq!(s.noop_min, Some(17_975));
        assert_eq!(s.worst_ok, Some(300_000));
        assert_eq!(s.worst_reject, Some(10));
    }
}
