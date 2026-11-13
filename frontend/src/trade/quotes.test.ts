import { expect, test } from "vitest";
import { availableRanges, change24h, entryPrice, quoteSides, sellerLadder, valueAt } from "./quotes";
import { sideLimits } from "../chain/matcher";
import type { Point } from "../compute/client";

const H = 3_600_000;

test("bid and ask sit the spread either side of the mark; an order pays the side it takes", () => {
  expect(quoteSides(0.3, 0)).toEqual({ bid: 0.3, ask: 0.3 });
  const q = quoteSides(0.3, 50)!;
  expect(q.bid).toBeCloseTo(0.2985, 10);
  expect(q.ask).toBeCloseTo(0.3015, 10);
  expect(entryPrice("long", 0.3, 50)).toBeCloseTo(0.3015, 10);
  expect(entryPrice("short", 0.3, 50)).toBeCloseTo(0.2985, 10);
  for (const [mark, spread] of [[0, 0], [NaN, 0], [0.3, -1], [0.3, NaN]]) expect(quoteSides(mark, spread)).toBeNull();
  expect(entryPrice("long", 0, 0)).toBeNaN();
});

test("the seller ladder is dearest first, drops unpriced quotes, and places the mark and index", () => {
  const quote = (provider: string, price: number | null) => ({ provider, quantization: "fp8", price, up: true }) as never;
  const rows = sellerLadder({ quotes: [quote("a", 0.2), quote("b", 0.5), quote("c", null), quote("d", 0.3), quote("e", 0.4)] }, 0.3, 0.35);
  expect(rows.map(r => [r.kind, r.price])).toEqual([["quote", 0.5], ["quote", 0.4], ["index", 0.35], ["mark", 0.3], ["quote", 0.3], ["quote", 0.2]]);
  // At a tie the mark sits above the index, both above quotes; a missing marker is left out.
  expect(sellerLadder({ quotes: [quote("a", 0.3)] }, 0.3, 0.3).map(r => r.kind)).toEqual(["mark", "index", "quote"]);
  expect(sellerLadder({ quotes: [quote("a", 0.3)] }, null, NaN).map(r => r.kind)).toEqual(["quote"]);
});

test("step series value, 24 h change, and only the ranges the data covers", () => {
  const pts: Point[] = [[0, 1], [10 * H, 2], [30 * H, 2.5]];
  expect(valueAt(pts, -1)).toBeNull();
  expect(valueAt(pts, 20 * H)).toBe(2);
  expect(change24h(pts, 30 * H)).toBeCloseTo(150, 10);
  expect(change24h(pts, 20 * H)).toBeNull();
  expect(change24h([[0, 1]], 0)).toBeNull();
  expect(availableRanges(pts).map(r => r.label)).toEqual(["24 h"]);
  expect(availableRanges([[0, 1], [7 * 24 * H, 1]]).map(r => r.label)).toEqual(["24 h", "7 d"]);
  expect(availableRanges([[0, 1], [12 * H, 1]])).toEqual([]);
});

test("each side's fill limit respects the per-fill cap and the inventory cap", () => {
  expect(sideLimits(100, 0, 1000)).toEqual({ long: 100, short: 100 });
  expect(sideLimits(100, -980, 1000)).toEqual({ long: 20, short: 100 });
  expect(sideLimits(100, 1000, 1000)).toEqual({ long: 100, short: 0 });
  expect(sideLimits(100, -1000, 1000)).toEqual({ long: 0, short: 100 });
});
