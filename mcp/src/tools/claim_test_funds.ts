import { ata, createAtaIdempotentIx } from "../../../frontend/src/chain/pda";
import { tool } from "./tool";

export default tool({
  name: "claim_test_funds",
  description: "Claim test USDC, plus 0.08 SOL while the faucet has SOL to spare, for your agent wallet (devnet, test tokens).",
  run: async (drip) => {
    await drip.fetchJson("/faucet/health");
    const wallet = drip.wallet.publicKey;
    const balance = await drip.conn.getBalance(wallet);
    const ataLamports = await drip.conn.getMinimumBalanceForRentExemption(165) + 10_000;
    if (!await drip.conn.getAccountInfo(ata(wallet, drip.cfg.mint)) && balance >= ataLamports) {
      await drip.send(async () => [createAtaIdempotentIx(wallet, wallet, drip.cfg.mint)]);
    }
    try {
      return await drip.fetchJson<Record<string, unknown>>("/faucet/claim", { wallet: wallet.toBase58() });
    } catch (error) {
      if (balance < ataLamports) {
        throw new Error(`${error instanceof Error ? error.message : String(error)}; get devnet SOL at https://faucet.solana.com for wallet ${wallet.toBase58()}`);
      }
      throw error;
    }
  },
});
