//! The public compute-price feed (docs/flop-compute/FEED.md): turns the
//! pusher's `flop_push` lines and the OpenRouter recorder's daily files into
//! `latest.json` and `history.json` for the site's /compute page. Read-only;
//! it does not verify signatures (`staging verify-push-log` does that).

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// History covers the last 7 days.
pub const WINDOW_MS: u64 = 7 * 86_400_000;
/// At most this many chart points.
pub const MAX_POINTS: usize = 600;
/// At most this many mark changes (newest kept).
pub const MAX_CHANGES: usize = 500;
/// A recorder silence longer than this is a gap (as `record-or-status`).
pub const GAP_MS: u64 = 15 * 60_000;

#[derive(Clone, Debug, PartialEq)]
struct Push {
    landed_ms: u64,
    mark_e6: u64,
    sig: String,
}

/// The push log, read incrementally: `update` reads only bytes after the
/// last complete line it saw, so a loop reads the big log once.
#[derive(Default)]
pub struct PushLog {
    offset: u64,
    pushes: Vec<Push>,
    latest: Option<Value>,
}

fn has(line: &[u8], pat: &[u8]) -> bool {
    line.windows(pat.len()).any(|w| w == pat)
}

impl PushLog {
    /// Read new complete lines. A shorter file than last time (rotated or
    /// truncated) starts over. A last line without its newline is left for
    /// the next call.
    pub fn update(&mut self, path: &Path) -> Result<()> {
        let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let len = f.metadata()?.len();
        if len < self.offset {
            *self = Self::default();
        }
        f.seek(SeekFrom::Start(self.offset))?;
        let mut r = BufReader::new(f.take(len - self.offset));
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let n = r.read_until(b'\n', &mut buf)?;
            if n == 0 || buf.last() != Some(&b'\n') {
                break;
            }
            self.offset += n as u64;
            self.ingest(&buf);
        }
        Ok(())
    }

    fn ingest(&mut self, line: &[u8]) {
        if !has(line, b"\"event\":\"flop_push\"") {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
        if v["event"] != "flop_push" {
            return;
        }
        let (Some(landed_ms), Some(mark_e6)) = (v["landed_ms"].as_u64(), v["mark_e6"].as_u64()) else { return };
        let sig = v["sig"].as_str().unwrap_or_default().to_owned();
        self.pushes.push(Push { landed_ms, mark_e6, sig });
        if self.latest.as_ref().and_then(|l| l["landed_ms"].as_u64()).is_none_or(|t| landed_ms >= t) {
            self.latest = Some(v);
        }
    }

    /// Drop pushes older than the window (the latest line is kept apart).
    fn prune(&mut self, from_ms: u64) {
        self.pushes.retain(|p| p.landed_ms >= from_ms);
        self.pushes.sort_by_key(|p| p.landed_ms);
    }
}

#[derive(Deserialize)]
struct RecRow {
    provider: Option<String>,
    quantization: Option<String>,
    prompt: Option<f64>,
    completion: Option<f64>,
    status: Option<i64>,
}

#[derive(Deserialize)]
struct RecLine {
    at_ms: u64,
    #[serde(default)]
    rows: Option<Vec<RecRow>>,
}

/// One recorder snapshot: its time and each listed (provider, quantization)
/// with up/down; `None` if the reply did not parse.
#[derive(Clone, Debug)]
struct Snap {
    at_ms: u64,
    sellers: Option<Vec<(String, String, bool)>>,
}

/// The recorder directory, cached per file by size, so a loop re-reads
/// only today's growing file.
#[derive(Default)]
pub struct Recorder {
    files: HashMap<PathBuf, (u64, Vec<Snap>)>,
}

fn read_snaps(path: &Path) -> Result<Vec<Snap>> {
    let mut out = Vec::new();
    for l in BufReader::new(File::open(path)?).split(b'\n') {
        // A torn or odd line is skipped, not fatal.
        let Ok(r) = serde_json::from_slice::<RecLine>(&l?) else { continue };
        let sellers = r.rows.map(|rows| {
            rows.into_iter()
                .filter_map(|row| {
                    let priced = matches!((row.prompt, row.completion), (Some(p), Some(c)) if p.is_finite() && c.is_finite() && p >= 0.0 && c >= 0.0 && p + c > 0.0);
                    Some((row.provider?, row.quantization.unwrap_or_else(|| "unknown".into()), priced && row.status == Some(0)))
                })
                .collect()
        });
        out.push(Snap { at_ms: r.at_ms, sellers });
    }
    Ok(out)
}

impl Recorder {
    /// Read the day files that can hold snapshots from `from_ms` on. A missing
    /// directory or day is no data, not an error.
    pub fn update(&mut self, dir: &Path, from_ms: u64) -> Result<()> {
        let first = day(from_ms);
        let Ok(entries) = std::fs::read_dir(dir) else {
            self.files.clear();
            return Ok(());
        };
        let mut keep = Vec::new();
        for p in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            if p.extension().is_none_or(|x| x != "jsonl") || stem.len() != 10 || stem < first {
                continue;
            }
            let Ok(len) = p.metadata().map(|m| m.len()) else { continue };
            if self.files.get(&p).is_none_or(|(l, _)| *l != len) {
                match read_snaps(&p) {
                    Ok(s) => {
                        self.files.insert(p.clone(), (len, s));
                    }
                    Err(e) => eprintln!("flop-feed: skip {}: {e:#}", p.display()),
                }
            }
            keep.push(p);
        }
        self.files.retain(|p, _| keep.contains(p));
        Ok(())
    }

    fn snaps(&self, from_ms: u64) -> Vec<&Snap> {
        let mut v: Vec<&Snap> = self.files.values().flat_map(|(_, s)| s).filter(|s| s.at_ms >= from_ms).collect();
        v.sort_by_key(|s| s.at_ms);
        v
    }
}

/// UTC day of a ms timestamp, `YYYY-MM-DD` (civil from days, H. Hinnant).
pub fn day(at_ms: u64) -> String {
    let z = (at_ms / 86_400_000) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

fn usd(e6: u64) -> f64 {
    e6 as f64 / 1e6
}

/// `latest.json` from the newest push line.
pub fn latest_json(line: &Value, now_ms: u64) -> Value {
    let inputs = &line["inputs"];
    let blend = inputs["blend_in_out"].as_f64().unwrap_or(crate::SPEC.blend);
    let tiers: Vec<String> = match inputs["quantizations"].as_array() {
        Some(a) => a.iter().filter_map(|q| q.as_str().map(String::from)).collect(),
        None => crate::SPEC.quantizations.iter().map(|q| q.to_string()).collect(),
    };
    let used: Vec<&Value> = inputs["used"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let key = |v: &Value| (v["provider"].as_str().unwrap_or_default().to_owned(), v["quantization"].as_str().unwrap_or_default().to_owned());
    let pricing_set: Vec<Value> = used
        .iter()
        .map(|u| json!({ "provider": u["provider"], "quantization": u["quantization"], "listed": u["listed"], "priced": u["priced"], "up": true }))
        .collect();
    let mut sellers: Vec<Value> = inputs["endpoints"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            let (p, c) = (e["prompt"].as_f64()?, e["completion"].as_f64()?);
            let blended = (blend * p + c) / (blend + 1.0) * 1e6;
            if !blended.is_finite() {
                return None;
            }
            let k = key(e);
            Some(json!({
                "provider": k.0, "quantization": k.1, "blended": blended,
                "up": e["status"].as_i64().unwrap_or(0) == 0,
                "in_set": used.iter().any(|u| key(u) == k),
                "tier": tiers.contains(&k.1),
            }))
        })
        .collect();
    sellers.sort_by(|a, b| a["blended"].as_f64().unwrap_or(0.0).total_cmp(&b["blended"].as_f64().unwrap_or(0.0)));
    let fs = &inputs["flop_side"];
    let win = &fs["window"];
    let flop_side = json!({
        "flop_per_gn": fs["flop_per_gn"].as_f64(),
        "prints": fs["prints"].as_u64().or(win["prints"].as_u64()),
        "buyers": fs["buyers"].as_u64().or(win["buyers"].as_u64()),
        "miners": fs["miners"].as_u64(),
        "gateway_share": win["gateway_gn_share"].as_f64(),
        "stale": if fs.is_object() { fs["stale"].as_str().map(String::from) } else { Some("not in the push line".to_owned()) },
    });
    json!({
        "v": 1,
        "generated_ms": now_ms,
        "mark_usd": line["mark_e6"].as_u64().map(usd),
        "landed_ms": line["landed_ms"],
        "source_ms": inputs["fetched_ms"].as_u64().or(line["published_ms"].as_u64()),
        "sig": line["sig"],
        "slot": line["slot"],
        "market": line["market"],
        "oracle": line["oracle"],
        "source": inputs["source"],
        "set": inputs["set"],
        "clamp": inputs["clamp"],
        "blend_in_out": blend,
        "pricing_set": pricing_set,
        "sellers": sellers,
        "flop_side": flop_side,
    })
}

/// At most `max` points, the last push in each equal time bucket.
fn downsample(pushes: &[Push], max: usize) -> Vec<&Push> {
    if pushes.len() <= max {
        return pushes.iter().collect();
    }
    let (t0, t1) = (pushes[0].landed_ms, pushes[pushes.len() - 1].landed_ms);
    let width = (t1 - t0) / max as u64 + 1;
    let mut by: BTreeMap<u64, &Push> = BTreeMap::new();
    for p in pushes {
        by.insert((p.landed_ms - t0) / width, p);
    }
    by.into_values().collect()
}

fn spans(snaps: &[&Snap], tiers: &[&str], now_ms: u64) -> (Vec<Value>, Vec<Value>) {
    // Per pricing-tier seller: down from the first snapshot it is down or
    // missing (after it was first listed) to the next one it is up.
    let mut open: BTreeMap<(String, String), Option<u64>> = BTreeMap::new();
    let mut outages = Vec::new();
    let mut gaps = Vec::new();
    let mut prev: Option<u64> = None;
    for s in snaps {
        if let Some(p) = prev.filter(|p| s.at_ms - p > GAP_MS) {
            gaps.push(json!({ "from_ms": p, "to_ms": s.at_ms }));
        }
        prev = Some(s.at_ms);
        let Some(sellers) = &s.sellers else { continue };
        for (prov, q, _) in sellers.iter().filter(|(_, q, _)| tiers.contains(&q.as_str())) {
            open.entry((prov.clone(), q.clone())).or_insert(None);
        }
        for (k, since) in open.iter_mut() {
            let up = sellers.iter().any(|(p, q, up)| *up && p == &k.0 && q == &k.1);
            match (up, *since) {
                (false, None) => *since = Some(s.at_ms),
                (true, Some(from)) => {
                    outages.push(json!({ "provider": k.0, "quantization": k.1, "from_ms": from, "to_ms": s.at_ms }));
                    *since = None;
                }
                _ => {}
            }
        }
    }
    for (k, since) in open {
        if let Some(from) = since {
            outages.push(json!({ "provider": k.0, "quantization": k.1, "from_ms": from, "to_ms": null }));
        }
    }
    if let Some(p) = prev.filter(|p| now_ms.saturating_sub(*p) > GAP_MS) {
        gaps.push(json!({ "from_ms": p, "to_ms": null }));
    }
    outages.sort_by_key(|o| o["from_ms"].as_u64());
    (outages, gaps)
}

/// `history.json`: the window's marks, outages and mark changes.
fn history_json(pushes: &[Push], snaps: &[&Snap], tiers: &[&str], from_ms: u64, now_ms: u64) -> Value {
    let points: Vec<Value> = downsample(pushes, MAX_POINTS).iter().map(|p| json!([p.landed_ms, usd(p.mark_e6)])).collect();
    let mut changes: Vec<Value> = pushes
        .windows(2)
        .filter(|w| w[0].mark_e6 != w[1].mark_e6)
        .map(|w| json!({ "t_ms": w[1].landed_ms, "from": usd(w[0].mark_e6), "to": usd(w[1].mark_e6), "sig": w[1].sig }))
        .collect();
    if changes.len() > MAX_CHANGES {
        changes.drain(..changes.len() - MAX_CHANGES);
    }
    let (outages, recorder_gaps) = spans(snaps, tiers, now_ms);
    json!({
        "v": 1, "generated_ms": now_ms, "from_ms": from_ms, "to_ms": now_ms,
        "points": points, "changes": changes, "outages": outages, "recorder_gaps": recorder_gaps,
    })
}

/// The feed's state across runs of `--loop`.
#[derive(Default)]
pub struct Feed {
    pub log: PushLog,
    pub recorder: Recorder,
}

impl Feed {
    /// Read what is new and build (`latest.json`, `history.json`).
    pub fn build(&mut self, log: &Path, or_dir: &Path, now_ms: u64) -> Result<(Value, Value)> {
        let from_ms = now_ms.saturating_sub(WINDOW_MS);
        self.log.update(log)?;
        self.log.prune(from_ms);
        self.recorder.update(or_dir, from_ms)?;
        let line = self.log.latest.as_ref().context("no flop_push line in the log yet")?;
        let latest = latest_json(line, now_ms);
        let tiers: Vec<&str> = match line["inputs"]["quantizations"].as_array() {
            Some(a) => a.iter().filter_map(Value::as_str).collect(),
            None => crate::SPEC.quantizations.to_vec(),
        };
        let history = history_json(&self.log.pushes, &self.recorder.snaps(from_ms), &tiers, from_ms, now_ms);
        Ok((latest, history))
    }
}

/// Write `<dir>/<name>` atomically: a temp file in the same directory, then rename.
pub fn write_atomic(dir: &Path, name: &str, v: &Value) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(v)?).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, dir.join(name))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // Shapes from the live log (2026-11-13), cut to a few endpoints.
    fn push(landed: u64, mark: u64) -> String {
        format!(
            r#"{{"event":"flop_push","inputs":{{"blend_in_out":15.0,"clamp":"off (n<3)","endpoints":[{{"completion":1.875e-6,"prompt":1.5e-7,"provider":"DeepInfra","quantization":"bf16","status":0}},{{"completion":3.2e-6,"prompt":4.5e-7,"provider":"GMICloud","quantization":"bf16","status":-2}},{{"completion":1.49e-6,"prompt":9.9e-7,"provider":"Cerebras","quantization":"fp16","status":0}},{{"completion":1.5e-6,"prompt":3.5e-8,"provider":"Darkbloom","quantization":"fp4","status":0}}],"fetched_ms":{f},"flop_side":{{"buyers":3,"flop_per_gn":0.002910360884749708,"miners":34,"prints":185,"window":{{"buyers":3,"gateway_gn_share":0.497988483079593,"prints":185}}}},"quantizations":["bf16","fp16"],"set":"preferred","source":"openrouter","used":[{{"listed":0.2578125,"priced":0.2578125,"provider":"DeepInfra","quantization":"bf16"}},{{"listed":1.02125,"priced":1.02125,"provider":"Cerebras","quantization":"fp16"}}]}},"landed_ms":{landed},"mark_e6":{mark},"market":"uZY6","oracle":"FLMp","published_ms":{f},"sig":"sig{landed}","slot":521759279,"v":1,"signature":"55881"}}"#,
            f = landed - 140_000
        )
    }

    fn rec(at: u64, gmi_status: i64) -> String {
        format!(
            r#"{{"at_ms":{at},"url":"u","rows":[{{"completion":1.875e-6,"endpoint":"deepinfra/bf16","model":"qwen/qwen3.8-27b","prompt":1.5e-7,"provider":"DeepInfra","quantization":"bf16","status":0}},{{"completion":3.2e-6,"prompt":4.5e-7,"provider":"GMICloud","quantization":"bf16","status":{gmi_status}}},{{"completion":1.5e-6,"prompt":3.5e-8,"provider":"Darkbloom","quantization":"fp4","status":-1}}],"raw":"{{\"data\":{{}}}}"}}"#
        )
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flop-feed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const NOW: u64 = 1_794_557_401_708;

    #[test]
    fn latest_has_mark_sellers_and_flop_side() {
        let v: Value = serde_json::from_str(&push(NOW, 303_551)).unwrap();
        let l = latest_json(&v, NOW);
        assert_eq!(l["mark_usd"], 0.303551);
        assert_eq!(l["sig"], format!("sig{NOW}"));
        assert_eq!(l["clamp"], "off (n<3)");
        assert_eq!(l["pricing_set"].as_array().unwrap().len(), 2);
        let s = l["sellers"].as_array().unwrap();
        assert_eq!(s.len(), 4);
        // Sorted by the 15:1 blend; DeepInfra bf16 is $0.2578.
        assert_eq!(s[0]["provider"], "Darkbloom");
        assert!((s[1]["blended"].as_f64().unwrap() - 0.2578125).abs() < 1e-9);
        let gmi = s.iter().find(|x| x["provider"] == "GMICloud").unwrap();
        assert_eq!((gmi["up"].as_bool(), gmi["in_set"].as_bool(), gmi["tier"].as_bool()), (Some(false), Some(false), Some(true)));
        assert_eq!(l["flop_side"]["miners"], 34);
        assert_eq!(l["flop_side"]["gateway_share"], 0.497988483079593);
        assert_eq!(l["flop_side"]["stale"], Value::Null);
    }

    #[test]
    fn a_stale_flop_side_keeps_the_window_and_reason() {
        let mut v: Value = serde_json::from_str(&push(NOW, 303_551)).unwrap();
        v["inputs"]["flop_side"] = json!({ "stale": "flop index: 2 buyers < 3", "window": { "prints": 40, "buyers": 2, "gateway_gn_share": 0.9 } });
        let f = &latest_json(&v, NOW)["flop_side"];
        assert_eq!(f["stale"], "flop index: 2 buyers < 3");
        assert_eq!((f["prints"].as_u64(), f["buyers"].as_u64(), f["miners"].as_u64()), (Some(40), Some(2), None));
        // Odd shapes do not panic.
        latest_json(&json!({ "inputs": 5 }), NOW);
        latest_json(&json!(null), NOW);
    }

    #[test]
    fn the_log_is_read_incrementally_and_tolerates_junk() {
        let d = tmp("log");
        let p = d.join("bots.log");
        let mut f = File::create(&p).unwrap();
        writeln!(f, "cycle 1 summary: progressed=0").unwrap();
        writeln!(f, "{}", push(NOW - 600_000, 300_000)).unwrap();
        writeln!(f, "{{\"event\":\"flop_push\",\"mark_e6\":").unwrap(); // unparsable
        writeln!(f, "{}", push(NOW - 300_000, 300_000)).unwrap();
        write!(f, "{}", &push(NOW, 310_000)[..200]).unwrap(); // torn last line
        f.flush().unwrap();
        let mut log = PushLog::default();
        log.update(&p).unwrap();
        assert_eq!(log.pushes.len(), 2);
        let off = log.offset;
        assert_eq!(off, std::fs::read(&p).unwrap().iter().rposition(|b| *b == b'\n').unwrap() as u64 + 1);
        // The rest of the torn line arrives: only it is read.
        writeln!(f, "{}", &push(NOW, 310_000)[200..]).unwrap();
        log.update(&p).unwrap();
        assert_eq!(log.pushes.len(), 3);
        assert_eq!(log.latest.as_ref().unwrap()["mark_e6"], 310_000);
        // Rotation: a shorter file starts over.
        std::fs::write(&p, format!("{}\n", push(NOW, 320_000))).unwrap();
        log.update(&p).unwrap();
        assert_eq!(log.pushes.len(), 1);
        assert_eq!(log.latest.as_ref().unwrap()["mark_e6"], 320_000);
    }

    #[test]
    fn history_downsamples_and_lists_changes() {
        let pushes: Vec<Push> = (0..2000).map(|i| Push { landed_ms: NOW - WINDOW_MS + i * 240_000, mark_e6: if i < 1000 { 300_000 } else { 310_000 }, sig: format!("s{i}") }).collect();
        let h = history_json(&pushes, &[], &["bf16"], NOW - WINDOW_MS, NOW);
        let pts = h["points"].as_array().unwrap();
        assert!(pts.len() <= MAX_POINTS && pts.len() > MAX_POINTS / 2, "{}", pts.len());
        assert_eq!(pts.last().unwrap()[0], pushes[1999].landed_ms);
        assert_eq!(h["changes"], json!([{ "t_ms": pushes[1000].landed_ms, "from": 0.3, "to": 0.31, "sig": "s1000" }]));
    }

    #[test]
    fn outages_and_gaps_come_from_the_recorder() {
        let d = tmp("rec");
        let t0 = NOW - 3_600_000;
        let lines = [rec(t0, 0), rec(t0 + 300_000, -2), "{\"at_ms\":".into(), rec(t0 + 600_000, -2), rec(t0 + 900_000, 0), rec(t0 + 1_200_000, -2), rec(t0 + 3_000_000, -2)];
        std::fs::write(d.join(format!("{}.jsonl", day(t0))), lines.join("\n")).unwrap();
        std::fs::write(d.join("2020-01-01.jsonl"), rec(1_577_836_800_000, 0)).unwrap(); // out of window
        std::fs::write(d.join("record.log"), "x").unwrap();
        let mut r = Recorder::default();
        r.update(&d, NOW - WINDOW_MS).unwrap();
        let (outages, gaps) = spans(&r.snaps(NOW - WINDOW_MS), &["bf16", "fp16"], NOW);
        // GMICloud down twice (the second still open); fp4 Darkbloom is not a pricing tier.
        assert_eq!(outages, vec![
            json!({ "provider": "GMICloud", "quantization": "bf16", "from_ms": t0 + 300_000, "to_ms": t0 + 900_000 }),
            json!({ "provider": "GMICloud", "quantization": "bf16", "from_ms": t0 + 1_200_000, "to_ms": null }),
        ]);
        assert_eq!(gaps, vec![json!({ "from_ms": t0 + 1_200_000, "to_ms": t0 + 3_000_000 })]);
        // A missing directory is no data.
        r.update(&d.join("nope"), 0).unwrap();
        assert!(r.snaps(0).is_empty());
    }

    #[test]
    fn build_writes_both_files_atomically() {
        let d = tmp("build");
        std::fs::write(d.join("bots.log"), format!("{}\n{}\n", push(NOW - 300_000, 300_000), push(NOW, 303_551))).unwrap();
        let mut feed = Feed::default();
        let (l, h) = feed.build(&d.join("bots.log"), &d.join("or-prices"), NOW + 1000).unwrap();
        assert_eq!(l["landed_ms"], NOW);
        assert_eq!(h["points"].as_array().unwrap().len(), 2);
        write_atomic(&d.join("out"), "latest.json", &l).unwrap();
        let back: Value = serde_json::from_slice(&std::fs::read(d.join("out/latest.json")).unwrap()).unwrap();
        assert_eq!(back, l);
        assert!(!d.join("out/.latest.json.tmp").exists());
        // No push yet: an error, not a panic.
        std::fs::write(d.join("empty.log"), "hello\n").unwrap();
        assert!(Feed::default().build(&d.join("empty.log"), &d, NOW).is_err());
    }

    #[test]
    fn day_is_utc_date() {
        assert_eq!(day(NOW), "2026-11-13");
        assert_eq!(day(0), "1970-01-01");
    }
}
