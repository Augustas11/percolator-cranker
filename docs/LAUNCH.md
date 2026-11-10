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
| 1 | `init-market` | test mint (if `create`), token accounts; market account + `InitMarket` in ONE tx; vault | payer, market key, launch key |
| 2 | `oracle` | `ConfigurePermissionlessResolve` + `ConfigureAuthMark`, one tx | launch key |
| 3 | `insurance` | `TopUpInsurance` seed | launch key |
| 4 | `backing` | `TopUpBackingBucket` on domains 0 and 1, one tx | launch key |
| 5 | `lp` | LP portfolio + deposit; test-trader portfolio + deposit (not mainnet). The matcher is off-chain (co-signed `TradeNoCpi`), so it has no on-chain config | LP, trader |
| 6 | `handover` | asset-0 roles to their holders (`UpdateAssetAuthority`: oracle, insurance operator, backing, insurance, asset admin), then `marketauth` to the multisig (`UpdateAuthority`) last; each new holder co-signs; verified on chain | launch key + each new holder |
| 7 | start bots | `staging bots` (pusher, matcher, backing, insurance, keeper) and the monitor, in the background with logs and pid files | hot keys |
| 8 | health | the on-chain role table (below), a clean keeper cycle, a fill, the mark lag, and this launch's monitor (its market, a snapshot under 30 s old, process alive) sane: chain read, Live, keeper log read, 2 portfolios, 2 fresh buckets, no senior shortfall | — |

Steps 1-6 are `staging step <name> <config> <cluster> <run dir>`: the R2
setup code (`staging/src/main.rs`), split into steps (`staging/src/launch.rs`).
The launch key (local and devnet: `keys/admin.json`; mainnet: a fresh
`keys/launch_authority.json`) holds marketauth and every asset role from
create + init until the handover. `staging roles` prints the on-chain table
and fails unless every role is with its holder and none with the launch key.

**Run dir** `launch-runs/<cluster>/` for `sol-perp.toml`, `launch-runs/<config stem>-<cluster>/` for any other config (gitignored; `LAUNCH_RUN_DIR`
overrides): `state.json` (program id, mint, market, portfolios, completed
steps), `keys/` (generated on local and devnet; on mainnet the owner places
the hot `payer.json` and `lp.json`), `handover/` (mainnet), `bots.log`,
`monitor.log`, `pids/`, `timings.txt`. A run dir whose handover is done
refuses further setup steps. The drill and `staging_local.sh` delete the
validator ledger at exit unless `KEEP_LEDGER=1`.

**Resume.** Re-run the same command. Completed steps are skipped (state
file); the failed one is retried. Each step also checks the chain first:
existing accounts are not recreated, and `InitMarket`, the policies, the
insurance seed, the backing top-up and the deposits are skipped when the
market already shows them (control sequences, bucket status, portfolio
capital). Bots already running are left alone. A run dir is bound to one
program id, one chain (genesis hash) and one collateral mint; anything else
needs a fresh run dir. `state.json` is written atomically. The LP and trader
portfolio keys are saved in `keys/` before anything is sent, and creation +
`InitPortfolio` is one transaction, so a crash (or a lost `state.json`)
resumes on the same portfolio and never deposits twice. Restarted bots read
the trader's open position back from the chain. Mainnet is decided by the
RPC's genesis hash inside the binary too, so a direct `staging step|bots`
call with a mislabeled cluster is refused.

**Safety.** Mainnet needs a non-empty `program_id`, `--mainnet-i-understand`
and an RPC whose genesis is mainnet's; local and devnet refuse an RPC whose
genesis is mainnet's. Fee shares, bounty paths and the keeper's withdrawal
test stay off. Never run devnet or mainnet from an agent session.

## Mainnet: launch key, then hand over

Option A (owner decision; `ops/KEYS.md`, "Launch key"). A fresh launch key
signs steps 1-5 locally, as on local and devnet: create + `InitMarket` in
one transaction (nobody can initialize our account in between; an existing
market is accepted only with our launch key as marketauth and our mint,
else "market taken"), then the policies, insurance, backing and LP. Before
steps 3-4 the owner funds the launch key's USDC account with the insurance
seed and backing (`CAPITAL_PLAN.md`).

Step 6, `handover`, follows the split order in `ops/KEYS.md`. Each role
change needs the NEW holder's signature (the wrapper's
`expect_incoming_authority`). On local and devnet the holders' keys are
generated in `keys/` (marketauth goes to a stand-in `keys/multisig.json`) and
everything is sent. On mainnet the holders are `[cluster.mainnet.roles]` and
`multisig_vault` (public keys), and the step writes `handover/`:

- `01`-`05-<role>.b58`: `UpdateAssetAuthority`, already signed by the payer
  and the launch key, each on its own durable nonce (authority: the launch
  key; `AdvanceNonceAccount` first), so it does not expire while a hardware
  wallet or Squads co-signs;
- `06-marketauth.b58`: `UpdateAuthority` for a Squads vault transaction
  (vault co-signs as the new marketauth; the launch key must also sign the
  execute);
- `README.md`: who co-signs each, and the order (each binds the next
  authority epoch).

It exits 10; re-running checks the chain, rewrites what is still pending,
and completes only when the role table is clean. Bots refuse to start until
then (on every cluster), and use only role keys that hold their role on
chain: the pusher's `keys/oracle_authority.json` is required; backing and
insurance run only where their key is (not on mainnet, where those are
hardware wallets and the vault).

Rehearsed locally with `LAUNCH_REHEARSE_MAINNET_PATH=1` (the mainnet code
path on a non-mainnet chain; refused on mainnet's genesis): the five role
files co-signed and landed, the marketauth one signed by a stand-in vault key
and the launch key, the table came out clean, bots started with only the
pusher key, a re-used market key was refused as "market taken", and a
handed-over run dir refused further steps.

*To verify on devnet before Day 0* (same flag): Squads import and execute
of `06-marketauth.b58` with the launch key as an extra signer; hardware
wallet co-signing of a pre-signed durable-nonce transaction (tooling);
`insurance_authority` held by the vault co-signing through a vault
transaction.

Not automated:

- **A matcher for external takers.** Our matcher is the rehearsal one: LP and
  our own test trader co-sign `TradeNoCpi`. Serving outside takers needs a
  `TradeCpi` matcher (upstream's `auth_matcher` fixture is the candidate,
  R2 reuse survey) or a co-signing RFQ service. Not built.

## Pusher latency and the matcher

The money case holds only with the posted mark within about **400 ms** of
the market (`research/replay/REPORT.md`). Config: `oracle.lag_target_ms`
(400). The pusher has its own thread and RPC client, polling every
`oracle.push_poll_ms` (400); it pushes at `push_min_move_bps` (1) or the
slot heartbeat. `oracle.push_interval_ms` remains the main bots tick (3,000).
Pushes and accrual use `priority_fee_micro_lamports` (10,000). Pushes use
`push_cu_limit` (14,000, about 2× the measured 6,596 CU local landed push);
accrual requests 1,400,000 CU.

The pusher also checks asset-0 accrual each poll. Devnet bot ticks can take
40–50 seconds, while the engine advances at most 32 slots per crank and
`PushAuthMark` never accrues. When the gap exceeds `oracle.accrual_lag_slots`
(default 32), a separate transaction sends up to `oracle.accrual_max_cranks_per_tx`
(default 10) Live cranks. Crank failures cannot revert a push; the `accrual`
role logs and counts them without triggering an abnormal exit. Accrual skips
preflight and polls processed status with a three-second deadline, including
one retry at half the crank count on `EngineNonProgress`. Trades refuse
the tick with `accrual_behind` while the snapshot exceeds `oracle.accrual_lag_slots`.

- **Measured lag** = source price time → push landed (processed, polled
  every 50 ms; the push skips preflight). Mock: the time the price was
  generated, so it is our pipeline only (local p50 ~510 ms with 400 ms
  slots). Pyth: the account's `publish_time`, whole seconds, so it reads up
  to 1 s high.
- The pusher logs `{"event":"mark_lag","ms":…}` per push; the monitor
  exports `percolator_keeper_mark_lag_ms` and rolling nearest-rank
  `percolator_keeper_mark_lag_p50_ms` / `percolator_keeper_mark_lag_p95_ms`
  over the last 300 pushes. `PusherMarkLagHigh` alerts on p95 > 400 ms for 5 min.
  The lag gauge keeps its last value if pushes stop, so the monitor also
  exports `percolator_keeper_mark_push_age_seconds` (from the event's
  `at_ms`), and `PusherMarkStale` fires when no push has landed for 2 min.
  The health check prints p50 and max over 5 pushes; over target it warns
  (`LAUNCH_LAG_STRICT=1` fails).
- **Price source:** `price_source = "exchanges"` is the devnet default until
  a paid Pyth plan exists (about $500/month, a pre-mainnet owner decision).
  Hermes has required a paid key since 2026-08-26. One reconnecting thread
  per venue reads Coinbase Exchange SOL-USD `level2_batch` (reported as
  `level2_50`), Kraken v2 SOL/USD ticker with `event_trigger: bbo`, and Binance
  SOLUSDT `ticker` (every second). Prices are best bid/ask midpoints. Binance
  USDT is treated as USD, without conversion. No exchange keys are used.
  The Binance websocket URL is `oracle.exchange_binance_url`, defaulting to
  `wss://stream.binance.com:9443/ws/solusdt@ticker`. Old `@trade` URLs warn
  but still work; `@bookTicker` URLs are rejected. An `exchange_max_age_ms`
  below 5,000 warns because quiet books can stall.
  `oracle.exchange_max_age_ms` (5,000) gives quotes 5 seconds of validity;
  all venues use event time;
  future timestamps tolerate `exchange_clock_skew_ms` (500 ms). Prices farther than `exchange_max_dev_bps` (50)
  from the median of all fresh venues are excluded; the remaining median
  (midpoint for two) needs `exchange_min_venues` (2). If exactly one venue
  is fresh, it may supply a fallback within `exchange_fallback_max_dev_bps`
  (25) of the last quorum median for `exchange_fallback_ms` (10,000 ms).
  Fallback never refreshes that window; 0 disables it. Two or more fresh
  venues that disagree never fall back.
  Otherwise missing quorum skips pushes without counting a push failure;
  `PusherMarkStale` catches staleness.
  Dropped venues and reasons log only when the filter state changes.
  Lag runs from the **newest exchange timestamp among venues used** to push
  landed. It does not represent every venue's age.
  Fallback pushes add `fallback: true` to `mark_lag`. `price_gap` logs closed
  gaps of at least 2 seconds between successful reads. `price_stall_stats`
  updates every 60 seconds and includes an ongoing gap. It reports the longest
  gap and percent of the rolling hour spent in gaps longer than 10 seconds,
  using a minimum 10-minute denominator during warm-up. `PusherPriceStalls`
  uses these stats and warns when that percent exceeds 1 for 15 minutes.
- `"mock"` remains local's seeded walk; mainnet stays `"pyth"`, reading the
  SOL/USD PriceUpdateV2 account (`oracle.pyth_price_account`) over RPC with
  owner, feed, age and confidence checks. The sponsored account was ~70 s old
  when checked (2026-10-29), too slow for the money case.
  `PriceSource::latest` remains the extension boundary for a later paid Hermes
  source; no Hermes client is built here.
- **Check prices without a chain:**
  `staging/target/release/staging prices launch/sol-perp.toml devnet` prints
  once per second: price e6, exchange publish time, age in ms, venues used,
  fallback status and their high-low spread in bps (or a quorum error).
  It polls every `push_poll_ms` (400 ms), logs closed `price_gap` events, and
  prints `price_stall_stats` every 60 seconds.
  It loads no keys, opens no RPC connection and submits no transactions.
  Stop with Ctrl-C. Public feed formats:
  [Coinbase level2](https://docs.cdp.coinbase.com/exchange/websocket-feed/channels),
  [Kraken ticker](https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/ticker),
  [Binance ticker](https://developers.binance.com/docs/binance-spot-api-docs/web-socket-streams).
- **The matcher refuses fills while the mark lags; keep it.** The engine
  refuses a risk-increasing trade while an asset's AuthMark target differs
  from its effective price (`trade_preflight_risk_gate`, engine `v16.rs`
  16153, `asset_has_target_effective_lag` 16013). The matcher never works
  around it: an open requires the last landed push's source publish age to be
  at most `oracle.open_max_mark_age_ms` (1,000), equal target/effective prices,
  and the asset cranked to the current slot in the same tx, and the mock pusher keeps each step
  inside `max_price_move_bps_per_slot`. With Pyth a larger move leaves the
  effective price walking behind, and opens are refused until it catches up.
  Do not remove this.
- **Matcher caps:** `max_fill_notional_usd` and `max_lp_inventory_usd` are
  both $100,000, valued at max(effective price, target price). The LP co-signs
  every fill; refusals log `trade=refused(reason)`
  without counting as failures. Inventory-reducing fills may exceed the inventory
  cap, but every fill must meet the ticket cap. REPORT.md finds 2.7–4.8 bps adverse
  selection above $100k; its inventory limit and `ops/CAPITAL_PLAN.md` set inventory
  at 10% of $1M/day. There is no on-chain OI cap at this pin: this matcher cap
  is the OI cap for fills it signs.

## Drill

```bash
scripts/day0_drill.sh            # DRILL_SKIP_SBF_BUILD=1 reuses the .so
```

A fresh validator (port 8699), the wrapper deployed with `solana program
deploy` to a newly generated program id ("upstream publishes a program ID"),
that id written into a temp copy of the config, `launch_market.sh` on local
(handover included), per-step and total times printed, everything stopped,
the ledger deleted (`KEEP_LEDGER=1` keeps it). Measured times: see
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
  `/etc/percolator/launch.env`) run the bots and monitor only.
  **Monitoring and alerting are supported through Compose only**:
  `deploy/alertmanager.yml` needs the token file and chat id that the
  Compose service prepares, and Prometheus scrapes `monitor:9184` on the
  Compose network. With the bots under systemd, run the whole monitoring
  stack from Compose: build the image once (`docker compose -f
  deploy/docker-compose.yml build bots`), then `docker compose -f
  deploy/docker-compose.yml up -d --no-deps monitor prometheus
  alertmanager` (the monitor reads the same
  run dir's `bots.log`), and leave `percolator-monitor.service` off.
  `percolator-staging@.service` stays for R2.
- Env: `LAUNCH_CLUSTER`, `LAUNCH_RPC_URL` (the paid RPC),
  `PERCOLATOR_PROGRAM_ID`, `LAUNCH_RUN_DIR`, `LAUNCH_UID`/`LAUNCH_GID`
  (owner of the run dir; Compose runs the bots and monitor as that user so
  the 0600 keys stay private), `TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`,
  and on mainnet, after the handover, `LAUNCH_MAINNET_I_UNDERSTAND=1`
  (default 0; the bots still check genesis and the on-chain roles).
  `deploy/.env` never enters the Docker build context (`.dockerignore`), and
  logs and error messages show the RPC host only, never its credentials.
- The run dir from the launch (state.json + keys/) is copied to the host.
  Mainnet: only the hot keys of `ops/KEYS.md`, placed on their host.
- **Keeper:** inside the bots process (`drip::cycle`, the keeper's own
  code). The `drip` binary itself boots its own market on 127.0.0.1:8899 at
  this pin, so it cannot crank an existing market; changing that would change
  the keeper.

**Fallback source on hosts that can't reach the exchanges.** Some networks
block the exchange endpoints; this one does at the ISP. Set
`price_source = "pyth"` with `oracle.pyth_rpc` pointing at a mainnet RPC.
The pusher then reads mainnet's sponsored Pyth SOL/USD account, read-only,
and pushes it on devnet. It's real Pyth, but about 5–10 s behind (Pyth's
sponsored update cadence), so it only keeps a devnet market live; it says
nothing about the 400 ms target. That's what the devnet market ran on
after the 2026-10-29 relist.

## Lifecycle drill

Run `scripts/lifecycle_drill.sh launch/sol-perp.toml local launch-runs/lifecycle` (or `devnet`).
Local needs a running validator with the wrapper deployed at the config's RPC.
Set `DRILL_PAYER=/path/to/payer.json` to fund both launches from the same payer.
The drill lists, checks a fill, stops bots, resolves, pays out, closes portfolios and the slab,
sweeps reclaimed SOL, then relists a fresh market and repeats the wind-down.
`DRILL_WIND_DOWN_RELIST=0` keeps the relisted market Live with bots stopped.
Each run writes `lifecycle.jsonl`; `timings.txt` records steps and net payer spend.
Resume cleanup with `staging/target/release/staging lifecycle <config> <cluster> <run dir>`; mainnet is refused.

## Key layout (owner decision, 2026-10-29)

One key, one job. Every key lives in the run dir's `keys/` (mode 0600), and no other
process signs with them.

| Key | Holds | Signs |
|---|---|---|
| payer | SOL for fees and rent; test-USDC mint authority on local and devnet | fees, rent, the faucet |
| launch key (`launch_authority` on mainnet, `admin` on local and devnet) | `marketauth` from create and init until the handover | setup steps 1-5, then the handover |
| multisig (Squads vault; `multisig.json` stand-in on local and devnet) | `marketauth` after the handover | resolve, policies, close |
| oracle (`oracle_authority`) | asset 0 oracle role; the matcher's price authority | ONLY the pusher: `PushAuthMark` plus `UpdateRef` in the same tx |
| bots' LP owner (`lp`) | the bots' LP portfolio (no matcher binding) | the bots' co-signed `TradeNoCpi` fills |
| matcher LP owner (`matcher_lp_owner`) | the matcher's own LP portfolio, bound to the matcher ctx | `SetMatcherConfig` on that portfolio only |
| trader (`trader`, local and devnet only) | the bots' test taker | test fills |
| insurance and backing role keys, asset admin | their asset 0 roles after the handover | their bots, or the owner |

The bots never trade or co-sign on the matcher's LP. A `TradeNoCpi` on it would
bump its position epoch and revoke the matcher's capability. The hedger nets
both LPs.

**Matcher reference age.** `[cluster.<name>].matcher_max_ref_age_slots` records the
value. It must stay at or above the pusher's landed-push gap p95, or the matcher refuses
fills as STALE.
- **Devnet: 80 slots.** The measured gap is p50 31 / p95 67 slots, because the pusher reads
  mainnet Pyth's sponsored account, which updates about every 10 s. Applied on 2026-10-29.
- **Mainnet target: 5 slots,** reachable only with a sub-second source. A fill priced off
  a stale reference is what latency arbitrage picks off (research/replay/REPORT.md).

Apply it by hand with the matcher LP owner key. **Known race:** the matcher's
`SetParams` rewrites every parameter, inventory included, and has no age-only
form. So a `TradeCpi` fill that lands between the read and the write is erased from
the matcher's inventory accounting. Pausing the frontend does NOT prevent this:
any client can send `TradeCpi` directly, and only the on-chain `paused` flag
stops fills. Setting that flag itself goes through `SetParams`. Treat this as a
devnet-only manual step, run when no taker is active, and re-check inventory
against the LP portfolio's position afterwards. An age-only (or
compare-and-set) matcher instruction would remove the race; that's a
matcher-program change for later. Read the current values first, and pass
them back unchanged except `--age`:

```bash
matcher_ops show --matcher <program> --ctx <ctx>
matcher_ops set-params --matcher <program> --payer <run>/keys/payer.json \
  --lp-owner <run>/keys/matcher_lp_owner.json --ctx <ctx> --fill <max_fill_abs> \
  --cap <max_inventory_abs> --age <new age> --deviation <max_mark_dev_bps> \
  --authority <price_authority> --paused <paused> --inventory <inventory>
```

`matcher_ops` targets devnet; it rejects mainnet URLs. An age-only matcher
instruction would remove the race; that's a matcher-program change for later.
