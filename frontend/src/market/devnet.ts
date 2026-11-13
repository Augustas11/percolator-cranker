import { Connection, SendTransactionError, Transaction } from "@solana/web3.js";
import type { Keypair, PublicKey, TransactionInstruction } from "@solana/web3.js";
import { PERP } from "./types";
import type { Fill, MarketClient, MarketSnapshot, Side } from "./types";
import type { DevnetConfig } from "../chain/config";
import { effectiveAbsQ, parseMarket, parsePortfolio } from "../chain/accounts";
import { ata, createAtaIdempotentIx } from "../chain/pda";
import { buildClose, buildCreatePortfolio, buildDeposit, buildTrade, buildWithdraw, findPortfolio } from "../chain/tx";
import { getWallet } from "../chain/wallet";
import { liquidityFrom, parseMatcher } from "../chain/matcher";
import { ChainError, toChainError } from "../chain/errors";
import type { Wallet } from "../chain/wallet";

function atoms(value: number, name: string) {
  const rounded = Math.round(value * 1e6);
  if (!Number.isFinite(value) || value <= 0 || !Number.isSafeInteger(rounded) || rounded <= 0) {
    throw new Error(`Enter ${name} above zero, with at most 6 decimals.`);
  }
  return BigInt(rounded);
}

export class DevnetMarketClient implements MarketClient {
  readonly tradingReady: boolean;
  /** The engine's funding is not read; the snapshot's funding fields are placeholders. */
  private matcherReadAt = 0;
  private conn: Connection;
  private wallet?: Wallet;
  private portfolio?: PublicKey;
  private listeners = new Set<(s: MarketSnapshot) => void>();
  private timer?: ReturnType<typeof setTimeout>;
  private disposed = false;
  private snapshot: MarketSnapshot = {
    market: { symbol: PERP.name, collateral: "USDC", maxLeverage: NaN, maintenanceMarginRatio: NaN },
    mark: { price: NaN, ts: Date.now() },
    funding: { rate: 0, nextFundingTs: Date.now() },
    portfolio: {
      owner: null, collateral: 0, legs: [], realizedPnl: NaN,
      health: { equity: 0, initialMarginUsed: 0, maintenanceMargin: 0, freeCollateral: 0, ratio: Infinity, status: "healthy" },
    },
    fills: [],
  };

  constructor(private cfg: DevnetConfig) {
    this.conn = new Connection(cfg.rpcUrl, "confirmed");
    this.tradingReady = !!(cfg.matcherProgram && cfg.matcherContext);
    // A failed poll keeps the last snapshot; wallet actions report RPC errors to the UI.
    void this.poll();
  }

  getSnapshot = () => this.snapshot;
  subscribe = (listener: (s: MarketSnapshot) => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  private async readMatcher() {
    if (!this.cfg.matcherContext || Date.now() - this.matcherReadAt < 10_000) return;
    this.matcherReadAt = Date.now();
    try {
      const [account, slot] = await Promise.all([this.conn.getAccountInfo(this.cfg.matcherContext), this.conn.getSlot("confirmed").catch(() => undefined)]);
      if (!account || this.disposed) return;
      this.snapshot = { ...this.snapshot, liquidity: liquidityFrom(parseMatcher(account.data), slot) };
      for (const listener of this.listeners) listener(this.snapshot);
    } catch { /* keep the last read; the Quotes panel says when it has none */ }
  }

  private async poll() {
    void this.readMatcher();
    try { await this.refresh(); } catch {}
    finally {
      if (!this.disposed) this.timer = setTimeout(() => { void this.poll(); }, 2000);
    }
  }

  private async refresh() {
    const accounts = await this.conn.getMultipleAccountsInfo([this.cfg.market, ...(this.portfolio ? [this.portfolio] : [])]);
    if (!accounts[0]) throw new Error("Market account not found");
    const market = parseMarket(accounts[0].data);
    if (this.portfolio && !accounts[1]) throw new Error("Portfolio account not found");
    const pf = accounts[1] ? parsePortfolio(accounts[1].data) : null;
    const price = Number(market.asset0.effectivePrice) / 1e6;
    const leg = pf?.legs.find(leg => leg.assetIndex === 0 && leg.marketId === market.asset0.marketId);
    const size = Number(leg ? effectiveAbsQ(market, leg) : 0n) / 1e6;
    // The engine counts only realizable positive PnL (v16.rs:12353-12378).
    const collateral = pf ? Number(pf.capital + (pf.pnl < 0n ? pf.pnl : 0n)) / 1e6 : 0;
    const mmRatio = Number(market.maintenanceMarginBps) / 10000;
    const imBps = Number(market.initialMarginBps);
    // Engine health (floors, lag penalty, v16.rs:4095-4139) is not reproduced client-side.
    const maintenanceMargin = size ? NaN : 0;
    const initialMarginUsed = size ? NaN : 0;
    const ratio = size ? NaN : Infinity;
    if (this.disposed) return;
    const now = Date.now();
    this.snapshot = {
      market: { symbol: PERP.name, collateral: "USDC", maxLeverage: imBps > 0 ? 10000 / imBps : NaN, initialMarginRatio: imBps > 0 ? imBps / 10000 : NaN, maintenanceMarginRatio: mmRatio,
        // The fee buildTrade sends: 12 bps, floored at the market's own base fee (chain/tx.ts).
        feeBps: Math.max(12, Number(market.config.tradeFeeBaseBps)) },
      mark: { price, ts: now }, funding: { rate: 0, nextFundingTs: now },
      portfolio: {
        owner: this.wallet?.publicKey.toBase58() ?? null, collateral, realizedPnl: NaN,
        legs: leg && size ? [{ side: leg.side, size, entryPrice: NaN, unrealizedPnl: NaN }] : [],
        health: {
          equity: collateral, initialMarginUsed, maintenanceMargin, freeCollateral: size ? NaN : collateral, ratio,
          status: size ? "unknown" : "healthy",
        },
      },
      fills: this.snapshot.fills,
      liquidity: this.snapshot.liquidity ?? null,
    };
    for (const listener of this.listeners) listener(this.snapshot);
  }

  async connect() {
    this.wallet = await getWallet();
    const account = await this.conn.getAccountInfo(this.cfg.market);
    if (!account) throw new Error("Market account not found");
    this.portfolio = await findPortfolio(this.conn, this.cfg.programId, parseMarket(account.data), this.wallet.publicKey) ?? undefined;
    await this.refresh();
    return this.wallet.publicKey.toBase58();
  }

  private connected() {
    if (!this.wallet) throw new Error("Connect a wallet first.");
    return this.wallet;
  }

  private requirePortfolio() {
    if (!this.portfolio) throw new Error("Deposit test USDC first.");
    return this.portfolio;
  }

  private async send(ixs: TransactionInstruction[], signers: Keypair[] = [], fill?: Pick<Fill, "side" | "size">, onConfirmed?: () => void) {
    const wallet = this.connected();
    const price = this.snapshot.mark.price;
    let signature: string;
    try {
      const latest = await this.conn.getLatestBlockhash("confirmed");
      const tx = new Transaction({ feePayer: wallet.publicKey, ...latest }).add(...ixs);
      if (signers.length) tx.partialSign(...signers);
      const signed = await wallet.signTransaction(tx);
      signature = await this.conn.sendRawTransaction(signed.serialize());
      const result = await this.conn.confirmTransaction({ signature, ...latest }, "confirmed");
      if (result.value.err) {
        const landed = await this.conn.getTransaction(signature, { commitment: "confirmed", maxSupportedTransactionVersion: 0 }).catch(() => null);
        throw toChainError(new Error(JSON.stringify(result.value.err)), landed?.meta?.logMessages ?? [], this.programs());
      }
    } catch (err) {
      if (err instanceof ChainError) throw err;
      if (err instanceof SendTransactionError) {
        const logs = err.logs ?? await err.getLogs(this.conn).catch(() => []);
        throw toChainError(err, logs ?? [], this.programs());
      }
      throw toChainError(err, [], this.programs());
    }
    onConfirmed?.();
    if (fill) this.snapshot = { ...this.snapshot, fills: [
      { ...fill, id: signature, price, ts: Date.now(), mine: true, wallet: this.snapshot.portfolio.owner ?? undefined }, ...this.snapshot.fills,
    ].slice(0, 20) };
    await this.refresh().catch(() => {});
    return signature;
  }

  private programs() {
    return { engine: this.cfg.programId.toBase58(), matcher: this.cfg.matcherProgram?.toBase58() };
  }

  async walletBalances() {
    const wallet = this.connected();
    const [lamports, token] = await Promise.all([
      this.conn.getBalance(wallet.publicKey, "confirmed"),
      this.conn.getTokenAccountBalance(ata(wallet.publicKey, this.cfg.mint), "confirmed").then(r => Number(r.value.amount) / 1e6, () => null),
    ]);
    return { sol: lamports / 1e9, usdc: token };
  }

  async createTokenAccount() {
    const wallet = this.connected();
    await this.send([createAtaIdempotentIx(wallet.publicKey, wallet.publicKey, this.cfg.mint)]);
  }

  async deposit(usd: number) {
    const wallet = this.connected();
    const amount = atoms(usd, "an amount");
    // Say plainly when the wallet has no test USDC, before the wallet is asked to sign a transfer that would fail.
    const held = await this.conn.getTokenAccountBalance(ata(wallet.publicKey, this.cfg.mint), "confirmed").then(r => BigInt(r.value.amount), () => null);
    if (held === null || held === 0n) throw new Error("You have no test USDC yet: get some from the faucet.");
    if (held < amount) throw new Error(`You have ${(Number(held) / 1e6).toLocaleString("en-US", { maximumFractionDigits: 6 })} test USDC: deposit that much or less.`);
    if (!this.portfolio) {
      const created = await buildCreatePortfolio(this.cfg, wallet.publicKey);
      await this.send(created.ixs, [created.portfolio], undefined, () => { this.portfolio = created.portfolio.publicKey; });
    }
    const ixs = await buildDeposit(this.conn, this.cfg, wallet.publicKey, this.requirePortfolio(), amount);
    await this.send([createAtaIdempotentIx(wallet.publicKey, wallet.publicKey, this.cfg.mint), ...ixs]);
  }

  async withdraw(usd: number) {
    const wallet = this.connected();
    const amount = atoms(usd, "an amount");
    const ixs = await buildWithdraw(this.conn, this.cfg, wallet.publicKey, this.requirePortfolio(), amount);
    await this.send([createAtaIdempotentIx(wallet.publicKey, wallet.publicKey, this.cfg.mint), ...ixs]);
  }

  private limit(side: Side) {
    return atoms(this.snapshot.mark.price * (side === "long" ? 1.005 : 0.995), "a limit price");
  }

  private async trade(sizeQ: bigint) {
    const wallet = this.connected();
    const side = sizeQ > 0n ? "long" : "short";
    const ixs = await buildTrade(this.conn, this.cfg, wallet.publicKey, this.requirePortfolio(), sizeQ, this.limit(side), 12n);
    await this.send(ixs, [], { side, size: Number(sizeQ < 0n ? -sizeQ : sizeQ) / 1e6 });
  }

  async openPosition(side: Side, size: number, leverage: number) {
    this.connected();
    if (side !== "long" && side !== "short") throw new Error("invalid side");
    const q = atoms(size, "a size");
    await this.refresh();
    const max = this.snapshot.market.maxLeverage;
    if (!Number.isFinite(max) || max < 1) throw new Error("The market account has not been read yet. Try again in a moment.");
    if (!Number.isFinite(leverage) || leverage < 1 || leverage > max) throw new Error(`leverage must be between 1 and ${max}`);
    await this.trade(side === "long" ? q : -q);
  }

  async closePosition() {
    const wallet = this.connected();
    const ixs = await buildClose(this.conn, this.cfg, wallet.publicKey, this.requirePortfolio(), 50n, 12n);
    await this.send(ixs);
  }

  dispose() {
    this.disposed = true;
    clearTimeout(this.timer);
    this.listeners.clear();
  }
}
