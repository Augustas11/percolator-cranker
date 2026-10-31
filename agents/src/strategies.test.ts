import { describe, expect, it } from "vitest";
import { strategies } from "./strategies";
import { trend } from "./strategies/trend";
import { meanrev } from "./strategies/meanrev";
import { breakout } from "./strategies/breakout";

const rising = Array.from({ length: 40 }, (_, i) => 100 + i);

describe("strategies", () => {
  for (const strategy of Object.values(strategies)) {
    it(`${strategy.name} stays flat during warmup`, () => {
      expect(strategy.decide([], 1)).toBe(0);
      expect(strategy.decide(rising.slice(0, strategy.name === "breakout" ? 30 : 19), -1)).toBe(0);
    });
  }
  it("trend follows rising and falling prices", () => {
    expect(trend.decide(rising, 0)).toBe(1);
    expect(trend.decide([...rising].reverse(), 0)).toBe(-1);
    expect(trend.decide(Array(20).fill(100), 1)).toBe(0);
  });
  it("meanrev fades spikes in either direction", () => {
    expect(meanrev.decide([...Array(19).fill(100), 120], 0)).toBe(-1);
    expect(meanrev.decide([...Array(19).fill(100), 80], 0)).toBe(1);
  });
  it("meanrev exits near the mean, holds between thresholds and handles zero variance", () => {
    expect(meanrev.decide([...Array.from({ length: 19 }, (_, i) => 91 + i), 100], 1)).toBe(0);
    const middle = [...Array(10).fill(90), ...Array(9).fill(110), 105];
    expect(meanrev.decide(middle, 1)).toBe(1);
    expect(meanrev.decide(middle, -1)).toBe(-1);
    expect(meanrev.decide(Array(20).fill(100), 1)).toBe(0);
    expect(meanrev.decide([...Array(5).fill(999), ...Array(20).fill(100)], -1)).toBe(0);
  });
  it("breakout enters above and below the previous 30 samples", () => {
    expect(breakout.decide([...Array(30).fill(100), 101], 0)).toBe(1);
    expect(breakout.decide([...Array(30).fill(100), 99], 0)).toBe(-1);
    expect(breakout.decide([999, ...Array(30).fill(100), 101], 0)).toBe(1);
  });
  it("breakout exits across the midpoint and otherwise holds", () => {
    const channel = Array.from({ length: 30 }, (_, i) => 90 + i);
    expect(breakout.decide([...channel, 100], 1)).toBe(0);
    expect(breakout.decide([...channel, 110], -1)).toBe(0);
    expect(breakout.decide([...channel, 110], 1)).toBe(1);
    expect(breakout.decide([...channel, 100], -1)).toBe(-1);
    expect(breakout.decide([...channel, 104.5], 0)).toBe(0);
  });
});
