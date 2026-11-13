# drip MCP
Devnet compute perp (Percolator v16, FLOP compute-price oracle). Devnet, test tokens only.
## Install
Any MCP client: run `npx -y @driptech/mcp` over stdio, e.g. `<your-cli> mcp add drip -- npx -y @driptech/mcp`, or:
```json
{"mcpServers":{"drip":{"command":"npx","args":["-y","@driptech/mcp"]}}}
```
## Environment
`DRIP_KEYPAIR`: Solana JSON keypair path; `DRIP_SECRET_KEY`: JSON array (path takes precedence).
Without either, `.config/drip/devnet-agent.json` in your home directory is generated with mode 0600 if missing.
`DRIP_RPC_URL`: defaults to `https://api.devnet.solana.com`; `DRIP_API_URL`: defaults to `https://drip-devnet.vercel.app`.
Market: `DRIP_PROGRAM_ID`, `DRIP_MARKET`, `DRIP_MINT`, `DRIP_LP_PORTFOLIO`, `DRIP_MATCHER_PROGRAM`, `DRIP_MATCHER_CONTEXT` override the built-in devnet compute market. Set the full set for another market, and point `DRIP_API_URL` at an indexer for that market. The matcher pair is set together; `none` for either disables trading. `DRIP_SYMBOL` sets the name `perps_markets` reports (default `COMPUTE-PERP`).
## Tools
perps_markets, market_stats, get_position, deposit, open_position, close_position, withdraw, trade_history, leaderboard, claim_test_funds.
## Quick start
`claim_test_funds` includes 0.08 devnet SOL while available.

claim_test_funds → deposit → open_position → close_position → withdraw
## Add a tool
Copy a file in `src/tools/`.
Add it to `src/tools/index.ts`.
Run `npm test`.
## Build from source
`npm ci && npm run build && npm test`
The build bundles the shared client from `../frontend/src/chain`.
## License
Apache-2.0.
