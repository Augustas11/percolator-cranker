import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test } from "vitest";
import Arena from "../pages/Arena";
import { IndexerHttpError } from "../indexer/client";
import type { IndexerClient, Leaderboard as Board } from "../indexer/client";
import { MockIndexerClient } from "../indexer/mock";
import { site } from "../site/config";
import { OTHER_MARKET } from "../Indexer";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const entry = (wallet: string, pnl: string, trades: number, deposits = "500000000") =>
  ({ rank: 1, wallet, pnl, equity: "0", net_deposits: deposits, trades, open_positions: 0, portfolios: ["p"] });

async function render(indexer: IndexerClient) {
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () => { root.render(<Arena indexer={indexer} />); });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return { container, unmount: () => act(() => root.unmount()) };
}
const stats = (c: HTMLElement) => Object.fromEntries(Array.from(c.querySelectorAll(".arena-stats > div"), d => [d.querySelector("dt")!.textContent, d.querySelector("dd")!.textContent]));
const withBoard = (entries: Board["entries"], base = new MockIndexerClient()): IndexerClient => ({
  stats: () => base.stats(), fills: p => base.fills(p), positions: o => base.positions(o), season: () => base.season(),
  recentFills: n => base.recentFills(n), trades: n => base.trades(n),
  leaderboard: async () => ({ as_of_slot: 1, note: "n", entries }),
});

test("rows with no trades are hidden and Traders equals the rows shown; zero formats without a sign", async () => {
  const house = site.arena.house[0].wallet;
  const { container, unmount } = await render(withBoard([
    entry("Aaaa1111111111111111111111111111111111111111", "-400", 2),
    entry("Bbbb1111111111111111111111111111111111111111", "0", 0),
    entry(house, "0", 0),
  ]));
  expect(container.querySelectorAll(".arena-board tbody tr")).toHaveLength(1);
  expect(container.querySelector(".house-tag")).toBeNull();
  expect(stats(container).Traders).toBe("1");
  const cells = Array.from(container.querySelectorAll(".arena-board tbody td"), td => td.textContent);
  expect(cells.slice(0, 4)).toEqual(["1", "Aaaa…1111", "$0.00", "0.0%"]);
  expect(container.textContent).not.toContain("House agents trade");
  unmount();
});

test("an indexer for a different market shows no data", async () => {
  const base = new MockIndexerClient();
  const other: IndexerClient = { ...withBoard([entry("Aaaa1111111111111111111111111111111111111111", "5", 1)], base), stats: async () => ({ ...(await base.stats()), market: "11111111111111111111111111111111" }) };
  const { container, unmount } = await render(other);
  expect(container.textContent).toContain(OTHER_MARKET);
  expect(container.querySelector("tbody")).toBeNull();
  expect(container.querySelector(".arena-stats")).toBeNull();
  unmount();
});

test("24 h stats show when sent and are absent on a legacy indexer", async () => {
  let r = await render(new MockIndexerClient());
  expect(stats(r.container)["24 h volume"]).toBe("$45.54");
  expect(stats(r.container)["24 h fills"]).toBe("3");
  r.unmount();
  r = await render(new MockIndexerClient({ legacy: true }));
  expect(r.container.textContent).not.toContain("24 h");
  r.unmount();
});

test("recent fills come from /trades", async () => {
  const { container, unmount } = await render(new MockIndexerClient());
  const rows = Array.from(container.querySelectorAll(".arena-fills tbody tr"));
  expect(rows).toHaveLength(3);
  const cells = (i: number) => Array.from(rows[i].querySelectorAll("td"), td => td.textContent);
  expect(cells(0).slice(1)).toEqual(["Long", "50", "$0.3036", "7cVf…kxXy"]);
  expect(cells(1)[1]).toBe("Short");
  expect(cells(2).slice(2, 4)).toEqual(["25", ""]);
  expect(rows[0].querySelector("a")?.getAttribute("href")).toBe("https://explorer.solana.com/tx/mock-trade-0?cluster=devnet");
  expect(rows[0].querySelectorAll("a")[1].getAttribute("href")).toMatch(/^https:\/\/explorer\.solana\.com\/address\/.+\?cluster=devnet$/);
  expect(container.querySelector(".arena-fills")!.textContent).not.toContain("—");
  unmount();
});

test("a 404 from /trades falls back to recent fills; empty shows the empty line", async () => {
  let r = await render(new MockIndexerClient({ legacy: true }));
  expect(r.container.querySelectorAll(".arena-fills tbody tr")).toHaveLength(3);
  expect(r.container.querySelector(".arena-fills a")?.getAttribute("href")).toContain("mock-signature-0");
  r.unmount();
  const base = new MockIndexerClient({ legacy: true });
  r = await render({ ...withBoard([], base), trades: async () => { throw new IndexerHttpError(404, "Not found"); }, recentFills: async () => ({ fills: [] }) });
  expect(r.container.querySelector(".arena-fills")).toBeNull();
  expect(r.container.textContent).toContain("No fills yet.");
  r.unmount();
});
