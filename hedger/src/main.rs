//! SOL-PERP LP hedger. Reads our LP's net SOL on the Percolator market and
//! offsets it on Hyperliquid with the replay's tight policy. Dry-run by
//! default: it logs the orders it would place, priced off the venue's public
//! book. README.md has the go-live steps.
//!
//! hedger [--config hedger.toml] [--minutes N] [--live]
//! hedger cost-check [--config hedger.toml]

mod chain;
mod policy;
mod venue;

use anyhow::{bail, Context, Result};
use policy::{Decision, Guard, Policy};
use serde::Deserialize;
use serde_json::{json, Value};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use venue::{now_ms, Hyperliquid, Venue};

#[derive(Deserialize)]
struct Config {
    chain: ChainCfg,
    venue: VenueCfg,
    policy: Policy,
    safety: Safety,
    run: RunCfg,
}
#[derive(Deserialize)]
struct ChainCfg {
    rpc: String,
    market: String,
    lp_portfolio: String,
    asset: usize,
}
#[derive(Deserialize)]
struct VenueCfg {
    network: String,
    coin: String,
    taker_fee_bps: f64,
    /// IOC limit = worst book level touched, moved this far against us.
    limit_slippage_bps: f64,
}
#[derive(Deserialize)]
struct Safety {
    /// Stop placing orders once the guard's P&L falls below -this (sticky until restart).
    max_loss_usd: f64,
    /// While this file exists no orders are placed.
    kill_file: PathBuf,
    /// Hold when our mark and the venue mid disagree by more (0 = off).
    max_mark_gap_bps: f64,
}
#[derive(Deserialize)]
struct RunCfg {
    tick_ms: u64,
    funding_every_ms: u64,
    metrics_listen: String,
}

fn log(event: &str, mut v: Value) {
    v["ts_ms"] = json!(now_ms());
    v["event"] = json!(event);
    println!("{v}");
}

fn main() {
    if let Err(e) = real_main() {
        log("fatal", json!({"error": format!("{e:#}")}));
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let mut args = std::env::args().skip(1).peekable();
    let cost_check = args.peek().map(String::as_str) == Some("cost-check");
    if cost_check {
        args.next();
    }
    let (mut path, mut minutes, mut live) = (PathBuf::from("hedger.toml"), None::<u64>, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--live" => live = true,
            "--config" => path = args.next().context("--config needs a path")?.into(),
            "--minutes" => minutes = Some(args.next().context("--minutes needs a number")?.parse()?),
            _ => bail!("unknown option {a}"),
        }
    }
    let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?)?;
    if cfg.run.tick_ms == 0 {
        bail!("run.tick_ms must be positive");
    }
    let mut hl = Hyperliquid::connect(&cfg.venue.network, &cfg.venue.coin)?;
    if cost_check {
        return run_cost_check(&hl, cfg.venue.taker_fee_bps);
    }
    if live {
        let (key, account) = credentials()?;
        hl = hl.with_signer(&key, &account)?;
    }
    run(cfg, hl, live, minutes)
}

/// Live credentials: HL_ACCOUNT_ADDRESS plus the API wallet key from
/// HL_SECRET_KEY or HL_SECRET_KEY_FILE (owner-only, outside this repo).
fn credentials() -> Result<(String, String)> {
    let account = std::env::var("HL_ACCOUNT_ADDRESS").context("--live needs HL_ACCOUNT_ADDRESS")?;
    if let Ok(k) = std::env::var("HL_SECRET_KEY") {
        return Ok((k, account));
    }
    let file = PathBuf::from(std::env::var("HL_SECRET_KEY_FILE").context("--live needs HL_SECRET_KEY or HL_SECRET_KEY_FILE")?);
    let real = file.canonicalize().with_context(|| format!("key file {}", file.display()))?;
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").canonicalize()?;
    if real.starts_with(&repo) {
        bail!("key file must live outside the repo ({})", repo.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(&real)?.permissions().mode() & 0o077 != 0 {
            bail!("key file must be owner-only (chmod 600)");
        }
    }
    Ok((std::fs::read_to_string(&real)?, account))
}

/// Walk today's book at the replay's hedge sizes: is 4.5 bps + ~0.5-0.7 bps right?
fn run_cost_check(hl: &Hyperliquid, fee: f64) -> Result<()> {
    let (fine, deep) = (hl.book(false)?, hl.book(true)?);
    let depth = |l: &[(f64, f64)]| l.iter().map(|(p, s)| p * s).sum::<f64>();
    let mut rows = Vec::new();
    for usd in [1e3, 1e4, 1e5, 1e6] {
        // The fine book where it reaches; else the aggregated one (slightly pessimistic).
        let side = |l: &[(f64, f64)]| policy::walk(l, usd / fine.mid).map(|(avg, _)| policy::cost_bps(avg, fine.mid, 0.0));
        let pick = |f: &[(f64, f64)], d: &[(f64, f64)]| side(f).map(|x| (x, "fine")).or(side(d).map(|x| (x, "aggregated")));
        let (buy, sell) = (pick(&fine.asks, &deep.asks), pick(&fine.bids, &deep.bids));
        let avg = buy.zip(sell).map(|(x, y)| (x.0 + y.0) / 2.0);
        rows.push(json!({"usd": usd, "slip_buy_bps": buy.map(|x| x.0), "slip_sell_bps": sell.map(|x| x.0),
            "book": buy.map(|x| x.1), "slip_avg_bps": avg, "all_in_bps": avg.map(|s| s + fee)}));
    }
    log("cost_check", json!({"venue": hl.name(), "mid": fine.mid, "book_time_ms": fine.time_ms,
        "taker_fee_bps": fee, "depth_usd_fine": [depth(&fine.bids), depth(&fine.asks)],
        "depth_usd_aggregated": [depth(&deep.bids), depth(&deep.asks)],
        "funding_hourly_bps": hl.funding_hourly()? * 1e4, "sizes": rows}));
    Ok(())
}

#[derive(Default)]
struct Stats {
    lp_sol: f64,
    hedge_sol: f64,
    mid: f64,
    mark: f64,
    funding_bps: f64,
    last_cost_bps: f64,
    volume_usd: f64,
    cost_usd: f64,
    orders: u64,
    errors: u64,
    chain_ok: bool,
    halted: bool,
    killed: bool,
}

fn run(cfg: Config, hl: Hyperliquid, live: bool, minutes: Option<u64>) -> Result<()> {
    let rpc = RpcClient::new_with_commitment(cfg.chain.rpc.clone(), CommitmentConfig::confirmed());
    let market: Pubkey = cfg.chain.market.parse().context("chain.market")?;
    let lp: Pubkey = cfg.chain.lp_portfolio.parse().context("chain.lp_portfolio")?;
    let metrics = Arc::new(RwLock::new(String::new()));
    serve(&cfg.run.metrics_listen, metrics.clone())?;
    log("start", json!({"mode": if live {"live"} else {"dry_run"}, "venue": hl.name(),
        "network": cfg.venue.network, "agent": hl.agent_address(), "market": market.to_string(),
        "lp_portfolio": lp.to_string(), "policy": format!("{:?}", cfg.policy),
        "max_loss_usd": cfg.safety.max_loss_usd, "kill_file": cfg.safety.kill_file}));
    let (started, mut guard, mut s) = (Instant::now(), Guard::default(), Stats::default());
    let (mut last_order, mut last_funding) = (None::<Instant>, None::<Instant>);
    // Dry-run's simulated hedge; live reads the venue each tick.
    let mut sim_hedge = 0.0;
    loop {
        if minutes.is_some_and(|m| started.elapsed() >= Duration::from_secs(m * 60)) {
            break;
        }
        let tick = Instant::now();
        s.killed = cfg.safety.kill_file.exists();
        match step(&cfg, &rpc, &market, &lp, &hl, live, &mut sim_hedge, &mut guard, &mut s, &mut last_order, &mut last_funding) {
            Ok(()) => {}
            Err(e) => {
                s.errors += 1;
                log("error", json!({"error": format!("{e:#}")}));
            }
        }
        *metrics.write().unwrap() = render(&s, &guard, &market.to_string(), live);
        std::thread::sleep(Duration::from_millis(cfg.run.tick_ms).saturating_sub(tick.elapsed()));
    }
    log("stop", json!({"orders": s.orders, "volume_usd": s.volume_usd, "cost_usd": s.cost_usd,
        "avg_cost_bps": avg_bps(&s), "guard_pnl_usd": guard.pnl_usd, "errors": s.errors}));
    Ok(())
}

fn avg_bps(s: &Stats) -> Option<f64> {
    (s.volume_usd > 0.0).then(|| s.cost_usd / s.volume_usd * 1e4)
}

#[allow(clippy::too_many_arguments)]
fn step(
    cfg: &Config,
    rpc: &RpcClient,
    market: &Pubkey,
    lp: &Pubkey,
    hl: &Hyperliquid,
    live: bool,
    sim_hedge: &mut f64,
    guard: &mut Guard,
    s: &mut Stats,
    last_order: &mut Option<Instant>,
    last_funding: &mut Option<Instant>,
) -> Result<()> {
    s.chain_ok = false;
    let c = chain::read_lp(rpc, market, lp, cfg.chain.asset)?;
    s.chain_ok = true;
    let mut book = hl.book(false)?;
    if last_funding.is_none_or(|t| t.elapsed() >= Duration::from_millis(cfg.run.funding_every_ms)) {
        s.funding_bps = hl.funding_hourly()? * 1e4;
        *last_funding = Some(Instant::now());
    }
    let hedge = if live { hl.position()? } else { *sim_hedge };
    (s.lp_sol, s.hedge_sol, s.mid, s.mark) = (c.net_sol, hedge, book.mid, c.mark_px);
    let residual = c.net_sol + hedge;
    guard.mark(residual, book.mid);
    if guard.breached(cfg.safety.max_loss_usd) && !s.halted {
        s.halted = true;
        log("halt", json!({"reason": "max_loss", "guard_pnl_usd": guard.pnl_usd}));
    }
    let gap_bps = (c.mark_px / book.mid - 1.0).abs() * 1e4;
    let since = last_order.map(|t| t.elapsed().as_millis() as u64);
    let mut d = policy::decide(&cfg.policy, c.net_sol, hedge, book.mid, hl.step(), since);
    if matches!(d, Decision::Order(_)) {
        d = if s.killed {
            Decision::Hold("kill_switch")
        } else if s.halted {
            Decision::Hold("halted")
        } else if !c.live {
            Decision::Hold("market_not_live")
        } else if cfg.safety.max_mark_gap_bps > 0.0 && gap_bps > cfg.safety.max_mark_gap_bps {
            Decision::Hold("mark_gap")
        } else {
            d
        };
    }
    let tick = json!({"slot": c.slot, "lp_sol": c.net_sol, "hedge_sol": hedge, "residual_sol": residual,
        "residual_usd": residual * book.mid, "mid": book.mid, "mark": c.mark_px, "mark_gap_bps": gap_bps,
        "funding_hourly_bps": s.funding_bps, "guard_pnl_usd": guard.pnl_usd});
    let Decision::Order(sol) = d else {
        let Decision::Hold(why) = d else { unreachable!() };
        let mut t = tick;
        t["hold"] = json!(why);
        log("tick", t);
        return Ok(());
    };
    let mut walked = policy::walk(if sol > 0.0 { &book.asks } else { &book.bids }, sol.abs());
    if walked.is_none() {
        // Larger than the top 20 levels: price off the aggregated book (mid unchanged).
        let mid = book.mid;
        book = hl.book(true)?;
        book.mid = mid;
        walked = policy::walk(if sol > 0.0 { &book.asks } else { &book.bids }, sol.abs());
    }
    let Some((avg, worst)) = walked else {
        let mut t = tick;
        t["hold"] = json!("book_too_thin");
        log("tick", t);
        return Ok(());
    };
    let slip = cfg.venue.limit_slippage_bps / 1e4;
    let limit = hl.round_px(if sol > 0.0 { worst * (1.0 + slip) } else { worst * (1.0 - slip) });
    *last_order = Some(Instant::now());
    let (filled, px) = if live {
        let f = hl.ioc(sol, limit)?;
        (f.sol, f.avg_px)
    } else {
        *sim_hedge += sol;
        (sol, avg)
    };
    let notional = filled.abs() * px;
    let cost_bps = if filled != 0.0 { policy::cost_bps(px, book.mid, cfg.venue.taker_fee_bps) } else { 0.0 };
    let cost_usd = notional * cost_bps / 1e4;
    s.orders += 1;
    s.volume_usd += notional;
    s.cost_usd += cost_usd;
    s.last_cost_bps = cost_bps;
    s.hedge_sol = hedge + filled;
    guard.fill(c.net_sol + hedge + filled, cost_usd);
    let mut t = tick;
    t["order"] = json!({"live": live, "would_place": !live, "sol": sol, "limit_px": limit,
        "filled_sol": filled, "avg_px": px, "notional_usd": notional, "cost_bps": cost_bps, "cost_usd": cost_usd,
        "volume_usd_total": s.volume_usd, "avg_cost_bps_total": avg_bps(s)});
    log("order", t);
    Ok(())
}

/// Prometheus text, `percolator_hedger_*` beside the monitor's `percolator_*`.
fn render(s: &Stats, g: &Guard, market: &str, live: bool) -> String {
    let rows: [(&str, &str, f64); 17] = [
        ("lp_net_sol", "gauge", s.lp_sol),
        ("hedge_position_sol", "gauge", s.hedge_sol),
        ("residual_delta_sol", "gauge", s.lp_sol + s.hedge_sol),
        ("residual_delta_usd", "gauge", (s.lp_sol + s.hedge_sol) * s.mid),
        ("venue_mid_usd", "gauge", s.mid),
        ("mark_usd", "gauge", s.mark),
        ("venue_funding_hourly_bps", "gauge", s.funding_bps),
        ("last_hedge_cost_bps", "gauge", s.last_cost_bps),
        ("hedge_cost_bps", "gauge", avg_bps(s).unwrap_or(0.0)),
        ("hedge_volume_usd", "counter", s.volume_usd),
        ("hedge_cost_usd", "counter", s.cost_usd),
        ("orders", "counter", s.orders as f64),
        ("errors", "counter", s.errors as f64),
        ("guard_pnl_usd", "gauge", g.pnl_usd),
        ("chain_read_ok", "gauge", s.chain_ok as u8 as f64),
        ("halted", "gauge", (s.halted || s.killed) as u8 as f64),
        ("live", "gauge", live as u8 as f64),
    ];
    let mut out = String::new();
    for (name, kind, v) in rows {
        out += &format!("# TYPE percolator_hedger_{name} {kind}\npercolator_hedger_{name}{{market=\"{market}\"}} {v}\n");
    }
    out
}

fn serve(addr: &str, body: Arc<RwLock<String>>) -> Result<()> {
    let listener = TcpListener::bind(addr).with_context(|| format!("bind {addr}"))?;
    std::thread::spawn(move || {
        for mut c in listener.incoming().flatten() {
            let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = c.read(&mut [0u8; 1024]);
            let b = body.read().unwrap().clone();
            let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len());
        }
    });
    Ok(())
}
