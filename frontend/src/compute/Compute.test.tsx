import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import Compute from "../pages/Compute";
import type { ComputeFeedClient, ComputeMarket } from "./client";
import { MockComputeFeedClient } from "./mock";
import { marketFixture } from "./fixture";
import { logScale } from "./charts";
import { site } from "../site/config";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let cleanup: (() => Promise<void>) | undefined;
afterEach(async () => {
  await cleanup?.();
  cleanup = undefined;
  vi.restoreAllMocks();
});

async function render(feed: ComputeFeedClient | null) {
  const container = document.createElement("div");
  const root = createRoot(container);
  cleanup = async () => { await act(async () => { root.unmount(); }); };
  await act(async () => { root.render(<Compute feed={feed} />); });
  return container;
}

/** A feed shaped like the deployed one: plain HTTP-style client, no mock marker. */
function liveFeed(now = Date.now(), patch: (m: ComputeMarket) => ComputeMarket = m => m): ComputeFeedClient {
  const mock = new MockComputeFeedClient(now);
  return { latest: () => mock.latest(), history: () => mock.history(), market: async () => patch(marketFixture(now)) };
}
const click = async (el: Element) => { await act(async () => { (el as HTMLElement).click(); }); };

test("renders the index, its scale and every section from the fixture", async () => {
  const now = Date.now(), market = marketFixture(now);
  const container = await render(liveFeed(now));
  const text = container.textContent!;
  expect(container.querySelector("h1")?.textContent).toBe("Compute index");
  expect(container.querySelector(".cx-level strong")?.textContent).toBe(market.index.level.toFixed(2));
  const scale = Array.from(container.querySelectorAll(".cx-scale li")).map(li => li.textContent);
  expect(scale).toEqual([`${market.index.n_quotes} quotes`, `${market.index.n_providers} providers`, `${market.index.n_models} models`, `${market.index.n_quotes_up} up now`]);
  expect(text).toContain("A 24 h change prints once the index is a day old.");
  expect(container.querySelector(".cx-widest")?.textContent).toContain("GLM 5.3: $0.150 to $3.18 across 46 sellers");
  expect(container.querySelector(".cx-widest strong")?.textContent).toBe("21×");
  // Two index points: the distribution, not a dead line.
  expect(container.querySelector(".swarm svg")?.getAttribute("aria-label")).toMatch(/^\d+ live quotes, each divided by its model's median price\. Half sit between/);
  expect(container.querySelector(".cx-index-line")).toBeNull();
  for (const heading of ["Price by model", "Providers", "Price moves", "Method"]) expect(text).toContain(heading);
  expect(text).toContain("Independent of Flop Labs and OpenRouter.");
  expect(container.querySelector(".compute-stale")).toBeNull();
});

test("draws one range strip per model on a shared log scale", async () => {
  const market = marketFixture();
  const container = await render(liveFeed());
  const rows = container.querySelectorAll(".board-row");
  expect(rows).toHaveLength(market.models.length);
  for (const row of Array.from(rows)) {
    const strip = row.querySelector('.strip[role="img"]')!;
    expect(strip.getAttribute("aria-label")).toMatch(/sellers up\. Cheapest \$[\d.]+, dearest \$[\d.]+ \([\d.]+×\)\. Middle half .* median \$[\d.]+\.$/);
    expect(strip.querySelector(".strip-box")).not.toBeNull();
    expect(strip.querySelector(".strip-median")).not.toBeNull();
  }
  // Default order: most sellers first.
  expect(rows[0].querySelector(".m-name")?.textContent).toBe("GLM 5.3");
  const glm = market.models.find(m => m.name === "GLM 5.3")!;
  expect(rows[0].querySelectorAll(".strip-dot")).toHaveLength(glm.quotes.filter(q => q.price !== null).length);
  expect(container.querySelectorAll(".strip-axis span").length).toBeGreaterThan(2);
  // Thin models are tagged.
  expect(Array.from(container.querySelectorAll(".board-row.is-thin .m-name")).map(n => n.textContent).sort()).toEqual(market.models.filter(m => m.thin).map(m => m.name).sort());
  expect(container.querySelectorAll(".board-row.is-thin").length).toBeGreaterThan(0);
});

test("a row expands to its full seller list and collapses again", async () => {
  const market = marketFixture();
  const container = await render(liveFeed());
  const head = container.querySelector<HTMLButtonElement>(".board-row .board-head")!;
  expect(head.getAttribute("aria-expanded")).toBe("false");
  expect(container.querySelector(".seller-table")).toBeNull();
  await click(head);
  expect(head.getAttribute("aria-expanded")).toBe("true");
  const panel = container.querySelector(`#${head.getAttribute("aria-controls")}`)!;
  const glm = market.models.find(m => m.name === "GLM 5.3")!;
  const rows = panel.querySelectorAll(".seller-table tbody tr");
  expect(rows).toHaveLength(glm.quotes.length);
  const first = Array.from(rows[0].children).map(c => c.textContent);
  expect(first[0]).toContain(glm.quotes[0].provider);
  expect(first[2]).toBe("$0.150");
  expect(first[5]).toMatch(/up|down/);
  expect(first[6]).toMatch(/%$/);
  await click(head);
  expect(head.getAttribute("aria-expanded")).toBe("false");
  expect(container.querySelector(".seller-table")).toBeNull();
});

test("sort buttons reorder the board", async () => {
  const container = await render(liveFeed());
  const button = Array.from(container.querySelectorAll(".cx-sort button")).find(b => b.textContent === "Highest median")!;
  await click(button);
  expect(button.getAttribute("aria-pressed")).toBe("true");
  const medians = Array.from(container.querySelectorAll(".board-row .c-median")).map(el => Number(el.textContent!.replace(/[^\d.]/g, "")));
  expect(medians).toEqual([...medians].sort((a, b) => b - a));
});

test("providers, price moves and dynamic pricers come from the feed", async () => {
  const market = marketFixture();
  const container = await render(liveFeed());
  const providers = container.querySelector('[aria-labelledby="compute-providers-heading"]')!;
  expect(providers.querySelectorAll("tbody tr")).toHaveLength(8);
  await click(providers.querySelector(".cx-more")!);
  expect(providers.querySelectorAll("tbody tr")).toHaveLength(market.providers.length);
  const moves = container.querySelector('[aria-labelledby="compute-moves-heading"]')!;
  expect(moves.querySelectorAll(".move-list:not(.dyn-list) li")).toHaveLength(market.changes.length);
  expect(moves.querySelectorAll(".dyn-list li")).toHaveLength(market.dynamic.length);
  expect(moves.querySelector(".dyn-list li")?.textContent).toContain("212 changes");
});

test("the perp and FLOP panels show the settlement price, its sellers and FLOP's rate", async () => {
  const container = await render(liveFeed());
  const perp = container.querySelector('[aria-labelledby="compute-perp-heading"]')!;
  expect(perp.querySelector(".perp-price strong")?.textContent).toBe("$0.3036");
  expect(Array.from(perp.querySelectorAll(".perp-sellers li")).map(li => li.textContent)).toEqual(["DeepInfra bf16$0.2578up", "Cerebras fp16$1.0213up"]);
  expect(perp.textContent).toContain("Solana devnet, test tokens");
  expect(perp.textContent).toContain("full-precision (bf16/fp16) sellers of one benchmark model");
  expect(perp.querySelector('a[href^="https://explorer.solana.com/tx/4w4fnV"]')).not.toBeNull();
  expect(perp.querySelector('a[href="https://explorer.solana.com/address/uZY6RPrFDqtbYbREA6UToWpzq1BKEsneZxcsE4As27J?cluster=devnet"]')?.textContent).toContain("Market account");
  expect(perp.querySelector(".perp-hist figcaption")?.textContent).toBe("7 d · low $0.3036 · high $0.3112");
  const flop = container.querySelector('[aria-labelledby="compute-flop-heading"]')!;
  expect(flop.querySelector(".perp-price strong")?.textContent).toBe("0.00291");
  expect(Array.from(flop.querySelectorAll(".flop-facts dd")).map(v => v.textContent)).toEqual(["3", "34", "185", "50%"]);
  expect(flop.textContent).toContain("not a traded price");
  const how = container.querySelector('[aria-labelledby="compute-how-heading"]')!;
  expect(how.textContent).toContain("staging verify-push-log");
  expect(how.textContent).toContain("Clamp now: off (n<3).");
  expect(how.textContent).toContain(`Combines the ${marketFixture().index.n_models} model medians`);
});

test("trade link appears when configured", async () => {
  const trade = site.computeTrade;
  site.computeTrade = "https://example.invalid/trade";
  try {
    const container = await render(liveFeed());
    expect(container.querySelector('a[href="https://example.invalid/trade"]')?.textContent).toContain("Trade the perp");
  } finally { site.computeTrade = trade; }
});

test("live mode never says mock or sample", async () => {
  const container = await render(liveFeed());
  await click(container.querySelector(".board-head")!);
  expect(container.textContent).not.toMatch(/mock|sample|fixture|placeholder/i);
  expect(container.innerHTML).not.toMatch(/mock|sample/i);
});

test("shows skeletons while loading, then unreachable when every file fails", async () => {
  let fail!: (e: Error) => void;
  const pending = new Promise<never>((_, reject) => { fail = reject; });
  const container = await render({ latest: () => pending, history: () => pending, market: () => pending });
  expect(container.querySelector('.cx-skeleton[aria-busy="true"]')).not.toBeNull();
  expect(container.querySelectorAll(".sk").length).toBeGreaterThan(3);
  await act(async () => { fail(new Error("price feed 502")); });
  expect(container.querySelector(".cx-skeleton")).toBeNull();
  expect(container.querySelector(".cx-hero [role=status]")?.textContent).toBe("Market feed unreachable (price feed 502). Try again in a minute.");
  expect(container.querySelector(".cx-panel [role=status]")?.textContent).toBe("Price feed unreachable (price feed 502). Try again in a minute.");
  expect(container.querySelector(".board")).toBeNull();
});

test("a failed market.json still shows the perp panel", async () => {
  const mock = new MockComputeFeedClient();
  const container = await render({ latest: () => mock.latest(), history: () => mock.history(), market: () => Promise.reject(new Error("price feed 404")) });
  expect(container.textContent).toContain("Market feed unreachable (price feed 404).");
  expect(container.querySelector('[aria-labelledby="compute-perp-heading"] .perp-price strong')?.textContent).toBe("$0.3036");
});

test("marks a stale market feed and a stale settlement price", async () => {
  const now = Date.now();
  const container = await render(liveFeed(now, m => ({ ...m, generated_ms: now - 45 * 60_000 })));
  expect(container.querySelector(".cx-hero .compute-stale")?.textContent).toBe("stale");
  expect(container.textContent).toContain("The market feed has not updated for over 30 minutes.");
  await cleanup?.();
  const mock = new MockComputeFeedClient(Date.now(), 45 * 60_000);
  const c2 = await render({ latest: () => mock.latest(), history: () => mock.history(), market: () => mock.market() });
  expect(c2.querySelector(".cx-panel .compute-stale")?.textContent).toBe("stale");
  expect(c2.textContent).toContain("No new price for over 30 minutes.");
});

test("latest.json from the future or a stopped feed is stale", async () => {
  const feed = new MockComputeFeedClient();
  const base = await feed.latest();
  const old = Date.now() - 31 * 60_000;
  vi.spyOn(feed, "latest").mockResolvedValue({ ...base, generated_ms: old, landed_ms: old - 60_000 });
  let container = await render(feed);
  expect(container.textContent).toContain("The price feed has not updated for over 30 minutes.");
  await cleanup?.();
  vi.spyOn(feed, "latest").mockResolvedValue({ ...base, landed_ms: base.generated_ms + 6 * 60_000 });
  container = await render(feed);
  expect(container.textContent).toContain("The latest price has a time in the future.");
});

test("an index with 12 or more points draws its line with a since date", async () => {
  const now = Date.now();
  const container = await render(liveFeed(now, m => ({ ...m, index: { ...m.index, change_24h_pct: 1.234, change_7d_pct: -0.5, series: Array.from({ length: 24 }, (_, i) => [now - (24 - i) * 3_600_000, 100 + i / 10] as [number, number]) } })));
  expect(container.querySelector(".swarm")).toBeNull();
  expect(container.querySelector(".cx-index-line figcaption")?.textContent).toMatch(/^Index since \d+ \w+ \d{4}$/);
  expect(Array.from(container.querySelectorAll(".cx-changes dd")).map(d => d.textContent)).toEqual(["+1.2%", "−0.5%"]);
  expect(container.textContent).not.toContain("A 24 h change prints");
});

test("a flop_standin price explains its own source", async () => {
  const feed = new MockComputeFeedClient();
  const base = await feed.latest();
  vi.spyOn(feed, "latest").mockResolvedValue({ ...base, source: "flop_standin" });
  const container = await render(feed);
  expect(container.querySelector('[aria-labelledby="compute-how-heading"]')!.textContent).toContain("FLOP's on-chain price (flop_standin)");
  expect(container.querySelector('[aria-labelledby="compute-flop-heading"]')!.textContent).toContain("This sets the settlement price right now");
});

test("without a feed the page says so", async () => {
  const container = await render(null);
  expect(container.textContent).toContain("Price feed not configured.");
});

test("log scale ticks stay on 1-2-5 steps and inside the range", () => {
  const s = logScale(0.04, 3.18);
  expect(s.lo).toBe(0.02);
  expect(s.hi).toBe(5);
  expect(s.ticks).toEqual([0.02, 0.05, 0.1, 0.2, 0.5, 1, 2, 5]);
  expect(s.at(s.lo)).toBe(0);
  expect(s.at(s.hi)).toBeCloseTo(100);
});
