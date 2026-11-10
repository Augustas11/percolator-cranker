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

use anyhow::{Context, Result};
use percolator_prog::ix::{CrankObservationHint, Instruction as ProgInstruction};
use solana_client::{
    client_error::{ClientError, ClientErrorKind},
    rpc_client::RpcClient,
    rpc_config::{RpcSendTransactionConfig, RpcTransactionConfig},
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

#[derive(Debug, Default)]
pub struct BlockhashCache(std::sync::Mutex<Option<(solana_sdk::hash::Hash, std::time::Instant)>>);

impl BlockhashCache {
    pub fn get(&self, rpc: &RpcClient) -> Result<solana_sdk::hash::Hash> {
        let mut cached = self.0.lock().unwrap();
        if let Some((hash, at)) = *cached {
            if at.elapsed() < std::time::Duration::from_secs(20) { return Ok(hash); }
        }
        let (hash, _) = rpc.get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
            .context("get_latest_blockhash")?;
        *cached = Some((hash, std::time::Instant::now()));
        Ok(hash)
    }

    pub fn invalidate(&self) { *self.0.lock().unwrap() = None; }
}

/// One crank sender at a time in this process. A crank that lands after
/// another one already did its work reverts on chain (EngineNonProgress) and
/// still pays its fee. Gated senders hold this from a preflight at
/// `processed` (which sees every earlier landed crank) through confirmation,
/// so a crank gone stale fails preflight instead, for free. The keeper's send
/// below holds it; the staging bots' sends and the pusher's accrual share it.
/// Only senders in this process are covered.
pub static SEND_GATE: std::sync::Mutex<SendGate> = std::sync::Mutex::new(SendGate { pending: None, last_send: None });

/// How long a blocking holder waits for a pending send before relying on its
/// own preflight (callers may hold other locks, e.g. the pusher's).
pub const SEND_GATE_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
/// A pending send is forgotten after this even if its blockhash can't be
/// checked (blockhashes expire in about 60-90 s).
pub const SEND_GATE_PENDING_MAX: std::time::Duration = std::time::Duration::from_secs(120);

pub struct SendGate {
    /// Sent, outcome unknown: it may land until its blockhash expires.
    pending: Option<(Signature, solana_sdk::hash::Hash, std::time::Instant)>,
    /// When a holder last sent: a market read from before then can be stale.
    pub last_send: Option<std::time::Instant>,
}

impl SendGate {
    /// Keeps an older unsettled send: one slot, and the first may land first.
    pub fn set_pending(&mut self, tx: &Transaction) {
        if self.pending.is_none() {
            self.pending = Some((tx.signatures[0], tx.message.recent_blockhash, std::time::Instant::now()));
        }
    }

    /// One check: true when no earlier uncertain send can still land.
    pub fn try_settle(&mut self, rpc: &RpcClient) -> bool {
        let Some((sig, hash, at)) = self.pending else { return true };
        let seen = matches!(rpc.get_signature_status_with_commitment(&sig, CommitmentConfig::processed()), Ok(Some(_)));
        let expired = matches!(rpc.is_blockhash_valid(&hash, CommitmentConfig::processed()), Ok(false));
        if seen || expired || at.elapsed() >= SEND_GATE_PENDING_MAX {
            self.pending = None;
            return true;
        }
        false
    }
}

/// Take the gate, waiting up to SEND_GATE_MAX_WAIT for a pending send.
pub fn hold_send_gate(rpc: &RpcClient) -> std::sync::MutexGuard<'static, SendGate> {
    let mut gate = SEND_GATE.lock().unwrap_or_else(|p| p.into_inner());
    let started = std::time::Instant::now();
    while !gate.try_settle(rpc) && started.elapsed() < SEND_GATE_MAX_WAIT {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    gate
}

/// The tx may have landed (or may still): not a preflight rejection and not
/// an on-chain failure.
pub fn send_outcome_uncertain(e: &ClientError) -> bool {
    !matches!(
        e.kind(),
        ClientErrorKind::TransactionError(_)
            | ClientErrorKind::RpcError(RpcError::RpcResponseError {
                data: RpcResponseErrorData::SendTransactionPreflightFailure(_),
                ..
            })
    )
}

fn send_and_confirm(rpc: &RpcClient, tx: &Transaction) -> std::result::Result<Signature, Box<ClientError>> {
    send_and_confirm_with_timeout(rpc, tx, std::time::Duration::from_secs(60))
}

fn send_and_confirm_with_timeout(rpc: &RpcClient, tx: &Transaction, timeout: std::time::Duration) -> std::result::Result<Signature, Box<ClientError>> {
    // Preflight at `processed`: under SEND_GATE it then sees every earlier
    // landed crank; at `confirmed` it could miss one landed a slot ago.
    let signature = rpc.send_transaction_with_config(tx, RpcSendTransactionConfig {
        preflight_commitment: Some(CommitmentConfig::processed().commitment),
        ..Default::default()
    })?;
    let started = std::time::Instant::now();
    let poll = std::time::Duration::from_millis(std::env::var("CRANKER_CONFIRM_POLL_MS")
        .ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(2000).max(1));
    loop {
        let response = rpc.get_signature_statuses(&[signature])?;
        if let Some(status) = response.value.first().and_then(Option::as_ref) {
            if status.satisfies_commitment(CommitmentConfig::confirmed()) {
                if let Some(error) = &status.err { return Err(Box::new(ClientErrorKind::TransactionError(error.clone()).into())); }
                return Ok(signature);
            }
        }
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err(Box::new(ClientErrorKind::Custom(format!("transaction not confirmed in {} s", timeout.as_secs_f64())).into()));
        }
        std::thread::sleep(poll.min(left));
    }
}

fn snapshot(rpc: &RpcClient, keys: &[Pubkey], contexts: &[&str]) -> Result<Vec<Vec<u8>>> {
    let accounts = rpc.get_multiple_accounts(keys).context(contexts[0].to_string())?;
    accounts.into_iter().zip(contexts).map(|(account, context)| {
        account.map(|a| a.data).ok_or_else(|| anyhow::anyhow!("account missing"))
            .context((*context).to_string())
    }).collect()
}

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
        /// Whole-tx CU from the preflight simulation, when returned.
        tx_cu: Option<u64>,
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
        /// Byte-diff count on the market account (Resolved route only):
        /// non-zero when a backing hint expired a bucket.
        market_bytes_diff: Option<usize>,
        /// CU the percolator program itself consumed (from the tx logs).
        program_cu: Option<u64>,
        /// CU for the whole tx, including the compute-budget ixs and (on the
        /// Resolved route) the ATA create.
        tx_cu: Option<u64>,
        /// The tx landed, but its post-state could not be read (the error).
        /// The diffs above are then unknown, not zero. Classified as
        /// Progress: the wrapper accepted it, and a confirmed tx must never
        /// count as a failure.
        unreconciled: Option<String>,
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
            CrankOutcome::Ok { mutated: true, .. }
            | CrankOutcome::Ok { unreconciled: Some(_), .. } => OutcomeClass::Progress,
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

    /// Whole-tx CU (what the cycle budget is charged), when known.
    pub fn tx_cu(&self) -> Option<u64> {
        match self {
            CrankOutcome::Ok { tx_cu, .. } | CrankOutcome::HardReject { tx_cu, .. } => *tx_cu,
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
/// Sum top-level invocations; nested consumption is already included in their totals.
pub fn program_cu_from_logs(program_id: &Pubkey, logs: &[String]) -> Option<u64> {
    let invoke_prefix = format!("Program {program_id} invoke [");
    let consumed_prefix = format!("Program {program_id} consumed ");
    let mut invocations = Vec::new();
    let mut total = None;
    for log in logs {
        if let Some(depth) = log.strip_prefix(&invoke_prefix) {
            invocations.push(depth == "1]");
        } else if let Some(rest) = log.strip_prefix(&consumed_prefix) {
            if invocations.pop() == Some(true) {
                if let Some(cu) = rest.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()) {
                    *total.get_or_insert(0) += cu;
                }
            }
        }
    }
    total
}

/// `(program_cu, tx_cu)` from a preflight-simulation failure, when the RPC
/// returned them.
fn preflight_cu(e: &ClientError, program_id: &Pubkey) -> (Option<u64>, Option<u64>) {
    match e.kind() {
        ClientErrorKind::RpcError(RpcError::RpcResponseError {
            data: RpcResponseErrorData::SendTransactionPreflightFailure(sim),
            ..
        }) => (
            sim.logs.as_deref().and_then(|l| program_cu_from_logs(program_id, l)),
            sim.units_consumed,
        ),
        _ => (None, None),
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
/// discriminants at the wrapper's `src/v16_program.rs:96` — see
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
    submit_crank_routed(
        rpc,
        program_id,
        payer,
        market,
        target_portfolio,
        &CrankRoute::Live { reward_portfolio },
        observations,
        now_slot,
        None,
    )
}

/// Account layout for a tag-5 crank. The wire is the same; the wrapper
/// reads the accounts differently once the market is Resolved.
#[derive(Debug, Clone)]
pub enum CrankRoute<'a> {
    /// `[payer(signer), market, portfolio, oracle tail…, reward portfolio?]`.
    Live { reward_portfolio: Option<&'a Pubkey> },
    /// Resolved (`handle_close_resolved`):
    /// `[owner, market, portfolio, owner_ata, vault, vault_authority, token_program]`.
    /// Account 0 must be the portfolio **owner** key. It needs to sign only
    /// while `slot - resolved_slot < force_close_delay_slots`; after that the
    /// crank is permissionless and the cranker only pays the fee. Slots 3–6
    /// carry the payout (to the owner's ATA, which the wrapper checks). An
    /// idempotent ATA create runs first so a missing ATA can't block the
    /// payout; the cranker pays that rent. No reward portfolio on this route.
    Resolved {
        owner: Pubkey,
        mint: Pubkey,
        vault: Pubkey,
        vault_authority: Pubkey,
    },
}

/// Like [`submit_crank`] with an explicit account route.
#[allow(clippy::too_many_arguments)]
pub fn submit_crank_routed(
    rpc: &RpcClient,
    program_id: &Pubkey,
    payer: &Keypair,
    market: &Pubkey,
    target_portfolio: &Pubkey,
    route: &CrankRoute<'_>,
    observations: &[FetchedObservation],
    now_slot: u64,
    deadline: Option<std::time::Instant>,
) -> Result<CrankOutcome> {
    submit_crank_routed_cached(rpc, &BlockhashCache::default(), program_id, payer, market,
        target_portfolio, route, observations, now_slot, deadline)
}

#[allow(clippy::too_many_arguments)]
pub fn submit_crank_routed_cached(
    rpc: &RpcClient,
    blockhash: &BlockhashCache,
    program_id: &Pubkey,
    payer: &Keypair,
    market: &Pubkey,
    target_portfolio: &Pubkey,
    route: &CrankRoute<'_>,
    observations: &[FetchedObservation],
    now_slot: u64,
    deadline: Option<std::time::Instant>,
) -> Result<CrankOutcome> {
    let reward_portfolio = match route {
        CrankRoute::Live { reward_portfolio } => *reward_portfolio,
        CrankRoute::Resolved { .. } => None,
    };
    let mut paired = matches!(route, CrankRoute::Live { .. }) && paired_crank_enabled();
    let (mut ixs, crank_ix_index) = build_crank_instructions(
        program_id,
        &payer.pubkey(),
        market,
        target_portfolio,
        route,
        observations,
        now_slot,
        paired,
    );
    // On the Resolved route a backing hint can make market-only progress
    // (expiring a bucket) without touching the target, so the market
    // account is diffed too. This cannot turn an unrelated market write into
    // a false "progress": in `handle_close_resolved` any engine selection
    // other than `CloseResolved` reverts with EngineNonProgress, and
    // `CloseResolved` is always one state transition (`ProgressOnly` or
    // `Closed`). A landed Resolved crank therefore always made progress; a
    // no-progress one never lands, so it is classified Idle, never Ok.
    let track_market = matches!(route, CrankRoute::Resolved { .. });

    // Snapshot pre-state. Layout 18 is required on the decode path;
    // a stale-layout account is a hard reject, not a crank.
    let mut keys = vec![*target_portfolio];
    let mut pre_contexts = vec!["read target portfolio pre-state"];
    let mut post_contexts = vec!["read target portfolio post-state"];
    if let Some(reward) = reward_portfolio {
        keys.push(*reward);
        pre_contexts.push("read reward portfolio pre-state");
        post_contexts.push("read reward portfolio post-state");
    }
    if track_market {
        keys.push(*market);
        pre_contexts.push("read market pre-state");
        post_contexts.push("read market post-state");
    }
    let pre = snapshot(rpc, &keys, &pre_contexts)?;
    let pre_target = &pre[0];
    if !layout_is_current(pre_target) {
        return Ok(CrankOutcome::HardReject {
            error: format!(
                "target portfolio layout_discriminator != {V16_LAYOUT_DISCRIMINATOR}"
            ),
            error_name: Some("InvalidAccountLen"),
            program_cu: None,
            tx_cu: None,
        });
    }
    let pre_reward = reward_portfolio.map(|_| &pre[1]);
    let pre_market = track_market.then(|| pre.last().unwrap());
    let pre_state_hash = fnv_hash(pre_target);

    // Fetch a confirmed blockhash to match preflight commitment on public
    // load-balanced RPCs. A preflight "Blockhash not found" means
    // the previous attempt was never forwarded, so a retry re-signs the same
    // instructions with a fresh blockhash and stays within the one budget
    // reservation the caller took. The deadline is re-checked before every
    // send.
    let mut attempt = 0;
    loop {
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            return Err(DeadlineReached.into());
        }
        let bh = blockhash.get(rpc)?;
        let tx = Transaction::new_signed_with_payer(
            &ixs,
            Some(&payer.pubkey()),
            &[payer],
            bh,
        );
        // Re-check right before the send: the blockhash fetch itself may have
        // crossed the deadline. The deadline is an ADMISSION deadline: once a
        // tx is sent, confirmation has its own 60 s bound.
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            return Err(DeadlineReached.into());
        }
        let mut gate = hold_send_gate(rpc);
        // The gate wait counts against the admission deadline too.
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            return Err(DeadlineReached.into());
        }
        let sent = send_and_confirm(rpc, &tx);
        gate.last_send = Some(std::time::Instant::now());
        if sent.as_ref().is_err_and(|e| send_outcome_uncertain(e)) { gate.set_pending(&tx); }
        drop(gate);
        match sent {
            Ok(signature) => {
                // The tx LANDED. From here on, read failures must not turn it
                // into a failure (that would strike a healthy account and drop
                // the record): keep a confirmed outcome and note what could
                // not be reconciled.
                let (program_cu, tx_cu) = if measure_cu() {
                    fetch_cu(rpc, program_id, &signature)
                } else {
                    (None, None)
                };
                let post = (|| -> Result<(Vec<u8>, Option<usize>, Option<usize>)> {
                    let post = snapshot(rpc, &keys, &post_contexts)?;
                    let post_target = post[0].clone();
                    let reward_bytes_diff = pre_reward.map(|a| byte_diff_count(a, &post[1]));
                    let market_bytes_diff = pre_market.map(|a| byte_diff_count(a, post.last().unwrap()));
                    Ok((post_target, reward_bytes_diff, market_bytes_diff))
                })();
                return Ok(match post {
                    Ok((post_target, reward_bytes_diff, market_bytes_diff)) => {
                        let target_bytes_diff = byte_diff_count(pre_target, &post_target);
                        let mutated = target_bytes_diff > 0
                            || reward_bytes_diff.is_some_and(|d| d > 0)
                            || market_bytes_diff.is_some_and(|d| d > 0);
                        CrankOutcome::Ok {
                            signature,
                            pre_state_hash,
                            post_state_hash: fnv_hash(&post_target),
                            mutated,
                            target_bytes_diff,
                            reward_bytes_diff,
                            market_bytes_diff,
                            program_cu,
                            tx_cu,
                            unreconciled: None,
                        }
                    }
                    Err(e) => CrankOutcome::Ok {
                        signature,
                        pre_state_hash,
                        post_state_hash: pre_state_hash,
                        mutated: false,
                        target_bytes_diff: 0,
                        reward_bytes_diff: None,
                        market_bytes_diff: None,
                        program_cu,
                        tx_cu,
                        unreconciled: Some(format!("{e:#}")),
                    },
                });
            }
            Err(e) => {
                let (program_cu, tx_cu) = preflight_cu(&e, program_id);
                let msg = format!("{e}");
                if msg.contains("Blockhash not found") { blockhash.invalidate(); }
                if msg.contains("Blockhash not found") && attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    attempt += 1;
                    continue;
                }
                // Definitive: the preflight simulation rejected it, or the tx
                // executed and failed. Anything else (transport error, status
                // poll failure, "unable to confirm") is UNCERTAIN: the tx may
                // have landed. Ask for its status once before assigning blame.
                let preflight_rejected = matches!(
                    e.kind(),
                    ClientErrorKind::RpcError(RpcError::RpcResponseError {
                        data: RpcResponseErrorData::SendTransactionPreflightFailure(_),
                        ..
                    })
                );
                let definitive = preflight_rejected
                    || matches!(e.kind(), ClientErrorKind::TransactionError(_));
                if !definitive {
                    let signature = tx.signatures[0];
                    match rpc.get_signature_status(&signature) {
                        Ok(Some(Ok(()))) => {
                            return Ok(CrankOutcome::Ok {
                                signature,
                                pre_state_hash,
                                post_state_hash: pre_state_hash,
                                mutated: false,
                                target_bytes_diff: 0,
                                reward_bytes_diff: None,
                                market_bytes_diff: None,
                                program_cu: None,
                                tx_cu: None,
                                unreconciled: Some(format!("landed, but confirmation errored: {msg}")),
                            })
                        }
                        Ok(Some(Err(te))) => {
                            let error = format!("{te}");
                            let error_name = wrapper_error_name_for_crank(&error, crank_ix_index, paired);
                            return Ok(CrankOutcome::HardReject { error, error_name, program_cu, tx_cu });
                        }
                        _ => return Err(Unconfirmed { signature, error: msg }.into()),
                    }
                }
                if paired {
                    if let Some(single) = single_crank_retry_instructions(
                        &ixs, crank_ix_index, preflight_rejected, &msg,
                    ) {
                        paired = false;
                        ixs = single;
                        continue;
                    }
                }
                let error_name = wrapper_error_name_for_crank(&msg, crank_ix_index, paired);
                return Ok(CrankOutcome::HardReject { error: msg, error_name, program_cu, tx_cu });
            }
        }
    }
}

/// The tx's instruction list: compute-budget ixs, (Resolved route) an
/// idempotent owner-ATA create, then the crank (twice when Live and paired).
/// Returns the first crank's index,
/// which error attribution needs. Pure: no RPC.
#[allow(clippy::too_many_arguments)]
pub fn build_crank_instructions(
    program_id: &Pubkey,
    payer: &Pubkey,
    market: &Pubkey,
    target_portfolio: &Pubkey,
    route: &CrankRoute<'_>,
    observations: &[FetchedObservation],
    now_slot: u64,
    paired: bool,
) -> (Vec<Instruction>, usize) {
    // Hint counts must match the fetched oracle accounts, otherwise the
    // wrapper consumes the wrong number of tail accounts per hint.
    let hints: Vec<CrankObservationHint> = observations
        .iter()
        .map(|obs| CrankObservationHint {
            asset_index: obs.asset_index,
            oracle_accounts: obs.oracle_account_count(),
        })
        .collect();
    let mut ixs = vec![cu_ix(), heap_ix()];
    let accounts = match route {
        // Live: payer, market, portfolio, then oracle accounts in hint
        // order, then optional reward portfolio.
        CrankRoute::Live { reward_portfolio } => {
            let mut accounts = vec![
                AccountMeta::new(*payer, true),
                AccountMeta::new(*market, false),
                AccountMeta::new(*target_portfolio, false),
            ];
            for obs in observations {
                for oracle_ai in &obs.oracle_accounts {
                    accounts.push(AccountMeta::new_readonly(*oracle_ai, false));
                }
            }
            if let Some(reward) = reward_portfolio {
                accounts.push(AccountMeta::new(**reward, false));
            }
            accounts
        }
        CrankRoute::Resolved {
            owner,
            mint,
            vault,
            vault_authority,
        } => {
            let owner_ata = spl_associated_token_account::get_associated_token_address(owner, mint);
            ixs.push(
                spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                    payer,
                    owner,
                    mint,
                    &spl_token::ID,
                ),
            );
            vec![
                AccountMeta::new_readonly(*owner, owner == payer),
                AccountMeta::new(*market, false),
                AccountMeta::new(*target_portfolio, false),
                AccountMeta::new(owner_ata, false),
                AccountMeta::new(*vault, false),
                AccountMeta::new_readonly(*vault_authority, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ]
        }
    };
    let crank_index = ixs.len();
    ixs.push(Instruction {
        program_id: *program_id,
        accounts,
        data: ProgInstruction::PermissionlessCrank {
            now_slot,
            observations: hints,
        }
        .encode(),
    });
    if paired && matches!(route, CrankRoute::Live { .. }) {
        ixs.push(ixs[crank_index].clone());
    }
    (ixs, crank_index)
}

/// Retry only a preflight rejection of the second crank, before any fee is paid.
pub fn should_fallback_to_single(preflight_rejected: bool, msg: &str, crank_ix_index: usize) -> bool {
    let Some((_, rest)) = msg.split_once("Error processing Instruction ") else {
        return false;
    };
    let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    preflight_rejected && n.parse::<usize>().ok() == Some(crank_ix_index + 1)
}

/// Preserve the first crank only when the pair was rejected before submission.
pub fn single_crank_retry_instructions(
    ixs: &[Instruction],
    crank_ix_index: usize,
    preflight_rejected: bool,
    msg: &str,
) -> Option<Vec<Instruction>> {
    if !should_fallback_to_single(preflight_rejected, msg, crank_ix_index) {
        return None;
    }
    ixs.get(..crank_ix_index + 1).map(|single| single.to_vec())
}

/// Pairing is on unless explicitly disabled, like CRANKER_MEASURE_CU.
fn paired_crank_enabled() -> bool {
    std::env::var("CRANKER_PAIRED_CRANK").ok().as_deref() != Some("0")
}

/// Attribute either crank in a pair, including failures confirmed on chain.
fn wrapper_error_name_for_crank(
    msg: &str,
    crank_ix_index: usize,
    paired: bool,
) -> Option<&'static str> {
    wrapper_error_name_for_ix(msg, crank_ix_index).or_else(|| {
        if paired {
            wrapper_error_name_for_ix(msg, crank_ix_index + 1)
        } else {
            None
        }
    })
}

/// Like [`wrapper_error_name`], but only when the error came from the
/// instruction at `ix_index` (the crank). Best-effort: when the message
/// carries no "Error processing Instruction N" text, the bare code is
/// mapped as before and could in principle belong to another instruction. A custom error from another
/// instruction in the tx (the ATA create, a token program) is not a
/// wrapper error and gets no wrapper name.
pub fn wrapper_error_name_for_ix(msg: &str, ix_index: usize) -> Option<&'static str> {
    let marker = "Error processing Instruction ";
    if let Some(i) = msg.find(marker) {
        let n: String = msg[i + marker.len()..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if n.parse::<usize>().ok() != Some(ix_index) {
            return None;
        }
    }
    wrapper_error_name(msg)
}

/// The cycle deadline passed before the tx was sent: nothing went on chain.
/// The caller treats it as budget deferral, never as an account failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadlineReached;

impl std::fmt::Display for DeadlineReached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cycle deadline reached before send")
    }
}

impl std::error::Error for DeadlineReached {}

/// The tx was sent but its fate is unknown (confirmation errored and the
/// status query gave no answer). Not an account failure: the caller keeps
/// the CU reservation charged and does not strike the account.
#[derive(Debug, Clone)]
pub struct Unconfirmed {
    pub signature: Signature,
    pub error: String,
}

impl std::fmt::Display for Unconfirmed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tx {} unconfirmed: {}", self.signature, self.error)
    }
}

impl std::error::Error for Unconfirmed {}

/// Compute-unit limit requested by every crank tx. The scheduler reserves
/// this much CU budget before each tx.
pub const TX_CU_LIMIT: u32 = 1_400_000;

/// `CRANKER_MEASURE_CU=0` skips the extra `getTransaction` per crank.
fn measure_cu() -> bool {
    std::env::var("CRANKER_MEASURE_CU").ok().as_deref() != Some("0")
}

fn cu_ix() -> Instruction {
    ComputeBudgetInstruction::set_compute_unit_limit(TX_CU_LIMIT)
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

    #[test]
    fn blockhash_cache_reuses_expires_and_invalidates() {
        let rpc = RpcClient::new_mock("succeeds".to_string());
        let cache = BlockhashCache::default();
        let hash = cache.get(&rpc).unwrap();
        let failing_rpc = RpcClient::new_mock("fails".to_string());
        assert_eq!(cache.get(&failing_rpc).unwrap(), hash);
        cache.invalidate();
        assert!(cache.get(&failing_rpc).is_err());
        *cache.0.lock().unwrap() = Some((hash, std::time::Instant::now() - std::time::Duration::from_secs(20)));
        assert!(cache.get(&failing_rpc).is_err());
        assert_eq!(cache.get(&rpc).unwrap(), hash);
    }

    #[test]
    fn processed_only_error_times_out_without_a_definitive_error() {
        use solana_client::rpc_request::RpcRequest;
        use solana_sdk::{instruction::InstructionError, transaction::TransactionError};
        use solana_transaction_status::{TransactionStatus, TransactionConfirmationStatus};
        let payer = Keypair::new();
        let tx = Transaction::new_signed_with_payer(&[], Some(&payer.pubkey()), &[&payer], solana_sdk::hash::Hash::default());
        let error = TransactionError::InstructionError(3, InstructionError::Custom(22));
        let status = TransactionStatus { slot: 1, confirmations: Some(0), status: Err(error.clone()),
            err: Some(error), confirmation_status: Some(TransactionConfirmationStatus::Processed) };
        let mocks = std::collections::HashMap::from([(RpcRequest::GetSignatureStatuses,
            serde_json::json!({"context": {"slot": 1}, "value": [status]}))]);
        let rpc = RpcClient::new_mock_with_mocks("succeeds".to_string(), mocks);
        let error = send_and_confirm_with_timeout(&rpc, &tx, std::time::Duration::ZERO).unwrap_err();
        assert!(matches!(error.kind(), ClientErrorKind::Custom(_)));
        assert!(error.to_string().contains("transaction not confirmed"));
        assert_eq!(wrapper_error_name_for_crank(&error.to_string(), 3, true), None);
    }

    #[test]
    fn confirmation_preserves_executed_custom_error() {
        use solana_client::rpc_request::RpcRequest;
        use solana_sdk::{instruction::InstructionError, transaction::TransactionError};
        use solana_transaction_status::{TransactionStatus, TransactionConfirmationStatus};
        let payer = Keypair::new();
        let tx = Transaction::new_signed_with_payer(&[], Some(&payer.pubkey()), &[&payer], solana_sdk::hash::Hash::default());
        let rpc = RpcClient::new_mock("succeeds".to_string());
        assert_eq!(send_and_confirm(&rpc, &tx).unwrap(), tx.signatures[0]);
        let error = TransactionError::InstructionError(3, InstructionError::Custom(22));
        let status = TransactionStatus { slot: 1, confirmations: Some(1), status: Err(error.clone()),
            err: Some(error), confirmation_status: Some(TransactionConfirmationStatus::Confirmed) };
        let mocks = std::collections::HashMap::from([(RpcRequest::GetSignatureStatuses,
            serde_json::json!({"context": {"slot": 1}, "value": [status]}))]);
        let rpc = RpcClient::new_mock_with_mocks("succeeds".to_string(), mocks);
        let error = send_and_confirm(&rpc, &tx).unwrap_err();
        assert!(matches!(error.kind(), ClientErrorKind::TransactionError(_)));
        assert_eq!(wrapper_error_name_for_crank(&error.to_string(), 3, true), Some("EngineNonProgress"));
    }

    fn reject(name: Option<&'static str>) -> CrankOutcome {
        CrankOutcome::HardReject {
            error: "x".into(),
            error_name: name,
            program_cu: None,
            tx_cu: None,
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
            market_bytes_diff: None,
            program_cu: None,
            tx_cu: None,
            unreconciled: None,
        };
        assert_eq!(ok(false).class(), OutcomeClass::Idle);
        assert_eq!(ok(true).class(), OutcomeClass::Progress);
    }

    /// Review finding: the Resolved route had no account-meta test. The
    /// wrapper's close_resolved branch reads exactly this layout.
    #[test]
    fn resolved_route_account_layout() {
        let (program, payer, market, target) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let (owner, mint, vault, vault_auth) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let route = CrankRoute::Resolved { owner, mint, vault, vault_authority: vault_auth };
        let hint = FetchedObservation { asset_index: 2, oracle_accounts: vec![] };
        let (ixs, idx) = build_crank_instructions(&program, &payer, &market, &target, &route, &[hint], 9, true);
        assert_eq!(idx, 3, "cu, heap, ATA create, crank");
        assert_eq!(ixs[2].program_id, spl_associated_token_account::ID);
        let crank = &ixs[idx];
        assert_eq!(crank.program_id, program);
        let keys: Vec<_> = crank.accounts.iter().map(|m| (m.pubkey, m.is_signer, m.is_writable)).collect();
        let owner_ata = spl_associated_token_account::get_associated_token_address(&owner, &mint);
        assert_eq!(
            keys,
            vec![
                (owner, false, false),
                (market, false, true),
                (target, false, true),
                (owner_ata, false, true),
                (vault, false, true),
                (vault_auth, false, false),
                (spl_token::ID, false, false),
            ]
        );
        // Payer is not a signer of the crank unless it owns the portfolio.
        let route_self = CrankRoute::Resolved { owner: payer, mint, vault, vault_authority: vault_auth };
        let (ixs, idx) = build_crank_instructions(&program, &payer, &market, &target, &route_self, &[], 9, true);
        assert!(ixs[idx].accounts[0].is_signer);
        // Wire: one oracle-less hint for asset 2.
        let (ixs, idx) = build_crank_instructions(
            &program, &payer, &market, &target, &route,
            &[FetchedObservation { asset_index: 2, oracle_accounts: vec![] }], 9, true,
        );
        assert_eq!(&ixs[idx].data[9..], &[1, 2, 0, 0]);
    }

    #[test]
    fn live_route_account_layout() {
        let (program, payer, market, target, reward, oracle) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let route = CrankRoute::Live { reward_portfolio: Some(&reward) };
        let obs = FetchedObservation { asset_index: 0, oracle_accounts: vec![oracle] };
        let (ixs, idx) = build_crank_instructions(&program, &payer, &market, &target, &route, &[obs], 1, false);
        assert_eq!(idx, 2);
        let keys: Vec<_> = ixs[idx].accounts.iter().map(|m| m.pubkey).collect();
        assert_eq!(keys, vec![payer, market, target, oracle, reward]);
        assert!(ixs[idx].accounts[0].is_signer);
    }

    #[test]
    fn pairing_duplicates_only_live_cranks() {
        let program = Pubkey::new_unique();
        let payer = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let target = Pubkey::new_unique();
        let live = CrankRoute::Live {
            reward_portfolio: None,
        };
        let (single, idx) =
            build_crank_instructions(&program, &payer, &market, &target, &live, &[], 7, false);
        let (paired, paired_idx) =
            build_crank_instructions(&program, &payer, &market, &target, &live, &[], 7, true);
        assert_eq!(idx, 2);
        assert_eq!(paired_idx, idx);
        assert_eq!(single.len(), 3);
        assert_eq!(paired.len(), 4);
        assert_eq!(&paired[..single.len()], single.as_slice());
        assert_eq!(paired[idx], paired[idx + 1]);
        let resolved = CrankRoute::Resolved {
            owner: payer,
            mint: Pubkey::new_unique(),
            vault: Pubkey::new_unique(),
            vault_authority: Pubkey::new_unique(),
        };
        let (off, idx) =
            build_crank_instructions(&program, &payer, &market, &target, &resolved, &[], 7, false);
        let (on, _) =
            build_crank_instructions(&program, &payer, &market, &target, &resolved, &[], 7, true);
        assert_eq!(idx, 3);
        assert_eq!(off.len(), 4);
        assert_eq!(on, off);
    }

    #[test]
    fn fallback_requires_preflight_rejection_and_second_instruction_attribution() {
        let ixs = vec![cu_ix(), heap_ix(), cu_ix(), cu_ix()];
        for suffix in [
            "custom program error: 0x16",
            "Computational budget exceeded",
        ] {
            let msg = format!("RPC response error -32002: Transaction simulation failed: Error processing Instruction 3: {suffix} [3 log messages]");
            assert!(should_fallback_to_single(true, &msg, 2));
            assert!(!should_fallback_to_single(false, &msg, 2));
            assert!(!should_fallback_to_single(true, &msg, 3));
            assert_eq!(single_crank_retry_instructions(&ixs, 2, true, &msg), Some(ixs[..3].to_vec()));
            assert_eq!(single_crank_retry_instructions(&ixs, 2, false, &msg), None);
            assert_eq!(single_crank_retry_instructions(&ixs, 3, true, &msg), None);
        }
        for msg in [
            "custom program error: 0x16",
            "Error processing Instruction x: error",
            "Error processing Instruction 30: error",
            "Error processing Instruction 1: error",
        ] {
            assert!(!should_fallback_to_single(true, msg, 2), "{msg}");
        }
    }

    #[test]
    fn paired_on_chain_second_crank_nonprogress_is_idle_without_retry() {
        for ix in [2, 3] {
            let msg = format!("Error processing Instruction {ix}: custom program error: 0x16");
            let error_name = wrapper_error_name_for_crank(&msg, 2, true);
            assert_eq!(error_name, Some("EngineNonProgress"));
            assert_eq!(reject(error_name).class(), OutcomeClass::Idle);
            assert!(!should_fallback_to_single(false, &msg, 2));
        }
        let second = "Error processing Instruction 3: custom program error: 0x16";
        assert_eq!(wrapper_error_name_for_crank(second, 2, false), None);
        let other = "Error processing Instruction 1: custom program error: 0x16";
        assert_eq!(wrapper_error_name_for_crank(other, 2, true), None);
    }

    #[test]
    fn error_names_only_for_the_crank_instruction() {
        let ours = "Error processing Instruction 3: custom program error: 0x16";
        let ata = "Error processing Instruction 2: custom program error: 0x1";
        assert_eq!(wrapper_error_name_for_ix(ours, 3), Some("EngineNonProgress"));
        assert_eq!(wrapper_error_name_for_ix(ata, 3), None);
        // No instruction index in the message: fall back to the code.
        assert_eq!(wrapper_error_name_for_ix("custom program error: 0x13", 3), Some("EngineStale"));
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
    fn program_cu_sums_top_level_invocations_without_nested_double_counting() {
        let pid = Pubkey::new_unique();
        let logs = vec![
            format!("Program {pid} invoke [1]"),
            format!("Program {pid} invoke [2]"),
            format!("Program {pid} consumed 100 of 1000 compute units"),
            format!("Program {pid} success"),
            format!("Program {pid} consumed 300 of 1000 compute units"),
            format!("Program {pid} success"),
            format!("Program {pid} invoke [1]"),
            format!("Program {pid} consumed 200 of 700 compute units"),
            format!("Program {pid} success"),
        ];
        assert_eq!(program_cu_from_logs(&pid, &logs), Some(500));
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

/// Error text with every http(s) URL replaced by its host: RPC transport
/// errors quote the request URL, and paid endpoints carry a key in it. A URL
/// runs to the next whitespace or double quote (commas, quotes and
/// parentheses are legal inside one, so they don't end it).
pub fn scrub_urls(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(i) = rest.find("http") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(after) = tail.strip_prefix("https://").or_else(|| tail.strip_prefix("http://")) else {
            out.push_str("http");
            rest = &tail[4..];
            continue;
        };
        let end = tail.find(|c: char| c.is_whitespace() || c == '"').unwrap_or(tail.len());
        let authority = after[..end - (tail.len() - after.len())].split(['/', '?', '#']).next().unwrap_or("");
        let host = authority.rsplit('@').next().unwrap_or("");
        out.push_str(&tail[..tail.len() - after.len()]);
        out.push_str(host);
        out.push_str("/[redacted]");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod scrub_tests {
    use super::scrub_urls;

    #[test]
    fn urls_keep_only_their_host() {
        for (input, secret) in [
            ("error sending request for url (https://rpc.example.com/SECRETKEY?api-key=X): timed out", "SECRET"),
            ("https://rpc.example.com/?api-key=abc,DEMO_SECRET failed", "DEMO_SECRET"),
            ("https://user,PASSWORD@rpc.example.com/path x", "PASSWORD"),
            ("'https://rpc.example.com/a'b(SECRET)' y", "SECRET"),
        ] {
            let m = scrub_urls(input);
            assert!(!m.contains(secret), "{input} -> {m}");
            assert!(m.contains("rpc.example.com/[redacted]"), "{input} -> {m}");
        }
        assert_eq!(scrub_urls("no url here; http only"), "no url here; http only");
        // Scrubbed before serializing, an error with a quoted URL stays valid JSON.
        let line = serde_json::json!({ "error": scrub_urls(r#"send "https://rpc.example.com/KEY" failed"#) }).to_string();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(!v["error"].as_str().unwrap().contains("KEY"));
    }
}

#[cfg(test)]
mod send_gate_tests {
    use super::*;

    #[test]
    fn send_gate_keeps_an_unseen_send_pending_until_the_cap() {
        // Unreachable RPC: the pending send is never seen, its blockhash never checked.
        let rpc = RpcClient::new("http://127.0.0.1:9".to_string());
        let tx = Transaction { signatures: vec![Signature::default()], ..Default::default() };
        let mut gate = SendGate { pending: None, last_send: None };
        assert!(gate.try_settle(&rpc), "nothing pending");
        gate.set_pending(&tx);
        assert!(!gate.try_settle(&rpc), "an unseen send may still land");
        let Some(old) = std::time::Instant::now().checked_sub(SEND_GATE_PENDING_MAX) else { return };
        gate.pending = Some((Signature::default(), tx.message.recent_blockhash, old));
        assert!(gate.try_settle(&rpc), "past the cap it counts as dropped");
        assert!(gate.pending.is_none());
    }
}
