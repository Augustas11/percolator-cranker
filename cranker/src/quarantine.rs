//! Per-account failure quarantine.
//!
//! Live-shaped test deployments (reported on upstream percolator #226) hit
//! accounts that no crank could refresh: every attempt reverted with the same engine
//! error (`EngineCounterUnderflow` on engine `5f2a7151`). Upstream fixed that
//! case; the cranker assumes the next one exists.
//!
//! Rule: when one account fails with the **same** error `threshold` times in
//! a row, stop trying it for a backoff window measured in cycles. The window
//! doubles on each re-quarantine and is capped. Any non-failure outcome
//! (progress or idle) clears the account's record. Every quarantine decision
//! is emitted as one JSON line so a report to upstream is a `grep`.

use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuarantineConfig {
    /// Consecutive same-error failures before quarantine. Default 3.
    pub threshold: u32,
    /// First backoff window, in cycles. Default 2.
    pub base_backoff_cycles: u64,
    /// Cap on the backoff window, in cycles. Default 64.
    pub max_backoff_cycles: u64,
}

impl Default for QuarantineConfig {
    fn default() -> Self {
        Self {
            threshold: 3,
            base_backoff_cycles: 2,
            max_backoff_cycles: 64,
        }
    }
}

impl QuarantineConfig {
    /// `CRANKER_QUARANTINE_THRESHOLD`, `CRANKER_QUARANTINE_BASE_CYCLES`,
    /// `CRANKER_QUARANTINE_MAX_CYCLES`; defaults otherwise.
    pub fn from_env() -> Self {
        let d = Self::default();
        let get = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
        Self {
            threshold: get("CRANKER_QUARANTINE_THRESHOLD")
                .map(|v| v.max(1) as u32)
                .unwrap_or(d.threshold),
            base_backoff_cycles: get("CRANKER_QUARANTINE_BASE_CYCLES")
                .map(|v| v.max(1))
                .unwrap_or(d.base_backoff_cycles),
            max_backoff_cycles: get("CRANKER_QUARANTINE_MAX_CYCLES")
                .map(|v| v.max(1))
                .unwrap_or(d.max_backoff_cycles),
        }
    }

    /// Backoff for the `strike`-th quarantine (1-based): base · 2^(strike-1), capped.
    pub fn backoff_for_strike(&self, strike: u32) -> u64 {
        let shift = strike.saturating_sub(1).min(32);
        self.base_backoff_cycles
            .saturating_mul(1u64 << shift)
            .min(self.max_backoff_cycles)
    }
}

#[derive(Debug, Clone, Default)]
struct Entry {
    last_error: String,
    streak: u32,
    strikes: u32,
    until_cycle: u64,
}

/// One failure as the scheduler saw it. `plan_class` is the cranker's own
/// label for what it was attempting (see `scheduler::Candidate::plan_class`);
/// the engine's selected plan is not observable from outside the tx.
#[derive(Debug, Clone)]
pub struct FailureReport<'a> {
    pub account: &'a Pubkey,
    pub error: &'a str,
    pub cycle: u64,
    pub slot: u64,
    pub plan_class: &'a str,
}

/// Emitted when an account enters quarantine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineEvent {
    pub account: Pubkey,
    pub error: String,
    pub cycle: u64,
    pub slot: u64,
    pub plan_class: String,
    pub strike: u32,
    pub backoff_cycles: u64,
    pub until_cycle: u64,
}

impl QuarantineEvent {
    /// One-line JSON, no serde dependency. Values are pubkeys, error names
    /// and labels, so escaping `"` and `\` is enough.
    pub fn to_json_line(&self) -> String {
        format!(
            "{{\"event\":\"quarantine\",\"account\":\"{}\",\"error\":\"{}\",\"cycle\":{},\"slot\":{},\"plan_class\":\"{}\",\"strike\":{},\"backoff_cycles\":{},\"until_cycle\":{}}}",
            self.account,
            json_escape(&self.error),
            self.cycle,
            self.slot,
            json_escape(&self.plan_class),
            self.strike,
            self.backoff_cycles,
            self.until_cycle,
        )
    }
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Debug, Default)]
pub struct Quarantine {
    cfg: QuarantineConfig,
    entries: HashMap<Pubkey, Entry>,
    events: Vec<QuarantineEvent>,
}

impl Quarantine {
    pub fn new(cfg: QuarantineConfig) -> Self {
        Self {
            cfg,
            entries: HashMap::new(),
            events: Vec::new(),
        }
    }

    pub fn config(&self) -> &QuarantineConfig {
        &self.cfg
    }

    /// `true` while `account` is inside a backoff window at `cycle`.
    pub fn is_blocked(&self, account: &Pubkey, cycle: u64) -> bool {
        self.entries
            .get(account)
            .map(|e| e.strikes > 0 && cycle < e.until_cycle)
            .unwrap_or(false)
    }

    /// Accounts blocked at `cycle`.
    pub fn blocked_count(&self, cycle: u64) -> usize {
        self.entries
            .values()
            .filter(|e| e.strikes > 0 && cycle < e.until_cycle)
            .count()
    }

    /// Record a failure. Returns the event when this failure tips the
    /// account into quarantine.
    pub fn record_failure(&mut self, f: FailureReport<'_>) -> Option<QuarantineEvent> {
        let cfg = self.cfg;
        let e = self.entries.entry(*f.account).or_default();
        if e.last_error == f.error {
            e.streak += 1;
        } else {
            e.last_error = f.error.to_string();
            e.streak = 1;
        }
        if e.streak < cfg.threshold {
            return None;
        }
        // After a quarantine, the next same-error failure re-quarantines
        // at once (with a longer window) instead of spending another
        // `threshold` txs on an account already known to be stuck.
        e.streak = cfg.threshold.saturating_sub(1);
        e.strikes += 1;
        let backoff_cycles = cfg.backoff_for_strike(e.strikes);
        e.until_cycle = f.cycle.saturating_add(1).saturating_add(backoff_cycles);
        let ev = QuarantineEvent {
            account: *f.account,
            error: f.error.to_string(),
            cycle: f.cycle,
            slot: f.slot,
            plan_class: f.plan_class.to_string(),
            strike: e.strikes,
            backoff_cycles,
            until_cycle: e.until_cycle,
        };
        self.events.push(ev.clone());
        Some(ev)
    }

    /// Progress or idle: the account is healthy again.
    pub fn record_ok(&mut self, account: &Pubkey) {
        self.entries.remove(account);
    }

    /// Every quarantine event so far, in order.
    pub fn events(&self) -> &[QuarantineEvent] {
        &self.events
    }

    /// Read-only view of one struck account: (last error, strikes,
    /// until_cycle). `None` when the account has no strike. Used by the
    /// opt-in status file; it does not change any quarantine decision.
    pub fn struck_entry(&self, account: &Pubkey) -> Option<(&str, u32, u64)> {
        self.entries
            .get(account)
            .filter(|e| e.strikes > 0)
            .map(|e| (e.last_error.as_str(), e.strikes, e.until_cycle))
    }

    /// Accounts that have ever been quarantined and are not cleared yet.
    pub fn struck_accounts(&self) -> Vec<Pubkey> {
        let mut v: Vec<Pubkey> = self
            .entries
            .iter()
            .filter(|(_, e)| e.strikes > 0)
            .map(|(k, _)| *k)
            .collect();
        v.sort_by_key(|k| k.to_bytes());
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail<'a>(q: &mut Quarantine, pk: &'a Pubkey, err: &'a str, cycle: u64) -> Option<QuarantineEvent> {
        q.record_failure(FailureReport {
            account: pk,
            error: err,
            cycle,
            slot: 100 + cycle,
            plan_class: "live:pending",
        })
    }

    #[test]
    fn quarantines_after_threshold_same_errors() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let pk = Pubkey::new_unique();
        assert!(fail(&mut q, &pk, "EngineCounterUnderflow", 0).is_none());
        assert!(fail(&mut q, &pk, "EngineCounterUnderflow", 1).is_none());
        let ev = fail(&mut q, &pk, "EngineCounterUnderflow", 2).expect("quarantined");
        assert_eq!(ev.strike, 1);
        assert_eq!(ev.backoff_cycles, 2);
        assert!(q.is_blocked(&pk, 3));
        assert!(q.is_blocked(&pk, 4));
        assert!(!q.is_blocked(&pk, 5));
        assert!(ev.to_json_line().contains("\"error\":\"EngineCounterUnderflow\""));
    }

    #[test]
    fn different_errors_reset_the_streak() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let pk = Pubkey::new_unique();
        fail(&mut q, &pk, "A", 0);
        fail(&mut q, &pk, "A", 1);
        assert!(fail(&mut q, &pk, "B", 2).is_none());
        assert!(fail(&mut q, &pk, "B", 3).is_none());
        assert!(!q.is_blocked(&pk, 4));
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let cfg = QuarantineConfig {
            threshold: 1,
            base_backoff_cycles: 2,
            max_backoff_cycles: 10,
        };
        assert_eq!(cfg.backoff_for_strike(1), 2);
        assert_eq!(cfg.backoff_for_strike(2), 4);
        assert_eq!(cfg.backoff_for_strike(3), 8);
        assert_eq!(cfg.backoff_for_strike(4), 10);
        assert_eq!(cfg.backoff_for_strike(40), 10);
        let mut q = Quarantine::new(cfg);
        let pk = Pubkey::new_unique();
        assert_eq!(fail(&mut q, &pk, "E", 0).unwrap().backoff_cycles, 2);
        assert_eq!(fail(&mut q, &pk, "E", 3).unwrap().backoff_cycles, 4);
    }

    #[test]
    fn released_account_requarantines_on_first_repeat() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let pk = Pubkey::new_unique();
        for c in 0..3 {
            fail(&mut q, &pk, "E", c);
        }
        assert!(!q.is_blocked(&pk, 5), "window over");
        let ev = fail(&mut q, &pk, "E", 5).expect("one failure is enough after a strike");
        assert_eq!((ev.strike, ev.backoff_cycles), (2, 4));
        // A different error still starts a fresh streak.
        let pk2 = Pubkey::new_unique();
        for c in 0..3 {
            fail(&mut q, &pk2, "E", c);
        }
        assert!(fail(&mut q, &pk2, "F", 5).is_none());
    }

    #[test]
    fn success_clears_record() {
        let mut q = Quarantine::new(QuarantineConfig::default());
        let pk = Pubkey::new_unique();
        for c in 0..3 {
            fail(&mut q, &pk, "E", c);
        }
        assert!(q.is_blocked(&pk, 3));
        q.record_ok(&pk);
        assert!(!q.is_blocked(&pk, 3));
        assert!(q.struck_accounts().is_empty());
        assert_eq!(q.events().len(), 1, "history is kept for the report");
    }
}
