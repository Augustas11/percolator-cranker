use super::*;
use std::sync::{Arc, Mutex};

mod exchanges;
pub use exchanges::Exchanges;

/// Replaceable source boundary, with independent public websocket readers.
// Later: paid pyth.dourolabs.app/hermes source, Authorization: Bearer from env.
pub enum PriceSource {
    Exchanges(Exchanges),
    Mock { price: u64, initial: u64, rng: u64, max_bps: u64 },
    Pyth { rpc: RpcClient, feed: [u8; 32], cfg: launch::OracleCfg },
}
impl PriceSource {
    pub fn latest(&mut self) -> Result<(u64, u64)> {
        match self {
            Self::Exchanges(source) => source.latest().map(|r| (r.price, r.publish_time_ms)),
            Self::Pyth { rpc, feed, cfg } => launch::pyth_sol_usd(rpc, feed, cfg),
            Self::Mock { price, initial, rng, max_bps } => {
                *rng = rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = *rng;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                let step = ((z ^ (z >> 31)) % (2 * *max_bps + 1)) as i128 - *max_bps as i128;
                *price = ((*price as i128 * (10_000 + step) / 10_000) as u64)
                    .clamp(*initial * 95 / 100, *initial * 105 / 100);
                Ok((*price, launch::now_ms()))
            }
        }
    }
}

pub struct Shared {
    pub market: Pubkey,
    pub live: bool,
    pub stop: bool,
    pub mark: u64,
    pub landed_ms: Option<u64>,
    pub role: Role,
}

pub fn start(st: &Staging) -> (Arc<Mutex<Shared>>, Arc<Mutex<()>>, std::thread::JoinHandle<()>) {
    let shared = Arc::new(Mutex::new(Shared {
        market: st.market, live: true, stop: false, mark: st.mark,
        landed_ms: None, role: Role::default(),
    }));
    let state = Arc::clone(&shared);
    let lifecycle = Arc::new(Mutex::new(()));
    let flight = Arc::clone(&lifecycle);
    // Read the same commitment at which pushes are observed landed, so the
    // next sequence and slot cannot come from an older confirmed snapshot.
    let rpc = RpcClient::new_with_commitment(st.rpc.url(), CommitmentConfig::processed());
    let mut source = if let Some(source) = &st.exchanges {
        PriceSource::Exchanges(source.clone())
    } else { match st.pyth_feed {
        Some(feed) => PriceSource::Pyth {
            rpc: RpcClient::new_with_commitment(launch::pyth_rpc_url(&st.cfg.oracle, &st.rpc.url()), CommitmentConfig::confirmed()),
            feed, cfg: st.cfg.oracle.clone(),
        },
        None => PriceSource::Mock {
            price: st.feed, initial: st.cfg.market.initial_price, rng: st.rng, max_bps: 0,
        },
    }};
    let cfg = st.cfg.oracle.clone();
    let heartbeat = st.push_heartbeat_slots;
    let payer = st.payer.insecure_clone();
    let oracle = st.oracle_kp.insecure_clone();
    let program_id = st.program_id;
    let keeper_log = st.keeper_log;
    let handle = std::thread::spawn(move || {
        let mut last_slot = 0;
        let mut market = Pubkey::default();
        loop {
            let started = Instant::now();
            // Only lifecycle transitions wait for in-flight pushes; shared
            // mark reads never hold a mutex across RPC calls.
            let in_flight = flight.lock().unwrap();
            let mut s = state.lock().unwrap();
            if s.stop { break; }
            if s.live {
                if market != s.market {
                    market = s.market;
                    last_slot = 0;
                    s.landed_ms = None;
                }
                let last_mark = s.mark;
                let has_landed = s.landed_ms.is_some();
                drop(s);
                let result = (|| -> Result<Option<String>> {
                    let data = rpc.get_account_data(&market)?;
                    let (_, g) = read_market(&data).map_err(|e| anyhow!("{e:?}"))?;
                    if g.mode != MarketModeV16::Live { return Ok(None); }
                    let seq = read_asset_control_sequences(&data, 0).map_err(|e| anyhow!("{e:?}"))?;
                    let slot = rpc.get_slot()?;
                    if let PriceSource::Mock { max_bps, .. } = &mut source {
                        *max_bps = (g.config.max_price_move_bps_per_slot * slot.saturating_sub(last_slot).max(1)).min(50);
                    }
                    let (mark, published) = source.latest()?;
                    let moved = mark.abs_diff(last_mark) as u128 * 10_000 / last_mark as u128;
                    if has_landed && slot.saturating_sub(last_slot) < heartbeat && moved < cfg.push_min_move_bps as u128 {
                        return Ok(None);
                    }
                    let ix = Instruction {
                        program_id,
                        accounts: vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(market, false)],
                        data: P::PushAuthMark {
                            asset_index: 0, market_id: g.assets[0].market_id, now_slot: slot,
                            mark_e6: mark, observation_sequence: seq.oracle_observation + 1,
                            authority_epoch: seq.authority_epoch,
                        }.encode(),
                    };
                    let tx = Transaction::new_signed_with_payer(&[
                        ComputeBudgetInstruction::set_compute_unit_limit(cfg.push_cu_limit),
                        ComputeBudgetInstruction::set_compute_unit_price(cfg.priority_fee_micro_lamports),
                        ComputeBudgetInstruction::request_heap_frame(128 * 1024), ix,
                    ], Some(&payer.pubkey()), &[&payer, &oracle], rpc.get_latest_blockhash()?);
                    let sig = rpc.send_transaction_with_config(&tx, RpcSendTransactionConfig { skip_preflight: true, ..Default::default() })?;
                    let sent = Instant::now();
                    loop {
                        if let Some(status) = rpc.get_signature_status_with_commitment(&sig, CommitmentConfig::processed())? {
                            status?;
                            let landed = launch::now_ms();
                            let lag = landed.saturating_sub(published);
                            let mut s = state.lock().unwrap();
                            s.mark = mark;
                            s.landed_ms = Some(landed);
                            drop(s);
                            last_slot = slot;
                            if keeper_log {
                                println!("{{\"event\":\"mark_lag\",\"ms\":{lag},\"target_ms\":{},\"at_ms\":{landed}}}", cfg.lag_target_ms);
                            }
                            return Ok(Some(format!("ok({lag}ms)")));
                        }
                        if sent.elapsed() > Duration::from_secs(30) { bail!("PushAuthMark {sig} not processed in 30 s"); }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                })();
                let outcome = state.lock().unwrap().role.record(result);
                if outcome != "-" { println!("push={outcome}"); }
            } else {
                drop(s);
            }
            drop(in_flight);
            if let Some(rest) = Duration::from_millis(cfg.push_poll_ms.max(1)).checked_sub(started.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    });
    (shared, lifecycle, handle)
}

/// Public prices only: no RPC, keys, market setup or transaction submission.
pub fn prices(args: &[String]) -> Result<()> {
    if args.len() != 2 { bail!("usage: staging prices <config> <cluster>"); }
    let cfg = launch::load(&args[0])?;
    let cluster = cfg.cluster.get(&args[1]).context("unknown cluster")?;
    if cluster.price_source != "exchanges" { bail!("prices requires price_source = exchanges (no chain access)"); }
    let mut source = Exchanges::new(&cfg.oracle)?;
    loop {
        match source.latest() {
            Ok(r) => println!("{}", serde_json::json!({
                "price_e6": r.price, "publish_time_ms": r.publish_time_ms,
                "age_ms": launch::now_ms().saturating_sub(r.publish_time_ms),
                "venues": r.venues, "spread_bps": r.spread_bps,
            })),
            Err(e) => println!("{}", serde_json::json!({"error": e.to_string()})),
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}
