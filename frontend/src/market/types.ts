// UI units (USD, SOL); the real client will convert the engine's fixed-point integers.
export type Side = "long" | "short";
export interface MarketInfo { symbol: "SOL-PERP"; collateral: "USDC"; maxLeverage: number; maintenanceMarginRatio: number }
export interface MarkPrice { price: number; ts: number }
export interface Funding { rate: number /* per hour, fraction */; nextFundingTs: number }
export interface Leg { side: Side; size: number /* base SOL, >0 */; entryPrice: number; unrealizedPnl: number }
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
  legs: Leg[]; // at most one leg for SOL-PERP
  realizedPnl: number;
  health: Health;
}
export interface Fill { id: string; side: Side; size: number; price: number; ts: number; mine: boolean }
export interface MarketSnapshot {
  market: MarketInfo;
  mark: MarkPrice;
  funding: Funding;
  portfolio: Portfolio;
  fills: Fill[]; // newest first, max 20
}
export interface MarketClient {
  readonly tradingReady?: boolean;
  getSnapshot(): MarketSnapshot;
  subscribe(listener: (s: MarketSnapshot) => void): () => void;
  connect(): Promise<string>;
  deposit(amount: number): Promise<void>;
  withdraw(amount: number): Promise<void>;
  openPosition(side: Side, size: number, leverage: number): Promise<void>;
  closePosition(): Promise<void>;
  dispose(): void;
}
