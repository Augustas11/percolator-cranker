import { effectiveAbsQ } from "../../../frontend/src/chain/accounts";
import { buildClose, buildTrade } from "../../../frontend/src/chain/tx";
import { units } from "../drip";
import { tool } from "./tool";

export function closeChunk(size: bigint, maxFill: bigint) {
  if (maxFill <= 0n) throw new Error("Matcher max fill must be positive");
  return size < maxFill ? size : maxFill;
}

export default tool({
  name: "close_position",
  description: "Close your SOL-PERP position (devnet, test tokens).",
  run: async (drip) => {
    const signatures: string[] = [], explorer: string[] = [];
    let closed = 0n;
    for (let i = 0; i < 10; i++) {
      const pf = await drip.portfolio();
      const market = await drip.market();
      const leg = pf?.legs.find(leg => leg.assetIndex === 0 && leg.marketId === market.asset0.marketId);
      const size = leg ? effectiveAbsQ(market, leg) : 0n;
      if (!pf || !leg || size === 0n) {
        if (i === 0) throw new Error("no open position");
        break;
      }
      const chunk = closeChunk(size, (await drip.matcher()).state.maxFill);
      const mark = market.asset0.effectivePrice;
      const result = await drip.send(() => chunk === size
        ? buildClose(drip.conn, drip.cfg, drip.wallet.publicKey, pf.key, 50n, 12n)
        : buildTrade(drip.conn, drip.cfg, drip.wallet.publicKey, pf.key, leg.side === "long" ? -chunk : chunk,
          leg.side === "long" ? mark - mark * 50n / 10000n : mark + mark * 50n / 10000n, 12n));
      signatures.push(result.signature); explorer.push(result.explorer);
      closed += chunk;
      if (chunk === size) break;
    }
    return { signatures, explorer, closed_sol: units(closed) };
  },
});
