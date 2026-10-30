# cranker/ — M2 minimal permissionless cranker

A learning artifact. **Not production code.** Runs against a local
`solana-test-validator` with `percolator-prog` deployed. Enters a fixed
number of loop cycles, discovers portfolios, and submits
`PermissionlessCrank` transactions with a three-way outcome classifier.

## Quick run

```
# From /Users/augstar/projects/percolator-cranker:
FORCE_SBF_REBUILD=1 ./scripts/m1_bringup.sh   # reset validator, deploy, M1
PERCOLATOR_PROGRAM_ID=<from bringup> CRANKER_CYCLES=3 \
    cargo run --release --manifest-path cranker/Cargo.toml
```

Reference / dry-run only. Not a paid live service. M3 is NO-GO until a
non-null program id lands on a public cluster. No bounty or mainnet path
exists in this crate. **Fee paths are off by default:** the default
bootstrap has no maintenance fee and zero cranker shares, and cranks attach
no reward portfolio. `CRANKER_ENABLE_FEE_SHARES=1` opts in (maintenance
fee plus 50% shares, so a share accrues into the keeper's own portfolio).
Nothing withdraws it unless `CRANKER_WITHDRAWAL_TEST=1` is also set, which
requires the opt-in.

Pre-merge preview of the upstream launch fixes (not the default):

```
./scripts/preview_launch_fixes.sh   # prog #456 head + engine 68859b94 (path patch)
```

**CU (2026-10-28, wrapper `5cb331dd` / engine `4db11a8c`):** a fresh no-op
`PermissionlessCrank` with the reward portfolio appended (fee opt-in, the
shape of the old baseline) consumes **18,565 program CU** (tx total 18,889),
+590 over the 17,975 measured on `d5e2ec6f` / `495a5590`. Without the
reward portfolio (the default since fees are opt-in): 18,363. Pre-merge preview
(prog #456 head + engine `68859b94`): 18,820 (19,144). The run prints
`cu: noop_min=… worst_ok=…` from the tx logs; set `CRANKER_MEASURE_CU=0`
to skip the extra `getTransaction` per crank.

Env vars:
* `CRANKER_CYCLES` (default `3`) — number of loop iterations.
* `CRANKER_CYCLE_MS` (default `30000`), `CRANKER_CYCLE_CU` (default
  `24000000`), `CRANKER_CYCLE_MAX_TX` (default `64`): the per-cycle budget.
  - **Wall clock** starts at cycle entry, so discovery and planning count.
  - **Admission is checked before every tx**, retries included: each tx
    reserves its full compute-unit limit (1.4M) up front, then is settled
    to the whole-tx CU measured from its logs. A tx is counted once it
    begins, whatever its outcome. The wall clock is an *admission*
    deadline: no tx is sent after it. It does not bound RPC calls already in
    progress: a tx already sent waits for the client's confirmation, which
    normally ends at blockhash expiry (about a minute) but can last longer if
    the validator stops advancing. A tx whose fate stays unknown is reported
    as `unconfirmed`, never as a failure of the account.
  - **When it runs out**, the rest of the queue is deferred, and rotation
    reaches it first next cycle.
* `CRANKER_QUIET_REFRESH_SECS` (default `300`, `0` = every cycle), `CRANKER_FLAT_REFRESH_SECS` (default `3600`, `0` = quiet cadence): flat accounts start their timer on first sight; fee+lien and backing expiry still trigger refresh, and an idle Live keeper probes one account every 240 seconds to prove liveness.
* `CRANKER_ATTEMPT_MAX_TX` (default `4`): the most txs one account may use
  in a cycle. This bounds an A6 lien drain, which continues next cycle.
* `CRANKER_IDLE_STUCK_CYCLES` (default `10`): a Resolved claimant that still
  holds value and has been idle this many cycles in a row is reported as
  `idle_pending_stuck`, with one `idle_with_pending_work` JSON event.
* `CRANKER_QUARANTINE_THRESHOLD` (default `3`),
  `CRANKER_QUARANTINE_BASE_CYCLES` (default `2`),
  `CRANKER_QUARANTINE_MAX_CYCLES` (default `64`) — same-error streak that
  quarantines an account, and its backoff (doubles per re-quarantine, capped).
* `CRANKER_ENABLE_FEE_SHARES=1` — opt in to fee paths (see the top of this
  file). `CRANKER_WITHDRAWAL_TEST=1` additionally needs it.
* `CRANKER_FAIL_ON_ERRORS=1` — exit nonzero if any attempt fails (CI sets
  it for the dry run).
* `CRANKER_PYTH_MODE=canonical` — enable Pyth canonical-PDA fallback
  in `PythFeedResolver`. Unset or `off` (default) means the resolver
  never hits, so hybrid+Pyth legs fall through to bytes-as-pubkey and
  the wrapper's Pyth reader will hard-reject at submit. Correct for
  our synthetic MANUAL market either way.
* `CRANKER_WITHDRAWAL_TEST=1` — after the loop, route maintenance
  fee to the cranker portfolio via `SyncMaintenanceFee` and prove
  Q6 empirically: `Withdraw` from the cranker portfolio and confirm
  the destination ATA balance moves. Sanity-check that a cranker
  portfolio's withdrawal path is byte-for-byte the same as a user
  portfolio's. See prep.md Q6 for the source-read side of the answer.

Each cycle (`cycle::run_one_cycle`, shared by the binary and the validator tests):

1. `getProgramAccounts` filtered by wrapper header
   `[MAGIC | VERSION | KIND_PORTFOLIO]` (11-byte memcmp @ offset 0).
2. Locally filter to portfolios whose provenance `market_group_id` equals
   the cranker's target market (skip stale portfolios from prior bringups).
3. Read the market once. **Live / Recovery:** one hint per active leg asset
   (fallback asset 0), each with its oracle accounts. **Resolved:** at most
   one oracle-less hint naming the asset whose backing bucket is due
   (`resolved.rs`); the wrapper turns it into
   `AutoCrankObservationV16 { asset_index, effective_price: 0, funding_rate_e9: 0 }`
   and the engine advances one bucket or returns `EngineNonProgress`.
   On Resolved the wrapper reads the accounts as `[owner, market, portfolio,
   owner_ata, vault, vault_authority, token_program]` (`CrankRoute::Resolved`).
   Account 0 must be the portfolio **owner** key, and it has to sign only
   while `slot − resolved_slot < force_close_delay_slots`. Inside that
   window, third-party candidates are counted idle (`resolved:owner-window`)
   and not submitted. After it, the cranker pays the fee, and any payout
   goes to the owner's ATA (created idempotently first, rent paid by the
   cranker). There's no reward portfolio on this route. Only lapsed **Fresh**
   buckets are hinted.
4. Order candidates (`scheduler.rs`). Each class (liquidatable, pending,
   quiet; see `work.rs`) is rotated every cycle, then the classes are
   interleaved 2:1:1. A stream of liquidatable accounts can't starve the
   others. Skip quarantined accounts. Spend the cycle budget.
5. Submit, classify **Progress / Idle / Failed**. `Ok(no-mut)` and an
   `EngineNonProgress` revert are idle. Any other revert, engine error or
   RPC error is a failure of that account only. Three same-error failures
   in a row quarantine it (`quarantine.rs`) and print one JSON line:

   ```
   {"event":"quarantine","account":"…","error":"EngineCounterUnderflow","cycle":12,"slot":…,"plan_class":"live:pending","strike":1,"backoff_cycles":2,"until_cycle":15}
   ```

Every cycle prints per-submission lines and a
`cycle N summary: progressed= idle= failed= quarantined_skipped= deferred= …`
line; the run ends with the mirror-custody table and CU stats.

## Upstream changes absorbed (wrapper `5cb331dd`, engine `4db11a8c`)

* **Resolved tag-5 hint** (above). `CloseResolved` is an alias for the same
  crank. Wire unchanged: still `[5 | now_slot | n | (asset_index u16,
  oracle_accounts u8)*]`; on Resolved the wrapper accepts `n ≤ 1` and
  `oracle_accounts == 0`.
* **Recovery crank bumps `position_epoch`** (prog `b40ffbd8`).
  * **Keeper.** The production crank path takes no binding at all (tag 5
    has no epoch field), so the keeper caches nothing that could go stale.
    Every owner instruction the repo builds (tests, Q6 spot-check) reads the
    binding from fresh account bytes right before sending.
  * **Not exercised in this PR's tests.** The epoch bump itself was
    observed in the separate load test (historical, see `../prep.md`); the
    market-wide Recovery crank path that `b40ffbd8` changed was never
    reached.
* **Funding is exact** (engine #227: only in the `68859b94` **preview**, not
  in the default pin `4db11a8c`): a leg whose F snapshot trails its target
  by less than one atom is F-settled. `work::leg_f_settled`
  is the only F test, so such legs never count as pending work. There is
  still no standalone funding settle entry point.
* **Provider surplus withdraw** (engine #223 / prog #454) reaches the
  existing `WithdrawBackingBucket` tag; it is not a crank and the cranker
  never calls it. No new tag to decode.
* **OI symmetry**: no cranker code or test asserts `OI_long == OI_short`.
  Any future check must exclude assets in Recovery.
* Layout discriminator still 18; `AutoCrankPlanV16` variants unchanged.

## What's real

* **Discovery.** `discovery.rs` reads the 11-byte header + provenance
  `market_group_id` / `owner`, requires layout discriminator **18**, and
  surfaces wrapper `portfolio_id`.
* **Plan-classification gate.** `plan_gate.rs` delegates to the upstream
  `percolator::auto_crank_plan_requires_caller_observation` predicate
  (`repo/src/v16.rs:190`). Covers A6 `ReleaseSourceLiens`, `AdvanceClose`,
  `FinalizeRecovery`, A8 `CloseResolved` without a hand-rolled match.
  A6 drain is one domain per crank until `Ok(no-mut)`.
  Never hand-roll the match — see the module docstring.
* **Three-way response classifier.** `submit.rs` distinguishes
  `Err(hard-reject)`, `Ok(no-mut)`, and `Ok(mut)` via (a) tx status,
  (b) pre/post state-hash comparison, (c) `PercolatorError` code lookup
  for friendly names. Retry safety follows: on `Ok(mut)`, do not blindly
  retry; on `Ok(no-mut)`, re-submission is a clean no-op today because
  the engine returns `NonProgress` for the same fresh state, but see
  M2.f in `../prep.md` for the harder retry cases (Liquidate,
  SettleBChunk, RefreshAccount{Some}).
* **Instruction encoding.** Reuses `percolator_prog::ix::Instruction::encode()`
  from the wrapper crate; no hand-written wire format.
* **Reward-portfolio append.** Only with `CRANKER_ENABLE_FEE_SHARES=1`
  (or the explicit `setup::bootstrap_with_fee` the integration tests use),
  which sets `UpdateLiquidationFeePolicy { cranker_share_bps: 5000 }`. The wrapper's
  reward-append path only accepts a trailing writable portfolio when
  `liquidation_cranker_fee_share_bps != 0`
  (`repo-prog/src/v16_program.rs:10467`), and the tx signer must own it
  (`:10473`, `:10485`) — the cranker portfolio is created with `payer`
  as the owner for that reason.

## Load test

The `live_shaped` load test is not part of this crate any more; it is
maintained separately. Its earlier 200-cycle results are kept in
`../prep.md` and `scenario-findings.md` as findings (F1 is upstream as
aeyakovenko/percolator#243).

## What's stubbed

Documented inline in the files, listed here for auditability:

* **`observation.rs::fetch_observation_for_asset`** — **Q4 answered
  2026-07-07: this is now a real walk, not a stub.** Reads
  `AssetOracleProfileV16` for the target asset out of the market slab
  byte-for-byte, dispatches on `oracle_mode`, and returns the pubkey
  list. See prep.md Q4 for the full mode→behavior table.
* **`observation.rs::PythFeedResolver`** — Pyth-specific pubkey
  resolution for HYBRID+Pyth assets, with two configurable mechanisms:
  * **Explicit `overrides: HashMap<[u8; 32], Pubkey>`** — operator-
    populated `feed_id → PriceUpdateV2 account` map. Wins when set.
    This is the operational-data edge.
  * **Canonical PDA fallback** — `PDA(seeds=[shard_u16_le, feed_id],
    program_id=PYTH_RECEIVER)`, matching Pyth's own
    `getPriceFeedAccountAddress`. Shard 0 is Pyth's mainnet default.
    Selected via `CRANKER_PYTH_MODE=canonical`.
  A cold-start `PythFeedResolver::empty()` (default) never resolves;
  hybrid+Pyth legs then fall through to bytes-as-pubkey (correct for
  Switchboard/Chainlink; wrong for Pyth, and the wrapper will
  hard-reject at submit — the correct failure mode). Six unit tests
  in `observation::tests` pin the resolver contract.
* **Hint budget.** Constant `HINT_BUDGET = 16` (= `CRANK_OBSERVATION_DECODE_MAX`);
  one hint per active leg asset per candidate on Live markets, at most one
  on Resolved markets.
* **Ranking.** `work::assess` is an off-chain estimate (liquidation lock or
  certified deficit → liquidatable; stale flags or K/F drift → pending). It
  only orders the queue; it never skips an account. The engine selector is
  the only authority on what a crank does.
* **Plan class.** The `plan_class` in logs is the cranker's own label
  (`live:liquidatable`, `resolved:backing-hint(2)`, …). The engine's chosen
  plan is not visible from outside the tx (prep.md Q1).
* **No priority-fee logic.** Every submission uses the same 1.4M CU
  limit and 128 KiB heap, no leader-slot prediction, no Jito bundles.
* **No `SyncMaintenanceFee`.** M2 only exercises `PermissionlessCrank`
  because that's the loop the mission brief was targeting; the M1
  client already proved the maintenance-fee routing path with a
  cranker portfolio appended.
* **No persistent state store.** Mirror-custody rows are in-memory only.
* **Program-id from env.** No wallet-driven discovery; `PERCOLATOR_PROGRAM_ID`
  must be supplied (defaults to the bringup script's build-time keypair).

## Reading a run

Sample summary:

```
cycle  target_portfolio                              outcome      tgtΔ  rwdΔ
0      NToY…                                        Ok(mut)      4     0
1      NToY…                                        Ok(no-mut)   0     0
2      NToY…                                        Ok(no-mut)   0     0
```

* `Ok(mut)` on cycle 0 means the engine synthesized an observation
  from committed state and dispatched a progress step (probably
  `RefreshAccount { Some(0) }` — asset 0 got a stale-mark refresh).
* `Ok(no-mut)` on subsequent cycles is the wrapper swallowing the
  engine's `NonProgress` result — the target's state is already
  current, no work to do. This is the intended cranker steady state.
* `rwdΔ = 0` on every row is honest: our synthetic market has
  `liquidation_fee_bps = 0`, so `retained_fee` is 0, so
  `retained_fee * cranker_share_bps / 10000 = 0`. The reward path
  runs; there's just nothing to reward.
* A `HardReject (<PercolatorError name>)` row logs the friendly
  variant name in parens (via `submit::wrapper_error_name`) so you can
  match a `Custom(0xN)` to the source enum without grepping.

## Files

| Path                       | Purpose                                             |
| -------------------------- | --------------------------------------------------- |
| `src/main.rs`              | Bootstrap, cycle loop, mirror-custody summary       |
| `src/cycle.rs`             | One discover → hint → schedule → submit cycle       |
| `src/scheduler.rs`         | Budget, priority order, rotation, idle vs failed    |
| `src/quarantine.rs`        | Per-account same-error quarantine + backoff         |
| `src/resolved.rs`          | Resolved-mode backing-bucket hint                   |
| `src/work.rs`              | Priority estimate, F-settled test                   |
| `src/binding.rs`           | portfolio_id / sequence / position_epoch bindings   |
| `src/setup.rs`             | Mint + market + user portfolio + reward portfolio   |
| `src/discovery.rs`         | `getProgramAccounts` + market-scoped ranking        |
| `src/observation.rs`       | Oracle-profile walk + Pyth resolver                 |
| `src/plan_gate.rs`         | Wrapper of upstream truth-table predicate           |
| `src/submit.rs`            | Build tx, submit, classify three-way, wrapper-error lookup |

## Related

* [../prep.md](../prep.md) — milestones M1/M2/M3, baked assumptions,
  open questions Q1–Q6.
* [../cranker-strategic-questions.md](../cranker-strategic-questions.md) —
  running answers to strategic questions (fee capture, latency, complexity floor).
* [../client/](../client/) — the M1 bring-up client from which this
  crate's `setup.rs` is adapted.
