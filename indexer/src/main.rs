mod api;
mod db;
mod decode;
mod ingest;
mod stats;

use anyhow::{bail, Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::{
    path::PathBuf,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub struct Config {
    pub rpc_url: String,
    pub program: Pubkey,
    pub market: Pubkey,
    pub db: PathBuf,
    pub bind: String,
    pub cors_origins: Vec<String>,
    pub poll_secs: u64,
    pub rpc_delay_ms: u64,
    pub cycle_secs: u64,
    pub uptime_slack_cycles: u64,
}
impl Config {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<(String, Self)> {
        let mut values = std::collections::BTreeMap::new();
        for (key, default) in [
            ("rpc-url", "https://api.devnet.solana.com"),
            ("program-id", "GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS"),
            ("market", "7p8DtcMHSjdCsM9xWvBwF6TebGBSL18wKvfM7BS1zyHq"),
            ("db", "indexer.sqlite"),
            ("bind", "127.0.0.1:8090"),
            (
                "cors-origins",
                "http://localhost:5173,http://127.0.0.1:5173",
            ),
            ("poll-secs", "10"),
            ("rpc-delay-ms", "250"),
            ("cycle-secs", "3"),
            ("uptime-slack-cycles", "10"),
        ] {
            let env = format!("INDEXER_{}", key.replace('-', "_").to_uppercase());
            values.insert(
                key.to_owned(),
                std::env::var(env).unwrap_or_else(|_| default.into()),
            );
        }
        let mut args = args.into_iter();
        let command = args
            .next()
            .context("usage: indexer <run|backfill|serve> [--flag value]")?;
        if !matches!(command.as_str(), "run" | "backfill" | "serve") {
            bail!("unknown command: {command}");
        }
        while let Some(flag) = args.next() {
            let key = flag.strip_prefix("--").context("expected --flag value")?;
            if !values.contains_key(key) {
                bail!("unknown flag: {flag}");
            }
            let value = args.next().context(format!("missing value for {flag}"))?;
            values.insert(key.into(), value);
        }
        let get = |k: &str| values.get(k).unwrap().clone();
        let number =
            |k: &str| -> Result<u64> { get(k).parse().with_context(|| format!("invalid {k}")) };
        let cfg = Self {
            rpc_url: get("rpc-url"),
            program: Pubkey::from_str(&get("program-id"))?,
            market: Pubkey::from_str(&get("market"))?,
            db: get("db").into(),
            bind: get("bind"),
            cors_origins: get("cors-origins")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            poll_secs: number("poll-secs")?,
            rpc_delay_ms: number("rpc-delay-ms")?,
            cycle_secs: number("cycle-secs")?,
            uptime_slack_cycles: number("uptime-slack-cycles")?,
        };
        anyhow::ensure!(
            cfg.poll_secs > 0 && cfg.cycle_secs > 0,
            "poll-secs and cycle-secs must be positive"
        );
        anyhow::ensure!(
            !cfg.cors_origins.iter().any(|o| o == "*"),
            "wildcard CORS is not supported"
        );
        Ok((command, cfg))
    }
}
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn main() -> Result<()> {
    let (command, cfg) = Config::parse(std::env::args().skip(1))?;
    let mut conn = db::open(&cfg.db)?;
    // Refuse mixing programs/markets; an RPC provider may change on the same network.
    for (key, value) in [
        ("program", cfg.program.to_string()),
        ("market", cfg.market.to_string()),
    ] {
        if let Some(existing) = db::meta(&conn, key)? {
            anyhow::ensure!(
                existing == value,
                "database {key} differs from configuration"
            );
        } else {
            db::set_meta(&conn, key, &value)?;
        }
    }
    match command.as_str() {
        "serve" => api::serve(cfg),
        "backfill" => {
            ingest::pass(&mut conn, &cfg)?;
            println!("{}", db::stats(&conn, &cfg, now())?);
            Ok(())
        }
        _ => {
            let server = api::server(&cfg)?;
            let http_cfg = cfg.clone();
            std::thread::spawn(move || api::requests(server, &http_cfg));
            loop {
                if let Err(e) = ingest::pass(&mut conn, &cfg) {
                    eprintln!("ingest: {e:#}");
                }
                std::thread::sleep(Duration::from_secs(cfg.poll_secs));
            }
        }
    }
}
