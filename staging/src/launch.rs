//! Day-0 launch on top of the R2 staging code: the config file, resumable
//! setup steps with a state file, and bots on an existing market.
//! Reference / dry-run only; not a paid live service; M3 NO-GO.
//! Driven by `scripts/launch_market.sh`; see docs/LAUNCH.md.

use crate::{Phase, Staging};
use anyhow::{anyhow, bail, Context, Result};
use drip::{cycle::SubmissionRecord, setup, submit::CrankOutcome};
use percolator::MAX_VAULT_TVL;
use percolator_prog::{ix::Instruction as P, oracle_v16, state::read_portfolio};
use percolator::BackingBucketStatusV16 as Bucket;
use serde::{Deserialize, Serialize};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    bs58,
    commitment_config::CommitmentConfig,
    instruction::Instruction,
    message::Message,
    pubkey::Pubkey,
    signature::{read_keypair_file, write_keypair_file, Keypair, Signer},
    transaction::Transaction,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

// ------------------------------------------------------------ config

// `pins` and `monitor` are read by scripts/launch_market.sh.
#[allow(dead_code)]
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub pins: Pins,
    pub cluster: BTreeMap<String, ClusterCfg>,
    pub oracle: OracleCfg,
    pub market: MarketCfg,
    pub resolve: ResolveCfg,
    pub matcher: MatcherCfg,
    pub amounts: AmountsCfg,
    pub monitor: MonitorCfg,
}

#[allow(dead_code)]
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Pins {
    pub wrapper_sha: String,
    pub engine_rev: String,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ClusterCfg {
    pub rpc: String,
    pub program_id: String,
    pub collateral_mint: String,
    pub price_source: String,
    #[serde(default)]
    pub multisig_vault: String,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct OracleCfg {
    pub pyth_feed_id: String,
    pub pyth_price_account: String,
    pub pyth_max_age_s: u64,
    pub pyth_max_conf_bps: u16,
    pub lag_target_ms: u64,
    pub push_interval_ms: u64,
    pub push_heartbeat_slots: u64,
    pub push_deviation_bps: u64,
}

/// InitMarket, field for field (`initial_price` is the mock start). TOML
/// integers are i64, so u128 fields are read as u64.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MarketCfg {
    pub max_portfolio_assets: u16,
    pub initial_price: u64,
    pub h_min: u64,
    pub h_max: u64,
    pub min_nonzero_mm_req: u64,
    pub min_nonzero_im_req: u64,
    pub maintenance_margin_bps: u64,
    pub initial_margin_bps: u64,
    pub max_trading_fee_bps: u64,
    pub trade_fee_base_bps: u64,
    pub liquidation_fee_bps: u64,
    pub liquidation_fee_cap: u64,
    pub min_liquidation_abs: u64,
    pub max_price_move_bps_per_slot: u64,
    pub max_accrual_dt_slots: u64,
    pub max_abs_funding_e9_per_slot: u64,
    pub min_funding_lifetime_slots: u64,
    pub max_account_b_settlement_chunks: u64,
    pub max_bankrupt_close_chunks: u64,
    pub max_bankrupt_close_lifetime_slots: u64,
    /// "max" (MAX_VAULT_TVL) or a number of atoms.
    pub public_b_chunk_atoms: String,
    pub maintenance_fee_per_slot: u64,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ResolveCfg {
    pub stale_slots: u64,
    pub force_close_delay_slots: u64,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MatcherCfg {
    pub size_q: i64,
    pub spread_bps: u64,
    pub fee_bps: u64,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AmountsCfg {
    pub lp_deposit: u64,
    pub trader_deposit: u64,
    pub insurance_seed: u64,
    pub insurance_topup: u64,
    pub insurance_every: u64,
    pub backing_per_domain: u64,
    pub backing_tenor_slots: u64,
    pub authority_test_mint: u64,
}

#[allow(dead_code)]
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MonitorCfg {
    pub listen: String,
}

pub fn load(path: &str) -> Result<Config> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read config {path}"))?;
    toml::from_str(&text).with_context(|| format!("parse config {path}"))
}

impl MarketCfg {
    pub fn init_ix(&self, initial_price: u64) -> Result<P> {
        let m = self;
        let public_b_chunk_atoms = match m.public_b_chunk_atoms.as_str() {
            "max" => MAX_VAULT_TVL,
            n => n.parse().context("market.public_b_chunk_atoms")?,
        };
        Ok(P::InitMarket {
            max_portfolio_assets: m.max_portfolio_assets,
            h_min: m.h_min,
            h_max: m.h_max,
            initial_price,
            min_nonzero_mm_req: m.min_nonzero_mm_req as u128,
            min_nonzero_im_req: m.min_nonzero_im_req as u128,
            maintenance_margin_bps: m.maintenance_margin_bps,
            initial_margin_bps: m.initial_margin_bps,
            max_trading_fee_bps: m.max_trading_fee_bps,
            trade_fee_base_bps: m.trade_fee_base_bps,
            liquidation_fee_bps: m.liquidation_fee_bps,
            liquidation_fee_cap: m.liquidation_fee_cap as u128,
            min_liquidation_abs: m.min_liquidation_abs as u128,
            max_price_move_bps_per_slot: m.max_price_move_bps_per_slot,
            max_accrual_dt_slots: m.max_accrual_dt_slots,
            max_abs_funding_e9_per_slot: m.max_abs_funding_e9_per_slot,
            min_funding_lifetime_slots: m.min_funding_lifetime_slots,
            max_account_b_settlement_chunks: m.max_account_b_settlement_chunks,
            max_bankrupt_close_chunks: m.max_bankrupt_close_chunks,
            max_bankrupt_close_lifetime_slots: m.max_bankrupt_close_lifetime_slots,
            public_b_chunk_atoms,
            maintenance_fee_per_slot: m.maintenance_fee_per_slot as u128,
        })
    }
}

// ------------------------------------------------------------ price source

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// The configured Pyth feed id, 32 bytes.
pub fn feed_id(o: &OracleCfg) -> Result<[u8; 32]> {
    let hex = o.pyth_feed_id.trim_start_matches("0x");
    let mut id = [0u8; 32];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(2 * i..2 * i + 2).context("pyth_feed_id: 64 hex chars")?, 16)?;
    }
    Ok(id)
}

/// Pyth SOL/USD from its on-chain PriceUpdateV2 account
/// (`oracle.pyth_price_account`), parsed by the wrapper's own reader
/// (`oracle_v16`, which checks the owner and the feed id): price in e6 and
/// the publish time in ms. Same RPC, no new dependency. Publish times are
/// whole seconds, so the lag measured from them reads up to 1 s high.
/// (Hermes, the lower-latency path upstream's `pyth-push-devnet.ts` uses,
/// answers 401 without an API key as of 2026-10-29.)
pub fn pyth_sol_usd(rpc: &RpcClient, feed: &[u8; 32], o: &OracleCfg) -> Result<(u64, u64)> {
    let key = Pubkey::from_str(&o.pyth_price_account).context("oracle.pyth_price_account")?;
    let acc = rpc.get_account(&key).with_context(|| format!("Pyth price account {key}"))?;
    let now = (now_ms() / 1000) as i64;
    let (e6, t) = oracle_v16::read_oracle_price_e6_from_bytes(&acc.owner, &key, &acc.data, feed, now, o.pyth_max_age_s, o.pyth_max_conf_bps)
        .map_err(|e| anyhow!("Pyth price {key}: {e:?}"))?;
    Ok((e6, t as u64 * 1000))
}

// ------------------------------------------------------------ multisig

/// Marks a step that stopped on an unsigned multisig transaction.
#[derive(Debug)]
struct Pending(PathBuf);
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "waiting for the multisig: {}", self.0.display())
    }
}
impl std::error::Error for Pending {}

const UNSIGNED_README: &str = "# Unsigned authority transactions

Reference / dry-run only; not a paid live service; M3 NO-GO.

Each `.b58` file is one legacy transaction, bincode-serialized and base58
encoded, unsigned, with the Squads vault as its only signer and fee payer.
`scripts/launch_market.sh` writes one when a step needs the market
authority, then stops. To land it:

1. Squads app -> the vault -> transaction builder -> import from base58;
   paste the file. Check the program id and accounts against
   `launch/sol-perp.toml` and the step named in the file name.
2. Set the execute transaction's compute limit to 1,400,000 and its heap
   frame to 128 KiB (the wrapper needs both; budget instructions cannot sit
   inside the vault transaction). *To verify* on devnet first.
3. Two signers approve; execute.
4. Re-run the same `launch_market.sh` command. It sees the change on chain,
   marks the step done and goes on (it may stop again at the next one).

A file older than its step's on-chain state is stale: re-running rewrites it.
";

pub fn write_unsigned(rpc: &RpcClient, run: &Path, label: &str, vault: Pubkey, ixs: &[Instruction]) -> Result<()> {
    let bh = rpc.get_latest_blockhash()?;
    let tx = Transaction::new_unsigned(Message::new_with_blockhash(ixs, Some(&vault), &bh));
    let dir = run.join("unsigned");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("README.md"), UNSIGNED_README)?;
    let file = dir.join(format!("{label}.b58"));
    std::fs::write(&file, bs58::encode(bincode::serialize(&tx)?).into_string())?;
    Err(Pending(file).into())
}

// ------------------------------------------------------------ keeper log

/// drip's per-crank and per-cycle lines, which the monitor parses
/// (monitor/src/keeper_log.rs).
pub fn print_keeper_lines(records: &[SubmissionRecord], summary_line: &str) {
    for r in records {
        match &r.outcome {
            CrankOutcome::Ok { signature, mutated, program_cu, .. } => println!(
                "  {} [{}] ← Ok({}): sig={signature} program_cu={program_cu:?}",
                r.target,
                r.plan_class,
                if *mutated { "mut" } else { "no-mut" }
            ),
            CrankOutcome::HardReject { error, error_name, .. } => println!(
                "  {} [{}] ← HardReject ({}): {error}",
                r.target,
                r.plan_class,
                error_name.unwrap_or("Unknown")
            ),
        }
    }
    println!("{summary_line}");
}

// ------------------------------------------------------------ state, keys

/// `<run dir>/state.json`: created addresses and completed steps.
#[derive(Serialize, Deserialize, Default)]
struct State {
    program_id: String,
    mint: String,
    market: String,
    lp_pf: String,
    trader_pf: String,
    done: Vec<String>,
}

impl State {
    fn load(run: &Path) -> Result<State> {
        match std::fs::read_to_string(run.join("state.json")) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(_) => Ok(State::default()),
        }
    }
    fn save(&self, run: &Path) -> Result<()> {
        std::fs::write(run.join("state.json"), serde_json::to_string_pretty(self)? + "\n")?;
        Ok(())
    }
}

/// `<run dir>/keys/<name>.json` (gitignored run dir), generated when
/// `create` and missing. Mainnet hot keys are placed there by the owner.
fn key(run: &Path, name: &str, create: bool) -> Result<Keypair> {
    let path = run.join("keys").join(format!("{name}.json"));
    if !path.exists() {
        if !create {
            bail!("{} missing (mainnet: the owner places hot keys, ops/KEYS.md)", path.display());
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        write_keypair_file(&Keypair::new(), &path).map_err(|e| anyhow!("write {}: {e}", path.display()))?;
    }
    read_keypair_file(&path).map_err(|e| anyhow!("read {}: {e}", path.display()))
}

/// `<config> <cluster> <run dir>` -> the staging state for that launch.
fn open(args: &[String]) -> Result<(Staging, State, PathBuf, String)> {
    let [config, cluster, run] = args else { bail!("usage: staging step|bots <config> <cluster> <run dir>") };
    let cfg = load(config)?;
    let c = cfg.cluster.get(cluster).with_context(|| format!("no [cluster.{cluster}] in {config}"))?.clone();
    let mainnet = cluster == "mainnet";
    if mainnet && std::env::var("LAUNCH_MAINNET_I_UNDERSTAND").as_deref() != Ok("1") {
        bail!("mainnet needs --mainnet-i-understand (scripts/launch_market.sh)");
    }
    // Env overrides (hosting): the program id, and a paid RPC kept out of the config.
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let id = env("PERCOLATOR_PROGRAM_ID").unwrap_or(c.program_id.clone());
    let program_id = Pubkey::from_str(&id).with_context(|| format!("program id {id:?} for {cluster}"))?;
    let run = PathBuf::from(run);
    let state = State::load(&run)?;
    if !state.program_id.is_empty() && state.program_id != program_id.to_string() {
        bail!("{} belongs to program {}; use a fresh run dir", run.display(), state.program_id);
    }
    let rpc = RpcClient::new_with_commitment(env("LAUNCH_RPC_URL").unwrap_or(c.rpc.clone()), CommitmentConfig::confirmed());
    let payer = key(&run, "payer", !mainnet)?;
    let lp = key(&run, "lp", !mainnet)?;
    // Mainnet: the vault is the authority and there is no test trader.
    let (admin, trader) = if mainnet { (Keypair::new(), Keypair::new()) } else { (key(&run, "admin", true)?, key(&run, "trader", true)?) };
    let (mint, own_mint) = match c.collateral_mint.as_str() {
        "create" if mainnet => bail!("mainnet needs a real collateral_mint"),
        "create" => (key(&run, "mint", true)?.pubkey(), true),
        m => (Pubkey::from_str(m).context("collateral_mint")?, false),
    };
    let mut st = Staging::new(cfg, rpc, program_id, payer, admin, lp, trader, mint, own_mint);
    st.run_dir = Some(run.clone());
    if mainnet {
        let vault = Pubkey::from_str(&c.multisig_vault).context("[cluster.mainnet] multisig_vault must be the Squads vault")?;
        st.multisig = Some(vault);
        st.admin_ata = spl_associated_token_account::get_associated_token_address(&vault, &mint);
    }
    if c.price_source == "pyth" {
        st.pyth_feed = Some(feed_id(&st.cfg.oracle)?);
    }
    if !state.market.is_empty() {
        st.set_market(Pubkey::from_str(&state.market)?);
    }
    for (pf, s) in [(&mut st.lp_pf, &state.lp_pf), (&mut st.trader_pf, &state.trader_pf)] {
        if !s.is_empty() {
            *pf = Pubkey::from_str(s)?;
        }
    }
    Ok((st, state, run, cluster.clone()))
}

// ------------------------------------------------------------ steps

pub const STEPS: [&str; 5] = ["init-market", "oracle", "insurance", "backing", "lp"];

/// `staging step <name> <config> <cluster> <run dir>`. Exit 10: stopped on
/// an unsigned multisig transaction (re-run after it lands).
pub fn step_main(args: &[String]) -> Result<()> {
    let name = args.first().map(String::as_str).unwrap_or("");
    if !STEPS.contains(&name) {
        bail!("unknown step {name:?}; steps: {}", STEPS.join(" "));
    }
    let (mut st, mut state, run, cluster) = open(&args[1..])?;
    if state.done.iter().any(|d| d == name) {
        println!("step {name}: already done");
        return Ok(());
    }
    state.program_id = st.program_id.to_string();
    let r = run_step(&mut st, &mut state, &run, &cluster, name);
    if r.is_ok() {
        state.done.push(name.into());
    }
    state.save(&run)?;
    match r {
        Ok(()) => println!("step {name}: done"),
        Err(e) if e.is::<Pending>() => {
            println!("step {name}: {e}");
            std::process::exit(10);
        }
        Err(e) => return Err(e.context(format!("step {name}"))),
    }
    Ok(())
}

fn run_step(st: &mut Staging, state: &mut State, run: &Path, cluster: &str, name: &str) -> Result<()> {
    if st.pyth_feed.is_some() {
        // InitMarket and AuthMark start at the live price.
        st.mark = st.source_price(0)?.0;
    }
    match name {
        "init-market" => {
            let bal = st.rpc.get_balance(&st.payer.pubkey())?;
            if cluster == "local" && bal < 100_000_000_000 {
                setup::rpc_airdrop(&st.rpc, &st.payer.pubkey(), 500_000_000_000)?;
            } else if bal < 1_000_000_000 {
                bail!("payer {} holds {bal} lamports; fund it (>= 10 SOL: market rent)", st.payer.pubkey());
            }
            if st.own_mint && !st.exists(&st.mint)? {
                let kp = key(run, "mint", true)?;
                setup::create_spl_mint(&st.rpc, &st.payer, &kp, &st.payer.pubkey(), 6).context("create test mint")?;
            }
            state.mint = st.mint.to_string();
            let mut owners = vec![st.admin_pubkey(), st.lp.pubkey()];
            if st.multisig.is_none() {
                owners.push(st.trader.pubkey());
            }
            for o in owners {
                let ata = spl_associated_token_account::get_associated_token_address(&o, &st.mint);
                if !st.exists(&ata)? {
                    setup::create_ata(&st.rpc, &st.payer, &o, &st.mint)?;
                }
            }
            let held: u64 = st.rpc.get_token_account_balance(&st.admin_ata)?.amount.parse()?;
            if st.own_mint && held == 0 {
                setup::mint_to(&st.rpc, &st.payer, &st.mint, &st.admin_ata, &st.payer, st.cfg.amounts.authority_test_mint)?;
            }
            st.create_market(&key(run, "market", true)?)?;
            state.market = st.market.to_string();
            state.save(run)?;
            st.init_market()
        }
        "oracle" => st.configure_oracle(),
        "insurance" => {
            let (_, _, s) = st.read()?;
            if s.insurance_top_up == 0 {
                st.top_up_insurance(st.cfg.amounts.insurance_seed)?;
            }
            Ok(())
        }
        "backing" => {
            let (_, g, _) = st.read()?;
            let empty: Vec<usize> =
                (0..2).filter(|&d| matches!(g.source_backing_buckets[d].status, Bucket::Empty | Bucket::Expired)).collect();
            if !empty.is_empty() {
                st.top_up_backing(&empty, st.rpc.get_slot()? + st.tenor)?;
            }
            Ok(())
        }
        "lp" => {
            // The matcher is off-chain (co-signed TradeNoCpi): no on-chain
            // matcher config, only the LP (and, off mainnet, the test
            // trader) portfolio with its deposit.
            let mut legs = vec![(true, st.cfg.amounts.lp_deposit)];
            if st.multisig.is_none() {
                legs.push((false, st.cfg.amounts.trader_deposit));
            }
            for (is_lp, amount) in legs {
                let (owner, slot) = if is_lp { (&st.lp, &mut state.lp_pf) } else { (&st.trader, &mut state.trader_pf) };
                if slot.is_empty() {
                    *slot = setup::create_portfolio(&st.rpc, &st.payer, owner, &st.market, st.program_id)?.to_string();
                }
                let pf = Pubkey::from_str(slot)?;
                let p = read_portfolio(&st.rpc.get_account_data(&pf)?).map_err(|e| anyhow!("read_portfolio: {e:?}"))?;
                if p.capital.get() == 0 {
                    st.deposit(owner, &pf, amount)?;
                }
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

/// `staging bots <config> <cluster> <run dir>`: roles 1-5 on the launched
/// market, logging in the format the monitor reads.
pub fn bots(args: &[String]) -> Result<Staging> {
    let (mut st, state, _, cluster) = open(args)?;
    if cluster == "mainnet" {
        bail!("mainnet bots run from deploy/ after the role split (docs/LAUNCH.md)");
    }
    if let Some(s) = STEPS.iter().find(|s| !state.done.iter().any(|d| d == *s)) {
        bail!("step {s} not done; run scripts/launch_market.sh first");
    }
    let (_, g, _) = st.read()?;
    (st.mark, st.feed) = (g.assets[0].effective_price, g.assets[0].effective_price);
    (st.keeper_log, st.phase) = (true, Phase::Live);
    Ok(st)
}
