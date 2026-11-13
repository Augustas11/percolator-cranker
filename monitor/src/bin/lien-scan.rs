//! Read-only: list every market account a wrapper program owns and print
//! the per-domain source-credit liens.
//!
//! Run it before an engine upgrade that changes the credit-rate formula
//! (upstream percolator PR 260 / #250). The new formula subtracts valid liens
//! (backing + insurance) from the claim bound; with no valid liens it equals
//! the old one, so only domains with valid liens would fail validation until
//! their stored rate is recomputed. Impaired liens are printed but do not
//! change the rate.
//!
//! Usage: lien-scan [RPC_URL] [PROGRAM_ID]
//! Defaults: public devnet RPC and our devnet program.
//!
//! Exit codes:
//!   0  every market decoded, none holds a valid lien
//!   1  at least one market holds a valid lien
//!   2  could not check everything (RPC error, or a market this build's
//!      layout cannot decode, which may still be live on-chain)
//!
//! Decodes with the host reader `read_market_boxed`, which covers the
//! configured asset slots (`max_market_slots`).
use anyhow::{Context, Result};
use percolator_prog::constants::{KIND_MARKET, MAGIC, VERSION};
use percolator_prog::state::read_market_boxed;
use solana_account_decoder::UiAccountEncoding;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};
use std::process::ExitCode;
use std::str::FromStr;

const DEFAULT_RPC: &str = "https://api.devnet.solana.com";
const DEFAULT_PROGRAM: &str = "GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("lien-scan: could not check: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    let mut args = std::env::args().skip(1);
    let rpc_url = args.next().unwrap_or_else(|| DEFAULT_RPC.to_string());
    let program = Pubkey::from_str(&args.next().unwrap_or_else(|| DEFAULT_PROGRAM.to_string()))
        .context("program id")?;
    let rpc = RpcClient::new(rpc_url);

    let mut prefix = Vec::with_capacity(11);
    prefix.extend_from_slice(&MAGIC.to_le_bytes());
    prefix.extend_from_slice(&VERSION.to_le_bytes());
    prefix.push(KIND_MARKET);
    let config = RpcProgramAccountsConfig {
        filters: Some(vec![RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
            0, prefix,
        ))]),
        account_config: RpcAccountInfoConfig {
            commitment: Some(CommitmentConfig::confirmed()),
            encoding: Some(UiAccountEncoding::Base64),
            ..RpcAccountInfoConfig::default()
        },
        ..RpcProgramAccountsConfig::default()
    };
    let accounts = rpc
        .get_program_accounts_with_config(&program, config)
        .context("getProgramAccounts KIND_MARKET")?;

    println!("program {program}: {} market accounts", accounts.len());
    let mut valid_lien_markets = 0;
    let mut undecoded = 0;
    for (pubkey, account) in accounts {
        let (_, group) = match read_market_boxed(&account.data) {
            Ok(m) => m,
            Err(e) => {
                undecoded += 1;
                println!(
                    "{pubkey}: NOT CHECKED, this build's layout cannot decode it ({e:?}, {} bytes)",
                    account.data.len()
                );
                continue;
            }
        };
        let mut valid = 0;
        let mut impaired_only = 0;
        for (domain, s) in group.source_credit.iter().enumerate() {
            let has_valid = s.valid_liened_backing_num != 0 || s.valid_liened_insurance_num != 0;
            let has_impaired =
                s.impaired_liened_backing_num != 0 || s.impaired_liened_insurance_num != 0;
            if !has_valid && !has_impaired {
                continue;
            }
            if has_valid {
                valid += 1;
            } else {
                impaired_only += 1;
            }
            println!(
                "  {pubkey} domain {domain}: valid_backing {} valid_insurance {} impaired_backing {} impaired_insurance {} credit_rate_num {}",
                s.valid_liened_backing_num,
                s.valid_liened_insurance_num,
                s.impaired_liened_backing_num,
                s.impaired_liened_insurance_num,
                s.credit_rate_num,
            );
        }
        if valid > 0 {
            valid_lien_markets += 1;
        }
        println!(
            "{pubkey}: mode {:?}, {} domains, {valid} with valid liens, {impaired_only} impaired only",
            group.mode,
            group.source_credit.len()
        );
    }

    if valid_lien_markets > 0 {
        println!("{valid_lien_markets} market(s) hold valid liens; {undecoded} not checked");
        return Ok(ExitCode::from(1));
    }
    if undecoded > 0 {
        println!("no valid liens in decoded markets; {undecoded} market(s) not checked");
        return Ok(ExitCode::from(2));
    }
    println!("no valid liens");
    Ok(ExitCode::SUCCESS)
}
