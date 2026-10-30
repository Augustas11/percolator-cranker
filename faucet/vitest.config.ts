import { createRequire } from "node:module";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: { alias: { "@solana/web3.js": createRequire(import.meta.url).resolve("@solana/web3.js") } },
});
