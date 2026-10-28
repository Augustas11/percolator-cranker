//! `live_shaped`: a deterministic, live-shaped load run on a local validator.
//!
//! Shape: N assets on AuthMark oracles, ~40 trader portfolios, one LP per
//! asset. Every tick is (scenario actions) + one real `cycle::run_one_cycle`
//! over every portfolio of the market. Phases, by fraction of `CRANKER_CYCLES`:
//!
//! | phase    | what the scenario does                                          |
//! |----------|-----------------------------------------------------------------|
//! | open     | portfolios, deposits, trades vs LPs, flat marks                 |
//! | stress   | seeded price path per asset (up / down / reversal), backing     |
//! |          | top-ups that lapse or get liened, deposit top-ups, re-opens     |
//! | recovery | SHUTDOWN of the last asset, ForceCloseAbandonedAsset pairs, the  |
//! |          | position-epoch re-read check                                    |
//! | resolve  | marks settle to the index, crank+ResolveMarket in one tx        |
//! | exits    | CloseResolved / ClaimResolvedPayoutTopup / ClosePortfolio, then  |
//! |          | backing + insurance withdrawals and CloseSlab                    |
//!
//! The cranker is never special-cased: it sees only discovered portfolios.
//! Scenario actions are best-effort where a live market would also reject
//! them (a re-open into a reset side, say); those are counted, not fatal.
//! Lifecycle steps the assertions depend on are retried and, if they never
//! land, reported as not achieved.

use anyhow::{anyhow, bail, Context, Result};
use percolator::{
    AssetLifecycleV16, BackingBucketStatusV16, MarketModeV16, PortfolioAccountV16Account,
    BOUND_SCALE, MAX_VAULT_TVL, POS_SCALE,
};
use percolator_prog::{
    processor::ASSET_ACTION_SHUTDOWN,
    ix::Instruction as P,
    state::{
        self, read_asset_control_sequences, read_market, read_portfolio, AssetControlSequencesV16,
        MarketGroupV16, WrapperConfigV16,
    },
};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair, Signer},
    system_instruction,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;
use std::time::Instant;

use super::chain::{token_amount, Chain, TxError};
use super::rng::SplitMix64;
use crate::binding::{self, BindingSnapshot};
use crate::cycle::{self, CrankCtx, CuStats, SubmissionRecord};
use crate::observation;
use crate::quarantine::{Quarantine, QuarantineConfig};
use crate::scheduler::CycleBudget;
use crate::setup;
use crate::submit::{self, CrankOutcome, OutcomeClass};

const RPC_URL: &str = "http://127.0.0.1:8899";
const PRICE0: u64 = 1_000_000;
const DEFAULT_SEED: u64 = 0x5EED_1A7E_2026_1028;
/// Vault dust bound per (asset, portfolio): engine rounding is floor per
/// account per asset, a few atoms at most. Documented in the report.
const DUST_ATOMS_PER_ASSET_ACCOUNT: u64 = 4;
const FORCE_CLOSE_DELAY_SLOTS: u64 = 10;
/// Large so asset 0 never goes permissionless-stale during the run.
const STALE_SLOTS: u64 = 100_000;

#[derive(Debug, Clone)]
pub struct ScenarioConfig {
    pub seed: u64,
    pub traders: usize,
    pub assets: usize,
    pub cycles: u64,
    pub program_id: Pubkey,
}

impl ScenarioConfig {
    pub fn from_env() -> Result<Self> {
        let get = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
        let program_id = std::env::var("PERCOLATOR_PROGRAM_ID")
            .context("PERCOLATOR_PROGRAM_ID must be set")?;
        let assets = get("CRANKER_SCENARIO_ASSETS").unwrap_or(3) as usize;
        if !(3..=14).contains(&assets) {
            bail!("CRANKER_SCENARIO_ASSETS must be 3..=14 (got {assets})");
        }
        let traders = get("CRANKER_SCENARIO_TRADERS").unwrap_or(40) as usize;
        if traders < 4 {
            bail!("CRANKER_SCENARIO_TRADERS must be >= 4");
        }
        let cycles = get("CRANKER_CYCLES").unwrap_or(200);
        if cycles < 40 {
            bail!("CRANKER_CYCLES must be >= 40 for the phase plan (got {cycles})");
        }
        Ok(Self {
            seed: get("CRANKER_SCENARIO_SEED").unwrap_or(DEFAULT_SEED),
            traders,
            assets,
            cycles,
            program_id: Pubkey::from_str(&program_id).context("parse PERCOLATOR_PROGRAM_ID")?,
        })
    }
}

/// Phase boundaries in cycles.
#[derive(Debug, Clone, Copy)]
struct Phases {
    topup: u64,
    stress_start: u64,
    stress_end: u64,
    settle: u64,
    resolve: u64,
}

impl Phases {
    fn new(c: u64) -> Self {
        let open_end = (c * 4 / 100).max(2);
        Self {
            topup: open_end,
            stress_start: open_end + 1,
            stress_end: c * 55 / 100,
            settle: c * 64 / 100,
            resolve: c * 68 / 100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Trader,
    Lp(u16),
    /// The cranker's own reward portfolio (payer-owned). Exits only.
    Reward,
}

struct Actor {
    name: String,
    owner: Keypair,
    portfolio: Pubkey,
    ata: Pubkey,
    role: Role,
    closed: bool,
    /// Owner ATA balance when the market resolved.
    ata_at_resolve: u64,
    /// capital + pnl when the market resolved (the claim estimate).
    equity_at_resolve: i128,
}

/// One assertion line in the report.
#[derive(Debug, Clone)]
pub struct Check {
    pub id: &'static str,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct ScenarioReport {
    pub seed: u64,
    pub traders: usize,
    pub assets: usize,
    pub cycles: u64,
    pub market: String,
    pub wall_secs: u64,
    pub progressed: usize,
    pub idle: usize,
    pub failed: usize,
    pub submissions: usize,
    pub cu: CuStats,
    pub liquidation_fee_cranks: usize,
    pub liquidatable_seen: usize,
    pub crank_leg_reductions: usize,
    pub buckets_expired: BTreeSet<usize>,
    pub buckets_impaired: BTreeSet<usize>,
    /// Live-mode domains seen Fresh past expiry: domain → (first, last) slot.
    pub lapsed_unnormalized_live: BTreeMap<usize, (u64, u64)>,
    pub recovery_assets: BTreeSet<usize>,
    pub market_recovery_seen: bool,
    pub funding_assets: BTreeSet<usize>,
    pub max_abs_f_delta: i128,
    pub netting_portfolios: usize,
    pub resolved_hints_sent: usize,
    pub quarantine_events: Vec<String>,
    pub max_fail_run: u64,
    pub fail_run_bound: u64,
    pub vault_after: u64,
    pub dust_bound: u64,
    pub close_slab: bool,
    /// Vault balance after every owner exit and every authority withdrawal,
    /// before CloseSlab's terminal sweep.
    pub vault_before_close_slab: u64,
    /// What CloseSlab swept to the market authority's ATA.
    pub swept_to_admin: u64,
    pub action_rejects: BTreeMap<String, usize>,
    pub notes: Vec<String>,
    pub checks: Vec<Check>,
}

impl ScenarioReport {
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        let mut line = |l: String| {
            s.push_str(&l);
            s.push('\n');
        };
        line("== live_shaped report ==".into());
        line(format!(
            "seed={:#x} traders={} assets={} cycles={} market={} wall={}s",
            self.seed, self.traders, self.assets, self.cycles, self.market, self.wall_secs
        ));
        line(format!(
            "cranker totals: submissions={} progressed={} idle={} failed={}",
            self.submissions, self.progressed, self.idle, self.failed
        ));
        line(format!(
            "cu: noop_min={:?} (tx {:?}) worst_ok={:?} worst_reject={:?}",
            self.cu.noop_min, self.cu.noop_tx, self.cu.worst_ok, self.cu.worst_reject
        ));
        line(format!(
            "liquidations: fee-paid cranks={} liquidatable-priority seen={} live crank leg reductions={}",
            self.liquidation_fee_cranks, self.liquidatable_seen, self.crank_leg_reductions
        ));
        line(format!(
            "backing buckets: expired domains={:?} impaired domains={:?}",
            self.buckets_expired, self.buckets_impaired
        ));
        line(format!(
            "lapsed-but-Fresh buckets in Live (domain → first..last slot seen): {:?}",
            self.lapsed_unnormalized_live
        ));
        line(format!(
            "recovery: assets={:?} market-wide Recovery seen={}",
            self.recovery_assets, self.market_recovery_seen
        ));
        line(format!(
            "funding: assets with moved F index={:?} max |ΔF|={}",
            self.funding_assets, self.max_abs_f_delta
        ));
        line(format!("reversal netting: portfolios with winning+losing legs={}", self.netting_portfolios));
        line(format!(
            "resolved: backing hints sent={}",
            self.resolved_hints_sent
        ));
        line(format!(
            "quarantine: events={} max consecutive-failure run={} cycles (bound {})",
            self.quarantine_events.len(),
            self.max_fail_run,
            self.fail_run_bound
        ));
        for e in &self.quarantine_events {
            line(format!("  {e}"));
        }
        line(format!(
            "vault: before CloseSlab={} atoms, after={} atoms (dust bound {}), close_slab={} swept_to_marketauth={}",
            self.vault_before_close_slab, self.vault_after, self.dust_bound, self.close_slab, self.swept_to_admin
        ));
        if !self.action_rejects.is_empty() {
            line("scenario action rejects (best-effort actions):".into());
            for (k, v) in &self.action_rejects {
                line(format!("  {k}: {v}"));
            }
        }
        for n in &self.notes {
            line(format!("note: {n}"));
        }
        line("assertions:".into());
        for c in &self.checks {
            line(format!(
                "  [{}] {}: {}",
                if c.passed { "PASS" } else { "FAIL" },
                c.id,
                c.detail
            ));
        }
        line(format!("RESULT: {}", if self.passed() { "PASS" } else { "FAIL" }));
        s
    }
}

// ---------------------------------------------------------------- decoding

fn read_mkt(rpc: &RpcClient, market: &Pubkey) -> Result<(Vec<u8>, WrapperConfigV16, MarketGroupV16)> {
    let data = rpc.get_account_data(market).context("read market")?;
    let (cfg, g) = read_market(&data).map_err(|e| anyhow!("read_market: {e:?}"))?;
    Ok((data, cfg, g))
}

fn seqs(data: &[u8], asset: usize) -> Result<AssetControlSequencesV16> {
    read_asset_control_sequences(data, asset).map_err(|e| anyhow!("control sequences: {e}"))
}

/// Active legs as (asset, signed basis_pos_q).
fn legs(p: &PortfolioAccountV16Account) -> Vec<(usize, i128)> {
    p.legs
        .iter()
        .filter(|l| l.active != 0)
        .map(|l| {
            let q = l.basis_pos_q.get().abs();
            (l.asset_index.get() as usize, if l.side == 0 { q } else { -q })
        })
        .collect()
}

/// Effective |position| after ADL scaling: |basis| · a_side / a_basis.
fn effective_abs(p: &PortfolioAccountV16Account, g: &MarketGroupV16, asset: usize) -> u128 {
    p.legs
        .iter()
        .filter(|l| l.active != 0 && l.asset_index.get() as usize == asset)
        .map(|l| {
            let a = &g.assets[asset];
            let a_side = if l.side == 0 { a.a_long } else { a.a_short };
            let basis = l.basis_pos_q.get().unsigned_abs();
            let ab = l.a_basis.get().max(1);
            basis.saturating_mul(a_side) / ab
        })
        .sum()
}

fn is_terminal(p: &PortfolioAccountV16Account) -> bool {
    p.capital.get() == 0 && p.pnl.get() == 0 && p.reserved_pnl.get() == 0 && legs(p).is_empty()
}

fn fetch_portfolios(
    rpc: &RpcClient,
    keys: &[Pubkey],
) -> Result<Vec<Option<(Vec<u8>, PortfolioAccountV16Account)>>> {
    let mut out = Vec::with_capacity(keys.len());
    for chunk in keys.chunks(100) {
        let accts = rpc
            .get_multiple_accounts_with_commitment(chunk, CommitmentConfig::confirmed())?
            .value;
        for a in accts {
            out.push(a.filter(|a| a.lamports > 0).and_then(|a| {
                read_portfolio(&a.data).ok().map(|p| (a.data, p))
            }));
        }
    }
    Ok(out)
}

fn mark_for(bps: i64) -> u64 {
    (PRICE0 as i128 * (10_000 + bps as i128) / 10_000) as u64
}

/// Market parameters. IM 10% / MM 5% so ~9x traders liquidate on a ~6%
/// adverse move. The index walks at most 20 bps/slot toward the pushed
/// mark, so a mark jump leaves mark != index for tens of slots → nonzero
/// funding. The 1_000 / 2_000 atom margin floors are what lets a nonzero
/// liquidation fee pass the engine's exact solvency-envelope check at tiny
/// notionals (with floors of 1 / 2 InitMarket fails with InvalidAccountData).
fn init_market_params(n: u16) -> P {
    P::InitMarket {
        max_portfolio_assets: n,
        h_min: 0,
        h_max: 10,
        initial_price: PRICE0,
        min_nonzero_mm_req: 1_000,
        min_nonzero_im_req: 2_000,
        maintenance_margin_bps: 500,
        initial_margin_bps: 1_000,
        max_trading_fee_bps: 100,
        trade_fee_base_bps: 0,
        liquidation_fee_bps: 50,
        liquidation_fee_cap: 10_000_000_000,
        min_liquidation_abs: 0,
        max_price_move_bps_per_slot: 20,
        max_accrual_dt_slots: 16,
        max_abs_funding_e9_per_slot: 10_000,
        min_funding_lifetime_slots: 16,
        max_account_b_settlement_chunks: 4,
        max_bankrupt_close_chunks: 4,
        max_bankrupt_close_lifetime_slots: 20,
        public_b_chunk_atoms: MAX_VAULT_TVL,
        maintenance_fee_per_slot: 0,
    }
}

// ---------------------------------------------------------------- scenario

struct World<'a> {
    cfg: ScenarioConfig,
    rpc: &'a RpcClient,
    chain: Chain<'a>,
    admin: Keypair,
    mint: Pubkey,
    market: Pubkey,
    vault: Pubkey,
    vault_authority: Pubkey,
    admin_ata: Pubkey,
    reward: Pubkey,
    actors: Vec<Actor>,
    report: ScenarioReport,
    /// Per-asset sign of the stress path (+1 up, -1 down), from the seed.
    dir: Vec<i64>,
    noise: Vec<Vec<i64>>,
    phases: Phases,
    shutdown_asset: usize,
}

impl<'a> World<'a> {
    fn reject(&mut self, what: &str, e: &TxError) {
        *self
            .report
            .action_rejects
            .entry(format!("{what}: {}", e.short()))
            .or_default() += 1;
    }

    fn slot(&self) -> Result<u64> {
        Ok(self.rpc.get_slot()?)
    }

    fn portfolio_keys(&self) -> Vec<Pubkey> {
        self.actors.iter().map(|a| a.portfolio).collect()
    }

    /// Stress-path offset in bps for `asset` at `cycle` (before noise).
    fn path_bps(&self, asset: usize, cycle: u64) -> i64 {
        let ph = self.phases;
        if cycle < ph.stress_start {
            return 0;
        }
        let span = (ph.stress_end - ph.stress_start).max(1) as f64;
        let p = ((cycle.min(ph.stress_end) - ph.stress_start) as f64 / span).min(1.0);
        let shape = match asset % 3 {
            // Trend then partial give-back: liquidations, then reversal.
            0 | 1 => {
                let v = if p < 0.4 {
                    1400.0 * p / 0.4
                } else if p < 0.7 {
                    1400.0
                } else {
                    1400.0 - 700.0 * (p - 0.7) / 0.3
                };
                if asset % 3 == 0 { v } else { -v }
            }
            // Up, then a full reversal through zero.
            _ => {
                if p < 0.35 {
                    900.0 * p / 0.35
                } else {
                    900.0 - 1500.0 * (p - 0.35) / 0.65
                }
            }
        };
        self.dir[asset] * shape as i64
    }

    fn mark(&self, asset: usize, cycle: u64) -> u64 {
        let c = cycle.min(self.phases.stress_end) as usize;
        let noise = if cycle >= self.phases.stress_start { self.noise[asset][c] } else { 0 };
        mark_for(self.path_bps(asset, cycle) + noise)
    }

    /// Domain that ends up backing winners on `asset` (the loser side's
    /// domain): long winners are backed via the short domain and vice versa.
    fn lien_domain(&self, asset: usize) -> usize {
        let peak = self.path_bps(asset, self.phases.stress_start + (self.phases.stress_end - self.phases.stress_start) / 2);
        if peak >= 0 { 2 * asset + 1 } else { 2 * asset }
    }

    // ------------------------------------------------------------ setup

    fn setup(&mut self) -> Result<()> {
        let rpc = self.rpc;
        let payer = self.chain.payer;
        let n = self.cfg.assets;
        setup::rpc_airdrop(rpc, &self.admin.pubkey(), 10_000_000_000)?;
        let mint_kp = Keypair::new();
        setup::create_spl_mint(rpc, payer, &mint_kp, &payer.pubkey(), 0)?;
        self.mint = mint_kp.pubkey();
        let market_kp = Keypair::new();
        let len = state::market_account_len_for_capacity(n).map_err(|e| anyhow!("{e:?}"))?;
        setup::create_program_owned_account(rpc, payer, &market_kp, len, &self.cfg.program_id)?;
        self.market = market_kp.pubkey();
        let (va, _) = Pubkey::find_program_address(&[b"vault", self.market.as_ref()], &self.cfg.program_id);
        self.vault_authority = va;
        self.vault = spl_associated_token_account::get_associated_token_address(&va, &self.mint);
        setup::create_ata(rpc, payer, &va, &self.mint)?;
        self.admin_ata = spl_associated_token_account::get_associated_token_address(&self.admin.pubkey(), &self.mint);
        setup::create_ata(rpc, payer, &self.admin.pubkey(), &self.mint)?;
        setup::mint_to(rpc, payer, &self.mint, &self.admin_ata, payer, 200_000_000)?;
        self.report.market = self.market.to_string();
        println!("market={} mint={} vault={}", self.market, self.mint, self.vault);

        let init = init_market_params(n as u16);
        let ix = self.chain.ix(
            init,
            vec![
                AccountMeta::new(self.admin.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(self.mint, false),
            ],
        );
        self.chain.send(&[ix], &[&self.admin]).map_err(|e| anyhow!("InitMarket: {e}"))?;

        // Liquidation cranker share: makes the reward portfolio a valid crank
        // tail and lets fee-paid liquidations show up as reward diffs.
        let (data, _, g) = read_mkt(rpc, &self.market)?;
        let s0 = seqs(&data, 0)?;
        let ix = self.chain.ix(
            P::UpdateLiquidationFeePolicy {
                cranker_share_bps: 5000,
                policy_sequence: s0.liquidation_fee + 1,
                authority_epoch: s0.authority_epoch,
            },
            self.admin_market_metas(),
        );
        self.chain.send(&[ix], &[&self.admin]).map_err(|e| anyhow!("UpdateLiquidationFeePolicy: {e}"))?;
        let (data, _, _) = read_mkt(rpc, &self.market)?;
        let s0 = seqs(&data, 0)?;
        let ix = self.chain.ix(
            P::ConfigurePermissionlessResolve {
                asset_generation_frontier: g.next_market_id,
                stale_slots: STALE_SLOTS,
                force_close_delay_slots: FORCE_CLOSE_DELAY_SLOTS,
                policy_sequence: s0.permissionless_resolve + 1,
                authority_epoch: s0.authority_epoch,
            },
            self.admin_market_metas(),
        );
        self.chain.send(&[ix], &[&self.admin]).map_err(|e| anyhow!("ConfigurePermissionlessResolve: {e}"))?;

        // AuthMark per asset.
        let (data, _, g) = read_mkt(rpc, &self.market)?;
        let now = self.slot()?;
        let mut ixs = Vec::new();
        for a in 0..n {
            let s = seqs(&data, a)?;
            ixs.push(self.chain.ix(
                P::ConfigureAuthMark {
                    asset_index: a as u16,
                    market_id: g.assets[a].market_id,
                    now_slot: now,
                    initial_mark_e6: PRICE0,
                    observation_sequence: s.oracle_observation + 1,
                    authority_epoch: s.authority_epoch,
                },
                self.admin_market_metas(),
            ));
        }
        self.chain.send(&ixs, &[&self.admin]).map_err(|e| anyhow!("ConfigureAuthMark: {e}"))?;

        // Cranker reward portfolio (payer-owned).
        self.reward = self.new_portfolios(1)?[0];
        let payer_kp = Keypair::from_bytes(&payer.to_bytes()).map_err(|e| anyhow!("{e}"))?;
        let payer_ata = spl_associated_token_account::get_associated_token_address(&payer.pubkey(), &self.mint);
        self.actors.push(Actor {
            name: "reward".into(),
            owner: payer_kp,
            portfolio: self.reward,
            ata: payer_ata,
            role: Role::Reward,
            closed: false,
            ata_at_resolve: 0,
            equity_at_resolve: 0,
        });

        // LPs then traders.
        let mut rng = SplitMix64::new(self.cfg.seed ^ 0xA11CE);
        let mut owners = Vec::new();
        for a in 0..n {
            owners.push((format!("lp{a}"), Keypair::new(), Role::Lp(a as u16), 200_000_000u64));
        }
        for t in 0..self.cfg.traders {
            owners.push((format!("t{t}"), Keypair::new(), Role::Trader, rng.range(200_000, 1_000_000)));
        }
        // Batched: [fund owner, create portfolio acct, ATA, mint, InitPortfolio].
        let plen = state::portfolio_account_len_for_market_slots(n).map_err(|e| anyhow!("{e:?}"))?;
        let rent = rpc.get_minimum_balance_for_rent_exemption(plen)?;
        let pkps: Vec<Keypair> = owners.iter().map(|_| Keypair::new()).collect();
        for chunk in (0..owners.len()).collect::<Vec<_>>().chunks(20) {
            let mut txs = Vec::new();
            for &i in chunk {
                let (_, owner, _, dep) = &owners[i];
                let ata = spl_associated_token_account::get_associated_token_address(&owner.pubkey(), &self.mint);
                let ixs = vec![
                    system_instruction::transfer(&payer.pubkey(), &owner.pubkey(), 50_000_000),
                    system_instruction::create_account(
                        &payer.pubkey(),
                        &pkps[i].pubkey(),
                        rent,
                        plen as u64,
                        &self.cfg.program_id,
                    ),
                    spl_associated_token_account::instruction::create_associated_token_account(
                        &payer.pubkey(),
                        &owner.pubkey(),
                        &self.mint,
                        &spl_token::ID,
                    ),
                    spl_token::instruction::mint_to(&spl_token::ID, &self.mint, &ata, &payer.pubkey(), &[], *dep)?,
                    self.chain.ix(
                        P::InitPortfolio,
                        vec![
                            AccountMeta::new(owner.pubkey(), true),
                            AccountMeta::new(self.market, false),
                            AccountMeta::new(pkps[i].pubkey(), false),
                        ],
                    ),
                ];
                txs.push((ixs, vec![owner, &pkps[i]]));
            }
            self.chain.send_batch_all("portfolio setup", &txs)?;
        }
        // Deposits (need the program-assigned portfolio_id).
        let keys: Vec<Pubkey> = pkps.iter().map(|k| k.pubkey()).collect();
        let fetched = fetch_portfolios(rpc, &keys)?;
        let mut txs = Vec::new();
        for (i, f) in fetched.iter().enumerate() {
            let (data, _) = f.as_ref().ok_or_else(|| anyhow!("portfolio {i} missing after init"))?;
            let b = binding::portfolio_binding(data)?;
            let (_, owner, _, dep) = &owners[i];
            txs.push((vec![self.deposit_ix(owner, &keys[i], b.portfolio_id, b.sequence, *dep as u128)], vec![owner]));
        }
        self.chain.send_batch_all("deposit", &txs)?;
        for (i, (name, owner, role, _)) in owners.into_iter().enumerate() {
            let ata = spl_associated_token_account::get_associated_token_address(&owner.pubkey(), &self.mint);
            self.actors.push(Actor {
                name,
                owner,
                portfolio: keys[i],
                ata,
                role,
                closed: false,
                ata_at_resolve: 0,
                equity_at_resolve: 0,
            });
        }
        println!("setup: {} portfolios (+reward) funded", self.actors.len() - 1);
        Ok(())
    }

    fn new_portfolios(&self, count: usize) -> Result<Vec<Pubkey>> {
        let plen = state::portfolio_account_len_for_market_slots(self.cfg.assets).map_err(|e| anyhow!("{e:?}"))?;
        let mut out = Vec::new();
        for _ in 0..count {
            let kp = Keypair::new();
            setup::create_program_owned_account(self.rpc, self.chain.payer, &kp, plen, &self.cfg.program_id)?;
            let ix = self.chain.ix(
                P::InitPortfolio,
                vec![
                    AccountMeta::new(self.chain.payer.pubkey(), true),
                    AccountMeta::new(self.market, false),
                    AccountMeta::new(kp.pubkey(), false),
                ],
            );
            self.chain.send(&[ix], &[]).map_err(|e| anyhow!("InitPortfolio: {e}"))?;
            out.push(kp.pubkey());
        }
        Ok(out)
    }

    fn admin_market_metas(&self) -> Vec<AccountMeta> {
        vec![
            AccountMeta::new(self.admin.pubkey(), true),
            AccountMeta::new(self.market, false),
        ]
    }

    fn deposit_ix(&self, owner: &Keypair, portfolio: &Pubkey, id: u64, seq: u64, amount: u128) -> Instruction {
        let ata = spl_associated_token_account::get_associated_token_address(&owner.pubkey(), &self.mint);
        self.chain.ix(
            P::Deposit {
                portfolio_id: id,
                expected_sequence: seq,
                amount,
            },
            vec![
                AccountMeta::new(owner.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(*portfolio, false),
                AccountMeta::new(ata, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
        )
    }

    fn payout_tail(&self, owner_ata: &Pubkey) -> Vec<AccountMeta> {
        vec![
            AccountMeta::new(*owner_ata, false),
            AccountMeta::new(self.vault, false),
            AccountMeta::new_readonly(self.vault_authority, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ]
    }

    fn lp_index(&self, asset: usize) -> usize {
        self.actors
            .iter()
            .position(|a| a.role == Role::Lp(asset as u16))
            .expect("one LP per asset")
    }

    fn trade_ix(
        &self,
        taker: usize,
        maker: usize,
        tb: binding::PortfolioBinding,
        mb: binding::PortfolioBinding,
        asset: usize,
        market_id: u64,
        size_q: i128,
        price: u64,
    ) -> Instruction {
        self.chain.ix(
            P::TradeNoCpi {
                account_a_portfolio_id: tb.portfolio_id,
                account_a_position_epoch: tb.position_epoch,
                account_b_portfolio_id: mb.portfolio_id,
                account_b_position_epoch: mb.position_epoch,
                asset_index: asset as u16,
                market_id,
                size_q,
                exec_price: price,
                fee_bps: 0,
                backing_fee_cap_bps: 0,
            },
            vec![
                AccountMeta::new(self.actors[taker].owner.pubkey(), true),
                AccountMeta::new(self.actors[maker].owner.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(self.actors[taker].portfolio, false),
                AccountMeta::new(self.actors[maker].portfolio, false),
            ],
        )
    }

    /// Open book: every trader takes 1-3 legs at ~4-9x total leverage
    /// against the LP of each asset. Lanes (one per LP) run in parallel,
    /// one trade per lane per round, a trader in at most one lane per round
    /// (each fill may move both parties' position epochs).
    fn open_book(&mut self) -> Result<()> {
        let n = self.cfg.assets;
        let mut rng = SplitMix64::new(self.cfg.seed ^ 0x0B00C);
        let mut lanes: Vec<Vec<(usize, i128)>> = vec![Vec::new(); n];
        for (i, a) in self.actors.iter().enumerate() {
            if a.role != Role::Trader {
                continue;
            }
            let k = match rng.range(0, 9) {
                0..=3 => 1,
                4..=7 => 2,
                _ => 3,
            };
            let mut assets: Vec<usize> = (0..n).collect();
            for j in (1..assets.len()).rev() {
                assets.swap(j, rng.range(0, j as u64) as usize);
            }
            // Deposit is the minted amount; read it back from the ATA mint
            // record is overkill, so use capital from chain below.
            let lev_x10 = rng.range(40, 90) as i128; // 4.0x .. 9.0x total
            let cap = {
                let d = self.rpc.get_account_data(&a.portfolio)?;
                read_portfolio(&d).map_err(|e| anyhow!("{e:?}"))?.capital.get() as i128
            };
            let total = cap * lev_x10 / 10;
            for &asset in assets.iter().take(k) {
                let notional = total / k as i128;
                let q = notional * POS_SCALE as i128 / PRICE0 as i128;
                let q = if rng.coin() { q } else { -q };
                lanes[asset].push((i, q));
            }
        }
        let (_, _, g) = read_mkt(self.rpc, &self.market)?;
        let mut cursor = vec![0usize; n];
        let mut opened = 0usize;
        loop {
            let mut used = BTreeSet::new();
            let mut round = Vec::new();
            for a in 0..n {
                if let Some(&(t, q)) = lanes[a].get(cursor[a]) {
                    if used.insert(t) {
                        round.push((a, t, q));
                        cursor[a] += 1;
                    }
                }
            }
            if round.is_empty() {
                break;
            }
            let mut keys = Vec::new();
            for &(a, t, _) in &round {
                keys.push(self.actors[t].portfolio);
                keys.push(self.actors[self.lp_index(a)].portfolio);
            }
            let fetched = fetch_portfolios(self.rpc, &keys)?;
            let mut txs = Vec::new();
            for (k, &(a, t, q)) in round.iter().enumerate() {
                let tb = binding::portfolio_binding(&fetched[2 * k].as_ref().unwrap().0)?;
                let mb = binding::portfolio_binding(&fetched[2 * k + 1].as_ref().unwrap().0)?;
                let lp = self.lp_index(a);
                let ix = self.trade_ix(t, lp, tb, mb, a, g.assets[a].market_id, q, PRICE0);
                txs.push((vec![ix], vec![&self.actors[t].owner, &self.actors[lp].owner]));
            }
            let res = self.chain.send_batch(&txs);
            for r in res {
                match r {
                    Ok(_) => opened += 1,
                    Err(e) => {
                        println!("open trade rejected: {e}");
                        self.reject("open-trade", &e);
                    }
                }
            }
        }
        println!("open book: {opened} trades filled");
        if opened == 0 {
            bail!("no trade filled at open; scenario cannot proceed");
        }
        Ok(())
    }

    /// One tx: PushAuthMark for every asset still pushable.
    fn push_marks(&mut self, cycle: u64, settle_to_index: bool) -> Result<()> {
        let (data, _, g) = read_mkt(self.rpc, &self.market)?;
        if g.mode != MarketModeV16::Live {
            return Ok(());
        }
        let now = self.slot()?;
        let mut ixs = Vec::new();
        for a in 0..self.cfg.assets {
            if g.assets[a].lifecycle != AssetLifecycleV16::Active {
                continue;
            }
            let mark = if settle_to_index {
                g.assets[a].effective_price
            } else {
                self.mark(a, cycle)
            };
            let s = seqs(&data, a)?;
            ixs.push(self.chain.ix(
                P::PushAuthMark {
                    asset_index: a as u16,
                    market_id: g.assets[a].market_id,
                    now_slot: now,
                    mark_e6: mark,
                    observation_sequence: s.oracle_observation + 1,
                    authority_epoch: s.authority_epoch,
                },
                self.admin_market_metas(),
            ));
        }
        if ixs.is_empty() {
            return Ok(());
        }
        if let Err(e) = self.chain.send(&ixs, &[&self.admin]) {
            println!("cycle {cycle}: PushAuthMark rejected: {e}");
            self.reject("push-mark", &e);
        }
        Ok(())
    }

    fn top_up_backing(&mut self, domain: usize, amount: u128, expiry_slot: u64) {
        let r = (|| -> Result<(), TxError> {
            let (data, _, g) = read_mkt(self.rpc, &self.market).map_err(txe)?;
            let a = domain / 2;
            let s = seqs(&data, a).map_err(txe)?;
            let prof = state::read_asset_oracle_profile(&data, a).map_err(|e| txe(anyhow!("{e:?}")))?;
            let (fee, share) = if domain % 2 == 0 {
                (prof.backing_trade_fee_bps_long, prof.backing_trade_fee_insurance_share_bps_long)
            } else {
                (prof.backing_trade_fee_bps_short, prof.backing_trade_fee_insurance_share_bps_short)
            };
            let ix = self.chain.ix(
                P::TopUpBackingBucket {
                    domain: domain as u16,
                    market_id: g.assets[a].market_id,
                    intent_id: s.backing_top_up + 1,
                    authority_epoch: s.authority_epoch,
                    backing_fee_bps: fee,
                    insurance_share_bps: share,
                    amount,
                    expiry_slot,
                },
                vec![
                    AccountMeta::new(self.admin.pubkey(), true),
                    AccountMeta::new(self.market, false),
                    AccountMeta::new(self.admin_ata, false),
                    AccountMeta::new(self.vault, false),
                    AccountMeta::new_readonly(spl_token::ID, false),
                ],
            );
            self.chain.send(&[ix], &[&self.admin]).map(|_| ())
        })();
        match r {
            Ok(()) => println!("top-up backing domain {domain}: {amount} atoms, expiry slot {expiry_slot}"),
            Err(e) => {
                println!("top-up backing domain {domain} rejected: {e}");
                self.reject("backing-top-up", &e);
            }
        }
    }

    fn withdraw_backing_ix(&self, domain: usize, market_id: u64, epoch: u64, amount: u128, earnings: bool) -> Instruction {
        let ix = if earnings {
            P::WithdrawBackingBucketEarnings {
                domain: domain as u16,
                market_id,
                authority_epoch: epoch,
                amount,
            }
        } else {
            P::WithdrawBackingBucket {
                domain: domain as u16,
                market_id,
                authority_epoch: epoch,
                amount,
            }
        };
        self.chain.ix(
            ix,
            vec![
                AccountMeta::new(self.admin.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(self.admin_ata, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new_readonly(self.vault_authority, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
        )
    }

    /// Largest amount in `1..=hi` that `build(amount)` would accept now
    /// (binary search by simulation), or 0.
    fn max_accepted(&self, hi: u128, build: impl Fn(u128) -> Instruction) -> u128 {
        if hi == 0 || self.chain.simulate(&[build(1)], &[&self.admin]).is_err() {
            return 0;
        }
        let (mut lo, mut hi) = (1u128, hi);
        while lo < hi {
            let mid = lo + (hi - lo + 1) / 2;
            if self.chain.simulate(&[build(mid)], &[&self.admin]).is_ok() {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    /// Withdraw what the engine allows from `domain` (principal or earnings).
    fn withdraw_backing(&mut self, domain: usize, cap: u128, earnings: bool) -> u128 {
        let Ok((data, _, g)) = read_mkt(self.rpc, &self.market) else { return 0 };
        let a = domain / 2;
        let Ok(s) = seqs(&data, a) else { return 0 };
        let mid = g.assets[a].market_id;
        let amt = self.max_accepted(cap, |x| self.withdraw_backing_ix(domain, mid, s.authority_epoch, x, earnings));
        if amt == 0 {
            return 0;
        }
        match self.chain.send(&[self.withdraw_backing_ix(domain, mid, s.authority_epoch, amt, earnings)], &[&self.admin]) {
            Ok(_) => {
                println!("withdraw backing{} domain {domain}: {amt}", if earnings { " earnings" } else { "" });
                amt
            }
            Err(e) => {
                self.reject("backing-withdraw", &e);
                0
            }
        }
    }

    /// Deposit top-ups for a few traders and small re-opens at the current
    /// index. Re-opens crank both parties first (fresh certs), like a UI
    /// would. All best-effort.
    fn stress_actions(&mut self, cycle: u64, rng: &mut SplitMix64) -> Result<()> {
        let traders: Vec<usize> = (0..self.actors.len()).filter(|&i| self.actors[i].role == Role::Trader).collect();
        for _ in 0..3 {
            let t = traders[rng.range(0, traders.len() as u64 - 1) as usize];
            let d = self.rpc.get_account_data(&self.actors[t].portfolio)?;
            let b = binding::portfolio_binding(&d)?;
            let amt = rng.range(50_000, 150_000);
            let ata = self.actors[t].ata;
            let mint_ix = spl_token::instruction::mint_to(&spl_token::ID, &self.mint, &ata, &self.chain.payer.pubkey(), &[], amt)?;
            let dep = self.deposit_ix(&self.actors[t].owner, &self.actors[t].portfolio, b.portfolio_id, b.sequence, amt as u128);
            match self.chain.send(&[mint_ix, dep], &[&self.actors[t].owner]) {
                Ok(_) => println!("cycle {cycle}: deposit top-up {} +{amt}", self.actors[t].name),
                Err(e) => self.reject("deposit-top-up", &e),
            }
        }
        let (_, _, g) = read_mkt(self.rpc, &self.market)?;
        for _ in 0..3 {
            let t = traders[rng.range(0, traders.len() as u64 - 1) as usize];
            let a = rng.range(0, self.cfg.assets as u64 - 2) as usize; // never the shutdown asset
            let lp = self.lp_index(a);
            self.pre_trade_crank(t, a)?;
            self.pre_trade_crank(lp, a)?;
            let f = fetch_portfolios(self.rpc, &[self.actors[t].portfolio, self.actors[lp].portfolio])?;
            let tb = binding::portfolio_binding(&f[0].as_ref().unwrap().0)?;
            let mb = binding::portfolio_binding(&f[1].as_ref().unwrap().0)?;
            let cap = f[0].as_ref().unwrap().1.capital.get() as i128;
            let px = g.assets[a].effective_price;
            let q = (cap * 2) * POS_SCALE as i128 / px as i128;
            let q = if rng.coin() { q } else { -q };
            let ix = self.trade_ix(t, lp, tb, mb, a, g.assets[a].market_id, q, px);
            match self.chain.send(&[ix], &[&self.actors[t].owner, &self.actors[lp].owner]) {
                Ok(_) => println!("cycle {cycle}: re-open {} asset {a} q={q} @ {px}", self.actors[t].name),
                Err(e) => {
                    println!("cycle {cycle}: re-open {} asset {a} rejected: {}", self.actors[t].name, e.short());
                    self.reject("re-open", &e);
                }
            }
        }
        Ok(())
    }

    /// Crank `i` with one hint per active leg asset plus `extra` (fresh
    /// health cert before a trade). Best-effort.
    fn pre_trade_crank(&self, i: usize, extra: usize) -> Result<()> {
        let now = self.slot()?;
        let pyth = observation::empty_pyth_resolver();
        let (mdata, _, _) = read_mkt(self.rpc, &self.market)?;
        let pd = self.rpc.get_account_data(&self.actors[i].portfolio)?;
        let mut assets: Vec<u16> = read_portfolio(&pd)
            .map(|pf| legs(&pf).iter().map(|(a, _)| *a as u16).collect())
            .unwrap_or_default();
        if !assets.contains(&(extra as u16)) {
            assets.push(extra as u16);
        }
        assets.sort_unstable();
        let hints = assets
            .into_iter()
            .map(|x| observation::fetch_observation_for_asset(&mdata, x, &pyth))
            .collect::<Result<Vec<_>>>()?;
        let _ = submit::submit_crank(
            self.rpc,
            &self.chain.program_id,
            self.chain.payer,
            &self.market,
            &self.actors[i].portfolio,
            None,
            &hints,
            now,
        );
        Ok(())
    }

    /// Winners add to their winning leg past what capital alone margins, so
    /// the engine liens fresh counterparty backing for the positive-PnL
    /// credit (`create_initial_margin_source_lien_if_needed`). When that
    /// bucket later lapses with the lien outstanding it goes Impaired.
    fn winner_reopen(&mut self, cycle: u64) -> Result<()> {
        let (_, _, g) = read_mkt(self.rpc, &self.market)?;
        let keys = self.portfolio_keys();
        for a in 0..self.cfg.assets.min(3) {
            if a == self.shutdown_asset {
                continue;
            }
            let px = g.assets[a].effective_price;
            let long_wins = px > PRICE0;
            let lp = self.lp_index(a);
            let f = fetch_portfolios(self.rpc, &keys)?;
            let mut winners: Vec<(usize, i128)> = f
                .iter()
                .enumerate()
                .filter(|(i, _)| self.actors[*i].role == Role::Trader)
                .filter_map(|(i, x)| {
                    let (_, p) = x.as_ref()?;
                    legs(p).iter().find(|(as_, q)| *as_ == a && (*q > 0) == long_wins).map(|_| (i, p.capital.get() as i128))
                })
                .collect();
            winners.truncate(4);
            let mut filled = 0;
            for (t, _) in winners {
                self.pre_trade_crank(t, a)?;
                self.pre_trade_crank(lp, a)?;
                let fr = fetch_portfolios(self.rpc, &[self.actors[t].portfolio, self.actors[lp].portfolio])?;
                let (Some((td, tp)), Some((md, _))) = (&fr[0], &fr[1]) else { continue };
                let tb = binding::portfolio_binding(td)?;
                let mb = binding::portfolio_binding(md)?;
                let equity = tp.capital.get() as i128 + tp.pnl.get().max(0);
                let notional: i128 = legs(tp)
                    .iter()
                    .map(|(x, q)| q.abs() * g.assets[*x].effective_price as i128 / POS_SCALE as i128)
                    .sum();
                let add = equity * 95 / 10 - notional;
                if add <= 0 {
                    continue;
                }
                let mut q = add * POS_SCALE as i128 / px as i128;
                for _ in 0..6 {
                    let sq = if long_wins { q } else { -q };
                    let ix = self.trade_ix(t, lp, tb, mb, a, g.assets[a].market_id, sq, px);
                    match self.chain.send(&[ix], &[&self.actors[t].owner, &self.actors[lp].owner]) {
                        Ok(_) => {
                            println!("cycle {cycle}: winner add {} asset {a} q={sq} @ {px} (pnl {})", self.actors[t].name, tp.pnl.get());
                            filled += 1;
                            break;
                        }
                        Err(e) => {
                            println!("cycle {cycle}: winner add {} asset {a} q={sq} rejected: {}", self.actors[t].name, e.short());
                            self.reject("winner-add", &e);
                            q /= 2;
                        }
                    }
                }
            }
            let (_, _, g2) = read_mkt(self.rpc, &self.market)?;
            let d = self.lien_domain(a);
            let b = g2.source_backing_buckets[d];
            println!(
                "cycle {cycle}: asset {a} winner adds={filled}; domain {d} status={:?} fresh_unliened={} valid_liened={} expiry={}",
                b.status, b.fresh_unliened_backing_num, b.valid_liened_backing_num, b.expiry_slot
            );
        }
        Ok(())
    }

    /// One tx: crank `lp` for `asset` (accrues the asset to now), then `tail`.
    fn crank_then(&self, crank_portfolio: &Pubkey, hint_assets: &[u16], tail: Instruction, signers: &[&Keypair]) -> Result<(), TxError> {
        let now = self.slot().map_err(txe)?;
        let crank = self.chain.ix(
            P::PermissionlessCrank {
                now_slot: now,
                observations: hint_assets
                    .iter()
                    .map(|&a| percolator_prog::ix::CrankObservationHint { asset_index: a, oracle_accounts: 0 })
                    .collect(),
            },
            vec![
                AccountMeta::new(self.chain.payer.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(*crank_portfolio, false),
            ],
        );
        self.chain.send(&[crank, tail], signers).map(|_| ())
    }

    fn shutdown_asset(&mut self, cycle: u64) -> Result<bool> {
        let a = self.shutdown_asset;
        let (data, _, g) = read_mkt(self.rpc, &self.market)?;
        if g.assets[a].lifecycle == AssetLifecycleV16::Recovery {
            return Ok(true);
        }
        let s0 = seqs(&data, 0)?;
        let now = self.slot()?;
        let ix = self.chain.ix(
            P::UpdateAssetLifecycle {
                action: ASSET_ACTION_SHUTDOWN,
                asset_index: a as u16,
                market_id: g.assets[a].market_id,
                authority_epoch: s0.authority_epoch,
                now_slot: now,
                initial_price: 0,
                max_init_fee: 0,
                insurance_authority: [0; 32],
                insurance_operator: [0; 32],
                backing_bucket_authority: [0; 32],
                oracle_authority: [0; 32],
            },
            self.admin_market_metas(),
        );
        // Shutdown accrues committed funding first and rejects a gap larger
        // than max_accrual_dt_slots, so crank the asset's LP in the same tx.
        let lp = self.actors[self.lp_index(a)].portfolio;
        let r = self
            .crank_then(&lp, &[a as u16], ix.clone(), &[&self.admin])
            .or_else(|e| {
                println!("cycle {cycle}: crank+shutdown rejected ({}), trying shutdown alone", e.short());
                self.chain.send(&[ix], &[&self.admin]).map(|_| ())
            });
        match r {
            Ok(()) => {
                println!("cycle {cycle}: asset {a} SHUTDOWN → Recovery");
                Ok(true)
            }
            Err(e) => {
                println!("cycle {cycle}: shutdown rejected: {e}");
                self.reject("shutdown", &e);
                Ok(false)
            }
        }
    }

    fn force_close_ix(&self, asset: usize, long: &Pubkey, short: &Pubkey, q: u128) -> Result<Instruction> {
        Ok(self.chain.ix(
            P::ForceCloseAbandonedAsset {
                asset_index: asset as u16,
                now_slot: self.slot()?,
                close_q: q,
            },
            vec![
                AccountMeta::new(self.chain.payer.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(*long, false),
                AccountMeta::new(*short, false),
            ],
        ))
    }

    fn owner_ix(&self, i: usize, ix: P) -> Instruction {
        self.chain.ix(
            ix,
            vec![
                AccountMeta::new(self.actors[i].owner.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(self.actors[i].portfolio, false),
            ],
        )
    }

    /// Up to `max_pairs` ForceCloseAbandonedAsset calls on the shutdown
    /// asset. The first pair that includes a trader runs the epoch re-read
    /// check (assertion d). Returns true when no leg on the asset remains.
    fn force_close_round(&mut self, cycle: u64, max_pairs: usize, d_done: &mut Option<Check>) -> Result<bool> {
        let a = self.shutdown_asset;
        let (_, cfg, g) = read_mkt(self.rpc, &self.market)?;
        if g.mode != MarketModeV16::Live {
            return Ok(true);
        }
        // force_close_delay is measured from the shutdown slot.
        let prof = {
            let data = self.rpc.get_account_data(&self.market)?;
            state::read_asset_oracle_profile(&data, a).map_err(|e| anyhow!("{e:?}"))?
        };
        if self.slot()? < prof.last_good_oracle_slot + cfg.force_close_delay_slots {
            return Ok(false);
        }
        let keys = self.portfolio_keys();
        let fetched = fetch_portfolios(self.rpc, &keys)?;
        let mut longs = Vec::new();
        let mut shorts = Vec::new();
        for (i, f) in fetched.iter().enumerate() {
            let Some((_, p)) = f else { continue };
            for (asset, q) in legs(p) {
                if asset == a {
                    let eff = effective_abs(p, &g, a);
                    if q > 0 { longs.push((i, eff)) } else { shorts.push((i, eff)) }
                }
            }
        }
        if longs.is_empty() && shorts.is_empty() {
            return Ok(true);
        }
        // Prefer trader-vs-trader / trader-vs-LP pairs with a trader first,
        // so the d-check runs on a trader portfolio.
        longs.sort_by_key(|(i, _)| (self.actors[*i].role != Role::Trader, *i));
        shorts.sort_by_key(|(i, _)| (self.actors[*i].role != Role::Trader, *i));
        let mut done = 0;
        let (mut li, mut si) = (0, 0);
        // One-sided residue (no counterparty leg left, e.g. after the
        // d-check forfeit): the owner forfeits it. ForceClose's own residue
        // path only applies once that side's OI is already zero.
        if longs.is_empty() || shorts.is_empty() {
            for &(i, _) in longs.iter().chain(shorts.iter()).take(max_pairs) {
                let d = self.rpc.get_account_data(&self.actors[i].portfolio)?;
                let b = binding::portfolio_binding(&d)?;
                let ix = self.owner_ix(
                    i,
                    P::ForfeitRecoveryLeg {
                        portfolio_id: b.portfolio_id,
                        position_epoch: b.position_epoch,
                        asset_index: a as u16,
                        b_delta_budget: MAX_VAULT_TVL,
                    },
                );
                match self.chain.send(&[ix], &[&self.actors[i].owner]) {
                    Ok(_) => println!("cycle {cycle}: ForfeitRecoveryLeg asset {a} by {}", self.actors[i].name),
                    Err(e) => {
                        println!("cycle {cycle}: forfeit {} rejected: {}", self.actors[i].name, e.short());
                        self.reject("forfeit-residue", &e);
                    }
                }
            }
            return Ok(false);
        }
        while done < max_pairs && li < longs.len() && si < shorts.len() {
            let (long, short, q) = {
                let ((l, lq), (s, sq)) = (longs[li], shorts[si]);
                (l, s, lq.min(sq).max(1))
            };
            let do_d = d_done.is_none()
                && (self.actors[long].role == Role::Trader || self.actors[short].role == Role::Trader)
                && li < longs.len()
                && si < shorts.len();
            let subject = if self.actors[long].role == Role::Trader { long } else { short };
            let close_q = if do_d { (q / 2).max(1) } else { q };
            let snap = if do_d {
                Some(BindingSnapshot::read(&self.rpc.get_account_data(&self.actors[subject].portfolio)?, cycle)?)
            } else {
                None
            };
            let ix = self.force_close_ix(a, &self.actors[long].portfolio, &self.actors[short].portfolio, close_q)?;
            match self.chain.send(&[ix], &[]) {
                Ok(_) => {
                    println!(
                        "cycle {cycle}: ForceCloseAbandonedAsset asset {a} long={} short={} q={close_q}",
                        self.actors[long].name, self.actors[short].name
                    );
                    if let Some(mut snap) = snap {
                        *d_done = Some(self.epoch_reread_check(cycle, subject, &mut snap));
                    }
                }
                Err(e) => {
                    println!("cycle {cycle}: force-close {}/{} q={close_q} rejected: {e}", self.actors[long].name, self.actors[short].name);
                    self.reject("force-close", &e);
                }
            }
            done += 1;
            // Re-read next round anyway; advance past the smaller side here.
            let (lq, sq) = (longs[li].1, shorts[si].1);
            if lq <= sq {
                li += 1;
            }
            if sq <= lq {
                si += 1;
            }
        }
        Ok(false)
    }

    /// Assertion d. `snap` was read before a ForceCloseAbandonedAsset that
    /// just landed on `subject`.
    fn epoch_reread_check(&mut self, cycle: u64, subject: usize, snap: &mut BindingSnapshot) -> Check {
        let mut detail = Vec::new();
        let mut ok = true;
        // The force close is a mutating permissionless step on this account.
        snap.note_crank(true);
        if snap.usable(cycle) {
            ok = false;
            detail.push("snapshot still usable after mutation".to_string());
        }
        let fresh = match self
            .rpc
            .get_account_data(&self.actors[subject].portfolio)
            .map_err(anyhow::Error::from)
            .and_then(|d| binding::portfolio_binding(&d))
        {
            Ok(b) => b,
            Err(e) => {
                return Check { id: "d", passed: false, detail: format!("re-read failed: {e}") };
            }
        };
        let stale = snap.binding;
        detail.push(format!(
            "{}: position_epoch {} → {}",
            self.actors[subject].name, stale.position_epoch, fresh.position_epoch
        ));
        if fresh.position_epoch <= stale.position_epoch {
            ok = false;
            detail.push("epoch did not advance".into());
        }
        let a = self.shutdown_asset as u16;
        let forfeit = |epoch: u64| P::ForfeitRecoveryLeg {
            portfolio_id: fresh.portfolio_id,
            position_epoch: epoch,
            asset_index: a,
            b_delta_budget: MAX_VAULT_TVL,
        };
        let owner = &self.actors[subject].owner;
        // ClosePortfolio checks (sequence, position_epoch) before anything
        // else and answers a stale binding with EngineStale. The one-portfolio
        // owner ixs (ForfeitRecoveryLeg, ConvertReleasedPnl, RebalanceReduce)
        // check it in `with_one_portfolio_view` and answer
        // EngineProvenanceMismatch. Both are the stale-binding rejection.
        let stale_close = P::ClosePortfolio {
            portfolio_id: stale.portfolio_id,
            expected_sequence: stale.sequence,
            position_epoch: stale.position_epoch,
        };
        match self.chain.send(&[self.owner_ix(subject, stale_close)], &[owner]) {
            Err(e) if e.is("EngineStale") => detail.push("stale-epoch ClosePortfolio → EngineStale".into()),
            Err(e) => {
                ok = false;
                detail.push(format!("stale-epoch ClosePortfolio → {} (want EngineStale)", e.short()));
            }
            Ok(_) => {
                ok = false;
                detail.push("stale-epoch ClosePortfolio ACCEPTED".into());
            }
        }
        match self.chain.send(&[self.owner_ix(subject, forfeit(stale.position_epoch))], &[owner]) {
            Err(e) if e.is("EngineStale") || e.is("EngineProvenanceMismatch") => {
                detail.push(format!("stale-epoch ForfeitRecoveryLeg → {}", e.short()))
            }
            Err(e) => {
                ok = false;
                detail.push(format!("stale-epoch ForfeitRecoveryLeg → {} (want a binding rejection)", e.short()));
            }
            Ok(_) => {
                ok = false;
                detail.push("stale-epoch ForfeitRecoveryLeg ACCEPTED".into());
            }
        }
        match self.chain.send(&[self.owner_ix(subject, forfeit(fresh.position_epoch))], &[owner]) {
            Ok(_) => detail.push("re-read-epoch ForfeitRecoveryLeg accepted".into()),
            Err(e) => {
                // Forfeit can be refused for economic reasons; any owner ix
                // that binds the epoch proves the point. RebalanceReduce
                // binds the same (portfolio_id, position_epoch).
                let rr = P::RebalanceReduce {
                    portfolio_id: fresh.portfolio_id,
                    position_epoch: fresh.position_epoch,
                    asset_index: a,
                    reduce_q: 1,
                };
                match self.chain.send(&[self.owner_ix(subject, rr)], &[owner]) {
                    Ok(_) => detail.push(format!(
                        "re-read-epoch ForfeitRecoveryLeg → {}; re-read-epoch RebalanceReduce accepted",
                        e.short()
                    )),
                    Err(e2) => {
                        ok = false;
                        detail.push(format!(
                            "re-read-epoch owner ix rejected: forfeit {} / rebalance {}",
                            e.short(),
                            e2.short()
                        ));
                    }
                }
            }
        }
        let c = Check { id: "d", passed: ok, detail: detail.join("; ") };
        println!("cycle {cycle}: epoch re-read check: {}", c.detail);
        c
    }

    /// Crank every exposed live asset and ResolveMarket in one tx.
    fn try_resolve(&mut self, cycle: u64) -> Result<bool> {
        let (data, _, g) = read_mkt(self.rpc, &self.market)?;
        match g.mode {
            MarketModeV16::Resolved => return Ok(true),
            MarketModeV16::Recovery => {
                self.report.market_recovery_seen = true;
                return Ok(false); // the cranker finalizes Recovery → Resolved
            }
            MarketModeV16::Live => {}
        }
        let s0 = seqs(&data, 0)?;
        let resolve = self.chain.ix(
            P::ResolveMarket {
                asset_generation_frontier: g.next_market_id,
                authority_epoch: s0.authority_epoch,
            },
            self.admin_market_metas(),
        );
        let now = self.slot()?;
        let mut ixs = Vec::new();
        for a in 0..self.cfg.assets {
            let asset = &g.assets[a];
            if asset.lifecycle != AssetLifecycleV16::Active || (asset.oi_eff_long_q == 0 && asset.oi_eff_short_q == 0) {
                continue;
            }
            ixs.push(self.chain.ix(
                P::PermissionlessCrank {
                    now_slot: now,
                    observations: vec![percolator_prog::ix::CrankObservationHint { asset_index: a as u16, oracle_accounts: 0 }],
                },
                vec![
                    AccountMeta::new(self.chain.payer.pubkey(), true),
                    AccountMeta::new(self.market, false),
                    AccountMeta::new(self.actors[self.lp_index(a)].portfolio, false),
                ],
            ));
        }
        ixs.push(resolve.clone());
        let r = self.chain.send(&ixs, &[&self.admin]).or_else(|e| {
            println!("cycle {cycle}: crank+ResolveMarket rejected ({}); trying ResolveMarket alone", e.short());
            self.chain.send(&[resolve], &[&self.admin])
        });
        match r {
            Ok(_) => {
                println!("cycle {cycle}: market RESOLVED");
                Ok(true)
            }
            Err(e) => {
                println!("cycle {cycle}: ResolveMarket rejected: {e}");
                self.reject("resolve", &e);
                Ok(false)
            }
        }
    }

    fn snapshot_at_resolve(&mut self) -> Result<()> {
        let keys = self.portfolio_keys();
        let f = fetch_portfolios(self.rpc, &keys)?;
        for (i, x) in f.iter().enumerate() {
            let ata = self.actors[i].ata;
            self.actors[i].ata_at_resolve = token_amount(self.rpc, &ata);
            if let Some((_, p)) = x {
                self.actors[i].equity_at_resolve = p.capital.get() as i128 + p.pnl.get();
            }
        }
        Ok(())
    }

    /// Owner-driven exits for up to `max` open portfolios. Returns the
    /// number still open.
    fn exit_round(&mut self, cycle: u64, max: usize) -> Result<usize> {
        let open: Vec<usize> = (0..self.actors.len()).filter(|&i| !self.actors[i].closed).collect();
        let start = (cycle as usize) % open.len().max(1);
        let mut worked = 0;
        for k in 0..open.len() {
            if worked >= max {
                break;
            }
            let i = open[(start + k) % open.len()];
            worked += 1;
            if self.ensure_ata(i).is_err() {
                continue;
            }
            for _ in 0..8 {
                let mut accts = vec![
                    AccountMeta::new(self.actors[i].owner.pubkey(), true),
                    AccountMeta::new(self.market, false),
                    AccountMeta::new(self.actors[i].portfolio, false),
                ];
                accts.extend(self.payout_tail(&self.actors[i].ata));
                let ix = self.chain.ix(P::CloseResolved { fee_rate_per_slot: 0 }, accts);
                match self.chain.send(&[ix], &[&self.actors[i].owner]) {
                    Ok(_) => {
                        let d = self.rpc.get_account_data(&self.actors[i].portfolio)?;
                        if read_portfolio(&d).map(|p| is_terminal(&p)).unwrap_or(false) {
                            break;
                        }
                    }
                    Err(e) if e.is("EngineNonProgress") => break,
                    Err(e) => {
                        self.reject("close-resolved", &e);
                        break;
                    }
                }
            }
            for _ in 0..4 {
                let before = token_amount(self.rpc, &self.actors[i].ata);
                let mut accts = vec![
                    AccountMeta::new(self.actors[i].owner.pubkey(), true),
                    AccountMeta::new(self.market, false),
                    AccountMeta::new(self.actors[i].portfolio, false),
                ];
                accts.extend(self.payout_tail(&self.actors[i].ata));
                let ix = self.chain.ix(P::ClaimResolvedPayoutTopup, accts);
                match self.chain.send(&[ix], &[&self.actors[i].owner]) {
                    Ok(_) if token_amount(self.rpc, &self.actors[i].ata) > before => continue,
                    Ok(_) => break,
                    Err(e) if e.is("EngineNonProgress") => break,
                    Err(e) => {
                        self.reject("claim-topup", &e);
                        break;
                    }
                }
            }
            let d = self.rpc.get_account_data(&self.actors[i].portfolio)?;
            let Ok(p) = read_portfolio(&d) else { continue };
            if !is_terminal(&p) {
                continue;
            }
            let b = binding::portfolio_binding(&d)?;
            let ix = self.owner_ix(
                i,
                P::ClosePortfolio {
                    portfolio_id: b.portfolio_id,
                    expected_sequence: b.sequence,
                    position_epoch: b.position_epoch,
                },
            );
            match self.chain.send(&[ix], &[&self.actors[i].owner]) {
                Ok(_) => {
                    self.actors[i].closed = true;
                    println!("cycle {cycle}: closed {}", self.actors[i].name);
                }
                Err(e) => self.reject("close-portfolio", &e),
            }
        }
        Ok(self.actors.iter().filter(|a| !a.closed).count())
    }

    fn ensure_ata(&self, i: usize) -> Result<()> {
        if self.rpc.get_account(&self.actors[i].ata).is_ok() {
            return Ok(());
        }
        setup::create_ata(self.rpc, self.chain.payer, &self.actors[i].owner.pubkey(), &self.mint)
    }

    /// `true` once the market slab is retired (account gone or no longer a
    /// decodable market).
    fn slab_closed(&self) -> bool {
        match self.rpc.get_account(&self.market) {
            Ok(a) => a.lamports == 0 || read_market(&a.data).is_err(),
            Err(_) => true,
        }
    }

    /// Backing principal/earnings and per-asset insurance, each withdrawn up
    /// to the largest amount the engine accepts right now.
    fn drain_withdrawals(&mut self) -> Result<()> {
        let (_, _, g) = read_mkt(self.rpc, &self.market)?;
        let vault_now = token_amount(self.rpc, &self.vault) as u128;
        for d in 0..g.source_backing_buckets.len().min(2 * self.cfg.assets) {
            let b = g.source_backing_buckets[d];
            if b.fresh_unliened_backing_num != 0 {
                self.withdraw_backing(d, vault_now, false);
            }
            if b.utilization_fee_earnings != 0 {
                self.withdraw_backing(d, vault_now, true);
            }
        }
        for a in 0..self.cfg.assets {
            let (data, _, g) = read_mkt(self.rpc, &self.market)?;
            let s = seqs(&data, a)?;
            let mid = g.assets[a].market_id;
            let hi = (token_amount(self.rpc, &self.vault) as u128).min(g.insurance);
            let build = |x: u128| {
                self.chain.ix(
                    P::WithdrawInsuranceAsset {
                        asset_index: a as u16,
                        market_id: mid,
                        authority_epoch: s.authority_epoch,
                        amount: x,
                    },
                    vec![
                        AccountMeta::new(self.admin.pubkey(), true),
                        AccountMeta::new(self.market, false),
                        AccountMeta::new(self.admin_ata, false),
                        AccountMeta::new(self.vault, false),
                        AccountMeta::new_readonly(self.vault_authority, false),
                        AccountMeta::new_readonly(spl_token::ID, false),
                    ],
                )
            };
            let amt = self.max_accepted(hi, &build);
            if amt > 0 {
                match self.chain.send(&[build(amt)], &[&self.admin]) {
                    Ok(_) => println!("withdraw insurance asset {a}: {amt}"),
                    Err(e) => self.reject("insurance-withdraw", &e),
                }
            }
        }
        Ok(())
    }

    /// Withdrawals, then CloseSlab until the slab is retired. CloseSlab is
    /// multi-step: a successful call may only advance the terminal scan
    /// (expire a bucket, recredit insurance) and return; only the final
    /// step sweeps the vault to the market authority and deallocates.
    fn final_drain(&mut self) -> Result<()> {
        self.drain_withdrawals()?;
        let (_, _, g) = read_mkt(self.rpc, &self.market)?;
        let vault_pre = token_amount(self.rpc, &self.vault);
        println!(
            "pre-CloseSlab: vault(token)={} vault(engine)={} insurance={} c_tot={} materialized={}",
            vault_pre, g.vault, g.insurance, g.c_tot, g.materialized_portfolio_count
        );
        self.report.vault_before_close_slab = vault_pre;
        let admin_before = token_amount(self.rpc, &self.admin_ata);
        for step in 0..32 {
            if self.slab_closed() {
                break;
            }
            let (data, _, _) = read_mkt(self.rpc, &self.market)?;
            let s0 = seqs(&data, 0)?;
            let ix = self.chain.ix(
                P::CloseSlab { authority_epoch: s0.authority_epoch },
                vec![
                    AccountMeta::new(self.admin.pubkey(), true),
                    AccountMeta::new(self.market, false),
                    AccountMeta::new(self.vault, false),
                    AccountMeta::new_readonly(self.vault_authority, false),
                    AccountMeta::new(self.admin_ata, false),
                    AccountMeta::new_readonly(spl_token::ID, false),
                    // Burned when claim-free residue is retired.
                    AccountMeta::new(self.mint, false),
                ],
            );
            match self.chain.send(&[ix], &[&self.admin]) {
                Ok(_) => {
                    println!("CloseSlab step {step}: ok (slab closed: {})", self.slab_closed());
                    if !self.slab_closed() {
                        // A scan step can recredit insurance; drain it again.
                        self.drain_withdrawals()?;
                    }
                }
                Err(e) => {
                    println!("CloseSlab step {step} rejected: {e}");
                    self.reject("close-slab", &e);
                    break;
                }
            }
        }
        self.report.close_slab = self.slab_closed();
        self.report.swept_to_admin = token_amount(self.rpc, &self.admin_ata).saturating_sub(admin_before);
        println!(
            "CloseSlab: retired={} swept to market authority={} atoms",
            self.report.close_slab, self.report.swept_to_admin
        );
        Ok(())
    }

    /// Per-tick market observations for the coverage counters.
    fn observe_market(&mut self, f0: &[(i128, i128)]) -> Result<MarketGroupV16> {
        let (_, _, g) = read_mkt(self.rpc, &self.market)?;
        for (d, b) in g.source_backing_buckets.iter().enumerate().take(2 * self.cfg.assets) {
            // Engine finding (see cranker/scenario-findings.md): in Live, a
            // Fresh bucket past expiry that no portfolio references is never
            // normalized, and blocks refreshes that would newly attribute
            // positive PnL to that domain.
            if g.mode == MarketModeV16::Live
                && b.status == BackingBucketStatusV16::Fresh
                && b.expiry_slot != 0
                && b.expiry_slot <= g.current_slot
            {
                let e = self.report.lapsed_unnormalized_live.entry(d).or_insert((g.current_slot, g.current_slot));
                e.1 = g.current_slot;
            }
            match b.status {
                BackingBucketStatusV16::Expired => {
                    self.report.buckets_expired.insert(d);
                }
                BackingBucketStatusV16::Impaired => {
                    self.report.buckets_impaired.insert(d);
                }
                _ => {}
            }
        }
        for a in 0..self.cfg.assets {
            let s = &g.assets[a];
            if s.lifecycle == AssetLifecycleV16::Recovery {
                self.report.recovery_assets.insert(a);
            }
            let dl = s.f_long_num - f0[a].0;
            let ds = s.f_short_num - f0[a].1;
            if dl != 0 || ds != 0 {
                self.report.funding_assets.insert(a);
            }
            self.report.max_abs_f_delta = self.report.max_abs_f_delta.max(dl.abs()).max(ds.abs());
        }
        if g.mode == MarketModeV16::Recovery {
            self.report.market_recovery_seen = true;
        }
        Ok(g)
    }

    fn count_netting(&mut self) -> Result<()> {
        let keys = self.portfolio_keys();
        let f = fetch_portfolios(self.rpc, &keys)?;
        let mid = self.phases.stress_start + (self.phases.stress_end - self.phases.stress_start) / 2;
        let mut n = 0;
        for (i, x) in f.iter().enumerate() {
            if self.actors[i].role != Role::Trader {
                continue;
            }
            let Some((_, p)) = x else { continue };
            let signs: BTreeSet<bool> = legs(p)
                .iter()
                .map(|&(a, q)| (q > 0) == (self.path_bps(a, mid) > 0))
                .collect();
            if signs.len() == 2 {
                n += 1;
            }
        }
        self.report.netting_portfolios = n;
        Ok(())
    }
}

fn txe(e: anyhow::Error) -> TxError {
    TxError { name: None, msg: e.to_string() }
}

/// Run the scenario end to end. `Err` = the scenario itself broke (setup,
/// RPC). Assertion failures come back in `ScenarioReport::checks`.
pub fn run() -> Result<ScenarioReport> {
    let started = Instant::now();
    let cfg = ScenarioConfig::from_env()?;
    let rpc = RpcClient::new_with_commitment(RPC_URL.to_string(), CommitmentConfig::confirmed());
    rpc.get_version().context("connect to local validator")?;
    let home = std::env::var("HOME").context("HOME")?;
    let payer = read_keypair_file(format!("{home}/.config/solana/id.json"))
        .map_err(|e| anyhow!("read payer keypair: {e}"))?;
    println!(
        "== live_shaped: seed={:#x} traders={} assets={} cycles={} program={} ==",
        cfg.seed, cfg.traders, cfg.assets, cfg.cycles, cfg.program_id
    );

    let mut rng = SplitMix64::new(cfg.seed);
    let dir: Vec<i64> = (0..cfg.assets).map(|_| if rng.coin() { 1 } else { -1 }).collect();
    let noise: Vec<Vec<i64>> = (0..cfg.assets)
        .map(|_| (0..=cfg.cycles).map(|_| rng.signed(40)).collect())
        .collect();
    let phases = Phases::new(cfg.cycles);
    println!("phases: {phases:?} dir={dir:?}");

    let mut w = World {
        rpc: &rpc,
        chain: Chain { rpc: &rpc, program_id: cfg.program_id, payer: &payer },
        admin: Keypair::new(),
        mint: Pubkey::default(),
        market: Pubkey::default(),
        vault: Pubkey::default(),
        vault_authority: Pubkey::default(),
        admin_ata: Pubkey::default(),
        reward: Pubkey::default(),
        actors: Vec::new(),
        report: ScenarioReport {
            seed: cfg.seed,
            traders: cfg.traders,
            assets: cfg.assets,
            cycles: cfg.cycles,
            ..Default::default()
        },
        dir,
        noise,
        phases,
        shutdown_asset: cfg.assets - 1,
        cfg,
    };
    w.setup()?;
    w.open_book()?;

    let (_, _, g0) = read_mkt(&rpc, &w.market)?;
    let f0: Vec<(i128, i128)> = g0.assets.iter().map(|a| (a.f_long_num, a.f_short_num)).collect();

    let pyth = observation::empty_pyth_resolver();
    let budget = CycleBudget {
        max_wall: std::time::Duration::from_millis(
            std::env::var("CRANKER_CYCLE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(6_000),
        ),
        max_submissions: std::env::var("CRANKER_CYCLE_MAX_TX").ok().and_then(|v| v.parse().ok()).unwrap_or(12),
        ..CycleBudget::default()
    };
    let qcfg = QuarantineConfig::from_env();
    let mut quarantine = Quarantine::new(qcfg);
    let mut records: Vec<SubmissionRecord> = Vec::new();
    let mut cu = CuStats::default();
    println!("budget: {budget:?} quarantine: {qcfg:?}");

    let mut act_rng = SplitMix64::new(w.cfg.seed ^ 0x57E55);
    let slot0 = w.slot()?;
    let mut shutdown_done = false;
    let mut force_close_done = false;
    let mut resolved_at: Option<u64> = None;
    let mut exits_done = false;
    let mut d_check: Option<Check> = None;
    let mut fail_start: HashMap<Pubkey, u64> = HashMap::new();
    let mut liquidatable: BTreeSet<Pubkey> = BTreeSet::new();
    let reward_key = w.reward;
    let exclude: Vec<Pubkey> = Vec::new();

    for cycle in 0..w.cfg.cycles {
        let ph = w.phases;
        // ---------------- scenario actions ----------------
        if resolved_at.is_none() {
            w.push_marks(cycle, cycle >= ph.settle)?;
        }
        if cycle == ph.topup {
            let now = w.slot()?;
            let spc = ((now - slot0) / cycle.max(1)).max(2);
            let stress = ph.stress_end - ph.stress_start;
            for a in 0..w.cfg.assets.min(3) {
                if a == w.shutdown_asset {
                    // Long-dated bucket, withdrawn (partly in Live, rest after exits).
                    w.top_up_backing(2 * a, 1_000_000, now + 10_000_000);
                    continue;
                }
                let lien = w.lien_domain(a);
                let lapse = lien ^ 1;
                w.top_up_backing(lapse, 1_000_000, now + spc * stress * 55 / 100);
                w.top_up_backing(lien, 3_000_000, now + spc * stress * 75 / 100);
            }
        }
        if cycle > ph.stress_start && cycle < ph.stress_end && (cycle - ph.stress_start) % (ph.stress_end - ph.stress_start).div_ceil(4).max(1) == 0 {
            w.stress_actions(cycle, &mut act_rng)?;
        }
        if cycle == ph.stress_start + (ph.stress_end - ph.stress_start) * 35 / 100 {
            w.winner_reopen(cycle)?;
        }
        if cycle == ph.stress_end - 2 {
            let d = 2 * w.shutdown_asset;
            let got = w.withdraw_backing(d, 400_000, false);
            if got == 0 {
                w.report.notes.push(format!("Live WithdrawBackingBucket on domain {d} not accepted"));
            }
            w.count_netting()?;
        }
        if cycle >= ph.stress_end && !shutdown_done && resolved_at.is_none() {
            shutdown_done = w.shutdown_asset(cycle)?;
        }
        if shutdown_done && !force_close_done && resolved_at.is_none() {
            force_close_done = w.force_close_round(cycle, 6, &mut d_check)?;
        }
        if cycle >= ph.resolve && resolved_at.is_none() && (force_close_done || cycle >= ph.resolve + 6) {
            if w.try_resolve(cycle)? {
                resolved_at = Some(cycle);
                w.snapshot_at_resolve()?;
            }
        }
        if resolved_at.is_some() && !exits_done {
            let (_, cfgm, g) = read_mkt(&rpc, &w.market)?;
            let _ = cfgm;
            if g.mode == MarketModeV16::Resolved {
                let open = w.exit_round(cycle, 10)?;
                if open == 0 {
                    exits_done = true;
                    w.final_drain()?;
                }
            }
        }

        // ---------------- one real cranker cycle ----------------
        let keys = w.portfolio_keys();
        let before = fetch_portfolios(&rpc, &keys)?;
        let first = records.len();
        let q_events_before = quarantine.events().len();
        let market_live = read_mkt(&rpc, &w.market).map(|(_, _, g)| g.mode == MarketModeV16::Live).unwrap_or(false);
        let summary = if w.report.close_slab {
            None
        } else {
            let ctx = CrankCtx {
                rpc: &rpc,
                program_id: &w.cfg.program_id,
                payer: &payer,
                market: &w.market,
                reward_portfolio: if market_live { Some(&reward_key) } else { None },
                pyth: &pyth,
                exclude: &exclude,
            };
            Some(cycle::run_one_cycle(&ctx, cycle, &budget, &mut quarantine, &mut records, &mut cu)?)
        };
        let after = fetch_portfolios(&rpc, &keys)?;
        let g = if w.report.close_slab { None } else { Some(w.observe_market(&f0)?) };

        // Liquidation evidence (Live only, scenario actions excluded: they
        // ran before `before` was read).
        if market_live {
            for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
                let (Some((_, pb)), Some((_, pa))) = (b, a) else { continue };
                if pa.health_cert.certified_liq_deficit.get() != 0 || pa.liquidation_lock != 0 {
                    liquidatable.insert(keys[i]);
                }
                let la: HashMap<usize, i128> = legs(pa).into_iter().collect();
                for (asset, q) in legs(pb) {
                    let still = la.get(&asset).copied().unwrap_or(0);
                    let recov = g.as_ref().map(|g| g.assets[asset].lifecycle == AssetLifecycleV16::Recovery).unwrap_or(false);
                    if !recov && still.abs() < q.abs() {
                        w.report.crank_leg_reductions += 1;
                    }
                }
            }
        }
        // Per-target attempt class this cycle → consecutive-failure runs.
        let mut by_target: BTreeMap<Pubkey, (bool, bool)> = BTreeMap::new(); // (any progress, last failed)
        for r in &records[first..] {
            let e = by_target.entry(r.target).or_default();
            match r.outcome.class() {
                OutcomeClass::Progress => e.0 = true,
                OutcomeClass::Failed => e.1 = true,
                OutcomeClass::Idle => e.1 = false,
            }
            if market_live {
                if let CrankOutcome::Ok { reward_bytes_diff: Some(d), .. } = &r.outcome {
                    if *d > 0 {
                        w.report.liquidation_fee_cranks += 1;
                    }
                }
            }
            if r.plan_class.starts_with("resolved:backing-hint") {
                w.report.resolved_hints_sent += 1;
            }
        }
        for (pk, (progress, failed)) in by_target {
            if failed && !progress {
                let start = *fail_start.entry(pk).or_insert(cycle);
                w.report.max_fail_run = w.report.max_fail_run.max(cycle - start + 1);
            } else {
                fail_start.remove(&pk);
            }
        }
        // Raw bytes of each newly quarantined account + the market, for an
        // offline replay of the failing crank (CRANKER_SCENARIO_DUMP_DIR).
        if let Ok(dir) = std::env::var("CRANKER_SCENARIO_DUMP_DIR") {
            for ev in &quarantine.events()[q_events_before..] {
                let tag = format!("{dir}/c{}-s{}-{}", ev.cycle, ev.slot, ev.account);
                if let (Ok(p), Ok(m)) = (rpc.get_account_data(&ev.account), rpc.get_account_data(&w.market)) {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(format!("{tag}.portfolio"), p);
                    let _ = std::fs::write(format!("{tag}.market"), m);
                }
            }
        }
        if let Some(s) = &summary {
            println!("{}", s.line());
            for r in &records[first..] {
                if r.outcome.class() == OutcomeClass::Failed {
                    println!(
                        "  failed {} [{}]: {}",
                        r.target,
                        r.plan_class,
                        r.outcome.error_key().unwrap_or_default()
                    );
                }
            }
            w.report.progressed += s.progressed;
            w.report.idle += s.idle;
            w.report.failed += s.failed;
        }
        // Accounts still in a failure run that the cranker can no longer
        // see (closed) end their run here.
        let live_keys: BTreeSet<Pubkey> = after.iter().zip(keys.iter()).filter(|(a, _)| a.is_some()).map(|(_, k)| *k).collect();
        fail_start.retain(|k, _| live_keys.contains(k));
    }

    // Exits that did not finish inside the cycle budget get a bounded tail.
    if resolved_at.is_some() && !exits_done {
        for extra in 0..40u64 {
            let open = w.exit_round(w.cfg.cycles + extra, 50)?;
            if open == 0 {
                exits_done = true;
                w.final_drain()?;
                w.report.notes.push(format!("exits finished {} rounds after the cycle budget", extra + 1));
                break;
            }
        }
    }

    // ---------------- assertions ----------------
    let rep = &mut w.report;
    rep.submissions = records.len();
    rep.cu = cu;
    rep.liquidatable_seen = liquidatable.len();
    rep.quarantine_events = quarantine.events().iter().map(|e| e.to_json_line()).collect();
    let qc = *quarantine.config();
    rep.fail_run_bound = qc.threshold as u64 + qc.max_backoff_cycles;
    let struck = quarantine.struck_accounts();
    rep.checks.push(Check {
        id: "a",
        passed: struck.is_empty() && rep.max_fail_run <= rep.fail_run_bound,
        detail: format!(
            "struck_accounts={} max consecutive-failure run={} (bound threshold {} + max backoff {} = {}) events={}",
            struck.len(),
            rep.max_fail_run,
            qc.threshold,
            qc.max_backoff_cycles,
            rep.fail_run_bound,
            rep.quarantine_events.len()
        ),
    });

    // b: every portfolio terminal; every positive claimant paid.
    let keys: Vec<Pubkey> = w.actors.iter().map(|a| a.portfolio).collect();
    let fin = fetch_portfolios(&rpc, &keys)?;
    let mut not_terminal = Vec::new();
    let mut unpaid = Vec::new();
    let mut paid = 0;
    for (i, a) in w.actors.iter().enumerate() {
        if let Some((_, p)) = &fin[i] {
            if !is_terminal(p) {
                not_terminal.push(a.name.clone());
            }
        }
        let got = token_amount(&rpc, &a.ata).saturating_sub(a.ata_at_resolve);
        if got > 0 {
            paid += 1;
        }
        if a.equity_at_resolve > 0 && got == 0 {
            unpaid.push(format!("{}(equity {})", a.name, a.equity_at_resolve));
        }
    }
    let rep = &mut w.report;
    if resolved_at.is_none() {
        rep.notes.push("market never reached Resolved".into());
    }
    rep.checks.push(Check {
        id: "b",
        passed: resolved_at.is_some() && not_terminal.is_empty() && unpaid.is_empty(),
        detail: format!(
            "resolved={} portfolios={} non-terminal={:?} positive-equity unpaid={:?} paid owners={}",
            resolved_at.is_some(),
            w.actors.len(),
            not_terminal,
            unpaid,
            paid
        ),
    });

    // c: vault dust.
    let rep = &mut w.report;
    rep.vault_after = token_amount(&rpc, &w.vault);
    rep.dust_bound = (w.cfg.assets as u64) * (w.actors.len() as u64) * DUST_ATOMS_PER_ASSET_ACCOUNT;
    rep.checks.push(Check {
        id: "c",
        passed: exits_done && (rep.close_slab || rep.vault_after <= rep.dust_bound),
        detail: format!(
            "exits_done={} close_slab={} vault before CloseSlab={} after={} atoms, dust bound={} (= assets × portfolios × {DUST_ATOMS_PER_ASSET_ACCOUNT}); CloseSlab swept {} to marketauth",
            exits_done, rep.close_slab, rep.vault_before_close_slab, rep.vault_after, rep.dust_bound, rep.swept_to_admin
        ),
    });

    // d: position epoch re-read after Recovery.
    rep.checks.push(d_check.unwrap_or(Check {
        id: "d",
        passed: false,
        detail: "no ForceCloseAbandonedAsset with a trader landed".into(),
    }));

    // e: coverage counters (each must be nonzero to count as covered).
    let cov = [
        ("liquidations", rep.liquidation_fee_cranks + rep.crank_leg_reductions > 0),
        ("expired bucket", !rep.buckets_expired.is_empty()),
        ("impaired bucket", !rep.buckets_impaired.is_empty()),
        ("recovery asset", !rep.recovery_assets.is_empty()),
        ("funding", !rep.funding_assets.is_empty()),
        ("cu noop", rep.cu.noop_min.is_some()),
    ];
    let missing: Vec<&str> = cov.iter().filter(|(_, ok)| !ok).map(|(n, _)| *n).collect();
    rep.checks.push(Check {
        id: "e",
        passed: missing.is_empty(),
        detail: if missing.is_empty() {
            "all coverage counters nonzero".into()
        } else {
            format!("not covered: {missing:?}")
        },
    });
    rep.wall_secs = started.elapsed().as_secs();
    let _ = BOUND_SCALE;
    Ok(std::mem::take(&mut w.report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The InitMarket parameters must pass the engine's own config check
    /// (shape + exact solvency envelope) — otherwise InitMarket fails on
    /// chain with an opaque InvalidAccountData.
    #[test]
    fn init_market_params_pass_engine_validation() {
        let P::InitMarket {
            max_portfolio_assets, h_min, h_max, min_nonzero_mm_req, min_nonzero_im_req,
            maintenance_margin_bps, initial_margin_bps, max_trading_fee_bps,
            liquidation_fee_bps, liquidation_fee_cap, min_liquidation_abs,
            max_price_move_bps_per_slot, max_accrual_dt_slots, max_abs_funding_e9_per_slot,
            min_funding_lifetime_slots, max_account_b_settlement_chunks,
            max_bankrupt_close_chunks, max_bankrupt_close_lifetime_slots, public_b_chunk_atoms, ..
        } = init_market_params(3)
        else {
            unreachable!()
        };
        let mut c = percolator::V16Config::public_user_fund(max_portfolio_assets, h_min, h_max);
        c.min_nonzero_mm_req = min_nonzero_mm_req;
        c.min_nonzero_im_req = min_nonzero_im_req;
        c.maintenance_margin_bps = maintenance_margin_bps;
        c.initial_margin_bps = initial_margin_bps;
        c.max_trading_fee_bps = max_trading_fee_bps;
        c.liquidation_fee_bps = liquidation_fee_bps;
        c.liquidation_fee_cap = liquidation_fee_cap;
        c.min_liquidation_abs = min_liquidation_abs;
        c.max_price_move_bps_per_slot = max_price_move_bps_per_slot;
        c.max_accrual_dt_slots = max_accrual_dt_slots;
        c.max_abs_funding_e9_per_slot = max_abs_funding_e9_per_slot;
        c.min_funding_lifetime_slots = min_funding_lifetime_slots;
        c.max_account_b_settlement_chunks = max_account_b_settlement_chunks;
        c.max_bankrupt_close_chunks = max_bankrupt_close_chunks;
        c.max_bankrupt_close_lifetime_slots = max_bankrupt_close_lifetime_slots;
        c.public_b_chunk_atoms = public_b_chunk_atoms;
        assert_eq!(c.validate_public_user_fund(), Ok(()));
    }

    #[test]
    fn phases_are_ordered() {
        for c in [40u64, 200, 500] {
            let p = Phases::new(c);
            assert!(p.topup < p.stress_start && p.stress_start < p.stress_end);
            assert!(p.stress_end < p.settle && p.settle < p.resolve && p.resolve < c);
        }
    }
}
