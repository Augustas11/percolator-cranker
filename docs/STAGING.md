# Staging

**Reference / dry-run only; not a paid live service; M3 NO-GO.** Nothing is
deployed. Staging is a local operator rehearsal of one **SOL-PERP** market:
one asset, SOL priced in USD, test USDC (6 decimals) as collateral, every
operator role run by a bot, for hours, judged by an exit code.

Private: `staging/` and these scripts are never exported to the public repo.

## What runs

`scripts/staging_local.sh` starts its own `solana-test-validator` (RPC
`127.0.0.1:8799`, so it can run beside the M1 validator on 8899) with the
pinned wrapper `.so` loaded at genesis, then runs the `staging` binary
(`staging/src/main.rs`).

Params (market, amounts, matcher, pusher) come from `launch/sol-perp.toml`
(`STAGING_CONFIG` overrides), shared with the Day-0 launch; the trade fee is
12 bps there. Setup, once: a test USDC mint (6 decimals, the run's payer is mint
authority), `InitMarket` with one asset (values and reasoning in
`docs/OPERATOR_PLAYBOOK.md` section 2), `ConfigurePermissionlessResolve`,
`ConfigureAuthMark` at $150, and an LP and a trader portfolio with $30k each.

Then one tick every `STAGING_TICK_MS` (3 s). Instruction account lists and
call order mirror the upstream wrapper's test support
(`repo-prog/tests/support/v16_svm.rs`), named in code comments:

| # | Role | What the bot does |
|---|---|---|
| 1 | price pusher | Runs on its own thread and RPC, polling every `oracle.push_poll_ms` (400). Sends `PushAuthMark` when the heartbeat (`STAGING_PUSH_HEARTBEAT_SLOTS`, 25, about 10 s) elapses or the price moves at least `oracle.push_min_move_bps` (1). Opens require a landed push within `oracle.open_max_mark_age_ms` (1,000); the engine target/effective-price gate remains active. |
| 2 | LP matcher | Our matcher is the only LP. LP and trader co-sign `TradeNoCpi` for 10 SOL (about $1.5k, 5% of a deposit) at mark ± 10 bps. The trader alternates open and close, so open interest never exceeds 10 SOL a side. Cranks of both portfolios go first in the same tx when they would land (trades need the asset accrued to the current slot). |
| 3 | backing manager | Keeps backing buckets on domains 0 (long) and 1 (short): $10k each, expiry `STAGING_BACKING_TENOR_SLOTS` ahead (9_000, about 1 h). In the last quarter of the tenor it rolls: it holds the matcher at flat, withdraws the principal not backing open PnL claims (`WithdrawBackingBucket`), and tops up a new bucket once the old one is `Empty` or `Expired`. |
| 4 | insurance | `TopUpInsurance` of $10 every `STAGING_INSURANCE_EVERY` (30) ticks. |
| 5 | keeper | One real `drip::cycle::run_one_cycle` over the market, the same code as the keeper binary, with no reward portfolio. |

**Lifecycle, once per run** at `STAGING_LIFECYCLE_TICK` (default: halfway;
0 turns it off). Roles 1-4 pause; the keeper skips the one Recovery tick
(cranks of a shut-down asset are refused inside the force-close delay) and
cranks otherwise.

1. **Live → Recovery**: `UpdateAssetLifecycle` SHUTDOWN of SOL (asset
   lifecycle RECOVERY, mark frozen).
2. **Recovery → Resolved**: `ResolveMarket` by the market authority.
3. The keeper's resolved route closes both portfolios and pays the owners
   (after `force_close_delay_slots` = 20, no owner signature needed).
4. **Relisted**: a fresh SOL-PERP market at the last mark, new portfolios,
   new deposits. Trading resumes. Takes about 7 ticks.

A resolved market cannot go back to Live, so "relisted" is a new market
account. The old one is left resolved: its backing and insurance are not
withdrawn and it is not closed (`CloseSlab` is not exercised).

The market-wide `Recovery` *mode* is not exercised: the engine enters it
itself, from a crank, only when bounded progress is impossible (a stuck
bankrupt close). The operator cannot trigger it. The rehearsal uses the
asset-level RECOVERY that SHUTDOWN gives.

The market authority key signs the push, backing, insurance and lifecycle
steps (it holds every role at `InitMarket`). Keys are generated per run; the
payer key is `<run dir>/payer.json`. Nothing is committed.

## Price source

The pusher signs whatever `mock_sol_usd` returns: locally, a seeded random
walk around $150, each step at most `max_price_move_bps_per_slot` times the
slots since the last push (capped at 50 bps), kept within ±5%. It is the one
function to swap. On devnet the pusher would
read Pyth SOL/USD (feed id is a parameter of the devnet script) and push that
price; that read is built for the Day-0 launch (`price_source = "pyth"`,
`docs/LAUNCH.md`); this R2 run keeps the mock.

A direct Pyth profile (`ConfigureHybridOracle`, the wrapper reading the Pyth
account in each crank) is a later option, not built. It removes the pusher
as a trusted price, but liveness then depends on a feed we do not control,
and with permissionless resolve set a stalled feed lets anyone resolve the
market. AuthMark keeps the oracle role, its key and its stale alarm as a
rehearsed operator duty. The oracle mode can only change while the market is
flat, so the choice is made per market.

## Start and stop

```bash
./scripts/staging_local.sh                      # 6 hours (STAGING_HOURS=6)
STAGING_HOURS=0.17 ./scripts/staging_local.sh   # ~10 minute smoke run
```

Ctrl-C stops it; the validator is killed on exit and its `ledger/` deleted
(`KEEP_LEDGER=1` keeps it). Each run writes
`.staging-runs/<timestamp>/` (gitignored): `staging.log`, `validator.log`,
`ledger/`, `payer.json`.

Knobs (environment): `STAGING_HOURS`, `STAGING_TICK_MS`, `STAGING_SEED`,
`STAGING_TRADE_EVERY_SECS` (default 300 in bots mode, 0 = every tick),
a close needed for a backing roll is not delayed by this cadence;
`STAGING_BACKING_TENOR_SLOTS`, `STAGING_INSURANCE_EVERY`,
`STAGING_LIFECYCLE_TICK`, `STAGING_PUSH_HEARTBEAT_SLOTS`,
`STAGING_SUMMARY_EVERY` (100),
`STAGING_MAX_CONSEC_FAIL` (10), `STAGING_TRADE_FEE_BPS` (config `matcher.fee_bps`, 12),
`STAGING_USDC_MINT` (use an existing mint whose authority is the payer),
`STAGING_RPC_PORT`, `STAGING_RUN_DIR`. The keeper's own `CRANKER_*` budget and
quarantine knobs apply too.

## Reading the output

One line per tick:

```
t42 SOL=150231000 push=ok(480ms) trade=+10000000 backing=- insurance=- keeper=p2/i0
```

`SOL` is the mark in USD e6. `-` means the role had nothing to do.
`FAIL(<error>)` is a failed tx (the wrapper error name when known).
`keeper=pN/iM` is accounts progressed / idle in that crank cycle. Lifecycle
steps add `lifecycle=...`.

Every `STAGING_SUMMARY_EVERY` ticks, and at the end, one JSON line:

```json
{"summary":"final","tick":7200,"elapsed_s":21600,"sol_usd_e6":150120000,
 "phase":"Live","market":"...","push":{"ok":..,"fail":..,"max_consec_fail":..},
 "trade":{..},"backing":{..},"insurance":{..},"lifecycle":{..},"keeper":{..},
 "crank":{"progressed":..,"idle":..,"failed":..},"insurance_balance":..,"vault":..}
```

- `fail` counts failed attempts per role; `max_consec_fail` is the longest
  run of failures in a row. `lifecycle.ok` is 3 after a full lifecycle.
- `crank.failed` counts failed crank attempts inside the keeper cycles. Any
  failed or unconfirmed crank transaction, and any RPC error that ended a
  drain, counts as a keeper failure (`keeper.fail`), even inside an attempt
  that also made progress.
- `insurance_balance` and `vault` are the current market's balances in USDC atoms.

**Exit code.** 0 when the run reached its end normally and, unless
`STAGING_LIFECYCLE_TICK=0`, the lifecycle completed (SHUTDOWN is retried
from its tick on until it lands; SOL must be relisted before the end). 1 as
soon as a role fails `STAGING_MAX_CONSEC_FAIL` times in a row, or more than
5% of its attempts (after 20), or when the lifecycle did not complete, so an unattended run is judged by the exit code and the
final summary line. Trade failures, like accrual failures, are counted but
never end the run. The script prints that line again at the end.

## Known upstream issues

From `cranker/scenario-findings.md`:

- **F1, lapsed unreferenced backing** (upstream #243). A `Fresh` bucket past
  expiry that no portfolio references wedges refresh and liquidation in Live.
  The backing manager rolls before expiry to avoid it. A bucket whose
  principal still backs open PnL claims is referenced by the claim holders,
  and the keeper's crank expires it; fills on the asset return `EngineStale`
  until then, so the matcher skips those ticks. If an unreferenced one is ever seen past
  expiry, the backing role fails with "(F1 shape)" and the run exits 1.
- **Live backing withdrawals are gated.** `WithdrawBackingBucket` returns
  `EngineLockActive` while any account has negative PnL, while the asset has
  open interest and its effective price lags the pushed mark, or when the
  domain would drop below full backing of the positive PnL sourced from it.
  So the roll waits for a flat book and withdraws only the principal above
  the open claims; the rest lapses at expiry (a small, documented provider
  loss in test USDC).

## Hosting recipe

`deploy/systemd/percolator-staging@.service` runs one binary per instance as
a sandboxed service: `percolator-staging@drip` for the keeper,
`percolator-staging@staging` for the staging bots.

```bash
cargo build --release --manifest-path cranker/Cargo.toml
cargo build --release --manifest-path staging/Cargo.toml
sudo install -D cranker/target/release/drip     /opt/percolator/bin/drip
sudo install -D staging/target/release/staging  /opt/percolator/bin/staging
sudo install -D -m 0644 deploy/systemd/percolator-staging@.service /etc/systemd/system/
# /etc/percolator/common.env: PERCOLATOR_PROGRAM_ID=..., STAGING_RPC_URL=...
# /etc/percolator/keys/<instance>.json: that bot's keypair, root-owned, 0600
sudo systemctl daemon-reload
sudo systemctl enable --now percolator-staging@staging
journalctl -u percolator-staging@staging -f
```

Notes:

- The keypair reaches the service as a systemd credential, linked to
  `$HOME/.config/solana/id.json` (where `drip` reads it) and named by
  `STAGING_PAYER` (where `staging` reads it).
- `drip` at this pin talks to `127.0.0.1:8899` and bootstraps its own
  market, so on a host it runs against a local validator only.
- `staging` exits when `STAGING_HOURS` ends; the unit restarts it only on
  failure.

## Devnet

Superseded by the Day-0 launch kit (`docs/LAUNCH.md`): `scripts/launch_market.sh
launch/sol-perp.toml devnet` replaces `staging_devnet_deploy.sh`, and the
devnet pusher reads the on-chain Pyth SOL/USD account. Not run yet; owner-run.

- `scripts/staging_usdc_faucet.sh <wallet> <usdc>`: mints test USDC to a
  tester's wallet (creating their token account); reads the config and the
  devnet launch run dir; `--yes` acts; per-call cap `FAUCET_MAX_USDC`.
  Refuses mainnet by URL and by genesis hash.
