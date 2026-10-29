import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test } from "vitest";
import App from "./App";
import { MockMarketClient } from "./market/mock";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

test("connects, deposits, opens a long, and keeps rendering after a tick", async () => {
  const client = new MockMarketClient({ autoTick: false, seed: 1 });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);

  async function click(text: string) {
    const button = Array.from(container.querySelectorAll("button")).find(b => b.textContent === text);
    expect(button).toBeDefined();
    await act(async () => { button!.click(); });
  }

  async function input(id: string, value: string) {
    const element = container.querySelector<HTMLInputElement>(`#${id}`)!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(element, value);
      element.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }

  try {
    await act(async () => { root.render(<App client={client} />); });
    expect(container.textContent).toContain("PoC demo on devnet. Test tokens only, no real funds.");
    expect(container.textContent).toContain("SOL-PERP");
    expect(container.textContent).toContain("No open position");

    await click("Connect wallet");
    await input("amount", "1000");
    await click("Deposit");
    expect(client.getSnapshot().portfolio.collateral).toBe(1000);
    await click("Long");
    await input("size", "1");
    await input("leverage", "5");
    await click("Open");

    const position = container.querySelector('[aria-labelledby="position-heading"]')!;
    expect(position.textContent).toContain("long");
    expect(position.textContent).toContain("1 SOL");
    expect(container.querySelector("tbody")!.textContent).toContain("you");
    expect(client.getSnapshot().portfolio.health.initialMarginUsed).toBe(30);
    const markBefore = container.querySelector('[aria-labelledby="market-heading"] dd')!.textContent;
    await act(async () => { client.tick(); });
    expect(container.querySelector('[aria-labelledby="market-heading"] dd')!.textContent).not.toBe(markBefore);
    expect(position.textContent).toContain("long");
    expect(position.textContent).toContain("1 SOL");
    expect(container.querySelector(".error")!.textContent).toBe("");
  } finally {
    await act(async () => { root.unmount(); });
    client.dispose();
    container.remove();
  }
});

test("shows unknown prices honestly and disables trading while the matcher is pending", async () => {
  const client = Object.assign(new MockMarketClient({ autoTick: false }), { tradingReady: false });
  await client.connect();
  await client.deposit(1000);
  await client.openPosition("long", 1, 5);
  const snapshot = client.getSnapshot();
  const unknown = { ...snapshot, portfolio: { ...snapshot.portfolio,
    legs: snapshot.portfolio.legs.map(leg => ({ ...leg, entryPrice: NaN, unrealizedPnl: NaN })),
    health: { equity: NaN, initialMarginUsed: NaN, maintenanceMargin: NaN, freeCollateral: NaN, ratio: NaN, status: "unknown" as const },
  } };
  client.getSnapshot = () => unknown;
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => { root.render(<App client={client} />); });
    expect(container.textContent).toContain("Trading: matcher pending");
    const health = container.querySelector('[aria-labelledby="health-heading"]')!;
    expect(Array.from(health.querySelectorAll("dd")).map(dd => dd.textContent)).toEqual(["$1,000.00", "—", "—", "—", "—", "—", "unknown"]);
    expect(health.querySelector(".badge.unknown")).not.toBeNull();
    expect(container.querySelector('[aria-labelledby="position-heading"]')!.textContent).toContain("Entry price—Unrealized PnL—");
    for (const label of ["Open", "Close position"]) {
      expect(Array.from(container.querySelectorAll("button")).find(button => button.textContent === label)!.disabled).toBe(true);
    }
  } finally {
    await act(async () => { root.unmount(); });
    client.dispose();
  }
});

test("renders infinite health values as unavailable", async () => {
  const client = new MockMarketClient({ autoTick: false });
  const snapshot = client.getSnapshot();
  const unknown = { ...snapshot, portfolio: { ...snapshot.portfolio, collateral: Infinity,
    health: { equity: Infinity, initialMarginUsed: -Infinity, maintenanceMargin: Infinity, freeCollateral: -Infinity, ratio: Infinity, status: "healthy" as const },
  } };
  client.getSnapshot = () => unknown;
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => { root.render(<App client={client} />); });
    const health = container.querySelector('[aria-labelledby="health-heading"]')!;
    expect(Array.from(health.querySelectorAll("dd")).map(dd => dd.textContent)).toEqual(["—", "—", "—", "—", "—", "—", "healthy"]);
  } finally {
    await act(async () => { root.unmount(); });
    client.dispose();
  }
});
