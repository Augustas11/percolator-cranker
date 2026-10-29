//! Landed crank statistics; idle is alive, but excluded from the success denominator.
use serde_json::{json, Value};

pub fn cranks_and_uptime(
    rows: &[(String, Option<i64>)],
    now: i64,
    cycle: u64,
    slack: u64,
) -> (Value, Value) {
    let count = |kind: &str| rows.iter().filter(|r| r.0 == kind).count();
    let (ok, idle, failed) = (count("ok"), count("idle"), count("failed"));
    let first = rows.iter().filter_map(|r| r.1).min();
    let last = rows.iter().filter_map(|r| r.1).max();
    let cranks = json!({"total":rows.len(),"ok":ok,"idle":idle,"failed":failed,"reverted":count("reverted"),
        "success_rate": (ok+failed > 0).then(|| ok as f64 / (ok+failed) as f64),"first_at":first,"last_at":last});
    let mut times: Vec<_> = rows
        .iter()
        .filter(|r| r.0 == "ok" || r.0 == "idle")
        .filter_map(|r| r.1)
        .filter(|t| *t <= now)
        .collect();
    times.sort_unstable();
    times.dedup();
    let start = times.first().copied();
    let window = start.map(|t| now.saturating_sub(t) as u64);
    let mut missed = 0u64;
    let mut longest = 0u64;
    let mut gaps = 0u64;
    let threshold = cycle.saturating_mul(slack);
    if start.is_some() {
        times.push(now);
        for pair in times.windows(2) {
            let g = pair[1].saturating_sub(pair[0]) as u64;
            longest = longest.max(g);
            if g > threshold {
                missed = missed.saturating_add(g - threshold);
                gaps += 1;
            }
        }
    }
    let uptime = json!({"cycle_secs":cycle,"window_start":start,"window_end":now,
        "covered_secs":window.map(|w| w.saturating_sub(missed)),"missed_secs":window.map(|_|missed),
        "uptime_pct":window.filter(|w| *w > 0).map(|w|100.0*(1.0-missed as f64/w as f64)),
        "longest_gap_secs":start.map(|_|longest),"gaps_over_slack":start.map(|_|gaps)});
    (cranks, uptime)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_and_single_instant_have_null_rates() {
        let (c, u) = cranks_and_uptime(&[], 100, 3, 10);
        assert!(c["success_rate"].is_null());
        for k in [
            "window_start",
            "missed_secs",
            "covered_secs",
            "uptime_pct",
            "longest_gap_secs",
            "gaps_over_slack",
        ] {
            assert!(u[k].is_null(), "{k}");
        }
        assert!(
            cranks_and_uptime(&[("ok".into(), Some(100))], 100, 3, 10).1["uptime_pct"].is_null()
        );
    }
    #[test]
    fn reverted_is_neither_success_denominator_nor_liveness() {
        let rows = [
            ("ok".into(), Some(0)),
            ("failed".into(), Some(3)),
            ("reverted".into(), Some(60)),
        ];
        let (c, u) = cranks_and_uptime(&rows, 90, 3, 10);
        assert_eq!(c["reverted"], 1);
        assert_eq!(c["success_rate"], 0.5);
        assert_eq!(u["missed_secs"], 60);
        assert!(
            cranks_and_uptime(&[("reverted".into(), Some(1))], 90, 3, 10).1["window_start"]
                .is_null()
        );
    }
    #[test]
    fn steady_including_idle_and_ten_minute_hole() {
        let rows = vec![
            ("ok".into(), Some(0)),
            ("idle".into(), Some(3)),
            ("ok".into(), Some(6)),
        ];
        let (c, u) = cranks_and_uptime(&rows, 9, 3, 10);
        assert_eq!(c["success_rate"], 1.0);
        assert_eq!(u["uptime_pct"], 100.0);
        let (_, u) = cranks_and_uptime(&rows, 606, 3, 10);
        assert_eq!(u["missed_secs"], 570);
        assert_eq!(u["covered_secs"], 36);
        assert_eq!(u["longest_gap_secs"], 600);
        assert_eq!(u["gaps_over_slack"], 1);
        assert!((u["uptime_pct"].as_f64().unwrap() - 100.0 * 36.0 / 606.0).abs() < 1e-10);
    }
}
