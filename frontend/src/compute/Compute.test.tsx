import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import Compute, { windowLabel } from "../pages/Compute";
import Docs from "../pages/Docs";
import type { ComputeFeedClient, ComputeMarket } from "./client";
import { MockComputeFeedClient } from "./mock";
import { marketFixture } from "./fixture";
import { logScale, spreadRatios, typicalPrice, upPriced, widestModels } from "./charts";
import { count, times, usd, usdSmall } from "./format";
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
  const typical = typicalPrice(market)!;
  expect(container.querySelector(".cx-label")?.textContent).toBe("Median price to run an open model");
  expect(container.querySelector(".cx-price")?.textContent).toBe(`${usd(typical.median)}per 1M tokens`);
  expect(container.querySelector(".cx-range")).toBeNull();
  expect(Array.from(container.querySelectorAll(".cx-proof li"), li => li.textContent)).toEqual([`${count(market.index.n_quotes_up)} live quotes`, `${count(market.index.n_providers)} providers`, `${count(market.models.length)} models`]);
  expect(typical.quotes).toBe(market.models.filter(m => !m.thin).reduce((n, m) => n + upPriced(m).length, 0));
  expect(container.querySelector(".cx-indexstat")).toBeNull();
  expect(container.querySelector(".cx-spread-line")?.textContent).toBe("The same model costs up to 21\u00d7 more depending on the seller.");
  expect(container.querySelector(".cx-spread-eg")).toBeNull();
  const wide = widestModels(market);
  expect(container.querySelector(".cx-hero-side .cx-spread-line strong")?.textContent).toBe(times(wide[0].min, wide[0].max));
  expect(container.querySelector(".ranks")).toBeNull();
  const swarm = container.querySelector(".cx-hero-side .swarm svg");
  expect(swarm?.getAttribute("aria-label")).toMatch(/^\d+ live quotes, each divided by its model's median price/);
  expect(container.querySelectorAll(".cx-hero-side circle.swarm-dot")).toHaveLength(spreadRatios(market).length);
  expect(container.querySelector(".swarm-count")).toBeNull();
  expect(container.querySelector(".cx-hero-side .swarm figcaption")?.textContent).toBe("below medianabove median");
  // Who it is for: each entry jumps to a section that exists.
  const who = Array.from(container.querySelectorAll(".cx-who a"));
  expect(who.map(a => a.getAttribute("href"))).toEqual(["#board", "#providers", "#perp"]);
  for (const a of who) expect(container.querySelector(a.getAttribute("href")!)).not.toBeNull();
  expect(who[0].textContent).toContain(`Cheapest seller for each of ${market.models.length} models`);
  expect(who[2].textContent).toContain("now $0.3036");
  // The perp sits right after the board, before providers.
  const order = ["#board", "#perp", "#providers"].map(id => Array.from(container.querySelectorAll("section, article")).indexOf(container.querySelector(id)!));
  expect(order).toEqual([...order].sort((a, b) => a - b));
  // The hero no longer carries an index line.
  expect(container.querySelector(".cx-index-line")).toBeNull();
  expect(text).not.toMatch(/\b(?:mock|sample)\b/i);
  for (const heading of ["Price by model", "Providers", "Price moves"]) expect(text).toContain(heading);
  // The method lives in Docs; /compute keeps one line pointing there.
  expect(container.querySelector('[aria-labelledby="compute-how-heading"]')).toBeNull();
  expect(container.querySelector(".cx-how a")?.getAttribute("href")).toBe("/docs#compute-index");
  expect(container.querySelector(".cx-how")?.textContent).toBe("How these prices are made");
  expect(text).not.toMatch(/Independent of|devnet only|no real value/i);
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

test("shows the top 20 models until asked for all", async () => {
  const container = await render(liveFeed(Date.now(), m => ({ ...m, models: [...m.models, ...m.models.slice(0, 5).map(x => ({ ...x, id: `${x.id}-b`, name: `${x.name} B` }))] })));
  expect(container.querySelectorAll(".board-row")).toHaveLength(20);
  const more = container.querySelector("#board .cx-more")!;
  expect(more.textContent).toBe("Show all 25 models");
  await click(more);
  expect(container.querySelectorAll(".board-row")).toHaveLength(25);
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
  expect(moves.textContent).not.toContain("Frequent repricers");
});

test("a seller with 3+ changes is grouped as a frequent repricer, never contradicted", async () => {
  const now = Date.now();
  const repeat = (at: number, from: number, to: number) => ({ at_ms: now - at * 60_000, model: "openai/gpt-oss-120b", provider: "PrimeIntellect", quantization: "fp4", from, to });
  const container = await render(liveFeed(now, m => ({ ...m, dynamic: [], changes: [repeat(5, 0.0385, 0.0493), repeat(15, 0.0493, 0.041), repeat(25, 0.041, 0.0385), { ...repeat(9, 0.2, 0.22), provider: "Novita" }] })));
  const moves = container.querySelector('[aria-labelledby="compute-moves-heading"]')!;
  expect(moves.textContent).toContain("Frequent repricers");
  const rows = Array.from(moves.querySelectorAll(".dyn-list li")).map(li => li.textContent);
  expect(rows).toEqual(["PrimeIntellectGPT-OSS-120B · fp4$0.0385–$0.04933 changes"]);
  expect(moves.querySelectorAll(".move-list:not(.dyn-list) li")).toHaveLength(1);
  expect(moves.textContent).not.toContain("No seller repriced");
  expect(moves.textContent).not.toContain("Dynamic pricers");
});

test("the perp and FLOP panels show the settlement price, its sellers and FLOP's rate", async () => {
  const container = await render(liveFeed());
  const perp = container.querySelector('[aria-labelledby="compute-perp-heading"]')!;
  expect(perp.querySelector(".perp-price strong")?.textContent).toBe("$0.3036");
  expect(Array.from(perp.querySelectorAll(".perp-sellers li")).map(li => li.textContent)).toEqual(["DeepInfra bf16$0.2578up", "Cerebras fp16$1.0213up"]);
  expect(perp.querySelector(".cx-k")?.textContent).toBe("COMPUTE-PERP · USD per 1M tokens · Qwen3.8-27B");
  expect(perp.querySelector(".trade-note")?.textContent).toBe("Test tokens");
  expect(perp.textContent).not.toContain("full-precision (bf16/fp16) sellers of one benchmark model");
  expect(perp.querySelector('a[href^="https://explorer.solana.com/tx/4w4fnV"]')).not.toBeNull();
  expect(perp.querySelector('.panel-actions a[href="/trade"]')?.textContent?.trim()).toBe("Trade COMPUTE-PERP");
  expect(perp.querySelector(".perp-hist figcaption")?.textContent).toBe("7 d · low $0.3036 · high $0.3112");
  const flop = container.querySelector('[aria-labelledby="compute-flop-heading"]')!;
  expect(Array.from(flop.querySelectorAll(".flop-parity .flop-facts dd"), v => v.textContent)).toEqual([usd(marketFixture().flop!.parity!.market_usd_per_bn_gn), "0.00291"]);
  expect(flop.querySelector(".flop-window")?.textContent).toBe("Last 6 hours");
  expect(Array.from(flop.querySelectorAll(".flop-facts:not(.flop-parity .flop-facts) dt")).map(v => v.textContent)).toEqual(["Buyer accounts", "Miners", "Jobs settled"]);
  expect(Array.from(flop.querySelectorAll(".flop-facts:not(.flop-parity .flop-facts) dd")).map(v => v.textContent)).toEqual(["3", "34", "185"]);
  expect(flop.textContent).not.toContain("FLOP's gateway buys on behalf");
  expect(flop.textContent).not.toContain("The perp settles on seller prices, not on this rate.");
});

test("trade link appears when configured", async () => {
  const trade = site.computeTrade;
  site.computeTrade = "https://example.invalid/trade";
  try {
    const container = await render(liveFeed());
    expect(container.querySelector('a[href="https://example.invalid/trade"]')?.textContent).toContain("Trade COMPUTE-PERP");
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
  expect(container.querySelector(".cx-hero p.cx-updated.is-stale")).not.toBeNull();
  expect(container.querySelector(".cx-hero .cx-updated .sr-only")?.textContent).toBe(" (stale: No new prices read for over 30 minutes.)");
  expect(container.querySelector(".cx-hero .compute-stale")).toBeNull();
  await cleanup?.();
  const mock = new MockComputeFeedClient(Date.now(), 45 * 60_000);
  const c2 = await render({ latest: () => mock.latest(), history: () => mock.history(), market: () => mock.market() });
  expect(c2.querySelector(".cx-panel .compute-stale")?.textContent).toMatch(/^stale: /);
  expect(c2.querySelector(".cx-panel .compute-stale")?.getAttribute("title")).toBe("No new price for over 30 minutes.");
});

test("staleness follows data_ms, not when the file was written", async () => {
  const now = Date.now();
  let container = await render(liveFeed(now, m => ({ ...m, generated_ms: now, data_ms: now - 45 * 60_000 })));
  expect(container.querySelector(".cx-hero .cx-updated.is-stale")).not.toBeNull();
  expect(container.querySelector(".cx-updated")?.textContent).toContain("Updated 45 min ago");
  await cleanup?.();
  container = await render(liveFeed(now, m => ({ ...m, generated_ms: now - 45 * 60_000, data_ms: now - 60_000 })));
  expect(container.querySelector(".cx-hero .cx-updated")).not.toBeNull();
  expect(container.querySelector(".cx-hero .cx-updated.is-stale")).toBeNull();
  await cleanup?.();
  container = await render(liveFeed(now, m => ({ ...m, data_ms: null })));
  expect(container.querySelector(".cx-hero .cx-updated.is-stale")?.textContent).toMatch(/^No recent update \(stale: /);
});

test("a stale model shows no green up dots", async () => {
  const now = Date.now();
  const container = await render(liveFeed(now, m => ({ ...m, models: m.models.map((x, i) => i === 0 ? { ...x, stale: true } : i === 1 ? { ...x, data_ms: now - 40 * 60_000 } : { ...x, data_ms: now }) })));
  const rows = container.querySelectorAll(".board-row");
  for (const row of [rows[0], rows[1]]) {
    expect(row.querySelector(".tag")?.textContent).toBe("stale");
    expect(row.querySelectorAll(".strip-dot:not(.is-stale)")).toHaveLength(0);
  }
  expect(rows[2].querySelectorAll(".strip-dot:not(.is-stale):not(.is-down)").length).toBeGreaterThan(0);
  await click(rows[0].querySelector(".board-head")!);
  expect(rows[0].querySelectorAll(".seller-table .state.is-up")).toHaveLength(0);
  expect(rows[0].querySelector(".seller-table .state")?.textContent).toBe("stale");
});

test("null index level, base and cheapest render as dashes", async () => {
  const container = await render(liveFeed(Date.now(), m => ({ ...m, index: { ...m.index, level: null, base_ms: null, series: [] }, models: m.models.map((x, i) => i === 0 ? { ...x, cheapest: null } : x) })));
  expect(container.querySelector(".cx-indexstat")).toBeNull();
  expect(container.querySelector(".board-row .c-cheap")?.textContent).toBe("cheapest —");
  expect(container.querySelector("#perp")).not.toBeNull();
});

test("one malformed section does not blank the page", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  const container = await render(liveFeed(Date.now(), m => ({ ...m, changes: null as never })));
  expect(container.querySelector(".cx-broken")?.textContent).toMatch(/^Price moves: this part of the feed could not be shown/);
  expect(container.querySelectorAll(".board-row").length).toBeGreaterThan(0);
  expect(container.querySelector("#perp")).not.toBeNull();
});

test("no models: a plain empty state, not a hung tab", async () => {
  expect(logScale(Infinity, -Infinity).ticks.length).toBeGreaterThan(0);
  const container = await render(liveFeed(Date.now(), m => ({ ...m, models: [], providers: [], changes: [], dynamic: [] })));
  expect(container.querySelector(".cx-label")?.textContent).toBe("No prices right now.");
  expect(container.querySelector("#board .cx-empty")?.textContent).toMatch(/^No prices right now/);
  expect(container.querySelector(".cx-proof")).toBeNull();
});

test("held quotes do not change the hero counts and the held sentence is gone", async () => {
  const container = await render(liveFeed(Date.now(), m => ({ ...m, models: m.models.map((x, i) => i === 0 ? { ...x, quotes_held: 2 } : x) })));
  const market = marketFixture();
  expect(Array.from(container.querySelectorAll(".cx-proof li b"), b => b.textContent)[0]).toBe(count(market.index.n_quotes_up));
  expect(container.querySelector(".cx-proof")?.textContent).not.toContain("held");
});

test("latest.json from the future or a stopped feed is stale", async () => {
  const feed = new MockComputeFeedClient();
  const base = await feed.latest();
  const old = Date.now() - 31 * 60_000;
  vi.spyOn(feed, "latest").mockResolvedValue({ ...base, generated_ms: old, landed_ms: old - 60_000 });
  let container = await render(feed);
  expect(container.querySelector('.compute-stale[title="The price feed has not updated for over 30 minutes."]')).not.toBeNull();
  await cleanup?.();
  vi.spyOn(feed, "latest").mockResolvedValue({ ...base, landed_ms: base.generated_ms + 6 * 60_000 });
  container = await render(feed);
  expect(container.querySelector('.compute-stale[title="The latest price has a time in the future."]')).not.toBeNull();
});

test("the hero shows no index line or index change, however long the series", async () => {
  const now = Date.now();
  const container = await render(liveFeed(now, m => ({ ...m, index: { ...m.index, change_24h_pct: 1.234, change_7d_pct: -0.5, series: Array.from({ length: 24 }, (_, i) => [now - (24 - i) * 3_600_000, 100 + i / 10] as [number, number]) } })));
  expect(container.querySelector(".cx-index-line")).toBeNull();
  expect(container.querySelector(".cx-indexstat")).toBeNull();
  expect(container.querySelector(".cx-hero")?.textContent).not.toMatch(/24 h|7 d/);
});

test("a flop_standin price still shows the FLOP panel without the removed explainer", async () => {
  const feed = new MockComputeFeedClient();
  const base = await feed.latest();
  vi.spyOn(feed, "latest").mockResolvedValue({ ...base, source: "flop_standin" });
  const container = await render(feed);
  expect(container.querySelector('[aria-labelledby="compute-flop-heading"]')!.textContent).not.toContain("This sets the settlement price right now");
});

test("without a feed the page says so", async () => {
  const container = await render(null);
  expect(container.textContent).toContain("The compute index is unreachable right now.");
});

test("log scale ticks stay on 1-2-5 steps and inside the range", () => {
  const s = logScale(0.04, 3.18);
  expect(s.lo).toBe(0.02);
  expect(s.hi).toBe(5);
  expect(s.ticks).toEqual([0.02, 0.05, 0.1, 0.2, 0.5, 1, 2, 5]);
  expect(s.at(s.lo)).toBe(0);
  expect(s.at(s.hi)).toBeCloseTo(100);
});

async function renderDocs(feed: ComputeFeedClient | null) {
  const container = document.createElement("div");
  const root = createRoot(container);
  cleanup = async () => { await act(async () => { root.unmount(); }); };
  await act(async () => { root.render(<Docs feed={feed} />); });
  return container;
}

test("docs explain how the index is made, with live values from the feed", async () => {
  const container = await renderDocs(liveFeed());
  expect(Array.from(container.querySelectorAll("h2"), h => h.textContent)).toEqual(["Compute index", "COMPUTE-PERP", "Arena", "Agents"]);
  const how = container.querySelector("#compute-index")!;
  expect(Array.from(how.querySelectorAll("summary"), d => d.textContent)).toEqual(["What is a quote?", "How do input and output prices become one price?", "When does a seller count as up?", "How is a model's price set?", "How often are prices read?", "How is the index calculated?", "What is the routed price?", "When is data marked stale?"]);
  expect(Array.from(container.querySelectorAll("details")).map(d => d.open)).toEqual([true, ...Array(14).fill(false)]);
  expect(how.textContent).toContain("every model every 5 minutes");
  expect(how.textContent).toContain("listed and up");
  expect(how.textContent).toContain(`Geometric mean of the ${marketFixture().index.n_models} model medians`);
  expect(how.textContent).toContain("Median of the quotes that are up.");
  expect(how.textContent).toContain("cheaper sellers get more (1 ÷ price²), weighted by each seller's recent uptime, with no seller above half the weight");
  expect(how.textContent).toContain("No new read for 30 minutes. The update time turns red");
  const perp = container.querySelector("#compute-perp")!;
  expect(Array.from(perp.querySelectorAll("summary"), d => d.textContent)).toEqual(["What does COMPUTE-PERP settle on?", "How can I check a settlement price?"]);
  expect(perp.textContent).toContain("Clamp now: off (n<3).");
  expect(perp.textContent).toContain("Each settlement price is written to Solana (latest transaction).");
  expect(container.textContent).not.toContain("signed line");
  expect(container.textContent).not.toMatch(/verify-push-log|stand-in/);
  expect(container.textContent).toContain("Independent of Flop Labs and OpenRouter.");
});

test("docs state the rules without live counts when there is no feed", async () => {
  const container = await renderDocs(null);
  const how = container.querySelector("#compute-index")!;
  expect(how.textContent).toContain("Geometric mean of the model medians");
  expect(how.textContent).not.toContain("Clamp now");
});

test("docs describe a flop_standin settlement price", async () => {
  const feed = new MockComputeFeedClient();
  const base = await feed.latest();
  vi.spyOn(feed, "latest").mockResolvedValue({ ...base, source: "flop_standin" });
  const container = await renderDocs(feed);
  expect(container.querySelector("#compute-perp")!.textContent).toContain("FLOP's on-chain price (flop_standin)");
});

test("mix switch: hidden without per-mix data, then moves the headline, the medians and the sort", async () => {
  let container = await render(liveFeed());
  expect(container.querySelector(".cx-mix")).toBeNull();
  await cleanup?.();
  const now = Date.now(), market = marketFixture(now);
  const cheap = [...market.models].filter(m => !m.thin).sort((a, b) => a.median - b.median)[0];
  container = await render(liveFeed(now, m => ({
    ...m, out_in_ratio_median: 4.13, mixes: { chat_3_1: 0.987, avg_15_1: null, agent_150_1: 0.111 },
    models: m.models.map(x => ({ ...x, mix: { chat_3_1: x.id === cheap.id ? 99 : x.median * 2, avg_15_1: x.median, agent_150_1: x.median / 2 } })),
  })));
  const buttons = Array.from(container.querySelectorAll<HTMLButtonElement>(".cx-mix button"));
  expect(buttons.map(b => b.textContent)).toEqual(["Chat 3:1", "Average 15:1", "Agent 150:1"]);
  expect(buttons[1].getAttribute("aria-pressed")).toBe("true");
  expect(container.querySelector(".cx-price strong")?.textContent).toBe(usd(typicalPrice(market)!.median));
  expect(container.querySelector(".cx-mix-why")).toBeNull();
  expect(container.querySelector("#board .cx-sub")?.textContent).toBe("USD per 1M tokens");
  expect(container.querySelector(".board-cols .c-median")?.textContent).toBe("Median");
  await click(buttons[0]);
  expect(container.querySelector(".cx-price strong")?.textContent).toBe("$0.987");
  expect(container.querySelector(".board-cols .c-median")?.textContent).toBe("Median 3:1");
  const glm = Array.from(container.querySelectorAll(".board-row")).find(r => r.querySelector(".m-name")?.textContent === "GLM 5.3")!;
  expect(glm.querySelector(".c-median")?.textContent).toBe(`median ${usd(market.models.find(m => m.name === "GLM 5.3")!.median * 2)}`);
  await click(Array.from(container.querySelectorAll("#board .cx-sort button")).find(b => b.textContent === "Highest median")!);
  expect(container.querySelector(".board-row .m-name")?.textContent).toBe(cheap.name);
  // Strips stay on the 15:1 quotes.
  expect(container.querySelector(".board-row .strip")?.getAttribute("aria-label")).toContain(`median ${usd(cheap.median)}`);
  // Back to 15:1: no top-level avg value, so the median of the model medians.
  await click(buttons[1]);
  expect(container.querySelector(".cx-price strong")?.textContent).toBe(usd(typicalPrice(market)!.median));
});

test("$ per bn G_n column: formula, nulls as dashes, cheapest first when sorted", async () => {
  let container = await render(liveFeed());
  expect(container.querySelector(".c-gn")).toBeNull();
  expect(Array.from(container.querySelectorAll("#board .cx-sort button")).map(b => b.textContent)).not.toContain("Per unit of compute");
  await cleanup?.();
  container = await render(liveFeed(Date.now(), m => ({
    ...m, compute_price: { median_usd_per_bn_gn: 0.0021, p25: 0.0012, p75: 0.0045, n_models: 19 },
    models: m.models.map((x, i) => ({ ...x, usd_per_bn_gn: i === 0 ? null : 0.001 * (i + 1), compute: i === 1 ? { params: 27e9, gn_per_token: 54, source: "2 × parameters" } : i === 2 ? { params: 30e9, gn_per_token: 6.6, source: "2x_active_params_est", basis: "2x_active_params_est" } : null })),
  })));
  const head = container.querySelector(".board-cols .c-gn")!;
  expect(head.textContent).toBe("Per unit of compute$ per bn G_n (FLOP's unit)");
  expect(head.getAttribute("title")).toContain("Price per 1M tokens ÷ compute per token");
  expect(head.getAttribute("title")).toContain("FLOP's registry value where it lists the model, otherwise 2 × active parameters, taken from the model id or estimated from its expert counts (marked est.); — when unknown.");
  expect(container.querySelector(".cx-gn-note")).toBeNull();
  const rows = container.querySelectorAll(".board-row");
  expect(rows[0].querySelector(".c-gn b")?.textContent).toBe("—");
  expect(rows[0].querySelector(".gn-track")).toBeNull();
  expect(rows[1].querySelector(".c-gn b")?.textContent).toBe("$0.002");
  expect(rows[1].querySelector(".gn-dot")).not.toBeNull();
  expect(rows[1].querySelector(".gn-est")).toBeNull();
  expect(rows[2].querySelector(".c-gn b + .gn-est")?.textContent).toBe(" est.");
  await click(rows[1].querySelector(".board-head")!);
  expect(rows[1].querySelector(".panel-meta")?.textContent).toContain("54 G_n per token (2 × parameters)");
  await click(Array.from(container.querySelectorAll("#board .cx-sort button")).find(b => b.textContent === "Per unit of compute")!);
  const sorted = Array.from(container.querySelectorAll(".board-row .c-gn b"), b => b.textContent);
  expect(sorted[0]).toBe("$0.002");
  expect(sorted[sorted.length - 1]).toBe("—");
});

const stripRouted = (m: ComputeMarket): ComputeMarket => {
  const { routed: _r, ...rest } = m;
  return { ...rest, models: m.models.map(({ routed: _x, routed_usd_per_bn_gn: _y, ...x }) => x), flop: m.flop ? { ...m.flop, implied_flop_usd_routed: undefined, parity: undefined } : m.flop };
};
const panel = (container: HTMLElement, id: string) => container.querySelector(`[aria-labelledby="${id}"]`)!;

test("hero: labelled numbers for the benchmark model, each hidden when absent", async () => {
  const now = Date.now(), market = marketFixture(now);
  const bench = market.models.find(m => m.id === "qwen/qwen3.8-27b")!;
  let container = await render(liveFeed(now));
  const block = container.querySelector(".cx-bench")!;
  expect(block.querySelector(".cx-k")?.textContent).toBe("Qwen3.8-27B · per 1M tokens");
  expect(Array.from(block.querySelectorAll("dt"), d => d.textContent)).toEqual(["What a request pays now", "Perp mark"]);
  const dds = Array.from(block.querySelectorAll("dd"), d => d.querySelector("strong")?.textContent);
  expect(dds).toEqual([usd(bench.routed!.price), "$0.3036"]);
  expect(block.querySelector(".is-main dd")?.textContent).toContain("1 h ");
  expect(block.querySelector(".is-main svg.spark")).not.toBeNull();
  expect(block.textContent).not.toContain("List prices are what sellers post");
  expect(block.textContent).not.toContain("Median list price");
  const go = block.querySelector("a.cx-bench-go");
  expect(go?.getAttribute("href")).toBe("/trade");
  expect(go?.textContent).toContain("Trade COMPUTE-PERP");
  // The headline above it stays the cross-model median, under its own label.
  expect(container.querySelector(".cx-label")?.textContent).toBe("Median price to run an open model");
  await cleanup?.();
  container = await render(liveFeed(now, stripRouted));
  expect(Array.from(container.querySelectorAll(".cx-bench dt"), d => d.textContent)).toEqual(["Perp mark"]);
  expect(container.querySelector(".cx-bench .spark")).toBeNull();
  await cleanup?.();
  container = await render(liveFeed(now, m => ({ ...stripRouted(m), models: [] })));
  expect(container.querySelector(".cx-bench dt")?.textContent).toBe("Perp mark");
  await cleanup?.();
  const feed = liveFeed(now, m => m);
  vi.spyOn(feed, "latest").mockRejectedValue(new Error("down"));
  container = await render(feed);
  expect(Array.from(container.querySelectorAll(".cx-bench dt"), d => d.textContent)).toEqual(["What a request pays now"]);
  // A null 1 h change is not shown.
  await cleanup?.();
  container = await render(liveFeed(now, m => ({ ...m, models: m.models.map(x => x.routed ? { ...x, routed: { ...x.routed, change_1h_pct: null } } : x) })));
  expect(container.querySelector(".cx-bench .cx-bench-chg")).toBeNull();
});

test("board: Paid now column, sortable, with where the money goes and a capped mark", async () => {
  const now = Date.now(), market = marketFixture(now);
  const container = await render(liveFeed(now, m => ({ ...m, models: m.models.map((x, i) => i === 1 && x.routed ? { ...x, routed: { ...x.routed, capped: true } } : x) })));
  expect(container.querySelector(".board-cols .c-paid")?.textContent).toBe("Paid now");
  const rows = Array.from(container.querySelectorAll(".board-row"));
  const glm = rows.find(r => r.querySelector(".m-name")?.textContent === "GLM 5.3")!;
  expect(glm.querySelector(".c-paid")?.textContent).toBe(`paid now ${usd(market.models[0].routed!.price)}`);
  expect(glm.querySelector(".paid-cap")).toBeNull();
  const capped = rows.find(r => r.querySelector(".m-name")?.textContent === "DeepSeek V4")!;
  expect(capped.querySelector(".paid-cap")?.textContent).toBe("capped");
  expect(capped.querySelector(".paid-cap")?.getAttribute("title")).toBe("No seller counts for more than half.");
  await click(glm.querySelector(".board-head")!);
  const top = glm.querySelector(".paid-top")!;
  expect(top.querySelector("h3")?.textContent).toBe("Where the money goes");
  const first = market.models[0].routed!.top[0];
  const cells = Array.from(top.querySelectorAll("tbody tr")[0].children, c => c.textContent);
  expect(cells[0]).toContain(first.provider);
  expect(cells.slice(2)).toEqual([usd(first.price), `${(first.share * 100).toFixed(first.share < 0.1 ? 1 : 0)}%`]);
  expect(top.querySelectorAll("tbody tr")).toHaveLength(market.models[0].routed!.top.length);
  await click(Array.from(container.querySelectorAll("#board .cx-sort button")).find(b => b.textContent === "Paid now")!);
  const paid = Array.from(container.querySelectorAll(".board-row .c-paid"), el => Number(el.textContent!.replace(/[^\d.]/g, "")));
  expect(paid).toEqual([...paid].sort((a, b) => b - a));
});

test("board: no routed prices, no Paid now column or sort", async () => {
  const container = await render(liveFeed(Date.now(), stripRouted));
  expect(container.querySelector(".c-paid")).toBeNull();
  expect(Array.from(container.querySelectorAll("#board .cx-sort button"), b => b.textContent)).not.toContain("Paid now");
  expect(container.querySelector(".board.has-paid")).toBeNull();
  await click(container.querySelector(".board-head")!);
  expect(container.querySelector(".paid-top")).toBeNull();
});

test("FLOP parity: headline is the routed implied price, list price beside it, arithmetic from the feed", async () => {
  const now = Date.now(), market = marketFixture(now);
  const container = await render(liveFeed(now));
  const flop = panel(container, "compute-flop-heading");
  const p = market.flop!.parity!;
  expect(flop.querySelector("h2")?.textContent).toBe("FLOP parity");
  expect(flop.querySelector(".flop-parity .perp-price strong")?.textContent).toBe(usdSmall(p.implied_flop_usd_routed! * 1e6));
  expect(flop.querySelector(".flop-parity .perp-price strong")?.textContent).toMatch(/^\$\d+\.\d+$/);
  expect(flop.querySelector(".flop-parity .perp-price span")?.textContent).toBe("per 1M FLOP, from what requests pay");
  expect(flop.querySelector(".flop-list")?.textContent).toBe(`${usdSmall(p.implied_flop_usd! * 1e6)} per 1M FLOP, from list prices`);
  expect(flop.querySelector(".flop-parity .perp-why")?.textContent).toBe("The FLOP price at which FLOP's rate equals what the open market pays for the same compute.");
  const facts = Array.from(flop.querySelectorAll(".flop-parity .flop-facts > div"), d => [d.querySelector("dt")?.textContent, d.querySelector("dd")?.textContent]);
  expect(facts).toEqual([["Market price of compute", usdSmall(p.market_usd_per_bn_gn)], ["FLOP's rate", "0.00291"]]);
  expect(flop.querySelector(".flop-parity .flop-facts > div small")?.textContent).toBe("$ per bn G_n, Qwen3.8-27B, from what requests pay");
  // implied FLOP price = USD per bn G_n / 1e9 / FLOP per G_n.
  expect(p.implied_flop_usd_routed!).toBeCloseTo(p.market_usd_per_bn_gn! / 1e9 / p.flop_rate_flop_per_gn!, 6);
  expect(flop.querySelector(".flop-window")?.textContent).toBe("Last 6 hours");
});

test("FLOP panel without parity falls back to the implied price; without flop it is hidden", async () => {
  const now = Date.now();
  const basis = marketFixture(now).models.find(m => m.name === "Qwen3.8-27B")!;
  let container = await render(liveFeed(now, m => ({ ...m, flop: { gn_price_flop: 0.0029, implied_flop_usd: 0.0123, basis_model: basis.id } })));
  let flop = panel(container, "compute-flop-heading");
  expect(flop.querySelector(".flop-parity")).toBeNull();
  const implied = flop.querySelector(".flop-implied")!;
  expect(implied.querySelector(".cx-k")?.textContent).toBe("Implied FLOP price · derived");
  expect(implied.querySelector("strong")?.textContent).toBe("$12,300.00");
  expect(implied.querySelector(".perp-price span")?.textContent).toBe("per 1M FLOP");
  expect(implied.querySelector(".perp-why")?.textContent).toBe("The FLOP price at which FLOP's rate equals what the open market pays for the same compute on Qwen3.8-27B.");
  await cleanup?.();
  container = await render(liveFeed(now, m => ({ ...m, flop: { gn_price_flop: null, implied_flop_usd: null, basis_model: null } })));
  flop = panel(container, "compute-flop-heading");
  expect(flop.querySelector(".flop-implied")).toBeNull();
  expect(flop.querySelector(".flop-parity")).toBeNull();
  await cleanup?.();
  container = await render(liveFeed(now, stripRouted));
  flop = panel(container, "compute-flop-heading");
  expect(flop.querySelector(".flop-parity")).toBeNull();
  expect(flop.querySelector(".flop-implied strong")?.textContent).toBe(usdSmall(marketFixture(now).flop!.implied_flop_usd! * 1e6));
});

test("FLOP parity: the market price of compute sits beside the list-price figure the board shows", async () => {
  const now = Date.now();
  const container = await render(liveFeed(now, m => ({ ...m, models: m.models.map(x => x.id === "qwen/qwen3.8-27b" ? { ...x, usd_per_bn_gn: 5.71 } : x) })));
  const flop = panel(container, "compute-flop-heading");
  const facts = Array.from(flop.querySelectorAll(".flop-parity .flop-facts > div"), d => [d.querySelector("dt")?.textContent, d.querySelector("dd")?.textContent, d.querySelector("small")?.textContent]);
  expect(facts[1]).toEqual(["At list prices", "$5.71", "$ per bn G_n, median list price, as on the board"]);
});

test("the perp panel counts the sellers it lists and drops the weighting sentence", async () => {
  const container = await render(liveFeed());
  const perp = panel(container, "compute-perp-heading");
  expect(perp.querySelector(".perp-why")).toBeNull();
  expect(perp.textContent).toContain("2 of 2 full-precision sellers up");
  expect(perp.querySelectorAll(".perp-sellers li")).toHaveLength(2);
});

test("one quote count on /compute: quotes up from the index", async () => {
  const market = marketFixture();
  const container = await render(liveFeed());
  expect(container.querySelector(".cx-proof b")?.textContent).toBe(String(market.index.n_quotes_up));
  expect(container.querySelector(".cx-proof li")?.textContent).toBe(`${count(market.index.n_quotes_up)} live quotes`);
});

test("the FLOP window label reads the feed's window, else six hours", () => {
  expect(windowLabel(site.flopWindowS)).toBe("Last 6 hours");
  expect(windowLabel(3600)).toBe("Last 1 hour");
  expect(windowLabel(1800)).toBe("Last 30 minutes");
});

test("FLOP parity without the routed figure labels everything as list-based", async () => {
  const now = Date.now();
  const container = await render(liveFeed(now, m => ({ ...m,
    models: m.models.map(x => x.id === "qwen/qwen3.8-27b" ? { ...x, usd_per_bn_gn: 5.71 } : x),
    flop: m.flop ? { ...m.flop, implied_flop_usd_routed: undefined, parity: m.flop.parity ? { ...m.flop.parity, implied_flop_usd_routed: null } : m.flop.parity } : m.flop })));
  const flop = panel(container, "compute-flop-heading");
  expect(flop.querySelector(".flop-parity .perp-price span")?.textContent).toBe("per 1M FLOP, from list prices");
  expect(flop.textContent).not.toContain("from what requests pay");
  expect(Array.from(flop.querySelectorAll(".flop-parity .flop-facts dt"), d => d.textContent)).not.toContain("At list prices");
  expect(flop.querySelector(".flop-parity .flop-facts small")?.textContent).toMatch(/, from list prices$/);
});

test("widestModels orders by dearest/cheapest, skips thin and stale models, and caps at six", async () => {
  const market = marketFixture(Date.now());
  const base = market.models[0];
  const mk = (id: string, min: number, max: number, extra: object = {}) => ({ ...base, id, name: id, min, max, median: (min + max) / 2, thin: false, stale: false, ...extra });
  const models = [mk("a", 1, 2), mk("b", 1, 9), mk("c", 1, 5), mk("thin", 1, 100, { thin: true }), mk("stale", 1, 90, { stale: true }), mk("flat", 1, 1), ...["d", "e", "f", "g"].map((id, i) => mk(id, 1, 3 + i / 10))];
  expect(widestModels({ ...market, models }).map(m => m.name)).toEqual(["b", "c", "g", "f", "e", "d"]);
});
