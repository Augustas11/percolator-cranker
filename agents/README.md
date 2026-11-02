# Starter agents

Three small trading templates for the drip devnet SOL-PERP market. Devnet,
test tokens only. All reads and trades go through the published `@driptech/mcp`
server over stdio; there is no chain SDK here. House agents are not prize-eligible.

- `trend`: follows EMA(5) versus EMA(20) of sampled mark prices.
- `meanrev`: fades a 20-sample z-score beyond ±1.5 and exits inside ±0.3.
- `breakout`: enters outside the previous 30-sample channel and exits across its midpoint.

## Quick start

Requires Node >=20 and npm. From `agents/`:

Give each agent its own wallet. Don't reuse `~/.config/drip/devnet-agent.json`:
that is the drip MCP's default wallet, and the agent would close
positions you opened there.

```sh
solana-keygen new --no-bip39-passphrase -o ~/.config/drip/agent-trend.json
npm ci && npm run build
DRIP_KEYPAIR_TREND=~/.config/drip/agent-trend.json npm start -- trend
```

If the portfolio is missing or holds no capital, the agent claims test funds
if needed and deposits 500 test USDC. Samples start empty on each run.

Each strategy runs with its own MCP child and needs its own wallet; two agents
given the same wallet are refused:

```sh
DRIP_KEYPAIR_TREND=/path/trend.json DRIP_KEYPAIR_MEANREV=/path/meanrev.json \
  npm start -- trend,meanrev
```

| Environment | Default | Meaning |
| --- | --- | --- |
| `DRIP_KEYPAIR_<NAME>` | unset | Agent wallet file, e.g. `DRIP_KEYPAIR_TREND` (required) |
| `DRIP_KEYPAIR` | unset | Wallet file when running a single agent |
| `DRIP_RPC_URL` | MCP devnet default | RPC endpoint, passed through to MCP |
| `DRIP_MCP_CMD` | `drip-mcp` (pinned in package-lock, on PATH under `npm start`) | MCP command, split on whitespace (no shell quoting) |
| `DEPOSIT_USDC` | `500` | Initial test USDC deposit when no portfolio exists |
| `INTERVAL_S` | `60` | Delay between samples, in seconds |
| `SIZE_SOL` | `0.5` | Size of each new position |
| `MAX_TRADES_PER_HOUR` | `6` | Rolling hourly trade-call limit; zero disables trading |
| `MIN_SOL` | `0.005` | Pause trading below this wallet SOL balance |

A flip closes then opens and requires two available trade calls. Failed calls
also consume budget because a timeout may follow an executed trade. The budget
is in memory and resets on restart. Low SOL pauses trades while sampling continues.
Errors double the delay up to ten minutes; a successful tick resets it.
Events are JSON lines on stdout. SIGINT/SIGTERM closes the MCP connection and
stops the agent, leaving its position open.

## Write your own

Copy a file in `src/strategies`, implement the pure `decide(prices, current)`
function, add it to `src/strategies/index.ts`, then run `npm test`. Directions
are `-1` (short), `0` (flat), and `1` (long). History holds at most 200 samples.

## Docker

Build from this directory, then mount one wallet file read-only:

```sh
docker build -t drip-agents .
docker run --rm --init --user "$(id -u):$(id -g)" \
  -v ~/.config/drip/agent-trend.json:/keys/trend.json:ro \
  -e DRIP_KEYPAIR_TREND=/keys/trend.json drip-agents trend
```

`--user` lets the container read a 0600 wallet file you own. The MCP is
installed in the image from package-lock.json. No wallet or secrets are copied
into the image.
