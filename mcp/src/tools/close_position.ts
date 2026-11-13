import { effectiveAbsQ } from "../../../frontend/src/chain/accounts";
import { buildClose } from "../../../frontend/src/chain/tx";
import { units } from "../drip";
import { tool } from "./tool";

export function closeChunk(size: bigint, maxFill: bigint) {
  if (maxFill <= 0n) throw new Error("Matcher max fill must be positive");
  return size < maxFill ? size : maxFill;
}

export default tool({
  name: "close_position",
  description: "Close your drip perp position (devnet, test tokens).",
  run: async (drip) => {
    const signatures: string[] = [], explorer: string[] = [];
    let start = 0n;
    for (let i = 0; i < 10; i++) {
      const pf = await drip.portfolio();
      const market = await drip.market();
      const leg = pf?.legs.find(leg => leg.assetIndex === 0 && leg.marketId === market.asset0.marketId);
      const size = leg ? effectiveAbsQ(market, leg) : 0n;
      if (!pf || !leg || size === 0n) {
        if (i === 0) throw new Error("no open position");
        break;
      }
      if (i === 0) start = size;
      const maxFill = (await drip.matcher()).state.maxFill;
      closeChunk(size, maxFill);
      const result = await drip.send(() => buildClose(drip.conn, drip.cfg, drip.wallet.publicKey, pf.key, 50n, 12n, maxFill));
      signatures.push(result.signature); explorer.push(result.explorer);
    }
    const pf = await drip.portfolio();
    const market = await drip.market();
    const leg = pf?.legs.find(leg => leg.assetIndex === 0 && leg.marketId === market.asset0.marketId);
    const remaining = leg ? effectiveAbsQ(market, leg) : 0n;
    return { signatures, explorer, closed: units(start - remaining), complete: remaining === 0n, remaining: units(remaining) };
  },
});
