# @PUBLIC_NAME@

An **independent, unofficial reference keeper (cranker)** for the Percolator
perpetuals risk engine on Solana. It is not affiliated with the upstream
authors, and it is **not a paid service**.

> **Status: reference / dry-run / staging only.** Nothing here is deployed on
> any public cluster. No fee claiming, no bounty path, no mainnet path. The
> upstream program has no published program ID yet; until it does, this code
> runs against a local `solana-test-validator`.

Upstream (read-only dependencies):
[`aeyakovenko/percolator`](https://github.com/aeyakovenko/percolator) (the
engine library) and
[`aeyakovenko/percolator-prog`](https://github.com/aeyakovenko/percolator-prog)
(the Solana wrapper program).

## What it does

Each cycle, the keeper:

1. **Discovers** every portfolio of a market (`getProgramAccounts`, header
   memcmp) and decodes it with the wrapper's own decoders.
2. **Builds the crank hints.** On a Live market, one hint per active leg
   asset. On a Resolved market, at most one oracle-less hint naming the
   asset whose backing bucket has lapsed. The Resolved crank routes the
   payout to the owner's token account.
3. **Schedules the crank.** Liquidatable accounts go first, then accounts
   with pending work, then quiet ones, rotated every cycle. The cycle spends
   a wall-clock, compute-unit and transaction budget. One failing account
   never blocks the queue.
4. **Submits `PermissionlessCrank`** and classifies each result:
   - **Progress:** the target's state changed.
   - **Idle:** the target didn't change, or the engine answered
     `EngineNonProgress`.
   - **Failed:** any other revert.
5. **Quarantines** accounts that fail with the same engine error three times
   in a row. Backoff grows exponentially and is capped, and each quarantine
   prints one JSON line you can grep for.

It also ships a deterministic load scenario, `live_shaped`. The scenario
drives a 3-asset market with about 40 traders through liquidations, funding,
backing expiry, an asset in Recovery, resolution and every exit, cranked by
the same cycle code.

## Pins

| Surface | Pin |
|---|---|
| Wrapper `percolator-prog` | `5cb331dde354517c6371a8acf92cecb194f3bb73` |
| Engine `percolator` | `4db11a8cb0053815e23a35d3a7d3edc265d8d866` (the wrapper's own Cargo pin) |
| Engine layout discriminator | 18 |

`scripts/pins.env` is the single source of truth for scripts.
`scripts/fetch_upstream.sh` refuses to continue if the wrapper's engine pin
and ours disagree.

## Compute-unit baselines (program CU, tx total in parentheses)

| Path | CU |
|---|---|
| No-op `PermissionlessCrank`, 1 asset | 18,565 (18,889) |
| No-op crank, 3 hints (`live_shaped`) | 41,695 (42,019) |
| Worst landed crank seen in `live_shaped` | 409,225 |

## Quickstart

```bash
./scripts/fetch_upstream.sh                 # wrapper @ pinned SHA → upstream/
FORCE_SBF_REBUILD=1 ./scripts/m1_bringup.sh # local validator + deploy + smoke test
source scripts/env.sh
CRANKER_CYCLES=3 cargo run --release -p percolator-cranker --bin cranker
./scripts/live_shaped.sh                    # 200-cycle load scenario
```

Prerequisites, configuration and how to read the output are covered in
[docs/RUNBOOK.md](docs/RUNBOOK.md).

## Known issue on the current pins

In the `live_shaped` scenario, some accounts become unrefreshable in Live.
This happens when a backing bucket lapses (passes its expiry) without any
account referencing it, and a later refresh would attribute new positive PnL
to that domain. Every crank on those accounts then reverts with
`EngineStale` until the market resolves.

The keeper quarantines those accounts and keeps cranking everyone else, but
the scenario's "no account stays unrefreshable" assertion fails. This
appears to be an engine-side liveness gap, not keeper behavior. It is still
present on the engine's newer master. See [docs/RUNBOOK.md](docs/RUNBOOK.md)
→ Known limitations.

## CI

`.github/workflows/ci.yml` has two jobs:

- **build + unit tests:** runs on pushes to `main` and on pull requests,
  against the pinned wrapper.
- **e2e:** runs on pull requests and manual dispatch. It does the SBF build
  (platform-tools v1.52), a local-validator bring-up, a 3-cycle dry run, the
  lifecycle integration test, and a 50-cycle `live_shaped` run.

The `live_shaped` step is **non-gating**, and its name says so, because of
the known issue above. Its report is uploaded as an artifact.

## Docs

- [docs/RUNBOOK.md](docs/RUNBOOK.md): prerequisites, build, bring-up, running, configuration, re-pinning, limitations
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): modules and the crank cycle
- [CHANGELOG.md](CHANGELOG.md), [SECURITY.md](SECURITY.md), [CONTRIBUTING.md](CONTRIBUTING.md)

## License

To be decided. See [CONTRIBUTING.md](CONTRIBUTING.md).
