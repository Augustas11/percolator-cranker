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
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer},
};

use crate::binding::SOURCE_LIEN_DRAIN_MAX_STEPS;
use crate::discovery;
use crate::observation::{self, FetchedObservation, PythFeedResolver};
use crate::quarantine::Quarantine;
use crate::resolved;
use crate::scheduler::{self, Attempt, Candidate, CycleBudget, CycleSummary};
use crate::submit::{self, CrankOutcome, CrankRoute, OutcomeClass};
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

/// CU charged for a crank whose CU could not be read (measurement off or
/// `getTransaction` missed), so the cycle CU budget still binds.
pub const UNKNOWN_CU_CHARGE: u64 = 200_000;

fn outcome_cu(o: &CrankOutcome) -> u64 {
    o.program_cu().unwrap_or(UNKNOWN_CU_CHARGE)
}

/// Fold one candidate's drain into a scheduler attempt. `rpc_error` is an
/// RPC failure that ended the drain early; steps that already landed still
/// count as progress.
fn attempt_from(outcomes: &[CrankOutcome], rpc_error: Option<&str>) -> Attempt {
    let cu: u64 = outcomes.iter().map(outcome_cu).sum();
    let progressed = outcomes.iter().filter(|o| o.class() == OutcomeClass::Progress).count();
    if progressed > 0 {
        if let Some(o) = outcomes.last().filter(|o| o.class() == OutcomeClass::Failed) {
            println!(
                "  note: progressed {progressed} step(s) then failed: {}",
                o.error_key().unwrap_or_default()
            );
        }
        return Attempt::Progress {
            cu,
            steps: progressed,
        };
    }
    if let Some(e) = rpc_error {
        return Attempt::Failed {
            error: format!("Rpc: {e}"),
            cu,
        };
    }
    match outcomes.last() {
        Some(o) if o.class() == OutcomeClass::Failed => Attempt::Failed {
            error: o.error_key().unwrap_or_else(|| "unknown".into()),
            cu,
        },
        _ => Attempt::Idle { cu },
    }
}

/// What the cycle decided to do with one candidate.
enum Plan {
    Submit {
        route_owner: Option<Pubkey>,
        hints: Vec<FetchedObservation>,
    },
    /// Resolved market inside `force_close_delay_slots`: the owner must sign,
    /// so a third-party crank would revert `Unauthorized`. Idle, no tx.
    OwnerWindow,
    /// Could not build hints for this account. Failed, no tx.
    HintError(String),
    /// TEST HOOK (`CRANKER_FAULT_ACCOUNTS`): report a synthetic failure for
    /// this account without submitting anything.
    InjectedFault,
}

/// Synthetic error reported for accounts named by the test hook.
pub const INJECTED_FAULT_ERROR: &str = "InjectedFault";

/// **Test hook, off unless set.** `CRANKER_FAULT_ACCOUNTS=<pk,pk,...>` makes
/// the cycle report `Failed { error: "InjectedFault" }` for those accounts
/// without sending a transaction, so monitoring and quarantine can be
/// validated against a healthy market. `CRANKER_FAULT_ACCOUNTS=@<file>`
/// reads the same comma/whitespace-separated list from a file, re-read every
/// cycle, so a fault can be injected into a keeper that is already running.
/// Unparseable entries are ignored. Real failure paths are untouched.
pub fn injected_fault_accounts() -> Vec<Pubkey> {
    let Ok(v) = std::env::var("CRANKER_FAULT_ACCOUNTS") else {
        return Vec::new();
    };
    let list = match v.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
        None => v,
    };
    parse_pubkey_list(&list)
}

fn parse_pubkey_list(s: &str) -> Vec<Pubkey> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter_map(|t| t.trim().parse::<Pubkey>().ok())
        .collect()
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
    let (cfg, group) = read_market(&market_data).map_err(|e| anyhow!("read_market: {e:?}"))?;
    let now_slot = ctx.rpc.get_slot()?;

    // Resolved-route accounts (payout to the owner's ATA).
    let resolved = group.mode == MarketModeV16::Resolved;
    let mint = Pubkey::new_from_array(cfg.collateral_mint);
    let (vault_authority, _) =
        Pubkey::find_program_address(&[b"vault", ctx.market.as_ref()], ctx.program_id);
    let vault = spl_associated_token_account::get_associated_token_address(&vault_authority, &mint);
    let owner_window = resolved
        && cfg.force_close_delay_slots != 0
        && now_slot.saturating_sub(group.resolved_slot) < cfg.force_close_delay_slots;
    let payer = ctx.payer.pubkey();

    let faults = injected_fault_accounts();
    let mut plans = std::collections::HashMap::new();
    let mut candidates = Vec::with_capacity(ranked.len());
    for p in &ranked {
        let priority = read_portfolio(&p.account.data)
            .map(|pf| work::assess(&pf, &group).priority)
            .unwrap_or(Priority::Pending);
        let owner = p.owner().unwrap_or_default();
        let (plan, plan_class) =
            match plan_hints(&group, &market_data, &p.account.data, ctx.pyth, now_slot) {
                _ if faults.contains(&p.pubkey) => (Plan::InjectedFault, "test:injected-fault".to_string()),
                Err(e) => (Plan::HintError(format!("HintError: {e}")), "hint-error".to_string()),
                Ok(_) if owner_window && owner != payer => {
                    (Plan::OwnerWindow, "resolved:owner-window".to_string())
                }
                Ok((hints, class)) => {
                    let plan_class = match (class, hints.first()) {
                        ("resolved:backing-hint", Some(h)) => format!("{class}({})", h.asset_index),
                        ("live" | "recovery", _) => format!("{class}:{}", priority.label()),
                        _ => class.to_string(),
                    };
                    let route_owner = resolved.then_some(owner);
                    (Plan::Submit { route_owner, hints }, plan_class)
                }
            };
        plans.insert(p.pubkey, plan);
        candidates.push(Candidate {
            pubkey: p.pubkey,
            priority,
            plan_class,
        });
    }

    let summary = scheduler::run_cycle(cycle, now_slot, candidates, budget, quarantine, |c, allowance| {
        let (route, hints) = match plans.get(&c.pubkey) {
            Some(Plan::Submit { route_owner: None, hints }) => (
                CrankRoute::Live {
                    reward_portfolio: ctx.reward_portfolio,
                },
                hints.as_slice(),
            ),
            Some(Plan::Submit {
                route_owner: Some(owner),
                hints,
            }) => (
                CrankRoute::Resolved {
                    owner: *owner,
                    mint,
                    vault,
                    vault_authority,
                },
                hints.as_slice(),
            ),
            Some(Plan::OwnerWindow) | None => return Ok((Attempt::Idle { cu: 0 }, 0)),
            Some(Plan::HintError(e)) => return Ok((Attempt::Failed { error: e.clone(), cu: 0 }, 0)),
            Some(Plan::InjectedFault) => {
                return Ok((
                    Attempt::Failed {
                        error: INJECTED_FAULT_ERROR.into(),
                        cu: 0,
                    },
                    0,
                ))
            }
        };
        let mut outcomes = Vec::new();
        let mut rpc_error = None;
        let mut slot = now_slot;
        for _ in 0..SOURCE_LIEN_DRAIN_MAX_STEPS.min(allowance.max(1)) {
            let o = match submit::submit_crank_routed(
                ctx.rpc,
                ctx.program_id,
                ctx.payer,
                ctx.market,
                &c.pubkey,
                &route,
                hints,
                slot,
            ) {
                Ok(o) => o,
                Err(e) => {
                    rpc_error = Some(e.to_string());
                    break;
                }
            };
            cu.observe(&o);
            // Keep draining only while the TARGET moved (A6 one domain per
            // call); a reward-portfolio byte diff alone is not drain progress.
            let keep_draining =
                matches!(&o, CrankOutcome::Ok { target_bytes_diff, .. } if *target_bytes_diff > 0);
            outcomes.push(o);
            if !keep_draining {
                break;
            }
            match ctx.rpc.get_slot() {
                Ok(s) => slot = s,
                Err(e) => {
                    rpc_error = Some(e.to_string());
                    break;
                }
            }
        }
        let attempt = attempt_from(&outcomes, rpc_error.as_deref());
        let sent = outcomes.len().max(1);
        for o in outcomes {
            records.push(SubmissionRecord {
                cycle,
                target: c.pubkey,
                plan_class: c.plan_class.clone(),
                outcome: o,
            });
        }
        Ok((attempt, sent))
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
        let a = attempt_from(&[ok(true, 100), ok(true, 100), ok(false, 18)], None);
        assert_eq!(a, Attempt::Progress { cu: 218, steps: 2 });
    }

    #[test]
    fn non_progress_revert_alone_is_idle() {
        assert_eq!(attempt_from(&[rej("EngineNonProgress")], None), Attempt::Idle { cu: 10 });
    }

    #[test]
    fn engine_error_alone_is_failed() {
        assert_eq!(
            attempt_from(&[rej("EngineCounterUnderflow")], None),
            Attempt::Failed {
                error: "EngineCounterUnderflow".into(),
                cu: 10
            }
        );
    }

    /// Review finding: an RPC error after a landed step must not erase the
    /// progress or strike the account.
    #[test]
    fn rpc_error_after_progress_is_still_progress() {
        assert_eq!(
            attempt_from(&[ok(true, 100)], Some("timeout")),
            Attempt::Progress { cu: 100, steps: 1 }
        );
        assert_eq!(
            attempt_from(&[], Some("timeout")),
            Attempt::Failed {
                error: "Rpc: timeout".into(),
                cu: 0
            }
        );
    }

    #[test]
    fn unknown_cu_is_charged() {
        let mut o = ok(false, 0);
        if let CrankOutcome::Ok { program_cu, .. } = &mut o {
            *program_cu = None;
        }
        assert_eq!(attempt_from(&[o], None), Attempt::Idle { cu: UNKNOWN_CU_CHARGE });
    }

    #[test]
    fn fault_list_parses_commas_and_whitespace() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        assert_eq!(parse_pubkey_list(&format!("{a}, {b}\nnot-a-key")), vec![a, b]);
        assert!(parse_pubkey_list("").is_empty());
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
