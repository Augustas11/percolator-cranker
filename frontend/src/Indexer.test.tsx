import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test, vi } from "vitest";
import App from "./App";
import { IndexerPanels } from "./Indexer";
import { MockMarketClient } from "./market/mock";
import { MockIndexerClient } from "./indexer/mock";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

test("connects and renders indexed mock stats, positions and trade history", async () => {
  const client = new MockMarketClient({ autoTick: false });
  const indexer = new MockIndexerClient();
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => { root.render(<App client={client} indexer={indexer} />); });
    expect(container.textContent).toContain("Connect wallet to see positions");
    const connect = Array.from(container.querySelectorAll("button")).find(b => b.textContent === "Connect wallet")!;
    await act(async () => { connect.click(); });
    const stats = container.querySelector('[aria-labelledby="indexer-stats-heading"]')!;
    expect(Array.from(container.querySelectorAll('h2[id^="indexer-"]')).map(h => h.textContent)).toEqual([
      "Market stats (mock data, not devnet)", "Your positions (mock data, not devnet)", "Your trade history (mock data, not devnet)",
    ]);
    expect(stats.textContent).toContain("Cranks landed (all signers)");
    expect(stats.textContent).toContain("Crank uptime");
    expect(stats.textContent).toContain("Possible liquidations (heuristic)");
    expect(stats.textContent).toContain("98%");
    expect(stats.textContent).toContain("99.5%");
    expect(stats.textContent).toContain("since slot 800");
    expect(stats.textContent).toContain("long 2.5 SOL / short 2.5 SOL");
    const positions = container.querySelector('[aria-labelledby="indexer-positions-heading"]')!;
    expect(positions.textContent).toContain("long 1.5 SOL");
    expect(positions.textContent).toContain("$1,000.00 USD");
    expect(positions.textContent).toContain("stored basis, not effective. Mock data.");
    const history = container.querySelector('[aria-labelledby="indexer-history-heading"]')!;
    expect(history.textContent).toContain("Latest 50 trades per current portfolio.");
    expect(history.textContent).toContain("Mock data.");
    const rows = history.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("executed");
    expect(rows[0].querySelectorAll("td")[2].textContent).toBe("1.5");
    expect(rows[0].textContent).toContain("$150.00");
    expect(rows[1].textContent).toContain("requested");
    expect(rows[0].querySelector("a")!.href).toBe("https://explorer.solana.com/tx/mock-signature-0?cluster=devnet");
    expect(container.textContent!.toLowerCase()).not.toContain("volume");
  } finally {
    await act(async () => { root.unmount(); });
    client.dispose();
  }
});

test("polls, keeps last good data on errors, and handles unavailable and disabled stats", async () => {
  vi.useFakeTimers();
  const indexer = new MockIndexerClient();
  const stats = await indexer.stats();
  const statsCall = vi.spyOn(indexer, "stats").mockResolvedValue({
    ...stats, as_of: null, cranks: { ...stats.cranks, success_rate: null }, uptime: { uptime_pct: null },
  });
  const positionsCall = vi.spyOn(indexer, "positions");
  const fillsCall = vi.spyOn(indexer, "fills");
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => { root.render(<IndexerPanels indexer={indexer} owner="tester" />); });
    expect(container.textContent).toContain("Indexer is still backfilling");
    expect(container.textContent).toContain("not indexed yet");
    expect(Array.from(container.querySelectorAll("dd")).map(dd => dd.textContent).slice(1, 3)).toEqual(["—", "—"]);
    statsCall.mockRejectedValue(new Error("stats unavailable"));
    positionsCall.mockRejectedValue(new Error("positions unavailable"));
    await act(async () => { vi.advanceTimersByTime(15000); });
    expect(statsCall).toHaveBeenCalledTimes(2);
    expect(container.textContent).toContain("long 1.5 SOL");
    expect(container.textContent).toContain("executed");
    expect(Array.from(container.querySelectorAll('[role="status"]')).map(p => p.textContent)).toEqual([
      "stats unavailable", "positions unavailable", "positions unavailable",
    ]);
    positionsCall.mockRestore();
    fillsCall.mockRejectedValue(new Error("fills unavailable"));
    await act(async () => { vi.advanceTimersByTime(15000); });
    expect(container.textContent).toContain("fills unavailable");
    expect(container.textContent).toContain("executed");
    await act(async () => { root.render(<IndexerPanels indexer={null} owner={null} />); });
    expect(container.querySelectorAll("section p")).toHaveLength(3);
    expect(container.textContent).not.toContain("executed");
    expect(container.textContent).toContain("Indexer not configured");
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
    expect(Array.from(rows).map(row => row.querySelectorAll("td")[1].textContent)).toEqual(["buy", "sell"]);
    expect(container.textContent).toContain("More history exists; not shown.");
  } finally {
    await act(async () => { root.unmount(); });
    vi.restoreAllMocks();
  }
});

test("waits for fills before showing empty history and tolerates malformed values", async () => {
  const indexer = new MockIndexerClient();
  const positions = await indexer.positions("tester");
  const fill = (await indexer.fills("buyer")).fills[0];
  vi.spyOn(indexer, "positions").mockResolvedValue({ ...positions, portfolios: [{ ...positions.portfolios[0], capital: "invalid", positions: [{ ...positions.portfolios[0].positions[0], basis_pos_q: "invalid" }] }] });
  let resolveFills!: (value: Awaited<ReturnType<typeof indexer.fills>>) => void;
  vi.spyOn(indexer, "fills").mockReturnValue(new Promise(resolve => { resolveFills = resolve; }));
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => { root.render(<IndexerPanels indexer={indexer} owner="tester" />); });
    expect(container.textContent).not.toContain("No indexed trades");
    expect(container.textContent).toContain("Capital: — USD");
    expect(container.textContent).toContain("long — SOL");
    await act(async () => { resolveFills({ fills: [null, Number.MAX_SAFE_INTEGER + 1, 1.5].map((mark_price, ix_index) => ({ ...fill, ix_index, mark_price, size_q: "invalid" })), next_before: null }); });
    const rows = container.querySelectorAll("tbody tr");
    expect(rows).toHaveLength(3);
    for (const row of rows) expect(Array.from(row.querySelectorAll("td")).slice(2, 4).map(td => td.textContent)).toEqual(["—", "—"]);
  } finally {
    await act(async () => { root.unmount(); });
    vi.restoreAllMocks();
  }
});
