export interface Stats {
  market: string;
  program: string;
  as_of_slot: number | null;
  as_of: number | null;
  cranks: { total: number; success_rate: number | null };
  /** Indexed fills, executed matcher fills included. */
  fills?: number;
  /** Optional (newer indexers): executed fills and USD volume over the last 24 h; volume is a USD decimal string like season.volume_usd. */
  fills_24h?: number;
  volume_24h?: string;
  uptime: { uptime_pct: number | null };
  liquidations_observed: number;
  liquidations_since_slot: number | null;
  open_interest: Record<string, { long_q: string; short_q: string; lifecycle: string }>;
}

export interface Positions {
  owner: string;
  as_of_slot: number | null;
  portfolios: {
    portfolio: string;
    last_changed_slot: number;
    capital: string;
    pnl: string;
    positions: { asset_index: number; market_id: number; side: "long" | "short"; basis_pos_q: string }[];
  }[];
}

export interface Fills {
  fills: {
    signature: string;
    ix_index: number;
    leg: number;
    slot: number;
    block_time: number | null;
    taker_portfolio: string;
    maker_portfolio: string;
    asset_index: number;
    size_q: string;
    reported_price: number | null;
    limit_price: number | null;
    mark_price: number | null;
    requested_fee_bps: number;
    status: "executed" | "requested";
    side: "buy" | "sell";
    price_note: string;
    /** Optional (newer indexers): the size the engine applied, signed from this portfolio's view, base units; and its price, e6. */
    executed_size?: string | null;
    applied_price?: number | null;
  }[];
  next_before: string | null;
}

export interface Leaderboard {
  as_of_slot: number | null;
  note: string;
  entries: { rank: number; wallet: string; pnl: string; equity: string; net_deposits: string; trades: number; open_positions: number; portfolios: string[] }[];
}

export interface RecentFills {
  fills: { signature: string; ix_index: number; leg: number; slot: number; block_time: number | null; side: "buy" | "sell"; size_q: string; mark_price: number | null; wallet: string | null; taker_portfolio: string; status: "executed" | "requested" }[];
}

/** GET /trades: market-wide executed fills, newest first (newer indexers only). */
export interface Trades {
  trades: {
    signature: string; ix_index: number; leg: number; slot: number; block_time: number | null;
    /** Taker view. */
    side: "buy" | "sell";
    /** Unsigned base units, 1e6 per unit of PERP.unit (same scale as /fills size_q). */
    size: string;
    price: number | null;
    price_source: "reported" | "matcher_oracle" | "last_mark" | null;
    wallet: string | null; taker_portfolio: string; maker_portfolio: string; asset_index: number;
  }[];
  next_before: string | null;
  price_note?: string;
}

/** An HTTP error from the indexer; status 404 on an endpoint means the indexer predates it. */
export class IndexerHttpError extends Error {
  constructor(public status: number, message: string) { super(message); }
}

export interface Season {
  season: number;
  since: number | null;
  agents: number;
  trades: number;
  volume_usd: string;
  open_interest_usd: string;
}

export interface IndexerClient {
  readonly mock?: boolean;
  recentFills?(limit: number): Promise<RecentFills>;
  trades?(limit: number, before?: string): Promise<Trades>;
  season?(): Promise<Season>;
  stats(): Promise<Stats>;
  leaderboard(): Promise<Leaderboard>;
  positions(owner: string): Promise<Positions>;
  fills(portfolio: string): Promise<Fills>;
}

export const indexerUrl = import.meta.env.VITE_INDEXER_URL ?? "http://127.0.0.1:8090";

export class HttpIndexerClient implements IndexerClient {
  private checkPromise?: Promise<Stats>;
  constructor(private baseUrl: string, private expected?: { market: string; program: string }) {}

  private async get<T>(path: string): Promise<T> {
    const response = await fetch(this.baseUrl.replace(/\/$/, "") + path, { signal: AbortSignal.timeout(10_000) });
    if (!response.ok) {
      const body = await response.json().catch(() => ({}));
      throw new IndexerHttpError(response.status, body?.error || `indexer ${response.status}`);
    }
    return response.json();
  }

  async stats() {
    const s = await this.get<Stats>("/stats");
    if (this.expected && (s.market !== this.expected.market || s.program !== this.expected.program)) {
      this.checkPromise = undefined;
      throw new Error(`indexer serves another market (${s.market})`);
    }
    return s;
  }

  private async check() {
    if (this.expected) await (this.checkPromise ??= this.stats().catch(error => {
      this.checkPromise = undefined;
      throw error;
    }));
  }

  async recentFills(limit: number) { await this.check(); return this.get<RecentFills>(`/fills/recent?limit=${limit}`); }
  async trades(limit: number, before?: string) {
    await this.check();
    return this.get<Trades>(`/trades?limit=${Math.min(500, Math.max(1, Math.floor(limit)))}${before ? `&before=${encodeURIComponent(before)}` : ""}`);
  }
  async season() { await this.check(); return this.get<Season>("/season"); }
  async leaderboard() { await this.check(); return this.get<Leaderboard>("/leaderboard"); }
  async positions(owner: string) { await this.check(); return this.get<Positions>(`/positions?owner=${encodeURIComponent(owner)}`); }
  async fills(portfolio: string) { await this.check(); return this.get<Fills>(`/fills?portfolio=${encodeURIComponent(portfolio)}&limit=50`); }
}
