import { effectiveAbsQ } from "../../../frontend/src/chain/accounts";
import { buildClose } from "../../../frontend/src/chain/tx";
import { tool } from "./tool";

export default tool({
  name: "close_position",
  description: "Close your SOL-PERP position (devnet, test tokens).",
  run: async (drip) => {
    const pf = await drip.portfolio();
    if (!pf) throw new Error("no open position");
    const market = await drip.market();
    if (!pf.legs.some(leg => leg.assetIndex === 0 && leg.marketId === market.asset0.marketId && effectiveAbsQ(market, leg) > 0n)) throw new Error("no open position");
    return drip.send(() => buildClose(drip.conn, drip.cfg, drip.wallet.publicKey, pf.key, 50n, 12n));
  },
});
