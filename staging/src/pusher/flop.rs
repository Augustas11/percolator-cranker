//! FLOP compute index (devnet demo, docs/flop-compute/INDEX.md).
//!
//! Reads realized compute-channel settlements from FLOP's read-only MCP server
//! and turns them into one mark: the GN-weighted median `pricePerGflop` over spot,
//! non-self-trade settlements in a rolling window. Too few prints, or none recent
//! enough, is an error: the pusher then sends nothing and the on-chain mark ages.
//! There is no fallback to FLOP's nominal gateway prices.
//!
//! Units: the mark is FLOP per TFLOP, e6 (`pricePerGflop` is planck per Gflop,
//! 18 decimals, so mark_e6 = planck_per_gflop / 1e9).

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
    pub planck_per_gflop: u128,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    settled_at: u64,
    aggregate_gn: String,
    price_per_gflop: String,
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

/// Integer part of a decimal string such as "2970000000000000.125".
fn integer_part(s: &str) -> Option<u128> {
    s.split('.').next()?.parse().ok()
}

impl Row {
    /// Spot, settled, not a self-trade, with a positive price and size.
    fn print(&self) -> Option<Print> {
        if self.market_kind != "spot" || self.settlement_kind != "settled" || self.self_trade {
            return None;
        }
        // Bounded so the GN sums in `index` can't overflow.
        let gn = self.aggregate_gn.parse().ok().filter(|&g: &u128| g > 0 && g <= u64::MAX as u128)?;
        let price = integer_part(&self.price_per_gflop).filter(|&p| p > 0)?;
        Some(Print { at_ms: self.settled_at, gn, planck_per_gflop: price })
    }
}

/// The index at `now_ms`: (mark_e6, newest print time). Errors say why it is stale.
pub fn index(prints: &[Print], now_ms: u64, o: &launch::OracleCfg) -> Result<(u64, u64)> {
    let from = now_ms.saturating_sub(o.flop_window_s * 1000);
    let mut window: Vec<&Print> = prints.iter().filter(|p| p.at_ms >= from && p.at_ms <= now_ms).collect();
    if window.len() < o.flop_min_prints {
        bail!("flop index stale: {} prints in the last {} s, need {}", window.len(), o.flop_window_s, o.flop_min_prints);
    }
    let newest = window.iter().map(|p| p.at_ms).max().unwrap_or(0);
    let age_s = now_ms.saturating_sub(newest) / 1000;
    if age_s > o.flop_max_age_s {
        bail!("flop index stale: newest print {age_s} s old, max {}", o.flop_max_age_s);
    }
    // GN-weighted median: the lowest price at which half the GN is covered.
    window.sort_by_key(|p| p.planck_per_gflop);
    let total: u128 = window.iter().map(|p| p.gn).sum();
    let mut covered = 0u128;
    let mut median = 0u128;
    for p in &window {
        covered += p.gn;
        if covered * 2 >= total {
            median = p.planck_per_gflop;
            break;
        }
    }
    let mark = (median + 500_000_000) / 1_000_000_000;
    if mark == 0 || mark > u64::MAX as u128 {
        bail!("flop index: price {median} planck/Gflop is out of range");
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
        // A 100-row page took 3-7 s from Pearl on 2026-11-10; 5 s failed every read.
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let cached: Cache = Arc::new(Mutex::new(None));
        let (writer, o) = (Arc::clone(&cached), cfg.clone());
        std::thread::spawn(move || loop {
            let fetched = fetch(&client, &o).map_err(|e| format!("{e:#}"));
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
        let text = client
            .post(&o.flop_mcp_url)
            .header("accept", "application/json, text/event-stream")
            .json(&body)
            .send()?
            .error_for_status()?
            .text()?;
        let page = parse_page(&text)?;
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

/// The MCP reply is JSON or server-sent events; the first event carrying a
/// `result` or `error` is the reply. The tool result is JSON text.
fn parse_page(text: &str) -> Result<Page> {
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
    serde_json::from_str(inner).context("flop: tool result is not a settlement page")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn cfg() -> launch::OracleCfg {
        let c = launch::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../launch/flop-compute.toml")).unwrap();
        c.oracle
    }

    fn p(age_s: u64, gn: u128, price: u128) -> Print {
        Print { at_ms: NOW - age_s * 1000, gn, planck_per_gflop: price }
    }

    fn many(n: usize, price: u128) -> Vec<Print> {
        (0..n).map(|i| p(60 + i as u64 * 60, 10, price)).collect()
    }

    #[test]
    fn shipped_config_has_sane_flop_gates() {
        let o = cfg();
        assert!(o.flop_min_prints >= 5);
        assert!(o.flop_max_age_s < o.flop_window_s);
    }

    #[test]
    fn mark_is_flop_per_tflop_e6() {
        let o = cfg();
        // 0.00297 FLOP per Gflop = 2.97 FLOP per TFLOP.
        let (mark, newest) = index(&many(o.flop_min_prints, 2_970_000_000_000_000), NOW, &o).unwrap();
        assert_eq!(mark, 2_970_000);
        assert_eq!(newest, NOW - 60_000);
    }

    #[test]
    fn median_is_gn_weighted() {
        let mut o = cfg();
        o.flop_min_prints = 3;
        // One big print at the high price outweighs two small cheap ones.
        let prints = [p(60, 1, 1_000_000_000_000_000), p(60, 1, 1_000_000_000_000_000), p(60, 10, 3_000_000_000_000_000)];
        assert_eq!(index(&prints, NOW, &o).unwrap().0, 3_000_000);
        // Equal weights: the lower middle.
        let prints = [p(60, 1, 1_000_000_000_000_000), p(60, 1, 2_000_000_000_000_000), p(60, 1, 3_000_000_000_000_000)];
        assert_eq!(index(&prints, NOW, &o).unwrap().0, 2_000_000);
    }

    #[test]
    fn too_few_prints_is_stale() {
        let o = cfg();
        let e = index(&many(o.flop_min_prints - 1, 2_970_000_000_000_000), NOW, &o).unwrap_err();
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
            (0..o.flop_min_prints as u64).map(|i| p(o.flop_max_age_s + 1 + i, 10, 2_970_000_000_000_000)).collect();
        let e = index(&prints, NOW, &o).unwrap_err();
        assert!(e.to_string().contains("newest print"), "{e}");
        // One fresh print ends the gap.
        let mut prints = prints;
        prints.push(p(10, 10, 2_970_000_000_000_000));
        assert!(index(&prints, NOW, &o).is_ok());
    }

    #[test]
    fn rows_filter_self_trades_forwards_and_junk() {
        let row = |kind: &str, settled: &str, self_trade: bool, gn: &str, price: &str| Row {
            settled_at: NOW,
            aggregate_gn: gn.into(),
            price_per_gflop: price.into(),
            market_kind: kind.into(),
            settlement_kind: settled.into(),
            self_trade,
            extrinsic_hash: "0x1".into(),
            event_index: 0,
        };
        assert_eq!(
            row("spot", "settled", false, "16", "2970000000000000.125").print(),
            Some(Print { at_ms: NOW, gn: 16, planck_per_gflop: 2_970_000_000_000_000 })
        );
        assert!(row("spot", "settled", true, "16", "1").print().is_none(), "self-trade");
        assert!(row("forward", "settled", false, "16", "1").print().is_none(), "forward");
        assert!(row("spot", "expired", false, "16", "1").print().is_none(), "not settled");
        assert!(row("spot", "settled", false, "0", "1").print().is_none(), "zero GN");
        assert!(row("spot", "settled", false, "16", "0.5").print().is_none(), "sub-planck price");
        assert!(row("spot", "settled", false, "x", "1").print().is_none(), "unparseable");
        assert!(row("spot", "settled", false, "18446744073709551616", "1").print().is_none(), "GN over u64");
    }

    #[test]
    fn parses_an_sse_reply_and_tool_errors() {
        let inner = r#"{"data":[{"settledAt":1,"aggregateGn":"16","pricePerGflop":"2970000000000000.125","marketKind":"spot","settlementKind":"settled","selfTrade":false,"extrinsicHash":"0x1","eventIndex":3,"extra":1}],"pagination":{"total":1,"limit":100,"offset":0,"hasMore":false}}"#;
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
