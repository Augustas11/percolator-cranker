use drip_matcher_host::*;
use litesvm::{types::FailedTransactionMetadata, LiteSVM};
use percolator_prog::{ix::Instruction as P, state};
use solana_sdk::{
    account::Account,
    clock::Clock,
    compute_budget::ComputeBudgetInstruction as CU,
    instruction::InstructionError,
    instruction::{AccountMeta as M, Instruction},
    program_option::COption,
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::{Transaction, TransactionError},
};
use spl_token::state::{Account as TokenAccount, AccountState, Mint};

const PRICE: u64 = 100_000_000;
const SIZE: i128 = 1_000_000;

struct Env {
    svm: LiteSVM,
    payer: Keypair,
    lp: Keypair,
    taker: Keypair,
    market: Pubkey,
    lp_pf: Pubkey,
    taker_pf: Pubkey,
    program: Pubkey,
    ctx: Keypair,
}

fn send(
    svm: &mut LiteSVM,
    payer: &Keypair,
    ixs: &[Instruction],
    signers: &[&Keypair],
) -> Result<(), FailedTransactionMetadata> {
    svm.expire_blockhash();
    let mut all = vec![
        CU::set_compute_unit_limit(1_400_000),
        CU::request_heap_frame(128 * 1024),
    ];
    all.extend_from_slice(ixs);
    let mut keys = vec![payer];
    for key in signers {
        if !keys.iter().any(|k| k.pubkey() == key.pubkey()) {
            keys.push(key);
        }
    }
    let tx = Transaction::new_signed_with_payer(
        &all,
        Some(&payer.pubkey()),
        &keys,
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).map(|_| ())
}
fn account(owner: Pubkey, data: Vec<u8>) -> Account {
    Account {
        lamports: 1_000_000_000,
        data,
        owner,
        executable: false,
        rent_epoch: 0,
    }
}
fn token(mint: Pubkey, owner: Pubkey, amount: u64) -> Vec<u8> {
    let mut data = vec![0; TokenAccount::LEN];
    TokenAccount::pack(
        TokenAccount {
            mint,
            owner,
            amount,
            delegate: COption::None,
            state: AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        },
        &mut data,
    )
    .unwrap();
    data
}
impl Env {
    fn new(init: bool, reference: bool) -> Self {
        let mut svm = LiteSVM::new();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let wrapper = std::env::var_os("PERCOLATOR_PROG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| root.join("../../repo-prog"))
            .join("target/deploy/percolator_prog.so");
        let matcher = root.join("../program/target/deploy/drip_matcher.so");
        svm.add_program(
            percolator_prog::id(),
            &std::fs::read(&wrapper).unwrap_or_else(|e| {
                panic!("missing pinned wrapper SBF {}: {e}", wrapper.display())
            }),
        );
        let program = Pubkey::new_unique();
        svm.add_program(
            program,
            &std::fs::read(&matcher).unwrap_or_else(|e| {
                panic!(
                    "missing matcher SBF {}; run cd matcher/program && cargo build-sbf: {e}",
                    matcher.display()
                )
            }),
        );
        let payer = Keypair::new();
        let lp = Keypair::new();
        let taker = Keypair::new();
        let ctx = Keypair::new();
        for key in [&payer, &lp, &taker] {
            svm.airdrop(&key.pubkey(), 100_000_000_000).unwrap();
        }
        svm.warp_to_slot(10);
        let market = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let vault_authority =
            Pubkey::find_program_address(&[b"vault", market.as_ref()], &percolator_prog::id()).0;
        let ata_program: Pubkey = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
            .parse()
            .unwrap();
        let vault = Pubkey::find_program_address(
            &[
                vault_authority.as_ref(),
                spl_token::ID.as_ref(),
                mint.as_ref(),
            ],
            &ata_program,
        )
        .0;
        let mut mint_data = vec![0; Mint::LEN];
        Mint::pack(
            Mint {
                mint_authority: COption::Some(payer.pubkey()),
                supply: 20_000_000_000,
                decimals: 6,
                is_initialized: true,
                freeze_authority: COption::None,
            },
            &mut mint_data,
        )
        .unwrap();
        svm.set_account(mint, account(spl_token::ID, mint_data))
            .unwrap();
        svm.set_account(
            vault,
            account(spl_token::ID, token(mint, vault_authority, 0)),
        )
        .unwrap();
        svm.set_account(
            market,
            account(
                percolator_prog::id(),
                vec![0; state::market_account_len_for_capacity(1).unwrap()],
            ),
        )
        .unwrap();
        let ix = wrapper_ix(
            percolator_prog::id(),
            P::InitMarket {
                max_portfolio_assets: 1,
                h_min: 0,
                h_max: 150,
                initial_price: PRICE,
                min_nonzero_mm_req: 1000,
                min_nonzero_im_req: 2000,
                maintenance_margin_bps: 1000,
                initial_margin_bps: 2000,
                max_trading_fee_bps: 100,
                trade_fee_base_bps: 12,
                liquidation_fee_bps: 50,
                liquidation_fee_cap: 10_000_000_000,
                min_liquidation_abs: 0,
                max_price_move_bps_per_slot: 5,
                max_accrual_dt_slots: 64,
                max_abs_funding_e9_per_slot: 10_000,
                min_funding_lifetime_slots: 64,
                max_account_b_settlement_chunks: 4,
                max_bankrupt_close_chunks: 4,
                max_bankrupt_close_lifetime_slots: 1500,
                public_b_chunk_atoms: percolator::MAX_VAULT_TVL,
                maintenance_fee_per_slot: 0,
            },
            vec![
                M::new(payer.pubkey(), true),
                M::new(market, false),
                M::new_readonly(mint, false),
            ],
        );
        send(&mut svm, &payer, &[ix], &[]).expect("InitMarket");
        let market_data = svm.get_account(&market).unwrap().data;
        let seq = state::read_asset_control_sequences(&market_data, 0).unwrap();
        let ix = wrapper_ix(
            percolator_prog::id(),
            P::ConfigureAuthMark {
                market_id: state::read_market(&market_data).unwrap().1.assets[0].market_id,
                asset_index: 0,
                now_slot: 10,
                initial_mark_e6: PRICE,
                observation_sequence: seq.oracle_observation + 1,
                authority_epoch: seq.authority_epoch,
            },
            vec![M::new(payer.pubkey(), true), M::new(market, false)],
        );
        send(&mut svm, &payer, &[ix], &[]).expect("ConfigureAuthMark");
        let mut portfolios = Vec::new();
        for owner in [&lp, &taker] {
            let pf = Pubkey::new_unique();
            svm.set_account(
                pf,
                account(
                    percolator_prog::id(),
                    vec![0; state::portfolio_account_len_for_market_slots(1).unwrap()],
                ),
            )
            .unwrap();
            let ix = wrapper_ix(
                percolator_prog::id(),
                P::InitPortfolio,
                vec![
                    M::new(owner.pubkey(), true),
                    M::new(market, false),
                    M::new(pf, false),
                ],
            );
            send(&mut svm, &payer, &[ix], &[owner]).expect("InitPortfolio");
            let data = svm.get_account(&pf).unwrap().data;
            assert_eq!(
                &data[PORTFOLIO_OWNER_OFF..PORTFOLIO_OWNER_OFF + 32],
                owner.pubkey().as_ref()
            );
            assert_eq!(
                state::read_portfolio_owner_preflight(&data).unwrap().1,
                owner.pubkey().to_bytes()
            );
            assert_eq!(&data[80..112], owner.pubkey().as_ref());
            assert_eq!(
                &data[0..8],
                &percolator_prog::constants::MAGIC.to_le_bytes()
            );
            assert_eq!(&data[8..10], &16u16.to_le_bytes());
            assert_eq!(data[10], 2);
            let source = Pubkey::new_unique();
            svm.set_account(
                source,
                account(spl_token::ID, token(mint, owner.pubkey(), 10_000_000_000)),
            )
            .unwrap();
            let ix = wrapper_ix(
                percolator_prog::id(),
                P::Deposit {
                    portfolio_id: state::read_portfolio_id(&data).unwrap(),
                    expected_sequence: state::read_portfolio_matcher_sequence(&data).unwrap(),
                    amount: 10_000_000_000,
                },
                vec![
                    M::new(owner.pubkey(), true),
                    M::new(market, false),
                    M::new(pf, false),
                    M::new(source, false),
                    M::new(vault, false),
                    M::new_readonly(spl_token::ID, false),
                ],
            );
            send(&mut svm, &payer, &[ix], &[owner]).expect("Deposit");
            portfolios.push(pf);
        }
        svm.set_account(ctx.pubkey(), account(program, vec![0; CTX_LEN]))
            .unwrap();
        let mut env = Self {
            svm,
            payer,
            lp,
            taker,
            market,
            lp_pf: portfolios[0],
            taker_pf: portfolios[1],
            program,
            ctx,
        };
        if init {
            let ix = env.init_ix(env.lp.pubkey(), true);
            send(&mut env.svm, &env.payer, &[ix], &[&env.lp, &env.ctx]).expect("Init ctx");
            let d = env.data(env.ctx.pubkey());
            assert_eq!(d.len(), CTX_LEN);
            assert_eq!(&d[..MATCHER_RETURN_LEN], &[0; MATCHER_RETURN_LEN]);
            assert_eq!(&d[MAGIC_OFF..MAGIC_OFF + 8], b"DRIPMTC1");
            assert_eq!(&d[VERSION_OFF..VERSION_OFF + 4], &1u32.to_le_bytes());
            for (off, key) in [
                (WRAPPER_OFF, percolator_prog::id()),
                (MARKET_OFF, market),
                (LP_PORTFOLIO_OFF, env.lp_pf),
                (LP_OWNER_OFF, env.lp.pubkey()),
                (PRICE_AUTHORITY_OFF, env.payer.pubkey()),
                (
                    DELEGATE_OFF,
                    delegate(
                        percolator_prog::id(),
                        market,
                        env.lp_pf,
                        env.lp.pubkey(),
                        program,
                        env.ctx.pubkey(),
                    ),
                ),
            ] {
                assert_eq!(&d[off..off + 32], key.as_ref());
            }
            assert_eq!(&d[ASSET_OFF..ASSET_OFF + 2], &0u16.to_le_bytes());
            assert_eq!(d[PAUSED_OFF], 0);
            assert_eq!(&d[FILL_OFF..FILL_OFF + 16], &2_000_000u128.to_le_bytes());
            assert_eq!(&d[CAP_OFF..CAP_OFF + 16], &3_000_000u128.to_le_bytes());
            assert_eq!(&d[INVENTORY_OFF..INVENTORY_OFF + 16], &0i128.to_le_bytes());
            assert_eq!(&d[AGE_OFF..AGE_OFF + 8], &20u64.to_le_bytes());
            assert_eq!(&d[DEVIATION_OFF..DEVIATION_OFF + 2], &100u16.to_le_bytes());
            assert!(d[REF_PRICE_OFF..CTX_LEN].iter().all(|&b| b == 0));
            let ix = set_matcher_config(
                percolator_prog::id(),
                market,
                env.lp_pf,
                env.lp.pubkey(),
                program,
                env.ctx.pubkey(),
                &env.data(env.lp_pf),
                &env.data(market),
                100_000,
            )
            .unwrap();
            send(&mut env.svm, &env.payer, &[ix], &[&env.lp]).expect("SetMatcherConfig");
        }
        if reference {
            env.reference(PRICE);
        }
        env
    }
    fn data(&self, key: Pubkey) -> Vec<u8> {
        self.svm.get_account(&key).unwrap().data
    }
    fn init_ix(&self, owner: Pubkey, ctx_signer: bool) -> Instruction {
        Instruction {
            program_id: self.program,
            accounts: vec![
                M::new_readonly(owner, true),
                M::new(self.ctx.pubkey(), ctx_signer),
                M::new_readonly(self.market, false),
                M::new_readonly(self.lp_pf, false),
                M::new_readonly(percolator_prog::id(), false),
            ],
            data: init_data(0, 2_000_000, 3_000_000, 20, 100, self.payer.pubkey()),
        }
    }
    fn reference(&mut self, price: u64) {
        let slot = self.svm.get_sysvar::<Clock>().slot;
        let push = push_mark(
            percolator_prog::id(),
            self.market,
            self.payer.pubkey(),
            &self.data(self.market),
            slot,
            PRICE,
        )
        .unwrap();
        let update = update_ref(self.program, self.payer.pubkey(), self.ctx.pubkey(), price);
        send(&mut self.svm, &self.payer, &[push, update], &[]).expect("PushAuthMark + UpdateRef");
        let d = self.data(self.ctx.pubkey());
        assert_eq!(&d[REF_PRICE_OFF..REF_PRICE_OFF + 8], &price.to_le_bytes());
        assert_eq!(&d[REF_SLOT_OFF..REF_SLOT_OFF + 8], &slot.to_le_bytes());
    }
    fn fill(&mut self, size: i128) -> Result<(), FailedTransactionMetadata> {
        let ix = trade(
            percolator_prog::id(),
            self.market,
            self.taker.pubkey(),
            self.taker_pf,
            self.lp_pf,
            self.program,
            self.ctx.pubkey(),
            &self.data(self.market),
            &self.data(self.taker_pf),
            &self.data(self.lp_pf),
            size,
        )
        .unwrap();
        send(&mut self.svm, &self.payer, &[ix], &[&self.taker])
    }
    fn inventory(&self) -> i128 {
        i128::from_le_bytes(
            self.data(self.ctx.pubkey())[INVENTORY_OFF..INVENTORY_OFF + 16]
                .try_into()
                .unwrap(),
        )
    }
    fn resync(&mut self, inventory: i128, cap: u128) {
        let ix = Instruction {
            program_id: self.program,
            accounts: vec![
                M::new_readonly(self.lp.pubkey(), true),
                M::new(self.ctx.pubkey(), false),
            ],
            data: set_params_data(2_000_000, cap, 20, 100, self.payer.pubkey(), 0, inventory),
        };
        send(&mut self.svm, &self.payer, &[ix], &[&self.lp]).unwrap();
    }
}
fn custom(result: Result<(), FailedTransactionMetadata>, code: u32) {
    let error = result.expect_err("must reject");
    assert!(
        error
            .meta
            .logs
            .iter()
            .any(|line| line.contains("Program log:") || line.contains("invoke")),
        "{:?}",
        error
    );
    assert!(
        matches!(error.err, TransactionError::InstructionError(_, InstructionError::Custom(c)) if c == code),
        "expected matcher code {code}: {error:?}"
    );
}

#[test]
fn fill_within_caps_succeeds() {
    let mut e = Env::new(true, true);
    let req_id = state::read_market(&e.data(e.market))
        .unwrap()
        .0
        .matcher_req_seq
        + 1;
    e.fill(SIZE).unwrap();
    assert_eq!(e.inventory(), -SIZE);
    for (pf, size) in [(e.taker_pf, SIZE), (e.lp_pf, -SIZE)] {
        let portfolio = state::read_portfolio(&e.data(pf)).unwrap();
        assert_eq!(
            portfolio.legs[0].try_to_runtime().unwrap().basis_pos_q,
            size
        );
    }
    let d = e.data(e.ctx.pubkey());
    assert_eq!(u32::from_le_bytes(d[0..4].try_into().unwrap()), 3);
    assert_eq!(u32::from_le_bytes(d[4..8].try_into().unwrap()), 1);
    assert_eq!(u64::from_le_bytes(d[8..16].try_into().unwrap()), PRICE);
    assert_eq!(i128::from_le_bytes(d[16..32].try_into().unwrap()), SIZE);
    assert_eq!(&d[32..40], &req_id.to_le_bytes());
    let del = delegate(
        percolator_prog::id(),
        e.market,
        e.lp_pf,
        e.lp.pubkey(),
        e.program,
        e.ctx.pubkey(),
    );
    assert_eq!(&d[40..48], &del.as_ref()[0..8]);
    assert_eq!(&d[48..56], &PRICE.to_le_bytes());
    assert_eq!(&d[56..64], &0u64.to_le_bytes());
}
#[test]
fn fill_pays_fee_floor_and_conserves_value() {
    let mut e = Env::new(true, true);
    let taker_before = state::read_portfolio(&e.data(e.taker_pf)).unwrap();
    let lp_before = state::read_portfolio(&e.data(e.lp_pf)).unwrap();
    let (cfg, market_before) = state::read_market(&e.data(e.market)).unwrap();
    assert_eq!(cfg.trade_fee_base_bps, 12);

    e.fill(SIZE).unwrap();

    let taker_after = state::read_portfolio(&e.data(e.taker_pf)).unwrap();
    let lp_after = state::read_portfolio(&e.data(e.lp_pf)).unwrap();
    let (_, market_after) = state::read_market(&e.data(e.market)).unwrap();
    // This lower bound floors both divisions; the engine ceils fee notional and
    // then the bps charge. These inputs divide exactly under either rounding.
    let notional = SIZE.unsigned_abs() * u128::from(PRICE) / 1_000_000;
    let fee_floor = notional * 12 / 10_000;
    assert_eq!(notional, 100_000_000);
    assert_eq!(fee_floor, 120_000);
    let taker_debit = taker_before.capital.get() - taker_after.capital.get();
    let lp_debit = lp_before.capital.get() - lp_after.capital.get();
    let insurance_fee = market_after.insurance - market_before.insurance;
    assert!(taker_debit >= fee_floor);
    assert_eq!(taker_debit, fee_floor);
    // TradeCpi charges both sides the base fee; the LP's net credit is negative.
    assert_eq!(lp_debit, fee_floor);
    let lp_credit = lp_after.capital.get() as i128 - lp_before.capital.get() as i128;
    assert_eq!(taker_debit as i128, lp_credit + insurance_fee as i128);
    assert_eq!(insurance_fee, 2 * fee_floor);
    assert_eq!(market_after.vault, market_before.vault);
    assert_eq!(market_before.c_tot - market_after.c_tot, insurance_fee);
    for (before, after) in [(&taker_before, &taker_after), (&lp_before, &lp_after)] {
        assert_eq!(before.pnl.get(), 0);
        assert_eq!(after.pnl.get(), 0);
    }
    // Domain budgets earmark the same insurance atoms, not additional value.
    assert_eq!(
        market_after.insurance_domain_budget_remaining_total
            - market_before.insurance_domain_budget_remaining_total,
        insurance_fee
    );
    for domain in 0..2 {
        assert_eq!(
            market_after.insurance_domain_budget[domain]
                - market_before.insurance_domain_budget[domain],
            fee_floor
        );
    }
}

#[test]
fn fee_below_market_minimum_rejected() {
    let mut e = Env::new(true, true);
    let market_before = e.data(e.market);
    let taker_before = e.data(e.taker_pf);
    let lp_before = e.data(e.lp_pf);
    let ctx_before = e.data(e.ctx.pubkey());
    let minimum = state::read_market(&market_before)
        .unwrap()
        .0
        .trade_fee_base_bps;
    assert_eq!(minimum, 12);
    let mut ix = trade(
        percolator_prog::id(),
        e.market,
        e.taker.pubkey(),
        e.taker_pf,
        e.lp_pf,
        e.program,
        e.ctx.pubkey(),
        &market_before,
        &taker_before,
        &lp_before,
        SIZE,
    )
    .unwrap();
    let mut instruction = P::decode(&ix.data).unwrap();
    match &mut instruction {
        P::TradeCpi { fee_bps, .. } => *fee_bps = minimum - 1,
        _ => panic!("expected TradeCpi"),
    }
    ix.data = instruction.encode();
    custom(
        send(&mut e.svm, &e.payer, &[ix], &[&e.taker]),
        percolator_prog::error::PercolatorError::InvalidInstruction as u32,
    );
    assert_eq!(e.data(e.market), market_before);
    assert_eq!(e.data(e.taker_pf), taker_before);
    assert_eq!(e.data(e.lp_pf), lp_before);
    assert_eq!(e.data(e.ctx.pubkey()), ctx_before);
}

#[test]
fn oversized_fill_rejected() {
    let mut e = Env::new(true, true);
    custom(e.fill(2_000_001), FILL_TOO_LARGE);
    assert_eq!(e.inventory(), 0);
}
#[test]
fn inventory_cap_rejected() {
    let mut e = Env::new(true, true);
    e.fill(2_000_000).unwrap();
    custom(e.fill(2_000_000), INVENTORY_CAP);
    e.resync(-4_000_000, 3_000_000);
    e.fill(-500_000).unwrap();
    assert_eq!(e.inventory(), -3_500_000);
}
#[test]
fn stale_mark_rejected() {
    let mut e = Env::new(true, true);
    e.svm.warp_to_slot(31);
    custom(e.fill(SIZE), STALE);
}
#[test]
fn no_ref_rejected() {
    let mut e = Env::new(true, false);
    custom(e.fill(SIZE), STALE);
}
#[test]
fn mark_deviation_rejected() {
    let mut e = Env::new(true, true);
    e.reference(PRICE * 2);
    custom(e.fill(SIZE), MARK_DEVIATES);
}
#[test]
fn init_by_non_owner_rejected() {
    let mut e = Env::new(false, false);
    let ix = e.init_ix(e.taker.pubkey(), true);
    custom(
        send(&mut e.svm, &e.payer, &[ix], &[&e.taker, &e.ctx]),
        BAD_AUTHORITY,
    );
}
#[test]
fn reinit_rejected() {
    let mut e = Env::new(true, true);
    let ix = e.init_ix(e.lp.pubkey(), true);
    custom(
        send(&mut e.svm, &e.payer, &[ix], &[&e.lp, &e.ctx]),
        ALREADY_INITIALIZED,
    );
}
#[test]
fn init_without_ctx_signer_rejected() {
    let mut e = Env::new(false, false);
    let ix = e.init_ix(e.lp.pubkey(), false);
    custom(
        send(&mut e.svm, &e.payer, &[ix], &[&e.lp]),
        MISSING_SIGNATURE,
    );
}
#[test]
fn direct_call_without_delegate_rejected() {
    let mut e = Env::new(true, true);
    let mut data = vec![0; MATCHER_CALL_LEN];
    data[27..43].copy_from_slice(&SIZE.to_le_bytes());
    let ix = Instruction {
        program_id: e.program,
        accounts: vec![
            M::new_readonly(e.taker.pubkey(), true),
            M::new(e.ctx.pubkey(), false),
        ],
        data,
    };
    custom(
        send(&mut e.svm, &e.payer, &[ix], &[&e.taker]),
        BAD_AUTHORITY,
    );
}
