//! OpenRouter price recorder: saves the benchmark model's endpoint listing
//! every 5 minutes, so mark-price rules can later be scored on real history.
//!
//! One file per UTC day, `<out dir>/YYYY-MM-DD.jsonl`, append-only. Each line is
//! one snapshot: `at_ms` (when the reply arrived, as the pusher stamps it),
//! the parsed `rows` (null if the reply did not parse, with `error`), and the
//! `raw` reply text, byte for byte. Uses the pusher's client and parser.
//! Failed fetches write nothing; `record-or-status` shows them as gaps.
//!
//! `staging record-or <config> <out dir>` runs it; `staging record-or-status
//! <out dir>` prints snapshots and rows per day and any gap over 15 minutes.

use super::flop::{fetch_or_text, http_client, record_rows};
use super::*;
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::Path;

const EVERY_MS: u64 = 300_000;
/// A snapshot this late is a gap: alert.
pub(super) const GAP_MS: u64 = 15 * 60_000;

fn day(at_ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(at_ms as i64).map(|t| t.format("%Y-%m-%d").to_string()).unwrap_or_else(|| "bad-date".into())
}

/// One snapshot line.
fn line(at_ms: u64, url: &str, text: &str) -> String {
    let mut v = serde_json::json!({ "at_ms": at_ms, "url": url, "rows": null, "raw": text });
    match record_rows(text) {
        Ok(rows) => v["rows"] = serde_json::to_value(rows).unwrap_or_default(),
        Err(e) => v["error"] = format!("{e:#}").into(),
    }
    let mut s = v.to_string();
    s.push('\n');
    s
}

/// Snapshot times in the files, oldest first, with per-day counts.
struct Day {
    name: String,
    snapshots: usize,
    rows: usize,
    unparsed: usize,
}

fn read_dir(dir: &Path) -> Result<(Vec<Day>, Vec<u64>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("read {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    let (mut days, mut ats) = (Vec::new(), Vec::new());
    for f in files {
        let mut d = Day { name: f.file_stem().unwrap_or_default().to_string_lossy().into(), snapshots: 0, rows: 0, unparsed: 0 };
        for l in std::io::BufReader::new(std::fs::File::open(&f)?).split(b'\n') {
            let l = l?;
            // A torn line (crash or full disk mid-write) is skipped, not fatal.
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&l) else { continue };
            let Some(at) = v["at_ms"].as_u64() else { continue };
            d.snapshots += 1;
            match v["rows"].as_array() {
                Some(r) => d.rows += r.len(),
                None => d.unparsed += 1,
            }
            ats.push(at);
        }
        days.push(d);
    }
    ats.sort_unstable();
    Ok((days, ats))
}

/// Spans between consecutive snapshots longer than GAP_MS.
pub(super) fn gaps(ats: &[u64]) -> Vec<(u64, u64)> {
    ats.windows(2).filter(|w| w[1] - w[0] > GAP_MS).map(|w| (w[0], w[1])).collect()
}

/// The newest snapshot time: last parseable line of the newest file only.
fn last_at(dir: &Path) -> Result<Option<u64>> {
    let newest = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .max();
    let Some(f) = newest else { return Ok(None) };
    Ok(std::io::BufReader::new(std::fs::File::open(f)?)
        .split(b'\n')
        .filter_map(|l| serde_json::from_slice::<serde_json::Value>(&l.ok()?).ok()?["at_ms"].as_u64())
        .last())
}

/// Append one line; first end a torn line left by a crash, so the new one parses.
fn append(path: &Path, line: &str) -> Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).read(true).append(true).open(path)?;
    if f.metadata()?.len() > 0 {
        let mut last = [0u8];
        f.seek(SeekFrom::End(-1))?;
        f.read_exact(&mut last)?;
        if last[0] != b'\n' {
            f.write_all(b"\n")?;
        }
    }
    f.write_all(line.as_bytes())?;
    Ok(())
}

fn fmt_ms(at_ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(at_ms as i64).map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default()
}

pub fn or_record(args: &[String]) -> Result<()> {
    if args.len() != 2 {
        bail!("usage: staging record-or <config> <out dir>");
    }
    let cfg = launch::load(&args[0])?;
    let url = cfg.oracle.or_url.clone();
    if !url.starts_with("https://") {
        bail!("or_url must be https");
    }
    let dir = Path::new(&args[1]);
    std::fs::create_dir_all(dir)?;
    let client = http_client()?;
    let mut last_ok = last_at(dir).unwrap_or_else(|e| {
        eprintln!("or-record: could not read the last snapshot time: {e:#}");
        None
    });
    let mut alerted = false;
    eprintln!("or-record: {url} every {} s into {}", EVERY_MS / 1000, dir.display());
    loop {
        let saved = fetch_or_text(&client, &url).and_then(|text| {
            let now = launch::now_ms();
            append(&dir.join(format!("{}.jsonl", day(now))), &line(now, &url, &text)).context("write failed")?;
            Ok(now)
        });
        let now = launch::now_ms();
        match saved {
            Ok(at) => {
                if let Some(prev) = last_ok.filter(|p| at.saturating_sub(*p) > GAP_MS) {
                    eprintln!("or-record alert: gap of {} min ended (last snapshot {})", at.saturating_sub(prev) / 60_000, fmt_ms(prev));
                }
                (last_ok, alerted) = (Some(at), false);
            }
            Err(e) => {
                eprintln!("or-record: {e:#}");
                // No snapshot yet this run and none on disk: count from now.
                let prev = *last_ok.get_or_insert(now);
                if !alerted && now.saturating_sub(prev) > GAP_MS {
                    eprintln!("or-record alert: no snapshot for {} min (last {}): {e:#}", now.saturating_sub(prev) / 60_000, fmt_ms(prev));
                    alerted = true;
                }
            }
        }
        // Next 5-minute mark of the wall clock.
        let now = launch::now_ms();
        std::thread::sleep(Duration::from_millis(EVERY_MS - now % EVERY_MS));
    }
}

/// Per-day counts and gaps; fails if the last snapshot is over 15 minutes old.
pub fn or_record_status(args: &[String]) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: staging record-or-status <out dir>");
    }
    let (days, ats) = read_dir(Path::new(&args[0]))?;
    println!("{:<12} {:>9} {:>7} {:>9}", "day", "snapshots", "rows", "unparsed");
    for d in &days {
        println!("{:<12} {:>9} {:>7} {:>9}", d.name, d.snapshots, d.rows, d.unparsed);
    }
    for (a, b) in gaps(&ats) {
        println!("gap {} min: {} to {}", (b - a) / 60_000, fmt_ms(a), fmt_ms(b));
    }
    let last = *ats.last().context("no snapshots")?;
    let age = launch::now_ms().saturating_sub(last);
    println!("last snapshot {} ({} min ago)", fmt_ms(last), age / 60_000);
    if age > GAP_MS {
        bail!("recorder stale: last snapshot {} min ago", age / 60_000);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"{"data":{"id":"qwen/qwen3.8-27b","endpoints":[
        {"provider_name":"DeepInfra","tag":"deepinfra/bf16","model_id":"qwen/qwen3.8-27b","quantization":"bf16",
         "pricing":{"prompt":"0.0000003","completion":"0.0000024"},"status":0},
        {"provider_name":"Odd","pricing":{"prompt":"n/a","completion":"0.000001"}}]}}"#;

    #[test]
    fn a_snapshot_keeps_raw_and_every_row() {
        let v: serde_json::Value = serde_json::from_str(&line(1_762_000_000_000, "u", REPLY)).unwrap();
        assert_eq!(v["raw"], REPLY);
        let rows = v["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["endpoint"], "deepinfra/bf16");
        assert_eq!(rows[0]["prompt"], 0.0000003);
        assert_eq!(rows[0]["status"], 0);
        // An unparseable price is kept as null, not dropped.
        assert!(rows[1]["prompt"].is_null() && rows[1]["status"].is_null());
    }

    #[test]
    fn a_bad_reply_is_saved_raw_with_the_error() {
        let v: serde_json::Value = serde_json::from_str(&line(1, "u", "<html>busy</html>")).unwrap();
        assert!(v["rows"].is_null());
        assert_eq!(v["raw"], "<html>busy</html>");
        assert!(v["error"].as_str().unwrap().contains("unexpected endpoints reply"));
    }

    #[test]
    fn gaps_are_spans_over_fifteen_minutes() {
        let m = 60_000;
        assert_eq!(gaps(&[0, 5 * m, 20 * m, 36 * m, 41 * m]), vec![(20 * m, 36 * m)]);
    }

    #[test]
    fn status_counts_days_and_skips_a_torn_line() {
        let dir = std::env::temp_dir().join(format!("or-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("2025-11-01.jsonl");
        let mut body = line(1_762_000_000_000, "u", REPLY).into_bytes();
        body.extend(line(1_762_000_300_000, "u", "busy").into_bytes());
        // Torn mid-way through a multibyte character, as a full disk leaves it.
        body.extend_from_slice(b"{\"at_ms\":17620,\"raw\":\"\xc3");
        std::fs::write(&f, body).unwrap();
        // The next append ends the torn line first, so it still parses.
        append(&f, &line(1_762_000_600_000, "u", REPLY)).unwrap();
        let (days, ats) = read_dir(&dir).unwrap();
        let last = last_at(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!((days.len(), days[0].snapshots, days[0].rows, days[0].unparsed), (1, 3, 4, 1));
        assert_eq!(ats, vec![1_762_000_000_000, 1_762_000_300_000, 1_762_000_600_000]);
        assert_eq!(last, Some(1_762_000_600_000));
        assert_eq!(day(1_762_000_000_000), "2025-11-01");
    }
}
