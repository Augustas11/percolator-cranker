import { Connection, SendTransactionError, Transaction } from "@solana/web3.js";
import type { Keypair, PublicKey, TransactionInstruction } from "@solana/web3.js";
import type { Fill, MarketClient, MarketSnapshot, Side } from "./types";
import type { DevnetConfig } from "../chain/config";
import { effectiveAbsQ, parseMarket, parsePortfolio } from "../chain/accounts";
import { createAtaIdempotentIx } from "../chain/pda";
import { buildCreatePortfolio, buildDeposit, buildTrade, buildWithdraw, findPortfolio } from "../chain/tx";
import { getWallet } from "../chain/wallet";
import type { Wallet } from "../chain/wallet";

function atoms(value: number, name: string) {
  const rounded = Math.round(value * 1e6);
  if (!Number.isFinite(value) || value <= 0 || !Number.isSafeInteger(rounded) || rounded <= 0) {
    throw new Error(`${name} must be positive and representable in millionths`);
  }
  return BigInt(rounded);
}

export class DevnetMarketClient implements MarketClient {
  readonly tradingReady: boolean;
  private conn: Connection;
  private wallet?: Wallet;
  private portfolio?: PublicKey;
  private listeners = new Set<(s: MarketSnapshot) => void>();
  private timer?: ReturnType<typeof setTimeout>;
  private disposed = false;
  private snapshot: MarketSnapshot = {
    market: { symbol: "SOL-PERP", collateral: "USDC", maxLeverage: 1, maintenanceMarginRatio: 0 },
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

  private async poll() {
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
    const maintenanceMargin = size * price * mmRatio;
    const initialMarginUsed = size * price * Number(market.initialMarginBps) / 10000;
    const ratio = size ? collateral / maintenanceMargin : Infinity;
    if (this.disposed) return;
    const now = Date.now();
    this.snapshot = {
      market: { symbol: "SOL-PERP", collateral: "USDC", maxLeverage: 10000 / Number(market.initialMarginBps), maintenanceMarginRatio: mmRatio },
      mark: { price, ts: now }, funding: { rate: 0, nextFundingTs: now },
      portfolio: {
        owner: this.wallet?.publicKey.toBase58() ?? null, collateral, realizedPnl: NaN,
        legs: leg && size ? [{ side: leg.side, size, entryPrice: NaN, unrealizedPnl: NaN }] : [],
        health: {
          equity: collateral, initialMarginUsed, maintenanceMargin, freeCollateral: Math.max(0, collateral - initialMarginUsed), ratio,
          status: collateral < 0 || ratio <= 1 ? "liquidatable" : ratio < 1.5 ? "warning" : "healthy",
        },
      },
      fills: this.snapshot.fills,
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
    if (!this.wallet) throw new Error("connect wallet first");
    return this.wallet;
  }

  private requirePortfolio() {
    if (!this.portfolio) throw new Error("deposit collateral first");
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
        throw new Error(`${JSON.stringify(result.value.err)}: ${landed?.meta?.logMessages?.at(-1) ?? "transaction failed"}`);
      }
    } catch (err) {
      if (err instanceof SendTransactionError) {
        const logs = err.logs ?? await err.getLogs(this.conn).catch(() => []);
        throw new Error(`${err.message}${logs?.length ? `: ${logs.at(-1)}` : ""}`);
      }
      throw err;
    }
    onConfirmed?.();
    if (fill) this.snapshot = { ...this.snapshot, fills: [
      { ...fill, id: signature, price, ts: Date.now(), mine: true }, ...this.snapshot.fills,
    ].slice(0, 20) };
    await this.refresh().catch(() => {});
    return signature;
  }

  async deposit(usd: number) {
    const wallet = this.connected();
    const amount = atoms(usd, "amount");
    if (!this.portfolio) {
      const created = await buildCreatePortfolio(this.cfg, wallet.publicKey);
      await this.send(created.ixs, [created.portfolio], undefined, () => { this.portfolio = created.portfolio.publicKey; });
    }
    const ixs = await buildDeposit(this.conn, this.cfg, wallet.publicKey, this.requirePortfolio(), amount);
    await this.send([createAtaIdempotentIx(wallet.publicKey, wallet.publicKey, this.cfg.mint), ...ixs]);
  }

  async withdraw(usd: number) {
    const wallet = this.connected();
    const amount = atoms(usd, "amount");
    const ixs = await buildWithdraw(this.conn, this.cfg, wallet.publicKey, this.requirePortfolio(), amount);
    await this.send([createAtaIdempotentIx(wallet.publicKey, wallet.publicKey, this.cfg.mint), ...ixs]);
  }

  private limit(side: Side) {
    return atoms(this.snapshot.mark.price * (side === "long" ? 1.005 : 0.995), "limit price");
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
    const q = atoms(size, "size");
    await this.refresh();
    const max = this.snapshot.market.maxLeverage;
    if (!Number.isFinite(leverage) || leverage < 1 || leverage > max) throw new Error(`leverage must be between 1 and ${max}`);
    await this.trade(side === "long" ? q : -q);
  }

  async closePosition() {
    this.connected();
    const accounts = await this.conn.getMultipleAccountsInfo([this.cfg.market, this.requirePortfolio()]);
    if (!accounts[0] || !accounts[1]) throw new Error("Trade account not found");
    const market = parseMarket(accounts[0].data), pf = parsePortfolio(accounts[1].data);
    const leg = pf.legs.find(leg => leg.assetIndex === 0 && leg.marketId === market.asset0.marketId);
    if (!leg) throw new Error("no open position");
    const magnitude = effectiveAbsQ(market, leg);
    if (!magnitude) throw new Error("no open position");
    await this.refresh();
    await this.trade(leg.side === "long" ? -magnitude : magnitude);
  }

  dispose() {
    this.disposed = true;
    clearTimeout(this.timer);
    this.listeners.clear();
  }
}
