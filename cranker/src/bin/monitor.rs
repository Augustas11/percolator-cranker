//! Read-only liveness and solvency monitor for one market.
//!
//! ```text
//! monitor --market <pk> --program <pk> [--rpc URL] [--interval-ms N]
//!         [--listen 127.0.0.1:9184] [--status-file PATH]
//!         [--expiry-warn-slots N] [--json]
//! ```
//!
//! Serves `/metrics` (Prometheus), `/snapshot.json` and `/` (dashboard).
//! `--json` prints one snapshot and exits. Never signs anything.
//! See `docs/MONITOR.md`.

use std::net::TcpListener;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use percolator_cranker::monitor::{
    http,
    snapshot::{MonitorConfig, Snapshot},
};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};

const USAGE: &str = "usage: monitor --market <pk> --program <pk> [--rpc URL] [--interval-ms N] \
[--listen 127.0.0.1:9184] [--status-file PATH] [--expiry-warn-slots N] [--json]";

struct Args {
    cfg: MonitorConfig,
    rpc: String,
    interval: Duration,
    listen: String,
    json: bool,
}

fn parse_args() -> Result<Args> {
    let mut market = None;
    let mut program = std::env::var("PERCOLATOR_PROGRAM_ID").ok();
    let mut rpc = "http://127.0.0.1:8899".to_string();
    let mut interval_ms = 5_000u64;
    let mut listen = "127.0.0.1:9184".to_string();
    let mut status_file = std::env::var("CRANKER_STATUS_FILE")
        .ok()
        .filter(|s| !s.is_empty());
    let mut expiry_warn_slots = 9_000u64;
    let mut json = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .with_context(|| format!("{a} needs a value\n{USAGE}"))
        };
        match a.as_str() {
            "--market" => market = Some(val()?),
            "--program" => program = Some(val()?),
            "--rpc" => rpc = val()?,
            "--interval-ms" => interval_ms = val()?.parse().context("--interval-ms")?,
            "--listen" => listen = val()?,
            "--status-file" => status_file = Some(val()?),
            "--expiry-warn-slots" => {
                expiry_warn_slots = val()?.parse().context("--expiry-warn-slots")?
            }
            "--json" => json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => bail!("unknown argument {other}\n{USAGE}"),
        }
    }
    let market = Pubkey::from_str(&market.context(USAGE)?).context("--market")?;
    let program =
        Pubkey::from_str(&program.context("--program (or PERCOLATOR_PROGRAM_ID) is required")?)
            .context("--program")?;
    Ok(Args {
        cfg: MonitorConfig {
            market,
            program,
            expiry_warn_slots,
            status_file: status_file.map(PathBuf::from),
        },
        rpc,
        interval: Duration::from_millis(interval_ms.max(250)),
        listen,
        json,
    })
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let rpc = RpcClient::new_with_commitment(args.rpc.clone(), CommitmentConfig::confirmed());
    if args.json {
        println!("{}", Snapshot::collect(&rpc, &args.cfg).to_json());
        return Ok(());
    }
    let listener =
        TcpListener::bind(&args.listen).with_context(|| format!("bind {}", args.listen))?;
    eprintln!(
        "monitor: market={} program={} rpc={} every {:?}; serving {} (/, /metrics, /snapshot.json)",
        args.cfg.market, args.cfg.program, args.rpc, args.interval, args.listen
    );
    let shared: http::Shared = Arc::new(Mutex::new(None));
    {
        let shared = shared.clone();
        std::thread::spawn(move || http::serve(listener, shared));
    }
    loop {
        let snap = Snapshot::collect(&rpc, &args.cfg);
        if let Err(e) = &snap.chain {
            eprintln!("monitor: chain read failed: {e}");
        }
        *shared.lock().unwrap_or_else(|e| e.into_inner()) = Some(snap);
        std::thread::sleep(args.interval);
    }
}
