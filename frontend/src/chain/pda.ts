import { Buffer } from "buffer";
import { PublicKey, SystemProgram, TransactionInstruction } from "@solana/web3.js";

export const TOKEN_PROGRAM_ID = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
export const ASSOCIATED_TOKEN_PROGRAM_ID = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

export function vaultAuthority(programId: PublicKey, market: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync([new TextEncoder().encode("vault"), market.toBytes()], programId)[0];
}

export function matcherDelegate(
  programId: PublicKey, market: PublicKey, makerAccount: PublicKey, makerOwner: PublicKey,
  matcherProgram: PublicKey, matcherContext: PublicKey,
): PublicKey {
  return PublicKey.findProgramAddressSync([
    new TextEncoder().encode("matcher"), market.toBytes(), makerAccount.toBytes(), makerOwner.toBytes(),
    matcherProgram.toBytes(), matcherContext.toBytes(),
  ], programId)[0];
}

export function ata(owner: PublicKey, mint: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync(
    [owner.toBytes(), TOKEN_PROGRAM_ID.toBytes(), mint.toBytes()], ASSOCIATED_TOKEN_PROGRAM_ID,
  )[0];
}

export function vault(programId: PublicKey, market: PublicKey, mint: PublicKey): PublicKey {
  return ata(vaultAuthority(programId, market), mint);
}

export function createAtaIdempotentIx(payer: PublicKey, owner: PublicKey, mint: PublicKey): TransactionInstruction {
  return new TransactionInstruction({
    programId: ASSOCIATED_TOKEN_PROGRAM_ID,
    data: Buffer.from([1]),
    keys: [
      { pubkey: payer, isSigner: true, isWritable: true },
      { pubkey: ata(owner, mint), isSigner: false, isWritable: true },
      { pubkey: owner, isSigner: false, isWritable: false },
      { pubkey: mint, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    ],
  });
}
