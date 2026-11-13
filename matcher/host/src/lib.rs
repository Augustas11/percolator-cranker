//! Wire builders shared by the devnet utility and SBF integration tests.
use percolator_prog::{ix::Instruction as P, state};
use solana_sdk::{
    instruction::{AccountMeta as M, Instruction},
    pubkey::Pubkey,
};

pub const MATCHER_CALL_LEN: usize = 67;
pub const MATCHER_RETURN_LEN: usize = 64;
pub const PORTFOLIO_OWNER_OFF: usize = 116;
pub const FILL_TOO_LARGE: u32 = 11;
pub const INVENTORY_CAP: u32 = 12;
pub const STALE: u32 = 9;
pub const MARK_DEVIATES: u32 = 10;
pub const BAD_AUTHORITY: u32 = 4;
pub const ALREADY_INITIALIZED: u32 = 5;
pub const MISSING_SIGNATURE: u32 = 3;

// Duplicated wire layout: host and SBF intentionally use different Solana SDKs.
pub const CTX_LEN: usize = 352;
pub const MAGIC_OFF: usize = 64;
pub const VERSION_OFF: usize = 72;
pub const WRAPPER_OFF: usize = 76;
pub const MARKET_OFF: usize = 108;
pub const LP_PORTFOLIO_OFF: usize = 140;
pub const LP_OWNER_OFF: usize = 172;
pub const DELEGATE_OFF: usize = 204;
pub const PRICE_AUTHORITY_OFF: usize = 236;
pub const ASSET_OFF: usize = 268;
pub const PAUSED_OFF: usize = 270;
pub const FILL_OFF: usize = 271;
pub const CAP_OFF: usize = 287;
pub const INVENTORY_OFF: usize = 303;
pub const AGE_OFF: usize = 319;
pub const DEVIATION_OFF: usize = 327;
pub const REF_PRICE_OFF: usize = 329;
pub const REF_SLOT_OFF: usize = 337;

pub fn wrapper_ix(program_id: Pubkey, ix: P, accounts: Vec<M>) -> Instruction {
    Instruction {
        program_id,
        accounts,
        data: ix.encode(),
    }
}

pub fn delegate(
    wrapper: Pubkey,
    market: Pubkey,
    portfolio: Pubkey,
    owner: Pubkey,
    program: Pubkey,
    ctx: Pubkey,
) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"matcher",
            market.as_ref(),
            portfolio.as_ref(),
            owner.as_ref(),
            program.as_ref(),
            ctx.as_ref(),
        ],
        &wrapper,
    )
    .0
}

pub fn init_data(
    asset: u16,
    fill: u128,
    cap: u128,
    age: u64,
    deviation: u16,
    authority: Pubkey,
) -> Vec<u8> {
    let mut data = vec![2];
    data.extend(asset.to_le_bytes());
    data.extend(params_data(fill, cap, age, deviation, authority));
    data
}

fn params_data(fill: u128, cap: u128, age: u64, deviation: u16, authority: Pubkey) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend(fill.to_le_bytes());
    data.extend(cap.to_le_bytes());
    data.extend(age.to_le_bytes());
    data.extend(deviation.to_le_bytes());
    data.extend(authority.to_bytes());
    data
}

pub fn set_params_data(
    fill: u128,
    cap: u128,
    age: u64,
    deviation: u16,
    authority: Pubkey,
    paused: u8,
    inventory: i128,
) -> Vec<u8> {
    let mut data = vec![5];
    data.extend(params_data(fill, cap, age, deviation, authority));
    data.push(paused);
    data.extend(inventory.to_le_bytes());
    data
}

pub fn update_ref(program: Pubkey, authority: Pubkey, ctx: Pubkey, price: u64) -> Instruction {
    let mut data = vec![4];
    data.extend(price.to_le_bytes());
    Instruction {
        program_id: program,
        accounts: vec![M::new_readonly(authority, true), M::new(ctx, false)],
        data,
    }
}

/// Fee to send and to allow: the market's base fee, never below the first market's 12 bps.
fn fee_bps(base: u64) -> Result<u16, solana_sdk::program_error::ProgramError> {
    u16::try_from(base.max(12)).map_err(|_| solana_sdk::program_error::ProgramError::InvalidAccountData)
}

pub fn set_matcher_config(
    wrapper: Pubkey,
    market: Pubkey,
    portfolio: Pubkey,
    owner: Pubkey,
    program: Pubkey,
    ctx: Pubkey,
    portfolio_data: &[u8],
    market_data: &[u8],
    expiry: u64,
) -> Result<Instruction, solana_sdk::program_error::ProgramError> {
    let (cfg, engine) = state::read_market(market_data)?;
    Ok(wrapper_ix(
        wrapper,
        P::SetMatcherConfig {
            portfolio_id: state::read_portfolio_id(portfolio_data)?,
            expected_sequence: state::read_portfolio_matcher_sequence(portfolio_data)?,
            position_epoch: state::read_portfolio_position_epoch(portfolio_data)?,
            asset_generation_frontier: engine.next_market_id,
            enabled: 1,
            // The wrapper refuses a TradeCpi when the market's base fee is above this cap.
            trade_fee_cap_bps: fee_bps(cfg.trade_fee_base_bps)?,
            expiry_slot: expiry,
        },
        vec![
            M::new(owner, true),
            M::new_readonly(market, false),
            M::new(portfolio, false),
            M::new_readonly(program, false),
            M::new_readonly(ctx, false),
            M::new_readonly(
                delegate(wrapper, market, portfolio, owner, program, ctx),
                false,
            ),
        ],
    ))
}

pub fn push_mark(
    wrapper: Pubkey,
    market: Pubkey,
    authority: Pubkey,
    data: &[u8],
    slot: u64,
    price: u64,
) -> Result<Instruction, solana_sdk::program_error::ProgramError> {
    let seq = state::read_asset_control_sequences(data, 0)?;
    Ok(wrapper_ix(
        wrapper,
        P::PushAuthMark {
            market_id: state::read_market(data)?.1.assets[0].market_id,
            asset_index: 0,
            now_slot: slot,
            mark_e6: price,
            observation_sequence: seq
                .oracle_observation
                .checked_add(1)
                .ok_or(solana_sdk::program_error::ProgramError::InvalidAccountData)?,
            authority_epoch: seq.authority_epoch,
        },
        vec![M::new(authority, true), M::new(market, false)],
    ))
}

pub fn trade(
    wrapper: Pubkey,
    market: Pubkey,
    taker: Pubkey,
    taker_pf: Pubkey,
    lp_pf: Pubkey,
    program: Pubkey,
    ctx: Pubkey,
    market_data: &[u8],
    taker_data: &[u8],
    lp_data: &[u8],
    size: i128,
) -> Result<Instruction, solana_sdk::program_error::ProgramError> {
    let owner = Pubkey::new_from_array(state::read_portfolio_owner_preflight(lp_data)?.1);
    Ok(wrapper_ix(
        wrapper,
        P::TradeCpi {
            account_a_portfolio_id: state::read_portfolio_id(taker_data)?,
            account_a_position_epoch: state::read_portfolio_position_epoch(taker_data)?,
            account_b_portfolio_id: state::read_portfolio_id(lp_data)?,
            account_b_position_epoch: state::read_portfolio_position_epoch(lp_data)?,
            account_b_matcher_sequence: state::read_portfolio_matcher_sequence(lp_data)?,
            asset_index: 0,
            market_id: state::read_market(market_data)?.1.assets[0].market_id,
            size_q: size,
            fee_bps: fee_bps(state::read_market(market_data)?.0.trade_fee_base_bps)?.into(),
            limit_price: 0,
            backing_fee_cap_bps: 0,
        },
        vec![
            M::new(taker, true),
            M::new(market, false),
            M::new(taker_pf, false),
            M::new(lp_pf, false),
            M::new_readonly(program, false),
            M::new(ctx, false),
            M::new_readonly(delegate(wrapper, market, lp_pf, owner, program, ctx), false),
        ],
    ))
}
