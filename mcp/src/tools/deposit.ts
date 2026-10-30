import { z } from "zod";
import { LAMPORTS_PER_SOL } from "@solana/web3.js";
import { parsePortfolio } from "../../../frontend/src/chain/accounts";
import { PORTFOLIO_LAMPORTS } from "../../../frontend/src/chain/instructions";
import { createAtaIdempotentIx } from "../../../frontend/src/chain/pda";
import { buildCreatePortfolio, buildDeposit } from "../../../frontend/src/chain/tx";
import { atoms } from "../drip";
import { tool } from "./tool";

export default tool({
  name: "deposit",
  description: "Deposit test USDC; a new portfolio costs SOL rent (devnet, test tokens).",
  inputSchema: { amount_usdc: z.number().positive() },
  run: async (drip, { amount_usdc }) => {
    const value = atoms(amount_usdc, "amount_usdc");
    if (await drip.tokenBalance() < value) throw new Error("Insufficient wallet test USDC; call claim_test_funds");
    let pf = await drip.portfolio();
    let created;
    if (!pf) {
      if (await drip.conn.getBalance(drip.wallet.publicKey) < PORTFOLIO_LAMPORTS + 20000) {
        throw new Error(`Creating a portfolio costs rent (${PORTFOLIO_LAMPORTS / LAMPORTS_PER_SOL} SOL) plus fees; call claim_test_funds`);
      }
      const portfolio = await buildCreatePortfolio(drip.cfg, drip.wallet.publicKey);
      created = { ...await drip.send(async () => portfolio.ixs, [portfolio.portfolio]), note: "Portfolio created; rent paid from your SOL", rent_lamports: PORTFOLIO_LAMPORTS };
      pf = { key: portfolio.portfolio.publicKey, ...parsePortfolio((await drip.account(portfolio.portfolio.publicKey)).data) };
    }
    const key = pf.key;
    return { ...await drip.send(async () => [createAtaIdempotentIx(drip.wallet.publicKey, drip.wallet.publicKey, drip.cfg.mint), ...await buildDeposit(drip.conn, drip.cfg, drip.wallet.publicKey, key, value)]), ...(created ? { portfolio_created: created } : {}) };
  },
});
