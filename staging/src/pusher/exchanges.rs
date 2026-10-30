use super::*;
use serde_json::Value;
use std::collections::BTreeSet;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Instant;
use tungstenite::{client::IntoClientRequest, Message};

const IDLE_RECONNECT: Duration = Duration::from_secs(60);
const MAX_BOOK_LEVELS: usize = 100_000;
const VENUES: [&str; 3] = ["coinbase", "kraken", "binance"];
const URLS: [&str; 2] = [
    "wss://ws-feed.exchange.coinbase.com",
    "wss://ws.kraken.com/v2",
];
const SUBSCRIPTIONS: [&str; 3] = [
    r#"{"type":"subscribe","product_ids":["SOL-USD"],"channels":["level2_batch"]}"#,
    r#"{"method":"subscribe","params":{"channel":"ticker","symbol":["SOL/USD"],"event_trigger":"bbo"}}"#,
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
    anchor: Option<(u64, Instant)>,
    fallback: bool,
}

pub struct Reading {
    pub price: u64,
    pub publish_time_ms: u64,
    pub venues: Vec<&'static str>,
    pub spread_bps: f64,
    pub fallback: bool,
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

fn filter(quotes: &Quotes, now: u64, cfg: &launch::OracleCfg, anchor: Option<(u64, u64)>) -> (Result<Reading>, Vec<String>) {
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
    let fresh_count = fresh.len();
    let baseline = median(&mut fresh.iter().map(|(_, q)| q.price).collect::<Vec<_>>()).unwrap_or(0);
    let used: Vec<_> = fresh.into_iter().filter(|(i, q)| {
        if q.price.abs_diff(baseline) as u128 * 10_000 > baseline as u128 * cfg.exchange_max_dev_bps as u128 {
            drops.push(format!("{}: outlier", VENUES[*i]));
            false
        } else { true }
    }).collect();
    let result = (|| {
        if used.is_empty() || used.len() < cfg.exchange_min_venues {
            // Only a lone fresh venue: other fresh venues that disagree mean no push.
            if fresh_count == 1 && cfg.exchange_fallback_ms > 0 {
                if let Some((price, _)) = anchor.filter(|(_, age_ms)| *age_ms <= cfg.exchange_fallback_ms) {
                    let (i, q) = used[0];
                    if q.price.abs_diff(price) as u128 * 10_000 <= price as u128 * cfg.exchange_fallback_max_dev_bps as u128 {
                        return Ok(Reading { price: q.price, publish_time_ms: q.timestamp,
                            venues: vec![VENUES[i]], spread_bps: 0.0, fallback: true });
                    }
                }
            }
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
            fallback: false,
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

#[derive(Default)]
struct Book { bids: BTreeSet<u64>, asks: BTreeSet<u64>, bad: bool }

fn mid(bid: u64, ask: u64, timestamp: u64) -> Option<Quote> {
    (bid < ask).then(|| Quote { price: bid + (ask - bid) / 2, timestamp })
}

fn level(side: &mut BTreeSet<u64>, price: &Value, size: &Value) -> Option<()> {
    let price = quote(price, 0)?.price;
    let size = size.as_str()?.parse::<f64>().ok()?;
    if !size.is_finite() || size < 0.0 { return None; }
    if size == 0.0 { side.remove(&price); } else { side.insert(price); }
    (side.len() <= MAX_BOOK_LEVELS).then_some(())
}

fn parse(venue: usize, text: &str, book: &mut Book) -> Option<Quote> {
    let v: Value = serde_json::from_str(text).ok()?;
    match venue {
        0 if v["product_id"] == "SOL-USD" && matches!(v["type"].as_str(), Some("snapshot" | "l2update")) => {
            let valid = (|| {
                match v["type"].as_str()? {
                    "snapshot" => {
                        *book = Book::default();
                        for row in v["bids"].as_array()? { level(&mut book.bids, &row[0], &row[1])?; }
                        for row in v["asks"].as_array()? { level(&mut book.asks, &row[0], &row[1])?; }
                    },
                    "l2update" => for row in v["changes"].as_array()? {
                        let side = match row[0].as_str()? { "buy" => &mut book.bids, "sell" => &mut book.asks, _ => return None };
                        level(side, &row[1], &row[2])?;
                    },
                    _ => return Some(()),
                }
                Some(())
            })();
            if valid.is_none() { book.bad = true; return None; }
            mid(*book.bids.last()?, *book.asks.first()?, timestamp(&v["time"])?)
        },
        1 if v["channel"] == "ticker" => v["data"].as_array()?.iter()
            .filter(|t| t["symbol"] == "SOL/USD")
            .filter_map(|t| mid(quote(&t["bid"], 0)?.price, quote(&t["ask"], 0)?.price, timestamp(&t["timestamp"])?))
            .max_by_key(|q| q.timestamp),
        // Binance SOL/USDT is treated as SOL/USD; no USDT/USD conversion.
        2 if v["e"] == "trade" && v["s"] == "SOLUSDT" => quote(&v["p"], v["T"].as_u64()?),
        2 if v["e"] == "24hrTicker" && v["s"] == "SOLUSDT" => mid(quote(&v["b"], 0)?.price, quote(&v["a"], 0)?.price, v["E"].as_u64()?),
        _ => None,
    }
}

fn is_idle_read(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(e) if matches!(e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

impl Exchanges {
    pub fn new(cfg: &launch::OracleCfg) -> Result<Self> {
        if !(1..=3).contains(&cfg.exchange_min_venues) { bail!("exchange_min_venues must be 1..=3"); }
        if !cfg.exchange_binance_url.starts_with("wss://") { bail!("exchange_binance_url must start with wss://"); }
        if cfg.exchange_binance_url.contains("@bookTicker") {
            bail!("exchange config: exchange_binance_url is a bookTicker stream; use .../solusdt@ticker");
        }
        if cfg.exchange_binance_url.contains("@trade") {
            eprintln!("exchange config: exchange_binance_url is a trade stream; use .../solusdt@ticker");
        }
        if cfg.exchange_max_age_ms < 5000 {
            eprintln!("exchange config: exchange_max_age_ms < 5000 stalls on quiet books");
        }
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
                        let mut book = Book::default();
                        let mut last_message = Instant::now();
                        loop {
                            if shared.strong_count() == 0 { return Ok(()); }
                            let message = match ws.read() {
                                Ok(message) => { last_message = Instant::now(); message },
                                Err(e) if is_idle_read(&e) => {
                                    if last_message.elapsed() >= IDLE_RECONNECT { bail!("websocket idle 60 s"); }
                                    continue;
                                },
                                Err(e) => return Err(e.into()),
                            };
                            match message {
                                Message::Text(text) => {
                                    let parsed = parse(i, &text, &mut book);
                                    if book.bad {
                                        if let Some(shared) = shared.upgrade() { shared.lock().unwrap()[i] = None; }
                                        bail!("coinbase book invalid");
                                    }
                                    if let Some(q) = parsed {
                                        if let Some(shared) = shared.upgrade() {
                                            let mut quotes = shared.lock().unwrap();
                                            cache_quote(&mut quotes[i], q, launch::now_ms(), skew_ms);
                                        }
                                        backoff = 1;
                                        last_error.clear();
                                    } else if i == 0 && book.bids.last().zip(book.asks.first()).is_none_or(|(bid, ask)| bid >= ask) {
                                        if let Some(shared) = shared.upgrade() { shared.lock().unwrap()[i] = None; }
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
        Ok(Self { quotes, cfg: cfg.clone(), last_drops: Vec::new(), anchor: None, fallback: false })
    }

    pub fn latest(&mut self) -> Result<Reading> {
        let quotes = *self.quotes.lock().unwrap();
        let now = launch::now_ms();
        let anchor = self.anchor.map(|(price, at)| (price, at.elapsed().as_millis().min(u64::MAX as u128) as u64));
        let (result, drops) = filter(&quotes, now, &self.cfg, anchor);
        let fallback = result.as_ref().is_ok_and(|r| r.fallback);
        if let Ok(r) = &result {
            if !r.fallback { self.anchor = Some((r.price, Instant::now())); }
            else if !self.fallback {
                eprintln!("exchange fallback: {} alone, {} ms since quorum", r.venues[0], anchor.unwrap().1);
            }
        }
        self.fallback = fallback;
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
    #[test]
    fn idle_read_errors() {
        use std::io::ErrorKind;
        for kind in [ErrorKind::WouldBlock, ErrorKind::TimedOut] {
            assert!(is_idle_read(&tungstenite::Error::Io(kind.into())));
        }
        assert!(!is_idle_read(&tungstenite::Error::Io(ErrorKind::ConnectionReset.into())));
        assert!(!is_idle_read(&tungstenite::Error::ConnectionClosed));
    }

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
        assert_eq!((cfg.oracle.exchange_fallback_ms, cfg.oracle.exchange_fallback_max_dev_bps), (10_000, 25));
        assert_eq!(cfg.oracle.exchange_binance_url, "wss://stream.binance.com:9443/ws/solusdt@ticker");
        assert_eq!((cfg.oracle.exchange_max_age_ms, cfg.oracle.exchange_max_dev_bps, cfg.oracle.exchange_min_venues), (5000, 50, 2));
    }
    #[test]
    fn non_wss_binance_url_rejected() {
        let mut cfg = cfg();
        cfg.exchange_binance_url = "ws://stream.binance.us:9443/ws/solusdt@trade".into();
        let error = Exchanges::new(&cfg).err().expect("non-wss Binance URL rejected");
        assert_eq!(error.to_string(), "exchange_binance_url must start with wss://");
    }
    #[test]
    fn legacy_exchange_config() {
        let text = include_str!("../../../launch/sol-perp.toml")
            .replace("@ticker", "@trade").replace("exchange_max_age_ms = 5000", "exchange_max_age_ms = 2000");
        let cfg: launch::Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.oracle.exchange_max_age_ms, 2000);
        drop(Exchanges::new(&cfg.oracle).unwrap());
        let mut cfg = cfg.oracle;
        cfg.exchange_binance_url = cfg.exchange_binance_url.replace("@trade", "@bookTicker");
        let error = Exchanges::new(&cfg).err().expect("bookTicker rejected");
        assert_eq!(error.to_string(), "exchange config: exchange_binance_url is a bookTicker stream; use .../solusdt@ticker");
    }
    #[test]
    fn invalid_coinbase_rows() {
        for message in [
            r#"{"type":"snapshot","product_id":"SOL-USD","bids":[["97","1"],["bad","1"]],"asks":[["98","1"]]}"#,
            r#"{"type":"snapshot","product_id":"SOL-USD","bids":[["97","1"]],"asks":[["98","-1"]]}"#,
            r#"{"type":"snapshot","product_id":"SOL-USD","bids":[],"asks":null}"#,
            r#"{"type":"l2update","product_id":"SOL-USD","changes":[["buy","97","NaN"]]}"#,
            r#"{"type":"l2update","product_id":"SOL-USD","changes":[["sell","bad","1"]]}"#,
            r#"{"type":"l2update","product_id":"SOL-USD","changes":[["other","97","1"]]}"#,
            r#"{"type":"l2update","product_id":"SOL-USD","changes":[["buy","97"]]}"#,
        ] {
            let mut book = Book::default();
            parse(0, include_str!("fixtures/coinbase_l2_snapshot.json"), &mut book).unwrap();
            assert!(parse(0, message, &mut book).is_none());
            assert!(book.bad, "{message}");
        }
    }
    #[test]
    fn coinbase_level_limit() {
        for side in ["buy", "sell"] {
            let mut book = Book::default();
            let levels = (1..=MAX_BOOK_LEVELS as u64).collect();
            if side == "buy" { book.bids = levels; } else { book.asks = levels; }
            let update = |size| format!(r#"{{"type":"l2update","product_id":"SOL-USD","changes":[["{side}","97","{size}"]]}}"#);
            assert!(parse(0, &update("0"), &mut book).is_none());
            assert!(!book.bad); // Exactly the cap is allowed.
            assert!(parse(0, &update("1"), &mut book).is_none());
            assert!(book.bad);
        }
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
        assert!(filter(&[q(100, 5500); 3], 5000, &cfg(), None).0.is_ok());
    }
    #[test]
    fn two_venue_spread() {
        assert!(filter(&[q(99_600_000, 5000), q(100_400_000, 5000), None], 5000, &cfg(), None).0.is_err());
        assert!(filter(&[q(99_750_000, 5000), q(100_250_000, 5000), None], 5000, &cfg(), None).0.is_ok());
        assert!(filter(&[q(99_750_000, 5000), q(100_250_001, 5000), None], 5000, &cfg(), None).0.is_err());
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
        let (r, drops) = filter(&[q(100_000_000, 4000), q(100_200_000, 4999), q(100_000_000, 0)], 5001, &cfg, None);
        let r = r.unwrap();
        assert_eq!(r.price, 100_100_000);
        assert_eq!(r.publish_time_ms, 4999);
        assert_eq!(r.venues, ["coinbase", "kraken"]);
        assert_eq!(drops, ["binance: stale"]);
        let (r, drops) = filter(&[q(100_000_000, 4990), q(100_200_000, 4999), q(110_000_000, 5000)], 5000, &cfg, None);
        assert_eq!(r.unwrap().publish_time_ms, 4999); // excluded newest must not mask lag
        assert_eq!(drops, ["binance: outlier"]);
        assert!(filter(&[q(100, 5000), None, None], 5000, &cfg, None).0.is_err());
        assert!(filter(&[None; 3], 5000, &cfg, None).0.is_err());
        assert!(filter(&[q(100, 5501); 3], 5000, &cfg, None).0.is_err());
        assert!(filter(&[q(0, 5000); 3], 5000, &cfg, None).0.is_err());
        assert!(filter(&[q(100, 0); 3], 5000, &cfg, None).0.is_ok());
        assert!(filter(&[q(100, 0); 3], 5001, &cfg, None).0.is_err());
    }
    #[test]
    fn deviation_boundary() {
        let cfg = cfg();
        let (r, _) = filter(&[q(99_500_000, 5000), q(100_000_000, 5000), q(100_500_000, 5000)], 5000, &cfg, None);
        assert_eq!(r.unwrap().venues.len(), 3);
        let (r, drops) = filter(&[q(99_499_999, 5000), q(100_000_000, 5000), q(100_500_001, 5000)], 5000, &cfg, None);
        assert!(r.is_err());
        assert_eq!(drops.len(), 2);
    }
    #[test]
    fn quote_messages() {
        let mut book = Book::default();
        let q = parse(0, include_str!("fixtures/coinbase_l2_snapshot.json"), &mut book).unwrap();
        assert_eq!(q, Quote { price: 97_080_000, timestamp: 1_793_350_856_013 });
        assert_eq!(parse(0, include_str!("fixtures/coinbase_l2update.json"), &mut book).unwrap().price, q.price);
        let update = |price: &str, size: &str| format!(r#"{{"type":"l2update","product_id":"SOL-USD","changes":[["buy","{price}","{size}"]],"time":"2026-10-30T09:00:57Z"}}"#);
        assert_eq!(parse(0, &update("97.08", "1"), &mut book).unwrap().price, 97_085_000);
        assert_eq!(parse(0, &update("97.08", "0.000"), &mut book).unwrap().price, q.price);
        assert!(parse(0, &update("97.09", "1"), &mut book).is_none());
        assert!(parse(0, &update("97.10", "1"), &mut book).is_none());
        assert_eq!(parse(0, include_str!("fixtures/coinbase_l2_snapshot.json"), &mut book).unwrap().price, q.price);
        book.asks.clear();
        assert!(parse(0, &update("97.07", "1"), &mut book).is_none());
        assert_eq!(parse(1, include_str!("fixtures/kraken_ticker.json"), &mut book).unwrap(),
            Quote { price: 97_065_000, timestamp: 1_793_350_854_416 });
        let ticker = parse(2, include_str!("fixtures/binance_ticker.json"), &mut book).unwrap();
        assert_eq!(ticker, Quote { price: 97_200_000, timestamp: 1_793_352_000_123 });
        let (_, drops) = filter(&[None, None, Some(ticker)], ticker.timestamp + 5001, &cfg(), None);
        assert!(drops.iter().any(|drop| drop == "binance: stale"));
        assert!(parse(2, r#"{"s":"SOLUSDT","b":"97.19","a":"97.21"}"#, &mut book).is_none());
        assert_eq!(parse(2, include_str!("fixtures/binance.json"), &mut book).unwrap(),
            Quote { price: 99_680_000, timestamp: 1_793_245_344_802 });
    }

    #[test]
    fn fallback_does_not_refresh_anchor() {
        let now = launch::now_ms();
        let anchor = Some((100_000_000, Instant::now() - Duration::from_millis(6000)));
        let mut source = Exchanges {
            quotes: Arc::new(Mutex::new([q(100_000_000, now), None, None])),
            cfg: cfg(), last_drops: Vec::new(), anchor, fallback: false,
        };
        assert!(source.latest().unwrap().fallback);
        assert_eq!(source.anchor, anchor);
        assert!(source.latest().unwrap().fallback);
        assert_eq!(source.anchor, anchor);
        source.anchor = Some((100_000_000, Instant::now() - Duration::from_millis(10_001)));
        assert!(source.latest().is_err());
    }

    #[test]
    fn fallback_window_and_deviation() {
        let mut cfg = cfg();
        // Wall-clock changes do not extend or shorten the monotonic fallback age.
        assert!(filter(&[q(100_000_000, 0), None, None], 0, &cfg, Some((100_000_000, 10_001))).0.is_err());
        assert!(filter(&[q(100_000_000, 1_000_000), None, None], 1_000_000, &cfg, Some((100_000_000, 6000))).0.is_ok());
        let anchor = Some((100_000_000, 6000));
        let reading = filter(&[q(100_250_000, 6000), None, None], 6000, &cfg, anchor).0.unwrap();
        assert!(reading.fallback);
        assert_eq!((reading.price, reading.publish_time_ms, reading.spread_bps), (100_250_000, 6000, 0.0));
        assert_eq!(reading.venues, ["coinbase"]);
        assert!(filter(&[q(90_000_000, 6000), q(100_000_000, 6000), q(110_000_000, 6000)], 6000, &cfg, anchor).0.is_err());
        assert!(filter(&[q(100_000_000, 10_000), None, None], 10_000, &cfg, Some((100_000_000, 10_000))).0.is_ok());
        assert!(filter(&[q(100_000_000, 10_001), None, None], 10_001, &cfg, Some((100_000_000, 10_001))).0.is_err());
        assert!(filter(&[q(100_000_000, 12000), None, None], 12000, &cfg, Some((100_000_000, 12000))).0.is_err());
        assert!(filter(&[q(100_250_001, 6000), None, None], 6000, &cfg, anchor).0.is_err());
        assert!(filter(&[q(99_600_000, 6000), q(100_400_000, 6000), None], 6000, &cfg, anchor).0.is_err());
        cfg.exchange_fallback_ms = 0;
        assert!(filter(&[q(100_000_000, 6000), None, None], 6000, &cfg, anchor).0.is_err());
    }
}
