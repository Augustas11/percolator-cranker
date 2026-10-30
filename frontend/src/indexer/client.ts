export interface Stats {
  market: string;
  program: string;
  as_of_slot: number | null;
  as_of: number | null;
  cranks: { total: number; success_rate: number | null };
  fills?: number;
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
  }[];
  next_before: string | null;
}

export interface Leaderboard {
  as_of_slot: number | null;
  note: string;
  entries: { rank: number; wallet: string; pnl: string; equity: string; net_deposits: string; trades: number; open_positions: number; portfolios: string[] }[];
}

export interface IndexerClient {
  readonly mock?: boolean;
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
      throw new Error(body?.error || `indexer ${response.status}`);
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

  async leaderboard() { await this.check(); return this.get<Leaderboard>("/leaderboard"); }
  async positions(owner: string) { await this.check(); return this.get<Positions>(`/positions?owner=${encodeURIComponent(owner)}`); }
  async fills(portfolio: string) { await this.check(); return this.get<Fills>(`/fills?portfolio=${encodeURIComponent(portfolio)}&limit=50`); }
}
