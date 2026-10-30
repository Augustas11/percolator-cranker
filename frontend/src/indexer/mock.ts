import type { Fills, IndexerClient, Leaderboard, Positions, RecentFills, Season, Stats } from "./client";

export class MockIndexerClient implements IndexerClient {
  readonly mock = true;
  async stats(): Promise<Stats> {
    return {
      market: "HyMKxfct1S9aFtq21GG3FUYUViQkmpZdk5pwGRtAz48C", program: "GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS",
      as_of_slot: 1000, as_of: 1793232000,
      cranks: { total: 200, success_rate: 0.98 }, uptime: { uptime_pct: 99.5 },
      liquidations_observed: 2, liquidations_since_slot: 800,
      open_interest: { asset_0: { long_q: "2500000", short_q: "2500000", lifecycle: "active" } },
    };
  }

  async recentFills(limit: number): Promise<RecentFills> {
    return { fills: [0, 1, 2].slice(0, limit).map(i => ({
      signature: `mock-signature-${i}`, ix_index: 0, leg: 0, slot: 999 - i, block_time: Math.floor(Date.now() / 1000) - 12 - i * 60,
      side: i === 1 ? "sell" : "buy", size_q: i === 1 ? "-1500000" : "500000", mark_price: 97750000,
      wallet: i === 2 ? null : "Ab12mockwalletlongyZ9q", taker_portfolio: "mock-portfolio", status: "executed",
    })) };
  }

  async season(): Promise<Season> {
    return { season: 0, since: 1793232000, agents: 12, trades: 345, volume_usd: "12345000000", open_interest_usd: "987500000" };
  }

  async leaderboard(): Promise<Leaderboard> {
    return { as_of_slot: 1000,
      note: "devnet, test tokens. PnL = equity (capital + engine PnL at the last indexed change, open positions marked at the last crank) minus net deposits.",
      entries: [{ rank: 1, wallet: "mock-trader-wallet", pnl: "12500000", equity: "1012500000", net_deposits: "1000000000", trades: 4, open_positions: 1, portfolios: ["mock-portfolio"] }],
    };
  }

  async positions(owner: string): Promise<Positions> {
    return { owner, as_of_slot: 1000, portfolios: [{
      portfolio: "mock-portfolio", last_changed_slot: 999, capital: "1000000000", pnl: "12500000",
      positions: [{ asset_index: 0, market_id: 0, side: "long", basis_pos_q: "1500000" }],
    }] };
  }

  async fills(portfolio: string): Promise<Fills> {
    return { fills: ["executed", "requested"].map((status, i) => ({
      signature: `mock-signature-${i}`, ix_index: 0, leg: 0, slot: 999 - i, block_time: 1793232000 - i * 15,
      taker_portfolio: portfolio, maker_portfolio: "mock-maker", asset_index: 0,
      size_q: i === 0 ? "-1500000" : "500000", reported_price: null, limit_price: null, mark_price: 150000000,
      requested_fee_bps: 10, status: status as "executed" | "requested", side: i === 0 ? "sell" : "buy",
      price_note: "last pushed mark",
    })), next_before: null };
  }
}
