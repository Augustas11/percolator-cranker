import type { Strategy } from "./types";

export const breakout: Strategy = {
  name: "breakout",
  decide(prices, current) {
    if (prices.length < 31) return 0;
    const channel = prices.slice(-31, -1);
    const high = Math.max(...channel), low = Math.min(...channel);
    const price = prices[prices.length - 1], midpoint = (high + low) / 2;
    if (price > high) return 1;
    if (price < low) return -1;
    if (current === 1 && price < midpoint || current === -1 && price > midpoint) return 0;
    return current;
  },
};
