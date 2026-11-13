# Starter agents

Three small trading templates for the drip devnet SOL-PERP market. Devnet,
test tokens only. All reads and trades go through the published `@driptech/mcp`
server over stdio; there is no chain SDK here. House agents are not prize-eligible.

- `trend`: follows EMA(5) versus EMA(20) of sampled mark prices.
- `meanrev`: fades a 20-sample z-score beyond ±1.5 and exits inside ±0.3.
- `breakout`: enters outside the previous 30-sample channel and exits across its midpoint.

## House agents (COMPUTE-PERP)

Two more agents for the COMPUTE-PERP market (USD per 1M tokens of
Qwen3.8-27B). They are house agents: their start and tick lines carry
`"house": true`, they are not prize-eligible, and they exist to put realistic,
explainable flow on devnet while the mark sits still. Each trades only for a
stated reason; none trades because time passed, and they are not paired to trade
against each other (every fill is against the LP's matcher). On this market the
size unit (`size` in the MCP, `SIZE_SOL` here) is 1M tokens.

- `basis`: trades the gap between the broad market and the settlement mark.
  Each tick it reads `<DRIP_FEED_URL>/market.json` and computes
  `basis = ln(price / mark)`, where `price` is the benchmark model's median
  across all its sellers (`DRIP_BASIS_SOURCE=median`) or its routed price, what
  a request pays now across the sellers that are up (`routed`). The gap has a
  standing level: the broad market includes cheaper, lower-precision sellers
  than the mark (about -5% for the median, about -45% for the routed price on
  2026-11-13). So the agent trades the deviation of the basis from its own
  mean over the last `DRIP_BASIS_WINDOW_H` hours, one sample per new snapshot:
  long when the broad market sits more than `DRIP_BASIS_ENTRY` above its usual
  gap to the mark, short when it sits that far below, but only after the same
  side shows on `DRIP_BASIS_CONFIRM` consecutive new snapshots. It goes flat
  once the deviation is inside `DRIP_BASIS_EXIT`; in between it holds. It does
  not trade until it has `DRIP_BASIS_MIN_SAMPLES` samples (the mean lives in
  memory, so a restart warms up again). If the feed is unreachable, older than
  `DRIP_FEED_MAX_AGE_S`, or the model is missing, thin or has no price, it
  holds its position and logs why (`reason`). It never trades on stale data.
- `hedger`: a month of inference, hedged. `DRIP_HEDGE_SIDE=long` is a buyer
  with an inference bill locking in its price; `short` is a seller hedging its
  revenue. It holds `DRIP_HEDGE_MTOK` million tokens (notional = budget x
  mark) on its side. A budget above one matcher fill (100) is built over
  several fills, one per tick and within `MAX_TRADES_PER_HOUR`. Once there, it
  trades again only when the held notional drifts from the target by more than
  `DRIP_HEDGE_BAND`: it adds when short of it, and closes and rebuilds when over.
  It never grows past initial margin (50%, 2x leverage) of 80% of its equity.
  It trades rarely by design; its job is standing open interest. Run one
  process per side, each with its own wallet.

Each tick line includes the inputs behind the decision (`price`, `mark`,
`basis`, `mean`, `deviation`, thresholds, `feed_age_s`, `samples`, `streak`,
`reason`; for the hedger `target_notional`, `held_notional`, `drift`, `band`,
`equity`, `reason`), so every trade can be explained afterwards.

```sh
DRIP_KEYPAIR_BASIS=/path/basis.json DRIP_FEED_URL=https://feed.example SIZE_SOL=50 npm start -- basis
DRIP_KEYPAIR_HEDGER=/path/hedge-long.json DRIP_HEDGE_SIDE=long npm start -- hedger
```

| Environment | Default | Meaning |
| --- | --- | --- |
| `DRIP_FEED_URL` | unset | Base URL serving `market.json` (required for `basis`) |
| `DRIP_BASIS_SOURCE` | `median` | `median` (`models[].median`) or `routed` (`models[].routed.price`) |
| `DRIP_BASIS_MODEL` | `qwen/qwen3.8-27b` | Model id to read from `market.json` |
| `DRIP_BASIS_ENTRY` | `0.08` median, `0.02` routed | Enter when the deviation from the mean exceeds this (log ratio) |
| `DRIP_BASIS_EXIT` | `0.02` median, `0.005` routed | Go flat when the deviation is inside this; must be under entry |
| `DRIP_BASIS_CONFIRM` | `2` | Consecutive new snapshots needed before entering or flipping |
| `DRIP_BASIS_WINDOW_H` | `6` | Hours of samples in the rolling mean |
| `DRIP_BASIS_MIN_SAMPLES` | `12` | Samples needed before trading (about an hour of 5-minute snapshots) |
| `DRIP_FEED_MAX_AGE_S` | `1800` | Hold when the feed's `data_ms` is older than this |
| `DRIP_HEDGE_SIDE` | unset | `long` or `short` (required for `hedger`) |
| `DRIP_HEDGE_MTOK` | `100` | Hedged monthly budget, in millions of tokens |
| `DRIP_HEDGE_BAND` | `0.1` | Re-size when held notional drifts more than this fraction |

Market limits (`perps_markets` on 2026-11-13): 100 units per fill, LP
inventory cap 1000 units, 2x max leverage. `basis` refuses a `SIZE_SOL` above
one fill. Keep the house agents' combined one-sided size under the LP cap, or
the matcher refuses the excess. The basis defaults were picked from the first
4.7 hours of all-seller prices (2026-11-13); revisit them once there are days of
feed history.

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

A strategy that needs outside data or its own size implements
`observe({ mark, current, size, equity, now })` instead (see `HouseStrategy`
in `src/strategies/types.ts`). It returns `{ target, size?, resize?, add?, inputs }`;
the agent logs `inputs` on the tick line and opens at `size` (default
`SIZE_SOL`). `resize` closes and reopens on the same side; `add` opens `size`
more on the side already held (one trade).

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
