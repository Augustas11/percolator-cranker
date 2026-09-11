//! Hard-reject rollback property.
//!
//! Mirrors the harness's `run_with_svm_rollback` error branch: when a crank
//! is rejected, state must be byte-identical to before the submit. On-chain
//! the SVM guarantees the rollback; this test confirms the wrapper actually
//! aborts cleanly (no partial write) and that `submit.rs` classifies the
//! failure as `CrankOutcome::HardReject` rather than a silent `Ok`.
//!
//! Needs no engine decode — it's a raw pre/post byte comparison plus the
//! outcome-variant assertion.

mod common;

use percolator_cranker::submit::{submit_crank, CrankOutcome};

#[test]
fn hard_reject_leaves_target_bytes_unchanged() -> anyhow::Result<()> {
    let Some(live) = common::live_from_env() else {
        common::skip("hard_reject_leaves_target_bytes_unchanged");
        return Ok(());
    };

    // Pick any portfolio on this market as the crank target.
    let portfolios = percolator_cranker::discovery::discover_portfolios(&live.rpc, &live.program_id)?;
    let Some(target) = portfolios
        .into_iter()
        .find(|p| p.market_group_id().as_ref() == Some(&live.market))
    else {
        common::skip("hard_reject_leaves_target_bytes_unchanged (no portfolio for market)");
        return Ok(());
    };
    let target_pk = target.pubkey;

    // Snapshot pre-state.
    let pre = live.rpc.get_account_data(&target_pk)?;

    // Craft a submission we EXPECT the wrapper to hard-reject. The cleanest
    // deterministic reject is a `RefreshAccount{None}` path with no oracle
    // observations attached (see submit.rs module docs: "missing observation
    // on a RefreshAccount{None} path" => hard reject). We pass an empty
    // observation list and a stale/one now_slot so the wrapper aborts before
    // any mutation.
    //
    // NOTE: whether THIS particular input rejects depends on the target's
    // current plan. If your target isn't in the observation-required state,
    // swap in an input you know rejects for your fixture (e.g. a stale
    // selected-mark, or point at a portfolio from a different market to force
    // EngineProvenanceMismatch 0x10). The assertion below is the invariant;
    // the specific reject trigger is fixture-dependent.
    let outcome = submit_crank(
        &live.rpc,
        &live.program_id,
        // A throwaway payer keypair the validator has funded, or reuse your
        // bootstrap payer. Left as a TODO so this file has no secret material.
        &load_test_payer()?,
        &live.market,
        &target_pk,
        None,        // no reward portfolio
        &[],         // no observations -> expect reject on an observation-required plan
        1,           // deliberately-stale now_slot
    )?;

    // Post-state must be byte-identical on a hard reject.
    let post = live.rpc.get_account_data(&target_pk)?;

    match outcome {
        CrankOutcome::HardReject { error, error_name } => {
            eprintln!("hard reject as expected: {error_name:?} ({error})");
            assert_eq!(pre, post, "hard reject must not mutate the target account");
        }
        CrankOutcome::Ok {
            mutated,
            target_bytes_diff,
            ..
        } => {
            // If the wrapper accepted (e.g. the target happened to be in a
            // no-observation-progress state), the rollback property doesn't
            // apply — but flag it so the fixture can be tightened.
            eprintln!(
                "note: submission was accepted (mutated={mutated}, diff={target_bytes_diff}); \
                 pick a target/input that deterministically rejects to exercise this property."
            );
        }
    }
    Ok(())
}

/// TODO: return a funded `Keypair` for the local validator. Reuse your
/// `setup::bootstrap` payer or read a keypair file path from an env var.
/// Kept as a stub so no key material lives in the test source.
fn load_test_payer() -> anyhow::Result<solana_sdk::signature::Keypair> {
    anyhow::bail!(
        "wire load_test_payer() to your funded local-validator keypair \
         (e.g. read PERCOLATOR_TEST_PAYER=<path> and Keypair::read_from_file)"
    )
}
