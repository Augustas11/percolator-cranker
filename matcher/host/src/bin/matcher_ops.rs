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
    fn flag(&self, name: &str) -> bool {
        self.0.get(name).is_some_and(|v| v == "true")
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
        bail!("usage: matcher_ops <init-ctx|set-matcher-config|update-ref|set-params|show|test-fill> --rpc URL --matcher PUBKEY --ctx PUBKEY [options]; each option also accepts MATCHER_UPPER_CASE env. See matcher/README.md");
    };
    let mut values = BTreeMap::new();
    while let Some(arg) = raw.next() {
        let name = arg
            .strip_prefix("--")
            .ok_or_else(|| anyhow!("expected --option, got {arg}"))?;
        let value = if name == "push-auth-mark" {
            "true".to_string()
        } else {
            raw.next()
                .ok_or_else(|| anyhow!("missing value for {arg}"))?
        };
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
        "update-ref" => {
            let authority = args.key("price-authority")?;
            let market = args.value("market")?;
            let data = rpc.get_account_data(&market)?;
            let price = state::read_market(&data)?.1.assets[0].effective_price;
            let mut ixs = Vec::new();
            if args.flag("push-auth-mark") {
                ixs.push(push_mark(
                    args.value("wrapper")?,
                    market,
                    authority.pubkey(),
                    &data,
                    rpc.get_slot()?,
                    price,
                )?);
            }
            ixs.push(update_ref(
                program,
                authority.pubkey(),
                args.value("ctx")?,
                price,
            ));
            send(&rpc, &payer, &ixs, &[&authority])?;
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
    let taker = Keypair::new();
    let pf = Keypair::new();
    let token = Keypair::new();
    let market_data = rpc.get_account_data(&market)?;
    let (cfg, group) = state::read_market(&market_data)?;
    let mint = Pubkey::new_from_array(cfg.collateral_mint);
    let mint_account = rpc.get_account(&mint)?;
    if mint_account.owner != spl_token::ID {
        bail!("test-fill requires an SPL Token test mint");
    }
    let mint_state = spl_token::state::Mint::unpack(&mint_account.data)?;
    if mint_state.mint_authority != solana_sdk::program_option::COption::Some(payer.pubkey()) {
        bail!("payer must be the test mint authority");
    }
    let amount: u64 = args.value("deposit")?;
    if amount == 0 {
        bail!("deposit must be positive");
    }
    let len =
        state::portfolio_account_len_for_market_slots(group.config.max_portfolio_assets as usize)?;
    let setup = vec![
        system_instruction::transfer(&payer.pubkey(), &taker.pubkey(), 1_000_000),
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
                M::new(taker.pubkey(), true),
                M::new(market, false),
                M::new(pf.pubkey(), false),
            ],
        ),
        system_instruction::create_account(
            &payer.pubkey(),
            &token.pubkey(),
            rpc.get_minimum_balance_for_rent_exemption(TokenAccount::LEN)?,
            TokenAccount::LEN as u64,
            &spl_token::ID,
        ),
        spl_token::instruction::initialize_account3(
            &spl_token::ID,
            &token.pubkey(),
            &mint,
            &taker.pubkey(),
        )?,
        spl_token::instruction::mint_to(
            &spl_token::ID,
            &mint,
            &token.pubkey(),
            &payer.pubkey(),
            &[],
            amount,
        )?,
    ];
    send(rpc, payer, &setup, &[&taker, &pf, &token])?;
    println!(
        "throwaway_taker={} portfolio={} token={}",
        taker.pubkey(),
        pf.pubkey(),
        token.pubkey()
    );
    let pf_data = rpc.get_account_data(&pf.pubkey())?;
    let vault_authority = Pubkey::find_program_address(&[b"vault", market.as_ref()], &wrapper).0;
    let ata: Pubkey = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL".parse()?;
    let vault = Pubkey::find_program_address(
        &[
            vault_authority.as_ref(),
            spl_token::ID.as_ref(),
            mint.as_ref(),
        ],
        &ata,
    )
    .0;
    let deposit = wrapper_ix(
        wrapper,
        P::Deposit {
            portfolio_id: state::read_portfolio_id(&pf_data)?,
            expected_sequence: state::read_portfolio_matcher_sequence(&pf_data)?,
            amount: amount as u128,
        },
        vec![
            M::new(taker.pubkey(), true),
            M::new(market, false),
            M::new(pf.pubkey(), false),
            M::new(token.pubkey(), false),
            M::new(vault, false),
            M::new_readonly(spl_token::ID, false),
        ],
    );
    send(rpc, payer, &[deposit], &[&taker])?;
    // Keep only cranks that simulate successfully, as in staging's fresh_cranks.
    let mut ixs = Vec::new();
    let now = rpc.get_slot()?;
    for portfolio in [lp_pf, pf.pubkey()] {
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
    // Public devnet takes longer than max_ref_age_slots to reach "confirmed",
    // so the refresh and the trade read and land at "processed".
    let fast = RpcClient::new_with_commitment(rpc.url(), CommitmentConfig::processed());
    // Rehearsal while the pusher is paused: refresh the mark and the matcher
    // reference just before the trade so it lands inside max_ref_age_slots.
    if args.0.contains_key("price-authority") {
        let authority = args.key("price-authority")?;
        let data = fast.get_account_data(&market)?;
        let price = state::read_market(&data)?.1.assets[0].effective_price;
        let push = push_mark(wrapper, market, authority.pubkey(), &data, fast.get_slot()?, price)?;
        let refresh = update_ref(program, authority.pubkey(), ctx, price);
        let tx = Transaction::new_signed_with_payer(
            &[push, refresh],
            Some(&payer.pubkey()),
            &[payer, &authority],
            fast.get_latest_blockhash()?,
        );
        let sig = fast.send_transaction(&tx)?;
        // Poll "processed" ourselves: send_and_confirm waits for more than the age.
        loop {
            if let Some(status) = fast.get_signature_status(&sig)? {
                status?;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        println!("{sig}");
    }
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
