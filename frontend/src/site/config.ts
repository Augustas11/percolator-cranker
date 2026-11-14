import { devnetConfig } from "../chain/config";

const repo = "https://github.com/Augustas11/drip-markets";
const market = devnetConfig.market.toBase58();

export const site = {
  paths: { home: "/", tools: "/tools", arena: "/arena", build: "/build", trade: "/trade", compute: "/compute", docs: "/docs", agents: "/agents", challenge: "/challenge" },
  computeTrade: "/trade" as string | null, // the COMPUTE-PERP trade page; null makes /compute link the market account instead
  computeRepo: "" as string, // the /compute page hides the link when empty
  repo,
  docs: `${repo}/blob/main/docs/RUNBOOK.md`,
  x: null as string | null, // TODO-OWNER: X profile URL
  telegram: null as string | null, // TODO-OWNER: Telegram URL
  status: null as string | null, // TODO-OWNER: status page URL
  feeBps: 15, // COMPUTE-PERP trade_fee_base_bps, read from the market account on devnet 2026-11-13
  issues: `${repo}/issues`,
  runbook: `${repo}/blob/main/docs/RUNBOOK.md`,
  officialSite: "https://drip-devnet.vercel.app",
  explorerAddress: (id: string) => `https://explorer.solana.com/address/${id}?cluster=devnet`,
  // Program id and market from chain/config.ts defaults (the COMPUTE-PERP market); false hides them on /agents.
  showAddresses: true as boolean,
  programId: devnetConfig.programId.toBase58(),
  market,
  mcp: {
    package: "@driptech/mcp",
    final: true,
    install: "claude mcp add drip -- npx -y @driptech/mcp",
    // The market the MCP trades: it reads chain/config.ts, so it is COMPUTE-PERP. null hides the compute blocks on /agents.
    computeMarket: market as string | null,
    sizeArg: "size", // the open_position argument that counts units of 1M tokens
    hasComputeIndexTool: false as boolean, // true once the MCP has a tool that reads the compute index

    json: `{\n  "mcpServers": {\n    "drip": { "command": "npx", "args": ["-y", "@driptech/mcp"] }\n  }\n}`,
  },
  agents: {
    run: "cd agents && npm ci && npm run build && DRIP_KEYPAIR_TREND=your-wallet.json npm start -- trend",
    source: `${repo}/tree/main/agents` as string | null,
  },
  arena: {
    // Each is optional; an unset value is not rendered anywhere, and with no season or prize the site-wide strip is hidden.
    season: null as number | null,
    startsAt: null as string | null, // e.g. "1 Dec 2026"
    endsAt: null as string | null,
    prizes: null as string | null,
    // Drip's starter agents (agents/ in the repo), tagged as house agents on the board.
    house: [
      { name: "trend", wallet: "J4DM567mVzWy5YvCsrPYihduwhZhVa8o5aZxDgXU22S4" },
      { name: "meanrev", wallet: "GeYEh9mwDU7fSYCZ5izWri562VQMQS8fKh9FZnJzybhp" },
      { name: "breakout", wallet: "BTnMJaRnc7pqWG9AGGnHqHQgQjcv6cpRz3ECGLZmBxXN" },
    ] as { name: string; wallet: string }[],
  },
  // The FLOP stats window in latest.json when flop_side does not carry one (the pusher's flop_window_s = 21600).
  flopWindowS: 21_600,
  faucet: null as string | null, // web faucet page not built; agents use the MCP tool claim_test_funds
  // The faucet's HTTP API (POST {faucetApi}/claim, GET {faucetApi}/health), proxied by vercel.json. It pays
  // COMPUTE-PERP's test USDC (the mint in chain/config.ts). Null hides "Get test USDC".
  faucetApi: "/faucet" as string | null,
  // The Compute Oracle Challenge. Off until it is built and dated: with enabled false the nav item, footer link and
  // /challenge route do not exist. With it on, each block renders only when its value is set.
  challenge: {
    enabled: true as boolean,
    opensAt: "2026-11-23T00:00:00Z" as string | null, // ISO time the first round opens
    firstCloseAt: "2026-11-30T00:00:00Z" as string | null, // ISO time of the first weekly close
    // Checked against the public repository on 2026-11-14: these five lines build the example rules and score one.
    practiceCmd: [
      "git clone https://github.com/Augustas11/drip-markets && cd drip-markets",
      "rustup target add wasm32-unknown-unknown",
      "bash flop-index/pins/fetch.sh 2026-11-11 data",
      "sh flop-index/challenge-template/build.sh",
      "cargo run --release --manifest-path flop-index/Cargo.toml --bin flop-score -- --data data --rule wasm:flop-index/challenge-template/out/median.wasm --split tune",
    ].join("\n") as string | null, // shell commands that score a rule locally
    repoPath: "https://github.com/Augustas11/drip-markets/tree/main/flop-index/challenge-template" as string | null, // template repository URL
    // Until the submit API exists (apiBase), entries are pull requests against this repository.
    entryRepo: "https://github.com/Augustas11/drip-markets" as string | null,
    apiBase: null as string | null, // serves board.json; also the API agents call
    prizes: null as string | null,
  },
};
