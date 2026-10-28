//! Transaction helpers for scenario actions (not for cranks: those go
//! through `submit::submit_crank` inside the cycle).
//!
//! Two shapes:
//! * `send` — one tx, confirmed before returning, blockhash-race retry.
//! * `send_batch` — many independent txs sent back-to-back, then confirmed
//!   together. Setup (40+ portfolios) and per-tick mark pushes would
//!   otherwise cost one confirmation round-trip each, which is most of the
//!   wall-clock budget of a 200-cycle run.

use anyhow::{anyhow, Result};
use percolator_prog::ix::Instruction as ProgInstruction;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    transaction::Transaction,
};
use std::time::{Duration, Instant};

use crate::submit::wrapper_error_name;

pub struct Chain<'a> {
    pub rpc: &'a RpcClient,
    pub program_id: Pubkey,
    pub payer: &'a Keypair,
}

/// A failed scenario tx: the RPC message plus the wrapper error name, if any.
#[derive(Debug, Clone)]
pub struct TxError {
    pub name: Option<&'static str>,
    pub msg: String,
}

impl TxError {
    fn from_msg(msg: String) -> Self {
        Self {
            name: wrapper_error_name(&msg),
            msg,
        }
    }

    pub fn is(&self, name: &str) -> bool {
        self.name == Some(name)
    }

    /// Short form for logs: wrapper name or first 160 chars.
    pub fn short(&self) -> String {
        match self.name {
            Some(n) => n.to_string(),
            None => self.msg.chars().take(160).collect(),
        }
    }
}

impl std::fmt::Display for TxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.short(), self.msg.lines().next().unwrap_or(""))
    }
}

impl std::error::Error for TxError {}

/// Upstream harness prelude: 1.4M CU, 256K heap (multi-asset cranks and
/// multi-ix txs need more than the 128K the single-crank path uses).
pub fn prelude() -> [Instruction; 2] {
    [
        ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
        ComputeBudgetInstruction::request_heap_frame(256 * 1024),
    ]
}

impl<'a> Chain<'a> {
    pub fn ix(&self, ix: ProgInstruction, accounts: Vec<AccountMeta>) -> Instruction {
        Instruction {
            program_id: self.program_id,
            accounts,
            data: ix.encode(),
        }
    }

    fn blockhash(&self) -> Result<Hash> {
        Ok(self
            .rpc
            .get_latest_blockhash_with_commitment(CommitmentConfig::processed())?
            .0)
    }

    fn build(&self, ixs: &[Instruction], signers: &[&Keypair], bh: Hash) -> Transaction {
        let mut all = prelude().to_vec();
        all.extend_from_slice(ixs);
        let mut s: Vec<&Keypair> = vec![self.payer];
        for k in signers {
            if k.pubkey() != self.payer.pubkey() {
                s.push(k);
            }
        }
        Transaction::new_signed_with_payer(&all, Some(&self.payer.pubkey()), &s, bh)
    }

    /// One tx, confirmed. Wrapper errors come back as `TxError` with a name.
    pub fn send(&self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<Signature, TxError> {
        let mut last = String::new();
        for attempt in 0..5 {
            let bh = self.blockhash().map_err(|e| TxError::from_msg(e.to_string()))?;
            let tx = self.build(ixs, signers, bh);
            match self.rpc.send_and_confirm_transaction(&tx) {
                Ok(sig) => return Ok(sig),
                Err(e) => {
                    last = format!("{e}");
                    if last.contains("Blockhash not found") && attempt < 4 {
                        std::thread::sleep(Duration::from_millis(400));
                        continue;
                    }
                    return Err(TxError::from_msg(last));
                }
            }
        }
        Err(TxError::from_msg(last))
    }

    /// Simulate only: `Ok(())` when the tx would succeed now.
    pub fn simulate(&self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<(), TxError> {
        let bh = self.blockhash().map_err(|e| TxError::from_msg(e.to_string()))?;
        let tx = self.build(ixs, signers, bh);
        let r = self
            .rpc
            .simulate_transaction(&tx)
            .map_err(|e| TxError::from_msg(e.to_string()))?;
        match r.value.err {
            None => Ok(()),
            Some(e) => {
                // Render like the send path so `wrapper_error_name` matches.
                let msg = match &e {
                    solana_sdk::transaction::TransactionError::InstructionError(
                        _,
                        solana_sdk::instruction::InstructionError::Custom(c),
                    ) => format!("simulation: custom program error: 0x{c:x}"),
                    other => format!("simulation: {other}"),
                };
                Err(TxError::from_msg(msg))
            }
        }
    }

    /// Send independent txs back-to-back, then wait for all of them.
    /// Results are in input order. Txs that touch the same writable account
    /// are fine (the bank serializes them) as long as none depends on
    /// another's *result* for its own arguments.
    pub fn send_batch(
        &self,
        txs: &[(Vec<Instruction>, Vec<&Keypair>)],
    ) -> Vec<Result<Signature, TxError>> {
        let mut out: Vec<Result<Signature, TxError>> = Vec::with_capacity(txs.len());
        let bh = match self.blockhash() {
            Ok(b) => b,
            Err(e) => return txs.iter().map(|_| Err(TxError::from_msg(e.to_string()))).collect(),
        };
        let mut bh = bh;
        for (ixs, signers) in txs {
            // Same blockhash-race retry as `send`: the test validator's RPC
            // occasionally hands out a hash its bank has not seen yet.
            let mut r = Err(TxError::from_msg(String::new()));
            for attempt in 0..5 {
                let tx = self.build(ixs, signers, bh);
                r = self.rpc.send_transaction(&tx).map_err(|e| TxError::from_msg(format!("{e}")));
                match &r {
                    Err(e) if e.msg.contains("Blockhash not found") && attempt < 4 => {
                        std::thread::sleep(Duration::from_millis(400));
                        if let Ok(b) = self.blockhash() {
                            bh = b;
                        }
                    }
                    _ => break,
                }
            }
            out.push(r);
        }
        let pending: Vec<(usize, Signature)> = out
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().ok().map(|s| (i, *s)))
            .collect();
        let mut done = vec![false; out.len()];
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let open: Vec<&(usize, Signature)> = pending.iter().filter(|(i, _)| !done[*i]).collect();
            if open.is_empty() {
                break;
            }
            if Instant::now() > deadline {
                for (i, _) in open {
                    out[*i] = Err(TxError::from_msg("batch tx not confirmed within 45s".into()));
                }
                break;
            }
            for chunk in open.chunks(200) {
                let sigs: Vec<Signature> = chunk.iter().map(|(_, s)| *s).collect();
                let Ok(st) = self.rpc.get_signature_statuses(&sigs) else {
                    continue;
                };
                for ((i, _), status) in chunk.iter().zip(st.value) {
                    let Some(status) = status else { continue };
                    if let Some(err) = &status.err {
                        let msg = match err {
                            solana_sdk::transaction::TransactionError::InstructionError(
                                _,
                                solana_sdk::instruction::InstructionError::Custom(c),
                            ) => format!("custom program error: 0x{c:x}"),
                            other => format!("{other}"),
                        };
                        out[*i] = Err(TxError::from_msg(msg));
                        done[*i] = true;
                    } else if status.satisfies_commitment(CommitmentConfig::confirmed()) {
                        done[*i] = true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        out
    }

    /// `send_batch`, failing on the first error with `what` as context.
    pub fn send_batch_all(
        &self,
        what: &str,
        txs: &[(Vec<Instruction>, Vec<&Keypair>)],
    ) -> Result<()> {
        for (i, r) in self.send_batch(txs).into_iter().enumerate() {
            r.map_err(|e| anyhow!("{what} #{i}: {e}"))?;
        }
        Ok(())
    }
}

/// SPL token amount of `ata`, 0 when the account does not exist.
pub fn token_amount(rpc: &RpcClient, ata: &Pubkey) -> u64 {
    use solana_sdk::program_pack::Pack;
    rpc.get_account_data(ata)
        .ok()
        .and_then(|d| spl_token::state::Account::unpack(&d).ok())
        .map(|a| a.amount)
        .unwrap_or(0)
}
