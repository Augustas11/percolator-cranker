import { describe, expect, it } from "vitest";
import { basis } from "./strategies/basis";
import { hedger } from "./strategies/hedger";
import { houseStrategies, strategies } from "./strategies";
import type { Book, Direction } from "./strategies/types";

const NOW = 1_800_000_000_000, MARK = 0.3;
const env = { DRIP_FEED_URL: "https://feed.test/" };
const book = (current: Direction = 0, size = 0, now = NOW): Book => ({ mark: MARK, current, size, now });
const market = (median: unknown, extra: Record<string, unknown> = {}, dataMs = NOW - 60_000) =>
  ({ data_ms: dataMs, models: [{ id: "other/model", median: 9 }, { id: "qwen/qwen3.8-27b", median, thin: false, ...extra }] });
// A median whose log gap to MARK is `gap`.
const at = (gap: number) => MARK * Math.exp(gap);

function feed(...snapshots: unknown[]) {
  const urls: string[] = [];
  let i = 0;
  const get = async (url: string) => {
    urls.push(url);
    const next = snapshots[Math.min(i++, snapshots.length - 1)];
    if (next instanceof Error) throw next;
    return next;
  };
  return { get, urls };
}

describe("basis", () => {
  it("reads market.json and needs two fresh snapshots past entry before going long", async () => {
    const source = feed(market(at(0.1), {}, NOW - 120_000), market(at(0.1), {}, NOW - 120_000), market(at(0.1)));
    const strategy = basis(env, source.get);
    const first = await strategy.observe(book());
    expect(first).toMatchObject({ target: 0, inputs: { reason: "awaiting_confirm", streak: 1, entry: 0.08, exit: 0.02 } });
    expect(source.urls[0]).toBe("https://feed.test/market.json");
    // Same snapshot again does not count as a confirmation.
    expect((await strategy.observe(book())).target).toBe(0);
    const third = await strategy.observe(book());
    expect(third.target).toBe(1);
    expect(third.inputs).toMatchObject({ reason: "market_above_mark", streak: 2, median: at(0.1), mark: MARK, feed_age_s: 60 });
    expect(third.inputs.basis).toBeCloseTo(0.1);
  });
  it("goes short when the broad market sits below the mark", async () => {
    const strategy = basis({ ...env, DRIP_BASIS_CONFIRM: "1" }, feed(market(at(-0.1))).get);
    expect((await strategy.observe(book())).target).toBe(-1);
  });
  it("holds inside the hysteresis band and exits only once the gap closes", async () => {
    const strategy = basis({ ...env, DRIP_BASIS_CONFIRM: "1" }, async () => market(at(0.05)));
    expect((await strategy.observe(book(1, 0.5))).target).toBe(1);
    expect((await strategy.observe(book(-1, 0.5))).target).toBe(-1);
    expect((await strategy.observe(book(0))).target).toBe(0);
    const closing = basis({ ...env, DRIP_BASIS_CONFIRM: "1" }, async () => market(at(-0.01)));
    expect(await closing.observe(book(1, 0.5))).toMatchObject({ target: 0, inputs: { reason: "gap_closed" } });
  });
  it("flips only after confirmation on the other side", async () => {
    const strategy = basis(env, feed(market(at(-0.1), {}, NOW - 1), market(at(-0.1))).get);
    expect((await strategy.observe(book(1, 0.5))).target).toBe(1);
    expect((await strategy.observe(book(1, 0.5))).target).toBe(-1);
  });
  it("holds the current position on unreachable, stale, thin or missing data", async () => {
    const cases: [unknown, string][] = [
      [new Error("down"), "feed_unreachable"],
      [null, "feed_no_time"],
      [{ models: [] }, "feed_no_time"],
      [market(at(0.5), {}, NOW - 31 * 60_000), "feed_stale"],
      [{ data_ms: NOW, models: [] }, "model_missing"],
      [{ data_ms: NOW }, "model_missing"],
      [market(at(0.5), { thin: true }), "model_thin"],
      [market(null), "median_missing"],
      [market(0), "median_missing"],
      [market(undefined), "median_missing"],
    ];
    for (const [snapshot, reason] of cases) {
      for (const current of [-1, 0, 1] as Direction[]) {
        const strategy = basis({ ...env, DRIP_BASIS_CONFIRM: "1" }, feed(snapshot).get);
        expect(await strategy.observe(book(current, 1))).toMatchObject({ target: current, inputs: { reason } });
      }
    }
  });
  it("uses the configured model and thresholds and validates them", async () => {
    const strategy = basis({ ...env, DRIP_BASIS_MODEL: "other/model", DRIP_BASIS_ENTRY: "0.5", DRIP_BASIS_EXIT: "0.1", DRIP_BASIS_CONFIRM: "1" },
      async () => market(at(0)));
    expect((await strategy.observe(book())).target).toBe(1); // ln(9 / 0.3) > 0.5
    expect(() => basis({})).toThrow("DRIP_FEED_URL");
    expect(() => basis({ ...env, DRIP_BASIS_EXIT: "0.1" })).toThrow("below");
    expect(() => basis({ ...env, DRIP_BASIS_ENTRY: "-1" })).toThrow("DRIP_BASIS_ENTRY");
    expect(() => basis({ ...env, DRIP_BASIS_CONFIRM: "1.5" })).toThrow("integer");
  });
});

describe("hedger", () => {
  const long = hedger({ DRIP_HEDGE_SIDE: "long", DRIP_HEDGE_MTOK: "100" });
  const short = hedger({ DRIP_HEDGE_SIDE: "short", DRIP_HEDGE_MTOK: "100" });
  it("opens its budget on its own side when flat or on the wrong side", async () => {
    expect(await long.observe(book(0))).toMatchObject({ target: 1, size: 100, resize: false, inputs: { side: "long", target_notional: 30, held_notional: 0 } });
    expect(await short.observe(book(0))).toMatchObject({ target: -1, size: 100, resize: false });
    expect(await long.observe(book(-1, 100))).toMatchObject({ target: 1, resize: false });
    expect(await short.observe(book(1, 100))).toMatchObject({ target: -1, resize: false });
  });
  it("holds inside the 10% band and re-sizes outside it, on both sides", async () => {
    for (const [strategy, side] of [[long, 1], [short, -1]] as const) {
      expect((await strategy.observe(book(side, 100))).resize).toBe(false);
      expect((await strategy.observe(book(side, 91))).resize).toBe(false);
      expect((await strategy.observe(book(side, 110))).resize).toBe(false);
      expect((await strategy.observe(book(side, 89))).resize).toBe(true);
      expect((await strategy.observe(book(side, 111))).resize).toBe(true);
    }
    expect((await long.observe(book(1, 89))).inputs.drift).toBeCloseTo(0.11);
  });
  it("validates its configuration", () => {
    expect(() => hedger({})).toThrow("DRIP_HEDGE_SIDE");
    expect(() => hedger({ DRIP_HEDGE_SIDE: "both" })).toThrow("DRIP_HEDGE_SIDE");
    expect(() => hedger({ DRIP_HEDGE_SIDE: "long", DRIP_HEDGE_MTOK: "0" })).toThrow("DRIP_HEDGE_MTOK");
    expect(() => hedger({ DRIP_HEDGE_SIDE: "long", DRIP_HEDGE_BAND: "x" })).toThrow("DRIP_HEDGE_BAND");
  });
});

describe("registry", () => {
  it("keeps house agents out of the starter strategies and tags them", () => {
    expect(Object.keys(strategies)).toEqual(["trend", "meanrev", "breakout"]);
    expect(houseStrategies.basis(env)).toMatchObject({ name: "basis", house: true });
    expect(houseStrategies.hedger({ DRIP_HEDGE_SIDE: "short" })).toMatchObject({ name: "hedger", house: true });
  });
});
