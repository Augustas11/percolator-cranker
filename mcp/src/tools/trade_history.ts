import { z } from "zod";
import { tool } from "./tool";

export default tool({
  name: "trade_history",
  description: "Read your portfolio's recent fills (devnet, test tokens).",
  inputSchema: { limit: z.number().int().min(1).max(100).default(20) },
  run: async (drip, { limit }) => {
    const pf = await drip.portfolio();
    return pf ? drip.fetchJson<object>(`/indexer/fills?portfolio=${pf.key.toBase58()}&limit=${limit}`) : { fills: [], next_before: null };
  },
});
