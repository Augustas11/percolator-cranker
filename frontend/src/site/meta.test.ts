// @vitest-environment node
import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { shell, shellFile, shells } from "../../scripts/route-shells";
import routes from "./routes.json";

const html = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
const vercel = JSON.parse(readFileSync(new URL("../../vercel.json", import.meta.url), "utf8")) as { rewrites: { source: string; destination: string }[] };

test("each route gets a static shell with its own title, description and og:url; gated routes keep the home shell", () => {
  const out = shells(html, routes);
  expect(out.map(s => s.file)).toEqual(["compute.html", "trade.html", "arena.html", "agents.html", "docs.html"]);
  const trade = out.find(s => s.file === "trade.html")!.html;
  expect(trade).toContain("<title>Trade COMPUTE-PERP · Drip</title>");
  expect(trade).toContain(`<meta property="og:title" content="Trade COMPUTE-PERP · Drip" />`);
  expect(trade).toContain(`<meta name="twitter:description" content="${routes.routes["/trade"].description}" />`);
  expect(trade).toContain(`<meta property="og:url" content="https://drip-devnet.vercel.app/trade" />`);
  expect(trade).not.toContain(routes.home.title);
  expect(html).toContain(`<meta property="og:url" content="https://drip-devnet.vercel.app/" />`);
  expect(() => shell("<title>x</title>", routes.home, "u")).toThrow(/no tag matching/);
});

test("vercel serves each route's shell before the app catch-all, and the aliases share the agents shell", () => {
  const sources = vercel.rewrites.map(r => r.source);
  const catchAll = sources.findIndex(s => s.startsWith("/((?!"));
  for (const [path, m] of Object.entries(routes.routes)) {
    const at = sources.indexOf(path);
    if ((m as { gated?: boolean }).gated) { expect(at).toBe(-1); continue; }
    expect(at).toBeGreaterThan(-1);
    expect(at).toBeLessThan(catchAll);
    expect(vercel.rewrites[at].destination).toBe(`/${shellFile(path)}`);
  }
  expect(vercel.rewrites.filter(r => r.source === "/tools" || r.source === "/build").map(r => r.destination)).toEqual(["/agents.html", "/agents.html"]);
});
