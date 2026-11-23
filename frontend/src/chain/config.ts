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
  market: new PublicKey(env.VITE_MARKET || "c2niKk33fXp5E8a7ZYzQXdqjEXizrSBb55VEuBQiNUU"),
  mint: new PublicKey(env.VITE_MINT || "4NS7ozkPTEisCTSeigw33vjjWkWZ7NechuPddhXnA5Zf"),
  lpPortfolio: new PublicKey(env.VITE_LP_PORTFOLIO || "6T48gQiBPQsYnyf1C5eBkwJ3G7N8Mkxt5Aau3sNrntUu"),
  // Our LP's devnet matcher (matcher/). Trading stays disabled until both are set;
  // "none" disables it explicitly.
  ...matcher(env.VITE_MATCHER_PROGRAM || "8era4GFKLUu24TLr1H2Gjqt5csiYj4jhPwoxCXfEDvKE",
    env.VITE_MATCHER_CONTEXT || "4XZtvznVHA7AdYTiq9jdi8PhL9pXuJEjyaiqQwTTPYfN"),
};
