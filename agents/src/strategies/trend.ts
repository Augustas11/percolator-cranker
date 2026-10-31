import type { Strategy } from "./types";

function ema(prices: number[], period: number): number {
  const alpha = 2 / (period + 1);
  return prices.slice(1).reduce((value, price) => alpha * price + (1 - alpha) * value, prices[0]);
}

export const trend: Strategy = {
  name: "trend",
  decide(prices) {
    if (prices.length < 20) return 0;
    const fast = ema(prices, 5), slow = ema(prices, 20);
    return fast > slow ? 1 : fast < slow ? -1 : 0;
  },
};
