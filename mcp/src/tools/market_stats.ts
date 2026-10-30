import { units } from "../drip";
import { tool } from "./tool";

export default tool({
  name: "market_stats",
  description: "Read market stats, mark price and matcher state (devnet, test tokens).",
  run: async (drip) => {
    return { ...await drip.fetchJson<object>("/indexer/stats"), mark_price: units((await drip.market()).asset0.effectivePrice), matcher: (await drip.matcher()).output };
  },
});
