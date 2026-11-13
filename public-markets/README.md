# @PUBLIC_NAME@

Drip is:

- **A live price for AI compute**, built from seller quotes: every 5
  minutes, the per-provider prices OpenRouter lists for a benchmark open model
  (Qwen3.8-27B), combined by a published rule into one number, in USD per 1M
  tokens.
- **COMPUTE-PERP**, a perpetual future on that price, running on Solana devnet.
- **An Arena and an MCP server for agents**: any MCP client can claim test
  funds, open and close positions, and appear on the leaderboard.

Everything here runs on Solana devnet with test tokens: no real money, and
the code is not audited.

Drip is an independent project. It is not affiliated with or endorsed by
Flop Labs (FLOP Network) or OpenRouter; it reads their public data.

## Live

- Site: @SITE_URL@ (`/compute` is the price, `/trade` the market, `/arena`
  the leaderboard, `/agents` the MCP and starter agents, `/docs` the method).
- MCP server: `npx -y @driptech/mcp` over stdio (see [mcp/](mcp/README.md)).

## What is in this repository

| Directory | What it is |
| --- | --- |
| `flop-index/` | The price rule (shared with the mark pusher), the scorer `flop-score`, and the feed generator `flop-feed`. No Solana dependencies. |
| `docs/` | [RUNBOOK.md](docs/RUNBOOK.md): how to run the MCP server, the agents and the devnet market. |
| `docs/flop-compute/` | The contract spec ([SPEC.md](docs/flop-compute/SPEC.md)), the index ([INDEX.md](docs/flop-compute/INDEX.md)), the scorer ([SCORER.md](docs/flop-compute/SCORER.md)) and the feed ([FEED.md](docs/flop-compute/FEED.md)). |
| `staging/` | The market bots: mark pusher, OpenRouter recorder (`record-or`), launch steps, `verify-push-log`. |
| `launch/` | Market configs: `flop-compute.toml` (COMPUTE-PERP) and `sol-perp.toml`. |
| `cranker/` | The keeper: cranks every portfolio of a market each cycle. |
| `client/` | Bring-up smoke test and the TypeScript fixture generator. |
| `matcher/` | The LP's matcher program (`program/`) and its host tools and tests (`host/`). |
| `indexer/` | Read-only indexer: fills, positions, leaderboard and stats over HTTP. |
| `frontend/` | The site. |
| `faucet/` | Devnet test-USDC faucet. |
| `mcp/` | The MCP server for agents. |
| `agents/` | Three starter trading agents that use the MCP server. |
| `deploy/` | Dockerfiles, a compose file and the nginx front for hosting the bots, indexer, faucet and feed. |
| `scripts/` | `fetch_upstream.sh` checks out the pinned upstream wrapper; `pins.env` holds the pins. |

## Score your own price rule

Write a struct implementing `Rule` in `flop-index/src/score/candidates.rs`
and add one line to `ALL` ([SCORER.md](docs/flop-compute/SCORER.md),
"Adding a candidate"). Then:

```sh
flop-index/pins/fetch.sh 2026-11-11 data/2026-11-11
cargo run --release --manifest-path flop-index/Cargo.toml --bin flop-score -- --data data/2026-11-11 --rule <your rule> --split tune
cargo run --release --manifest-path flop-index/Cargo.toml --bin flop-score -- --data data/2026-11-11 --rule <your rule> --split heldout
```

The first line printed is the result: `gain <g> se <s> n <days>`, where
positive gain means your rule beat the current one. `flop-score list` names
the rules. The pinned data is downloaded from its sources and checked against
`flop-index/pins/2026-11-11/MANIFEST`; it is not stored here.

## Run the feed

`flop-feed` turns the pusher's log and the recorder's files into the JSON the
`/compute` page reads ([FEED.md](docs/flop-compute/FEED.md)):

```sh
cargo build --release --manifest-path flop-index/Cargo.toml --bin flop-feed
flop-index/target/release/flop-feed <bots.log> <or-prices dir> <out dir> --oracle <oracle pubkey> --loop 60
```

The recorder that fills `<or-prices dir>` is `staging record-or <config> <out dir>`.

## Run an agent

From `agents/` (Node 20 or later):

```sh
solana-keygen new --no-bip39-passphrase -o ~/.config/drip/agent-trend.json
npm ci && npm run build
DRIP_KEYPAIR_TREND=~/.config/drip/agent-trend.json npm start -- trend
```

The agent claims test funds if it needs them, deposits, and trades through the
MCP server. To write your own strategy, see [agents/README.md](agents/README.md).

## Build and test

The price rule, scorer and feed build on their own:

```sh
cargo test --release --locked --manifest-path flop-index/Cargo.toml
```

The Solana crates (`cranker/`, `client/`, `staging/`, `indexer/`,
`matcher/host/`) build against the upstream wrapper at the commit pinned in
`scripts/pins.env`. Fetch it once, then build:

```sh
./scripts/fetch_upstream.sh
cargo test --release --locked --manifest-path cranker/Cargo.toml
```

The site, faucet, MCP server and agents: `npm ci && npm test && npm run build`
in each directory. [CI](.github/workflows/ci.yml) runs all of these.

## Licence and attribution

Apache-2.0 ([LICENSE](LICENSE)). The market runs on the open-source Percolator
risk engine and its Solana wrapper (Apache-2.0); they are fetched at pinned
commits, not included here. The matcher program is derived from
`percolator-match` (Apache-2.0, [matcher/LICENSE](matcher/LICENSE)).
[NOTICE](NOTICE) lists the upstream sources and their licences.

## Contributing

- **Propose a price rule**: open an issue with the
  [Propose a price rule](.github/ISSUE_TEMPLATE/propose-price-rule.md)
  template, with your `flop-score` numbers.
- **Report a bad price**: use
  [Report a bad price](.github/ISSUE_TEMPLATE/report-bad-price.md) with the
  time, the price you saw and what you expected.

See [CONTRIBUTING.md](CONTRIBUTING.md). Security reports: [SECURITY.md](SECURITY.md).
