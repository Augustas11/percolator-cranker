//! FLOP compute index (devnet demo, docs/flop-compute/INDEX.md).
//!
//! Reads realized compute-channel settlements from FLOP's read-only MCP server
//! and turns them into one mark. FLOP side: the G_n-weighted median of the
//! buyer's price, escrow / aggregateGn, over spot, non-self-trade settlements in
//! a rolling window. Too few prints, too few distinct miners, or none recent
//! enough, is an error: the pusher then sends nothing and the on-chain mark ages.
//! There is no fallback to FLOP's nominal gateway prices.
//!
//! Units: the mark is USD per 1M tokens of the reference model, e6:
//! (FLOP per G_n) x flop_gn_per_mtok x FLOP/USD. FLOP/USD is a STAND-IN on
//! devnet (`OracleCfg::flop_usd_standin`, derived from OpenRouter's price for the
//! same model at a configured input:output blend) until a live feed exists.

use super::*;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// A reader thread refetches every `flop_refresh_s`; `latest()` only reads its
/// cache, so the pusher never waits on FLOP while holding the trade lock.
/// The reader thread's latest result: `None` until its first fetch.
type Cache = Arc<Mutex<Option<Result<Vec<Print>, String>>>>;

#[derive(Clone)]
pub struct FlopIndex {
    cfg: launch::OracleCfg,
    cached: Cache,
}

/// One settlement as the index uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct Print {
    pub at_ms: u64,
    pub gn: u128,
    /// Buyer's price: escrow / aggregateGn, planck (1e-18 FLOP) per G_n.
    pub planck_per_gn: u128,
    pub miner: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    settled_at: u64,
    aggregate_gn: String,
    escrow: String,
    miner: String,
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
        // Bounded so the GN sums in `index` can't overflow.
        let gn = self.aggregate_gn.parse().ok().filter(|&g: &u128| g > 0 && g <= u64::MAX as u128)?;
        let escrow: u128 = self.escrow.parse().ok()?;
        let price = Some(escrow / gn).filter(|&p| p > 0)?;
        Some(Print { at_ms: self.settled_at, gn, planck_per_gn: price, miner: self.miner.clone() })
    }
}

/// The index at `now_ms`: (mark_e6, newest print time). Errors say why it is stale.
pub fn index(prints: &[Print], now_ms: u64, o: &launch::OracleCfg) -> Result<(u64, u64)> {
    let from = now_ms.saturating_sub(o.flop_window_s * 1000);
    let mut window: Vec<&Print> = prints.iter().filter(|p| p.at_ms >= from && p.at_ms <= now_ms).collect();
    if window.len() < o.flop_min_prints {
        bail!("flop index stale: {} prints in the last {} s, need {}", window.len(), o.flop_window_s, o.flop_min_prints);
    }
    let miners = window.iter().map(|p| p.miner.as_str()).collect::<HashSet<_>>().len();
    if miners < o.flop_min_miners {
        bail!("flop index stale: {miners} distinct miners in the last {} s, need {}", o.flop_window_s, o.flop_min_miners);
    }
    let newest = window.iter().map(|p| p.at_ms).max().unwrap_or(0);
    let age_s = now_ms.saturating_sub(newest) / 1000;
    if age_s > o.flop_max_age_s {
        bail!("flop index stale: newest print {age_s} s old, max {}", o.flop_max_age_s);
    }
    // GN-weighted median: the lowest price at which half the GN is covered.
    window.sort_by_key(|p| p.planck_per_gn);
    let total: u128 = window.iter().map(|p| p.gn).sum();
    let mut covered = 0u128;
    let mut median = 0u128;
    for p in &window {
        covered += p.gn;
        if covered * 2 >= total {
            median = p.planck_per_gn;
            break;
        }
    }
    // USD e6 per 1M tokens = planck/G_n / 1e18 x G_n per 1M tokens x FLOP/USD x 1e6.
    let mark = (median as f64 / 1e12 * o.flop_gn_per_mtok as f64 * o.flop_usd_standin()).round();
    if !(mark >= 1.0 && mark < u64::MAX as f64) {
        bail!("flop index: price {median} planck/G_n gives mark {mark}, out of range");
    }
    Ok((mark as u64, newest))
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
        eprintln!(
            "flop index: USD per 1M tokens; reference ${:.6}/1M at {}:1 in:out; FLOP/USD STAND-IN {:.10}",
            cfg.flop_ref_usd_per_mtok(), cfg.flop_blend_in_out, cfg.flop_usd_standin()
        );
        // A 100-row page took 3-7 s from Pearl on 2026-11-10; 5 s failed every read.
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let cached: Cache = Arc::new(Mutex::new(None));
        let (writer, o) = (Arc::clone(&cached), cfg.clone());
        let mut models: Vec<Model> = Vec::new();
        let (mut models_at, mut last_table) = (None::<Instant>, String::new());
        std::thread::spawn(move || loop {
            // Model registry for the status table: hourly, and kept on a failed read.
            if models_at.is_none_or(|t: Instant| t.elapsed() >= Duration::from_secs(3600)) {
                match fetch_models(&client, &o) {
                    Ok(m) => (models, models_at) = (m, Some(Instant::now())),
                    Err(e) => eprintln!("flop model registry read failed: {e:#}"),
                }
            }
            let fetched = fetch(&client, &o).map_err(|e| format!("{e:#}"));
            if let Ok(prints) = &fetched {
                if let Ok((mark, _)) = index(prints, launch::now_ms(), &o) {
                    let t = table(mark, &models, &o);
                    if t != last_table {
                        eprintln!("{t}");
                        last_table = t;
                    }
                }
            }
            let mut cache = writer.lock().unwrap_or_else(|p| p.into_inner());
            match fetched {
                // One failed read keeps the last good prints; `index` still
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
        Ok(Self { cfg: cfg.clone(), cached })
    }

    pub fn latest(&mut self) -> Result<(u64, u64)> {
        match &*self.cached.lock().unwrap_or_else(|p| p.into_inner()) {
            None => bail!("flop index: no read yet"),
            Some(Ok(prints)) => index(prints, launch::now_ms(), &self.cfg),
            Some(Err(e)) => bail!("flop read failed: {e}"),
        }
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
        // Miners rotate over 7 (60 s steps too) so the distinct-miner gate passes by default.
        Print { at_ms: NOW - age_s * 1000, gn, planck_per_gn: price, miner: format!("m{}", age_s % 7) }
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
            market_kind: kind.into(),
            settlement_kind: settled.into(),
            self_trade,
            extrinsic_hash: "0x1".into(),
            event_index: 0,
        };
        // 10 FLOP escrow over 3436 G_n: 0.00291 FLOP/G_n (a live row, 2026-11-11).
        assert_eq!(
            row("spot", "settled", false, "3436", "10000000000000000000").print(),
            Some(Print { at_ms: NOW, gn: 3436, planck_per_gn: 2_910_360_884_749_708, miner: "m".into() })
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
        let inner = r#"{"data":[{"settledAt":1,"aggregateGn":"16","escrow":"48000000000000000","miner":"m1","marketKind":"spot","settlementKind":"settled","selfTrade":false,"extrinsicHash":"0x1","eventIndex":3,"extra":1}],"pagination":{"total":1,"limit":100,"offset":0,"hasMore":false}}"#;
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
}
