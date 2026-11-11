//! Local money-path tests (LiteSVM + the pinned wrapper SBF, no network).
//!
//! These prove drip protects money when the market misbehaves, not just that
//! it runs. Scenario 1: an account falls below maintenance while the mark
//! moves every slot (as our pusher does). Every PushAuthMark bumps the
//! market's oracle epoch, so a single crank per slot always finds a stale
//! health certificate, refreshes, and never reaches the Liquidate arm.
//! Two cranks in one transaction share the slot and price: the first
//! refreshes, the second sees a current certificate and liquidates.
//! Scenarios 2-5 cover multiple liquidation victims, gap catchup (including
//! the ignored F1 regression), funding against a zero-funding control, and
//! permissionless Recovery through Resolved owner payouts.
//! Self-skips without the SBF unless MONEY_PATH_REQUIRE_SBF=1 (e2e CI).

mod common;

use anyhow::Result;
use drip::{
    observation::{fetch_observation_for_asset, PythFeedResolver},
    submit::{build_crank_instructions, single_crank_retry_instructions, CrankRoute, TX_CU_LIMIT},
};
use litesvm::{
    types::{FailedTransactionMetadata, TransactionMetadata},
    LiteSVM,
};
use percolator_prog::{ix::Instruction as P, state};
use solana_sdk::{
    account::Account,
    clock::Clock,
    compute_budget::ComputeBudgetInstruction as CU,
    instruction::{AccountMeta as M, Instruction, InstructionError},
    program_option::COption,
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::{Transaction, TransactionError},
};
use spl_token::state::{Account as TokenAccount, AccountState, Mint};

const PRICE: u64 = 100_000_000;
const SIZE: i128 = 10_000_000;

fn wrapper_ix(program_id: Pubkey, ix: P, accounts: Vec<M>) -> Instruction {
    Instruction {
        program_id,
        accounts,
        data: ix.encode(),
    }
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

struct Env {
    svm: LiteSVM,
    payer: Keypair,
    market: Pubkey,
    victim: Pubkey,
    portfolios: Vec<Pubkey>,
    owners: Vec<Keypair>,
    mint: Pubkey,
    vault: Pubkey,
}
impl Env {
    fn new(test: &str) -> Result<Option<Self>> {
        Self::with_positions(test, &[SIZE], 0)
    }
    fn with_positions(test: &str, sizes: &[i128], funding_cap: u64) -> Result<Option<Self>> {
        Self::with_chunk(test, sizes, funding_cap, percolator::MAX_VAULT_TVL)
    }
    fn with_chunk(
        test: &str,
        sizes: &[i128],
        funding_cap: u64,
        chunk: u128,
    ) -> Result<Option<Self>> {
        let root = std::env::var_os("PERCOLATOR_PROG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../repo-prog")
            });
        let path = root.join("target/deploy/percolator_prog.so");
        if !path.exists() {
            assert!(
                std::env::var("MONEY_PATH_REQUIRE_SBF").as_deref() != Ok("1"),
                "{test}: pinned wrapper SBF not built: {}",
                path.display()
            );
            eprintln!("SKIP {test}: pinned wrapper SBF not built");
            return Ok(None);
        }
        let mut svm = LiteSVM::new();
        svm.add_program(percolator_prog::id(), &std::fs::read(path)?);
        let payer = Keypair::new();
        let lp = Keypair::new();
        let mut owners = vec![lp];
        owners.extend(sizes.iter().map(|_| Keypair::new()));
        for key in std::iter::once(&payer).chain(owners.iter()) {
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
                min_nonzero_mm_req: 10_000_000,
                min_nonzero_im_req: 20_000_000,
                maintenance_margin_bps: 1000,
                initial_margin_bps: 2000,
                max_trading_fee_bps: 100,
                trade_fee_base_bps: 12,
                liquidation_fee_bps: 50,
                liquidation_fee_cap: 10_000_000_000,
                // A minimum fee above the full-position proportional fee forces
                // full closure instead of repeated maintenance-restoring partials.
                min_liquidation_abs: 5_000_000,
                max_price_move_bps_per_slot: 100,
                max_accrual_dt_slots: 4,
                max_abs_funding_e9_per_slot: funding_cap,
                min_funding_lifetime_slots: 64,
                max_account_b_settlement_chunks: 4,
                max_bankrupt_close_chunks: 4,
                max_bankrupt_close_lifetime_slots: 1500,
                public_b_chunk_atoms: chunk,
                maintenance_fee_per_slot: 0,
            },
            vec![
                M::new(payer.pubkey(), true),
                M::new(market, false),
                M::new_readonly(mint, false),
            ],
        );
        send(&mut svm, &payer, &[ix], &[]).expect("InitMarket");
        check(&svm, market, &[]);
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
        check(&svm, market, &[]);
        let mut portfolios = Vec::new();
        for (i, owner) in owners.iter().enumerate() {
            let deposit = if i == 0 {
                10_000_000_000u64
            } else {
                210_000_000u64
            };
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
            portfolios.push(pf);
            check(&svm, market, &portfolios);
            let data = svm.get_account(&pf).unwrap().data;
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
                    amount: u128::from(deposit),
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
            check(&svm, market, &portfolios);
        }

        let mut e = Self {
            svm,
            payer,
            market,
            victim: portfolios[1],
            portfolios,
            owners,
            mint,
            vault,
        };
        // InitMarket leaves the liquidation reward share at zero: no reward portfolio.
        assert_eq!(
            state::read_market(&e.data(market))?
                .0
                .liquidation_cranker_fee_share_bps,
            0
        );
        e.check();
        for (i, size) in sizes.iter().enumerate() {
            if *size == 0 {
                continue;
            }
            let victim = e.portfolios[i + 1];
            let ix = wrapper_ix(
                percolator_prog::id(),
                P::TradeNoCpi {
                    account_a_portfolio_id: state::read_portfolio_id(&e.data(victim))?,
                    account_a_position_epoch: state::read_portfolio_position_epoch(
                        &e.data(victim),
                    )?,
                    account_b_portfolio_id: state::read_portfolio_id(&e.data(e.portfolios[0]))?,
                    account_b_position_epoch: state::read_portfolio_position_epoch(
                        &e.data(e.portfolios[0]),
                    )?,
                    asset_index: 0,
                    market_id: state::read_market(&e.data(market))?.1.assets[0].market_id,
                    size_q: *size,
                    exec_price: PRICE,
                    fee_bps: 12,
                    backing_fee_cap_bps: 0,
                },
                vec![
                    M::new(e.owners[i + 1].pubkey(), true),
                    M::new(e.owners[0].pubkey(), true),
                    M::new(market, false),
                    M::new(victim, false),
                    M::new(e.portfolios[0], false),
                ],
            );
            send(
                &mut e.svm,
                &e.payer,
                &[ix],
                &[&e.owners[0], &e.owners[i + 1]],
            )
            .expect("TradeNoCpi");
            e.check();
        }
        Ok(Some(e))
    }
    fn data(&self, key: Pubkey) -> Vec<u8> {
        self.svm.get_account(&key).unwrap().data
    }
    fn check(&self) {
        check(&self.svm, self.market, &self.portfolios);
        let vault = TokenAccount::unpack(&self.data(self.vault)).unwrap().amount;
        assert_eq!(
            u128::from(vault),
            common::decode_market(&self.data(self.market))
                .unwrap()
                .vault
        );
    }
    fn push(&mut self, price: u64) {
        let slot = self.svm.get_sysvar::<Clock>().slot + 1;
        self.svm.warp_to_slot(slot);
        let data = self.data(self.market);
        let seq = state::read_asset_control_sequences(&data, 0).unwrap();
        let ix = wrapper_ix(
            percolator_prog::id(),
            P::PushAuthMark {
                market_id: state::read_market(&data).unwrap().1.assets[0].market_id,
                asset_index: 0,
                now_slot: slot,
                mark_e6: price,
                observation_sequence: seq.oracle_observation + 1,
                authority_epoch: seq.authority_epoch,
            },
            vec![
                M::new(self.payer.pubkey(), true),
                M::new(self.market, false),
            ],
        );
        send(&mut self.svm, &self.payer, &[ix], &[]).expect("PushAuthMark");
        self.check();
    }
    fn crank(
        &mut self,
        paired: bool,
    ) -> std::result::Result<TransactionMetadata, Box<FailedTransactionMetadata>> {
        self.crank_portfolio(self.victim, paired)
    }
    fn crank_portfolio(
        &mut self,
        portfolio: Pubkey,
        paired: bool,
    ) -> std::result::Result<TransactionMetadata, Box<FailedTransactionMetadata>> {
        let obs =
            fetch_observation_for_asset(&self.data(self.market), 0, &PythFeedResolver::empty())
                .unwrap();
        assert!(obs.oracle_accounts.is_empty());
        let slot = self.svm.get_sysvar::<Clock>().slot;
        let (ixs, idx) = build_crank_instructions(
            &percolator_prog::id(),
            &self.payer.pubkey(),
            &self.market,
            &portfolio,
            &CrankRoute::Live {
                reward_portfolio: None,
            },
            &[obs],
            slot,
            paired,
        );
        let result = self.send_crank(&ixs);
        if let Err(ref failure) = result {
            eprintln!("slot {slot} paired={paired}: {}", failure.err);
            // LiteSVM failure models preflight rejection: no instruction changes landed.
            if paired {
                if let Some(single) =
                    single_crank_retry_instructions(&ixs, idx, true, &failure.err.to_string())
                {
                    return self.send_crank(&single);
                }
            }
        }
        result
    }
    fn send_crank(
        &mut self,
        ixs: &[Instruction],
    ) -> std::result::Result<TransactionMetadata, Box<FailedTransactionMetadata>> {
        self.svm.expire_blockhash();
        let tx = Transaction::new_signed_with_payer(
            ixs,
            Some(&self.payer.pubkey()),
            &[&self.payer],
            self.svm.latest_blockhash(),
        );
        let result = self.svm.send_transaction(tx).map_err(Box::new);
        self.check();
        let cu = match &result {
            Ok(meta) => meta.compute_units_consumed,
            Err(f) => f.meta.compute_units_consumed,
        };
        eprintln!("crank ixs={} CU={cu}", ixs.len());
        assert!(
            cu < u64::from(TX_CU_LIMIT) * 9 / 10,
            "retain at least 10% CU headroom"
        );
        result
    }
}

fn check(svm: &LiteSVM, market: Pubkey, portfolios: &[Pubkey]) {
    let mg = common::decode_market(&svm.get_account(&market).unwrap().data).unwrap();
    let portfolios: Vec<_> = portfolios
        .iter()
        .map(|p| common::decode_portfolio(&svm.get_account(p).unwrap().data).unwrap())
        .collect();
    common::assert_conservation(&mg, &portfolios);
}

fn send(
    svm: &mut LiteSVM,
    payer: &Keypair,
    ixs: &[Instruction],
    signers: &[&Keypair],
) -> std::result::Result<TransactionMetadata, Box<FailedTransactionMetadata>> {
    svm.expire_blockhash();
    let mut all = vec![
        CU::set_compute_unit_limit(TX_CU_LIMIT),
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
    svm.send_transaction(tx).map_err(Box::new)
}

fn moving_market(test: &str, paired: bool) -> Result<()> {
    let Some(mut e) = Env::new(test)? else {
        return Ok(());
    };
    let mut price = PRICE;
    let mut below_slot = None;
    for step in 1..=40 {
        // A 1% move per slot crosses MM in about 13 slots. Continue changing
        // the target after crossing so a quiet slot cannot rescue a stale cert.
        price = if step <= 20 || step % 2 == 0 {
            price * 99 / 100
        } else {
            price + price / 100
        };
        e.push(price);
        // Zero funding/maintenance and this single long give equity = deposit
        // minus entry fee plus size * (mark-entry)/1e6; MM is 10% of notional.
        let equity = 210_000_000i128 - 1_200_000
            + SIZE * (i128::from(price) - i128::from(PRICE)) / 1_000_000;
        let mm = SIZE * i128::from(price) / 1_000_000 / 10;
        if equity < mm && below_slot.is_none() {
            below_slot = Some(step);
        }
        let result = e.crank(paired);
        assert!(result.is_ok(), "crank: {result:?}");
        let a = common::decode_portfolio(&e.data(e.victim))?;
        eprintln!("step={step} price={price} equity={equity} mm={mm} basis={} cert_epoch={} cert_equity={} cert_mm={}", a.legs[0].basis_pos_q.get(), a.health_cert.cert_oracle_epoch.get(), a.health_cert.certified_equity.get(), a.health_cert.certified_maintenance_req.get());
        if a.legs[0].basis_pos_q.get() == 0 {
            let below = below_slot.expect("must cross maintenance before liquidation");
            eprintln!(
                "{test}: liquidated at step {step}, {} slots after below maintenance",
                step - below
            );
            assert!(step - below <= 8);
            return Ok(());
        }
        if let Some(below) = below_slot {
            assert!(
                step - below < 8,
                "{test}: still open {step} slots from start, 8 slots after below MM"
            );
        }
    }
    panic!("never crossed maintenance or liquidated");
}

#[test]
fn paired_crank_liquidates_in_a_moving_market() -> Result<()> {
    moving_market("paired_crank_liquidates_in_a_moving_market", true)
}

#[test]
#[ignore = "known-bad: crank cert goes stale every slot; engine b22bef7d has percolator#248 but the wrapper crank still needs accrue-first wiring; the keeper uses paired cranks (paired_crank_liquidates_in_a_moving_market passes)"]
fn single_crank_liquidates_in_a_moving_market() -> Result<()> {
    moving_market("single_crank_liquidates_in_a_moving_market", false)
}

#[test]
fn paired_crank_on_healthy_account_keeps_first_step() -> Result<()> {
    let Some(mut e) = Env::new("paired_crank_on_healthy_account_keeps_first_step")? else {
        return Ok(());
    };
    e.push(PRICE * 99 / 100);
    let before = e.data(e.victim);
    let obs = fetch_observation_for_asset(&e.data(e.market), 0, &PythFeedResolver::empty())?;
    let (ixs, idx) = build_crank_instructions(
        &percolator_prog::id(),
        &e.payer.pubkey(),
        &e.market,
        &e.victim,
        &CrankRoute::Live {
            reward_portfolio: None,
        },
        &[obs],
        e.svm.get_sysvar::<Clock>().slot,
        true,
    );
    let failure = e.send_crank(&ixs).expect_err("second crank must be idle");
    assert_eq!(
        failure.err,
        TransactionError::InstructionError((idx + 1) as u8, InstructionError::Custom(0x16))
    );
    assert_eq!(
        e.data(e.victim),
        before,
        "failed pair rolls back first refresh"
    );
    // LiteSVM failure models preflight rejection: no instruction changes landed.
    let single = single_crank_retry_instructions(&ixs, idx, true, &failure.err.to_string())
        .expect("preflight rejection retries the first crank");
    e.send_crank(&single).expect("single refresh lands");
    assert_ne!(e.data(e.victim), before, "fallback keeps refresh progress");
    let prior = common::decode_portfolio(&before)?;
    let after = common::decode_portfolio(&e.data(e.victim))?;
    assert!(after.health_cert.cert_oracle_epoch.get() > prior.health_cert.cert_oracle_epoch.get());
    assert_eq!(after.health_cert.valid, 1);
    assert_eq!(after.legs[0].basis_pos_q, prior.legs[0].basis_pos_q);
    Ok(())
}

// Mirror submit's three-way classification and scheduler::record_attempt:
// successful unchanged target bytes and EngineNonProgress are idle; only a
// non-idle rejection contributes a failure streak. Failed pairs are atomic.
fn classified_crank(
    e: &mut Env,
    pf: Pubkey,
    q: &mut drip::quarantine::Quarantine,
    cycle: u64,
) -> drip::submit::OutcomeClass {
    use drip::{
        quarantine::FailureReport,
        submit::{wrapper_error_name, CrankOutcome, OutcomeClass},
    };
    let before = e.data(pf);
    let priority = drip::work::assess(
        &common::decode_portfolio(&before).unwrap(),
        &common::decode_market(&e.data(e.market)).unwrap(),
    )
    .priority;
    let plan_class = format!("live:{}", priority.label());
    let pending_work = priority != drip::work::Priority::Quiet;
    let class = match e.crank_portfolio(pf, true) {
        Ok(_) if before != e.data(pf) => OutcomeClass::Progress,
        Ok(_) => OutcomeClass::Idle,
        Err(f) => {
            let error = f.err.to_string();
            let outcome = CrankOutcome::HardReject {
                error_name: wrapper_error_name(&error),
                error,
                program_cu: None,
                tx_cu: Some(f.meta.compute_units_consumed),
            };
            if outcome.class() == OutcomeClass::Failed {
                let error = outcome.error_key().unwrap();
                q.record_failure(FailureReport {
                    account: &pf,
                    error: &error,
                    cycle,
                    slot: e.svm.get_sysvar::<Clock>().slot,
                    plan_class: &plan_class,
                });
            }
            outcome.class()
        }
    };
    match class {
        OutcomeClass::Progress => q.record_ok(&pf),
        OutcomeClass::Idle => {
            q.record_idle(&pf, pending_work);
        }
        OutcomeClass::Failed => {}
    }
    class
}

// The 33-slot gap needs ceil(33/4) accrual segments, at most five paired
// attempts, plus refresh/liquidation. Sixteen attempts leave room for bounded
// source/close continuations. Require a current certificate AND idle, so
// market-only catchup cannot be mistaken for an unchanged healthy account.
fn converge(e: &mut Env, pf: Pubkey, bound: u64) -> Result<u64> {
    let mut q = drip::quarantine::Quarantine::default();
    for attempt in 1..=bound {
        let class = classified_crank(e, pf, &mut q, attempt);
        assert_ne!(
            class,
            drip::submit::OutcomeClass::Failed,
            "non-idle reject for {pf}"
        );
        let mg = common::decode_market(&e.data(e.market))?;
        let p = common::decode_portfolio(&e.data(pf))?;
        if class == drip::submit::OutcomeClass::Idle
            && p.health_cert.valid != 0
            && p.health_cert.cert_risk_epoch.get() == mg.risk_epoch
            && p.health_cert.cert_asset_set_epoch.get() == mg.asset_set_epoch
            && p.health_cert.cert_oracle_epoch.get() == mg.oracle_epoch
            && p.health_cert.cert_funding_epoch.get() == mg.funding_epoch
            && mg.assets[0].slot_last == e.svm.get_sysvar::<Clock>().slot
        {
            return Ok(attempt);
        }
    }
    panic!("{pf} failed to converge within {bound} paired attempts");
}

#[test]
fn fast_drop_liquidates_every_underwater_account() -> Result<()> {
    let sizes = [10_000_000i128, 9_500_000, 9_000_000, 8_500_000];
    let Some(mut e) =
        Env::with_positions("fast_drop_liquidates_every_underwater_account", &sizes, 0)?
    else {
        return Ok(());
    };
    let mut q = drip::quarantine::Quarantine::default();
    let mut crossed = [None; 4];
    let mut closed = [None; 4];
    let mut price = PRICE;
    for step in 1..=40u64 {
        price = price * 99 / 100;
        e.push(price);
        for (i, size) in sizes.iter().enumerate() {
            if closed[i].is_some() {
                continue;
            }
            let pf = e.portfolios[i + 1];
            let equity = 210_000_000 - size * i128::from(PRICE) / 1_000_000 * 12 / 10_000
                + size * (i128::from(price) - i128::from(PRICE)) / 1_000_000;
            let mm = (size * i128::from(price) / 1_000_000 / 10).max(10_000_000);
            if equity < mm && crossed[i].is_none() {
                crossed[i] = Some(step);
            }
            assert_ne!(
                classified_crank(&mut e, pf, &mut q, step),
                drip::submit::OutcomeClass::Failed
            );
            assert!(!q.is_blocked(&pf, step));
            if common::decode_portfolio(&e.data(pf))?.legs[0]
                .basis_pos_q
                .get()
                == 0
            {
                let below = crossed[i].expect("liquidation must follow maintenance crossing");
                assert!(
                    step - below <= 8,
                    "one refresh + liquidation should fit in eight slots"
                );
                closed[i] = Some(step);
            } else if let Some(below) = crossed[i] {
                assert!(step - below < 8);
            }
        }
        if closed.iter().all(Option::is_some) {
            break;
        }
    }
    assert!(closed.iter().all(Option::is_some));
    assert!(
        crossed.windows(2).all(|w| w[0] != w[1]),
        "sizes cross on different slots"
    );
    assert!(q.events().is_empty());
    let mg = common::decode_market(&e.data(e.market))?;
    eprintln!("fast drop: crossed={crossed:?} liquidated={closed:?} quarantines={} insurance={} counterparty_capital={}", q.events().len(), mg.insurance, common::decode_portfolio(&e.data(e.portfolios[0]))?.capital.get());
    Ok(())
}

#[test]
fn gapped_price_catch_up_has_no_wedge() -> Result<()> {
    let Some(mut e) = Env::new("gapped_price_catch_up_has_no_wedge")? else {
        return Ok(());
    };
    // Matcher stale-mark refusal is independently covered by
    // matcher/host/tests/litesvm.rs::stale_mark_rejected. Here test drip's
    // accrual catchup after eight max-accrual windows with no pusher.
    e.svm.warp_to_slot(42);
    e.push(PRICE * 80 / 100);
    let counterparty = e.portfolios[0];
    let counterparty_attempts = converge(&mut e, counterparty, 16)?;
    let price = common::decode_market(&e.data(e.market))?.assets[0].effective_price;
    let equity = 208_800_000 + SIZE * (i128::from(price) - i128::from(PRICE)) / 1_000_000;
    let mm = SIZE * i128::from(price) / 10_000_000;
    assert!(equity < mm, "gap must leave a genuinely underwater victim");
    let victim = e.victim;
    let victim_attempts = converge(&mut e, victim, 16)?;
    assert_eq!(
        common::decode_portfolio(&e.data(victim))?.legs[0]
            .basis_pos_q
            .get(),
        0
    );
    eprintln!("gap: 32 unpushed slots, catchup paired attempts=[{counterparty_attempts}, {victim_attempts}], equity={equity} mm={mm}, victim fully liquidated");
    Ok(())
}

#[test]
fn funding_over_many_slots_stays_consistent() -> Result<()> {
    let Some(mut funded) = Env::with_positions(
        "funding_over_many_slots_stays_consistent",
        &[1_000_000],
        1_000,
    )?
    else {
        return Ok(());
    };
    let mut control = Env::with_positions("funding control", &[1_000_000], 0)?.unwrap();
    // AUTH_MARK is price-managed. canonical_accrual_path_for_target_view
    // invokes funding_rate_from_checkpoint_view even with an oracle-less
    // hint. Re-offset the target above the 1% clamp on every push so the
    // premium persists after the initial ramp, including the final 100 slots.
    let initial = common::decode_market(&funded.data(funded.market))?;
    let initial_legs: Vec<_> = funded
        .portfolios
        .iter()
        .map(|pf| common::decode_portfolio(&funded.data(*pf)).unwrap().legs[0])
        .collect();
    let mut window_epochs = vec![initial.funding_epoch];
    let mut max_attempts = 0;
    for step in 1..=300 {
        for e in [&mut funded, &mut control] {
            let price = common::decode_market(&e.data(e.market))?.assets[0].effective_price;
            e.push(price * 120 / 100);
            if step % 4 == 0 {
                for pf in e.portfolios.clone() {
                    max_attempts = max_attempts.max(converge(e, pf, 8)?);
                }
            }
        }
        if step % 100 == 0 {
            let epoch = common::decode_market(&funded.data(funded.market))?.funding_epoch;
            assert!(
                epoch > *window_epochs.last().unwrap(),
                "funding must advance in every 100-slot window"
            );
            window_epochs.push(epoch);
        }
    }
    let mg = common::decode_market(&funded.data(funded.market))?;
    let cg = common::decode_market(&control.data(control.market))?;
    // Positive mark premium charges longs and credits shorts.
    assert!(mg.assets[0].f_long_num < initial.assets[0].f_long_num);
    assert!(mg.assets[0].f_short_num > initial.assets[0].f_short_num);
    assert_eq!(cg.assets[0].f_long_num, 0);
    assert_eq!(cg.assets[0].f_short_num, 0);
    assert_eq!(mg.assets[0].effective_price, cg.assets[0].effective_price);
    let mut transfers = Vec::new();
    let mut expected_transfers = Vec::new();
    for i in 0..funded.portfolios.len() {
        let f = common::decode_portfolio(&funded.data(funded.portfolios[i]))?;
        let c = common::decode_portfolio(&control.data(control.portfolios[i]))?;
        transfers
            .push(f.capital.get() as i128 + f.pnl.get() - c.capital.get() as i128 - c.pnl.get());
        let leg = &initial_legs[i];
        assert_eq!(f.legs[0].basis_pos_q, leg.basis_pos_q);
        assert_eq!(f.legs[0].a_basis, leg.a_basis);
        let index = if leg.basis_pos_q.get() > 0 {
            mg.assets[0].f_long_num
        } else {
            mg.assets[0].f_short_num
        };
        // v16::leg_kf_delta_components_for_settlement_from_asset floors
        // abs(basis_pos_q) * (f_now - f_snap) / (a_basis * POS_SCALE).
        // Summing <=75 separately floored settlements differs from one
        // aggregate floor by <75 atoms per portfolio (no position changes).
        let expected = (leg.basis_pos_q.get().abs() * (index - leg.f_snap.get()))
            .div_euclid(leg.a_basis.get() as i128 * percolator::POS_SCALE as i128);
        expected_transfers.push(expected);
        assert_eq!(transfers[i].signum(), -leg.basis_pos_q.get().signum());
        assert!(
            (transfers[i] - expected).abs() < 75,
            "portfolio {i}: transfer={} expected={expected}",
            transfers[i]
        );
    }
    // Two portfolios, <=75 settlements: at most one atom per side/settlement.
    let rounding_bound = 2 * 75;
    assert!(transfers.iter().sum::<i128>().abs() <= rounding_bound);
    eprintln!("funding: 300 slots window_epochs={window_epochs:?} epoch={} f_long={} f_short={} transfers={transfers:?} expected={expected_transfers:?} net={} rounding_bound={rounding_bound} max_paired_attempts={max_attempts}", mg.funding_epoch, mg.assets[0].f_long_num, mg.assets[0].f_short_num, transfers.iter().sum::<i128>());
    Ok(())
}

#[test]
#[ignore = "known-bad on pin: F1 / upstream percolator#243: unreferenced Fresh backing lapses in Live and winner refresh reverts EngineStale (0x13)"]
fn lapsed_unreferenced_backing_does_not_wedge_winner() -> Result<()> {
    let Some(mut e) = Env::new("lapsed_unreferenced_backing_does_not_wedge_winner")? else {
        return Ok(());
    };
    let data = e.data(e.market);
    let seq = state::read_asset_control_sequences(&data, 0)?;
    let source = Pubkey::new_unique();
    e.svm.set_account(
        source,
        account(spl_token::ID, token(e.mint, e.payer.pubkey(), 1_000_000)),
    )?;
    let ix = wrapper_ix(
        percolator_prog::id(),
        P::TopUpBackingBucket {
            domain: 1,
            market_id: common::decode_market(&data)?.assets[0].market_id,
            intent_id: seq.backing_top_up + 1,
            authority_epoch: seq.authority_epoch,
            backing_fee_bps: 0,
            insurance_share_bps: 0,
            amount: 1_000_000,
            expiry_slot: 30,
        },
        vec![
            M::new(e.payer.pubkey(), true),
            M::new(e.market, false),
            M::new(source, false),
            M::new(e.vault, false),
            M::new_readonly(spl_token::ID, false),
        ],
    );
    send(&mut e.svm, &e.payer, &[ix], &[]).expect("TopUpBackingBucket");
    e.check();
    let bucket = common::decode_market(&e.data(e.market))?.source_backing_buckets[1];
    assert_eq!(bucket.status, percolator::BackingBucketStatusV16::Fresh);
    assert_eq!(bucket.valid_liened_backing_num, 0);
    for pf in &e.portfolios {
        let p = common::decode_portfolio(&e.data(*pf))?;
        assert!(
            p.source_domains
                .iter()
                .all(|d| d.source_claim_market_id.get() == 0),
            "bucket must lapse unreferenced"
        );
    }
    e.svm.warp_to_slot(31);
    e.push(PRICE * 110 / 100);
    let mut q = drip::quarantine::Quarantine::default();
    let victim = e.victim;
    let mut classes = Vec::new();
    for cycle in 0..16 {
        classes.push(classified_crank(&mut e, victim, &mut q, cycle));
        if !q.events().is_empty() {
            break;
        }
    }
    eprintln!(
        "F1: outcomes={classes:?} quarantine_events={:?}",
        q.events()
    );
    assert!(
        classes
            .iter()
            .all(|c| *c != drip::submit::OutcomeClass::Failed),
        "winner must refresh after an unreferenced backing bucket lapses"
    );
    assert!(q.events().is_empty());
    converge(&mut e, victim, 16)?;
    assert!(common::decode_portfolio(&e.data(victim))?.pnl.get() > 0);
    Ok(())
}

#[test]
fn recovery_to_resolved_pays_every_claimant() -> Result<()> {
    // A tiny public B chunk prevents this insolvent close from progressing
    // within its chunk budget, declaring permissionless Recovery. A raw mark
    // jump alone is clamped by the wrapper, not Recovery. Include a passive
    // depositor alongside the insolvent long and the solvent short.
    let Some(mut e) = Env::with_chunk(
        "recovery_to_resolved_pays_every_claimant",
        &[SIZE, 0],
        0,
        10_000_000,
    )?
    else {
        return Ok(());
    };
    let total_deposits = common::decode_market(&e.data(e.market))?.vault;
    e.svm.warp_to_slot(50);
    e.push(PRICE / 2);
    let counterparty = e.portfolios[0];
    // Settle the well-funded short first, so the victim sees the whole gap
    // before its first health refresh and is bankrupt rather than partially
    // liquidated at the first crossing of maintenance.
    converge(&mut e, counterparty, 16)?;
    // Only the preceding oracle push needs the market/oracle authority.
    // Every crank entering/advancing Recovery and paying Resolved claims
    // below is signed solely by this fresh, unprivileged keeper.
    let keeper = Keypair::new();
    e.svm.airdrop(&keeper.pubkey(), 100_000_000_000).unwrap();
    assert_ne!(keeper.pubkey(), e.payer.pubkey());
    assert!(e.owners.iter().all(|owner| owner.pubkey() != keeper.pubkey()));
    e.payer = keeper;
    let victim = e.victim;
    for _ in 0..8 {
        e.crank_portfolio(victim, true)
            .expect("bankruptcy continuation");
        if common::decode_market(&e.data(e.market))?.mode == percolator::MarketModeV16::Recovery {
            break;
        }
    }
    assert_eq!(
        common::decode_market(&e.data(e.market))?.mode,
        percolator::MarketModeV16::Recovery
    );
    let mut mg = common::decode_market(&e.data(e.market))?;
    assert!(
        mg.recovery_reason.is_some(),
        "must go through Recovery, not admin resolve"
    );
    for _ in 0..8 {
        if mg.mode == percolator::MarketModeV16::Resolved {
            break;
        }
        for pf in e.portfolios.clone() {
            if common::decode_market(&e.data(e.market))?.mode == percolator::MarketModeV16::Resolved
            {
                break;
            }
            e.crank_portfolio(pf, true).expect("Recovery continuation");
        }
        mg = common::decode_market(&e.data(e.market))?;
    }
    assert_eq!(mg.mode, percolator::MarketModeV16::Resolved);
    eprintln!(
        "recovery: reason={:?} resolved_slot={}",
        mg.recovery_reason, mg.resolved_slot
    );
    // Bankruptcy creates backing with a raw expiry. This engine pin does
    // not honor the newer horizon cap, so wait for raw expiry before asking
    // drip's Resolved hints to normalize it and finalize haircut receipts.
    let expiry = mg
        .source_backing_buckets
        .iter()
        .filter(|b| b.status == percolator::BackingBucketStatusV16::Fresh)
        .map(|b| b.expiry_slot)
        .max()
        .unwrap_or(mg.resolved_slot);
    let delay = state::read_market(&e.data(e.market))?.0.force_close_delay_slots;
    let payout_slot = (expiry + 1).max(mg.resolved_slot + delay);
    e.svm.warp_to_slot(payout_slot);
    assert!(!drip::resolved::resolved_crank_hints(&mg, payout_slot, 0).is_empty());
    let vault_authority =
        Pubkey::find_program_address(&[b"vault", e.market.as_ref()], &percolator_prog::id()).0;
    let mut paid = vec![0u128; e.portfolios.len()];
    let mut closed = vec![false; e.portfolios.len()];
    // Revisit winners after losers: positive payouts wait for the engine's
    // blocker census. A <=1B-atom position loss needs <=100 ten-million
    // atom B chunks; 28 further calls cover detach, expiry and receipts.
    let mut rounds = 0;
    for round in 0..128 {
        rounds = round + 1;
        for i in 0..e.portfolios.len() {
            if closed[i] {
                continue;
            }
            let pf = e.portfolios[i];
            let owner = e.owners[i].pubkey();
            let ata = spl_associated_token_account::get_associated_token_address(&owner, &e.mint);
            let balance = |e: &Env| {
                e.svm
                    .get_account(&ata)
                    .map(|a| TokenAccount::unpack(&a.data).unwrap().amount)
                    .unwrap_or(0)
            };
            let before = balance(&e);
            let mg = common::decode_market(&e.data(e.market))?;
            let hints =
                drip::resolved::resolved_crank_hints(&mg, e.svm.get_sysvar::<Clock>().slot, round);
            // Replay the exact engine transition on copies of the authoritative
            // account views. The returned Closed{payout} is independent of the
            // SPL transfer and catches a wrapper that pays a different amount.
            let mut market_data = e.data(e.market);
            let mut portfolio_data = e.data(pf);
            let (cfg, mut group) = state::market_view_mut(&mut market_data)?;
            let mut portfolio = state::portfolio_view_mut_for_market_slots(&mut portfolio_data, 1)?;
            let observations: Vec<_> = hints
                .iter()
                .map(|h| percolator::AutoCrankObservationV16 {
                    asset_index: usize::from(h.asset_index),
                    effective_price: 0,
                    funding_rate_e9: 0,
                })
                .collect();
            let expected = group.permissionless_auto_crank_not_atomic(
                &mut portfolio,
                percolator::AutoCrankWorkV16 {
                    now_slot: e.svm.get_sysvar::<Clock>().slot,
                    observations: &observations,
                    resolved_close_fee_rate_per_slot: cfg.maintenance_fee_per_slot,
                },
            );
            let engine_payout = match &expected {
                Ok(result) => match result.outcome {
                    percolator::AutoCrankOutcomeV16::ResolvedClose(
                        percolator::ResolvedCloseOutcomeV16::Closed { payout },
                    ) => payout,
                    percolator::AutoCrankOutcomeV16::ResolvedClose(
                        percolator::ResolvedCloseOutcomeV16::ProgressOnly,
                    ) => 0,
                    percolator::AutoCrankOutcomeV16::NoAction => 0,
                    ref other => panic!("unexpected Resolved engine outcome: {other:?}"),
                },
                Err(percolator::V16Error::NonProgress) => 0,
                Err(error) => panic!("engine payout preview: {error:?}"),
            };
            let (ixs, _) = build_crank_instructions(
                &percolator_prog::id(),
                &e.payer.pubkey(),
                &e.market,
                &pf,
                &CrankRoute::Resolved {
                    owner,
                    mint: e.mint,
                    vault: e.vault,
                    vault_authority,
                },
                &hints,
                e.svm.get_sysvar::<Clock>().slot,
                true,
            );
            match e.send_crank(&ixs) {
                Ok(_) => assert!(expected.is_ok(), "SBF must agree with pinned engine"),
                Err(f) => {
                    assert!(
                        matches!(expected, Err(percolator::V16Error::NonProgress)),
                        "SBF rejection must agree with engine preview: {expected:?}"
                    );
                    assert_eq!(
                        drip::submit::wrapper_error_name(&f.err.to_string()),
                        Some("EngineNonProgress"),
                        "resolved reject: {f:?}"
                    );
                }
            }
            assert_eq!(
                u128::from(balance(&e) - before),
                engine_payout,
                "owner receives exactly the engine-reported payout"
            );
            paid[i] += engine_payout;
            let p = common::decode_portfolio(&e.data(pf))?;
            closed[i] = p.capital.get() == 0
                && p.pnl.get() == 0
                && p.active_bitmap.iter().all(|x| x.get() == 0)
                && (p.resolved_payout_receipt.present == 0
                    || p.resolved_payout_receipt.finalized != 0);
        }
        if closed.iter().all(|x| *x) {
            break;
        }
    }
    assert!(
        closed.iter().all(|x| *x),
        "every claimant must finish: paid={paid:?} closed={closed:?}"
    );
    // Use the engine's complete economic-emptiness predicate, including
    // locks, source obligations and finalized receipts, rather than relying
    // only on capital/pnl == 0. Deregister copies; leave on-chain accounts
    // available for the final conservation check.
    let mut market_data = e.data(e.market);
    let (_, mut group) = state::market_view_mut(&mut market_data)?;
    for pf in &e.portfolios {
        let mut data = e.data(*pf);
        let portfolio = state::portfolio_view_mut_for_market_slots(&mut data, 1)?;
        group
            .deregister_empty_materialized_portfolio_not_atomic(&portfolio.as_view())
            .expect("every portfolio must satisfy the engine's closed predicate");
    }
    // The short receives its own deposit and the bankrupt long's deposit,
    // less one retained 12-bps entry fee. The other entry fee is consumed
    // as bankruptcy insurance; the passive depositor's principal is untouched.
    let entry_fee = SIZE as u128 * u128::from(PRICE) / percolator::POS_SCALE * 12 / 10_000;
    assert_eq!(paid[0], 10_000_000_000 + 210_000_000 - entry_fee);
    assert_eq!(paid[1], 0, "bankrupt long has no remaining claim");
    assert_eq!(
        paid[2], 210_000_000,
        "passive depositor receives its entire principal"
    );
    let mg = common::decode_market(&e.data(e.market))?;
    assert_eq!(paid.iter().sum::<u128>() + mg.vault, total_deposits);
    // No externally deposited backing in this scenario. Allow one atom per account and
    // per side for integer settlement dust beyond protocol-owned insurance.
    let dust = e.portfolios.len() as u128 + 2;
    eprintln!(
        "resolved: rounds={rounds} paid={paid:?} vault_residual={} insurance={} dust_bound={dust}",
        mg.vault, mg.insurance
    );
    assert!(mg.vault <= mg.insurance + dust);
    e.check();
    Ok(())
}

/// Winner-first guard (engine #172 site 2) on the pinned wrapper: after a
/// price drop that no crank has accrued yet, `pending_gains` must flag the
/// short counterparty and not the long victim, on real account state. Then
/// cranking the winner to its fixpoint (what the cycle's front pass does; the
/// selection itself is unit-tested in cycle.rs) settles its gain while the
/// victim is still unliquidated.
#[test]
fn pending_gains_flags_the_winner_before_any_crank() -> Result<()> {
    let Some(mut e) = Env::new("pending_gains_flags_the_winner_before_any_crank")? else {
        return Ok(());
    };
    let counterparty = e.portfolios[0];
    let victim = e.victim;
    e.svm.warp_to_slot(50);
    e.push(PRICE / 2);
    let gains = |e: &Env, pf: Pubkey| {
        drip::work::pending_gains(
            &common::decode_portfolio(&e.data(pf)).unwrap(),
            &common::decode_market(&e.data(e.market)).unwrap(),
        )
    };
    assert_eq!(gains(&e, counterparty), vec![(0, 1)], "short winner flagged before accrual");
    assert!(gains(&e, victim).is_empty(), "long loser is never a winner");
    // Crank the winner to its fixpoint first: the catch-up accrues the move,
    // then the leg settles a gain.
    converge(&mut e, counterparty, 16)?;
    let winner = common::decode_portfolio(&e.data(counterparty))?;
    let mg = common::decode_market(&e.data(e.market))?;
    assert_eq!(winner.legs[0].k_snap.get(), mg.assets[0].k_short, "winner settled K");
    assert!(winner.pnl.get() > 0, "winner holds the gain: {}", winner.pnl.get());
    assert!(gains(&e, victim).is_empty());
    Ok(())
}
