import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import Site from "../site/Site";
import { site } from "../site/config";
import { MockMarketClient } from "../market/mock";
import { MockIndexerClient } from "../indexer/mock";
import { MockComputeFeedClient } from "../compute/mock";
import { MOCK_BOARD, mockBoard } from "../challenge/board";
import type { BoardLoader } from "../challenge/board";
import { countdown, nextClose } from "./Challenge";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const original = { ...site.challenge };
let cleanup: (() => Promise<void>) | undefined;
afterEach(async () => {
  await cleanup?.();
  cleanup = undefined;
  Object.assign(site.challenge, original);
  vi.restoreAllMocks();
});

async function render(path: string, board: BoardLoader | null = mockBoard) {
  window.history.pushState({}, "", path);
  const client = new MockMarketClient({ autoTick: false });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  cleanup = async () => { await act(async () => { root.unmount(); }); client.dispose(); container.remove(); };
  await act(async () => {
    root.render(<Site client={client} indexer={new MockIndexerClient()} computeFeed={new MockComputeFeedClient()} challengeBoard={board} />);
  });
  return container;
}

const FULL = {
  enabled: true, opensAt: "2026-12-01T00:00:00Z", firstCloseAt: "2026-12-06T00:00:00Z", practiceCmd: "flop-score --rule target/rule.wasm",
  repoPath: "https://github.com/example/rule-template", apiBase: "https://api.example.invalid/challenge", prizes: "1,000 USDC to the top three",
};

test("flag off: no nav item, no footer link, no route, and the swarm paths do not redirect", async () => {
  Object.assign(site.challenge, { enabled: false });
  for (const path of ["/challenge", "/swarm", "/swarm-markets"]) {
    const container = await render(path);
    expect(window.location.pathname).toBe(path);
    expect(container.querySelector("h1")?.textContent).toBe("Page not found");
    expect(container.querySelector('a[href="/challenge"]')).toBeNull();
    expect(container.textContent).not.toMatch(/challenge/i);
    await cleanup?.(); cleanup = undefined;
  }
  const docs = await render("/docs");
  expect(docs.querySelector("#challenge")).toBeNull();
});

test("flag on with every value: nav, footer, the full page and the rules in Docs", async () => {
  Object.assign(site.challenge, FULL);
  vi.spyOn(Date, "now").mockReturnValue(Date.parse("2026-12-08T12:00:00Z"));
  const container = await render("/challenge");
  expect(Array.from(container.querySelectorAll(".nav-links a"), a => a.textContent)).toEqual(["Index", "Arena", "Challenge", "Agents", "Docs"]);
  expect(container.querySelector('footer a[href="/challenge"]')?.textContent).toBe("Challenge");
  expect(document.title).toBe("Compute Oracle Challenge · Drip");
  expect(container.querySelector("h1")?.textContent).toBe("Build the compute price oracle.");
  expect(Array.from(container.querySelectorAll("main h2"), h => h.textContent)).toEqual(["Why an oracle", "What you submit", "How it is scored", "What you win", "Agents and swarms welcome", "Score a rule locally", "Leaderboard"]);
  // Live numbers from market.json: the benchmark model's sellers up and its spread.
  const bench = (await new MockComputeFeedClient().market()).models.find(m => m.id === "qwen/qwen3.8-27b")!;
  expect(container.querySelector(".ch-pitch")?.textContent).toMatch(new RegExp(`^${bench.sellers_up} sellers list the same AI model at prices up to [\\d.]+× apart\\. Your code turns their quotes into one price\\.$`));
  expect(container.querySelector(".ch-score-line")?.textContent).toBe("Follow real price changes, ignore fake ones. Tested on next week's market and attacked with fake listings.");
  expect(container.querySelector('[aria-labelledby="ch-win"] p')?.textContent).toBe("The best oracle runs the live market after an independent review, with your name on it.");
  expect(Array.from(container.querySelectorAll('[aria-labelledby="ch-why"] li'), l => l.textContent)).toEqual([
    "On-chain markets settle on whatever their oracle reports.",
    "An oracle that can be faked or that lags gets traders liquidated.",
    "Nobody has built a tested one for the price of AI compute.",
  ]);
  expect(container.querySelector("main")?.textContent).not.toMatch(/Price Challenge|settlement rule|sets the settlement price|!/);
  expect(Array.from(container.querySelectorAll(".ch-dates dt"), d => d.textContent)).toEqual(["Opens", "First close", "Next close in"]);
  expect(container.querySelector(".ch-next dd")?.textContent).toBe("4d 12:00:00");
  expect(container.querySelector(".ch-prizes")?.textContent).toBe("Prizes: 1,000 USDC to the top three");
  expect(container.textContent).toContain("challenge_submit");
  expect(container.querySelector('[aria-labelledby="ch-try"] pre')?.textContent).toBe(FULL.practiceCmd);
  expect(Array.from(container.querySelectorAll(".ch-hero a"), a => [a.textContent?.trim(), a.getAttribute("href")])).toEqual([["Read the rules", "/docs#challenge"], ["Get the template", FULL.repoPath]]);
  expect(container.textContent).not.toMatch(/TBD|coming soon|Paradigm|imd\.fun/i);
  // Season board and round board, reference entries from the data.
  expect(Array.from(container.querySelectorAll(".ch-board-block h3"), h => h.textContent)).toEqual(["Season", "Round 2026-W49"]);
  const round = container.querySelectorAll(".ch-board")[1];
  const rows = Array.from(round.querySelectorAll("tbody tr"));
  expect(rows).toHaveLength(MOCK_BOARD.entries.length);
  expect(rows[0].querySelector(".ch-handle")?.textContent).toBe("tessellate");
  expect(rows[0].querySelector(".num")?.textContent).toBe("+0.214 ± 0.041");
  expect(rows[0].querySelector(".ch-beats")?.textContent).toBe("yes");
  expect(rows[0].querySelector(".ch-terms")?.textContent).toContain("spoof 0.004");
  expect(Array.from(round.querySelectorAll("tr.is-ref .ch-handle"), h => h.textContent)).toEqual(["median of all sellers", "live rule"]);
  await cleanup?.(); cleanup = undefined;
  const docs = await render("/docs");
  expect(docs.querySelector("#challenge h2")?.textContent).toBe("Compute Oracle Challenge rules");
  expect(docs.querySelector("#challenge p")?.textContent).toBe("An oracle is the code that reports a price to a market; this one reports the price COMPUTE-PERP settles on.");
  expect(docs.querySelector("#challenge")?.textContent).toContain("Prizes: 1,000 USDC to the top three.");
});

test("flag on with partial config renders only the configured blocks", async () => {
  Object.assign(site.challenge, { enabled: true, opensAt: null, firstCloseAt: null, practiceCmd: null, repoPath: null, apiBase: null, prizes: null, entryRepo: null });
  const container = await render("/challenge");
  expect(container.querySelector("h1")?.textContent).toBe("Build the compute price oracle.");
  expect(container.querySelector(".ch-dates")).toBeNull();
  expect(container.querySelector(".ch-prizes")).toBeNull();
  expect(container.querySelector("#ch-board")).toBeNull();
  expect(container.querySelector('[aria-labelledby="ch-try"]')).toBeNull();
  expect(container.querySelector(".ch-enter")).toBeNull();
  expect(container.textContent).not.toContain("challenge_submit");
  expect(container.textContent).not.toContain("Get the template");
  expect(container.textContent).not.toMatch(/TBD|coming soon|Prizes/i);
  await cleanup?.(); cleanup = undefined;
  Object.assign(site.challenge, { firstCloseAt: "2026-12-06T00:00:00Z" });
  const c2 = await render("/challenge");
  expect(Array.from(c2.querySelectorAll(".ch-dates dt"), d => d.textContent)).toEqual(["First close", "Next close in"]);
});

test("the shipped default config: dates, pull-request entry, template link and practice command, no board", async () => {
  const container = await render("/challenge");
  expect(container.querySelector("h1")?.textContent).toBe("Build the compute price oracle.");
  expect(Array.from(container.querySelectorAll("a"), a => a.textContent?.trim()).filter(t => t === "Get the template").length).toBeGreaterThan(0);
  for (const a of Array.from(container.querySelectorAll("a")).filter(a => a.textContent?.trim() === "Get the template")) expect(a.getAttribute("href")).toBe(site.challenge.repoPath);
  const local = container.querySelector('[aria-labelledby="ch-try"]')!;
  expect(local.querySelector("h2")?.textContent).toBe("Score a rule locally");
  expect(local.querySelector("pre")?.textContent).toBe(site.challenge.practiceCmd);
  expect(Array.from(container.querySelectorAll(".ch-dates dt"), d => d.textContent).slice(0, 2)).toEqual(["Opens", "First close"]);
  expect(new Date(site.challenge.firstCloseAt!).getTime() - new Date(site.challenge.opensAt!).getTime()).toBe(7 * 86_400_000);
  expect(container.querySelector(".ch-enter a")?.getAttribute("href")).toBe(`${site.challenge.entryRepo}/pulls`);
  expect(container.querySelector("#ch-board")).toBeNull();
});

test("the swarm paths redirect to the challenge when it is on", async () => {
  Object.assign(site.challenge, { enabled: true });
  for (const path of ["/swarm", "/swarm-markets"]) {
    const container = await render(path);
    expect(window.location.pathname).toBe("/challenge");
    expect(container.querySelector("h1")?.textContent).toBe("Build the compute price oracle.");
    await cleanup?.(); cleanup = undefined;
  }
});

test("board: empty and unreachable states", async () => {
  Object.assign(site.challenge, { enabled: true, apiBase: "https://api.example.invalid", firstCloseAt: "2026-12-06T00:00:00Z" });
  let container = await render("/challenge", async () => ({ round: { id: "2026-W49", closed_at: "2026-12-06T00:00:00Z", scored_at: null }, entries: [] }));
  expect(container.querySelector('[aria-labelledby="ch-board"]')?.textContent).toBe("LeaderboardNo scored round yet. First close: 6 Dec, 00:00 UTC.");
  await cleanup?.(); cleanup = undefined;
  container = await render("/challenge", async () => { throw new Error("HTTP 502"); });
  expect(container.querySelector('[aria-labelledby="ch-board"] [role="status"]')?.textContent).toBe("The board is unreachable right now (HTTP 502). Try again in a minute.");
});

test("board: a malformed entry shows the unreachable note, not a blank page", async () => {
  Object.assign(site.challenge, { enabled: true, apiBase: "https://api.example.invalid" });
  const { terms: _t, se: _s, module_hash: _m, ...broken } = MOCK_BOARD.entries[0];
  for (const bad of [
    { ...MOCK_BOARD, entries: [broken] },
    { ...MOCK_BOARD, season: { entries: [{ ...MOCK_BOARD.entries[0], terms: { ...MOCK_BOARD.entries[0].terms, jump: "x" } }] } },
  ]) {
    const container = await render("/challenge", async () => bad as never);
    expect(container.querySelector("h1")?.textContent).toBe("Build the compute price oracle.");
    expect(container.querySelector('[aria-labelledby="ch-board"] [role="status"]')?.textContent).toBe("The board is unreachable right now (board.json: unexpected shape). Try again in a minute.");
    expect(container.querySelector(".ch-board")).toBeNull();
    await cleanup?.(); cleanup = undefined;
  }
});

test("httpBoard validates entries and times out after 10 s", async () => {
  const { httpBoard, isBoard } = await import("../challenge/board");
  expect(isBoard(MOCK_BOARD)).toBe(true);
  const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ ...MOCK_BOARD, entries: [{ rank: 1 }] })));
  const timeout = vi.spyOn(AbortSignal, "timeout");
  await expect(httpBoard("https://api.example.invalid/")()).rejects.toThrow("board.json: unexpected shape");
  expect(timeout).toHaveBeenCalledWith(10_000);
  expect(fetchSpy.mock.calls[0][0]).toBe("https://api.example.invalid/board.json");
  expect((fetchSpy.mock.calls[0][1] as RequestInit).signal).toBe(timeout.mock.results[0].value);
});

test("next close and countdown", () => {
  const first = Date.parse("2026-12-06T00:00:00Z");
  expect(nextClose(first, first - 1000)).toBe(first);
  expect(nextClose(first, first + 1)).toBe(first + 7 * 86_400_000);
  expect(countdown(90_061_000)).toBe("1d 01:01:01");
  expect(countdown(-5)).toBe("00:00:00");
});
