//! Local money-path tests (LiteSVM + the pinned wrapper SBF, no network).
//!
//! These prove drip protects money when the market misbehaves, not just that
//! it runs. Scenario 1: an account falls below maintenance while the mark
//! moves every slot (as our pusher does). Every PushAuthMark bumps the
//! market's oracle epoch, so a single crank per slot always finds a stale
//! health certificate, refreshes, and never reaches the Liquidate arm.
//! Two cranks in one transaction share the slot and price: the first
//! refreshes, the second sees a current certificate and liquidates.
//! Self-skips locally when the SBF is not built; CI requires it.

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
}
impl Env {
    fn new(test: &str) -> Result<Option<Self>> {
        let root = std::env::var_os("PERCOLATOR_PROG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../repo-prog")
            });
        let path = root.join("target/deploy/percolator_prog.so");
        if !path.exists() {
            assert!(std::env::var_os("CI").is_none(), "{test}: pinned wrapper SBF not built: {}", path.display());
            eprintln!("SKIP {test}: pinned wrapper SBF not built");
            return Ok(None);
        }
        let mut svm = LiteSVM::new();
        svm.add_program(percolator_prog::id(), &std::fs::read(path)?);
        let payer = Keypair::new();
        let lp = Keypair::new();
        let taker = Keypair::new();
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
                max_abs_funding_e9_per_slot: 0,
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
        for (owner, deposit) in [(&lp, 10_000_000_000u64), (&taker, 210_000_000u64)] {
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
        };
        // InitMarket leaves the liquidation reward share at zero: no reward portfolio.
        assert_eq!(
            state::read_market(&e.data(market))?
                .0
                .liquidation_cranker_fee_share_bps,
            0
        );
        e.check();
        let ix = wrapper_ix(
            percolator_prog::id(),
            P::TradeNoCpi {
                account_a_portfolio_id: state::read_portfolio_id(&e.data(e.victim))?,
                account_a_position_epoch: state::read_portfolio_position_epoch(&e.data(e.victim))?,
                account_b_portfolio_id: state::read_portfolio_id(&e.data(e.portfolios[0]))?,
                account_b_position_epoch: state::read_portfolio_position_epoch(
                    &e.data(e.portfolios[0]),
                )?,
                asset_index: 0,
                market_id: state::read_market(&e.data(market))?.1.assets[0].market_id,
                size_q: SIZE,
                exec_price: PRICE,
                fee_bps: 12,
                backing_fee_cap_bps: 0,
            },
            vec![
                M::new(taker.pubkey(), true),
                M::new(lp.pubkey(), true),
                M::new(market, false),
                M::new(e.victim, false),
                M::new(e.portfolios[0], false),
            ],
        );
        send(&mut e.svm, &e.payer, &[ix], &[&lp, &taker]).expect("TradeNoCpi");
        e.check();
        Ok(Some(e))
    }
    fn data(&self, key: Pubkey) -> Vec<u8> {
        self.svm.get_account(&key).unwrap().data
    }
    fn check(&self) {
        check(&self.svm, self.market, &self.portfolios);
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
        let obs =
            fetch_observation_for_asset(&self.data(self.market), 0, &PythFeedResolver::empty())
                .unwrap();
        assert!(obs.oracle_accounts.is_empty());
        let slot = self.svm.get_sysvar::<Clock>().slot;
        let (ixs, idx) = build_crank_instructions(
            &percolator_prog::id(),
            &self.payer.pubkey(),
            &self.market,
            &self.victim,
            &CrankRoute::Live {
                reward_portfolio: None,
            },
            &[obs],
            slot,
            paired,
        );
        let result = self.send_crank(&ixs);
        if let Err(ref failure) = result {
            eprintln!(
                "slot {slot} paired={paired}: {} logs={:?}",
                failure.err, failure.meta.logs
            );
            // LiteSVM failure models preflight rejection: no instruction changes landed.
            if paired {
                if let Some(single) = single_crank_retry_instructions(&ixs, idx, true, &failure.err.to_string()) {
                    let retry = self.send_crank(&single);
                    assert!(retry.is_ok(), "single fallback: {retry:?}");
                    return retry;
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
#[ignore = "known-bad on pinned engine 4db11a8c: crank cert goes stale every slot (upstream percolator#248); re-check (un-ignore) after a re-pin onto percolator#248; the wrapper may still need paired cranks"]
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
