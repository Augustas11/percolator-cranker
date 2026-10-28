//! One crank cycle: order candidates, spend a bounded budget, never let one
//! account starve the rest.
//!
//! * **Order.** Each priority class (liquidatable, pending, quiet) is
//!   rotated by the cycle number, then the classes are **interleaved** in a
//!   fixed 2:1:1 pattern. A steady stream of liquidatable accounts therefore
//!   cannot starve pending or quiet ones: every class gets a share of every
//!   cycle's budget.
//! * **Budget.** Wall-clock (measured from cycle entry, discovery included),
//!   compute units and tx count. Enforced **per tx**: the submit closure
//!   gets an [`AttemptBudget`] and must call `begin_tx` before every tx,
//!   which checks every budget and reserves the worst-case CU
//!   (`submit::TX_CU_LIMIT`), then `settle_tx` with the measured CU. A tx is
//!   counted when it is begun, whatever its outcome. One attempt (an A6
//!   drain) is also capped at `max_txs_per_attempt`. What doesn't fit is
//!   *deferred*, and rotation reaches it first next cycle.
//! * **Isolation.** A failed submission (revert, engine error, RPC error) is
//!   recorded and the loop moves on. Quarantined accounts are skipped
//!   without spending budget.
//! * **Idle vs failed vs stuck.** `EngineNonProgress` or `Ok(no-mut)` is
//!   idle, not a failure. Idle that persists for an account the cycle
//!   flagged as still owed work (`Candidate::pending_work`) is reported as
//!   `idle_pending_stuck`, so a claim that never becomes payable doesn't
//!   hide behind "idle".
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
    /// The cycle believes this account is still owed work (e.g. a resolved
    /// claimant with capital or PnL left). Persistent idle on such an
    /// account is reported as stuck.
    pub pending_work: bool,
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
    /// Max txs one candidate may send in one cycle (bounds an A6 drain;
    /// the rest continues next cycle).
    pub max_txs_per_attempt: usize,
    /// CU reserved before each tx: the requested compute-unit limit, i.e.
    /// the most one tx can consume.
    pub cu_per_tx_reserve: u64,
    /// Consecutive idle cycles on a `pending_work` account before it is
    /// reported as stuck.
    pub idle_stuck_cycles: u64,
}

impl Default for CycleBudget {
    fn default() -> Self {
        Self {
            max_wall: Duration::from_secs(30),
            max_cu: 24_000_000,
            max_submissions: 64,
            max_txs_per_attempt: 4,
            cu_per_tx_reserve: crate::submit::TX_CU_LIMIT as u64,
            idle_stuck_cycles: 10,
        }
    }
}

impl CycleBudget {
    /// `CRANKER_CYCLE_MS`, `CRANKER_CYCLE_CU`, `CRANKER_CYCLE_MAX_TX`,
    /// `CRANKER_ATTEMPT_MAX_TX`, `CRANKER_IDLE_STUCK_CYCLES`.
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
            max_txs_per_attempt: get("CRANKER_ATTEMPT_MAX_TX")
                .map(|v| (v as usize).max(1))
                .unwrap_or(d.max_txs_per_attempt),
            cu_per_tx_reserve: d.cu_per_tx_reserve,
            idle_stuck_cycles: get("CRANKER_IDLE_STUCK_CYCLES")
                .map(|v| v.max(1))
                .unwrap_or(d.idle_stuck_cycles),
        }
    }
}

/// Cycle-wide remaining budget.
#[derive(Debug)]
struct Meter {
    deadline: Instant,
    txs_left: usize,
    cu_left: u64,
    reserve: u64,
    per_attempt: usize,
}

impl Meter {
    fn can_send(&self) -> bool {
        self.txs_left > 0 && Instant::now() < self.deadline && self.cu_left >= self.reserve
    }
}

/// Budget view handed to one attempt. Call [`begin_tx`](Self::begin_tx)
/// before every tx (it returns `false` when any budget is exhausted) and
/// [`settle_tx`](Self::settle_tx) after it with the measured CU, if known.
#[derive(Debug)]
pub struct AttemptBudget<'m> {
    meter: &'m mut Meter,
    txs: usize,
    open_reserve: u64,
}

impl AttemptBudget<'_> {
    /// Admit one more tx: counts it immediately and reserves the worst-case
    /// CU. A tx that is begun is counted whatever happens to it afterwards.
    pub fn begin_tx(&mut self) -> bool {
        if self.open_reserve != 0 || self.txs >= self.meter.per_attempt || !self.meter.can_send() {
            return false;
        }
        self.meter.txs_left -= 1;
        self.meter.cu_left -= self.meter.reserve;
        self.open_reserve = self.meter.reserve;
        self.txs += 1;
        true
    }

    /// Replace the open reservation with the measured CU. `None` (unknown)
    /// keeps the whole reservation charged.
    pub fn settle_tx(&mut self, cu: Option<u64>) {
        let used = cu.map_or(self.open_reserve, |c| c.min(self.open_reserve));
        self.meter.cu_left += self.open_reserve - used;
        self.open_reserve = 0;
    }

    /// Wall-clock time left in the cycle (bound RPC waits with it).
    pub fn time_left(&self) -> Duration {
        self.meter.deadline.saturating_duration_since(Instant::now())
    }

    pub fn txs_sent(&self) -> usize {
        self.txs
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CycleSummary {
    pub cycle: u64,
    pub progressed: usize,
    pub idle: usize,
    pub failed: usize,
    pub quarantined_skipped: usize,
    pub budget_deferred: usize,
    pub newly_quarantined: usize,
    /// Txs begun this cycle (each counted once, whatever its outcome).
    pub txs: usize,
    /// CU charged this cycle: measured CU, or the full reserve when unknown.
    pub cu_spent: u64,
    pub max_attempt_cu: u64,
    /// `pending_work` accounts idle for `idle_stuck_cycles` or more.
    pub idle_pending_stuck: usize,
    pub failures: Vec<(Pubkey, String)>,
    pub progressed_accounts: Vec<Pubkey>,
}

impl CycleSummary {
    pub fn line(&self) -> String {
        format!(
            "cycle {} summary: progressed={} idle={} failed={} quarantined_skipped={} deferred={} new_quarantine={} idle_pending_stuck={} txs={} cu={} max_tx_cu={}",
            self.cycle,
            self.progressed,
            self.idle,
            self.failed,
            self.quarantined_skipped,
            self.budget_deferred,
            self.newly_quarantined,
            self.idle_pending_stuck,
            self.txs,
            self.cu_spent,
            self.max_attempt_cu,
        )
    }
}

/// Interleave pattern over priority classes (2:1:1). Empty classes are
/// skipped, so a lone class gets the whole budget.
const CLASS_PATTERN: [Priority; 4] = [
    Priority::Liquidatable,
    Priority::Liquidatable,
    Priority::Pending,
    Priority::Quiet,
];

/// Rotate each priority class by `cycle`, then interleave the classes with
/// [`CLASS_PATTERN`].
pub fn order_candidates(mut cands: Vec<Candidate>, cycle: u64) -> Vec<Candidate> {
    cands.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.pubkey.to_bytes().cmp(&b.pubkey.to_bytes()))
    });
    let mut classes: Vec<std::collections::VecDeque<Candidate>> = Vec::new();
    for p in [Priority::Liquidatable, Priority::Pending, Priority::Quiet] {
        let mut class: Vec<Candidate> = cands.iter().filter(|c| c.priority == p).cloned().collect();
        let n = class.len();
        if n > 0 {
            class.rotate_left((cycle as usize) % n);
        }
        classes.push(class.into());
    }
    let mut out = Vec::with_capacity(cands.len());
    while out.len() < cands.len() {
        for p in CLASS_PATTERN {
            if let Some(c) = classes[p as usize].pop_front() {
                out.push(c);
            }
        }
    }
    out
}

/// Run one cycle. `started` is the cycle's entry time (discovery and
/// planning count against the wall budget). `submit` performs one
/// candidate's crank(s) within the given [`AttemptBudget`]; it must not
/// panic, and an `Err` is a failure of that candidate only.
pub fn run_cycle<F>(
    cycle: u64,
    slot: u64,
    started: Instant,
    candidates: Vec<Candidate>,
    budget: &CycleBudget,
    quarantine: &mut Quarantine,
    mut submit: F,
) -> CycleSummary
where
    F: FnMut(&Candidate, &mut AttemptBudget<'_>) -> anyhow::Result<Attempt>,
{
    let mut s = CycleSummary {
        cycle,
        ..CycleSummary::default()
    };
    let mut meter = Meter {
        deadline: started + budget.max_wall,
        txs_left: budget.max_submissions,
        cu_left: budget.max_cu,
        reserve: budget.cu_per_tx_reserve,
        per_attempt: budget.max_txs_per_attempt.max(1),
    };
    for cand in &order_candidates(candidates, cycle) {
        if quarantine.is_blocked(&cand.pubkey, cycle) {
            s.quarantined_skipped += 1;
            continue;
        }
        if !meter.can_send() {
            s.budget_deferred += 1;
            continue;
        }
        let txs_before = meter.txs_left;
        let cu_before = meter.cu_left;
        let mut ab = AttemptBudget {
            meter: &mut meter,
            txs: 0,
            open_reserve: 0,
        };
        // An unsettled tx (the closure bailed mid-tx) keeps its whole
        // reservation charged: the meter was debited in `begin_tx`.
        let result = submit(cand, &mut ab);
        let attempt = result.unwrap_or_else(|e| Attempt::Failed {
            error: format!("Rpc: {e}"),
            cu: 0,
        });
        s.txs += txs_before - meter.txs_left;
        let charged = cu_before - meter.cu_left;
        s.cu_spent = s.cu_spent.saturating_add(charged);
        s.max_attempt_cu = s.max_attempt_cu.max(attempt.cu());
        match attempt {
            Attempt::Progress { .. } => {
                s.progressed += 1;
                s.progressed_accounts.push(cand.pubkey);
                quarantine.record_ok(&cand.pubkey);
            }
            Attempt::Idle { .. } => {
                s.idle += 1;
                let streak = quarantine.record_idle(&cand.pubkey, cand.pending_work);
                if cand.pending_work && streak >= budget.idle_stuck_cycles {
                    s.idle_pending_stuck += 1;
                    if streak == budget.idle_stuck_cycles {
                        println!(
                            "{{\"event\":\"idle_with_pending_work\",\"account\":\"{}\",\"cycle\":{},\"slot\":{},\"plan_class\":{},\"idle_cycles\":{}}}",
                            cand.pubkey,
                            cycle,
                            slot,
                            serde_json::Value::String(cand.plan_class.clone()),
                            streak
                        );
                    }
                }
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
            pending_work: false,
        }
    }

    fn pk(i: u8) -> Pubkey {
        Pubkey::new_from_array([i; 32])
    }

    /// One tx per attempt, settled with `cu`.
    fn one_tx(ab: &mut AttemptBudget<'_>, cu: u64, a: Attempt) -> anyhow::Result<Attempt> {
        if !ab.begin_tx() {
            return Ok(Attempt::Idle { cu: 0 });
        }
        ab.settle_tx(Some(cu));
        Ok(a)
    }

    fn run<F>(cycle: u64, cands: Vec<Candidate>, b: &CycleBudget, q: &mut Quarantine, f: F) -> CycleSummary
    where
        F: FnMut(&Candidate, &mut AttemptBudget<'_>) -> anyhow::Result<Attempt>,
    {
        run_cycle(cycle, 1_000 + cycle, Instant::now(), cands, b, q, f)
    }

    #[test]
    fn classes_interleave_and_rotate() {
        let (a, b, c, l, p) = (pk(1), pk(2), pk(3), pk(9), pk(5));
        let cands = vec![
            cand(a, Priority::Quiet),
            cand(b, Priority::Quiet),
            cand(c, Priority::Quiet),
            cand(l, Priority::Liquidatable),
            cand(p, Priority::Pending),
        ];
        let o0: Vec<_> = order_candidates(cands.clone(), 0).iter().map(|c| c.pubkey).collect();
        let o1: Vec<_> = order_candidates(cands, 1).iter().map(|c| c.pubkey).collect();
        assert_eq!(o0, vec![l, p, a, b, c]);
        assert_eq!(o1, vec![l, p, b, c, a]);
    }

    /// Review finding: sustained liquidatable load must not starve pending
    /// or quiet accounts.
    #[test]
    fn sustained_liquidatable_load_leaves_share_for_other_classes() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let budget = CycleBudget {
            max_submissions: 12,
            ..CycleBudget::default()
        };
        let mut hits: HashMap<Priority, usize> = HashMap::new();
        for cycle in 0..10 {
            let mut cands: Vec<Candidate> = (0..30u8).map(|i| cand(pk(i), Priority::Liquidatable)).collect();
            cands.extend((100..110u8).map(|i| cand(pk(i), Priority::Pending)));
            cands.extend((200..210u8).map(|i| cand(pk(i), Priority::Quiet)));
            run(cycle, cands, &budget, &mut q, |c, ab| {
                *hits.entry(c.priority).or_default() += 1;
                one_tx(ab, 1, Attempt::Progress { cu: 1, steps: 1 })
            });
        }
        assert_eq!(hits[&Priority::Liquidatable], 60);
        assert_eq!(hits[&Priority::Pending], 30);
        assert_eq!(hits[&Priority::Quiet], 30);
    }

    /// Acceptance: a persistent per-account engine error gets quarantined
    /// while every other account keeps progressing, cycle after cycle.
    #[test]
    fn persistent_error_is_quarantined_and_others_progress() {
        let stuck = pk(0);
        let healthy: Vec<Pubkey> = (1..=5u8).map(pk).collect();
        let mut q = Quarantine::new(QuarantineConfig::default());
        let budget = CycleBudget::default();
        let mut progressed: HashMap<Pubkey, usize> = HashMap::new();
        let mut stuck_attempts = 0usize;
        let cycles = 20u64;
        let mut quarantined_cycles = 0usize;
        for cycle in 0..cycles {
            let mut cands = vec![cand(stuck, Priority::Liquidatable)];
            cands.extend(healthy.iter().map(|p| cand(*p, Priority::Pending)));
            let s = run(cycle, cands, &budget, &mut q, |c, ab| {
                if c.pubkey == stuck {
                    stuck_attempts += 1;
                    one_tx(ab, 50_000, Attempt::Failed { error: "EngineCounterUnderflow".into(), cu: 50_000 })
                } else {
                    *progressed.entry(c.pubkey).or_default() += 1;
                    one_tx(ab, 40_000, Attempt::Progress { cu: 40_000, steps: 1 })
                }
            });
            assert_eq!(s.progressed, healthy.len(), "cycle {cycle}: healthy accounts starved");
            quarantined_cycles += s.quarantined_skipped;
        }
        for p in &healthy {
            assert_eq!(progressed[p], cycles as usize);
        }
        assert!(stuck_attempts < cycles as usize / 2, "stuck tried {stuck_attempts}x");
        assert!(quarantined_cycles > 0);
        assert!(q.events().len() >= 2, "re-quarantine with growing backoff");
        assert!(q.events()[1].backoff_cycles > q.events()[0].backoff_cycles);
    }

    #[test]
    fn idle_and_failed_are_counted_separately() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let cands = (0..3u8).map(|i| cand(pk(i), Priority::Quiet)).collect();
        let s = run(0, cands, &CycleBudget::default(), &mut q, |c, ab| match c.pubkey.to_bytes()[0] {
            0 => one_tx(ab, 18_000, Attempt::Idle { cu: 18_000 }),
            1 => one_tx(ab, 0, Attempt::Failed { error: "EngineStale".into(), cu: 0 }),
            _ => {
                assert!(ab.begin_tx());
                anyhow::bail!("connection reset")
            }
        });
        assert_eq!((s.progressed, s.idle, s.failed), (0, 1, 2));
        assert!(s.failures.iter().any(|(_, e)| e.starts_with("Rpc:")));
        assert_eq!(s.txs, 3, "the tx that errored is still counted");
        // The errored tx never settled: its whole reserve stays charged.
        assert_eq!(s.cu_spent, 18_000 + TX_RESERVE);
    }

    const TX_RESERVE: u64 = crate::submit::TX_CU_LIMIT as u64;

    /// A drain that would send 10 txs per attempt is stopped by each budget
    /// in turn: per-attempt cap, cycle tx cap, CU, wall clock.
    #[test]
    fn multi_step_drain_respects_every_budget() {
        let drain = |q: &mut Quarantine, budget: CycleBudget, started: Instant| {
            let cands: Vec<Candidate> = (0..4u8).map(|i| cand(pk(i), Priority::Pending)).collect();
            let mut per_attempt = Vec::new();
            let s = run_cycle(0, 0, started, cands, &budget, q, |_, ab| {
                let mut n = 0;
                while n < 10 && ab.begin_tx() {
                    ab.settle_tx(Some(1_000));
                    n += 1;
                }
                per_attempt.push(n);
                Ok(Attempt::Progress { cu: 1_000 * n as u64, steps: n })
            });
            (s, per_attempt)
        };
        let mut q = Quarantine::new(QuarantineConfig::default());

        let b = CycleBudget { max_txs_per_attempt: 3, ..CycleBudget::default() };
        let (s, per) = drain(&mut q, b, Instant::now());
        assert_eq!(per, vec![3, 3, 3, 3]);
        assert_eq!(s.txs, 12);

        let b = CycleBudget { max_txs_per_attempt: 10, max_submissions: 7, ..CycleBudget::default() };
        let (s, per) = drain(&mut q, b, Instant::now());
        assert_eq!(per, vec![7]);
        assert_eq!((s.txs, s.budget_deferred), (7, 3));

        // CU: each tx reserves TX_RESERVE up front; the budget fits 2 reserves.
        let b = CycleBudget { max_txs_per_attempt: 10, max_cu: 2 * TX_RESERVE, ..CycleBudget::default() };
        let (s, _) = drain(&mut q, b, Instant::now());
        assert!(s.cu_spent <= 2 * TX_RESERVE);
        assert!(s.txs >= 2, "settled CU is refunded, so later txs still fit");

        // Wall clock starts at cycle entry: an already-expired cycle sends nothing.
        let b = CycleBudget { max_wall: Duration::from_millis(1), ..CycleBudget::default() };
        let (s, per) = drain(&mut q, b, Instant::now() - Duration::from_secs(1));
        assert!(per.is_empty());
        assert_eq!((s.txs, s.budget_deferred), (0, 4));
    }

    #[test]
    fn rotation_reaches_deferred_accounts() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let budget = CycleBudget { max_submissions: 2, ..CycleBudget::default() };
        let mut seen = std::collections::HashSet::new();
        for cycle in 0..2 {
            let cands = (0..4u8).map(|i| cand(pk(i), Priority::Quiet)).collect();
            let s = run(cycle, cands, &budget, &mut q, |c, ab| {
                seen.insert(c.pubkey);
                one_tx(ab, 1, Attempt::Idle { cu: 1 })
            });
            assert_eq!(s.budget_deferred, 2);
        }
        assert_eq!(seen.len(), 3, "rotation by one reaches a deferred account");
    }

    /// Review finding: a claimant whose payout never becomes ready must not
    /// hide behind "idle" forever.
    #[test]
    fn persistent_idle_on_pending_work_is_reported() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let budget = CycleBudget { idle_stuck_cycles: 3, ..CycleBudget::default() };
        let mut stuck_counts = Vec::new();
        for cycle in 0..5 {
            let mut owed = cand(pk(1), Priority::Pending);
            owed.pending_work = true;
            let cands = vec![owed, cand(pk(2), Priority::Quiet)];
            let s = run(cycle, cands, &budget, &mut q, |_, ab| one_tx(ab, 1, Attempt::Idle { cu: 1 }));
            stuck_counts.push(s.idle_pending_stuck);
        }
        assert_eq!(stuck_counts, vec![0, 0, 1, 1, 1], "only the owed account, from cycle 3 on");
    }
}
