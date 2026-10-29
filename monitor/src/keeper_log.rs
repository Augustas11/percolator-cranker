use anyhow::Result;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
    time::SystemTime,
};

#[derive(Default)]
pub struct KeeperLog {
    offset: u64,
    observed_len: u64,
    pub market: Option<String>,
    pub signature: Option<String>,
    pub last_success_slot: Option<u64>,
    pub first_seen_slot: Option<u64>,
    /// Pusher: source price timestamp to push landing, ms (staging `mark_lag` event).
    mark_lag_ms: Option<u64>,
    mark_lags: VecDeque<u64>,
    /// Unix ms when that push landed: a stopped pusher shows as a growing age.
    mark_lag_at_ms: Option<u64>,
    /// Unix ms when this log was first read: before any push lands, the push
    /// age counts from here, so a pusher that never succeeds still alerts.
    watch_start_ms: Option<u64>,
    last_cycle: u64,
    summary: BTreeMap<String, u64>,
    attempts: [u64; 3],
    cycles: [u64; 3],
    cu: VecDeque<u64>,
    quarantine: BTreeMap<String, Value>,
    events: BTreeMap<String, Value>,
}

pub fn cu_stats(values: &VecDeque<u64>) -> (u64, u64) {
    let mut sorted: Vec<_> = values.iter().copied().collect();
    sorted.sort_unstable();
    if sorted.is_empty() {
        (0, 0)
    } else {
        (sorted[(sorted.len() - 1) / 2], *sorted.last().unwrap())
    }
}

/// Nearest-rank percentiles over the most recent 300 landed pushes.
fn lag_percentiles(values: &VecDeque<u64>) -> (Option<u64>, Option<u64>) {
    let mut sorted: Vec<_> = values.iter().copied().collect();
    sorted.sort_unstable();
    if sorted.is_empty() { return (None, None); }
    (Some(sorted[(sorted.len() * 50 + 99) / 100 - 1]),
     Some(sorted[(sorted.len() * 95 + 99) / 100 - 1]))
}

impl KeeperLog {
    pub fn line(&mut self, line: &str) {
        self.watch_start_ms.get_or_insert_with(|| {
            SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
        });
        if let Some((left, right)) = line.split_once('=') {
            if left.starts_with("market") && left.trim_end() == "market" && left.len() > 6 {
                if let Ok(pk) = right.trim().parse::<solana_sdk::pubkey::Pubkey>() {
                    self.market = Some(pk.to_string());
                }
            }
        }
        if line.starts_with("cycle ") {
            if let Some((cycle, fields)) = line[6..].split_once(" summary: ") {
                if let Ok(n) = cycle.parse() {
                    let summary: BTreeMap<String, u64> = fields
                        .split_whitespace()
                        .filter_map(|s| {
                            let (k, v) = s.split_once('=')?;
                            Some((k.into(), v.parse().ok()?))
                        })
                        .collect();
                    if ["progressed", "idle", "failed", "cu"]
                        .iter()
                        .all(|k| summary.contains_key(*k))
                    {
                        self.last_cycle = n;
                        let counts = [summary["progressed"], summary["idle"], summary["failed"]];
                        for (total, n) in self.attempts.iter_mut().zip(counts) {
                            *total = total.saturating_add(n);
                        }
                        let class = if counts[0] > 0 {
                            0
                        } else if counts[2] > 0 {
                            2
                        } else {
                            1
                        };
                        self.cycles[class] += 1;
                        self.summary = summary;
                    }
                }
            }
        }
        if line.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                match v["event"].as_str() {
                    Some("quarantine") => {
                        if let (Some(account), Some(slot)) =
                            (v["account"].as_str(), v["slot"].as_u64())
                        {
                            let first = self
                                .quarantine
                                .get(account)
                                .and_then(|q| q["first_strike_slot"].as_u64())
                                .unwrap_or(slot);
                            self.quarantine.insert(account.into(),json!({"account":account,"first_strike_slot":first,
                                "error":v["error"],"strike":v["strike"],"until_cycle":v["until_cycle"],"last_event_slot":slot}));
                        }
                    }
                    Some("mark_lag") => {
                        self.mark_lag_ms = v["ms"].as_u64();
                        self.mark_lag_at_ms = v["at_ms"].as_u64();
                        if let Some(lag) = self.mark_lag_ms {
                            if self.mark_lags.len() == 300 { self.mark_lags.pop_front(); }
                            self.mark_lags.push_back(lag);
                        }
                    }
                    Some("unconfirmed" | "idle_with_pending_work") => {
                        self.events.insert(v["event"].as_str().unwrap().into(), v);
                    }
                    _ => {}
                }
            }
        }
        if let Some((account_class, outcome)) = line.trim_start().split_once(" ← ") {
            let Some((account, _)) = account_class.split_once(" [") else {
                return;
            };
            if outcome.starts_with("Ok(") {
                if let Some(tail) = outcome.split_once("program_cu=Some(").map(|(_, s)| s) {
                    if let Some(n) = tail
                        .split_once(')')
                        .and_then(|(n, _)| n.parse::<u64>().ok())
                    {
                        self.cu.push_back(n);
                        if self.cu.len() > 256 {
                            self.cu.pop_front();
                        }
                    }
                }
                // Any landed crank clears drip's strikes: Progress via `record_ok`,
                // Idle via `record_idle` (quarantine.rs). It also counts for liveness.
                self.quarantine.remove(account);
                if let Some(sig) = outcome
                    .split_once("sig=")
                    .and_then(|(_, s)| s.split_whitespace().next())
                {
                    self.signature = Some(sig.into());
                }
            } else if let Some((name, error)) = outcome
                .strip_prefix("HardReject (")
                .and_then(|s| s.split_once("): "))
            {
                // drip classes `EngineNonProgress` as Idle (`submit::is_non_progress`),
                // which clears strikes; any other reject is a failure: keep its error.
                if drip::submit::is_non_progress(Some(name)) {
                    self.quarantine.remove(account);
                } else if let Some(q) = self.quarantine.get_mut(account) {
                    q["error"] = json!(error);
                }
            }
        }
    }

    pub fn read(&mut self, path: &Path) -> Result<f64> {
        let mut file = File::open(path)?;
        let meta = file.metadata()?;
        if meta.len() < self.observed_len {
            *self = Self::default();
        }
        self.observed_len = meta.len();
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new(file);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            let n = reader.read_until(b'\n', &mut bytes)?;
            if n == 0 || bytes.last() != Some(&b'\n') {
                break;
            }
            self.line(String::from_utf8_lossy(&bytes).trim_end_matches(['\n', '\r']));
            self.offset += n as u64;
        }
        Ok(SystemTime::now()
            .duration_since(meta.modified()?)
            .unwrap_or_default()
            .as_secs_f64())
    }

    pub fn snapshot(&self, slot: u64, ok: bool, age: Option<f64>, error: Option<String>) -> Value {
        let (p50, max) = cu_stats(&self.cu);
        let outcomes = |v: [u64; 3]| json!({"progress":v[0],"idle":v[1],"failed":v[2]});
        let mut last = outcomes([
            *self.summary.get("progressed").unwrap_or(&0),
            *self.summary.get("idle").unwrap_or(&0),
            *self.summary.get("failed").unwrap_or(&0),
        ]);
        for k in ["quarantined_skipped", "deferred"] {
            last[k] = json!(self.summary.get(k).copied().unwrap_or(0));
        }
        let rows: Vec<_> = self
            .quarantine
            .values()
            .map(|q| {
                let mut q = q.clone();
                q["slots"] =
                    json!(slot.saturating_sub(q["first_strike_slot"].as_u64().unwrap_or(slot)));
                q
            })
            .collect();
        let (lag_p50, lag_p95) = lag_percentiles(&self.mark_lags);
        json!({"mark_lag_p50_ms":lag_p50,"mark_lag_p95_ms":lag_p95,"log_ok":ok,"log_age_seconds":age,"log_error":error,"last_cycle":self.last_cycle,
            "last_cycle_summary":self.summary,"last_cycle_attempts":last,"last_cycle_cu":self.summary.get("cu").copied().unwrap_or(0),
            "attempts_total":outcomes(self.attempts),"cycles_total":outcomes(self.cycles),"crank_cu":{"p50":p50,"max":max},
            "quarantined_accounts":rows,"last_success_signature":self.signature,"last_success_slot":self.last_success_slot,
            // Before any known success, count from the first slot this log was watched,
            // so a keeper failing from its start still trips the no-success alert.
            "slots_since_last_success":self.last_success_slot.or(self.first_seen_slot).map(|s|slot.saturating_sub(s)),"last_events":self.events,"mark_lag_ms":self.mark_lag_ms,
            "mark_push_age_seconds":self.mark_lag_at_ms.or(self.watch_start_ms).map(|at| {
                let now = SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
                now.saturating_sub(at) / 1000
            })})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rolling_lag_percentiles() {
        let mut k = KeeperLog::default();
        assert_eq!(lag_percentiles(&k.mark_lags), (None, None));
        k.line(r#"{"event":"mark_lag","ms":9999,"at_ms":1}"#);
        assert_eq!(lag_percentiles(&k.mark_lags), (Some(9999), Some(9999)));
        for ms in (1..=300).rev() {
            k.line(&format!("{{\"event\":\"mark_lag\",\"ms\":{ms},\"at_ms\":1}}"));
        }
        assert_eq!(k.mark_lags.len(), 300);
        assert_eq!(lag_percentiles(&k.mark_lags), (Some(150), Some(285)));
    }
    const SUMMARY: &str="cycle 7 summary: progressed=2 idle=3 failed=1 unconfirmed=0 quarantined_skipped=4 deferred=5 new_quarantine=1 idle_pending_stuck=0 txs=6 cu=100 max_attempt_cu=50";
    #[test]
    fn literal_logs_totals_quarantine_and_progress() {
        let mut k = KeeperLog::default();
        k.line(SUMMARY);
        k.line(r#"{"event":"quarantine","account":"11111111111111111111111111111111","error":"EngineCounterUnderflow","cycle":7,"slot":100,"plan_class":"live:pending","strike":1,"backoff_cycles":2,"until_cycle":10}"#);
        k.line(r#"{"event":"quarantine","account":"11111111111111111111111111111111","error":"EngineCounterUnderflow","cycle":10,"slot":110,"strike":2,"until_cycle":15}"#);
        k.line("  11111111111111111111111111111111 [live:pending] ← HardReject (EngineCounterUnderflow): error");
        assert_eq!(
            k.snapshot(120, true, None, None)["quarantined_accounts"][0]["slots"],
            20
        );
        // An idle landed crank clears the strike, as drip's `record_idle` does.
        k.line("  11111111111111111111111111111111 [live:pending] ← Ok(no-mut): sig=idle target_diff=0 reward_diff=Some(0) program_cu=None");
        assert!(k.quarantine.is_empty());
        assert_eq!(k.signature.as_deref(), Some("idle"));
        k.line(r#"{"event":"quarantine","account":"11111111111111111111111111111111","error":"E","cycle":11,"slot":115,"strike":1,"until_cycle":13}"#);
        k.line("  11111111111111111111111111111111 [live:pending] ← Ok(mut): sig=progress target_diff=1 reward_diff=Some(1) program_cu=Some(42)");
        assert!(k.quarantine.is_empty());
        assert_eq!(k.signature.as_deref(), Some("progress"));
        k.line("  11111111111111111111111111111111 [live:pending] ← Ok(unreconciled): sig=later target_diff=0 reward_diff=None program_cu=Some(10)");
        assert_eq!(k.signature.as_deref(), Some("later"));
        assert_eq!(k.attempts, [2, 3, 1]);
        assert_eq!(k.cycles, [1, 0, 0]);
        assert_eq!(cu_stats(&k.cu), (10, 42));
        k.line(&SUMMARY.replace("progressed=2", "progressed=0"));
        k.line(
            &SUMMARY
                .replace("progressed=2", "progressed=0")
                .replace("failed=1", "failed=0"),
        );
        assert_eq!(k.cycles, [1, 1, 1]);
        // The staging pusher's lag event (Day-0 launch, docs/LAUNCH.md).
        k.line(r#"{"event":"mark_lag","ms":512,"target_ms":400,"at_ms":1000}"#);
        let s = k.snapshot(120, true, None, None);
        assert_eq!(s["mark_lag_ms"], 512);
        // A push long ago: the age grows, so a stopped pusher alerts.
        assert!(s["mark_push_age_seconds"].as_u64().unwrap() > 1_000_000);
    }
    #[test]
    fn non_progress_reject_clears_other_rejects_update_error_and_no_success_baseline() {
        let pk = "11111111111111111111111111111111";
        let q = format!(r#"{{"event":"quarantine","account":"{pk}","error":"A","cycle":1,"slot":10,"strike":1,"until_cycle":3}}"#);
        let mut k = KeeperLog::default();
        k.line(&q);
        k.line(&format!("  {pk} [live] ← HardReject (Custom): B"));
        assert_eq!(k.quarantine[pk]["error"], "B");
        k.line(&format!("  {pk} [live] ← HardReject (EngineNonProgress): idle"));
        assert!(k.quarantine.is_empty());
        // Failing from the start: no success yet, measured from the first watched slot.
        k.first_seen_slot = Some(100);
        assert_eq!(k.snapshot(900, true, None, None)["slots_since_last_success"], 800);
        k.last_success_slot = Some(850);
        assert_eq!(k.snapshot(900, true, None, None)["slots_since_last_success"], 50);
    }
    #[test]
    fn tail_partial_line_and_shrink_reset() {
        let path = std::env::temp_dir().join(format!("monitor-log-{}", std::process::id()));
        std::fs::write(&path, format!("{SUMMARY}\ncycle 8 summary: progressed=1")).unwrap();
        let mut k = KeeperLog::default();
        k.read(&path).unwrap();
        assert_eq!(k.last_cycle, 7);
        k.read(&path).unwrap();
        assert_eq!(k.attempts, [2, 3, 1]);
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b" idle=0 failed=0 cu=5\n")
            .unwrap();
        k.read(&path).unwrap();
        assert_eq!(k.last_cycle, 8);
        assert_eq!(k.attempts, [3, 3, 1]);
        std::fs::write(
            &path,
            "market            = 11111111111111111111111111111111\n",
        )
        .unwrap();
        k.read(&path).unwrap();
        assert_eq!(k.attempts, [0, 0, 0]);
        assert_eq!(
            k.market.as_deref(),
            Some("11111111111111111111111111111111")
        );
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn nearest_rank_and_window_limit() {
        assert_eq!(cu_stats(&VecDeque::new()), (0, 0));
        assert_eq!(cu_stats(&VecDeque::from([5, 1, 9])), (5, 9));
        let mut k = KeeperLog::default();
        for n in 0..300 {
            k.line(&format!(
                "  account [class] ← Ok(no-mut): sig=s program_cu=Some({n})"
            ));
        }
        assert_eq!(k.cu.len(), 256);
        assert_eq!(cu_stats(&k.cu), (171, 299));
    }
}
