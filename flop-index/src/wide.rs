//! The broad compute price index (docs/flop-compute/FEED.md, "Broad index
//! method"): turns the wide OpenRouter collector's per-model day files into
//! `market.json`. Read-only.

use crate::feed::{day, MAX_FUTURE_MS};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const DAY_MS: u64 = 86_400_000;
/// Data held in memory.
pub const HOLD_MS: u64 = 30 * DAY_MS;
/// Day files ending before this age keep one snapshot per model per hour.
pub const FULL_RES_MS: u64 = 8 * DAY_MS;
/// A model median needs at least this many usable quotes; fewer carry the last.
pub const MIN_QUOTES: usize = 3;
/// A model without a fresh median for longer than this leaves the index.
pub const DROP_MS: u64 = DAY_MS;
/// Chart points: index, and per model.
pub const INDEX_POINTS: usize = 500;
pub const MODEL_POINTS: usize = 200;
/// Price changes listed (newest kept), over the last 7 days.
pub const MAX_CHANGES: usize = 200;
pub const CHANGES_MS: u64 = 7 * DAY_MS;
/// A blended price moving by more than this fraction is a change.
pub const CHANGE_REL: f64 = 0.001;
/// A quote changing in more than this share of its snapshots is a dynamic pricer.
pub const DYNAMIC_SHARE: f64 = 0.2;
/// ... and only once it has at least this many priced snapshots.
pub const DYNAMIC_MIN_SNAPS: usize = 24;
/// Snapshots of different models further apart than this are different cycles.
const CYCLE_GAP_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug)]
struct Row {
    quote: u32,
    /// Blended USD per 1M tokens, if both prices are usable.
    price: Option<f64>,
    up: bool,
}

#[derive(Debug)]
struct Snap {
    at_ms: u64,
    rows: Vec<Row>,
}

struct DayFile {
    len: u64,
    snaps: Vec<Snap>,
    thinned: bool,
}

/// Each quote's (provider, quantization), interned so a row is small.
#[derive(Default)]
struct Quotes {
    names: Vec<(String, String)>,
    ids: HashMap<String, u32>,
}

impl Quotes {
    fn id(&mut self, key: String, provider: &str, quant: &str) -> u32 {
        if let Some(i) = self.ids.get(&key) {
            return *i;
        }
        let i = self.names.len() as u32;
        self.names.push((provider.to_owned(), quant.to_owned()));
        self.ids.insert(key, i);
        i
    }
}

/// (15 × prompt + completion) / 16 in USD per 1M tokens, if usable.
pub fn blend(prompt: Option<f64>, completion: Option<f64>) -> Option<f64> {
    let (p, c) = (prompt?, completion?);
    let b = crate::SPEC.blend;
    (p.is_finite() && c.is_finite() && p >= 0.0 && c >= 0.0 && p + c > 0.0).then(|| (b * p + c) / (b + 1.0) * 1e6)
}

fn read_day(path: &Path, quotes: &mut Quotes) -> Result<Vec<Snap>> {
    let mut out = Vec::new();
    for l in BufReader::new(File::open(path)?).split(b'\n') {
        // A torn or odd line is skipped, not fatal.
        let Ok(v) = serde_json::from_slice::<Value>(&l?) else { continue };
        let (Some(at_ms), Some(rows)) = (v["at_ms"].as_u64(), v["rows"].as_array()) else { continue };
        let rows = rows
            .iter()
            .filter_map(|r| {
                let provider = r["provider"].as_str()?;
                let quant = r["quantization"].as_str().unwrap_or("unknown");
                let key = r["tag"].as_str().map(String::from).unwrap_or_else(|| format!("{provider}/{quant}"));
                // A missing status reads as 0 (up), as in the pusher.
                let up = r["status"].is_null() || r["status"].as_i64() == Some(0);
                Some(Row { quote: quotes.id(key, provider, quant), price: blend(r["prompt"].as_f64(), r["completion"].as_f64()), up })
            })
            .collect();
        out.push(Snap { at_ms, rows });
    }
    out.sort_by_key(|s| s.at_ms);
    Ok(out)
}

/// Keep the first snapshot of each hour.
fn thin(snaps: &mut Vec<Snap>) {
    let mut last = None;
    snaps.retain(|s| {
        let h = s.at_ms / 3_600_000;
        let keep = last != Some(h);
        last = Some(h);
        keep
    });
}

/// The wide collector's directory, cached per day file by size, so a loop
/// re-reads only the growing files.
#[derive(Default)]
pub struct Wide {
    quotes: Quotes,
    /// Model id → its day files.
    models: BTreeMap<String, HashMap<PathBuf, DayFile>>,
    /// The index so far; `None` until a base is set.
    pub state: Option<IndexState>,
}

impl Wide {
    /// Read the model directories and their day files from `now_ms` − 30 days
    /// on. A missing directory or day is no data, not an error.
    pub fn update(&mut self, dir: &Path, now_ms: u64) -> Result<()> {
        let first = day(now_ms.saturating_sub(HOLD_MS));
        let full_from = day(now_ms.saturating_sub(FULL_RES_MS));
        let Ok(entries) = std::fs::read_dir(dir) else {
            self.models.clear();
            return Ok(());
        };
        let mut seen = BTreeSet::new();
        for d in entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()) {
            let id = d.file_name().unwrap_or_default().to_string_lossy().replace("__", "/");
            let Ok(files) = std::fs::read_dir(&d) else { continue };
            let cache = self.models.entry(id.clone()).or_default();
            seen.insert(id);
            let mut keep = Vec::new();
            for p in files.filter_map(|e| e.ok().map(|e| e.path())) {
                let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                if p.extension().is_none_or(|x| x != "jsonl") || stem.len() != 10 || stem < first {
                    continue;
                }
                let Ok(len) = p.metadata().map(|m| m.len()) else { continue };
                if cache.get(&p).is_none_or(|f| f.len != len) {
                    match read_day(&p, &mut self.quotes) {
                        Ok(snaps) => {
                            cache.insert(p.clone(), DayFile { len, snaps, thinned: false });
                        }
                        Err(e) => eprintln!("flop-feed: skip {}: {e:#}", p.display()),
                    }
                }
                if let Some(f) = cache.get_mut(&p).filter(|f| !f.thinned && stem < full_from) {
                    thin(&mut f.snaps);
                    f.thinned = true;
                }
                keep.push(p);
            }
            cache.retain(|p, _| keep.contains(p));
        }
        self.models.retain(|id, _| seen.contains(id));
        Ok(())
    }

    /// Start from a saved index state (see `IndexState::load`).
    pub fn with_state(state: Option<IndexState>) -> Self {
        Self { state, ..Self::default() }
    }

    /// Advance the index over what was read and build `market.json`. Save
    /// `self.state` afterwards (`index-state.json`).
    pub fn market_json(&mut self, now_ms: u64) -> Result<Value> {
        let from = now_ms.saturating_sub(HOLD_MS);
        let models: Vec<Model> = self
            .models
            .iter()
            .map(|(id, files)| {
                let mut snaps: Vec<&Snap> = files.values().flat_map(|f| &f.snaps).filter(|s| s.at_ms >= from && s.at_ms <= now_ms + MAX_FUTURE_MS).collect();
                snaps.sort_by_key(|s| s.at_ms);
                Model::new(id, snaps)
            })
            .collect();
        if models.iter().all(|m| m.snaps.is_empty()) {
            bail!("no wide collector data yet");
        }
        advance(&models, &mut self.state, now_ms);
        Ok(market(&models, &self.quotes.names, self.state.as_ref(), now_ms))
    }
}

/// One model's snapshots and its median at each.
struct Model<'a> {
    id: &'a str,
    snaps: Vec<&'a Snap>,
    /// Median of usable quotes, or the last one carried forward.
    median: Vec<Option<f64>>,
    /// Whether `median` was computed at that snapshot (not carried).
    fresh: Vec<bool>,
}

fn usable(s: &Snap) -> Vec<f64> {
    let mut v: Vec<f64> = s.rows.iter().filter(|r| r.up).filter_map(|r| r.price).collect();
    v.sort_by(f64::total_cmp);
    v
}

impl<'a> Model<'a> {
    fn new(id: &'a str, snaps: Vec<&'a Snap>) -> Self {
        let (mut median, mut fresh, mut last) = (Vec::new(), Vec::new(), None);
        for s in &snaps {
            let u = usable(s);
            let ok = u.len() >= MIN_QUOTES;
            if ok {
                last = Some(percentile(&u, 0.5));
            }
            median.push(last);
            fresh.push(ok);
        }
        Self { id, snaps, median, fresh }
    }

    /// The median at the last snapshot at or before `t`.
    fn median_at(&self, t: u64) -> Option<f64> {
        let n = self.snaps.partition_point(|s| s.at_ms <= t);
        n.checked_sub(1).and_then(|i| self.median[i])
    }
}

/// Linear-interpolated percentile of sorted, non-empty values (q in 0..=1).
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    let pos = q * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

fn pct(now: f64, then: Option<f64>) -> Option<f64> {
    then.filter(|t| *t > 0.0).map(|t| (now / t - 1.0) * 100.0)
}

/// Step-chart points: the ends and both sides of every change; over `max`,
/// each equal time slice keeps its lowest and highest point.
fn steps(pts: &[(u64, f64)], max: usize) -> Vec<(u64, f64)> {
    let n = pts.len();
    let mut keep: Vec<usize> = (0..n).filter(|&i| i == 0 || i + 1 == n || pts[i - 1].1 != pts[i].1 || pts[i + 1].1 != pts[i].1).collect();
    if keep.len() > max {
        let (t0, t1) = (pts[0].0, pts[n - 1].0);
        let width = (t1 - t0) / (max as u64 / 2).max(1) + 1;
        let mut by: BTreeMap<u64, (usize, usize)> = BTreeMap::new();
        for (i, p) in pts.iter().enumerate() {
            let e = by.entry((p.0 - t0) / width).or_insert((i, i));
            if p.1 < pts[e.0].1 {
                e.0 = i;
            }
            if p.1 > pts[e.1].1 {
                e.1 = i;
            }
        }
        keep = by.into_values().flat_map(|(a, b)| [a.min(b), a.max(b)]).collect();
        keep.dedup();
    }
    keep.into_iter().map(|i| pts[i]).collect()
}

/// A basket model's base median (after chain-link scaling) and its median
/// at the last step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Basket {
    pub base: f64,
    pub last: f64,
}

/// The index state, persisted as `index-state.json` so the base never moves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexState {
    pub v: u32,
    pub base_ms: u64,
    /// The last step: its snapshot time and level.
    pub at_ms: u64,
    pub level: f64,
    /// Models in the index now.
    pub models: BTreeMap<String, Basket>,
    /// `[t_ms, level]` for the last 30 days; past 8 days, one per hour.
    pub series: Vec<(u64, f64)>,
}

impl IndexState {
    /// Read a state file: `Ok(None)` if there is none, an error if it is
    /// there but unreadable (never silently re-base).
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        let s: Self = serde_json::from_slice(&bytes).with_context(|| format!("{} is unreadable; refusing to re-base (fix it, or pass --rebase)", path.display()))?;
        if s.v != 1 || !(s.level.is_finite() && s.level > 0.0) || s.models.values().any(|b| !(b.base.is_finite() && b.base > 0.0 && b.last.is_finite() && b.last > 0.0)) {
            bail!("{} has invalid values; refusing to re-base (fix it, or pass --rebase)", path.display());
        }
        Ok(Some(s))
    }

    /// One step at snapshot `t` with the models in the index now (id → median).
    /// Models leaving: the remaining bases are scaled so the level at the last
    /// medians is unchanged. Then the level is the Jevons index over the
    /// models staying. Models joining: base = median / (level / 100).
    fn step(&mut self, t: u64, active: &BTreeMap<&str, f64>) {
        let gone: Vec<String> = self.models.keys().filter(|k| !active.contains_key(k.as_str())).cloned().collect();
        if !gone.is_empty() {
            for k in &gone {
                self.models.remove(k);
            }
            if !self.models.is_empty() {
                let mean = self.models.values().map(|b| (b.last / b.base).ln()).sum::<f64>() / self.models.len() as f64;
                let f = (mean - (self.level / 100.0).ln()).exp();
                self.models.values_mut().for_each(|b| b.base *= f);
            }
        }
        // No median moved: the level stays exactly (no float drift after a rescale).
        if self.models.iter().any(|(k, b)| active[k.as_str()] != b.last) {
            let mean = self.models.iter().map(|(k, b)| (active[k.as_str()] / b.base).ln()).sum::<f64>() / self.models.len() as f64;
            self.level = 100.0 * mean.exp();
        }
        for (k, m) in active {
            let b = self.models.entry((*k).to_owned()).or_insert(Basket { base: m / (self.level / 100.0), last: *m });
            b.last = *m;
        }
        self.at_ms = t;
        self.series.push((t, self.level));
    }

    /// Keep 30 days of series; past 8 days, the first point of each hour.
    fn prune(&mut self, now_ms: u64) {
        let (from, full) = (now_ms.saturating_sub(HOLD_MS), now_ms.saturating_sub(FULL_RES_MS));
        let mut last_hour = None;
        self.series.retain(|(t, _)| {
            if *t < from {
                return false;
            }
            if *t >= full {
                return true;
            }
            let h = t / 3_600_000;
            let keep = last_hour != Some(h);
            last_hour = Some(h);
            keep
        });
    }
}

/// Advance the chain-linked Jevons index over snapshots after the state's
/// last step. Without a state, the base is the first snapshot where every
/// model priced within the first day of the data has a median.
fn advance(models: &[Model], state: &mut Option<IndexState>, now_ms: u64) {
    // Cycles: snapshots of all models in time order; a new cycle starts after
    // a pause or when a model repeats. Its time is its last snapshot.
    let mut ev: Vec<(u64, usize, usize)> = models.iter().enumerate().flat_map(|(m, x)| x.snaps.iter().enumerate().map(move |(i, s)| (s.at_ms, m, i))).collect();
    ev.sort();
    let mut cycles: Vec<Vec<(u64, usize, usize)>> = Vec::new();
    for e in ev {
        match cycles.last_mut() {
            Some(c) if e.0 - c.last().unwrap().0 <= CYCLE_GAP_MS && !c.iter().any(|x| x.1 == e.1) => c.push(e),
            _ => cycles.push(vec![e]),
        }
    }
    let t_of = |c: &Vec<(u64, usize, usize)>| c.last().unwrap().0;
    let mut base = None;
    if state.is_none() {
        let Some(t_first) = cycles.first().map(t_of) else { return };
        let mut first_fresh: Vec<Option<u64>> = vec![None; models.len()];
        for c in &cycles {
            for &(_, m, i) in c {
                if models[m].fresh[i] && first_fresh[m].is_none() {
                    first_fresh[m] = Some(t_of(c));
                }
            }
        }
        base = first_fresh.iter().flatten().filter(|t| **t <= t_first + DAY_MS).max().copied();
        if base.is_none() {
            return;
        }
    }
    let mut cur: Vec<Option<f64>> = vec![None; models.len()];
    let mut last_fresh: Vec<Option<u64>> = vec![None; models.len()];
    for c in &cycles {
        let t = t_of(c);
        for &(_, m, i) in c {
            if models[m].median[i].is_some() {
                cur[m] = models[m].median[i];
            }
            if models[m].fresh[i] {
                last_fresh[m] = Some(t);
            }
        }
        if state.as_ref().is_some_and(|s| t <= s.at_ms) || base.is_some_and(|b| t < b) {
            continue;
        }
        // In the index: a median, and a fresh one within the last day.
        let active: BTreeMap<&str, f64> = (0..models.len()).filter_map(|m| Some((models[m].id, cur[m].filter(|_| last_fresh[m].is_some_and(|f| t - f <= DROP_MS))?))).collect();
        match state {
            Some(s) => s.step(t, &active),
            None => {
                let models = active.iter().map(|(k, m)| ((*k).to_owned(), Basket { base: *m, last: *m })).collect();
                *state = Some(IndexState { v: 1, base_ms: t, at_ms: t, level: 100.0, models, series: vec![(t, 100.0)] });
            }
        }
    }
    if let Some(s) = state {
        s.prune(now_ms);
    }
}

/// Before or at `t` in a time-sorted series.
fn at_or_before(s: &[(u64, f64)], t: u64) -> Option<f64> {
    let n = s.partition_point(|p| p.0 <= t);
    n.checked_sub(1).map(|i| s[i].1)
}

/// "qwen/qwen3.8-27b" → ("Qwen3.8 27B", "Qwen").
pub fn name_family(id: &str) -> (String, String) {
    let (org, rest) = id.split_once('/').unwrap_or(("", id));
    const FAMILIES: &[(&str, &str)] = &[
        ("qwen", "Qwen"), ("deepseek", "DeepSeek"), ("moonshotai", "Kimi"), ("z-ai", "GLM"), ("minimax", "MiniMax"), ("openai", "GPT-OSS"),
        ("meta-llama", "Llama"), ("google", "Gemma"), ("nvidia", "Nemotron"), ("mistralai", "Mistral"),
    ];
    let family = FAMILIES.iter().find(|f| f.0 == org).map(|f| f.1.to_owned()).unwrap_or_else(|| org.to_owned());
    let word = |w: &str| -> String {
        let size = w.ends_with('b') && w.len() > 1 && w[..w.len() - 1].trim_start_matches(|c: char| c.is_ascii_alphabetic()).chars().all(|c| c.is_ascii_digit() || c == '.') && w.chars().any(|c| c.is_ascii_digit());
        if size || ["gpt", "oss", "glm", "it"].contains(&w) {
            return w.to_uppercase();
        }
        for (_, f) in FAMILIES {
            let lf = f.to_lowercase();
            if w.starts_with(&lf) && !f.contains('-') {
                return format!("{f}{}", &w[lf.len()..]);
            }
        }
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    };
    (rest.split('-').filter(|w| !w.is_empty()).map(word).collect::<Vec<_>>().join(" "), family)
}

fn market(models: &[Model], names: &[(String, String)], state: Option<&IndexState>, now_ms: u64) -> Value {
    let day_ago = now_ms.saturating_sub(DAY_MS);
    let (base, series, n_models) = match state {
        Some(s) => (Some(s.base_ms), s.series.as_slice(), s.models.len()),
        None => (None, &[][..], 0),
    };
    let level = state.map(|s| s.level);
    let mut out_models = Vec::new();
    // Provider → (models, quotes, up snapshots, snapshots, cheapest_in).
    let mut provs: BTreeMap<&str, (BTreeSet<&str>, usize, usize, usize, usize)> = BTreeMap::new();
    let (mut n_quotes, mut n_up) = (0, 0);
    let mut changes: Vec<(u64, &str, u32, f64, f64)> = Vec::new();
    let mut dynamic = Vec::new();
    for m in models {
        let Some(latest) = m.snaps.last() else { continue };
        // Uptime: share of the model's snapshots in the last day where the quote was listed and up.
        let recent: Vec<&&Snap> = m.snaps.iter().filter(|s| s.at_ms >= day_ago).collect();
        let uptime = |q: u32| -> (usize, usize) { (recent.iter().filter(|s| s.rows.iter().any(|r| r.quote == q && r.up)).count(), recent.len()) };
        let u = usable(latest);
        let thin = u.len() < MIN_QUOTES;
        n_quotes += latest.rows.len();
        n_up += latest.rows.iter().filter(|r| r.up).count();
        let carried = *m.median.last().unwrap();
        let Some(median) = (if thin { carried.or_else(|| (!u.is_empty()).then(|| percentile(&u, 0.5))) } else { Some(percentile(&u, 0.5)) }) else { continue };
        let stat = |q: f64| if u.is_empty() { median } else { percentile(&u, q) };
        let cheapest = latest.rows.iter().filter(|r| r.up).filter_map(|r| r.price.map(|p| (r.quote, p))).min_by(|a, b| a.1.total_cmp(&b.1));
        let mut quotes: Vec<(&Row, (usize, usize))> = latest.rows.iter().map(|r| (r, uptime(r.quote))).collect();
        quotes.sort_by(|a, b| (!a.0.up, a.0.price.is_none()).cmp(&(!b.0.up, b.0.price.is_none())).then(a.0.price.unwrap_or(0.0).total_cmp(&b.0.price.unwrap_or(0.0))));
        let mut listed = BTreeSet::new();
        for (r, (up, of)) in &quotes {
            let p = provs.entry(names[r.quote as usize].0.as_str()).or_default();
            p.0.insert(m.id);
            p.1 += 1;
            p.2 += up;
            p.3 += of;
            listed.insert(r.quote);
        }
        if let Some((q, _)) = cheapest {
            provs.entry(names[q as usize].0.as_str()).or_default().4 += 1;
        }
        let pts: Vec<(u64, f64)> = m.snaps.iter().zip(&m.median).filter_map(|(s, v)| Some((s.at_ms, (*v)?))).collect();
        let (name, family) = name_family(m.id);
        out_models.push(json!({
            "id": m.id, "name": name, "family": family,
            "sellers_total": latest.rows.len(), "sellers_up": latest.rows.iter().filter(|r| r.up).count(),
            "median": median, "p25": stat(0.25), "p75": stat(0.75), "min": stat(0.0), "max": stat(1.0),
            "thin": thin,
            "cheapest": cheapest.map(|(q, p)| json!({ "provider": names[q as usize].0, "quantization": names[q as usize].1, "price": p })),
            "change_24h_pct": pct(median, m.median_at(day_ago)),
            "series": steps(&pts, MODEL_POINTS).iter().map(|p| json!([p.0, p.1])).collect::<Vec<_>>(),
            "quotes": quotes.iter().map(|(r, (up, of))| json!({
                "provider": names[r.quote as usize].0, "quantization": names[r.quote as usize].1,
                "price": r.price, "up": r.up, "uptime_24h_pct": (*of > 0).then(|| *up as f64 / *of as f64 * 100.0),
            })).collect::<Vec<_>>(),
        }));
        // Price changes per quote over the last 7 days.
        let mut last: HashMap<u32, f64> = HashMap::new();
        // Quote → (snapshots priced, changes (at, from, to), min, max).
        type Per = (usize, Vec<(u64, f64, f64)>, f64, f64);
        let mut per: BTreeMap<u32, Per> = BTreeMap::new();
        for s in m.snaps.iter().filter(|s| s.at_ms >= now_ms.saturating_sub(CHANGES_MS)) {
            for r in &s.rows {
                let Some(p) = r.price else { continue };
                let e = per.entry(r.quote).or_insert((0, Vec::new(), p, p));
                e.0 += 1;
                (e.2, e.3) = (e.2.min(p), e.3.max(p));
                if let Some(old) = last.insert(r.quote, p) {
                    if (p - old).abs() > CHANGE_REL * old {
                        e.1.push((s.at_ms, old, p));
                    }
                }
            }
        }
        for (q, (snaps, ch, lo, hi)) in per {
            if snaps >= DYNAMIC_MIN_SNAPS && ch.len() as f64 > DYNAMIC_SHARE * snaps as f64 {
                dynamic.push(json!({
                    "model": m.id, "provider": names[q as usize].0, "quantization": names[q as usize].1,
                    "min": lo, "max": hi, "changes_24h": ch.iter().filter(|c| c.0 >= day_ago).count(),
                }));
            } else {
                changes.extend(ch.into_iter().map(|(t, a, b)| (t, m.id, q, a, b)));
            }
        }
    }
    changes.sort_by_key(|c| std::cmp::Reverse(c.0));
    changes.truncate(MAX_CHANGES);
    dynamic.sort_by(|a, b| b["changes_24h"].as_u64().cmp(&a["changes_24h"].as_u64()));
    out_models.sort_by(|a, b| b["sellers_up"].as_u64().cmp(&a["sellers_up"].as_u64()).then(a["id"].as_str().cmp(&b["id"].as_str())));
    let mut providers: Vec<Value> = provs
        .iter()
        .map(|(name, (ms, q, up, of, cheap))| json!({ "name": name, "models": ms.len(), "quotes": q, "uptime_24h_pct": (*of > 0).then(|| *up as f64 / *of as f64 * 100.0), "cheapest_in": cheap }))
        .collect();
    providers.sort_by(|a, b| b["models"].as_u64().cmp(&a["models"].as_u64()).then(a["name"].as_str().cmp(&b["name"].as_str())));
    let first = models.iter().filter_map(|m| m.snaps.first()).map(|s| s.at_ms).min().unwrap_or(now_ms);
    json!({
        "generated_ms": now_ms,
        "window": { "from_ms": first, "to_ms": now_ms },
        "index": {
            "level": level, "base_ms": base,
            "change_24h_pct": level.and_then(|l| pct(l, at_or_before(series, day_ago))),
            "change_7d_pct": level.and_then(|l| pct(l, at_or_before(series, now_ms.saturating_sub(7 * DAY_MS)))),
            "n_models": n_models, "n_quotes": n_quotes, "n_quotes_up": n_up, "n_providers": providers.len(),
            "series": steps(series, INDEX_POINTS).iter().map(|p| json!([p.0, p.1])).collect::<Vec<_>>(),
        },
        "models": out_models,
        "providers": providers,
        "changes": changes.iter().map(|(t, m, q, a, b)| json!({ "at_ms": t, "model": m, "provider": names[*q as usize].0, "quantization": names[*q as usize].1, "from": a, "to": b })).collect::<Vec<_>>(),
        "dynamic": dynamic,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_794_528_000_000; // 2026-11-13T00:00:00Z
    const MIN: u64 = 60_000;
    const HOUR: u64 = 3_600_000;

    /// A collector line; each row is (provider, USD per 1M blended, status).
    /// prompt = completion = price / 1e6, so the 15:1 blend is the price.
    fn line(model: &str, at: u64, rows: &[(&str, f64, i64)]) -> String {
        let rows: Vec<Value> = rows.iter().map(|(p, x, s)| json!({ "provider": p, "tag": format!("{}/bf16", p.to_lowercase()), "quantization": "bf16", "prompt": x / 1e6, "completion": x / 1e6, "status": s })).collect();
        json!({ "at_ms": at, "model": model, "rows": rows }).to_string()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flop-wide-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Write lines into `<dir>/<model>/<day>.jsonl`, appending.
    fn put(dir: &Path, lines: &[String]) {
        use std::io::Write;
        for l in lines {
            let v: Value = serde_json::from_str(l).unwrap();
            let m = dir.join(v["model"].as_str().unwrap().replace('/', "__"));
            std::fs::create_dir_all(&m).unwrap();
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(m.join(format!("{}.jsonl", day(v["at_ms"].as_u64().unwrap())))).unwrap();
            writeln!(f, "{l}").unwrap();
        }
    }

    fn run(dir: &Path, now: u64) -> Value {
        let mut w = Wide::default();
        w.update(dir, now).unwrap();
        w.market_json(now).unwrap()
    }

    fn model<'a>(v: &'a Value, id: &str) -> &'a Value {
        v["models"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap()
    }

    fn levels(v: &Value) -> Vec<f64> {
        v["index"]["series"].as_array().unwrap().iter().map(|p| p[1].as_f64().unwrap()).collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9 * b.abs().max(1.0)
    }

    /// Three sellers for `m` at `price`, the first at `first`.
    fn three(m: &str, at: u64, first: f64, price: f64) -> String {
        line(m, at, &[("A", first, 0), ("B", price, 0), ("C", price, 0)])
    }

    #[test]
    fn percentiles_and_blend() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert_eq!([0.0, 0.25, 0.5, 0.75, 1.0].map(|q| percentile(&v, q)), [1.0, 1.75, 2.5, 3.25, 4.0]);
        assert_eq!(percentile(&[1.0, 2.0, 10.0], 0.5), 2.0);
        assert_eq!(percentile(&[7.0], 0.25), 7.0);
        // (15 × 1e-7 + 1.6e-6) / 16 × 1e6 = 0.19375
        assert!(close(blend(Some(1e-7), Some(1.6e-6)).unwrap(), 0.19375));
        assert_eq!(blend(Some(0.0), Some(1e-6)), Some(1.0 / 16.0));
        for (p, c) in [(None, Some(1e-6)), (Some(1e-6), None), (Some(0.0), Some(0.0)), (Some(-1e-6), Some(1e-6)), (Some(f64::NAN), Some(1e-6)), (Some(f64::INFINITY), Some(1e-6))] {
            assert_eq!(blend(p, c), None, "{p:?} {c:?}");
        }
    }

    #[test]
    fn a_thin_model_carries_its_median() {
        let d = tmp("thin");
        put(&d, &[
            three("x/a", T0, 1.0, 2.0),
            line("x/a", T0 + 5 * MIN, &[("A", 1.0, 0), ("B", 4.0, 0), ("C", 4.0, -2)]),
            line("x/b", T0, &[("A", 1.0, 0), ("B", 3.0, 0)]),
        ]);
        let v = run(&d, T0 + 6 * MIN);
        let a = model(&v, "x/a");
        // Two usable quotes: the median stays 2, the spread is the two left.
        assert_eq!((a["thin"].as_bool(), a["median"].as_f64(), a["min"].as_f64(), a["max"].as_f64()), (Some(true), Some(2.0), Some(1.0), Some(4.0)));
        assert_eq!(a["series"], json!([[T0, 2.0], [T0 + 5 * MIN, 2.0]]));
        assert_eq!((a["sellers_total"].as_u64(), a["sellers_up"].as_u64()), (Some(3), Some(2)));
        assert_eq!(a["cheapest"], json!({ "provider": "A", "quantization": "bf16", "price": 1.0 }));
        // Never 3 quotes: shown with its own median, not in the index.
        let b = model(&v, "x/b");
        assert_eq!((b["thin"].as_bool(), b["median"].as_f64()), (Some(true), Some(2.0)));
        assert_eq!(v["index"]["n_models"], 1);
        assert_eq!(levels(&v), vec![100.0, 100.0]);
    }

    #[test]
    fn a_model_joining_later_does_not_move_the_index() {
        let d = tmp("join");
        let t = [T0, T0 + 5 * MIN, T0 + 25 * HOUR, T0 + 25 * HOUR + 5 * MIN];
        let mut lines = Vec::new();
        for (k, at) in t.iter().enumerate() {
            lines.push(three("x/a", *at, 1.0, if k == 0 { 2.0 } else { 4.0 })); // doubles at t1
            lines.push(three("x/b", at + 1000, 1.0, 3.0));
            lines.push(three("x/c", at + 2000, 1.0, 5.0));
            if k >= 2 {
                lines.push(three("x/d", at + 3000, 1.0, if k == 2 { 7.0 } else { 14.0 })); // joins at t2, doubles at t3
            }
        }
        put(&d, &lines);
        let v = run(&d, t[3] + MIN);
        let l1 = 100.0 * 2f64.powf(1.0 / 3.0);
        let got = levels(&v);
        assert_eq!(got.len(), 4);
        assert!(close(got[1], l1) && close(got[2], l1), "{got:?}");
        assert!(close(got[3], l1 * 2f64.powf(0.25)), "{got:?}");
        assert_eq!((v["index"]["n_models"].as_u64(), v["index"]["base_ms"].as_u64()), (Some(4), Some(T0 + 2000)));
        assert!(close(v["index"]["change_24h_pct"].as_f64().unwrap(), (got[3] / l1 - 1.0) * 100.0));
        assert!(close(model(&v, "x/a")["change_24h_pct"].as_f64().unwrap(), 0.0));
        assert_eq!(model(&v, "x/d")["change_24h_pct"], Value::Null);
    }

    #[test]
    fn a_model_that_stops_reporting_leaves_without_a_jump() {
        let d = tmp("drop");
        let t = [T0, T0 + 5 * MIN, T0 + HOUR, T0 + 25 * HOUR + 10 * MIN, T0 + 25 * HOUR + 15 * MIN];
        let mut lines = Vec::new();
        for (k, at) in t.iter().enumerate() {
            lines.push(three("x/a", *at, 1.0, if k == 0 { 2.0 } else { 4.0 })); // doubles at t1
            lines.push(three("x/b", at + 1000, 1.0, if k < 4 { 3.0 } else { 6.0 })); // doubles at t4
            lines.push(three("x/c", at + 2000, 1.0, 5.0));
            match k {
                0 => lines.push(three("x/d", at + 3000, 1.0, 7.0)),
                1 => lines.push(three("x/d", at + 3000, 1.0, 21.0)), // triples, then goes quiet
                2 => lines.push(line("x/d", at + 3000, &[("A", 1.0, 0)])), // listed but thin: not fresh
                _ => {}
            }
        }
        put(&d, &lines);
        let v = run(&d, t[4] + MIN);
        let got = levels(&v);
        let l1 = 100.0 * (2.0f64 * 3.0).powf(0.25);
        // The chart keeps both sides of each change: t0, t1, then t3 (D just
        // dropped, level unchanged) and t4 (B doubled among the three left).
        let times: Vec<u64> = v["index"]["series"].as_array().unwrap().iter().map(|p| p[0].as_u64().unwrap()).collect();
        assert_eq!(times, [t[0] + 3000, t[1] + 3000, t[3] + 2000, t[4] + 2000]);
        assert!(close(got[1], l1) && close(got[2], l1), "{got:?}");
        assert!(close(got[3], l1 * 2f64.powf(1.0 / 3.0)), "{got:?}");
        assert_eq!(v["index"]["n_models"], 3);
    }

    #[test]
    fn dynamic_pricers_are_listed_apart_from_changes() {
        let d = tmp("dyn");
        let lines: Vec<String> = (0..30u64)
            .map(|k| {
                let x = 1.0 + k as f64 * 0.01; // changes every time
                let y = if k < 6 { 2.0 } else { 2.02 }; // one 1% change
                let z = if k < 3 { 3.0 } else { 3.0015 }; // 0.05%: noise
                line("x/a", T0 + k * 5 * MIN, &[("X", x, 0), ("Y", y, 0), ("Z", z, 0)])
            })
            .collect();
        put(&d, &lines);
        // Under 24 snapshots nothing is dynamic: X's changes are listed.
        let few = tmp("dyn-few");
        put(&few, &lines[..23]);
        let v = run(&few, T0 + 3 * HOUR);
        assert_eq!((v["changes"].as_array().unwrap().len(), v["dynamic"].as_array().unwrap().len()), (22 + 1, 0));
        let v = run(&d, T0 + 3 * HOUR);
        assert_eq!(v["changes"], json!([{ "at_ms": T0 + 30 * MIN, "model": "x/a", "provider": "Y", "quantization": "bf16", "from": 2.0, "to": 2.02 }]));
        let dy = v["dynamic"].as_array().unwrap();
        assert_eq!(dy.len(), 1);
        assert_eq!((dy[0]["provider"].as_str(), dy[0]["changes_24h"].as_u64(), dy[0]["min"].as_f64()), (Some("X"), Some(29), Some(1.0)));
        assert!(close(dy[0]["max"].as_f64().unwrap(), 1.29));
    }

    #[test]
    fn uptime_counts_missing_only_when_the_model_was_fetched() {
        let d = tmp("up");
        let up = |at| line("x/a", at, &[("Q", 1.0, 0), ("R", 2.0, 0), ("S", 3.0, 0)]);
        let mut null_status = up(T0 + 15 * MIN);
        null_status = null_status.replace("\"status\":0", "\"status\":null");
        put(&d, &[
            up(T0 - 2 * DAY_MS), // outside the last day
            up(T0),
            line("x/a", T0 + 5 * MIN, &[("Q", 1.0, -2), ("R", 2.0, 0), ("S", 3.0, 0)]),
            line("x/a", T0 + 10 * MIN, &[("R", 2.0, 0), ("S", 3.0, 0)]),
            json!({ "at_ms": T0 + 12 * MIN, "model": "x/a", "rows": [] }).to_string(),
            null_status,
        ]);
        // Another model's listing at other times does not count against x/a.
        put(&d, &[three("x/b", T0 + 7 * MIN, 1.0, 1.0)]);
        let v = run(&d, T0 + 20 * MIN);
        let q = |p: &str| model(&v, "x/a")["quotes"].as_array().unwrap().iter().find(|x| x["provider"] == p).unwrap()["uptime_24h_pct"].as_f64().unwrap();
        assert_eq!((q("Q"), q("R")), (40.0, 80.0));
        let prov = v["providers"].as_array().unwrap();
        let a = prov.iter().find(|p| p["name"] == "A").unwrap();
        assert_eq!((a["models"].as_u64(), a["quotes"].as_u64(), a["uptime_24h_pct"].as_f64(), a["cheapest_in"].as_u64()), (Some(1), Some(1), Some(100.0), Some(1)));
        assert_eq!(prov.iter().find(|p| p["name"] == "Q").unwrap()["cheapest_in"], 1);
    }

    #[test]
    fn junk_nulls_and_gaps_do_not_panic() {
        let d = tmp("junk");
        let m = d.join("x__a");
        std::fs::create_dir_all(&m).unwrap();
        let nulls = json!({ "at_ms": T0 + MIN, "rows": [
            { "provider": "N", "quantization": null, "prompt": null, "completion": 1e-6, "status": 0 },
            { "provider": "U", "prompt": 1e-6, "completion": 1e-6, "status": "weird" },
            { "quantization": "bf16", "prompt": 1e-6, "completion": 1e-6 },
            { "provider": "A", "quantization": "fp8", "prompt": 1e-6, "completion": 1e-6 },
        ] });
        std::fs::write(m.join(format!("{}.jsonl", day(T0))), format!("garbage\n{{}}\n{{\"at_ms\":{T0},\"rows\":null}}\n{nulls}\n{{\"at_ms\":{}", T0 + 2 * MIN)).unwrap();
        // A day two days back and nothing in between: just a gap.
        std::fs::write(m.join(format!("{}.jsonl", day(T0 - 2 * DAY_MS))), format!("{}\n", three("x/a", T0 - 2 * DAY_MS, 1.0, 1.0))).unwrap();
        std::fs::write(m.join("notes.txt"), "x").unwrap();
        std::fs::create_dir_all(d.join("x__empty")).unwrap();
        let v = run(&d, T0 + 3 * MIN);
        let a = model(&v, "x/a");
        let quotes = a["quotes"].as_array().unwrap();
        assert_eq!(quotes.len(), 3); // the row with no provider is skipped
        assert_eq!(quotes[0], json!({ "provider": "A", "quantization": "fp8", "price": 1.0, "up": true, "uptime_24h_pct": 100.0 }));
        assert_eq!(quotes[1]["price"], Value::Null);
        assert_eq!((quotes[1]["provider"].as_str(), quotes[1]["quantization"].as_str()), (Some("N"), Some("unknown")));
        // A status that is not 0 or null is down.
        assert_eq!((quotes[2]["provider"].as_str(), quotes[2]["up"].as_bool()), (Some("U"), Some(false)));
        // One usable quote: thin, the median carried from two days ago.
        assert_eq!((a["thin"].as_bool(), a["median"].as_f64()), (Some(true), Some(1.0)));
        // Its last fresh median is over a day old: out of the index.
        assert_eq!(v["index"]["n_models"], 0);
        // Nothing at all: an error, not a panic.
        let mut w = Wide::default();
        w.update(&d.join("missing"), T0).unwrap();
        assert!(w.market_json(T0).is_err());
    }

    #[test]
    fn a_loop_rereads_only_grown_files_and_thins_old_days() {
        let d = tmp("loop");
        let old = T0 - 10 * DAY_MS;
        put(&d, &(0..12u64).map(|k| three("x/a", old + k * 5 * MIN, 1.0, 1.0)).collect::<Vec<_>>());
        put(&d, &[three("x/a", T0, 1.0, 1.0)]);
        let mut w = Wide::default();
        w.update(&d, T0 + MIN).unwrap();
        let files = &w.models["x/a"];
        assert_eq!(files.values().map(|f| f.snaps.len()).sum::<usize>(), 2);
        let n_quotes = w.quotes.names.len();
        put(&d, &[three("x/a", T0 + 5 * MIN, 1.0, 2.0)]);
        w.update(&d, T0 + 6 * MIN).unwrap();
        assert_eq!(w.models["x/a"].values().map(|f| f.snaps.len()).sum::<usize>(), 3);
        assert_eq!(w.quotes.names.len(), n_quotes);
        // Past 30 days a day file is dropped.
        w.update(&d, old + 31 * DAY_MS).unwrap();
        assert_eq!(w.models["x/a"].len(), 1);
    }

    /// Save the state as the binary does and load it back.
    fn reload(w: &Wide, d: &Path) -> Option<IndexState> {
        crate::feed::write_atomic(d, "index-state.json", &serde_json::to_value(w.state.as_ref().unwrap()).unwrap()).unwrap();
        IndexState::load(&d.join("index-state.json")).unwrap()
    }

    #[test]
    fn a_restart_after_a_gap_keeps_base_and_level() {
        let (d, out) = (tmp("restart"), tmp("restart-out"));
        let cycle = |at: u64, a: f64| vec![three("x/a", at, 1.0, a), three("x/b", at + 1000, 1.0, 3.0), three("x/c", at + 2000, 1.0, 5.0)];
        put(&d, &[cycle(T0, 2.0), cycle(T0 + 5 * MIN, 4.0)].concat());
        let mut w = Wide::default();
        w.update(&d, T0 + 6 * MIN).unwrap();
        w.market_json(T0 + 6 * MIN).unwrap();
        let l1 = 100.0 * 2f64.powf(1.0 / 3.0);
        assert!(close(w.state.as_ref().unwrap().level, l1));
        let saved = reload(&w, &out);
        assert_eq!(saved, w.state);
        // The feed is down for 6 hours while prices move; a new process
        // continues from the saved bases.
        put(&d, &[cycle(T0 + 3 * HOUR, 8.0), cycle(T0 + 6 * HOUR, 6.0)].concat());
        let mut again = Wide::with_state(saved);
        again.update(&d, T0 + 6 * HOUR + MIN).unwrap();
        let v = again.market_json(T0 + 6 * HOUR + MIN).unwrap();
        assert_eq!(v["index"]["base_ms"], T0 + 2000);
        // Level from the bases alone: A at 6 vs base 2, B and C flat.
        assert!(close(v["index"]["level"].as_f64().unwrap(), 100.0 * 3f64.powf(1.0 / 3.0)));
        // The same as a process that never stopped.
        w.update(&d, T0 + 6 * HOUR + MIN).unwrap();
        let cont = w.market_json(T0 + 6 * HOUR + MIN).unwrap();
        assert!(close(cont["index"]["level"].as_f64().unwrap(), v["index"]["level"].as_f64().unwrap()));
        assert_eq!(cont["index"]["series"], v["index"]["series"]);
        // Running again over the same data changes nothing.
        let st = again.state.clone();
        again.market_json(T0 + 6 * HOUR + 2 * MIN).unwrap();
        assert_eq!(again.state, st);
    }

    #[test]
    fn retention_does_not_move_the_base() {
        let (d, out) = (tmp("retain"), tmp("retain-out"));
        let cycle = |at: u64, a: f64| vec![three("x/a", at, 1.0, a), three("x/b", at + 1000, 1.0, 3.0), three("x/c", at + 2000, 1.0, 5.0)];
        put(&d, &cycle(T0, 2.0));
        let mut w = Wide::default();
        w.update(&d, T0 + MIN).unwrap();
        w.market_json(T0 + MIN).unwrap();
        let saved = reload(&w, &out);
        // 40 days on: the first day file is past the 30 days held.
        let later = T0 + 40 * DAY_MS;
        put(&d, &cycle(later, 4.0));
        let mut w = Wide::with_state(saved);
        w.update(&d, later + MIN).unwrap();
        assert_eq!(w.models["x/a"].len(), 1);
        let v = w.market_json(later + MIN).unwrap();
        assert_eq!(v["index"]["base_ms"], T0 + 2000);
        assert!(close(v["index"]["level"].as_f64().unwrap(), 100.0 * 2f64.powf(1.0 / 3.0)));
        // Old series points age out with the data; the base does not.
        assert_eq!(v["index"]["series"].as_array().unwrap().len(), 1);
        // Without the state the same data would be a new base at 100.
        let fresh = run(&d, later + MIN);
        assert_eq!((fresh["index"]["base_ms"].as_u64(), fresh["index"]["level"].as_f64()), (Some(later + 2000), Some(100.0)));
    }

    #[test]
    fn an_unreadable_state_refuses_to_start() {
        let d = tmp("state");
        let p = d.join("index-state.json");
        assert_eq!(IndexState::load(&p).unwrap(), None); // none yet: a new base is fine
        for bad in ["", "{\"v\":1", "[]", r#"{"v":1,"base_ms":1,"at_ms":1,"level":-5.0,"models":{},"series":[]}"#, r#"{"v":1,"base_ms":1,"at_ms":1,"level":100.0,"models":{"x/a":{"base":0.0,"last":1.0}},"series":[]}"#, r#"{"v":2,"base_ms":1,"at_ms":1,"level":100.0,"models":{},"series":[]}"#] {
            std::fs::write(&p, bad).unwrap();
            let e = format!("{:#}", IndexState::load(&p).unwrap_err());
            assert!(e.contains("refusing to re-base") && e.contains("--rebase"), "{bad}: {e}");
        }
        std::fs::create_dir_all(d.join("dir.json")).unwrap();
        assert!(IndexState::load(&d.join("dir.json")).is_err());
    }

    #[test]
    fn names_and_families() {
        let want = [
            ("qwen/qwen3.8-27b", "Qwen3.8 27B", "Qwen"),
            ("qwen/qwen3.5-397b-a17b", "Qwen3.5 397B A17B", "Qwen"),
            ("deepseek/deepseek-v4-flash", "DeepSeek V4 Flash", "DeepSeek"),
            ("moonshotai/kimi-k2.7-code", "Kimi K2.7 Code", "Kimi"),
            ("z-ai/glm-5.3", "GLM 5.3", "GLM"),
            ("minimax/minimax-m3", "MiniMax M3", "MiniMax"),
            ("openai/gpt-oss-120b", "GPT OSS 120B", "GPT-OSS"),
            ("meta-llama/llama-3.3-70b-instruct", "Llama 3.3 70B Instruct", "Llama"),
            ("google/gemma-4-31b-it", "Gemma 4 31B IT", "Gemma"),
            ("nvidia/nemotron-3-ultra-550b-a55b", "Nemotron 3 Ultra 550B A55B", "Nemotron"),
            ("mistralai/mistral-small-2603", "Mistral Small 2603", "Mistral"),
            ("acme/web-model", "Web Model", "acme"),
        ];
        for (id, name, fam) in want {
            assert_eq!(name_family(id), (name.to_owned(), fam.to_owned()), "{id}");
        }
    }

    /// Real collector lines (2026-11-13), three models, plus junk.
    #[test]
    fn end_to_end_on_the_fixture() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wide"));
        let now = 1_794_559_100_000;
        let v = run(dir, now);
        let i = &v["index"];
        assert_eq!((i["level"].as_f64(), i["base_ms"].as_u64(), i["n_models"].as_u64()), (Some(100.0), Some(1_794_559_021_841), Some(2)));
        assert_eq!((i["n_quotes"].as_u64(), i["n_quotes_up"].as_u64(), i["n_providers"].as_u64()), (Some(27), Some(24), Some(i["n_providers"].as_u64().unwrap())));
        assert_eq!((i["change_24h_pct"].clone(), i["change_7d_pct"].clone()), (Value::Null, Value::Null));
        assert_eq!(v["window"], json!({ "from_ms": 1_794_558_705_679u64, "to_ms": now }));
        let q = model(&v, "qwen/qwen3.8-27b");
        assert_eq!((q["name"].as_str(), q["sellers_total"].as_u64(), q["sellers_up"].as_u64(), q["thin"].as_bool()), (Some("Qwen3.8 27B"), Some(22), Some(19), Some(false)));
        assert!(close(q["median"].as_f64().unwrap(), 0.296875));
        assert!(close(q["min"].as_f64().unwrap(), 0.1146875) && close(q["max"].as_f64().unwrap(), 1.02125));
        assert_eq!(q["cheapest"]["provider"], "Chutes");
        let qs = q["quotes"].as_array().unwrap();
        assert!(qs.windows(2).all(|w| (!w[0]["up"].as_bool().unwrap(), w[0]["price"].as_f64()) <= (!w[1]["up"].as_bool().unwrap(), w[1]["price"].as_f64())));
        assert!(!qs.last().unwrap()["up"].as_bool().unwrap());
        let m = model(&v, "mistralai/mistral-small-2603");
        assert!(close(m["median"].as_f64().unwrap(), 0.1959375));
        let t = model(&v, "qwen/qwen3-14b");
        assert_eq!((t["thin"].as_bool(), t["sellers_up"].as_u64()), (Some(true), Some(2)));
        assert!(close(t["median"].as_f64().unwrap(), 0.1175));
        // Its empty listing five minutes earlier counts as down for both quotes.
        assert!(t["quotes"].as_array().unwrap().iter().all(|x| x["uptime_24h_pct"] == 50.0));
        let ids: Vec<&str> = v["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["qwen/qwen3.8-27b", "mistralai/mistral-small-2603", "qwen/qwen3-14b"]);
        let p = v["providers"].as_array().unwrap();
        assert!(p.windows(2).all(|w| w[0]["models"].as_u64() >= w[1]["models"].as_u64()));
        assert_eq!(p.iter().map(|x| x["cheapest_in"].as_u64().unwrap()).sum::<u64>(), 3);
        assert_eq!((v["changes"].as_array().map(Vec::len), v["dynamic"].as_array().map(Vec::len)), (Some(0), Some(0)));
    }
}
