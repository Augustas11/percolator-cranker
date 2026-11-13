import { LAMPORTS_PER_SOL } from "@solana/web3.js";
import { effectiveAbsQ } from "../../../frontend/src/chain/accounts";
import { units } from "../drip";
import { tool } from "./tool";

export default tool({
  name: "get_position",
  description: "Read your wallet, collateral and drip perp position (devnet, test tokens).",
  run: async (drip) => {
    const pf = await drip.portfolio(), market = await drip.market(), balance = await drip.tokenBalance();
    const leg = pf?.legs.find(leg => leg.assetIndex === 0 && leg.marketId === market.asset0.marketId);
    const size = leg ? effectiveAbsQ(market, leg) : 0n;
    const capital = pf?.capital ?? 0n, pnl = pf?.pnl ?? 0n;
    return {
      wallet: drip.wallet.publicKey.toBase58(), sol_balance: await drip.conn.getBalance(drip.wallet.publicKey) / LAMPORTS_PER_SOL,
      wallet_usdc: units(balance), wallet_usdc_atoms: balance.toString(), portfolio: pf?.key.toBase58() ?? null,
      capital: units(capital), capital_atoms: capital.toString(), engine_pnl: units(pnl), engine_pnl_atoms: pnl.toString(),
      equity: units(capital + pnl), equity_atoms: (capital + pnl).toString(), mark_price: units(market.asset0.effectivePrice),
      position: leg && size ? { symbol: drip.symbol, side: leg.side, size: units(size), size_atoms: size.toString() } : null,
    };
  },
});
