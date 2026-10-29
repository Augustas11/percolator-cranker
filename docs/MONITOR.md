# Private market monitor

`monitor/` is a private, read-only liveness and solvency monitor; never export
it in the public distribution. The monitor never signs or sends transactions.
It reads one market, its portfolios and optionally drip's existing stdout log.
The separate `fault-proxy` binary is a test-only transaction forwarding hook.

## Run

```bash
source scripts/env.sh
cargo build --release --manifest-path monitor/Cargo.toml
# Start the keeper as usual, capturing both output streams:
cargo run --release --manifest-path cranker/Cargo.toml --bin drip > keeper.log 2>&1
# Another shell (source scripts/env.sh there too):
monitor/target/release/monitor --program "$PERCOLATOR_PROGRAM_ID" \
  --keeper-log keeper.log --listen 127.0.0.1:9184
# One snapshot, including chain_error on an RPC failure, then exit 0:
monitor/target/release/monitor --program "$PERCOLATOR_PROGRAM_ID" \
  --market <MARKET> --json
```

### Devnet

One command, from the checkout whose `launch-runs/devnet/` holds the launch
(`state.json` and `bots.log` from `scripts/launch_market.sh <config> devnet`). Read-only:
no keys.

```bash
R=launch-runs/devnet; monitor/target/release/monitor --rpc https://api.devnet.solana.com \
  --program "$(jq -r .program_id $R/state.json)" --market "$(jq -r .market $R/state.json)" \
  --keeper-log $R/bots.log --listen 127.0.0.1:9184 --interval-ms 10000 --expiry-warn-slots 15000
```

Open `http://127.0.0.1:9184/` for the static dashboard (it reads
`/snapshot.json`), or `curl -s 127.0.0.1:9184/metrics`. Without the bots
log, drop `--keeper-log` and only chain metrics remain. Pick a free port if
`launch_market.sh` already runs its own monitor on 9184.

- **Rate limits.** One poll makes three chain reads: `getSlot`,
  `getAccountInfo` on the market, and `getProgramAccounts` (full data, about
  9.5 KB per portfolio). A `getSignatureStatuses` lookup is added only until
  a new crank signature resolves. 10 s keeps that far below the public
  endpoint's per-IP limits. A 429 or timeout on a chain read is a nonfatal
  `chain_error`, and MonitorChainReadFailing waits 5 min. A failed signature
  lookup is just retried on the next poll.
- **Slot time.** Devnet measured ~240 ms/slot (`getRecentPerformanceSamples`,
  2026-10-29), not 400. The slot thresholds fire sooner in wall time:
  750 slots ≈ 3 min, 1500 ≈ 6 min. That is the safe direction, so they
  stay. `--expiry-warn-slots 15000` keeps the bucket warning at about an hour.
- **Paging.** The market is read on its own. `getProgramAccounts` (filtered
  to portfolio accounts, then to this market locally) returns all of our
  program's portfolios in one unpaged response, so no paging is needed on
  devnet. On upstream's shared devnet program id it would also return other
  markets' portfolios.

`--program` defaults to `PERCOLATOR_PROGRAM_ID`. Supply `--market` or a
`--keeper-log` containing a boot line `market            = <pubkey>`; the last
complete matching line wins. Defaults: `--rpc http://127.0.0.1:8899`,
`--interval-ms 5000`, `--expiry-warn-slots 9000`. No log is required when
`--market` is explicit. Bind privately: non-loopback listeners warn on stderr.

| GET / HEAD endpoint | Content |
|---|---|
| `/` | Embedded dashboard, no external resources; polls every five seconds |
| `/snapshot.json` | JSON snapshot, including errors and bucket/asset details |
| `/metrics` | Prometheus text 0.0.4 |

HTTP connections close after one response; other methods get 405 and other
paths get 404. Chain errors are nonfatal: `chain_read_ok=0`, JSON `chain_error`
contains the error, and the last successful chain data (if any) remains stale.
Keeper data continues to refresh independently. Missing keeper data is omitted
when no log is configured. Unreadable logs retain their previously parsed data.
Slot, market, and portfolio RPC reads use confirmed commitment, but are separate
requests, not an atomic multi-account snapshot.

## Metrics and pinned sources

All metrics below have prefix `percolator_` and a `market` label. Citations
refer to engine `src/v16.rs` at **4db11a8c** and wrapper
`src/v16_program.rs` at **5cb331dd**. Field names/lines are verified at those
pins. Counters are identified below; other families are gauges.

| Metric suffix (additional labels) | Source / meaning |
|---|---|
| `accounts_total` | Engine header `materialized_portfolio_count`, v16.rs:8133 |
| `accounts_discovered` | Discovered portfolios with matching market and layout 18 (separate RPC read; can differ briefly) |
| `accounts_decode_errors` | Matching portfolios rejected by wrapper `read_portfolio`, v16_program.rs:2881 |
| `accounts_out_of_date{kind}` | `stale_certificate`: header `stale_certificate_count`, v16.rs:8134; `b_stale`: `b_stale_account_count`, v16.rs:8135; `kf_stale`: sum of asset long/short counters, v16.rs:4926–4927 |
| `asset_kf_stale_accounts{asset,side}` | Individual `stale_account_count_long/short`, v16.rs:4926–4927 |
| `backing_buckets{status}` | Stored empty/fresh/expired/impaired status; enum v16.rs:4304, bucket v16.rs:5071 |
| `backing_buckets_expiring{window_slots}` | Fresh with expiry ≤ current slot + warning window, including past expiry |
| `backing_buckets_expiring_with_liens{window_slots}` | Same set with `valid_liened_backing_num > 0`, v16.rs:5074 |
| `market_mode{mode}` | Live/resolved/recovery, one hot; enum v16.rs:4293 |
| `asset_lifecycle{asset,lifecycle}` | Current lifecycle = 1 for each non-disabled asset; enum v16.rs:4283 |
| `asset_funding_last_accrual_slot{asset}` | `AssetStateV16.slot_last`, v16.rs:4905 |
| `resolved_claimants_unpaid` | Capital > 0, positive PnL, or present unfinalized receipt |
| `resolved_owed_atoms{component}` | Components described below |
| `engine_header_atoms{term}` | All six collateral terms from raw header, v16.rs:8120–8129 |
| `residual_atoms` | Engine residual, v16.rs:9061 |
| `unowned_collateral_atoms` | Signed residual minus positive PnL total |
| `senior_shortfall_atoms` | max(senior claims − vault, 0) |
| `chain_slot` | Confirmed RPC slot |
| `monitor_chain_read_ok` | 1 on successful chain read, 0 otherwise |
| `monitor_snapshot_unix_seconds` | Poll time |
| `keeper_log_ok`, `keeper_log_age_seconds` | Configured log read success; now minus file mtime |
| `keeper_last_cycle`, `keeper_last_cycle_cu` | Latest complete summary |
| `keeper_last_cycle_attempts{outcome}` | progress/idle/failed/quarantined_skipped/deferred |
| `keeper_attempts_total{outcome}` | Counter: summary totals, progress/idle/failed |
| `keeper_cycles_total{outcome}` | Counter: progress if progressed > 0, failed if no progress and failed > 0, idle otherwise |
| `keeper_crank_cu{stat}` | Nearest-rank p50 and max of last 256 outcome lines with `Some(CU)` |
| `keeper_quarantined_accounts` | Accounts struck without subsequent observed Progress |
| `keeper_account_quarantined_slots{account,error,strike}` | Current chain slot minus first quarantine event slot |
| `keeper_last_success_slot` | Slot of the latest landed crank signature (`getSignatureStatuses` with history), absent until known |
| `keeper_mark_lag_ms` | Day-0 bots log only: the pusher's last `mark_lag` event, source price time to push landed (docs/LAUNCH.md); latest sample only |
| `keeper_mark_lag_p50_ms`, `keeper_mark_lag_p95_ms` | Nearest-rank p50/p95 source-to-landing lag over the last 300 pushes; `PusherMarkLagHigh` uses p95 > 400 ms |
| `keeper_mark_push_age_seconds` | Seconds since the last landed push (or monitor watch start before the first push); drives `PusherMarkStale` |
| `keeper_slots_since_last_success` | Chain slot minus that slot, saturating at zero. Any landed crank (`Ok(mut)`, `Ok(no-mut)`, `Ok(unreconciled)`) counts as successful |

Bucket JSON lists every non-empty bucket, with expiry, signed slots to expiry
and exact lien numerator. Domain `d` maps to asset `d/2`, even = long, odd =
short, as engine `domain_asset_side`, v16.rs:9076. Status is stored engine
status, not reclassified by wall-clock expiry.

Resolved metrics are zero outside Resolved mode. Wrapper `read_portfolio`
returns the engine Pod (v16_program.rs:217,2881), exposing capital/PnL
(v16.rs:20599) and `resolved_payout_receipt` (v16.rs:20627).
Receipt fields are in `ResolvedPayoutReceiptV16Account`, v16.rs:20510; the
payout rate is in the market's `resolved_payout_ledger` (wrapper mirror
v16_program.rs:318, engine `ResolvedPayoutLedgerV16`, v16.rs:5954).
- `resolved_claimants_unpaid`: open receipts (present, not finalized), plus
  portfolios with no receipt and `capital + pnl > 0`.
- `resolved_owed_atoms{component="receipt_claimable_now"}`: **exact**. Σ
  `floor(face · rate_num / rate_den) − paid_effective` over open receipts, as
  the engine's `resolved_receipt_claimable_against_ledger` (v16.rs:18365). It is
  0 while `payout_halted` (the engine refuses claims then). The engine's wide
  mul-div is private at the pin, so the monitor uses an exact u128
  decomposition; if an intermediate would overflow, the metric is **omitted**
  (JSON `null`) instead of approximated. That happens roughly once
  `face × residual` exceeds ~3.4e26 atoms² (e.g. a 1e13-atom claim against a
  ~3.4e13-atom residual), i.e. on large markets; `receipt_face_remaining` is
  still reported then.
- `{component="receipt_face_remaining"}`: exact Σ `face − paid_effective`, the
  ceiling before the rate haircut.
- `{component="unreceipted_stored_equity"}`: stored equity before settlement,
  Σ `max(capital + pnl, 0)` for portfolios with no receipt. **Not exact owed and
  not a bound**: the resolved close (v16.rs:~19452) still settles fees and K/F,
  which can move it either way, and the receipt's positive face is then paid at
  the haircut rate. The unpaid-claimant count uses the same stored values.
  If a receipt's `paid_effective` exceeds its rate-adjusted face (two reads mixed),
  `receipt_claimable_now` is omitted rather than reported as 0.
- `resolved_payout_halted`: ledger `payout_halted`.
Decode failures are
counted and excluded from sums; inspect `accounts_decode_errors` alongside them.

## Exact collateral calculation

```text
fresh    = source_fresh_backing_total_num / BOUND_SCALE  (floor)
senior   = c_tot + insurance + backing_provider_earnings_total + fresh
residual = max(vault − senior, 0)
unowned  = residual − pnl_pos_tot                         (signed i128)
```

Senior addition and residual subtraction saturate exactly as engine
`residual()` (v16.rs:9061); fresh principal uses v16.rs:9051 and
`BOUND_SCALE = 10^12` (engine `src/lib.rs:25` at 4db11a8c).
The roadmap's `vault − c_tot − insurance − pnl_pos_tot` omits two senior
claims: provider earnings and fresh backing principal. We report the engine-exact
formula; every term is exact, with no approximation. Unowned can be negative.

The host mirror lacks the fresh total. All six terms therefore come from one
`bytemuck::pod_read_unaligned::<MarketGroupV16HeaderAccount>` read, whose type
is v16.rs:8116, at `MARKET_GROUP_OFF` for its exact size
(v16_program.rs:58,62). No unsafe casts or approximate reconstruction are used.
Unrepresentable signed results or overflowing resolved sums produce a chain
error instead of silently wrapping.

JSON stores all u128/i128 values as strings. Prometheus text emits decimal
integers, but Prometheus ingests f64 and can lose precision above 2^53.

## Keeper log mapping

The incremental tail consumes complete lines only, remembers its byte offset,
and resets parsed state/counters when the file shrinks. Totals cover the
observed log, not keeper history before that file. Replacing a file without
shrinking it is not detected; use truncation for rotation.

Summary fields map directly to totals and the latest cycle. Any landed crank
line (`Ok(mut)`, `Ok(unreconciled)`, `Ok(no-mut)`; `cranker/src/main.rs`
`log_outcome`) clears that account's observed quarantine, mirroring drip:
Progress calls `Quarantine::record_ok` and Idle calls `Quarantine::record_idle`,
and both remove the strike entry (`cranker/src/quarantine.rs`). A
`HardReject (EngineNonProgress)` line also clears it, since drip classes it as
Idle (`submit::is_non_progress`, reused here). Any other `HardReject` keeps the
account and updates its last error. JSON quarantine events keep the first strike slot and update
error, strike, until-cycle and last-event slot. JSON `unconfirmed` and
`idle_with_pending_work` events are kept in `keeper.last_events`; other event
types are ignored.

The latest landed-crank signature is resolved to a slot with
`getSignatureStatuses` (history search), retried each poll until found, then
cached. Until a success is known, `keeper_slots_since_last_success` counts
from the first chain slot the monitor saw with this log, so a keeper that fails
from its start still trips the alert. Log errors do not abort chain collection. Monitor restarts and log
truncation reset log counters. The keeper needs no change: it already prints
every line used here.

## Alerts

Load `monitor/alerts.yml` into Prometheus (group `percolator-monitor`). Name
the scrape job `percolator-monitor`: MonitorDown matches `up{job="percolator-monitor"}`.

| Alert | Threshold | Hold / severity |
|---|---|---|
| KeeperAccountUnrefreshable | `max by (market, account)` quarantined slots > 1500 (≈10 min at 400 ms/slot) | 1m / warning |
| UnownedCollateralDrift | abs(delta(unowned[1h])) > 1000 atoms; tune per market | 1m / warning |
| BackingBucketExpiringWithLiens | expiring with liens > 0 | 1m / warning |
| KeeperNoSuccessfulCrank | slots since last landed crank > 750, Live mode only | 1m / warning |
| KeeperLogStale | log age > 120s or log read fails | 1m / critical |
| PusherMarkLagHigh | p95 `mark_lag` over the last 300 pushes > 400 ms | 5m / warning |
| PusherMarkStale | no push landed for > 120 s (`keeper_mark_push_age_seconds`) | 2m / critical |
| MonitorChainReadFailing | chain read fails | 5m / critical |
| MonitorDown | scrape `up == 0` or monitor metrics absent | 5m / critical |
| MonitorSnapshotStale | snapshot older than 60 s (hung poll loop) | 2m / critical |

The recording rule is `percolator:unowned_collateral_drift_atoms_per_hour =
delta(percolator_unowned_collateral_atoms[1h])`. Idle landed cranks count as
successful, so a quiet Live market does not trip KeeperNoSuccessfulCrank;
progress is tracked separately in `keeper_attempts_total`. The alert is gated
to Live mode: in Resolved mode drip may send nothing (owner window) or get only
`EngineNonProgress` rejects, which land no transaction. KeeperLogStale covers
a stopped keeper in every mode. The 1000 atoms/h drift default is a
placeholder: trading moves `pnl_pos_tot`, so set it per market before relying
on it. The owner-window skip prints nothing, so it cannot clear a quarantine
entry; drip's next landed crank or `EngineNonProgress` reject does. The dashboard lists threshold conditions before Prometheus hold
times. Its hourly drift uses an hour of local browser samples and boundary
interpolation; Prometheus remains authoritative for scrape-based delta and
alert hold durations. Threshold constants match the rules.

## Prior art

Checked read-only on 2026-10-28: upstream engine and wrapper, their
contributors' public repos, and Prometheus exporter and alerting conventions.
- **Reused:** the wrapper decoders `read_market` and `read_portfolio`, drip's
  `discover_portfolios`, and the engine's `BOUND_SCALE` and Pod types. The
  formula is the engine's own `residual()`. It is private (v16.rs:9061), so it
  is restated here, not reimplemented differently.
- **Adopted patterns:**
  - Unit suffixes (`_atoms`, `_slots`, `_seconds`), `_total` only on counters,
    and gauges for engine counts that can fall.
  - A signed unowned value plus a separate `senior_shortfall_atoms`, matching
    the engine's `validate_header_aggregate_totals` (v16.rs:8579), which treats
    senior claims above vault as invalid state.
  - `for:` holds, severity labels, and a monitor-down alert.
- **Rejected:**
  - Third-party Percolator keepers and indexers target pre-v16 program models,
    so there is nothing to reuse.
  - Upstream has no monitoring or decoder tooling beyond layout dumpers.
  - Other perps keepers' metrics cover bot liveness, not this engine's
    solvency.
- **Deliberate exception:** `keeper_account_quarantined_slots` carries an
  `account` label. Its cardinality is bounded by the quarantine list, and the
  account is the point of the alert.

## Fault proxy (test only)

```bash
monitor/target/release/fault-proxy --listen 127.0.0.1:9185 \
  --upstream http://127.0.0.1:18899 --faults ./fault-accounts
# Point --upstream at a local test validator (any port). Run drip through the proxy:
env -u NO_PROXY -u no_proxy HTTP_PROXY=http://127.0.0.1:9185 \
  cargo run --release --manifest-path cranker/Cargo.toml --bin drip > keeper.log 2>&1
```

The fault file lists one pubkey per line; missing file means no faults.
It is re-read per request. Only `sendTransaction` is inspected: base64 decode,
bincode deserialize, compare all transaction account keys. Matching requests
get preflight error -32002, InstructionError Custom(4242), with Solana 1.18's
simulation-result fields. Batches and nonmatching requests forward unchanged.
The proxy ignores the absolute request target host and forwards to the explicit
plain-HTTP upstream. Remove an account from the fault file to stop injection.
This binary belongs only in isolated tests; it does forward transactions.

## Validation

Run on 2026-10-28 against a fresh local market: a `solana-test-validator`
on a private port with the wrapper at `5cb331dd` loaded, and unmodified `drip`
(`CRANKER_CYCLES=600`) bootstrapping its own market `A6eM…kbZM`: 2 portfolios,
Live, `vault = c_tot = 1,000,000`, all cranks `Ok(no-mut)`. drip ran through
`fault-proxy` (`HTTP_PROXY`), with the monitor polling every 2 s. No Prometheus
server was run. Each alert expression was checked by hand against captured
`/metrics`, and the rules file was YAML-parsed (`promtool` not installed; run
`promtool check rules monitor/alerts.yml` where it is).

**Baseline.** `accounts_total 2`, `accounts_discovered 2`, all out-of-date
counters 0, `market_mode{mode="live"} 1`, `unowned_collateral_atoms 0`,
`senior_shortfall_atoms 0`, `keeper_crank_cu{stat="p50"} 18039`,
`keeper_attempts_total{outcome="idle"} 38`, 0 failed.

**Fault 1: a forced-failing account.** At 18:56 one cranked portfolio
(`J3kAj…7BWfe`) was written to the fault file. drip quarantined it on the
injected preflight error (strike 1 at slot 1114, backed off and retried up to
strike 8). The monitor tracked it throughout. At 19:09:46:

```
percolator_keeper_quarantined_accounts 1
percolator_keeper_account_quarantined_slots{account="J3kAj…7BWfe",error="RPC response error -32002: Transaction simulation failed: injected fault [1 log messages]",strike="8"} 1521
percolator_keeper_attempts_total{outcome="failed"} 10
```

KeeperAccountUnrefreshable (`> 1500`) is **true**. The dashboard listed it
under Conditions. Every other condition was false: log age 2 s, 5 slots since
the last landed crank, chain read OK.

**Fault 2: a stopped cranker.** drip was killed at 19:10:08, with the last
landed crank at slot 2683.

| after kill | log age (s) | slots since landed crank |
|---|---|---|
| +6 s | 5 | 10 |
| +2m06s | **126** | 134 |
| +5m06s | 307 | 470 |
| +7m37s | 457 | **785** |

KeeperLogStale (`> 120 s`) became true at about +2 min, and
KeeperNoSuccessfulCrank (`> 750`, Live) at about +7.5 min. MonitorChainReadFailing
stayed false.

**Not covered here.** Buckets, Resolved claims and non-zero unowned collateral
stayed at their trivial values on this bootstrap market. They are covered by
unit tests only. Running the monitor against the SOL-PERP staging market is a
follow-up once that market lands.

### Devnet run (2026-10-29)

The SOL-PERP market `7p8DtcMH…zyHq` on program `GaXu3DXK…HKSHS`, monitored
with the devnet command above. The bots were paused, so the price feed was stale. Every
chain metric was checked against an independent decode of the raw
`getAccountInfo` / `getProgramAccounts` bytes, at offsets from `offset_of!` on
the pinned engine types. The two decodes matched:

| Metric | Value | On-chain source |
|---|---|---|
| accounts total / discovered | 2 / 2, then 4 / 4 after another session opened two portfolios | header `materialized_portfolio_count`; unfiltered `getProgramAccounts`, minus the market |
| out of date (stale cert / b_stale / kf_stale) | 0 / 0 / 0 | header counters; asset-0 long/short stale counts |
| asset mode | Live, asset 0 Active | header `mode` = 0; slot lifecycle = 2 |
| backing buckets | 2 Fresh, expiry 516282841, no liens; stored Fresh after expiry (no crank) | asset-0 `backing_long/short` |
| insurance | 45,978,524 atoms | header `insurance` |
| unowned collateral | 0 (vault 84,010,000,000 = c_tot 63,964,021,476 + insurance + fresh 20,000,000,000) | six header terms |
| resolved claimants | 0 (Live) | per-portfolio capital/PnL, no receipts |

Alerts with the bots paused (threshold conditions, by hand from `/metrics`):
KeeperNoSuccessfulCrank (8,554 slots since the last landed crank > 750,
Live), KeeperLogStale (log age 2,059 s), PusherMarkStale (2,087 s since the
last push), PusherMarkLagHigh (last lag 110,610 ms: Pyth's devnet account
was ~2 min old). BackingBucketExpiringWithLiens and MonitorChainReadFailing
stayed false. Before this change the dashboard showed only the first two.

Unit tests: `cargo test --release --manifest-path monitor/Cargo.toml` has 13
passing (10 monitor, 3 fault-proxy).
