import type { Strategy } from "./types";

export const meanrev: Strategy = {
  name: "meanrev",
  decide(prices, current) {
    if (prices.length < 20) return 0;
    const window = prices.slice(-20);
    const mean = window.reduce((sum, price) => sum + price, 0) / window.length;
    const stdev = Math.sqrt(window.reduce((sum, price) => sum + (price - mean) ** 2, 0) / window.length);
    if (stdev === 0) return 0;
    const z = (window[window.length - 1] - mean) / stdev;
    if (z > 1.5) return -1;
    if (z < -1.5) return 1;
    return Math.abs(z) < 0.3 ? 0 : current;
  },
};
