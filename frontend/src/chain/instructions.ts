import { Buffer } from "buffer";
import { ComputeBudgetProgram, PublicKey, SystemProgram, TransactionInstruction } from "@solana/web3.js";
import type { AccountMeta } from "@solana/web3.js";
import { Writer } from "./bytes";
import type { Integer } from "./bytes";
import { layout } from "./layout";
import { TOKEN_PROGRAM_ID } from "./pda";

// Standard rent: 3480 lamports/byte-year * 2 years, including 128 bytes of overhead.
export const PORTFOLIO_LAMPORTS = (layout.PORTFOLIO_ACCOUNT_LEN + 128) * 6960;

export interface DepositArgs { portfolioId: Integer; expectedSequence: Integer; amount: Integer }
export type WithdrawArgs = DepositArgs;
export interface PermissionlessCrankArgs {
  nowSlot: Integer;
  observations: { assetIndex: number; oracleAccounts: number }[];
}
export interface TradeNoCpiArgs {
  accountAPortfolioId: Integer;
  accountAPositionEpoch: Integer;
  accountBPortfolioId: Integer;
  accountBPositionEpoch: Integer;
  assetIndex: number;
  marketId: Integer;
  sizeQ: Integer;
  execPrice: Integer;
  feeBps: Integer;
  backingFeeCapBps: number;
}
export interface ClosePortfolioArgs { portfolioId: Integer; expectedSequence: Integer; positionEpoch: Integer }
export interface TradeCpiArgs extends Omit<TradeNoCpiArgs, "execPrice"> {
  accountBMatcherSequence: Integer;
  limitPrice: Integer;
}
export interface CloseResolvedArgs { feeRatePerSlot: Integer }

export function encodeInitPortfolio(_args: Record<string, never> = {}): Uint8Array {
  return new Writer().u8(1).finish();
}
export function encodeDeposit(args: DepositArgs): Uint8Array {
  return new Writer().u8(3).u64(args.portfolioId).u64(args.expectedSequence).u128(args.amount).finish();
}
export function encodeWithdraw(args: WithdrawArgs): Uint8Array {
  return new Writer().u8(4).u64(args.portfolioId).u64(args.expectedSequence).u128(args.amount).finish();
}
export function encodePermissionlessCrank(args: PermissionlessCrankArgs): Uint8Array {
  const w = new Writer().u8(5).u64(args.nowSlot).u8(args.observations.length);
  for (const hint of args.observations) w.u16(hint.assetIndex).u8(hint.oracleAccounts);
  return w.finish();
}
export function encodeTradeNoCpi(args: TradeNoCpiArgs): Uint8Array {
  return new Writer().u8(6)
    .u64(args.accountAPortfolioId).u64(args.accountAPositionEpoch)
    .u64(args.accountBPortfolioId).u64(args.accountBPositionEpoch)
    .u16(args.assetIndex).u64(args.marketId).i128(args.sizeQ)
    .u64(args.execPrice).u64(args.feeBps).u16(args.backingFeeCapBps).finish();
}
export function encodeClosePortfolio(args: ClosePortfolioArgs): Uint8Array {
  return new Writer().u8(8).u64(args.portfolioId).u64(args.expectedSequence).u64(args.positionEpoch).finish();
}
export function encodeTradeCpi(args: TradeCpiArgs): Uint8Array {
  return new Writer().u8(10)
    .u64(args.accountAPortfolioId).u64(args.accountAPositionEpoch)
    .u64(args.accountBPortfolioId).u64(args.accountBPositionEpoch).u64(args.accountBMatcherSequence)
    .u16(args.assetIndex).u64(args.marketId).i128(args.sizeQ)
    .u64(args.feeBps).u64(args.limitPrice).u16(args.backingFeeCapBps).finish();
}
export function encodeCloseResolved(args: CloseResolvedArgs): Uint8Array {
  return new Writer().u8(30).u128(args.feeRatePerSlot).finish();
}

function meta(pubkey: PublicKey, isSigner = false, isWritable = true): AccountMeta {
  return { pubkey, isSigner, isWritable };
}
function instruction(programId: PublicKey, keys: AccountMeta[], data: Uint8Array): TransactionInstruction {
  return new TransactionInstruction({ programId, keys, data: Buffer.from(data) });
}

export function initPortfolioIx(programId: PublicKey, owner: PublicKey, market: PublicKey, portfolio: PublicKey): TransactionInstruction {
  return instruction(programId, [meta(owner, true), meta(market), meta(portfolio)], encodeInitPortfolio());
}

/** The caller must also sign the transaction with the new portfolio keypair. */
export function createPortfolioIxs(
  programId: PublicKey, owner: PublicKey, market: PublicKey, portfolio: PublicKey,
): [TransactionInstruction, TransactionInstruction] {
  const space = layout.PORTFOLIO_ACCOUNT_LEN;
  return [
    SystemProgram.createAccount({ fromPubkey: owner, newAccountPubkey: portfolio, space, lamports: PORTFOLIO_LAMPORTS, programId }),
    initPortfolioIx(programId, owner, market, portfolio),
  ];
}

export function depositIx(
  programId: PublicKey, owner: PublicKey, market: PublicKey, portfolio: PublicKey,
  ownerAta: PublicKey, vault: PublicKey, args: DepositArgs,
): TransactionInstruction {
  return instruction(programId, [meta(owner, true), meta(market), meta(portfolio), meta(ownerAta), meta(vault), meta(TOKEN_PROGRAM_ID, false, false)], encodeDeposit(args));
}
export function withdrawIx(
  programId: PublicKey, owner: PublicKey, market: PublicKey, portfolio: PublicKey,
  ownerAta: PublicKey, vault: PublicKey, vaultAuthority: PublicKey, args: WithdrawArgs,
): TransactionInstruction {
  return instruction(programId, [meta(owner, true), meta(market), meta(portfolio), meta(ownerAta), meta(vault), meta(vaultAuthority, false, false), meta(TOKEN_PROGRAM_ID, false, false)], encodeWithdraw(args));
}
export function crankIx(
  programId: PublicKey, payer: PublicKey, market: PublicKey, portfolio: PublicKey,
  args: PermissionlessCrankArgs, oracleAccounts: PublicKey[] = [],
): TransactionInstruction {
  return instruction(programId, [meta(payer, true), meta(market), meta(portfolio), ...oracleAccounts.map(key => meta(key, false, false))], encodePermissionlessCrank(args));
}
/** A is the taker; positive sizeQ makes A long. */
export function tradeNoCpiIx(
  programId: PublicKey, ownerA: PublicKey, ownerB: PublicKey, market: PublicKey,
  portfolioA: PublicKey, portfolioB: PublicKey, args: TradeNoCpiArgs,
): TransactionInstruction {
  return instruction(programId, [meta(ownerA, true), meta(ownerB, true), meta(market), meta(portfolioA), meta(portfolioB)], encodeTradeNoCpi(args));
}
export function closePortfolioIx(
  programId: PublicKey, owner: PublicKey, market: PublicKey, portfolio: PublicKey, args: ClosePortfolioArgs,
): TransactionInstruction {
  return instruction(programId, [meta(owner, true), meta(market), meta(portfolio)], encodeClosePortfolio(args));
}
export function tradeCpiIx(
  programId: PublicKey, signerA: PublicKey, market: PublicKey, portfolioA: PublicKey, portfolioB: PublicKey,
  matcherProgram: PublicKey, matcherContext: PublicKey, matcherDelegate: PublicKey, args: TradeCpiArgs,
  tail: { pubkey: PublicKey; isWritable?: boolean }[] = [],
): TransactionInstruction {
  if (tail.length > 32) throw new Error("TradeCpi supports at most 32 tail accounts");
  return instruction(programId, [
    meta(signerA, true), meta(market), meta(portfolioA), meta(portfolioB),
    meta(matcherProgram, false, false), meta(matcherContext), meta(matcherDelegate, false, false),
    ...tail.map(key => meta(key.pubkey, false, key.isWritable ?? false)),
  ], encodeTradeCpi(args));
}
export function closeResolvedIx(
  programId: PublicKey, owner: PublicKey, market: PublicKey, portfolio: PublicKey,
  ownerAta: PublicKey, vault: PublicKey, vaultAuthority: PublicKey, ownerIsSigner: boolean, args: CloseResolvedArgs,
): TransactionInstruction {
  return instruction(programId, [meta(owner, ownerIsSigner, false), meta(market), meta(portfolio), meta(ownerAta), meta(vault), meta(vaultAuthority, false, false), meta(TOKEN_PROGRAM_ID, false, false)], encodeCloseResolved(args));
}
export function computeBudgetIxs(): TransactionInstruction[] {
  return [
    ComputeBudgetProgram.setComputeUnitLimit({ units: 1_400_000 }),
    ComputeBudgetProgram.requestHeapFrame({ bytes: 128 * 1024 }),
  ];
}
