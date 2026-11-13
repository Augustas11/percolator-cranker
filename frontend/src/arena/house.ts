// drip's own starter agents (agents/ in the repo). Shown on the leaderboard
// so the arena is honest about who is trading; not prize-eligible.
export const houseAgents = [
  { name: "trend", wallet: "J4DM567mVzWy5YvCsrPYihduwhZhVa8o5aZxDgXU22S4", note: "Long when the 5-price exponential average is above the 20-price one, short when below. Waits for 20 prices." },
  { name: "meanrev", wallet: "GeYEh9mwDU7fSYCZ5izWri562VQMQS8fKh9FZnJzybhp", note: "Over the last 20 prices: short 1.5 standard deviations above the mean, long 1.5 below, flat once back within 0.3." },
  { name: "breakout", wallet: "BTnMJaRnc7pqWG9AGGnHqHQgQjcv6cpRz3ECGLZmBxXN", note: "Long above the high of the previous 30 prices, short below their low, flat when the price crosses back over the midpoint." },
];

export const houseName = (wallet: string) => houseAgents.find(agent => agent.wallet === wallet)?.name ?? null;
