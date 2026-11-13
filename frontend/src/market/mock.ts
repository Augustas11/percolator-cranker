import { PERP } from "./types";
import type { Fill, Leg, MarketClient, MarketSnapshot, Side } from "./types";

const EPS = 1e-9;
// Wallets that trade against the local market client.
const TRADERS = ["7cVfgArCheMR6Cs4t6vz5rfnqd56vZq4ndaBrY5xkxXy", "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin", "3Kq7m1dYpJ9cZr8bW4XfTnS2vHgE6uLkQaN5oPzR1tMs"];

interface MockOptions { tickMs?: number; seed?: number; startPrice?: number; autoTick?: boolean; sol?: number; usdc?: number | null }

function positive(value: number, name: string) {
  if (!Number.isFinite(value) || value <= 0) throw new Error(`Enter ${name} above zero.`);
}

// Small deterministic mulberry32 generator.
function mulberry32(seed: number) {
  return () => {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t ^= t + Math.imul(t ^ (t >>> 7), 61 | t);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function freeze<T extends object>(value: T): T {
  for (const child of Object.values(value)) {
    if (child && typeof child === "object") freeze(child);
  }
  return Object.freeze(value);
}

export class MockMarketClient implements MarketClient {
  private snapshot: MarketSnapshot;
  private listeners = new Set<(s: MarketSnapshot) => void>();
  private timer?: ReturnType<typeof setInterval>;
  private random: () => number;
  private fillId = 0;

  /** The mock wallet's own balances: SOL, and test USDC (null = no token account yet). */
  private wallet: { sol: number; usdc: number | null };

  constructor({ tickMs = 1000, seed = 1, startPrice = 0.3036, autoTick = true, sol = 1, usdc = 10_000 }: MockOptions = {}) {
    this.wallet = { sol, usdc };
    positive(tickMs, "tickMs");
    positive(startPrice, "startPrice");
    if (!Number.isFinite(seed)) throw new Error("seed must be finite");
    this.random = mulberry32(seed);
    const now = Date.now();
    this.snapshot = freeze({
      market: { symbol: PERP.name, collateral: "USDC", maxLeverage: 2, initialMarginRatio: 0.5, maintenanceMarginRatio: 0.25, feeBps: 15 },
      mark: { price: startPrice, ts: now },
      funding: { rate: 0.0001, nextFundingTs: now + 3_600_000 },
      portfolio: {
        owner: null, collateral: 0, legs: [], realizedPnl: 0,
        health: { equity: 0, initialMarginUsed: 0, maintenanceMargin: 0, freeCollateral: 0, ratio: Infinity, status: "healthy" },
      },
      fills: [],
      liquidity: { spreadBps: 0, maxFill: 100, maxLong: 100, maxShort: 100, paused: false, deviationBps: 500 },
    });
    if (autoTick) this.timer = setInterval(() => this.tick(), tickMs);
  }

  getSnapshot = () => this.snapshot;

  subscribe = (listener: (s: MarketSnapshot) => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  private connected() {
    if (!this.snapshot.portfolio.owner) throw new Error("Connect a wallet first.");
  }

  private publish(next: MarketSnapshot) {
    const portfolio = next.portfolio;
    const leg = portfolio.legs[0];
    const pnl = leg ? (next.mark.price - leg.entryPrice) * leg.size * (leg.side === "long" ? 1 : -1) : 0;
    const maintenanceMargin = leg ? leg.size * next.mark.price * next.market.maintenanceMarginRatio : 0;
    const equity = portfolio.collateral + pnl;
    const initialMarginUsed = leg ? portfolio.health.initialMarginUsed : 0;
    const ratio = leg ? equity / maintenanceMargin : Infinity;
    this.snapshot = freeze({
      ...next,
      portfolio: {
        ...portfolio,
        legs: leg ? [{ ...leg, unrealizedPnl: pnl }] : [],
        health: {
          equity, initialMarginUsed, maintenanceMargin,
          freeCollateral: Math.max(0, equity - initialMarginUsed), ratio,
          status: equity < 0 || ratio <= 1 ? "liquidatable" : ratio < 1.5 ? "warning" : "healthy",
        },
      },
      fills: next.fills.slice(0, 20),
    });
    for (const listener of this.listeners) listener(this.snapshot);
  }

  private fill(side: Side, size: number, mine: boolean): Fill {
    const wallet = mine ? this.snapshot.portfolio.owner ?? undefined : TRADERS[Math.floor(this.random() * TRADERS.length)];
    return { id: String(++this.fillId), side, size, mine, wallet, price: this.snapshot.mark.price, ts: Date.now() };
  }

  tick() {
    const s = this.snapshot;
    const now = Date.now();
    const mark = { price: s.mark.price * (1 + (this.random() - 0.5) * 0.004), ts: now };
    const bot = this.random() < 0.35
      ? { ...this.fill(this.random() < 0.5 ? "long" : "short", Math.round(10 + this.random() * 90), false), price: mark.price }
      : null;
    this.publish({
      ...s, mark,
      funding: { rate: 0.0001 + (this.random() - 0.5) * 0.00002, nextFundingTs: now >= s.funding.nextFundingTs ? now + 3_600_000 : s.funding.nextFundingTs },
      fills: bot ? [bot, ...s.fills] : s.fills,
    });
  }

  async connect() {
    const owner = "Trader1111111111111111111111111111111111111";
    this.publish({ ...this.snapshot, portfolio: { ...this.snapshot.portfolio, owner } });
    return owner;
  }

  async deposit(amount: number) {
    this.connected();
    positive(amount, "an amount");
    if (!this.wallet.usdc) throw new Error("You have no test USDC yet: get some from the faucet.");
    if (amount > this.wallet.usdc) throw new Error(`You have ${this.wallet.usdc.toLocaleString("en-US", { maximumFractionDigits: 6 })} test USDC: deposit that much or less.`);
    this.wallet.usdc -= amount;
    const s = this.snapshot;
    this.publish({ ...s, portfolio: { ...s.portfolio, collateral: s.portfolio.collateral + amount } });
  }

  async withdraw(amount: number) {
    this.connected();
    positive(amount, "an amount");
    const s = this.snapshot;
    // EPS absorbs float residue so "withdraw everything" works after decimal deposits.
    if (amount > s.portfolio.health.freeCollateral + EPS || amount > s.portfolio.collateral + EPS) throw new Error("insufficient free collateral");
    this.wallet.usdc = (this.wallet.usdc ?? 0) + amount;
    this.publish({ ...s, portfolio: { ...s.portfolio, collateral: Math.max(0, s.portfolio.collateral - amount) } });
  }

  async openPosition(side: Side, size: number, leverage: number) {
    this.connected();
    positive(size, "a size");
    if (side !== "long" && side !== "short") throw new Error("invalid side");
    const s = this.snapshot;
    if (!Number.isFinite(leverage) || leverage < 1 || leverage > s.market.maxLeverage) throw new Error(`leverage must be between 1 and ${s.market.maxLeverage}`);
    const existing = s.portfolio.legs[0];
    if (existing && existing.side !== side) throw new Error("close the current position first");
    const notional = size * s.mark.price;
    positive(notional, "notional");
    const margin = notional / leverage;
    if (margin > s.portfolio.health.freeCollateral) throw new Error("insufficient free collateral");
    const totalSize = (existing?.size ?? 0) + size;
    positive(totalSize, "size");
    const leg: Leg = {
      side, size: totalSize,
      entryPrice: existing ? existing.entryPrice * (existing.size / totalSize) + s.mark.price * (size / totalSize) : s.mark.price,
      unrealizedPnl: 0,
    };
    this.publish({
      ...s, portfolio: { ...s.portfolio, legs: [leg], health: { ...s.portfolio.health, initialMarginUsed: s.portfolio.health.initialMarginUsed + margin } },
      fills: [this.fill(side, size, true), ...s.fills],
    });
  }

  async walletBalances() {
    this.connected();
    return { ...this.wallet };
  }

  async createTokenAccount() {
    this.connected();
    if (this.wallet.sol <= 0) throw new Error("The wallet needs a little SOL to pay the fee.");
    this.wallet.usdc ??= 0;
  }

  /** Mock mode only: what the faucet's claim does to the wallet. */
  credit(usdc: number) { this.wallet.usdc = (this.wallet.usdc ?? 0) + usdc; }

  async closePosition() {
    this.connected();
    const s = this.snapshot;
    const leg = s.portfolio.legs[0];
    if (!leg) throw new Error("no open position");
    this.publish({
      ...s,
      portfolio: { ...s.portfolio, collateral: s.portfolio.collateral + leg.unrealizedPnl, realizedPnl: s.portfolio.realizedPnl + leg.unrealizedPnl, legs: [] },
      fills: [this.fill(leg.side === "long" ? "short" : "long", leg.size, true), ...s.fills],
    });
  }

  dispose() {
    clearInterval(this.timer);
    this.listeners.clear();
  }
}
