//! One read-only snapshot of a market plus the keeper status file.
//!
//! Everything on chain is read with plain `getAccountInfo` /
//! `getProgramAccounts` and decoded with the wrapper's own decoders
//! (`read_market`, `read_portfolio`). The one header field the host mirror
//! lacks (`source_fresh_backing_total_num`) is read from the raw engine
//! header (`MarketGroupV16HeaderAccount` at `MARKET_GROUP_OFF`), the same
//! bytemuck type the wrapper uses. Nothing here signs or sends anything.
//!
//! Field meanings and engine citations: `docs/MONITOR.md`.

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use percolator::{
    AssetLifecycleV16, BackingBucketStatusV16, MarketGroupV16HeaderAccount, MarketModeV16,
    PortfolioAccountV16Account, ResolvedPayoutLedgerV16, BOUND_SCALE,
};
use percolator_prog::{
    constants::{MARKET_GROUP_LEN, MARKET_GROUP_OFF},
    state::{read_market, read_portfolio},
};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{program_pack::Pack, pubkey::Pubkey};

use super::json::quote;
use super::status::{now_unix_ms, KeeperStatus};
use crate::binding::V16_LAYOUT_DISCRIMINATOR;
use crate::discovery;
use crate::work;

#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub market: Pubkey,
    pub program: Pubkey,
    /// A Fresh bucket expiring within this many slots counts as "expiring soon".
    pub expiry_warn_slots: u64,
    pub status_file: Option<PathBuf>,
}

pub fn mode_label(m: MarketModeV16) -> &'static str {
    match m {
        MarketModeV16::Live => "live",
        MarketModeV16::Resolved => "resolved",
        MarketModeV16::Recovery => "recovery",
    }
}

pub const MODES: [&str; 3] = ["live", "resolved", "recovery"];

pub fn lifecycle_label(l: AssetLifecycleV16) -> &'static str {
    match l {
        AssetLifecycleV16::Disabled => "disabled",
        AssetLifecycleV16::PendingActivation => "pending_activation",
        AssetLifecycleV16::Active => "active",
        AssetLifecycleV16::DrainOnly => "drain_only",
        AssetLifecycleV16::Retired => "retired",
        AssetLifecycleV16::Recovery => "recovery",
    }
}

pub const LIFECYCLES: [&str; 6] = [
    "disabled",
    "pending_activation",
    "active",
    "drain_only",
    "retired",
    "recovery",
];

pub fn bucket_status_label(s: BackingBucketStatusV16) -> &'static str {
    match s {
        BackingBucketStatusV16::Empty => "empty",
        BackingBucketStatusV16::Fresh => "fresh",
        BackingBucketStatusV16::Expired => "expired",
        BackingBucketStatusV16::Impaired => "impaired",
    }
}

pub const BUCKET_STATUSES: [&str; 4] = ["fresh", "impaired", "expired", "empty"];

// ---------------------------------------------------------------- pure math

/// Engine header terms of `residual()` plus the junior claims.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HeaderTerms {
    pub vault: u128,
    pub c_tot: u128,
    pub insurance: u128,
    pub backing_provider_earnings_total: u128,
    pub source_fresh_backing_total_num: u128,
    pub pnl_pos_tot: u128,
    pub pnl_pos_bound_tot: u128,
}

impl HeaderTerms {
    pub fn from_header(h: &MarketGroupV16HeaderAccount) -> Self {
        Self {
            vault: h.vault.get(),
            c_tot: h.c_tot.get(),
            insurance: h.insurance.get(),
            backing_provider_earnings_total: h.backing_provider_earnings_total.get(),
            source_fresh_backing_total_num: h.source_fresh_backing_total_num.get(),
            pnl_pos_tot: h.pnl_pos_tot.get(),
            pnl_pos_bound_tot: h.pnl_pos_bound_tot.get(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Unowned {
    pub terms: HeaderTerms,
    /// `source_fresh_backing_total_num / BOUND_SCALE` (floor, as the engine).
    pub source_fresh_backing_total_atoms: u128,
    /// Sum of the senior claims (saturating, as the engine).
    pub senior: u128,
    /// Engine `residual()`: `vault − senior`, saturating at 0.
    pub residual: u128,
    /// `vault < senior`: residual saturated; the shortfall is `senior − vault`.
    pub senior_shortfall: u128,
    /// `residual − pnl_pos_tot` (signed: negative = junior claims exceed residual).
    pub unowned: i128,
    /// `residual − pnl_pos_bound_tot` (signed).
    pub unowned_vs_bound: i128,
}

fn signed_diff(a: u128, b: u128) -> i128 {
    if a >= b {
        i128::try_from(a - b).unwrap_or(i128::MAX)
    } else {
        i128::try_from(b - a).map(|d| -d).unwrap_or(i128::MIN)
    }
}

/// Unowned collateral by the engine's own residual definition
/// (`residual()`, engine `src/v16.rs:9061` at 4db11a8c).
pub fn unowned(t: HeaderTerms) -> Unowned {
    let source_fresh_backing_total_atoms = t.source_fresh_backing_total_num / BOUND_SCALE;
    let senior = t
        .c_tot
        .saturating_add(t.insurance)
        .saturating_add(t.backing_provider_earnings_total)
        .saturating_add(source_fresh_backing_total_atoms);
    let residual = t.vault.saturating_sub(senior);
    Unowned {
        terms: t,
        source_fresh_backing_total_atoms,
        senior,
        residual,
        senior_shortfall: senior.saturating_sub(t.vault),
        unowned: signed_diff(residual, t.pnl_pos_tot),
        unowned_vs_bound: signed_diff(residual, t.pnl_pos_bound_tot),
    }
}

/// Exact `floor(a · b / d)` with a 256-bit intermediate. `None` when
/// `d == 0` or the quotient does not fit in u128.
pub fn mul_div_floor(a: u128, b: u128, d: u128) -> Option<u128> {
    if d == 0 {
        return None;
    }
    if let Some(p) = a.checked_mul(b) {
        return Some(p / d);
    }
    // 128×128 → 256 via 64-bit limbs.
    let (a1, a0) = (a >> 64, a & u64::MAX as u128);
    let (b1, b0) = (b >> 64, b & u64::MAX as u128);
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 64) + (p01 & u64::MAX as u128) + (p10 & u64::MAX as u128);
    let lo = (p00 & u64::MAX as u128) | (mid << 64);
    let hi = p11 + (p01 >> 64) + (p10 >> 64) + (mid >> 64);
    if hi >= d {
        return None;
    }
    // Restoring division of (hi, lo) by d, one bit at a time.
    let mut rem = hi;
    let mut q: u128 = 0;
    for i in (0..128).rev() {
        let carry = rem >> 127;
        rem = (rem << 1) | ((lo >> i) & 1);
        q <<= 1;
        if carry == 1 || rem >= d {
            rem = rem.wrapping_sub(d);
            q |= 1;
        }
    }
    Some(q)
}

/// Resolved-payout figures aggregated over portfolios.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedClaims {
    pub portfolios: u64,
    /// Portfolios with capital > 0, positive PnL, or an open receipt.
    pub claimants_unpaid: u64,
    pub receipts_open: u64,
    pub receipts_finalized: u64,
    /// Σ (terminal_positive_claim_face − paid_effective) over open receipts. Exact.
    pub receipt_face_remaining_atoms: u128,
    /// Σ (floor(face · rate_num / rate_den) − paid_effective): what open
    /// receipts can claim at the ledger's current rate. Exact; 0 while halted.
    pub receipt_claimable_now_atoms: u128,
    /// Σ capital of every portfolio still open. Exact field sum.
    pub capital_atoms: u128,
    /// Σ max(pnl, 0) of portfolios without a receipt (face, not payout).
    pub unreceipted_positive_pnl_atoms: u128,
    /// Σ |min(pnl, 0)| (losses still to settle against capital).
    pub negative_pnl_atoms: u128,
    pub ledger_payout_halted: bool,
    pub ledger_finalized: bool,
    pub ledger_rate_num: u128,
    pub ledger_rate_den: u128,
    pub ledger_snapshot_residual: u128,
}

pub fn resolved_claims<'a>(
    portfolios: impl Iterator<Item = &'a PortfolioAccountV16Account>,
    l: &ResolvedPayoutLedgerV16,
) -> ResolvedClaims {
    let mut r = ResolvedClaims {
        ledger_payout_halted: l.payout_halted,
        ledger_finalized: l.finalized,
        ledger_rate_num: l.current_payout_rate_num,
        ledger_rate_den: l.current_payout_rate_den,
        ledger_snapshot_residual: l.snapshot_residual,
        ..ResolvedClaims::default()
    };
    for p in portfolios {
        r.portfolios += 1;
        let capital = p.capital.get();
        let pnl = p.pnl.get();
        let rc = &p.resolved_payout_receipt;
        let present = rc.present != 0;
        let finalized = rc.finalized != 0;
        r.capital_atoms = r.capital_atoms.saturating_add(capital);
        if pnl < 0 {
            r.negative_pnl_atoms = r.negative_pnl_atoms.saturating_add(pnl.unsigned_abs());
        }
        if present {
            let face = rc.terminal_positive_claim_face.get();
            let paid = rc.paid_effective.get();
            if finalized {
                r.receipts_finalized += 1;
            } else {
                r.receipts_open += 1;
                r.receipt_face_remaining_atoms = r
                    .receipt_face_remaining_atoms
                    .saturating_add(face.saturating_sub(paid));
                if !l.payout_halted {
                    if let Some(gross) =
                        mul_div_floor(face, l.current_payout_rate_num, l.current_payout_rate_den)
                    {
                        r.receipt_claimable_now_atoms = r
                            .receipt_claimable_now_atoms
                            .saturating_add(gross.saturating_sub(paid));
                    }
                }
            }
        } else if pnl > 0 {
            r.unreceipted_positive_pnl_atoms =
                r.unreceipted_positive_pnl_atoms.saturating_add(pnl as u128);
        }
        if capital > 0 || pnl > 0 || (present && !finalized) {
            r.claimants_unpaid += 1;
        }
    }
    r
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketView {
    pub asset: usize,
    pub side: &'static str,
    pub domain: usize,
    pub status: &'static str,
    pub expiry_slot: u64,
    /// `expiry_slot − now_slot` (negative = past expiry).
    pub slots_to_expiry: i128,
    pub fresh_unliened_backing_num: u128,
    pub valid_liened_backing_num: u128,
    /// Some portfolio's `source_domains` holds an occupied entry for this domain.
    pub referenced: bool,
}

impl BucketView {
    pub fn fresh(&self) -> bool {
        self.status == "fresh"
    }
    /// Engine lapse test: Fresh and `expiry_slot <= slot`.
    pub fn fresh_past_expiry(&self) -> bool {
        self.fresh() && self.slots_to_expiry <= 0
    }
    pub fn fresh_expiring_within(&self, n: u64) -> bool {
        self.fresh() && self.slots_to_expiry > 0 && self.slots_to_expiry <= n as i128
    }
    pub fn expiring_with_liens(&self, n: u64) -> bool {
        self.fresh() && self.valid_liened_backing_num > 0 && self.slots_to_expiry <= n as i128
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BucketCounts {
    pub fresh: u64,
    pub impaired: u64,
    pub expired: u64,
    pub empty: u64,
    pub fresh_expiring_soon: u64,
    pub fresh_past_expiry: u64,
    /// Fresh past expiry while the market is Live (the known wedge condition).
    pub fresh_past_expiry_in_live: u64,
    /// …and no portfolio references the domain (no Live path can expire it).
    pub fresh_past_expiry_in_live_unreferenced: u64,
    pub expiring_with_liens: u64,
}

pub fn bucket_counts(buckets: &[BucketView], live: bool, warn_slots: u64) -> BucketCounts {
    let mut c = BucketCounts::default();
    for b in buckets {
        match b.status {
            "fresh" => c.fresh += 1,
            "impaired" => c.impaired += 1,
            "expired" => c.expired += 1,
            _ => c.empty += 1,
        }
        c.fresh_expiring_soon += b.fresh_expiring_within(warn_slots) as u64;
        if b.fresh_past_expiry() {
            c.fresh_past_expiry += 1;
            if live {
                c.fresh_past_expiry_in_live += 1;
                c.fresh_past_expiry_in_live_unreferenced += (!b.referenced) as u64;
            }
        }
        c.expiring_with_liens += b.expiring_with_liens(warn_slots) as u64;
    }
    c
}

// ---------------------------------------------------------------- snapshot

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountCounts {
    /// Portfolios of this market found by discovery (layout-checked).
    pub total: u64,
    pub decode_errors: u64,
    /// Engine header `materialized_portfolio_count`.
    pub materialized_portfolio_count: u64,
    /// Portfolios with `stale_state != 0`.
    pub stale_state: u64,
    /// Engine header `stale_certificate_count`.
    pub stale_certificate_count: u64,
    /// Portfolios with `b_stale_state != 0`.
    pub b_stale_state: u64,
    /// Engine header `b_stale_account_count`.
    pub b_stale_account_count: u64,
    /// Σ over assets/sides of the engine's `stale_account_count_{long,short}`.
    pub kf_stale_cohort_engine: u64,
    /// Active legs with `kf_epoch_snap < kf_epoch_{side}` (portfolio side).
    pub kf_stale_cohort_observed: u64,
    /// Portfolios with ≥ 1 of the above engine flags/stale legs (union).
    pub out_of_date: u64,
    /// Keeper's off-chain priority estimate (`work::assess`): not engine-defined.
    pub pending_estimate: u64,
    pub liquidatable_estimate: u64,
    /// Active legs whose only gap is a sub-atom F remainder. NOT pending work.
    pub f_subatom_legs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetView {
    pub asset: usize,
    pub lifecycle: &'static str,
    /// `AssetStateV16.slot_last`: last accrual slot.
    pub slot_last: u64,
    pub stale_account_count_long: u64,
    pub stale_account_count_short: u64,
    pub stale_legs_observed_long: u64,
    pub stale_legs_observed_short: u64,
}

#[derive(Debug, Clone)]
pub struct ChainSnapshot {
    pub slot: u64,
    pub header_current_slot: u64,
    pub mode: &'static str,
    pub accounts: AccountCounts,
    pub assets: Vec<AssetView>,
    pub buckets: Vec<BucketView>,
    pub bucket_counts: BucketCounts,
    pub unowned: Unowned,
    /// Σ over domains of the mirror's `source_credit[d].fresh_reserved_backing_num`,
    /// the quantity the engine aggregates into `source_fresh_backing_total_num`.
    /// Cross-check for the raw header read.
    pub source_fresh_backing_sum_domains_num: u128,
    pub spl_vault: Pubkey,
    /// SPL token balance of the market vault (`None` if unreadable).
    pub spl_vault_balance: Option<u64>,
    pub resolved: Option<ResolvedClaims>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub taken_unix_ms: u64,
    pub market: Pubkey,
    pub program: Pubkey,
    pub expiry_warn_slots: u64,
    pub chain: Result<ChainSnapshot, String>,
    /// `None` when no status file is configured.
    pub keeper: Option<Result<KeeperStatus, String>>,
    /// Seconds since the status file's `written_unix_ms`.
    pub keeper_age_seconds: Option<f64>,
}

impl Snapshot {
    pub fn collect(rpc: &RpcClient, cfg: &MonitorConfig) -> Self {
        let taken_unix_ms = now_unix_ms();
        let keeper = cfg.status_file.as_ref().map(|p| KeeperStatus::read(p));
        let keeper_age_seconds = match &keeper {
            Some(Ok(k)) => Some(taken_unix_ms.saturating_sub(k.written_unix_ms) as f64 / 1000.0),
            _ => None,
        };
        Self {
            taken_unix_ms,
            market: cfg.market,
            program: cfg.program,
            expiry_warn_slots: cfg.expiry_warn_slots,
            chain: collect_chain(rpc, cfg).map_err(|e| format!("{e:#}")),
            keeper,
            keeper_age_seconds,
        }
    }

    pub fn keeper_ok(&self) -> Option<&KeeperStatus> {
        self.keeper.as_ref().and_then(|k| k.as_ref().ok())
    }
}

pub fn read_header(market_data: &[u8]) -> Result<MarketGroupV16HeaderAccount> {
    let bytes = market_data
        .get(MARKET_GROUP_OFF..MARKET_GROUP_OFF + MARKET_GROUP_LEN)
        .ok_or_else(|| anyhow!("market account too short for the engine header"))?;
    debug_assert_eq!(
        MARKET_GROUP_LEN,
        std::mem::size_of::<MarketGroupV16HeaderAccount>()
    );
    Ok(bytemuck::pod_read_unaligned::<MarketGroupV16HeaderAccount>(
        bytes,
    ))
}

fn collect_chain(rpc: &RpcClient, cfg: &MonitorConfig) -> Result<ChainSnapshot> {
    let market_data = rpc
        .get_account_data(&cfg.market)
        .context("read market account")?;
    let (wcfg, group) = read_market(&market_data).map_err(|e| anyhow!("read_market: {e:?}"))?;
    let header = read_header(&market_data)?;
    let slot = rpc.get_slot().context("getSlot")?;

    let mut accounts = AccountCounts {
        materialized_portfolio_count: group.materialized_portfolio_count,
        stale_certificate_count: group.stale_certificate_count,
        b_stale_account_count: group.b_stale_account_count,
        ..AccountCounts::default()
    };
    let found = discovery::discover_portfolios(rpc, &cfg.program)?;
    let mut portfolios = Vec::new();
    for p in found
        .iter()
        .filter(|p| p.market_group_id() == Some(cfg.market))
        .filter(|p| p.layout_discriminator() == Some(V16_LAYOUT_DISCRIMINATOR))
    {
        accounts.total += 1;
        match read_portfolio(&p.account.data) {
            Ok(pf) => portfolios.push(pf),
            Err(_) => accounts.decode_errors += 1,
        }
    }

    let n_assets = (group.config.max_market_slots as usize).min(group.assets.len());
    let mut assets: Vec<AssetView> = group.assets[..n_assets]
        .iter()
        .enumerate()
        .map(|(i, a)| AssetView {
            asset: i,
            lifecycle: lifecycle_label(a.lifecycle),
            slot_last: a.slot_last,
            stale_account_count_long: a.stale_account_count_long,
            stale_account_count_short: a.stale_account_count_short,
            stale_legs_observed_long: 0,
            stale_legs_observed_short: 0,
        })
        .collect();
    let mut referenced = vec![false; group.source_backing_buckets.len()];
    for pf in &portfolios {
        let w = work::assess(pf, &group);
        accounts.f_subatom_legs += w.f_subatom_legs as u64;
        match w.priority {
            work::Priority::Liquidatable => accounts.liquidatable_estimate += 1,
            work::Priority::Pending => accounts.pending_estimate += 1,
            work::Priority::Quiet => {}
        }
        let mut out_of_date = pf.stale_state != 0 || pf.b_stale_state != 0;
        accounts.stale_state += (pf.stale_state != 0) as u64;
        accounts.b_stale_state += (pf.b_stale_state != 0) as u64;
        for leg in pf.legs.iter().filter(|l| l.active != 0) {
            let ai = leg.asset_index.get() as usize;
            let (Some(a), Some(v)) = (group.assets.get(ai), assets.get_mut(ai)) else {
                continue;
            };
            let long = leg.side == 0;
            let epoch = if long {
                a.kf_epoch_long
            } else {
                a.kf_epoch_short
            };
            if leg.kf_epoch_snap.get() < epoch {
                out_of_date = true;
                accounts.kf_stale_cohort_observed += 1;
                if long {
                    v.stale_legs_observed_long += 1;
                } else {
                    v.stale_legs_observed_short += 1;
                }
            }
            if leg.stale != 0 || leg.b_stale != 0 {
                out_of_date = true;
            }
        }
        accounts.out_of_date += out_of_date as u64;
        for sd in pf.source_domains.iter().filter(|d| d.is_occupied()) {
            if let Some(r) = referenced.get_mut(sd.domain.get() as usize) {
                *r = true;
            }
        }
    }
    accounts.kf_stale_cohort_engine = assets
        .iter()
        .map(|a| a.stale_account_count_long + a.stale_account_count_short)
        .sum();

    let buckets: Vec<BucketView> = group
        .source_backing_buckets
        .iter()
        .enumerate()
        .filter(|(d, _)| d / 2 < n_assets)
        .map(|(d, b)| BucketView {
            asset: d / 2,
            side: if d % 2 == 0 { "long" } else { "short" },
            domain: d,
            status: bucket_status_label(b.status),
            expiry_slot: b.expiry_slot,
            slots_to_expiry: b.expiry_slot as i128 - slot as i128,
            fresh_unliened_backing_num: b.fresh_unliened_backing_num,
            valid_liened_backing_num: b.valid_liened_backing_num,
            referenced: referenced[d],
        })
        .collect();
    let live = group.mode == MarketModeV16::Live;
    let bucket_counts = bucket_counts(&buckets, live, cfg.expiry_warn_slots);

    let mint = Pubkey::new_from_array(wcfg.collateral_mint);
    let (vault_authority, _) =
        Pubkey::find_program_address(&[b"vault", cfg.market.as_ref()], &cfg.program);
    let spl_vault =
        spl_associated_token_account::get_associated_token_address(&vault_authority, &mint);
    let spl_vault_balance = rpc
        .get_account_data(&spl_vault)
        .ok()
        .and_then(|d| spl_token::state::Account::unpack(&d).ok())
        .map(|a| a.amount);

    let resolved = (group.mode == MarketModeV16::Resolved)
        .then(|| resolved_claims(portfolios.iter(), &group.resolved_payout_ledger));
    Ok(ChainSnapshot {
        slot,
        header_current_slot: group.current_slot,
        mode: mode_label(group.mode),
        accounts,
        assets,
        buckets,
        bucket_counts,
        unowned: unowned(HeaderTerms::from_header(&header)),
        source_fresh_backing_sum_domains_num: group.source_credit.iter().fold(0u128, |a, sc| {
            a.saturating_add(sc.fresh_reserved_backing_num)
        }),
        spl_vault,
        spl_vault_balance,
        resolved,
    })
}

// ---------------------------------------------------------------- JSON

fn obj(fields: &[(&str, String)]) -> String {
    let parts: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{}:{}", quote(k), v))
        .collect();
    format!("{{{}}}", parts.join(","))
}

fn opt<T: ToString>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "null".into())
}

impl Snapshot {
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        let _ = write!(
            s,
            "{{\"taken_unix_ms\":{},\"market\":{},\"program\":{},\"expiry_warn_slots\":{},",
            self.taken_unix_ms,
            quote(&self.market.to_string()),
            quote(&self.program.to_string()),
            self.expiry_warn_slots
        );
        match &self.chain {
            Err(e) => {
                let _ = write!(s, "\"chain\":null,\"chain_error\":{},", quote(e));
            }
            Ok(c) => {
                let _ = write!(s, "\"chain\":{},\"chain_error\":null,", chain_json(c));
            }
        }
        match &self.keeper {
            None => s.push_str("\"keeper\":null,\"keeper_error\":null,"),
            Some(Err(e)) => {
                let _ = write!(s, "\"keeper\":null,\"keeper_error\":{},", quote(e));
            }
            Some(Ok(k)) => {
                let _ = write!(s, "\"keeper\":{},\"keeper_error\":null,", k.to_json());
            }
        }
        let derived = keeper_derived(self);
        let _ = write!(
            s,
            "\"keeper_derived\":{}}}",
            obj(&[
                (
                    "status_age_seconds",
                    opt(self.keeper_age_seconds.map(|v| format!("{v:.3}")))
                ),
                ("cu_p50", opt(derived.cu_p50)),
                ("cu_max", opt(derived.cu_max)),
                (
                    "slots_since_last_progress",
                    opt(derived.slots_since_last_progress)
                ),
            ])
        );
        s
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeeperDerived {
    pub cu_p50: Option<u64>,
    pub cu_max: Option<u64>,
    /// Chain slot now − `last_progress_slot` (needs both).
    pub slots_since_last_progress: Option<u64>,
}

/// Nearest-rank median (lower middle for even counts) and max.
pub fn cu_p50_max(window: &[u64]) -> (Option<u64>, Option<u64>) {
    if window.is_empty() {
        return (None, None);
    }
    let mut v = window.to_vec();
    v.sort_unstable();
    (Some(v[(v.len() - 1) / 2]), v.last().copied())
}

pub fn keeper_derived(s: &Snapshot) -> KeeperDerived {
    let Some(k) = s.keeper_ok() else {
        return KeeperDerived::default();
    };
    let (cu_p50, cu_max) = cu_p50_max(&k.cu_window);
    let slot = s.chain.as_ref().ok().map(|c| c.slot);
    KeeperDerived {
        cu_p50,
        cu_max,
        slots_since_last_progress: match (slot, k.last_progress_slot) {
            (Some(now), Some(p)) => Some(now.saturating_sub(p)),
            _ => None,
        },
    }
}

fn chain_json(c: &ChainSnapshot) -> String {
    let a = &c.accounts;
    let accounts = obj(&[
        ("total", a.total.to_string()),
        ("decode_errors", a.decode_errors.to_string()),
        (
            "materialized_portfolio_count",
            a.materialized_portfolio_count.to_string(),
        ),
        ("stale_state", a.stale_state.to_string()),
        (
            "stale_certificate_count",
            a.stale_certificate_count.to_string(),
        ),
        ("b_stale_state", a.b_stale_state.to_string()),
        ("b_stale_account_count", a.b_stale_account_count.to_string()),
        (
            "kf_stale_cohort_engine",
            a.kf_stale_cohort_engine.to_string(),
        ),
        (
            "kf_stale_cohort_observed",
            a.kf_stale_cohort_observed.to_string(),
        ),
        ("out_of_date", a.out_of_date.to_string()),
        ("pending_estimate", a.pending_estimate.to_string()),
        ("liquidatable_estimate", a.liquidatable_estimate.to_string()),
        ("f_subatom_legs", a.f_subatom_legs.to_string()),
    ]);
    let assets: Vec<String> = c
        .assets
        .iter()
        .map(|x| {
            obj(&[
                ("asset", x.asset.to_string()),
                ("lifecycle", quote(x.lifecycle)),
                ("slot_last", x.slot_last.to_string()),
                (
                    "slots_since_accrual",
                    c.slot.saturating_sub(x.slot_last).to_string(),
                ),
                (
                    "stale_account_count_long",
                    x.stale_account_count_long.to_string(),
                ),
                (
                    "stale_account_count_short",
                    x.stale_account_count_short.to_string(),
                ),
                (
                    "stale_legs_observed_long",
                    x.stale_legs_observed_long.to_string(),
                ),
                (
                    "stale_legs_observed_short",
                    x.stale_legs_observed_short.to_string(),
                ),
            ])
        })
        .collect();
    let buckets: Vec<String> = c
        .buckets
        .iter()
        .map(|b| {
            obj(&[
                ("asset", b.asset.to_string()),
                ("side", quote(b.side)),
                ("domain", b.domain.to_string()),
                ("status", quote(b.status)),
                ("expiry_slot", b.expiry_slot.to_string()),
                ("slots_to_expiry", b.slots_to_expiry.to_string()),
                (
                    "fresh_unliened_backing_num",
                    b.fresh_unliened_backing_num.to_string(),
                ),
                (
                    "valid_liened_backing_num",
                    b.valid_liened_backing_num.to_string(),
                ),
                ("referenced", b.referenced.to_string()),
            ])
        })
        .collect();
    let bc = &c.bucket_counts;
    let bucket_counts = obj(&[
        ("fresh", bc.fresh.to_string()),
        ("impaired", bc.impaired.to_string()),
        ("expired", bc.expired.to_string()),
        ("empty", bc.empty.to_string()),
        ("fresh_expiring_soon", bc.fresh_expiring_soon.to_string()),
        ("fresh_past_expiry", bc.fresh_past_expiry.to_string()),
        (
            "fresh_past_expiry_in_live",
            bc.fresh_past_expiry_in_live.to_string(),
        ),
        (
            "fresh_past_expiry_in_live_unreferenced",
            bc.fresh_past_expiry_in_live_unreferenced.to_string(),
        ),
        ("expiring_with_liens", bc.expiring_with_liens.to_string()),
    ]);
    let u = &c.unowned;
    let t = &u.terms;
    let unowned = obj(&[
        ("vault", t.vault.to_string()),
        ("c_tot", t.c_tot.to_string()),
        ("insurance", t.insurance.to_string()),
        (
            "backing_provider_earnings_total",
            t.backing_provider_earnings_total.to_string(),
        ),
        (
            "source_fresh_backing_total_num",
            t.source_fresh_backing_total_num.to_string(),
        ),
        (
            "source_fresh_backing_total_atoms",
            u.source_fresh_backing_total_atoms.to_string(),
        ),
        (
            "source_fresh_backing_sum_domains_num",
            c.source_fresh_backing_sum_domains_num.to_string(),
        ),
        ("senior", u.senior.to_string()),
        ("residual", u.residual.to_string()),
        ("senior_shortfall", u.senior_shortfall.to_string()),
        ("pnl_pos_tot", t.pnl_pos_tot.to_string()),
        ("pnl_pos_bound_tot", t.pnl_pos_bound_tot.to_string()),
        ("unowned", u.unowned.to_string()),
        ("unowned_vs_bound", u.unowned_vs_bound.to_string()),
        ("spl_vault", quote(&c.spl_vault.to_string())),
        ("spl_vault_balance", opt(c.spl_vault_balance)),
        (
            "spl_vault_minus_engine_vault",
            opt(c.spl_vault_balance.map(|b| signed_diff(b as u128, t.vault))),
        ),
    ]);
    let resolved = match &c.resolved {
        None => "null".to_string(),
        Some(r) => obj(&[
            ("portfolios", r.portfolios.to_string()),
            ("claimants_unpaid", r.claimants_unpaid.to_string()),
            ("receipts_open", r.receipts_open.to_string()),
            ("receipts_finalized", r.receipts_finalized.to_string()),
            (
                "receipt_face_remaining_atoms",
                r.receipt_face_remaining_atoms.to_string(),
            ),
            (
                "receipt_claimable_now_atoms",
                r.receipt_claimable_now_atoms.to_string(),
            ),
            ("capital_atoms", r.capital_atoms.to_string()),
            (
                "unreceipted_positive_pnl_atoms",
                r.unreceipted_positive_pnl_atoms.to_string(),
            ),
            ("negative_pnl_atoms", r.negative_pnl_atoms.to_string()),
            ("ledger_payout_halted", r.ledger_payout_halted.to_string()),
            ("ledger_finalized", r.ledger_finalized.to_string()),
            ("ledger_rate_num", r.ledger_rate_num.to_string()),
            ("ledger_rate_den", r.ledger_rate_den.to_string()),
            (
                "ledger_snapshot_residual",
                r.ledger_snapshot_residual.to_string(),
            ),
        ]),
    };
    obj(&[
        ("slot", c.slot.to_string()),
        ("header_current_slot", c.header_current_slot.to_string()),
        ("mode", quote(c.mode)),
        ("accounts", accounts),
        ("assets", format!("[{}]", assets.join(","))),
        ("buckets", format!("[{}]", buckets.join(","))),
        ("bucket_counts", bucket_counts),
        ("unowned_collateral", unowned),
        ("resolved_claims", resolved),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unowned_matches_engine_residual_formula() {
        let t = HeaderTerms {
            vault: 10_000,
            c_tot: 6_000,
            insurance: 500,
            backing_provider_earnings_total: 100,
            // 1_200.9 atoms of fresh backing → floor 1_200
            source_fresh_backing_total_num: 1_200 * BOUND_SCALE + BOUND_SCALE * 9 / 10,
            pnl_pos_tot: 1_000,
            pnl_pos_bound_tot: 3_000,
        };
        let u = unowned(t);
        assert_eq!(u.source_fresh_backing_total_atoms, 1_200);
        assert_eq!(u.senior, 7_800);
        assert_eq!(u.residual, 2_200);
        assert_eq!(u.unowned, 1_200);
        assert_eq!(u.unowned_vs_bound, -800);
        assert_eq!(u.senior_shortfall, 0);
    }

    #[test]
    fn residual_saturates_like_the_engine() {
        let u = unowned(HeaderTerms {
            vault: 100,
            c_tot: 150,
            pnl_pos_tot: 5,
            ..HeaderTerms::default()
        });
        assert_eq!((u.residual, u.senior_shortfall, u.unowned), (0, 50, -5));
    }

    #[test]
    fn mul_div_floor_is_exact_past_u128() {
        assert_eq!(mul_div_floor(7, 3, 2), Some(10));
        assert_eq!(mul_div_floor(1, 1, 0), None);
        let big = u128::MAX / 3;
        // (big * 6) / 4 = big * 1.5 — the product overflows u128.
        assert_eq!(mul_div_floor(big, 6, 4), Some(big + big / 2));
        assert_eq!(
            mul_div_floor(u128::MAX, u128::MAX, u128::MAX),
            Some(u128::MAX)
        );
        assert_eq!(mul_div_floor(u128::MAX, 2, 1), None);
    }

    fn bucket(status: &'static str, to_expiry: i128, liened: u128, referenced: bool) -> BucketView {
        BucketView {
            asset: 0,
            side: "long",
            domain: 0,
            status,
            expiry_slot: 0,
            slots_to_expiry: to_expiry,
            fresh_unliened_backing_num: 1,
            valid_liened_backing_num: liened,
            referenced,
        }
    }

    #[test]
    fn bucket_classification() {
        let bs = vec![
            bucket("fresh", 0, 0, false), // past expiry (engine: expiry <= slot), unreferenced
            bucket("fresh", -5, 7, true), // past expiry, liened, referenced
            bucket("fresh", 50, 3, true), // expiring soon with liens
            bucket("fresh", 5_000, 0, false), // far
            bucket("impaired", -10, 0, false),
            bucket("empty", 0, 0, false),
        ];
        let live = bucket_counts(&bs, true, 100);
        assert_eq!((live.fresh, live.impaired, live.empty), (4, 1, 1));
        assert_eq!(live.fresh_past_expiry, 2);
        assert_eq!(live.fresh_past_expiry_in_live, 2);
        assert_eq!(live.fresh_past_expiry_in_live_unreferenced, 1);
        assert_eq!(live.fresh_expiring_soon, 1);
        assert_eq!(live.expiring_with_liens, 2);
        let resolved = bucket_counts(&bs, false, 100);
        assert_eq!(resolved.fresh_past_expiry, 2);
        assert_eq!(resolved.fresh_past_expiry_in_live, 0);
    }

    #[test]
    fn resolved_claims_sum_exact_fields() {
        use percolator::{V16PodI128, V16PodU128};
        let mut paid: PortfolioAccountV16Account = bytemuck::Zeroable::zeroed();
        paid.resolved_payout_receipt.present = 1;
        paid.resolved_payout_receipt.finalized = 1;
        paid.resolved_payout_receipt.terminal_positive_claim_face = V16PodU128::new(50);
        paid.resolved_payout_receipt.paid_effective = V16PodU128::new(50);
        let mut open: PortfolioAccountV16Account = bytemuck::Zeroable::zeroed();
        open.capital = V16PodU128::new(1_000);
        open.resolved_payout_receipt.present = 1;
        open.resolved_payout_receipt.terminal_positive_claim_face = V16PodU128::new(300);
        open.resolved_payout_receipt.paid_effective = V16PodU128::new(100);
        let mut fresh: PortfolioAccountV16Account = bytemuck::Zeroable::zeroed();
        fresh.pnl = V16PodI128::new(70);
        let mut loser: PortfolioAccountV16Account = bytemuck::Zeroable::zeroed();
        loser.pnl = V16PodI128::new(-20);
        let ledger = ResolvedPayoutLedgerV16 {
            snapshot_residual: 0,
            terminal_claim_exact_receipts_num: 0,
            terminal_claim_bound_unreceipted_num: 0,
            current_payout_rate_num: 1,
            current_payout_rate_den: 2,
            snapshot_slot: 0,
            payout_halted: false,
            finalized: false,
        };
        let all = [paid, open, fresh, loser];
        let r = resolved_claims(all.iter(), &ledger);
        assert_eq!(r.portfolios, 4);
        assert_eq!(r.claimants_unpaid, 2, "open receipt + unreceipted winner");
        assert_eq!((r.receipts_open, r.receipts_finalized), (1, 1));
        assert_eq!(r.receipt_face_remaining_atoms, 200);
        assert_eq!(r.receipt_claimable_now_atoms, 50, "floor(300/2) - 100");
        assert_eq!(r.capital_atoms, 1_000);
        assert_eq!(r.unreceipted_positive_pnl_atoms, 70);
        assert_eq!(r.negative_pnl_atoms, 20);
        let halted = ResolvedPayoutLedgerV16 {
            payout_halted: true,
            ..ledger
        };
        assert_eq!(
            resolved_claims(all.iter(), &halted).receipt_claimable_now_atoms,
            0
        );
    }

    #[test]
    fn cu_percentiles() {
        assert_eq!(cu_p50_max(&[]), (None, None));
        assert_eq!(cu_p50_max(&[5, 1, 3]), (Some(3), Some(5)));
        assert_eq!(cu_p50_max(&[4, 1, 3, 2]), (Some(2), Some(4)));
    }
}
