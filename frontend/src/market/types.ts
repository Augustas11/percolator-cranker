// UI units: USD, and units of PERP.unit (on chain, 1e6 base units = one unit); the clients convert the engine's fixed-point integers.
export type Side = "long" | "short";

/** The one market the site trades. Every label for it comes from here. */
export const PERP = {
  name: "COMPUTE-PERP",
  /** One unit of size. */
  unit: "1M tokens",
  unitShort: "Mtok",
  quote: "USD per 1M tokens",
  benchmark: "Qwen3.8-27B",
  benchmarkId: "qwen/qwen3.8-27b",
  priceDecimals: 4,
  /** The matcher fills at most this many units per trade. */
  maxFillUnits: 100,
  /** Default trade size on /trade: about $30 of notional near $0.30. */
  defaultSize: 100,
} as const;
export const perpSubtitle = `${PERP.quote} · ${PERP.benchmark}`;
/** A mark or fill price, always PERP.priceDecimals decimals: $0.3036. */
export const perpPrice = (n: number | null | undefined) => n == null || !Number.isFinite(n) ? "—"
  : n.toLocaleString("en-US", { style: "currency", currency: "USD", minimumFractionDigits: PERP.priceDecimals, maximumFractionDigits: PERP.priceDecimals });
/** A size in units: "100 × 1M tokens". */
export const perpSize = (n: number) => `${n.toLocaleString("en-US", { maximumFractionDigits: 2 })} × ${PERP.unit}`;

export interface MarketInfo {
  symbol: string; collateral: "USDC"; maxLeverage: number; maintenanceMarginRatio: number;
  /** Trade fee the client sends, in bps of notional; absent until the market account is read. */
  feeBps?: number;
}
/** The liquidity account behind every fill (the matcher context). Sizes in units of PERP.unit. */
export interface Liquidity {
  /** Fill price over the mark, in bps each side. The matcher fills at the mark, so 0. */
  spreadBps: number;
  maxFill: number;
  /** Largest long / short one fill can open now, given the LP's inventory cap. */
  maxLong: number;
  maxShort: number;
  paused: boolean;
  /** How far the mark may sit from the matcher's reference price before fills are refused. */
  deviationBps: number;
}
export interface MarkPrice { price: number; ts: number }
export interface Funding { rate: number /* per hour, fraction */; nextFundingTs: number }
export interface Leg { side: Side; size: number /* units of PERP.unit, >0 */; entryPrice: number; unrealizedPnl: number }
export interface Health {
  equity: number;
  initialMarginUsed: number;
  maintenanceMargin: number;
  freeCollateral: number;
  ratio: number; // equity / maintenanceMargin, Infinity if flat
  status: "healthy" | "warning" | "liquidatable" | "unknown";
}
export interface Portfolio {
  owner: string | null;
  collateral: number;
  legs: Leg[]; // at most one leg: the site trades one market
  realizedPnl: number;
  health: Health;
}
export interface Fill { id: string; side: Side; size: number; price: number; ts: number; mine: boolean; wallet?: string }
export interface MarketSnapshot {
  market: MarketInfo;
  mark: MarkPrice;
  funding: Funding;
  portfolio: Portfolio;
  fills: Fill[]; // newest first, max 20
  /** Null or absent until the matcher context is read. */
  liquidity?: Liquidity | null;
}
export interface MarketClient {
  readonly tradingReady?: boolean;
  /** False when the client has no funding source and the funding fields are placeholders. */
  readonly fundingKnown?: boolean;
  getSnapshot(): MarketSnapshot;
  subscribe(listener: (s: MarketSnapshot) => void): () => void;
  connect(): Promise<string>;
  deposit(amount: number): Promise<void>;
  withdraw(amount: number): Promise<void>;
  openPosition(side: Side, size: number, leverage: number): Promise<void>;
  closePosition(): Promise<void>;
  dispose(): void;
}
