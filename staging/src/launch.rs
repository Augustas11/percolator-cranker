//! Day-0 launch on top of the R2 staging code: the config file, resumable
//! setup steps with a state file, and bots on an existing market.
//! Reference / dry-run only; not a paid live service; M3 NO-GO.
//! Driven by `scripts/launch_market.sh`; see docs/LAUNCH.md.

use crate::{Phase, Staging};
use anyhow::{anyhow, bail, Context, Result};
use drip::{cycle::SubmissionRecord, setup, submit::CrankOutcome};
use percolator::MAX_VAULT_TVL;
use percolator_prog::{
    ix::Instruction as P,
    oracle_v16,
    processor::{ASSET_AUTH_ADMIN, ASSET_AUTH_BACKING_BUCKET, ASSET_AUTH_INSURANCE, ASSET_AUTH_INSURANCE_OPERATOR, ASSET_AUTH_ORACLE},
    state::{read_asset_oracle_profile, read_market, read_portfolio, AssetOracleProfileV16},
};
use percolator::BackingBucketStatusV16 as Bucket;
use serde::{Deserialize, Serialize};
use solana_client::{nonce_utils, rpc_client::RpcClient};
use solana_sdk::{
    bs58,
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    message::Message,
    pubkey::Pubkey,
    system_instruction,
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
    pub matcher_ctxs: Vec<String>,
    /// Recorded target for the matcher ctxs' max reference age (slots); applied
    /// by hand (docs/LAUNCH.md). Not read by the bots.
    #[serde(default)]
    pub matcher_max_ref_age_slots: u64,
    #[serde(default)]
    pub multisig_vault: String,
    /// Mainnet: the asset-0 role holders the `handover` step moves to
    /// (public keys, ops/KEYS.md). Local and devnet generate them in keys/.
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct OracleCfg {
    #[serde(default = "default_exchange_binance_url")]
    pub exchange_binance_url: String,
    #[serde(default = "default_exchange_clock_skew")]
    pub exchange_clock_skew_ms: u64,
    #[serde(default = "default_exchange_max_age")]
    pub exchange_max_age_ms: u64,
    #[serde(default = "default_exchange_max_dev")]
    pub exchange_max_dev_bps: u64,
    #[serde(default = "default_exchange_min_venues")]
    pub exchange_min_venues: usize,
    pub pyth_feed_id: String,
    pub pyth_price_account: String,
    /// Read-only RPC the Pyth price account is read from, when it is not the
    /// cluster's own (e.g. devnet pushing marks read from mainnet's sponsored
    /// SOL/USD account, which Pyth keeps fresh only on mainnet). Never sends.
    #[serde(default)]
    pub pyth_rpc: String,
    pub pyth_max_age_s: u64,
    pub pyth_max_conf_bps: u16,
    pub lag_target_ms: u64,
    pub push_interval_ms: u64,
    pub push_heartbeat_slots: u64,
    #[serde(default = "default_push_poll_ms")]
    pub push_poll_ms: u64,
    #[serde(default = "default_confirm_poll_ms")]
    pub confirm_poll_ms: u64,
    #[serde(default = "default_accrual_lag_slots")]
    pub accrual_lag_slots: u64,
    #[serde(default = "default_accrual_max_cranks")]
    pub accrual_max_cranks_per_tx: u64,
    #[serde(default = "default_push_min_move_bps")]
    pub push_min_move_bps: u64,
    #[serde(default = "default_priority_fee")]
    pub priority_fee_micro_lamports: u64,
    #[serde(default = "default_push_cu_limit")]
    pub push_cu_limit: u32,
    #[serde(default = "default_open_mark_age")]
    pub open_max_mark_age_ms: u64,
}

fn default_exchange_binance_url() -> String { "wss://stream.binance.com:9443/ws/solusdt@trade".into() }
fn default_exchange_clock_skew() -> u64 { 500 }
fn default_exchange_max_age() -> u64 { 2000 }
fn default_exchange_max_dev() -> u64 { 50 }
fn default_exchange_min_venues() -> usize { 2 }
fn default_accrual_lag_slots() -> u64 { 32 }
fn default_accrual_max_cranks() -> u64 { 10 }
fn default_confirm_poll_ms() -> u64 { 500 }
fn default_push_poll_ms() -> u64 { 400 }
fn default_push_min_move_bps() -> u64 { 1 }
fn default_priority_fee() -> u64 { 10_000 }
fn default_push_cu_limit() -> u32 { 14_000 }
fn default_open_mark_age() -> u64 { 1000 }

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
    pub program_id: String,
    pub size_q: i64,
    pub max_fill_notional_usd: u64,
    pub max_lp_inventory_usd: u64,
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

// ------------------------------------------------------------ handover

/// Marks the handover step waiting for co-signatures (exit 10).
#[derive(Debug)]
struct Pending(PathBuf);
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "waiting for co-signatures: {}", self.0.display())
    }
}
impl std::error::Error for Pending {}

/// Asset-0 roles in ops/KEYS.md's split order (marketauth goes last); the
/// name is the config key under `[cluster.<name>.roles]` and the keys/ file.
const ROLES: [(&str, u8); 5] = [
    ("oracle_authority", ASSET_AUTH_ORACLE),
    ("insurance_operator", ASSET_AUTH_INSURANCE_OPERATOR),
    ("backing_bucket_authority", ASSET_AUTH_BACKING_BUCKET),
    ("insurance_authority", ASSET_AUTH_INSURANCE),
    ("asset_admin", ASSET_AUTH_ADMIN),
];

const HANDOVER_README: &str = "# Handover transactions (mainnet)

Reference / dry-run only; not a paid live service; M3 NO-GO.

The launch key holds marketauth and every asset-0 role until these land
(ops/KEYS.md, \"Launch key\"). Land them IN FILE ORDER: each binds the next
authority epoch. Every file is base58 bincode of one legacy transaction.

- `NN-<role>.b58` (UpdateAssetAuthority, tag 65): already signed by the fee
  payer and the launch key, on a durable nonce (it does not expire). The NEW
  holder named in the file adds its signature (hardware wallet for
  insurance_operator, backing_bucket_authority, asset_admin; the pusher host
  key for oracle_authority; the Squads vault for insurance_authority, as a
  vault transaction), then anyone submits it.
- `06-marketauth.b58` (UpdateAuthority, tag 32): unsigned, the Squads vault
  as payer. Import into Squads as a vault transaction; the vault co-signs as
  the new marketauth and the LAUNCH KEY must also sign the execute
  transaction (it is the current marketauth). *To verify on devnet.*

Then re-run the same launch_market.sh command: it checks the chain, rewrites
what is still pending, and finishes only when the role table is clean.
";

fn holder(p: &AssetOracleProfileV16, kind: u8) -> [u8; 32] {
    match kind {
        ASSET_AUTH_ORACLE => p.oracle_authority,
        ASSET_AUTH_INSURANCE_OPERATOR => p.insurance_operator,
        ASSET_AUTH_BACKING_BUCKET => p.backing_bucket_authority,
        ASSET_AUTH_INSURANCE => p.insurance_authority,
        _ => p.asset_admin,
    }
}

/// Where each role must end: the 5 asset roles, then marketauth (the Squads
/// vault; local and devnet: a generated stand-in key, keys/multisig.json).
fn targets(st: &Staging, cluster: &str, run: &Path) -> Result<Vec<(&'static str, Pubkey)>> {
    let c = &st.cfg.cluster[cluster];
    let pk = |name: &str, v: Option<&String>| -> Result<Pubkey> {
        match cluster {
            "mainnet" => Pubkey::from_str(v.map(String::as_str).unwrap_or(""))
                .with_context(|| format!("[cluster.mainnet] {name} must be a public key")),
            _ => Ok(key(run, name, true)?.pubkey()),
        }
    };
    let mut out = Vec::new();
    for (name, _) in ROLES {
        out.push((name, pk(name, c.roles.get(name))?));
    }
    let vault = if cluster == "mainnet" { pk("multisig_vault", Some(&c.multisig_vault))? } else { pk("multisig", None)? };
    out.push(("marketauth", vault));
    Ok(out)
}

/// The on-chain role table against the targets. Ok only when every holder
/// is its target and none is the launch (bootstrap admin) key.
pub fn verify_roles(st: &Staging, cluster: &str, run: &Path) -> Result<()> {
    let data = st.rpc.get_account_data(&st.market)?;
    let (cfg, _) = read_market(&data).map_err(|e| anyhow!("read_market: {e:?}"))?;
    let p = read_asset_oracle_profile(&data, 0).map_err(|e| anyhow!("oracle profile: {e:?}"))?;
    let launch = st.admin.pubkey();
    let mut bad = 0;
    println!("{:<26} {:<45} ok", "role", "holder (on chain)");
    for ((name, want), kind) in targets(st, cluster, run)?.into_iter().zip(ROLES.iter().map(Some).chain([None])) {
        let got = Pubkey::new_from_array(kind.map_or(cfg.marketauth, |(_, k)| holder(&p, *k)));
        let ok = got == want && got != launch;
        bad += usize::from(!ok);
        println!("{name:<26} {got:<45} {}", if ok { "yes".into() } else { format!("NO (want {want})") });
    }
    if bad > 0 {
        bail!("{bad} role(s) not handed over; launch key {launch}");
    }
    Ok(())
}

/// Mainnet: one role change, partially signed (payer + launch key) on its
/// own durable nonce, written for the new holder to co-sign.
fn write_role_tx(st: &Staging, run: &Path, file: &Path, nonce_name: &str, ix: Instruction) -> Result<()> {
    let nonce = key(run, nonce_name, true)?;
    if !st.exists(&nonce.pubkey())? {
        let rent = st.rpc.get_minimum_balance_for_rent_exemption(solana_sdk::nonce::State::size())?;
        let ixs = system_instruction::create_nonce_account(&st.payer.pubkey(), &nonce.pubkey(), &st.admin.pubkey(), rent);
        let bh = st.rpc.get_latest_blockhash()?;
        let tx = Transaction::new_signed_with_payer(&ixs, Some(&st.payer.pubkey()), &[&st.payer, &nonce], bh);
        st.rpc.send_and_confirm_transaction(&tx).context("create nonce account")?;
    }
    let hash = nonce_utils::data_from_account(&st.rpc.get_account(&nonce.pubkey())?)?.blockhash();
    let ixs = [system_instruction::advance_nonce_account(&nonce.pubkey(), &st.admin.pubkey()), ix];
    let mut tx = Transaction::new_with_payer(&ixs, Some(&st.payer.pubkey()));
    tx.partial_sign(&[&st.payer, &st.admin], hash);
    std::fs::write(file, bs58::encode(bincode::serialize(&tx)?).into_string())?;
    Ok(())
}

/// The handover: asset-0 roles first, marketauth last (ops/KEYS.md split
/// order). Each change needs the new holder's co-signature (the wrapper's
/// `expect_incoming_authority`). Local/devnet: generated keys sign here.
/// Mainnet: written to `<run>/handover/` and the step stops (exit 10).
fn handover(st: &Staging, run: &Path, cluster: &str) -> Result<()> {
    let mainnet = cluster == "mainnet";
    let t = targets(st, cluster, run)?;
    if !mainnet && st.own_mint {
        // The bots' backing and insurance keys fund top-ups from their own
        // token accounts once they hold the roles.
        for (_, holder) in &t[2..4] {
            let ata = spl_associated_token_account::get_associated_token_address(holder, &st.mint);
            if !st.exists(&ata)? {
                setup::create_ata(&st.rpc, &st.payer, holder, &st.mint)?;
                setup::mint_to(&st.rpc, &st.payer, &st.mint, &ata, &st.payer, st.cfg.amounts.authority_test_mint / 2)?;
            }
        }
    }
    let dir = run.join("handover");
    if mainnet {
        std::fs::create_dir_all(&dir)?;
    }
    let (data, g, s) = st.read()?;
    let p = read_asset_oracle_profile(&data, 0).map_err(|e| anyhow!("oracle profile: {e:?}"))?;
    let (mut epoch, mut pending) = (s.authority_epoch, false);
    let metas = |new: &Pubkey| {
        vec![AccountMeta::new(st.admin.pubkey(), true), AccountMeta::new(*new, true), AccountMeta::new(st.market, false)]
    };
    for (i, ((name, kind), (_, new))) in ROLES.iter().zip(&t).enumerate() {
        if holder(&p, *kind) == new.to_bytes() {
            continue;
        }
        let ix = P::UpdateAssetAuthority { asset_index: 0, market_id: g.assets[0].market_id, authority_epoch: epoch, kind: *kind, new_pubkey: new.to_bytes() };
        let ix = st.prog_ix(ix, metas(new));
        if mainnet {
            write_role_tx(st, run, &dir.join(format!("{:02}-{name}.b58", i + 1)), &format!("nonce_{name}"), ix)?;
            pending = true;
        } else {
            st.send(&[ix], &[&st.admin, &key(run, name, false)?]).with_context(|| format!("UpdateAssetAuthority {name}"))?;
        }
        epoch += 1;
    }
    let vault = t[5].1;
    let (cfg, _) = read_market(&data).map_err(|e| anyhow!("read_market: {e:?}"))?;
    if cfg.marketauth != vault.to_bytes() {
        let ix = st.prog_ix(P::UpdateAuthority { authority_epoch: epoch, new_pubkey: vault.to_bytes() }, metas(&vault));
        if mainnet {
            let tx = Transaction::new_unsigned(Message::new(&[ix], Some(&vault)));
            std::fs::write(dir.join("06-marketauth.b58"), bs58::encode(bincode::serialize(&tx)?).into_string())?;
            pending = true;
        } else {
            st.send(&[ix], &[&st.admin, &key(run, "multisig", false)?]).context("UpdateAuthority")?;
        }
    }
    if pending {
        std::fs::write(dir.join("README.md"), HANDOVER_README)?;
        return Err(Pending(dir).into());
    }
    verify_roles(st, cluster, run)
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
    /// Genesis hash of the chain this launch ran on: a reset validator or
    /// another cluster cannot reuse the completed steps.
    #[serde(default)]
    genesis: String,
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
    /// Write a temp file and rename it, so a crash never leaves a torn state.
    fn save(&self, run: &Path) -> Result<()> {
        let tmp = run.join("state.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)? + "\n")?;
        std::fs::rename(&tmp, run.join("state.json"))?;
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
    // The chain decides mainnet, not the label: every entry point (script,
    // direct binary, hosting) is checked here before any key is used.
    let genesis = rpc.get_genesis_hash().context("read genesis hash")?.to_string();
    // LAUNCH_REHEARSE_MAINNET_PATH=1: run the mainnet code path (launch key,
    // co-sign handover files) on a NON-mainnet chain, to rehearse it on
    // local or devnet. It can only ever relax toward a test chain.
    let rehearse = mainnet && genesis != MAINNET_GENESIS && env("LAUNCH_REHEARSE_MAINNET_PATH").as_deref() == Some("1");
    if (genesis == MAINNET_GENESIS) != mainnet && !rehearse {
        bail!("RPC genesis {genesis} does not match cluster {cluster:?}");
    }
    if !state.genesis.is_empty() && state.genesis != genesis {
        bail!("{} belongs to chain {}; use a fresh run dir", run.display(), state.genesis);
    }
    let payer = key(&run, "payer", !mainnet)?;
    let lp = key(&run, "lp", !mainnet)?;
    // The admin holds marketauth from create + InitMarket to the handover.
    // Mainnet: a fresh launch key per run dir; no test trader.
    let admin = key(&run, if mainnet { "launch_authority" } else { "admin" }, true)?;
    let trader = if mainnet { Keypair::new() } else { key(&run, "trader", true)? };
    let (mint, own_mint) = match c.collateral_mint.as_str() {
        "create" if mainnet => bail!("mainnet needs a real collateral_mint"),
        "create" => (key(&run, "mint", true)?.pubkey(), true),
        m => (Pubkey::from_str(m).context("collateral_mint")?, false),
    };
    if !state.mint.is_empty() && state.mint != mint.to_string() {
        bail!("{} uses mint {}, the config says {mint}; use a fresh run dir", run.display(), state.mint);
    }
    let mut st = Staging::new(cfg, rpc, program_id, payer, admin, lp, trader, mint, own_mint);
    st.matcher_ctxs = c.matcher_ctxs.clone();
    match c.price_source.as_str() {
        "pyth" => st.pyth_feed = Some(feed_id(&st.cfg.oracle)?),
        "exchanges" => st.exchanges = Some(crate::pusher::Exchanges::new(&st.cfg.oracle)?),
        "mock" => {},
        other => bail!("unknown price_source {other:?}"),
    }
    if !state.market.is_empty() {
        st.set_market(Pubkey::from_str(&state.market)?);
    }
    for (pf, s) in [(&mut st.lp_pf, &state.lp_pf), (&mut st.trader_pf, &state.trader_pf)] {
        if !s.is_empty() {
            *pf = Pubkey::from_str(s)?;
        }
    }
    let mut state = state;
    state.genesis = genesis;
    Ok((st, state, run, cluster.clone()))
}

const MAINNET_GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";

// ------------------------------------------------------------ steps

pub const STEPS: [&str; 6] = ["init-market", "oracle", "insurance", "backing", "lp", "handover"];

/// `staging step <name> <config> <cluster> <run dir>`. Exit 10: the mainnet
/// handover waits for co-signatures (re-run after they land).
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
    if state.done.iter().any(|d| d == "handover") {
        bail!("{}: handover done, the launch key is retired; use a fresh run dir", run.display());
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
    if (st.pyth_feed.is_some() || st.exchanges.is_some()) && matches!(name, "init-market" | "oracle") {
        // InitMarket and AuthMark start at the live price; no other step
        // (handover least of all) needs a price.
        // Allow websocket subscriptions to produce the initial quorum; never
        // substitute the mock price when the live source is unavailable.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            match st.source_price(0) {
                Ok((price, _)) => { st.mark = price; break; },
                Err(_) if st.exchanges.is_some() && std::time::Instant::now() < deadline =>
                    std::thread::sleep(std::time::Duration::from_millis(100)),
                Err(e) => return Err(e),
            }
        }
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
            let mut owners = vec![st.admin.pubkey(), st.lp.pubkey()];
            if cluster != "mainnet" {
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
            let market = key(run, "market", true)?;
            state.market = market.pubkey().to_string();
            state.save(run)?;
            st.create_market(&market)
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
            if cluster != "mainnet" {
                legs.push((false, st.cfg.amounts.trader_deposit));
            }
            for (is_lp, amount) in legs {
                // The portfolio key is saved before anything is sent, and
                // creation + InitPortfolio is one tx, so a crash or a lost
                // state.json resumes on the same account: never a second
                // portfolio or a second deposit.
                let kp = key(run, if is_lp { "lp_portfolio" } else { "trader_portfolio" }, true)?;
                let (owner, slot) = if is_lp { (&st.lp, &mut state.lp_pf) } else { (&st.trader, &mut state.trader_pf) };
                *slot = kp.pubkey().to_string();
                if !st.exists(&kp.pubkey())? {
                    st.create_portfolio(owner, &kp)?;
                }
                let pf = kp.pubkey();
                let p = read_portfolio(&st.rpc.get_account_data(&pf)?).map_err(|e| anyhow!("read_portfolio: {e:?}"))?;
                if p.capital.get() == 0 {
                    st.deposit(owner, &pf, amount)?;
                }
            }
            Ok(())
        }
        "handover" => handover(st, run, cluster),
        _ => unreachable!(),
    }
}

/// `staging bots <config> <cluster> <run dir>`: roles 1-5 on the launched
/// market, logging in the format the monitor reads.
pub fn bots(args: &[String]) -> Result<Staging> {
    let (mut st, state, run, cluster) = open(args)?;
    if let Some(s) = STEPS.iter().find(|s| !state.done.iter().any(|d| d == *s)) {
        bail!("step {s} not done; run scripts/launch_market.sh first");
    }
    verify_roles(&st, &cluster, &run).context("handover not verified on chain; bots refuse")?;
    // Role keys on this host that hold their role on chain: the pusher's is
    // required; backing and insurance run only where their key is.
    let (data, g, _) = st.read()?;
    let p = read_asset_oracle_profile(&data, 0).map_err(|e| anyhow!("oracle profile: {e:?}"))?;
    let load = |name: &str, kind: u8| key(&run, name, false).ok().filter(|k| k.pubkey().to_bytes() == holder(&p, kind));
    st.oracle_kp = load("oracle_authority", ASSET_AUTH_ORACLE).context("keys/oracle_authority.json missing or not the on-chain oracle")?;
    st.backing_kp = load("backing_bucket_authority", ASSET_AUTH_BACKING_BUCKET);
    st.insurance_kp = load("insurance_authority", ASSET_AUTH_INSURANCE);
    (st.mark, st.feed) = (g.assets[0].effective_price, g.assets[0].effective_price);
    // A restart keeps managing an open trader position: the matcher only ever
    // holds 0 or one `size_q` either way, so the leg's sign gives it back.
    let q = if st.trader_pf == Pubkey::default() {
        0 // mainnet: no test trader, the matcher is idle
    } else {
        read_portfolio(&st.rpc.get_account_data(&st.trader_pf)?).map_err(|e| anyhow!("read_portfolio: {e:?}"))?.legs[0].basis_pos_q.get()
    };
    if q != 0 {
        st.open = Some(q.signum() * st.cfg.matcher.size_q as i128);
        println!("bots: trader holds a position ({q}); the matcher closes it first");
    }
    (st.keeper_log, st.phase) = (true, Phase::Live);
    Ok(st)
}

pub fn lifecycle_main(args: &[String]) -> Result<()> {
    use crate::{binding, cycle, CrankCtx, CuStats, CycleBudget, MarketModeV16, Quarantine, QuarantineConfig, BOUND_SCALE};
    use serde_json::json;
    use std::{io::Write, thread::sleep, time::{Duration, Instant}};

    if args.get(1).map(String::as_str) == Some("mainnet") {
        bail!("lifecycle refuses mainnet");
    }
    let (mut st, state, run, _) = open(args)?;
    if let Some(s) = STEPS.iter().find(|s| !state.done.iter().any(|d| d == *s)) {
        bail!("step {s} not done; run scripts/launch_market.sh first");
    }
    st.admin = key(&run, "multisig", false)?;
    st.admin_ata = spl_associated_token_account::get_associated_token_address(&st.admin.pubkey(), &st.mint);
    let emit = |value: serde_json::Value| -> Result<()> {
        let line = serde_json::to_string(&value)?;
        writeln!(std::fs::OpenOptions::new().create(true).append(true).open(run.join("lifecycle.jsonl"))?, "{line}")?;
        println!("{line}");
        Ok(())
    };
    let record = |st: &Staging, phase: &str, extra: serde_json::Value| -> Result<()> {
        let mut value = json!({"event":"lifecycle", "phase":phase,
            "sig":st.last_sig.get().map(|s| s.to_string()).unwrap_or_default(),
            "slot":st.rpc.get_slot()?, "unix_ms":now_ms()});
        value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        emit(value)
    };
    let retired = |st: &Staging| -> Result<bool> {
        Ok(st.rpc.get_account_with_commitment(&st.market, CommitmentConfig::confirmed())?.value
            .is_none_or(|a| a.data.len() <= percolator_prog::constants::HEADER_LEN))
    };
    let send = |st: &Staging, ixs: &[Instruction], signer: &Keypair| -> Result<()> {
        if !st.simulates(ixs, &[signer]) {
            let r = st.rpc.simulate_transaction(&st.tx(ixs, &[signer])?)?;
            bail!("lifecycle simulation: {:?}; logs: {:?}", r.value.err, r.value.logs);
        }
        st.send(ixs, &[signer])
    };
    // A retired slab has only a tombstone, not a readable market config.
    if !retired(&st)? {
        let (data, g, _) = st.read()?;
        let (cfg, _) = read_market(&data).map_err(|e| anyhow!("read_market: {e:?}"))?;
        if cfg.marketauth != st.admin.pubkey().to_bytes() {
            bail!("keys/multisig.json is not the on-chain marketauth");
        }
        (st.mark, st.feed) = (g.assets[0].effective_price, g.assets[0].effective_price);
        if g.mode == MarketModeV16::Live && g.assets[0].lifecycle != percolator::AssetLifecycleV16::Recovery {
            st.shutdown()?;
            record(&st, "recovery", json!({}))?;
        }
        if st.read()?.1.mode == MarketModeV16::Live {
            let started = Instant::now();
            loop {
                let (data, g, s) = st.read()?;
                let mut ixs = st.fresh_cranks(&data, &[st.trader_pf, st.lp_pf], st.rpc.get_slot()?)?;
                ixs.push(st.prog_ix(P::ResolveMarket {
                    asset_generation_frontier: g.next_market_id, authority_epoch: s.authority_epoch,
                }, st.admin_metas()));
                if st.simulates(&ixs, &[&st.admin]) {
                    st.send(&ixs, &[&st.admin]).context("ResolveMarket")?;
                    record(&st, "resolved", json!({}))?;
                    break;
                }
                if started.elapsed() >= Duration::from_secs(120) {
                    let r = st.rpc.simulate_transaction(&st.tx(&ixs, &[&st.admin])?)?;
                    bail!("ResolveMarket timed out: {:?}; logs: {:?}", r.value.err, r.value.logs);
                }
                sleep(Duration::from_secs(2));
            }
        }
        if st.read()?.1.mode != MarketModeV16::Resolved {
            bail!("lifecycle requires a Live or Resolved market");
        }
        let budget = CycleBudget::from_env();
        let mut quarantine = Quarantine::new(QuarantineConfig::from_env());
        let mut cu = CuStats::default();
        for tick in 0..200 {
            if st.portfolio_settled(&st.lp_pf)? && st.portfolio_settled(&st.trader_pf)? { break; }
            let ctx = CrankCtx {
                blockhash: &st.blockhash, rpc: &st.rpc, program_id: &st.program_id,
                payer: &st.payer, market: &st.market, reward_portfolio: None,
                pyth: &st.pyth, exclude: &[],
            };
            cycle::run_one_cycle(&ctx, tick, &budget, &mut quarantine, &mut Vec::new(), &mut cu)?;
            sleep(Duration::from_secs(3));
        }
        if !(st.portfolio_settled(&st.lp_pf)? && st.portfolio_settled(&st.trader_pf)?) {
            bail!("portfolios still open after 200 cycles");
        }
        st.last_sig.set(None);
        record(&st, "paid_out", json!({}))?;
        for pf in [st.lp_pf, st.trader_pf] {
            let Some(a) = st.rpc.get_account_with_commitment(&pf, CommitmentConfig::confirmed())?.value else { continue };
            read_portfolio(&a.data).map_err(|e| anyhow!("read_portfolio {pf}: {e:?}"))?;
            let b = binding::portfolio_binding(&a.data)?;
            let ix = st.prog_ix(P::ClosePortfolio {
                portfolio_id: b.portfolio_id, expected_sequence: b.sequence, position_epoch: b.position_epoch,
            }, vec![AccountMeta::new(st.admin.pubkey(), true), AccountMeta::new(st.market, false), AccountMeta::new(pf, false)]);
            send(&st, &[ix], &st.admin).context("ClosePortfolio")?;
            record(&st, "portfolio_closed", json!({"portfolio":pf.to_string()}))?;
        }
        for d in 0..2 {
            let (_, g, s) = st.read()?;
            let b = &g.source_backing_buckets[d];
            let amount = b.fresh_unliened_backing_num / BOUND_SCALE;
            if b.status != Bucket::Fresh || amount == 0 { continue; }
            let k = key(&run, "backing_bucket_authority", false)?;
            let ata = spl_associated_token_account::get_associated_token_address(&k.pubkey(), &st.mint);
            if st.rpc.get_account_with_commitment(&ata, CommitmentConfig::confirmed())?.value.is_none() {
                setup::create_ata(&st.rpc, &st.payer, &k.pubkey(), &st.mint)?;
            }
            let mut metas = st.role_metas(&k);
            metas.extend([
                AccountMeta::new(ata, false), AccountMeta::new(st.vault, false),
                AccountMeta::new_readonly(st.vault_auth, false), AccountMeta::new_readonly(spl_token::ID, false),
            ]);
            let ix = st.prog_ix(P::WithdrawBackingBucket {
                domain: d as u16, market_id: g.assets[0].market_id,
                authority_epoch: s.authority_epoch, amount,
            }, metas);
            send(&st, &[ix], &k).context("WithdrawBackingBucket")?;
            record(&st, "backing_withdrawn", json!({"domain":d,"amount":amount}))?;
        }
        let (_, g, s) = st.read()?;
        let amount = g.insurance_domain_budget_remaining_total;
        if amount > 0 {
            let k = key(&run, "insurance_authority", false)?;
            let ata = spl_associated_token_account::get_associated_token_address(&k.pubkey(), &st.mint);
            if st.rpc.get_account_with_commitment(&ata, CommitmentConfig::confirmed())?.value.is_none() {
                setup::create_ata(&st.rpc, &st.payer, &k.pubkey(), &st.mint)?;
            }
            let mut metas = st.role_metas(&k);
            metas.extend([
                AccountMeta::new(ata, false), AccountMeta::new(st.vault, false),
                AccountMeta::new_readonly(st.vault_auth, false), AccountMeta::new_readonly(spl_token::ID, false),
            ]);
            let ix = st.prog_ix(P::WithdrawInsuranceAsset {
                asset_index: 0, market_id: g.assets[0].market_id,
                authority_epoch: s.authority_epoch, amount,
            }, metas);
            send(&st, &[ix], &k).context("WithdrawInsuranceAsset")?;
            record(&st, "insurance_withdrawn", json!({"amount":amount}))?;
        }
        if st.rpc.get_account_with_commitment(&st.admin_ata, CommitmentConfig::confirmed())?.value.is_none() {
            setup::create_ata(&st.rpc, &st.payer, &st.admin.pubkey(), &st.mint)?;
        }
        for _ in 0..20 {
            if retired(&st)? { break; }
            let b = binding::market_authority_binding(&st.rpc.get_account_data(&st.market)?)?;
            let ix = st.prog_ix(P::CloseSlab { authority_epoch: b.authority_epoch }, vec![
                AccountMeta::new(st.admin.pubkey(), true), AccountMeta::new(st.market, false),
                AccountMeta::new(st.vault, false), AccountMeta::new_readonly(st.vault_auth, false),
                AccountMeta::new(st.admin_ata, false), AccountMeta::new_readonly(spl_token::ID, false),
                AccountMeta::new(st.mint, false),
            ]);
            send(&st, &[ix], &st.admin).context("CloseSlab")?;
            record(&st, "slab_closed", json!({"retired":retired(&st)?}))?;
        }
        if !retired(&st)? { bail!("market still active after 20 CloseSlab calls"); }
    }
    let lamports = st.rpc.get_balance(&st.admin.pubkey())?;
    if lamports != 0 {
        let a = st.rpc.get_account(&st.admin.pubkey())?;
        if a.owner != solana_sdk::system_program::ID || !a.data.is_empty() || st.admin.pubkey() == st.payer.pubkey() {
            bail!("marketauth must be a plain system account distinct from payer");
        }
        st.send(&[system_instruction::transfer(&st.admin.pubkey(), &st.payer.pubkey(), lamports)], &[&st.admin])?;
        record(&st, "swept", json!({"lamports":lamports}))?;
    }
    let account = |key: &Pubkey| st.rpc.get_account_with_commitment(key, CommitmentConfig::confirmed()).map(|r| r.value);
    let tokens = |key: &Pubkey| -> Result<u64> {
        if account(key)?.is_none() { return Ok(0); }
        Ok(st.rpc.get_token_account_balance(key)?.amount.parse()?)
    };
    let market = account(&st.market)?;
    let vault = account(&st.vault)?;
    let lp_pf = account(&st.lp_pf)?;
    let trader_pf = account(&st.trader_pf)?;
    emit(json!({"event":"lifecycle_final", "slot":st.rpc.get_slot()?, "unix_ms":now_ms(),
        "market":{"address":st.market.to_string(), "lamports":market.as_ref().map_or(0, |a| a.lamports),
            "data_len":market.as_ref().map_or(0, |a| a.data.len()), "retired":retired(&st)?},
        "vault":{"exists":vault.is_some(), "token_amount":tokens(&st.vault)?},
        "lp_pf":{"exists":lp_pf.is_some(), "lamports":lp_pf.map_or(0, |a| a.lamports)},
        "trader_pf":{"exists":trader_pf.is_some(), "lamports":trader_pf.map_or(0, |a| a.lamports)},
        "lp_tokens":tokens(&spl_associated_token_account::get_associated_token_address(&st.lp.pubkey(), &st.mint))?,
        "trader_tokens":tokens(&spl_associated_token_account::get_associated_token_address(&st.trader.pubkey(), &st.mint))?,
        "marketauth_tokens":tokens(&st.admin_ata)?, "payer_lamports":st.rpc.get_balance(&st.payer.pubkey())?}))
}

/// `staging roles <config> <cluster> <run dir>`: print the on-chain role
/// table; fail unless the handover is complete (the health check).
pub fn roles_main(args: &[String]) -> Result<()> {
    let (st, _, run, cluster) = open(args)?;
    verify_roles(&st, &cluster, &run)
}

/// An RPC URL without credentials (paid endpoints carry them in the path,
/// the query or userinfo): scheme and host only.
pub fn redact(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    if scheme.is_empty() { host.to_string() } else { format!("{scheme}://{host}/…") }
}

/// True when the RPC serves mainnet (its genesis hash).
pub fn is_mainnet(rpc: &RpcClient) -> Result<bool> {
    Ok(rpc.get_genesis_hash().context("read genesis hash")?.to_string() == MAINNET_GENESIS)
}

/// Error text with every URL cut to its host (the keeper's own scrubber).
pub fn scrub(msg: &str) -> String {
    drip::submit::scrub_urls(msg)
}

/// The RPC to read the Pyth price account from: `oracle.pyth_rpc` when set
/// (read-only), else the cluster's.
pub fn pyth_rpc_url(o: &OracleCfg, cluster_rpc: &str) -> String {
    if o.pyth_rpc.is_empty() { cluster_rpc.to_string() } else { o.pyth_rpc.clone() }
}

#[cfg(test)]
mod config_tests {
    #[test]
    fn confirmation_poll_defaults_and_override() {
        let config = include_str!("../../launch/sol-perp.toml");
        for (line, expected) in [("", 500), ("confirm_poll_ms = 750", 750)] {
            let text = config.replace("confirm_poll_ms = 500", line);
            let cfg: super::Config = toml::from_str(&text).unwrap();
            assert_eq!(cfg.oracle.confirm_poll_ms, expected);
        }
    }

    /// The shipped config must load (deny_unknown_fields catches stray keys).
    #[test]
    fn shipped_config_loads() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../launch/sol-perp.toml");
        let cfg = super::load(path).expect("launch/sol-perp.toml loads");
        assert_eq!(cfg.cluster["devnet"].matcher_max_ref_age_slots, 80);
        assert_eq!(cfg.oracle.exchange_binance_url, "wss://stream.binance.com:9443/ws/solusdt@trade");
    }
}
