import { units } from "../drip";
import { tool } from "./tool";

export default tool({
  name: "perps_markets",
  description: "List the drip perp market and matcher limits (devnet, test tokens).",
  run: async (drip) => {
    const market = await drip.market();
    return {
      symbol: drip.symbol, network: "devnet", collateral: "test USDC", market: drip.cfg.market.toBase58(),
      program: drip.cfg.programId.toBase58(), mint: drip.cfg.mint.toBase58(), lp_portfolio: drip.cfg.lpPortfolio.toBase58(),
      matcher_program: drip.cfg.matcherProgram?.toBase58(), matcher_context: drip.cfg.matcherContext?.toBase58(),
      mark_price: units(market.asset0.effectivePrice), max_leverage: 10000 / Number(market.initialMarginBps),
      maintenance_margin_ratio: Number(market.maintenanceMarginBps) / 10000, matcher: (await drip.matcher()).output,
    };
  },
});
