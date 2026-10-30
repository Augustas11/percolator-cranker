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
    pub leaderboard_exclude: Vec<String>,
    pub start_slot: Option<u64>,
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
            ("market", "HyMKxfct1S9aFtq21GG3FUYUViQkmpZdk5pwGRtAz48C"),
            ("db", "indexer.sqlite"),
            ("bind", "127.0.0.1:8090"),
            (
                "cors-origins",
                "http://localhost:5173,http://127.0.0.1:5173",
            ),
            ("leaderboard-exclude", "6wH2d4CARhEWeGC2deBDb9pVT1a7Tgd48VUiFmkTNAHY,TVA9xtfvX8uiFAqDHCpeD4HzwT6CYb5iwD6J16cba46,GUvzajrrfWpizSGj38iCC7TfB7rbbWxTgCszPAh1VRyh,Fxi2q4iFrhPpfemCzkCK2haseAuoh6GvK7h7zwmaNZeu,CUJ86gVNfnTowKt3k7QVbCa589hpUtZhiypEnt9Ben8B,ANNQ2dksYNTWJG33jo5okUPb7AqpjUu6gcMN62bWdJc7"),
            ("start-slot", ""),
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
            leaderboard_exclude: get("leaderboard-exclude")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    Pubkey::from_str(s).context("invalid leaderboard-exclude pubkey")?;
                    Ok(s.to_owned())
                })
                .collect::<Result<Vec<_>>>()?,
            start_slot: if get("start-slot").is_empty() {
                None
            } else {
                Some(number("start-slot")?)
            },
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
fn redact_rpc(text: &str, rpc_url: &str) -> String {
    if rpc_url.is_empty() {
        text.to_owned()
    } else {
        text.replace(rpc_url, "<rpc>")
    }
}
fn main() -> Result<()> {
    let (command, cfg) = Config::parse(std::env::args().skip(1))?;
    run(&command, &cfg).map_err(|e| anyhow::anyhow!(redact_rpc(&format!("{e:#}"), &cfg.rpc_url)))
}
fn run(command: &str, cfg: &Config) -> Result<()> {
    let mut conn = db::open(&cfg.db)?;
    db::check_start_slot(&conn, cfg.start_slot)?;
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
    match command {
        "serve" => api::serve(cfg.clone()),
        "backfill" => {
            ingest::pass(&mut conn, cfg)?;
            println!("{}", db::cached_stats(&conn, cfg)?);
            Ok(())
        }
        _ => {
            let server = api::server(cfg)?;
            let http_cfg = cfg.clone();
            std::thread::spawn(move || api::requests(server, &http_cfg));
            loop {
                if let Err(e) = ingest::pass(&mut conn, cfg) {
                    eprintln!("ingest: {}", redact_rpc(&format!("{e:#}"), &cfg.rpc_url));
                }
                std::thread::sleep(Duration::from_secs(cfg.poll_secs));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn leaderboard_exclude_validates_pubkeys_and_accepts_empty_override() {
        let (_, cfg) =
            Config::parse(["serve".into(), "--leaderboard-exclude".into(), "".into()]).unwrap();
        assert!(cfg.leaderboard_exclude.is_empty());
        let p = "11111111111111111111111111111111";
        let (_, cfg) = Config::parse([
            "serve".into(),
            "--leaderboard-exclude".into(),
            format!(" {p}, {p} "),
        ])
        .unwrap();
        assert_eq!(cfg.leaderboard_exclude, vec![p, p]);
        assert!(
            Config::parse(["serve".into(), "--leaderboard-exclude".into(), "bad".into()]).is_err()
        );
    }
    #[test]
    fn start_slot_parses_optional_u64() {
        for (value, expected) in [
            ("", None),
            ("0", Some(0)),
            ("123", Some(123)),
            ("18446744073709551615", Some(u64::MAX)),
        ] {
            let (_, cfg) =
                Config::parse(["serve".into(), "--start-slot".into(), value.into()]).unwrap();
            assert_eq!(cfg.start_slot, expected);
        }
        for value in ["-1", "bad", "18446744073709551616"] {
            assert!(Config::parse(["serve".into(), "--start-slot".into(), value.into()]).is_err());
        }
    }
    #[test]
    fn empty_start_slot_environment_is_unset() {
        // Keep environment overrides in a child so parallel config tests are unaffected.
        const CHILD: &str = "INDEXER_TEST_EMPTY_START_SLOT";
        if std::env::var_os(CHILD).is_some() {
            let (_, cfg) = Config::parse(["serve".into()]).unwrap();
            assert_eq!(cfg.start_slot, None);
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::empty_start_slot_environment_is_unset"])
            .env(CHILD, "1")
            .env("INDEXER_START_SLOT", "")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    #[test]
    fn rpc_redaction_covers_error_chains_and_repeated_urls() {
        let url = "https://rpc.invalid/private-key";
        let err = anyhow::anyhow!("request to {url} failed").context(format!("RPC {url}"));
        assert_eq!(
            redact_rpc(&format!("{err:#}"), url),
            "RPC <rpc>: request to <rpc> failed"
        );
        assert_eq!(redact_rpc("unrelated error", url), "unrelated error");
        assert_eq!(redact_rpc("error", ""), "error");
    }
}
