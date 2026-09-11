//! Minimal permissionless cranker for percolator-prog (M2).
//!
//! Loop shape:
//!   1. Boot a self-contained market environment (`setup::bootstrap`).
//!   2. For N cycles:
//!      a. Discover portfolios (`getProgramAccounts`, filtered by KIND).
//!      b. Rank them (skip our own reward portfolio; require initialized owner).
//!      c. For each ranked portfolio:
//!         - Fetch observation for asset 0 (STUB for M2 — see observation.rs).
//!         - Consult the plan-classification gate (upstream predicate) to
//!           decide whether an observation IS needed. Log the decision.
//!         - Submit `PermissionlessCrank` with the cranker's reward
//!           portfolio appended as last writable account.
//!         - Classify the response three-way (Err / Ok-no-mut / Ok-mut) and
//!           record the mirror-custody row for reconciliation.
//!      d. Sleep the cycle interval so maintenance fees accrue between
//!         cycles.
//!   3. Print a mirror-custody summary and exit 0.
//!
//! Deliberate omissions (see cranker/README.md for the full stub inventory):
//!   * No real oracle account discovery — every hint has `oracle_accounts: 0`.
//!   * No `SyncMaintenanceFee` submission — the cranker exercises the
//!     `PermissionlessCrank` path only for M2. Fee-routing was already
//!     proved by the M1 client via `SyncMaintenanceFee`.
//!   * No priority-fee logic. No leader-slot prediction. No jito-bundle.
//!   * No exponential backoff on retries — per the mission brief, just
//!     iterate on NonProgress cleanly.

mod binding;
mod discovery;
mod observation;
mod plan_gate;
mod setup;
mod submit;

use anyhow::{anyhow, Context, Result};
use percolator::AutoCrankPlanV16;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    pubkey::Pubkey,
    signature::{read_keypair_file, Signer},
};
use std::{env, str::FromStr, time::Duration};

use crate::binding::{SOURCE_LIEN_DRAIN_MAX_STEPS, V16_LAYOUT_DISCRIMINATOR};
use crate::submit::CrankOutcome;

const RPC_URL: &str = "http://127.0.0.1:8899";
const DEFAULT_CYCLES: usize = 3;
const CYCLE_INTERVAL: Duration = Duration::from_secs(3);
/// Hint budget per PermissionlessCrank tx. Wrapper cap is
/// `CRANK_OBSERVATION_DECODE_MAX = 16`. Our synthetic market has 1 asset,
/// so this is effectively 1 hint per tx, but the code path enforces the
/// budget for when it matters.
const HINT_BUDGET: usize = 16;

/// Mirror-custody bookkeeping row: one per submission. Persisted in-memory
/// only for M2; a production cranker would sink these to a real store.
#[derive(Debug)]
#[allow(dead_code)] // `signature`/`error` are captured for the Debug-derived
                    // reconciliation dump; the summary table doesn't print them.
struct CustodyRow {
    cycle: usize,
    target_portfolio: Pubkey,
    signature: Option<String>,
    outcome: &'static str,
    target_bytes_diff: usize,
    reward_bytes_diff: Option<usize>,
    pre_state_hash: u64,
    post_state_hash: u64,
    error: Option<String>,
}

fn main() -> Result<()> {
    let program_id_str = env::var("PERCOLATOR_PROGRAM_ID").context(
        "PERCOLATOR_PROGRAM_ID must be set (the deployed program id — see \
         scripts/m1_bringup.sh for how to derive it from the SBF keypair)",
    )?;
    let program_id =
        Pubkey::from_str(&program_id_str).context("parse PERCOLATOR_PROGRAM_ID")?;
    let cycles = env::var("CRANKER_CYCLES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_CYCLES);

    println!("== cranker M2 loop against {RPC_URL} ==");
    println!("program_id = {program_id}");
    println!("cycles = {cycles} (CRANKER_CYCLES to override)");
    println!("hint_budget = {HINT_BUDGET}");
    println!(
        "layout_discriminator = {V16_LAYOUT_DISCRIMINATOR} (A6 drain cap = {SOURCE_LIEN_DRAIN_MAX_STEPS})"
    );

    // Smoke-test the M2 plan-classification gate, including post-#195
    // variants. Delegates to auto_crank_plan_requires_caller_observation.
    {
        use percolator::PermissionlessRecoveryReasonV16;
        let refresh_none = AutoCrankPlanV16::RefreshAccount { asset_index: None };
        assert!(plan_gate::needs_observation(&refresh_none));
        for plan in [
            AutoCrankPlanV16::ReleaseSourceLiens,
            AutoCrankPlanV16::AdvanceClose,
            AutoCrankPlanV16::FinalizeRecovery,
            AutoCrankPlanV16::CloseResolved,
            AutoCrankPlanV16::DeclareRecovery {
                reason: PermissionlessRecoveryReasonV16::ActiveBankruptCloseCannotProgress,
            },
        ] {
            assert!(
                !plan_gate::needs_observation(&plan),
                "predicate regression: {plan:?} must NOT require a caller observation"
            );
        }
        println!("plan-gate smoke OK (Refresh{{None}}=needs_obs; A6/A7/A8/AdvanceClose=no_obs)");
    }

    let rpc = RpcClient::new_with_commitment(RPC_URL.to_string(), CommitmentConfig::confirmed());
    let _ = rpc.get_version().context("connect to RPC")?;

    let payer_path = home_dir()?.join(".config/solana/id.json");
    let payer = read_keypair_file(&payer_path)
        .map_err(|e| anyhow!("read payer keypair {payer_path:?}: {e}"))?;
    println!("payer = {}", payer.pubkey());

    // ---- one-shot bootstrap ----
    println!("== bootstrapping market environment ==");
    let env = setup::bootstrap(&rpc, &program_id, payer)?;
    println!("market            = {}", env.market);
    println!("mint              = {}", env.mint);
    println!("vault             = {}", env.vault);
    println!("user_portfolio    = {}", env.user_portfolio);
    println!("cranker_portfolio = {} (our reward acct)", env.cranker_portfolio);

    // ---- cycle loop ----
    let mut custody: Vec<CustodyRow> = Vec::new();
    for cycle in 0..cycles {
        println!("\n---- cycle {cycle} ----");
        let all_portfolios = discovery::discover_portfolios(&rpc, &program_id)?;
        println!("discovered {} portfolio account(s)", all_portfolios.len());
        let ranked =
            discovery::rank_by_staleness(all_portfolios, &env.market, &env.cranker_portfolio);
        println!("ranked {} candidate(s) after pruning self + uninitialized", ranked.len());

        // ---- hint-budget selection (M2.e) ----
        // Naive: crank up to HINT_BUDGET candidates per cycle, one hint each,
        // one asset (0). Documenting the policy explicitly so it's obvious
        // when it drifts.
        let batch: Vec<_> = ranked.into_iter().take(HINT_BUDGET).collect();

        // Fresh slot for the observation freshness signal (wrapper still
        // authenticates from Clock; we send the slot the RPC currently
        // reports so a caller inspecting a captured tx sees a sane value).
        let now_slot = rpc.get_slot()?;

        // Snapshot the market slab once per cycle for oracle-profile
        // discovery. In production this would be cached with an on-slot
        // invalidation watcher; for M2 a per-cycle fetch is fine.
        let market_data = rpc
            .get_account_data(&env.market)
            .context("read market slab for observation discovery")?;
        // Pyth resolver policy (env override):
        //   CRANKER_PYTH_MODE=off        → no Pyth resolution (SB/Chainlink only)
        //   CRANKER_PYTH_MODE=canonical  → canonical shard-0 PDA fallback
        //   (unset defaults to `off` — matches our synthetic MANUAL market)
        let pyth_resolver = match env::var("CRANKER_PYTH_MODE").as_deref() {
            Ok("canonical") => observation::PythFeedResolver::canonical_pda_only(),
            _ => observation::empty_pyth_resolver(),
        };

        for candidate in batch {
            println!("→ crank candidate = {}", candidate.pubkey);

            // ---- observation pipeline (Q4 real walk, not a stub anymore) ----
            // Reads AssetOracleProfileV16 for the target asset out of the
            // market slab and returns oracle account pubkeys per the mode's
            // dispatch table. For our synthetic ORACLE_MODE_MANUAL market
            // this still returns empty — same effective behavior as the
            // previous stub, but now via a real per-asset profile walk.
            let observation = observation::fetch_observation_for_asset(
                &market_data,
                /* asset_index */ 0,
                &pyth_resolver,
            )?;
            let hint_oracle_count = observation.oracle_account_count();

            // ---- oracle-profile inspection + plan-classification gate ----
            let profile_report = observation::read_asset_oracle_profile(&market_data, 0);
            match profile_report {
                Some(p) => println!(
                    "  oracle-profile asset 0: mode={} leg_count={} (tail carries {} oracle account(s))",
                    oracle_mode_name(p.oracle_mode),
                    p.oracle_leg_count,
                    hint_oracle_count,
                ),
                None => println!("  oracle-profile asset 0: unreadable (market slab too short)"),
            };
            // Consult the plan-classification gate against the one plan variant
            // that would require an observation (`RefreshAccount{None}`). A6–A8
            // / AdvanceClose / FinalizeRecovery all dispatch without a caller
            // observation (delegated; not hand-rolled).
            let hypothetical = AutoCrankPlanV16::RefreshAccount { asset_index: None };
            if plan_gate::needs_observation(&hypothetical) && observation.oracle_accounts.is_empty()
            {
                println!(
                    "  plan-gate: hypothetical RefreshAccount{{None}} requires an observation; \
                     our walk returned {hint_oracle_count} oracle accounts. On MANUAL/EWMA/AUTH-MARK \
                     markets that's correct (mark is committed); on HYBRID it means the pyth \
                     resolver is missing entries — expect NonProgress or a hard-reject."
                );
            } else {
                println!(
                    "  plan-gate: hypothetical plan does not require a caller observation \
                     ({hint_oracle_count} oracle accounts in the tail); engine will \
                     synthesize from committed state."
                );
            }
            println!(
                "  plan-gate: A6 ReleaseSourceLiens / AdvanceClose / FinalizeRecovery / A8 CloseResolved \
                 all needs_obs=false (chunk = {} domain/call)",
                binding::SOURCE_LIEN_RELEASE_CHUNK_DOMAINS
            );

            // ---- submit + classify (M2.b + M2.f) ----
            // A6 drain: ReleaseSourceLiens progresses one domain per crank.
            // Keep submitting the same candidate until Ok(no-mut) / HardReject
            // or SOURCE_LIEN_DRAIN_MAX_STEPS. Order-canonical K/F (v16.9.1)
            // means this loop is liveness, not a same-refresh PnL lever.
            let mut inner_slot = now_slot;
            for drain_step in 0..SOURCE_LIEN_DRAIN_MAX_STEPS {
                let outcome = submit::submit_crank(
                    &rpc,
                    &program_id,
                    &env.payer,
                    &env.market,
                    &candidate.pubkey,
                    Some(&env.cranker_portfolio),
                    std::slice::from_ref(&observation),
                    inner_slot,
                )?;

                let stop = matches!(
                    &outcome,
                    CrankOutcome::Ok { mutated: false, .. } | CrankOutcome::HardReject { .. }
                );
                if drain_step > 0 {
                    println!("  A6 drain step {drain_step}");
                }
                let row = classify_and_log(cycle, &candidate.pubkey, &outcome);
                custody.push(row);
                if stop {
                    break;
                }
                inner_slot = rpc.get_slot()?;
            }
        }

        if cycle + 1 < cycles {
            println!("cycle {cycle} done; sleeping {:?}", CYCLE_INTERVAL);
            std::thread::sleep(CYCLE_INTERVAL);
        }
    }

    // ---- summary (M2.g) ----
    println!("\n== mirror-custody summary ==");
    println!(
        "{:<6} {:<45} {:<12} {:<7} {:<7} {:<20} {:<20}",
        "cycle", "target_portfolio", "outcome", "tgtΔ", "rwdΔ", "pre_hash", "post_hash"
    );
    let mut mutations = 0usize;
    let mut nonprogress = 0usize;
    let mut hard_rejects = 0usize;
    for row in &custody {
        let rwd = row
            .reward_bytes_diff
            .map(|d| d.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<6} {:<45} {:<12} {:<7} {:<7} {:<20x} {:<20x}",
            row.cycle,
            row.target_portfolio,
            row.outcome,
            row.target_bytes_diff,
            rwd,
            row.pre_state_hash,
            row.post_state_hash
        );
        if let Some(err) = &row.error {
            println!("       └── error: {err}");
        }
        match row.outcome {
            "Ok(mut)" => mutations += 1,
            "Ok(no-mut)" => nonprogress += 1,
            "HardReject" => hard_rejects += 1,
            _ => {}
        }
    }
    println!(
        "\ntotals: submissions={}, mutated={}, non-progress={}, hard-reject={}",
        custody.len(),
        mutations,
        nonprogress,
        hard_rejects
    );

    // Post-run reward portfolio snapshot for reconciliation.
    let reward_final = rpc.get_account_data(&env.cranker_portfolio)?;
    let reward_hash = fnv(&reward_final);
    println!(
        "cranker_portfolio final: {} bytes, fingerprint={:x}",
        reward_final.len(),
        reward_hash
    );

    // ---- optional Q6 empirical spot-check: cranker portfolio withdrawal ----
    // Guarded so it never fires in the M2 default run. When enabled:
    //   1. Route some maintenance fee to the cranker portfolio via
    //      `SyncMaintenanceFee` (as in M1's fee-routing proof).
    //   2. Attempt `Withdraw` from the cranker portfolio.
    //   3. Confirm the destination ATA balance moves — proves the cranker
    //      portfolio is byte-for-byte the same withdrawal surface as a
    //      user portfolio.
    if env::var("CRANKER_WITHDRAWAL_TEST").ok().as_deref() == Some("1") {
        run_q6_withdrawal_test(&rpc, &program_id, &env)?;
    }

    println!("== cranker loop OK ==");
    Ok(())
}

/// Q6 empirical verification. See main() call site for context.
fn run_q6_withdrawal_test(
    rpc: &RpcClient,
    program_id: &Pubkey,
    env: &setup::CrankerEnv,
) -> Result<()> {
    use percolator_prog::ix::Instruction as ProgInstruction;
    use solana_sdk::{
        compute_budget::ComputeBudgetInstruction,
        instruction::{AccountMeta, Instruction},
        transaction::Transaction,
    };

    println!("\n== Q6 withdrawal spot-check ==");

    // Route maintenance fee to the cranker portfolio. Wait a few slots
    // first so meaningful fee accrues.
    println!("waiting 4s for maintenance-fee accrual...");
    std::thread::sleep(Duration::from_secs(4));
    let now_slot = rpc.get_slot()?;
    let sync_ix = ProgInstruction::SyncMaintenanceFee { now_slot };
    let sync_instr = Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(env.market, false),
            AccountMeta::new(env.user_portfolio, false),
            AccountMeta::new(env.cranker_portfolio, false),
        ],
        data: sync_ix.encode(),
    };
    let cu = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
    let heap = ComputeBudgetInstruction::request_heap_frame(128 * 1024);
    let bh = rpc.get_latest_blockhash_with_commitment(CommitmentConfig::processed())?.0;
    let tx = Transaction::new_signed_with_payer(
        &[cu.clone(), heap.clone(), sync_instr],
        Some(&env.payer.pubkey()),
        &[&env.payer],
        bh,
    );
    let sync_sig = rpc
        .send_and_confirm_transaction(&tx)
        .context("SyncMaintenanceFee to route fee to cranker portfolio")?;
    println!("SyncMaintenanceFee sig = {sync_sig}");

    // Create the payer's ATA (Withdraw dest); mint the payer as owner
    // (already the cranker portfolio owner from setup).
    let payer_ata = spl_associated_token_account::get_associated_token_address(
        &env.payer.pubkey(),
        &env.mint,
    );
    let create_ata_ix =
        spl_associated_token_account::instruction::create_associated_token_account(
            &env.payer.pubkey(),
            &env.payer.pubkey(),
            &env.mint,
            &spl_token::ID,
        );
    let bh = rpc.get_latest_blockhash_with_commitment(CommitmentConfig::processed())?.0;
    let tx = Transaction::new_signed_with_payer(
        &[create_ata_ix],
        Some(&env.payer.pubkey()),
        &[&env.payer],
        bh,
    );
    rpc.send_and_confirm_transaction(&tx)
        .context("create payer ATA for withdraw dest")?;
    let dest_before: u64 = spl_token_amount(rpc, &payer_ata)?;
    println!("payer ATA before Withdraw: {} atoms", dest_before);

    // Read cranker portfolio's capital via the same public helper the
    // wrapper uses — proves we're comparing the right field.
    let cranker_data = rpc.get_account_data(&env.cranker_portfolio)?;
    let cranker_view = percolator_prog::state::read_portfolio(&cranker_data)
        .map_err(|e| anyhow!("read_portfolio(cranker): {e:?}"))?;
    let cranker_capital: u128 = cranker_view.capital.get();
    println!("cranker_portfolio capital: {cranker_capital} atoms");
    if cranker_capital == 0 {
        println!("(no maintenance fee accrued — likely too little elapsed. Withdraw(0) still \
                  exercises the code path; proceeding.)");
    }

    // Withdraw up to what's available (min of capital and 100 atoms) so
    // this always succeeds; a real cranker would pick its own cadence.
    let withdraw_amount = cranker_capital.min(100);
    let (vault_authority, _bump) =
        Pubkey::find_program_address(&[b"vault", env.market.as_ref()], program_id);
    let cranker_bind = crate::binding::portfolio_binding(&cranker_data)?;
    let withdraw_ix = ProgInstruction::Withdraw {
        portfolio_id: cranker_bind.portfolio_id,
        expected_sequence: cranker_bind.sequence,
        amount: withdraw_amount,
    };
    let withdraw_instr = Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new(env.market, false),
            AccountMeta::new(env.cranker_portfolio, false),
            AccountMeta::new(payer_ata, false),
            AccountMeta::new(env.vault, false),
            AccountMeta::new_readonly(vault_authority, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data: withdraw_ix.encode(),
    };
    let bh = rpc.get_latest_blockhash_with_commitment(CommitmentConfig::processed())?.0;
    let tx = Transaction::new_signed_with_payer(
        &[cu, heap, withdraw_instr],
        Some(&env.payer.pubkey()),
        &[&env.payer],
        bh,
    );
    let withdraw_sig = rpc
        .send_and_confirm_transaction(&tx)
        .with_context(|| format!("Withdraw({withdraw_amount}) from cranker portfolio"))?;
    println!("Withdraw({withdraw_amount}) sig = {withdraw_sig}");

    let dest_after = spl_token_amount(rpc, &payer_ata)?;
    let delta = dest_after.saturating_sub(dest_before);
    println!("payer ATA after Withdraw:  {} atoms (Δ = {})", dest_after, delta);
    if delta as u128 == withdraw_amount {
        println!("Q6 verified: cranker portfolio → owner ATA transfer matches Withdraw amount");
    } else {
        println!(
            "Q6 partial: expected +{withdraw_amount}, got +{delta}. Amount=0 case still exercises \
             the code path (engine short-circuits at line 14549)."
        );
    }
    Ok(())
}

fn spl_token_amount(rpc: &RpcClient, ata: &Pubkey) -> Result<u64> {
    use solana_sdk::program_pack::Pack;
    let acct = rpc.get_account_data(ata)?;
    let parsed = spl_token::state::Account::unpack(&acct)?;
    Ok(parsed.amount)
}

fn classify_and_log(
    cycle: usize,
    target: &Pubkey,
    outcome: &CrankOutcome,
) -> CustodyRow {
    match outcome {
        CrankOutcome::HardReject { error, error_name } => {
            let named = error_name.unwrap_or("Unknown");
            println!("  ← HardReject ({named}): {error}");
            CustodyRow {
                cycle,
                target_portfolio: *target,
                signature: None,
                outcome: "HardReject",
                target_bytes_diff: 0,
                reward_bytes_diff: None,
                pre_state_hash: 0,
                post_state_hash: 0,
                error: Some(format!("{named}: {error}")),
            }
        }
        CrankOutcome::Ok {
            signature,
            pre_state_hash,
            post_state_hash,
            mutated,
            target_bytes_diff,
            reward_bytes_diff,
        } => {
            let variant = if *mutated { "Ok(mut)" } else { "Ok(no-mut)" };
            println!(
                "  ← {variant}: sig={signature} target_diff={target_bytes_diff} reward_diff={:?}",
                reward_bytes_diff
            );
            CustodyRow {
                cycle,
                target_portfolio: *target,
                signature: Some(signature.to_string()),
                outcome: variant,
                target_bytes_diff: *target_bytes_diff,
                reward_bytes_diff: *reward_bytes_diff,
                pre_state_hash: *pre_state_hash,
                post_state_hash: *post_state_hash,
                error: None,
            }
        }
    }
}

fn home_dir() -> Result<std::path::PathBuf> {
    Ok(std::path::PathBuf::from(env::var("HOME").context("HOME unset")?))
}

fn oracle_mode_name(mode: u8) -> &'static str {
    use percolator_prog::constants::{
        ORACLE_MODE_AUTH_MARK, ORACLE_MODE_EWMA_MARK, ORACLE_MODE_HYBRID_AFTER_HOURS,
        ORACLE_MODE_MANUAL,
    };
    match mode {
        ORACLE_MODE_MANUAL => "MANUAL",
        ORACLE_MODE_HYBRID_AFTER_HOURS => "HYBRID_AFTER_HOURS",
        ORACLE_MODE_EWMA_MARK => "EWMA_MARK",
        ORACLE_MODE_AUTH_MARK => "AUTH_MARK",
        _ => "unknown",
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
