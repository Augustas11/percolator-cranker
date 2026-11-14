import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import Site from "./Site";
import { site } from "./config";
import { Link } from "./router";
import { MockMarketClient } from "../market/mock";
import { MockIndexerClient } from "../indexer/mock";
import type { IndexerClient } from "../indexer/client";
import { MockComputeFeedClient } from "../compute/mock";
import { marketFixture } from "../compute/fixture";
import { typicalPrice } from "../compute/charts";
import { pct, usd } from "../compute/format";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let cleanup: (() => Promise<void>) | undefined;
afterEach(async () => {
  await cleanup?.();
  cleanup = undefined;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

async function render(path: string, indexer: IndexerClient | null = new MockIndexerClient(), client = new MockMarketClient({ autoTick: false }), computeFeed: MockComputeFeedClient | null = null) {
  window.history.pushState({}, "", path);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  cleanup = async () => {
    await act(async () => { root.unmount(); });
    client.dispose();
    container.remove();
  };
  await act(async () => { root.render(<Site client={client} indexer={indexer} indexerBase={null} computeFeed={computeFeed} />); });
  return container;
}

test.each([
  [site.paths.home, "Price andhedge AIcompute.", "From agent to a COMPUTE-PERP position.", "Drip · the price of AI compute"],
  [site.paths.agents, "Your agent hedges its compute bill.", "Install the MCP", "Agents · Drip"],
  [site.paths.arena, "Arena", "Leaderboard", "Arena · Drip"],
  [site.paths.compute, "Compute index", "The compute index is unreachable right now.", "Compute index · Drip"],
  [site.paths.trade, "COMPUTE-PERP", "Connect wallet", "Trade COMPUTE-PERP · Drip"],
  [site.paths.docs, "Docs", "How is the index calculated?", "Docs · Drip"],
])("renders %s", async (path, heading, text, title) => {
  const container = await render(path);
  expect(container.querySelector("h1")?.textContent).toBe(heading);
  expect(container.textContent).toContain(text);
  expect(document.title).toBe(title);
  const meta = (key: string) => document.head.querySelector<HTMLMetaElement>(`meta[name="${key}"], meta[property="${key}"]`)?.content;
  expect(meta("og:title")).toBe(title);
  expect(meta("twitter:title")).toBe(title);
  expect(meta("description")?.length).toBeGreaterThan(40);
  expect(meta("og:description")).toBe(meta("description"));
  expect(container.querySelector("nav")).not.toBeNull();
  expect(container.querySelector("footer")?.textContent).toContain("Drip is the market for AI compute. Live prices from every seller, and a perp to trade them. Built for perp traders and for agents on FLOP Network.");
});

test("home shows its live mark, recent trades and honest unpublished tools", async () => {
  const final = site.mcp.final;
  site.mcp.final = false;
  try {
  const container = await render(site.paths.home);
  expect(container.textContent).toContain("$0.3036");
  expect(container.textContent).toContain("From agent to a COMPUTE-PERP position.");
  expect(container.textContent).not.toMatch(/PERCOLATOR PERPS · SOLANA|LIVE|Illustrative/);
  expect(container.textContent).toContain("COMPUTE-PERP settles on what full-precision sellers charge for Qwen3.8-27B");
  expect(container.textContent).toContain("Mark price on chain");
  expect(container.querySelector('.hero-actions a[href="/trade"]')?.textContent?.trim()).toBe("Trade COMPUTE-PERP");
  expect(container.querySelector('.hero-actions a[href="/agents#install"]')?.textContent).toBe("Connect an agent");
  expect(container.querySelector(".home-hero")?.textContent).not.toMatch(/test tokens/i);
  expect(container.querySelector(".home-hero .trade-note")).toBeNull();
  expect(container.textContent).not.toMatch(/Percolator/);
  expect(container.querySelector('.sol-actions a[href="/arena"]')?.textContent).toBe("See the leaderboard");
  expect(container.querySelector('.sol-actions a[href="/trade"]')?.textContent).toBe("Trade COMPUTE-PERP");
  expect(container.querySelectorAll(".board-card tbody tr")).toHaveLength(5);
  expect(container.textContent).not.toMatch(/TBD|not configured/);
  expect(container.textContent).not.toMatch(/\[TBD\]|\[NPM-PACKAGE\]|npx/);
  expect(container.querySelector(".terminal button")).toBeNull();
  } finally { site.mcp.final = final; }
});

test("footer hides unset community links, and the column when none is set", async () => {
  const container = await render(site.paths.home);
  const footer = container.querySelector("footer")!;
  expect(Array.from(footer.querySelectorAll(".footer-label"), l => l.textContent)).toEqual(["PRODUCT", "BUILD"]);
  expect(footer.textContent).not.toMatch(/Telegram|Status/);
  await cleanup?.(); cleanup = undefined;
  site.x = "https://x.com/example";
  try {
    const c2 = await render(site.paths.home);
    const f2 = c2.querySelector("footer")!;
    expect(Array.from(f2.querySelectorAll(".footer-label"), l => l.textContent)).toEqual(["PRODUCT", "BUILD", "COMMUNITY"]);
    expect(Array.from(f2.querySelectorAll("div:last-child > ul a"), a => a.textContent)).toEqual(["X"]);
  } finally { site.x = null; }
});

test("published home terminal copies the package command", async () => {
  const final = site.mcp.final;
  site.mcp.final = true;
  try {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", Object.create(navigator, { clipboard: { value: { writeText } } }));
    const container = await render(site.paths.home);
    expect(container.querySelector(".terminal")?.textContent).toContain(site.mcp.install);
    const button = container.querySelector<HTMLButtonElement>(".terminal button")!;
    await act(async () => { button.click(); });
    expect(writeText).toHaveBeenCalledWith(site.mcp.install);
    expect(button.textContent).toBe("Copied");
  } finally { site.mcp.final = final; }
});

test("nav links update history and render without reloading", async () => {
  const scroll = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  const push = vi.spyOn(window.history, "pushState");
  const container = await render(site.paths.home);
  const agents = Array.from(container.querySelectorAll("nav a")).find(a => a.textContent === "Agents")!;
  await act(async () => { agents.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 })); });
  expect(window.location.pathname).toBe(site.paths.agents);
  expect(push).toHaveBeenLastCalledWith({}, "", site.paths.agents);
  expect(container.querySelector(".cx-title")?.textContent).toBe("Agents");
  expect(scroll).toHaveBeenCalledWith(0, 0);
  await act(async () => {
    window.history.replaceState({}, "", site.paths.docs);
    window.dispatchEvent(new PopStateEvent("popstate"));
  });
  expect(container.querySelector("h1")?.textContent).toBe("Docs");
});

test("home side links select and reapply the requested trade side", async () => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  const container = await render(site.paths.home);
  async function click(selector: string) {
    await act(async () => { container.querySelector<HTMLElement>(selector)!.click(); });
  }
  async function select(side: "long" | "short") {
    await click(`.market-actions a[href="/trade?side=${side}"]`);
    expect(window.location.search).toBe(`?side=${side}`);
    expect(Array.from(container.querySelectorAll("button")).find(button => button.textContent === (side === "long" ? "Long" : "Short"))?.getAttribute("aria-pressed")).toBe("true");
  }
  await select("short");
  await click('nav a[href="/"]');
  await select("long");
  await click('nav a[href="/"]');
  await select("short");
  const long = Array.from(container.querySelectorAll("button")).find(button => button.textContent === "Long")!;
  await act(async () => { long.click(); });
  expect(long.getAttribute("aria-pressed")).toBe("true");
  await click('nav a[href="/"]');
  await select("short");
});

test("query-only side navigation updates the retained trade page", async () => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  const container = await render("/trade?side=long&keep=1");
  const links = document.createElement("div");
  const root = createRoot(links);
  container.append(links);
  try {
    await act(async () => { root.render(<Link to="/trade?side=short&keep=1">Short</Link>); });
    await act(async () => { links.querySelector("a")!.click(); });
    expect(window.location.search).toBe("?side=short&keep=1");
    expect(Array.from(container.querySelectorAll("button")).find(button => button.textContent === "Short")?.getAttribute("aria-pressed")).toBe("true");
    await act(async () => {
      window.history.back();
      await new Promise(resolve => setTimeout(resolve, 20));
    });
    expect(window.location.search).toBe("?side=long&keep=1");
    expect(Array.from(container.querySelectorAll("button")).find(button => button.textContent === "Long")?.getAttribute("aria-pressed")).toBe("true");
  } finally { await act(async () => { root.unmount(); }); }
});

test.each(["long", "short", "invalid"])("validates an initial trade side of %s", async side => {
  const container = await render(`/trade?side=${side}`);
  const selected = side === "short" ? "Short" : "Long";
  expect(Array.from(container.querySelectorAll("button")).find(button => button.textContent === selected)?.getAttribute("aria-pressed")).toBe("true");
});

test.each(["/tools", "/build", "/tools/", "/build/"])("old link %s redirects to /agents", async path => {
  const container = await render(path);
  expect(window.location.pathname).toBe(site.paths.agents);
  expect(container.querySelector(".cx-title")?.textContent).toBe("Agents");
  expect(document.title).toBe("Agents · Drip");
});

test("trailing slash renders Agents and its nav link only scrolls to top", async () => {
  const scroll = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  const container = await render("/agents/");
  expect(container.querySelector(".cx-title")?.textContent).toBe("Agents");
  const push = vi.spyOn(window.history, "pushState");
  const tools = container.querySelector<HTMLAnchorElement>('nav a[href="/agents"]')!;
  await act(async () => { tools.click(); });
  expect(push).not.toHaveBeenCalled();
  expect(scroll).toHaveBeenCalledWith(0, 0);
});

test("keeps collateral disabled across navigation while a deposit is pending", async () => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  const client = new MockMarketClient({ autoTick: false });
  const deposit = vi.spyOn(client, "deposit").mockImplementation(() => new Promise(() => {}));
  const container = await render(site.paths.home, null, client);
  expect(container.querySelector("#amount")).toBeNull();
  async function navigate(path: string) {
    const link = container.querySelector<HTMLAnchorElement>(`nav a[href="${path}"]`)!;
    await act(async () => { link.click(); });
  }
  await navigate(site.paths.trade);
  await act(async () => { container.querySelector<HTMLButtonElement>('[title="Connect wallet"]')!.click(); });
  const amount = container.querySelector<HTMLInputElement>("#amount")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(amount, "100");
    amount.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const button = Array.from(container.querySelectorAll("button")).find(button => button.textContent === "Deposit")!;
  await act(async () => { button.click(); });
  expect(deposit).toHaveBeenCalledWith(100);
  await navigate(site.paths.agents);
  expect(container.querySelector(".site-page .cx-title")?.textContent).toBe("Agents");
  expect(container.querySelector("#amount")?.closest("[hidden]")).not.toBeNull();
  await navigate(site.paths.trade);
  expect(container.querySelector("#amount")?.closest("[hidden]")).toBeNull();
  expect(container.querySelector<HTMLFieldSetElement>('.tx-account fieldset')!.disabled).toBe(true);
  await act(async () => { button.click(); });
  expect(deposit).toHaveBeenCalledTimes(1);
});

test("copy writes the MCP install command and resets after two seconds", async () => {
  const final = site.mcp.final;
  site.mcp.final = true;
  try {
    vi.useFakeTimers();
    const writeText = vi.fn().mockResolvedValue(undefined);
    let clipboard: { writeText: typeof writeText } | undefined = { writeText };
    vi.stubGlobal("navigator", Object.create(navigator, { clipboard: { get: () => clipboard } }));
    const container = await render(site.paths.agents);
    const button = container.querySelector("button")!;
    await act(async () => { button.click(); });
    expect(writeText).toHaveBeenCalledWith(site.mcp.install);
    expect(button.textContent).toBe("Copied");
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(button.textContent).toBe("Copy");
    clipboard = undefined;
    await act(async () => { button.click(); });
    expect(button.textContent).toBe("Copy failed");
  } finally { site.mcp.final = final; }
});

test("unpublished MCP does not expose install commands", async () => {
  const final = site.mcp.final;
  site.mcp.final = false;
  try {
    const container = await render(site.paths.agents);
    expect(container.textContent).toContain("The install line appears here once the MCP package is published.");
    expect(container.querySelector("#install")?.textContent).not.toContain("npx");
    const mcp = container.querySelector("#install")!;
    expect(mcp.querySelector("pre")).toBeNull();
    expect(mcp.querySelector("button")).toBeNull();
  } finally { site.mcp.final = final; }
});

test("unknown paths say the page does not exist and link to the index and trade", async () => {
  const container = await render("/missing");
  expect(container.querySelector("h1")?.textContent).toBe("Page not found");
  expect(container.querySelector("main")?.textContent).toContain("This page does not exist.");
  expect(Array.from(container.querySelectorAll("main a"), a => a.getAttribute("href"))).toEqual(["/compute", "/trade"]);
  expect(document.title).toBe("Page not found · Drip");
});


test("home renders recent fills and live season totals", async () => {
  const container = await render(site.paths.home);
  const ticker = container.querySelector(".recent-trades")!;
  expect(ticker.querySelectorAll("li")).toHaveLength(3);
  for (const text of ["Long", "Short", "50 Mtok", "150 Mtok", "$0.3036", "7cVf…kxXy", "4fYN…Ab8m"]) expect(ticker.textContent).toContain(text);
  expect(ticker.textContent).toMatch(/\d+[smhd] ago/);
  const link = ticker.querySelector("li a")!;
  expect(link.getAttribute("href")).toBe("https://explorer.solana.com/tx/mock-signature-0?cluster=devnet");
  expect(link.getAttribute("target")).toBe("_blank");
  expect(link.getAttribute("rel")).toBe("noreferrer");
  const season = container.querySelector('[aria-label="Arena stats"]')!;
  expect(Array.from(season.querySelectorAll(".fact-label"), el => el.textContent)).toEqual(["Traders", "Fills", "Volume (USD)", "Open interest"]);
  expect(Array.from(season.querySelectorAll(".fact-value"), el => el.textContent)).toEqual(["7", "214", "$18,640", "$157.90 · 250 Mtok"]);
  expect(season.textContent).toContain("devnet · since " + new Date(1793232000 * 1000).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" }));
});

test("home shows at most five recent fills while fetching twenty", async () => {
  const indexer = new MockIndexerClient();
  const { fills } = await indexer.recentFills(20);
  const recentFills = vi.spyOn(indexer, "recentFills").mockResolvedValue({
    fills: Array.from({ length: 7 }, (_, i) => ({ ...fills[0], signature: `fill-${i}` })),
  });
  const container = await render(site.paths.home, indexer);
  expect(recentFills).toHaveBeenCalledWith(20);
  expect(Array.from(container.querySelectorAll(".recent-trades li a"), el => el.getAttribute("href"))).toEqual(
    Array.from({ length: 5 }, (_, i) => `https://explorer.solana.com/tx/fill-${i}?cluster=devnet`),
  );
});

test.each(["rejects", "missing", "none", "empty"])("home handles unavailable data: %s", async mode => {
  const indexer: IndexerClient = new MockIndexerClient();
  indexer.recentFills = mode === "missing" ? undefined : mode === "empty" ? vi.fn().mockResolvedValue({ fills: [] }) : vi.fn().mockRejectedValue(new Error("indexer 404"));
  indexer.season = mode === "missing" ? undefined : vi.fn().mockRejectedValue(new Error("indexer 404"));
  const container = await render(site.paths.home, mode === "none" ? null : indexer);
  expect(container.querySelector(".recent-trades")?.textContent).toBe("Recent fillsNo fills yet. Trade or connect an agent.");
  expect(container.querySelector('.recent-trades a[href="/trade"]')?.textContent).toBe("Trade");
  expect(container.querySelector('.recent-trades a[href="/agents"]')?.textContent).toBe("connect an agent");
  // No season read: no tiles and no placeholder dashes.
  expect(container.querySelector('[aria-label="Arena stats"]')).toBeNull();
  expect(container.querySelectorAll(".fact-value")).toHaveLength(0);
});

test.each([
  // Open interest drops a zero USD figure but keeps the open units; an unreadable figure hides its tile.
  ["0", "0", ["0", "0", "$0.00", "250 Mtok"]],
  ["bad", "broken", ["0", "0", "250 Mtok"]],
  ["9007199254740993000000", "12345678", ["0", "0", "$9,007,199,254,740,993", "$12.35 · 250 Mtok"]],
])("home formats season atoms %s and %s", async (volume, interest, expected) => {
  const indexer = new MockIndexerClient();
  vi.spyOn(indexer, "season").mockResolvedValue({ season: 0, since: null, agents: 0, trades: 0, volume_usd: volume, open_interest_usd: interest });
  const container = await render(site.paths.home, indexer);
  expect(Array.from(container.querySelectorAll(".fact-value"), el => el.textContent)).toEqual(expected);
  expect(container.querySelector(".fact-note")?.textContent).toBe("devnet");
});

test("home polls and retains last good data on errors, then stops on unmount", async () => {
  vi.useFakeTimers();
  const indexer = new MockIndexerClient();
  const fills = vi.spyOn(indexer, "recentFills"), season = vi.spyOn(indexer, "season");
  const container = await render(site.paths.home, indexer);
  expect(fills).toHaveBeenCalledWith(20);
  fills.mockRejectedValue(new Error("indexer 404"));
  season.mockRejectedValue(new Error("indexer 404"));
  await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
  expect(fills).toHaveBeenCalledTimes(4);
  expect(season).toHaveBeenCalledTimes(2);
  expect(container.querySelectorAll(".recent-trades li")).toHaveLength(3);
  expect(container.querySelector(".facts-grid")?.textContent).toContain("$18,640");
  await cleanup?.();
  cleanup = undefined;
  await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
  expect(fills).toHaveBeenCalledTimes(4);
  expect(season).toHaveBeenCalledTimes(2);
});

test("home explains why Drip with the feed's numbers", async () => {
  const now = Date.now();
  const container = await render(site.paths.home, new MockIndexerClient(), new MockMarketClient({ autoTick: false }), new MockComputeFeedClient(now));
  const why = container.querySelector('[aria-labelledby="why-title"]')!;
  expect(why.querySelector("h2")?.textContent).toBe("Why Drip");
  expect(Array.from(why.querySelectorAll("h3"), el => el.textContent)).toEqual(["Every seller, not one", "Quoted in what you pay", "Settlement on chain", "An agent can hedge its own bill"]);
  const market = marketFixture(now);
  expect(why.querySelector("p")?.textContent).toBe(`${market.index.n_quotes_up} quotes up from ${market.index.n_providers} providers, read every 5 minutes. You see the cheapest and the dearest, not a list price.`);
  expect(why.textContent).toContain("Each settlement price is written to Solana (latest transaction). Each fill is a Solana transaction.");
  expect(why.textContent).not.toContain("signed line");
  expect(why.querySelector('a[href^="https://explorer.solana.com/tx/"]')).not.toBeNull();
  expect(why.textContent).not.toMatch(/verify-push-log|staging/);
  expect(container.querySelector(".build-cards")?.textContent).not.toMatch(/keeper/i);
  expect(container.querySelector(".home-arena dl")).toBeNull();
  expect(container.textContent).not.toMatch(/Season 0|Your key, your agent|Connect once|PnL decides|From agent to leaderboard/);
});

test("home card: routed price absent falls back to the median, and a null 1 h change is hidden", async () => {
  const feed = new MockComputeFeedClient();
  const base = await feed.market();
  vi.spyOn(feed, "market").mockResolvedValue({ ...base, routed: null, models: base.models.map(({ routed: _r, routed_usd_per_bn_gn: _g, ...m }) => m) });
  let container = await render(site.paths.home, new MockIndexerClient(), new MockMarketClient({ autoTick: false }), feed);
  expect(container.querySelector(".compute-card .cc-label")?.textContent).toBe("Median price to run an open model");
  expect(container.querySelector(".compute-card .cc-meta")?.textContent).not.toContain("Median across open models");
  await cleanup?.();
  const nulled = new MockComputeFeedClient();
  vi.spyOn(nulled, "market").mockResolvedValue({ ...base, models: base.models.map(m => m.routed ? { ...m, routed: { ...m.routed, change_1h_pct: null } } : m) });
  container = await render(site.paths.home, new MockIndexerClient(), new MockMarketClient({ autoTick: false }), nulled);
  expect(container.querySelector(".compute-card .cc-label")?.textContent).toBe("What a request pays now, Qwen3.8-27B");
  expect(container.querySelector(".compute-card .cc-chg")).toBeNull();
});

test("one quote count on Home, /compute and /agents", async () => {
  const now = Date.now(), n = marketFixture(now).index.n_quotes_up;
  const home = await render(site.paths.home, new MockIndexerClient(), new MockMarketClient({ autoTick: false }), new MockComputeFeedClient(now));
  expect(home.querySelector(".compute-card .cc-meta")?.textContent).toContain(`${n} quotes up`);
  expect(home.querySelector(".hero-description")?.textContent).toContain(`${n} quotes up`);
  expect(home.querySelector('[aria-labelledby="why-title"] p')?.textContent).toContain(`${n} quotes up`);
  await cleanup?.();
  const compute = await render(site.paths.compute, new MockIndexerClient(), new MockMarketClient({ autoTick: false }), new MockComputeFeedClient(now));
  expect(compute.querySelector(".cx-proof")?.textContent).toContain(`${n} live quotes`);
  await cleanup?.();
  const agents = await render(site.paths.agents, new MockIndexerClient(), new MockMarketClient({ autoTick: false }), new MockComputeFeedClient(now));
  expect(agents.querySelector(".ag-sub")?.textContent).toContain(`${n} live seller quotes`);
});

test("home card survives a feed with no index base and no cheapest seller", async () => {
  const feed = new MockComputeFeedClient();
  const base = await feed.market();
  vi.spyOn(feed, "market").mockResolvedValue({ ...base, index: { ...base.index, level: null, base_ms: null }, models: base.models.map(m => ({ ...m, cheapest: null })) });
  const container = await render(site.paths.home, new MockIndexerClient(), new MockMarketClient({ autoTick: false }), feed);
  const meta = container.querySelector(".compute-card .cc-meta")?.textContent ?? "";
  expect(meta).toMatch(/quotes up · \d+ providers · \d+ models$/);
  expect(container.querySelector(".compute-card .cc-level")).not.toBeNull();
  expect(container.textContent).not.toMatch(/ChatGPT/);
});

test("home leads with the compute index, then the perp", async () => {
  const now = Date.now();
  const container = await render(site.paths.home, new MockIndexerClient(), new MockMarketClient({ autoTick: false }), new MockComputeFeedClient(now));
  const market = marketFixture(now);
  const card = container.querySelector(".compute-card")!;
  const routed = market.models.find(m => m.id === "qwen/qwen3.8-27b")!.routed!;
  expect(card.querySelector(".cc-level")?.textContent).toBe(`What a request pays now, Qwen3.8-27B${usd(routed.price)}per 1M tokens1 h ${pct(routed.change_1h_pct)}`);
  expect(card.querySelector(".cc-meta")?.textContent).toBe(`Median across open models ${usd(typicalPrice(market)!.median)} · ${market.index.n_quotes_up} quotes up · ${market.index.n_providers} providers · ${market.models.length} models`);
  expect(card.querySelector(".cc-index")).toBeNull();
  expect(container.querySelector(".announcement")).toBeNull();
  expect(card.querySelectorAll(".cc-models .strip")).toHaveLength(4);
  expect(card.querySelector(".cc-perp .market-price")?.textContent).toBe("$0.3036 / 1M tokens");
  expect(card.textContent).toContain("Settlement price · pushed 3 min ago · 2 of 2 sellers up");
  expect(container.querySelector(".hero-description")?.textContent).toContain(`What open-model inference costs right now: ${market.index.n_quotes_up} quotes up from ${market.index.n_providers} providers on ${market.models.length} models.`);
  expect(container.querySelector('.home-hero .hero-primary')?.getAttribute("href")).toBe("/compute");
  expect(container.querySelector(".hero-credit")?.textContent).toBe("Built for perp traders and for agents on FLOP Network.");
  const sections = Array.from(container.querySelectorAll("main > section")).map(s => s.className);
  expect(sections.indexOf("home-sol home-container")).toBe(1);
  expect(Array.from(container.querySelectorAll(".nav-links a")).map(a => [a.textContent, a.getAttribute("href")])).toEqual([["Index", "/compute"], ["Arena", "/arena"], ["Challenge", "/challenge"], ["Agents", "/agents"], ["Docs", "/docs"]]);
  expect(Array.from(container.querySelectorAll(".nav-actions a"), a => [a.textContent, a.getAttribute("href")])).toEqual([["Trade", "/trade"]]);
  expect(container.querySelector(".nav-links")?.textContent).not.toContain("SOL-PERP");
  expect(card.querySelector(".cc-perp .market-name")?.textContent).toBe("COMPUTE-PERP");
  expect(card.querySelector(".cc-perp .market-subtitle")?.textContent).toBe("USD per 1M tokens · Qwen3.8-27B");
  expect(container.querySelector("#sol-title")?.textContent).toBe("Trade the price of compute.");
  expect(container.querySelector(".home-sol .market-name")?.textContent).toBe("COMPUTE-PERP");
  expect(Array.from(container.querySelectorAll("footer a")).filter(a => ["Index", "Docs"].includes(a.textContent!)).map(a => a.getAttribute("href"))).toEqual(["/compute", "/docs"]);
  expect(container.querySelector(".home-hero")?.textContent).not.toMatch(/mock|sample/i);
});

test("the arena strip announces only a set season or prize, and never on home or compute", async () => {
  const pages = [site.paths.home, site.paths.compute, site.paths.arena, site.paths.agents];
  for (const path of pages) {
    const container = await render(path);
    expect(container.querySelector(".announcement")).toBeNull();
    await cleanup?.(); cleanup = undefined;
  }
  const arena = { ...site.arena };
  site.arena.season = 1; site.arena.prizes = "500 USDC to the top three";
  try {
    for (const [path, shown] of [[site.paths.home, false], [site.paths.compute, false], [site.paths.arena, true], [site.paths.agents, true]] as const) {
      const container = await render(path);
      expect(container.querySelector(".announcement")?.textContent?.trim() ?? null).toBe(shown ? "Arena season 1, prizes: 500 USDC to the top three.Agents and traders on COMPUTE-PERP, ranked by PnL.See the leaderboard" : null);
      await cleanup?.(); cleanup = undefined;
    }
  } finally { Object.assign(site.arena, arena); }
});

test("no page says TBD or SOL-PERP, and SOL only means fee SOL", async () => {
  for (const path of Object.values(site.paths)) {
    const container = await render(path, new MockIndexerClient(), new MockMarketClient({ autoTick: false }), new MockComputeFeedClient());
    const text = container.textContent!;
    expect(text, path).not.toMatch(/TBD|SOL-PERP|not configured|coming soon/i);
    for (const m of text.matchAll(/\bSOL\b/g)) expect(text.slice(m.index! - 20, m.index! + 12), path).toMatch(/devnet SOL for fees/);
    await cleanup?.(); cleanup = undefined;
  }
});

test("arena: live stats, a populated board sorted by PnL with house tags, fills and ranking rules", async () => {
  const container = await render(site.paths.arena);
  expect(container.querySelector(".arena-season")).toBeNull();
  expect(Array.from(container.querySelectorAll(".arena-stats dt"), d => d.textContent)).toEqual(["Traders", "Fills", "24 h volume", "24 h fills", "Volume", "Open interest"]);
  expect(Array.from(container.querySelectorAll(".arena-stats dd"), d => d.textContent)).toEqual(["7", "214", "$45.54", "3", "$18,640.00", "$157.90 · 250 Mtok"]);
  const rows = Array.from(container.querySelectorAll(".arena-board tbody tr"));
  expect(rows).toHaveLength(7);
  expect(Array.from(rows[0].querySelectorAll("td"), td => td.textContent)).toEqual(["1", "7cVf…kxXy", "$41.87", "+4.2%", "38", "long 180 Mtok"]);
  expect(rows[1].querySelector(".house-tag")?.textContent).toBe("house · trend");
  expect(Array.from(rows[2].querySelectorAll("td"), td => td.textContent)[5]).toBe("short 60 Mtok");
  expect(rows.map(r => r.querySelector("td:nth-child(3)")!.textContent)).toEqual(["$41.87", "$18.42", "$12.05", "$4.31", "-$2.64", "-$6.98", "-$15.20"]);
  expect(container.querySelectorAll(".arena-fills tbody tr")).toHaveLength(3);
  expect(container.querySelector(".arena-rules")).toBeNull();
  expect(container.textContent).not.toContain("Joining");
  expect(Array.from(container.querySelectorAll("a")).find(a => a.textContent === "How ranking works")?.getAttribute("href")).toBe("/docs#arena");
  expect(Array.from(container.querySelectorAll(".arena-cta a"), a => [a.textContent, a.getAttribute("href")]).map(([t, h]) => [t!.trim(), h])).toEqual([["Trade", "/trade"], ["Connect an agent", "/agents"]]);
  await cleanup?.(); cleanup = undefined;
  const arena = { ...site.arena };
  site.arena.season = 1; site.arena.startsAt = "1 Dec 2026";
  try {
    const c2 = await render(site.paths.arena);
    expect(c2.querySelector(".arena-season")?.textContent).toBe("Season 1 · From 1 Dec 2026");
  } finally { Object.assign(site.arena, arena); }
});

test("docs: the Arena ranking facts live under #arena", async () => {
  const container = await render(site.paths.docs);
  const section = container.querySelector("section#arena")!;
  expect(section.querySelector("h2")?.textContent).toBe("Arena");
  expect(Array.from(section.querySelectorAll("summary"), s => s.textContent)).toEqual(["Which market does the Arena rank?", "How is the score calculated?", "How do I get on the board?", "Are house agents on the board?"]);
  expect(section.textContent).toContain("A wallet appears after its first fill. There is no sign-up.");
  expect(Array.from(container.querySelectorAll("details")).filter(d => d.open).length).toBe(1);
  expect(section.querySelector("details")!.open).toBe(false);
});

test("docs: a #arena hash opens the Arena group's first question", async () => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  const container = await render(`${site.paths.docs}#arena`);
  expect(Array.from(container.querySelectorAll("details")).filter(d => d.open)).toEqual([container.querySelector("section#arena details")]);
});

test("arena: empty board and an unreachable indexer", async () => {
  const empty = new MockIndexerClient();
  vi.spyOn(empty, "leaderboard").mockResolvedValue({ as_of_slot: 1, note: "", entries: [] });
  vi.spyOn(empty, "recentFills").mockResolvedValue({ fills: [] });
  vi.spyOn(empty, "trades").mockResolvedValue({ trades: [], next_before: null });
  let container = await render(site.paths.arena, empty);
  expect(container.textContent).toContain("No fills yet. The first trade puts a wallet on the board.");
  expect(container.querySelector(".arena-fills")).toBeNull();
  expect(container.textContent).toContain("No fills yet.");
  await cleanup?.(); cleanup = undefined;
  container = await render(site.paths.arena, null);
  expect(container.textContent).toContain("The leaderboard is unreachable right now");
  expect(container.querySelector(".arena-stats")).toBeNull();
});

test("footer blurb positions Drip and every GitHub link goes to drip-markets", async () => {
  const container = await render(site.paths.agents);
  expect(container.querySelector(".footer-about p")?.textContent).toBe("Drip is the market for AI compute. Live prices from every seller, and a perp to trade them. Built for perp traders and for agents on FLOP Network.");
  const github = Array.from(container.querySelectorAll("a")).map(a => a.getAttribute("href")!).filter(h => h.includes("github.com/Augustas11"));
  expect(github.length).toBeGreaterThan(2);
  for (const href of github) expect(href).toMatch(/^https:\/\/github\.com\/Augustas11\/drip-markets(\/|$)/);
  expect(site.agents.source).toBe("https://github.com/Augustas11/drip-markets/tree/main/agents");
});

test("agents page: one flow from install to the Arena, compute examples only once the market is configured", async () => {
  const container = await render(site.paths.agents, new MockIndexerClient(), new MockMarketClient({ autoTick: false }), new MockComputeFeedClient());
  expect(Array.from(container.querySelectorAll("main h2"), h => h.textContent)).toEqual(["Install the MCP", "What your agent can call", "Price data over HTTP", "Strategies", "The Arena", "Code"]);
  expect(Array.from(container.querySelectorAll(".ag-groups h3"), h => h.textContent)).toEqual(["Prices", "Trading", "Account"]);
  const names = Array.from(container.querySelectorAll(".ag-groups dt"), d => d.textContent);
  for (const tool of ["perps_markets", "market_stats", "get_position", "deposit", "open_position", "close_position", "withdraw", "trade_history", "leaderboard", "claim_test_funds"]) expect(names).toContain(tool);
  // The feed files are HTTP, not MCP tools: their own group, with their URLs.
  expect(names).not.toContain("market.json");
  expect(Array.from(container.querySelectorAll(".ag-files dt"), d => d.textContent)).toEqual(["market.json", "latest.json"]);
  expect(Array.from(container.querySelectorAll(".ag-files a"), a => a.getAttribute("href"))).toEqual([`${site.officialSite}/compute-feed/market.json`, `${site.officialSite}/compute-feed/latest.json`]);
  expect(container.textContent).not.toContain("rewrites both every minute");
  // The MCP trades the configured market, COMPUTE-PERP.
  const quotes = (await new MockComputeFeedClient().market()).index.n_quotes_up;
  expect(container.querySelector("h1.ag-lead")?.textContent).toBe("Your agent hedges its compute bill.");
  expect(container.querySelector(".ag-sub")?.textContent).toBe(`It reads ${quotes} live seller quotes, trades COMPUTE-PERP and ranks on the Arena by PnL.`);
  expect(container.querySelector("#install pre")?.textContent).toBe("claude mcp add drip -- npx -y @driptech/mcp");
  expect(Array.from(container.querySelectorAll("#install pre"), p => p.textContent)).toContain("Claim test funds, deposit 500 USDC and open a 100-unit long on COMPUTE-PERP. Then show my position.");
  expect(container.textContent).toContain("Long COMPUTE-PERP");
  expect(container.textContent).toContain("Short COMPUTE-PERP");
  expect(container.textContent).not.toMatch(/size_sol|keeper|prize|30 seconds/i);
  expect(container.textContent).toContain('size counts units of 1M tokens, up to 100 per fill: { side: "long", size: 100 }.');
  expect(container.textContent).toContain("Mark price, fills and open interest.");
  expect(container.textContent).toContain("House agents are tagged on the board.");
  const prompts = Array.from(container.querySelectorAll("#install pre"), p => p.textContent);
  expect(prompts).toContain("Open a 50-unit long on COMPUTE-PERP to hedge my inference bill, then show my position.");
  expect(prompts.join(" ")).not.toContain("Read the compute index");
  expect(container.textContent).not.toMatch(/ChatGPT|5-sample|SOL-PERP/);
  expect(container.textContent).toContain("MCP clients that run local servers");
  expect(container.textContent).toContain("exponential average");
  expect(container.querySelector(".addresses")?.textContent).toContain("COMPUTE-PERP market");
  expect(container.querySelector(".addresses")?.textContent).toContain("uZY6RPrFDqtbYbREA6UToWpzq1BKEsneZxcsE4As27J");
  expect(container.textContent).not.toContain("Built for agents on FLOP Network");
  expect(container.querySelectorAll("#install pre")).toHaveLength(4);
  expect(container.textContent).not.toMatch(/Percolator|market: "/);
  await cleanup?.(); cleanup = undefined;
  const compute = site.mcp.computeMarket;
  site.mcp.computeMarket = null;
  site.showAddresses = false;
  try {
    const c2 = await render(site.paths.agents);
    expect(c2.querySelectorAll("#install pre")).toHaveLength(3);
    expect(c2.querySelector(".ag-lead")?.textContent).toBe("Your agent trades compute prices.");
    expect(c2.querySelector(".ag-lead")?.textContent).not.toMatch(/hedges/);
    expect(c2.querySelector(".ag-sub")?.textContent).not.toContain("trades");
    expect(c2.textContent).not.toMatch(/Long COMPUTE-PERP/);
    expect(c2.querySelector(".addresses")).toBeNull();
  } finally { site.mcp.computeMarket = compute; site.showAddresses = true; }
  await (cleanup as (() => Promise<void>) | undefined)?.(); cleanup = undefined;
  site.mcp.hasComputeIndexTool = true; site.mcp.sizeArg = "units";
  try {
    const c3 = await render(site.paths.agents);
    expect(Array.from(c3.querySelectorAll("#install pre"), p => p.textContent)).toContain("Read the compute index, then size a long on COMPUTE-PERP to hedge my inference bill. Show my position.");
    expect(c3.textContent).toContain('{ side: "long", units: 100 }');
  } finally { site.mcp.hasComputeIndexTool = false; site.mcp.sizeArg = "size"; }
});

test("home: an indexer serving another market feeds no fills, season tiles or leaderboard", async () => {
  const other = new MockIndexerClient();
  const stats = await other.stats();
  vi.spyOn(other, "stats").mockResolvedValue({ ...stats, market: "HyMKxfct1S9aFtq21GG3FUYUViQkmpZdk5pwGRtAz48C" });
  const board = vi.spyOn(other, "leaderboard"), fills = vi.spyOn(other, "recentFills"), season = vi.spyOn(other, "season");
  const container = await render(site.paths.home, other);
  expect(board).not.toHaveBeenCalled();
  expect(fills).not.toHaveBeenCalled();
  expect(season).not.toHaveBeenCalled();
  expect(container.querySelector('[aria-label="Arena stats"]')).toBeNull();
  expect(container.querySelectorAll(".recent-trades li")).toHaveLength(0);
});

test("a nav that fails to render leaves the brand link and the page", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  vi.resetModules();
  vi.doMock("./Nav", async original => ({ ...(await original<typeof import("./Nav")>()), default: () => { throw new Error("nav broke"); } }));
  const { default: BrokenNavSite } = await import("./Site");
  window.history.pushState({}, "", site.paths.docs);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const client = new MockMarketClient({ autoTick: false });
  cleanup = async () => { await act(async () => { root.unmount(); }); client.dispose(); container.remove(); vi.doUnmock("./Nav"); vi.resetModules(); };
  await act(async () => { root.render(<BrokenNavSite client={client} indexer={null} indexerBase={null} computeFeed={null} />); });
  expect(container.querySelector(".site-header .brand")?.textContent).toBe("Drip");
  expect(container.querySelector('.site-header a[href="/arena"]')).toBeNull();
  expect(container.querySelector("main")?.textContent).not.toBe("");
  expect(container.querySelector("main .not-found")).toBeNull();
});

test("agents page describes perps_markets as the one market", async () => {
  const container = await render("/agents");
  expect(container.textContent).toContain("The market, its leverage and matcher limits.");
  expect(container.textContent).not.toContain("Markets, leverage");
});
