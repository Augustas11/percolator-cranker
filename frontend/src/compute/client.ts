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
  };
}

export interface ComputeHistory {
  v: number;
  generated_ms: number;
  from_ms: number;
  to_ms: number;
  points: [number, number][];
  changes: { t_ms: number; from: number; to: number; sig: string }[];
  outages: { provider: string; quantization: string; from_ms: number; to_ms: number | null }[];
  recorder_gaps: { from_ms: number; to_ms: number | null }[];
}

export interface ComputeFeedClient {
  readonly mock?: boolean;
  latest(): Promise<ComputeLatest>;
  history(): Promise<ComputeHistory>;
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
}
