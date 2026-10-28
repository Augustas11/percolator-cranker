//! Keeper status file: the keeper-side state the monitor cannot read on
//! chain (quarantine, crank outcomes, CU, last progress).
//!
//! Opt-in: the keeper writes it only when `CRANKER_STATUS_FILE` is set. It is
//! rewritten after every cycle with an atomic temp-file + rename, so a reader
//! never sees a half-written file. Writing never changes what the cycle or
//! the scheduler does; a write error is printed and ignored.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use solana_sdk::pubkey::Pubkey;

use super::json::{self, quote, Value};
use crate::cycle::SubmissionRecord;
use crate::quarantine::Quarantine;
use crate::scheduler::CycleSummary;

pub const STATUS_VERSION: u64 = 1;
/// Recent per-crank program-CU samples kept in the file.
pub const CU_WINDOW: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LastCycle {
    pub progressed: u64,
    pub idle: u64,
    pub failed: u64,
    pub quarantined_skipped: u64,
    pub budget_deferred: u64,
    pub newly_quarantined: u64,
    pub cu_spent: u64,
    pub max_attempt_cu: u64,
}

/// Cumulative counts since the keeper started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Totals {
    pub cycles: u64,
    /// Cycle classes: progress (≥ 1 account progressed), failed (none
    /// progressed, ≥ 1 failed), idle (otherwise).
    pub cycles_progress: u64,
    pub cycles_idle: u64,
    pub cycles_failed: u64,
    /// Per-attempt (per account per cycle) outcomes.
    pub attempts_progress: u64,
    pub attempts_idle: u64,
    pub attempts_failed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantinedAccount {
    pub account: String,
    /// Last error the account failed with (scheduler error key).
    pub error: String,
    pub strike: u32,
    pub until_cycle: u64,
    /// Inside its backoff window right now (skipped by the scheduler).
    pub blocked: bool,
    /// Wall-clock time the keeper first saw this account struck.
    pub since_unix_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeeperStatus {
    pub version: u64,
    pub written_unix_ms: u64,
    pub market: String,
    pub cycle: u64,
    pub slot: u64,
    pub last_cycle: LastCycle,
    pub totals: Totals,
    /// Program CU of recent submitted cranks, oldest first.
    pub cu_window: Vec<u64>,
    pub quarantined: Vec<QuarantinedAccount>,
    pub last_progress_slot: Option<u64>,
    pub last_progress_cycle: Option<u64>,
}

impl KeeperStatus {
    pub fn to_json(&self) -> String {
        let lc = &self.last_cycle;
        let t = &self.totals;
        let mut s = String::new();
        let _ = write!(
            s,
            "{{\"version\":{},\"written_unix_ms\":{},\"market\":{},\"cycle\":{},\"slot\":{},",
            self.version,
            self.written_unix_ms,
            quote(&self.market),
            self.cycle,
            self.slot
        );
        let _ = write!(
            s,
            "\"last_cycle\":{{\"progressed\":{},\"idle\":{},\"failed\":{},\"quarantined_skipped\":{},\"budget_deferred\":{},\"newly_quarantined\":{},\"cu_spent\":{},\"max_attempt_cu\":{}}},",
            lc.progressed, lc.idle, lc.failed, lc.quarantined_skipped, lc.budget_deferred, lc.newly_quarantined, lc.cu_spent, lc.max_attempt_cu
        );
        let _ = write!(
            s,
            "\"totals\":{{\"cycles\":{},\"cycles_progress\":{},\"cycles_idle\":{},\"cycles_failed\":{},\"attempts_progress\":{},\"attempts_idle\":{},\"attempts_failed\":{}}},",
            t.cycles, t.cycles_progress, t.cycles_idle, t.cycles_failed, t.attempts_progress, t.attempts_idle, t.attempts_failed
        );
        let cu: Vec<String> = self.cu_window.iter().map(u64::to_string).collect();
        let _ = write!(s, "\"cu_window\":[{}],", cu.join(","));
        let q: Vec<String> = self
            .quarantined
            .iter()
            .map(|q| {
                format!(
                    "{{\"account\":{},\"error\":{},\"strike\":{},\"until_cycle\":{},\"blocked\":{},\"since_unix_ms\":{}}}",
                    quote(&q.account),
                    quote(&q.error),
                    q.strike,
                    q.until_cycle,
                    q.blocked,
                    q.since_unix_ms
                )
            })
            .collect();
        let _ = write!(s, "\"quarantined\":[{}],", q.join(","));
        let opt = |v: Option<u64>| v.map(|v| v.to_string()).unwrap_or_else(|| "null".into());
        let _ = write!(
            s,
            "\"last_progress_slot\":{},\"last_progress_cycle\":{}}}",
            opt(self.last_progress_slot),
            opt(self.last_progress_cycle)
        );
        s
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let v = json::parse(text)?;
        let req = |o: &Value, k: &str| o.u64_at(k).ok_or_else(|| format!("missing/invalid `{k}`"));
        let version = req(&v, "version")?;
        if version != STATUS_VERSION {
            return Err(format!("unsupported status version {version}"));
        }
        let lc = v.get("last_cycle").ok_or("missing `last_cycle`")?;
        let t = v.get("totals").ok_or("missing `totals`")?;
        let cu_window = v
            .get("cu_window")
            .and_then(Value::as_array)
            .ok_or("missing `cu_window`")?
            .iter()
            .map(|x| x.as_u64().ok_or_else(|| "bad cu sample".to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let quarantined = v
            .get("quarantined")
            .and_then(Value::as_array)
            .ok_or("missing `quarantined`")?
            .iter()
            .map(|q| {
                Ok(QuarantinedAccount {
                    account: q
                        .get("account")
                        .and_then(Value::as_str)
                        .ok_or("bad account")?
                        .to_string(),
                    error: q
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    strike: req(q, "strike")? as u32,
                    until_cycle: req(q, "until_cycle")?,
                    blocked: q.get("blocked") == Some(&Value::Bool(true)),
                    since_unix_ms: req(q, "since_unix_ms")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            version,
            written_unix_ms: req(&v, "written_unix_ms")?,
            market: v
                .get("market")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            cycle: req(&v, "cycle")?,
            slot: req(&v, "slot")?,
            last_cycle: LastCycle {
                progressed: req(lc, "progressed")?,
                idle: req(lc, "idle")?,
                failed: req(lc, "failed")?,
                quarantined_skipped: req(lc, "quarantined_skipped")?,
                budget_deferred: req(lc, "budget_deferred")?,
                newly_quarantined: req(lc, "newly_quarantined")?,
                cu_spent: req(lc, "cu_spent")?,
                max_attempt_cu: req(lc, "max_attempt_cu")?,
            },
            totals: Totals {
                cycles: req(t, "cycles")?,
                cycles_progress: req(t, "cycles_progress")?,
                cycles_idle: req(t, "cycles_idle")?,
                cycles_failed: req(t, "cycles_failed")?,
                attempts_progress: req(t, "attempts_progress")?,
                attempts_idle: req(t, "attempts_idle")?,
                attempts_failed: req(t, "attempts_failed")?,
            },
            cu_window,
            quarantined,
            last_progress_slot: v.get("last_progress_slot").and_then(Value::as_u64),
            last_progress_cycle: v.get("last_progress_cycle").and_then(Value::as_u64),
        })
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        Self::from_json(&text)
    }
}

/// Write `contents` to `path` atomically: a sibling temp file, then rename.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Keeper-side hook. Holds the cumulative state between cycles.
pub struct StatusWriter {
    path: PathBuf,
    status: KeeperStatus,
    cu: VecDeque<u64>,
    first_struck: HashMap<Pubkey, u64>,
}

impl StatusWriter {
    /// `Some` only when `CRANKER_STATUS_FILE` is set and non-empty.
    pub fn from_env(market: &Pubkey) -> Option<Self> {
        let path = std::env::var("CRANKER_STATUS_FILE")
            .ok()
            .filter(|p| !p.is_empty())?;
        println!("status file: {path} (CRANKER_STATUS_FILE)");
        Some(Self::new(PathBuf::from(path), market))
    }

    pub fn new(path: PathBuf, market: &Pubkey) -> Self {
        Self {
            path,
            status: KeeperStatus {
                version: STATUS_VERSION,
                market: market.to_string(),
                ..KeeperStatus::default()
            },
            cu: VecDeque::new(),
            first_struck: HashMap::new(),
        }
    }

    /// Fold one finished cycle into the status. `records` are this cycle's
    /// submissions only.
    pub fn observe(
        &mut self,
        summary: &CycleSummary,
        records: &[SubmissionRecord],
        quarantine: &Quarantine,
        now_ms: u64,
    ) -> &KeeperStatus {
        let s = &mut self.status;
        s.cycle = summary.cycle;
        s.slot = summary.slot;
        s.last_cycle = LastCycle {
            progressed: summary.progressed as u64,
            idle: summary.idle as u64,
            failed: summary.failed as u64,
            quarantined_skipped: summary.quarantined_skipped as u64,
            budget_deferred: summary.budget_deferred as u64,
            newly_quarantined: summary.newly_quarantined as u64,
            cu_spent: summary.cu_spent,
            max_attempt_cu: summary.max_attempt_cu,
        };
        let t = &mut s.totals;
        t.cycles += 1;
        t.attempts_progress += summary.progressed as u64;
        t.attempts_idle += summary.idle as u64;
        t.attempts_failed += summary.failed as u64;
        if summary.progressed > 0 {
            t.cycles_progress += 1;
            s.last_progress_slot = Some(summary.slot);
            s.last_progress_cycle = Some(summary.cycle);
        } else if summary.failed > 0 {
            t.cycles_failed += 1;
        } else {
            t.cycles_idle += 1;
        }
        for r in records {
            if let Some(cu) = r.outcome.program_cu() {
                self.cu.push_back(cu);
                if self.cu.len() > CU_WINDOW {
                    self.cu.pop_front();
                }
            }
        }
        s.cu_window = self.cu.iter().copied().collect();
        let struck = quarantine.struck_accounts();
        self.first_struck.retain(|k, _| struck.contains(k));
        s.quarantined = struck
            .iter()
            .filter_map(|pk| {
                let (error, strike, until_cycle) = quarantine.struck_entry(pk)?;
                let since = *self.first_struck.entry(*pk).or_insert(now_ms);
                Some(QuarantinedAccount {
                    account: pk.to_string(),
                    error: error.to_string(),
                    strike,
                    until_cycle,
                    blocked: quarantine.is_blocked(pk, summary.cycle + 1),
                    since_unix_ms: since,
                })
            })
            .collect();
        s.written_unix_ms = now_ms;
        &self.status
    }

    /// `observe` + atomic write. Errors are printed, never returned.
    pub fn record(
        &mut self,
        summary: &CycleSummary,
        records: &[SubmissionRecord],
        quarantine: &Quarantine,
    ) {
        let json = self
            .observe(summary, records, quarantine, now_unix_ms())
            .to_json();
        if let Err(e) = write_atomic(&self.path, &json) {
            eprintln!("status file write {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quarantine::{FailureReport, QuarantineConfig};

    fn summary(cycle: u64, slot: u64, progressed: usize, failed: usize) -> CycleSummary {
        CycleSummary {
            cycle,
            slot,
            progressed,
            failed,
            ..CycleSummary::default()
        }
    }

    #[test]
    fn writer_tracks_totals_progress_and_quarantine() {
        let market = Pubkey::new_unique();
        let stuck = Pubkey::new_unique();
        let mut q = Quarantine::new(QuarantineConfig::default());
        let mut w = StatusWriter::new(PathBuf::from("unused"), &market);
        w.observe(&summary(0, 100, 2, 0), &[], &q, 1_000);
        for c in 1..=3 {
            q.record_failure(FailureReport {
                account: &stuck,
                error: "InjectedFault",
                cycle: c,
                slot: 100 + c,
                plan_class: "test",
            });
        }
        let s = w.observe(&summary(3, 130, 0, 1), &[], &q, 5_000).clone();
        assert_eq!(s.totals.cycles, 2);
        assert_eq!((s.totals.cycles_progress, s.totals.cycles_failed), (1, 1));
        assert_eq!(s.last_progress_slot, Some(100));
        assert_eq!(s.quarantined.len(), 1);
        assert_eq!(s.quarantined[0].error, "InjectedFault");
        assert_eq!(s.quarantined[0].since_unix_ms, 5_000);
        assert!(s.quarantined[0].blocked);
        // `since` is sticky while the account stays struck.
        let s2 = w.observe(&summary(4, 140, 0, 0), &[], &q, 9_000).clone();
        assert_eq!(s2.quarantined[0].since_unix_ms, 5_000);
        assert_eq!(s2.totals.cycles_idle, 1);
        q.record_ok(&stuck);
        assert!(w
            .observe(&summary(5, 150, 1, 0), &[], &q, 9_500)
            .quarantined
            .is_empty());
    }

    #[test]
    fn json_round_trip() {
        let s = KeeperStatus {
            version: STATUS_VERSION,
            written_unix_ms: 1,
            market: "M".into(),
            cycle: 7,
            slot: 99,
            last_cycle: LastCycle {
                progressed: 1,
                cu_spent: 18_000,
                ..LastCycle::default()
            },
            totals: Totals {
                cycles: 8,
                attempts_failed: 3,
                ..Totals::default()
            },
            cu_window: vec![18_000, 300_000],
            quarantined: vec![QuarantinedAccount {
                account: "A".into(),
                error: "Engine\"X".into(),
                strike: 2,
                until_cycle: 12,
                blocked: true,
                since_unix_ms: 5,
            }],
            last_progress_slot: Some(90),
            last_progress_cycle: None,
        };
        assert_eq!(KeeperStatus::from_json(&s.to_json()).unwrap(), s);
        assert!(KeeperStatus::from_json("{\"version\":2}").is_err());
    }

    #[test]
    fn atomic_write_replaces_file() {
        let dir = std::env::temp_dir().join(format!("status-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("status.json");
        write_atomic(&p, "one").unwrap();
        write_atomic(&p, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        let leftovers = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(leftovers, 1, "no temp file left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
