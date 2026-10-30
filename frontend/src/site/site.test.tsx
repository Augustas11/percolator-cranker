import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import Site from "./Site";
import { site } from "./config";
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
  [site.paths.home, "drip", "How it works", "drip"],
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
  expect(container.querySelector("footer")?.textContent).toContain("Devnet only. Test tokens have no value. Software provided as-is.");
});

test("home renders mock metrics and optional fills honestly", async () => {
  const container = await render(site.paths.home);
  expect(Array.from(container.querySelectorAll(".metric-grid dd")).map(dd => dd.textContent)).toEqual(["200", "—", "2.5 SOL", "99.5%"]);
  expect(container.textContent).toContain("Devnet, test tokens. (mock data)");
  expect(container.textContent).toContain(`As of ${new Date(1793232000 * 1000).toLocaleString()}`);
});

test("home handles an unconfigured indexer", async () => {
  const container = await render(site.paths.home, null);
  expect(container.textContent).toContain("Indexer not configured");
});

test("home handles an unavailable indexer", async () => {
  const indexer = new MockIndexerClient();
  vi.spyOn(indexer, "stats").mockRejectedValue(new Error("offline"));
  const container = await render(site.paths.home, indexer);
  expect(container.textContent).toContain("Market data unavailable");
});

test("home refreshes every 30 seconds and stops on unmount", async () => {
  vi.useFakeTimers();
  const indexer = new MockIndexerClient();
  const stats = await indexer.stats();
  const fetch = vi.spyOn(indexer, "stats").mockResolvedValue({ ...stats, fills: 12, as_of: null, uptime: { uptime_pct: null } });
  const container = await render(site.paths.home, indexer);
  expect(container.textContent).toContain("No data yet");
  expect(Array.from(container.querySelectorAll(".metric-grid dd")).map(dd => dd.textContent)).toEqual(["200", "12", "2.5 SOL", "—"]);
  await act(async () => { await vi.advanceTimersByTimeAsync(30000); });
  expect(fetch).toHaveBeenCalledTimes(2);
  await cleanup!();
  cleanup = undefined;
  await vi.advanceTimersByTimeAsync(30000);
  expect(fetch).toHaveBeenCalledTimes(2);
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

test("arena shows the placeholder when no leaderboard file exists", async () => {
  if (Object.keys(import.meta.glob("../arena/Leaderboard.tsx")).length) return;
  const container = await render(site.paths.arena);
  expect(container.textContent).toContain("Leaderboard coming soon.");
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
});

test("unknown paths offer a link home", async () => {
  const container = await render("/missing");
  expect(container.querySelector("h1")?.textContent).toBe("Page not found");
  expect(container.querySelector("main a")?.getAttribute("href")).toBe(site.paths.home);
});
