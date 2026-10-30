const repo = "https://github.com/Augustas11/drip";
const market = "HyMKxfct1S9aFtq21GG3FUYUViQkmpZdk5pwGRtAz48C";

export const site = {
  heroLine: null as string | null, // TODO-OWNER: hero line slot. null = not rendered.
  paths: { home: "/", tools: "/tools", arena: "/arena", build: "/build", trade: "/trade" },
  repo,
  issues: `${repo}/issues`,
  runbook: `${repo}/blob/main/docs/RUNBOOK.md`,
  officialSite: "https://drip-devnet.vercel.app",
  explorerAddress: (id: string) => `https://explorer.solana.com/address/${id}?cluster=devnet`,
  programId: "GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS",
  market,
  mcp: {
    final: false, // TODO-MCP: flip when the MCP package ships
    install: "claude mcp add drip -- npx -y drip-mcp", // TODO-MCP
    json: `{\n  "mcpServers": {\n    "drip": { "command": "npx", "args": ["-y", "drip-mcp"] }\n  }\n}`, // TODO-MCP
  },
  faucet: null as string | null, // TODO-MCP: faucet URL or route; null = "coming soon"
  cli: "CRANKER_CYCLES=3 cargo run --release -p drip --bin drip",
  sdk: {
    final: false, // TODO-OWNER: TS SDK package name when published
    snippet: `import { Connection, PublicKey } from "@solana/web3.js";
const connection = new Connection("https://api.devnet.solana.com");
const market = new PublicKey("${market}");
const info = await connection.getAccountInfo(market);
console.log(info?.data.length);`,
  },
  upstream: [
    { name: "aeyakovenko/percolator", url: "https://github.com/aeyakovenko/percolator", note: "Risk engine" },
    { name: "aeyakovenko/percolator-prog", url: "https://github.com/aeyakovenko/percolator-prog", note: "On-chain program" },
    { name: "aeyakovenko/percolator-cli", url: "https://github.com/aeyakovenko/percolator-cli", note: "Upstream CLI and deploy scripts" },
  ],
};
