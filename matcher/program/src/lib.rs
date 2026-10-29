// Derived from aeyakovenko/percolator-match @ 60aac3a (Apache-2.0). Changes:
// authenticated owner + context initialization (#4), no reinitialization (#10),
// single-asset binding (#8), checked inventory/limit math (#9), and exact mark
// pricing without vAMM impact (#11 not applicable). Removed vAMM, passive LP,
// and batch/tag 3; added authority reference prices, freshness, and owner controls.
use solana_program::{
    account_info::AccountInfo, clock::Clock, entrypoint::ProgramResult,
    program_error::ProgramError, pubkey::Pubkey, sysvar::Sysvar,
};

pub const BAD_DATA: u32 = 1;
pub const BAD_ACCOUNT: u32 = 2;
pub const MISSING_SIGNATURE: u32 = 3;
pub const BAD_AUTHORITY: u32 = 4;
pub const ALREADY_INITIALIZED: u32 = 5;
pub const BAD_PARAMS: u32 = 6;
pub const PAUSED: u32 = 7;
pub const BAD_ASSET: u32 = 8;
pub const STALE: u32 = 9;
pub const MARK_DEVIATES: u32 = 10;
pub const FILL_TOO_LARGE: u32 = 11;
pub const INVENTORY_CAP: u32 = 12;
pub const BAD_PRICE: u32 = 13;
pub const BAD_CLOCK: u32 = 14;
pub const MATCHER_CALL_TAG: u8 = 0;
pub const MATCHER_CALL_LEN: usize = 67;
pub const MATCHER_RETURN_LEN: usize = 64;
pub const LIMIT: u128 = 1_000_000_000_000_000;

// Packed little-endian state; the final seven bytes are reserved padding.
pub const CTX_LEN: usize = 352;
pub const MAGIC_OFF: usize = 64;
pub const MAGIC: u64 = u64::from_le_bytes(*b"DRIPMTC1");
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

fn require(ok: bool, code: u32) -> ProgramResult {
    if ok {
        Ok(())
    } else {
        Err(ProgramError::Custom(code))
    }
}
fn number<const N: usize>(data: &[u8], off: usize) -> [u8; N] {
    // Callers validate the enclosing fixed-length layout first.
    data[off..off + N].try_into().unwrap()
}
fn validate_params(fill: u128, cap: u128, age: u64, deviation: u16) -> ProgramResult {
    require(
        fill > 0 && fill <= cap && cap <= LIMIT && (1..=9000).contains(&age) && deviation <= 500,
        BAD_PARAMS,
    )
}
fn next_inventory(
    inventory: i128,
    size: i128,
    fill: u128,
    cap: u128,
) -> Result<i128, ProgramError> {
    let abs = size
        .checked_abs()
        .ok_or(ProgramError::Custom(FILL_TOO_LARGE))? as u128;
    require(abs > 0 && abs <= fill, FILL_TOO_LARGE)?;
    let next = inventory
        .checked_sub(size)
        .ok_or(ProgramError::Custom(INVENTORY_CAP))?;
    let next_abs = next
        .checked_abs()
        .ok_or(ProgramError::Custom(INVENTORY_CAP))? as u128;
    let old_abs = inventory
        .checked_abs()
        .ok_or(ProgramError::Custom(INVENTORY_CAP))? as u128;
    require(next_abs <= cap || next_abs < old_abs, INVENTORY_CAP)?;
    Ok(next)
}

pub const FLAG_VALID: u32 = 1;
pub const MATCHER_ABI_VERSION: u32 = 3;

/// Matcher return structure written to context account at offset 0
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MatcherReturn {
    pub abi_version: u32,
    pub flags: u32,
    pub exec_price_e6: u64,
    pub exec_size: i128,
    pub req_id: u64,
    pub lp_account_id: u64,
    pub oracle_price_e6: u64,
    pub asset_index: u64,
}

impl MatcherReturn {
    /// Write to context account data at offset 0 (ABI required)
    pub fn write_to(&self, data: &mut [u8]) -> Result<(), ProgramError> {
        if data.len() < MATCHER_RETURN_LEN {
            return Err(ProgramError::AccountDataTooSmall);
        }
        data[0..4].copy_from_slice(&self.abi_version.to_le_bytes());
        data[4..8].copy_from_slice(&self.flags.to_le_bytes());
        data[8..16].copy_from_slice(&self.exec_price_e6.to_le_bytes());
        data[16..32].copy_from_slice(&self.exec_size.to_le_bytes());
        data[32..40].copy_from_slice(&self.req_id.to_le_bytes());
        data[40..48].copy_from_slice(&self.lp_account_id.to_le_bytes());
        data[48..56].copy_from_slice(&self.oracle_price_e6.to_le_bytes());
        data[56..64].copy_from_slice(&self.asset_index.to_le_bytes());
        Ok(())
    }

    pub fn filled(
        exec_price: u64,
        exec_size: i128,
        req_id: u64,
        lp_account_id: u64,
        asset_index: u16,
        oracle_price_e6: u64,
    ) -> Self {
        Self {
            abi_version: MATCHER_ABI_VERSION,
            flags: FLAG_VALID,
            exec_price_e6: exec_price,
            exec_size,
            req_id,
            lp_account_id,
            oracle_price_e6,
            asset_index: asset_index as u64,
        }
    }
}

/// Parsed matcher call from instruction data
#[derive(Clone, Copy, Debug)]
pub struct MatcherCall {
    pub req_id: u64,
    pub asset_index: u16,
    pub lp_account_id: u64,
    pub oracle_price_e6: u64,
    pub req_size: i128,
}

impl MatcherCall {
    pub fn parse(data: &[u8]) -> Result<Self, ProgramError> {
        if data.len() != MATCHER_CALL_LEN {
            return Err(ProgramError::Custom(BAD_DATA));
        }
        if data[0] != MATCHER_CALL_TAG {
            return Err(ProgramError::Custom(BAD_DATA));
        }

        let req_id = u64::from_le_bytes(data[1..9].try_into().unwrap());
        let asset_index = u16::from_le_bytes(data[9..11].try_into().unwrap());
        let lp_account_id = u64::from_le_bytes(data[11..19].try_into().unwrap());
        let oracle_price_e6 = u64::from_le_bytes(data[19..27].try_into().unwrap());
        let req_size = i128::from_le_bytes(data[27..43].try_into().unwrap());

        // Verify reserved bytes are zero
        for &b in &data[43..67] {
            if b != 0 {
                return Err(ProgramError::Custom(BAD_DATA));
            }
        }

        Ok(Self {
            req_id,
            asset_index,
            lp_account_id,
            oracle_price_e6,
            req_size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_call_layout() {
        let mut data = [0; MATCHER_CALL_LEN];
        data[1..9].copy_from_slice(&42u64.to_le_bytes());
        data[27..43].copy_from_slice(&(-7i128).to_le_bytes());
        let call = MatcherCall::parse(&data).unwrap();
        assert_eq!(call.req_id, 42);
        assert_eq!(call.req_size, -7);
        assert!(MatcherCall::parse(&data[..66]).is_err());
        let mut long = data.to_vec();
        long.push(0);
        assert!(MatcherCall::parse(&long).is_err());
        data[66] = 1;
        assert!(MatcherCall::parse(&data).is_err());
    }
    #[test]
    fn limits_and_risk_reduction() {
        assert_eq!(next_inventory(0, 10, 10, 20).unwrap(), -10);
        assert_eq!(next_inventory(-30, -5, 10, 20).unwrap(), -25);
        assert_eq!(
            next_inventory(-20, 1, 10, 20),
            Err(ProgramError::Custom(INVENTORY_CAP))
        );
        for size in [0, i128::MIN, 11, -11] {
            assert_eq!(
                next_inventory(0, size, 10, 20),
                Err(ProgramError::Custom(FILL_TOO_LARGE))
            );
        }
        assert!(next_inventory(i128::MAX, -1, 10, 20).is_err());
        assert!(validate_params(1, LIMIT, 9000, 500).is_ok());
        for (fill, cap, age, dev) in [
            (0, 1, 1, 0),
            (2, 1, 1, 0),
            (1, LIMIT + 1, 1, 0),
            (1, 1, 0, 0),
            (1, 1, 9001, 0),
            (1, 1, 1, 501),
        ] {
            assert!(validate_params(fill, cap, age, dev).is_err());
        }
    }
}

#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

// Wrapper 5cb331dd v16_program.rs:866-880 (header), :2078-2083
// (portfolio_wire starts at HEADER_LEN=16), :2889-2906 (owner preflight).
// Engine 4db11a8 src/v16.rs:7358,20596: provenance is 100 bytes, owner follows.
pub const PORTFOLIO_OWNER_OFF: usize = 116;
const PORTFOLIO_PROVENANCE_OWNER_OFF: usize = 80;
const WRAPPER_MAGIC: u64 = 0x5045_5243_5631_3600;

fn context<'a, 'b>(
    program_id: &Pubkey,
    accounts: &'a [AccountInfo<'b>],
) -> Result<(&'a AccountInfo<'b>, &'a AccountInfo<'b>), ProgramError> {
    require(accounts.len() >= 2, BAD_ACCOUNT)?;
    let authority = &accounts[0];
    let ctx = &accounts[1];
    require(
        ctx.owner == program_id && ctx.is_writable && ctx.data_len() >= CTX_LEN,
        BAD_ACCOUNT,
    )?;
    require(authority.is_signer, MISSING_SIGNATURE)?;
    Ok((authority, ctx))
}

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    ix: &[u8],
) -> ProgramResult {
    let tag = *ix.first().ok_or(ProgramError::Custom(BAD_DATA))?;
    let expected = match tag {
        0 => 67,
        2 => 77,
        4 => 9,
        5 => 92,
        _ => return Err(ProgramError::InvalidInstructionData),
    };
    require(ix.len() == expected, BAD_DATA)?;
    let (authority, ctx) = context(program_id, accounts)?;
    let mut data = ctx
        .try_borrow_mut_data()
        .map_err(|_| ProgramError::Custom(BAD_ACCOUNT))?;
    if tag == 2 {
        return initialize(program_id, accounts, ix, &mut data);
    }
    require(
        u64::from_le_bytes(number(&data, MAGIC_OFF)) == MAGIC
            && u32::from_le_bytes(number(&data, VERSION_OFF)) == 1,
        BAD_ACCOUNT,
    )?;
    match tag {
        0 => {
            require(
                authority.key.as_ref() == &data[DELEGATE_OFF..DELEGATE_OFF + 32],
                BAD_AUTHORITY,
            )?;
            let call = MatcherCall::parse(ix)?;
            require(data[PAUSED_OFF] == 0, PAUSED)?;
            require(
                call.asset_index == u16::from_le_bytes(number(&data, ASSET_OFF)),
                BAD_ASSET,
            )?;
            require(
                call.lp_account_id == u64::from_le_bytes(number(&data, DELEGATE_OFF)),
                BAD_AUTHORITY,
            )?;
            require(call.oracle_price_e6 > 0, BAD_PRICE)?;
            let slot = Clock::get()
                .map_err(|_| ProgramError::Custom(BAD_CLOCK))?
                .slot;
            let ref_slot = u64::from_le_bytes(number(&data, REF_SLOT_OFF));
            let age = u64::from_le_bytes(number(&data, AGE_OFF));
            require(
                ref_slot != 0 && slot >= ref_slot && slot - ref_slot <= age,
                STALE,
            )?;
            let reference = u64::from_le_bytes(number(&data, REF_PRICE_OFF));
            let deviation = u16::from_le_bytes(number(&data, DEVIATION_OFF));
            // u64 difference * 10,000 and u16 * u64 are bounded within u128.
            require(
                (call.oracle_price_e6.abs_diff(reference) as u128) * 10_000
                    <= (deviation as u128) * (reference as u128),
                MARK_DEVIATES,
            )?;
            let next = next_inventory(
                i128::from_le_bytes(number(&data, INVENTORY_OFF)),
                call.req_size,
                u128::from_le_bytes(number(&data, FILL_OFF)),
                u128::from_le_bytes(number(&data, CAP_OFF)),
            )?;
            data[INVENTORY_OFF..INVENTORY_OFF + 16].copy_from_slice(&next.to_le_bytes());
            MatcherReturn::filled(
                call.oracle_price_e6,
                call.req_size,
                call.req_id,
                call.lp_account_id,
                call.asset_index,
                call.oracle_price_e6,
            )
            .write_to(&mut data)
        }
        4 => {
            require(
                authority.key.as_ref() == &data[PRICE_AUTHORITY_OFF..PRICE_AUTHORITY_OFF + 32],
                BAD_AUTHORITY,
            )?;
            let price = u64::from_le_bytes(number(ix, 1));
            require(price > 0, BAD_PRICE)?;
            let slot = Clock::get()
                .map_err(|_| ProgramError::Custom(BAD_CLOCK))?
                .slot;
            data[REF_PRICE_OFF..REF_PRICE_OFF + 8].copy_from_slice(&price.to_le_bytes());
            data[REF_SLOT_OFF..REF_SLOT_OFF + 8].copy_from_slice(&slot.to_le_bytes());
            Ok(())
        }
        5 => {
            require(
                authority.key.as_ref() == &data[LP_OWNER_OFF..LP_OWNER_OFF + 32],
                BAD_AUTHORITY,
            )?;
            let inventory = i128::from_le_bytes(number(ix, 76));
            require(
                ix[75] <= 1 && inventory.checked_abs().is_some_and(|v| v as u128 <= LIMIT),
                BAD_PARAMS,
            )?;
            store_params(&mut data, ix, 1)?;
            data[PAUSED_OFF] = ix[75];
            data[INVENTORY_OFF..INVENTORY_OFF + 16].copy_from_slice(&inventory.to_le_bytes());
            Ok(())
        }
        _ => unreachable!(),
    }
}

fn store_params(data: &mut [u8], ix: &[u8], off: usize) -> ProgramResult {
    validate_params(
        u128::from_le_bytes(number(ix, off)),
        u128::from_le_bytes(number(ix, off + 16)),
        u64::from_le_bytes(number(ix, off + 32)),
        u16::from_le_bytes(number(ix, off + 40)),
    )?;
    data[FILL_OFF..FILL_OFF + 16].copy_from_slice(&ix[off..off + 16]);
    data[CAP_OFF..CAP_OFF + 16].copy_from_slice(&ix[off + 16..off + 32]);
    data[AGE_OFF..AGE_OFF + 8].copy_from_slice(&ix[off + 32..off + 40]);
    data[DEVIATION_OFF..DEVIATION_OFF + 2].copy_from_slice(&ix[off + 40..off + 42]);
    data[PRICE_AUTHORITY_OFF..PRICE_AUTHORITY_OFF + 32].copy_from_slice(&ix[off + 42..off + 74]);
    Ok(())
}

fn initialize(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    ix: &[u8],
    data: &mut [u8],
) -> ProgramResult {
    require(accounts.len() >= 5, BAD_ACCOUNT)?;
    let owner = &accounts[0];
    let ctx = &accounts[1];
    let market = &accounts[2];
    let portfolio = &accounts[3];
    let wrapper = &accounts[4];
    require(ctx.is_signer, MISSING_SIGNATURE)?;
    require(
        data[64..CTX_LEN].iter().all(|&b| b == 0),
        ALREADY_INITIALIZED,
    )?;
    require(
        wrapper.executable && market.owner == wrapper.key && portfolio.owner == wrapper.key,
        BAD_ACCOUNT,
    )?;
    let pf = portfolio
        .try_borrow_data()
        .map_err(|_| ProgramError::Custom(BAD_ACCOUNT))?;
    require(pf.len() >= PORTFOLIO_OWNER_OFF + 32, BAD_ACCOUNT)?;
    require(
        u64::from_le_bytes(number(&pf, 0)) == WRAPPER_MAGIC
            && u16::from_le_bytes(number(&pf, 8)) == 16
            && pf[10] == 2,
        BAD_ACCOUNT,
    )?;
    require(
        &pf[PORTFOLIO_OWNER_OFF..PORTFOLIO_OWNER_OFF + 32] == owner.key.as_ref()
            && &pf[PORTFOLIO_PROVENANCE_OWNER_OFF..PORTFOLIO_PROVENANCE_OWNER_OFF + 32]
                == owner.key.as_ref(),
        BAD_AUTHORITY,
    )?;
    let delegate = Pubkey::find_program_address(
        &[
            b"matcher",
            market.key.as_ref(),
            portfolio.key.as_ref(),
            owner.key.as_ref(),
            program_id.as_ref(),
            ctx.key.as_ref(),
        ],
        wrapper.key,
    )
    .0;
    store_params(data, ix, 3)?;
    data[..64].fill(0);
    data[MAGIC_OFF..MAGIC_OFF + 8].copy_from_slice(&MAGIC.to_le_bytes());
    data[VERSION_OFF..VERSION_OFF + 4].copy_from_slice(&1u32.to_le_bytes());
    for (off, key) in [
        (WRAPPER_OFF, wrapper.key),
        (MARKET_OFF, market.key),
        (LP_PORTFOLIO_OFF, portfolio.key),
        (LP_OWNER_OFF, owner.key),
        (DELEGATE_OFF, &delegate),
    ] {
        data[off..off + 32].copy_from_slice(key.as_ref());
    }
    data[ASSET_OFF..ASSET_OFF + 2].copy_from_slice(&ix[1..3]);
    Ok(())
}
