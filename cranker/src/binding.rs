//! Wrapper incarnation bindings used on the decode/submit path.
//!
//! After wrapper `#135`, several ixs bind `(portfolio_id, expected_sequence,
//! position_epoch)` and market-authority ixs bind `(policy_sequence,
//! authority_epoch)`. Portfolio accounts also carry engine layout
//! discriminator 18 in the provenance header. Helpers here read those
//! fields from the wrapper's own offsets / decoders — no hand-rolled
//! engine view.

use anyhow::{anyhow, Result};
use percolator_prog::constants::{HEADER_LEN, PORTFOLIO_ID_OFF};
use percolator_prog::state::{
    read_asset_control_sequences, read_market, read_portfolio_id,
    read_portfolio_matcher_sequence, read_portfolio_position_epoch,
};

/// Engine `V16_LAYOUT_DISCRIMINATOR` at pins `495a5590`, `4db11a8c` and
/// `68859b94` (unchanged across the launch-fix batch). Not re-exported
/// from `percolator`'s default `lib.rs` surface; keep in lockstep with
/// `repo/src/v16.rs`.
pub const V16_LAYOUT_DISCRIMINATOR: u16 = 18;

/// ProvenanceHeaderV16Account: 32 + 32 + 32 + version(2) + layout(2).
const LAYOUT_DISCRIMINATOR_OFF: usize = HEADER_LEN + 96 + 2;

/// A6 drain: engine `V16_SOURCE_LIEN_RELEASE_CHUNK_DOMAINS = 1`.
/// Loop this many domains per call until the account stops mutating.
pub const SOURCE_LIEN_RELEASE_CHUNK_DOMAINS: usize = 1;

/// Cap on inner drain steps for one candidate in one cycle. Bounded by
/// the wrapper's max source-domain count so a stuck liened account can
/// actually clear without an unbounded retry.
pub const SOURCE_LIEN_DRAIN_MAX_STEPS: usize =
    percolator_prog::constants::WRAPPER_MAX_BOUNDED_SOURCE_DOMAINS;

#[derive(Debug, Clone, Copy)]
pub struct PortfolioBinding {
    pub portfolio_id: u64,
    pub sequence: u64,
    #[allow(dead_code)] // used by ClosePortfolio in tests via the lib target
    pub position_epoch: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct MarketAuthorityBinding {
    pub authority_epoch: u64,
    pub liquidation_fee_seq: u64,
    pub maintenance_fee_seq: u64,
    #[allow(dead_code)] // used by ResolveMarket in tests via the lib target
    pub next_market_id: u64,
}

pub fn layout_discriminator(data: &[u8]) -> Option<u16> {
    let end = LAYOUT_DISCRIMINATOR_OFF + 2;
    if data.len() < end {
        return None;
    }
    Some(u16::from_le_bytes(
        data[LAYOUT_DISCRIMINATOR_OFF..end].try_into().ok()?,
    ))
}

pub fn layout_is_current(data: &[u8]) -> bool {
    layout_discriminator(data) == Some(V16_LAYOUT_DISCRIMINATOR)
}

pub fn read_portfolio_id_bytes(data: &[u8]) -> Option<u64> {
    let end = PORTFOLIO_ID_OFF + 8;
    if data.len() < end {
        return None;
    }
    let id = u64::from_le_bytes(data[PORTFOLIO_ID_OFF..end].try_into().ok()?);
    if id == 0 {
        None
    } else {
        Some(id)
    }
}

pub fn portfolio_binding(data: &[u8]) -> Result<PortfolioBinding> {
    let portfolio_id =
        read_portfolio_id(data).map_err(|e| anyhow!("read_portfolio_id: {e}"))?;
    let sequence = read_portfolio_matcher_sequence(data)
        .map_err(|e| anyhow!("read_portfolio_matcher_sequence: {e}"))?;
    let position_epoch = read_portfolio_position_epoch(data)
        .map_err(|e| anyhow!("read_portfolio_position_epoch: {e}"))?;
    Ok(PortfolioBinding {
        portfolio_id,
        sequence,
        position_epoch,
    })
}

/// A `PortfolioBinding` plus when it was read.
///
/// Since wrapper `b40ffbd8` a Recovery crank that changes the position
/// vector bumps `position_epoch` (`state::bump_portfolio_position_epoch`).
/// So does any matcher fill. A binding read before a crank is therefore
/// stale the moment that crank mutates the account, and it is never valid
/// in a later cycle. Callers re-read with `portfolio_binding` on fresh
/// account bytes whenever `usable` says no.
#[derive(Debug, Clone, Copy)]
pub struct BindingSnapshot {
    pub binding: PortfolioBinding,
    pub cycle: u64,
    invalidated: bool,
}

impl BindingSnapshot {
    pub fn read(data: &[u8], cycle: u64) -> Result<Self> {
        Ok(Self {
            binding: portfolio_binding(data)?,
            cycle,
            invalidated: false,
        })
    }

    /// Call after every crank on this account. Any observed mutation
    /// (including a Recovery step) may have moved `position_epoch` or
    /// `sequence`.
    pub fn note_crank(&mut self, mutated: bool) {
        self.invalidated |= mutated;
    }

    pub fn usable(&self, cycle: u64) -> bool {
        !self.invalidated && self.cycle == cycle
    }
}

pub fn market_authority_binding(data: &[u8]) -> Result<MarketAuthorityBinding> {
    let seqs = read_asset_control_sequences(data, 0)
        .map_err(|e| anyhow!("read_asset_control_sequences: {e}"))?;
    let (_cfg, mg) = read_market(data).map_err(|e| anyhow!("read_market: {e:?}"))?;
    Ok(MarketAuthorityBinding {
        authority_epoch: seqs.authority_epoch,
        liquidation_fee_seq: seqs.liquidation_fee,
        maintenance_fee_seq: seqs.maintenance_fee,
        next_market_id: mg.next_market_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(epoch: u64, cycle: u64) -> BindingSnapshot {
        BindingSnapshot {
            binding: PortfolioBinding {
                portfolio_id: 1,
                sequence: 1,
                position_epoch: epoch,
            },
            cycle,
            invalidated: false,
        }
    }

    #[test]
    fn binding_is_not_reused_after_mutation_or_across_cycles() {
        let mut s = snap(4, 7);
        assert!(s.usable(7));
        s.note_crank(false);
        assert!(s.usable(7), "idle crank leaves the binding valid");
        assert!(!s.usable(8), "never reused in a later cycle");
        s.note_crank(true);
        assert!(!s.usable(7), "a mutating (e.g. Recovery) crank invalidates it");
    }

    #[test]
    fn layout_const_is_eighteen() {
        assert_eq!(V16_LAYOUT_DISCRIMINATOR, 18);
        assert_eq!(SOURCE_LIEN_RELEASE_CHUNK_DOMAINS, 1);
    }
}
