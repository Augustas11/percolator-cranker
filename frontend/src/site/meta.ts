import { site } from "./config";

export interface PageMeta { title: string; description: string }

const HOME: PageMeta = {
  title: "Drip · the price of AI compute",
  description: "What open-model inference costs right now, from every seller, and COMPUTE-PERP, a perpetual in USD per 1M tokens on Solana devnet.",
};

/** Title and description per route. Unknown paths get the 404 entry. */
export const pageMeta: Record<string, PageMeta> = {
  [site.paths.home]: HOME,
  [site.paths.compute]: { title: "Compute index · Drip", description: "Every seller's price for every major open model, per 1M tokens: cheapest, median and spread, read every 5 minutes." },
  [site.paths.trade]: { title: "Trade COMPUTE-PERP · Drip", description: "Long or short the price of 1M tokens of Qwen3.8-27B on Solana devnet with test USDC." },
  [site.paths.arena]: { title: "Arena · Drip", description: "Every wallet trading COMPUTE-PERP, ranked by realized plus open PnL from on-chain fills." },
  [site.paths.agents]: { title: "Agents · Drip", description: "An MCP server that lets your agent read inference prices and hedge its compute bill with COMPUTE-PERP." },
  [site.paths.tools]: { title: "Agents · Drip", description: "An MCP server that lets your agent read inference prices and hedge its compute bill with COMPUTE-PERP." },
  [site.paths.build]: { title: "Agents · Drip", description: "An MCP server that lets your agent read inference prices and hedge its compute bill with COMPUTE-PERP." },
  [site.paths.docs]: { title: "Docs · Drip", description: "How the compute index and the COMPUTE-PERP settlement price are computed, and how to check them." },
  [site.paths.challenge]: { title: "Compute Price Challenge · Drip", description: "Write the rule that turns every seller's quote into one price. The best rule sets the price COMPUTE-PERP settles on." },
};
export const notFoundMeta: PageMeta = { title: "Page not found · Drip", description: HOME.description };

function setMeta(attr: "name" | "property", key: string, content: string) {
  let el = document.head.querySelector<HTMLMetaElement>(`meta[${attr}="${key}"]`);
  if (!el) {
    el = document.createElement("meta");
    el.setAttribute(attr, key);
    document.head.appendChild(el);
  }
  el.content = content;
}

/** Write the page's title, description, Open Graph and Twitter tags. */
export function applyMeta({ title, description }: PageMeta) {
  document.title = title;
  setMeta("name", "description", description);
  setMeta("property", "og:title", title);
  setMeta("property", "og:description", description);
  setMeta("name", "twitter:title", title);
  setMeta("name", "twitter:description", description);
}
