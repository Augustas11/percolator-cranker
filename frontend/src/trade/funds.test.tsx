import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import App from "../App";
import { MockMarketClient } from "../market/mock";
import { MockIndexerClient } from "../indexer/mock";
import { MockComputeFeedClient } from "../compute/mock";
import { entryFromFills } from "../Indexer";
import type { OwnFill } from "../Indexer";
import { faucetMessage } from "../faucet";
import type { IndexerClient } from "../indexer/client";
import type { ComputeFeedClient } from "../compute/client";
import type { MarketClient } from "../market/types";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

/** The faucet as the page sees it through /faucet: health always answers; claim answers with `claim`. */
function stubFaucet(claim: () => Response) {
  const fetch = vi.fn(async (url: string) => url.endsWith("/health")
    ? json(200, { ok: true, claims_today: 12, daily_cap: 200, amount_usdc: 1000, sol_drips_today: 0, sol_daily_cap: 12, sol_paused: false })
    : claim());
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

async function mount(client: MarketClient, props: { indexer?: IndexerClient | null; computeFeed?: ComputeFeedClient | null } = {}) {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => { root.render(<App client={client} {...props} />); });
  const button = (text: string) => Array.from(container.querySelectorAll("button")).find(b => b.textContent === text);
  return {
    container, button,
    async click(text: string) { const b = button(text); expect(b, text).toBeDefined(); await act(async () => { b!.click(); }); },
    async input(id: string, value: string) {
      const el = container.querySelector<HTMLInputElement>(`#${id}`)!;
      await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(el, value); el.dispatchEvent(new Event("input", { bubbles: true })); });
    },
    async unmount() { await act(async () => { root.unmount(); }); container.remove(); },
  };
}

test("a wallet with SOL but no test-USDC account creates it first, then claims; limits come from the faucet", async () => {
  const fetch = stubFaucet(() => json(200, { ok: true, wallet: "x", amount_usdc: 1000, signature: "sig", sol: 0, next_claim_after: 1 }));
  const client = new MockMarketClient({ autoTick: false, sol: 1, usdc: null });
  const ui = await mount(client);
  try {
    expect(ui.button("Get test USDC")).toBeUndefined();
    await ui.click("Connect wallet");
    expect(ui.button("Get test USDC")).toBeUndefined();
    expect(ui.container.querySelector(".tx-faucet")!.textContent).toContain("Step 1 of 2: one transaction from your wallet, a small SOL fee. Then claim.");
    expect(ui.container.querySelector("#faucet-help")!.textContent).toBe("1,000 test USDC per claim, one claim per wallet every 24 h, 200 claims a day across all wallets.");
    expect(ui.container.querySelector(".tx-account")!.textContent).toContain("no test USDC account");
    // Deposit before any test USDC: a plain message, nothing signed.
    await ui.input("amount", "10");
    await ui.click("Deposit");
    expect(ui.container.querySelector(".error")!.textContent).toBe("You have no test USDC yet: get some from the faucet.");
    await ui.click("Create your test USDC account");
    expect(ui.container.querySelector(".tx-notice")!.textContent).toBe("Your test USDC account is ready. Now claim from the faucet.");
    expect(ui.button("Create your test USDC account")).toBeUndefined();
    await ui.click("Get test USDC");
    const claim = (fetch.mock.calls as unknown as [string, RequestInit][]).find(([url]) => url === "/faucet/claim")!;
    expect(JSON.parse(claim[1].body as string)).toEqual({ wallet: client.getSnapshot().portfolio.owner });
    expect(ui.container.querySelector(".tx-notice")!.textContent).toBe("Sent 1,000 test USDC to your wallet.");
  } finally { await ui.unmount(); client.dispose(); }
});

test("a wallet with almost no SOL claims directly: the faucet creates its account", async () => {
  stubFaucet(() => json(200, {}));
  const client = new MockMarketClient({ autoTick: false, sol: 0.01, usdc: null });
  const ui = await mount(client);
  try {
    await ui.click("Connect wallet");
    expect(ui.button("Create your test USDC account")).toBeUndefined();
    expect(ui.button("Get test USDC")).toBeDefined();
  } finally { await ui.unmount(); client.dispose(); }
});

test("a refused claim reads as plain words", async () => {
  const now = Math.floor(Date.now() / 1000);
  stubFaucet(() => json(429, { error: "Wallet already claimed in the last 24 hours", next_claim_after: now + 5 * 3600 + 120 }));
  const client = new MockMarketClient({ autoTick: false });
  const ui = await mount(client);
  try {
    await ui.click("Connect wallet");
    await ui.click("Get test USDC");
    expect(ui.container.querySelector(".error")!.textContent).toMatch(/^This wallet already got test USDC in the last 24 hours\. Next claim in 5 h [12] min\.$/);
  } finally { await ui.unmount(); client.dispose(); }
  expect(faucetMessage(429, "Global daily cap reached", undefined, now)).toBe("The faucet has paid out all of today's claims. Try again tomorrow.");
  expect(faucetMessage(400, "Create the test-USDC token account first (the faucet creates it only for wallets that receive SOL)", undefined, now)).toBe("Create your test USDC account first, then claim.");
  expect(faucetMessage(503, "RPC unavailable or transaction not confirmed", undefined, now)).toBe("The faucet could not send right now. Try again in a minute.");
});

const fill = (slot: number, size: string | null, price: number | null, portfolio = "p1", status: "executed" | "requested" = "executed"): OwnFill => ({
  signature: `s${slot}`, ix_index: 0, leg: 0, slot, block_time: null, taker_portfolio: portfolio, maker_portfolio: "m", asset_index: 0,
  size_q: size ?? "0", reported_price: null, limit_price: null, mark_price: null, requested_fee_bps: 12, status, side: size?.startsWith("-") ? "sell" : "buy",
  price_note: "", executed_size: size, applied_price: price, portfolio,
});

test("entry price is the size-weighted average of the fills that built the position, or nothing", () => {
  // 10 at 0.30, 30 at 0.34 → 40 at 0.33; selling 20 keeps the average.
  const built = [fill(3, "-20000000", 350000), fill(2, "30000000", 340000), fill(1, "10000000", 300000)];
  expect(entryFromFills(built, 20, true)).toBeCloseTo(0.33, 10);
  // A flip starts a new average at the flipping fill's price.
  expect(entryFromFills([...built, fill(4, "-50000000", 320000)], -30, true)).toBeCloseTo(0.32, 10);
  // Requested (not executed) fills are ignored.
  expect(entryFromFills([...built, fill(5, "90000000", 1, "p1", "requested")], 20, true)).toBeCloseTo(0.33, 10);
  // Not exact: history not complete, a fill without exact fields, or fills that do not add up to the position.
  expect(entryFromFills(built, 20, false)).toBeNull();
  expect(entryFromFills([...built, fill(0, null, null)], 20, true)).toBeNull();
  expect(entryFromFills(built, 25, true)).toBeNull();
  expect(entryFromFills(null, 20, true)).toBeNull();
});

test("an open position with unknown entry shows the entry derived from the wallet's indexed fills", async () => {
  const client = new MockMarketClient({ autoTick: false });
  await client.connect();
  await client.deposit(1000);
  await client.openPosition("long", 40, 2);
  const snap = client.getSnapshot();
  const unknown = { ...snap, portfolio: { ...snap.portfolio, legs: snap.portfolio.legs.map(l => ({ ...l, entryPrice: NaN, unrealizedPnl: NaN })) } };
  client.getSnapshot = () => unknown;
  const indexer = new MockIndexerClient();
  vi.spyOn(indexer, "fills").mockResolvedValue({ fills: [fill(2, "30000000", 340000, "mock-portfolio"), fill(1, "10000000", 300000, "mock-portfolio")], next_before: null });
  const ui = await mount(client, { indexer });
  try {
    const position = ui.container.querySelector('[aria-labelledby="position-heading"]')!;
    const heads = Array.from(position.querySelectorAll("th"), th => th.textContent);
    expect(heads).toEqual(["Market", "Side", "Size", "Entry price", "Mark", "Unrealized PnL", "Margin", "Action"]);
    const cells = Array.from(position.querySelectorAll("tbody td"), td => td.textContent);
    expect(cells[3]).toBe("$0.3300");
    expect(cells[5]).toBe(`$${((0.3036 - 0.33) * 40).toFixed(2)}`.replace("$-", "-$"));
  } finally { await ui.unmount(); client.dispose(); }
});

test("fills paused: the order button is disabled with the reason", async () => {
  const client = new MockMarketClient({ autoTick: false });
  await client.connect();
  const snap = client.getSnapshot();
  const stale = { ...snap, liquidity: { ...snap.liquidity!, refPrice: 0.3036, refAgeSlots: 900, maxAgeSlots: 300 } };
  client.getSnapshot = () => stale;
  const ui = await mount(client);
  try {
    const reason = "Fills are paused: the liquidity account's reference price is older than its 300-slot limit.";
    expect(ui.container.querySelector(".tx-order .tx-paused")!.textContent).toBe(reason);
    expect(ui.container.querySelector(".tx-quotes .tx-paused")!.textContent).toBe(reason);
    expect(ui.button("Open long")!.disabled).toBe(true);
    expect(ui.container.querySelector(".tx-quotes")!.textContent).toContain("or if that reference price is older than 300 slots (about 2 minutes).");
  } finally { await ui.unmount(); client.dispose(); }
});

test("a chart with no pushed marks shows an empty state, not an endless skeleton", async () => {
  const feed = new MockComputeFeedClient();
  const history = await feed.history();
  vi.spyOn(feed, "history").mockResolvedValue({ ...history, points: [] });
  const client = new MockMarketClient({ autoTick: false });
  const ui = await mount(client, { computeFeed: feed });
  try {
    expect(ui.container.querySelector(".tx-chart-skel")).toBeNull();
    expect(ui.container.querySelector(".tx-chart-empty")!.textContent).toBe("No mark has been pushed yet.");
  } finally { await ui.unmount(); client.dispose(); }
});

test("an indexer without /trades falls back to recent fills on the Trades tab", async () => {
  const client = new MockMarketClient({ autoTick: false });
  const ui = await mount(client, { indexer: new MockIndexerClient({ legacy: true }) });
  try {
    await ui.click("Trades");
    const rows = Array.from(ui.container.querySelectorAll("#book-panel-trades tbody tr"), tr => Array.from(tr.querySelectorAll("td"), td => td.textContent).slice(1, 4));
    expect(rows).toEqual([["Long", "50", "$0.3036"], ["Short", "150", "$0.3036"], ["Long", "50", "$0.3036"]]);
  } finally { await ui.unmount(); client.dispose(); }
});
