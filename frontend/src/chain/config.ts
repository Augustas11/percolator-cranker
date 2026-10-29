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
  return program === "none" || context === "none"
    ? {} : { matcherProgram: new PublicKey(program), matcherContext: new PublicKey(context) };
}
export const devnetConfig: DevnetConfig = {
  rpcUrl: env.VITE_RPC_URL || "https://api.devnet.solana.com",
  programId: new PublicKey(env.VITE_PROGRAM_ID || "GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS"),
  market: new PublicKey(env.VITE_MARKET || "7p8DtcMHSjdCsM9xWvBwF6TebGBSL18wKvfM7BS1zyHq"),
  mint: new PublicKey(env.VITE_MINT || "Ev8AkZC9pT2QCncYFut8GyGB2ffWN1yvnoVbiUTz77q7"),
  lpPortfolio: new PublicKey(env.VITE_LP_PORTFOLIO || "GKJHRYBKwtswFHK5oyGknpcXxYEMjh7rDDR9f1KU9T5P"),
  // Our LP's devnet matcher (matcher/ on branch claude/onchain-matcher). Set both to
  // "none" to run with trading disabled.
  ...matcher(env.VITE_MATCHER_PROGRAM || "8era4GFKLUu24TLr1H2Gjqt5csiYj4jhPwoxCXfEDvKE",
    env.VITE_MATCHER_CONTEXT || "BHsZrk4cck5xBUhFkE59G9ZJtDLvSZLEtAmpn3wNbDTK"),
};
