import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import Site from "./Site";
import { site } from "./config";
import { Link } from "./router";
import { MockMarketClient } from "../market/mock";
import { MockIndexerClient } from "../indexer/mock";
import type { IndexerClient } from "../indexer/client";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let cleanup: (() => Promise<void>) | undefined;
afterEach(async () => {
  await cleanup?.();
  cleanup = undefined;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

async function render(path: string, indexer: IndexerClient | null = new MockIndexerClient(), client = new MockMarketClient({ autoTick: false })) {
  window.history.pushState({}, "", path);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  cleanup = async () => {
    await act(async () => { root.unmount(); });
    client.dispose();
    container.remove();
  };
  await act(async () => { root.render(<Site client={client} indexer={indexer} indexerBase={null} />); });
  return container;
}

test.each([
  [site.paths.home, "The House ofPercolatorPerps.", "From agent to leaderboard.", "drip"],
  [site.paths.tools, "Tools", "Keeper CLI", "Tools · drip"],
  [site.paths.arena, "Arena", "Rules", "Arena · drip"],
  [site.paths.build, "Build", "Add an MCP tool", "Build · drip"],
  [site.paths.trade, "SOL-PERP", "Connect wallet", "Trade · drip"],
])("renders %s", async (path, heading, text, title) => {
  const container = await render(path);
  expect(container.querySelector("h1")?.textContent).toBe(heading);
  expect(container.textContent).toContain(text);
  expect(document.title).toBe(title);
  expect(container.querySelector("nav")).not.toBeNull();
  expect(container.querySelector("footer")?.textContent).toContain("Devnet only. Test tokens, no real value.");
});

test("home shows its live mark, recent trades and honest unpublished tools", async () => {
  const container = await render(site.paths.home);
  expect(container.textContent).toContain("$150.00");
  expect(container.textContent).toContain("From agent to leaderboard.");
  expect(container.textContent).not.toMatch(/PERCOLATOR PERPS · SOLANA|LIVE|Illustrative/);
  expect(container.textContent).toContain("Give agents a wallet, leverage and a live SOL-PERP market.");
  expect(container.textContent).toContain("Mark price");
  expect(container.querySelector('.hero-actions a[href="/trade"]')?.textContent?.trim()).toBe("Trade SOL-PERP");
  expect(container.querySelector('.hero-actions a[href="/arena"]')?.textContent).toBe("Watch agents trade");
  expect(container.textContent).toContain("Prizes TBD.");
  expect(container.textContent).toContain("Indexer not configured");
  expect(container.textContent).not.toMatch(/\[TBD\]|\[NPM-PACKAGE\]|npx/);
  expect(container.querySelector(".terminal button")).toBeNull();
});

test("footer community destinations stay plain text until configured", async () => {
  const container = await render(site.paths.home);
  const footer = container.querySelector("footer")!;
  for (const label of ["X", "Telegram", "Status"]) {
    expect(Array.from(footer.querySelectorAll("span")).some(el => el.textContent === label)).toBe(true);
    expect(Array.from(footer.querySelectorAll("a")).some(el => el.textContent === label)).toBe(false);
  }
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
  const tools = Array.from(container.querySelectorAll("nav a")).find(a => a.textContent === "Tools")!;
  await act(async () => { tools.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 })); });
  expect(window.location.pathname).toBe(site.paths.tools);
  expect(push).toHaveBeenLastCalledWith({}, "", site.paths.tools);
  expect(container.querySelector("h1")?.textContent).toBe("Tools");
  expect(scroll).toHaveBeenCalledWith(0, 0);
  await act(async () => {
    window.history.replaceState({}, "", site.paths.build);
    window.dispatchEvent(new PopStateEvent("popstate"));
  });
  expect(container.querySelector("h1")?.textContent).toBe("Build");
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

test("trailing slash renders Tools and its nav link only scrolls to top", async () => {
  const scroll = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  const container = await render("/tools/");
  expect(container.querySelector("h1")?.textContent).toBe("Tools");
  const push = vi.spyOn(window.history, "pushState");
  const tools = container.querySelector<HTMLAnchorElement>('nav a[href="/tools"]')!;
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
  await navigate(site.paths.tools);
  expect(container.querySelector(".site-page h1")?.textContent).toBe("Tools");
  expect(container.querySelector("#amount")?.closest("[hidden]")).not.toBeNull();
  await navigate(site.paths.trade);
  expect(container.querySelector("#amount")?.closest("[hidden]")).toBeNull();
  expect(container.querySelector<HTMLFieldSetElement>('[aria-labelledby="collateral-heading"] fieldset')!.disabled).toBe(true);
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
    const container = await render(site.paths.tools);
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
    const container = await render(site.paths.tools);
    expect(container.textContent).toContain("The MCP package is not published yet. The install command will appear here when it is.");
    expect(container.textContent).not.toContain("npx");
    const mcp = container.querySelector("section")!;
    expect(mcp.querySelector("pre")).toBeNull();
    expect(mcp.querySelector("button")).toBeNull();
  } finally { site.mcp.final = final; }
});

test("unknown paths offer a link home", async () => {
  const container = await render("/missing");
  expect(container.querySelector("h1")?.textContent).toBe("Page not found");
  expect(container.querySelector("main a")?.getAttribute("href")).toBe(site.paths.home);
});


test("home renders recent fills and live season totals", async () => {
  const container = await render(site.paths.home);
  const ticker = container.querySelector(".recent-trades")!;
  expect(ticker.querySelectorAll("li")).toHaveLength(3);
  for (const text of ["Long", "Short", "0.5 SOL", "1.5 SOL", "$97.75", "Ab12…yZ9q", "mock…olio"]) expect(ticker.textContent).toContain(text);
  expect(ticker.textContent).toMatch(/\d+[smhd] ago/);
  const link = ticker.querySelector("li a")!;
  expect(link.getAttribute("href")).toBe("https://explorer.solana.com/tx/mock-signature-0?cluster=devnet");
  expect(link.getAttribute("target")).toBe("_blank");
  expect(link.getAttribute("rel")).toBe("noreferrer");
  const season = container.querySelector('[aria-label="Season 0"]')!;
  expect(Array.from(season.querySelectorAll(".fact-label"), el => el.textContent)).toEqual(["Agents", "Trades", "Volume (USD)", "Open interest (USD)"]);
  expect(Array.from(season.querySelectorAll(".fact-value"), el => el.textContent)).toEqual(["12", "345", "$12,345", "$987.50"]);
  expect(season.textContent).toContain("Season 0 · devnet · since " + new Date(1793232000 * 1000).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" }));
});

test.each(["rejects", "missing", "none", "empty"])("home handles unavailable data: %s", async mode => {
  const indexer: IndexerClient = new MockIndexerClient();
  indexer.recentFills = mode === "missing" ? undefined : mode === "empty" ? vi.fn().mockResolvedValue({ fills: [] }) : vi.fn().mockRejectedValue(new Error("indexer 404"));
  indexer.season = mode === "missing" ? undefined : vi.fn().mockRejectedValue(new Error("indexer 404"));
  const container = await render(site.paths.home, mode === "none" ? null : indexer);
  expect(container.querySelector(".recent-trades")?.textContent).toContain("No agent trades yet.");
  expect(container.querySelector('.recent-trades a[href="/tools"]')?.textContent).toBe("Be the first.");
  expect(Array.from(container.querySelectorAll(".fact-value"), el => el.textContent)).toEqual(["—", "—", "—", "—"]);
  expect(container.querySelector(".home-facts")?.textContent).toContain("Season 0 · devnet · since —");
});

test.each([
  ["0", "0", "$0.00", "$0.00"],
  ["bad", "broken", "—", "—"],
  ["9007199254740993000000", "12345678", "$9,007,199,254,740,993", "$12.35"],
])("home formats season atoms %s and %s", async (volume, interest, expectedVolume, expectedInterest) => {
  const indexer = new MockIndexerClient();
  vi.spyOn(indexer, "season").mockResolvedValue({ season: 0, since: null, agents: 0, trades: 0, volume_usd: volume, open_interest_usd: interest });
  const container = await render(site.paths.home, indexer);
  expect(Array.from(container.querySelectorAll(".fact-value"), el => el.textContent)).toEqual(["0", "0", expectedVolume, expectedInterest]);
  expect(container.querySelector(".home-facts")?.textContent).toContain("since —");
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
  expect(container.querySelector(".facts-grid")?.textContent).toContain("$12,345");
  await cleanup?.();
  cleanup = undefined;
  await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
  expect(fills).toHaveBeenCalledTimes(4);
  expect(season).toHaveBeenCalledTimes(2);
});

test("home explains why Percolator", async () => {
  const container = await render(site.paths.home);
  const why = container.querySelector('[aria-labelledby="why-title"]')!;
  expect(why.querySelector("h2")?.textContent).toBe("Why Percolator");
  expect(Array.from(why.querySelectorAll("h3"), el => el.textContent)).toEqual(["Built by Anatoly Yakovenko", "Profits are backed", "Each market stands alone", "Anyone can run it"]);
  expect(Array.from(why.querySelectorAll("p"), el => el.textContent)).toEqual([
    "Solana's co-founder wrote Percolator as an open-source perps engine.",
    "Gains pay out only up to what the market can cover. No paper profits.",
    "A blow-up in one market can't take money from another. The engine ships proofs that trades can't create value.",
    "Any keeper can crank a market. No company-run matching server.",
  ]);
});
