import { Connection, Keypair, PublicKey, TransactionMessage, VersionedTransaction } from "@solana/web3.js";
import type { TransactionInstruction } from "@solana/web3.js";
import type { DevnetConfig } from "./config";
import { parseMarket, parsePortfolio } from "./accounts";
import { layout } from "./layout";
import { computeBudgetIxs, crankIx, createPortfolioIxs, depositIx, tradeCpiIx, withdrawIx } from "./instructions";
import { ata, matcherDelegate, vault, vaultAuthority } from "./pda";

export async function findPortfolio(conn: Connection, programId: PublicKey, owner: PublicKey): Promise<PublicKey | null> {
  const accounts = await conn.getProgramAccounts(programId, { filters: [
    { dataSize: layout.PORTFOLIO_ACCOUNT_LEN },
    { memcmp: { offset: layout.HEADER_LEN + layout["pf.owner"], bytes: owner.toBase58() } },
  ] });
  return accounts[0]?.pubkey ?? null;
}

export async function withFreshCrank(
  conn: Connection, payer: PublicKey, programId: PublicKey, market: PublicKey,
  portfolios: PublicKey[], ix: TransactionInstruction[],
): Promise<TransactionInstruction[]> {
  const kept: TransactionInstruction[] = [];
  const budget = computeBudgetIxs();
  const nowSlot = await conn.getSlot();
  const { blockhash } = await conn.getLatestBlockhash();
  for (const portfolio of portfolios) {
    const crank = crankIx(programId, payer, market, portfolio, {
      nowSlot, observations: [{ assetIndex: 0, oracleAccounts: 0 }],
    });
    const tx = new VersionedTransaction(new TransactionMessage({
      payerKey: payer, recentBlockhash: blockhash, instructions: [...budget, ...kept, crank],
    }).compileToV0Message());
    const result = await conn.simulateTransaction(tx, { sigVerify: false, replaceRecentBlockhash: true });
    if (!result.value.err) kept.push(crank);
  }
  return [...budget, ...kept, ...ix];
}

async function readPortfolio(conn: Connection, portfolio: PublicKey) {
  const account = await conn.getAccountInfo(portfolio);
  if (!account) throw new Error("Portfolio account not found");
  return parsePortfolio(account.data);
}

export async function buildDeposit(conn: Connection, cfg: DevnetConfig, owner: PublicKey, portfolio: PublicKey, amountAtoms: bigint) {
  const pf = await readPortfolio(conn, portfolio);
  return [...computeBudgetIxs(), depositIx(cfg.programId, owner, cfg.market, portfolio,
    ata(owner, cfg.mint), vault(cfg.programId, cfg.market, cfg.mint),
    { portfolioId: pf.portfolioId, expectedSequence: pf.sequence, amount: amountAtoms })];
}

export async function buildWithdraw(conn: Connection, cfg: DevnetConfig, owner: PublicKey, portfolio: PublicKey, amountAtoms: bigint) {
  const cranks = await withFreshCrank(conn, owner, cfg.programId, cfg.market, [portfolio], []);
  const pf = await readPortfolio(conn, portfolio);
  return [...cranks, withdrawIx(cfg.programId, owner, cfg.market, portfolio,
    ata(owner, cfg.mint), vault(cfg.programId, cfg.market, cfg.mint), vaultAuthority(cfg.programId, cfg.market),
    { portfolioId: pf.portfolioId, expectedSequence: pf.sequence, amount: amountAtoms })];
}

export async function buildTrade(
  conn: Connection, cfg: DevnetConfig, owner: PublicKey, portfolio: PublicKey,
  sizeQ: bigint, limitPrice: bigint, feeBps: bigint,
) {
  if (!cfg.matcherProgram || !cfg.matcherContext) throw new Error("Trading is waiting on the matcher program (devnet PoC)");
  const cranks = await withFreshCrank(conn, owner, cfg.programId, cfg.market, [portfolio, cfg.lpPortfolio], []);
  const accounts = await conn.getMultipleAccountsInfo([portfolio, cfg.lpPortfolio, cfg.market]);
  if (accounts.some(account => !account)) throw new Error("Trade account not found");
  const a = parsePortfolio(accounts[0]!.data), b = parsePortfolio(accounts[1]!.data);
  const market = parseMarket(accounts[2]!.data);
  const delegate = matcherDelegate(cfg.programId, cfg.market, cfg.lpPortfolio, b.owner, cfg.matcherProgram, cfg.matcherContext);
  return [...cranks, tradeCpiIx(cfg.programId, owner, cfg.market, portfolio, cfg.lpPortfolio,
    cfg.matcherProgram, cfg.matcherContext, delegate, {
      accountAPortfolioId: a.portfolioId, accountAPositionEpoch: a.positionEpoch,
      accountBPortfolioId: b.portfolioId, accountBPositionEpoch: b.positionEpoch, accountBMatcherSequence: b.sequence,
      assetIndex: 0, marketId: market.asset0.marketId, sizeQ, feeBps, limitPrice, backingFeeCapBps: 0,
    })];
}

export async function buildCreatePortfolio(conn: Connection, cfg: DevnetConfig, owner: PublicKey) {
  const portfolio = Keypair.generate();
  const ixs = await createPortfolioIxs(conn, cfg.programId, owner, cfg.market, portfolio.publicKey);
  return { ixs: [...computeBudgetIxs(), ...ixs], portfolio };
}
