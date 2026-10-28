//! One crank cycle: order candidates, spend a bounded budget, never let one
//! account starve the rest.
//!
//! * **Order.** Priority first (liquidatable → pending → quiet), then a
//!   per-cycle rotation inside each priority class so the same account is
//!   not always first or always cut off.
//! * **Budget.** Wall-clock, compute units and tx count per cycle. Every tx
//!   counts, including A6 drain steps: the submit closure gets the remaining
//!   tx allowance and reports how many it sent. When a budget runs out the
//!   rest of the queue is *deferred*, and the rotation puts it earlier next
//!   cycle. A quarter of the tx budget is reserved for quiet accounts so a
//!   large liquidatable/pending backlog can't starve them forever.
//! * **Isolation.** A failed submission (revert, engine error, RPC error) is
//!   recorded and the loop moves on. Quarantined accounts are skipped
//!   without spending budget.
//! * **Idle vs failed.** `EngineNonProgress` or `Ok(no-mut)` is idle, not a
//!   failure, and is reported separately.
//!
//! The submit step is a closure so the same loop runs against a real
//! validator (`main.rs`, the load scenario) and in unit tests.

use solana_sdk::pubkey::Pubkey;
use std::time::{Duration, Instant};

use crate::quarantine::{FailureReport, Quarantine};
use crate::work::Priority;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub pubkey: Pubkey,
    pub priority: Priority,
    /// What the cranker is attempting, e.g. `live:liquidatable` or
    /// `resolved:backing-hint(2)`. Logged with quarantine events.
    pub plan_class: String,
}

/// What one candidate's submission (including any A6 drain steps) came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attempt {
    Progress { cu: u64, steps: usize },
    Idle { cu: u64 },
    Failed { error: String, cu: u64 },
}

impl Attempt {
    fn cu(&self) -> u64 {
        match self {
            Attempt::Progress { cu, .. } | Attempt::Idle { cu } | Attempt::Failed { cu, .. } => *cu,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CycleBudget {
    pub max_wall: Duration,
    pub max_cu: u64,
    /// Max txs per cycle (A6 drain steps included).
    pub max_submissions: usize,
    /// CU reserved before each attempt; an attempt is not started unless
    /// this much budget remains.
    pub cu_per_attempt_reserve: u64,
}

impl Default for CycleBudget {
    fn default() -> Self {
        Self {
            max_wall: Duration::from_secs(30),
            max_cu: 24_000_000,
            max_submissions: 64,
            cu_per_attempt_reserve: 600_000,
        }
    }
}

impl CycleBudget {
    /// `CRANKER_CYCLE_MS`, `CRANKER_CYCLE_CU`, `CRANKER_CYCLE_MAX_TX`.
    pub fn from_env() -> Self {
        let d = Self::default();
        let get = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
        Self {
            max_wall: get("CRANKER_CYCLE_MS")
                .map(Duration::from_millis)
                .unwrap_or(d.max_wall),
            max_cu: get("CRANKER_CYCLE_CU").unwrap_or(d.max_cu),
            max_submissions: get("CRANKER_CYCLE_MAX_TX")
                .map(|v| v as usize)
                .unwrap_or(d.max_submissions),
            cu_per_attempt_reserve: d.cu_per_attempt_reserve,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CycleSummary {
    pub cycle: u64,
    /// Slot the cycle was planned at (the `slot` passed to `run_cycle`).
    pub slot: u64,
    pub progressed: usize,
    pub idle: usize,
    pub failed: usize,
    pub quarantined_skipped: usize,
    pub budget_deferred: usize,
    pub newly_quarantined: usize,
    pub cu_spent: u64,
    pub max_attempt_cu: u64,
    pub failures: Vec<(Pubkey, String)>,
    pub progressed_accounts: Vec<Pubkey>,
}

impl CycleSummary {
    pub fn line(&self) -> String {
        format!(
            "cycle {} summary: progressed={} idle={} failed={} quarantined_skipped={} deferred={} new_quarantine={} cu={} max_tx_cu={}",
            self.cycle,
            self.progressed,
            self.idle,
            self.failed,
            self.quarantined_skipped,
            self.budget_deferred,
            self.newly_quarantined,
            self.cu_spent,
            self.max_attempt_cu,
        )
    }
}

/// Stable priority sort, then rotate each priority class by `cycle`.
pub fn order_candidates(mut cands: Vec<Candidate>, cycle: u64) -> Vec<Candidate> {
    cands.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.pubkey.to_bytes().cmp(&b.pubkey.to_bytes()))
    });
    let mut out = Vec::with_capacity(cands.len());
    let mut i = 0;
    while i < cands.len() {
        let p = cands[i].priority;
        let j = cands[i..]
            .iter()
            .position(|c| c.priority != p)
            .map(|k| i + k)
            .unwrap_or(cands.len());
        let class = &mut cands[i..j];
        let n = class.len();
        if n > 0 {
            class.rotate_left((cycle as usize) % n);
        }
        out.extend_from_slice(class);
        i = j;
    }
    out
}

/// If the high-priority classes alone would fill the tx budget, move the
/// first `max_submissions / 4` (at least 1) quiet accounts ahead of the
/// high-priority tail. Order within each class is kept.
pub fn apply_quiet_reserve(ordered: Vec<Candidate>, max_submissions: usize) -> Vec<Candidate> {
    let (high, quiet): (Vec<_>, Vec<_>) =
        ordered.into_iter().partition(|c| c.priority != Priority::Quiet);
    let reserve = (max_submissions / 4).max(1).min(quiet.len());
    let head = max_submissions.saturating_sub(reserve);
    if high.len() <= head {
        return high.into_iter().chain(quiet).collect();
    }
    let mut high = high.into_iter();
    let mut quiet = quiet.into_iter();
    let mut out: Vec<Candidate> = high.by_ref().take(head).collect();
    out.extend(quiet.by_ref().take(reserve));
    out.extend(high);
    out.extend(quiet);
    out
}

/// Run one cycle. `submit` performs one candidate's crank(s) given the
/// remaining tx allowance and returns the attempt plus the number of txs it
/// sent. It must not panic; an `Err` is a failure of that candidate only.
pub fn run_cycle<F>(
    cycle: u64,
    slot: u64,
    candidates: Vec<Candidate>,
    budget: &CycleBudget,
    quarantine: &mut Quarantine,
    mut submit: F,
) -> CycleSummary
where
    F: FnMut(&Candidate, usize) -> anyhow::Result<(Attempt, usize)>,
{
    let started = Instant::now();
    let mut s = CycleSummary {
        cycle,
        slot,
        ..CycleSummary::default()
    };
    let ordered = apply_quiet_reserve(order_candidates(candidates, cycle), budget.max_submissions);
    let mut txs = 0usize;
    for cand in &ordered {
        if quarantine.is_blocked(&cand.pubkey, cycle) {
            s.quarantined_skipped += 1;
            continue;
        }
        let out_of_budget = txs >= budget.max_submissions
            || started.elapsed() >= budget.max_wall
            || s.cu_spent.saturating_add(budget.cu_per_attempt_reserve) > budget.max_cu;
        if out_of_budget {
            s.budget_deferred += 1;
            continue;
        }
        let (attempt, sent) = submit(cand, budget.max_submissions - txs).unwrap_or_else(|e| {
            (
                Attempt::Failed {
                    error: format!("Rpc: {e}"),
                    cu: 0,
                },
                1,
            )
        });
        txs += sent;
        s.cu_spent = s.cu_spent.saturating_add(attempt.cu());
        s.max_attempt_cu = s.max_attempt_cu.max(attempt.cu());
        match attempt {
            Attempt::Progress { .. } => {
                s.progressed += 1;
                s.progressed_accounts.push(cand.pubkey);
                quarantine.record_ok(&cand.pubkey);
            }
            Attempt::Idle { .. } => {
                s.idle += 1;
                quarantine.record_ok(&cand.pubkey);
            }
            Attempt::Failed { error, .. } => {
                s.failed += 1;
                if let Some(ev) = quarantine.record_failure(FailureReport {
                    account: &cand.pubkey,
                    error: &error,
                    cycle,
                    slot,
                    plan_class: &cand.plan_class,
                }) {
                    println!("{}", ev.to_json_line());
                    s.newly_quarantined += 1;
                }
                s.failures.push((cand.pubkey, error));
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quarantine::QuarantineConfig;
    use std::collections::HashMap;

    fn cand(pk: Pubkey, priority: Priority) -> Candidate {
        Candidate {
            pubkey: pk,
            priority,
            plan_class: format!("live:{}", priority.label()),
        }
    }

    #[test]
    fn liquidatable_first_and_rotation_within_class() {
        let a = Pubkey::new_from_array([1; 32]);
        let b = Pubkey::new_from_array([2; 32]);
        let c = Pubkey::new_from_array([3; 32]);
        let l = Pubkey::new_from_array([9; 32]);
        let cands = vec![
            cand(a, Priority::Quiet),
            cand(b, Priority::Quiet),
            cand(c, Priority::Quiet),
            cand(l, Priority::Liquidatable),
        ];
        let o0: Vec<_> = order_candidates(cands.clone(), 0).iter().map(|c| c.pubkey).collect();
        let o1: Vec<_> = order_candidates(cands, 1).iter().map(|c| c.pubkey).collect();
        assert_eq!(o0, vec![l, a, b, c]);
        assert_eq!(o1, vec![l, b, c, a]);
    }

    /// Acceptance: a persistent per-account engine error gets quarantined
    /// while every other account keeps progressing, cycle after cycle.
    #[test]
    fn persistent_error_is_quarantined_and_others_progress() {
        let stuck = Pubkey::new_from_array([0; 32]); // sorts first: worst case for starvation
        let healthy: Vec<Pubkey> = (1..=5u8).map(|i| Pubkey::new_from_array([i; 32])).collect();
        let mut q = Quarantine::new(QuarantineConfig::default());
        let budget = CycleBudget::default();
        let mut progressed: HashMap<Pubkey, usize> = HashMap::new();
        let mut stuck_attempts = 0usize;
        let cycles = 20u64;
        let mut quarantined_cycles = 0usize;
        for cycle in 0..cycles {
            let mut cands = vec![cand(stuck, Priority::Liquidatable)];
            cands.extend(healthy.iter().map(|pk| cand(*pk, Priority::Pending)));
            let s = run_cycle(cycle, 1_000 + cycle, cands, &budget, &mut q, |c, _| {
                if c.pubkey == stuck {
                    stuck_attempts += 1;
                    Ok((
                        Attempt::Failed {
                            error: "EngineCounterUnderflow".into(),
                            cu: 50_000,
                        },
                        1,
                    ))
                } else {
                    *progressed.entry(c.pubkey).or_default() += 1;
                    Ok((Attempt::Progress { cu: 40_000, steps: 1 }, 1))
                }
            });
            assert_eq!(s.progressed, healthy.len(), "cycle {cycle}: healthy accounts starved");
            quarantined_cycles += s.quarantined_skipped;
        }
        for pk in &healthy {
            assert_eq!(progressed[pk], cycles as usize);
        }
        // 3 strikes to enter, then backoffs 2, 4, 8 … so far fewer than 20 tries.
        assert!(stuck_attempts < cycles as usize / 2, "stuck tried {stuck_attempts}×");
        assert!(quarantined_cycles > 0);
        assert!(q.events().len() >= 2, "re-quarantine with growing backoff");
        assert!(q.events()[1].backoff_cycles > q.events()[0].backoff_cycles);
    }

    #[test]
    fn idle_and_failed_are_counted_separately() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let pks: Vec<Pubkey> = (0..3u8).map(|i| Pubkey::new_from_array([i; 32])).collect();
        let cands = pks.iter().map(|pk| cand(*pk, Priority::Quiet)).collect();
        let s = run_cycle(0, 0, cands, &CycleBudget::default(), &mut q, |c, _| {
            Ok((
                match c.pubkey.to_bytes()[0] {
                    0 => Attempt::Idle { cu: 18_000 },
                    1 => Attempt::Failed { error: "EngineStale".into(), cu: 0 },
                    _ => anyhow::bail!("connection reset"),
                },
                1,
            ))
        });
        assert_eq!((s.progressed, s.idle, s.failed), (0, 1, 2));
        assert!(s.failures.iter().any(|(_, e)| e.starts_with("Rpc:")));
    }

    #[test]
    fn budget_defers_the_tail_and_rotation_reaches_it() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let budget = CycleBudget {
            max_submissions: 2,
            ..CycleBudget::default()
        };
        let pks: Vec<Pubkey> = (0..4u8).map(|i| Pubkey::new_from_array([i; 32])).collect();
        let mut seen = std::collections::HashSet::new();
        for cycle in 0..2 {
            let cands = pks.iter().map(|pk| cand(*pk, Priority::Quiet)).collect();
            let s = run_cycle(cycle, 0, cands, &budget, &mut q, |c, _| {
                seen.insert(c.pubkey);
                Ok((Attempt::Idle { cu: 1 }, 1))
            });
            assert_eq!(s.budget_deferred, 2);
        }
        assert_eq!(seen.len(), 3, "rotation by one reaches a deferred account");
    }

    #[test]
    fn drain_txs_count_against_the_budget() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let budget = CycleBudget {
            max_submissions: 5,
            ..CycleBudget::default()
        };
        let pks: Vec<Pubkey> = (0..4u8).map(|i| Pubkey::new_from_array([i; 32])).collect();
        let cands = pks.iter().map(|pk| cand(*pk, Priority::Pending)).collect();
        let mut allowances = Vec::new();
        let s = run_cycle(0, 0, cands, &budget, &mut q, |_, allowance| {
            allowances.push(allowance);
            let sent = allowance.min(3);
            Ok((Attempt::Progress { cu: 1, steps: sent }, sent))
        });
        assert_eq!(allowances, vec![5, 2]);
        assert_eq!((s.progressed, s.budget_deferred), (2, 2));
    }

    /// Review finding: a pending backlog bigger than the budget must not
    /// starve quiet accounts forever.
    #[test]
    fn quiet_accounts_get_reserved_slots() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let budget = CycleBudget {
            max_submissions: 8,
            ..CycleBudget::default()
        };
        let quiet = Pubkey::new_from_array([200; 32]);
        let mut cands: Vec<Candidate> = (0..20u8)
            .map(|i| cand(Pubkey::new_from_array([i; 32]), Priority::Pending))
            .collect();
        cands.push(cand(quiet, Priority::Quiet));
        let mut hit = false;
        run_cycle(0, 0, cands, &budget, &mut q, |c, _| {
            hit |= c.pubkey == quiet;
            Ok((Attempt::Idle { cu: 1 }, 1))
        });
        assert!(hit);
    }
}
