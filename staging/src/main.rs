//! Local staging market: SOL-PERP (one asset, SOL priced in USD, test USDC
//! with 6 decimals as collateral) on an AUTH_MARK oracle, with the operator
//! roles run as bots. Reference / dry-run only; not a paid live service;
//! M3 NO-GO. See docs/STAGING.md.
//!
//! Setup: test USDC mint, market (InitMarket, one asset), permissionless
//! resolve policy, ConfigureAuthMark, an LP and a trader portfolio with
//! deposits. Then one tick every `STAGING_TICK_MS`:
//!
//! 1. price pusher: `PushAuthMark` of the price source (`mock_sol_usd`) on a
//!    heartbeat or a deviation;
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
use percolator::{BackingBucketStatusV16 as Bucket, MarketModeV16, BOUND_SCALE};
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
    signature::{read_keypair_file, Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use std::{env, str::FromStr, time::Duration, time::Instant};

mod launch;
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

fn env_or<T: FromStr>(key: &str, default: T) -> T {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Wrapper error name when known, else the first 120 chars of the error.
fn short(e: &anyhow::Error) -> String {
    let msg = format!("{e:#}");
    submit::wrapper_error_name(&msg)
        .map(str::to_string)
        .unwrap_or_else(|| msg.chars().take(120).collect())
}

#[derive(Default)]
struct Role {
    ok: u64,
    fail: u64,
    consec: u64,
    max_consec: u64,
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
        format!("{{\"ok\":{},\"fail\":{},\"max_consec_fail\":{}}}", self.ok, self.fail, self.max_consec)
    }

    fn abnormal(&self, max_consec: u64) -> bool {
        let n = self.ok + self.fail;
        self.consec >= max_consec || (n >= 20 && self.fail * 20 > n)
    }
}

struct Staging {
    cfg: Config,
    rpc: RpcClient,
    program_id: Pubkey,
    payer: Keypair,
    /// Holds every market role (marketauth at InitMarket). On mainnet the
    /// Squads vault (`multisig`) does instead, and this key is unused.
    admin: Keypair,
    multisig: Option<Pubkey>,
    /// Launch run dir: unsigned multisig transactions go to `<run>/unsigned`.
    run_dir: Option<std::path::PathBuf>,
    /// Price source: this Pyth feed (SOL/USD) when set, else the seeded mock.
    pyth_feed: Option<[u8; 32]>,
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
    last_push_slot: u64,
    push_heartbeat_slots: u64,
    push_deviation_bps: u64,
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
        let bh = self.rpc.get_latest_blockhash()?;
        Ok(Transaction::new_signed_with_payer(&all, Some(&self.payer.pubkey()), &keys, bh))
    }

    fn send(&self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<()> {
        self.rpc.send_and_confirm_transaction(&self.tx(ixs, signers)?)?;
        Ok(())
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

    fn admin_pubkey(&self) -> Pubkey {
        self.multisig.unwrap_or_else(|| self.admin.pubkey())
    }

    fn admin_metas(&self) -> Vec<AccountMeta> {
        vec![AccountMeta::new(self.admin_pubkey(), true), AccountMeta::new(self.market, false)]
    }

    /// An authority transaction: signed by the local admin key, or, with a
    /// multisig, written unsigned for Squads (`launch::write_unsigned`) and
    /// the step stops until it lands.
    fn send_admin(&self, label: &str, ixs: &[Instruction]) -> Result<()> {
        match (self.multisig, &self.run_dir) {
            (Some(vault), Some(dir)) => launch::write_unsigned(&self.rpc, dir, label, vault, ixs),
            _ => self.send(ixs, &[&self.admin]).context(label.to_string()),
        }
    }

    /// admin, market, admin ATA, vault, token program: the top-up shape.
    fn funding_metas(&self) -> Vec<AccountMeta> {
        let mut m = self.admin_metas();
        m.push(AccountMeta::new(self.admin_ata, false));
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
    fn fresh_cranks(&self, data: &[u8], portfolios: &[Pubkey]) -> Result<Vec<Instruction>> {
        let obs = observation::fetch_observation_for_asset(data, 0, &self.pyth)?;
        let now = self.rpc.get_slot()?;
        let mut out = Vec::new();
        for pf in portfolios {
            let route = CrankRoute::Live { reward_portfolio: None };
            let (mut ixs, i) = submit::build_crank_instructions(
                &self.program_id, &self.payer.pubkey(), &self.market, pf, &route, &[obs.clone()], now,
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
            rpc,
            program_id,
            payer,
            admin,
            multisig: None,
            run_dir: None,
            pyth_feed: None,
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
            last_push_slot: 0,
            push_heartbeat_slots: env_or("STAGING_PUSH_HEARTBEAT_SLOTS", o.push_heartbeat_slots),
            push_deviation_bps: env_or("STAGING_PUSH_DEVIATION_BPS", o.push_deviation_bps),
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
        self.init_market()?;
        self.configure_oracle()?;
        self.lp_pf = setup::create_portfolio(&self.rpc, &self.payer, &self.lp, &self.market, self.program_id)?;
        self.trader_pf = setup::create_portfolio(&self.rpc, &self.payer, &self.trader, &self.market, self.program_id)?;
        self.deposit(&self.lp, &self.lp_pf, self.cfg.amounts.lp_deposit)?;
        self.deposit(&self.trader, &self.trader_pf, self.cfg.amounts.trader_deposit)?;
        (self.open, self.rolling, self.phase, self.resolved_ticks) = (None, false, Phase::Live, 0);
        Ok(())
    }

    /// The market account (created when absent) and its collateral vault.
    fn create_market(&mut self, market: &Keypair) -> Result<()> {
        self.set_market(market.pubkey());
        if !self.exists(&self.market)? {
            let len = state::market_account_len_for_capacity(1).map_err(|e| anyhow!("{e:?}"))?;
            setup::create_program_owned_account(&self.rpc, &self.payer, market, len, &self.program_id)?;
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

    /// InitMarket (`[market]` in the config), unless already initialized.
    fn init_market(&self) -> Result<()> {
        if self.read().is_ok() {
            return Ok(());
        }
        let mut metas = self.admin_metas();
        metas.push(AccountMeta::new_readonly(self.mint, false));
        self.send_admin("InitMarket", &[self.prog_ix(self.cfg.market.init_ix(self.mark)?, metas)])
    }

    /// Permissionless-resolve policy and AuthMark, each unless already set
    /// (their control sequences start at 0). One authority tx, so one
    /// multisig round on mainnet.
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
            self.last_push_slot = now;
        }
        Ok(())
    }

    // ------------------------------------------------------------ roles

    /// The pusher's price source: SOL/USD e6 and its timestamp (unix ms).
    /// Pyth (on-chain price account, `launch::pyth_sol_usd`) when configured;
    /// else a seeded mock path, one step of at most `max_bps`, kept within
    /// +-5% of the configured initial price, timestamped when generated.
    fn source_price(&mut self, max_bps: u64) -> Result<(u64, u64)> {
        if let Some(feed) = &self.pyth_feed {
            return launch::pyth_sol_usd(&self.rpc, feed, &self.cfg.oracle);
        }
        let p0 = self.cfg.market.initial_price;
        let step = (self.next_rand() % (2 * max_bps + 1)) as i128 - max_bps as i128;
        let next = self.feed as i128 * (10_000 + step) / 10_000;
        self.feed = (next as u64).clamp(p0 * 95 / 100, p0 * 105 / 100);
        Ok((self.feed, launch::now_ms()))
    }

    /// `force`: the matcher opens this tick; a risk-increasing fill is
    /// refused (EngineLockActive) unless the mark was pushed just before.
    fn push_price(&mut self, force: bool) -> Result<Option<String>> {
        let (_, g, s) = self.read()?;
        let now = self.rpc.get_slot()?;
        // The effective price walks at most max_price_move_bps_per_slot
        // toward the mark; the mock keeps each step inside what it can follow.
        let max_bps = (g.config.max_price_move_bps_per_slot * now.saturating_sub(self.last_push_slot).max(1)).min(50);
        let (mark, source_ms) = self.source_price(max_bps)?;
        // Push rule of Pyth's price_pusher (pattern only): push when the
        // heartbeat has elapsed or the price moved past the deviation.
        let moved_bps = mark.abs_diff(self.mark) * 10_000 / self.mark;
        if !force && now.saturating_sub(self.last_push_slot) < self.push_heartbeat_slots && moved_bps < self.push_deviation_bps {
            return Ok(None);
        }
        // Mirrors v16_svm.rs `push_auth_mark` (upstream test support).
        let ix = P::PushAuthMark {
            asset_index: 0,
            market_id: g.assets[0].market_id,
            now_slot: now,
            mark_e6: mark,
            observation_sequence: s.oracle_observation + 1,
            authority_epoch: s.authority_epoch,
        };
        // Mark lag: source price time to the push landing (processed, polled
        // every 50 ms); then wait for confirmed as the other roles do.
        // No preflight: one RPC round trip less on the latency path; a
        // failed push still surfaces through its status below.
        let tx = self.tx(&[self.prog_ix(ix, self.admin_metas())], &[&self.admin])?;
        let no_preflight = RpcSendTransactionConfig { skip_preflight: true, ..Default::default() };
        let sig = self.rpc.send_transaction_with_config(&tx, no_preflight)?;
        let mut lag = None;
        let t0 = Instant::now();
        loop {
            let st = self.rpc.get_signature_status_with_commitment(&sig, CommitmentConfig::processed())?;
            if let Some(r) = st {
                r?;
                lag.get_or_insert(launch::now_ms().saturating_sub(source_ms));
                if self.rpc.confirm_transaction(&sig)? {
                    break;
                }
            }
            if t0.elapsed() > Duration::from_secs(30) {
                bail!("PushAuthMark {sig} not confirmed in 30 s");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let lag = lag.unwrap_or_default();
        if self.keeper_log {
            println!(
                "{{\"event\":\"mark_lag\",\"ms\":{lag},\"target_ms\":{},\"at_ms\":{}}}",
                self.cfg.oracle.lag_target_ms,
                launch::now_ms()
            );
        }
        self.mark = mark;
        self.last_push_slot = now;
        Ok(Some(format!("ok({lag}ms)")))
    }

    fn match_trade(&mut self) -> Result<Option<String>> {
        let (data, g, _) = self.read()?;
        if self.rolling && self.open.is_none() {
            return Ok(None);
        }
        // A bucket just past expiry refuses fills (EngineStale) until the
        // keeper's crank of a claim holder expires it: skip this tick.
        let now = self.rpc.get_slot()?;
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
        let mark = g.assets[0].effective_price;
        let half = mark * self.cfg.matcher.spread_bps / 10_000;
        let price = if q > 0 { mark + half } else { mark - half };
        let tb = binding::portfolio_binding(&self.rpc.get_account_data(&self.trader_pf)?)?;
        let lb = binding::portfolio_binding(&self.rpc.get_account_data(&self.lp_pf)?)?;
        let mut ixs = self.fresh_cranks(&data, &[self.trader_pf, self.lp_pf])?;
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
        self.send(&ixs, &[&self.trader, &self.lp])?;
        self.open = if self.open.is_some() { None } else { Some(q) };
        Ok(Some(format!("{}{}", if q > 0 { "+" } else { "-" }, q.unsigned_abs())))
    }

    fn manage_backing(&mut self) -> Result<Option<String>> {
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
            ixs.push(self.prog_ix(ix, self.funding_metas()));
        }
        self.send_admin("TopUpBackingBucket", &ixs)
    }

    fn withdraw_backing(&self, data: &[u8], d: usize, amount: u128) -> Result<()> {
        let (_, g, s) = self.read()?;
        let mut ixs = self.fresh_cranks(data, &[self.trader_pf, self.lp_pf])?;
        let mut metas = self.admin_metas();
        metas.extend([
            AccountMeta::new(self.admin_ata, false),
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
        self.send(&ixs, &[&self.admin]).context("WithdrawBackingBucket")
    }

    fn top_up_insurance(&self, amount: u64) -> Result<Option<String>> {
        let (_, g, s) = self.read()?;
        // Mirrors v16_svm.rs `top_up_insurance`.
        let ix = P::TopUpInsurance {
            market_id: g.assets[0].market_id,
            intent_id: s.insurance_top_up + 1,
            authority_epoch: s.authority_epoch,
            amount: amount as u128,
        };
        self.send_admin("TopUpInsurance", &[self.prog_ix(ix, self.funding_metas())])?;
        Ok(Some(format!("+{amount}")))
    }
}

impl Staging {
    // ------------------------------------------------------------ lifecycle

    /// Live -> Recovery: SHUTDOWN of SOL (asset 0). Fresh cranks go first:
    /// shutdown accrues committed funding up to now.
    fn shutdown(&mut self) -> Result<Option<String>> {
        let (data, g, s) = self.read()?;
        let mut ixs = self.fresh_cranks(&data, &[self.trader_pf, self.lp_pf])?;
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
        self.send(&ixs, &[&self.admin]).context("SHUTDOWN")?;
        self.phase = Phase::Recovery;
        Ok(Some("SOL shut down (Recovery)".into()))
    }

    /// Recovery -> Resolved: ResolveMarket by the market authority.
    fn resolve(&mut self) -> Result<Option<String>> {
        let (data, g, s) = self.read()?;
        let mut ixs = self.fresh_cranks(&data, &[self.trader_pf, self.lp_pf])?;
        // Mirrors v16_svm.rs `resolve_market`.
        let ix = P::ResolveMarket { asset_generation_frontier: g.next_market_id, authority_epoch: s.authority_epoch };
        ixs.push(self.prog_ix(ix, self.admin_metas()));
        self.send(&ixs, &[&self.admin]).context("ResolveMarket")?;
        self.phase = Phase::Resolved;
        Ok(Some("market resolved".into()))
    }

    /// Resolved -> relisted: once the keeper's resolved route has closed both
    /// portfolios (paid out to the owners), open a fresh SOL-PERP market.
    fn relist_when_closed(&mut self) -> Result<Option<String>> {
        self.resolved_ticks += 1;
        // Read errors fail the step: only a confirmed-absent account or a
        // read, settled portfolio counts as closed.
        let closed = |pf: &Pubkey| -> Result<bool> {
            match self.rpc.get_account_with_commitment(pf, CommitmentConfig::confirmed())?.value {
                None => Ok(true),
                Some(a) => {
                    let p = read_portfolio(&a.data).map_err(|e| anyhow!("read_portfolio {pf}: {e:?}"))?;
                    Ok(p.capital.get() == 0 && p.pnl.get() == 0)
                }
            }
        };
        if !(closed(&self.lp_pf)? && closed(&self.trader_pf)?) {
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

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run(r2_setup()?, false),
        Some("step") => launch::step_main(&args[1..]),
        Some("bots") => run(launch::bots(&args[1..])?, true),
        Some(m) => bail!("unknown mode {m}; usage: staging [step <name> | bots] <config> <cluster> <run dir>"),
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
    Staging::setup(cfg, rpc, program_id, payer)
}

/// The bots loop. `bots`: an existing launch market, run until stopped
/// (no lifecycle); else the R2 run of `STAGING_HOURS` with its lifecycle.
fn run(mut st: Staging, bots: bool) -> Result<()> {
    let hours: f64 = env_or("STAGING_HOURS", if bots { 24.0 * 365.0 } else { 6.0 });
    let tick_ms: u64 = env_or("STAGING_TICK_MS", st.cfg.oracle.push_interval_ms);
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

    let budget = CycleBudget::from_env();
    let mut quarantine = Quarantine::new(QuarantineConfig::from_env());
    let mut cu = CuStats::default();
    let (mut push, mut trade, mut backing, mut insurance) = (Role::default(), Role::default(), Role::default(), Role::default());
    let mut lifecycle = Role::default();
    // Default: halfway through the run. 0 turns the lifecycle off.
    let lifecycle_tick: u64 = if bots { 0 } else { env_or("STAGING_LIFECYCLE_TICK", (hours * 1_800_000.0 / tick_ms as f64) as u64) };
    let mut keeper = Keeper::default();
    let started = Instant::now();
    let deadline = started + Duration::from_secs_f64(hours * 3600.0);
    let mut tick = 0u64;
    let mut abnormal = false;
    while Instant::now() < deadline && !abnormal {
        tick += 1;
        let t0 = Instant::now();
        let l = lifecycle.record(match st.phase {
            // From its tick on, SHUTDOWN is retried until it lands.
            Phase::Live if lifecycle_tick != 0 && tick >= lifecycle_tick && !st.relisted => st.shutdown(),
            Phase::Recovery => st.resolve(),
            Phase::Resolved => st.relist_when_closed(),
            Phase::Live => Ok(None),
        });
        let live = st.phase == Phase::Live;
        let p = if live { push.record(st.push_price(st.open.is_none() && !st.rolling)) } else { "-".into() };
        let t = if live { trade.record(st.match_trade()) } else { "-".into() };
        let b = if live { backing.record(st.manage_backing()) } else { "-".into() };
        let i = if live && tick % insurance_every == 0 { insurance.record(st.top_up_insurance(st.cfg.amounts.insurance_topup)) } else { "-".into() };

        let ctx = CrankCtx {
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
        let k = if st.phase == Phase::Recovery { "-".into() } else { keeper.role.record(
            cycle::run_one_cycle(&ctx, tick, &budget, &mut quarantine, &mut records, &mut cu).and_then(|s| {
                keeper.progressed += s.progressed as u64;
                keeper.idle += s.idle as u64;
                keeper.failed += s.failed as u64;
                if st.keeper_log {
                    launch::print_keeper_lines(&records, &s.line());
                }
                // A failed crank tx or an RPC error inside an attempt that
                // also progressed is classed Progress by the keeper; staging
                // still counts it as a keeper failure.
                let failed_txs = records.iter().filter(|r| r.outcome.class() == OutcomeClass::Failed).count();
                if s.failed > 0 || failed_txs > 0 || !s.unconfirmed.is_empty() || !s.drain_rpc_errors.is_empty() {
                    bail!(
                        "{} failed ({failed_txs} failed txs), {} unconfirmed, rpc after progress {:?}: {:?}",
                        s.failed,
                        s.unconfirmed.len(),
                        s.drain_rpc_errors,
                        s.failures
                    );
                }
                Ok(Some(format!("p{}/i{}", s.progressed, s.idle)))
            }),
        ) };
        let l = if l == "-" { String::new() } else { format!(" lifecycle={l}") };
        println!("t{tick} SOL={} push={p} trade={t} backing={b} insurance={i} keeper={k}{l}", st.mark);

        let roles = [("push", &push), ("trade", &trade), ("backing", &backing), ("insurance", &insurance), ("lifecycle", &lifecycle)];
        abnormal = roles.iter().any(|(_, r)| r.abnormal(max_consec)) || keeper.role.abnormal(max_consec);
        if tick % summary_every == 0 {
            println!("{}", summary(tick, started, &st, &roles, &keeper, false));
        }
        if let Some(rest) = Duration::from_millis(tick_ms).checked_sub(t0.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    let roles = [("push", &push), ("trade", &trade), ("backing", &backing), ("insurance", &insurance), ("lifecycle", &lifecycle)];
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
