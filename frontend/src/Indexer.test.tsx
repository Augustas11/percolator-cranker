import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test, vi } from "vitest";
import { OTHER_MARKET, OwnFills, UNREACHABLE, openInterestText, useTradeIndexer } from "./Indexer";
import type { TradeIndexer } from "./Indexer";
import type { IndexerClient } from "./indexer/client";
import { MockIndexerClient } from "./indexer/mock";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let last: TradeIndexer;
function Harness({ indexer, owner }: { indexer: IndexerClient | null; owner: string | null }) {
  const data = useTradeIndexer(indexer, owner);
  last = data;
  return <OwnFills data={data} connected={owner !== null} reachable={indexer !== null && !data.statsError} />;
}

async function mount(indexer: IndexerClient | null, owner: string | null) {
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () => { root.render(<Harness indexer={indexer} owner={owner} />); });
  return { container, render: (i: IndexerClient | null, o: string | null) => act(async () => { root.render(<Harness indexer={i} owner={o} />); }), unmount: () => act(async () => { root.unmount(); }) };
}

test("reads stats, volume, recent trades and the wallet's own fills, with no operator lines", async () => {
  const ui = await mount(new MockIndexerClient(), "tester");
  try {
    expect(last.sameMarket).toBe(true);
    expect(last.season!.volume_usd).toBe("18640000000");
    expect(last.recent).toHaveLength(3);
    const rows = ui.container.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(2);
    expect(Array.from(rows[0].querySelectorAll("td")).slice(1, 5).map(td => td.textContent)).toEqual(["short", "150", "$0.3036", "executed"]);
    expect(rows[1].textContent).toContain("requested");
    expect(rows[0].querySelector("a")!.href).toBe("https://explorer.solana.com/tx/mock-signature-0?cluster=devnet");
    const text = ui.container.textContent!.toLowerCase();
    for (const word of ["crank", "uptime", "liquidation", "heuristic", "mock data", "portfolio:"]) expect(text).not.toContain(word);
  } finally { await ui.unmount(); }
});

test("asks for a wallet before showing fills", async () => {
  const ui = await mount(new MockIndexerClient(), null);
  try { expect(ui.container.textContent).toBe("Connect a wallet to see your fills."); } finally { await ui.unmount(); }
});

test("an indexer serving another market is flagged, not shown as this market", async () => {
  const indexer = new MockIndexerClient();
  const stats = await indexer.stats();
  vi.spyOn(indexer, "stats").mockResolvedValue({ ...stats, market: "HyMKxfct1S9aFtq21GG3FUYUViQkmpZdk5pwGRtAz48C" });
  const ui = await mount(indexer, "tester");
  try {
    expect(last.sameMarket).toBe(false);
    expect(ui.container.textContent).toBe(OTHER_MARKET);
    expect(ui.container.querySelector("tbody")).toBeNull();
  } finally { await ui.unmount(); vi.restoreAllMocks(); }
});

test("polls, keeps last good data on errors, and shows the unreachable state", async () => {
  vi.useFakeTimers();
  const indexer = new MockIndexerClient();
  const statsCall = vi.spyOn(indexer, "stats");
  const positionsCall = vi.spyOn(indexer, "positions");
  const fillsCall = vi.spyOn(indexer, "fills");
  const ui = await mount(indexer, "tester");
  try {
    expect(ui.container.textContent).toContain("executed");
    positionsCall.mockRejectedValue(new Error("positions unavailable"));
    await act(async () => { vi.advanceTimersByTime(15000); });
    expect(statsCall).toHaveBeenCalledTimes(2);
    expect(ui.container.textContent).toContain("executed");
    expect(Array.from(ui.container.querySelectorAll('[role="status"]')).map(p => p.textContent)).toEqual(["positions unavailable"]);
    positionsCall.mockRestore();
    fillsCall.mockRejectedValue(new Error("fills unavailable"));
    await act(async () => { vi.advanceTimersByTime(15000); });
    expect(ui.container.textContent).toContain("fills unavailable");
    expect(ui.container.textContent).toContain("executed");
    statsCall.mockRejectedValue(new Error("stats unavailable"));
    await act(async () => { vi.advanceTimersByTime(15000); });
    expect(last.stats).not.toBeNull();
    expect(last.statsError).toBe("stats unavailable");
    expect(ui.container.textContent).toBe(UNREACHABLE);
    await ui.render(null, null);
    expect(last.stats).toBeNull();
    expect(ui.container.textContent).toBe(UNREACHABLE);
    expect(vi.getTimerCount()).toBe(0);
  } finally {
    await ui.unmount();
    vi.restoreAllMocks();
    vi.useRealTimers();
  }
});

test("keeps both sides of a trade between owned portfolios", async () => {
  const indexer = new MockIndexerClient();
  const positions = await indexer.positions("tester");
  const fill = (await indexer.fills("buyer")).fills[0];
  vi.spyOn(indexer, "positions").mockResolvedValue({ ...positions, portfolios: ["buyer", "seller"].map(portfolio => ({ ...positions.portfolios[0], portfolio })) });
  vi.spyOn(indexer, "fills").mockImplementation(async portfolio => ({
    fills: [{ ...fill, taker_portfolio: "buyer", maker_portfolio: "seller", side: portfolio === "buyer" ? "buy" : "sell" }], next_before: "998",
  }));
  const ui = await mount(indexer, "tester");
  try {
    const rows = ui.container.querySelectorAll("tbody tr");
    expect(Array.from(rows).map(row => row.querySelectorAll("td")[1].textContent).sort()).toEqual(["long", "short"]);
    expect(ui.container.textContent).toContain("Older fills are on the explorer.");
  } finally { await ui.unmount(); vi.restoreAllMocks(); }
});

test("waits for fills before showing empty history and tolerates malformed values", async () => {
  const indexer = new MockIndexerClient();
  const fill = (await indexer.fills("buyer")).fills[0];
  let resolveFills!: (value: Awaited<ReturnType<typeof indexer.fills>>) => void;
  vi.spyOn(indexer, "fills").mockReturnValue(new Promise(resolve => { resolveFills = resolve; }));
  const ui = await mount(indexer, "tester");
  try {
    expect(ui.container.textContent).toBe("Loading your fills.");
    await act(async () => { resolveFills({ fills: [null, Number.MAX_SAFE_INTEGER + 1, 1.5].map((mark_price, ix_index) => ({ ...fill, ix_index, mark_price, size_q: "invalid", executed_size: null, applied_price: null })), next_before: null }); });
    const rows = ui.container.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(3);
    for (const row of rows) expect(Array.from(row.querySelectorAll("td")).slice(2, 4).map(td => td.textContent)).toEqual(["—", "—"]);
  } finally { await ui.unmount(); vi.restoreAllMocks(); }
});

test("open interest text drops a zero USD figure, keeps units, and is absent when both are empty", () => {
  expect(openInterestText("157900000", 250)).toBe("$157.90 · 250 Mtok");
  expect(openInterestText("0", 100)).toBe("100 Mtok");
  expect(openInterestText("157900000", 0)).toBe("$157.90");
  expect(openInterestText("0", 0)).toBeNull();
  expect(openInterestText(null, null)).toBeNull();
  expect(openInterestText("broken", undefined)).toBeNull();
});
