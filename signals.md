# Cranker-Enabling Signal Inventory

Canonical "how close are we to cranker-buildable?" reference. One section per
signal type (1a–1f from the watch priority list). Append commits as they land.

Baseline captured at `origin/master` = `bbe745a` on 2026-06-19 (cold start).

---

## 1a. `cfg_max_*_chunks` / `cfg_*_chunk*` knobs (crank-bounded ops)

Baseline set on `origin/master` (`spec.md`):
- `cfg_max_account_b_settlement_chunks` — spec.md:157
- `cfg_max_bankrupt_close_chunks` — spec.md:158
- `cfg_pending_obligation_settlement_chunks` — spec.md:162
- `cfg_public_b_chunk_atoms` — spec.md:156, 1332 (chunk *size*, not a max-count)

**Watch:** a NEW `cfg_max_<operation>_chunks` = a new operation became
crank-callable. The funding flow (1c) is the prime candidate for the next one
(e.g. a `cfg_max_funding_*_chunks`).

_Appended hits:_ (none yet)

## 1b. New phase enum mirroring the bankruptcy pattern (Touched → … → AccountFinalized)

Baseline: NO Rust `enum *Phase` exists in `src/`. The bankruptcy lifecycle phase
tokens `Touched` (spec.md:1390) … `AccountFinalized` (spec.md:1401) live in
`spec.md` prose only.

**Watch:** a new phase enum (especially in `src/`) signals a new crankable
lifecycle. A funding-settlement phase enum would be the strongest catch.

_Appended hits:_ (none yet)

## 1c. Funding flow (HIGH — most actively-evolving cranker-adjacent surface)

Baseline: funding **counters/budgets** present on `origin/master` (`spec.md`),
landed via the 2026-06-19 "v16 funding flow counters" work —
`FUNDING_DEN` (80), `GLOBAL_MAX_ABS_FUNDING_E9_PER_SLOT` (116),
`cfg_max_abs_funding_e9_per_slot`, `cfg_min_funding_lifetime_slots`,
`cfg_max_accrual_dt_slots`, `funding_epoch` (737), `funding_budget_num` (191).

**No** `funding_apply` / `settle_funding` / `crank_funding` / `apply_funding` /
`funding_advance` symbol exists yet. **This is THE watch point:** the next stage
is turning these counters into a crank-callable funding apply/settle path. The
first such symbol is a top-priority 🔔 catch.

_Appended hits:_
- **2026-06-23** — funding became a **crank observation input** (not yet a
  settle/apply step). `AutoCrankObservationV16.funding_rate_e9` (src/v16.rs:4424)
  and `PermissionlessCrankRequestV16.funding_rate_e9` (src/v16.rs:5095): keepers
  now submit a funding rate with each crank observation. Funding refs 109→120.
  Still NO funding arm in `AutoCrankPlanV16` — the crank-callable funding
  settle/apply path remains the open 1c catch. Window `bbe745a..91a46c0`.
- **2026-07-18 (OPEN PR, not on master) — funding K/F settlement made
  crank-cadence-invariant (engine DRAFT PR #129
  `codex/fix-funding-crank-cadence-20260716`).** Not a new `pub fn`; reworks the
  existing accrual path so settlement no longer depends on crank frequency:
  carries persistent Euclidean K/F remainders per leg, preserves them across
  resize/leg transforms, applies funding index precision *before* settlement
  rounding, and **bumps `V16_LAYOUT_DISCRIMINATOR` 17→18**. Fixes: 10 sub-atom
  cranks charging 10× vs 1 crank, and frequent cranks *erasing* nonzero funding.
  Wrapper repro percolator-prog PR #247. This is the closest funding-settlement
  correctness work to a paid-cranker requirement seen so far; still DRAFT.
  Confidence: HIGH (verified diff, discriminator bump confirmed). See digest
  2026-07-18 §1②.
- **2026-07-18 (OPEN PR) — funded-flat asset reuse (engine DRAFT #124).** Treats
  current/epoch-start F indices as inert anchors once claimant/value fields empty,
  fixing a partial public DoS on finite market-slot capacity. Funding lifecycle.
  DRAFT. Confidence: MEDIUM.

## 1d. New crank-shaped public entry point (`pub fn crank_*|advance_*|settle_*|apply_*|continue_*`)

Baseline: NO `pub fn` matching `(crank|advance|settle|apply|continue)_` exists on
`origin/master`.

**Observed commits:**
- **2026-08-06 — 🔔 NEW permissionless progress path: `SourceBackingExpired`
  (DRAFT PR #173, aeyakovenko, branch `codex/pr135-live-expiry-engine-20260805`,
  base master, NOT merged).** First *widening* of the crank outcome surface since
  cold-start (the #149–169 wave only *bounded* existing loops). `src/v16.rs`:
  `PermissionlessProgressOutcomeV16` gains a 5th variant
  `SourceBackingExpired { domain: usize }` (master had 4; `SourceBackingExpired`
  count on master = 0). New machinery driven from the existing public
  `permissionless_auto_crank_not_atomic`: `kernel_lapsed_source_backing_scan_step`,
  `first_lapsed_source_backing_for_account`,
  `expire_first_lapsed_source_backing_for_account_not_atomic`, and
  `AccountRefreshCertOutcomeV16::SourceBackingExpired(usize)`. Expires exactly one
  lapsed source-backing domain per auto-crank call, bounded by
  `PORTFOLIO_SOURCE_DOMAIN_CAP` (`src/v16.rs:22`, = `2 * V16_MAX_PORTFOLIO_ASSETS_N`;
  Kani cfg 4) — signal-type 1d + 1b-adjacent (new outcome variant) + 1a-adjacent
  (per-call cap). Unblocks owner risk-reduction when a funded live portfolio
  outlives its backing bucket. No new `pub fn` (folds into `RefreshAccount` cert
  path). Also present on staging branch `codex/pr135-engine-integration-20260805`
  (no PR; combined engine pin; Kani proofs 244→253). Confidence: HIGH (verified
  enum diff both branches vs master). See digest 2026-08-06 §1.

**Adjacent baseline (not a `pub fn` match, but cranker-load-bearing):**
- **`CrankForward` liveness profile** — `cfg_public_liveness_profile ==
  CrankForward` (spec.md:142). spec.md:1477: a public `CrankForward` market
  "MUST expose permissionless terminal recovery" for ~15 enumerated stuck states
  (B settlement failure, B-index exhaustion, source-credit underbacking, lien
  impairment, backing-expiry, active-close failure, domain lock/barrier,
  insurance exhaustion, snapshot re-aging, close-drift expiry, oracle
  unavailability, asset-lifecycle, claim-bound, payout-lane conflict).
  Field: `public_liveness_profile_crank_forward: bool` (src/v16.rs:2127, default
  `true` at 2183). **This is the existing crank-surface contract** — the place a
  reference cranker would hook in. Watch for `pub fn` entry points that
  *implement* these permissionless-recovery paths.
- **Crank-reward primitive** — `src/v16.rs:8013` "Pays an account from
  unbudgeted insurance surplus, e.g. a crank reward." (mechanism exists; not yet
  wired to a defined crank-payment schedule — see green-light priority 2).

_Appended hits:_
- **2026-06-23 — 🔔 MAJOR.** `f5f45dc` collapsed the public crank surface to
  exactly ONE entry point: `pub fn permissionless_auto_crank_not_atomic`
  (src/v16.rs:11298). Engine selects the action AND asset from current state via
  proven selector `select_auto_crank_plan` (src/v16.rs:1210); one instruction =
  one bounded step; absent observation → `NonProgress` no-op (out-of-order
  keeper safe). The prior caller-chosen `permissionless_crank_not_atomic`
  demoted to `pub(crate)`. Engine-selectable actions (`AutoCrankPlanV16`,
  src/v16.rs:4443): NoAction / RefreshAccount / SettleBChunk / Liquidate /
  DeclareRecovery / CloseResolved. README:159 "Permissionless crank: one public
  route" documents the full keeper calling convention (observations+budget →
  call once → mirror custody off `result.selected`). Crank refs 31→110. **This
  is the reference-cranker template, now fully specified.** Window
  `bbe745a..91a46c0`. Supporting: `2e7017f`, `e903491`, `1463dc0`, `806ddfd`.
- **2026-06-23 — 🚦 the entry point is now DEPLOYED in the wrapper.** Sibling
  repo `aeyakovenko/percolator-prog` (public, Solana) exposes
  `Instruction::PermissionlessCrank { now_slot, close_q, observations }` →
  `handle_permissionless_crank` (`src/v16_program.rs:5153`), wrapping the engine
  auto-crank. Program id `declare_id!("Perco1ator111…")` (`:32`). Crank rewards
  defined: `liquidation_cranker_fee_share_bps` / `maintenance_cranker_fee_share_bps`
  (`:520-521`) — keeper appends its own portfolio account to collect the share.
  See digest 2026-06-23 §2. The reference cranker is buildable against a concrete
  program (caveat: unaudited/educational, placeholder-style program id).
- **2026-06-26 — 🔔 new public predicate on the crank path.** `f5d291e` added
  free `pub fn auto_crank_plan_requires_caller_observation(plan:
  &AutoCrankPlanV16) -> bool` (src/v16.rs:187), **exported in lib.rs:54**
  (wrapper-usable). Exhaustive match → `RefreshAccount{None}` is the UNIQUE plan
  hard-requiring a caller observation; `Liquidate`/`SettleBChunk`/`DeclareRecovery`/
  `CloseResolved`/`NoAction`/`RefreshAccount{Some}` are all dispatchable from
  committed state. Backing mechanism `obs_or_current_asset` closure (`3d250d2`,
  src/v16.rs:11328) synthesizes an obs from committed `asset_state` when absent.
  **Reshapes the cranker contract:** keepers liquidate/B-settle from on-chain
  state without signing a fresh observation. Window `91a46c0..83387267`.
  ⚠️ This CHANGES the 2026-06-23 "absent observation → NonProgress no-op"
  property: now only `RefreshAccount{None}` no-ops without an obs.
- **2026-07-06 — 🔔 crank contract thinning (OPEN DRAFT, not landed).** PR #92
  (`issue90`, aeyakovenko/codex) removes caller-chosen liquidation size from the
  public crank surface: `AutoCrankWorkV16.liquidation_max_close_q` and
  `LiquidationRequestV16.close_q` are **deleted**; the engine computes the minimum
  maintenance-health-restoring close size (`liquidation_engine_close_request_q`,
  `liquidation_projected_healthy_after_close`, `liquidation_leg_maintenance_
  requirement` — all new *private* fns) and derives the fee from config
  (`liquidation_fee_for_close`, `min_liquidation_abs`/`liquidation_fee_cap`
  clamps). New doc: keeper supplies only observations + resolved-close fee rate;
  "the engine selects the step/asset, closes the selected leg, and derives the
  fee from config." Commits `43302e6`/`52b0286`/`a3e0357`. **No new `pub fn`** —
  this is the SAME single `permissionless_auto_crank_not_atomic` route, made
  thinner (2026-06-23 entry point → 2026-06-26 obs relaxation → now size/fee
  relaxation). Signal-type 1d (crank-contract surface change). **Status: DRAFT,
  master unmoved — NOT landed.** Assumption-invalidator for wrapper
  `PermissionlessCrank { close_q }` if merged (digest 2026-07-06 §9). Confidence:
  MEDIUM.
- **2026-07-10 — 🔔 crank leg-selection liveness wave (3 OPEN READY PRs, not
  landed).** Same single `permissionless_auto_crank_not_atomic` route; the fixes
  constrain `select_auto_crank_plan`'s *leg self-selection* against production
  lifecycle state, closing three deterministic stall classes a naive keeper hits:
  - **#98** (`codex/fix-recovery-first-autocrank`): crank picked the first active
    leg without lifecycle check → targeting a `Recovery` leg for refresh/liquidate
    → permanent `LockActive` livelock on mixed portfolios. Fix filters to first
    `Active`/`DrainOnly` leg; `liquidatable` now requires ≥1 dispatchable leg.
  - **#99** (`codex/fix-resetpending-autocrank`, stacked on #98): a prior-epoch
    `ResetPending` leg sorting before a live liquidatable leg → permanent
    `InvalidLeg` unless the owner forfeits. Fix classifies it as a bounded
    refresh/cleanup task, detaching ≤1 inert obligation per call (CU-bounded).
  - **#100** (`codex/fix-adl-exit-progress`): partial-ADL basis/OI mismatch →
    `CounterUnderflow`; a dead zero-effective-OI `Normal` leg blocks all later
    reductions. Fix caps work by matched effective OI + unsigned auto-crank
    settles/detaches the residue.
  Each touches `src/v16.rs` + `tests/v16_spec_tests.rs` only; **no wrapper
  wire-format or fee-share change.** Driven by wrapper LiteSVM reproductions
  (wrapper branches `codex/probe-liquidation-recovery`,
  `codex/rebalance-reduce-local-stale`). Signal-type 1d (crank-contract liveness
  hardening). **Status: READY, master unmoved @ `68986f7` — NOT landed.** These
  do NOT add crank surface; they make the existing reference-crank dispatch
  robust. Cranker relevance: HIGH (enumerates keeper failure modes). Confidence:
  HIGH (diffs + PR-body reproductions read directly).
- **2026-07-18 — 🔔 FIRST NEW crank-shaped `pub fn` recorded: `build_auto_crank_plan`
  (engine DRAFT PR #129→ actually PR #127 `codex/fix-recovery-leg-auto-crank-20260715`).**
  Confirmed **absent from `origin/master`**; the branch adds
  `pub fn build_auto_crank_plan(&self, account: &PortfolioV16View<'_>)
  -> V16Result<AutoCrankPlanV16>`, wrapping `build_actionable_summary` +
  `auto_crank_selected_assets` + `V16Core::select_auto_crank_plan` into one call
  that returns the exact plan the engine will accept — so a wrapper/cranker gets
  the engine-selected continuation "without duplicating lifecycle logic" (PR body).
  This is the first *additive* crank `pub fn` since the watch began (prior 1d
  entries were contraction). Directly serves the reference-cranker thesis: the
  keeper can reproduce the engine's plan locally instead of composing selection
  by hand or inferring from state diffs (partial answer to prep.md Q1). PR #127
  also fixes an auto-crank livelock (skip `Recovery` legs; require a dispatchable
  leg before `liquidatable`), superseding older #98. **Status: DRAFT, master
  unmoved @ `143e68c` — NOT landed.** Cranker relevance: HIGH. Confidence: HIGH
  (signature diffed, novelty confirmed vs master). See digest 2026-07-18 §1①.
- **2026-07-18 — anti-grief crank pinning fixes (engine DRAFT #128, #133).** #128
  returns `NonProgress` before mutation when a zero-delta oracle crank would erase
  elapsed time (`slot_last` pinning); #133 preserves the positive-time accrual
  slot so a stale crank can't suppress the honest Hybrid observation via
  landing-order race. Existing crank path, no new surface. DRAFT. Confidence:
  HIGH.

## 1e. Liveness roster additions (A-series obligations)

Baseline: NO `A1`–`A7` enumerated liveness-roster catalog on `origin/master`
`spec.md` (grep for `A[0-9]+` returns no roster). The enumerated permissionless-
recovery list lives as prose at spec.md:1477 (see 1d). PR **#78** Phase 4
("enforced liveness roster — selection and rank-decrease per class") would
introduce the formal roster. Watch PR #78 merge.

_Appended hits:_
- **2026-06-23** — PR #78 MERGED (2026-06-22) but did **not** add a spec-level
  A1–A7 catalog. Its Phase 4 "enforced liveness roster (Pillar L)" landed as
  proof/certification gates (`481b524` enforce Pillar L roster, `ce39cba` no-LoF
  strength roster, `253bb8f` route-fidelity roster) rather than a spec A-series.
  The `AutoCrankPlanV16` variants now enumerate the crank-driven progress paths
  in code instead. No spec A-series roster appeared. Still watching. Reference-model / fuzz harness for a crank path

Baseline: ~265 certified Kani artifacts. Harness:
`scripts/run_kani_full_audit.sh`, `scripts/kani_audit_certified.tsv`,
`src/v16_kani_api.rs`, `kani-list.json`. No `tests/reference_model_conformance.rs`
on master yet, and no Kani harness named for funding/settlement/continuation
crank paths specifically.

**Watch:** a new conformance test or Kani harness naming funding, settlement, or
continuation = a crank path is being proven before it ships.

_Appended hits:_
- **2026-06-23** — NEW `tests/reference_model_conformance.rs` (+468 lines;
  `a223aa9`, `47f4c2b`, `368411c`, `c6a0d18`). Pins the resolved-payout
  `claimable` **settlement** arithmetic `floor(face*rate_num/rate_den)-paid`
  against an independent native-u128 reference (Tier A exhaustive + Tier B
  adversarial sampling over the no-overflow domain) plus U8 bound-num conversion.
  Proves the arithmetic behind the `CloseResolved` crank step before ship. Kani
  harness *count* steady at 398 — strengthening, not new harnesses.
- **2026-06-26** — NEW crank-path proofs/conformance for the no-observation
  dispatch (`f5d291e`, `8338726`): `proof_v16_auto_crank_refresh_is_unique_
  observation_requiring_plan` (tests/proofs_v16.rs, +28), and spec tests
  `v16_auto_crank_liquidates_current_account_without_observation` +
  `v16_auto_crank_progress_realizable_without_observation_for_every_class`
  (tests/v16_spec_tests.rs, +397, helper `assert_observation_independent`).
  Prove the weakened observation precondition is outcome-equivalent to the
  with-obs path for every non-refresh `ActionableState` class — the
  `Liquidate`/`SettleBChunk` crank paths are proven before the relaxation ships.

- **2026-07-01** — Resolved-close crank path made *more capable* + *better
  bounded* (refinement, not new surface). `228d9b3` (PR #85) adds private
  `clear_resolved_close_leg`: a `CloseResolved` crank step can now drain
  *unattributed* bad debt inline instead of stalling into the recovery-fallback
  envelope — one fewer stall-into-`DeclareRecovery` case. Proved by `500ebee`:
  `proof_v16_resolved_unattributed_bad_debt_clears_without_recovery` +
  `proof_v16_resolved_two_active_legs_are_unattributed_for_bankruptcy`
  (tests/proofs_v16.rs). `c8aab33` (PR #86) makes finalized close ledgers inert
  (crank stop condition — a finalized ledger reports no work vs re-entering the
  loop). `1a45760` proves the `ConvertReleasedPnl` preflight freshness gate.
  **No new `cfg_*_chunks` knob, no new `*Phase` enum, no new `pub fn`** — all
  touched fns are private `MarketGroupV16ViewMut` methods. Signal-type 1f
  (proof of an existing crank progress path). Confidence: LOW.

- **2026-07-14 — Keeper liquidation-sizing input REMOVED from the crank action
  (engine PR #92 `b97e174` + wrapper PR #206 `970ddc5`, both merged 2026-07-10).**
  Signal-type: crank-action payload contraction (1d-adjacent — shrinks the public
  crank surface rather than adding to it). `LiquidationRequestV16` went
  `{ asset_index, close_q, fee_bps }` → `{ asset_index }` (src/v16.rs:5404); engine
  now self-selects close size (`liquidation_engine_close_request_q`, src/v16.rs:497)
  and fee (`liquidation_fee_for_close`, src/v16.rs:16408); `liquidation_max_close_q`
  deleted. Wrapper `PermissionlessCrank` variant dropped `close_q` from both the
  variant def and the tag-5 wire codec (`repo-prog` src/v16_program.rs:2447). Net:
  the reference cranker's Liquidate action is now observation-only — keeper picks
  neither size nor fee. Hardens the "keeper supplies oracle accounts, engine
  decides everything else" model. Confidence: HIGH (verified diff both repos).
  New Kani entries `kani_liquidation_engine_close_request_q`,
  `kani_liquidation_fee_for_close`. See digest 2026-07-14 §1/§9.
- **2026-07-14 — Open `[REAL DoS]` auto-crank liveness wave (#98–#120, ~18 open,
  none merged).** Signal-type 1f-adjacent (bounds/liveness of the *existing*
  `permissionless_auto_crank_not_atomic` surface, src/v16.rs:11734). No new knob,
  phase, or `pub fn`. Operator-side reliability of the crank loop. Tracked in
  watchlist.md. Confidence: MEDIUM (open, not landed).
- **2026-07-22 — First EXTERNAL fuzz-harness contribution (PR #137, `julian-osec`,
  open/not-draft).** Extends `tests/v16_fuzzing.rs` (+1590/−61) with more
  instructions, value-range tweaks, slot advancing, and `V16_FUZZ_DIAGNOSTICS=1`
  logging; **end goal is a drive-to-close run — close all accounts + the market,
  verifying invariants along the way**, i.e. a full auto-crank lifecycle
  continuation walk (advance → crank → close). First non-aeyakovenko author on the
  *fuzz* surface (prior externals: proofs/Kani/Z3 only). Convergent with
  aeyakovenko's own fuzz additions this window (#142, #144 touch `v16_fuzzing.rs`;
  #145 adds `tests/backing_double_claim_fuzz.rs`). Signal-type 1f + external
  contributor (priority 6). Confidence: HIGH the PR exists and drives the crank
  lifecycle; MEDIUM on strategic weight (open, not landed). See digest 2026-07-22
  §1/§6.
- **2026-07-22 — `V16_LAYOUT_DISCRIMINATOR` 17→18 now consensus across DRAFT PRs.**
  Second-window confirmation: PR #148 also declares
  `pub const V16_LAYOUT_DISCRIMINATOR: u16 = 18;` (originally in #129). Signal-type
  1c-adjacent (ABI/layout version, not a program-id). Still DRAFT-only; watch for
  the batch to land and force a client layout-version bump.
- **2026-07-30 — DRAFT wave #149–169 (18 new, aeyakovenko codex, none merged):
  crank/settlement *bounding & liveness*, no new surface.** Titles include #165
  "Resolve terminal recovery through auto-crank", #160 "Bound max-source PnL
  conversion", #161 "Bound resolved close work per call", #164 "Carry fractional
  oracle caps across max-dt cranks". Grep battery over the three most crank-shaped
  branches (#161/#160/#165) vs `origin/master`: **zero new `cfg_*_chunks` knob,
  zero new `pub fn crank/advance/settle/apply/continue/build_`, zero new `*Phase`
  enum.** These harden the *existing* auto-crank loop's CU bounds/liveness
  (operator-side reliability, priority 8) without widening the crank API.
  Signal-type 1f-adjacent. Confidence: HIGH (verified grep + branch diffs). 3rd
  consecutive dry-master window. See digest 2026-07-30 §1.
- **2026-07-30 — Codex label scheme escalated `[REAL …]` → `[BLOCKER …]`
  (meta-signal).** Root cause: sole wrapper-repo commit this window,
  `percolator-prog` `36fa587e` (2026-07-24) "Update scripts/loop.md: six-label
  severity scheme". The automation now triages on a 6-tier ladder and surfaces a
  "BLOCKER" class of launch-blocking correctness/DoS defects in crank/settlement
  paths. Leading indicator the engine is being driven toward a launch correctness
  bar — but **nothing merged**; tone, not threshold. Confidence: HIGH (traced to
  the loop.md commit). See digest 2026-07-30 §1②.

---
**2026-08-18 — no new LANDED cranker signal.** Engine `master` frozen @
`143e68c` (5th window). No new `cfg_*_chunks` (1a), no new `*Phase` enum (1b),
no new funding apply/settle symbol (1c), no new `pub fn crank/advance/settle/
apply_*` (1d), no liveness-roster addition landed (1e), no new reference-model/
Kani harness landed (1f). Open drafts #173/#174 carry proof churn but none
merged. Wrapper-side note (indexer, not 1a–1f): `percolator-prog` added
`portfolio_id` incarnation field (`567c76c9`) → distributor must bind
`(market, portfolio, portfolio_id)`; portfolio account layout +8 bytes.
