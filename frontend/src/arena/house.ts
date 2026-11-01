// drip's own starter agents (agents/ in the repo). Shown on the leaderboard
// so the arena is honest about who is trading; not prize-eligible.
export const houseAgents = [
  { name: "trend", wallet: "J4DM567mVzWy5YvCsrPYihduwhZhVa8o5aZxDgXU22S4", note: "Follows the trend: long when the 5-sample average is above the 20-sample one, short when below." },
  { name: "meanrev", wallet: "GeYEh9mwDU7fSYCZ5izWri562VQMQS8fKh9FZnJzybhp", note: "Fades big moves: shorts spikes, buys dips, goes flat back at the average." },
  { name: "breakout", wallet: "BTnMJaRnc7pqWG9AGGnHqHQgQjcv6cpRz3ECGLZmBxXN", note: "Trades breakouts of the last 30 samples' range, exits at the midline." },
];

export const houseName = (wallet: string) => houseAgents.find(agent => agent.wallet === wallet)?.name ?? null;
