import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { basis, jsonFetcher } from "./strategies/basis";
import { hedger } from "./strategies/hedger";
import { houseStrategies, strategies } from "./strategies";
import type { Book, Direction, HouseStrategy } from "./strategies/types";

const T0 = 1_800_000_000_000, STEP = 300_000, MARK = 0.3;
const env = { DRIP_FEED_URL: "https://feed.test/", DRIP_BASIS_MIN_SAMPLES: "3" };
const book = (current: Direction = 0, size = 0, now = T0, equity = 500): Book => ({ mark: MARK, current, size, equity, now });
const model = (price: unknown, extra: Record<string, unknown> = {}) =>
  ({ id: "qwen/qwen3.8-27b", median: price, routed: { price }, thin: false, ...extra });
const snapshot = (price: unknown, dataMs: number, extra: Record<string, unknown> = {}) =>
  ({ data_ms: dataMs, models: [{ id: "other/model", median: 9 }, model(price, extra)] });
// A price whose log gap to MARK is `gap`.
const at = (gap: number) => MARK * Math.exp(gap);

// Serves one new 5-minute snapshot per observe, gaps given relative to MARK.
function series(gaps: number[]) {
  const urls: string[] = [];
  let i = 0;
  const get = async (url: string) => { urls.push(url); const k = Math.min(i++, gaps.length - 1); return snapshot(at(gaps[k]), T0 + k * STEP); };
  const tick = (strategy: HouseStrategy, current: Direction = 0) =>
    strategy.observe(book(current, 1, T0 + Math.min(i, gaps.length - 1) * STEP + 60_000));
  return { get, urls, tick };
}
// A standing gap of -5% (the broad market below the mark), then the given gaps.
const usual = (...tail: number[]) => [-0.05, -0.05, -0.05, ...tail];

describe("basis", () => {
  it("warms up on the first snapshots, then trades the deviation from its own mean", async () => {
    const feed = series(usual(-0.05, 0.05, 0.05));
    const strategy = basis(env, feed.get);
    for (let k = 0; k < 3; k++) expect(await feed.tick(strategy)).toMatchObject({ target: 0, inputs: { reason: "warming_up", samples: k } });
    expect(feed.urls[0]).toBe("https://feed.test/market.json");
    // A standing gap is not a signal on its own.
    expect(await feed.tick(strategy)).toMatchObject({ target: 0, inputs: { reason: "hold", source: "median" } });
    expect(await feed.tick(strategy)).toMatchObject({ target: 0, inputs: { reason: "awaiting_confirm", streak: 1 } });
    const long = await feed.tick(strategy);
    expect(long).toMatchObject({ target: 1, inputs: { reason: "market_above_usual", streak: 2, entry: 0.08, exit: 0.02, feed_age_s: 60 } });
    expect(long.inputs.deviation).toBeGreaterThan(0.08);
    expect(long.inputs.mean).toBeCloseTo((-0.05 * 4 + 0.05) / 5);
  });
  it("does not count a repeated snapshot as a confirmation or a new sample", async () => {
    let calls = 0;
    const strategy = basis({ ...env, DRIP_BASIS_CONFIRM: "2" }, async () => snapshot(at(calls++ < 3 ? -0.05 : 0.1), T0 + Math.min(calls, 4) * STEP));
    for (let k = 0; k < 4; k++) await strategy.observe(book(0, 0, T0 + 5 * STEP));
    const first = await strategy.observe(book(0, 0, T0 + 5 * STEP));
    const again = await strategy.observe(book(0, 0, T0 + 5 * STEP));
    expect(first.inputs.streak).toBe(again.inputs.streak);
    expect(again.target).toBe(0);
  });
  it("goes short below its usual gap, holds in the band and exits near the mean", async () => {
    const feed = series(usual(-0.2, -0.2, -0.14, -0.115));
    const strategy = basis(env, feed.get);
    for (let k = 0; k < 3; k++) await feed.tick(strategy);
    expect((await feed.tick(strategy)).target).toBe(0);
    expect((await feed.tick(strategy)).target).toBe(-1);
    expect((await feed.tick(strategy, -1)).target).toBe(-1); // inside the band: hold
    expect(await feed.tick(strategy, -1)).toMatchObject({ target: 0, inputs: { reason: "gap_back_to_usual" } });
  });
  it("forgets samples older than the window", async () => {
    const strategy = basis({ ...env, DRIP_BASIS_WINDOW_H: "0.25", DRIP_BASIS_MIN_SAMPLES: "2" }, series([-0.05, -0.05, -0.05, -0.05, -0.05]).get);
    const results = [];
    for (let k = 0; k < 5; k++) results.push(await strategy.observe(book(0, 0, T0 + k * STEP + 60_000)));
    expect(results.map(r => r.inputs.samples)).toEqual([0, 1, 2, 2, 2]);
  });
  it("reads the routed price when DRIP_BASIS_SOURCE=routed, with tighter default thresholds", async () => {
    let i = 0;
    const strategy = basis({ ...env, DRIP_BASIS_SOURCE: "routed", DRIP_BASIS_CONFIRM: "1" }, async () => {
      const k = i++;
      return { data_ms: T0 + k * STEP, models: [model(9, { routed: { price: at(k < 3 ? -0.4 : -0.37) } })] };
    });
    for (let k = 0; k < 3; k++) await strategy.observe(book(0, 0, T0 + k * STEP));
    const plan = await strategy.observe(book(0, 0, T0 + 3 * STEP));
    expect(plan).toMatchObject({ target: 1, inputs: { source: "routed", entry: 0.02, exit: 0.005, price: at(-0.37) } });
    const missing = basis({ ...env, DRIP_BASIS_SOURCE: "routed" }, async () => snapshot(0.3, T0, { routed: { price: null } }));
    expect(await missing.observe(book(1, 1))).toMatchObject({ target: 1, inputs: { reason: "price_missing" } });
  });
  it("holds the current position on unreachable, stale, thin or missing data", async () => {
    const cases: [unknown, string][] = [
      [new Error("down"), "feed_unreachable"],
      [null, "feed_no_time"],
      [{ models: [] }, "feed_no_time"],
      [snapshot(at(0.5), T0 - 31 * 60_000), "feed_stale"],
      [{ data_ms: T0, models: [] }, "model_missing"],
      [{ data_ms: T0 }, "model_missing"],
      [snapshot(at(0.5), T0, { thin: true }), "model_thin"],
      [snapshot(at(0.5), T0, { thin: "true" }), "model_thin"],
      [snapshot(at(0.5), T0, { thin: 1 }), "model_thin"],
      [snapshot(at(0.5), T0 + 61_000), "feed_in_future"],
      [snapshot(3e-7, T0), "price_implausible"],
      [snapshot(MARK * 5.1, T0), "price_implausible"],
      [snapshot(MARK / 5.1, T0), "price_implausible"],
      [snapshot(null, T0), "price_missing"],
      [snapshot(0, T0), "price_missing"],
      [snapshot(undefined, T0), "price_missing"],
    ];
    for (const [response, reason] of cases) {
      for (const current of [-1, 0, 1] as Direction[]) {
        const strategy = basis({ ...env, DRIP_BASIS_MIN_SAMPLES: "1", DRIP_BASIS_CONFIRM: "1" }, async () => {
          if (response instanceof Error) throw response;
          return response;
        });
        expect(await strategy.observe(book(current, 1))).toMatchObject({ target: current, inputs: { reason } });
      }
    }
  });
  it("does not flip on mark noise between feed updates", async () => {
    // Confirmed long on two snapshots; the same snapshot then looks like a short only because the mark moved.
    const feed = series(usual(0.1, 0.1));
    const strategy = basis(env, feed.get);
    for (let k = 0; k < 4; k++) await feed.tick(strategy);
    expect((await feed.tick(strategy)).target).toBe(1);
    for (let k = 0; k < 3; k++) {
      const plan = await strategy.observe({ mark: MARK * 1.5, current: 1, size: 1, equity: 500, now: T0 + 4 * STEP + 90_000 });
      expect(plan.inputs.deviation).toBeLessThan(-0.08);
      expect(plan).toMatchObject({ target: 1, inputs: { reason: "awaiting_confirm" } });
    }
  });
  it("keeps its history through a future or rewound timestamp", async () => {
    const snaps = [0, 1, 2, 3].map(k => snapshot(at(-0.05), T0 + k * STEP));
    let i = 0;
    const strategy = basis(env, async () => snaps[Math.min(i++, snaps.length - 1)]);
    for (let k = 0; k < 4; k++) await strategy.observe(book(0, 0, T0 + k * STEP));
    snaps.push(snapshot(at(-0.05), T0 + 100 * 3_600_000));
    expect(await strategy.observe(book(0, 0, T0 + 3 * STEP))).toMatchObject({ inputs: { reason: "feed_in_future" } });
    snaps.push(snapshot(at(-0.05), T0 + STEP));
    expect(await strategy.observe(book(0, 0, T0 + 3 * STEP))).toMatchObject({ inputs: { reason: "feed_went_back" } });
    snaps.push(snapshot(at(-0.05), T0 + 4 * STEP));
    expect(await strategy.observe(book(0, 0, T0 + 4 * STEP))).toMatchObject({ inputs: { reason: "hold", samples: 4 } });
  });
  it("holds on an implausible deviation and logs the feed error text", async () => {
    const feed = series(usual(-0.05, 0.7, 0.7));
    const strategy = basis(env, feed.get);
    for (let k = 0; k < 4; k++) await feed.tick(strategy);
    expect(await feed.tick(strategy)).toMatchObject({ target: 0, inputs: { reason: "deviation_implausible", max_deviation: 0.5 } });
    expect(await feed.tick(strategy, -1)).toMatchObject({ target: -1, inputs: { reason: "deviation_implausible" } });
    const down = basis(env, async () => { throw new Error("x".repeat(500)); });
    const plan = await down.observe(book());
    expect(plan.inputs).toMatchObject({ reason: "feed_unreachable" });
    expect(plan.inputs.error).toHaveLength(200);
  });
  it("validates its configuration", () => {
    expect(() => basis({})).toThrow("DRIP_FEED_URL");
    expect(() => basis({ ...env, DRIP_BASIS_SOURCE: "mark" })).toThrow("median or routed");
    expect(() => basis({ ...env, DRIP_BASIS_EXIT: "0.1" })).toThrow("below");
    expect(() => basis({ ...env, DRIP_BASIS_ENTRY: "-1" })).toThrow("DRIP_BASIS_ENTRY");
    expect(() => basis({ ...env, DRIP_BASIS_CONFIRM: "1.5" })).toThrow("integers");
    expect(() => basis({ ...env, DRIP_BASIS_MIN_SAMPLES: "0" })).toThrow("DRIP_BASIS_MIN_SAMPLES");
    expect(() => basis({ ...env, SIZE_SOL: "101" })).toThrow("one matcher fill");
    expect(() => basis({ ...env, DRIP_BASIS_WINDOW_H: "1", DRIP_BASIS_MIN_SAMPLES: "12" })).toThrow("does not fit");
    expect(() => basis({ ...env, DRIP_BASIS_MAX_DEVIATION: "0.05" })).toThrow("MAX_DEVIATION");
  });
});

describe("jsonFetcher", () => {
  let server: Server, base = "";
  beforeAll(async () => {
    server = createServer((req, res) => {
      if (req.url === "/json") res.end(JSON.stringify({ data_ms: 1 }));
      else if (req.url === "/html") res.end("<html>oops</html>");
      else if (req.url === "/big") res.end("[" + "1,".repeat(2_000) + "1]");
      else if (req.url === "/slow") setTimeout(() => res.end("{}"), 2_000);
      else { res.statusCode = 404; res.end(); }
    });
    await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
    base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });
  afterAll(() => { server.closeAllConnections(); server.close(); });
  const get = jsonFetcher(300, 1_000);
  it("parses JSON and rejects HTML, oversize, slow and failed responses", async () => {
    expect(await get(`${base}/json`)).toEqual({ data_ms: 1 });
    await expect(get(`${base}/html`)).rejects.toThrow();
    await expect(get(`${base}/big`)).rejects.toThrow("too large");
    await expect(get(`${base}/slow`)).rejects.toThrow();
    await expect(get(`${base}/missing`)).rejects.toThrow("HTTP 404");
  });
  it("feeds those failures to the basis agent as holds", async () => {
    for (const path of ["html", "big", "slow"]) {
      const strategy = basis({ DRIP_FEED_URL: `${base}/${path}#` }, get);
      expect(await strategy.observe(book(1, 1))).toMatchObject({ target: 1, inputs: { reason: "feed_unreachable" } });
    }
  });
});

describe("hedger", () => {
  const long = hedger({ DRIP_HEDGE_SIDE: "long", DRIP_HEDGE_MTOK: "100" });
  const short = hedger({ DRIP_HEDGE_SIDE: "short", DRIP_HEDGE_MTOK: "100" });
  it("opens its budget on its own side when flat or on the wrong side", async () => {
    expect(await long.observe(book(0))).toMatchObject({ target: 1, size: 100, inputs: { reason: "open", side: "long", target_notional: 30, held_notional: 0 } });
    expect(await short.observe(book(0))).toMatchObject({ target: -1, size: 100, inputs: { reason: "open" } });
    expect(await long.observe(book(-1, 100))).toMatchObject({ target: 1, size: 100, inputs: { reason: "wrong_side" } });
    expect(await short.observe(book(1, 100))).toMatchObject({ target: -1, size: 100, inputs: { reason: "wrong_side" } });
  });
  it("holds inside the 10% band, adds when short of it and re-sizes when over, on both sides", async () => {
    for (const [strategy, side] of [[long, 1], [short, -1]] as const) {
      for (const size of [100, 91, 110]) expect(await strategy.observe(book(side, size))).toMatchObject({ target: side, inputs: { reason: "within_band" } });
      expect(await strategy.observe(book(side, 89))).toMatchObject({ target: side, size: 11, add: true, inputs: { reason: "build" } });
      expect(await strategy.observe(book(side, 111))).toMatchObject({ target: side, size: 11, reduce: true, inputs: { reason: "over_target" } });
    }
    expect((await long.observe(book(1, 89))).inputs.drift).toBeCloseTo(0.11);
  });
  it("builds a budget larger than one matcher fill over several fills", async () => {
    const big = hedger({ DRIP_HEDGE_SIDE: "short", DRIP_HEDGE_MTOK: "250" });
    expect(await big.observe(book(0))).toMatchObject({ target: -1, size: 100 });
    expect(await big.observe(book(-1, 100))).toMatchObject({ target: -1, size: 100, add: true });
    expect(await big.observe(book(-1, 200))).toMatchObject({ target: -1, size: 50, add: true });
    expect(await big.observe(book(-1, 250))).toMatchObject({ inputs: { reason: "within_band" } });
    expect(await big.observe(book(-1, 400))).toMatchObject({ target: -1, size: 100, reduce: true });
    expect(await big.observe(book(-1, 300))).toMatchObject({ target: -1, size: 50, reduce: true });
  });
  it("stops growing before initial margin would use 80% of equity", async () => {
    const big = hedger({ DRIP_HEDGE_SIDE: "long", DRIP_HEDGE_MTOK: "1000" });
    // 300 units held + 100 more at $0.30 and 50% margin = $60 > 0.8 x $70.
    expect(await big.observe(book(1, 300, T0, 70))).toMatchObject({ target: 1, inputs: { reason: "margin_limit", margin_needed: 60 } });
    expect((await big.observe(book(1, 300, T0, 70))).add).toBeUndefined();
    expect(await big.observe(book(1, 300, T0, 500))).toMatchObject({ add: true, size: 100 });
    expect(await big.observe(book(0, 0, T0, 10))).toMatchObject({ target: 0, inputs: { reason: "margin_limit" } });
    // On the wrong side with no room for the hedge: close it rather than keep it.
    expect(await big.observe(book(-1, 50, T0, 10))).toMatchObject({ target: 0, inputs: { reason: "margin_limit" } });
    // Equity that is not a number never allows growth.
    for (const equity of [Number.NaN, -Infinity]) {
      expect(await big.observe(book(1, 300, T0, equity))).toMatchObject({ target: 1, inputs: { reason: "margin_limit" } });
    }
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
