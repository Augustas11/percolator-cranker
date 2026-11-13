import { describe, expect, it, vi } from "vitest";
import { clock, config, runAgent, type Clock, type Config, type Position } from "./agent";
import { basis } from "./strategies/basis";
import type { Book, HouseStrategy, Plan, Strategy } from "./strategies/types";

const house = (plan: (book: Book) => Plan): HouseStrategy => ({ name: "house", house: true, observe: async book => plan(book) });

const position: Position = {
  wallet: "test-wallet", sol_balance: 1, wallet_usdc: 1000,
  portfolio: "test-portfolio", capital: 500, equity: 500, mark_price: 100, position: null,
};

async function run(options: {
  ticks?: number;
  position?: Partial<Position>;
  target?: Strategy["decide"];
  strategy?: HouseStrategy;
  cfg?: Partial<Config>;
  error?: (name: string, count: number) => boolean;
} = {}) {
  const stop = new AbortController();
  const events: Record<string, unknown>[] = [], sleeps: number[] = [];
  const calls: { name: string; args?: Record<string, unknown> }[] = [];
  const state = { ...position, ...options.position };
  let now = 0;
  const time: Clock = {
    now: () => now,
    async sleep(ms) {
      sleeps.push(ms);
      now += ms;
      if (sleeps.length >= (options.ticks ?? 1)) stop.abort();
    },
  };
  const close = vi.fn(async () => {});
  await runAgent(options.strategy ?? { name: "test", decide: options.target ?? (() => 0) }, {
    ...config({}, stop.signal), log: event => events.push(event), ...options.cfg,
  }, {
    async call(name, args) {
      calls.push({ name, args });
      if (options.error?.(name, calls.filter(call => call.name === name).length)) throw new Error("test failure");
      if (name === "deposit") { state.portfolio = "created"; state.capital = 500; }
      if (name === "close_position") state.position = null;
      if (name === "open_position") state.position = { side: args!.side as "long" | "short", size: args!.size as number };
      return { ...state };
    },
    close,
  }, time);
  return { calls, events, sleeps, close };
}

describe("agent loop", () => {
  it("claims and deposits only when a portfolio is missing, then bootstraps once", async () => {
    const result = await run({ ticks: 2, position: { portfolio: null, wallet_usdc: 0 } });
    expect(result.calls.slice(0, 3)).toEqual([
      { name: "get_position" }, { name: "claim_test_funds" }, { name: "deposit", args: { amount_usdc: 500 } },
    ]);
    expect(result.calls.filter(call => call.name === "deposit")).toHaveLength(1);
    expect(result.events.filter(event => event.event === "bootstrap")).toHaveLength(1);
    expect(result.close).toHaveBeenCalledOnce();
  });
  it("funds an existing portfolio that has no capital", async () => {
    const result = await run({ position: { capital: 0 } });
    expect(result.calls.map(call => call.name)).toEqual(["get_position", "deposit", "get_position"]);
  });
  it("leaves a funded portfolio alone", async () => {
    const result = await run();
    expect(result.calls.map(call => call.name)).toEqual(["get_position", "get_position"]);
  });
  it("does not claim when the wallet already has enough USDC", async () => {
    const result = await run({ position: { portfolio: null } });
    expect(result.calls.map(call => call.name)).toEqual(["get_position", "deposit", "get_position"]);
  });
  it("flips by closing before opening and uses the configured size", async () => {
    const result = await run({ position: { position: { side: "short", size: 1 } }, target: () => 1, cfg: { sizeSol: 0.7 } });
    expect(result.calls.slice(-2)).toEqual([
      { name: "close_position" }, { name: "open_position", args: { side: "long", size: 0.7 } },
    ]);
    expect(result.events.filter(event => event.event === "trade")).toHaveLength(2);
  });
  it("reserves two trades for a flip before closing", async () => {
    const result = await run({ position: { position: { side: "short", size: 1 } }, target: () => 1, cfg: { maxTradesPerHour: 1 } });
    expect(result.calls.every(call => call.name === "get_position")).toBe(true);
    expect(result.events.some(event => event.event === "skip_budget")).toBe(true);
  });
  it("counts both sides of a flip and resets the rolling budget after an hour", async () => {
    let tick = 0;
    const result = await run({ ticks: 4, position: { position: { side: "short", size: 1 } },
      target: () => ++tick === 1 ? 1 : -1, cfg: { maxTradesPerHour: 2, intervalS: 1200 } });
    expect(result.events.filter(event => event.event === "skip_budget")).toHaveLength(2);
    expect(result.events.filter(event => event.event === "trade")).toHaveLength(4);
  });
  it("reads but stops trading on low SOL, logging once", async () => {
    const result = await run({ ticks: 3, position: { sol_balance: 0.001 }, target: () => 1 });
    expect(result.calls.every(call => call.name === "get_position")).toBe(true);
    expect(result.events.filter(event => event.event === "low_sol")).toHaveLength(1);
    expect(result.events.filter(event => event.event === "tick")).toHaveLength(3);
  });
  it("backs off errors up to ten minutes and resets after success", async () => {
    const result = await run({ ticks: 6, error: (name, count) => name === "get_position" && count <= 5 });
    expect(result.sleeps).toEqual([120000, 240000, 480000, 600000, 600000, 60000]);
    expect(result.events.filter(event => event.event === "error")).toHaveLength(5);
  });
  it("retries failed bootstrap and never opens after a failed close", async () => {
    const result = await run({ ticks: 2, position: { portfolio: null, position: { side: "short", size: 1 } },
      target: () => 1, error: (name, count) => name === "deposit" && count === 1 || name === "close_position" });
    expect(result.calls.filter(call => call.name === "deposit")).toHaveLength(2);
    expect(result.calls.some(call => call.name === "open_position")).toBe(false);
    expect(result.close).toHaveBeenCalledOnce();
  });
  it("does not trade when the target matches the current position", async () => {
    const result = await run({ position: { position: { side: "long", size: 0.5 } }, target: () => 1 });
    expect(result.calls.every(call => call.name === "get_position")).toBe(true);
  });
  it("bounds the price history at 200 samples", async () => {
    const lengths: number[] = [];
    await run({ ticks: 205, target: prices => { lengths.push(prices.length); return 0; } });
    expect(lengths.slice(-6)).toEqual(Array(6).fill(200));
  });
  it("closes MCP immediately on shutdown, unblocking a pending request without trading", async () => {
    const stop = new AbortController();
    const events: Record<string, unknown>[] = [];
    let rejectRequest: (error: Error) => void = () => {};
    const close = vi.fn(async () => rejectRequest(new Error("Connection closed")));
    const call = vi.fn(() => new Promise((_, reject) => {
      rejectRequest = reject;
      queueMicrotask(() => stop.abort());
    }));
    await runAgent({ name: "test", decide: () => 1 }, {
      ...config({}, stop.signal), log: event => events.push(event),
    }, { call, close });
    expect(close).toHaveBeenCalledOnce();
    expect(call).toHaveBeenCalledExactlyOnceWith("get_position");
    expect(events.map(event => event.event)).toEqual(["start"]);
  });
  it("interrupts the real clock on shutdown", async () => {
    vi.useFakeTimers();
    try {
      const stop = new AbortController();
      const sleep = clock.sleep(600000, stop.signal);
      stop.abort();
      await sleep;
      expect(vi.getTimerCount()).toBe(0);
      await clock.sleep(600000, stop.signal);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
  it("tags house agents and logs their inputs on every tick", async () => {
    const books: Book[] = [];
    const result = await run({ strategy: house(book => { books.push(book); return { target: 0, inputs: { basis: 0.01, reason: "hold" } }; }) });
    expect(result.events[0]).toEqual({ agent: "house", event: "start", house: true });
    expect(result.events.find(event => event.event === "tick")).toEqual({
      agent: "house", event: "tick", house: true, basis: 0.01, reason: "hold", mark: 100, target: 0, current: 0, size: 0.5, add: false, reduce: false,
    });
    expect(books).toEqual([{ mark: 100, current: 0, size: 0, equity: 500, now: 0 }]);
    expect(result.calls.every(call => call.name === "get_position")).toBe(true);
  });
  it("opens a house plan at its own size, adds and reduces as one order each", async () => {
    const opened = await run({ strategy: house(() => ({ target: -1, size: 120, inputs: {} })) });
    expect(opened.calls.at(-1)).toEqual({ name: "open_position", args: { side: "short", size: 120 } });
    const held = await run({ position: { position: { side: "long", size: 80 } }, strategy: house(() => ({ target: 1, size: 100, inputs: {} })) });
    expect(held.calls.every(call => call.name === "get_position")).toBe(true);
    const added = await run({ position: { position: { side: "short", size: 100 } }, strategy: house(() => ({ target: -1, size: 50, add: true, inputs: {} })) });
    expect(added.calls.slice(1)).toEqual([{ name: "get_position" }, { name: "open_position", args: { side: "short", size: 50 } }]);
    const reduced = await run({ position: { position: { side: "long", size: 150 } }, strategy: house(() => ({ target: 1, size: 50, reduce: true, inputs: {} })) });
    expect(reduced.calls.slice(1)).toEqual([{ name: "get_position" }, { name: "open_position", args: { side: "short", size: 50 } }]);
    expect(reduced.events.find(event => event.event === "trade")).toEqual({ agent: "house", event: "trade", action: "reduce", side: "short", size: 50 });
    const capped = await run({ position: { position: { side: "short", size: 100 } }, cfg: { maxTradesPerHour: 0 }, strategy: house(() => ({ target: -1, size: 50, add: true, inputs: {} })) });
    expect(capped.events.some(event => event.event === "skip_budget")).toBe(true);
    const flat = await run({ strategy: house(() => ({ target: 0, reduce: true, add: true, inputs: {} })) });
    expect(flat.calls.every(call => call.name === "get_position")).toBe(true);
  });
  it("refuses an invalid house size, a reduce that would flip, and bad position fields without trading", async () => {
    const cases: [Partial<Position>, Plan, string][] = [
      [{}, { target: 1, size: Number.NaN, inputs: {} }, "Invalid plan size"],
      [{ position: { side: "long", size: 50 } }, { target: 1, size: 50, reduce: true, inputs: {} }, "Reduce must be smaller than the position"],
      [{ position: { side: "long", size: Number.NaN } }, { target: 1, size: 50, add: true, inputs: {} }, "Invalid position size"],
      [{ equity: undefined as unknown as number }, { target: 1, size: 50, inputs: {} }, "Invalid equity"],
    ];
    for (const [state, plan, message] of cases) {
      const result = await run({ position: state, strategy: house(() => plan) });
      expect(result.calls.every(call => call.name === "get_position")).toBe(true);
      expect(result.events.some(event => event.event === "error" && event.message === message)).toBe(true);
    }
  });
  it("caps a hostile feed at the hourly trade budget", async () => {
    // New snapshot every tick, swinging the basis +-30% in pairs: the most flips the rules allow.
    let now = 0, k = 0;
    const strategy = basis({ DRIP_FEED_URL: "x" }, async () => {
      const gap = Math.floor(k++ / 2) % 2 ? -0.3 : 0.3;
      return { data_ms: now, models: [{ id: "qwen/qwen3.8-27b", median: 100 * Math.exp(gap) }] };
    });
    const observe = strategy.observe.bind(strategy);
    strategy.observe = book => { now = book.now; return observe(book); };
    const result = await run({ ticks: 180, strategy });
    const trades = result.events.filter(event => event.event === "trade").length;
    expect(trades).toBeGreaterThan(6);
    expect(trades).toBeLessThanOrEqual(6 * 3);
  });
  it("validates configuration", () => {
    const signal = new AbortController().signal;
    expect(() => config({ INTERVAL_S: "0" }, signal)).toThrow("INTERVAL_S");
    expect(() => config({ SIZE_SOL: "NaN" }, signal)).toThrow("SIZE_SOL");
    expect(() => config({ MAX_TRADES_PER_HOUR: "1.5" }, signal)).toThrow("integer");
    expect(config({ MAX_TRADES_PER_HOUR: "0" }, signal).maxTradesPerHour).toBe(0);
  });
});
