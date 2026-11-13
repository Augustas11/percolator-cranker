// The compute price feed written by flop-index's flop-feed (docs/flop-compute/FEED.md).
export interface PricingRow { provider: string; quantization: string; listed: number; priced: number; up: boolean }
export interface Seller { provider: string; quantization: string; blended: number; up: boolean; in_set: boolean; tier: boolean }

export interface ComputeLatest {
  v: number;
  generated_ms: number;
  mark_usd: number | null;
  landed_ms: number;
  source_ms: number | null;
  sig: string | null;
  slot: number | null;
  market: string | null;
  oracle: string | null;
  source: string | null;
  set: string | null;
  clamp: string | null;
  blend_in_out: number;
  pricing_set: PricingRow[];
  sellers: Seller[];
  flop_side: {
    flop_per_gn: number | null;
    prints: number | null;
    buyers: number | null;
    miners: number | null;
    gateway_share: number | null;
    stale: string | null;
    /** The window the counts cover, in seconds; absent from feeds that predate the field. */
    window_s?: number | null;
  };
}

export interface ComputeHistory {
  v: number;
  generated_ms: number;
  from_ms: number;
  to_ms: number;
  /** Highest and lowest mark over every push in the window (null when none). */
  high: number | null;
  low: number | null;
  points: [number, number][];
  changes: { t_ms: number; from: number; to: number; sig: string }[];
  outages: { provider: string; quantization: string; from_ms: number; to_ms: number | null }[];
  recorder_gaps: { from_ms: number; to_ms: number | null }[];
}

export type Point = [number, number];

/** Input:output token mixes the feed prices at. `avg_15_1` is the blend every other price on the page uses. */
export type MixKey = "chat_3_1" | "avg_15_1" | "agent_150_1";
export type Mixes = Partial<Record<MixKey, number | null>>;

export interface MarketQuote { provider: string; quantization: string; price: number | null; up: boolean; uptime_24h_pct: number | null }
/**
 * What a request pays right now when it is spread across the sellers that are up, the way OpenRouter's default routing
 * spreads traffic: cheaper sellers get more (1 / price squared), weighted by recent uptime, no seller above half.
 */
export interface Routed {
  price: number;
  sellers: number;
  /** Where the money goes: the largest shares, share as a fraction of 1. */
  top: { provider: string; quantization: string; price: number; share: number }[];
  change_1h_pct: number | null;
  change_24h_pct: number | null;
  series: Point[];
  /** True when uptime was missing and equal weights were used instead. */
  uptime_fallback?: boolean;
  /** True when one seller hit the half-weight cap. */
  capped?: boolean;
}
export interface FlopParity {
  market_usd_per_bn_gn: number | null;
  flop_rate_flop_per_gn: number | null;
  implied_flop_usd: number | null;
  implied_flop_usd_routed: number | null;
  as_of_ms: number | null;
}

export interface MarketModel {
  id: string;
  name: string;
  family: string;
  sellers_total: number;
  sellers_up: number;
  median: number;
  p25: number;
  p75: number;
  min: number;
  max: number;
  thin?: boolean;
  /** Newest data read for this model; absent from feeds written before the field existed. */
  data_ms?: number | null;
  /** True when the feed has no fresh read of this model. */
  stale?: boolean;
  /** Quotes counted in the median at their last price while the seller is offline. */
  quotes_held?: number;
  /** FLOP's compute count for the model: G_n per token, from FLOP's registry or 2 × parameters. */
  compute?: { params: number | null; gn_per_token: number | null; source: string | null; basis?: string | null } | null;
  /** USD per billion G_n: the median price per 1M tokens over compute per token; min and max over its sellers. */
  usd_per_bn_gn?: number | null;
  usd_per_bn_gn_min?: number | null;
  usd_per_bn_gn_max?: number | null;
  /** The model median at each token mix. */
  mix?: Mixes | null;
  /** Output price over input price. */
  out_in_ratio?: number | null;
  /** Null when no seller has a usable quote. */
  cheapest: { provider: string; quantization: string; price: number } | null;
  change_24h_pct: number | null;
  series: Point[];
  quotes: MarketQuote[];
  /** Optional (newer feeds): the routed price and its USD per billion G_n. */
  routed?: Routed | null;
  routed_usd_per_bn_gn?: number | null;
}
export interface MarketProvider { name: string; models: number; quotes: number; uptime_24h_pct: number | null; cheapest_in: number }
export interface MarketChange { at_ms: number; model: string; provider: string; quantization: string; from: number; to: number }
export interface MarketDynamic { model: string; provider: string; quantization: string; min: number; max: number; changes_24h: number }

/** The broad market: every open model's seller quotes (market.json). USD per 1M tokens, 15:1 input:output. */
export interface ComputeMarket {
  generated_ms: number;
  /** Newest data actually read; absent from feeds written before the field existed. */
  data_ms?: number | null;
  window: { from_ms: number; to_ms: number };
  index: {
    /** Null until a base exists. */
    level: number | null;
    base_ms: number | null;
    change_24h_pct: number | null;
    change_7d_pct: number | null;
    n_models: number;
    n_quotes: number;
    n_quotes_up: number;
    n_providers: number;
    series: Point[];
  };
  models: MarketModel[];
  /** Median and middle half of the models' USD per billion G_n. */
  compute_price?: { median_usd_per_bn_gn: number | null; p25: number | null; p75: number | null; n_models: number | null } | null;
  /** The going rate (median of model medians) at each token mix. */
  mixes?: Mixes | null;
  out_in_ratio_median?: number | null;
  /** FLOP's on-chain rate set against the open market for the same compute. */
  flop?: { gn_price_flop: number | null; implied_flop_usd: number | null; basis_model: string | null; implied_flop_usd_routed?: number | null; parity?: FlopParity | null } | null;
  /** Optional (newer feeds): the benchmark model's routed price. */
  routed?: { benchmark: string; price: number | null; usd_per_bn_gn: number | null; series: Point[] } | null;
  providers: MarketProvider[];
  changes: MarketChange[];
  dynamic: MarketDynamic[];
}

export interface ComputeFeedClient {
  latest(): Promise<ComputeLatest>;
  history(): Promise<ComputeHistory>;
  market(): Promise<ComputeMarket>;
}

export const computeFeedUrl = import.meta.env.VITE_COMPUTE_FEED_URL ?? "/compute-feed";

export class HttpComputeFeedClient implements ComputeFeedClient {
  constructor(private baseUrl: string) {}

  private async get<T>(name: string): Promise<T> {
    const response = await fetch(`${this.baseUrl.replace(/\/$/, "")}/${name}`, { signal: AbortSignal.timeout(10_000), cache: "no-store" });
    if (!response.ok) throw new Error(`price feed ${response.status}`);
    return response.json();
  }

  latest() { return this.get<ComputeLatest>("latest.json"); }
  history() { return this.get<ComputeHistory>("history.json"); }
  market() { return this.get<ComputeMarket>("market.json"); }
}
