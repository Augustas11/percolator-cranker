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
  (JSON `null`) instead of approximated.
- `{component="receipt_face_remaining"}`: exact Σ `face − paid_effective`, the
  ceiling before the rate haircut.
- `{component="unreceipted_equity_upper_bound"}`: Σ `max(capital + pnl, 0)` for
  portfolios with no receipt. **An upper bound, not exact owed**: the resolved
  close (v16.rs:~19452) still settles fees and K/F, and the receipt's
  positive face is then paid at the haircut rate.
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

Load `monitor/alerts.yml` into Prometheus (group `percolator-monitor`).

| Alert | Threshold | Hold / severity |
|---|---|---|
| KeeperAccountUnrefreshable | `max by (market, account)` quarantined slots > 1500 (≈10 min at 400 ms/slot) | 1m / warning |
| UnownedCollateralDrift | abs(delta(unowned[1h])) > 1000 atoms; tune per market | 1m / warning |
| BackingBucketExpiringWithLiens | expiring with liens > 0 | 1m / warning |
| KeeperNoSuccessfulCrank | slots since last landed crank > 750 | 1m / warning |
| KeeperLogStale | log age > 120s or log read fails | 1m / critical |
| MonitorChainReadFailing | chain read fails | 5m / critical |
| MonitorDown | scrape `up == 0` or monitor metrics absent | 5m / critical |

The recording rule is `percolator:unowned_collateral_drift_atoms_per_hour =
delta(percolator_unowned_collateral_atoms[1h])`. Idle cranks count as
successful, so a quiet market does not trip KeeperNoSuccessfulCrank; progress
is tracked separately in `keeper_attempts_total`. The dashboard lists threshold conditions before Prometheus hold
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
    the engine's `validate_header_aggregate_totals` (v16.rs:8578), which treats
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

## Validation (filled in by integrator)
