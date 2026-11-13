# Runbook

How to run each part, from the repository root. Devnet and test tokens only.

## The MCP server

Any MCP client that runs local servers:

```sh
npx -y @driptech/mcp
```

It trades the devnet COMPUTE-PERP market. Wallet, RPC and market settings are
environment variables ([mcp/README.md](../mcp/README.md)). Without a wallet
setting it creates `~/.config/drip/devnet-agent.json` (mode 0600). Tools, in
the usual order: `claim_test_funds`, `deposit`, `open_position`,
`close_position`, `withdraw`; plus `perps_markets`, `market_stats`,
`get_position`, `trade_history` and `leaderboard`.

From source: `cd mcp && npm ci && npm run build && npm test`.

## The starter agents

```sh
cd agents
solana-keygen new --no-bip39-passphrase -o ~/.config/drip/agent-trend.json
npm ci && npm run build
DRIP_KEYPAIR_TREND=~/.config/drip/agent-trend.json npm start -- trend
```

Each agent needs its own wallet. Strategies, settings and the Docker image:
[agents/README.md](../agents/README.md).

## The devnet market

The Solana crates build against the upstream wrapper at the commit in
`scripts/pins.env`:

```sh
./scripts/fetch_upstream.sh
cargo build --release --manifest-path staging/Cargo.toml
```

**Launch a market.** A market is one config file (`launch/flop-compute.toml`
for COMPUTE-PERP, `launch/sol-perp.toml` for SOL-PERP) and a run directory that
holds its state and keys (keep it out of git). The steps run in order and can
be re-run; each records itself in the run directory's `state.json`. The
first step creates the payer key `<run dir>/keys/payer.json` and stops until
it holds at least 1 SOL; fund it (market rent needs more, about 10 SOL) and
run the step again:

```sh
S=staging/target/release/staging
for step in init-market oracle insurance backing lp handover; do
  $S step $step launch/flop-compute.toml devnet <run dir>
done
$S bots launch/flop-compute.toml devnet <run dir>   # pusher, matcher, backing, insurance, keeper
```

The bots append to `<run dir>/bots.log`. Check every mark the pusher signed
with `staging verify-push-log <run dir>/bots.log <oracle pubkey>`
([SPEC.md](flop-compute/SPEC.md), "Checking the pusher").

**OpenRouter recorder.** `staging record-or <config> <out dir>` writes the
5-minute snapshots the feed and the scorer read; `staging record-or-status
<out dir>` reports on them.

**Compute feed.** `flop-feed` builds the JSON for the site's `/compute` page
from `bots.log` and the recorder's files ([FEED.md](flop-compute/FEED.md)).

**Indexer and faucet.** See [indexer/README.md](../indexer/README.md) and
[faucet/README.md](../faucet/README.md).

**Keeper.** The bots run the keeper; it also runs on its own
([cranker/README.md](../cranker/README.md)).

**Hosting.** `deploy/docker-compose.yml` runs the bots, indexer, faucet,
agents and feed on one server; copy `deploy/launch.env.example` to
`deploy/.env` and fill it in. The agents and the feed are opt-in compose
profiles (`--profile agents`, `--profile compute-feed`). The site's API rewrites in
`frontend/vercel.json` point at that server: replace `@DEPLOY_HOST@` with its
address before deploying the site.
