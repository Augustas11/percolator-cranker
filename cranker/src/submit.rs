//! Submission + three-way response classification.
//!
//! After the 2026-06-26 no-observation-progress relaxation + the two
//! 2026-07-01 wrapper hard-reject guards (`6869e58`, `8067a13`), the
//! response shape for `PermissionlessCrank` is three-way:
//!
//! * `Err(hard reject)` — wrapper aborted (stale-selected-mark, stale
//!   window, missing observation on a `RefreshAccount{None}` path).
//!   The tx failed; state is unchanged. Retry only after understanding
//!   why (fetch fresh observation, wait for next slot, etc.).
//! * `Ok(no mutation)` — engine returned `NonProgress` from
//!   `RefreshAccount{None}` because no observation was attached. Safe
//!   to retry once we have an observation to attach.
//! * `Ok(mutation)` — engine dispatched a progress step (`Liquidate`,
//!   `SettleBChunk`, `RefreshAccount{Some}`, `ReleaseSourceLiens`,
//!   `AdvanceClose`, `DeclareRecovery`, `FinalizeRecovery`,
//!   `CloseResolved`). NOT idempotent. Retry only after re-reading state
//!   and confirming the goal isn't already achieved. A6
//!   (`ReleaseSourceLiens`) progresses one domain per call
//!   (`V16_SOURCE_LIEN_RELEASE_CHUNK_DOMAINS = 1`); drain in a loop.
//!
//! For scheduling, those three collapse to `OutcomeClass`: Progress, Idle,
//! Failed. A tx that reverts with `EngineNonProgress` (0x16) is **idle**:
//! the wrapper's resolved branch and the Hybrid same-slot rule return it
//! atomically when there is nothing to do. Only other reverts are failures.

use anyhow::{anyhow, Context, Result};
use percolator_prog::ix::{CrankObservationHint, Instruction as ProgInstruction};
use solana_client::{
    client_error::{ClientError, ClientErrorKind},
    rpc_client::RpcClient,
    rpc_config::RpcTransactionConfig,
    rpc_request::{RpcError, RpcResponseErrorData},
};
use solana_transaction_status::{option_serializer::OptionSerializer, UiTransactionEncoding};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    transaction::Transaction,
};

use crate::binding::{layout_is_current, V16_LAYOUT_DISCRIMINATOR};
use crate::observation::FetchedObservation;

/// Classification of a submission's outcome. Encapsulates the three-way
/// shape described in the module docs and carries enough context to log
/// or reconcile downstream.
#[derive(Debug)]
pub enum CrankOutcome {
    HardReject {
        error: String,
        /// Friendly name for the wrapper's `Custom(N)` code, if the error
        /// message matched a known variant. `None` for non-wrapper errors
        /// (RPC issues, blockhash timeouts, etc.).
        error_name: Option<&'static str>,
        /// Program CU from the preflight simulation logs, when the RPC
        /// returned them.
        program_cu: Option<u64>,
    },
    Ok {
        signature: Signature,
        pre_state_hash: u64,
        post_state_hash: u64,
        mutated: bool,
        /// Byte-diff count between pre- and post-state on the target
        /// portfolio. Zero means the engine returned `NonProgress` (or
        /// a fresh refresh that didn't move any observable byte).
        target_bytes_diff: usize,
        /// Byte-diff count on the cranker reward portfolio, if provided.
        /// Non-zero indicates a fee share landed.
        reward_bytes_diff: Option<usize>,
        /// CU the percolator program itself consumed (from the tx logs).
        program_cu: Option<u64>,
        /// CU for the whole tx, including the two compute-budget ixs.
        tx_cu: Option<u64>,
    },
}

/// Scheduling view of a `CrankOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeClass {
    Progress,
    Idle,
    Failed,
}

impl CrankOutcome {
    pub fn class(&self) -> OutcomeClass {
        match self {
            CrankOutcome::Ok { mutated: true, .. } => OutcomeClass::Progress,
            CrankOutcome::Ok { mutated: false, .. } => OutcomeClass::Idle,
            CrankOutcome::HardReject { error_name, .. } if is_non_progress(*error_name) => {
                OutcomeClass::Idle
            }
            CrankOutcome::HardReject { .. } => OutcomeClass::Failed,
        }
    }

    /// Short error key for quarantine streaks: the wrapper variant name,
    /// or the first line of the RPC error when the code is unknown.
    pub fn error_key(&self) -> Option<String> {
        match self {
            CrankOutcome::HardReject { error_name: Some(n), .. } => Some((*n).to_string()),
            CrankOutcome::HardReject { error, .. } => {
                Some(error.lines().next().unwrap_or("unknown").chars().take(120).collect())
            }
            CrankOutcome::Ok { .. } => None,
        }
    }

    pub fn program_cu(&self) -> Option<u64> {
        match self {
            CrankOutcome::Ok { program_cu, .. } | CrankOutcome::HardReject { program_cu, .. } => {
                *program_cu
            }
        }
    }
}

pub fn is_non_progress(error_name: Option<&str>) -> bool {
    error_name == Some("EngineNonProgress")
}

/// Parse `Program <id> consumed N of M compute units` for `program_id`.
/// Takes the last match: the top-level invocation logs its total last.
pub fn program_cu_from_logs(program_id: &Pubkey, logs: &[String]) -> Option<u64> {
    let prefix = format!("Program {program_id} consumed ");
    logs.iter().rev().find_map(|l| {
        let rest = l.strip_prefix(&prefix)?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

fn preflight_logs(e: &ClientError) -> Option<&[String]> {
    match e.kind() {
        ClientErrorKind::RpcError(RpcError::RpcResponseError {
            data: RpcResponseErrorData::SendTransactionPreflightFailure(sim),
            ..
        }) => sim.logs.as_deref(),
        _ => None,
    }
}

/// `(program_cu, tx_cu)` for a confirmed signature.
pub fn fetch_cu(rpc: &RpcClient, program_id: &Pubkey, sig: &Signature) -> (Option<u64>, Option<u64>) {
    let cfg = RpcTransactionConfig {
        encoding: Some(UiTransactionEncoding::Json),
        commitment: Some(CommitmentConfig::confirmed()),
        max_supported_transaction_version: Some(0),
    };
    let Ok(tx) = rpc.get_transaction_with_config(sig, cfg) else {
        return (None, None);
    };
    let Some(meta) = tx.transaction.meta else {
        return (None, None);
    };
    let tx_cu = match meta.compute_units_consumed {
        OptionSerializer::Some(v) => Some(v),
        _ => None,
    };
    let program_cu = match &meta.log_messages {
        OptionSerializer::Some(logs) => program_cu_from_logs(program_id, logs),
        _ => None,
    };
    (program_cu, tx_cu)
}

/// Match the `Custom(N)` code in a wrapper error message to a
/// `PercolatorError` variant name. Codes are the u32-cast enum
/// discriminants at `repo-prog/src/v16_program.rs:96` — see
/// `PercolatorError` there. Keep in sync manually or the friendly names
/// silently drift; not worth pulling in a proc-macro for this.
pub fn wrapper_error_name(msg: &str) -> Option<&'static str> {
    let hex_marker = "custom program error: 0x";
    let idx = msg.find(hex_marker)?;
    let after = &msg[idx + hex_marker.len()..];
    let hex_str: String = after
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    if hex_str.is_empty() {
        return None;
    }
    let code = u32::from_str_radix(&hex_str, 16).ok()?;
    Some(match code {
        0x00 => "InvalidMagic",
        0x01 => "InvalidVersion",
        0x02 => "AlreadyInitialized",
        0x03 => "NotInitialized",
        0x04 => "InvalidAccountKind",
        0x05 => "InvalidAccountLen",
        0x06 => "ExpectedSigner",
        0x07 => "ExpectedWritable",
        0x08 => "Unauthorized",
        0x09 => "InvalidInstruction",
        0x0A => "InvalidMint",
        0x0B => "InvalidTokenAccount",
        0x0C => "InvalidVaultAccount",
        0x0D => "InvalidTokenProgram",
        0x0E => "EngineInvalidConfig",
        0x0F => "EngineArithmeticOverflow",
        0x10 => "EngineProvenanceMismatch",
        0x11 => "EngineHiddenLeg",
        0x12 => "EngineInvalidLeg",
        0x13 => "EngineStale",
        0x14 => "EngineBStale",
        0x15 => "EngineLockActive",
        0x16 => "EngineNonProgress",
        0x17 => "EngineRecoveryRequired",
        0x18 => "EngineCounterOverflow",
        0x19 => "EngineCounterUnderflow",
        0x1A => "OracleInvalid",
        0x1B => "OracleStale",
        0x1C => "OracleConfTooWide",
        0x1D => "InvalidOracleKey",
        0x1E => "AssetGenerationMismatch",
        0x1F => "RentExemptRequired",
        _ => return None,
    })
}

/// Submit a single `PermissionlessCrank` with the given hint list.
///
/// Account order (verified in M1):
///   `[payer(signer) → market(writable) → portfolio(writable)
///     → oracle_accounts(readonly, hint-ordered)
///     → cranker_reward_portfolio(writable, optional last)]`
///
/// `pre_state_hash` / `post_state_hash` are cheap non-crypto fingerprints
/// used only for local mutation detection. Not authoritative.
pub fn submit_crank(
    rpc: &RpcClient,
    program_id: &Pubkey,
    payer: &Keypair,
    market: &Pubkey,
    target_portfolio: &Pubkey,
    reward_portfolio: Option<&Pubkey>,
    observations: &[FetchedObservation],
    now_slot: u64,
) -> Result<CrankOutcome> {
    // Sanity: hint counts must match the fetched oracle accounts, otherwise
    // the wrapper consumes the wrong number of tail accounts per hint.
    let hints: Vec<CrankObservationHint> = observations
        .iter()
        .map(|obs| CrankObservationHint {
            asset_index: obs.asset_index,
            oracle_accounts: obs.oracle_account_count(),
        })
        .collect();

    // Build account meta list: payer, market, portfolio, then oracle
    // accounts in hint order, then optional reward portfolio.
    let mut accounts = vec![
        AccountMeta::new(payer.pubkey(), true),
        AccountMeta::new(*market, false),
        AccountMeta::new(*target_portfolio, false),
    ];
    for obs in observations {
        for oracle_ai in &obs.oracle_accounts {
            accounts.push(AccountMeta::new_readonly(*oracle_ai, false));
        }
    }
    if let Some(reward) = reward_portfolio {
        accounts.push(AccountMeta::new(*reward, false));
    }

    let ix = ProgInstruction::PermissionlessCrank {
        now_slot,
        observations: hints,
    };
    let instruction = Instruction {
        program_id: *program_id,
        accounts,
        data: ix.encode(),
    };

    // Snapshot pre-state. Layout 18 is required on the decode path;
    // a stale-layout account is a hard reject, not a crank.
    let pre_target = rpc
        .get_account_data(target_portfolio)
        .context("read target portfolio pre-state")?;
    if !layout_is_current(&pre_target) {
        return Ok(CrankOutcome::HardReject {
            error: format!(
                "target portfolio layout_discriminator != {V16_LAYOUT_DISCRIMINATOR}"
            ),
            error_name: Some("InvalidAccountLen"),
            program_cu: None,
        });
    }
    let pre_reward = match reward_portfolio {
        Some(r) => Some(rpc.get_account_data(r).context("read reward portfolio pre-state")?),
        None => None,
    };
    let pre_state_hash = fnv_hash(&pre_target);

    // Submit with retry-on-blockhash-not-found (same pattern the M1 client
    // uses; the test validator's RPC occasionally races the bank).
    let mut last_err: Option<String> = None;
    for attempt in 0..5 {
        let (bh, _) = rpc
            .get_latest_blockhash_with_commitment(CommitmentConfig::processed())
            .context("get_latest_blockhash")?;
        let tx = Transaction::new_signed_with_payer(
            &[cu_ix(), heap_ix(), instruction.clone()],
            Some(&payer.pubkey()),
            &[payer],
            bh,
        );
        match rpc.send_and_confirm_transaction(&tx) {
            Ok(signature) => {
                // Post-state.
                let post_target = rpc
                    .get_account_data(target_portfolio)
                    .context("read target portfolio post-state")?;
                let post_reward = match reward_portfolio {
                    Some(r) => Some(
                        rpc.get_account_data(r)
                            .context("read reward portfolio post-state")?,
                    ),
                    None => None,
                };
                let target_bytes_diff = byte_diff_count(&pre_target, &post_target);
                let reward_bytes_diff = match (&pre_reward, &post_reward) {
                    (Some(a), Some(b)) => Some(byte_diff_count(a, b)),
                    _ => None,
                };
                let post_state_hash = fnv_hash(&post_target);
                let mutated = target_bytes_diff > 0
                    || reward_bytes_diff.map(|d| d > 0).unwrap_or(false);
                let (program_cu, tx_cu) = if measure_cu() {
                    fetch_cu(rpc, program_id, &signature)
                } else {
                    (None, None)
                };
                return Ok(CrankOutcome::Ok {
                    signature,
                    pre_state_hash,
                    post_state_hash,
                    mutated,
                    target_bytes_diff,
                    reward_bytes_diff,
                    program_cu,
                    tx_cu,
                });
            }
            Err(e) => {
                let program_cu = preflight_logs(&e).and_then(|l| program_cu_from_logs(program_id, l));
                let msg = format!("{e}");
                if msg.contains("Blockhash not found") && attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    last_err = Some(msg);
                    continue;
                }
                let error_name = wrapper_error_name(&msg);
                return Ok(CrankOutcome::HardReject { error: msg, error_name, program_cu });
            }
        }
    }
    Err(anyhow!(last_err.unwrap_or_else(|| "unknown submit error".to_string())))
}

/// `CRANKER_MEASURE_CU=0` skips the extra `getTransaction` per crank.
fn measure_cu() -> bool {
    std::env::var("CRANKER_MEASURE_CU").ok().as_deref() != Some("0")
}

fn cu_ix() -> Instruction {
    ComputeBudgetInstruction::set_compute_unit_limit(1_400_000)
}

fn heap_ix() -> Instruction {
    ComputeBudgetInstruction::request_heap_frame(128 * 1024)
}

fn byte_diff_count(a: &[u8], b: &[u8]) -> usize {
    let common = a.len().min(b.len());
    let mut diff = a[..common]
        .iter()
        .zip(b[..common].iter())
        .filter(|(x, y)| x != y)
        .count();
    diff += a.len().abs_diff(b.len());
    diff
}

/// Cheap non-crypto 64-bit fingerprint (FNV-1a). Just for local
/// pre/post mutation detection; never authoritative for anything.
fn fnv_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use percolator_prog::ix::{CrankObservationHint, Instruction as ProgInstruction};

    fn reject(name: Option<&'static str>) -> CrankOutcome {
        CrankOutcome::HardReject {
            error: "x".into(),
            error_name: name,
            program_cu: None,
        }
    }

    #[test]
    fn non_progress_revert_is_idle_not_failed() {
        assert_eq!(
            wrapper_error_name("custom program error: 0x16"),
            Some("EngineNonProgress")
        );
        assert_eq!(reject(Some("EngineNonProgress")).class(), OutcomeClass::Idle);
        assert_eq!(reject(Some("EngineCounterUnderflow")).class(), OutcomeClass::Failed);
        assert_eq!(reject(None).class(), OutcomeClass::Failed);
        let ok = |mutated| CrankOutcome::Ok {
            signature: Signature::default(),
            pre_state_hash: 0,
            post_state_hash: 0,
            mutated,
            target_bytes_diff: 0,
            reward_bytes_diff: None,
            program_cu: None,
            tx_cu: None,
        };
        assert_eq!(ok(false).class(), OutcomeClass::Idle);
        assert_eq!(ok(true).class(), OutcomeClass::Progress);
    }

    #[test]
    fn program_cu_parses_top_level_line() {
        let pid = Pubkey::new_unique();
        let logs = vec![
            "Program ComputeBudget111111111111111111111111111111 consumed 150 of 1400000 compute units".to_string(),
            format!("Program {pid} invoke [1]"),
            format!("Program {pid} consumed 17975 of 1399700 compute units"),
            format!("Program {pid} success"),
        ];
        assert_eq!(program_cu_from_logs(&pid, &logs), Some(17_975));
        assert_eq!(program_cu_from_logs(&Pubkey::new_unique(), &logs), None);
    }

    #[test]
    fn permissionless_crank_wire_has_no_close_q() {
        let ix = ProgInstruction::PermissionlessCrank {
            now_slot: 1,
            observations: vec![CrankObservationHint {
                asset_index: 0,
                oracle_accounts: 0,
            }],
        };
        let bytes = ix.encode();
        // tag=5 (1) | now_slot (8) | n (1) | hint (asset_index 2 + oracle_accounts 1)
        assert_eq!(bytes.len(), 1 + 8 + 1 + 3);
        assert_eq!(bytes[0], 5);
        let decoded = ProgInstruction::decode(&bytes).expect("round-trip");
        match decoded {
            ProgInstruction::PermissionlessCrank {
                now_slot,
                observations,
            } => {
                assert_eq!(now_slot, 1);
                assert_eq!(observations.len(), 1);
            }
            other => panic!("unexpected decode: {other:?}"),
        }
    }
}
