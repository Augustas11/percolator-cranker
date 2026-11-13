import { devnetConfig } from "../chain/config";

const repo = "https://github.com/Augustas11/drip-markets";
const market = devnetConfig.market.toBase58();

export const site = {
  heroLine: null as string | null, // TODO-OWNER: hero line slot. null = not rendered.
  paths: { home: "/", tools: "/tools", arena: "/arena", build: "/build", trade: "/trade", compute: "/compute", docs: "/docs", agents: "/agents" },
  computeTrade: null as string | null, // TODO-OWNER: trade page for the compute perp; until set, /compute links the market account on Solana explorer
  computeRepo: "" as string, // empty while the repo is private; the /compute page hides the link when empty
  repo,
  docs: `${repo}/blob/main/docs/RUNBOOK.md`,
  x: null as string | null, // TODO-OWNER: X profile URL
  telegram: null as string | null, // TODO-OWNER: Telegram URL
  status: null as string | null, // TODO-OWNER: status page URL
  feeBps: 12,
  maxFill: "$100k",
  markLag: { value: "0.7 s", note: "p50, devnet, measured" }, // static until measured live
  issues: `${repo}/issues`,
  runbook: `${repo}/blob/main/docs/RUNBOOK.md`,
  officialSite: "https://drip-devnet.vercel.app",
  explorerAddress: (id: string) => `https://explorer.solana.com/address/${id}?cluster=devnet`,
  programId: devnetConfig.programId.toBase58(),
  market,
  mcp: {
    package: "@driptech/mcp",
    final: true,
    install: "claude mcp add drip -- npx -y @driptech/mcp",
    computeMarket: null as string | null, // TODO-OWNER: the MCP `market` value for COMPUTE-PERP; /agents shows compute trading examples once set
    json: `{\n  "mcpServers": {\n    "drip": { "command": "npx", "args": ["-y", "@driptech/mcp"] }\n  }\n}`,
  },
  agents: {
    run: "cd agents && npm ci && npm run build && DRIP_KEYPAIR_TREND=your-wallet.json npm start -- trend",
    source: `${repo}/tree/main/agents` as string | null,
  },
  faucet: null as string | null, // web faucet page not built; agents use the MCP tool claim_test_funds
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
