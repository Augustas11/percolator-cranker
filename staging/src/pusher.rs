use super::*;
use std::sync::{Arc, Mutex};

mod exchanges;
pub use exchanges::Exchanges;

// Public RPCs rate-limit per method (429, Retry-After 10 s), and the client sleeps on it.
const CONFIRM_POLL: Duration = Duration::from_millis(200);

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
    let configured_ctxs = st.matcher_ctxs.iter().map(|ctx| Pubkey::from_str(ctx).context("matcher_ctxs")).collect::<Result<Vec<_>>>()?;
    let mut matcher_ctxs = Vec::new();
    for ctx in configured_ctxs {
        match rpc.get_account_data(&ctx) {
            Ok(data) if ctx_authority_matches(&data, &oracle.pubkey()) => matcher_ctxs.push(ctx),
            Ok(_) => eprintln!("push: dropping matcher ctx {ctx}: price_authority does not match {} (or invalid layout)", oracle.pubkey()),
            Err(e) => eprintln!("push: dropping matcher ctx {ctx}: cannot read account: {}", short(&e.into())),
        }
    }
    // Values do not affect wire size; validate the exact instruction shape before spawning.
    let push = Instruction {
        program_id,
        accounts: vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(st.market, false)],
        data: P::PushAuthMark { asset_index: 0, market_id: 0, now_slot: 0, mark_e6: 0,
            observation_sequence: 0, authority_epoch: 0 }.encode(),
    };
    let startup_tx = Transaction::new_signed_with_payer(
        &push_instructions(&cfg, push, matcher_program, oracle.pubkey(), &matcher_ctxs, 0),
        Some(&payer.pubkey()), &[&payer, &oracle], solana_sdk::hash::Hash::default(),
    );
    if bincode::serialized_size(&startup_tx)? > 1232 { bail!("push transaction exceeds 1232 bytes at startup"); }
    // Bound individual accrual RPC requests as well as the shared polling deadline.
    let accrual_rpc = RpcClient::new_with_timeout_and_commitment(
        st.rpc.url(), Duration::from_millis(500), CommitmentConfig::processed(),
    );
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
                    let ixs = push_instructions(&cfg, ix, matcher_program, oracle.pubkey(), &matcher_ctxs, mark);
                    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[&payer, &oracle], rpc.get_latest_blockhash()?);
                    if bincode::serialized_size(&tx)? > 1232 { bail!("push transaction exceeds 1232 bytes"); }
                    log_read_state(&mut read_error, None);
                    send_attempted = true;
                    let src_age_ms = launch::now_ms().saturating_sub(published);
                    let send_started = Instant::now();
                    let sig = rpc.send_transaction_with_config(&tx, RpcSendTransactionConfig { skip_preflight: true, ..Default::default() })?;
                    let sent = Instant::now();
                    let send_ms = sent.duration_since(send_started).as_millis();
                    loop {
                        if let Some(status) = rpc.get_signature_status_with_commitment(&sig, CommitmentConfig::processed())? {
                            status?;
                            let confirm_ms = sent.elapsed().as_millis();
                            let landed = launch::now_ms();
                            let lag = landed.saturating_sub(published);
                            let mut s = state.lock().unwrap();
                            s.mark = mark;
                            s.landed_ms = Some(landed);
                            s.published_ms = Some(published);
                            drop(s);
                            last_slot = slot;
                            if keeper_log {
                                println!("{{\"event\":\"mark_lag\",\"ms\":{lag},\"src_age_ms\":{src_age_ms},\"send_ms\":{send_ms},\"confirm_ms\":{confirm_ms},\"target_ms\":{},\"at_ms\":{landed}}}", cfg.lag_target_ms);
                            }
                            return Ok(Some(format!("ok({lag}ms)")));
                        }
                        if sent.elapsed() > Duration::from_secs(30) { bail!("PushAuthMark {sig} not processed in 30 s"); }
                        std::thread::sleep(CONFIRM_POLL);
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
                let client = Arc::clone(accrual_rpc.get_inner_client());
                let accrual_payer = payer.insecure_clone();
                let accrual_cfg = cfg.clone();
                let accrual_pyth = pyth.clone();
                let accrual = run_bounded(&accrual_rpc, Duration::from_secs(3), async move {
                    let payer = accrual_payer;
                    let cfg = accrual_cfg;
                    let pyth = accrual_pyth;
                    let deadline = Instant::now() + Duration::from_secs(3);
                    let data = client.get_account_data(&market).await?;
                    let (_, g) = read_market(&data).map_err(|e| anyhow!("{e:?}"))?;
                    if g.mode != MarketModeV16::Live { return Ok(None); }
                    let slot = client.get_slot().await?;
                    let mut cranks = accrual_crank_count(slot.saturating_sub(g.assets[0].slot_last), cfg.accrual_lag_slots, cfg.accrual_max_cranks_per_tx);
                    if cranks == 0 || portfolio == Pubkey::default() { return Ok(None); }
                    let obs = observation::fetch_observation_for_asset(&data, 0, &pyth)?;
                    let route = CrankRoute::Live { reward_portfolio: None };
                    let (mut built, index) = submit::build_crank_instructions(
                        &program_id, &payer.pubkey(), &market, &portfolio, &route, &[obs], slot, false,
                    );
                    let crank = built.swap_remove(index);
                    for attempt in 0..2 {
                        let mut ixs = vec![
                            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
                            ComputeBudgetInstruction::set_compute_unit_price(cfg.priority_fee_micro_lamports),
                            ComputeBudgetInstruction::request_heap_frame(128 * 1024),
                        ];
                        for _ in 0..cranks { ixs.push(crank.clone()); }
                        if Instant::now() >= deadline { bail!("accrual deadline before send"); }
                        let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[&payer], client.get_latest_blockhash().await?);
                        let result = send_accrual(&client, &tx, deadline).await;
                        match result {
                            Err(e) if attempt == 0 && submit::wrapper_error_name(&e.to_string()) == Some("EngineNonProgress") => {
                                cranks = retry_crank_count(cranks);
                            }
                            other => { other?; break; }
                        }
                    }
                    Ok(Some(format!("ok({cranks} cranks)")))
                });
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

// Cancel the future itself, including the RPC client's HTTP 429 retry sleeps.
fn run_bounded<T: Send + 'static>(rpc: &RpcClient, timeout: Duration,
    future: impl std::future::Future<Output = Result<T>> + Send + 'static) -> Result<T> {
    let task = rpc.runtime().spawn(future);
    let abort = task.abort_handle();
    let (done, finished) = std::sync::mpsc::channel();
    let watchdog = std::thread::spawn(move || {
        if finished.recv_timeout(timeout).is_err() { abort.abort(); }
    });
    let result = rpc.runtime().block_on(task);
    let _ = done.send(());
    let _ = watchdog.join();
    result.map_err(|e| anyhow!("accrual deadline/cancellation: {e}"))?
}

fn ctx_authority_matches(data: &[u8], authority: &Pubkey) -> bool {
    data.get(236..268) == Some(authority.as_ref())
}

fn retry_crank_count(count: u64) -> u64 { (count / 2).max(1) }

fn push_instructions(cfg: &launch::OracleCfg, push: Instruction, matcher: Pubkey, authority: Pubkey, ctxs: &[Pubkey], mark: u64) -> Vec<Instruction> {
    let mut ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(cfg.push_cu_limit),
        ComputeBudgetInstruction::set_compute_unit_price(cfg.priority_fee_micro_lamports),
        ComputeBudgetInstruction::request_heap_frame(128 * 1024), push,
    ];
    ixs.extend(ctxs.iter().map(|ctx| update_ref(matcher, authority, *ctx, mark)));
    ixs
}

async fn send_accrual(rpc: &solana_client::nonblocking::rpc_client::RpcClient, tx: &Transaction, deadline: Instant) -> Result<()> {
    let sig = rpc.send_transaction_with_config(tx, RpcSendTransactionConfig { skip_preflight: true, ..Default::default() }).await?;
    while Instant::now() < deadline {
        if let Some(status) = rpc.get_signature_status_with_commitment(&sig, CommitmentConfig::processed()).await? {
            status?;
            return Ok(());
        }
        std::thread::sleep(CONFIRM_POLL);
    }
    bail!("accrual {sig} not processed before deadline")
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
    fn accrual_cancels_stalled_rpc() {
        let rpc = RpcClient::new("http://127.0.0.1:1".to_string());
        let started = Instant::now();
        let result = run_bounded::<()>(&rpc, Duration::from_millis(30), std::future::pending());
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(1));

        // A request timeout alone does not bound Solana's Retry-After sleep.
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let rpc = RpcClient::new(format!("http://{}", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).unwrap();
            socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 119\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let client = Arc::clone(rpc.get_inner_client());
        let started = Instant::now();
        let result = run_bounded(&rpc, Duration::from_millis(250), async move {
            Ok(client.get_slot().await?)
        });
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        server.join().unwrap();
    }

    #[test]
    fn refreshed_accrual_count_and_retry() {
        let slot = 200u64;
        assert_eq!(accrual_crank_count(slot.saturating_sub(100), 32, 10), 4);
        assert_eq!(accrual_crank_count(slot.saturating_sub(180), 32, 10), 0);
        for (count, retry) in [(10, 5), (5, 2), (2, 1), (1, 1)] {
            assert_eq!(retry_crank_count(count), retry);
        }
    }

    #[test]
    fn ctx_authority_filter() {
        let authority = Pubkey::new_unique();
        let mut data = vec![0; 268];
        data[236..268].copy_from_slice(authority.as_ref());
        assert!(ctx_authority_matches(&data, &authority));
        assert!(!ctx_authority_matches(&data, &Pubkey::new_unique()));
        assert!(!ctx_authority_matches(&data[..267], &authority));
        assert!(!ctx_authority_matches(&[], &authority));
    }

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
        let matcher = Pubkey::from_str(&cfg.matcher.program_id).unwrap();
        let oversized = push_instructions(&cfg.oracle, push.clone(), matcher, oracle.pubkey(),
            &(0..40).map(|_| Pubkey::new_unique()).collect::<Vec<_>>(), mark);
        let oversized_tx = Transaction::new_signed_with_payer(&oversized,
            Some(&payer.pubkey()), &[&payer, &oracle], solana_sdk::hash::Hash::default());
        assert!(bincode::serialized_size(&oversized_tx).unwrap() > 1232);
        let tx = Transaction::new_signed_with_payer(
            &push_instructions(&cfg.oracle, push, matcher, oracle.pubkey(), &[ctx], mark),
            Some(&payer.pubkey()), &[&payer, &oracle], solana_sdk::hash::Hash::default());
        let bytes = bincode::serialized_size(&tx).unwrap();
        assert!(bytes <= 1232, "{bytes} bytes");
        println!("PushAuthMark + UpdateRef: {bytes} bytes, {} signatures", tx.signatures.len());
        assert_eq!(tx.signatures.len(), 2);
        assert!(cfg.cluster["local"].matcher_ctxs.is_empty());
        assert!(cfg.cluster["mainnet"].matcher_ctxs.is_empty());
    }
}
