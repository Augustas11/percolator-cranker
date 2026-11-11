//! Local staging market: SOL-PERP (one asset, SOL priced in USD, test USDC
//! with 6 decimals as collateral) on an AUTH_MARK oracle, with the operator
//! roles run as bots. Reference / dry-run only; not a paid live service;
//! M3 NO-GO. See docs/STAGING.md.
//!
//! Setup: test USDC mint, market (InitMarket, one asset), permissionless
//! resolve policy, ConfigureAuthMark, an LP and a trader portfolio with
//! deposits. A dedicated price thread runs beside the main bots tick:
//!
//! 1. price pusher: `PushAuthMark` of the price source (`mock_sol_usd`) on a
//!    heartbeat or a deviation, polling every `oracle.push_poll_ms`;
//! 2. LP matcher: LP and trader co-sign `TradeNoCpi` at mark +- a fixed
//!    spread; the trader alternates open and close, so |position| <= one size;
//! 3. backing manager: keeps domains 0 and 1 funded; before expiry it
//!    withdraws the unliened principal, then tops up a new bucket (a roll);
//! 4. insurance: a small `TopUpInsurance` every `STAGING_INSURANCE_EVERY` ticks;
//! 5. keeper: one real `drip::cycle::run_one_cycle`.
//!
//! Once per run, at `STAGING_LIFECYCLE_TICK`, SOL goes through its lifecycle:
//! SHUTDOWN (asset Recovery) -> ResolveMarket (Resolved; the keeper closes
//! both portfolios) -> relisted as a fresh market. Roles 1-4 pause meanwhile.
//!
//! One log line per tick, a JSON summary every `STAGING_SUMMARY_EVERY` ticks
//! and at the end. Exit 1 when a role fails `STAGING_MAX_CONSEC_FAIL` times
//! in a row, or more than 5% of its attempts over the run.
//!
//! Params come from `launch/sol-perp.toml` (`STAGING_CONFIG`). Day-0 launch
//! modes (`launch.rs`, `scripts/launch_market.sh`): `staging step <name>
//! <config> <cluster> <run dir>` runs one resumable setup step, and `staging
//! bots <config> <cluster> <run dir>` runs roles 1-5 on that existing market.

use anyhow::{anyhow, bail, Context, Result};
use drip::{
    binding,
    cycle::{self, CrankCtx, CuStats},
    observation::{self, PythFeedResolver},
    quarantine::{Quarantine, QuarantineConfig},
    scheduler::CycleBudget,
    setup,
    submit::{self, CrankRoute, OutcomeClass},
};
use percolator::{BackingBucketStatusV16 as Bucket, MarketModeV16, BOUND_SCALE, AssetStateV16, PortfolioLegV16Account, MIN_A_SIDE, ADL_ONE};
use percolator_prog::{
    ix::Instruction as P,
    processor::ASSET_ACTION_SHUTDOWN,
    state::{self, read_asset_control_sequences, read_asset_oracle_profile, read_market, read_portfolio},
    state::{AssetControlSequencesV16, MarketGroupV16},
};
use solana_client::{rpc_client::RpcClient, rpc_config::RpcSendTransactionConfig};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair, Signature, Signer},
    system_instruction,
    transaction::Transaction,
};
use std::{env, str::FromStr, time::Duration, time::Instant};

mod launch;
mod pusher;
use launch::Config;

/// Params (market, amounts, matcher, oracle): `launch/sol-perp.toml`.
/// Amounts are collateral atoms (6 decimals); prices are e6 USD per SOL;
/// sizes are 1e-6 SOL (POS_SCALE), so notional = q * price / 1e6.
const DEFAULT_CONFIG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../launch/sol-perp.toml");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Live,
    /// SOL shut down: asset lifecycle RECOVERY.
    Recovery,
    /// Market resolved; waiting for the keeper to close both portfolios.
    Resolved,
}

#[derive(Debug, PartialEq, Eq)]
enum RejectKind {
    Program,
    Transport,
}

/// How a keeper crank outcome counts toward the abnormal exit. Only
/// Failed-class rejects count: e.g. EngineNonProgress is a HardReject the
/// keeper classes Idle (nothing to do). The keeper records a HardReject only
/// for a definitive outcome (preflight rejected, or executed and failed), so
/// it is a program failure by structure, not by error text, except
/// "Blockhash not found", a cluster/RPC condition that also arrives as a
/// rejected preflight.
fn reject_kind(o: &submit::CrankOutcome) -> Option<RejectKind> {
    match o {
        submit::CrankOutcome::HardReject { error, .. } if o.class() == OutcomeClass::Failed => {
            Some(if error.contains("Blockhash not found") { RejectKind::Transport } else { RejectKind::Program })
        }
        _ => None,
    }
}

fn env_or<T: FromStr>(key: &str, default: T) -> T {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Wrapper error name when known, else the first 120 chars of the error,
/// with any URL cut to its host (RPC errors can carry a paid endpoint key).
fn short(e: &anyhow::Error) -> String {
    let msg = launch::scrub(&format!("{e:#}"));
    submit::wrapper_error_name(&msg)
        .map(str::to_string)
        .unwrap_or_else(|| msg.chars().take(120).collect())
}

// Keeper records preserve the standard Solana instruction-error text even
// when a program code is unknown to drip. Blockhash/RPC failures do not.
fn is_program_error(error: &str) -> bool {
    error.contains("Error processing Instruction ") || error.contains("InstructionError(")
        || error.contains("\"InstructionError\"")
}

#[derive(Default, Clone)]
struct Role {
    ok: u64,
    fail: u64,
    consec: u64,
    max_consec: u64,
    transport_errors: u64,
}

impl Role {
    /// Count one attempt; returns the short log word. `Ok(None)` means the
    /// role had nothing to do this tick and is not counted.
    fn record(&mut self, r: Result<Option<String>>) -> String {
        match r {
            Ok(None) => "-".into(),
            Ok(Some(s)) => {
                self.ok += 1;
                self.consec = 0;
                s
            }
            Err(e) => {
                self.fail += 1;
                self.consec += 1;
                self.max_consec = self.max_consec.max(self.consec);
                format!("FAIL({})", short(&e))
            }
        }
    }

    fn json(&self) -> String {
        format!("{{\"ok\":{},\"fail\":{},\"max_consec_fail\":{},\"transport_errors\":{}}}", self.ok, self.fail, self.max_consec, self.transport_errors)
    }

    fn abnormal(&self, max_consec: u64, ratio: bool) -> bool {
        let n = self.ok + self.fail;
        self.consec >= max_consec || (ratio && n >= 20 && self.fail * 20 > n)
    }
}

struct Staging {
    cfg: Config,
    rpc: RpcClient,
    blockhash: submit::BlockhashCache,
    last_sig: std::cell::Cell<Option<Signature>>,
    program_id: Pubkey,
    payer: Keypair,
    /// marketauth at InitMarket, holding every asset-0 role until the
    /// launch `handover` (launch.rs). Mainnet: the fresh launch key.
    admin: Keypair,
    /// Role signers for the bots: the admin until the handover, then the
    /// holders' keys. `None`: that role's key is not on this host (skipped).
    oracle_kp: Keypair,
    backing_kp: Option<Keypair>,
    insurance_kp: Option<Keypair>,
    /// Live source configuration; neither set means the seeded local mock.
    pyth_feed: Option<[u8; 32]>,
    exchanges: Option<pusher::Exchanges>,
    flop: Option<pusher::FlopIndex>,
    /// Bots mode: also print drip-format crank lines and `mark_lag` events,
    /// which the monitor reads from this log.
    keeper_log: bool,
    mint: Pubkey,
    /// The payer is the mint authority (test mint): deposits mint first.
    own_mint: bool,
    market: Pubkey,
    vault: Pubkey,
    vault_auth: Pubkey,
    admin_ata: Pubkey,
    lp: Keypair,
    lp_pf: Pubkey,
    trader: Keypair,
    trader_pf: Pubkey,
    pyth: PythFeedResolver,
    rng: u64,
    /// Last price-source reading, and the last mark pushed on-chain.
    feed: u64,
    mark: u64,
    push_heartbeat_slots: u64,
    matcher_ctxs: Vec<String>,
    pushed: Option<std::sync::Arc<std::sync::Mutex<pusher::Shared>>>,
    /// The trader's open size, as the matcher last left it.
    open: Option<i128>,
    /// A backing roll is due: the matcher stays flat until it is done.
    rolling: bool,
    tenor: u64,
    fee_bps: u64,
    phase: Phase,
    resolved_ticks: u64,
    /// The lifecycle finished (SOL relisted); a run that enables it must.
    relisted: bool,
}

impl Staging {
    fn next_rand(&mut self) -> u64 {
        // SplitMix64.
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn tx(&self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<Transaction> {
        let mut all = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
            ComputeBudgetInstruction::request_heap_frame(128 * 1024),
        ];
        all.extend_from_slice(ixs);
        let mut keys: Vec<&Keypair> = vec![&self.payer];
        keys.extend_from_slice(signers);
        let bh = self.blockhash.get(&self.rpc)?;
        Ok(Transaction::new_signed_with_payer(&all, Some(&self.payer.pubkey()), &keys, bh))
    }

    fn send(&self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<()> {
        self.send_sig(ixs, signers)?;
        Ok(())
    }

    fn send_sig(&self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<Signature> {
        let tx = self.tx(ixs, signers)?;
        // Preflight through confirmation under the gate: a crank in this tx
        // that went stale fails preflight (no fee) instead of on chain.
        let mut gate = submit::hold_send_gate(&self.rpc);
        let sent = self.rpc.send_and_confirm_transaction_with_spinner_and_config(
            &tx, self.rpc.commitment(),
            RpcSendTransactionConfig { preflight_commitment: Some(CommitmentConfig::processed().commitment), ..Default::default() },
        );
        gate.last_send = Some(Instant::now());
        if sent.as_ref().is_err_and(submit::send_outcome_uncertain) { gate.set_pending(&tx); }
        drop(gate);
        let sig = sent.inspect_err(|e| {
            if e.to_string().contains("Blockhash not found") { self.blockhash.invalidate(); }
        })?;
        self.last_sig.set(Some(sig));
        Ok(sig)
    }

    fn simulates(&self, ixs: &[Instruction], signers: &[&Keypair]) -> bool {
        let Ok(tx) = self.tx(ixs, signers) else { return false };
        matches!(self.rpc.simulate_transaction(&tx), Ok(r) if r.value.err.is_none())
    }

    /// Create and InitPortfolio `kp` for `owner` in one tx (same account
    /// layout as `drip::setup::create_portfolio`).
    fn create_portfolio(&self, owner: &Keypair, kp: &Keypair) -> Result<()> {
        let len = state::portfolio_account_len_for_market_slots(1).map_err(|e| anyhow!("{e:?}"))?;
        let rent = self.rpc.get_minimum_balance_for_rent_exemption(len)?;
        let create = system_instruction::create_account(&self.payer.pubkey(), &kp.pubkey(), rent, len as u64, &self.program_id);
        let init = self.prog_ix(
            P::InitPortfolio,
            vec![AccountMeta::new(owner.pubkey(), true), AccountMeta::new(self.market, false), AccountMeta::new(kp.pubkey(), false)],
        );
        self.send(&[create, init], &[kp, owner]).context("create portfolio")
    }

    fn prog_ix(&self, ix: P, accounts: Vec<AccountMeta>) -> Instruction {
        Instruction { program_id: self.program_id, accounts, data: ix.encode() }
    }

    fn role_metas(&self, k: &Keypair) -> Vec<AccountMeta> {
        vec![AccountMeta::new(k.pubkey(), true), AccountMeta::new(self.market, false)]
    }

    fn admin_metas(&self) -> Vec<AccountMeta> {
        self.role_metas(&self.admin)
    }

    /// An authority transaction, signed by the admin (launch) key.
    fn send_admin(&self, label: &str, ixs: &[Instruction]) -> Result<()> {
        self.send(ixs, &[&self.admin]).context(label.to_string())
    }

    /// role key, market, its token account, vault, token program: the top-up shape.
    fn funding_metas(&self, k: &Keypair) -> Vec<AccountMeta> {
        let mut m = self.role_metas(k);
        m.push(AccountMeta::new(spl_associated_token_account::get_associated_token_address(&k.pubkey(), &self.mint), false));
        m.push(AccountMeta::new(self.vault, false));
        m.push(AccountMeta::new_readonly(spl_token::ID, false));
        m
    }

    fn read(&self) -> Result<(Vec<u8>, MarketGroupV16, AssetControlSequencesV16)> {
        let data = self.rpc.get_account_data(&self.market).context("read market")?;
        let (_, g) = read_market(&data).map_err(|e| anyhow!("read_market: {e:?}"))?;
        let s = read_asset_control_sequences(&data, 0).map_err(|e| anyhow!("control sequences: {e:?}"))?;
        Ok((data, g, s))
    }

    /// Cranks of `portfolios` that would land now. A crank with nothing to
    /// do reverts, so only the ones that simulate clean go into a tx. Trades
    /// and Live backing withdrawals need the asset accrued to the current
    /// slot, which these cranks do in the same tx.
    fn fresh_cranks(&self, data: &[u8], portfolios: &[Pubkey], now: u64) -> Result<Vec<Instruction>> {
        let obs = observation::fetch_observation_for_asset(data, 0, &self.pyth)?;
        let mut out = Vec::new();
        for pf in portfolios {
            let route = CrankRoute::Live { reward_portfolio: None };
            let (mut ixs, i) = submit::build_crank_instructions(
                &self.program_id, &self.payer.pubkey(), &self.market, pf, &route, &[obs.clone()], now, false,
            );
            // Simulate after the cranks already kept: the first one may
            // accrue the asset and leave this one with nothing to do.
            out.push(ixs.swap_remove(i));
            if !self.simulates(&out, &[]) {
                out.pop();
            }
        }
        Ok(out)
    }

    // ------------------------------------------------------------ setup

    fn exists(&self, pk: &Pubkey) -> Result<bool> {
        Ok(self.rpc.get_account_with_commitment(pk, CommitmentConfig::confirmed())?.value.is_some())
    }

    /// Deposit `amount` from the owner's token account, minting it first
    /// when we own the test mint.
    fn deposit(&self, owner: &Keypair, portfolio: &Pubkey, amount: u64) -> Result<()> {
        let ata = spl_associated_token_account::get_associated_token_address(&owner.pubkey(), &self.mint);
        if self.own_mint {
            setup::mint_to(&self.rpc, &self.payer, &self.mint, &ata, &self.payer, amount)?;
        }
        let b = binding::portfolio_binding(&self.rpc.get_account_data(portfolio)?)?;
        let ix = self.prog_ix(
            P::Deposit { portfolio_id: b.portfolio_id, expected_sequence: b.sequence, amount: amount as u128 },
            vec![
                AccountMeta::new(owner.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(*portfolio, false),
                AccountMeta::new(ata, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
        );
        self.send(&[ix], &[owner]).context("Deposit")
    }

    #[allow(clippy::too_many_arguments)]
    fn new(cfg: Config, rpc: RpcClient, program_id: Pubkey, payer: Keypair, admin: Keypair, lp: Keypair, trader: Keypair, mint: Pubkey, own_mint: bool) -> Staging {
        let o = cfg.oracle.clone();
        let admin_ata = spl_associated_token_account::get_associated_token_address(&admin.pubkey(), &mint);
        Staging {
            blockhash: submit::BlockhashCache::default(),
            last_sig: std::cell::Cell::new(None),
            rpc,
            program_id,
            payer,
            oracle_kp: admin.insecure_clone(),
            backing_kp: Some(admin.insecure_clone()),
            insurance_kp: Some(admin.insecure_clone()),
            admin,
            pyth_feed: None,
            exchanges: None,
            flop: None,
            keeper_log: false,
            mint,
            own_mint,
            market: Pubkey::default(),
            vault: Pubkey::default(),
            vault_auth: Pubkey::default(),
            admin_ata,
            lp_pf: Pubkey::default(),
            lp,
            trader_pf: Pubkey::default(),
            trader,
            pyth: observation::empty_pyth_resolver(),
            rng: env_or("STAGING_SEED", 42u64),
            feed: cfg.market.initial_price,
            mark: cfg.market.initial_price,
            push_heartbeat_slots: env_or("STAGING_PUSH_HEARTBEAT_SLOTS", o.push_heartbeat_slots),
            matcher_ctxs: cfg.cluster.get("local").map(|c| c.matcher_ctxs.clone()).unwrap_or_default(),
            pushed: None,
            open: None,
            rolling: false,
            tenor: env_or("STAGING_BACKING_TENOR_SLOTS", cfg.amounts.backing_tenor_slots),
            fee_bps: env_or("STAGING_TRADE_FEE_BPS", cfg.matcher.fee_bps),
            phase: Phase::Live,
            resolved_ticks: 0,
            relisted: false,
            cfg,
        }
    }

    /// R2: fresh role keys and test mint, then a market (`open_market`).
    fn setup(cfg: Config, rpc: RpcClient, program_id: Pubkey, payer: Keypair) -> Result<Staging> {
        let (admin, lp, trader) = (Keypair::new(), Keypair::new(), Keypair::new());
        let fund: Vec<Instruction> = [&admin, &lp, &trader]
            .iter()
            .map(|k| system_instruction::transfer(&payer.pubkey(), &k.pubkey(), 10_000_000_000))
            .collect();
        let bh = rpc.get_latest_blockhash()?;
        let tx = Transaction::new_signed_with_payer(&fund, Some(&payer.pubkey()), &[&payer], bh);
        rpc.send_and_confirm_transaction(&tx).context("fund role keys")?;

        // Test USDC: an existing mint whose authority is the payer
        // (STAGING_USDC_MINT), or a fresh one.
        let mint = match env::var("STAGING_USDC_MINT") {
            Ok(m) => Pubkey::from_str(&m).context("STAGING_USDC_MINT")?,
            Err(_) => {
                let kp = Keypair::new();
                setup::create_spl_mint(&rpc, &payer, &kp, &payer.pubkey(), 6).context("create test USDC mint")?;
                kp.pubkey()
            }
        };
        for owner in [&admin, &lp, &trader] {
            setup::create_ata(&rpc, &payer, &owner.pubkey(), &mint)?;
        }
        let mut st = Staging::new(cfg, rpc, program_id, payer, admin, lp, trader, mint, true);
        setup::mint_to(&st.rpc, &st.payer, &mint, &st.admin_ata, &st.payer, st.cfg.amounts.authority_test_mint)?;
        st.open_market()?;
        Ok(st)
    }

    /// A fresh SOL-PERP market at the current mark, with new LP and trader
    /// portfolios and deposits. Used at setup and to relist SOL.
    fn open_market(&mut self) -> Result<()> {
        self.create_market(&Keypair::new())?;
        self.configure_oracle()?;
        self.lp_pf = setup::create_portfolio(&self.rpc, &self.payer, &self.lp, &self.market, self.program_id)?;
        self.trader_pf = setup::create_portfolio(&self.rpc, &self.payer, &self.trader, &self.market, self.program_id)?;
        self.deposit(&self.lp, &self.lp_pf, self.cfg.amounts.lp_deposit)?;
        self.deposit(&self.trader, &self.trader_pf, self.cfg.amounts.trader_deposit)?;
        (self.open, self.rolling, self.phase, self.resolved_ticks) = (None, false, Phase::Live, 0);
        Ok(())
    }

    /// The market account and `InitMarket` (`[market]` in the config) in
    /// ONE tx (payer, market key, admin), so no one can initialize our
    /// account in between; then the collateral vault. An existing market is
    /// accepted only with our admin as marketauth and our mint.
    fn create_market(&mut self, market: &Keypair) -> Result<()> {
        self.set_market(market.pubkey());
        if !self.exists(&self.market)? {
            let len = state::market_account_len_for_capacity(1).map_err(|e| anyhow!("{e:?}"))?;
            let rent = self.rpc.get_minimum_balance_for_rent_exemption(len)?;
            let create = system_instruction::create_account(&self.payer.pubkey(), &self.market, rent, len as u64, &self.program_id);
            let mut metas = self.admin_metas();
            metas.push(AccountMeta::new_readonly(self.mint, false));
            let init = self.prog_ix(self.cfg.market.init_ix(self.mark)?, metas);
            self.send(&[create, init], &[market, &self.admin]).context("create market + InitMarket")?;
        } else {
            let data = self.rpc.get_account_data(&self.market)?;
            let ours = read_market(&data).is_ok_and(|(c, _)| {
                c.marketauth == self.admin.pubkey().to_bytes() && c.collateral_mint == self.mint.to_bytes()
            });
            if !ours {
                bail!("market {} exists without our marketauth {} and mint {}: market taken; use a fresh run dir", self.market, self.admin.pubkey(), self.mint);
            }
        }
        if !self.exists(&self.vault)? {
            setup::create_ata(&self.rpc, &self.payer, &self.vault_auth, &self.mint)?;
        }
        Ok(())
    }

    fn set_market(&mut self, market: Pubkey) {
        self.market = market;
        self.vault_auth = Pubkey::find_program_address(&[b"vault", self.market.as_ref()], &self.program_id).0;
        self.vault = spl_associated_token_account::get_associated_token_address(&self.vault_auth, &self.mint);
    }

    /// Permissionless-resolve policy and AuthMark, each unless already set
    /// (their control sequences start at 0), in one authority tx.
    fn configure_oracle(&mut self) -> Result<()> {
        let (_, g, s) = self.read()?;
        let now = self.rpc.get_slot()?;
        let mut ixs = Vec::new();
        // SHUTDOWN needs a force-close delay; this also arms stale resolve.
        if s.permissionless_resolve == 0 {
            let ix = P::ConfigurePermissionlessResolve {
                asset_generation_frontier: g.next_market_id,
                stale_slots: self.cfg.resolve.stale_slots,
                force_close_delay_slots: self.cfg.resolve.force_close_delay_slots,
                policy_sequence: s.permissionless_resolve + 1,
                authority_epoch: s.authority_epoch,
            };
            ixs.push(self.prog_ix(ix, self.admin_metas()));
        }
        // AUTH_MARK must be configured while the market is flat. Mirrors
        // v16_svm.rs `configure_auth_mark`; same order as the upstream CLI
        // devnet smoke T10 (InitMarket, ConfigureAuthMark, PushAuthMark).
        if s.oracle_observation == 0 {
            let ix = P::ConfigureAuthMark {
                asset_index: 0,
                market_id: g.assets[0].market_id,
                now_slot: now,
                initial_mark_e6: self.mark,
                observation_sequence: s.oracle_observation + 1,
                authority_epoch: s.authority_epoch,
            };
            ixs.push(self.prog_ix(ix, self.admin_metas()));
        }
        if !ixs.is_empty() {
            self.send_admin("ConfigureOracle", &ixs)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ roles

    /// The pusher's price source: SOL/USD e6 and its timestamp (unix ms).
    /// Exchanges or Pyth (on-chain account) when configured;
    /// else a seeded mock path, one step of at most `max_bps`, kept within
    /// +-5% of the configured initial price, timestamped when generated.
    fn source_price(&mut self, max_bps: u64) -> Result<(u64, u64)> {
        if let Some(source) = &mut self.exchanges { return source.latest().map(|r| (r.price, r.publish_time_ms)); }
        if let Some(source) = &mut self.flop { return source.latest(); }
        let mut source = match self.pyth_feed {
            Some(feed) => pusher::PriceSource::Pyth {
                rpc: RpcClient::new_with_commitment(launch::pyth_rpc_url(&self.cfg.oracle, &self.rpc.url()), CommitmentConfig::confirmed()),
                feed, cfg: self.cfg.oracle.clone(),
            },
            None => pusher::PriceSource::Mock {
                price: self.feed, initial: self.cfg.market.initial_price, rng: self.rng, max_bps,
            },
        };
        let reading = source.latest()?;
        if let pusher::PriceSource::Mock { price, rng, .. } = source {
            self.feed = price;
            self.rng = rng;
        }
        Ok((reading.0, reading.1))
    }

    fn match_trade(&mut self, lifecycle: &std::sync::Mutex<()>) -> Result<Option<String>> {
        if self.trader_pf == Pubkey::default() {
            return Ok(None); // no test trader (mainnet)
        }
        // Match the pusher's processed confirmation, including the preceding
        // push, and keep market/LP in one snapshot through trade confirmation.
        let _in_flight = lifecycle.lock().unwrap();
        let snapshot = self.rpc.get_multiple_accounts_with_commitment(
            &[self.market, self.lp_pf, self.trader_pf], CommitmentConfig::processed(),
        )?;
        let data = &snapshot.value[0].as_ref().context("market missing")?.data;
        let (_, g) = read_market(data).map_err(|e| anyhow!("{e:?}"))?;
        if self.rolling && self.open.is_none() {
            return Ok(None);
        }
        // A bucket just past expiry refuses fills (EngineStale) until the
        // keeper's crank of a claim holder expires it: skip this tick.
        let now = snapshot.context.slot;
        if now.saturating_sub(g.assets[0].slot_last) > self.cfg.oracle.accrual_lag_slots {
            return Ok(Some("refused(accrual_behind)".into()));
        }
        if g.source_backing_buckets[..2].iter().any(|b| b.status == Bucket::Fresh && b.expiry_slot <= now) {
            return Ok(None);
        }
        // Trader is account a: size_q > 0 makes it long.
        let coin = self.next_rand() % 2 == 0;
        let q = match self.open {
            Some(q) => -q,
            None if coin => self.cfg.matcher.size_q as i128,
            None => -(self.cfg.matcher.size_q as i128),
        };
        if self.open.is_none() {
            let fresh = self.pushed.as_ref().is_some_and(|p| {
                let p = p.lock().unwrap();
                p.market == self.market && p.published_ms.is_some_and(|at| launch::now_ms().saturating_sub(at) <= self.cfg.oracle.open_max_mark_age_ms)
            });
            if !fresh { return Ok(Some("refused(stale_mark)".into())); }
        }
        let mark = g.assets[0].effective_price;
        let lp_data = &snapshot.value[1].as_ref().context("LP portfolio missing")?.data;
        let lp = read_portfolio(lp_data).map_err(|e| anyhow!("{e:?}"))?;
        let lp_position = net_q(&lp.legs, 0, &g.assets[0])?;
        if let Err(reason) = fill_allowed(q, mark, g.assets[0].raw_oracle_target_price, lp_position, &self.cfg.matcher) {
            return Ok(Some(format!("refused({reason})")));
        }
        let half = mark * self.cfg.matcher.spread_bps / 10_000;
        let price = if q > 0 { mark + half } else { mark - half };
        let tb = binding::portfolio_binding(&snapshot.value[2].as_ref().context("trader portfolio missing")?.data)?;
        let lb = binding::portfolio_binding(lp_data)?;
        let mut ixs = self.fresh_cranks(data, &[self.trader_pf, self.lp_pf], now)?;
        ixs.push(self.prog_ix(
            P::TradeNoCpi {
                account_a_portfolio_id: tb.portfolio_id,
                account_a_position_epoch: tb.position_epoch,
                account_b_portfolio_id: lb.portfolio_id,
                account_b_position_epoch: lb.position_epoch,
                asset_index: 0,
                market_id: g.assets[0].market_id,
                size_q: q,
                exec_price: price,
                fee_bps: self.fee_bps,
                backing_fee_cap_bps: 0,
            },
            vec![
                AccountMeta::new(self.trader.pubkey(), true),
                AccountMeta::new(self.lp.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(self.trader_pf, false),
                AccountMeta::new(self.lp_pf, false),
            ],
        ));
        if let Err(error) = self.send(&ixs, &[&self.trader, &self.lp]) {
            // The snapshot can already have target/effective lag; the engine
            // still refuses the fill.
            if short(&error) == "EngineLockActive" {
                return Ok(Some("refused(target_effective_lag)".into()));
            }
            return Err(error);
        }
        self.open = if self.open.is_some() { None } else { Some(q) };
        Ok(Some(format!("{}{}", if q > 0 { "+" } else { "-" }, q.unsigned_abs())))
    }

    fn manage_backing(&mut self) -> Result<Option<String>> {
        if self.backing_kp.is_none() {
            return Ok(None);
        }
        let (data, g, _) = self.read()?;
        if g.mode != MarketModeV16::Live {
            return Ok(None);
        }
        let now = g.current_slot.max(self.rpc.get_slot()?);
        let margin = self.tenor / 4;
        let mut did = Vec::new();
        let mut rolling = false;
        for d in 0..2usize {
            let b = g.source_backing_buckets[d];
            let unliened = b.fresh_unliened_backing_num / BOUND_SCALE;
            match b.status {
                Bucket::Empty | Bucket::Expired => {
                    self.top_up_backing(&[d], now + self.tenor)?;
                    did.push(format!("d{d}:topup"));
                }
                // Past expiry with principal left. If positive PnL is still
                // sourced from the domain, its holders reference it and the
                // keeper's crank expires it. If not, it is the F1 wedge.
                Bucket::Fresh if b.expiry_slot <= now && unliened > 0 => {
                    if g.source_credit[d].positive_claim_bound_num == 0 {
                        bail!("domain {d} Fresh past expiry with {unliened} unliened, unreferenced (F1 shape)");
                    }
                }
                // The roll: withdraw what is not backing open claims (the
                // engine keeps the domain fully backed). Live withdrawals also
                // need no exposed mark lag, so the matcher waits at flat.
                Bucket::Fresh if b.expiry_slot <= now + margin => {
                    let claims = g.source_credit[d].positive_claim_bound_num.div_ceil(BOUND_SCALE);
                    let free = unliened.saturating_sub(claims);
                    if free > 0 {
                        rolling = true;
                        if self.open.is_none() {
                            self.withdraw_backing(&data, d, free)?;
                            did.push(format!("d{d}:out{free}"));
                        }
                    }
                }
                _ => {}
            }
        }
        self.rolling = rolling;
        Ok((!did.is_empty()).then(|| did.join(",")))
    }

    /// Top up the `domains` (0 long, 1 short) in one authority tx.
    fn top_up_backing(&self, domains: &[usize], expiry_slot: u64) -> Result<()> {
        let k = self.backing_kp.as_ref().context("no backing key on this host")?;
        let (data, g, s) = self.read()?;
        let p = read_asset_oracle_profile(&data, 0).map_err(|e| anyhow!("oracle profile: {e:?}"))?;
        let mut ixs = Vec::new();
        for (i, &d) in domains.iter().enumerate() {
            let (fee, share) = if d == 0 {
                (p.backing_trade_fee_bps_long, p.backing_trade_fee_insurance_share_bps_long)
            } else {
                (p.backing_trade_fee_bps_short, p.backing_trade_fee_insurance_share_bps_short)
            };
            // Mirrors v16_svm.rs `top_up_backing_bucket_without_ledger`.
            let ix = P::TopUpBackingBucket {
                domain: d as u16,
                market_id: g.assets[0].market_id,
                intent_id: s.backing_top_up + 1 + i as u64,
                authority_epoch: s.authority_epoch,
                backing_fee_bps: fee,
                insurance_share_bps: share,
                amount: self.cfg.amounts.backing_per_domain as u128,
                expiry_slot,
            };
            ixs.push(self.prog_ix(ix, self.funding_metas(k)));
        }
        self.send(&ixs, &[k]).context("TopUpBackingBucket")
    }

    fn withdraw_backing(&self, data: &[u8], d: usize, amount: u128) -> Result<()> {
        let (_, g, s) = self.read()?;
        let k = self.backing_kp.as_ref().context("no backing key on this host")?;
        let mut ixs = self.fresh_cranks(data, &[self.trader_pf, self.lp_pf], self.rpc.get_slot()?)?;
        let mut metas = self.role_metas(k);
        metas.extend([
            AccountMeta::new(spl_associated_token_account::get_associated_token_address(&k.pubkey(), &self.mint), false),
            AccountMeta::new(self.vault, false),
            AccountMeta::new_readonly(self.vault_auth, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ]);
        // Mirrors v16_svm.rs `withdraw_backing_bucket`.
        let ix = P::WithdrawBackingBucket {
            domain: d as u16,
            market_id: g.assets[0].market_id,
            authority_epoch: s.authority_epoch,
            amount,
        };
        ixs.push(self.prog_ix(ix, metas));
        self.send(&ixs, &[k]).context("WithdrawBackingBucket")
    }

    fn top_up_insurance(&self, amount: u64) -> Result<Option<String>> {
        let Some(k) = &self.insurance_kp else { return Ok(None) };
        let (_, g, s) = self.read()?;
        // Mirrors v16_svm.rs `top_up_insurance`.
        let ix = P::TopUpInsurance {
            market_id: g.assets[0].market_id,
            intent_id: s.insurance_top_up + 1,
            authority_epoch: s.authority_epoch,
            amount: amount as u128,
        };
        self.send(&[self.prog_ix(ix, self.funding_metas(k))], &[k]).context("TopUpInsurance")?;
        Ok(Some(format!("+{amount}")))
    }
}

impl Staging {
    // ------------------------------------------------------------ lifecycle

    /// Live -> Recovery: SHUTDOWN of SOL (asset 0). Fresh cranks go first:
    /// shutdown accrues committed funding up to now.
    fn shutdown_ixs(&self) -> Result<Vec<Instruction>> {
        let (data, g, s) = self.read()?;
        let mut ixs = self.fresh_cranks(&data, &[self.trader_pf, self.lp_pf], self.rpc.get_slot()?)?;
        // Mirrors v16_svm.rs `shutdown_asset` (the role keys are ignored by
        // SHUTDOWN; upstream passes the admin's).
        let admin = self.admin.pubkey().to_bytes();
        let ix = P::UpdateAssetLifecycle {
            action: ASSET_ACTION_SHUTDOWN,
            asset_index: 0,
            market_id: g.assets[0].market_id,
            authority_epoch: s.authority_epoch,
            now_slot: self.rpc.get_slot()?,
            initial_price: 0,
            max_init_fee: u128::MAX,
            insurance_authority: admin,
            insurance_operator: admin,
            backing_bucket_authority: admin,
            oracle_authority: admin,
        };
        ixs.push(self.prog_ix(ix, self.admin_metas()));
        Ok(ixs)
    }

    fn shutdown(&mut self) -> Result<Option<String>> {
        self.send(&self.shutdown_ixs()?, &[&self.admin]).context("SHUTDOWN")?;
        self.phase = Phase::Recovery;
        Ok(Some("SOL shut down (Recovery)".into()))
    }

    /// Recovery -> Resolved: ResolveMarket by the market authority.
    fn resolve_ixs(&self) -> Result<Vec<Instruction>> {
        let (data, g, s) = self.read()?;
        let mut ixs = self.fresh_cranks(&data, &[self.trader_pf, self.lp_pf], self.rpc.get_slot()?)?;
        // Mirrors v16_svm.rs `resolve_market`.
        let ix = P::ResolveMarket { asset_generation_frontier: g.next_market_id, authority_epoch: s.authority_epoch };
        ixs.push(self.prog_ix(ix, self.admin_metas()));
        Ok(ixs)
    }

    fn resolve(&mut self) -> Result<Option<String>> {
        self.send(&self.resolve_ixs()?, &[&self.admin]).context("ResolveMarket")?;
        self.phase = Phase::Resolved;
        Ok(Some("market resolved".into()))
    }

    fn portfolio_settled(&self, pf: &Pubkey) -> Result<bool> {
        // Read errors fail the step: only a confirmed-absent account or a
        // read, settled portfolio counts as closed.
        match self.rpc.get_account_with_commitment(pf, CommitmentConfig::confirmed())?.value {
            None => Ok(true),
            Some(a) => {
                let p = read_portfolio(&a.data).map_err(|e| anyhow!("read_portfolio {pf}: {e:?}"))?;
                Ok(p.capital.get() == 0
                    && p.pnl.get() == 0
                    && p.reserved_pnl.get() == 0
                    && p.legs.iter().all(|l| l.active == 0)
                    && (p.resolved_payout_receipt.present == 0
                        || p.resolved_payout_receipt.finalized != 0))
            }
        }
    }

    /// Resolved -> relisted: once the keeper's resolved route has closed both
    /// portfolios (paid out to the owners), open a fresh SOL-PERP market.
    fn relist_when_closed(&mut self) -> Result<Option<String>> {
        self.resolved_ticks += 1;
        if !(self.portfolio_settled(&self.lp_pf)? && self.portfolio_settled(&self.trader_pf)?) {
            if self.resolved_ticks > 100 {
                bail!("portfolios still open {} ticks after resolve", self.resolved_ticks);
            }
            return Ok(None);
        }
        let old = self.market;
        self.open_market()?;
        self.relisted = true;
        Ok(Some(format!("SOL relisted: market {old} -> {}", self.market)))
    }
}

#[derive(Default)]
struct Keeper {
    role: Role,
    progressed: u64,
    idle: u64,
    failed: u64,
}

fn summary(tick: u64, started: Instant, st: &Staging, roles: &[(&str, &Role)], k: &Keeper, last: bool) -> String {
    let (ins, vault) = st.read().map(|(_, g, _)| (g.insurance, g.vault)).unwrap_or_default();
    let roles: Vec<String> = roles.iter().map(|(n, r)| format!("\"{n}\":{}", r.json())).collect();
    format!(
        "{{\"summary\":{},\"tick\":{tick},\"elapsed_s\":{},\"sol_usd_e6\":{},\"phase\":\"{:?}\",\"market\":\"{}\",{},\"keeper\":{},\"crank\":{{\"progressed\":{},\"idle\":{},\"failed\":{}}},\"insurance_balance\":{ins},\"vault\":{vault}}}",
        if last { "\"final\"" } else { "\"periodic\"" },
        started.elapsed().as_secs(),
        st.mark,
        st.phase,
        st.market,
        roles.join(","),
        k.role.json(),
        k.progressed,
        k.idle,
        k.failed,
    )
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("Error: {}", launch::scrub(&format!("{e:#}")));
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run(r2_setup()?, false),
        Some("step") => launch::step_main(&args[1..]),
        Some("bots") => run(launch::bots(&args[1..])?, true),
        Some("roles") => launch::roles_main(&args[1..]),
        Some("lifecycle") => launch::lifecycle_main(&args[1..]),
        Some("prices") => pusher::prices(&args[1..]),
        Some(m) => bail!("unknown mode {m}; usage: staging [step <name> | bots | roles | prices | lifecycle] <config> <cluster> <run dir>"),
    }
}

/// R2: a fresh market from env (`STAGING_*`) plus the config's params.
fn r2_setup() -> Result<Staging> {
    let url = env::var("STAGING_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8799".into());
    if url.to_lowercase().contains("mainnet") {
        bail!("refusing a mainnet RPC: {}", launch::redact(&url));
    }
    let cfg = launch::load(&env::var("STAGING_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG.into()))?;
    let program_id = Pubkey::from_str(&env::var("PERCOLATOR_PROGRAM_ID").context("PERCOLATOR_PROGRAM_ID must be set")?)?;
    let payer_path = env::var("STAGING_PAYER")
        .unwrap_or_else(|_| format!("{}/.config/solana/id.json", env::var("HOME").unwrap_or_default()));
    let payer = read_keypair_file(&payer_path).map_err(|e| anyhow!("read payer {payer_path}: {e}"))?;
    let rpc = RpcClient::new_with_commitment(url.clone(), CommitmentConfig::confirmed());
    rpc.get_version().with_context(|| format!("connect to {}", launch::redact(&url)))?;
    // The R2 rehearsal is local/test only: the chain decides, not the URL.
    if launch::is_mainnet(&rpc)? {
        bail!("R2 staging refuses mainnet ({})", launch::redact(&url));
    }
    Staging::setup(cfg, rpc, program_id, payer)
}

fn trade_due(last: Option<Instant>, now: Instant, every_secs: u64, roll_close: bool) -> bool {
    // A backing roll must not wait on the trader cadence to close.
    roll_close || every_secs == 0 || last.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(every_secs))
}

/// The bots loop. `bots`: an existing launch market, run until stopped
/// (no lifecycle); else the R2 run of `STAGING_HOURS` with its lifecycle.
fn run(mut st: Staging, bots: bool) -> Result<()> {
    let hours: f64 = env_or("STAGING_HOURS", if bots { 24.0 * 365.0 } else { 6.0 });
    let tick_ms: u64 = env_or("STAGING_TICK_MS", st.cfg.oracle.push_interval_ms);
    let trade_every_secs: u64 = env_or("STAGING_TRADE_EVERY_SECS", if bots { 300 } else { 0 });
    let summary_every: u64 = env_or("STAGING_SUMMARY_EVERY", 100);
    let insurance_every: u64 = env_or("STAGING_INSURANCE_EVERY", st.cfg.amounts.insurance_every);
    let max_consec: u64 = env_or("STAGING_MAX_CONSEC_FAIL", 10);
    println!(
        "staging SOL-PERP: rpc={} program={} usdc_mint={} market={} lp={} trader={} hours={hours} tick_ms={tick_ms} backing_tenor_slots={}",
        launch::redact(&st.rpc.url()), st.program_id, st.mint, st.market, st.lp_pf, st.trader_pf, st.tenor
    );
    if st.keeper_log {
        // drip's boot line: the monitor discovers the market from it.
        println!("market            = {}", st.market);
    }

    let (pushed, push_lifecycle, pusher_thread) = pusher::start(&st)?;
    st.pushed = Some(std::sync::Arc::clone(&pushed));
    let budget = CycleBudget::from_env();
    let mut quarantine = Quarantine::new(QuarantineConfig::from_env());
    let mut cu = CuStats::default();
    let mut push;
    let mut accrual;
    let (mut trade, mut backing, mut insurance) = (Role::default(), Role::default(), Role::default());
    let mut lifecycle = Role::default();
    // Default: halfway through the run. 0 turns the lifecycle off.
    let lifecycle_tick: u64 = if bots { 0 } else { env_or("STAGING_LIFECYCLE_TICK", (hours * 1_800_000.0 / tick_ms as f64) as u64) };
    let mut keeper = Keeper::default();
    let started = Instant::now();
    let deadline = started + Duration::from_secs_f64(hours * 3600.0);
    let mut tick = 0u64;
    let mut last_trade = None;
    let mut abnormal = false;
    while Instant::now() < deadline && !abnormal {
        tick += 1;
        let t0 = Instant::now();
        let transitioning = st.phase != Phase::Live
            || (lifecycle_tick != 0 && tick >= lifecycle_tick && !st.relisted);
        let in_flight = if transitioning { Some(push_lifecycle.lock().unwrap()) } else { None };
        let mut push_state = pushed.lock().unwrap();
        st.mark = push_state.mark;
        let l = lifecycle.record(match st.phase {
            // From its tick on, SHUTDOWN is retried until it lands.
            Phase::Live if lifecycle_tick != 0 && tick >= lifecycle_tick && !st.relisted => st.shutdown(),
            Phase::Recovery => st.resolve(),
            Phase::Resolved => st.relist_when_closed(),
            Phase::Live => Ok(None),
        });
        let live = st.phase == Phase::Live;
        push_state.live = live;
        push_state.portfolio = st.lp_pf;
        if push_state.market != st.market {
            push_state.market = st.market;
            push_state.mark = st.mark;
            push_state.landed_ms = None;
            push_state.published_ms = None;
        }
        push = push_state.role.clone();
        accrual = push_state.accrual.clone();
        drop(push_state);
        drop(in_flight);
        let p = "thread";
        let trade_now = Instant::now();
        let t = if live && trade_due(last_trade, trade_now, trade_every_secs, st.rolling && st.open.is_some()) {
            let r = trade.record(st.match_trade(&push_lifecycle));
            // A refusal or skip sends nothing: retry next tick instead of waiting
            // out the cadence (the first tick often runs before the first mark push).
            if r != "-" && !r.starts_with("refused(") {
                last_trade = Some(trade_now);
            }
            r
        } else { "-".into() };
        let b = if live { backing.record(st.manage_backing()) } else { "-".into() };
        let i = if live && tick % insurance_every == 0 { insurance.record(st.top_up_insurance(st.cfg.amounts.insurance_topup)) } else { "-".into() };

        let ctx = CrankCtx {
            blockhash: &st.blockhash,
            rpc: &st.rpc,
            program_id: &st.program_id,
            payer: &st.payer,
            market: &st.market,
            reward_portfolio: None,
            pyth: &st.pyth,
            exclude: &[],
        };
        let mut records = Vec::new();
        // Inside the force-close delay after SHUTDOWN, cranks of the
        // shut-down asset are refused (EngineLockActive): the keeper waits
        // for the resolve on the next tick.
        let k = if st.phase == Phase::Recovery { "-".into() } else {
            let result = cycle::run_one_cycle(&ctx, tick, &budget, &mut quarantine, &mut records, &mut cu);
            // Records retain failed txs even when the cycle also made progress.
            // The keeper only records a HardReject for a DEFINITIVE outcome
            // (preflight rejected, or the tx executed and failed), so every
            // one is a program failure, decided by its structure, not by its
            // error text. The one exception is "Blockhash not found": a
            // cluster/RPC condition that also arrives as a rejected preflight.
            let program_failures = records.iter().filter(|r| reject_kind(&r.outcome) == Some(RejectKind::Program)).count();
            let rejected_transport = records.iter().filter(|r| {
                let t = reject_kind(&r.outcome) == Some(RejectKind::Transport);
                if t { if let submit::CrankOutcome::HardReject { error, .. } = &r.outcome { eprintln!("keeper transport: {}", launch::scrub(error)); } }
                t
            }).count();
            let mut transport_errors = rejected_transport;
            let result = match result {
                Ok(s) => {
                    keeper.progressed += s.progressed as u64;
                    keeper.idle += s.idle as u64;
                    keeper.failed += program_failures as u64;
                    let rpc_failures: Vec<_> = s.failures.iter().filter(|(_, e)| e.starts_with("Rpc:")).collect();
                    transport_errors += s.unconfirmed.len() + s.drain_rpc_errors.len() + rpc_failures.len();
                    if !s.unconfirmed.is_empty() || !s.drain_rpc_errors.is_empty() || !rpc_failures.is_empty() {
                        eprintln!("keeper transport: unconfirmed={:?}, rpc={rpc_failures:?}, after_progress={:?}", s.unconfirmed, s.drain_rpc_errors);
                    }
                    if st.keeper_log { launch::print_keeper_lines(&records, &s.line()); }
                    if program_failures > 0 {
                        Err(anyhow!("{program_failures} failed program txs: {:?}", s.failures))
                    } else if s.progressed + s.idle == 0 && transport_errors > 0 {
                        // Transport-only cycle: neither a success nor a failure,
                        // so it can't reset or dilute the failure accounting.
                        Ok(None)
                    } else {
                        Ok(Some(format!("p{}/i{}", s.progressed, s.idle)))
                    }
                }
                Err(e) => {
                    if program_failures > 0 || is_program_error(&format!("{e:#}")) {
                        keeper.failed += program_failures.max(1) as u64;
                        Err(e)
                    } else {
                        transport_errors += 1;
                        eprintln!("keeper transport: {}", short(&e));
                        Ok(None)
                    }
                }
            };
            if transport_errors > 0 {
                keeper.role.transport_errors += transport_errors as u64;
                eprintln!("keeper: {transport_errors} transport errors (excluded from abnormal exit)");
            }
            keeper.role.record(result)
        };
        let l = if l == "-" { String::new() } else { format!(" lifecycle={l}") };
        println!("t{tick} SOL={} push={p} accrual={} trade={t} backing={b} insurance={i} keeper={k}{l}", st.mark, accrual.json());

        let roles = [("push", &push), ("accrual", &accrual), ("trade", &trade), ("backing", &backing), ("insurance", &insurance), ("lifecycle", &lifecycle)];
        abnormal = roles.iter().any(|(name, r)| *name != "accrual" && *name != "trade" && r.abnormal(max_consec, !bots)) || keeper.role.abnormal(max_consec, !bots);
        if tick % summary_every == 0 {
            println!("{}", summary(tick, started, &st, &roles, &keeper, false));
        }
        if let Some(rest) = Duration::from_millis(tick_ms).checked_sub(t0.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    pushed.lock().unwrap().stop = true;
    pusher_thread.join().map_err(|_| anyhow!("pusher thread panicked"))?;
    push = pushed.lock().unwrap().role.clone();
    accrual = pushed.lock().unwrap().accrual.clone();
    abnormal |= push.abnormal(max_consec, !bots);
    let roles = [("push", &push), ("accrual", &accrual), ("trade", &trade), ("backing", &backing), ("insurance", &insurance), ("lifecycle", &lifecycle)];
    println!("{}", summary(tick, started, &st, &roles, &keeper, true));
    if abnormal {
        eprintln!("staging: ABNORMAL failure rate, see the summary");
        std::process::exit(1);
    }
    if lifecycle_tick != 0 && !st.relisted {
        eprintln!("staging: lifecycle did not complete (shutdown -> resolve -> relist)");
        std::process::exit(1);
    }
    println!("staging: OK");
    Ok(())
}

// Effective signed quantity, copied from hedger/src/chain.rs::net_q.
fn net_q(legs: &[PortfolioLegV16Account], asset: usize, a: &AssetStateV16) -> Result<i128> {
    let mut net = 0i128;
    for l in legs.iter().filter(|l| l.active != 0 && l.asset_index.get() as usize == asset) {
        let (current_a, epoch, sign) = match l.side {
            0 => (a.a_long, a.epoch_long, 1),
            1 => (a.a_short, a.epoch_short, -1),
            s => bail!("unknown leg side {s}"),
        };
        let epoch_snap = l.epoch_snap.get();
        if epoch_snap < epoch {
            continue;
        }
        if epoch_snap > epoch {
            bail!("leg epoch {epoch_snap} ahead of asset epoch {epoch}");
        }
        let (raw, basis) = (l.basis_pos_q.get().unsigned_abs(), l.a_basis.get());
        if !(MIN_A_SIDE..=ADL_ONE).contains(&basis) || !(1..=basis).contains(&current_a) {
            bail!("leg A index out of range");
        }
        let prod = raw.checked_mul(current_a).context("effective quantity overflow")?;
        let eff = prod.div_ceil(basis);
        net += sign * i128::try_from(eff).context("effective quantity exceeds i128")?;
    }
    Ok(net)
}

/// size_q is the trader delta; the LP takes its opposite. Compare full
/// products so one atom above a dollar limit cannot disappear in rounding.
fn fill_allowed(size_q: i128, effective_price: u64, target_price: u64, lp_position_q: i128, cfg: &launch::MatcherCfg) -> Result<(), &'static str> {
    let mark_e6 = effective_price.max(target_price);
    let next = lp_position_q.checked_sub(size_q).ok_or("inventory_overflow")?;
    let notional = size_q.unsigned_abs().checked_mul(mark_e6 as u128).ok_or("notional_overflow")?;
    if notional > cfg.max_fill_notional_usd as u128 * 1_000_000_000_000 {
        return Err("max_fill_notional");
    }
    if next.unsigned_abs() <= lp_position_q.unsigned_abs() { return Ok(()); }
    let inventory = next.unsigned_abs().checked_mul(mark_e6 as u128).ok_or("inventory_overflow")?;
    if inventory > cfg.max_lp_inventory_usd as u128 * 1_000_000_000_000 {
        return Err("max_lp_inventory");
    }
    Ok(())
}

#[cfg(test)]
mod fill_tests {
    use super::*;
    #[test]
    fn trade_cadence() {
        let last = Instant::now();
        assert!(trade_due(Some(last), last, 0, false));
        assert!(trade_due(None, last, 300, false));
        assert!(!trade_due(Some(last), last + Duration::from_secs(299), 300, false));
        assert!(trade_due(Some(last), last + Duration::from_secs(300), 300, false));
        assert!(trade_due(Some(last), last + Duration::from_secs(301), 300, false));
        assert!(trade_due(Some(last), last, 300, true));
    }

    #[test]
    fn abnormal_ratio_is_rehearsal_only() {
        let mut role = Role { ok: 19, fail: 2, consec: 1, ..Role::default() };
        assert!(role.abnormal(10, true));
        assert!(!role.abnormal(10, false));
        role.consec = 10;
        assert!(role.abnormal(10, true));
        assert!(role.abnormal(10, false));
        role = Role { ok: 18, fail: 1, ..Role::default() };
        assert!(!role.abnormal(10, true)); // Fewer than 20 attempts.
        role = Role { ok: 19, fail: 1, ..Role::default() };
        assert!(!role.abnormal(10, true)); // Exactly 5% is allowed.
    }

    #[test]
    fn keeper_transport_vs_program_errors() {
        for error in ["Error processing Instruction 3: custom program error: 0x17",
            "TransactionError(InstructionError(3, ComputationalBudgetExceeded))",
            r#"{"InstructionError":[3,{"Custom":23}]}"#] {
            assert!(is_program_error(error), "{error}");
        }
        for error in ["-32002: Transaction simulation failed: Blockhash not found",
            "request timed out", "error sending request", "unable to confirm transaction"] {
            assert!(!is_program_error(error), "{error}");
        }
        let role = Role { transport_errors: 100, ..Role::default() };
        assert!(!role.abnormal(5, true));
        assert!(role.json().contains("\"transport_errors\":100"));
    }

    fn cfg() -> launch::MatcherCfg {
        launch::MatcherCfg { program_id: Pubkey::new_unique().to_string(), size_q: 1, spread_bps: 10, fee_bps: 12, max_fill_notional_usd: 100_000, max_lp_inventory_usd: 100_000 }
    }
    #[test]
    fn cap_uses_higher_price() {
        assert_eq!(fill_allowed(1_000_000_000, 100_000_000, 100_000_001, 0, &cfg()), Err("max_fill_notional"));
        assert_eq!(fill_allowed(1_000_000_000, 100_000_001, 100_000_000, 0, &cfg()), Err("max_fill_notional"));
        assert_eq!(fill_allowed(-1, 100_000_000, 100_000_001, 999_999_999, &cfg()), Err("max_lp_inventory"));
    }
    #[test]
    fn scaled_and_reset_inventory() {
        use percolator::{V16PodI128, V16PodU128, V16PodU32, V16PodU64};
        let mut a = AssetStateV16::default();
        a.a_long = ADL_ONE / 2;
        a.epoch_long = 2;
        a.a_short = ADL_ONE;
        a.epoch_short = 3;
        let leg = PortfolioLegV16Account {
            active: 1, asset_index: V16PodU32::new(0), side: 0,
            basis_pos_q: V16PodI128::new(5), a_basis: V16PodU128::new(ADL_ONE),
            epoch_snap: V16PodU64::new(2), ..Default::default()
        };
        let mut reset = leg;
        reset.side = 1;
        assert_eq!(net_q(&[leg, reset], 0, &a).unwrap(), 3); // ceil(5 / 2), reset short contributes zero
        reset.epoch_snap = V16PodU64::new(3);
        assert_eq!(net_q(&[leg, reset], 0, &a).unwrap(), -2);
    }
    #[test]
    fn fill_limit_exact_and_one_atom_over() {
        assert_eq!(fill_allowed(1_000_000_000, 100_000_000, 100_000_000, 0, &cfg()), Ok(()));
        assert_eq!(fill_allowed(1_000_000_001, 100_000_000, 100_000_000, 0, &cfg()), Err("max_fill_notional"));
    }
    #[test]
    fn inventory_limit_and_reduction() {
        assert_eq!(fill_allowed(-1, 100_000_000, 100_000_000, 999_999_999, &cfg()), Ok(()));
        assert_eq!(fill_allowed(-1, 100_000_000, 100_000_000, 1_000_000_000, &cfg()), Err("max_lp_inventory"));
        assert_eq!(fill_allowed(1, 100_000_000, 100_000_000, 2_000_000_000, &cfg()), Ok(()));
        assert_eq!(fill_allowed(-1, 100_000_000, 100_000_000, -2_000_000_000, &cfg()), Ok(()));
    }
}

#[cfg(test)]
mod reject_kind_tests {
    use super::*;

    fn hr(error: &str, name: Option<&'static str>) -> submit::CrankOutcome {
        submit::CrankOutcome::HardReject { error: error.into(), error_name: name.map(Into::into), program_cu: None, tx_cu: None }
    }

    #[test]
    fn idle_rejects_do_not_count() {
        // EngineNonProgress is the keeper's normal idle answer, not a failure.
        let o = hr("custom program error: 0x16", Some("EngineNonProgress"));
        assert_eq!(o.class(), OutcomeClass::Idle);
        assert_eq!(reject_kind(&o), None);
    }

    #[test]
    fn definitive_rejects_are_program_failures_without_marker_text() {
        // A structured preflight failure whose text has no program-error marker.
        assert_eq!(reject_kind(&hr("RPC response error -32002: Transaction simulation failed", None)), Some(RejectKind::Program));
        assert_eq!(reject_kind(&hr("custom program error: 0x13", Some("EngineStale"))), Some(RejectKind::Program));
    }

    #[test]
    fn blockhash_not_found_is_transport() {
        assert_eq!(reject_kind(&hr("RPC response error -32002: Transaction simulation failed: Blockhash not found", None)), Some(RejectKind::Transport));
    }
}
