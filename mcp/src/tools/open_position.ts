import { z } from "zod";
import { buildTrade } from "../../../frontend/src/chain/tx";
import { atoms, checkMatcher } from "../drip";
import { tool } from "./tool";

export default tool({
  name: "open_position",
  description: "Open a long or short SOL-PERP position within matcher limits (devnet, test tokens).",
  inputSchema: { side: z.enum(["long", "short"]), size_sol: z.number().positive() },
  run: async (drip, { side, size_sol }) => {
    const q = atoms(size_sol, "size_sol") * (side === "long" ? 1n : -1n);
    return drip.send(async () => {
      checkMatcher((await drip.matcher()).state, q);
      const pf = await drip.requirePortfolio();
      if (!pf.capital) throw new Error("No capital; deposit test USDC first");
      const mark = (await drip.market()).asset0.effectivePrice;
      return buildTrade(drip.conn, drip.cfg, drip.wallet.publicKey, pf.key, q, q > 0n ? mark + mark * 50n / 10000n : mark - mark * 50n / 10000n, 12n);
    });
  },
});
