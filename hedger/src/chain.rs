//! Our LP's net SOL position, read from the Percolator market and the LP
//! portfolio with the wrapper's own decoders (as monitor/src/chain.rs).

use anyhow::{anyhow, bail, Context, Result};
use percolator::{AssetStateV16, MarketModeV16, PortfolioLegV16Account, MIN_A_SIDE, ADL_ONE, POS_SCALE};
use percolator_prog::state::{read_market, read_portfolio};
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;

#[derive(Debug, Clone)]
pub struct LpRead {
    pub slot: u64,
    /// Signed effective SOL (long > 0).
    pub net_sol: f64,
    /// Our market's effective (mark) price, USD per SOL.
    pub mark_px: f64,
    pub live: bool,
}

/// Both accounts in one call so they come from the same slot.
pub fn read_lp(rpc: &RpcClient, market: &Pubkey, lp: &Pubkey, asset: usize) -> Result<LpRead> {
    let res = rpc
        .get_multiple_accounts_with_commitment(&[*market, *lp], rpc.commitment())
        .context("getMultipleAccounts")?;
    let [m, p] = res.value.as_slice() else { bail!("short getMultipleAccounts reply") };
    let m = m.as_ref().context("market account not found")?;
    let p = p.as_ref().context("LP portfolio not found")?;
    let (_, group) = read_market(&m.data).map_err(|e| anyhow!("read_market: {e:?}"))?;
    let pf = read_portfolio(&p.data).map_err(|e| anyhow!("read_portfolio: {e:?}"))?;
    if pf.provenance_header.market_group_id != market.to_bytes() {
        bail!("LP portfolio belongs to another market");
    }
    let a = group.assets.get(asset).context("asset index out of range")?;
    let q = net_q(&pf.legs, asset, a)?;
    Ok(LpRead {
        slot: res.context.slot,
        net_sol: q as f64 / POS_SCALE as f64,
        mark_px: a.effective_price as f64 / 1e6,
        live: group.mode == MarketModeV16::Live,
    })
}

/// Sum of the asset's legs at the current side A index, as the engine's
/// `effective_abs_quantity_for_leg`: ceil(|basis| x A_now / A_basis) for a
/// current-epoch leg; 0 for a leg from before a side reset (it owns no OI).
pub fn net_q(legs: &[PortfolioLegV16Account], asset: usize, a: &AssetStateV16) -> Result<i128> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use percolator::{V16PodI128, V16PodU128, V16PodU32, V16PodU64};

    fn leg(side: u8, q: i128, a_basis: u128, epoch: u64) -> PortfolioLegV16Account {
        PortfolioLegV16Account {
            active: 1,
            asset_index: V16PodU32::new(0),
            side,
            basis_pos_q: V16PodI128::new(q),
            a_basis: V16PodU128::new(a_basis),
            epoch_snap: V16PodU64::new(epoch),
            ..Default::default()
        }
    }

    #[test]
    fn net_effective_quantity_scales_with_a_and_skips_reset_legs() {
        let mut a = AssetStateV16::default();
        a.a_long = ADL_ONE;
        a.a_short = ADL_ONE / 2; // shorts were ADL'd by half
        a.epoch_long = 3;
        a.epoch_short = 1;
        // 10 SOL long, 4 SOL short at half A -> 2, and a stale short from epoch 0.
        let legs = [
            leg(0, 10 * POS_SCALE as i128, ADL_ONE, 3),
            leg(1, -4 * POS_SCALE as i128, ADL_ONE, 1),
            leg(1, -7 * POS_SCALE as i128, ADL_ONE, 0),
        ];
        assert_eq!(net_q(&legs, 0, &a).unwrap(), 8 * POS_SCALE as i128);
        // Ceil, as the engine: 3 raw at A 1/2 is 1.5 -> 2.
        let odd = [leg(1, -3, ADL_ONE, 1)];
        assert_eq!(net_q(&odd, 0, &a).unwrap(), -2);
        // Other asset and inactive legs are ignored.
        let mut other = leg(0, 5, ADL_ONE, 3);
        other.asset_index = V16PodU32::new(1);
        let mut off = leg(0, 5, ADL_ONE, 3);
        off.active = 0;
        assert_eq!(net_q(&[other, off], 0, &a).unwrap(), 0);
        // A leg from a later epoch than the asset is corrupt: refuse.
        assert!(net_q(&[leg(0, 5, ADL_ONE, 4)], 0, &a).is_err());
    }
}
