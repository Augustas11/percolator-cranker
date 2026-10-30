import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import Leaderboard from "./Leaderboard";
import { devnetConfig } from "../chain/config";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const stats = { market: devnetConfig.market.toBase58(), program: devnetConfig.programId.toBase58() };
const wallet = "6zZNoKi4wrhnd2XmtejyEs6f64JPVt3bQUtjZVj6P2gR";
afterEach(() => { vi.unstubAllGlobals(); });

async function render(board: unknown, ok = true, owner: string | null = null) {
  vi.stubGlobal("fetch", vi.fn(async (url: string) => url.endsWith("/stats")
    ? { ok: true, json: async () => stats }
    : { ok, status: 503, json: async () => ok ? board : { error: "database unavailable" } }));
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () => { root.render(<Leaderboard indexerUrl="/indexer" owner={owner} />); });
  return { container, root };
}

test("renders ranked rows and marks the connected wallet", async () => {
  const { container, root } = await render({ as_of_slot: 1, note: "devnet, test tokens. PnL note.", entries: [
    { rank: 1, wallet, pnl: "12500000", equity: "1012500000", net_deposits: "1000000000", trades: 4, open_positions: 1, portfolios: ["p"] },
  ] }, true, wallet);
  expect(container.querySelector("h2")!.textContent).toBe("Leaderboard (devnet, test tokens)");
  expect(container.textContent).toContain("PnL note.");
  const cells = Array.from(container.querySelectorAll("tbody td")).map(td => td.textContent);
  expect(cells).toEqual(["1", "6zZN…P2gR (you)", "$12.50", "4", "1"]);
  act(() => root.unmount());
});

test("shows the empty state, errors and a missing indexer", async () => {
  let { container, root } = await render({ as_of_slot: 1, note: "n", entries: [] });
  expect(container.textContent).toContain("No traders yet");
  act(() => root.unmount());
  ({ container, root } = await render(null, false));
  expect(container.textContent).toContain("database unavailable");
  expect(container.textContent).not.toContain("No traders yet");
  act(() => root.unmount());
  const empty = document.createElement("div");
  const r = createRoot(empty);
  await act(async () => { r.render(<Leaderboard indexerUrl={null} />); });
  expect(empty.textContent).toContain("Indexer not configured");
  act(() => r.unmount());
});
