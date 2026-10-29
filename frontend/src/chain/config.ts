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
export const devnetConfig: DevnetConfig = {
  rpcUrl: env.VITE_RPC_URL || "https://api.devnet.solana.com",
  programId: new PublicKey(env.VITE_PROGRAM_ID || "GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS"),
  market: new PublicKey(env.VITE_MARKET || "HyMKxfct1S9aFtq21GG3FUYUViQkmpZdk5pwGRtAz48C"),
  mint: new PublicKey(env.VITE_MINT || "Ev8AkZC9pT2QCncYFut8GyGB2ffWN1yvnoVbiUTz77q7"),
  lpPortfolio: new PublicKey(env.VITE_LP_PORTFOLIO || "6wH2d4CARhEWeGC2deBDb9pVT1a7Tgd48VUiFmkTNAHY"),
  // Our LP's devnet matcher (matcher/). Trading stays disabled until both are set;
  // "none" disables it explicitly.
  ...matcher(env.VITE_MATCHER_PROGRAM || "8era4GFKLUu24TLr1H2Gjqt5csiYj4jhPwoxCXfEDvKE",
    env.VITE_MATCHER_CONTEXT || "GMXktcTjqh8z3h2oCgUnodcLbh6hH4W6YUfL4hZMaXe7"),
};
