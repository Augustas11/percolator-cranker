import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test } from "vitest";
import PriceChart from "./PriceChart";
import type { Point } from "../compute/client";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

async function draw(mark: Point[], from: number, to: number, index: Point[] | null = null) {
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () => { root.render(<PriceChart mark={mark} index={index} from={from} to={to} />); });
  const out = { text: container.textContent, paths: Array.from(container.querySelectorAll("path"), p => p.getAttribute("d") ?? "") };
  await act(async () => { root.unmount(); });
  return out;
}

test("an empty or reversed window draws nothing instead of dividing by zero", async () => {
  const pts: Point[] = [[0, 0.3], [1000, 0.31]];
  for (const [from, to] of [[500, 500], [1000, 0], [NaN, 1000]]) {
    const out = await draw(pts, from, to);
    expect(out.text).toBe("No mark pushed in this range.");
    expect(out.paths).toEqual([]);
  }
});

test("non-finite points are dropped before drawing", async () => {
  const out = await draw([[0, 0.3], [500, NaN], [NaN, 0.5], [600, Infinity], [1000, 0.31]] as Point[], 0, 1000, [[0, 0.3], [800, NaN]] as Point[]);
  expect(out.paths.length).toBeGreaterThan(0);
  for (const d of out.paths) expect(d).not.toMatch(/NaN|Infinity/);
});
