# Day-0 launch kit (tech half)

**Reference / dry-run only; not a paid live service; M3 NO-GO.** Private:
`launch/`, `deploy/` and this file are never exported.

**Goal:** under 1 hour from "upstream publishes the official program ID" to
"our SOL-PERP market live with bots and liquidity", measured by the drill.
The ops half (keys, capital, runbook, go/no-go) is `ops/`; the runbook's
T-1h table calls `scripts/launch_market.sh`.

## One config

`launch/sol-perp.toml` holds everything launch-relevant: program id and RPC
per cluster, the wrapper and engine pins, the collateral mint (or `create`
for a test mint on local and devnet), the Pyth SOL/USD feed id and the
pusher's timing, the `InitMarket` params, the resolve policy, the matcher,
and the insurance, backing and LP amounts. The R2 staging run
(`scripts/staging_local.sh`) reads the same file for its params.

- **Pins:** `scripts/pins.env` is the one source of truth (it drives
  `scripts/fetch_upstream.sh` and the crates' `build.rs`). The config repeats
  them so a launch names what it was built for; `launch_market.sh` refuses to
  run when they differ.
- **Swapping the program:** edit `program_id` (and, for a new release, the
  pins in both files), run `scripts/fetch_upstream.sh`, done. The id can be
  any deployed wrapper: ours or upstream's (devnet default:
  `Bu1J8eQQ…zgLG`, upstream's, read only, never written to).
- **Trade fee 12 bps**, not the money model's 5. At 5 the market loses to
  latency arbitrage at every reachable lag; 8 is not enough on its own
  (`research/replay/REPORT.md`). The matcher's `fee_bps` must be at least
  `trade_fee_base_bps` (the wrapper refuses less).
- **Secrets stay out:** a paid RPC URL comes from `LAUNCH_RPC_URL`; keys live
  in the run dir (below), never in the config.

## Launch

```bash
scripts/launch_market.sh launch/sol-perp.toml local     # needs a validator at the config's RPC
scripts/launch_market.sh launch/sol-perp.toml devnet
scripts/launch_market.sh launch/sol-perp.toml mainnet --mainnet-i-understand
scripts/launch_market.sh stop <cluster>                 # stops bots + monitor
```

| # | Step | What | Signer |
|---|---|---|---|
| 1 | `init-market` | test mint (if `create`), token accounts, market account, vault, `InitMarket` | authority |
| 2 | `oracle` | `ConfigurePermissionlessResolve` + `ConfigureAuthMark`, one tx | authority |
| 3 | `insurance` | `TopUpInsurance` seed | authority |
| 4 | `backing` | `TopUpBackingBucket` on domains 0 and 1, one tx | authority |
| 5 | `lp` | LP portfolio + deposit; test-trader portfolio + deposit (not mainnet). The matcher is off-chain (co-signed `TradeNoCpi`), so it has no on-chain config | LP, trader |
| 6 | start bots | `staging bots` (pusher, matcher, backing, insurance, keeper) and the monitor, in the background with logs and pid files | hot keys |
| 7 | health | a clean keeper cycle, a fill, the mark lag, monitor sane (chain read, Live, keeper log read, 2 portfolios, 2 fresh buckets, no senior shortfall) | — |

Steps 1-5 are `staging step <name> <config> <cluster> <run dir>`: the R2
setup code (`staging/src/main.rs`), split into steps (`staging/src/launch.rs`).
The authority holds every asset role at `InitMarket`.

**Run dir** `launch-runs/<cluster>/` (gitignored; `LAUNCH_RUN_DIR`
overrides): `state.json` (program id, mint, market, portfolios, completed
steps), `keys/` (generated on local and devnet; on mainnet the owner places
the hot `payer.json` and `lp.json`), `unsigned/`, `bots.log`, `monitor.log`,
`pids/`, `timings.txt`.

**Resume.** Re-run the same command. Completed steps are skipped (state
file); the failed one is retried. Each step also checks the chain first:
existing accounts are not recreated, and `InitMarket`, the policies, the
insurance seed, the backing top-up and the deposits are skipped when the
market already shows them (control sequences, bucket status, portfolio
capital). Bots already running are left alone. A run dir is bound to one
program id.

**Safety.** Mainnet needs a non-empty `program_id`, `--mainnet-i-understand`
and an RPC whose genesis is mainnet's; local and devnet refuse an RPC whose
genesis is mainnet's. Fee shares, bounty paths and the keeper's withdrawal
test stay off. Never run devnet or mainnet from an agent session.

## Mainnet and the multisig

On mainnet the Squads vault (`multisig_vault`, 2-of-3, `ops/KEYS.md`) is the
market authority (option 1 in `ops/KEYS.md`: `InitMarket` signed by the
vault). Every authority transaction is **not signed**: the step writes it
to `unsigned/<label>.b58` (base58 bincode, vault as signer and fee payer, no
compute-budget instructions) with a `README.md`, prints how to resume, and
exits 10. After the multisig executes it, the same command sees it on chain
and goes on. Rounds: `InitMarket`, oracle, insurance, backing (4); the LP
deposit is signed by the hot LP key. No Squads SDK.

*To verify on devnet before Day 0:* Squads import of these files, that the
wrapper accepts the vault as signer through Squads' CPI, and the execute
transaction's compute limit (1.4M) and heap frame (128 KiB).

Not automated, so mainnet stops after step 5 with a note:

- **Role split** (`UpdateAssetAuthority`: oracle to the pusher key, etc.,
  `ops/KEYS.md` "split order"). The incoming key must co-sign, so it is a
  multisig + hot-key procedure, not a script step. Until then the vault holds
  the oracle role and no bot can push. `staging bots` refuses mainnet.
- **A matcher for external takers.** Our matcher is the rehearsal one: LP and
  our own test trader co-sign `TradeNoCpi`. Serving outside takers needs a
  `TradeCpi` matcher (upstream's `auth_matcher` fixture is the candidate,
  R2 reuse survey) or a co-signing RFQ service. Not built.

## Pusher latency and the matcher

The money case holds only with the posted mark within about **400 ms** of
the market (`research/replay/REPORT.md`). Config: `oracle.lag_target_ms`
(400), `oracle.push_interval_ms` (the bots tick, 3,000), heartbeat and
deviation.

- **Measured lag** = source price time → push landed (processed, polled
  every 50 ms; the push skips preflight). Mock: the time the price was
  generated, so it is our pipeline only (local p50 ~510 ms with 400 ms
  slots). Pyth: the account's `publish_time`, whole seconds, so it reads up
  to 1 s high.
- The pusher logs `{"event":"mark_lag","ms":…}` per push; the monitor
  exports it as `percolator_keeper_mark_lag_ms` (it reads the bots log) and
  `monitor/alerts.yml` alerts `PusherMarkLagHigh` above 400 ms for 5 min.
  The health check prints p50 and max over 5 pushes; over target it warns
  (`LAUNCH_LAG_STRICT=1` fails).
- **Price source:** `price_source = "pyth"` (devnet, mainnet) reads the
  Pyth SOL/USD PriceUpdateV2 account (`oracle.pyth_price_account`) over the
  same RPC and parses it with the wrapper's own reader (owner, feed id, age
  `pyth_max_age_s`, confidence `pyth_max_conf_bps`); no new dependency.
  `"mock"` (local) keeps the seeded walk. Tested locally against devnet's
  account cloned into the validator; not run on devnet.
- **That source is too slow for the money case.** Devnet's sponsored
  SOL/USD account was ~70 s old when checked (2026-10-29). The low-latency
  path is Hermes (what upstream's `pyth-push-devnet.ts` reads), and Hermes'
  price endpoint now answers 401 without an API key. Next small PR: a Hermes
  (with key) or Pyth Lazer source behind `source_price`; mainnet needs it.
- **Sub-400 ms needs the pusher on its own loop.** One bots tick runs all
  roles; a faster tick also speeds the matcher, backing and keeper.
- **The matcher refuses fills while the mark lags; keep it.** The engine
  refuses a risk-increasing trade while an asset's AuthMark target differs
  from its effective price (`trade_preflight_risk_gate`, engine `v16.rs`
  16153, `asset_has_target_effective_lag` 16013). The matcher never works
  around it: it opens only right after a push and with the asset cranked to
  the current slot in the same tx, and the mock pusher keeps each step
  inside `max_price_move_bps_per_slot`. With Pyth a larger move leaves the
  effective price walking behind, and opens are refused until it catches up.
  Do not remove this.

## Drill

```bash
scripts/day0_drill.sh            # DRILL_SKIP_SBF_BUILD=1 reuses the .so
```

A fresh validator (port 8699), the wrapper deployed with `solana program
deploy` to a newly generated program id ("upstream publishes a program ID"),
that id written into a temp copy of the config, `launch_market.sh` on local,
per-step and total times printed, everything stopped. Measured times: see
`prep.md`, "Day-0 drill".

**Finding (keeper, not changed here):** drip's canonical Pyth PDA
(`observation::pyth_price_update_v2_pda`) derives from the receiver program
(`rec5EK…`); Pyth's sponsored accounts derive from the push-oracle program
(`pythWS…`). Devnet SOL/USD exists only at the latter (`7UVimf…`). It
matters only for Pyth-profile (Hybrid) assets, not AuthMark.

**Finding:** with the matcher trading every tick, its in-tx cranks keep both
portfolios fresh, so the keeper's cranks are mostly idle
(`EngineNonProgress`) and a landed keeper crank can take minutes. The health
check therefore asks for a clean keeper cycle (submitted, none failed), not
a landed crank; and `KeeperNoSuccessfulCrank` (750 slots) may fire on such a
market. Revisit the alert before Day 0.

## Devnet (owner-run)

1. `scripts/launch_market.sh launch/sol-perp.toml devnet` once: it creates
   `launch-runs/devnet/keys/` and stops at "payer … fund it".
2. Fund the payer with ≥ 10 SOL (market account rent) from the devnet
   faucet. The other keys need no SOL.
3. Re-run the same command. `program_id` defaults to upstream's devnet
   program; set ours to rehearse on our deployment.
4. Testers get test USDC with `scripts/staging_usdc_faucet.sh <wallet>
   <usdc> [--yes]` (reads the config and the devnet run dir).

## Nightly rehearsal

`.github/workflows/day0-nightly.yml`, nightly and on dispatch, report-only:
it clones upstream wrapper `main` (not our pin), points our crates at its
engine rev, builds with `PERCOLATOR_SKIP_PIN_CHECK=1`, and runs the drill.
The job summary has both SHAs and the times. A red run means upstream moved.

## Hosting

One server; `deploy/`:

- `docker-compose.yml` + `Dockerfile`: bots, monitor (127.0.0.1:9184),
  Prometheus (127.0.0.1:9090, rules `monitor/alerts.yml`), Alertmanager to
  Telegram. `cp deploy/launch.env.example deploy/.env`, fill it in (0600,
  gitignored), `docker compose -f deploy/docker-compose.yml up -d --build`.
- `systemd/percolator-bots.service`, `systemd/percolator-monitor.service`
  (user `percolator`, run dir `/var/lib/percolator-launch`, env
  `/etc/percolator/launch.env`); Prometheus and Alertmanager from the distro
  with `deploy/prometheus.yml` (targets 127.0.0.1) and
  `deploy/alertmanager.yml`. `percolator-staging@.service` stays for R2.
- Env: `LAUNCH_CLUSTER`, `LAUNCH_RPC_URL` (the paid RPC),
  `PERCOLATOR_PROGRAM_ID`, `LAUNCH_RUN_DIR`, `TELEGRAM_BOT_TOKEN`,
  `TELEGRAM_CHAT_ID`.
- The run dir from the launch (state.json + keys/) is copied to the host.
  Mainnet: only the hot keys of `ops/KEYS.md`, placed on their host.
- **Keeper:** inside the bots process (`drip::cycle`, the keeper's own
  code). The `drip` binary itself boots its own market on 127.0.0.1:8899 at
  this pin, so it cannot crank an existing market; changing that would change
  the keeper.
