import { units } from "../drip";
import { tool } from "./tool";

interface Leaderboard {
  as_of_slot: number | null;
  note: string;
  entries: { rank: number; wallet: string; pnl: string; equity: string; net_deposits: string; trades: number; open_positions: number; portfolios: unknown }[];
}

export default tool({
  name: "leaderboard",
  description: "Read the trading leaderboard and your entry (devnet, test tokens).",
  run: async (drip) => {
    const board = await drip.fetchJson<Leaderboard>("/indexer/leaderboard");
    const entries = board.entries.map(entry => ({ ...entry, pnl_usdc: units(BigInt(entry.pnl)), equity_usdc: units(BigInt(entry.equity)) }));
    return { ...board, entries, you: entries.find(entry => entry.wallet === drip.wallet.publicKey.toBase58()) ?? null };
  },
});
