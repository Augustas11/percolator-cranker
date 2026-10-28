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
