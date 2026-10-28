# Monitor

`monitor` is a read-only liveness and solvency monitor for one market and the
keeper that cranks it. It never signs or sends a transaction.

- **On chain:** it reads the market account and every portfolio of the market
  (`getProgramAccounts`, the same header memcmp the keeper uses), then decodes
  them with the wrapper's decoders (`read_market`, `read_portfolio`).
- **Keeper side:** it reads the keeper's status file. Quarantine, crank
  outcomes and CU live only in the keeper process, so the keeper writes them
  to that file after every cycle.

Code: `crates/cranker/src/monitor/` (lib) and `crates/cranker/src/bin/monitor.rs`.
Assets: `crates/cranker/monitor/dashboard.html` (embedded in the binary with
`include_str!`) and `crates/cranker/monitor/alerts.yml`. In the private tree
the crate lives at `cranker/`.

## Run it

```bash
source scripts/env.sh
# keeper, with the status file on
CRANKER_STATUS_FILE=/tmp/keeper-status.json CRANKER_CYCLES=1000 \
  cargo run --release -p percolator-cranker --bin cranker
# monitor (another shell); the market pubkey is printed by the keeper
cargo run --release -p percolator-cranker --bin monitor -- \
  --market <MARKET> --program "$PERCOLATOR_PROGRAM_ID" \
  --status-file /tmp/keeper-status.json \
  --rpc http://127.0.0.1:8899 --interval-ms 5000 \
  --listen 127.0.0.1:9184 --expiry-warn-slots 9000   # these four are the defaults
# one snapshot, then exit
cargo run --release -p percolator-cranker --bin monitor -- --market <MARKET> --program <PROGRAM> --json
```

| Endpoint | Content |
|---|---|
| `/metrics` | Prometheus text format 0.0.4 |
| `/snapshot.json` | The same data as JSON, plus per-bucket and per-asset rows |
| `/` | Static dashboard. Plain JS that polls `snapshot.json` every 5 s. |

- The server is a small blocking HTTP/1.1 server on `std::net::TcpListener`
  that answers GET and HEAD only. Bind it to localhost.
- A poll thread refreshes the snapshot every `--interval-ms`.
- `--program` defaults to `PERCOLATOR_PROGRAM_ID`, and `--status-file` to
  `CRANKER_STATUS_FILE`.
- The monitor adds no crate dependencies. JSON is read and written by a
  small module (`monitor/json.rs`) that keeps integers exact.

### Keeper hooks (opt-in)

| Env | Effect |
|---|---|
| `CRANKER_STATUS_FILE=<path>` | The `cranker` binary rewrites `<path>` after every cycle: a temp file and a rename, so readers never see a partial file. Writing never changes a cycle or scheduler decision, and a write error is printed and ignored. |
| `CRANKER_FAULT_ACCOUNTS=<pk,pk,…>` | **Test hook.** The cycle reports `Failed { error: "InjectedFault" }` for these accounts and sends no transaction. They then go through the normal quarantine path. `@<file>` reads the list from a file and re-reads it every cycle, so a fault can be injected into a running keeper. Real failure paths are untouched. |

Status file fields (`version: 1`):

| Field | Meaning |
|---|---|
| `cycle`, `slot` | Last cycle number, and the slot it was planned at |
| `last_cycle` | The cycle summary: `progressed`, `idle`, `failed`, `quarantined_skipped`, `budget_deferred`, `newly_quarantined`, `cu_spent`, `max_attempt_cu` |
| `totals` | Cumulative counts. Attempts (one account in one cycle) by outcome. Cycles by class: **progress** (at least one account progressed), **failed** (none progressed and at least one failed), **idle** (anything else). |
| `cu_window` | Program CU of the last 256 submitted cranks |
| `quarantined[]` | Accounts the keeper holds struck: quarantined at least once and not yet cleared by a success. Each has its last `error`, `strike`, `until_cycle`, `blocked` (inside its backoff window) and `since_unix_ms` (when first struck). |
| `last_progress_slot`, `last_progress_cycle` | Last cycle with at least one Progress crank |

## Metrics

Every metric has a `market` label. Line numbers are engine
`aeyakovenko/percolator` at `4db11a8c` (`src/v16.rs`), and wrapper
`aeyakovenko/percolator-prog` at `5cb331dd` (`src/v16_program.rs`).

### Accounts

| Metric | Source |
|---|---|
| `percolator_accounts_total` | Program-owned portfolios whose `market_group_id` is this market and whose layout discriminator is 18 (`discovery.rs`) |
| `percolator_engine_materialized_portfolio_count` | Header `materialized_portfolio_count` (`MarketGroupV16HeaderAccount`, v16.rs:8116) |
| `percolator_accounts_out_of_date{kind,source}` | See below |
| `percolator_accounts_out_of_date_mismatch{kind}` | \|portfolio scan − engine counter\| for each kind |
| `percolator_accounts_keeper_estimate{priority}` | The keeper's off-chain ordering estimate (`work::assess`). This is a heuristic, not an engine definition. |
| `percolator_accounts_decode_errors` | Discovered portfolios that `read_portfolio` rejects |

**Out of date, by the engine's own fields.** There are three kinds. For each,
the monitor reports the portfolio-side scan (`source="portfolio"`) and the
engine's own counter (`source="engine"`), and reconciles the two through the
`_mismatch` gauge.

| kind | portfolio side | engine counter | engine source |
|---|---|---|---|
| `stale_state` | `PortfolioAccountV16Account.stale_state != 0` (v16.rs:20620) | header `stale_certificate_count` (v16.rs:8134) | `clear_account_stale` (v16.rs:13448) decrements the counter. At `4db11a8c` nothing sets `stale_state`, so both sides read 0. |
| `b_stale_state` | `b_stale_state != 0` (v16.rs:20621) | header `b_stale_account_count` (v16.rs:8135) | `mark_account_b_stale` (v16.rs:13772) and `clear_account_b_stale` (v16.rs:13787) move the flag and the counter together |
| `kf_stale_cohort` | active legs with `kf_epoch_snap < kf_epoch_{long,short}` of their asset | Σ `AssetStateV16.stale_account_count_{long,short}` (v16.rs:4926) | `kernel_mark_kf_stale_cohorts` (v16.rs:1075) sets the count to the stored position count when an epoch moves. `kernel_settle_kf_stale_cohort` (v16.rs:1122) decrements it as each leg settles. |

`kind="any", source="portfolio"` counts portfolios with any of the above, or
with a leg flagged `stale` / `b_stale`. Per asset and side, the
`kf_stale_cohort` pair is `percolator_asset_kf_stale_accounts{asset,side,source}`.
A nonzero mismatch for a whole poll means the scan and the engine disagree.
One-poll blips can occur because the scan and the market read are separate
RPC calls.

### Keeper (from the status file)

| Metric | Meaning |
|---|---|
| `percolator_keeper_status_ok` | 1 if the configured file parsed. The metric is absent when no file is configured. |
| `percolator_keeper_status_age_seconds` | now − `written_unix_ms` |
| `percolator_keeper_cycle`, `percolator_keeper_cycle_slot` | Last cycle, and its slot |
| `percolator_keeper_last_cycle_attempts{outcome}` | Last cycle: `progress`, `idle`, `failed`, `quarantined_skipped`, `budget_deferred` |
| `percolator_keeper_attempts_total{outcome}` (counter) | Attempts since keeper start: progress / idle / failed |
| `percolator_keeper_cycles_total{outcome}` (counter) | Cycles by class: progress / idle / failed |
| `percolator_keeper_crank_cu{stat="p50"\|"max"}` | Nearest-rank median and max over `cu_window` |
| `percolator_keeper_quarantined_accounts` | Struck accounts (quarantined or unrefreshable, not yet cleared) |
| `percolator_keeper_account_quarantined_seconds{account,error,strike,until_cycle}` | Per struck account: seconds since first struck. `error` is the last error code. |
| `percolator_keeper_last_progress_slot` | Slot of the last cycle with at least one Progress crank |
| `percolator_keeper_slots_since_last_progress` | chain slot − `last_progress_slot` |

### Backing buckets

Buckets come from the wrapper mirror, `MarketGroupV16.source_backing_buckets`
(engine `BackingBucketV16`, v16.rs:5071; statuses v16.rs:4304). Domain
`d = 2·asset + side`, with long = 0 (`domain_asset_side`, v16.rs:~9076). "Now"
is the RPC slot, at confirmed commitment.

| Metric | Meaning |
|---|---|
| `percolator_backing_buckets{status}` | Count by `fresh`, `impaired`, `expired`, `empty` |
| `percolator_backing_buckets_fresh_expiring_soon{window_slots}` | Fresh with `0 < expiry_slot − now ≤ window` (`--expiry-warn-slots`, default 9000) |
| `percolator_backing_buckets_fresh_past_expiry` | Fresh with `expiry_slot ≤ now`. This is the engine's lapse test, for example `validate_source_domain_ledger_current` (v16.rs:9202). |
| `percolator_backing_buckets_fresh_past_expiry_in_live` | Same, while the market is Live. **This is the known wedge condition.** |
| `percolator_backing_buckets_fresh_past_expiry_in_live_unreferenced` | Subset where no portfolio holds an occupied `source_domains` entry for the domain (`PortfolioSourceDomainV16Account::is_occupied`, v16.rs:20568). No Live instruction can expire such a bucket. The asset-level sweep `expire_lapsed_source_backing_for_asset_not_atomic` (v16.rs:19765) runs on the resolve/retire paths only. |
| `percolator_backing_buckets_expiring_with_liens{window_slots}` | Fresh with `valid_liened_backing_num > 0` and `expiry_slot − now ≤ window`, past expiry included. When it expires, liened backing becomes Impaired. |
| `percolator_backing_bucket_slots_to_expiry{asset,side}` | For each Fresh bucket: `expiry_slot − now` |
| `percolator_backing_bucket_valid_liened_num{asset,side}` | For each non-empty bucket, in `BOUND_SCALE` units |

### Modes

| Metric | Source |
|---|---|
| `percolator_market_mode{mode}` | `MarketModeV16` (v16.rs:4293): live / resolved / recovery. The current one is 1. |
| `percolator_asset_lifecycle{asset,lifecycle}` | `AssetStateV16.lifecycle` (`AssetLifecycleV16`, v16.rs:4283): disabled, pending_activation, active, drain_only, retired, recovery |

### Resolved claims (Resolved markets only)

| Metric | Exact? | Source |
|---|---|---|
| `percolator_resolved_claimants_unpaid` | exact count | Portfolios still open with `capital > 0`, or `pnl > 0`, or a receipt with `present && !finalized` |
| `percolator_resolved_receipts{state}` | exact | `ResolvedPayoutReceiptV16Account.present` / `.finalized` (v16.rs:20510) |
| `percolator_resolved_owed_atoms{component="receipt_face_remaining"}` | exact | Σ `terminal_positive_claim_face − paid_effective` over open receipts. This is the ceiling the engine enforces in `apply_resolved_payout_receipt_payment` (v16.rs:6041). |
| `…{component="receipt_claimable_now"}` | exact | Σ `floor(face · current_payout_rate_num / current_payout_rate_den) − paid_effective`. This is `resolved_receipt_claimable_against_ledger` (v16.rs:18365), using an exact 256-bit mul-div. It is 0 while `payout_halted`. |
| `…{component="capital"}` | exact field sum | Σ `capital` of open portfolios |
| `…{component="unreceipted_positive_pnl"}` | **face only** | Σ `max(pnl, 0)` of portfolios without a receipt |
| `…{component="negative_pnl"}` | exact field sum | Σ \|min(pnl, 0)\| |
| `percolator_resolved_payout_halted` | exact | Ledger `payout_halted` (v16.rs:20465) |

**What is not exact.** For a portfolio without a receipt, the amount it will
be paid is not computable from public state alone.

- The receipt's face is fixed when it is created
  (`create_resolved_payout_receipt_if_needed`, v16.rs:18316), after the close
  path settles K/F, fees and losses against capital.
- The payout rate can still move until the ledger finalizes.

So the monitor reports that portfolio's capital and positive PnL face as
upper-bound components, not as "owed". Open receipts are exact on both
counts: the face remaining, and what is claimable at the current rate.

### Unowned collateral

The definition is the engine's own. `residual()` (v16.rs:9061) is:

```
residual = vault − ( c_tot
                   + insurance
                   + backing_provider_earnings_total
                   + source_fresh_backing_total_num / BOUND_SCALE )      (saturating at 0)
unowned            = residual − pnl_pos_tot          (signed)
unowned_vs_bound   = residual − pnl_pos_bound_tot    (signed; junior_claim_bound(), v16.rs:9072)
```

| Term | Read from | Exact? |
|---|---|---|
| `vault`, `c_tot`, `insurance`, `pnl_pos_tot`, `pnl_pos_bound_tot` | Raw engine header (`MarketGroupV16HeaderAccount`, v16.rs:8116-8125) | yes |
| `backing_provider_earnings_total` | Raw header (v16.rs:8127). `backing_provider_earnings_total()` (v16.rs:9043) returns the field. | yes |
| `source_fresh_backing_total_num` | Raw header (v16.rs:8129). The host mirror `MarketGroupV16` lacks it (wrapper v16_program.rs:273). | yes |
| `/ BOUND_SCALE` | `BOUND_SCALE = 10^12` (engine lib.rs:25). Floor division, as `source_fresh_backing_total_atoms()` (v16.rs:9051). | yes, the same floor as the engine |

- **How the header is read.** It is the engine's own bytemuck type
  `MarketGroupV16HeaderAccount`, at `MARKET_GROUP_OFF = HEADER_LEN +
  WRAPPER_CONFIG_LEN` with length `MARKET_GROUP_LEN =
  size_of::<MarketGroupV16HeaderAccount>()` (wrapper v16_program.rs:58-62).
  The wrapper's `market_header()` (v16_program.rs:2032) reads it the same
  way. The monitor uses `pod_read_unaligned`.
- **Cross-check.** The engine maintains `source_fresh_backing_total_num` as
  Σ `fresh_reserved_backing_num` over source domains
  (`update_source_credit_aggregate_totals`, v16.rs:8989). The snapshot
  carries that sum, computed from the mirror's `source_credit[]`, as
  `source_fresh_backing_sum_domains_num`. On every poll in the validation run
  it equals the raw header value.
- **Every term is exact.** Nothing is approximated. Two things to know:
  - `residual` saturates like the engine. When the senior claims exceed
    `vault`, `percolator_senior_shortfall_atoms` reports by how much (it
    should be 0).
  - Prometheus stores f64, so values above 2^53 lose precision on
    ingestion. The JSON snapshot keeps them exact.

| Metric | Meaning |
|---|---|
| `percolator_engine_header_atoms{term}` | Each term in atoms. `source_fresh_backing_total` is already divided by `BOUND_SCALE`. |
| `percolator_engine_source_fresh_backing_total_num` | The raw `_num` value |
| `percolator_residual_atoms` | `residual()` |
| `percolator_senior_shortfall_atoms` | `max(senior − vault, 0)` |
| `percolator_unowned_collateral_atoms` | `residual − pnl_pos_tot` |
| `percolator_unowned_collateral_vs_bound_atoms` | `residual − pnl_pos_bound_tot` |
| `percolator_spl_vault_balance_atoms{vault}` | SPL token `amount` of the market vault, `ATA(PDA["vault", market], collateral_mint)` |
| `percolator_spl_vault_minus_engine_vault_atoms` | SPL balance − engine `vault` |

`unowned` is negative while live junior claims (`pnl_pos_tot`) exceed the
residual. That is normal during trading: positive PnL is paid out haircut.
`unowned` approaches the engine's claim-free residue as the market winds
down. Alert on its **drift**, not on its sign.

### Funding

| Metric | Meaning |
|---|---|
| `percolator_asset_funding_last_accrual_slot{asset}` | `AssetStateV16.slot_last` (v16.rs:4905) |
| `percolator_asset_slots_since_accrual{asset}` | chain slot − `slot_last` |
| `percolator_legs_f_subatom_remainder` | Active legs whose only funding gap is worth less than one atom (`work::leg_f_settled`). **This is not pending work:** the engine considers these legs settled. |

### Monitor itself

`percolator_monitor_chain_read_ok`, `percolator_monitor_snapshot_unix_seconds`,
`percolator_chain_slot`, `percolator_engine_current_slot`.

## Alerts

`crates/cranker/monitor/alerts.yml` holds one recording rule and the alerts
below. Thresholds are literals in the file; edit them there.

| Alert | Expression (default) |
|---|---|
| `KeeperAccountQuarantined` | `percolator_keeper_account_quarantined_seconds > 600`: an account unrefreshable or quarantined for more than 10 minutes |
| `UnownedCollateralDrift` | `abs(percolator:unowned_collateral_drift_atoms_per_hour) > 1000`, where the recording rule is `delta(percolator_unowned_collateral_atoms[1h])` |
| `BackingBucketExpiringWithLiens` | `percolator_backing_buckets_expiring_with_liens > 0` |
| `FreshBucketPastExpiryInLive` | `percolator_backing_buckets_fresh_past_expiry_in_live > 0` for 1m (critical) |
| `KeeperNoSuccessfulCrank` | `percolator_keeper_slots_since_last_progress > 750`. That is about 5 minutes at 400 ms slots. A quiet market with no work also trips it, so read it together with `percolator_accounts_out_of_date`. |
| `KeeperNeverProgressed` | Status OK for 15m with no `last_progress_slot` |
| `KeeperStatusFileStale` | `percolator_keeper_status_age_seconds > 120 or percolator_keeper_status_ok == 0` (critical): the keeper stopped |
| `MonitorChainReadFailing` | `percolator_monitor_chain_read_ok == 0` for 5m |

The dashboard shows the same conditions with the same thresholds.

**Rule-file check.** `promtool` is not installed on the validation machine,
so the file was checked by a YAML parse instead. Every rule has `expr` and
exactly one of `alert` / `record`. To validate it properly, run
`promtool check rules crates/cranker/monitor/alerts.yml`.

## Validation

These runs used a local `solana-test-validator`, with the wrapper deployed
at the pin. The monitor polled every 2 s, and a sampler saved `/metrics` and
`/snapshot.json` every 15-20 s. No Prometheus server was run. **Each alert
condition below was evaluated by hand** against the captured `/metrics`
text, using a small script that applies the same comparisons and thresholds
as `alerts.yml`. `UnownedCollateralDrift` was extrapolated from the sample
window, because no run lasted an hour.

### Run A: metrics evolving over a full `live_shaped` run (historical: the load test is maintained separately)

```bash
CRANKER_CYCLES=60 CRANKER_STATUS_FILE=<dir>/status.json CRANKER_FAULT_ACCOUNTS=@<dir>/faults \
  target/release/live_shaped
target/release/monitor --market <MARKET> --program <PROGRAM> --status-file <dir>/status.json --interval-ms 2000
```

One line per sample, abbreviated (21 polls, market `Brvm…`):

| poll | slot | mode | keeper cycle | quarantined | out of date (kf legs / engine) | fresh / expired buckets | Fresh past expiry in Live | unowned | SPL − vault | resolved unpaid |
|---|---|---|---|---|---|---|---|---|---|---|
| s02 | 10271 | live | 1 | 0 | 0 / 0 | 5 / 0 | 0 | 0 | 0 | – |
| s03 | 10314 | live | 5 | 2 | 60 / 60 | 6 / 0 | 0 | −85,902 | 0 | – |
| s07 | 10475 | live | 18 | 2 | 54 / 54 | 5 / 1 | **1** | −1,738,734 | 0 | – |
| s09 | 10562 | live | 26 | 3 | 54 / 54 | 1 / 5 | 0 | 7,079,843 | 0 | – |
| s11 | 10648 | live | 34 | 3 | 39 / 39 | 3 / 3 | **1** | 7,019,431 | 0 | – |
| s13 | 10728 | live | 40 | 3 | 2 / 2 | 1 / 5 | 0 | 7,101,821 | 0 | – |
| s15 | 10805 | resolved | 45 | 2 | 0 / 0 | 2 / 4 | 0 | 6,994,311 | 0 | 42 |
| s19 | 10960 | resolved | 59 | 3 | 0 / 0 | 1 / 5 | 0 | 9,878,859 | 0 | 23 |
| s21 | 11032 | resolved | 59 | 3 | 0 / 0 | 1 / 5 | 0 | 11,815,947 | 0 | 4 |
| s22 | – | – | – | – | – | – | – | – | – | – |

- **s22:** `chain_error: "read_market: Custom(5)"` after `CloseSlab`. The chain
  read fails cleanly.
- **Out of date.** The portfolio-side K/F cohort scan equals the engine's
  `stale_account_count_*` sum on every poll, and `stale_state` /
  `b_stale_state` reconcile at 0 / 0. Every `_mismatch` gauge is 0.
- **Cross-check.** `source_fresh_backing_total_num`, read from the raw
  header, equals Σ `source_credit[d].fresh_reserved_backing_num` on every
  poll, so the raw-header offset is right.
- **SPL vault.** The SPL vault balance equals the engine `vault` on every
  poll.
- **Wedge detection.** At s07 the monitor saw the wedge condition:

  ```
  percolator_backing_buckets_fresh_past_expiry_in_live{market="Brvm…"} 1
  percolator_backing_buckets_fresh_past_expiry_in_live_unreferenced{market="Brvm…"} 0
  {"asset":1,"side":"long","domain":2,"status":"fresh","expiry_slot":10470,"slots_to_expiry":-5,"valid_liened_backing_num":0,"referenced":true}
  ```

  The scenario's own report, `lapsed-but-Fresh buckets in Live`, lists
  domain 2 at slots 10477..10617 and domain 3 at 10629..10643. That matches
  s07 and s11. `FreshBucketPastExpiryInLive` evaluates true at those polls.
  Both buckets were referenced, so the subset that blocks refresh read 0.
- **Keeper status.**

  ```
  percolator_keeper_attempts_total{outcome="progress"} 63
  percolator_keeper_attempts_total{outcome="idle"} 27
  percolator_keeper_attempts_total{outcome="failed"} 13
  percolator_keeper_cycles_total{outcome="progress"} 11
  percolator_keeper_cycles_total{outcome="failed"} 2
  percolator_keeper_crank_cu{stat="p50"} 114513
  percolator_keeper_crank_cu{stat="max"} 394535
  ```

- **Unowned collateral drift.** From s03 to s21, `unowned` moved by
  11,901,849 atoms in 361 s. Extrapolated, that is far above the
  1000 atoms/h default, so `UnownedCollateralDrift` would fire. That is
  expected in a stress run: trading PnL moves `pnl_pos_tot`, and lapsed
  backing moves into the residual. Tune the threshold per market.
- **Scenario result.** The scenario's `RESULT` was FAIL. Assertion (a)
  failed because the two injected accounts never recover; that is the
  intended effect of the hook. Assertions (c) and (e) are pre-existing
  results at this pin: unexplained residue at CloseSlab, and no Impaired
  bucket at 60 cycles. The hooks do not change them.

### Fault 1: a forced-failing account (`CRANKER_FAULT_ACCOUNTS`)

Run B used `CRANKER_CYCLES=200`. After setup, two accounts that the keeper
was actively cranking were written to the fault file:
`A5MSob7z…` and `GxXTdodD…`. The keeper logged:

```json
{"event":"quarantine","account":"GxXTdodDGqUhFTEvzgg37Z8yKjcmAiPbBmuM7TJJ9Fyb","error":"InjectedFault","cycle":31,"slot":11053,"plan_class":"test:injected-fault","strike":1,"backoff_cycles":2,"until_cycle":34}
{"event":"quarantine","account":"GxXTdodDGqUhFTEvzgg37Z8yKjcmAiPbBmuM7TJJ9Fyb","error":"InjectedFault","cycle":127,"slot":11980,"plan_class":"test:injected-fault","strike":6,"backoff_cycles":64,"until_cycle":192}
```

`/metrics` at keeper cycle 154, with the keeper still running (status age
8.6 s):

```
percolator_keeper_quarantined_accounts{market="Bkn…"} 2
percolator_keeper_account_quarantined_seconds{market="Bkn…",account="A5MSob7ziyeFrQ4HrjUeCmZnFAddoovQBQjEaDtiane2",error="InjectedFault",strike="5",until_cycle="174"} 501.735
percolator_keeper_account_quarantined_seconds{market="Bkn…",account="GxXTdodDGqUhFTEvzgg37Z8yKjcmAiPbBmuM7TJJ9Fyb",error="InjectedFault",strike="6",until_cycle="192"} 619.046
```

Evaluation: `KeeperAccountQuarantined` (`> 600`) is **true** for `GxXTdodD…`
(619 s) and not yet for `A5MSob7z…` (502 s). Every other condition was false
at that poll: status age 8.6 s, 24 slots since progress, 0 lapsed Fresh
buckets, chain read OK. The dashboard showed both accounts under Conditions.

### Fault 2: a stopped keeper (killed mid-run)

The keeper process was killed at keeper cycle 154, with the market Resolved
and exits in progress. The monitor kept polling:

| seconds after kill | status age (s) | slots since last progress |
|---|---|---|
| −13 | 21.8 | 38 |
| +17 | 26.7 | 63 |
| +107 | 117.4 | 231 |
| +137 | **147.6** | 282 |
| +318 | 327.0 | 589 |
| +408 | 417.7 | **753** |

Final poll:

```
percolator_keeper_status_ok{market="Bkn…"} 1
percolator_keeper_status_age_seconds{market="Bkn…"} 425.779
percolator_keeper_last_progress_slot{market="Bkn…"} 12262
percolator_chain_slot{market="Bkn…"} 13030
percolator_keeper_slots_since_last_progress{market="Bkn…"} 768
```

Evaluation:
- `KeeperStatusFileStale` (`> 120 s`) became **true** between +107 s and
  +137 s.
- `KeeperNoSuccessfulCrank` (`> 750 slots`) became **true** at +408 s.
- `KeeperAccountQuarantined` stayed true, because the frozen status file
  still lists both struck accounts.
- `MonitorChainReadFailing` stayed false: the market is still readable.

The Resolved snapshot at that point showed 2 claimants still unpaid:
`resolved_claims.claimants_unpaid = 2`, with 7 receipts finalized,
`capital_atoms = 1,126,855` and `unreceipted_positive_pnl_atoms = 1,049,573`.
SPL vault = engine vault = 12,759,008.

### Unit tests

`cargo test --release --manifest-path cranker/Cargo.toml` passes 61 lib
tests. The monitor's pure parts are covered by:
- `monitor::json`: parse, escape, and exact big integers.
- `monitor::status`: writer totals and quarantine age, JSON round trip, and
  an atomic write that leaves no temp file.
- `monitor::snapshot`: the `residual()` / unowned math (floor and
  saturation), an exact 256-bit mul-div, bucket classification, resolved
  claims, and CU percentiles.
- `monitor::metrics`: label and HELP escaping, and one family per name.
- `monitor::http`: routes.
- `cycle`: parsing the fault list.
