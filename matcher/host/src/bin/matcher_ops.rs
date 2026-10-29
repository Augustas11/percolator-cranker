//! Devnet-only operator utility. Run without arguments for usage.
use anyhow::{anyhow, bail, Context, Result};
use drip_matcher_host::*;
use percolator_prog::{
    ix::{CrankObservationHint, Instruction as P},
    state,
};
use solana_client::rpc_client::RpcClient;
use solana_sdk::program_pack::Pack;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction as CU,
    instruction::{AccountMeta as M, Instruction},
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use spl_token::state::Account as TokenAccount;
use std::{collections::BTreeMap, str::FromStr};

struct Args(BTreeMap<String, String>);
impl Args {
    fn get(&self, name: &str) -> Result<String> {
        self.0
            .get(name)
            .cloned()
            .or_else(|| {
                std::env::var(format!("MATCHER_{}", name.replace('-', "_").to_uppercase())).ok()
            })
            .ok_or_else(|| {
                anyhow!(
                    "missing --{name} (or MATCHER_{})",
                    name.replace('-', "_").to_uppercase()
                )
            })
    }
    fn value<T: FromStr>(&self, name: &str) -> Result<T>
    where
        T::Err: std::fmt::Display,
    {
        self.get(name)?
            .parse()
            .map_err(|e| anyhow!("--{name}: {e}"))
    }
    fn key(&self, name: &str) -> Result<Keypair> {
        read_keypair_file(self.get(name)?).map_err(|_| anyhow!("cannot read --{name} keypair"))
    }
}
fn send(rpc: &RpcClient, payer: &Keypair, ixs: &[Instruction], signers: &[&Keypair]) -> Result<()> {
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
        rpc.get_latest_blockhash()?,
    );
    let sim = rpc.simulate_transaction(&tx)?.value;
    if let Some(err) = sim.err {
        for line in sim.logs.unwrap_or_default() {
            eprintln!("  {line}");
        }
        bail!("simulation failed: {err}");
    }
    println!("{}", rpc.send_and_confirm_transaction(&tx)?);
    Ok(())
}
fn main() -> Result<()> {
    let mut raw = std::env::args().skip(1);
    let Some(command) = raw.next() else {
        bail!("usage: matcher_ops <init-ctx|init-lp|set-matcher-config|set-params|show|test-fill> --rpc URL --matcher PUBKEY --ctx PUBKEY [options]; each option also accepts MATCHER_UPPER_CASE env. See matcher/README.md");
    };
    let mut values = BTreeMap::new();
    while let Some(arg) = raw.next() {
        let name = arg
            .strip_prefix("--")
            .ok_or_else(|| anyhow!("expected --option, got {arg}"))?;
        let value = raw
            .next()
            .ok_or_else(|| anyhow!("missing value for {arg}"))?;
        if values.insert(name.to_string(), value).is_some() {
            bail!("duplicate option {arg}");
        }
    }
    let args = Args(values);
    let url = args
        .get("rpc")
        .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
    if url.to_lowercase().contains("mainnet") {
        bail!("mainnet RPC URLs are forbidden");
    }
    let rpc = RpcClient::new_with_commitment(url, CommitmentConfig::confirmed());
    // Devnet only: the URL check above is advisory; the genesis hash is not.
    if rpc.get_genesis_hash()?.to_string() != "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG" {
        bail!("not devnet (genesis hash mismatch)");
    }
    let program: Pubkey = args.value("matcher")?;
    if command == "show" {
        let ctx: Pubkey = args.value("ctx")?;
        let account = rpc.get_account(&ctx)?;
        if account.owner != program || account.data.len() < CTX_LEN {
            bail!("invalid matcher context");
        }
        println!(
            "ctx={ctx} owner={} bytes={}",
            account.owner,
            account.data.len()
        );
        show_context(&account.data)?;
        return Ok(());
    }
    if command == "check" {
        return preflight(&rpc, &args, program);
    }
    let payer = args.key("payer")?;
    match command.as_str() {
        "init-ctx" => {
            let ctx = args.key("ctx-keypair")?;
            let owner = args.key("lp-owner")?;
            let wrapper: Pubkey = args.value("wrapper")?;
            let create = system_instruction::create_account(
                &payer.pubkey(),
                &ctx.pubkey(),
                rpc.get_minimum_balance_for_rent_exemption(CTX_LEN)?,
                CTX_LEN as u64,
                &program,
            );
            let init = Instruction {
                program_id: program,
                accounts: vec![
                    M::new_readonly(owner.pubkey(), true),
                    M::new(ctx.pubkey(), true),
                    M::new_readonly(args.value("market")?, false),
                    M::new_readonly(args.value("portfolio")?, false),
                    M::new_readonly(wrapper, false),
                ],
                data: init_data(
                    args.value("asset")?,
                    args.value("fill")?,
                    args.value("cap")?,
                    args.value("age")?,
                    args.value("deviation")?,
                    args.value("authority")?,
                ),
            };
            send(&rpc, &payer, &[create, init], &[&ctx, &owner])?;
            println!("ctx={}", ctx.pubkey());
        }
        "set-matcher-config" => {
            let owner = args.key("lp-owner")?;
            let market = args.value("market")?;
            let portfolio = args.value("portfolio")?;
            let expiry = rpc
                .get_slot()?
                .checked_add(args.value("expiry-slots")?)
                .context("expiry overflow")?;
            let ix = set_matcher_config(
                args.value("wrapper")?,
                market,
                portfolio,
                owner.pubkey(),
                program,
                args.value("ctx")?,
                &rpc.get_account_data(&portfolio)?,
                &rpc.get_account_data(&market)?,
                expiry,
            )?;
            send(&rpc, &payer, &[ix], &[&owner])?;
        }
        "set-params" => {
            let owner = args.key("lp-owner")?;
            let ix = Instruction {
                program_id: program,
                accounts: vec![
                    M::new_readonly(owner.pubkey(), true),
                    M::new(args.value("ctx")?, false),
                ],
                data: set_params_data(
                    args.value("fill")?,
                    args.value("cap")?,
                    args.value("age")?,
                    args.value("deviation")?,
                    args.value("authority")?,
                    args.value("paused")?,
                    args.value("inventory")?,
                ),
            };
            send(&rpc, &payer, &[ix], &[&owner])?;
        }
        "init-lp" => {
            // The matcher's own LP portfolio: one LP portfolio per fill path, because a
            // non-matcher fill on the LP clears its matcher capability (README).
            let pf = args.key("portfolio-keypair")?;
            funded_portfolio(
                &rpc,
                &payer,
                args.value("wrapper")?,
                args.value("market")?,
                &args.key("lp-owner")?,
                &pf,
                args.value("deposit")?,
            )?;
            println!("lp_portfolio={}", pf.pubkey());
        }
        "test-fill" => test_fill(&rpc, &payer, &args, program)?,
        _ => bail!("unknown command {command}"),
    }
    Ok(())
}

fn test_fill(rpc: &RpcClient, payer: &Keypair, args: &Args, program: Pubkey) -> Result<()> {
    let wrapper: Pubkey = args.value("wrapper")?;
    let market: Pubkey = args.value("market")?;
    let lp_pf: Pubkey = args.value("portfolio")?;
    let ctx: Pubkey = args.value("ctx")?;
    // Devnet SOL rule: send nothing unless every precondition holds (read-only).
    preflight(rpc, args, program)?;
    // Saved taker keypairs: the portfolio (~0.05 SOL rent) is created and funded once,
    // and retries cost only fees.
    let taker = args.key("taker-keypair")?;
    let pf = args.key("taker-portfolio-keypair")?;
    if rpc.get_balance(&taker.pubkey())? < 5_000_000 {
        send(
            rpc,
            payer,
            &[system_instruction::transfer(
                &payer.pubkey(),
                &taker.pubkey(),
                10_000_000,
            )],
            &[],
        )?;
    }
    funded_portfolio(
        rpc,
        payer,
        wrapper,
        market,
        &taker,
        &pf,
        args.value("deposit")?,
    )?;
    println!("taker={} portfolio={}", taker.pubkey(), pf.pubkey());
    // Keep only cranks that simulate successfully, as in staging's fresh_cranks.
    let mut ixs = Vec::new();
    let now = rpc.get_slot()?;
    // --crank PF1,PF2: other position holders; the trade's engine gate needs the whole
    // market's accrual current in this slot, not only the two trading portfolios.
    let mut crank_pfs = vec![lp_pf, pf.pubkey()];
    if let Ok(list) = args.get("crank") {
        for key in list.split(',').filter(|k| !k.is_empty()) {
            crank_pfs.push(key.parse()?);
        }
    }
    for portfolio in crank_pfs {
        let crank = wrapper_ix(
            wrapper,
            P::PermissionlessCrank {
                now_slot: now,
                observations: vec![CrankObservationHint {
                    asset_index: 0,
                    oracle_accounts: 0,
                }],
            },
            vec![
                M::new(taker.pubkey(), true),
                M::new(market, false),
                M::new(portfolio, false),
            ],
        );
        let mut candidate = vec![
            CU::set_compute_unit_limit(1_400_000),
            CU::request_heap_frame(128 * 1024),
        ];
        candidate.extend(ixs.clone());
        candidate.push(crank.clone());
        let tx = Transaction::new_signed_with_payer(
            &candidate,
            Some(&taker.pubkey()),
            &[&taker],
            rpc.get_latest_blockhash()?,
        );
        if rpc.simulate_transaction(&tx)?.value.err.is_none() {
            ixs.push(crank);
        }
    }
    // The pusher refreshes the matcher reference; read and land at "processed" so the
    // trade sees it within max_ref_age_slots (public devnet confirms slowly).
    let fast = RpcClient::new_with_commitment(rpc.url(), CommitmentConfig::processed());
    ixs.push(trade(
        wrapper,
        market,
        taker.pubkey(),
        pf.pubkey(),
        lp_pf,
        program,
        ctx,
        &fast.get_account_data(&market)?,
        &fast.get_account_data(&pf.pubkey())?,
        &fast.get_account_data(&lp_pf)?,
        args.value("size")?,
    )?);
    // Final trade transaction has only the taker as signer and fee payer.
    send(&fast, &taker, &ixs, &[])
}

/// Idempotent setup of `owner`'s portfolio `pf` on `market`: create it only if the account
/// is absent, then deposit `amount` while the owner's faucet-funded token account still
/// holds it, so a re-run completes a setup that stopped between the two transactions.
fn funded_portfolio(
    rpc: &RpcClient,
    payer: &Keypair,
    wrapper: Pubkey,
    market: Pubkey,
    owner: &Keypair,
    pf: &Keypair,
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        bail!("deposit must be positive");
    }
    let (cfg, group) = state::read_market(&rpc.get_account_data(&market)?)?;
    let mint = Pubkey::new_from_array(cfg.collateral_mint);
    let ata_program: Pubkey = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL".parse()?;
    let ata = |authority: &Pubkey| {
        Pubkey::find_program_address(
            &[authority.as_ref(), spl_token::ID.as_ref(), mint.as_ref()],
            &ata_program,
        )
        .0
    };
    let source = ata(&owner.pubkey());
    let held = match rpc
        .get_account_with_commitment(&source, rpc.commitment())?
        .value
    {
        Some(account) => TokenAccount::unpack(&account.data)?.amount,
        None => 0,
    };
    let absent = rpc
        .get_account_with_commitment(&pf.pubkey(), rpc.commitment())?
        .value
        .is_none();
    if absent {
        if held < amount {
            bail!("{source} holds {held} test-USDC atoms, need {amount}: run the faucet first");
        }
        let len = state::portfolio_account_len_for_market_slots(
            group.config.max_portfolio_assets as usize,
        )?;
        let create = [
            system_instruction::create_account(
                &payer.pubkey(),
                &pf.pubkey(),
                rpc.get_minimum_balance_for_rent_exemption(len)?,
                len as u64,
                &wrapper,
            ),
            wrapper_ix(
                wrapper,
                P::InitPortfolio,
                vec![
                    M::new(owner.pubkey(), true),
                    M::new(market, false),
                    M::new(pf.pubkey(), false),
                ],
            ),
        ];
        send(rpc, payer, &create, &[owner, pf])?;
    }
    if held < amount {
        return Ok(()); // already deposited on an earlier run
    }
    let pf_data = rpc.get_account_data(&pf.pubkey())?;
    let vault_authority = Pubkey::find_program_address(&[b"vault", market.as_ref()], &wrapper).0;
    let deposit = wrapper_ix(
        wrapper,
        P::Deposit {
            portfolio_id: state::read_portfolio_id(&pf_data)?,
            expected_sequence: state::read_portfolio_matcher_sequence(&pf_data)?,
            amount: amount as u128,
        },
        vec![
            M::new(owner.pubkey(), true),
            M::new(market, false),
            M::new(pf.pubkey(), false),
            M::new(source, false),
            M::new(ata(&vault_authority), false),
            M::new_readonly(spl_token::ID, false),
        ],
    );
    send(rpc, payer, &[deposit], &[owner])
}

/// Read-only devnet fill preconditions; prints each and fails if any does not hold.
fn preflight(rpc: &RpcClient, args: &Args, program: Pubkey) -> Result<()> {
    let market: Pubkey = args.value("market")?;
    let lp_pf: Pubkey = args.value("portfolio")?;
    let ctx: Pubkey = args.value("ctx")?;
    let now = rpc.get_slot()?;
    let (_, g) = state::read_market(&rpc.get_account_data(&market)?)?;
    let asset = &g.assets[0];
    let c = rpc.get_account_data(&ctx)?;
    let ref_price = u64::from_le_bytes(c[REF_PRICE_OFF..REF_PRICE_OFF + 8].try_into()?);
    let ref_slot = u64::from_le_bytes(c[REF_SLOT_OFF..REF_SLOT_OFF + 8].try_into()?);
    let age = u64::from_le_bytes(c[AGE_OFF..AGE_OFF + 8].try_into()?);
    let dev = u16::from_le_bytes(c[DEVIATION_OFF..DEVIATION_OFF + 2].try_into()?) as u128;
    let lp = rpc.get_account_data(&lp_pf)?;
    let cfg = state::read_portfolio_matcher_config(&lp)?;
    let expiry = state::read_portfolio_matcher_expiry(&lp)?;
    let eff = asset.effective_price;
    println!(
        "slot={now} market.current_slot={} market.slot_last={} asset.slot_last={} ref_slot={ref_slot}",
        g.current_slot, g.slot_last, asset.slot_last
    );
    // The trade transaction's own cranks catch up the mark and accrual, and test-fill
    // simulates it before sending, so these two are shown but do not gate.
    for (name, ok) in [
        ("target == effective", asset.raw_oracle_target_price == eff),
        ("not loss-stale", !g.loss_stale_active),
    ] {
        println!("{} {name}", if ok { "info" } else { "info-NOT" });
    }
    let checks = [
        ("market Live", g.mode == percolator::MarketModeV16::Live),
        (
            "accrual within 32 slots",
            now.saturating_sub(g.slot_last) <= 32,
        ),
        (
            "matcher ref fresh",
            ref_slot != 0 && now.saturating_sub(ref_slot) <= age,
        ),
        (
            "matcher ref near mark",
            (eff.abs_diff(ref_price) as u128) * 10_000 <= dev * ref_price as u128,
        ),
        (
            "LP capability set for this matcher",
            cfg.enabled() == 1
                && cfg.matcher_program == program.to_bytes()
                && cfg.matcher_context == ctx.to_bytes()
                && expiry > now,
        ),
    ];
    for (name, ok) in &checks {
        println!("{} {name}", if *ok { "PASS" } else { "FAIL" });
    }
    if checks.iter().any(|(_, ok)| !ok) {
        bail!("preconditions not met; nothing sent");
    }
    Ok(())
}

fn show_context(data: &[u8]) -> Result<()> {
    if &data[MAGIC_OFF..MAGIC_OFF + 8] != b"DRIPMTC1"
        || u32::from_le_bytes(data[VERSION_OFF..VERSION_OFF + 4].try_into()?) != 1
    {
        bail!("uninitialized or unsupported matcher context");
    }
    for (name, off) in [
        ("wrapper", WRAPPER_OFF),
        ("market", MARKET_OFF),
        ("portfolio", LP_PORTFOLIO_OFF),
        ("lp_owner", LP_OWNER_OFF),
        ("delegate", DELEGATE_OFF),
        ("price_authority", PRICE_AUTHORITY_OFF),
    ] {
        println!(
            "{name}={}",
            Pubkey::new_from_array(data[off..off + 32].try_into()?)
        );
    }
    println!(
        "asset={} paused={}",
        u16::from_le_bytes(data[ASSET_OFF..ASSET_OFF + 2].try_into()?),
        data[PAUSED_OFF]
    );
    println!(
        "max_fill_abs={} max_inventory_abs={} inventory={}",
        u128::from_le_bytes(data[FILL_OFF..FILL_OFF + 16].try_into()?),
        u128::from_le_bytes(data[CAP_OFF..CAP_OFF + 16].try_into()?),
        i128::from_le_bytes(data[INVENTORY_OFF..INVENTORY_OFF + 16].try_into()?)
    );
    println!(
        "max_ref_age_slots={} max_mark_dev_bps={} ref_price_e6={} ref_slot={}",
        u64::from_le_bytes(data[AGE_OFF..AGE_OFF + 8].try_into()?),
        u16::from_le_bytes(data[DEVIATION_OFF..DEVIATION_OFF + 2].try_into()?),
        u64::from_le_bytes(data[REF_PRICE_OFF..REF_PRICE_OFF + 8].try_into()?),
        u64::from_le_bytes(data[REF_SLOT_OFF..REF_SLOT_OFF + 8].try_into()?)
    );
    Ok(())
}
