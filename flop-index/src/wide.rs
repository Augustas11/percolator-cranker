//! The broad compute price index (docs/flop-compute/FEED.md, "Broad index
//! method"): turns the wide OpenRouter collector's per-model day files into
//! `market.json`. Read-only.

use crate::feed::{day, MAX_FUTURE_MS};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::borrow::Cow;
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
/// A quote that goes down, unpriced or unlisted still counts in its model's
/// median, at its last up price, for this long.
pub const HOLD_DOWN_MS: u64 = 6 * 3_600_000;
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
/// A model whose newest snapshot is older than this is stale: none of its
/// quotes is reported up.
pub const STALE_MS: u64 = 30 * 60_000;
/// The index steps only through snapshots at least this old, so a cycle
/// still being written is complete; later arrivals for a stepped time are ignored.
pub const STEP_LAG_MS: u64 = 2 * 60_000;
/// Per step, a model's index input moves by at most this factor either way.
pub const MAX_STEP_RATIO: f64 = 1.25;
/// The method version saved in the state; a state from another version is
/// re-linked once (see `advance`).
pub const METHOD: u32 = 1;
/// Snapshots of different models further apart than this are different cycles.
const CYCLE_GAP_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug)]
struct Row {
    quote: u32,
    /// The listing in its own model id: the tag (else provider and
    /// quantization). Price history is per listing, before any merging.
    listing: u32,
    /// Blended USD per 1M tokens, if both prices are usable.
    price: Option<f64>,
    /// (prompt, completion) in USD per token, when `price` is set.
    pc: Option<[f64; 2]>,
    up: bool,
    /// OpenRouter's `uptime_30m` (0–100), if it has recent traffic.
    uptime: Option<f64>,
}

#[derive(Clone, Debug)]
struct Snap {
    at_ms: u64,
    /// One row per quote (provider, quantization).
    rows: Vec<Row>,
    /// Every listing as fetched (empty in a merged alias snapshot).
    raw: Vec<Row>,
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
    listings: HashMap<String, u32>,
    /// Per quote id: its provider's id.
    prov: Vec<u32>,
    providers: HashMap<String, u32>,
}

impl Quotes {
    fn id(&mut self, key: String, provider: &str, quant: &str) -> u32 {
        if let Some(i) = self.ids.get(&key) {
            return *i;
        }
        let i = self.names.len() as u32;
        self.names.push((provider.to_owned(), quant.to_owned()));
        self.ids.insert(key, i);
        let n = self.providers.len() as u32;
        self.prov.push(*self.providers.entry(provider.to_owned()).or_insert(n));
        i
    }

    fn listing(&mut self, key: String) -> u32 {
        let n = self.listings.len() as u32;
        *self.listings.entry(key).or_insert(n)
    }
}

/// (15 × prompt + completion) / 16 in USD per 1M tokens, if usable.
pub fn blend(prompt: Option<f64>, completion: Option<f64>) -> Option<f64> {
    let (p, c) = (prompt?, completion?);
    (p.is_finite() && c.is_finite() && p >= 0.0 && c >= 0.0 && p + c > 0.0).then(|| mix([p, c], crate::SPEC.blend))
}

/// (r × prompt + completion) / (r + 1) in USD per 1M tokens: the price of
/// r input tokens per output token.
pub fn mix(pc: [f64; 2], r: f64) -> f64 {
    (r * pc[0] + pc[1]) / (r + 1.0) * 1e6
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
                let key = format!("{provider}\u{0}{quant}");
                let listing = quotes.listing(r["tag"].as_str().map(String::from).unwrap_or_else(|| key.clone()));
                // A missing status reads as 0 (up), as in the pusher.
                let up = r["status"].is_null() || r["status"].as_i64() == Some(0);
                let (p, c) = (r["prompt"].as_f64(), r["completion"].as_f64());
                let price = blend(p, c);
                let uptime = r["uptime_30m"].as_f64().filter(|u| u.is_finite()).map(|u| u.clamp(0.0, 100.0));
                Some(Row { quote: quotes.id(key, provider, quant), listing, price, pc: price.and_then(|_| Some([p?, c?])), up, uptime })
            })
            .collect::<Vec<Row>>();
        out.push(Snap { at_ms, rows: dedup(rows.clone()), raw: rows });
    }
    out.sort_by_key(|s| s.at_ms);
    Ok(out)
}

/// One row per quote (provider, quantization): the usable one with the
/// lowest price, else an up one, else any.
fn dedup(rows: Vec<Row>) -> Vec<Row> {
    let rank = |r: &Row| (if r.up && r.price.is_some() { 0 } else if r.up { 1 } else { 2 }, r.price.unwrap_or(f64::INFINITY));
    let mut best: BTreeMap<u32, Row> = BTreeMap::new();
    for r in rows {
        let e = best.entry(r.quote).or_insert(r);
        let (a, b) = (rank(&r), rank(e));
        if a.0 < b.0 || (a.0 == b.0 && a.1 < b.1) {
            *e = r;
        }
    }
    best.into_values().collect()
}

/// The id a dated snapshot `X-<4, 6 or 8 digits>` is an alias of: `X`,
/// only if `X` is itself a model here (same org, as it is the same prefix)
/// and both have Hugging Face ids that are equal, or the dated one's is
/// `X`'s plus the same suffix. Different releases (`mistral-small-2501`
/// and `-2603`, `Qwen3-235B-A22B` and `-2507` with its own HF repo) stay apart.
pub fn alias_of<'a>(id: &str, ids: &BTreeSet<&'a str>, hf: &HashMap<String, String>) -> Option<&'a str> {
    let (stem, tail) = id.rsplit_once('-')?;
    if ![4, 6, 8].contains(&tail.len()) || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let x = *ids.get(stem)?;
    let (hx, hd) = (hf.get(x)?, hf.get(id)?);
    (hx == hd || *hd == format!("{hx}-{tail}")).then_some(x)
}

/// One demand line: Hugging Face downloads (30 days), likes, parameter
/// count and the model's `config.json` shape.
#[derive(Clone, Debug)]
struct Demand {
    at_ms: u64,
    downloads: Option<u64>,
    likes: Option<u64>,
    params: Option<u64>,
    config: Option<Config>,
}

/// The fields of a model's Hugging Face `config.json` used to estimate its
/// active parameters; any may be missing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Config {
    pub hidden_size: Option<u64>,
    pub num_hidden_layers: Option<u64>,
    pub vocab_size: Option<u64>,
    pub experts_per_tok: Option<u64>,
    pub experts: Option<u64>,
    pub shared_experts: Option<u64>,
}

impl Config {
    fn from_json(v: &Value) -> Option<Self> {
        let f = |k: &str| v[k].as_u64();
        v.is_object().then(|| Self {
            hidden_size: f("hidden_size"), num_hidden_layers: f("num_hidden_layers"), vocab_size: f("vocab_size"),
            experts_per_tok: f("experts_per_tok"), experts: f("experts"), shared_experts: f("shared_experts"),
        })
    }

    /// A mixture of experts: more than one expert.
    pub fn has_experts(&self) -> bool {
        self.experts.is_some_and(|e| e > 1)
    }

    /// Estimated active parameters of a mixture-of-experts model with
    /// `total` weights: attention (layers × 4 × hidden²) and embedding
    /// (vocab × hidden, vocab 150,000 if unknown) always run, the rest is
    /// expert weights of which (per token + shared) / (experts + shared) run.
    /// Without layers or hidden size: total × that share. None for a config
    /// that cannot be right: more experts per token than experts, or fixed
    /// weights at or over the total.
    pub fn estimate_active(&self, total: u64) -> Option<u64> {
        let (e, k) = (self.experts.filter(|e| *e > 1)?, self.experts_per_tok.filter(|k| *k > 0)?);
        if k > e || total == 0 {
            return None;
        }
        let sh = self.shared_experts.unwrap_or(0) as f64;
        let share = (k as f64 + sh) / (e as f64 + sh);
        let t = total as f64;
        let a = match (self.num_hidden_layers, self.hidden_size) {
            (Some(l), Some(h)) => {
                let (l, h) = (l as f64, h as f64);
                let fixed = l * 4.0 * h * h + self.vocab_size.unwrap_or(150_000) as f64 * h;
                if fixed >= t {
                    return None;
                }
                fixed + (t - fixed) * share
            }
            _ => t * share,
        };
        Some(a.clamp(0.0, t).round() as u64)
    }
}

/// G_n per token of a model and how it was found (`Compute::gn`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gn {
    pub gn_per_token: Option<f64>,
    pub active_params: Option<u64>,
    pub basis: Option<&'static str>,
    /// Where `active_params` came from: `table`, `id` or `config_estimate`.
    pub active_source: Option<&'static str>,
}

/// The model whose USD per G_n prices FLOP (`flop.implied_flop_usd`).
pub const BASIS_MODEL: &str = "qwen/qwen3.8-27b";

/// G_n per token from FLOP's model registry (`gflopsPerToken`, SPEC.md and
/// INDEX.md), by OpenRouter id. Qwen2.5-0.5B (2) has no OpenRouter id here.
const REGISTRY: [(&str, f64); 2] = [("qwen/qwen3.8-27b", 52.0), ("qwen/qwen3-8b", 17.0)];

/// FLOP's published anchor DeepSeek-V3 → 74 G_n per token (yellowpaper
/// R4.2, flop-labs/yellowpaper#18: 685B weights, about 37B active), for
/// these ids only when their Hugging Face parameter count is DeepSeek-V3's.
const V3_ANCHOR: (f64, f64) = (74.0, 685e9);
const V3_IDS: [&str; 3] = ["deepseek/deepseek-chat-v3.1", "deepseek/deepseek-v3.2", "deepseek/deepseek-r1-0528"];
/// A parameter count within this share of the anchor's matches it.
const ANCHOR_TOL: f64 = 0.01;
/// Without a known active count, 2 × params is used only up to this many
/// weights: a model over it is almost certainly sparse.
pub const DENSE_MAX_PARAMS: u64 = 100_000_000_000;

/// Compute-unit settings: registry G_n per token and active parameter
/// counts by model id, and FLOP's price in FLOP per G_n (if given).
#[derive(Clone, Debug)]
pub struct Compute {
    pub registry: HashMap<String, f64>,
    pub active_params: HashMap<String, u64>,
    pub gn_price_flop: Option<f64>,
}

impl Default for Compute {
    fn default() -> Self {
        Self { registry: REGISTRY.iter().map(|(id, g)| (id.to_string(), *g)).collect(), active_params: HashMap::new(), gn_price_flop: None }
    }
}

/// Active parameters from an id that carries `-a<N>b` after its size, as
/// in `qwen3.5-397b-a17b` (17e9) or `qwen3-30b-a3b-instruct-2507` (3e9).
pub fn active_from_id(id: &str) -> Option<u64> {
    let name = id.rsplit('/').next()?;
    name.split('-').skip(1).find_map(|w| {
        let n = w.strip_prefix('a')?.strip_suffix('b')?;
        let v: f64 = n.parse().ok().filter(|_| n.bytes().all(|b| b.is_ascii_digit() || b == b'.'))?;
        Some((v * 1e9).round() as u64).filter(|a| *a > 0)
    })
}

impl Compute {
    /// Add (or override) entries from a file, `#` comments, one per line:
    /// `<model id> <G_n per token>` (registry) or `<model id> active_params
    /// <count>`. A bad line is an error.
    pub fn read_registry(&mut self, path: &Path) -> Result<()> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        for (i, l) in text.lines().enumerate() {
            let l = l.split('#').next().unwrap_or("").trim();
            if l.is_empty() {
                continue;
            }
            let f: Vec<&str> = l.split_whitespace().collect();
            match f[..] {
                [id, "active_params", n] => {
                    let n = n.parse::<u64>().ok().filter(|n| *n > 0).with_context(|| format!("{}:{}: active_params must be a whole number > 0", path.display(), i + 1))?;
                    self.active_params.insert(id.to_owned(), n);
                }
                [id, g] => {
                    let g = g.parse::<f64>().ok().filter(|g| g.is_finite() && *g > 0.0).with_context(|| format!("{}:{}: G_n per token must be a number > 0", path.display(), i + 1))?;
                    self.registry.insert(id.to_owned(), g);
                }
                _ => bail!("{}:{}: want `<model id> <G_n per token>` or `<model id> active_params <count>`", path.display(), i + 1),
            }
        }
        Ok(())
    }

    /// G_n per token for a model's ids (shortest first), parameter count
    /// and config, by the first that applies: the registry (FLOP's anchors
    /// included); 2 × active params from the table, else the id, else
    /// estimated from the config; 2 × params for a model of at most 100B
    /// weights whose config shows no experts; else unknown.
    /// A parameter count of 0 is unknown; a G_n that is not > 0 is none.
    pub fn gn(&self, ids: &[&str], params: Option<u64>, config: Option<&Config>) -> Gn {
        let params = params.filter(|p| *p > 0);
        let active = ids
            .iter()
            .find_map(|id| self.active_params.get(*id).filter(|a| **a > 0).map(|a| (*a, "table")))
            .or_else(|| ids.iter().find_map(|id| active_from_id(id).map(|a| (a, "id"))))
            .or_else(|| config.zip(params).and_then(|(c, p)| c.estimate_active(p)).map(|a| (a, "config_estimate")));
        let (active_params, active_source) = (active.map(|a| a.0), active.map(|a| a.1));
        let gn = |g: f64, basis| if g.is_finite() && g > 0.0 { Gn { gn_per_token: Some(g), active_params, basis: Some(basis), active_source } } else { Gn::default() };
        let anchor = ids.iter().any(|id| V3_IDS.contains(id)) && params.is_some_and(|p| (p as f64 / V3_ANCHOR.1 - 1.0).abs() <= ANCHOR_TOL);
        if let Some(g) = ids.iter().find_map(|id| self.registry.get(*id)) {
            return gn(*g, "flop_registry");
        }
        if anchor {
            return gn(V3_ANCHOR.0, "flop_registry");
        }
        match (active, params) {
            (Some((a, src)), _) => gn(2.0 * a as f64 / 1e9, if src == "config_estimate" { "2x_active_params_est" } else { "2x_active_params" }),
            (None, Some(p)) if p <= DENSE_MAX_PARAMS && !config.is_some_and(Config::has_experts) => gn(2.0 * p as f64 / 1e9, "2x_params_dense"),
            _ => Gn::default(),
        }
    }
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
    /// Model id → Hugging Face id, from `hf-map.tsv`.
    hf: HashMap<String, String>,
    /// Demand day files: size and (model id, line).
    demand: HashMap<PathBuf, (u64, Vec<(String, Demand)>)>,
    /// The index so far; `None` until a base is set.
    pub state: Option<IndexState>,
    pub compute: Compute,
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
        // Optional inputs: a missing or odd one is no data, not an error.
        self.hf = std::fs::read_to_string(dir.join("hf-map.tsv"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut f = l.split('\t');
                let (id, hf) = (f.next()?.trim(), f.next()?.trim());
                (!id.is_empty() && !hf.is_empty()).then(|| (id.to_owned(), hf.to_lowercase()))
            })
            .collect();
        self.update_demand(&dir.join("demand"), &first);
        let mut seen = BTreeSet::new();
        // Model directories are `<org>__<name>`; anything else is not a model.
        for d in entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().contains("__"))) {
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

    fn update_demand(&mut self, dir: &Path, first: &str) {
        let mut keep = Vec::new();
        for p in std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok().map(|e| e.path())) {
            let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            if p.extension().is_none_or(|x| x != "jsonl") || stem.len() != 10 || stem.as_str() < first {
                continue;
            }
            let Ok(len) = p.metadata().map(|m| m.len()) else { continue };
            if self.demand.get(&p).is_none_or(|f| f.0 != len) {
                let Ok(text) = std::fs::read(&p) else { continue };
                let lines = text
                    .split(|b| *b == b'\n')
                    .filter_map(|l| {
                        let v: Value = serde_json::from_slice(l).ok()?;
                        Some((v["model"].as_str()?.to_owned(), Demand { at_ms: v["at_ms"].as_u64()?, downloads: v["downloads_30d"].as_u64(), likes: v["likes"].as_u64(), params: v["params"].as_u64(), config: Config::from_json(&v["config"]) }))
                    })
                    .collect();
                self.demand.insert(p.clone(), (len, lines));
            }
            keep.push(p);
        }
        self.demand.retain(|p, _| keep.contains(p));
    }

    /// Start from a saved index state (see `IndexState::load`).
    pub fn with_state(state: Option<IndexState>) -> Self {
        Self { state, ..Self::default() }
    }

    /// Advance the index over what was read and build `market.json`. Save
    /// `self.state` afterwards (`index-state.json`).
    pub fn market_json(&mut self, now_ms: u64) -> Result<Value> {
        let from = now_ms.saturating_sub(HOLD_MS);
        // Aliases: a dated snapshot joins its undated model (`alias_of`).
        let ids: BTreeSet<&str> = self.models.keys().map(String::as_str).collect();
        let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for id in &ids {
            groups.entry(alias_of(id, &ids, &self.hf).unwrap_or(id)).or_default().push(id);
        }
        let models: Vec<Model> = groups
            .into_values()
            .map(|mut ids| {
                ids.sort_by_key(|id| (id.len(), *id));
                let per: Vec<Vec<&Snap>> = ids
                    .iter()
                    .map(|id| {
                        let mut v: Vec<&Snap> = self.models[*id].values().flat_map(|f| &f.snaps).filter(|s| s.at_ms >= from && s.at_ms <= now_ms + MAX_FUTURE_MS).collect();
                        v.sort_by_key(|s| s.at_ms);
                        v
                    })
                    .collect();
                Model::group(ids, per, &self.quotes.prov)
            })
            .collect();
        advance(&models, &mut self.state, now_ms);
        let mut demand: HashMap<&str, Vec<&Demand>> = HashMap::new();
        for (m, d) in self.demand.values().flat_map(|f| &f.1) {
            demand.entry(m.as_str()).or_default().push(d);
        }
        Ok(market(&models, &self.quotes.names, &self.quotes.prov, self.state.as_ref(), &demand, &self.compute, now_ms))
    }
}

/// The market feed of one out dir: loads `index-state.json` when first
/// needed, and again after a failure, so a bad state file stops only
/// `market.json` (each run returns the error) until it is fixed.
pub struct MarketFeed {
    pub dir: PathBuf,
    pub compute: Compute,
    rebase: bool,
    wide: Option<Wide>,
}

impl MarketFeed {
    /// `rebase`: ignore any saved state once and start a new base.
    pub fn new(dir: PathBuf, rebase: bool) -> Self {
        Self { dir, compute: Compute::default(), rebase, wide: None }
    }

    /// Read new data, step the index, write `index-state.json` then `market.json`.
    pub fn run(&mut self, out: &Path, now_ms: u64) -> Result<()> {
        if self.wide.is_none() {
            let state = if self.rebase { None } else { IndexState::load(&out.join("index-state.json"))? };
            self.rebase = false;
            self.wide = Some(Wide { compute: self.compute.clone(), ..Wide::with_state(state) });
        }
        let w = self.wide.as_mut().unwrap();
        w.update(&self.dir, now_ms)?;
        let market = w.market_json(now_ms)?;
        if let Some(s) = &w.state {
            crate::feed::write_atomic(out, "index-state.json", &serde_json::to_value(s)?)?;
        }
        crate::feed::write_atomic(out, "market.json", &market)
    }
}

/// One model (a group of alias ids) and its median at each snapshot.
struct Model<'a> {
    /// The shortest id in the group, and the others.
    id: &'a str,
    aliases: Vec<&'a str>,
    /// For a group, each member fetch gives a snapshot of every member's
    /// latest listing (within a day), merged by (provider, quantization).
    snaps: Vec<Cow<'a, Snap>>,
    /// Per snapshot: the member's own fetch behind it, which member that
    /// is, and the bitmask of members listed in it.
    fetched: Vec<&'a Snap>,
    src: Vec<usize>,
    members: Vec<u64>,
    /// Median over distinct providers (held ones included), or the last one
    /// carried forward.
    median: Vec<Option<f64>>,
    /// At the newest snapshot: each provider in the median's set at its
    /// lowest (held) price: (provider, price, (prompt, completion)).
    held_now: Vec<Held>,
    /// Whether `median` was computed at that snapshot (not carried).
    fresh: Vec<bool>,
}

/// The prices in a model's median after member `k`'s fetch `s`: every
/// listing of every member seen up within `HOLD_DOWN_MS`, at its last up
/// price (so a listing that is down now is held), then one per provider
/// (across precisions, regions and aliases): its lowest. Sorted by price.
type Held = (u32, f64, [f64; 2]);
/// (member, listing) → (quote, last up price, its (prompt, completion), when last seen up).
type UpSince = HashMap<(usize, u32), (u32, f64, [f64; 2], u64)>;

fn held(s: &Snap, k: usize, prov: &[u32], up: &mut UpSince) -> Vec<Held> {
    for r in s.raw.iter().filter(|r| r.up) {
        if let (Some(p), Some(pc)) = (r.price, r.pc) {
            up.insert((k, r.listing), (r.quote, p, pc, s.at_ms));
        }
    }
    up.retain(|_, (_, _, _, t)| *t <= s.at_ms && s.at_ms - *t <= HOLD_DOWN_MS);
    let mut low: HashMap<u32, (f64, [f64; 2])> = HashMap::new();
    for (q, p, pc, _) in up.values() {
        let e = low.entry(prov[*q as usize]).or_insert((*p, *pc));
        // Ties at the 15:1 price: the lower prompt price, so the pick is stable.
        if p.total_cmp(&e.0).then(pc[0].total_cmp(&e.1[0])).is_lt() {
            *e = (*p, *pc);
        }
    }
    let mut v: Vec<Held> = low.into_iter().map(|(k, (p, pc))| (k, p, pc)).collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    v
}

fn usable(s: &Snap) -> Vec<f64> {
    let mut v: Vec<f64> = s.rows.iter().filter(|r| r.up).filter_map(|r| r.price).collect();
    v.sort_by(f64::total_cmp);
    v
}

impl<'a> Model<'a> {
    /// `ids` shortest first; `per` each id's snapshots in time order.
    fn group(ids: Vec<&'a str>, per: Vec<Vec<&'a Snap>>, prov: &[u32]) -> Self {
        let (mut snaps, mut fetched, mut src, mut members) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        if per.len() == 1 {
            snaps = per[0].iter().map(|s| Cow::Borrowed(*s)).collect();
            fetched = per[0].clone();
            src = vec![0; snaps.len()];
            members = vec![1; snaps.len()];
        } else {
            let mut ev: Vec<(u64, usize, &Snap)> = per.iter().enumerate().flat_map(|(k, v)| v.iter().map(move |s| (s.at_ms, k, *s))).collect();
            ev.sort_by_key(|e| (e.0, e.1));
            let mut latest: Vec<Option<&Snap>> = vec![None; per.len()];
            for (t, k, s) in ev {
                latest[k] = Some(s);
                let live: Vec<(usize, &Snap)> = latest.iter().enumerate().filter_map(|(j, l)| l.filter(|l| t - l.at_ms <= DROP_MS).map(|l| (j, l))).collect();
                snaps.push(Cow::Owned(Snap { at_ms: t, rows: dedup(live.iter().flat_map(|(_, l)| l.rows.iter().copied()).collect()), raw: Vec::new() }));
                fetched.push(s);
                src.push(k);
                members.push(live.iter().fold(0u64, |m, (j, _)| m | 1u64 << (*j).min(63)));
            }
        }
        Self::new(ids[0], ids[1..].to_vec(), snaps, fetched, src, members, prov)
    }

    /// The member ids in a bitmask from `members`.
    fn member_ids(&self, mask: u64) -> Vec<String> {
        std::iter::once(self.id).chain(self.aliases.iter().copied()).enumerate().filter(|(j, _)| mask & (1u64 << (*j).min(63)) != 0).map(|(_, id)| id.to_owned()).collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn new(id: &'a str, aliases: Vec<&'a str>, snaps: Vec<Cow<'a, Snap>>, fetched: Vec<&'a Snap>, src: Vec<usize>, members: Vec<u64>, prov: &[u32]) -> Self {
        let (mut median, mut fresh, mut last, mut held_now) = (Vec::new(), Vec::new(), None, Vec::new());
        let mut up = UpSince::new();
        for (f, k) in fetched.iter().zip(&src) {
            held_now = held(f, *k, prov, &mut up);
            let ok = held_now.len() >= MIN_QUOTES;
            if ok {
                last = Some(percentile(&held_now.iter().map(|h| h.1).collect::<Vec<_>>(), 0.5));
            }
            median.push(last);
            fresh.push(ok);
        }
        Self { id, aliases, snaps, fetched, src, members, median, held_now, fresh }
    }

    /// The median at the last snapshot at or before `t`.
    fn median_at(&self, t: u64) -> Option<f64> {
        let n = self.snaps.partition_point(|s| s.at_ms <= t);
        n.checked_sub(1).and_then(|i| self.median[i])
    }
}

/// Input:output token ratios of `mix`: chat 3:1, average 15:1, agent 150:1.
pub const MIXES: [f64; 3] = [3.0, 15.0, 150.0];

/// USD per 1M tokens → USD per billion G_n at `gn` G_n per token.
pub fn usd_per_bn_gn(usd_per_m: f64, gn: f64) -> f64 {
    usd_per_m / (gn * 1e6) * 1e9
}

/// The median over a model's median set of each provider's price at `r`:1.
fn mix_median(set: &[Held], r: f64) -> Option<f64> {
    quantile(&mut set.iter().map(|h| mix(h.2, r)).collect::<Vec<_>>(), 0.5)
}

/// The median over a model's median set of completion ÷ prompt (a free
/// prompt has none).
fn out_in_ratio(set: &[Held]) -> Option<f64> {
    quantile(&mut set.iter().filter(|h| h.2[0] > 0.0).map(|h| h.2[1] / h.2[0]).collect::<Vec<_>>(), 0.5)
}

/// `percentile` of unsorted values, `None` when empty.
fn quantile(v: &mut [f64], q: f64) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    (!v.is_empty()).then(|| percentile(v, q))
}

/// Routed price points per model, at most.
pub const ROUTED_POINTS: usize = 300;
/// The most of a model's routed weight one provider may carry.
pub const MAX_SHARE: f64 = 0.5;

/// Shares ∝ `w`, no share over `cap` (the excess goes to the others in
/// proportion to their weight) when at least 1 / `cap` weights are above 0.
/// Also whether the cap acted.
pub fn route_shares(w: &[f64], cap: f64) -> (Vec<f64>, bool) {
    let total: f64 = w.iter().sum();
    let mut s: Vec<f64> = w.iter().map(|x| x / total).collect();
    if (w.iter().filter(|x| **x > 0.0).count() as f64) * cap < 1.0 {
        return (s, false);
    }
    let (mut fixed, mut capped) = (vec![false; w.len()], false);
    loop {
        let free = 1.0 - cap * fixed.iter().filter(|f| **f).count() as f64;
        let rest: f64 = w.iter().zip(&fixed).filter(|(_, f)| !**f).map(|(x, _)| x).sum();
        let mut again = false;
        for i in 0..w.len() {
            if fixed[i] {
                continue;
            }
            s[i] = w[i] / rest * free;
            if s[i] > cap * (1.0 + 1e-12) {
                (s[i], fixed[i], again, capped) = (cap, true, true, true);
            }
        }
        if !again {
            return (s, capped);
        }
    }
}

/// What a request pays now, spread over a model's sellers the way
/// OpenRouter's default routing spreads traffic.
#[derive(Debug, PartialEq)]
pub struct Routed {
    pub price: f64,
    /// (quote, price, share), largest share first; shares sum to 1.
    pub sellers: Vec<(u32, f64, f64)>,
    pub uptime_fallback: bool,
    pub capped: bool,
}

/// Over the up, priced quotes of a snapshot, one per provider (its lowest),
/// weight ∝ uptime_30m / 100 ÷ price² (no recent uptime: 0; all 0: 1 ÷
/// price² each), shares capped at `MAX_SHARE`; price = Σ share × price.
fn routed(rows: &[Row], prov: &[u32]) -> Option<Routed> {
    let mut low: BTreeMap<u32, (u32, f64, Option<f64>)> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.up) {
        let Some(p) = r.price.filter(|p| *p > 0.0) else { continue };
        let e = low.entry(prov[r.quote as usize]).or_insert((r.quote, p, r.uptime));
        if p < e.1 {
            *e = (r.quote, p, r.uptime);
        }
    }
    let picks: Vec<(u32, f64, Option<f64>)> = low.into_values().collect();
    if picks.is_empty() {
        return None;
    }
    let mut w: Vec<f64> = picks.iter().map(|(_, p, u)| u.map_or(0.0, |u| u / 100.0) / (p * p)).collect();
    let uptime_fallback = !w.iter().any(|x| *x > 0.0);
    if uptime_fallback {
        w = picks.iter().map(|(_, p, _)| 1.0 / (p * p)).collect();
    }
    let (share, capped) = route_shares(&w, MAX_SHARE);
    let price = picks.iter().zip(&share).map(|((_, p, _), s)| p * s).sum();
    let mut sellers: Vec<(u32, f64, f64)> = picks.iter().zip(&share).filter(|(_, s)| **s > 0.0).map(|((q, p, _), s)| (*q, *p, *s)).collect();
    sellers.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.1.total_cmp(&b.1)));
    Some(Routed { price, sellers, uptime_fallback, capped })
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
    /// The alias ids listed when it joined (empty: just the model's id).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
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
    /// The method version that wrote it (0: before versions were saved).
    #[serde(default)]
    pub method: u32,
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

    /// One step at snapshot `t` with the models in the index now (id →
    /// (median, alias ids listed)). Models leaving: the remaining bases are
    /// scaled so the level at the last inputs is unchanged. Then the level
    /// is the Jevons index over the models staying, each input being its
    /// median with the move from its last input capped at `MAX_STEP_RATIO`.
    /// Models joining: base = median / (level / 100). A model whose listed
    /// aliases changed leaves and joins again in the same step.
    fn step(&mut self, t: u64, active: &BTreeMap<&str, (f64, Vec<String>)>) {
        let same = |k: &str, b: &Basket, now: &[String]| if b.members.is_empty() { now.len() == 1 && now[0] == k } else { b.members == now };
        let gone: Vec<String> = self.models.iter().filter(|(k, b)| active.get(k.as_str()).is_none_or(|a| !same(k, b, &a.1))).map(|(k, _)| k.clone()).collect();
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
        let input = |k: &str, b: &Basket| b.last * (active[k].0 / b.last).clamp(1.0 / MAX_STEP_RATIO, MAX_STEP_RATIO);
        // No input moved: the level stays exactly (no float drift after a rescale).
        if self.models.iter().any(|(k, b)| input(k, b) != b.last) {
            let mean = self.models.iter().map(|(k, b)| (input(k, b) / b.base).ln()).sum::<f64>() / self.models.len() as f64;
            self.level = 100.0 * mean.exp();
            let inputs: Vec<(String, f64)> = self.models.iter().map(|(k, b)| (k.clone(), input(k, b))).collect();
            for (k, x) in inputs {
                self.models.get_mut(&k).unwrap().last = x;
            }
        }
        for (k, (m, members)) in active {
            self.models.entry((*k).to_owned()).or_insert_with(|| Basket { base: m / (self.level / 100.0), last: *m, members: members_field(k, members) });
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

/// Members as stored: empty when it is just the model's own id.
fn members_field(id: &str, members: &[String]) -> Vec<String> {
    if members.len() == 1 && members[0] == id { Vec::new() } else { members.to_vec() }
}

/// Advance the chain-linked Jevons index over snapshots after the state's
/// last step. Without a state, the base is the first snapshot where every
/// model priced within the first day of the data has a median.
fn advance(models: &[Model], state: &mut Option<IndexState>, now_ms: u64) {
    // Cycles: snapshots of all models in time order; a new cycle starts after
    // a pause or when a model repeats. Its time is its last snapshot.
    // A repeat is the same fetched id (an alias group has one snapshot per member fetch).
    let mut ev: Vec<(u64, usize, usize, usize)> = models.iter().enumerate().flat_map(|(m, x)| x.snaps.iter().enumerate().map(move |(i, s)| (s.at_ms, m, i, x.src[i]))).collect();
    ev.sort();
    let mut cycles: Vec<Vec<(u64, usize, usize, usize)>> = Vec::new();
    for e in ev {
        match cycles.last_mut() {
            Some(c) if e.0 - c.last().unwrap().0 <= CYCLE_GAP_MS && !c.iter().any(|x| x.1 == e.1 && x.3 == e.3) => c.push(e),
            _ => cycles.push(vec![e]),
        }
    }
    let t_of = |c: &Vec<(u64, usize, usize, usize)>| c.last().unwrap().0;
    // A state from another method version: once, each model's input at the
    // last step is recomputed from the data and its base scaled by the same
    // ratio, so the level at that step is unchanged. Never otherwise: a
    // stepped time is not revisited.
    if let Some(st) = state.as_mut().filter(|st| st.method != METHOD) {
        for (k, b) in st.models.iter_mut() {
            if let Some(r) = models.iter().find(|m| m.id == k).and_then(|m| m.median_at(st.at_ms)).filter(|r| *r != b.last) {
                b.base *= r / b.last;
                b.last = r;
            }
        }
        st.method = METHOD;
    }
    // Snapshots up to the last step were used (or were late): for the index,
    // a model in it keeps its last input until a newer snapshot.
    let stepped = state.as_ref().map(|s| s.at_ms);
    let cutoff = now_ms.saturating_sub(STEP_LAG_MS);
    let mut base = None;
    if state.is_none() {
        let Some(t_first) = cycles.first().map(t_of) else { return };
        let mut first_fresh: Vec<Option<u64>> = vec![None; models.len()];
        for c in &cycles {
            for &(_, m, i, _) in c {
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
    let mut old: Vec<Option<f64>> = vec![None; models.len()];
    let mut last_fresh: Vec<Option<u64>> = vec![None; models.len()];
    let mut listed: Vec<u64> = vec![0; models.len()];
    for c in cycles.iter().take_while(|c| t_of(c) <= cutoff) {
        let t = t_of(c);
        let done = stepped.is_some_and(|s| t <= s);
        for &(_, m, i, _) in c {
            listed[m] = models[m].members[i];
            if models[m].median[i].is_some() {
                if done { old[m] = models[m].median[i] } else { cur[m] = models[m].median[i] }
            }
            if models[m].fresh[i] {
                last_fresh[m] = Some(t);
            }
        }
        if done || base.is_some_and(|b| t < b) {
            continue;
        }
        let input = |m: usize| cur[m].or_else(|| state.as_ref().and_then(|s| s.models.get(models[m].id)).map(|b| b.last)).or(old[m]);
        // In the index: a median, and a fresh one within the last day.
        let active: BTreeMap<&str, (f64, Vec<String>)> = (0..models.len()).filter_map(|m| Some((models[m].id, (input(m).filter(|_| last_fresh[m].is_some_and(|f| t - f <= DROP_MS))?, models[m].member_ids(listed[m]))))).collect();
        match state {
            Some(s) => s.step(t, &active),
            None => {
                let models = active.iter().map(|(k, (m, mem))| ((*k).to_owned(), Basket { base: *m, last: *m, members: members_field(k, mem) })).collect();
                *state = Some(IndexState { v: 1, base_ms: t, at_ms: t, level: 100.0, models, series: vec![(t, 100.0)], method: METHOD });
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

/// The demand block of a model from its own lines, else its aliases'.
fn demand_json(m: &Model, demand: &HashMap<&str, Vec<&Demand>>) -> Value {
    let lines = std::iter::once(m.id).chain(m.aliases.iter().copied()).find_map(|id| demand.get(id).filter(|v| !v.is_empty()));
    let Some(lines) = lines else { return json!({ "hf_downloads_30d": null, "hf_likes": null, "hf_downloads_change_7d_pct": null, "as_of_ms": null }) };
    let new = lines.iter().max_by_key(|d| d.at_ms).unwrap();
    let old = lines.iter().filter(|d| d.at_ms + 6 * DAY_MS <= new.at_ms).max_by_key(|d| d.at_ms);
    let change = match (new.downloads, old.and_then(|o| o.downloads)) {
        (Some(n), Some(o)) if o > 0 => Some((n as f64 / o as f64 - 1.0) * 100.0),
        _ => None,
    };
    json!({ "hf_downloads_30d": new.downloads, "hf_likes": new.likes, "hf_downloads_change_7d_pct": change, "as_of_ms": new.at_ms })
}

#[allow(clippy::too_many_arguments)]
fn market(models: &[Model], names: &[(String, String)], prov: &[u32], state: Option<&IndexState>, demand: &HashMap<&str, Vec<&Demand>>, compute: &Compute, now_ms: u64) -> Value {
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
    // Over non-thin models: USD per bn G_n, mixes, completion ÷ prompt.
    let (mut per_gn, mut mixes, mut ratios) = (Vec::new(), [Vec::new(), Vec::new(), Vec::new()], Vec::new());
    let (mut basis_gn, mut n_unknown, mut n_estimated) = (None, 0, 0);
    // The benchmark's routed price, its USD per bn G_n, series and time.
    type Bench = (Option<f64>, Option<f64>, Vec<Value>, u64);
    let mut bench: Option<Bench> = None;
    for m in models {
        let Some(latest) = m.snaps.last() else { continue };
        // Stale: nothing fetched for this model for 30 minutes; none of its quotes is up.
        let stale = now_ms.saturating_sub(latest.at_ms) > STALE_MS;
        let is_up = |r: &Row| r.up && !stale;
        // Uptime: share of the model's snapshots in the last day where the quote was listed and up.
        let recent: Vec<&Snap> = m.snaps.iter().map(|s| &**s).filter(|s| s.at_ms >= day_ago).collect();
        let uptime = |q: u32| -> (usize, usize) { (recent.iter().filter(|s| s.rows.iter().any(|r| r.quote == q && r.up)).count(), recent.len()) };
        // min, max and cheapest: quotes up now. median, p25, p75: the index
        // input's set (one price per provider, held ones included).
        let u: Vec<f64> = if stale { Vec::new() } else { usable(latest) };
        let h: Vec<f64> = m.held_now.iter().map(|x| x.1).collect();
        let thin = h.len() < MIN_QUOTES;
        n_quotes += latest.rows.len();
        n_up += latest.rows.iter().filter(|r| is_up(r)).count();
        let Some(median) = m.median.last().copied().flatten().or_else(|| (!h.is_empty()).then(|| percentile(&h, 0.5))).or_else(|| (!u.is_empty()).then(|| percentile(&u, 0.5))) else { continue };
        let spread = |q: f64| if h.is_empty() { median } else { percentile(&h, q) };
        let range = |q: f64| if u.is_empty() { median } else { percentile(&u, q) };
        let up_provs: BTreeSet<u32> = latest.rows.iter().filter(|r| is_up(r) && r.price.is_some()).map(|r| prov[r.quote as usize]).collect();
        let quotes_held = m.held_now.iter().filter(|x| !up_provs.contains(&x.0)).count();
        let cheapest = latest.rows.iter().filter(|r| is_up(r)).filter_map(|r| r.price.map(|p| (r.quote, p))).min_by(|a, b| a.1.total_cmp(&b.1));
        let mut quotes: Vec<(&Row, (usize, usize))> = latest.rows.iter().map(|r| (r, uptime(r.quote))).collect();
        quotes.sort_by(|a, b| (!is_up(a.0), a.0.price.is_none()).cmp(&(!is_up(b.0), b.0.price.is_none())).then(a.0.price.unwrap_or(0.0).total_cmp(&b.0.price.unwrap_or(0.0))));
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
        // Compute units: G_n per token (`Compute::gn`).
        let ids = || std::iter::once(m.id).chain(m.aliases.iter().copied());
        // The newest line with a parameter count, and its config.
        let line = ids().filter_map(|id| demand.get(id)).flatten().filter(|d| d.params.is_some()).max_by_key(|d| d.at_ms);
        let params = line.and_then(|d| d.params);
        let c = compute.gn(&ids().collect::<Vec<_>>(), params, line.and_then(|d| d.config.as_ref()));
        let gn = c.gn_per_token;
        let per_bn = |usd_per_m: f64| gn.map(|g| usd_per_bn_gn(usd_per_m, g));
        let mix_at: Vec<Option<f64>> = MIXES.iter().map(|r| mix_median(&m.held_now, *r)).collect();
        let out_in = out_in_ratio(&m.held_now);
        // Totals: models neither thin nor stale, finite values only.
        let finite = |x: Option<f64>| x.filter(|x| x.is_finite());
        if !thin && !stale {
            let u = finite(per_bn(median));
            per_gn.extend(u);
            n_unknown += usize::from(u.is_none());
            n_estimated += usize::from(u.is_some() && c.basis == Some("2x_active_params_est"));
            for (v, x) in mixes.iter_mut().zip(&mix_at) {
                v.extend(finite(*x));
            }
            ratios.extend(finite(out_in));
            if ids().any(|id| id == BASIS_MODEL) {
                basis_gn = u;
            }
        }
        // Routed price: at every snapshot (series), and now (none if stale).
        let r_pts: Vec<(u64, f64)> = m.snaps.iter().filter_map(|s| Some((s.at_ms, routed(&s.rows, prov)?.price))).collect();
        let r_now = if stale { None } else { routed(&latest.rows, prov) };
        let r_price = r_now.as_ref().map(|r| r.price);
        let r_series: Vec<Value> = steps(&r_pts, ROUTED_POINTS).iter().map(|p| json!([p.0, p.1])).collect();
        let routed_gn = r_price.and_then(per_bn).filter(|x| x.is_finite());
        let routed_json = json!({
            "price": r_price,
            "sellers": r_now.as_ref().map_or(0, |r| r.sellers.len()),
            "top": r_now.as_ref().map_or(Vec::new(), |r| r.sellers.iter().take(5).map(|(q, p, sh)| json!({ "provider": names[*q as usize].0, "quantization": names[*q as usize].1, "price": p, "share": sh })).collect()),
            "change_1h_pct": r_price.and_then(|p| pct(p, at_or_before(&r_pts, now_ms.saturating_sub(3_600_000)))),
            "change_24h_pct": r_price.and_then(|p| pct(p, at_or_before(&r_pts, day_ago))),
            "uptime_fallback": r_now.as_ref().is_some_and(|r| r.uptime_fallback),
            "capped": r_now.as_ref().is_some_and(|r| r.capped),
            "series": r_series,
        });
        if ids().any(|id| id == BASIS_MODEL) {
            bench = Some((r_price, routed_gn, r_series.clone(), latest.at_ms));
        }
        // Quotes listed now that were not listed 7 days ago (null without data that old).
        let week_ago = now_ms.saturating_sub(7 * DAY_MS);
        let then = m.snaps.partition_point(|s| s.at_ms <= week_ago).checked_sub(1).map(|i| &m.snaps[i]);
        let added = then.map(|then| latest.rows.iter().filter(|r| !then.rows.iter().any(|o| o.quote == r.quote)).count());
        out_models.push(json!({
            "id": m.id, "aliases": m.aliases, "name": name, "family": family,
            "demand": demand_json(m, demand), "sellers_added_7d": added,
            "data_ms": latest.at_ms, "stale": stale,
            "sellers_total": latest.rows.len(), "sellers_up": latest.rows.iter().filter(|r| is_up(r)).count(),
            "median": median, "p25": spread(0.25), "p75": spread(0.75), "min": range(0.0), "max": range(1.0),
            "thin": thin, "quotes_held": quotes_held,
            "compute": { "params": params, "active_params": c.active_params, "active_params_source": c.active_source, "gn_per_token": gn, "basis": c.basis, "source": c.basis },
            "usd_per_bn_gn": per_bn(median), "usd_per_bn_gn_min": per_bn(range(0.0)), "usd_per_bn_gn_max": per_bn(range(1.0)),
            "mix": { "chat_3_1": mix_at[0], "avg_15_1": mix_at[1], "agent_150_1": mix_at[2] },
            "out_in_ratio": out_in,
            "routed": routed_json, "routed_usd_per_bn_gn": routed_gn,
            "cheapest": cheapest.map(|(q, p)| json!({ "provider": names[q as usize].0, "quantization": names[q as usize].1, "price": p })),
            "change_24h_pct": pct(median, m.median_at(day_ago)),
            "series": steps(&pts, MODEL_POINTS).iter().map(|p| json!([p.0, p.1])).collect::<Vec<_>>(),
            "quotes": quotes.iter().map(|(r, (up, of))| json!({
                "provider": names[r.quote as usize].0, "quantization": names[r.quote as usize].1,
                "price": r.price, "up": is_up(r), "uptime_24h_pct": (*of > 0).then(|| *up as f64 / *of as f64 * 100.0),
            })).collect::<Vec<_>>(),
        }));
        // Price changes over the last 7 days, per listing of each member id
        // as fetched (never across aliases): its first price is not a change.
        let mut last: HashMap<(usize, u32), f64> = HashMap::new();
        // (member, listing) → (quote, snapshots priced, changes (at, from, to), min, max).
        type Per = (u32, usize, Vec<(u64, f64, f64)>, f64, f64);
        let mut per: BTreeMap<(usize, u32), Per> = BTreeMap::new();
        for (f, k) in m.fetched.iter().zip(&m.src).filter(|(f, _)| f.at_ms >= now_ms.saturating_sub(CHANGES_MS)) {
            for r in &f.raw {
                let Some(p) = r.price else { continue };
                let e = per.entry((*k, r.listing)).or_insert((r.quote, 0, Vec::new(), p, p));
                e.1 += 1;
                (e.3, e.4) = (e.3.min(p), e.4.max(p));
                if let Some(old) = last.insert((*k, r.listing), p) {
                    if (p - old).abs() > CHANGE_REL * old {
                        e.2.push((f.at_ms, old, p));
                    }
                }
            }
        }
        for (q, snaps, ch, lo, hi) in per.into_values() {
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
    let mut ranked: Vec<(&str, u64)> = out_models.iter().filter_map(|m| Some((m["id"].as_str()?, m["demand"]["hf_downloads_30d"].as_u64()?))).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let demand_rank: Vec<&str> = ranked.into_iter().map(|r| r.0).collect();
    let mut providers: Vec<Value> = provs
        .iter()
        .map(|(name, (ms, q, up, of, cheap))| json!({ "name": name, "models": ms.len(), "quotes": q, "uptime_24h_pct": (*of > 0).then(|| *up as f64 / *of as f64 * 100.0), "cheapest_in": cheap }))
        .collect();
    providers.sort_by(|a, b| b["models"].as_u64().cmp(&a["models"].as_u64()).then(a["name"].as_str().cmp(&b["name"].as_str())));
    // USD per G_n ÷ FLOP per G_n = USD per FLOP.
    let to_flop = |usd_per_bn: Option<f64>| usd_per_bn.zip(compute.gn_price_flop).filter(|(_, f)| *f > 0.0).map(|(u, f)| u / 1e9 / f);
    let routed_bench = bench.as_ref().and_then(|b| b.1);
    let (implied, implied_routed) = (to_flop(basis_gn), to_flop(routed_bench));
    let first = models.iter().filter_map(|m| m.snaps.first()).map(|s| s.at_ms).min();
    let data_ms = models.iter().filter_map(|m| m.snaps.last()).map(|s| s.at_ms).max();
    json!({
        "generated_ms": now_ms,
        "data_ms": data_ms,
        "window": { "from_ms": first, "to_ms": data_ms },
        "index": {
            "level": level, "base_ms": base,
            "change_24h_pct": level.and_then(|l| pct(l, at_or_before(series, day_ago))),
            "change_7d_pct": level.and_then(|l| pct(l, at_or_before(series, now_ms.saturating_sub(7 * DAY_MS)))),
            "n_models": n_models, "n_quotes": n_quotes, "n_quotes_up": n_up, "n_providers": providers.len(),
            "series": steps(series, INDEX_POINTS).iter().map(|p| json!([p.0, p.1])).collect::<Vec<_>>(),
        },
        "compute_price": {
            "median_usd_per_bn_gn": quantile(&mut per_gn, 0.5), "p25": quantile(&mut per_gn, 0.25), "p75": quantile(&mut per_gn, 0.75), "n_models": per_gn.len(), "n_estimated": n_estimated, "n_unknown": n_unknown,
        },
        "mixes": { "chat_3_1": quantile(&mut mixes[0], 0.5), "avg_15_1": quantile(&mut mixes[1], 0.5), "agent_150_1": quantile(&mut mixes[2], 0.5) },
        "out_in_ratio_median": quantile(&mut ratios, 0.5),
        "routed": {
            "benchmark": BASIS_MODEL, "price": bench.as_ref().and_then(|b| b.0), "usd_per_bn_gn": routed_bench,
            "series": bench.as_ref().map_or(Vec::new(), |b| b.2.clone()),
        },
        "flop": {
            "gn_price_flop": compute.gn_price_flop, "basis_model": BASIS_MODEL,
            "implied_flop_usd": implied, "implied_flop_usd_routed": implied_routed,
            "parity": {
                "market_usd_per_bn_gn": routed_bench, "flop_rate_flop_per_gn": compute.gn_price_flop,
                "implied_flop_usd": implied, "implied_flop_usd_routed": implied_routed,
                "as_of_ms": bench.as_ref().map(|b| b.3),
            },
        },
        "models": out_models,
        "demand_rank": demand_rank,
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
        w.market_json(now + STEP_LAG_MS).unwrap()
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
        assert_eq!((a["thin"].as_bool(), a["median"].as_f64(), a["min"].as_f64(), a["max"].as_f64()), (Some(false), Some(2.0), Some(1.0), Some(4.0)));
        // C is down but held: it still counts, and the card says so.
        assert_eq!(a["quotes_held"], 1);
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
            lines.push(three("x/a", *at, 1.0, if k == 0 { 2.0 } else { 2.4 })); // +20% at t1
            lines.push(three("x/b", at + 1000, 1.0, 3.0));
            lines.push(three("x/c", at + 2000, 1.0, 5.0));
            if k >= 2 {
                lines.push(three("x/d", at + 3000, 1.0, if k == 2 { 7.0 } else { 8.4 })); // joins at t2, +20% at t3
            }
        }
        put(&d, &lines);
        let v = run(&d, t[3] + MIN);
        let l1 = 100.0 * 1.2f64.powf(1.0 / 3.0);
        let got = levels(&v);
        assert_eq!(got.len(), 4);
        assert!(close(got[1], l1) && close(got[2], l1), "{got:?}");
        assert!(close(got[3], l1 * 1.2f64.powf(0.25)), "{got:?}");
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
            lines.push(three("x/a", *at, 1.0, if k == 0 { 2.0 } else { 2.4 })); // +20% at t1
            lines.push(three("x/b", at + 1000, 1.0, if k < 4 { 3.0 } else { 3.6 })); // +20% at t4
            lines.push(three("x/c", at + 2000, 1.0, 5.0));
            match k {
                0 => lines.push(three("x/d", at + 3000, 1.0, 7.0)),
                1 => lines.push(three("x/d", at + 3000, 1.0, 7.7)), // +10%, then goes quiet
                _ => {}
            }
        }
        put(&d, &lines);
        let v = run(&d, t[4] + MIN);
        let got = levels(&v);
        let l1 = 100.0 * (1.2f64 * 1.1).powf(0.25);
        // The chart keeps both sides of each change: t0, t1, then t3 (D just
        // dropped, level unchanged) and t4 (B up 20% among the three left).
        let times: Vec<u64> = v["index"]["series"].as_array().unwrap().iter().map(|p| p[0].as_u64().unwrap()).collect();
        assert_eq!(times, [t[0] + 3000, t[1] + 3000, t[3] + 2000, t[4] + 2000]);
        assert!(close(got[1], l1) && close(got[2], l1), "{got:?}");
        assert!(close(got[3], l1 * 1.2f64.powf(1.0 / 3.0)), "{got:?}");
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
        let v = w.market_json(T0).unwrap();
        assert_eq!((v["models"].clone(), v["data_ms"].clone(), v["index"]["level"].clone(), v["index"]["base_ms"].clone()), (json!([]), Value::Null, Value::Null, Value::Null));
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
        put(&d, &[cycle(T0, 2.0), cycle(T0 + 5 * MIN, 2.4)].concat());
        let mut w = Wide::default();
        w.update(&d, T0 + 6 * MIN).unwrap();
        w.market_json(T0 + 6 * MIN + STEP_LAG_MS).unwrap();
        let l1 = 100.0 * 1.2f64.powf(1.0 / 3.0);
        assert!(close(w.state.as_ref().unwrap().level, l1));
        let saved = reload(&w, &out);
        assert_eq!(saved, w.state);
        // The feed is down for 6 hours while prices move; a new process
        // continues from the saved bases.
        put(&d, &[cycle(T0 + 3 * HOUR, 2.88), cycle(T0 + 6 * HOUR, 2.7)].concat());
        let mut again = Wide::with_state(saved);
        again.update(&d, T0 + 6 * HOUR + MIN).unwrap();
        let v = again.market_json(T0 + 6 * HOUR + MIN + STEP_LAG_MS).unwrap();
        assert_eq!(v["index"]["base_ms"], T0 + 2000);
        // Level from the bases alone: A at 2.7 vs base 2, B and C flat.
        assert!(close(v["index"]["level"].as_f64().unwrap(), 100.0 * 1.35f64.powf(1.0 / 3.0)));
        // The same as a process that never stopped.
        w.update(&d, T0 + 6 * HOUR + MIN).unwrap();
        let cont = w.market_json(T0 + 6 * HOUR + MIN + STEP_LAG_MS).unwrap();
        assert!(close(cont["index"]["level"].as_f64().unwrap(), v["index"]["level"].as_f64().unwrap()));
        assert_eq!(cont["index"]["series"], v["index"]["series"]);
        // Running again over the same data changes nothing.
        let st = again.state.clone();
        again.market_json(T0 + 6 * HOUR + 2 * MIN + STEP_LAG_MS).unwrap();
        assert_eq!(again.state, st);
    }

    #[test]
    fn retention_does_not_move_the_base() {
        let (d, out) = (tmp("retain"), tmp("retain-out"));
        let cycle = |at: u64, a: f64| vec![three("x/a", at, 1.0, a), three("x/b", at + 1000, 1.0, 3.0), three("x/c", at + 2000, 1.0, 5.0)];
        put(&d, &cycle(T0, 2.0));
        let mut w = Wide::default();
        w.update(&d, T0 + MIN).unwrap();
        w.market_json(T0 + MIN + STEP_LAG_MS).unwrap();
        let saved = reload(&w, &out);
        // 40 days on: the first day file is past the 30 days held.
        let later = T0 + 40 * DAY_MS;
        put(&d, &cycle(later, 2.4));
        let mut w = Wide::with_state(saved);
        w.update(&d, later + MIN).unwrap();
        assert_eq!(w.models["x/a"].len(), 1);
        let v = w.market_json(later + MIN + STEP_LAG_MS).unwrap();
        assert_eq!(v["index"]["base_ms"], T0 + 2000);
        assert!(close(v["index"]["level"].as_f64().unwrap(), 100.0 * 1.2f64.powf(1.0 / 3.0)));
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
    fn dated_snapshots_are_one_model() {
        // Real hf-map.tsv rows (2026-11-13), lowercased as read, plus the
        // reviewer's negative cases.
        let hf: HashMap<String, String> = [
            ("deepseek/deepseek-v4-flash", "deepseek-ai/deepseek-v4-flash"),
            ("deepseek/deepseek-v4-flash-0731", "deepseek-ai/deepseek-v4-flash-0731"),
            ("deepseek/deepseek-v4-pro", "deepseek-ai/deepseek-v4-pro"),
            ("deepseek/deepseek-v4-pro-0813", "deepseek-ai/deepseek-v4-pro-0813"),
            ("deepseek/deepseek-v4.1-flash", "deepseek-ai/deepseek-v4.1-flash"),
            ("deepseek/deepseek-v4.1-flash-1017", "deepseek-ai/deepseek-v4.1-flash-1017"),
            ("x/same", "x/same-weights"),
            ("x/same-20261017", "x/same-weights"),
            ("mistralai/mistral-small-2501", "mistralai/mistral-small-24b-instruct-2501"),
            ("mistralai/mistral-small-2603", "mistralai/mistral-small-4-2603"),
            ("qwen/qwen3-235b-a22b", "qwen/qwen3-235b-a22b"),
            ("qwen/qwen3-235b-a22b-2507", "qwen/qwen3-235b-a22b-instruct-2507"),
            ("x/nohf-0731", "x/nohf-0731"),
        ]
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
        .into_iter()
        .collect();
        let ids: BTreeSet<&str> = hf.keys().map(String::as_str).chain(["x/nohf"]).collect();
        for (dated, base) in [("deepseek/deepseek-v4-flash-0731", "deepseek/deepseek-v4-flash"), ("deepseek/deepseek-v4-pro-0813", "deepseek/deepseek-v4-pro"), ("deepseek/deepseek-v4.1-flash-1017", "deepseek/deepseek-v4.1-flash"), ("x/same-20261017", "x/same")] {
            assert_eq!(alias_of(dated, &ids, &hf), Some(base), "{dated}");
        }
        // Different releases: no undated id, a different HF repo, or no HF id.
        for id in ["mistralai/mistral-small-2501", "mistralai/mistral-small-2603", "qwen/qwen3-235b-a22b-2507", "x/nohf-0731", "deepseek/deepseek-v4-flash", "qwen/qwen3-235b-a22b"] {
            assert_eq!(alias_of(id, &ids, &hf), None, "{id}");
        }
        let d = tmp("alias");
        std::fs::write(d.join("hf-map.tsv"), "x/flash\tX/Flash\tx/flash-1\nx/flash-0731\tX/Flash-0731\tx/flash-2\nx/other\tX/Other\tx/o\nbad line\n").unwrap();
        let cyc = |at: u64, alias: bool| {
            let mut v = vec![line("x/flash", at, &[("A", 2.0, 0), ("B", 3.0, 0), ("C", 4.0, 0)]), three("x/other", at + 1000, 1.0, 1.0)];
            if alias {
                // B cheaper on the alias, D only there, A down there.
                v.push(line("x/flash-0731", at + 2000, &[("A", 1.0, -2), ("B", 2.5, 0), ("D", 9.0, 0)]));
            }
            v
        };
        put(&d, &[cyc(T0, false), cyc(T0 + 5 * MIN, false), cyc(T0 + 25 * HOUR, true), cyc(T0 + 25 * HOUR + 5 * MIN, true)].concat());
        let v = run(&d, T0 + 25 * HOUR + 6 * MIN);
        let ids: Vec<&str> = v["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["x/flash", "x/other"]);
        let f = model(&v, "x/flash");
        assert_eq!(f["aliases"], json!(["x/flash-0731"]));
        let q: Vec<(String, Option<f64>, bool)> = f["quotes"].as_array().unwrap().iter().map(|q| (q["provider"].as_str().unwrap().to_owned(), q["price"].as_f64(), q["up"].as_bool().unwrap())).collect();
        assert_eq!(q, [("A".into(), Some(2.0), true), ("B".into(), Some(2.5), true), ("C".into(), Some(4.0), true), ("D".into(), Some(9.0), true)]);
        assert_eq!((v["index"]["n_models"].as_u64(), v["index"]["n_quotes"].as_u64(), v["index"]["n_providers"].as_u64()), (Some(2), Some(7), Some(4)));
        // The alias showing up re-links the group (median 3 → 3.25): no jump.
        assert!(levels(&v).iter().all(|l| *l == 100.0), "{:?}", levels(&v));
    }

    #[test]
    fn demand_and_sellers_added() {
        let d = tmp("demand");
        std::fs::create_dir_all(d.join("demand")).unwrap();
        let dl = |at: u64, m: &str, n: u64| json!({ "at_ms": at, "model": m, "hf": "h", "downloads_30d": n, "likes": 7 }).to_string();
        let now = T0 + 8 * DAY_MS;
        std::fs::write(d.join(format!("demand/{}.jsonl", day(T0 + DAY_MS))), format!("{}\n", dl(T0 + DAY_MS, "x/a", 1000))).unwrap();
        std::fs::write(d.join(format!("demand/{}.jsonl", day(T0 + 5 * DAY_MS))), format!("{}\n{}\n", dl(T0 + 5 * DAY_MS, "x/a", 1100), dl(T0 + 5 * DAY_MS, "x/b", 10))).unwrap();
        std::fs::write(d.join(format!("demand/{}.jsonl", day(now))), format!("{}\njunk\n{}\n{{\"at_ms\":1", dl(now - HOUR, "x/a", 1500), dl(now - HOUR, "x/b", 50))).unwrap();
        put(&d, &[
            line("x/a", T0, &[("A", 1.0, 0), ("B", 1.0, 0)]),
            line("x/a", now - MIN, &[("A", 1.0, 0), ("B", 1.0, -2), ("C", 1.0, 0), ("D", 1.0, 0)]),
            three("x/b", now - 2 * DAY_MS, 1.0, 1.0),
            three("x/c", now - MIN, 1.0, 1.0),
        ]);
        let v = run(&d, now);
        let a = model(&v, "x/a");
        // 1500 vs the 1000 from 7 days before (the 1100 is under 6 days older).
        assert_eq!(a["demand"], json!({ "hf_downloads_30d": 1500, "hf_likes": 7, "hf_downloads_change_7d_pct": 50.0, "as_of_ms": now - HOUR }));
        assert_eq!(model(&v, "x/b")["demand"]["hf_downloads_change_7d_pct"], Value::Null);
        assert_eq!(model(&v, "x/c")["demand"]["hf_downloads_30d"], Value::Null);
        assert_eq!(v["demand_rank"], json!(["x/a", "x/b"]));
        // C and D are new since 7 days ago; x/b has no data that old.
        assert_eq!(a["sellers_added_7d"], 2);
        assert_eq!(model(&v, "x/b")["sellers_added_7d"], Value::Null);
    }

    #[test]
    fn a_method_change_relinks_instead_of_jumping() {
        let d = tmp("relink");
        let cycle = |at: u64, a: f64| vec![three("x/a", at, 1.0, a), three("x/b", at + 1000, 1.0, 3.0), three("x/c", at + 2000, 1.0, 5.0)];
        put(&d, &[cycle(T0, 2.0), cycle(T0 + 5 * MIN, 4.0)].concat());
        let mut w = Wide::default();
        w.update(&d, T0 + 6 * MIN).unwrap();
        w.market_json(T0 + 6 * MIN + STEP_LAG_MS).unwrap();
        // A state saved by an older method: x/a's last median was 3, not 4.
        let mut st = w.state.clone().unwrap();
        st.method = 0;
        let b = st.models.get_mut("x/a").unwrap();
        (b.base, b.last) = (b.base * 0.75, 3.0);
        let level = st.level;
        put(&d, &cycle(T0 + 10 * MIN, 4.0));
        let mut w = Wide::with_state(Some(st));
        w.update(&d, T0 + 11 * MIN).unwrap();
        let v = w.market_json(T0 + 11 * MIN + STEP_LAG_MS).unwrap();
        assert!(close(v["index"]["level"].as_f64().unwrap(), level));
        assert!(close(w.state.unwrap().models["x/a"].base, 1.0 * 4.0 / 3.0 * 0.75 * 2.0));
    }

    #[test]
    fn a_seller_going_down_is_held_for_six_hours() {
        let d = tmp("hold");
        // Four sellers: 1, 2, 3, 10. D (10) flaps down and back, then leaves.
        let snap = |at: u64, d_status: Option<i64>| {
            let mut rows = vec![("A", 1.0, 0), ("B", 2.0, 0), ("C", 3.0, 0)];
            if let Some(st) = d_status {
                rows.push(("D", 10.0, st));
            }
            line("x/a", at, &rows)
        };
        put(&d, &[
            snap(T0, Some(0)),
            snap(T0 + 5 * MIN, Some(-2)), // down
            snap(T0 + 10 * MIN, None),    // unlisted
            snap(T0 + 15 * MIN, Some(0)), // back
            snap(T0 + 20 * MIN, None),    // gone for good
            snap(T0 + 6 * HOUR + 10 * MIN, None), // last up at +15 min: still held
            snap(T0 + 6 * HOUR + 20 * MIN, None),
            three("x/b", T0 + 1000, 1.0, 1.0),
        ]);
        let v = run(&d, T0 + 6 * HOUR + 26 * MIN);
        let a = model(&v, "x/a");
        // Median of 1, 2, 3, 10 is 2.5 while D counts; 2 once it has been gone over 6 h.
        assert_eq!(a["series"], json!([[T0, 2.5], [T0 + 6 * HOUR + 10 * MIN, 2.5], [T0 + 6 * HOUR + 20 * MIN, 2.0]]));
        // The card's spread is what is listed up now.
        assert_eq!((a["sellers_up"].as_u64(), a["max"].as_f64()), (Some(3), Some(3.0)));
        // The index moves only when D has really left.
        let l = levels(&v);
        assert_eq!(l.first(), Some(&100.0));
        assert!(close(*l.last().unwrap(), 100.0 * (2.0f64 / 2.5).sqrt()), "{l:?}");
        assert!(l[..l.len() - 1].iter().all(|x| *x == 100.0), "{l:?}");
    }

    /// 2026-11-13 09:40-09:50: AtlasCloud lists the plain id at 0.16 and the
    /// dated alias at 0.232; the plain listing goes down, then both. Merging
    /// used to show 0.16 → 0.232 → 0.16 as two changes.
    #[test]
    fn alias_listings_never_make_changes() {
        let d = tmp("alias-changes");
        std::fs::write(d.join("hf-map.tsv"), "x/flash\tX/Flash\nx/flash-1017\tX/Flash-1017\n").unwrap();
        let t = [T0, T0 + 5 * MIN, T0 + 10 * MIN, T0 + 15 * MIN];
        let atlas_plain = [0, -2, -2, 0];
        let atlas_dated = [0, 0, -2, 0];
        let mut lines = Vec::new();
        for k in 0..4 {
            lines.push(line("x/flash", t[k], &[("AtlasCloud", 0.16, atlas_plain[k]), ("B", 0.1, 0), ("C", 1.0, 0)]));
            // D first appears on the alias at the second fetch, and reprices once for real.
            let mut dated = vec![("AtlasCloud", 0.232, atlas_dated[k]), ("B", 0.1, 0)];
            if k >= 1 {
                dated.push(("D", if k < 3 { 3.0 } else { 2.7 }, 0));
            }
            lines.push(line("x/flash-1017", t[k] + 1000, &dated));
        }
        lines.push(three("x/other", T0 + 2000, 1.0, 1.0));
        put(&d, &lines);
        let v = run(&d, T0 + 16 * MIN);
        assert_eq!(v["changes"], json!([{ "at_ms": t[3] + 1000, "model": "x/flash", "provider": "D", "quantization": "bf16", "from": 3.0, "to": 2.7 }]));
        // The model shows the lower of the two AtlasCloud listings.
        let atlas = model(&v, "x/flash")["quotes"].as_array().unwrap().iter().find(|q| q["provider"] == "AtlasCloud").unwrap().clone();
        assert_eq!(atlas["price"], 0.16);
        // The index input does not flip either: AtlasCloud sits at the median
        // and stays at 0.16 (held while down); only D joining moves it.
        let series: Vec<f64> = model(&v, "x/flash")["series"].as_array().unwrap().iter().map(|p| p[1].as_f64().unwrap()).collect();
        assert_eq!(series.len(), 4, "{series:?}");
        assert!(close(series[0], 0.16) && close(series[1], 0.16) && close(series[2], 0.58) && close(series[3], 0.58), "{series:?}");
    }

    #[test]
    fn a_quiet_collector_shows_as_stale() {
        let d = tmp("stale");
        put(&d, &[three("x/a", T0, 1.0, 2.0), three("x/b", T0 + 40 * MIN, 1.0, 2.0)]);
        let v = run(&d, T0 + 40 * MIN + MIN);
        assert_eq!((v["data_ms"].as_u64(), v["window"]["to_ms"].as_u64()), (Some(T0 + 40 * MIN), Some(T0 + 40 * MIN)));
        let a = model(&v, "x/a");
        assert_eq!((a["data_ms"].as_u64(), a["stale"].as_bool(), a["sellers_up"].as_u64(), a["cheapest"].clone()), (Some(T0), Some(true), Some(0), Value::Null));
        assert!(a["quotes"].as_array().unwrap().iter().all(|q| q["up"] == false));
        assert_eq!(model(&v, "x/b")["stale"], false);
        assert_eq!(v["index"]["n_quotes_up"], 3);
    }

    #[test]
    fn a_provider_counts_once() {
        let d = tmp("provider");
        // One provider in three regions and two precisions: one seller, thin.
        let rows: Vec<Value> = [("p/us", "fp8", 1.0), ("p/eu", "fp8", 1.1), ("p/x", "bf16", 3.0)]
            .iter()
            .map(|(tag, q, x)| json!({ "provider": "P", "tag": tag, "quantization": q, "prompt": x / 1e6, "completion": x / 1e6, "status": 0 }))
            .collect();
        let l = json!({ "at_ms": T0, "model": "x/a", "rows": rows }).to_string();
        put(&d, &[l, line("x/a", T0 + 5 * MIN, &[("P", 1.0, 0), ("Q", 2.0, 0)]), three("x/b", T0 + 1000, 1.0, 1.0)]);
        let v = run(&d, T0 + 6 * MIN);
        let a = model(&v, "x/a");
        // The listing keeps every quote; the median set is one price per provider.
        assert_eq!((a["thin"].as_bool(), a["sellers_total"].as_u64()), (Some(true), Some(2)));
        assert_eq!(v["index"]["n_models"], 1);
        assert_eq!(a["median"], 1.5);
    }

    #[test]
    fn one_model_moves_the_index_by_at_most_a_quarter_per_step() {
        let d = tmp("cap");
        // A's median goes 2 → 10 (one absurd jump) and stays.
        let lines: Vec<String> = (0..10u64).flat_map(|k| [three("x/a", T0 + k * 5 * MIN, 1.0, if k == 0 { 2.0 } else { 10.0 }), three("x/b", T0 + k * 5 * MIN + 1000, 1.0, 3.0)]).collect();
        put(&d, &lines);
        let mut w = Wide::default();
        w.update(&d, T0 + HOUR).unwrap();
        w.market_json(T0 + HOUR).unwrap();
        let st = w.state.unwrap();
        let got: Vec<f64> = st.series.iter().map(|p| p.1).collect();
        // Input 2 → 2.5 → 3.125 …, capped at 1.25× per step; the level moves by √1.25 a step.
        for w in got.windows(2) {
            assert!(w[1] / w[0] <= MAX_STEP_RATIO.sqrt() + 1e-12, "{got:?}");
        }
        assert!(close(got[1], 100.0 * 1.25f64.sqrt()));
        // It catches up: 2 × 1.25^8 > 10.
        assert_eq!(st.models["x/a"].last, 10.0);
        assert!(close(*got.last().unwrap(), 100.0 * 5f64.sqrt()));
    }

    #[test]
    fn late_snapshots_never_restep_the_index() {
        let (d, out) = (tmp("late"), tmp("late-out"));
        let cycle = |at: u64, a: f64| vec![three("x/a", at, 1.0, a), three("x/b", at + 1000, 1.0, 3.0)];
        put(&d, &[cycle(T0, 2.0), cycle(T0 + 5 * MIN, 2.2)].concat());
        // At +6 min the second cycle is under 2 minutes old: not stepped yet.
        let mut w = Wide::default();
        w.update(&d, T0 + 6 * MIN).unwrap();
        w.market_json(T0 + 6 * MIN).unwrap();
        assert_eq!(w.state.as_ref().unwrap().at_ms, T0 + 1000);
        w.market_json(T0 + 8 * MIN).unwrap();
        let st = w.state.clone().unwrap();
        assert_eq!(st.at_ms, T0 + 5 * MIN + 1000);
        // A snapshot for an already-stepped time arrives late (x/a at +3 min, at 2.6).
        put(&d, &[three("x/a", T0 + 3 * MIN, 1.0, 2.6)]);
        let mut again = Wide::with_state(reload(&w, &out));
        again.update(&d, T0 + 9 * MIN).unwrap();
        again.market_json(T0 + 9 * MIN).unwrap();
        assert_eq!(again.state.as_ref().unwrap(), &st);
        // The next real cycle steps from the saved inputs, not from the late one.
        put(&d, &cycle(T0 + 10 * MIN, 2.2));
        again.update(&d, T0 + 13 * MIN).unwrap();
        again.market_json(T0 + 13 * MIN).unwrap();
        assert_eq!(again.state.as_ref().unwrap().level, st.level);
    }

    #[test]
    fn a_bad_state_stops_only_the_market_file() {
        let (d, out) = (tmp("bad-state"), tmp("bad-state-out"));
        put(&d, &[three("x/a", T0, 1.0, 2.0)]);
        std::fs::write(out.join("index-state.json"), "{").unwrap();
        let mut f = MarketFeed::new(d.clone(), false);
        for _ in 0..2 {
            let e = format!("{:#}", f.run(&out, T0 + 5 * MIN).unwrap_err());
            assert!(e.contains("--rebase"), "{e}");
            assert!(!out.join("market.json").exists());
        }
        // Fixed (removed): the next run starts a base.
        std::fs::remove_file(out.join("index-state.json")).unwrap();
        f.run(&out, T0 + 5 * MIN).unwrap();
        assert!(out.join("market.json").exists() && out.join("index-state.json").exists());
        // --rebase over a bad file starts a fresh base at once.
        std::fs::write(out.join("index-state.json"), "{").unwrap();
        MarketFeed::new(d, true).run(&out, T0 + 5 * MIN).unwrap();
        assert!(IndexState::load(&out.join("index-state.json")).unwrap().is_some());
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

    #[test]
    fn compute_units_mixes_and_the_flop_price() {
        // (15 × 1e-7 + 1.6e-6) / 16 × 1e6 = 0.19375; 3:1 and 150:1 likewise.
        assert!(close(mix([1e-7, 1.6e-6], 15.0), 0.19375));
        assert!(close(mix([1e-7, 1.6e-6], 3.0), 0.475));
        assert!(close(mix([1e-7, 1.6e-6], 150.0), 16.6 / 151.0));
        // $0.2969 per 1M tokens at 52 G_n per token: $5.71 per billion G_n.
        assert!(close(usd_per_bn_gn(0.2969, 52.0), 5.709615384615385));

        let d = tmp("compute");
        std::fs::create_dir_all(d.join("demand")).unwrap();
        let now = T0 + HOUR;
        let row = |p: &str, i: f64, o: f64| json!({ "provider": p, "tag": p, "quantization": "bf16", "prompt": i, "completion": o, "status": 0 });
        let lines = [
            // 15:1: A 0.15625, B 0.25, C 0.28125. 3:1: 0.325, 0.4, 0.825.
            // 150:1: 16/151, 31/151, 18/151. completion ÷ prompt: 10, 5, 30.
            json!({ "at_ms": now - MIN, "model": "qwen/qwen3.8-27b", "rows": [row("A", 1e-7, 1e-6), row("B", 2e-7, 1e-6), row("C", 1e-7, 3e-6)] }).to_string(),
            three("x/b", now - MIN, 1.0, 1.0),
            three("x/c", now - MIN, 1.0, 1.0),
            line("x/d", now - MIN, &[("A", 2.0, 0), ("B", 2.0, 0)]),
        ];
        put(&d, &lines);
        let dl = |at: u64, m: &str, n: Value| json!({ "at_ms": at, "model": m, "params": n }).to_string();
        // x/b: the newest known count (7e9), not the later null.
        let demand = [dl(now - 3 * MIN, "x/b", json!(7_000_000_000u64)), dl(now - 2 * MIN, "x/b", Value::Null), dl(now - 2 * MIN, "x/d", json!(1_000_000_000u64))];
        std::fs::write(d.join(format!("demand/{}.jsonl", day(now))), demand.join("\n")).unwrap();
        let mut w = Wide::default();
        w.compute.gn_price_flop = Some(0.002);
        w.update(&d, now).unwrap();
        let v = w.market_json(now + STEP_LAG_MS).unwrap();

        let q = model(&v, "qwen/qwen3.8-27b");
        assert!(close(q["median"].as_f64().unwrap(), 0.25));
        assert_eq!(q["compute"], json!({ "params": null, "active_params": null, "active_params_source": null, "gn_per_token": 52.0, "basis": "flop_registry", "source": "flop_registry" }));
        assert!(close(q["usd_per_bn_gn"].as_f64().unwrap(), 0.25 / 52.0 * 1e3));
        assert!(close(q["usd_per_bn_gn_min"].as_f64().unwrap(), 0.15625 / 52.0 * 1e3));
        assert!(close(q["usd_per_bn_gn_max"].as_f64().unwrap(), 0.28125 / 52.0 * 1e3));
        assert!(close(q["mix"]["chat_3_1"].as_f64().unwrap(), 0.4));
        assert!(close(q["mix"]["avg_15_1"].as_f64().unwrap(), 0.25));
        assert!(close(q["mix"]["agent_150_1"].as_f64().unwrap(), 18.0 / 151.0));
        assert!(close(q["out_in_ratio"].as_f64().unwrap(), 10.0));

        let b = model(&v, "x/b");
        assert_eq!(b["compute"], json!({ "params": 7_000_000_000u64, "active_params": null, "active_params_source": null, "gn_per_token": 14.0, "basis": "2x_params_dense", "source": "2x_params_dense" }));
        assert!(close(b["usd_per_bn_gn"].as_f64().unwrap(), 1.0 / 14.0 * 1e3));
        assert_eq!((b["mix"]["chat_3_1"].as_f64(), b["out_in_ratio"].as_f64()), (Some(1.0), Some(1.0)));
        let c = model(&v, "x/c");
        assert_eq!((c["compute"].clone(), c["usd_per_bn_gn"].clone()), (json!({ "params": null, "active_params": null, "active_params_source": null, "gn_per_token": null, "basis": null, "source": null }), Value::Null));
        // x/d is thin: it has a value but is left out of the totals.
        let x = model(&v, "x/d");
        assert_eq!((x["thin"].as_bool(), x["usd_per_bn_gn"].as_f64().map(|u| close(u, 1e3))), (Some(true), Some(true)));

        // Over the non-thin models with a value: 4.8077 and 71.43.
        let cp = &v["compute_price"];
        // x/c has no G_n: left out and counted.
        assert_eq!((cp["n_models"].as_u64(), cp["n_estimated"].as_u64(), cp["n_unknown"].as_u64()), (Some(2), Some(0), Some(1)));
        assert!(close(cp["median_usd_per_bn_gn"].as_f64().unwrap(), (0.25 / 52.0 * 1e3 + 1e3 / 14.0) / 2.0));
        assert!(close(cp["p25"].as_f64().unwrap(), 0.25 / 52.0 * 1e3 + 0.25 * (1e3 / 14.0 - 0.25 / 52.0 * 1e3)));
        // Medians of the model medians (qwen, x/b, x/c): x/b and x/c are 1.
        assert_eq!(v["mixes"], json!({ "chat_3_1": 1.0, "avg_15_1": 1.0, "agent_150_1": 1.0 }));
        assert_eq!(v["out_in_ratio_median"], 1.0);
        // $4.8077 per billion G_n / 1e9 / 0.002 FLOP per G_n.
        assert_eq!((v["flop"]["gn_price_flop"].as_f64(), v["flop"]["basis_model"].as_str()), (Some(0.002), Some(BASIS_MODEL)));
        assert!(close(v["flop"]["implied_flop_usd"].as_f64().unwrap(), 0.25 / 52.0 * 1e3 / 1e9 / 0.002));

        // A registry file adds or overrides entries; a bad line is an error.
        let f = d.join("compute.tsv");
        std::fs::write(&f, "# id  G_n per token\nx/c\t4 # set by hand\n\nqwen/qwen3.8-27b 50\n").unwrap();
        let mut cfg = Compute::default();
        cfg.read_registry(&f).unwrap();
        assert_eq!((cfg.registry["x/c"], cfg.registry["qwen/qwen3.8-27b"], cfg.registry["qwen/qwen3-8b"]), (4.0, 50.0, 17.0));
        for bad in ["x/c", "x/c four", "x/c 0", "x/c 4 5", "x/c active_params 1.5", "x/c active_params 0"] {
            std::fs::write(&f, bad).unwrap();
            assert!(Compute::default().read_registry(&f).is_err(), "{bad}");
        }
    }

    #[test]
    fn active_params_from_ids() {
        for (id, n) in [
            ("qwen/qwen3.5-397b-a17b", Some(17e9)), ("qwen/qwen3.6-35b-a3b", Some(3e9)), ("google/gemma-4-26b-a4b-it", Some(4e9)),
            ("nvidia/nemotron-3-ultra-550b-a55b", Some(55e9)), ("qwen/qwen3.8-2.4t-a95b", Some(95e9)), ("reflection/beam-1.0-501b-a23b", Some(23e9)),
            ("qwen/qwen3-30b-a3b-instruct-2507", Some(3e9)), ("x/m-30b-a2.5b", Some(2.5e9)),
            ("qwen/qwen3.8-27b", None), ("a/ab-8b", None), ("x/a3b", None), ("x/m-ab", None), ("x/m-a0b", None), ("x/m-a-3b", None),
        ] {
            assert_eq!(active_from_id(id), n.map(|n: f64| n as u64), "{id}");
        }
    }

    #[test]
    fn the_compute_basis_in_order() {
        let mut c = Compute::default();
        let g = |c: &Compute, ids: &[&str], p: Option<u64>, cfg: Option<&Config>| {
            let r = c.gn(ids, p, cfg);
            (r.gn_per_token, r.active_params, r.basis, r.active_source)
        };
        const NONE: (Option<f64>, Option<u64>, Option<&str>, Option<&str>) = (None, None, None, None);
        // Dense fallback only up to 100B weights; over it, unknown.
        assert_eq!(g(&c, &["x/dense"], Some(DENSE_MAX_PARAMS), None), (Some(200.0), None, Some("2x_params_dense"), None));
        assert_eq!(g(&c, &["x/big"], Some(DENSE_MAX_PARAMS + 1), None), NONE);
        assert_eq!(g(&c, &["x/none"], None, None), NONE);
        // A zero count is unknown, and a G_n of 0 is none.
        assert_eq!(g(&c, &["x/zero"], Some(0), None), NONE);
        assert_eq!(g(&c, &["x/zero"], Some(0), Some(&Config { experts: Some(8), experts_per_tok: Some(2), ..Config::default() })), NONE);
        assert_eq!(active_from_id("x/m-1b-a0.0000000001b"), None);
        assert_eq!(g(&c, &["x/m-1b-a0.0000000001b"], Some(0), None), NONE);
        let mut z = Compute::default();
        z.registry.insert("x/z".into(), 0.0);
        z.active_params.insert("x/y".into(), 0);
        assert_eq!((g(&z, &["x/z"], None, None), g(&z, &["x/y"], None, None)), (NONE, NONE));
        // Experts in the config: never dense, even if they give no estimate.
        let moe = Config { experts: Some(32), ..Config::default() };
        assert_eq!(g(&c, &["x/small-moe"], Some(20_000_000_000), Some(&moe)), NONE);
        let one = Config { experts: Some(1), experts_per_tok: Some(1), ..Config::default() };
        assert_eq!(g(&c, &["x/dense"], Some(1_000_000_000), Some(&one)).2, Some("2x_params_dense"));
        // Config estimate: below the id and the table, above dense.
        let est = Config { experts: Some(10), experts_per_tok: Some(2), ..Config::default() };
        assert_eq!(g(&c, &["x/m"], Some(50_000_000_000), Some(&est)), (Some(20.0), Some(10_000_000_000), Some("2x_active_params_est"), Some("config_estimate")));
        // An `-a<N>b` id: 2 × active, whatever the total, ahead of the config.
        assert_eq!(g(&c, &["x/m-397b-a17b"], Some(397_000_000_000), Some(&est)), (Some(34.0), Some(17_000_000_000), Some("2x_active_params"), Some("id")));
        // FLOP's DeepSeek-V3 anchor, only at its parameter count.
        assert_eq!(g(&c, &["deepseek/deepseek-v3.2"], Some(685_355_329_792), None), (Some(74.0), None, Some("flop_registry"), None));
        assert_eq!(g(&c, &["deepseek/deepseek-r1-0528"], Some(684_531_386_000), None).0, Some(74.0));
        assert_eq!(g(&c, &["deepseek/deepseek-v3.2"], Some(763_205_315_794), None), NONE);
        assert_eq!(g(&c, &["deepseek/deepseek-v3.2"], None, None), NONE);
        assert_eq!(g(&c, &["deepseek/deepseek-v4.1-flash"], Some(685_000_000_000), None), NONE);
        // The anchor beats the config estimate (which it then reports).
        assert_eq!(g(&c, &["deepseek/deepseek-v3.2"], Some(685_355_329_792), Some(&V3)).2, Some("flop_registry"));
        // The table's active count beats the id's; the registry beats both.
        c.active_params.insert("x/m-397b-a17b".into(), 20_000_000_000);
        assert_eq!(g(&c, &["x/m-397b-a17b"], None, None), (Some(40.0), Some(20_000_000_000), Some("2x_active_params"), Some("table")));
        c.registry.insert("x/m-397b-a17b".into(), 50.0);
        assert_eq!(g(&c, &["x/m-397b-a17b"], None, None), (Some(50.0), Some(20_000_000_000), Some("flop_registry"), Some("table")));
        // A registry file entry overrides the anchor.
        c.registry.insert("deepseek/deepseek-v3.2".into(), 80.0);
        assert_eq!(g(&c, &["deepseek/deepseek-v3.2"], Some(685_355_329_792), None).0, Some(80.0));
        // A file sets both kinds.
        let f = tmp("compute-file").join("c.tsv");
        std::fs::write(&f, "z/k active_params 32000000000 # from the model card\nz/k2 9\n").unwrap();
        let mut c = Compute::default();
        c.read_registry(&f).unwrap();
        assert_eq!((g(&c, &["z/k"], Some(1_000_000_000_000), None).0, c.registry["z/k2"]), (Some(64.0), 9.0));
    }

    #[test]
    fn totals_skip_thin_stale_and_infinite_values() {
        let d = tmp("compute-totals");
        let now = T0 + HOUR;
        put(&d, &[
            // The basis model is thin: no implied FLOP price.
            line("qwen/qwen3.8-27b", now - MIN, &[("A", 1.0, 0), ("B", 1.0, 0)]),
            three("x/ok", now - MIN, 1.0, 1.0),
            // Stale (over 30 minutes old): left out of the totals.
            three("x/stale", now - HOUR, 9.0, 9.0),
            // A G_n so small that USD per G_n overflows to infinity.
            three("x/inf", now - MIN, 1.0, 1.0),
        ]);
        let mut w = Wide::default();
        w.compute.gn_price_flop = Some(0.002);
        w.compute.registry.extend([("x/ok".to_owned(), 10.0), ("x/stale".to_owned(), 10.0), ("x/inf".to_owned(), 1e-320)]);
        w.update(&d, now).unwrap();
        let v = w.market_json(now + STEP_LAG_MS).unwrap();
        assert_eq!(model(&v, "qwen/qwen3.8-27b")["thin"], true);
        assert!(model(&v, "qwen/qwen3.8-27b")["usd_per_bn_gn"].is_number());
        assert_eq!(model(&v, "x/stale")["stale"], true);
        assert_eq!(model(&v, "x/inf")["usd_per_bn_gn"], Value::Null);
        assert_eq!(v["flop"]["implied_flop_usd"], Value::Null);
        // Only x/ok counts: $1 per 1M at 10 G_n per token = $100 per bn G_n.
        assert_eq!(v["compute_price"], json!({ "median_usd_per_bn_gn": 100.0, "p25": 100.0, "p75": 100.0, "n_models": 1, "n_estimated": 0, "n_unknown": 1 }));
        assert_eq!(v["mixes"]["avg_15_1"], 1.0);
    }

    #[test]
    fn route_shares_cap_one_provider_at_half() {
        let r = |w: &[f64]| route_shares(w, MAX_SHARE);
        assert_eq!(r(&[1.0, 1.0]), (vec![0.5, 0.5], false));
        assert_eq!(r(&[3.0, 1.0]), (vec![0.5, 0.5], true));
        assert_eq!(r(&[8.0, 1.0, 1.0]), (vec![0.5, 0.25, 0.25], true));
        assert_eq!(r(&[1.0, 2.0, 1.0]), (vec![0.25, 0.5, 0.25], false));
        // One seller with weight carries it all: the cap cannot apply.
        assert_eq!(r(&[2.0, 0.0]), (vec![1.0, 0.0], false));
    }

    #[test]
    fn routed_price_weights_cap_uptime_and_parity() {
        let d = tmp("routed");
        let now = T0 + 3 * HOUR;
        // (provider, USD per 1M at 15:1, status, uptime_30m)
        let snap = |m: &str, at: u64, rows: &[(&str, f64, i64, Option<f64>)]| {
            let rows: Vec<Value> = rows.iter().map(|(p, x, st, u)| json!({ "provider": p, "tag": p, "quantization": "bf16", "prompt": x / 1e6, "completion": x / 1e6, "status": st, "uptime_30m": u })).collect();
            json!({ "at_ms": at, "model": m, "rows": rows }).to_string()
        };
        put(&d, &[
            // Weights 100/2², 100/2², 100/4² → shares 4/9, 4/9, 1/9, none
            // over half: Σ(u/p) / Σ(u/p²) = 1.25 / 0.5625 = 2.2222.
            snap("qwen/qwen3.8-27b", now - 2 * HOUR, &[("A", 2.0, 0, Some(100.0)), ("B", 2.0, 0, Some(100.0)), ("C", 4.0, 0, Some(100.0))]),
            // A 1/1², B 1/2², C 0.5/4²: A would take 78% (price 1.2683); capped
            // at 50%, B and C share the rest 8:1: 0.5 × 1 + 4/9 × 2 + 1/18 × 4.
            // D has no recent uptime (weight 0); E is down (not counted).
            snap("qwen/qwen3.8-27b", now - MIN, &[("A", 1.0, 0, Some(100.0)), ("B", 2.0, 0, Some(100.0)), ("C", 4.0, 0, Some(50.0)), ("D", 0.5, 0, None), ("E", 0.1, -2, Some(100.0))]),
            // Every seller without uptime: 1 / price² each, then the cap.
            snap("x/null", now - MIN, &[("A", 1.0, 0, None), ("B", 2.0, 0, None), ("C", 4.0, 0, None)]),
        ]);
        let mut w = Wide::default();
        w.compute.gn_price_flop = Some(0.002);
        w.update(&d, now).unwrap();
        let v = w.market_json(now + STEP_LAG_MS).unwrap();
        let q = &model(&v, "qwen/qwen3.8-27b")["routed"];
        let want = 0.5 + 8.0 / 9.0 + 4.0 / 18.0;
        assert!(close(q["price"].as_f64().unwrap(), want), "{q}");
        assert_eq!((q["sellers"].as_u64(), q["capped"].as_bool(), q["uptime_fallback"].as_bool()), (Some(3), Some(true), Some(false)));
        let top: Vec<(&str, f64)> = q["top"].as_array().unwrap().iter().map(|t| (t["provider"].as_str().unwrap(), t["share"].as_f64().unwrap())).collect();
        assert_eq!(top.iter().map(|t| t.0).collect::<Vec<_>>(), ["A", "B", "C"]);
        assert!(close(top[0].1, 0.5) && close(top[1].1, 4.0 / 9.0) && close(top[2].1, 1.0 / 18.0));
        assert!(close(q["change_1h_pct"].as_f64().unwrap(), (want / (1.25 / 0.5625) - 1.0) * 100.0));
        assert_eq!(q["change_24h_pct"], Value::Null);
        let series: Vec<f64> = q["series"].as_array().unwrap().iter().map(|p| p[1].as_f64().unwrap()).collect();
        assert!(series.len() == 2 && close(series[0], 1.25 / 0.5625) && close(series[1], want));
        let x = &model(&v, "x/null")["routed"];
        assert_eq!((x["uptime_fallback"].as_bool(), x["capped"].as_bool()), (Some(true), Some(true)));
        // A 0.5; B, C 1/4 : 1/16 of the rest → 0.4, 0.1.
        assert!(close(x["price"].as_f64().unwrap(), 0.5 + 0.8 + 0.4));
        // Parity at 52 G_n per token: USD per bn G_n, then ÷ 1e9 ÷ FLOP per G_n.
        let gn = want / 52.0 * 1e3;
        assert!(close(model(&v, "qwen/qwen3.8-27b")["routed_usd_per_bn_gn"].as_f64().unwrap(), gn));
        assert_eq!(model(&v, "x/null")["routed_usd_per_bn_gn"], Value::Null);
        assert_eq!((v["routed"]["benchmark"].as_str(), v["routed"]["series"].as_array().map(Vec::len)), (Some(BASIS_MODEL), Some(2)));
        assert!(close(v["routed"]["price"].as_f64().unwrap(), want) && close(v["routed"]["usd_per_bn_gn"].as_f64().unwrap(), gn));
        let par = &v["flop"]["parity"];
        assert!(close(v["flop"]["implied_flop_usd_routed"].as_f64().unwrap(), gn / 1e9 / 0.002));
        assert_eq!((par["flop_rate_flop_per_gn"].as_f64(), par["as_of_ms"].as_u64(), par["implied_flop_usd_routed"].clone()), (Some(0.002), Some(now - MIN), v["flop"]["implied_flop_usd_routed"].clone()));
        assert!(close(par["market_usd_per_bn_gn"].as_f64().unwrap(), gn));
        // The median (list prices, held) is a different number: D at 0.5 counts there.
        assert_eq!(par["implied_flop_usd"], v["flop"]["implied_flop_usd"]);
    }

    /// DeepSeek-V3's `config.json` (685B weights).
    const V3: Config = Config { hidden_size: Some(7168), num_hidden_layers: Some(61), vocab_size: Some(129_280), experts_per_tok: Some(8), experts: Some(256), shared_experts: Some(1) };

    #[test]
    fn active_params_estimated_from_the_config() {
        let rel = |a: u64, want: f64| a as f64 / want;
        // DeepSeek-V3: FLOP's anchor 74 G_n per token = 37B active.
        let a = V3.estimate_active(685_355_329_792).unwrap();
        assert!((rel(a, 37e9) - 1.0).abs() < 0.01, "{a}");
        // gpt-oss-120b: OpenAI states 5.1B active of 116.8B.
        let oss = Config { hidden_size: Some(2880), num_hidden_layers: Some(36), vocab_size: Some(201_088), experts_per_tok: Some(4), experts: Some(128), shared_experts: None };
        let a = oss.estimate_active(116_829_156_672).unwrap();
        assert!((rel(a, 5.1e9) - 1.0).abs() < 0.06, "{a}");
        // By hand: 2 layers × 4 × 10² + 1000 × 10 = 10,800 fixed; of the
        // other 89,200, (2 + 1) / (8 + 1).
        let c = Config { hidden_size: Some(10), num_hidden_layers: Some(2), vocab_size: Some(1000), experts_per_tok: Some(2), experts: Some(8), shared_experts: Some(1) };
        assert_eq!(c.estimate_active(100_000), Some(10_800 + 89_200 / 3));
        // Unknown vocab: 150,000.
        assert_eq!(Config { vocab_size: None, ..c }.estimate_active(10_000_000), Some((800.0 + 1_500_000.0 + (10_000_000.0 - 1_500_800.0) / 3.0_f64).round() as u64));
        // No layers or hidden size: total × share.
        assert_eq!(Config { num_hidden_layers: None, ..c }.estimate_active(900), Some(300));
        // Configs that cannot be right give no estimate: fixed weights at or
        // over the total, more per token than experts, no experts, total 0.
        assert_eq!((c.estimate_active(10_800), c.estimate_active(5_000), c.estimate_active(10_801).is_some()), (None, None, true));
        assert_eq!(Config { experts_per_tok: Some(9), ..c }.estimate_active(100_000), None);
        assert_eq!(Config { experts_per_tok: Some(8), ..c }.estimate_active(100_000), Some(100_000));
        assert_eq!(Config { experts: Some(0), ..c }.estimate_active(100_000), None);
        assert_eq!(c.estimate_active(0), None);
        // No experts, or none per token: no estimate.
        assert_eq!(Config { experts: None, ..c }.estimate_active(100_000), None);
        assert_eq!(Config { experts_per_tok: None, ..c }.estimate_active(100_000), None);
        assert_eq!(Config::from_json(&json!({ "experts": 8, "experts_per_tok": "x", "hidden_size": null })), Some(Config { experts: Some(8), ..Config::default() }));
        assert_eq!(Config::from_json(&Value::Null), None);
    }

    /// Collector lines in the real shape (synthetic sellers and prices), three models, plus junk.
    #[test]
    fn end_to_end_on_the_fixture() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wide"));
        let now = 1_794_559_100_000;
        let v = run(dir, now);
        let i = &v["index"];
        assert_eq!((i["level"].as_f64(), i["base_ms"].as_u64(), i["n_models"].as_u64()), (Some(100.0), Some(1_794_559_021_841), Some(1)));
        // The Mistral model lists three regional endpoints of one (provider, quantization): one quote.
        assert_eq!((i["n_quotes"].as_u64(), i["n_quotes_up"].as_u64()), (Some(25), Some(22)));
        assert_eq!((i["change_24h_pct"].clone(), i["change_7d_pct"].clone()), (Value::Null, Value::Null));
        // The window ends at the newest snapshot read, not at write time.
        assert_eq!(v["window"], json!({ "from_ms": 1_794_558_705_679u64, "to_ms": 1_794_559_021_841u64 }));
        assert_eq!((v["data_ms"].as_u64(), v["generated_ms"].as_u64()), (Some(1_794_559_021_841), Some(now + STEP_LAG_MS)));
        let q = model(&v, "qwen/qwen3.8-27b");
        assert_eq!((q["name"].as_str(), q["sellers_total"].as_u64(), q["sellers_up"].as_u64(), q["thin"].as_bool()), (Some("Qwen3.8 27B"), Some(22), Some(19), Some(false)));
        assert!(close(q["median"].as_f64().unwrap(), 0.3109375));
        // 52 G_n per token (registry): 0.3109375 / 52e6 × 1e9 = $5.980 per billion G_n.
        assert_eq!(q["compute"], json!({ "params": null, "active_params": null, "active_params_source": null, "gn_per_token": 52.0, "basis": "flop_registry", "source": "flop_registry" }));
        assert!(close(q["usd_per_bn_gn"].as_f64().unwrap(), 0.3109375 / 52.0 * 1e3));
        assert_eq!(model(&v, "qwen/qwen3-14b")["usd_per_bn_gn"], Value::Null);
        assert_eq!((v["compute_price"]["n_models"].as_u64(), v["compute_price"]["n_unknown"].as_u64(), v["flop"]["implied_flop_usd"].clone()), (Some(1), Some(0), Value::Null));
        assert!(close(q["min"].as_f64().unwrap(), 0.09828125) && close(q["max"].as_f64().unwrap(), 1.2075));
        assert_eq!(q["cheapest"]["provider"], "Seller E");
        let qs = q["quotes"].as_array().unwrap();
        assert!(qs.windows(2).all(|w| (!w[0]["up"].as_bool().unwrap(), w[0]["price"].as_f64()) <= (!w[1]["up"].as_bool().unwrap(), w[1]["price"].as_f64())));
        assert!(!qs.last().unwrap()["up"].as_bool().unwrap());
        let m = model(&v, "mistralai/mistral-small-2603");
        // The cheapest of the three: (15 × 1.32e-7 + 5.28e-7) / 16 × 1e6.
        assert_eq!((m["sellers_total"].as_u64(), m["thin"].as_bool()), (Some(1), Some(true)));
        assert!(close(m["median"].as_f64().unwrap(), 0.15675));
        let t = model(&v, "qwen/qwen3-14b");
        assert_eq!((t["thin"].as_bool(), t["sellers_up"].as_u64()), (Some(true), Some(2)));
        assert!(close(t["median"].as_f64().unwrap(), 0.109828125));
        // Its empty listing five minutes earlier counts as down for both quotes.
        assert!(t["quotes"].as_array().unwrap().iter().all(|x| x["uptime_24h_pct"] == 50.0));
        let ids: Vec<&str> = v["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["qwen/qwen3.8-27b", "qwen/qwen3-14b", "mistralai/mistral-small-2603"]);
        // No demand files or hf map in the fixture: nulls, nothing fails.
        assert_eq!(q["demand"], json!({ "hf_downloads_30d": null, "hf_likes": null, "hf_downloads_change_7d_pct": null, "as_of_ms": null }));
        assert_eq!((q["sellers_added_7d"].clone(), q["aliases"].clone(), v["demand_rank"].clone()), (Value::Null, json!([]), json!([])));
        let p = v["providers"].as_array().unwrap();
        assert!(p.windows(2).all(|w| w[0]["models"].as_u64() >= w[1]["models"].as_u64()));
        assert_eq!(p.iter().map(|x| x["cheapest_in"].as_u64().unwrap()).sum::<u64>(), 3);
        assert_eq!((v["changes"].as_array().map(Vec::len), v["dynamic"].as_array().map(Vec::len)), (Some(0), Some(0)));
    }
}
