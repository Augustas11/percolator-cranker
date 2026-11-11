//! FLOP compute perp price (devnet demo, docs/flop-compute/INDEX.md, SPEC.md).
//!
//! The mark is USD per 1M tokens of the benchmark model (Qwen3.8-27B), e6.
//! `flop_mark_source` picks how it is made:
//!
//! - `openrouter` (devnet default): the live USD leg. OpenRouter's public
//!   endpoints list for the benchmark model, each endpoint's input:output blend
//!   (`flop_blend_in_out`), median across endpoints of the preferred
//!   quantization. No successful fetch in `or_max_age_s` is stale (hold).
//! - `flop_standin`: FLOP side x `flop_gn_per_mtok` x a FLOP/USD stand-in
//!   (`OracleCfg::flop_usd_standin`). Kept for mainnet, where a live FLOP/USD
//!   replaces the stand-in.
//!
//! FLOP side, always computed (as the mark under `flop_standin`, as a check
//! under `openrouter`): G_n-weighted median of the buyer's price,
//! escrow / aggregateGn, over spot, non-self-trade settlements of every model in
//! a rolling window. No single buyer (agent) or miner may carry more than
//! `flop_max_agent_share` / `flop_max_miner_share` of the window's weight: its
//! weight is scaled down to the cap, not dropped. Too few prints, buyers or
//! miners, or no recent print, is stale. There is no fallback to FLOP's nominal
//! gateway prices.

use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// A reader thread refetches every `flop_refresh_s`; `latest()` only reads its
/// cache, so the pusher never waits on FLOP while holding the trade lock.
/// The reader thread's latest result: `None` until its first fetch.
type Cache = Arc<Mutex<Option<Result<Vec<Print>, String>>>>;
/// The last good OpenRouter quote; `None` until the first successful fetch.
type OrCache = Arc<Mutex<Option<OrQuote>>>;

#[derive(Clone)]
pub struct FlopIndex {
    cfg: launch::OracleCfg,
    cached: Cache,
    or: OrCache,
    /// What the last `latest()` mark was made from, for the signed push log.
    last_inputs: Option<serde_json::Value>,
}

/// One settlement as the index uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct Print {
    pub at_ms: u64,
    pub gn: u128,
    /// Buyer's price: escrow / aggregateGn, planck (1e-18 FLOP) per G_n.
    pub planck_per_gn: u128,
    pub miner: String,
    /// The buyer.
    pub agent: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    settled_at: u64,
    aggregate_gn: String,
    escrow: String,
    miner: String,
    agent: String,
    market_kind: String,
    settlement_kind: String,
    self_trade: bool,
    extrinsic_hash: String,
    event_index: u64,
}

#[derive(Deserialize)]
struct Page {
    data: Vec<Row>,
    pagination: Pagination,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    has_more: bool,
}

const PAGE: usize = 100;
/// 2,000 settlements; FLOP devnet ran ~210 in a 6 h window on 2026-11-10.
const MAX_PAGES: usize = 20;

impl Row {
    /// Spot, settled, not a self-trade, with a positive price and size.
    fn print(&self) -> Option<Print> {
        if self.market_kind != "spot" || self.settlement_kind != "settled" || self.self_trade {
            return None;
        }
        // Bounded so the GN sums can't overflow.
        let gn = self.aggregate_gn.parse().ok().filter(|&g: &u128| g > 0 && g <= u64::MAX as u128)?;
        let escrow: u128 = self.escrow.parse().ok()?;
        let price = Some(escrow / gn).filter(|&p| p > 0)?;
        Some(Print { at_ms: self.settled_at, gn, planck_per_gn: price, miner: self.miner.clone(), agent: self.agent.clone() })
    }
}

/// The FLOP side at one instant, with what went into it.
#[derive(Clone, Debug, PartialEq)]
pub struct FlopSide {
    pub planck_per_gn: u128,
    pub newest_ms: u64,
    pub prints: usize,
    pub agents: usize,
    pub miners: usize,
    /// Buyers / miners whose weight was scaled down to the share cap.
    pub capped_agents: Vec<String>,
    pub capped_miners: Vec<String>,
}

impl FlopSide {
    pub fn flop_per_gn(&self) -> f64 {
        self.planck_per_gn as f64 / 1e18
    }
}

/// Scale down any group carrying more than `cap` of the total weight to exactly
/// the cap (given the others), and name it. Returns whether anything moved.
fn cap_groups(w: &mut [f64], key: impl Fn(usize) -> String, cap: f64, capped: &mut BTreeSet<String>) -> bool {
    if !(cap > 0.0 && cap < 1.0) {
        return false;
    }
    let total: f64 = w.iter().sum();
    let mut by: HashMap<String, f64> = HashMap::new();
    for (i, x) in w.iter().enumerate() {
        *by.entry(key(i)).or_default() += x;
    }
    let mut moved = false;
    for (g, wg) in by {
        if wg > cap * total * (1.0 + 1e-9) {
            let factor = cap / (1.0 - cap) * (total - wg) / wg;
            for (i, x) in w.iter_mut().enumerate() {
                if key(i) == g {
                    *x *= factor;
                }
            }
            capped.insert(g);
            moved = true;
        }
    }
    moved
}

/// Who the window's G_n came from. Logged even when the side is stale, so the
/// gateway's share is visible: buyers are counted per on-chain agent, and the
/// gateway agent stands for every off-chain gateway customer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WindowMix {
    pub prints: usize,
    pub buyers: usize,
    pub gateway_agent: String,
    /// Share of the window's G_n settled under the gateway agent (0 when empty).
    pub gateway_gn_share: f64,
}

pub fn window_mix(prints: &[Print], now_ms: u64, o: &launch::OracleCfg) -> WindowMix {
    let from = now_ms.saturating_sub(o.flop_window_s * 1000);
    let window: Vec<&Print> = prints.iter().filter(|p| p.at_ms >= from && p.at_ms <= now_ms).collect();
    let total: f64 = window.iter().map(|p| p.gn as f64).sum();
    let gateway: f64 = window.iter().filter(|p| p.agent == o.flop_gateway_agent).map(|p| p.gn as f64).sum();
    WindowMix {
        prints: window.len(),
        buyers: window.iter().map(|p| p.agent.as_str()).collect::<HashSet<_>>().len(),
        gateway_agent: o.flop_gateway_agent.clone(),
        gateway_gn_share: if total > 0.0 { gateway / total } else { 0.0 },
    }
}

/// The FLOP side at `now_ms`. Errors say why it is stale.
pub fn flop_side(prints: &[Print], now_ms: u64, o: &launch::OracleCfg) -> Result<FlopSide> {
    let from = now_ms.saturating_sub(o.flop_window_s * 1000);
    let mut window: Vec<&Print> = prints.iter().filter(|p| p.at_ms >= from && p.at_ms <= now_ms).collect();
    if window.len() < o.flop_min_prints {
        bail!("flop index stale: {} prints in the last {} s, need {}", window.len(), o.flop_window_s, o.flop_min_prints);
    }
    let miners = window.iter().map(|p| p.miner.as_str()).collect::<HashSet<_>>().len();
    if miners < o.flop_min_miners {
        bail!("flop index stale: {miners} distinct miners in the last {} s, need {}", o.flop_window_s, o.flop_min_miners);
    }
    let agents = window.iter().map(|p| p.agent.as_str()).collect::<HashSet<_>>().len();
    if agents < o.flop_min_agents {
        bail!("flop index stale: {agents} distinct buyers in the last {} s, need {}", o.flop_window_s, o.flop_min_agents);
    }
    let newest = window.iter().map(|p| p.at_ms).max().unwrap_or(0);
    let age_s = now_ms.saturating_sub(newest) / 1000;
    if age_s > o.flop_max_age_s {
        bail!("flop index stale: newest print {age_s} s old, max {}", o.flop_max_age_s);
    }
    // Weights: G_n, then buyer and miner shares capped (alternating until stable).
    window.sort_by_key(|p| p.planck_per_gn);
    let mut w: Vec<f64> = window.iter().map(|p| p.gn as f64).collect();
    let (mut capped_agents, mut capped_miners) = (BTreeSet::new(), BTreeSet::new());
    for _ in 0..64 {
        let a = cap_groups(&mut w, |i| window[i].agent.clone(), o.flop_max_agent_share, &mut capped_agents);
        let m = cap_groups(&mut w, |i| window[i].miner.clone(), o.flop_max_miner_share, &mut capped_miners);
        if !a && !m {
            break;
        }
    }
    // Weighted median: the lowest price at which half the weight is covered.
    let total: f64 = w.iter().sum();
    if !(total > 0.0) {
        bail!("flop index: no weight left after caps");
    }
    let mut covered = 0.0;
    let mut median = window[window.len() - 1].planck_per_gn;
    for (p, x) in window.iter().zip(&w) {
        covered += x;
        if covered * 2.0 >= total * (1.0 - 1e-12) {
            median = p.planck_per_gn;
            break;
        }
    }
    Ok(FlopSide {
        planck_per_gn: median,
        newest_ms: newest,
        prints: window.len(),
        agents,
        miners,
        capped_agents: capped_agents.into_iter().collect(),
        capped_miners: capped_miners.into_iter().collect(),
    })
}

/// The `flop_standin` mark at `now_ms`: (mark_e6, newest print time).
pub fn index(prints: &[Print], now_ms: u64, o: &launch::OracleCfg) -> Result<(u64, u64)> {
    index_from(&flop_side(prints, now_ms, o)?, o)
}

fn index_from(side: &FlopSide, o: &launch::OracleCfg) -> Result<(u64, u64)> {
    // USD e6 per 1M tokens = planck/G_n / 1e18 x G_n per 1M tokens x FLOP/USD x 1e6.
    let mark = (side.planck_per_gn as f64 / 1e12 * o.flop_gn_per_mtok as f64 * o.flop_usd_standin()).round();
    if !(mark >= 1.0 && mark < u64::MAX as f64) {
        bail!("flop index: price {} planck/G_n gives mark {mark}, out of range", side.planck_per_gn);
    }
    Ok((mark as u64, side.newest_ms))
}

pub use flop_index::{combine, or_quote_with, Guard, OrEndpoint, OrQuote, Used};

/// Pick endpoints: the first quantization tier with at least `or_min_endpoints`;
/// else all listed tiers together if that reaches it; else every endpoint.
/// Each endpoint's price is its input:output blend; `or_weighting` combines them.
pub fn or_quote(endpoints: &[OrEndpoint], at_ms: u64, o: &launch::OracleCfg) -> Result<OrQuote> {
    or_quote_with(endpoints, at_ms, o.flop_blend_in_out, &o.or_quantizations, o.or_min_endpoints, &o.or_weighting, o.or_min_frac_of_median, Guard::Clamp)
}

#[derive(Deserialize)]
struct OrReply {
    data: OrData,
}
#[derive(Deserialize)]
struct OrData {
    endpoints: Vec<OrRow>,
}
#[derive(Deserialize)]
struct OrRow {
    provider_name: String,
    #[serde(default)]
    quantization: Option<String>,
    pricing: OrPricing,
    /// 0 is up; missing counts as up.
    #[serde(default)]
    status: Option<i64>,
    /// Recorder only; any JSON type, so an odd value never breaks the pusher.
    #[serde(default)]
    tag: Option<serde_json::Value>,
    #[serde(default)]
    model_id: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct OrPricing {
    prompt: String,
    completion: String,
}

fn parse_or(text: &str) -> Result<Vec<OrEndpoint>> {
    let reply: OrReply = serde_json::from_str(text).context("openrouter: unexpected endpoints reply")?;
    Ok(reply
        .data
        .endpoints
        .into_iter()
        .filter_map(|r| {
            Some(OrEndpoint {
                provider: r.provider_name,
                quantization: r.quantization.unwrap_or_else(|| "unknown".into()),
                prompt: r.pricing.prompt.parse().ok()?,
                completion: r.pricing.completion.parse().ok()?,
                status: r.status.unwrap_or(0),
            })
        })
        .collect())
}

/// The client for FLOP and OpenRouter reads; the price recorder uses it too.
/// A 100-row page took 3-7 s from Pearl on 2026-11-10; 5 s failed every read.
pub(super) fn http_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

/// OpenRouter's endpoints listing, as text.
pub(super) fn fetch_or_text(client: &reqwest::blocking::Client, url: &str) -> Result<String> {
    Ok(client.get(url).send()?.error_for_status()?.text()?)
}

fn fetch_or(client: &reqwest::blocking::Client, o: &launch::OracleCfg) -> Result<OrQuote> {
    or_quote(&parse_or(&fetch_or_text(client, &o.or_url)?)?, launch::now_ms(), o)
}

/// One endpoint as the price recorder saves it: every row, priced or not.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub(super) struct RecRow {
    pub model: Option<String>,
    /// OpenRouter's endpoint tag, e.g. "deepinfra/bf16".
    pub endpoint: Option<String>,
    pub provider: String,
    pub quantization: Option<String>,
    /// USD per token, as listed (strings parsed; None if unparseable).
    pub prompt: Option<f64>,
    pub completion: Option<f64>,
    pub status: Option<i64>,
}

/// The recorder's parsed rows, from the same reply the pusher parses.
pub(super) fn record_rows(text: &str) -> Result<Vec<RecRow>> {
    let reply: OrReply = serde_json::from_str(text).context("openrouter: unexpected endpoints reply")?;
    Ok(reply
        .data
        .endpoints
        .into_iter()
        .map(|r| RecRow {
            model: r.model_id.and_then(|v| v.as_str().map(String::from)),
            endpoint: r.tag.and_then(|v| v.as_str().map(String::from)),
            provider: r.provider_name,
            quantization: r.quantization,
            prompt: r.pricing.prompt.parse().ok(),
            completion: r.pricing.completion.parse().ok(),
            status: r.status,
        })
        .collect())
}

/// An alert line when the set of clamped endpoints changes (the first promo,
/// or the last one ending); None otherwise. `prev` is updated.
pub fn clamp_alert(prev: &mut Vec<String>, q: &OrQuote) -> Option<String> {
    let now: Vec<String> = q.used.iter().filter(|u| u.priced != u.listed).map(|u| format!("{} {}", u.provider, u.quantization)).collect();
    if now == *prev {
        return None;
    }
    *prev = now;
    if prev.is_empty() {
        return Some(format!("flop alert: no endpoint clamped (set {}, mark ${:.4})", q.set, q.usd_e6 as f64 / 1e6));
    }
    let legs: Vec<String> = q
        .used
        .iter()
        .filter(|u| u.priced != u.listed)
        .map(|u| format!("{} {} ${:.4} to ${:.4}", u.provider, u.quantization, u.listed, u.priced))
        .collect();
    Some(format!("flop alert: clamped {} (set {}, mark ${:.4})", legs.join(", "), q.set, q.usd_e6 as f64 / 1e6))
}

/// The mark under `openrouter`: the last good quote while it is fresh.
pub fn or_mark(q: Option<&OrQuote>, now_ms: u64, o: &launch::OracleCfg) -> Result<(u64, u64)> {
    let q = q.context("openrouter: no successful fetch yet")?;
    let age_s = now_ms.saturating_sub(q.at_ms) / 1000;
    if age_s > o.or_max_age_s {
        bail!("openrouter stale: last good fetch {age_s} s ago, max {}", o.or_max_age_s);
    }
    Ok((q.usd_e6, q.at_ms))
}

/// One status line: the mark's legs, the FLOP-implied FLOP/USD, caps applied.
pub fn status(or: Option<&OrQuote>, side: &Result<FlopSide>, mix: Option<&WindowMix>, o: &launch::OracleCfg) -> String {
    let mut t = format!("flop status: mark_source={}", o.flop_mark_source);
    match or {
        Some(q) => {
            let legs: Vec<String> = q
                .used
                .iter()
                .map(|u| {
                    if u.priced != u.listed {
                        format!("{} {} ${:.4} clamped to ${:.4}", u.provider, u.quantization, u.listed, u.priced)
                    } else {
                        format!("{} {} ${:.4}", u.provider, u.quantization, u.listed)
                    }
                })
                .collect();
            t.push_str(&format!("; openrouter ${:.4} ({} {} set: {})", q.usd_e6 as f64 / 1e6, o.or_weighting, q.set, legs.join(", ")));
        }
        None => t.push_str("; openrouter none"),
    }
    match side {
        Ok(s) => {
            t.push_str(&format!(
                "; flop {:.6} FLOP/G_n over {} prints, {} buyers, {} miners",
                s.flop_per_gn(), s.prints, s.agents, s.miners
            ));
            if !s.capped_agents.is_empty() || !s.capped_miners.is_empty() {
                t.push_str(&format!("; capped buyers {:?} miners {:?}", s.capped_agents, s.capped_miners));
            }
            if let Some(q) = or {
                let fx = q.usd_e6 as f64 / 1e6 / (s.flop_per_gn() * o.flop_gn_per_mtok as f64);
                t.push_str(&format!("; FLOP-implied FLOP/USD {fx:.10} (stand-in {:.10})", o.flop_usd_standin()));
            }
        }
        Err(e) => t.push_str(&format!("; {e}")),
    }
    if let Some(m) = mix {
        t.push_str(&format!(
            "; window {} prints, {} on-chain buyers, gateway agent {:.1}% of G_n",
            m.prints, m.buyers, m.gateway_gn_share * 100.0
        ));
    }
    t
}

impl FlopIndex {
    pub fn new(cfg: &launch::OracleCfg) -> Result<Self> {
        if !cfg.flop_mcp_url.starts_with("https://") {
            bail!("flop_mcp_url must start with https://");
        }
        if cfg.flop_min_prints == 0 || cfg.flop_window_s == 0 || cfg.flop_window_s > 7 * 86_400 {
            bail!("flop_min_prints must be > 0 and flop_window_s in 1..=604800");
        }
        if cfg.flop_max_age_s >= cfg.flop_window_s || cfg.flop_refresh_s < 5 {
            bail!("flop_max_age_s must be under flop_window_s, and flop_refresh_s at least 5");
        }
        let positive = |x: f64| x > 0.0 && x.is_finite();
        if !(positive(cfg.flop_ref_usd_in) && positive(cfg.flop_ref_usd_out) && positive(cfg.flop_blend_in_out)
            && positive(cfg.flop_anchor_flop_per_gn) && cfg.flop_gn_per_mtok > 0)
        {
            bail!("flop_ref_usd_in/out, flop_blend_in_out, flop_anchor_flop_per_gn and flop_gn_per_mtok must be > 0");
        }
        // Strictly under half: a group capped at exactly half ties everyone
        // else, and the lower-median would then take its price when it is low.
        let share = |x: f64, n: usize| x > 0.0 && x < 0.5 && x * n as f64 >= 1.0;
        if !(share(cfg.flop_max_agent_share, cfg.flop_min_agents) && share(cfg.flop_max_miner_share, cfg.flop_min_miners)) {
            bail!("flop_max_agent_share / flop_max_miner_share must be under 0.5, and share x flop_min_agents / flop_min_miners >= 1 so the caps can hold");
        }
        match cfg.flop_mark_source.as_str() {
            "openrouter" => {
                combine(&[1.0], &cfg.or_weighting)?;
                if !(cfg.or_min_frac_of_median >= 0.0 && cfg.or_min_frac_of_median < 1.0) {
                    bail!("or_min_frac_of_median must be in [0, 1)");
                }
                if !cfg.or_url.starts_with("https://") || cfg.or_min_endpoints == 0 || cfg.or_refresh_s < 60
                    || cfg.or_max_age_s < cfg.or_refresh_s
                {
                    bail!("or_url must be https, or_min_endpoints > 0, or_refresh_s >= 60, or_max_age_s >= or_refresh_s");
                }
            }
            "flop_standin" => {}
            other => bail!("flop_mark_source must be openrouter or flop_standin, not {other:?}"),
        }
        eprintln!(
            "flop index: USD per 1M tokens of the benchmark ({} GF/token); mark_source={}; {}:1 in:out; FLOP/USD STAND-IN {:.10}",
            cfg.flop_gn_per_mtok / 1_000_000, cfg.flop_mark_source, cfg.flop_blend_in_out, cfg.flop_usd_standin()
        );
        let client = http_client()?;
        let cached: Cache = Arc::new(Mutex::new(None));
        let or: OrCache = Arc::new(Mutex::new(None));
        let (writer, or_writer, o) = (Arc::clone(&cached), Arc::clone(&or), cfg.clone());
        let mut models: Vec<Model> = Vec::new();
        let (mut models_at, mut or_at) = (None::<Instant>, None::<Instant>);
        let (mut last_table, mut last_status) = (String::new(), String::new());
        let mut clamped: Vec<String> = Vec::new();
        std::thread::spawn(move || loop {
            // Model registry for the status table: hourly, and kept on a failed read.
            if models_at.is_none_or(|t: Instant| t.elapsed() >= Duration::from_secs(3600)) {
                match fetch_models(&client, &o) {
                    Ok(m) => (models, models_at) = (m, Some(Instant::now())),
                    Err(e) => eprintln!("flop model registry read failed: {e:#}"),
                }
            }
            // OpenRouter: every or_refresh_s; a failed read keeps the last good
            // quote, which `or_mark` ages out after or_max_age_s.
            if o.flop_mark_source == "openrouter" && or_at.is_none_or(|t: Instant| t.elapsed() >= Duration::from_secs(o.or_refresh_s)) {
                or_at = Some(Instant::now());
                match fetch_or(&client, &o) {
                    Ok(q) => {
                        if let Some(line) = clamp_alert(&mut clamped, &q) {
                            eprintln!("{line}");
                        }
                        *or_writer.lock().unwrap_or_else(|p| p.into_inner()) = Some(q);
                    }
                    Err(e) => eprintln!("openrouter read failed: {e:#}"),
                }
            }
            let fetched = fetch(&client, &o).map_err(|e| format!("{e:#}"));
            let now = launch::now_ms();
            let or_now = or_writer.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let side = match &fetched {
                Ok(prints) => flop_side(prints, now, &o),
                Err(e) => Err(anyhow!("flop read failed: {e}")),
            };
            let mix = fetched.as_ref().ok().map(|p| window_mix(p, now, &o));
            let st = status(or_now.as_ref(), &side, mix.as_ref(), &o);
            if st != last_status {
                eprintln!("{st}");
                last_status = st;
            }
            let mark = match o.flop_mark_source.as_str() {
                "openrouter" => or_mark(or_now.as_ref(), now, &o).ok(),
                _ => fetched.as_ref().ok().and_then(|p| index(p, now, &o).ok()),
            };
            if let Some((mark, _)) = mark {
                let t = table(mark, &models, &o);
                if t != last_table {
                    eprintln!("{t}");
                    last_table = t;
                }
            }
            let mut cache = writer.lock().unwrap_or_else(|p| p.into_inner());
            match fetched {
                // One failed read keeps the last good prints; the FLOP side still
                // ages them out after `flop_max_age_s`.
                Err(e) if matches!(*cache, Some(Ok(_))) => eprintln!("flop read failed (keeping last read): {e}"),
                Err(e) => {
                    eprintln!("flop read failed: {e}");
                    *cache = Some(Err(e));
                }
                Ok(prints) => *cache = Some(Ok(prints)),
            }
            drop(cache);
            std::thread::sleep(Duration::from_secs(o.flop_refresh_s));
        });
        Ok(Self { cfg: cfg.clone(), cached, or, last_inputs: None })
    }

    pub fn latest(&mut self) -> Result<(u64, u64)> {
        self.last_inputs = None;
        let now = launch::now_ms();
        let (side, mix) = match &*self.cached.lock().unwrap_or_else(|p| p.into_inner()) {
            None => (Err(anyhow!("flop index: no read yet")), None),
            Some(Ok(prints)) => (flop_side(prints, now, &self.cfg), Some(window_mix(prints, now, &self.cfg))),
            Some(Err(e)) => (Err(anyhow!("flop read failed: {e}")), None),
        };
        let side_json = match &side {
            Ok(s) => serde_json::json!({
                "flop_per_gn": s.flop_per_gn(), "prints": s.prints, "buyers": s.agents, "miners": s.miners,
                "newest_ms": s.newest_ms, "capped_buyers": s.capped_agents, "capped_miners": s.capped_miners,
                "window": mix,
            }),
            Err(e) => serde_json::json!({ "stale": e.to_string(), "window": mix }),
        };
        let o = &self.cfg;
        let (mark, inputs) = if o.flop_mark_source == "openrouter" {
            let q = self.or.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let mark = or_mark(q.as_ref(), now, o)?;
            let q = q.expect("or_mark checked");
            (mark, serde_json::json!({
                "source": "openrouter", "url": o.or_url, "fetched_ms": q.at_ms, "set": q.set,
                "blend_in_out": o.flop_blend_in_out, "quantizations": o.or_quantizations,
                "min_endpoints": o.or_min_endpoints, "weighting": o.or_weighting,
                "min_frac_of_median": o.or_min_frac_of_median, "guard": "clamp", "used": q.used,
                "endpoints": q.endpoints, "flop_side": side_json,
            }))
        } else {
            let side = side?;
            let mark = index_from(&side, o)?;
            (mark, serde_json::json!({
                "source": "flop_standin", "gn_per_mtok": o.flop_gn_per_mtok,
                "flop_usd_standin": o.flop_usd_standin(), "flop_side": side_json,
            }))
        };
        self.last_inputs = Some(inputs);
        Ok(mark)
    }

    /// What the last `latest()` mark was made from (None after an error).
    pub fn last_inputs(&self) -> Option<&serde_json::Value> {
        self.last_inputs.as_ref()
    }
}

/// Newest-first settlements back to the window start, each counted once (a
/// settlement landing between pages shifts the offsets by a row).
fn fetch(client: &reqwest::blocking::Client, o: &launch::OracleCfg) -> Result<Vec<Print>> {
    let from = launch::now_ms().saturating_sub(o.flop_window_s * 1000);
    let mut seen = HashSet::new();
    let mut prints = Vec::new();
    for page in 0..MAX_PAGES {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "flop_list_settlements",
                       "arguments": {"limit": PAGE, "offset": page * PAGE}},
        });
        let page = parse_page(&post(client, o, &body)?)?;
        let oldest = page.data.iter().map(|r| r.settled_at).min();
        for row in &page.data {
            if seen.insert((row.extrinsic_hash.clone(), row.event_index)) {
                prints.extend(row.print());
            }
        }
        if !page.pagination.has_more || oldest.is_none_or(|t| t < from) {
            return Ok(prints);
        }
    }
    bail!("more than {} settlements in the window; raise MAX_PAGES", PAGE * MAX_PAGES)
}

fn post(client: &reqwest::blocking::Client, o: &launch::OracleCfg, body: &serde_json::Value) -> Result<String> {
    Ok(client
        .post(&o.flop_mcp_url)
        .header("accept", "application/json, text/event-stream")
        .json(body)
        .send()?
        .error_for_status()?
        .text()?)
}

fn call(client: &reqwest::blocking::Client, o: &launch::OracleCfg, tool: &str, args: serde_json::Value) -> Result<serde_json::Value> {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": args},
    });
    let inner = tool_text(&post(client, o, &body)?)?;
    serde_json::from_str(&inner).with_context(|| format!("flop: {tool} result is not JSON"))
}

/// One registry model's compute per token, for the conversion table.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub name: String,
    pub gflops_per_token: f64,
}

/// Every model in FLOP's registry with its `gflopsPerToken`.
fn fetch_models(client: &reqwest::blocking::Client, o: &launch::OracleCfg) -> Result<Vec<Model>> {
    let found = call(client, o, "flop_discover_models", serde_json::json!({}))?;
    let names: Vec<String> = found["models"]
        .as_array()
        .context("flop: no models list")?
        .iter()
        .filter_map(|m| m["modelName"].as_str().map(str::to_owned))
        .collect();
    let mut out = Vec::new();
    for name in names {
        let p = call(client, o, "flop_get_model_pricing", serde_json::json!({"model_id": name}))?;
        if let Some(g) = p["gflopsPerToken"].as_f64().filter(|g| *g > 0.0) {
            out.push(Model { name, gflops_per_token: g });
        }
    }
    out.sort_by(|a, b| b.gflops_per_token.total_cmp(&a.gflops_per_token));
    Ok(out)
}

/// The benchmark mark converted per model and to pure compute. A model with g
/// GF/token costs mark x g x 1e6 / flop_gn_per_mtok per 1M tokens; an exaFLOP
/// (1e9 G_n) costs mark x 1e9 / flop_gn_per_mtok.
pub fn table(mark_e6: u64, models: &[Model], o: &launch::OracleCfg) -> String {
    let usd = mark_e6 as f64 / 1e6;
    let per_gn = usd / o.flop_gn_per_mtok as f64;
    let mut t = format!(
        "flop table: mark ${usd:.4} per 1M tokens (benchmark: {} GF/token); ${:.4} per exaFLOP",
        o.flop_gn_per_mtok / 1_000_000,
        per_gn * 1e9
    );
    for m in models {
        t.push_str(&format!("; {} ({} GF/token) ${:.4}", m.name, m.gflops_per_token, per_gn * m.gflops_per_token * 1e6));
    }
    t
}

/// The MCP reply is JSON or server-sent events; the first event carrying a
/// `result` or `error` is the reply. The tool result is JSON text.
fn parse_page(text: &str) -> Result<Page> {
    serde_json::from_str(&tool_text(text)?).context("flop: tool result is not a settlement page")
}

fn tool_text(text: &str) -> Result<String> {
    let normalized = text.replace("\r\n", "\n");
    let mut events = normalized.split("\n\n").filter_map(|event| {
        let data: Vec<&str> = event
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(|l| l.strip_prefix(' ').unwrap_or(l))
            .collect();
        (!data.is_empty()).then(|| data.join("\n"))
    });
    let reply: serde_json::Value = match events
        .find_map(|d| serde_json::from_str::<serde_json::Value>(&d).ok().filter(|v| v.get("result").is_some() || v.get("error").is_some()))
    {
        Some(v) => v,
        None => serde_json::from_str(text).context("flop: reply is not JSON")?,
    };
    if let Some(err) = reply.get("error") {
        bail!("flop: {err}");
    }
    let inner = reply["result"]["content"][0]["text"].as_str().context("flop: no tool result text")?;
    if reply["result"]["isError"].as_bool() == Some(true) {
        bail!("flop: {inner}");
    }
    Ok(inner.to_owned())
}

const SIG_KEY: &str = ",\"signature\":\"";

/// One signed push-log line: what was pushed, from what, signed by the oracle
/// authority. The signature covers the exact bytes of the line before
/// `,"signature":"..."` plus the closing `}` (re-serializing parsed JSON need
/// not reproduce float text, so the bytes are what is signed).
pub fn signed_push(oracle: &Keypair, fields: serde_json::Value) -> Result<String> {
    let mut rec = fields;
    let obj = rec.as_object_mut().context("push record must be an object")?;
    obj.insert("event".into(), "flop_push".into());
    obj.insert("oracle".into(), oracle.pubkey().to_string().into());
    obj.remove("signature");
    let payload = serde_json::to_string(&rec)?;
    let sig = oracle.sign_message(payload.as_bytes());
    Ok(format!("{}{SIG_KEY}{sig}\"}}", &payload[..payload.len() - 1]))
}

/// Check one push-log line: the signature, the signer (when `oracle` is
/// given), and for an OpenRouter mark that the logged endpoints give it.
pub fn verify_push(line: &str, oracle: Option<&Pubkey>) -> Result<u64> {
    let line = line.trim_end();
    let at = line.rfind(SIG_KEY).context("no signature")?;
    let sig: Signature = line[at + SIG_KEY.len()..].strip_suffix("\"}").context("signature must end the line")?.parse()?;
    let payload = format!("{}}}", &line[..at]);
    let rec: serde_json::Value = serde_json::from_str(&payload).context("not JSON")?;
    let signer: Pubkey = rec["oracle"].as_str().context("no oracle")?.parse()?;
    if oracle.is_some_and(|o| *o != signer) {
        bail!("signed by {signer}, not the expected oracle");
    }
    if !sig.verify(signer.as_ref(), payload.as_bytes()) {
        bail!("bad signature");
    }
    let mark = rec["mark_e6"].as_u64().context("no mark_e6")?;
    let inputs = &rec["inputs"];
    if inputs["source"] == "openrouter" {
        let blend = inputs["blend_in_out"].as_f64().context("blend_in_out")?;
        let quants: Vec<String> = serde_json::from_value(inputs["quantizations"].clone())?;
        let min: usize = serde_json::from_value(inputs["min_endpoints"].clone())?;
        let eps: Vec<OrEndpoint> = serde_json::from_value(inputs["endpoints"].clone())?;
        // Lines from before weighting was logged were medians.
        let weighting = inputs["weighting"].as_str().unwrap_or("median");
        let frac = inputs["min_frac_of_median"].as_f64().unwrap_or(0.0);
        let guard = match inputs["guard"].as_str() {
            Some("clamp") => Guard::Clamp,
            None => Guard::LegacyDrop,
            Some(g) => bail!("unknown guard {g}"),
        };
        let q = or_quote_with(&eps, 0, blend, &quants, min, weighting, frac, guard)?;
        if q.usd_e6 != mark {
            bail!("logged endpoints give {} but {mark} was pushed", q.usd_e6);
        }
        if let Some(logged) = inputs["used"].as_array() {
            // Float tolerance: a logged f64 may parse back one ulp off.
            let near = |v: &serde_json::Value, x: f64| v.as_f64().is_some_and(|y| (y - x).abs() <= 1e-9 * x.abs().max(1.0));
            let same = logged.len() == q.used.len()
                && logged.iter().zip(&q.used).all(|(l, u)| {
                    l["provider"] == u.provider.as_str() && l["quantization"] == u.quantization.as_str() && near(&l["listed"], u.listed) && near(&l["priced"], u.priced)
                });
            if !same {
                bail!("logged per-endpoint prices don't match the recomputed ones");
            }
        }
        if let Some(set) = inputs["set"].as_str() {
            if set != q.set {
                bail!("logged endpoints price from set {} but the line says {set}", q.set);
            }
        }
    }
    Ok(mark)
}

/// `staging verify-push-log <bots.log> <oracle pubkey>`
pub fn verify_log(args: &[String]) -> Result<()> {
    let path = args.first().context("usage: staging verify-push-log <bots.log> <oracle pubkey>")?;
    // Required: the signer is named inside each line, so without the expected
    // key any self-signed line would verify.
    let oracle: Pubkey = args.get(1).context("usage: staging verify-push-log <bots.log> <oracle pubkey>")?.parse()?;
    let oracle = Some(oracle);
    let text = std::fs::read_to_string(path).with_context(|| format!("read {path}"))?;
    let (mut ok, mut bad) = (0, 0);
    for (n, line) in text.lines().enumerate() {
        if !line.starts_with("{\"event\":\"flop_push\"") && !line.contains("\"event\":\"flop_push\"") {
            continue;
        }
        match verify_push(line, oracle.as_ref()) {
            Ok(_) => ok += 1,
            Err(e) => {
                bad += 1;
                println!("line {}: {e:#}", n + 1);
            }
        }
    }
    println!("{ok} pushes verified, {bad} failed");
    if bad > 0 || ok == 0 {
        bail!("push log did not verify");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn cfg() -> launch::OracleCfg {
        let c = launch::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../launch/flop-compute.toml")).unwrap();
        c.oracle
    }

    /// 0.003 FLOP per G_n, the devnet gateway list price.
    const P3: u128 = 3_000_000_000_000_000;

    fn p(age_s: u64, gn: u128, price: u128) -> Print {
        // Miners rotate over 7 and buyers over 5 (60 s steps too) so the
        // distinct-miner and distinct-buyer gates and the caps pass by default.
        Print { at_ms: NOW - age_s * 1000, gn, planck_per_gn: price, miner: format!("m{}", age_s % 7), agent: format!("a{}", (age_s + age_s / 60) % 5) }
    }

    fn many(n: usize, price: u128) -> Vec<Print> {
        (0..n).map(|i| p(60 + i as u64 * 60, 10, price)).collect()
    }

    #[test]
    fn shipped_config_has_sane_flop_gates() {
        let o = cfg();
        assert!(o.flop_min_prints >= 5);
        assert!(o.flop_max_age_s < o.flop_window_s);
        assert!(o.flop_min_miners >= 3);
        assert_eq!(o.flop_gn_per_mtok, 52_000_000);
        // 15:1 at $0.15 in / $1.875 out = $0.2578125 per 1M; at 0.003 FLOP/G_n and
        // 52e6 G_n per 1M tokens that is FLOP/USD 0.0000016527.
        assert!((o.flop_ref_usd_per_mtok() - 0.2578125).abs() < 1e-12);
        assert!((o.flop_usd_standin() - 1.6526442e-6).abs() < 1e-12);
    }

    #[test]
    fn shipped_config_is_the_scorers_baseline() {
        // The scorer (flop-index, docs/flop-compute/SCORER.md) scores
        // flop_index::SPEC; it must be the rule the pusher ships.
        let o = cfg();
        let s = &flop_index::SPEC;
        assert_eq!(o.flop_blend_in_out, s.blend);
        assert_eq!(o.or_quantizations, s.quantizations.iter().map(|q| q.to_string()).collect::<Vec<_>>());
        assert_eq!(o.or_min_endpoints, s.min_endpoints);
        assert_eq!(o.or_weighting, s.weighting);
        assert_eq!(o.or_min_frac_of_median, s.min_frac_of_median);
    }

    #[test]
    fn mark_is_usd_per_mtok_e6() {
        let o = cfg();
        // 0.003 FLOP/G_n, the anchor, prices 1M tokens at the blended reference: $0.2578125.
        let (mark, newest) = index(&many(o.flop_min_prints, P3), NOW, &o).unwrap();
        assert!(mark.abs_diff(257_813) <= 1, "{mark}");
        assert_eq!(newest, NOW - 60_000);
    }

    #[test]
    fn table_converts_per_model_and_per_exaflop() {
        let o = cfg();
        let models = [
            Model { name: "Qwen/Qwen3.8-27B".into(), gflops_per_token: 52.0 },
            Model { name: "Qwen/Qwen3-8B".into(), gflops_per_token: 17.0 },
            Model { name: "Qwen/Qwen2.5-0.5B-Instruct".into(), gflops_per_token: 2.0 },
        ];
        let t = table(257_813, &models, &o);
        // $0.257813 / 0.052 per exaFLOP; 17/52 and 2/52 of the mark per model.
        assert!(t.contains("mark $0.2578 per 1M tokens (benchmark: 52 GF/token)"), "{t}");
        assert!(t.contains("$4.9579 per exaFLOP"), "{t}");
        assert!(t.contains("Qwen/Qwen3.8-27B (52 GF/token) $0.2578"), "{t}");
        assert!(t.contains("Qwen/Qwen3-8B (17 GF/token) $0.0843"), "{t}");
        assert!(t.contains("Qwen/Qwen2.5-0.5B-Instruct (2 GF/token) $0.0099"), "{t}");
    }

    #[test]
    fn too_few_miners_is_stale() {
        let o = cfg();
        let mut prints = many(o.flop_min_prints, P3);
        for x in &mut prints {
            x.miner = "only".into();
        }
        let e = index(&prints, NOW, &o).unwrap_err();
        assert!(e.to_string().contains("distinct miners"), "{e}");
    }

    #[test]
    fn median_is_gn_weighted() {
        let mut o = cfg();
        o.flop_min_prints = 3;
        o.flop_min_miners = 1;
        o.flop_min_agents = 1;
        o.flop_max_agent_share = 1.0;
        o.flop_max_miner_share = 1.0;
        // One big print at the high price outweighs two small cheap ones.
        let prints = [p(60, 1, 1_000_000_000_000_000), p(60, 1, 1_000_000_000_000_000), p(60, 10, P3)];
        assert!(index(&prints, NOW, &o).unwrap().0.abs_diff(257_813) <= 1);
        // Equal weights: the lower middle (0.002 FLOP/G_n = 2/3 of that, $0.171875).
        let prints = [p(60, 1, 1_000_000_000_000_000), p(60, 1, 2_000_000_000_000_000), p(60, 1, P3)];
        assert!(index(&prints, NOW, &o).unwrap().0.abs_diff(171_875) <= 1);
    }

    #[test]
    fn too_few_prints_is_stale() {
        let o = cfg();
        let e = index(&many(o.flop_min_prints - 1, P3), NOW, &o).unwrap_err();
        assert!(e.to_string().contains("stale"), "{e}");
        // Prints older than the window don't count.
        let old: Vec<Print> = (0..o.flop_min_prints as u64).map(|i| p(o.flop_window_s + 1 + i, 10, 1)).collect();
        assert!(index(&old, NOW, &o).is_err());
    }

    #[test]
    fn gap_since_the_newest_print_is_stale() {
        let o = cfg();
        // Enough prints in the window, but the newest is older than the max age.
        let prints: Vec<Print> =
            (0..o.flop_min_prints as u64).map(|i| p(o.flop_max_age_s + 1 + i, 10, P3)).collect();
        let e = index(&prints, NOW, &o).unwrap_err();
        assert!(e.to_string().contains("newest print"), "{e}");
        // One fresh print ends the gap.
        let mut prints = prints;
        prints.push(p(10, 10, P3));
        assert!(index(&prints, NOW, &o).is_ok());
    }

    #[test]
    fn rows_filter_self_trades_forwards_and_junk() {
        let row = |kind: &str, settled: &str, self_trade: bool, gn: &str, escrow: &str| Row {
            settled_at: NOW,
            aggregate_gn: gn.into(),
            escrow: escrow.into(),
            miner: "m".into(),
            agent: "a".into(),
            market_kind: kind.into(),
            settlement_kind: settled.into(),
            self_trade,
            extrinsic_hash: "0x1".into(),
            event_index: 0,
        };
        // 10 FLOP escrow over 3436 G_n: 0.00291 FLOP/G_n (a live row, 2026-11-11).
        assert_eq!(
            row("spot", "settled", false, "3436", "10000000000000000000").print(),
            Some(Print { at_ms: NOW, gn: 3436, planck_per_gn: 2_910_360_884_749_708, miner: "m".into(), agent: "a".into() })
        );
        assert!(row("spot", "settled", true, "16", "16").print().is_none(), "self-trade");
        assert!(row("forward", "settled", false, "16", "16").print().is_none(), "forward");
        assert!(row("spot", "expired", false, "16", "16").print().is_none(), "not settled");
        assert!(row("spot", "settled", false, "0", "16").print().is_none(), "zero GN");
        assert!(row("spot", "settled", false, "16", "15").print().is_none(), "sub-planck price");
        assert!(row("spot", "settled", false, "16", "1.5").print().is_none(), "fractional escrow");
        assert!(row("spot", "settled", false, "x", "16").print().is_none(), "unparseable");
        assert!(row("spot", "settled", false, "18446744073709551616", "1").print().is_none(), "GN over u64");
    }

    #[test]
    fn parses_an_sse_reply_and_tool_errors() {
        let inner = r#"{"data":[{"settledAt":1,"aggregateGn":"16","escrow":"48000000000000000","miner":"m1","agent":"a1","marketKind":"spot","settlementKind":"settled","selfTrade":false,"extrinsicHash":"0x1","eventIndex":3,"extra":1}],"pagination":{"total":1,"limit":100,"offset":0,"hasMore":false}}"#;
        let reply = serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":inner}]}});
        let sse = format!("event: message\ndata: {reply}\n\n");
        // `data:` with no space is valid SSE too.
        assert_eq!(parse_page(&format!("data:{reply}\n")).unwrap().data.len(), 1);
        // A notification event before the reply is skipped.
        let note = r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{}}"#;
        assert_eq!(parse_page(&format!("data: {note}\n\ndata: {reply}\n\n")).unwrap().data.len(), 1);
        let page = parse_page(&sse).unwrap();
        assert_eq!(page.data.len(), 1);
        assert!(!page.pagination.has_more);
        let err = serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"isError":true,"content":[{"type":"text","text":"limit too big"}]}});
        assert!(parse_page(&err.to_string()).is_err());
    }

    #[test]
    fn a_flooding_buyer_is_capped_not_dropped() {
        let o = cfg();
        // 30 honest prints at 0.003 from 5 buyers, then one buyer floods the
        // window with 10x the honest weight at 0.03 FLOP/G_n.
        let mut prints = many(30, P3);
        for i in 0..30 {
            let mut x = p(61 + i * 60, 100, 10 * P3);
            x.agent = "whale".into();
            prints.push(x);
        }
        let side = flop_side(&prints, NOW, &o).unwrap();
        assert_eq!(side.capped_agents, vec!["whale".to_string()]);
        // Capped under half the weight, the whale is a minority: the median is
        // still the honest price.
        assert_eq!(side.planck_per_gn, P3);
        // Uncapped (all weight allowed), it would set the price.
        let mut loose = o.clone();
        loose.flop_max_agent_share = 1.0;
        assert_eq!(flop_side(&prints, NOW, &loose).unwrap().planck_per_gn, 10 * P3);
        // A whale pricing LOW can't drag it down either.
        for x in prints.iter_mut().filter(|x| x.agent == "whale") {
            x.planck_per_gn = P3 / 10;
        }
        assert_eq!(flop_side(&prints, NOW, &o).unwrap().planck_per_gn, P3);
        // A smaller cap leaves the whale a minority, kept in the window.
        let mut tight = o.clone();
        tight.flop_max_agent_share = 0.3;
        let s = flop_side(&prints, NOW, &tight).unwrap();
        assert_eq!((s.planck_per_gn, s.prints), (P3, 60));
    }

    #[test]
    fn a_flooding_miner_is_capped() {
        let o = cfg();
        let mut prints = many(30, P3);
        for i in 0..30 {
            let mut x = p(61 + i * 60, 100, 10 * P3);
            x.miner = "bigminer".into();
            prints.push(x);
        }
        let side = flop_side(&prints, NOW, &o).unwrap();
        assert!(side.capped_miners.contains(&"bigminer".to_string()), "{side:?}");
        assert_eq!(side.planck_per_gn, P3);
    }

    #[test]
    fn fewer_than_three_buyers_is_stale() {
        let o = cfg();
        assert_eq!(o.flop_min_agents, 3);
        let mut prints = many(o.flop_min_prints, P3);
        for (i, x) in prints.iter_mut().enumerate() {
            x.agent = format!("a{}", i % 2);
        }
        let e = flop_side(&prints, NOW, &o).unwrap_err();
        assert!(e.to_string().contains("distinct buyers"), "{e}");
        prints[0].agent = "a2".into();
        assert!(flop_side(&prints, NOW, &o).is_ok());
    }

    fn ep(provider: &str, q: &str, prompt: f64, completion: f64) -> OrEndpoint {
        OrEndpoint { provider: provider.into(), quantization: q.into(), prompt: prompt * 1e-6, completion: completion * 1e-6, status: 0 }
    }

    #[test]
    fn openrouter_picks_the_preferred_tier_and_takes_the_median() {
        let mut o = cfg();
        assert_eq!(o.flop_mark_source, "openrouter");
        // Tier selection, checked with the median weighting and no outlier guard.
        o.or_weighting = "median".into();
        o.or_min_frac_of_median = 0.0;
        // 2026-11-11 list: blended DeepInfra bf16 $0.2578, GMICloud bf16 $0.6219, Cerebras fp16 $1.021.
        let eps = [
            ep("DeepInfra", "bf16", 0.15, 1.875),
            ep("GMICloud", "bf16", 0.40, 3.95),
            ep("Cerebras", "fp16", 0.85, 3.6),
            ep("Cheap", "fp8", 0.01, 0.02),
        ];
        let q = or_quote(&eps, NOW, &o).unwrap();
        assert_eq!(q.set, "bf16");
        assert_eq!(q.used.len(), 2);
        // Even count: mean of the two (0.2578125 + 0.621875) / 2.
        assert!(q.usd_e6.abs_diff(439_844) <= 1, "{}", q.usd_e6);
        // One bf16 and one fp16: the preferred tiers together, mean of the two.
        let q = or_quote(&[eps[0].clone(), eps[2].clone(), eps[3].clone()], NOW, &o).unwrap();
        assert_eq!(q.set, "preferred");
        assert!(q.usd_e6.abs_diff(639_844) <= 1, "{}", q.usd_e6);
        // Only one preferred endpoint at all: every endpoint.
        let q = or_quote(&[eps[0].clone(), eps[3].clone()], NOW, &o).unwrap();
        assert_eq!(q.set, "all");
        assert!(or_quote(&[], NOW, &o).is_err());
    }

    #[test]
    fn openrouter_goes_stale_without_a_good_fetch() {
        let o = cfg();
        assert!(or_mark(None, NOW, &o).is_err());
        let q = OrQuote { usd_e6: 439_844, at_ms: NOW - o.or_max_age_s * 1000, used: vec![], set: "bf16".into(), endpoints: vec![] };
        assert_eq!(or_mark(Some(&q), NOW, &o).unwrap(), (439_844, q.at_ms));
        let e = or_mark(Some(&q), NOW + 1000, &o).unwrap_err();
        assert!(e.to_string().contains("stale"), "{e}");
    }

    #[test]
    fn parses_openrouter_endpoints() {
        let text = r#"{"data":{"id":"qwen/qwen3.8-27b","endpoints":[
            {"provider_name":"DeepInfra","quantization":"bf16","pricing":{"prompt":"0.00000015","completion":"0.000001875"}},
            {"provider_name":"X","pricing":{"prompt":"0.0000001","completion":"0.0000002"}},
            {"provider_name":"Bad","quantization":"fp8","pricing":{"prompt":"n/a","completion":"0"}}]}}"#;
        let eps = parse_or(text).unwrap();
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[1].quantization, "unknown");
        assert!((eps[0].completion - 1.875e-6).abs() < 1e-15);
    }

    #[test]
    fn the_gateway_share_is_logged_even_when_stale() {
        let o = cfg();
        let gw = o.flop_gateway_agent.clone();
        assert_eq!(gw, "5GsXb3TmNGixuEsbtixqHnqWr9UQJ2M8bkqBQWteNekgF98S");
        // Every print from the gateway and one other buyer: 2 buyers, stale.
        let mut prints = many(o.flop_min_prints, P3);
        for (i, x) in prints.iter_mut().enumerate() {
            x.agent = if i == 0 { "solo".into() } else { gw.clone() };
        }
        let side = flop_side(&prints, NOW, &o);
        assert!(side.is_err());
        let m = window_mix(&prints, NOW, &o);
        assert_eq!((m.prints, m.buyers), (prints.len(), 2));
        let want = 1.0 - prints[0].gn as f64 / prints.iter().map(|p| p.gn as f64).sum::<f64>();
        assert!((m.gateway_gn_share - want).abs() < 1e-12, "{m:?}");
        let s = status(None, &side, Some(&m), &o);
        assert!(s.contains("distinct buyers") && s.contains("2 on-chain buyers, gateway agent"), "{s}");
        // Nothing in the window: share 0, not NaN.
        assert_eq!(window_mix(&[], NOW, &o).gateway_gn_share, 0.0);
    }

    #[test]
    fn status_logs_the_flop_implied_rate() {
        let o = cfg();
        let q = OrQuote { usd_e6: 257_813, at_ms: NOW, used: vec![Used { provider: "DeepInfra".into(), quantization: "bf16".into(), listed: 0.2578, priced: 0.2578 }], set: "bf16".into(), endpoints: vec![] };
        let side = flop_side(&many(o.flop_min_prints, P3), NOW, &o);
        let s = status(Some(&q), &side, None, &o);
        // At the anchor price the implied rate equals the stand-in.
        assert!(s.contains("FLOP-implied FLOP/USD 0.000001652"), "{s}");
        assert!(s.contains("5 buyers, 7 miners"), "{s}");
    }

    #[test]
    fn signed_push_log_verifies_and_catches_tampering() {
        let o = cfg();
        let eps = vec![ep("DeepInfra", "bf16", 0.15, 1.875), ep("GMICloud", "bf16", 0.40, 3.95), ep("Cerebras", "fp16", 0.85, 3.6)];
        let q = or_quote(&eps, NOW, &o).unwrap();
        let oracle = Keypair::new();
        let inputs = serde_json::json!({
            "source": "openrouter", "set": q.set, "blend_in_out": o.flop_blend_in_out,
            "quantizations": o.or_quantizations, "min_endpoints": o.or_min_endpoints, "weighting": o.or_weighting,
            "min_frac_of_median": o.or_min_frac_of_median, "guard": "clamp", "used": q.used, "endpoints": q.endpoints,
        });
        let line = signed_push(&oracle, serde_json::json!({"mark_e6": q.usd_e6, "slot": 7, "inputs": inputs})).unwrap();
        assert_eq!(verify_push(&line, Some(&oracle.pubkey())).unwrap(), q.usd_e6);
        // Wrong expected signer.
        assert!(verify_push(&line, Some(&Keypair::new().pubkey())).is_err());
        // An edited mark breaks the signature.
        let forged = line.replace(&format!("\"mark_e6\":{}", q.usd_e6), "\"mark_e6\":500000");
        assert_ne!(forged, line);
        assert!(verify_push(&forged, None).unwrap_err().to_string().contains("bad signature"));
        // A correctly signed mark that its own inputs don't give is caught too.
        let wrong = signed_push(&oracle, serde_json::json!({"mark_e6": q.usd_e6 + 5000, "inputs": serde_json::from_str::<serde_json::Value>(&line).unwrap()["inputs"].clone()})).unwrap();
        assert!(verify_push(&wrong, None).unwrap_err().to_string().contains("was pushed"));
    }

    #[test]
    fn shipped_market_maker_is_small_and_pulls_when_stale() {
        let c = launch::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../launch/flop-compute.toml")).unwrap();
        let m = c.matcher;
        assert!(m.pull_when_stale && m.log_fills);
        assert_eq!(m.kill_file, "/run-dir/KILL");
        assert!(m.max_lp_inventory_usd <= 1000 && m.max_fill_notional_usd <= m.max_lp_inventory_usd);
        // One fill at ~$0.44 stays under the fill cap.
        assert!(m.size_q as u128 * 439_844 <= m.max_fill_notional_usd as u128 * 1_000_000_000_000);
    }

    #[test]
    fn inverse_square_weights_toward_the_cheap_endpoint() {
        let o = cfg();
        assert_eq!(o.or_weighting, "inverse_square");
        let (a, b) = (0.2578125, 0.621875);
        // Weights 1/b^2: sum(1/b) / sum(1/b^2).
        let want = (1.0 / a + 1.0 / b) / (1.0 / (a * a) + 1.0 / (b * b));
        assert!((combine(&[a, b], "inverse_square").unwrap() - want).abs() < 1e-12);
        // Today's bf16 pair: DeepInfra $0.2578, GMICloud $0.6219 -> ~$0.311.
        let eps = [ep("DeepInfra", "bf16", 0.15, 1.875), ep("GMICloud", "bf16", 0.45, 3.2), ep("Cerebras", "fp16", 0.99, 1.49)];
        let q = or_quote(&eps, NOW, &o).unwrap();
        assert_eq!((q.set.as_str(), q.usd_e6), ("bf16", 311_207));
        // The cheap one dominates but doesn't own it: well under the midpoint, above the cheap price.
        let m = q.usd_e6 as f64 / 1e6;
        assert!(m > a && m - a < (b - m) / 2.0, "{m}");
    }

    #[test]
    fn one_endpoint_is_its_own_price_and_an_expensive_newcomer_barely_moves_it() {
        assert!((combine(&[0.4], "inverse_square").unwrap() - 0.4).abs() < 1e-12);
        assert_eq!(combine(&[0.4], "median").unwrap(), 0.4);
        let (a, b) = (0.2578125, 0.621875);
        let before = combine(&[a, b], "inverse_square").unwrap();
        let after = combine(&[a, b, 5.0], "inverse_square").unwrap();
        // A $5 endpoint moves the weighted mark ~3%; the median would jump to $0.62.
        assert!(after > before && (after - before) / before < 0.05, "{before} -> {after}");
        assert_eq!(combine(&[a, b, 5.0], "median").unwrap(), b);
        assert!(combine(&[], "inverse_square").is_err());
        assert!(combine(&[0.0], "inverse_square").is_err());
        assert!(combine(&[0.3], "mean").is_err());
    }

    #[test]
    fn down_endpoints_do_not_count() {
        let o = cfg();
        let mut eps = vec![ep("DeepInfra", "bf16", 0.15, 1.875), ep("GMICloud", "bf16", 0.45, 3.2), ep("X", "bf16", 0.45, 3.2)];
        eps[0].status = -1;
        let q = or_quote(&eps, NOW, &o).unwrap();
        // DeepInfra is down: the two remaining bf16 endpoints, both $0.6219.
        assert_eq!(q.used.len(), 2);
        assert!(q.used.iter().all(|u| u.provider != "DeepInfra"));
        assert!(q.usd_e6.abs_diff(621_875) <= 1, "{}", q.usd_e6);
        // The down endpoint is still logged, so the log recomputes the same mark.
        assert_eq!(q.endpoints.len(), 3);
    }

    /// Today's list in miniature: the bf16 pair, Cerebras fp16, a cheap fp8.
    fn list_with_deepinfra_cut(cut: f64) -> Vec<OrEndpoint> {
        vec![
            ep("DeepInfra", "bf16", 0.15 * (1.0 - cut), 1.875 * (1.0 - cut)),
            ep("GMICloud", "bf16", 0.45, 3.2),
            ep("Cerebras", "fp16", 0.99, 1.49),
            ep("Chutes", "fp8", 0.039, 1.25),
        ]
    }

    fn mark(eps: &[OrEndpoint]) -> f64 {
        or_quote(eps, NOW, &cfg()).unwrap().usd_e6 as f64 / 1e6
    }

    #[test]
    fn a_15_percent_deepinfra_cut_still_pushes_from_bf16() {
        let o = cfg();
        let q = or_quote(&list_with_deepinfra_cut(0.15), NOW, &o).unwrap();
        assert_eq!(q.set, "bf16");
        // Deeper cuts clamp DeepInfra but stay in bf16.
        for cut in [0.22, 0.5, 0.9] {
            let q = or_quote(&list_with_deepinfra_cut(cut), NOW, &o).unwrap();
            assert_eq!(q.set, "bf16-clamped", "cut {cut}");
            assert_eq!(q.used.len(), 2);
            let d = q.used.iter().find(|u| u.provider == "DeepInfra").unwrap();
            assert!(d.priced > d.listed, "{d:?}");
        }
    }

    #[test]
    fn the_mark_moves_continuously_across_the_old_cliff() {
        // Under the drop rule a 22% cut jumped the mark ~3x ($0.24 -> $0.73).
        let (a, b) = (mark(&list_with_deepinfra_cut(0.21)), mark(&list_with_deepinfra_cut(0.22)));
        assert!((a - b).abs() / a < 0.02, "{a} -> {b}");
        // No step anywhere on a fine sweep: 0.1% more cut moves it under 0.5%.
        let mut prev = mark(&list_with_deepinfra_cut(0.0));
        for i in 1..990 {
            let m = mark(&list_with_deepinfra_cut(i as f64 / 1000.0));
            assert!((prev - m).abs() / prev < 0.005, "cut {}: {prev} -> {m}", i as f64 / 1000.0);
            prev = m;
        }
    }

    #[test]
    fn a_cut_never_raises_the_mark_for_endpoints_under_twice_it() {
        // Inverse-square weighting is only monotone for endpoints priced under 2x
        // the mark: a cheaper expensive endpoint wins weight (SPEC: "How
        // endpoints combine"). Today both bf16 endpoints are under 2x.
        let base = [
            ("DeepInfra", 0.15, 1.875),
            ("GMICloud", 0.45, 3.2),
            ("Novita", 0.30, 2.4),
            ("Pricey", 1.2, 9.0),
        ];
        for k in 2..=base.len() {
            for who in 0..k {
                let list = |cut: f64| -> Vec<OrEndpoint> {
                    base[..k]
                        .iter()
                        .enumerate()
                        .map(|(i, &(p, i_, o_))| {
                            let f = if i == who { 1.0 - cut } else { 1.0 };
                            ep(p, "bf16", i_ * f, o_ * f)
                        })
                        .collect()
                };
                let mut prev = mark(&list(0.0));
                for step in 1..100 {
                    let eps = list(step as f64 / 100.0);
                    let m = mark(&eps);
                    let me = (15.0 * eps[who].prompt + eps[who].completion) / 16.0 * 1e6;
                    if me < 2.0 * m {
                        // $0.00001 slack: usd_e6 rounding, and a step that crosses 2x.
                        assert!(m <= prev + 1e-5, "k {k} who {who} step {step}: {prev} -> {m}");
                    } else {
                        // Above 2x the mark a cut may raise it, but only a little.
                        assert!(m <= prev * 1.01, "k {k} who {who} step {step}: {prev} -> {m}");
                    }
                    prev = m;
                }
            }
        }
    }

    #[test]
    fn a_one_cent_promo_cannot_pull_the_mark_below_about_half() {
        let o = cfg();
        let (a, b) = ((0.15, 1.875), (0.45, 3.2));
        let fair = mark(&[ep("DeepInfra", "bf16", a.0, a.1), ep("GMICloud", "bf16", b.0, b.1)]);
        // A third bf16 listing at ~$0.01 would take ~99% of 1/b² weight unclamped.
        let eps = [ep("DeepInfra", "bf16", a.0, a.1), ep("GMICloud", "bf16", b.0, b.1), ep("Promo", "bf16", 0.005, 0.08)];
        let q = or_quote(&eps, NOW, &o).unwrap();
        assert_eq!(q.set, "bf16-clamped");
        let m = q.usd_e6 as f64 / 1e6;
        assert!(m >= 0.5 * fair, "{m} vs {fair}");
        let mut loose = o.clone();
        loose.or_min_frac_of_median = 0.0;
        assert!(or_quote(&eps, NOW, &loose).unwrap().usd_e6 < 20_000);
        // Two endpoints, one slashed to ~$0.01: it pushes, clamped, not below half.
        let eps = [ep("DeepInfra", "bf16", 0.005, 0.08), ep("GMICloud", "bf16", b.0, b.1)];
        let q = or_quote(&eps, NOW, &o).unwrap();
        assert_eq!(q.set, "bf16-clamped");
        let median = (0.0098 + 0.621875) / 2.0;
        assert!(q.usd_e6 as f64 / 1e6 >= 0.5 * median, "{}", q.usd_e6);
    }

    #[test]
    fn the_spec_example_cut_raises_the_mark_a_little() {
        // SPEC "Cheap-outlier clamp": an endpoint over 2x the mark cutting its price.
        let o = cfg();
        let third = |usd: f64| ep("Third", "bf16", usd, usd);
        let pair = [ep("DeepInfra", "bf16", 0.15, 1.875), ep("GMICloud", "bf16", 0.45, 3.2)];
        let with = |usd: f64| or_quote(&[pair[0].clone(), pair[1].clone(), third(usd)], NOW, &o).unwrap();
        let (hi, lo) = (with(3.0), with(0.8));
        assert_eq!((hi.set.as_str(), hi.usd_e6, lo.usd_e6), ("bf16-clamped", 395_508, 419_152));
        // Clamped to half the $0.621875 median.
        assert!((hi.used[0].priced - 0.3109375).abs() < 1e-9, "{:?}", hi.used[0]);
        // Unclamped, as hand-computed: $0.328 -> $0.351.
        let mut loose = o.clone();
        loose.or_min_frac_of_median = 0.0;
        let raw = |usd: f64| or_quote(&[pair[0].clone(), pair[1].clone(), third(usd)], NOW, &loose).unwrap().usd_e6;
        assert_eq!((raw(3.0), raw(0.8)), (328_046, 350_999));
    }

    #[test]
    fn a_clamp_raises_one_alert_and_one_all_clear() {
        let o = cfg();
        let pair = [ep("DeepInfra", "bf16", 0.15, 1.875), ep("GMICloud", "bf16", 0.45, 3.2)];
        let promo = ep("Promo", "bf16", 0.005, 0.08);
        let mut prev = Vec::new();
        let clean = or_quote(&pair, NOW, &o).unwrap();
        assert_eq!(clamp_alert(&mut prev, &clean), None);
        let hit = or_quote(&[pair[0].clone(), pair[1].clone(), promo], NOW, &o).unwrap();
        let line = clamp_alert(&mut prev, &hit).unwrap();
        assert!(line.starts_with("flop alert: clamped Promo bf16 $0.0097 to $0.1289"), "{line}");
        // One promo: about $0.170, as SPEC says.
        assert!(hit.usd_e6.abs_diff(170_000) < 1_000, "{}", hit.usd_e6);
        // Same clamp on the next fetch: no repeat.
        assert_eq!(clamp_alert(&mut prev, &hit), None);
        assert!(clamp_alert(&mut prev, &clean).unwrap().starts_with("flop alert: no endpoint clamped"));
    }

    #[test]
    fn half_the_set_cheap_drags_the_floor_down_too() {
        // Known limit (SPEC): the floor is half the median, so when cheap
        // listings are half the set the median itself is cheap. Two promos next
        // to today's pair take the mark from $0.311 to ~$0.076.
        let b = (0.45, 3.2);
        let eps = [
            ep("DeepInfra", "bf16", 0.15, 1.875),
            ep("GMICloud", "bf16", b.0, b.1),
            ep("PromoA", "bf16", 0.005, 0.08),
            ep("PromoB", "bf16", 0.005, 0.08),
        ];
        let m = mark(&eps);
        assert!((0.07..0.08).contains(&m), "{m}");
    }

    #[test]
    fn down_endpoints_fall_through_to_the_next_set() {
        let o = cfg();
        // bf16 has one up endpoint: fp16 alone has one too, so bf16+fp16.
        let mut eps = list_with_deepinfra_cut(0.0);
        eps[1].status = 1;
        let q = or_quote(&eps, NOW, &o).unwrap();
        // DeepInfra is under half of that pair's median, so it is clamped there.
        assert_eq!(q.set, "preferred-clamped");
        // Only the fp8 and one bf16 up: every up endpoint.
        eps[2].status = 1;
        let q = or_quote(&eps, NOW, &o).unwrap();
        assert!(q.set.starts_with("all"), "{}", q.set);
    }

    #[test]
    fn every_set_failing_holds() {
        let o = cfg();
        // One up endpoint in total: no set reaches 2.
        let e = or_quote(&[ep("DeepInfra", "bf16", 0.15, 1.875)], NOW, &o).unwrap_err();
        assert!(e.to_string().contains("no set has 2 up endpoints"), "{e}");
        // Down endpoints don't help.
        let mut eps = list_with_deepinfra_cut(0.0);
        for x in eps.iter_mut().skip(1) {
            x.status = 1;
        }
        assert!(or_quote(&eps, NOW, &o).is_err());
        assert!(or_quote(&[], NOW, &o).is_err());
    }

    fn line_for(o: &launch::OracleCfg, oracle: &Keypair, q: &OrQuote, set: &str, used: serde_json::Value) -> String {
        let inputs = serde_json::json!({
            "source": "openrouter", "set": set, "blend_in_out": o.flop_blend_in_out,
            "quantizations": o.or_quantizations, "min_endpoints": o.or_min_endpoints, "weighting": o.or_weighting,
            "min_frac_of_median": o.or_min_frac_of_median, "guard": "clamp", "used": used, "endpoints": q.endpoints,
        });
        signed_push(oracle, serde_json::json!({"mark_e6": q.usd_e6, "inputs": inputs})).unwrap()
    }

    #[test]
    fn the_verifier_passes_clamped_and_fallback_lines() {
        let o = cfg();
        let oracle = Keypair::new();
        // Clamped: logs DeepInfra's listed and clamped price.
        let q = or_quote(&list_with_deepinfra_cut(0.5), NOW, &o).unwrap();
        assert_eq!(q.set, "bf16-clamped");
        let line = line_for(&o, &oracle, &q, &q.set, serde_json::to_value(&q.used).unwrap());
        assert!(line.contains("\"listed\"") && line.contains("\"priced\""));
        assert_eq!(verify_push(&line, Some(&oracle.pubkey())).unwrap(), q.usd_e6);
        // Signed, right mark, wrong set named: caught.
        let wrong = line_for(&o, &oracle, &q, "bf16", serde_json::to_value(&q.used).unwrap());
        assert!(verify_push(&wrong, Some(&oracle.pubkey())).unwrap_err().to_string().contains("set"));
        // Signed, right mark, a clamped price logged as unclamped: caught.
        let mut used = serde_json::to_value(&q.used).unwrap();
        for u in used.as_array_mut().unwrap() {
            u["priced"] = u["listed"].clone();
        }
        let wrong = line_for(&o, &oracle, &q, &q.set, used);
        assert!(verify_push(&wrong, Some(&oracle.pubkey())).unwrap_err().to_string().contains("per-endpoint"));
        // Fallback: GMICloud down, priced from bf16+fp16.
        let mut eps = list_with_deepinfra_cut(0.0);
        eps[1].status = 1;
        let q = or_quote(&eps, NOW, &o).unwrap();
        assert!(q.set.starts_with("preferred"), "{}", q.set);
        let line = line_for(&o, &oracle, &q, &q.set, serde_json::to_value(&q.used).unwrap());
        assert_eq!(verify_push(&line, Some(&oracle.pubkey())).unwrap(), q.usd_e6);
    }

    #[test]
    fn lines_from_before_the_clamp_still_verify_under_the_drop_rule() {
        let o = cfg();
        let oracle = Keypair::new();
        // A promo the drop rule removed: those lines priced the bf16 pair only.
        let eps = [ep("DeepInfra", "bf16", 0.15, 1.875), ep("GMICloud", "bf16", 0.45, 3.2), ep("Promo", "bf16", 0.005, 0.08)];
        let q = or_quote_with(&eps, NOW, o.flop_blend_in_out, &o.or_quantizations, 2, "inverse_square", 0.5, Guard::LegacyDrop).unwrap();
        assert_eq!((q.set.as_str(), q.usd_e6), ("bf16-trimmed", 311_207));
        let inputs = serde_json::json!({
            "source": "openrouter", "set": q.set, "blend_in_out": o.flop_blend_in_out,
            "quantizations": o.or_quantizations, "min_endpoints": 2, "weighting": "inverse_square",
            "min_frac_of_median": 0.5, "endpoints": q.endpoints,
        });
        let line = signed_push(&oracle, serde_json::json!({"mark_e6": q.usd_e6, "inputs": inputs})).unwrap();
        assert_eq!(verify_push(&line, Some(&oracle.pubkey())).unwrap(), 311_207);
        // A guard the verifier doesn't know is refused, not read as the old rule.
        let mut odd = inputs.clone();
        odd["guard"] = "winsorize".into();
        let line = signed_push(&oracle, serde_json::json!({"mark_e6": q.usd_e6, "inputs": odd})).unwrap();
        assert!(verify_push(&line, Some(&oracle.pubkey())).unwrap_err().to_string().contains("unknown guard"));
    }
}
