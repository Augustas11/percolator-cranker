use super::*;
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

mod exchanges;
mod flop;
mod or_record;
pub use or_record::{or_record, or_record_status};
pub use exchanges::Exchanges;
pub use flop::FlopIndex;
pub use flop::verify_log as verify_push_log;

const STALL_WINDOW_MS: u64 = 3_600_000;

#[derive(Default)]
struct StallStats {
    first: Option<u64>,
    last: Option<u64>,
    gaps: VecDeque<(u64, u64)>,
}

impl StallStats {
    // Polling start counts as the last good read, so an outage from startup is an open gap.
    fn new(now: u64) -> Self { Self { first: Some(now), last: Some(now), gaps: VecDeque::new() } }

    fn record_ok(&mut self, now: u64) -> Option<u64> {
        self.first.get_or_insert(now);
        let gap = self.last.replace(now).map(|last| now.saturating_sub(last));
        if let Some(ms) = gap { self.gaps.push_back((now, ms)); }
        while self.gaps.front().is_some_and(|(at, _)| *at <= now.saturating_sub(STALL_WINDOW_MS)) {
            self.gaps.pop_front();
        }
        gap.filter(|ms| *ms >= 2000)
    }

    fn window_ms(&self, now: u64) -> u64 { now.saturating_sub(self.first.unwrap_or(now)).min(STALL_WINDOW_MS) }

    fn summary(&self, now: u64) -> (u64, f64) {
        let window = self.window_ms(now);
        let start = now.saturating_sub(window);
        let open = self.last.map(|last| (now, now.saturating_sub(last)));
        let mut longest = 0;
        let mut over = 0;
        for &(at, ms) in self.gaps.iter().chain(open.iter()) {
            let clipped = at.min(now).saturating_sub(at.saturating_sub(ms).max(start));
            longest = longest.max(clipped);
            if ms > 10_000 { over += clipped; }
        }
        (longest, 100.0 * over as f64 / window.max(600_000) as f64)
    }

    fn print_summary(&self, now: u64) {
        let (longest, over) = self.summary(now);
        println!("{{\"event\":\"price_stall_stats\",\"window_s\":{},\"longest_ms\":{longest},\"over_10s_pct\":{over:.2},\"at_ms\":{now}}}", self.window_ms(now) / 1000);
    }
}

/// Replaceable source boundary, with independent public websocket readers.
// Later: paid pyth.dourolabs.app/hermes source, Authorization: Bearer from env.
pub enum PriceSource {
    Exchanges(Exchanges),
    Flop(FlopIndex),
    Mock { price: u64, initial: u64, rng: u64, max_bps: u64 },
    Pyth { rpc: RpcClient, feed: [u8; 32], cfg: launch::OracleCfg },
}
impl PriceSource {
    pub fn latest(&mut self) -> Result<(u64, u64, bool)> {
        match self {
            Self::Exchanges(source) => source.latest().map(|r| (r.price, r.publish_time_ms, r.fallback)),
            Self::Flop(source) => source.latest().map(|(price, at)| (price, at, false)),
            Self::Pyth { rpc, feed, cfg } => launch::pyth_sol_usd(rpc, feed, cfg).map(|(price, at)| (price, at, false)),
            Self::Mock { price, initial, rng, max_bps } => {
                *rng = rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = *rng;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                let step = ((z ^ (z >> 31)) % (2 * *max_bps + 1)) as i128 - *max_bps as i128;
                *price = ((*price as i128 * (10_000 + step) / 10_000) as u64)
                    .clamp(*initial * 95 / 100, *initial * 105 / 100);
                Ok((*price, launch::now_ms(), false))
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
    } else if let Some(source) = &st.flop {
        PriceSource::Flop(source.clone())
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
        let blockhash = Arc::new(Mutex::new(None::<(solana_sdk::hash::Hash, Instant)>));
        let mut last_landed = None::<Instant>;
        let mut last_accrual_read = None::<Instant>;
        let mut max_move_bps_per_slot = 0u64;
        let mut last_slot = 0;
        let mut market = Pubkey::default();
        let mut read_error = None;
        let mut stalls = StallStats::new(launch::now_ms());
        let mut stats_at = launch::now_ms();
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
                    stalls = StallStats::new(launch::now_ms());
                    stats_at = launch::now_ms();
                    last_slot = 0;
                    last_landed = None;
                    last_accrual_read = None;
                    s.landed_ms = None;
                    s.published_ms = None;
                }
                let portfolio = s.portfolio;
                let last_mark = s.mark;
                let has_landed = s.landed_ms.is_some();
                drop(s);
                let mut send_attempted = false;
                let mut market_snapshot = None;
                let result = (|| -> Result<Option<String>> {
                    let elapsed = last_landed.map_or(Duration::ZERO, |at| at.elapsed());
                    if let PriceSource::Mock { max_bps, .. } = &mut source {
                        let gap = (elapsed.as_millis() / 400).max(1).min(u64::MAX as u128) as u64;
                        *max_bps = max_move_bps_per_slot.saturating_mul(gap).min(50);
                    }
                    let (mark, published, fallback) = source.latest()?;
                    let now = launch::now_ms();
                    if let Some(gap) = stalls.record_ok(now) {
                        if keeper_log { println!("{{\"event\":\"price_gap\",\"ms\":{gap},\"at_ms\":{now}}}"); }
                    }
                    let moved = if last_mark == 0 { u128::MAX }
                        else { mark.abs_diff(last_mark) as u128 * 10_000 / last_mark as u128 };
                    let heartbeat_due = elapsed >= Duration::from_millis(heartbeat.saturating_mul(400));
                    if has_landed && !heartbeat_due && moved < cfg.push_min_move_bps as u128 {
                        log_read_state(&mut read_error, None);
                        return Ok(None);
                    }
                    let response = rpc.get_account_with_commitment(&market, CommitmentConfig::processed())?;
                    let slot = response.context.slot;
                    let data = response.value.context("market missing")?.data;
                    market_snapshot = Some((data.clone(), slot, Instant::now()));
                    last_accrual_read = Some(Instant::now());
                    let (_, g) = read_market(&data).map_err(|e| anyhow!("{e:?}"))?;
                    if g.mode != MarketModeV16::Live { return Ok(None); }
                    max_move_bps_per_slot = g.config.max_price_move_bps_per_slot;
                    let seq = read_asset_control_sequences(&data, 0).map_err(|e| anyhow!("{e:?}"))?;
                    if has_landed && !heartbeat_due && slot.saturating_sub(last_slot) < heartbeat
                        && moved < cfg.push_min_move_bps as u128 { return Ok(None); }
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
                    let cached = *blockhash.lock().unwrap();
                    let hash = match cached.filter(|(_, at)| at.elapsed() < Duration::from_secs(20)) {
                        Some((hash, _)) => hash,
                        None => {
                            let hash = rpc.get_latest_blockhash()?;
                            *blockhash.lock().unwrap() = Some((hash, Instant::now()));
                            hash
                        }
                    };
                    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[&payer, &oracle], hash);
                    if bincode::serialized_size(&tx)? > 1232 { bail!("push transaction exceeds 1232 bytes"); }
                    log_read_state(&mut read_error, None);
                    send_attempted = true;
                    let src_age_ms = launch::now_ms().saturating_sub(published);
                    let send_started = Instant::now();
                    let sig = rpc.send_transaction_with_config(&tx, RpcSendTransactionConfig { skip_preflight: true, ..Default::default() })
                        .inspect_err(|e| {
                            if e.to_string().contains("Blockhash not found") { *blockhash.lock().unwrap() = None; }
                        })?;
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
                            last_landed = Some(sent);
                            // Checkable pusher: what was pushed and from what, signed
                            // by the oracle authority (`staging verify-push-log`).
                            if let PriceSource::Flop(f) = &source {
                                if let Some(inputs) = f.last_inputs() {
                                    let rec = serde_json::json!({
                                        "v": 1, "market": market.to_string(), "slot": slot, "sig": sig.to_string(),
                                        "mark_e6": mark, "published_ms": published, "landed_ms": landed, "inputs": inputs,
                                    });
                                    match flop::signed_push(&oracle, rec) {
                                        Ok(line) => println!("{line}"),
                                        Err(e) => eprintln!("flop push log: {e:#}"),
                                    }
                                }
                            }
                            if keeper_log {
                                println!("{{\"event\":\"mark_lag\",\"ms\":{lag},\"src_age_ms\":{src_age_ms},\"send_ms\":{send_ms},\"confirm_ms\":{confirm_ms},\"target_ms\":{},\"at_ms\":{landed}{}}}", cfg.lag_target_ms, if fallback { ",\"fallback\":true" } else { "" });
                            }
                            return Ok(Some(format!("ok({lag}ms)")));
                        }
                        if sent.elapsed() > Duration::from_millis(cfg.push_confirm_timeout_ms) {
                            // Dropped; the next round re-reads the sequence, so at most one lands.
                            *blockhash.lock().unwrap() = None;
                            bail!("PushAuthMark {sig} not processed in {} ms", cfg.push_confirm_timeout_ms);
                        }
                        std::thread::sleep(Duration::from_millis(cfg.confirm_poll_ms.max(1)));
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
                let accrual_due = market_snapshot.is_some() || last_accrual_read.is_none_or(
                    |at| at.elapsed() >= Duration::from_millis(cfg.accrual_lag_slots.saturating_mul(400) / 2));
                // Another sender holds the gate (its cranks accrue too): skip
                // this round rather than stall pushes behind it.
                let mut gate = match submit::SEND_GATE.try_lock() {
                    Ok(g) => Some(g),
                    Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
                    Err(std::sync::TryLockError::WouldBlock) => None,
                };
                let accrual_due = accrual_due && gate.as_mut().is_some_and(|g| g.try_settle(&rpc));
                if accrual_due {
                    let g = gate.as_mut().expect("gate held");
                    // A read from before another sender's last send could
                    // size too many cranks; the extra one would revert.
                    let market_snapshot = market_snapshot.filter(|(_, _, read_at)| g.last_send.is_none_or(|at| *read_at > at));
                    let sent = Arc::new(Mutex::new(None::<Transaction>));
                    let accrual_sent = Arc::clone(&sent);
                    last_accrual_read = Some(Instant::now());
                    let client = Arc::clone(accrual_rpc.get_inner_client());
                    let accrual_payer = payer.insecure_clone();
                    let accrual_cfg = cfg.clone();
                    let accrual_pyth = pyth.clone();
                    let accrual_blockhash = Arc::clone(&blockhash);
                    let cached_hash = *blockhash.lock().unwrap();
                    let accrual = run_bounded(&accrual_rpc, Duration::from_secs(3), async move {
                        let payer = accrual_payer;
                        let cfg = accrual_cfg;
                        let pyth = accrual_pyth;
                        let deadline = Instant::now() + Duration::from_secs(3);
                        let (data, slot) = match market_snapshot {
                            Some((data, slot, read_at)) if read_at.elapsed() < Duration::from_secs(1) => (data, slot),
                            _ => {
                                let response = client.get_account_with_commitment(&market, CommitmentConfig::processed()).await?;
                                (response.value.context("market missing")?.data, response.context.slot)
                            }
                        };
                        let (_, g) = read_market(&data).map_err(|e| anyhow!("{e:?}"))?;
                        if g.mode != MarketModeV16::Live { return Ok(None); }
                        let mut cranks = accrual_crank_count(slot.saturating_sub(g.assets[0].slot_last), cfg.accrual_lag_slots, cfg.accrual_max_cranks_per_tx);
                        if cranks == 0 || portfolio == Pubkey::default() { return Ok(None); }
                        let obs = observation::fetch_observation_for_asset(&data, 0, &pyth)?;
                        let route = CrankRoute::Live { reward_portfolio: None };
                        let (mut built, index) = submit::build_crank_instructions(
                            &program_id, &payer.pubkey(), &market, &portfolio, &route, &[obs], slot, false,
                        );
                        let crank = built.swap_remove(index);
                        let hash = match cached_hash.filter(|(_, at)| at.elapsed() < Duration::from_secs(20)) {
                            Some((hash, _)) => hash,
                            None => {
                                let hash = client.get_latest_blockhash().await?;
                                *accrual_blockhash.lock().unwrap() = Some((hash, Instant::now()));
                                hash
                            }
                        };
                        for attempt in 0..2 {
                            let mut ixs = vec![
                                ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
                                ComputeBudgetInstruction::set_compute_unit_price(cfg.priority_fee_micro_lamports),
                                ComputeBudgetInstruction::request_heap_frame(128 * 1024),
                            ];
                            for _ in 0..cranks { ixs.push(crank.clone()); }
                            if Instant::now() >= deadline { bail!("accrual deadline before send"); }
                            let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[&payer], hash);
                            let result = send_accrual(&client, &tx, deadline, cfg.confirm_poll_ms, &accrual_sent).await;
                            if result.as_ref().err().is_some_and(|e| e.to_string().contains("Blockhash not found")) {
                                *accrual_blockhash.lock().unwrap() = None;
                            }
                            match result {
                                Err(e) if attempt == 0 && submit::wrapper_error_name(&e.to_string()) == Some("EngineNonProgress") => {
                                    cranks = retry_crank_count(cranks);
                                }
                                other => { other?; break; }
                            }
                        }
                        Ok(Some(format!("ok({cranks} cranks)")))
                    });
                    g.last_send = Some(Instant::now());
                    // Cut short after sending: it may still land, so the
                    // next holder waits for it first.
                    if accrual.is_err() {
                        if let Some(tx) = sent.lock().unwrap().take() { g.set_pending(&tx); }
                    }
                    let outcome = state.lock().unwrap().accrual.record(accrual);
                    if outcome != "-" { println!("accrual={outcome}"); }
                }
                drop(gate);

                let now = launch::now_ms();
                if now.saturating_sub(stats_at) >= 60_000 {
                    if keeper_log { stalls.print_summary(now); }
                    stats_at = now;
                }
            } else {
                stalls = StallStats::new(launch::now_ms());
                stats_at = launch::now_ms();
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

async fn send_accrual(rpc: &solana_client::nonblocking::rpc_client::RpcClient, tx: &Transaction, deadline: Instant, confirm_poll_ms: u64, sent: &Mutex<Option<Transaction>>) -> Result<()> {
    let sig = rpc.send_transaction_with_config(tx, RpcSendTransactionConfig { skip_preflight: true, ..Default::default() }).await?;
    // Only once the RPC took it: it can land from here on.
    *sent.lock().unwrap() = Some(tx.clone());
    while Instant::now() < deadline {
        if let Some(status) = rpc.get_signature_status_with_commitment(&sig, CommitmentConfig::processed()).await? {
            status?;
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(confirm_poll_ms.max(1))
            .min(deadline.saturating_duration_since(Instant::now())));
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
    let mut stalls = StallStats::new(launch::now_ms());
    let mut printed = None::<u64>;
    let mut stats_at = launch::now_ms();
    loop {
        let result = source.latest();
        let now = launch::now_ms();
        if result.is_ok() {
            if let Some(gap) = stalls.record_ok(now) {
                println!("{{\"event\":\"price_gap\",\"ms\":{gap},\"at_ms\":{now}}}");
            }
        }
        if printed.is_none_or(|at| now.saturating_sub(at) >= 1000) {
            match result {
                Ok(r) => println!("{}", serde_json::json!({
                    "price_e6": r.price, "publish_time_ms": r.publish_time_ms,
                    "age_ms": now.saturating_sub(r.publish_time_ms), "fallback": r.fallback,
                    "venues": r.venues, "spread_bps": r.spread_bps,
                })),
                Err(e) => println!("{}", serde_json::json!({"error": e.to_string(), "fallback": false})),
            }
            printed = Some(now);
        }
        if now.saturating_sub(stats_at) >= 60_000 {
            stalls.print_summary(now);
            stats_at = now;
        }
        std::thread::sleep(Duration::from_millis(cfg.oracle.push_poll_ms.max(1)));
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
    fn stall_window_closed_and_open() {
        let mut stats = StallStats::default();
        assert_eq!(stats.summary(0), (0, 0.0));
        assert_eq!(stats.record_ok(0), None);
        assert_eq!(stats.record_ok(1000), None);
        assert_eq!(stats.summary(1000), (1000, 0.0));
        assert_eq!(stats.record_ok(3000), Some(2000));
        assert_eq!(stats.summary(13_000), (10_000, 0.0));
        assert_eq!(stats.summary(23_000), (20_000, 100.0 * 20_000.0 / 600_000.0));
        assert_eq!(stats.record_ok(23_000), Some(20_000));
        assert_eq!(stats.summary(23_000), (20_000, 100.0 * 20_000.0 / 600_000.0));
        assert_eq!(stats.summary(3_613_000), (3_590_000, 100.0));
        stats.record_ok(7_213_001);
        assert_eq!(stats.gaps.len(), 1);
        assert_eq!(stats.summary(7_213_001), (STALL_WINDOW_MS, 100.0));
        // No good read since polling started: the whole wait is an open gap.
        let mut stats = StallStats::new(0);
        assert_eq!(stats.summary(1_200_000), (1_200_000, 100.0));
        assert_eq!(stats.record_ok(1_200_000), Some(1_200_000));
    }

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
