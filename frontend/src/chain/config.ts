import { PublicKey } from "@solana/web3.js";

export interface DevnetConfig {
  rpcUrl: string;
  programId: PublicKey;
  market: PublicKey;
  mint: PublicKey;
  lpPortfolio: PublicKey;
  matcherProgram?: PublicKey;
  matcherContext?: PublicKey;
}

const env = import.meta.env ?? {};
function matcher(program: string, context: string) {
  return !program || !context || program === "none" || context === "none"
    ? {} : { matcherProgram: new PublicKey(program), matcherContext: new PublicKey(context) };
}
// Defaults: the COMPUTE-PERP market on devnet (USD per 1M tokens of Qwen3.8-27B).
export const devnetConfig: DevnetConfig = {
  rpcUrl: env.VITE_RPC_URL || "https://api.devnet.solana.com",
  programId: new PublicKey(env.VITE_PROGRAM_ID || "GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS"),
  market: new PublicKey(env.VITE_MARKET || "uZY6RPrFDqtbYbREA6UToWpzq1BKEsneZxcsE4As27J"),
  mint: new PublicKey(env.VITE_MINT || "4NS7ozkPTEisCTSeigw33vjjWkWZ7NechuPddhXnA5Zf"),
  lpPortfolio: new PublicKey(env.VITE_LP_PORTFOLIO || "GcS4f5uCiYozfEtHzmttMKkHab2ympTasA9MvHashhSZ"),
  // Our LP's devnet matcher (matcher/). Trading stays disabled until both are set;
  // "none" disables it explicitly.
  ...matcher(env.VITE_MATCHER_PROGRAM || "8era4GFKLUu24TLr1H2Gjqt5csiYj4jhPwoxCXfEDvKE",
    env.VITE_MATCHER_CONTEXT || "C2uacd9rM16jMBJuseqLByDeVorD6eacaJQKR4seHczq"),
};
