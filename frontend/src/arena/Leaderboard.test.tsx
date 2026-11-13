import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test } from "vitest";
import Leaderboard from "./Leaderboard";
import type { IndexerClient, Leaderboard as Board, Positions } from "../indexer/client";
import { MockIndexerClient } from "../indexer/mock";
import { site } from "../site/config";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const wallet = "6zZNoKi4wrhnd2XmtejyEs6f64JPVt3bQUtjZVj6P2gR";
const entry = (w: string, pnl: string, extra: Partial<Board["entries"][number]> = {}) =>
  ({ rank: 9, wallet: w, pnl, equity: "0", net_deposits: "500000000", trades: 3, open_positions: 0, portfolios: ["p"], ...extra });

function client(board: Board | Error, positions: Record<string, Positions["portfolios"][number]["positions"]> = {}): IndexerClient {
  const mock = new MockIndexerClient();
  return {
    stats: () => mock.stats(), fills: p => mock.fills(p),
    leaderboard: async () => { if (board instanceof Error) throw board; return board; },
    positions: async owner => ({ owner, as_of_slot: 1, portfolios: [{ portfolio: "p", last_changed_slot: 1, capital: "0", pnl: "0", positions: positions[owner] ?? [] }] }),
  };
}

async function render(indexer: IndexerClient | null, owner: string | null = null) {
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () => { root.render(<Leaderboard indexer={indexer} owner={owner} />); });
  return { container, unmount: () => act(() => root.unmount()) };
}

test("sorts by PnL, shows PnL against deposits, the open position and the connected wallet", async () => {
  const { container, unmount } = await render(client({ as_of_slot: 1, note: "n", entries: [
    entry("Low1111111111111111111111111111111111111111", "-25000000"),
    entry(wallet, "50000000", { open_positions: 1 }),
  ] }, { [wallet]: [{ asset_index: 0, market_id: 1, side: "short", basis_pos_q: "-120000000" }] }), wallet);
  const rows = Array.from(container.querySelectorAll("tbody tr"), tr => Array.from(tr.querySelectorAll("td"), td => td.textContent));
  expect(rows[0]).toEqual(["1", "6zZN…P2gR (you)", "$50.00", "+10.0%", "3", "short 120 Mtok"]);
  expect(rows[1]).toEqual(["2", "Low1…1111", "-$25.00", "−5.0%", "3", "—"]);
  expect(container.querySelector("tbody a")?.getAttribute("href")).toBe(`https://explorer.solana.com/address/${wallet}?cluster=devnet`);
  unmount();
});

test("tags house agents from the config list", async () => {
  const house = site.arena.house[0];
  const { container, unmount } = await render(client({ as_of_slot: 1, note: "n", entries: [entry(house.wallet, "0")] }));
  expect(container.querySelector(".house-tag")?.textContent).toBe(`house · ${house.name}`);
  unmount();
});

test("empty board, unreachable indexer and no indexer", async () => {
  let r = await render(client({ as_of_slot: 1, note: "n", entries: [] }));
  expect(r.container.textContent).toContain("No fills yet. The first trade puts a wallet on the board.");
  expect(Array.from(r.container.querySelectorAll("a"), a => a.getAttribute("href"))).toEqual(["/trade", "/agents"]);
  r.unmount();
  r = await render(client(new Error("database unavailable")));
  expect(r.container.textContent).toContain("The leaderboard is unreachable right now (database unavailable)");
  r.unmount();
  r = await render(null);
  expect(r.container.textContent).toContain("The leaderboard is unreachable right now");
  expect(r.container.textContent).not.toMatch(/not configured/i);
  r.unmount();
});
