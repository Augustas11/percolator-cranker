//! Fixtures for the frontend's TypeScript client (`frontend/src/chain/`).
//!
//! Emits, from the pinned upstream wrapper crate (no hand-derived bytes):
//! * `layout.json`: account offsets via `offset_of!` (the same source as
//!   upstream `examples/dump_layout.rs`) plus the wrapper's layout constants;
//! * `instructions.json`: `Instruction::encode` bytes for the 7 instructions
//!   the client builds, with the arguments used;
//! * `accounts.json`: values upstream's own readers parse out of the devnet
//!   account dumps (`market.bin`, `portfolio.bin`, `lp_portfolio.bin`).
//!
//!   cargo run --bin ts_fixtures -- ../frontend/src/chain/fixtures

use core::mem::{offset_of, size_of};
use percolator::{
    AssetStateV16Account, EngineAssetSlotV16Account, MarketGroupV16HeaderAccount,
    PortfolioAccountV16Account, PortfolioLegV16Account, V16ConfigAccount,
};
use percolator_prog::constants::*;
use percolator_prog::ix::{CrankObservationHint, Instruction as P};
use percolator_prog::state::{self, AssetOracleStorageV16, WrapperConfigV16};
use std::fmt::Write as _;

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: ts_fixtures <fixtures-dir>");
    let dir = std::path::Path::new(&dir);
    std::fs::write(dir.join("layout.json"), layout()).unwrap();
    std::fs::write(dir.join("instructions.json"), instructions()).unwrap();
    let mut accounts = String::from("{\n");
    accounts += &format!("  \"market\": {},\n", market(&std::fs::read(dir.join("market.bin")).unwrap()));
    accounts += &format!("  \"portfolio\": {},\n", portfolio(&std::fs::read(dir.join("portfolio.bin")).unwrap()));
    accounts += &format!("  \"lp_portfolio\": {}\n}}\n", portfolio(&std::fs::read(dir.join("lp_portfolio.bin")).unwrap()));
    std::fs::write(dir.join("accounts.json"), accounts).unwrap();
    std::fs::write(dir.join("pdas.json"), pdas()).unwrap();
}

/// PDAs with the wrapper's seeds (`derive_vault_authority`, `canonical_vault_address`,
/// `derive_matcher_delegate` in v16_program.rs; private there, so the seeds are
/// restated here) for the devnet market and sample matcher keys.
fn pdas() -> String {
    use solana_program::pubkey::Pubkey;
    let pk = |s: &str| s.parse::<Pubkey>().unwrap();
    let program = pk("GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS");
    let market = pk("7p8DtcMHSjdCsM9xWvBwF6TebGBSL18wKvfM7BS1zyHq");
    let mint = pk("Ev8AkZC9pT2QCncYFut8GyGB2ffWN1yvnoVbiUTz77q7");
    let lp_pf = pk("GKJHRYBKwtswFHK5oyGknpcXxYEMjh7rDDR9f1KU9T5P");
    let lp_owner = Pubkey::new_from_array([0x3f; 32]);
    let matcher_prog = Pubkey::new_from_array([0x4d; 32]);
    let matcher_ctx = Pubkey::new_from_array([0x63; 32]);
    let ata_prog = pk("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
    let (vault_auth, _) = Pubkey::find_program_address(&[b"vault", market.as_ref()], &program);
    let (vault, _) =
        Pubkey::find_program_address(&[vault_auth.as_ref(), spl_token::ID.as_ref(), mint.as_ref()], &ata_prog);
    let (delegate, _) = Pubkey::find_program_address(
        &[b"matcher", market.as_ref(), lp_pf.as_ref(), lp_owner.as_ref(), matcher_prog.as_ref(), matcher_ctx.as_ref()],
        &program,
    );
    format!(
        "{{\n  \"program\": \"{program}\",\n  \"market\": \"{market}\",\n  \"mint\": \"{mint}\",\n  \"vaultAuthority\": \"{vault_auth}\",\n  \
         \"vault\": \"{vault}\",\n  \"matcherDelegate\": {{\"makerAccount\": \"{lp_pf}\", \"makerOwner\": \"{lp_owner}\", \
         \"matcherProgram\": \"{matcher_prog}\", \"matcherContext\": \"{matcher_ctx}\", \"delegate\": \"{delegate}\"}}\n}}\n"
    )
}

fn layout() -> String {
    let asset0 = MARKET_GROUP_OFF
        + MarketGroupV16HeaderAccount::dynamic_asset_slot_offset::<AssetOracleStorageV16>(0).unwrap();
    let fields: Vec<(&str, usize)> = vec![
        ("HEADER_LEN", HEADER_LEN),
        ("MAGIC_OFF", 0),
        ("VERSION_OFF", 8),
        ("KIND_OFF", 10),
        ("MARKET_GROUP_OFF", MARKET_GROUP_OFF),
        ("MARKET_GROUP_LEN", MARKET_GROUP_LEN),
        ("ASSET0_SLOT_OFF", asset0),
        ("ASSET_ORACLE_WRAPPER_LEN", ASSET_ORACLE_WRAPPER_LEN),
        ("ENGINE_ASSET_SLOT_LEN", size_of::<EngineAssetSlotV16Account>()),
        ("PORTFOLIO_ACCOUNT_LEN", PORTFOLIO_ACCOUNT_LEN),
        ("PORTFOLIO_MATCHER_CONTROL_OFF", PORTFOLIO_MATCHER_CONTROL_OFF),
        ("PORTFOLIO_ID_OFF", PORTFOLIO_ID_OFF),
        ("PORTFOLIO_MATCHER_SEQUENCE_OFF", PORTFOLIO_MATCHER_SEQUENCE_OFF),
        // WrapperConfigV16, from HEADER_LEN.
        ("cfg.marketauth", offset_of!(WrapperConfigV16, marketauth)),
        ("cfg.collateral_mint", offset_of!(WrapperConfigV16, collateral_mint)),
        ("cfg.trade_fee_base_bps", offset_of!(WrapperConfigV16, trade_fee_base_bps)),
        ("cfg.oracle_mode", offset_of!(WrapperConfigV16, oracle_mode)),
        // MarketGroupV16HeaderAccount, from MARKET_GROUP_OFF.
        ("group.market_group_id", offset_of!(MarketGroupV16HeaderAccount, market_group_id)),
        ("group.config", offset_of!(MarketGroupV16HeaderAccount, config)),
        ("group.vault", offset_of!(MarketGroupV16HeaderAccount, vault)),
        ("group.insurance", offset_of!(MarketGroupV16HeaderAccount, insurance)),
        ("group.current_slot", offset_of!(MarketGroupV16HeaderAccount, current_slot)),
        ("group.mode", offset_of!(MarketGroupV16HeaderAccount, mode)),
        // V16ConfigAccount, from group.config.
        ("config.maintenance_margin_bps", offset_of!(V16ConfigAccount, maintenance_margin_bps)),
        ("config.initial_margin_bps", offset_of!(V16ConfigAccount, initial_margin_bps)),
        ("config.max_trading_fee_bps", offset_of!(V16ConfigAccount, max_trading_fee_bps)),
        // AssetStateV16Account, from ASSET0_SLOT_OFF + ASSET_ORACLE_WRAPPER_LEN + slot.asset
        // (v16_program.rs asset_slot_range: the oracle wrapper precedes the engine slot).
        ("slot.asset", offset_of!(EngineAssetSlotV16Account, asset)),
        ("asset.market_id", offset_of!(AssetStateV16Account, market_id)),
        ("asset.lifecycle", offset_of!(AssetStateV16Account, lifecycle)),
        ("asset.effective_price", offset_of!(AssetStateV16Account, effective_price)),
        ("asset.slot_last", offset_of!(AssetStateV16Account, slot_last)),
        ("asset.a_long", offset_of!(AssetStateV16Account, a_long)),
        ("asset.a_short", offset_of!(AssetStateV16Account, a_short)),
        ("asset.epoch_long", offset_of!(AssetStateV16Account, epoch_long)),
        ("asset.epoch_short", offset_of!(AssetStateV16Account, epoch_short)),
        // PortfolioAccountV16Account, from HEADER_LEN.
        // provenance_header.market_group_id is the header's first field.
        ("pf.provenance_header", offset_of!(PortfolioAccountV16Account, provenance_header)),
        ("pf.owner", offset_of!(PortfolioAccountV16Account, owner)),
        ("pf.capital", offset_of!(PortfolioAccountV16Account, capital)),
        ("pf.pnl", offset_of!(PortfolioAccountV16Account, pnl)),
        ("pf.active_bitmap", offset_of!(PortfolioAccountV16Account, active_bitmap)),
        ("pf.legs", offset_of!(PortfolioAccountV16Account, legs)),
        ("LEG_LEN", size_of::<PortfolioLegV16Account>()),
        ("LEG_COUNT", percolator::V16_MAX_PORTFOLIO_ASSETS_N),
        // PortfolioLegV16Account, from pf.legs + i * LEG_LEN.
        ("leg.active", offset_of!(PortfolioLegV16Account, active)),
        ("leg.asset_index", offset_of!(PortfolioLegV16Account, asset_index)),
        ("leg.market_id", offset_of!(PortfolioLegV16Account, market_id)),
        ("leg.side", offset_of!(PortfolioLegV16Account, side)),
        ("leg.basis_pos_q", offset_of!(PortfolioLegV16Account, basis_pos_q)),
        ("leg.a_basis", offset_of!(PortfolioLegV16Account, a_basis)),
        ("leg.epoch_snap", offset_of!(PortfolioLegV16Account, epoch_snap)),
    ];
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("  \"{k}\": {v}")).collect();
    format!("{{\n{}\n}}\n", body.join(",\n"))
}

fn instructions() -> String {
    // Distinctive values: every byte position and sign path is exercised.
    let cases: Vec<(&str, String, P)> = vec![
        ("InitPortfolio", "{}".into(), P::InitPortfolio),
        (
            "Deposit",
            r#"{"portfolioId":"7","expectedSequence":"258","amount":"340282366920938463463374607431768211455"}"#.into(),
            P::Deposit { portfolio_id: 7, expected_sequence: 258, amount: u128::MAX },
        ),
        (
            "Withdraw",
            r#"{"portfolioId":"18446744073709551615","expectedSequence":"1","amount":"30000000000"}"#.into(),
            P::Withdraw { portfolio_id: u64::MAX, expected_sequence: 1, amount: 30_000_000_000 },
        ),
        (
            "PermissionlessCrank",
            r#"{"nowSlot":"516276483","observations":[{"assetIndex":0,"oracleAccounts":0}]}"#.into(),
            P::PermissionlessCrank {
                now_slot: 516_276_483,
                observations: vec![CrankObservationHint { asset_index: 0, oracle_accounts: 0 }],
            },
        ),
        (
            "TradeNoCpi",
            r#"{"accountAPortfolioId":"3","accountAPositionEpoch":"5","accountBPortfolioId":"2","accountBPositionEpoch":"9","assetIndex":0,"marketId":"1","sizeQ":"-10000000","execPrice":"149850000","feeBps":"12","backingFeeCapBps":0}"#.into(),
            P::TradeNoCpi {
                account_a_portfolio_id: 3,
                account_a_position_epoch: 5,
                account_b_portfolio_id: 2,
                account_b_position_epoch: 9,
                asset_index: 0,
                market_id: 1,
                size_q: -10_000_000,
                exec_price: 149_850_000,
                fee_bps: 12,
                backing_fee_cap_bps: 0,
            },
        ),
        (
            "TradeCpi",
            r#"{"accountAPortfolioId":"3","accountAPositionEpoch":"5","accountBPortfolioId":"2","accountBPositionEpoch":"9","accountBMatcherSequence":"11","assetIndex":0,"marketId":"1","sizeQ":"250000","feeBps":"12","limitPrice":"150150000","backingFeeCapBps":7}"#.into(),
            P::TradeCpi {
                account_a_portfolio_id: 3,
                account_a_position_epoch: 5,
                account_b_portfolio_id: 2,
                account_b_position_epoch: 9,
                account_b_matcher_sequence: 11,
                asset_index: 0,
                market_id: 1,
                size_q: 250_000,
                fee_bps: 12,
                limit_price: 150_150_000,
                backing_fee_cap_bps: 7,
            },
        ),
        (
            "ClosePortfolio",
            r#"{"portfolioId":"3","expectedSequence":"4","positionEpoch":"6"}"#.into(),
            P::ClosePortfolio { portfolio_id: 3, expected_sequence: 4, position_epoch: 6 },
        ),
        (
            "CloseResolved",
            r#"{"feeRatePerSlot":"0"}"#.into(),
            P::CloseResolved { fee_rate_per_slot: 0 },
        ),
    ];
    let body: Vec<String> = cases
        .iter()
        .map(|(name, args, ix)| format!("  \"{name}\": {{\"args\": {args}, \"hex\": \"{}\"}}", hex(&ix.encode())))
        .collect();
    format!("{{\n{}\n}}\n", body.join(",\n"))
}

fn market(data: &[u8]) -> String {
    let (cfg, g) = state::read_market(data).expect("read_market");
    let a = &g.assets[0];
    format!(
        "{{\"marketauth\": \"{}\", \"collateralMint\": \"{}\", \"tradeFeeBaseBps\": \"{}\", \"oracleMode\": {}, \
         \"maintenanceMarginBps\": \"{}\", \"initialMarginBps\": \"{}\", \"vault\": \"{}\", \"insurance\": \"{}\", \
         \"currentSlot\": \"{}\", \"mode\": {}, \"asset0\": {{\"marketId\": \"{}\", \"effectivePrice\": \"{}\", \
         \"slotLast\": \"{}\", \"aLong\": \"{}\", \"aShort\": \"{}\", \"epochLong\": \"{}\", \"epochShort\": \"{}\"}}, \
         \"marketGroupId\": \"{}\"}}",
        hex(&cfg.marketauth),
        hex(&cfg.collateral_mint),
        cfg.trade_fee_base_bps,
        cfg.oracle_mode,
        g.config.maintenance_margin_bps,
        g.config.initial_margin_bps,
        g.vault,
        g.insurance,
        g.current_slot,
        g.mode as u8,
        a.market_id,
        a.effective_price,
        a.slot_last,
        a.a_long,
        a.a_short,
        a.epoch_long,
        a.epoch_short,
        hex(&g.market_group_id),
    )
}

fn portfolio(data: &[u8]) -> String {
    let p = state::read_portfolio(data).expect("read_portfolio");
    let legs: Vec<String> = p
        .legs
        .iter()
        .filter(|l| l.active != 0)
        .map(|l| {
            format!(
                "{{\"assetIndex\": {}, \"marketId\": \"{}\", \"side\": {}, \"basisPosQ\": \"{}\", \"aBasis\": \"{}\", \"epochSnap\": \"{}\"}}",
                l.asset_index.get(), l.market_id.get(), l.side, l.basis_pos_q.get(), l.a_basis.get(), l.epoch_snap.get()
            )
        })
        .collect();
    format!(
        "{{\"owner\": \"{}\", \"portfolioId\": \"{}\", \"sequence\": \"{}\", \"positionEpoch\": \"{}\", \
         \"capital\": \"{}\", \"pnl\": \"{}\", \"marketGroupId\": \"{}\", \"legs\": [{}]}}",
        hex(&p.owner),
        state::read_portfolio_id(data).unwrap(),
        state::read_portfolio_matcher_sequence(data).unwrap(),
        state::read_portfolio_position_epoch(data).unwrap(),
        p.capital.get(),
        p.pnl.get(),
        hex(&p.provenance_header.market_group_id),
        legs.join(", "),
    )
}
