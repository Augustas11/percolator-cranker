import { Buffer } from "buffer";
import { Connection, Keypair, PublicKey, TransactionMessage, VersionedTransaction } from "@solana/web3.js";
import type { TransactionInstruction } from "@solana/web3.js";
import type { DevnetConfig } from "./config";
import { effectiveAbsQ, parseMarket, parsePortfolio } from "./accounts";
import { layout } from "./layout";
import { computeBudgetIxs, crankIx, createPortfolioIxs, depositIx, tradeCpiIx, withdrawIx } from "./instructions";
import { ata, matcherDelegate, vault, vaultAuthority } from "./pda";

const MAX_CRANKS = 8;

export async function findPortfolio(conn: Connection, programId: PublicKey, market: { marketGroupId: Uint8Array }, owner: PublicKey): Promise<PublicKey | null> {
  const accounts = await conn.getProgramAccounts(programId, { filters: [
    { dataSize: layout.PORTFOLIO_ACCOUNT_LEN },
    { memcmp: { offset: layout.HEADER_LEN + layout["pf.provenance_header"], bytes: new PublicKey(market.marketGroupId).toBase58() } },
    { memcmp: { offset: layout.HEADER_LEN + layout["pf.owner"], bytes: owner.toBase58() } },
  ] });
  return accounts[0]?.pubkey ?? null;
}

export async function positionedPortfolios(conn: Connection, programId: PublicKey, market: { marketGroupId: Uint8Array }): Promise<PublicKey[]> {
  const accounts = await conn.getProgramAccounts(programId, { filters: [
    { dataSize: layout.PORTFOLIO_ACCOUNT_LEN },
    { memcmp: { offset: layout.HEADER_LEN + layout["pf.provenance_header"], bytes: new PublicKey(market.marketGroupId).toBase58() } },
  ] });
  return accounts.filter(({ account }) => parsePortfolio(account.data).legs.some(leg => leg.assetIndex === 0 && leg.basisPosQ !== 0n))
    .map(({ pubkey }) => pubkey);
}

export async function withFreshCrank(
  conn: Connection, payer: PublicKey, programId: PublicKey, market: PublicKey,
  portfolios: PublicKey[], readAccounts: PublicKey[],
): Promise<{ ixs: TransactionInstruction[]; accounts: Uint8Array[] }> {
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
    if (result.value.err) continue;
    // Only the refreshes that are kept have to fit, not every positioned portfolio.
    if (kept.length === MAX_CRANKS) throw new Error(`More than ${MAX_CRANKS} portfolios need a market refresh; they do not fit in one transaction`);
    kept.push(crank);
  }
  const ixs = [...budget, ...kept];
  if (!kept.length) {
    const accounts = await conn.getMultipleAccountsInfo(readAccounts);
    return { ixs, accounts: accounts.map(account => {
      if (!account) throw new Error("Market refresh: account not found");
      return account.data;
    }) };
  }
  const tx = new VersionedTransaction(new TransactionMessage({
    payerKey: payer, recentBlockhash: blockhash, instructions: ixs,
  }).compileToV0Message());
  const result = await conn.simulateTransaction(tx, {
    sigVerify: false, replaceRecentBlockhash: true,
    accounts: { encoding: "base64", addresses: readAccounts.map(key => key.toBase58()) },
  });
  if (result.value.err) throw new Error(`Market refresh simulation failed: ${JSON.stringify(result.value.err)}`);
  const accounts = result.value.accounts;
  if (!accounts || accounts.length !== readAccounts.length) throw new Error("Market refresh: accounts missing");
  return { ixs, accounts: accounts.map(account => {
    if (!account) throw new Error("Market refresh: account not found");
    return Buffer.from(account.data[0], "base64");
  }) };
}

async function readPortfolio(conn: Connection, portfolio: PublicKey) {
  const account = await conn.getAccountInfo(portfolio);
  if (!account) throw new Error("Portfolio account not found");
  return parsePortfolio(account.data);
}

async function crankSet(conn: Connection, cfg: DevnetConfig, first: PublicKey[]) {
  const marketAccount = await conn.getAccountInfo(cfg.market);
  if (!marketAccount) throw new Error("Market account not found");
  const positioned = await positionedPortfolios(conn, cfg.programId, parseMarket(marketAccount.data));
  return [...new Set([...first, ...positioned].map(key => key.toBase58()))].map(key => new PublicKey(key));
}

export async function buildDeposit(conn: Connection, cfg: DevnetConfig, owner: PublicKey, portfolio: PublicKey, amountAtoms: bigint) {
  const pf = await readPortfolio(conn, portfolio);
  return [...computeBudgetIxs(), depositIx(cfg.programId, owner, cfg.market, portfolio,
    ata(owner, cfg.mint), vault(cfg.programId, cfg.market, cfg.mint),
    { portfolioId: pf.portfolioId, expectedSequence: pf.sequence, amount: amountAtoms })];
}

export async function buildWithdraw(conn: Connection, cfg: DevnetConfig, owner: PublicKey, portfolio: PublicKey, amountAtoms: bigint) {
  const portfolios = await crankSet(conn, cfg, [portfolio]);
  const { ixs, accounts } = await withFreshCrank(conn, owner, cfg.programId, cfg.market, portfolios, [portfolio]);
  const pf = parsePortfolio(accounts[0]);
  return [...ixs, withdrawIx(cfg.programId, owner, cfg.market, portfolio,
    ata(owner, cfg.mint), vault(cfg.programId, cfg.market, cfg.mint), vaultAuthority(cfg.programId, cfg.market),
    { portfolioId: pf.portfolioId, expectedSequence: pf.sequence, amount: amountAtoms })];
}

export async function buildTrade(
  conn: Connection, cfg: DevnetConfig, owner: PublicKey, portfolio: PublicKey,
  sizeQ: bigint, limitPrice: bigint, feeBps: bigint,
) {
  return buildTradeIxs(conn, cfg, owner, portfolio, () => ({ sizeQ, limitPrice }), feeBps);
}

export async function buildClose(
  conn: Connection, cfg: DevnetConfig, owner: PublicKey, portfolio: PublicKey,
  slippageBps: bigint, feeBps: bigint, maxSizeQ?: bigint,
) {
  return buildTradeIxs(conn, cfg, owner, portfolio, (pf, market) => {
    const leg = pf.legs.find(leg => leg.assetIndex === 0 && leg.marketId === market.asset0.marketId);
    let q = leg ? effectiveAbsQ(market, leg) : 0n;
    if (!leg || q === 0n) throw new Error("no open position");
    if (maxSizeQ !== undefined) q = q > maxSizeQ ? maxSizeQ : q;
    const price = market.asset0.effectivePrice;
    const slippage = price * slippageBps / 10000n;
    return { sizeQ: leg.side === "long" ? -q : q, limitPrice: leg.side === "long" ? price - slippage : price + slippage };
  }, feeBps);
}

async function buildTradeIxs(
  conn: Connection, cfg: DevnetConfig, owner: PublicKey, portfolio: PublicKey,
  order: (pf: ReturnType<typeof parsePortfolio>, market: ReturnType<typeof parseMarket>) => { sizeQ: bigint; limitPrice: bigint },
  feeBps: bigint,
) {
  if (!cfg.matcherProgram || !cfg.matcherContext) throw new Error("Trading is waiting on the matcher program (devnet PoC)");
  const portfolios = await crankSet(conn, cfg, [portfolio, cfg.lpPortfolio]);
  const { ixs, accounts } = await withFreshCrank(conn, owner, cfg.programId, cfg.market,
    portfolios, [portfolio, cfg.lpPortfolio, cfg.market]);
  const a = parsePortfolio(accounts[0]), b = parsePortfolio(accounts[1]);
  const market = parseMarket(accounts[2]);
  const { sizeQ, limitPrice } = order(a, market);
  // The wrapper refuses a TradeCpi fee under the market's trade_fee_base_bps, and markets
  // differ (12 bps on the first SOL-PERP market, 15 on COMPUTE-PERP), so never send less than the market's own.
  const fee = feeBps < market.config.tradeFeeBaseBps ? market.config.tradeFeeBaseBps : feeBps;
  const delegate = matcherDelegate(cfg.programId, cfg.market, cfg.lpPortfolio, b.owner, cfg.matcherProgram, cfg.matcherContext);
  return [...ixs, tradeCpiIx(cfg.programId, owner, cfg.market, portfolio, cfg.lpPortfolio,
    cfg.matcherProgram, cfg.matcherContext, delegate, {
      accountAPortfolioId: a.portfolioId, accountAPositionEpoch: a.positionEpoch,
      accountBPortfolioId: b.portfolioId, accountBPositionEpoch: b.positionEpoch, accountBMatcherSequence: b.sequence,
      assetIndex: 0, marketId: market.asset0.marketId, sizeQ, feeBps: fee, limitPrice, backingFeeCapBps: 0,
    })];
}

export async function buildCreatePortfolio(cfg: DevnetConfig, owner: PublicKey) {
  const portfolio = Keypair.generate();
  const ixs = await createPortfolioIxs(cfg.programId, owner, cfg.market, portfolio.publicKey);
  return { ixs: [...computeBudgetIxs(), ...ixs], portfolio };
}
