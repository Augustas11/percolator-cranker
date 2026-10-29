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
    pub portfolio: Pubkey,
    pub live: bool,
    pub stop: bool,
    pub mark: u64,
    pub landed_ms: Option<u64>,
    pub published_ms: Option<u64>,
    pub role: Role,
    pub accrual: Role,
}

pub fn start(st: &Staging) -> Result<(Arc<Mutex<Shared>>, Arc<Mutex<()>>, std::thread::JoinHandle<()>)> {
    let shared = Arc::new(Mutex::new(Shared {
        market: st.market, portfolio: st.lp_pf, live: true, stop: false, mark: st.mark,
        landed_ms: None, published_ms: None, role: Role::default(), accrual: Role::default(),
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
    let pyth = st.pyth.clone();
    let heartbeat = st.push_heartbeat_slots;
    let payer = st.payer.insecure_clone();
    let oracle = st.oracle_kp.insecure_clone();
    let program_id = st.program_id;
    let keeper_log = st.keeper_log;
    let matcher_program = Pubkey::from_str(&st.cfg.matcher.program_id).context("matcher.program_id")?;
    let matcher_ctxs = st.matcher_ctxs.iter().map(|ctx| Pubkey::from_str(ctx).context("matcher_ctxs")).collect::<Result<Vec<_>>>()?;
    let handle = std::thread::spawn(move || {
        let mut last_slot = 0;
        let mut market = Pubkey::default();
        let mut read_error = None;
        loop {
            let started = Instant::now();
            // Lifecycle transitions and trades wait for in-flight pushes;
            // shared mark reads never hold a mutex across RPC calls.
            let in_flight = flight.lock().unwrap();
            let mut s = state.lock().unwrap();
            if s.stop { break; }
            if s.live {
                if market != s.market {
                    market = s.market;
                    last_slot = 0;
                    s.landed_ms = None;
                    s.published_ms = None;
                }
                let portfolio = s.portfolio;
                let last_mark = s.mark;
                let has_landed = s.landed_ms.is_some();
                drop(s);
                let mut send_attempted = false;
                let mut accrual_snapshot = None;
                let result = (|| -> Result<Option<String>> {
                    let data = rpc.get_account_data(&market)?;
                    let (_, g) = read_market(&data).map_err(|e| anyhow!("{e:?}"))?;
                    if g.mode != MarketModeV16::Live { return Ok(None); }
                    let seq = read_asset_control_sequences(&data, 0).map_err(|e| anyhow!("{e:?}"))?;
                    let slot = rpc.get_slot()?;
                    accrual_snapshot = Some((data.clone(), slot, g.assets[0].slot_last));
                    if let PriceSource::Mock { max_bps, .. } = &mut source {
                        *max_bps = (g.config.max_price_move_bps_per_slot * slot.saturating_sub(last_slot).max(1)).min(50);
                    }
                    let (mark, published) = source.latest()?;
                    let moved = mark.abs_diff(last_mark) as u128 * 10_000 / last_mark as u128;
                    if has_landed && slot.saturating_sub(last_slot) < heartbeat && moved < cfg.push_min_move_bps as u128 {
                        log_read_state(&mut read_error, None);
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
                    let mut ixs = vec![
                        ComputeBudgetInstruction::set_compute_unit_limit(cfg.push_cu_limit),
                        ComputeBudgetInstruction::set_compute_unit_price(cfg.priority_fee_micro_lamports),
                        ComputeBudgetInstruction::request_heap_frame(128 * 1024), ix,
                    ];
                    ixs.extend(matcher_ctxs.iter().map(|ctx| update_ref(matcher_program, oracle.pubkey(), *ctx, mark)));
                    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[&payer, &oracle], rpc.get_latest_blockhash()?);
                    if bincode::serialized_size(&tx)? > 1232 { bail!("push transaction exceeds 1232 bytes"); }
                    log_read_state(&mut read_error, None);
                    send_attempted = true;
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
                            s.published_ms = Some(published);
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
                // Reads/build preparation are not PushAuthMark send attempts.
                let result = match result {
                    Err(e) if !send_attempted => {
                        log_read_state(&mut read_error, Some(short(&e)));
                        Ok(None)
                    },
                    other => other,
                };
                let outcome = state.lock().unwrap().role.record(result);
                if outcome != "-" { println!("push={outcome}"); }
                // Separate transaction: a failed crank must never revert a mark.
                // The lifecycle lock still covers this send and confirmation.
                let accrual = (|| -> Result<Option<String>> {
                    let Some((data, slot, slot_last)) = accrual_snapshot else { return Ok(None); };
                    let cranks = accrual_crank_count(slot.saturating_sub(slot_last), cfg.accrual_lag_slots, cfg.accrual_max_cranks_per_tx);
                    if cranks == 0 || portfolio == Pubkey::default() { return Ok(None); }
                    let obs = observation::fetch_observation_for_asset(&data, 0, &pyth)?;
                    let route = CrankRoute::Live { reward_portfolio: None };
                    let (mut built, index) = submit::build_crank_instructions(
                        &program_id, &payer.pubkey(), &market, &portfolio, &route, &[obs], slot,
                    );
                    let crank = built.swap_remove(index);
                    let mut ixs = vec![
                        ComputeBudgetInstruction::set_compute_unit_limit(cranks.saturating_mul(125_000).min(1_400_000) as u32),
                        ComputeBudgetInstruction::request_heap_frame(128 * 1024),
                    ];
                    for _ in 0..cranks { ixs.push(crank.clone()); }
                    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[&payer], rpc.get_latest_blockhash()?);
                    rpc.send_and_confirm_transaction(&tx)?;
                    Ok(Some(format!("ok({cranks} cranks)")))
                })();
                let outcome = state.lock().unwrap().accrual.record(accrual);
                if outcome != "-" { println!("accrual={outcome}"); }

            } else {
                drop(s);
            }
            drop(in_flight);
            // Even an overrun yields the lifecycle mutex to trades/shutdown.
            std::thread::sleep(Duration::from_millis(cfg.push_poll_ms.max(1)).saturating_sub(started.elapsed()).max(Duration::from_millis(5)));
        }
    });
    Ok((shared, lifecycle, handle))
}

/// The engine advances at most 32 slots per Live crank.
fn accrual_crank_count(gap: u64, lag_slots: u64, max_cranks: u64) -> u64 {
    if gap <= lag_slots { return 0; }
    (gap / 32 + u64::from(gap % 32 != 0)).min(max_cranks)
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

// Log source/RPC availability transitions, without counting them as failed sends.
fn log_read_state(previous: &mut Option<String>, error: Option<String>) {
    if *previous != error {
        eprintln!("push read: {}", error.as_deref().unwrap_or("recovered"));
        *previous = error;
    }
}

fn update_ref(program_id: Pubkey, authority: Pubkey, ctx: Pubkey, mark_e6: u64) -> Instruction {
    let mut data = vec![4];
    data.extend_from_slice(&mark_e6.to_le_bytes());
    Instruction {
        program_id,
        accounts: vec![AccountMeta::new_readonly(authority, true), AccountMeta::new(ctx, false)],
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accrual_threshold_and_crank_count() {
        let cfg = launch::load(DEFAULT_CONFIG).unwrap();
        assert_eq!(cfg.oracle.accrual_lag_slots, 32);
        assert_eq!(cfg.oracle.accrual_max_cranks_per_tx, 10);
        for gap in [0, 1, 31, 32] { assert_eq!(accrual_crank_count(gap, 32, 10), 0); }
        for (gap, expected) in [(33, 2), (64, 2), (65, 3), (320, 10), (321, 10), (u64::MAX, 10)] {
            assert_eq!(accrual_crank_count(gap, 32, 10), expected);
        }
        assert_eq!(accrual_crank_count(64, 64, 10), 0);
        assert_eq!(accrual_crank_count(65, 64, 10), 3);
        assert_eq!(accrual_crank_count(65, 32, 1), 1);
        assert_eq!(accrual_crank_count(65, 32, 0), 0);
    }

    #[test]
    fn matcher_update_ref_wire_and_transaction_size() {
        let payer = Keypair::new();
        let oracle = Keypair::new();
        let cfg = launch::load(DEFAULT_CONFIG).unwrap();
        let ctx = Pubkey::from_str(&cfg.cluster["devnet"].matcher_ctxs[0]).unwrap();
        let mark = 150_000_000u64;
        let ix = update_ref(Pubkey::from_str(&cfg.matcher.program_id).unwrap(), oracle.pubkey(), ctx, mark);
        assert_eq!(ix.program_id.to_string(), "8era4GFKLUu24TLr1H2Gjqt5csiYj4jhPwoxCXfEDvKE");
        assert_eq!(ix.data, [vec![4], mark.to_le_bytes().to_vec()].concat());
        assert_eq!(ix.accounts, vec![AccountMeta::new_readonly(oracle.pubkey(), true), AccountMeta::new(ctx, false)]);
        let push = Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(Pubkey::new_unique(), false)],
            data: P::PushAuthMark { asset_index: 0, market_id: 1, now_slot: 1, mark_e6: mark, observation_sequence: 1, authority_epoch: 1 }.encode(),
        };
        let tx = Transaction::new_signed_with_payer(&[
            ComputeBudgetInstruction::set_compute_unit_limit(cfg.oracle.push_cu_limit),
            ComputeBudgetInstruction::set_compute_unit_price(cfg.oracle.priority_fee_micro_lamports),
            ComputeBudgetInstruction::request_heap_frame(128 * 1024), push, ix,
        ], Some(&payer.pubkey()), &[&payer, &oracle], solana_sdk::hash::Hash::default());
        let bytes = bincode::serialized_size(&tx).unwrap();
        assert!(bytes <= 1232, "{bytes} bytes");
        println!("PushAuthMark + UpdateRef: {bytes} bytes, {} signatures", tx.signatures.len());
        assert_eq!(tx.signatures.len(), 2);
        assert!(cfg.cluster["local"].matcher_ctxs.is_empty());
        assert!(cfg.cluster["mainnet"].matcher_ctxs.is_empty());
    }
}
