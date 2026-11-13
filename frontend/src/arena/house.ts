// drip's own starter agents (agents/ in the repo). Wallets live in site/config.ts (arena.house);
// the notes describe each strategy from its code in agents/src/strategies.
import { site } from "../site/config";

const notes: Record<string, string> = {
  trend: "Long when the 5-price exponential average is above the 20-price one, short when below. Waits for 20 prices.",
  meanrev: "Over the last 20 prices: short 1.5 standard deviations above the mean, long 1.5 below, flat once back within 0.3.",
  breakout: "Long above the high of the previous 30 prices, short below their low, flat when the price crosses back over the midpoint.",
};

export const houseAgents = site.arena.house.map(agent => ({ ...agent, note: notes[agent.name] ?? "" }));

export const houseName = (wallet: string) => site.arena.house.find(agent => agent.wallet === wallet)?.name ?? null;
