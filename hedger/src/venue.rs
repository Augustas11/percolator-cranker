//! Hedge venue. `Venue` is the seam for a later Drift or Binance drop-in;
//! Hyperliquid is the one built. Its order signing follows the official
//! Python SDK (hyperliquid-python-sdk, MIT, utils/signing.py `sign_l1_action`)
//! and is pinned to a vector that SDK produced (tests below).

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};
use std::time::Duration;

pub struct Book {
    /// Best first: bids descending, asks ascending. (price, size SOL)
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
    pub mid: f64,
    /// Venue exchange time, ms.
    pub time_ms: u64,
}

pub struct Fill {
    /// Signed SOL actually filled.
    pub sol: f64,
    pub avg_px: f64,
}

pub trait Venue {
    fn name(&self) -> &'static str;
    /// Size increment, SOL.
    fn step(&self) -> f64;
    /// `coarse`: an aggregated book that reaches deeper (wider levels) for
    /// sizes the top of the book cannot fill.
    fn book(&self, coarse: bool) -> Result<Book>;
    /// Hourly funding rate as a fraction (longs pay when positive).
    fn funding_hourly(&self) -> Result<f64>;
    /// Live only: our signed SOL position on the venue.
    fn position(&self) -> Result<f64>;
    /// Live only: immediate-or-cancel at `limit_px`; returns what filled.
    fn ioc(&self, sol: f64, limit_px: f64) -> Result<Fill>;
}

pub struct Hyperliquid {
    http: reqwest::blocking::Client,
    url: String,
    mainnet: bool,
    coin: String,
    asset: u32,
    sz_decimals: u32,
    /// Live only: signs as an API (agent) wallet for `account`.
    signer: Option<(libsecp256k1::SecretKey, String)>,
}

impl Hyperliquid {
    pub fn connect(network: &str, coin: &str) -> Result<Self> {
        let (url, mainnet) = match network {
            "mainnet" => ("https://api.hyperliquid.xyz", true),
            "testnet" => ("https://api.hyperliquid-testnet.xyz", false),
            n => bail!("venue.network must be mainnet or testnet, not {n:?}"),
        };
        // Use system roots, not the bundled webpki-roots 0.25.
        let http = reqwest::blocking::Client::builder().use_native_tls().timeout(Duration::from_secs(5)).build()?;
        let mut v = Self {
            http,
            url: url.into(),
            mainnet,
            coin: coin.into(),
            asset: 0,
            sz_decimals: 0,
            signer: None,
        };
        let meta = v.info(json!({"type": "meta"}))?;
        let universe = meta["universe"].as_array().context("meta.universe")?;
        let i = universe
            .iter()
            .position(|a| a["name"] == coin)
            .with_context(|| format!("{coin} not listed on Hyperliquid {network}"))?;
        v.asset = i as u32;
        v.sz_decimals = universe[i]["szDecimals"].as_u64().context("szDecimals")? as u32;
        Ok(v)
    }

    /// `key_hex`: the API wallet's secret key. `account`: the main account it trades for.
    pub fn with_signer(mut self, key_hex: &str, account: &str) -> Result<Self> {
        let bytes = hex::decode(key_hex.trim().trim_start_matches("0x")).context("key is not hex")?;
        let sk = libsecp256k1::SecretKey::parse_slice(&bytes).map_err(|_| anyhow::anyhow!("bad secret key"))?;
        let account = account.trim().to_lowercase();
        if account.len() != 42 || !account.starts_with("0x") || hex::decode(&account[2..]).is_err() {
            bail!("HL_ACCOUNT_ADDRESS must be a 0x address");
        }
        self.signer = Some((sk, account));
        Ok(self)
    }

    pub fn check_agent(&self) -> Result<()> {
        let (sk, account) = self.signer.as_ref().context("agent check needs live credentials")?;
        let role = self.info(json!({"type": "userRole", "user": address(sk).to_lowercase()}))?;
        if role["role"] != "agent" || role["data"]["user"].as_str().map(str::to_lowercase).as_ref() != Some(account) {
            bail!("API wallet must be an agent for HL_ACCOUNT_ADDRESS");
        }
        Ok(())
    }

    pub fn agent_address(&self) -> Option<String> {
        self.signer.as_ref().map(|(sk, _)| address(sk))
    }

    fn info(&self, body: Value) -> Result<Value> {
        let r = self.http.post(format!("{}/info", self.url)).json(&body).send()?;
        let status = r.status();
        let v: Value = r.json().context("info reply is not JSON")?;
        if !status.is_success() {
            bail!("info {status}: {v}");
        }
        Ok(v)
    }

    /// Hyperliquid prices: 5 significant figures and at most 6 - szDecimals decimals.
    pub fn round_px(&self, px: f64) -> f64 {
        round_px(px, self.sz_decimals)
    }
}

impl Venue for Hyperliquid {
    fn name(&self) -> &'static str {
        "hyperliquid"
    }

    fn step(&self) -> f64 {
        10f64.powi(-(self.sz_decimals as i32))
    }

    fn book(&self, coarse: bool) -> Result<Book> {
        // 20 levels a side; nSigFigs 3 buckets them at 0.1 near $100.
        let req = if coarse {
            json!({"type": "l2Book", "coin": self.coin, "nSigFigs": 3})
        } else {
            json!({"type": "l2Book", "coin": self.coin})
        };
        let b = self.info(req)?;
        let side = |i: usize| -> Result<Vec<(f64, f64)>> {
            b["levels"][i]
                .as_array()
                .context("l2Book.levels")?
                .iter()
                .map(|l| Ok((num(&l["px"])?, num(&l["sz"])?)))
                .collect()
        };
        let (bids, asks) = (side(0)?, side(1)?);
        let (Some(bid), Some(ask)) = (bids.first(), asks.first()) else { bail!("empty book") };
        if bid.0 >= ask.0 {
            bail!("crossed book");
        }
        let mid = (bid.0 + ask.0) / 2.0;
        Ok(Book { bids, asks, mid, time_ms: b["time"].as_u64().unwrap_or(0) })
    }

    fn funding_hourly(&self) -> Result<f64> {
        let v = self.info(json!({"type": "metaAndAssetCtxs"}))?;
        num(&v[1][self.asset as usize]["funding"])
    }

    fn position(&self) -> Result<f64> {
        let (_, account) = self.signer.as_ref().context("position needs live credentials")?;
        let s = self.info(json!({"type": "clearinghouseState", "user": account}))?;
        let rows = s["assetPositions"].as_array().context("assetPositions")?;
        for r in rows {
            if r["position"]["coin"] == self.coin.as_str() {
                return num(&r["position"]["szi"]);
            }
        }
        Ok(0.0)
    }

    fn ioc(&self, sol: f64, limit_px: f64) -> Result<Fill> {
        let (sk, _) = self.signer.as_ref().context("orders need live credentials")?;
        let action = order_action(self.asset, sol > 0.0, wire(limit_px, 8), wire(sol.abs(), self.sz_decimals));
        let nonce = now_ms();
        let (r, s, v) = sign_l1(sk, &action, nonce, self.mainnet)?;
        let body = json!({"action": action, "nonce": nonce,
            "signature": {"r": r, "s": s, "v": v}, "vaultAddress": null, "expiresAfter": null});
        let reply: Value = self.http.post(format!("{}/exchange", self.url)).json(&body).send()?.json()?;
        if reply["status"] != "ok" {
            bail!("exchange: {reply}");
        }
        let statuses = &reply["response"]["data"]["statuses"];
        let Some(statuses) = statuses.as_array().filter(|s| s.len() == 1) else {
            bail!("unexpected order status: {statuses}");
        };
        let st = &statuses[0];
        if let Some(e) = st["error"].as_str() {
            bail!("order refused: {e}");
        }
        let f = &st["filled"];
        if f.is_null() {
            bail!("unexpected order status: {st}");
        }
        Ok(Fill { sol: num(&f["totalSz"])?.copysign(sol), avg_px: num(&f["avgPx"])? })
    }
}

fn num(v: &Value) -> Result<f64> {
    let x: f64 = v.as_str().with_context(|| format!("expected a number string, got {v}"))?.parse()?;
    if !x.is_finite() {
        bail!("non-finite number {x}");
    }
    Ok(x)
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

pub fn round_px(px: f64, sz_decimals: u32) -> f64 {
    if !(px > 0.0 && px.is_finite()) {
        return 0.0;
    }
    let sig = 4 - px.log10().floor() as i32; // decimals that keep 5 significant figures
    let d = sig.clamp(0, 6 - sz_decimals as i32);
    let f = 10f64.powi(d);
    (px * f).round() / f
}

/// The SDK's `float_to_wire`: fixed decimals, trailing zeros trimmed.
pub fn wire(x: f64, decimals: u32) -> String {
    let s = format!("{:.*}", decimals as usize, x);
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
    if s == "-0" { "0".into() } else { s.into() }
}

// Field order is the msgpack key order the venue hashes: keep it as the SDK's dicts.
#[derive(Serialize)]
pub struct OrderAction {
    #[serde(rename = "type")]
    kind: &'static str,
    orders: Vec<OrderWire>,
    grouping: &'static str,
}
#[derive(Serialize)]
struct OrderWire {
    a: u32,
    b: bool,
    p: String,
    s: String,
    r: bool,
    t: OrderType,
}
#[derive(Serialize)]
struct OrderType {
    limit: Limit,
}
#[derive(Serialize)]
struct Limit {
    tif: &'static str,
}

pub fn order_action(asset: u32, is_buy: bool, px: String, sz: String) -> OrderAction {
    OrderAction {
        kind: "order",
        orders: vec![OrderWire { a: asset, b: is_buy, p: px, s: sz, r: false, t: OrderType { limit: Limit { tif: "Ioc" } } }],
        grouping: "na",
    }
}

fn keccak(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Keccak256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// SDK `action_hash` with no vault and no expiry.
pub fn action_hash(action: &OrderAction, nonce: u64) -> Result<[u8; 32]> {
    let mut data = rmp_serde::to_vec_named(action)?;
    data.extend_from_slice(&nonce.to_be_bytes());
    data.push(0);
    Ok(keccak(&[&data]))
}

/// EIP-712 sign of the phantom agent {source, connectionId} (SDK `l1_payload`).
pub fn sign_l1(sk: &libsecp256k1::SecretKey, action: &OrderAction, nonce: u64, mainnet: bool) -> Result<(String, String, u8)> {
    let conn = action_hash(action, nonce)?;
    let mut chain = [0u8; 32];
    chain[30..].copy_from_slice(&1337u16.to_be_bytes());
    let domain = keccak(&[
        &keccak(&[b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"]),
        &keccak(&[b"Exchange"]),
        &keccak(&[b"1"]),
        &chain,
        &[0u8; 32],
    ]);
    let source: &[u8] = if mainnet { b"a" } else { b"b" };
    let agent = keccak(&[&keccak(&[b"Agent(string source,bytes32 connectionId)"]), &keccak(&[source]), &conn]);
    let digest = keccak(&[b"\x19\x01", &domain, &agent]);
    let (sig, rec) = libsecp256k1::sign(&libsecp256k1::Message::parse(&digest), sk);
    let b = sig.serialize();
    Ok((format!("0x{}", hex::encode(&b[..32])), format!("0x{}", hex::encode(&b[32..])), 27 + rec.serialize()))
}

pub fn address(sk: &libsecp256k1::SecretKey) -> String {
    let pk = libsecp256k1::PublicKey::from_secret_key(sk).serialize();
    format!("0x{}", hex::encode(&keccak(&[&pk[1..]])[12..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Produced by hyperliquid-python-sdk 0.24.0: `sign_l1_action` of
    // order_wires_to_order_action([order_request_to_order_wire(
    //   {coin SOL, is_buy, sz 12.34, limit_px 101.25, Ioc, reduce_only False}, 5)]),
    // vault None, nonce 1700000000000, expires None. Test key only.
    const KEY: &str = "0123456789012345678901234567890123456789012345678901234567890123";

    #[test]
    fn signing_matches_official_sdk() {
        let sk = libsecp256k1::SecretKey::parse_slice(&hex::decode(KEY).unwrap()).unwrap();
        assert_eq!(address(&sk), "0x14791697260e4c9a71f18484c9f997b308e59325");
        let a = order_action(5, true, wire(101.25, 8), wire(12.34, 2));
        assert_eq!(
            hex::encode(rmp_serde::to_vec_named(&a).unwrap()),
            "83a474797065a56f72646572a66f72646572739186a16105a162c3a170a63130312e3235a173a531322e3334a172c2a17481a56c696d697481a3746966a3496f63a867726f7570696e67a26e61"
        );
        assert_eq!(
            hex::encode(action_hash(&a, 1_700_000_000_000).unwrap()),
            "257b832f82d6f84bbdb29c20cf894d3a76d7a33ff59b4000f892f9da332c6342"
        );
        let (r, s, v) = sign_l1(&sk, &a, 1_700_000_000_000, true).unwrap();
        assert_eq!(r, "0x154fd76418095d1a48c326e154f4d65b2ed0ada92c8ae45889f3fbfb79aec910");
        assert_eq!(s, "0x0177c4a353dde2097526e3d5d5099152c2f9055794498898622a9db384cc35be");
        assert_eq!(v, 28);
        let (r, s, v) = sign_l1(&sk, &a, 1_700_000_000_000, false).unwrap();
        assert_eq!(r, "0x0375841b0d6ecaa1c96405edb178c39971a0be60be9c1f434a9a051f825295bf");
        assert_eq!(s, "0x636b2e9cecaa4e580c5e6ca450e7db53435ef8cf65665dd6e5ae38636b70c565");
        assert_eq!(v, 27);
    }

    #[test]
    fn price_and_size_wire_format() {
        assert_eq!(round_px(99.78765, 2), 99.788);
        assert_eq!(round_px(101.2549, 2), 101.25);
        assert_eq!(round_px(1.234567, 2), 1.2346);
        assert_eq!(round_px(123456.7, 2), 123457.0);
        assert_eq!(wire(101.25, 8), "101.25");
        assert_eq!(wire(100.0, 8), "100");
        assert_eq!(wire(12.3449, 2), "12.34");
        assert_eq!(wire(0.001, 2), "0");
    }
}
