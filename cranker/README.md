# cranker/ — M2 minimal permissionless cranker

A learning artifact. **Not production code.** Runs against a local
`solana-test-validator` with `percolator-prog` deployed. Enters a fixed
number of loop cycles, discovers portfolios, and submits
`PermissionlessCrank` transactions with a three-way outcome classifier.

## Quick run

```
# From /Users/augstar/percolator-watch:
scripts/m1_bringup.sh                 # provisions a validator + program
PERCOLATOR_PROGRAM_ID=<from bringup> CRANKER_CYCLES=3 \
    cargo run --release -p percolator-cranker
```

Env vars:
* `CRANKER_CYCLES` (default `3`) — number of loop iterations.
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

Each cycle:

1. `getProgramAccounts` filtered by wrapper header
   `[MAGIC | VERSION | KIND_PORTFOLIO]` (11-byte memcmp @ offset 0).
2. Locally filter to portfolios whose provenance `market_group_id` equals
   the cranker's target market (skip stale portfolios from prior bringups).
3. For each ranked candidate, fetch a stub observation, consult the
   plan-classification gate, submit a `PermissionlessCrank`, and classify
   the response.

Every cycle prints a per-submission line plus a mirror-custody summary
at the end.

## What's real

* **Discovery.** `discovery.rs` reads the 11-byte header + provenance
  `market_group_id` and `owner` at fixed byte offsets. Layout confirmed
  against `PortfolioAccountV16Account` in `repo/src/v16.rs:15754` and
  `ProvenanceHeaderV16Account` at `repo/src/v16.rs:5324`.
* **Plan-classification gate.** `plan_gate.rs` delegates to the upstream
  `percolator::auto_crank_plan_requires_caller_observation` predicate
  (`repo/src/v16.rs:187`, exported at `repo/src/lib.rs:54`), pinned by
  the proofs
  `proof_v16_auto_crank_refresh_is_unique_observation_requiring_plan`
  and
  `v16_auto_crank_progress_realizable_without_observation_for_every_class`.
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
* **Reward-portfolio append.** Enabled via `UpdateLiquidationFeePolicy
  { cranker_share_bps: 5000 }` in `setup::bootstrap`. The wrapper's
  reward-append path only accepts a trailing writable portfolio when
  `liquidation_cranker_fee_share_bps != 0`
  (`repo-prog/src/v16_program.rs:10467`), and the tx signer must own it
  (`:10473`, `:10485`) — the cranker portfolio is created with `payer`
  as the owner for that reason.

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
  we send at most one hint per submission today because the synthetic
  market has one asset. Selection policy is: crank every ranked
  candidate one asset at a time. A production cranker would batch
  hints across assets per candidate.
* **Ranking.** `discovery::rank_by_staleness` sorts by pubkey bytes
  after filtering to the target market. In production this is where
  we'd score by `(last_accrual_slot age) × capital` or by distance to
  liquidation threshold.
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
| `src/main.rs`              | Loop, cycle scheduling, mirror-custody summary      |
| `src/setup.rs`             | Mint + market + user portfolio + reward portfolio   |
| `src/discovery.rs`         | `getProgramAccounts` + market-scoped ranking        |
| `src/observation.rs`       | Oracle-account fetch (STUB)                         |
| `src/plan_gate.rs`         | Wrapper of upstream truth-table predicate           |
| `src/submit.rs`            | Build tx, submit, classify three-way, wrapper-error lookup |

## Related

* [../prep.md](../prep.md) — milestones M1/M2/M3, baked assumptions,
  open questions Q1–Q6.
* [../cranker-strategic-questions.md](../cranker-strategic-questions.md) —
  running answers to strategic questions (fee capture, latency, complexity floor).
* [../client/](../client/) — the M1 bring-up client from which this
  crate's `setup.rs` is adapted.
