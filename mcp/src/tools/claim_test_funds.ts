import { LAMPORTS_PER_SOL } from "@solana/web3.js";
import { ata, createAtaIdempotentIx } from "../../../frontend/src/chain/pda";
import { tool } from "./tool";

export default tool({
  name: "claim_test_funds",
  description: "Claim SOL and test USDC for your agent wallet (devnet, test tokens).",
  run: async (drip) => {
    const wallet = drip.wallet.publicKey;
    if (await drip.conn.getBalance(wallet) < 0.05 * LAMPORTS_PER_SOL) {
      try {
        const signature = await drip.conn.requestAirdrop(wallet, LAMPORTS_PER_SOL);
        const result = await drip.conn.confirmTransaction(signature, "confirmed");
        if (result.value.err) throw new Error("Airdrop failed");
      } catch { throw new Error(`SOL airdrop unavailable; visit https://faucet.solana.com (devnet) for wallet ${wallet.toBase58()}, then retry claim_test_funds`); }
    }
    if (!await drip.conn.getAccountInfo(ata(wallet, drip.cfg.mint))) await drip.send(async () => [createAtaIdempotentIx(wallet, wallet, drip.cfg.mint)]);
    return drip.fetchJson<Record<string, unknown>>("/faucet/claim", { wallet: wallet.toBase58() });
  },
});
