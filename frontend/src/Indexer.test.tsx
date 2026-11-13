import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test, vi } from "vitest";
import App from "./App";
import { IndexerPanels } from "./Indexer";
import { MockMarketClient } from "./market/mock";
import { MockIndexerClient } from "./indexer/mock";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

test("connects and renders open interest, volume and the wallet's own fills, with no operator lines", async () => {
  const client = new MockMarketClient({ autoTick: false });
  const indexer = new MockIndexerClient();
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => { root.render(<App client={client} indexer={indexer} />); });
    expect(container.textContent).toContain("Connect a wallet to see your fills.");
    const connect = Array.from(container.querySelectorAll("button")).find(b => b.textContent === "Connect wallet")!;
    await act(async () => { connect.click(); });
    expect(Array.from(container.querySelectorAll('h2[id^="indexer-"]')).map(h => h.textContent)).toEqual(["COMPUTE-PERP stats", "Your fills"]);
    const stats = container.querySelector('[aria-labelledby="indexer-stats-heading"]')!;
    expect(Array.from(stats.querySelectorAll("dt")).map(d => d.textContent)).toEqual(["Open interest", "Volume", "Fills", "As of"]);
    expect(stats.textContent).toContain("long 250 / short 250 × 1M tokens");
    expect(stats.textContent).toContain("$18,640.00");
    const history = container.querySelector('[aria-labelledby="indexer-history-heading"]')!;
    const rows = history.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(2);
    expect(Array.from(rows[0].querySelectorAll("td")).slice(1, 5).map(td => td.textContent)).toEqual(["short", "150", "$0.3036", "executed"]);
    expect(rows[1].textContent).toContain("requested");
    expect(rows[0].querySelector("a")!.href).toBe("https://explorer.solana.com/tx/mock-signature-0?cluster=devnet");
    const text = container.textContent!.toLowerCase();
    for (const word of ["crank", "uptime", "liquidation", "heuristic", "stored basis", "mock data", "portfolio:"]) expect(text).not.toContain(word);
  } finally {
    await act(async () => { root.unmount(); });
    client.dispose();
  }
});

test("polls, keeps last good data on errors, and shows the unreachable state", async () => {
  vi.useFakeTimers();
  const indexer = new MockIndexerClient();
  const stats = await indexer.stats();
  const statsCall = vi.spyOn(indexer, "stats").mockResolvedValue({ ...stats, as_of: null });
  const positionsCall = vi.spyOn(indexer, "positions");
  const fillsCall = vi.spyOn(indexer, "fills");
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => { root.render(<IndexerPanels indexer={indexer} owner="tester" />); });
    expect(container.textContent).toContain("Indexing");
    statsCall.mockRejectedValue(new Error("stats unavailable"));
    positionsCall.mockRejectedValue(new Error("positions unavailable"));
    await act(async () => { vi.advanceTimersByTime(15000); });
    expect(statsCall).toHaveBeenCalledTimes(2);
    expect(container.textContent).toContain("long 250 / short 250");
    expect(container.textContent).toContain("executed");
    expect(Array.from(container.querySelectorAll('[role="status"]')).map(p => p.textContent)).toEqual(["stats unavailable", "positions unavailable"]);
    positionsCall.mockRestore();
    fillsCall.mockRejectedValue(new Error("fills unavailable"));
    await act(async () => { vi.advanceTimersByTime(15000); });
    expect(container.textContent).toContain("fills unavailable");
    expect(container.textContent).toContain("executed");
    await act(async () => { root.render(<IndexerPanels indexer={null} owner={null} />); });
    expect(container.textContent).not.toContain("executed");
    expect(Array.from(container.querySelectorAll('[role="status"]')).map(p => p.textContent)).toEqual([
      "The indexer is unreachable right now. Try again in a minute.", "The indexer is unreachable right now. Try again in a minute.",
    ]);
    expect(container.textContent).not.toContain("not configured");
    expect(vi.getTimerCount()).toBe(0);
  } finally {
    await act(async () => { root.unmount(); });
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
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => { root.render(<IndexerPanels indexer={indexer} owner="tester" />); });
    const rows = container.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(2);
    expect(Array.from(rows).map(row => row.querySelectorAll("td")[1].textContent)).toEqual(["long", "short"]);
    expect(container.textContent).toContain("Older fills are on the explorer.");
  } finally {
    await act(async () => { root.unmount(); });
    vi.restoreAllMocks();
  }
});

test("waits for fills before showing empty history and tolerates malformed values", async () => {
  const indexer = new MockIndexerClient();
  const fill = (await indexer.fills("buyer")).fills[0];
  let resolveFills!: (value: Awaited<ReturnType<typeof indexer.fills>>) => void;
  vi.spyOn(indexer, "fills").mockReturnValue(new Promise(resolve => { resolveFills = resolve; }));
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => { root.render(<IndexerPanels indexer={indexer} owner="tester" />); });
    expect(container.textContent).not.toContain("No fills yet.");
    await act(async () => { resolveFills({ fills: [null, Number.MAX_SAFE_INTEGER + 1, 1.5].map((mark_price, ix_index) => ({ ...fill, ix_index, mark_price, size_q: "invalid" })), next_before: null }); });
    const rows = container.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(3);
    for (const row of rows) expect(Array.from(row.querySelectorAll("td")).slice(2, 4).map(td => td.textContent)).toEqual(["—", "—"]);
  } finally {
    await act(async () => { root.unmount(); });
    vi.restoreAllMocks();
  }
});
