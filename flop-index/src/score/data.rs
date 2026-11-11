//! Pinned price data: a folder with a MANIFEST of `sha256 loader file` lines.
//! Only files listed there are read, and only after their sha256 matches.

use crate::OrEndpoint;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One listing of a group's endpoints at one time.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub at_ms: u64,
    pub endpoints: Vec<OrEndpoint>,
}

/// One model's snapshots, oldest first.
#[derive(Clone, Debug)]
pub struct Series {
    pub group: String,
    pub snaps: Vec<Snapshot>,
}

pub const DAY_MS: u64 = 86_400_000;

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Write `dir/MANIFEST` for `loader:file` specs. Refuses to overwrite one.
pub fn pin(dir: &Path, specs: &[String]) -> Result<String> {
    let manifest = dir.join("MANIFEST");
    if manifest.exists() {
        bail!("{} exists; a pin is never rewritten, make a new dated folder", manifest.display());
    }
    if specs.is_empty() {
        bail!("nothing to pin: give loader:file ...");
    }
    let mut out = String::from("# sha256 loader file (flop-score pin)\n");
    for spec in specs {
        let (loader, file) = spec.split_once(':').with_context(|| format!("{spec}: want loader:file"))?;
        if !LOADERS.contains(&loader) {
            bail!("unknown loader {loader}; one of {LOADERS:?}");
        }
        if file.contains('/') || file.contains(char::is_whitespace) {
            bail!("{file}: a plain file name inside the folder, no spaces");
        }
        let bytes = std::fs::read(dir.join(file)).with_context(|| format!("read {file}"))?;
        out.push_str(&format!("{} {loader} {file}\n", sha256_hex(&bytes)));
    }
    std::fs::write(&manifest, &out)?;
    Ok(out)
}

pub const LOADERS: [&str; 3] = ["daily-csv", "changes-csv", "recorder-jsonl"];

/// Read every pinned file in `dir`; fail if any hash differs. Returns the
/// series and the MANIFEST's own sha256 (names the data in reports).
pub fn load(dir: &Path) -> Result<(Vec<Series>, String)> {
    let manifest = std::fs::read(dir.join("MANIFEST")).with_context(|| format!("no MANIFEST in {}", dir.display()))?;
    let mut groups: BTreeMap<String, BTreeMap<u64, Vec<OrEndpoint>>> = BTreeMap::new();
    let mut files = 0;
    let mut seen = BTreeSet::new();
    for line in String::from_utf8(manifest.clone())?.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let [sha, loader, file] = f[..] else { bail!("bad MANIFEST line {line:?}") };
        if file.contains('/') {
            bail!("MANIFEST names {file}: pinned files sit in the folder itself");
        }
        if !seen.insert(file.to_owned()) {
            bail!("MANIFEST lists {file} twice");
        }
        let bytes = std::fs::read(dir.join(file)).with_context(|| format!("read pinned {file}"))?;
        let got = sha256_hex(&bytes);
        if got != sha {
            bail!("{file}: sha256 {got}, MANIFEST says {sha}");
        }
        let text = String::from_utf8(bytes).with_context(|| format!("{file}: not UTF-8"))?;
        match loader {
            "daily-csv" => daily_csv(&text, &mut groups),
            "changes-csv" => changes_csv(&text, &mut groups),
            "recorder-jsonl" => recorder_jsonl(&text, &mut groups),
            other => bail!("unknown loader {other}"),
        }
        .with_context(|| format!("load {file}"))?;
        files += 1;
    }
    if files == 0 {
        bail!("MANIFEST lists no files");
    }
    Ok((into_series(groups), sha256_hex(&manifest)))
}

/// Keep groups that ever list endpoints from 2+ providers.
pub fn into_series(groups: BTreeMap<String, BTreeMap<u64, Vec<OrEndpoint>>>) -> Vec<Series> {
    groups
        .into_iter()
        .filter(|(_, snaps)| snaps.values().any(|eps| eps.iter().map(|e| &e.provider).collect::<BTreeSet<_>>().len() >= 2))
        .map(|(group, snaps)| Series { group, snaps: snaps.into_iter().map(|(at_ms, endpoints)| Snapshot { at_ms, endpoints }).collect() })
        .collect()
}

/// Model names compare lowercase, letters, digits and dots only.
pub fn norm(model: &str) -> String {
    model.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.').collect()
}

/// `YYYY-MM-DD` to UTC midnight in ms.
pub fn date_ms(d: &str) -> Result<u64> {
    let p: Vec<&str> = d.split('-').collect();
    let [y, m, dd] = p[..] else { bail!("bad date {d:?}") };
    let (y, m, dd): (i64, i64, i64) = (y.parse()?, m.parse()?, dd.parse()?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days_in = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&m) || dd < 1 || dd > days_in[(m - 1) as usize] || y < 1970 {
        bail!("bad date {d:?}");
    }
    // Days from civil (H. Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Ok((era * 146_097 + doe - 719_468) as u64 * DAY_MS)
}

fn rows(text: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let head: Vec<String> = lines.next().context("empty csv")?.split(',').map(|s| s.trim().to_owned()).collect();
    let mut out = Vec::new();
    for l in lines {
        if l.contains('"') {
            bail!("quoted csv field not supported: {l:?}");
        }
        let r: Vec<String> = l.split(',').map(|s| s.trim().to_owned()).collect();
        if r.len() != head.len() {
            bail!("csv row has {} fields, header {}: {l:?}", r.len(), head.len());
        }
        out.push(r);
    }
    Ok((head, out))
}

fn col(head: &[String], name: &str) -> Result<usize> {
    head.iter().position(|h| h == name).with_context(|| format!("no column {name}"))
}

fn ep(provider: &str, in_mtok: f64, out_mtok: f64) -> OrEndpoint {
    // Daily sources carry no quantization or status: "unknown", up.
    OrEndpoint { provider: provider.to_owned(), quantization: "unknown".into(), prompt: in_mtok * 1e-6, completion: out_mtok * 1e-6, status: 0 }
}

/// github.com/romanshumy/llm-prices-data price-history.csv: one row per
/// model, provider and day. Group `daily:<model>`.
fn daily_csv(text: &str, groups: &mut BTreeMap<String, BTreeMap<u64, Vec<OrEndpoint>>>) -> Result<()> {
    let (h, rs) = rows(text)?;
    let (cm, cp, cd, ce, ci, co) = (col(&h, "model")?, col(&h, "provider")?, col(&h, "date")?, col(&h, "event")?, col(&h, "input_per_mtok")?, col(&h, "output_per_mtok")?);
    // (group, day) -> provider -> endpoint; a later row the same day wins.
    let mut cells: BTreeMap<(String, u64), BTreeMap<String, OrEndpoint>> = BTreeMap::new();
    for r in &rs {
        if r[ce] == "removed" || r[ci].is_empty() || r[co].is_empty() {
            continue;
        }
        let (i, o): (f64, f64) = (r[ci].parse()?, r[co].parse()?);
        cells.entry((format!("daily:{}", norm(&r[cm])), date_ms(&r[cd])?)).or_default().insert(r[cp].clone(), ep(&r[cp], i, o));
    }
    for ((g, at), eps) in cells {
        groups.entry(g).or_default().entry(at).or_default().extend(eps.into_values());
    }
    Ok(())
}

/// github.com/sophie-nguyenthuthuy/llm-pricing-history data/history.csv:
/// one row per price change. Uses the litellm rows (per provider), not
/// OpenRouter's own (one price per model) nor ":free" ids. A listing first
/// seen through a change counts once both its prices are known. Group
/// `changes:<model>`, one snapshot per calendar day from the first date to
/// the last.
fn changes_csv(text: &str, groups: &mut BTreeMap<String, BTreeMap<u64, Vec<OrEndpoint>>>) -> Result<()> {
    let (h, mut rs) = rows(text)?;
    let (cd, ce, cs, cp, cm, cf, cn) =
        (col(&h, "date")?, col(&h, "event")?, col(&h, "source")?, col(&h, "provider")?, col(&h, "model")?, col(&h, "field")?, col(&h, "new")?);
    rs.sort_by(|a, b| a[cd].cmp(&b[cd])); // stable: file order within a day
    let mut live: BTreeMap<(String, String), (Option<f64>, Option<f64>)> = BTreeMap::new();
    let dated: Vec<(u64, &Vec<String>)> = rs.iter().map(|r| Ok((date_ms(&r[cd])?, r))).collect::<Result<_>>()?;
    let (Some(&(first, _)), Some(&(last, _))) = (dated.first(), dated.last()) else { return Ok(()) };
    let mut it = dated.into_iter().peekable();
    let mut at = first;
    while at <= last {
        while let Some((_, r)) = it.next_if(|(d, _)| *d <= at) {
            if r[cs] != "litellm" || r[cp] == "openrouter" || r[cm].contains(":free") {
                continue;
            }
            let key = (r[cp].clone(), r[cm].clone());
            match r[ce].as_str() {
                "added" => {
                    let (i, o) = r[cn].split_once('/').with_context(|| format!("added without in/out: {:?}", r[cn]))?;
                    live.insert(key, (opt(i)?, opt(o)?));
                }
                "changed" => {
                    let v = live.entry(key).or_insert((None, None));
                    match r[cf].as_str() {
                        "input_usd_per_mtok" => v.0 = opt(&r[cn])?,
                        "output_usd_per_mtok" => v.1 = opt(&r[cn])?,
                        _ => {}
                    }
                }
                "removed" => {
                    live.remove(&key);
                }
                _ => {}
            }
        }
        // One endpoint per provider per group: two ids of one provider that
        // normalise to the same model keep the later one in id order.
        let mut day: BTreeMap<String, BTreeMap<String, OrEndpoint>> = BTreeMap::new();
        for ((provider, model), (i, o)) in &live {
            if let (Some(i), Some(o)) = (i, o) {
                let g = format!("changes:{}", norm(model.rsplit('/').next().unwrap_or(model)));
                day.entry(g).or_default().insert(provider.clone(), ep(provider, *i, *o));
            }
        }
        for (g, eps) in day {
            groups.entry(g).or_default().entry(at).or_default().extend(eps.into_values());
        }
        at += DAY_MS;
    }
    Ok(())
}

/// A price, or None when the source leaves it empty (then not listed).
fn opt(s: &str) -> Result<Option<f64>> {
    Ok(if s.is_empty() { None } else { Some(s.parse().with_context(|| format!("bad price {s:?}"))?) })
}

/// The 5-minute OpenRouter recorder (recorder PR 74, or_record.rs): one
/// line per snapshot, `{at_ms, rows: [{model, provider, quantization,
/// prompt, completion, status}] | null}`, prices USD per token (number or
/// string). Group `recorder:<model>`. A null status counts as up, as in the
/// pusher.
fn recorder_jsonl(text: &str, groups: &mut BTreeMap<String, BTreeMap<u64, Vec<OrEndpoint>>>) -> Result<()> {
    let num = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()));
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).with_context(|| format!("line {}", n + 1))?;
        let at = v["at_ms"].as_u64().with_context(|| format!("line {}: no at_ms", n + 1))?;
        let Some(rows) = v["rows"].as_array() else { continue };
        // A row's model may be null: then the model in the fetched URL
        // (.../models/<org>/<model>/endpoints).
        let url_model = v["url"].as_str().and_then(|u| u.split("/models/").nth(1)).and_then(|m| m.strip_suffix("/endpoints"));
        for r in rows {
            // Neither names the model: skip rather than mix models.
            let Some(model) = r["model"].as_str().or(url_model) else { continue };
            let (Some(provider), Some(p), Some(c)) = (r["provider"].as_str(), num(&r["prompt"]), num(&r["completion"])) else {
                continue;
            };
            let e = OrEndpoint {
                provider: provider.to_owned(),
                quantization: r["quantization"].as_str().unwrap_or("unknown").to_owned(),
                prompt: p,
                completion: c,
                status: r["status"].as_i64().unwrap_or(0),
            };
            groups.entry(format!("recorder:{model}")).or_default().entry(at).or_default().push(e);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc_midnight() {
        assert_eq!(date_ms("1970-01-01").unwrap(), 0);
        assert_eq!(date_ms("2026-11-11").unwrap(), 1_794_355_200_000);
        assert!(date_ms("2026-13-01").is_err());
        assert!(date_ms("2026-02-29").is_err());
        assert!(date_ms("2024-02-29").is_ok());
    }

    #[test]
    fn a_changed_file_does_not_load() {
        let dir = std::env::temp_dir().join(format!("flop-pin-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let csv = "model_id,model,provider,date,event,source,input_per_mtok,output_per_mtok,cached_input_per_mtok\n\
                   a,M,P1,2026-11-01,snapshot,,1.0,2.0,\na,M,P2,2026-11-01,snapshot,,1.5,2.0,\n";
        std::fs::write(dir.join("d.csv"), csv).unwrap();
        pin(&dir, &["daily-csv:d.csv".into()]).unwrap();
        assert!(pin(&dir, &["daily-csv:d.csv".into()]).is_err(), "a pin is never rewritten");
        let (s, _) = load(&dir).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].snaps[0].endpoints.len(), 2);
        std::fs::write(dir.join("d.csv"), csv.replace("1.5", "1.4")).unwrap();
        let e = load(&dir).unwrap_err().to_string();
        assert!(e.contains("sha256"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changes_replay_adds_changes_and_removals() {
        let csv = "date,event,source,provider,model,field,old,new\n\
            2026-08-01,added,litellm,a,a/Qwen/Q-27B,,,0.4/3\n\
            2026-08-01,added,litellm,b,b/q-27b,,,0.5/3\n\
            2026-08-01,added,litellm,openrouter,openrouter/q-27b,,,0.1/1\n\
            2026-08-02,changed,litellm,a,a/Qwen/Q-27B,input_usd_per_mtok,0.4,0.2\n\
            2026-08-02,changed,litellm,c,c/q-27b,input_usd_per_mtok,0.3,0.35\n\
            2026-08-03,removed,litellm,b,b/q-27b,,0.5/3,\n";
        let mut g = BTreeMap::new();
        changes_csv(csv, &mut g).unwrap();
        let s = &g["changes:q27b"];
        let day = |d| s[&date_ms(d).unwrap()].iter().map(|e| (e.provider.clone(), (e.prompt * 1e6 * 100.0).round() as i64)).collect::<Vec<_>>();
        assert_eq!(day("2026-08-01"), vec![("a".into(), 40), ("b".into(), 50)]);
        // c has only one known price: not listed yet.
        assert_eq!(day("2026-08-02"), vec![("a".into(), 20), ("b".into(), 50)]);
        assert_eq!(day("2026-08-03"), vec![("a".into(), 20)]);
    }

    #[test]
    fn recorder_lines_load_as_the_recorder_writes_them() {
        let text = concat!(
            r#"{"at_ms":1794355200000,"url":"https://openrouter.ai/api/v1/models/qwen/qwen3.8-27b/endpoints","rows":[{"model":null,"endpoint":"deepinfra/bf16","provider":"DeepInfra","quantization":"bf16","prompt":1.5e-7,"completion":1.875e-6,"status":0},{"model":null,"endpoint":"gmicloud/bf16","provider":"GMICloud","quantization":"bf16","prompt":4.5e-7,"completion":3.2e-6,"status":-2},{"model":null,"endpoint":"x","provider":"X","quantization":null,"prompt":null,"completion":1e-6,"status":null}],"raw":"..."}"#,
            "\n",
            r#"{"at_ms":1794355500000,"url":"u","rows":null,"error":"timeout","raw":""}"#,
            "\n"
        );
        let mut g = BTreeMap::new();
        recorder_jsonl(text, &mut g).unwrap();
        let s = &g["recorder:qwen/qwen3.8-27b"];
        assert_eq!(s.len(), 1, "a failed fetch is no snapshot");
        let eps = &s[&1_794_355_200_000];
        assert_eq!(eps.len(), 2, "an unpriced row is skipped");
        assert_eq!((eps[0].provider.as_str(), eps[0].quantization.as_str(), eps[0].status), ("DeepInfra", "bf16", 0));
        assert_eq!(eps[1].status, -2);
    }
}
