import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test, vi } from "vitest";
import App from "./App";
import { MockMarketClient } from "./market/mock";
import { MockIndexerClient } from "./indexer/mock";
import { MockComputeFeedClient } from "./compute/mock";
import type { ComputeFeedClient } from "./compute/client";
import type { IndexerClient } from "./indexer/client";
import type { MarketClient } from "./market/types";
import { ChainError } from "./chain/errors";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

async function mount(client: MarketClient, props: { indexer?: IndexerClient | null; computeFeed?: ComputeFeedClient | null } = {}) {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => { root.render(<App client={client} {...props} />); });
  const button = (text: string) => Array.from(container.querySelectorAll("button")).find(b => b.textContent === text);
  return {
    container,
    button,
    async click(text: string) {
      const b = button(text);
      expect(b, text).toBeDefined();
      await act(async () => { b!.click(); });
    },
    async input(id: string, value: string) {
      const element = container.querySelector<HTMLInputElement>(`#${id}`)!;
      await act(async () => {
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(element, value);
        element.dispatchEvent(new Event("input", { bubbles: true }));
      });
    },
    stats: () => Array.from(container.querySelectorAll(".tx-stats > div"), d => [d.querySelector("dt")!.textContent, d.querySelector("dd")!.textContent]),
    async unmount() { await act(async () => { root.unmount(); }); container.remove(); },
  };
}

test("connects, deposits, opens a long, and keeps rendering after a tick", async () => {
  const client = new MockMarketClient({ autoTick: false, seed: 1 });
  const ui = await mount(client);
  const { container } = ui;
  try {
    expect(container.querySelector("h1")!.textContent).toBe("COMPUTE-PERP");
    expect(container.querySelector(".trade-sub")!.textContent).toBe("USD per 1M tokens · Qwen3.8-27B");
    expect(container.querySelector(".tx-why")!.getAttribute("href")).toBe("/compute");
    expect(container.querySelector(".tx-why")!.textContent).toBe("What moves this price");
    expect(container.querySelector('[aria-labelledby="market-heading"] dd')!.textContent).toBe("$0.3036");
    expect(container.querySelector<HTMLInputElement>("#size")!.value).toBe("100");
    expect(container.querySelector(".trade-notional")!.textContent).toBe("$30.36 notional");
    expect(container.querySelector("#size-help")!.textContent).toContain("up to 100 units per fill");
    expect(Array.from(container.querySelectorAll(".trade-cases li"), li => li.textContent)).toEqual([
      "Long hedges an inference bill: it gains when prices rise.", "Short hedges a seller's or miner's revenue: it gains when prices fall.",
    ]);
    expect(container.textContent!.match(/Test tokens/gi)).toHaveLength(1);
    expect(container.textContent).not.toMatch(/SOL-PERP|\bSOL\b/);
    // Before a wallet: prompts only, no balances, no health badge.
    expect(container.querySelector(".tx-account")!.textContent).toBe("Connect a wallet to see your balance and deposit test USDC.");
    expect(container.querySelector('[aria-labelledby="position-heading"]')!.textContent).toBe("Your positionConnect a wallet to see your position.");
    expect(container.querySelector(".badge")).toBeNull();
    // Summary: entry at the ask (spread 0, so the mark), the market's fee, the engine's initial margin. No leverage control: the order sends none.
    const summary = () => Array.from(container.querySelectorAll(".tx-summary dt, .tx-summary dd"), el => el.textContent);
    expect(summary()).toEqual(["Entry price", "$0.3036", "Fee (15 bps)", "$0.05", "Margin required (50%)", "$15.18", "Max leverage", "2x", "Max this side", "100 Mtok"]);
    expect(container.querySelector("#leverage, input[type=range]")).toBeNull();

    await ui.click("Connect wallet");
    expect(ui.button("Connect wallet")).toBeUndefined();
    expect(container.textContent).toContain("No open position");
    await ui.input("amount", "1000");
    await ui.click("Deposit");
    expect(client.getSnapshot().portfolio.collateral).toBe(1000);
    expect(Array.from(container.querySelectorAll(".tx-account dt, .tx-account dd"), d => d.textContent)).toEqual([
      "Collateral", "$1,000.00", "Equity", "$1,000.00", "Free collateral", "$1,000.00", "Health", "healthy", "In your wallet", "9,000 test USDC"]);
    // Free collateral over margin plus fee per unit: 1000 / (mark × (0.5 + 0.0015)).
    const most = Math.floor(1000 / (client.getSnapshot().mark.price * 0.5015) * 100) / 100;
    expect(summary().slice(-2)).toEqual(["Max for your collateral", `${most.toLocaleString("en-US", { maximumFractionDigits: 2 })} Mtok`]);
    await ui.click("Short");
    expect(ui.button("Open short")).toBeDefined();
    await ui.click("Long");
    await ui.input("size", "150");
    expect(container.querySelector("#size-help")!.className).toContain("is-over");
    await ui.click("Open long");
    expect(container.querySelector(".error")!.textContent).toBe("The liquidity account fills up to 100 units (100 × 1M tokens) per trade. Split a larger order into several trades.");
    expect(client.getSnapshot().portfolio.legs).toHaveLength(0);
    await ui.click("25");
    expect(container.querySelector<HTMLInputElement>("#size")!.value).toBe("25");
    await ui.input("size", "50");
    expect(container.querySelector(".trade-notional")!.textContent).toBe("$15.18 notional");
    await ui.click("Open long");

    const position = container.querySelector('[aria-labelledby="position-heading"]')!;
    const cells = () => Array.from(position.querySelectorAll("tbody td"), td => td.textContent);
    expect(cells().slice(0, 5)).toEqual(["COMPUTE-PERP", "long", "50 × 1M tokens", "$0.3036", "$0.3036"]);
    expect(client.getSnapshot().portfolio.health.initialMarginUsed).toBeCloseTo(7.59, 6);
    expect(cells()[6]).toBe("$7.59");
    await ui.click("Fills");
    expect(container.querySelector("#bottom-panel-fills tbody")!.textContent).toContain("$0.3036");
    const markBefore = container.querySelector('[aria-labelledby="market-heading"] dd')!.textContent;
    await act(async () => { client.tick(); });
    expect(container.querySelector('[aria-labelledby="market-heading"] dd')!.textContent).not.toBe(markBefore);
    expect(cells().slice(1, 3)).toEqual(["long", "50 × 1M tokens"]);
  } finally {
    await ui.unmount();
    client.dispose();
  }
});

test("shows unknown prices honestly and disables trading while the matcher is pending", async () => {
  const client = Object.assign(new MockMarketClient({ autoTick: false }), { tradingReady: false });
  await client.connect();
  await client.deposit(1000);
  await client.openPosition("long", 50, 2);
  const snapshot = client.getSnapshot();
  const unknown = { ...snapshot, portfolio: { ...snapshot.portfolio, realizedPnl: NaN,
    legs: snapshot.portfolio.legs.map(leg => ({ ...leg, entryPrice: NaN, unrealizedPnl: NaN })),
    health: { equity: NaN, initialMarginUsed: NaN, maintenanceMargin: NaN, freeCollateral: NaN, ratio: NaN, status: "unknown" as const },
  } };
  client.getSnapshot = () => unknown;
  const ui = await mount(client);
  try {
    expect(ui.container.textContent).toContain("Trading is off in this build: no matcher is set for COMPUTE-PERP.");
    expect(ui.container.querySelector(".tx-quotes")!.textContent).toContain("No matcher is set for COMPUTE-PERP in this build.");
    // Unknown values are left out, not shown as dashes; no "unknown" health badge.
    const health = ui.container.querySelector('[aria-labelledby="health-heading"]')!;
    expect(Array.from(health.querySelectorAll("dt, dd")).map(dd => dd.textContent)).toEqual(["Collateral", "$1,000.00"]);
    expect(ui.container.querySelector(".badge")).toBeNull();
    const position = ui.container.querySelector('[aria-labelledby="position-heading"]')!;
    expect(Array.from(position.querySelectorAll("th"), th => th.textContent)).toEqual(["Market", "Side", "Size", "Mark", "Action"]);
    expect(Array.from(position.querySelectorAll("tbody td"), td => td.textContent).slice(0, 4)).toEqual(["COMPUTE-PERP", "long", "50 × 1M tokens", "$0.3036"]);
    expect(position.textContent).not.toMatch(/—|Realized/);
    for (const label of ["Open long", "Close position"]) expect(ui.button(label)!.disabled).toBe(true);
  } finally {
    await ui.unmount();
    client.dispose();
  }
});

test("renders infinite health values as unavailable", async () => {
  const client = new MockMarketClient({ autoTick: false });
  const snapshot = client.getSnapshot();
  const unknown = { ...snapshot, portfolio: { ...snapshot.portfolio, collateral: Infinity,
    health: { equity: Infinity, initialMarginUsed: -Infinity, maintenanceMargin: Infinity, freeCollateral: -Infinity, ratio: Infinity, status: "healthy" as const },
  } };
  await client.connect();
  const owner = client.getSnapshot().portfolio.owner;
  const connected = { ...unknown, portfolio: { ...unknown.portfolio, owner } };
  client.getSnapshot = () => connected;
  const ui = await mount(client);
  try {
    const health = ui.container.querySelector('[aria-labelledby="health-heading"]')!;
    expect(Array.from(health.querySelectorAll("dt, dd")).map(dd => dd.textContent)).toEqual(["Status", "healthy"]);
  } finally {
    await ui.unmount();
    client.dispose();
  }
});

test("a malformed benchmark model in market.json does not take the page down", async () => {
  const client = new MockMarketClient({ autoTick: false });
  const feed = new MockComputeFeedClient();
  const good = await feed.market();
  const broken = { ...good, models: good.models.map(m => m.id === "qwen/qwen3.8-27b" ? { ...m, quotes: undefined, series: undefined, median: "x" } : m) };
  vi.spyOn(feed, "market").mockResolvedValue(broken as never);
  const history = await feed.history();
  vi.spyOn(feed, "history").mockResolvedValue({ ...history, points: [...history.points, ["bad", 1], null, [1, Infinity]] } as never);
  const ui = await mount(client, { computeFeed: feed });
  try {
    expect(ui.container.querySelector("h1")!.textContent).toBe("COMPUTE-PERP");
    expect(ui.stats().map(s => s[0])).not.toContain("Index (all sellers)");
    expect(ui.container.querySelector(".tx-ladder li:not(.tx-ladder-head)")).toBeNull();
    expect(ui.container.querySelector(".tx-sides")).not.toBeNull();
    expect(ui.container.querySelector("svg.tx-chart")).not.toBeNull();
    expect(ui.button("Connect wallet")).toBeDefined();
  } finally { await ui.unmount(); client.dispose(); vi.restoreAllMocks(); }

  // Models that are not an array at all: no benchmark, no crash.
  const c2 = new MockMarketClient({ autoTick: false });
  const f2 = new MockComputeFeedClient();
  vi.spyOn(f2, "market").mockResolvedValue({ ...good, models: { nope: 1 } } as never);
  const ui2 = await mount(c2, { computeFeed: f2 });
  try {
    expect(ui2.container.querySelector("h1")!.textContent).toBe("COMPUTE-PERP");
    expect(ui2.container.querySelector(".tx-quotes")!.textContent).toContain("The feed has no quotes for Qwen3.8-27B right now.");
  } finally { await ui2.unmount(); c2.dispose(); vi.restoreAllMocks(); }
});

test("market bar: each stat only when its source has a value", async () => {
  // No feed, no indexer: only the mark. Funding has no source anywhere, so it never shows.
  const bare = new MockMarketClient({ autoTick: false });
  let ui = await mount(bare);
  try {
    expect(ui.stats()).toEqual([["Mark", "$0.3036"]]);
    expect(ui.container.querySelector("#bottom-tab-funding")).toBeNull();
    expect(ui.container.textContent).not.toMatch(/funding|per hour/i);
  } finally { await ui.unmount(); bare.dispose(); }

  // Every source: the feed adds the index and 24 h change, the indexer 24 h volume, 24 h fills and open interest.
  const client = new MockMarketClient({ autoTick: false });
  ui = await mount(client, { indexer: new MockIndexerClient(), computeFeed: new MockComputeFeedClient() });
  try {
    expect(ui.stats().map(s => s[0])).toEqual(["Mark", "Index (all sellers)", "Paid now (all sellers)", "24 h change", "24 h volume", "24 h fills", "Open interest"]);
    expect(Object.fromEntries(ui.stats())["24 h volume"]).toBe("$45.54");
    expect(Object.fromEntries(ui.stats())["24 h fills"]).toBe("3");
    expect(Object.fromEntries(ui.stats())["Open interest"]).toBe("$157.90 · 250 Mtok");
  } finally { await ui.unmount(); client.dispose(); }

  // An older indexer without the 24 h fields: all-time volume instead, no 24 h fills.
  const c1 = new MockMarketClient({ autoTick: false });
  ui = await mount(c1, { indexer: new MockIndexerClient({ legacy: true }) });
  try {
    expect(ui.stats().map(s => s[0])).toEqual(["Mark", "Volume (all time)", "Open interest"]);
    expect(Object.fromEntries(ui.stats())["Volume (all time)"]).toBe("$18,640.00");
  } finally { await ui.unmount(); c1.dispose(); }

  // An indexer serving another market: its numbers are not shown as COMPUTE-PERP's.
  const other = new MockIndexerClient();
  const stats = await other.stats();
  vi.spyOn(other, "stats").mockResolvedValue({ ...stats, market: "HyMKxfct1S9aFtq21GG3FUYUViQkmpZdk5pwGRtAz48C" });
  const c2 = new MockMarketClient({ autoTick: false });
  ui = await mount(c2, { indexer: other });
  try {
    expect(ui.stats().map(s => s[0])).toEqual(["Mark"]);
    await ui.click("Trades");
    expect(ui.container.querySelector("#book-panel-trades")!.textContent).toContain("The indexer reads a different market, not COMPUTE-PERP.");
  } finally { await ui.unmount(); c2.dispose(); vi.restoreAllMocks(); }
});

test("quotes: bid and ask from the mark and spread, and the seller ladder with the mark and index", async () => {
  const client = new MockMarketClient({ autoTick: false });
  const feed = new MockComputeFeedClient();
  const ui = await mount(client, { computeFeed: feed });
  try {
    const sides = Array.from(ui.container.querySelectorAll(".tx-sides > div"), d => Array.from(d.querySelectorAll("dd"), dd => dd.textContent));
    expect(sides).toEqual([["$0.3036", "up to 100 Mtok"], ["$0.3036", "up to 100 Mtok"]]);
    expect(ui.container.querySelector(".tx-quotes")!.textContent).toContain("Spread 0 bps: the account fills at the mark, plus a 15 bps fee.");
    const rows = Array.from(ui.container.querySelectorAll(".tx-ladder li:not(.tx-ladder-head)"));
    const prices = rows.map(r => Number(r.querySelector(".p")!.textContent!.replace("$", "")));
    expect(prices).toEqual([...prices].sort((a, b) => b - a));
    const bench = (await feed.market()).models.find(m => m.id === "qwen/qwen3.8-27b")!;
    expect(rows.filter(r => r.classList.contains("q"))).toHaveLength(bench.quotes.filter(q => q.price !== null).length);
    const mark = rows.findIndex(r => r.classList.contains("m-mark")), index = rows.findIndex(r => r.classList.contains("m-index"));
    expect(rows[mark].textContent).toBe("$0.3036Mark");
    expect(rows[index].querySelector(".p")!.textContent).toBe(`$${bench.median.toFixed(4)}`);
    for (const r of rows.slice(0, mark)) expect(r.className).toContain("above");
    for (const r of rows.slice(mark + 1).filter(r => r.classList.contains("q"))) expect(r.className).toContain("below");
  } finally { await ui.unmount(); client.dispose(); }
});

test("tabs switch panels in the book and the bottom panel, with arrow keys", async () => {
  const client = new MockMarketClient({ autoTick: false });
  const ui = await mount(client, { indexer: new MockIndexerClient() });
  const shown = (id: string) => Array.from(ui.container.querySelectorAll(`[id^="${id}-panel-"]`)).filter(p => !(p as HTMLElement).hidden).map(p => p.id);
  try {
    expect(shown("book")).toEqual(["book-panel-quotes"]);
    await ui.click("Trades");
    expect(shown("book")).toEqual(["book-panel-trades"]);
    const trades = Array.from(ui.container.querySelectorAll("#book-panel-trades tbody tr"), tr => Array.from(tr.querySelectorAll("td"), td => td.textContent).slice(1));
    // Market-wide executed fills from /trades; a fill with no price shows its size and an empty price.
    expect(trades).toEqual([["Long", "50", "$0.3036", "7cVf…kxXy"], ["Short", "150", "$0.3036", "J4DM…22S4"], ["Long", "25", "", "9xQe…VFin"]]);
    expect(ui.container.querySelector<HTMLAnchorElement>("#book-panel-trades tbody td:last-child a")!.href).toBe("https://explorer.solana.com/address/7cVfgArCheMR6Cs4t6vz5rfnqd56vZq4ndaBrY5xkxXy?cluster=devnet");
    expect(shown("bottom")).toEqual(["bottom-panel-positions"]);
    const tab = ui.container.querySelector<HTMLButtonElement>("#bottom-tab-positions")!;
    await act(async () => { tab.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true })); });
    expect(shown("bottom")).toEqual(["bottom-panel-fills"]);
    expect(ui.container.querySelector("#bottom-tab-fills")!.getAttribute("aria-selected")).toBe("true");
    await ui.click("Account");
    expect(shown("bottom")).toEqual(["bottom-panel-account"]);
    expect(Array.from(ui.container.querySelectorAll("#bottom-tab-positions ~ [role=tab], #bottom-tab-positions"), t => t.textContent)).toEqual(["Positions", "Fills", "Account"]);
  } finally { await ui.unmount(); client.dispose(); }
});

test("mobile tabs pick the section on screen", async () => {
  const client = new MockMarketClient({ autoTick: false });
  const ui = await mount(client, { indexer: new MockIndexerClient() });
  try {
    const main = ui.container.querySelector("main.tx")!;
    const tabs = Array.from(ui.container.querySelectorAll(".tx-mtabs [role=tab]"), t => t.textContent);
    expect(tabs).toEqual(["Trade", "Quotes", "Trades", "Positions"]);
    expect(main.getAttribute("data-mobile")).toBe("trade");
    expect(ui.container.querySelector(".tx-order")!.classList.contains("m-trade")).toBe(true);
    await act(async () => { ui.container.querySelector<HTMLButtonElement>("#m-tab-trades")!.click(); });
    expect(main.getAttribute("data-mobile")).toBe("trades");
    // The book follows the mobile tab, so Trades shows even though its own tab bar is hidden on phones.
    expect((ui.container.querySelector("#book-panel-trades") as HTMLElement).hidden).toBe(false);
    await act(async () => { ui.container.querySelector<HTMLButtonElement>("#m-tab-positions")!.click(); });
    expect(main.getAttribute("data-mobile")).toBe("positions");
    expect(ui.container.querySelector(".tx-bottom")!.classList.contains("m-positions")).toBe(true);
  } finally { await ui.unmount(); client.dispose(); }
});

test("the routed price: a market-bar stat and a chart line, both hidden without the feed field; legend toggles lines", async () => {
  const feed = new MockComputeFeedClient();
  const market = await feed.market();
  const bench = market.models.find(m => m.id === "qwen/qwen3.8-27b")!;
  // 13 index reads over the last 2 h, enough for the index line.
  const now = Date.now();
  const withIndex = { ...market, models: market.models.map(m => m === bench ? { ...m, series: Array.from({ length: 13 }, (_, i) => [now - (12 - i) * 600_000, m.median] as [number, number]) } : m) };
  vi.spyOn(feed, "market").mockResolvedValue(withIndex);
  const client = new MockMarketClient({ autoTick: false });
  let ui = await mount(client, { computeFeed: feed });
  try {
    expect(Object.fromEntries(ui.stats())["Paid now (all sellers)"]).toBe("$" + bench.routed!.price.toFixed(4));
    const legend = () => Array.from(ui.container.querySelectorAll(".tx-legend button"), b => [b.textContent, b.getAttribute("aria-pressed")]);
    // Default on: Mark and Paid now; the list-price index is off until clicked.
    expect(legend()).toEqual([["Mark", "true"], ["Paid now (all sellers)", "true"], ["Index (all sellers)", "false"]]);
    expect(ui.container.querySelector(".tx-routed-line")).not.toBeNull();
    expect(ui.container.querySelector(".tx-index-line")).toBeNull();
    await ui.click("Index (all sellers)");
    expect(ui.container.querySelector(".tx-index-line")).not.toBeNull();
    await ui.click("Paid now (all sellers)");
    expect(ui.container.querySelector(".tx-routed-line")).toBeNull();
    expect(legend()[1]).toEqual(["Paid now (all sellers)", "false"]);
    await ui.click("Mark");
    expect(ui.container.querySelector(".tx-mark-line")).toBeNull();
    await ui.click("Index (all sellers)");
    expect(ui.container.querySelector(".tx-chart-frame")!.textContent).toContain("Every line is off. Turn one on in the legend.");
  } finally { await ui.unmount(); client.dispose(); }

  // Absent: no stat, no line, no legend entry.
  const f2 = new MockComputeFeedClient();
  vi.restoreAllMocks();
  vi.spyOn(f2, "market").mockResolvedValue({ ...market, routed: undefined, models: market.models.map(m => ({ ...m, routed: undefined })) });
  const c2 = new MockMarketClient({ autoTick: false });
  ui = await mount(c2, { computeFeed: f2 });
  try {
    expect(ui.stats().map(s => s[0])).not.toContain("Paid now (all sellers)");
    expect(ui.container.querySelector(".tx-routed-line")).toBeNull();
    expect(Array.from(ui.container.querySelectorAll(".tx-legend button"), b => b.textContent)).toEqual(["Mark"]);
  } finally { await ui.unmount(); c2.dispose(); vi.restoreAllMocks(); }
});

test("chart empty states: one pushed mark with no live mark, and none at all; no leverage row before the market is read", async () => {
  const t = Date.UTC(2026, 10, 12, 14, 5);
  for (const [points, text] of [
    [[[t, 0.3036]], "One mark pushed so far: $0.3036 at 12 Nov, 14:05 UTC. The chart starts with the second."],
    [[], "No mark has been pushed yet."],
  ] as [[number, number][], string][]) {
    const client = new MockMarketClient({ autoTick: false });
    const snapshot = client.getSnapshot();
    const unread = { ...snapshot, mark: { ...snapshot.mark, price: NaN }, market: { ...snapshot.market, maxLeverage: NaN } };
    client.getSnapshot = () => unread;
    const feed = new MockComputeFeedClient();
    const history = await feed.history();
    vi.spyOn(feed, "history").mockResolvedValue({ ...history, points });
    const ui = await mount(client, { computeFeed: feed });
    try {
      expect(ui.container.querySelector(".tx-chart-empty")!.textContent).toBe(text);
      expect(ui.container.querySelector(".tx-summary")?.textContent ?? "").not.toContain("Max leverage");
      expect(ui.container.textContent).not.toContain("NaN");
    } finally { await ui.unmount(); client.dispose(); vi.restoreAllMocks(); }
  }
});

test("a failed send shows a plain sentence, with the raw chain text behind Details", async () => {
  const client = new MockMarketClient({ autoTick: false });
  const ui = await mount(client);
  try {
    await ui.click("Connect wallet");
    await ui.input("amount", "1000");
    await ui.click("Deposit");
    vi.spyOn(client, "openPosition").mockRejectedValue(new ChainError("You declined the transaction in your wallet.", "WalletSignTransactionError: User rejected the request."));
    await ui.input("size", "10");
    await ui.click("Open long");
    expect(ui.container.querySelector(".error")!.textContent).toBe("You declined the transaction in your wallet.");
    const details = ui.container.querySelector(".tx-error-details")!;
    expect(details.querySelector("summary")!.textContent).toBe("Details");
    expect(details.querySelector("pre")!.textContent).toBe("WalletSignTransactionError: User rejected the request.");
    // A plain error with no separate raw text gets no disclosure.
    vi.mocked(client.openPosition).mockRejectedValue(new Error("Something else."));
    await ui.click("Open long");
    expect(ui.container.querySelector(".error")!.textContent).toBe("Something else.");
    expect(ui.container.querySelector(".tx-error-details")).toBeNull();
  } finally { await ui.unmount(); client.dispose(); vi.restoreAllMocks(); }
});
