use super::*;
use serde_json::Value;
use std::net::{TcpStream, ToSocketAddrs};
use tungstenite::{client::IntoClientRequest, Message};

const VENUES: [&str; 3] = ["coinbase", "kraken", "binance"];
const URLS: [&str; 2] = [
    "wss://ws-feed.exchange.coinbase.com",
    "wss://ws.kraken.com/v2",
];
const SUBSCRIPTIONS: [&str; 3] = [
    r#"{"type":"subscribe","product_ids":["SOL-USD"],"channels":["ticker"]}"#,
    r#"{"method":"subscribe","params":{"channel":"trade","symbol":["SOL/USD"],"snapshot":true}}"#,
    "",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Quote { price: u64, timestamp: u64 }

type Quotes = [Option<Quote>; 3];

#[derive(Clone)]
pub struct Exchanges {
    quotes: Arc<Mutex<Quotes>>,
    cfg: launch::OracleCfg,
    last_drops: Vec<String>,
}

pub struct Reading {
    pub price: u64,
    pub publish_time_ms: u64,
    pub venues: Vec<&'static str>,
    pub spread_bps: f64,
}

// Midpoint for an even number of venues, avoiding addition overflow.
fn median(prices: &mut [u64]) -> Option<u64> {
    prices.sort_unstable();
    let n = prices.len();
    if n == 0 { None } else { Some(prices[(n - 1) / 2] + (prices[n / 2] - prices[(n - 1) / 2]) / 2) }
}

// Reject bad clocks before they can poison the timestamp ordering.
fn cache_quote(cached: &mut Option<Quote>, q: Quote, now: u64, skew_ms: u64) {
    if q.timestamp > now.saturating_add(skew_ms) { return; }
    if cached.map_or(true, |old| old.timestamp > now || q.timestamp >= old.timestamp) {
        *cached = Some(q);
    }
}

fn filter(quotes: &Quotes, now: u64, cfg: &launch::OracleCfg) -> (Result<Reading>, Vec<String>) {
    let mut drops = Vec::new();
    let fresh: Vec<_> = quotes.iter().enumerate().filter_map(|(i, q)| {
        let reason = match q {
            None => "missing",
            Some(q) if q.timestamp > now.saturating_add(cfg.exchange_clock_skew_ms) => "future timestamp",
            Some(q) if now.saturating_sub(q.timestamp) > cfg.exchange_max_age_ms => "stale",
            Some(q) if q.price == 0 => "invalid price",
            Some(q) => return Some((i, *q)),
        };
        drops.push(format!("{}: {reason}", VENUES[i]));
        None
    }).collect();
    let baseline = median(&mut fresh.iter().map(|(_, q)| q.price).collect::<Vec<_>>()).unwrap_or(0);
    let used: Vec<_> = fresh.into_iter().filter(|(i, q)| {
        if q.price.abs_diff(baseline) as u128 * 10_000 > baseline as u128 * cfg.exchange_max_dev_bps as u128 {
            drops.push(format!("{}: outlier", VENUES[*i]));
            false
        } else { true }
    }).collect();
    let result = (|| {
        if used.is_empty() || used.len() < cfg.exchange_min_venues {
            bail!("exchange quorum: {} usable venues, need {}", used.len(), cfg.exchange_min_venues);
        }
        let mut prices: Vec<_> = used.iter().map(|(_, q)| q.price).collect();
        let price = median(&mut prices).unwrap();
        if used.len() == 2 && (prices[1] - prices[0]) as u128 * 10_000 > price as u128 * cfg.exchange_max_dev_bps as u128 {
            bail!("exchange quorum: two-venue spread exceeds {} bps", cfg.exchange_max_dev_bps);
        }
        Ok(Reading {
            price,
            publish_time_ms: used.iter().map(|(_, q)| q.timestamp).max().unwrap(),
            venues: used.iter().map(|(i, _)| VENUES[*i]).collect(),
            spread_bps: (prices.last().unwrap() - prices[0]) as f64 * 10_000.0 / price as f64,
        })
    })();
    (result, drops)
}

fn timestamp(v: &Value) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok()?.timestamp_millis().try_into().ok()
}

fn quote(price: &Value, timestamp: u64) -> Option<Quote> {
    let price = price.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| price.as_f64())?;
    let scaled = (price * 1_000_000.0).round();
    if !scaled.is_finite() || scaled < 1.0 || scaled >= u64::MAX as f64 { return None; }
    Some(Quote { price: scaled as u64, timestamp })
}

fn parse(venue: usize, text: &str) -> Option<Quote> {
    let v: Value = serde_json::from_str(text).ok()?;
    match venue {
        0 if v["type"] == "ticker" && v["product_id"] == "SOL-USD" =>
            quote(&v["price"], timestamp(&v["time"])?),
        1 if v["channel"] == "trade" => v["data"].as_array()?.iter()
            .filter(|t| t["symbol"] == "SOL/USD")
            .filter_map(|t| quote(&t["price"], timestamp(&t["timestamp"])?))
            .max_by_key(|q| q.timestamp),
        // Binance SOL/USDT is treated as SOL/USD; no USDT/USD conversion.
        2 if v["e"] == "trade" && v["s"] == "SOLUSDT" => quote(&v["p"], v["T"].as_u64()?),
        _ => None,
    }
}

impl Exchanges {
    pub fn new(cfg: &launch::OracleCfg) -> Result<Self> {
        if !(1..=3).contains(&cfg.exchange_min_venues) { bail!("exchange_min_venues must be 1..=3"); }
        if !cfg.exchange_binance_url.starts_with("wss://") { bail!("exchange_binance_url must start with wss://"); }
        let quotes: Arc<Mutex<Quotes>> = Arc::new(Mutex::new([None; 3]));
        for i in 0..3 {
            // Weak ownership lets readers stop after the last source is dropped.
            let shared = Arc::downgrade(&quotes);
            let skew_ms = cfg.exchange_clock_skew_ms;
            let url = if i == 2 { cfg.exchange_binance_url.clone() } else { URLS[i].to_string() };
            std::thread::spawn(move || {
                let mut backoff = 1;
                let mut last_error = String::new();
                while shared.strong_count() != 0 {
                    let result = (|| -> Result<()> {
                        let req = url.as_str().into_client_request()?;
                        let host = req.uri().host().context("websocket host")?;
                        let port = req.uri().port_u16().unwrap_or(443);
                        let mut tcp = None;
                        for addr in (host, port).to_socket_addrs()? {
                            if let Ok(s) = TcpStream::connect_timeout(&addr, Duration::from_secs(5)) { tcp = Some(s); break; }
                        }
                        let tcp = tcp.context("websocket TCP connect failed")?;
                        tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
                        tcp.set_write_timeout(Some(Duration::from_secs(5)))?;
                        let (mut ws, _) = tungstenite::client_tls(req, tcp)?;
                        if !SUBSCRIPTIONS[i].is_empty() { ws.send(Message::Text(SUBSCRIPTIONS[i].into()))?; }
                        eprintln!("exchange {} connected", VENUES[i]);
                        loop {
                            if shared.strong_count() == 0 { return Ok(()); }
                            match ws.read()? {
                                Message::Text(text) => {
                                    if let Some(q) = parse(i, &text) {
                                        if let Some(shared) = shared.upgrade() {
                                            let mut quotes = shared.lock().unwrap();
                                            cache_quote(&mut quotes[i], q, launch::now_ms(), skew_ms);
                                        }
                                        backoff = 1;
                                        last_error.clear();
                                    }
                                },
                                Message::Ping(_) => ws.flush()?, // tungstenite queues the matching pong.
                                Message::Close(_) => bail!("websocket closed"),
                                _ => {},
                            }
                        }
                    })();
                    if let Err(e) = result {
                        let error = e.to_string();
                        if error != last_error { eprintln!("exchange {} reconnect: {error}", VENUES[i]); last_error = error; }
                    }
                    for _ in 0..backoff {
                        if shared.strong_count() == 0 { return; }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    backoff = (backoff * 2).min(30);
                }
            });
        }
        Ok(Self { quotes, cfg: cfg.clone(), last_drops: Vec::new() })
    }

    pub fn latest(&mut self) -> Result<Reading> {
        let quotes = *self.quotes.lock().unwrap();
        let (result, drops) = filter(&quotes, launch::now_ms(), &self.cfg);
        if drops != self.last_drops {
            eprintln!("exchange filter: {}", if drops.is_empty() { "all venues usable".into() } else { drops.join(", ") });
            self.last_drops = drops;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cfg() -> launch::OracleCfg {
        launch::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../launch/sol-perp.toml")).unwrap().oracle
    }
    fn q(price: u64, timestamp: u64) -> Option<Quote> { Some(Quote { price, timestamp }) }

    #[test]
    fn config_defaults() {
        let text = include_str!("../../../launch/sol-perp.toml");
        let old = text.lines().filter(|line| !line.starts_with("exchange_")).collect::<Vec<_>>().join("\n");
        let cfg: launch::Config = toml::from_str(&old).unwrap();
        assert_eq!(cfg.oracle.exchange_clock_skew_ms, 500);
        assert_eq!(cfg.oracle.exchange_binance_url, "wss://stream.binance.com:9443/ws/solusdt@trade");
        assert_eq!((cfg.oracle.exchange_max_age_ms, cfg.oracle.exchange_max_dev_bps, cfg.oracle.exchange_min_venues), (2000, 50, 2));
    }
    #[test]
    fn non_wss_binance_url_rejected() {
        let mut cfg = cfg();
        cfg.exchange_binance_url = "ws://stream.binance.us:9443/ws/solusdt@trade".into();
        let error = Exchanges::new(&cfg).err().expect("non-wss Binance URL rejected");
        assert_eq!(error.to_string(), "exchange_binance_url must start with wss://");
    }
    #[test]
    fn future_quote_recovers() {
        let mut cached = q(100, 4999);
        cache_quote(&mut cached, Quote { price: 999, timestamp: 6000 }, 5000, 500);
        assert_eq!(cached, q(100, 4999));
        cache_quote(&mut cached, Quote { price: 101, timestamp: 5000 }, 5000, 500);
        assert_eq!(cached, q(101, 5000));
        cached = q(999, 6000); // recover even an already poisoned cache
        cache_quote(&mut cached, Quote { price: 102, timestamp: 5000 }, 5000, 500);
        assert_eq!(cached, q(102, 5000));
        assert!(filter(&[q(100, 5500); 3], 5000, &cfg()).0.is_ok());
    }
    #[test]
    fn two_venue_spread() {
        assert!(filter(&[q(99_600_000, 5000), q(100_400_000, 5000), None], 5000, &cfg()).0.is_err());
        assert!(filter(&[q(99_750_000, 5000), q(100_250_000, 5000), None], 5000, &cfg()).0.is_ok());
        assert!(filter(&[q(99_750_000, 5000), q(100_250_001, 5000), None], 5000, &cfg()).0.is_err());
    }
    #[test]
    fn medians() {
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [9]), Some(9));
        assert_eq!(median(&mut [3, 1, 2]), Some(2));
        assert_eq!(median(&mut [4, 2]), Some(3));
        assert_eq!(median(&mut [u64::MAX, u64::MAX]), Some(u64::MAX));
    }
    #[test]
    fn stale_outlier_and_quorum() {
        let cfg = cfg();
        let (r, drops) = filter(&[q(100_000_000, 4000), q(100_200_000, 4999), q(100_000_000, 2999)], 5000, &cfg);
        let r = r.unwrap();
        assert_eq!(r.price, 100_100_000);
        assert_eq!(r.publish_time_ms, 4999);
        assert_eq!(r.venues, ["coinbase", "kraken"]);
        assert_eq!(drops, ["binance: stale"]);
        let (r, drops) = filter(&[q(100_000_000, 4990), q(100_200_000, 4999), q(110_000_000, 5000)], 5000, &cfg);
        assert_eq!(r.unwrap().publish_time_ms, 4999); // excluded newest must not mask lag
        assert_eq!(drops, ["binance: outlier"]);
        assert!(filter(&[q(100, 5000), None, None], 5000, &cfg).0.is_err());
        assert!(filter(&[None; 3], 5000, &cfg).0.is_err());
        assert!(filter(&[q(100, 5501); 3], 5000, &cfg).0.is_err());
        assert!(filter(&[q(0, 5000); 3], 5000, &cfg).0.is_err());
        assert!(filter(&[q(100, 3000); 3], 5000, &cfg).0.is_ok());
    }
    #[test]
    fn deviation_boundary() {
        let cfg = cfg();
        let (r, _) = filter(&[q(99_500_000, 5000), q(100_000_000, 5000), q(100_500_000, 5000)], 5000, &cfg);
        assert_eq!(r.unwrap().venues.len(), 3);
        let (r, drops) = filter(&[q(99_499_999, 5000), q(100_000_000, 5000), q(100_500_001, 5000)], 5000, &cfg);
        assert!(r.is_err());
        assert_eq!(drops.len(), 2);
    }
    // Fixtures captured from the public endpoints on 2026-10-29.
    #[test]
    fn coinbase_message() {
        let q = parse(0, include_str!("fixtures/coinbase.json")).unwrap();
        assert_eq!(q, Quote { price: 99_550_000, timestamp: 1_793_245_344_007 });
        assert!(parse(0, r#"{"type":"heartbeat","time":"2026-10-29T00:00:00Z"}"#).is_none());
    }
    #[test]
    fn kraken_message() {
        let q = parse(1, include_str!("fixtures/kraken.json")).unwrap();
        assert_eq!(q, Quote { price: 99_550_000, timestamp: 1_793_245_339_625 });
        assert!(parse(1, r#"{"channel":"heartbeat"}"#).is_none());
    }
    #[test]
    fn binance_message() {
        let q = parse(2, include_str!("fixtures/binance.json")).unwrap();
        assert_eq!(q, Quote { price: 99_680_000, timestamp: 1_793_245_344_802 });
        assert!(parse(2, r#"{"e":"trade","s":"BTCUSDT","p":"1","T":1}"#).is_none());
    }
}
