import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import Compute from "../pages/Compute";
import type { ComputeFeedClient } from "./client";
import { MockComputeFeedClient } from "./mock";
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

test("renders the price, chart, sellers, FLOP side and how-to from the mock feed", async () => {
  const container = await render(new MockComputeFeedClient());
  const text = container.textContent!;
  expect(container.querySelector("h1")?.textContent).toBe("Compute price");
  expect(text).toContain("Independent of Flop Labs");
  expect(text).toContain("sample data");
  expect(container.querySelector(".compute-price strong")?.textContent).toBe("$0.3036");
  expect(text).toContain("per 1M tokens");
  expect(text).toContain("Updated 3 min ago");
  expect(container.querySelector(".compute-stale")).toBeNull();

  const chart = container.querySelector('svg[role="img"]')!;
  expect(chart.getAttribute("aria-label")).toContain("from $0.3112 to $0.3036");
  expect(chart.querySelectorAll(".chart-outage")).toHaveLength(1);
  expect(text).toContain("GMICloud (bf16) was down since");

  const sellers = container.querySelector('[aria-labelledby="compute-sellers-heading"]')!;
  const rows = Array.from(sellers.querySelectorAll(":scope > .table-scroll tbody tr")).map(r => Array.from(r.querySelectorAll("td")).map(td => td.textContent));
  expect(rows).toEqual([
    ["DeepInfra", "bf16", "$0.2578", "$0.2578", "up"],
    ["Cerebras", "fp16", "$1.0213", "$1.0213", "up"],
    ["GMICloud", "bf16", "$0.6219", "not used", "down"],
  ]);
  const others = sellers.querySelector("details")!;
  expect(others.open).toBe(false);
  expect(others.querySelector("summary")?.textContent).toBe("All other sellers (1)");
  expect(others.textContent).toContain("Darkbloom");

  const flop = container.querySelector('[aria-labelledby="compute-flop-heading"]')!;
  expect(Array.from(flop.querySelectorAll(".fact-value")).map(v => v.textContent)).toEqual(["0.00291", "3", "34", "50%"]);
  expect(flop.textContent).toContain("shown for comparison only");

  const how = container.querySelector('[aria-labelledby="compute-how-heading"]')!;
  expect(how.querySelector('a[href="https://explorer.solana.com/tx/mock-push-signature?cluster=devnet"]')).not.toBeNull();
  expect(how.textContent).toContain("staging verify-push-log");
  expect(how.textContent).toContain("Now: off (n<3).");
  expect(how.textContent).toContain("Price changes in the last 7 days (1)");
  expect(how.querySelector('a[href="https://explorer.solana.com/tx/mock-change-signature?cluster=devnet"]')).not.toBeNull();
  expect(how.textContent).not.toContain("Source code");
  expect(site.computeRepo).toBe("");
});

test("shows a stale badge when the latest price is over 30 minutes old", async () => {
  const container = await render(new MockComputeFeedClient(Date.now(), 45 * 60_000));
  expect(container.querySelector(".compute-stale")?.textContent).toBe("stale");
  expect(container.textContent).toContain("No new price for over 30 minutes.");
  expect(container.textContent).toContain("Updated 45 min ago");
});

test("shows loading, then unreachable when the feed fails", async () => {
  const feed = new MockComputeFeedClient();
  let fail!: (e: Error) => void;
  vi.spyOn(feed, "latest").mockReturnValue(new Promise((_, reject) => { fail = reject; }));
  const container = await render(feed);
  expect(container.textContent).toContain("Loading the price feed…");
  await act(async () => { fail(new Error("price feed 502")); });
  expect(container.querySelector('[role="status"]')?.textContent).toBe("Price feed unreachable (price feed 502). Try again in a minute.");
  expect(container.querySelector(".compute-price")).toBeNull();
});

test("shows the repo link only when configured", async () => {
  const repo = site.computeRepo;
  site.computeRepo = "https://example.invalid/repo";
  try {
    const container = await render(new MockComputeFeedClient());
    expect(container.querySelector('a[href="https://example.invalid/repo"]')).not.toBeNull();
  } finally { site.computeRepo = repo; }
});
