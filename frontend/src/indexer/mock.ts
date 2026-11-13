import { IndexerHttpError } from "./client";
import type { Fills, IndexerClient, Leaderboard, Positions, RecentFills, Season, Stats, Trades } from "./client";

// Mock mode only: a COMPUTE-PERP board near $0.30, sizes in units of 1M tokens (1e6 base units each), PnL and deposits in USDC atoms.
// [wallet, pnl, net deposits, trades, open position in base units (signed, 0 = flat)]
const BOARD: [string, number, number, number, number][] = [
  ["7cVfgArCheMR6Cs4t6vz5rfnqd56vZq4ndaBrY5xkxXy", 41_870_000, 1_000_000_000, 38, 180_000_000],
  ["J4DM567mVzWy5YvCsrPYihduwhZhVa8o5aZxDgXU22S4", 18_420_000, 500_000_000, 61, 100_000_000],
  ["9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin", 12_050_000, 750_000_000, 14, -60_000_000],
  ["GeYEh9mwDU7fSYCZ5izWri562VQMQS8fKh9FZnJzybhp", 4_310_000, 500_000_000, 44, 0],
  ["3Kq7nKxuBfTZ8b2RmhWcVkqYH1jTCrPZf4SgcQhzn5Lw", -2_640_000, 400_000_000, 9, 40_000_000],
  ["BTnMJaRnc7pqWG9AGGnHqHQgQjcv6cpRz3ECGLZmBxXN", -6_980_000, 500_000_000, 27, -100_000_000],
  ["5ZWj7a1f8tWkjBESHKgrLmXshuXxqeY9SYcfbshpAqPG", -15_200_000, 600_000_000, 21, 0],
];

export class MockIndexerClient implements IndexerClient {
  readonly mock = true;
  /** legacy: an indexer from before /trades, the 24 h stats and executed fill fields. */
  constructor(private options: { legacy?: boolean } = {}) {}

  async stats(): Promise<Stats> {
    return {
      ...(this.options.legacy ? {} : { fills_24h: 3, volume_24h: "45540000" }),
      market: "uZY6RPrFDqtbYbREA6UToWpzq1BKEsneZxcsE4As27J", program: "GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS",
      as_of_slot: 1000, as_of: 1793232000,
      cranks: { total: 200, success_rate: 0.98 }, uptime: { uptime_pct: 99.5 },
      liquidations_observed: 2, liquidations_since_slot: 800,
      open_interest: { asset_0: { long_q: "250000000", short_q: "250000000", lifecycle: "active" } },
    };
  }

  async recentFills(limit: number): Promise<RecentFills> {
    return { fills: [0, 1, 2].slice(0, limit).map(i => ({
      signature: `mock-signature-${i}`, ix_index: 0, leg: 0, slot: 999 - i, block_time: Math.floor(Date.now() / 1000) - 12 - i * 60,
      side: i === 1 ? "sell" : "buy", size_q: i === 1 ? "-150000000" : "50000000", mark_price: 303600,
      wallet: i === 2 ? null : BOARD[i][0], taker_portfolio: "4fYNw3dojWmQ4dXtSGE9epjRGy9pgEtqBJ1pTzAkAb8m", status: "executed",
    })) };
  }

  async trades(limit: number): Promise<Trades> {
    if (this.options.legacy) throw new IndexerHttpError(404, "Not found");
    const now = Math.floor(Date.now() / 1000);
    return { trades: [0, 1, 2].slice(0, limit).map(i => ({
      signature: `mock-trade-${i}`, ix_index: 0, leg: 0, slot: 999 - i, block_time: i === 2 ? null : now - 30 - i * 90,
      side: i === 1 ? "sell" as const : "buy" as const, size: ["50000000", "150000000", "25000000"][i], price: i === 2 ? null : 303600,
      price_source: i === 2 ? null : "matcher_oracle" as const, wallet: BOARD[i][0],
      taker_portfolio: `${BOARD[i][0].slice(0, 6)}-portfolio`, maker_portfolio: "mock-maker", asset_index: 0,
    })), next_before: null, price_note: "matcher fills at the oracle price" };
  }

  async season(): Promise<Season> {
    return { season: 0, since: 1793232000, agents: BOARD.length, trades: BOARD.reduce((n, r) => n + r[3], 0), volume_usd: "18640000000", open_interest_usd: "157900000" };
  }

  async leaderboard(): Promise<Leaderboard> {
    return { as_of_slot: 1000,
      note: "devnet, test tokens. PnL = equity (capital + engine PnL at the last indexed change, open positions marked at the last crank) minus net deposits.",
      entries: BOARD.map(([wallet, pnl, deposits, trades, open], i) => ({
        rank: i + 1, wallet, pnl: String(pnl), equity: String(deposits + pnl), net_deposits: String(deposits), trades, open_positions: open ? 1 : 0, portfolios: [`${wallet.slice(0, 6)}-portfolio`],
      })),
    };
  }

  async positions(owner: string): Promise<Positions> {
    const row = BOARD.find(([wallet]) => wallet === owner);
    const q = row ? row[4] : 150_000_000;
    return { owner, as_of_slot: 1000, portfolios: [{
      portfolio: "mock-portfolio", last_changed_slot: 999, capital: "1000000000", pnl: "12500000",
      positions: q ? [{ asset_index: 0, market_id: 0, side: q > 0 ? "long" : "short", basis_pos_q: String(q) }] : [],
    }] };
  }

  async fills(portfolio: string): Promise<Fills> {
    return { fills: ["executed", "requested"].map((status, i) => ({
      signature: `mock-signature-${i}`, ix_index: 0, leg: 0, slot: 999 - i, block_time: 1793232000 - i * 15,
      taker_portfolio: portfolio, maker_portfolio: "mock-maker", asset_index: 0,
      size_q: i === 0 ? "-150000000" : "50000000", reported_price: null, limit_price: null, mark_price: 303600,
      requested_fee_bps: 10, status: status as "executed" | "requested", side: i === 0 ? "sell" : "buy",
      price_note: "last pushed mark",
      ...(this.options.legacy ? {} : { executed_size: status === "executed" ? (i === 0 ? "-150000000" : "50000000") : null, applied_price: status === "executed" ? 303600 : null }),
    })), next_before: null };
  }
}
