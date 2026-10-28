use anyhow::{anyhow, Context, Result};
use percolator::{
    AssetLifecycleV16, BackingBucketStatusV16, BackingBucketV16, MarketGroupV16HeaderAccount,
    MarketModeV16, PortfolioAccountV16Account, BOUND_SCALE,
};
use percolator_prog::{
    constants::MARKET_GROUP_OFF,
    state::{read_market, read_portfolio},
};
use serde_json::{json, Value};
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;

// All six terms are read from the same account buffer, including fresh principal.
pub fn collateral(h: &MarketGroupV16HeaderAccount) -> Result<Value> {
    let vault = h.vault.get();
    let fresh = h.source_fresh_backing_total_num.get() / BOUND_SCALE;
    let senior = h
        .c_tot
        .get()
        .saturating_add(h.insurance.get())
        .saturating_add(h.backing_provider_earnings_total.get())
        .saturating_add(fresh);
    let residual = vault.saturating_sub(senior);
    let pnl = h.pnl_pos_tot.get();
    let unowned = if residual >= pnl {
        i128::try_from(residual - pnl).context("positive unowned exceeds i128")?
    } else {
        // Include i128::MIN without an overflowing cast or negation.
        let magnitude = pnl - residual;
        if magnitude == (1u128 << 127) {
            i128::MIN
        } else {
            -i128::try_from(magnitude).context("negative unowned exceeds i128")?
        }
    };
    Ok(json!({
        "residual_atoms": residual.to_string(), "unowned_collateral_atoms": unowned.to_string(),
        "senior_shortfall_atoms": senior.saturating_sub(vault).to_string(),
        "terms": {"vault": vault.to_string(), "c_tot": h.c_tot.get().to_string(),
            "insurance": h.insurance.get().to_string(), "pnl_pos_tot": pnl.to_string(),
            "backing_provider_earnings_total": h.backing_provider_earnings_total.get().to_string(),
            "source_fresh_backing_total": fresh.to_string()}
    }))
}

pub fn buckets(rows: &[BackingBucketV16], now: u64, window: u64) -> Value {
    let mut counts = json!({"empty":0,"fresh":0,"expired":0,"impaired":0});
    let mut expiring = 0u64;
    let mut liens = 0u64;
    let mut nonempty = Vec::new();
    for (domain, b) in rows.iter().enumerate() {
        let status = match b.status {
            BackingBucketStatusV16::Empty => "empty",
            BackingBucketStatusV16::Fresh => "fresh",
            BackingBucketStatusV16::Expired => "expired",
            BackingBucketStatusV16::Impaired => "impaired",
        };
        counts[status] = json!(counts[status].as_u64().unwrap() + 1);
        if b.status == BackingBucketStatusV16::Fresh && b.expiry_slot <= now.saturating_add(window)
        {
            expiring += 1;
            if b.valid_liened_backing_num > 0 {
                liens += 1;
            }
        }
        if b.status != BackingBucketStatusV16::Empty {
            nonempty.push(
                json!({"asset":domain / 2,"side":if domain % 2 == 0 {"long"} else {"short"},
                "status":status,"expiry_slot":b.expiry_slot,
                "slots_to_expiry":(i128::from(b.expiry_slot)-i128::from(now)).to_string(),
                "valid_liened_backing_num":b.valid_liened_backing_num.to_string()}),
            );
        }
    }
    json!({"counts":counts,"expiring":expiring,"expiring_with_liens":liens,
        "window_slots":window,"rows":nonempty})
}

pub fn resolved_sums<'a>(
    resolved: bool,
    portfolios: impl IntoIterator<Item = &'a PortfolioAccountV16Account>,
) -> Result<Value> {
    let (mut unpaid, mut face, mut capital, mut pnl) = (0u64, 0u128, 0u128, 0u128);
    if resolved {
        for p in portfolios {
            let r = p.resolved_payout_receipt;
            let pending = r.present != 0 && r.finalized == 0;
            if p.capital.get() > 0 || p.pnl.get() > 0 || pending {
                unpaid += 1;
            }
            capital = capital
                .checked_add(p.capital.get())
                .context("capital sum overflow")?;
            if pending {
                let remaining = r
                    .terminal_positive_claim_face
                    .get()
                    .checked_sub(r.paid_effective.get())
                    .context("receipt paid exceeds face")?;
                face = face
                    .checked_add(remaining)
                    .context("receipt sum overflow")?;
            }
            if r.present == 0 {
                pnl = pnl
                    .checked_add(p.pnl.get().max(0) as u128)
                    .context("pnl sum overflow")?;
            }
        }
    }
    Ok(json!({"claimants_unpaid":unpaid,"owed_atoms":{
        "receipt_face_remaining":face.to_string(),"capital":capital.to_string(),
        "unreceipted_positive_pnl":pnl.to_string()}}))
}

pub fn fetch(rpc: &RpcClient, program: &Pubkey, market: &Pubkey, window: u64) -> Result<Value> {
    let slot = rpc.get_slot().context("getSlot")?;
    let data = rpc.get_account_data(market).context("getAccount market")?;
    let (_, group) = read_market(&data).map_err(|e| anyhow!("read_market: {e:?}"))?;
    let raw = data
        .get(
            MARKET_GROUP_OFF..MARKET_GROUP_OFF + std::mem::size_of::<MarketGroupV16HeaderAccount>(),
        )
        .context("short market header")?;
    let header = bytemuck::pod_read_unaligned::<MarketGroupV16HeaderAccount>(raw);
    let found = drip::discovery::discover_portfolios(rpc, program)?;
    let mut decoded = Vec::new();
    let mut total = 0u64;
    let mut errors = 0u64;
    for p in found.iter().filter(|p| {
        p.market_group_id() == Some(*market)
            && p.layout_discriminator() == Some(drip::binding::V16_LAYOUT_DISCRIMINATOR)
    }) {
        total += 1;
        match read_portfolio(&p.account.data) {
            Ok(p) => decoded.push(p),
            Err(_) => errors += 1,
        }
    }
    let mut assets = Vec::new();
    let mut kf_stale = 0u64;
    let mut kf_rows = Vec::new();
    for (i, a) in group.assets.iter().enumerate() {
        kf_stale += a.stale_account_count_long + a.stale_account_count_short;
        for (side, count) in [
            ("long", a.stale_account_count_long),
            ("short", a.stale_account_count_short),
        ] {
            kf_rows.push(json!({"asset":i,"side":side,"count":count}));
        }
        let lifecycle = match a.lifecycle {
            AssetLifecycleV16::Disabled => continue,
            AssetLifecycleV16::PendingActivation => "pending_activation",
            AssetLifecycleV16::Active => "active",
            AssetLifecycleV16::DrainOnly => "drain_only",
            AssetLifecycleV16::Retired => "retired",
            AssetLifecycleV16::Recovery => "recovery",
        };
        assets
            .push(json!({"asset":i,"lifecycle":lifecycle,"funding_last_accrual_slot":a.slot_last}));
    }
    let mode = match group.mode {
        MarketModeV16::Live => "live",
        MarketModeV16::Resolved => "resolved",
        MarketModeV16::Recovery => "recovery",
    };
    Ok(
        json!({"chain_slot":slot,"mode":mode,"accounts":{"total":total,"decode_errors":errors,
        "out_of_date":{"stale_certificate":group.stale_certificate_count,"b_stale":group.b_stale_account_count,"kf_stale":kf_stale},
        "asset_kf_stale":kf_rows},"assets":assets,
        "buckets":buckets(&group.source_backing_buckets,slot,window),
        "collateral":collateral(&header)?,"resolved_claims":resolved_sums(group.mode == MarketModeV16::Resolved,&decoded)?}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use percolator::{V16PodI128, V16PodU128};
    #[test]
    fn exact_residual_floor_shortfall_and_signed_unowned() {
        let mut h = MarketGroupV16HeaderAccount::default();
        h.vault = V16PodU128::new(100);
        h.c_tot = V16PodU128::new(20);
        h.insurance = V16PodU128::new(10);
        h.backing_provider_earnings_total = V16PodU128::new(5);
        h.source_fresh_backing_total_num = V16PodU128::new(3 * BOUND_SCALE + BOUND_SCALE - 1);
        h.pnl_pos_tot = V16PodU128::new(70);
        let v = collateral(&h).unwrap();
        assert_eq!(v["residual_atoms"], "62");
        assert_eq!(v["unowned_collateral_atoms"], "-8");
        h.vault = V16PodU128::new(1);
        let v = collateral(&h).unwrap();
        assert_eq!(v["senior_shortfall_atoms"], "37");
        assert_eq!(v["residual_atoms"], "0");
        h.pnl_pos_tot = V16PodU128::new(1u128 << 127);
        assert_eq!(
            collateral(&h).unwrap()["unowned_collateral_atoms"],
            i128::MIN.to_string()
        );
        h.pnl_pos_tot = V16PodU128::new(u128::MAX);
        assert!(collateral(&h).is_err());
    }
    #[test]
    fn bucket_status_and_inclusive_window_including_past_expiry() {
        let mut b = BackingBucketV16::EMPTY;
        b.status = BackingBucketStatusV16::Fresh;
        b.expiry_slot = 90;
        b.valid_liened_backing_num = 1;
        let mut boundary = b;
        boundary.expiry_slot = 110;
        let mut outside = b;
        outside.expiry_slot = 111;
        let mut expired = b;
        expired.status = BackingBucketStatusV16::Expired;
        let mut impaired = b;
        impaired.status = BackingBucketStatusV16::Impaired;
        let v = buckets(
            &[
                BackingBucketV16::EMPTY,
                b,
                boundary,
                outside,
                expired,
                impaired,
            ],
            100,
            10,
        );
        assert_eq!(v["expiring"], 2);
        assert_eq!(v["expiring_with_liens"], 2);
        assert_eq!(
            v["counts"],
            json!({"empty":1,"fresh":3,"expired":1,"impaired":1})
        );
        assert_eq!(v["rows"][0]["side"], "short");
        assert_eq!(v["rows"][0]["slots_to_expiry"], "-10");
    }
    #[test]
    fn resolved_receipts_and_unreceipted_face() {
        let mut a = PortfolioAccountV16Account::default();
        a.capital = V16PodU128::new(9);
        a.pnl = V16PodI128::new(12);
        let mut b = PortfolioAccountV16Account::default();
        b.pnl = V16PodI128::new(99);
        b.resolved_payout_receipt.present = 1;
        b.resolved_payout_receipt.terminal_positive_claim_face = V16PodU128::new(80);
        b.resolved_payout_receipt.paid_effective = V16PodU128::new(30);
        let mut c = PortfolioAccountV16Account::default();
        c.pnl = V16PodI128::new(-5);
        let v = resolved_sums(true, [&a, &b, &c]).unwrap();
        assert_eq!(v["claimants_unpaid"], 2);
        assert_eq!(
            v["owed_atoms"],
            json!({"capital":"9","receipt_face_remaining":"50","unreceipted_positive_pnl":"12"})
        );
        b.resolved_payout_receipt.finalized = 1;
        b.pnl = V16PodI128::new(0);
        assert_eq!(resolved_sums(true, [&b]).unwrap()["claimants_unpaid"], 0);
        assert_eq!(
            resolved_sums(false, [&a]).unwrap()["owed_atoms"]["capital"],
            "0"
        );
    }
}
