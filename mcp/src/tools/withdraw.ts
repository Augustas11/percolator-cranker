import { z } from "zod";
import { createAtaIdempotentIx } from "../../../frontend/src/chain/pda";
import { buildWithdraw } from "../../../frontend/src/chain/tx";
import { atoms } from "../drip";
import { tool } from "./tool";

export default tool({
  name: "withdraw",
  description: "Withdraw test USDC from your portfolio (devnet, test tokens).",
  inputSchema: { amount_usdc: z.number().positive() },
  run: async (drip, { amount_usdc }) => {
    const value = atoms(amount_usdc, "amount_usdc");
    return drip.send(async () => {
      const pf = await drip.requirePortfolio();
      if (value > pf.capital) throw new Error("Withdrawal exceeds portfolio capital");
      return [createAtaIdempotentIx(drip.wallet.publicKey, drip.wallet.publicKey, drip.cfg.mint), ...await buildWithdraw(drip.conn, drip.cfg, drip.wallet.publicKey, pf.key, value)];
    });
  },
});
