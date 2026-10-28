# Personal Cranker Prep — Working State

The watcher uses this file to size each digest's "Personal cranker prep" section
against your actual prep state. Edit freely; the watcher reads but does not
overwrite. (The watcher may append a `## Watcher notes` block at the bottom each
run with what changed wrapper-side relative to this state.)

Last reviewed by you: **2026-06-23 — positioning locked: open-source reference cranker + service operator** · **M1 prototype completed same day** · **2026-06-26 — re-pinned M1 against wrapper `acd5c0c` + engine `83387267`, M2 plan revised for the new no-observation-progress surface.** · **2026-07-07 — absorbed 2026-07-01 + 2026-07-06 digests, re-pinned to wrapper `62c8abf` + engine `c8aab33`, M2 cranker built in `./cranker/` and validated end-to-end, Q4 answered (real `AssetOracleProfileV16` walk + Pyth PDA resolver added), Q5 CLOSED, Q6 CLOSED (cranker withdrawal identical to user withdrawal — 1200-atom fee accrual + 100-atom Withdraw verified end-to-end; no portfolio-owner-rotation instruction exists → operator key rotation costs a Close+Init cycle). Only Q1 (no `msg!` for plan variant) + Q2 (v1.52 tooling) remain open; both are low-priority observability/tooling issues.** · **2026-07-13 — drift inspection against upstream `master`/`main`: DRAFT PR #92 MERGED as `b97e174`/`970ddc5` on 2026-07-10, deleting `close_q` from `PermissionlessCrank`. Our client + cranker code now fails to compile against `origin/main`. Wrapper up 6 commits (`62c8abf → 186e9c6`), engine up 6 commits (`c8aab33 → 143e68c`), all inspected. Prep.md assumption block updated to reflect the merged shape; re-pin + code fix deferred pending user go/no-go.** · **2026-09-11 — executed revised M2.i: wrapper `d5e2ec6f` (#135) + engine pin `495a5590` (deliberate wrapper pin, one behind master tip `8eb7142a` / #195). `close_q` deleted; Deposit/Withdraw/Close*/fee-policy bind `portfolio_id` / sequence / `authority_epoch`; layout 18 + A6 one-domain drain loop. Dry-run only. M3 still NO-GO.** · **2026-10-28 — M2.j: re-pinned to wrapper `5cb331dd` + engine `4db11a8c` (prog #456 still draft; engine `68859b94` only as path-patched pre-merge preview). Resolved-mode backing hint, NonProgress = idle, per-account quarantine + cycle budget, `live_shaped` load scenario. Reference / dry-run only. M3 still NO-GO.**

---

## Strategic positioning (decide first; gates everything below)

**Current pick: open-source reference cranker + service operator (hybrid).**
Publish the reference implementation other people can fork, AND run it as a
paid service. Locked 2026-06-23.

What this means for trade-offs:
- **Reference side.** The public implementation needs to be clear, well-
  documented, security-defensible, and an obvious starting point for anyone
  else who wants to operate a keeper. Forks ARE the goal — competing
  forks make the protocol healthier. Optimize for legibility, not moat.
- **Operator side.** Your own production deployment competes with those
  forks on uptime, latency, hint quality, and reliability — operational
  craftsmanship, not code secrecy.
- **The asymmetry to exploit.** You see every fork's improvements (open
  source), and they see yours. Your edge comes from operational data
  (which portfolios actually get stuck, which oracle paths are flaky,
  which fee-share configs are profitable) — not code.

Signals that matter most for this positioning, in priority order:
1. Anything blocking the publishable reference (instruction instability,
   undocumented behavior, missing example code in `percolator-prog`).
2. Anything affecting fee economics on the operator side
   (`liquidation_cranker_fee_share_bps` / `maintenance_cranker_fee_share_bps`
   defaults; governance over them; new fee modalities).
3. Community formation (contributors arriving, forks, example bots
   appearing in `percolator-prog/examples/` or sibling repos). Other people
   doing the same thing = both validation and competition.
4. Deployment-readiness signals (the M3 escalation triggers).
5. Engine API stability (your reference will pin against specific upstream
   versions; bumps cost re-publication).

Considered and not chosen (for the record):
- *Sole/dominant operator* — incompatible with publishing the reference.
- *Best-operator only* — leaves the reference-publication template-setting
  position to someone else.
- *Reference-only* — leaves the operator economics on the table.

---

## Prep milestones (check as you complete; the digest reports unblocked ones)

### M1. Local-validator prototype (target: ~1–2 days focused work) — **DONE 2026-06-23**
- [x] Cloned `aeyakovenko/percolator-prog` to `./repo-prog` (pinned commit
      pushed 2026-06-23T03:58Z).
- [x] Built with `cargo build-sbf --tools-version v1.52`. On 2026-06-23 the
      flag silently downgraded to v1.42 (recorded as Q2). On 2026-06-26 I
      side-loaded platform-tools v1.52 by hand into
      `~/.local/share/solana/install/releases/2.0.21/.../sdk/sbf/dependencies/platform-tools-v1.52`
      (curl from `anza-xyz/platform-tools/releases/v1.52`) — `cargo
      build-sbf --tools-version v1.52` then actually uses v1.52. **Side
      note:** the wrapper's `Cargo.lock` at `acd5c0c` is format-version
      4 and the v1.52 toolchain's bundled cargo only reads up to v3;
      patched in place (`version = 4` → `version = 3`, pinned versions
      preserved) — recorded in the M2 friction notes.
      `cargo test --lib` passes 5/5 unit tests. `tests/v16_cu.rs` selected
      cranker subset (4 tests including
      `v16_cu_permissionless_crank_refresh_is_bounded`) passes; full
      suite is 538 tests not run here for time. `tests/v16_kani.rs` is
      `#[cfg(kani)]`-only — needs separate Kani toolchain; not run.
- [x] Deployed program to `solana-test-validator` at id
      `3bgLEkBtCM4LoA89XnP26UefYACrWi1CkiwvgMtJxWzx` (the build-time
      keypair under `target/deploy/`). **The wrapper has no `crate::ID`
      runtime check**, so the placeholder `declare_id!` is irrelevant
      for local-validator deploy — confirmed by successful crank.
- [x] `InitPortfolio` + `Deposit` of 1_000_000 atoms confirmed on-chain.
      A "stuck" state was *not* explicitly synthesised — the engine's
      bounded refresh path runs on a fresh portfolio too, which is
      sufficient for round-trip verification. Real stuck states (stale
      oracle / b-stale dispatch / expired close) deferred to M2.
- [x] `Instruction::PermissionlessCrank` round-trip executed, consumed
      **37_375 CU** at the M1 pin (c51bf6b + 91a46c0); re-pinned on
      2026-06-26 to wrapper `acd5c0c` + engine `83387267`, the same
      round-trip consumes **37_716 CU** (+341, ~0.9%), consistent with
      the cost of the new `obs_or_current_asset` synthesis closure and
      the predicate check the upstream commits added. Re-pinned again
      2026-07-07 to wrapper `62c8abf` + engine `c8aab33` (wrapper's
      own engine pin, not master `68986f7` — wrapper hasn't tracked
      forward yet): no-op crank now consumes **42_817 CU** (+5_101,
      ~+13.5% vs 2026-06-26). The jump is attributable to the two new
      pre-flight guards `6869e58` "Reject stale selected-mark auto
      cranks" and `8067a13` "Freeze stale-window auto-crank liquidation"
      that run on every crank, even the no-op refresh path. Still
      well under the 1.4M tx CU limit. Tx logs
      (`solana confirm -v`) show only generic "Program invoke /
      success" — **the wrapper does NOT emit a `msg!()` identifying the
      selected `AutoCrankPlanV16` variant.** A cranker bot cannot scrape
      logs for plan/outcome; it must infer from state diffs. Recorded
      in `cranker-strategic-questions.md` Q4.
- [x] **M1 re-pin observation (incidental).** `MARKET_GROUP_OFF` grew
      from 448 to **464** bytes and `WRAPPER_CONFIG_LEN` from 432 to
      **448** bytes between c51bf6b and acd5c0c. The public instruction
      surface is unchanged (hint shape, observation cap, fee field
      names, account ordering, declare_id all identical), so my client
      reads sizes via `state::market_account_len_for_capacity(...)` and
      picked them up automatically — no client edit needed. Likely a
      new internal wrapper-config field landed in one of the 9 wrapper
      commits (CPI/matcher-binding hardening or fee-policy gate);
      tracked but not blocking. Watcher should diff `WrapperConfigV16`
      next run.
- [x] Fee-collection routing verified via maintenance fee path:
      `maintenance_fee_per_slot=100` + `UpdateMaintenanceFeePolicy(
      cranker_share_bps=5000)` + `SyncMaintenanceFee` with cranker
      portfolio appended → both the user portfolio (4 bytes diff) and
      the cranker portfolio (2 bytes diff) mutated. Exact atom amounts
      not decoded yet (needs `PortfolioV16View` parsing) — M2 follow-up.
      The append-portfolio-last convention is confirmed wired.

**Bring-up:** `./scripts/m1_bringup.sh` is idempotent (resets ledger +
re-deploys + runs client). Client lives in `./client/` and is built
against `repo-prog` as a path dep.

### M2. Minimal cranker loop (target: ~1 week after M1) — **REVISED 2026-06-26 · DONE 2026-07-07**

The order below was reshuffled after upstream 3d250d2 / f5d291e / 8338726:
the no-observation clear path is now first (it can drive `Liquidate` and
`SettleBChunk` from committed state without any oracle wiring), and the
authenticated-observation pipeline is gated to the single plan variant
that still strictly requires one (`RefreshAccount{None}`).

- [x] **M2.a — Portfolio discovery.** `getProgramAccounts` filter against
      the local validator to enumerate `KIND_PORTFOLIO` accounts; rank
      by a cheap "looks-stuck" heuristic (capital below maintenance
      threshold; close progress non-zero; slot since last accrual large).
      Discovery is oracle-independent and can land first.
- [x] **M2.b — No-observation clear path (the new fast track).** Build
      and submit `PermissionlessCrank` with `observations: vec![]` (or
      a single asset-only `CrankObservationHint{asset_index, oracle_accounts:
      0}` with zero oracle tail) against a discovered stuck account.
      Engine should select `Liquidate` / `SettleBChunk` / `RefreshAccount{Some}`
      and progress from committed state. Verify mutation via state-diff
      check (same byte-diff probe as M1 used for the fee path). This
      replaces what was previously the "oracle pipeline first" assumption.
- [x] **M2.c — Plan-classification gate.** Wrap every submission decision
      in a call to `percolator::auto_crank_plan_requires_caller_observation`.
      If the heuristic predicts `RefreshAccount{None}` is the only useful
      next step for an account, the cranker MUST fetch a fresh
      observation before submitting; otherwise it may skip the oracle
      hop. Do NOT hand-roll the variant match — upstream pins the
      truth-table with proofs, and a hand-roll silently rots when a new
      plan variant lands.
- [x] **M2.d — Authenticated-observation pipeline (now narrower-scope).**
      Wire up Pyth/Switchboard/Chainlink-account fetching for the
      `RefreshAccount{None}` path only. Stubbed first (hard-coded
      Pyth feed id + faked price account on the local validator), real
      account-set second. The earlier baseline plan assumed every
      submission needed this — it doesn't anymore.
- [x] **M2.e — Hint-budget selection.** Choose up to 16 hints per call
      (`CRANK_OBSERVATION_DECODE_MAX`). With the no-obs clear path
      available, the budget mostly carries asset-routing pointers, not
      oracle-account fan-outs. Document the policy chosen.
- [x] **M2.f — Retry / NonProgress loop with NEW safety rule.**
      *Old rule* (now invalid): "no-observation submissions are
      idempotent because the engine returns NonProgress without
      mutation, so retry on transient error is free."
      *New rule (2026-06-26):* a same-tx retry of a no-observation
      submission CAN produce a second mutation when the selected plan
      is `Liquidate`, `SettleBChunk`, or `RefreshAccount{Some}` —
      because `obs_or_current_asset` re-synthesizes a fresh observation
      from whatever the asset state is now. Concretely:
      * **Liquidate is idempotent only in the "account already healthy"
        sense.** A second Liquidate against a still-unhealthy account
        will run again, charge fees again, and move the cranker reward
        balance again.
      * **SettleBChunk likewise advances the B-cursor each call.**
        Replaying the tx until success ⇒ duplicate progress, not a
        clean no-op.
      * **RefreshAccount{Some} mutates the committed-state copy each
        call** when the selected asset moved.
      * **Only `RefreshAccount{None}` is the old-style clean no-op**
        (still returns NonProgress without an observation) and the
        only place blind retry is safe.
      Implementation rule: before retrying ANY submission whose
      pre-flight plan classification was anything other than
      `RefreshAccount{None}`, **re-read the target portfolio's account
      data** and re-decide. If the post-state already reflects the
      progress you wanted, abort the retry. Cheaper to spend an RPC
      round-trip than to double-pay a liquidation crank.
      * **NEW 2026-07-07 (wrapper-side hard-rejects — 2026-07-01
        digest).** The wrapper at `62c8abf` added two `PermissionlessCrank`
        guards that don't `NonProgress` — they *fail the whole tx*:
        - `6869e58` "Reject stale selected-mark auto cranks" — the
          crank hard-rejects when the pending selected asset lacks a
          matching observation (`reject_missing_pending_selected_observation_view`,
          call-site at `src/v16_program.rs:10608` on wrapper `main`).
        - `8067a13` "Freeze stale-window auto-crank liquidation" —
          liquidation is blocked when the per-slot price movement
          exceeds `max_price_move_bps_per_slot`.
        Consequence: `NonProgress` is not the only "you got nothing"
        outcome — the tx can now come back with a wrapper error too.
        The retry loop must distinguish `Ok(no state change)` from
        `Err(hard reject)` from `Ok(state change I don't want to
        repeat)`. All three shapes exist now; retry policy must
        classify by looking at BOTH the tx status AND the pre/post
        state hash. This layers on top of M2.c's plan-classification
        gate; it doesn't replace it.
- [x] **M2.g — Mirror-custody bookkeeping.** Persist `(account, slot,
      pre-state-hash, post-state-hash, did-progress)` per submission
      and reconcile against expected fee accrual on the cranker
      portfolio. Replaces the earlier "read result.selected" plan —
      the wrapper doesn't expose `AutoCrankResultV16` in tx return
      data or logs (per M1 finding), so reconciliation is state-diff
      based, not result-based.

**Friction note (toolchain):** the new wrapper-side dep graph at
`acd5c0c` requires platform-tools v1.52 (deployable build) and the
`Cargo.lock` is format-version 4. The bundled cargo on platform-tools
v1.52 still reads only up to v3. M2 client builds need either a
v3-downgraded lockfile (what I did locally — `sed -i '' '3s/version =
4/version = 3/' Cargo.lock`) or a newer Agave bundle. Worth a
once-only fix-up in `scripts/m1_bringup.sh` if this persists.

- [x] **M2.h — Re-pinned 2026-07-07.**

- [x] **M2.i — Re-pin required (drift refresh 2026-09-11; supersedes 2026-07-13 plan). DONE 2026-09-11.**
      The 2026-07-13 targets are **stale**. Do **not** re-pin to wrapper
      `186e9c6` / engine `143e68c` — those tips are now **~716 / ~123**
      commits behind current `origin/main` / `origin/master`.

      **Current upstream (fetched 2026-09-11):**
      - Engine `origin/master` = `8eb7142aada316f6c476f5c4fa815d3a806706d5`
        (merge `#195`, 2026-09-03). Thaw after weeks frozen at `143e68c4`.
      - Wrapper `origin/main` = `d5e2ec6fa727a1049a62851c3be19c638815b4e2`
        (merge `#135`, 2026-09-03). Cargo engine pin =
        `495a5590c97055bd71c6f94d849ff0298f243145` (parent of `#195` merge;
        one commit behind master tip).
      - Our tree **was** at wrapper path-dep `62c8abf0` (2026-06-28) and
        cranker Cargo engine rev `c8aab338…`. Re-pin landed 2026-09-11
        (see completion note below).

      **Still true from the 2026-07-13 finding (re-verified on `origin/main`):**
      - `Instruction::PermissionlessCrank { now_slot, observations }` —
        **`close_q` remains deleted**. Mechanical delete applied 2026-09-11.

      **New breakages / obligations since 2026-07-13 (must fold into re-pin):**
      1. **Engine plan surface grew.** `AutoCrankPlanV16` on master now
         includes `ReleaseSourceLiens`, `AdvanceClose`, `FinalizeRecovery`
         (plus prior variants). Roster is **A1–A8** with
         `source_liens_releasable` = A6 and `resolved_winner` = A8
         (`ActionableSummaryV16`). `plan_gate` / any exhaustive match
         must cover the new variants; A6 drain is
         `V16_SOURCE_LIEN_RELEASE_CHUNK_DOMAINS = 1` (one domain/call).
      2. **Layout discriminator = 18** (`V16_LAYOUT_DISCRIMINATOR`).
         Account readers assuming 17 are wrong.
      3. **Spec v16.9.1** — whole-account K/F order is normative
         (negatives first; source domain + canonical slot). Crank firing
         order is no longer a same-refresh K/F PnL lever.
      4. **Wrapper `#135`:** `portfolio_id` / incarnation threading is on
         `main` across many ix paths; `PORTFOLIO_ACCOUNT_LEN` formula
         changed vs our June pin. Indexer/distributor and any fixed-len
         portfolio decode must bind `(market, portfolio pubkey, portfolio_id)`.
      5. **Public helpers on master:** `advance_terminal_slab_not_atomic`,
         `advance_resolved_slot_not_atomic`,
         `force_close_recovery_pair_not_atomic` — optional for M2 loop,
         relevant for M-recovery / terminal paths.
      6. **Open post-thaw risk (do not treat master as wrapper-safe yet):**
         engine `#197`–`#200` still open (receipt/obligation/source-capacity
         / side-OI). Prefer re-pin target = **wrapper `main` + its Cargo
         engine pin (`495a5590`)** unless you explicitly want master tip
         `8eb7142a` and accept the one-commit skew.

      **Revised actions before this checkbox can be ticked:**
      1. Delete `close_q: …` from all `PermissionlessCrank { … }` literals.
      2. Bump `./repo-prog` to `origin/main` (`d5e2ec6f…`) and set
         `percolator` git rev in `cranker/Cargo.toml` + `client/Cargo.toml`
         to the wrapper's pin `495a5590…` (or document a deliberate
         master-tip override).
      3. Re-apply `Cargo.lock` v4→v3 patch on `./repo-prog` if SBF
         tooling still requires it.
      4. Extend plan coverage for A6–A8 / new plan variants; implement
         one-domain lien-release loop; bump layout-18 decodes.
      5. Rebuild SBF + redeploy test-validator; re-run
         `scripts/m1_bringup.sh` and `cargo run --release` in `cranker/`;
         expect CU band to move — re-baseline, do not assume ~42_817.
      6. Keep M3 gated: placeholder `declare_id!`, RPC null (see Watcher
         notes).

      **Explicitly deferred (not part of M2.i):** funding apply/settle
      crank (still absent), paid live service / fee-share defaults (still 0),
      mainnet deploy.

      **M2.i completion (2026-09-11):**
      - Pins: wrapper `./repo-prog` = `d5e2ec6fa727a1049a62851c3be19c638815b4e2`
        (`origin/main`, merge `#135`); engine git rev in
        `cranker/Cargo.toml` + `client/Cargo.toml` =
        `495a5590c97055bd71c6f94d849ff0298f243145` (wrapper Cargo pin, **not**
        master-tip override `8eb7142a`). `./repo` checked out to the same
        engine pin. `repo-prog/Cargo.lock` kept at format v3 for SBF v1.52.
      - `close_q` gone from every `PermissionlessCrank` literal; wire is
        `[tag=5 | now_slot(8) | n(1) | hints]`.
      - Plan gate delegates to
        `auto_crank_plan_requires_caller_observation` for A6
        `ReleaseSourceLiens`, `AdvanceClose`, `FinalizeRecovery`, A8
        `CloseResolved`. Inner drain loop: one domain per crank until
        `Ok(no-mut)` / `HardReject`, cap
        `WRAPPER_MAX_BOUNDED_SOURCE_DOMAINS`.
      - Decode/submit bind layout 18 and wrapper `portfolio_id` (Deposit /
        Withdraw / ClosePortfolio / CloseSlab / ResolveMarket / fee-policy
        `policy_sequence` + `authority_epoch`).
      - Dry-run: `FORCE_SBF_REBUILD=1 ./scripts/m1_bringup.sh` then
        `PERCOLATOR_PROGRAM_ID=<id> cargo run --release` in `./cranker/`.
        No-op `PermissionlessCrank` **17_975 program CU** (tx total 18_299)
        — do not assume ~42_817. Fresh MANUAL deposit portfolio is already
        idle on this pin (3-cycle cranker run: all `Ok(no-mut)`).
      - Still reference / dry-run only. M3 = NO-GO (placeholder
        `declare_id!`, RPC null). Fee shares remain test-only (bootstrap
        sets 5000 bps locally; upstream defaults still 0).

- [x] **M2.j — Re-pin to the upstream "launch fixes" + stuck-account hardening. DONE 2026-10-28.**

      **Pins (verified with `git rev-parse` + `Cargo.toml` on 2026-10-28):**

      | Surface | Old (M2.i) | New default (Step A) | Pre-merge preview (Step B) |
      |---|---|---|---|
      | Wrapper `percolator-prog` | `d5e2ec6f` | `5cb331dde354517c6371a8acf92cecb194f3bb73` (`origin/main`) | `febc3731b7653544179a529e5a529ff137d5648f` (prog #456 head, `fix/repin-launch-fixes`) |
      | Engine `percolator` rev | `495a5590` | `4db11a8cb0053815e23a35d3a7d3edc265d8d866` (wrapper Cargo pin) | `68859b94f58eb8e580ee685c933a49d3ccaffbfe` (master, 2026-10-27 merge batch), **path-patched** |

      - Target rule: prog #456 is still an **open draft** (stack #452 → #454
        → #456, none merged), so the default is Step A. Step B runs only via
        `scripts/preview_launch_fixes.sh` (cwd-scoped `.cargo/config.toml`
        `[patch]` under `.preview/`, gitignored). It is labeled "pre-merge
        preview" and is never the default. When #456 merges: set `repo-prog`
        to the new `origin/main`, copy its Cargo engine rev into
        `cranker/Cargo.toml` + `client/Cargo.toml`, `cargo update -p
        percolator`, re-run bring-up + 3 cycles.
      - Stop-checks passed: `V16_LAYOUT_DISCRIMINATOR = 18` and the
        `AutoCrankPlanV16` enum are byte-identical at `495a5590`, `4db11a8c`
        and `68859b94`. No new plan class, chunk knob or phase enum.

      **Upstream diff absorbed** (`git diff d5e2ec6f..5cb331dd -- src/v16_program.rs`;
      engine `495a5590..4db11a8c` is 5 commits, `4db11a8c..68859b94` 60):
      1. Resolved tag 5 takes ≤ 1 hint with `oracle_accounts == 0` →
         `AutoCrankObservationV16 { asset_index, 0, 0 }`; engine advances one
         backing bucket or returns NonProgress. Wire encoder unchanged; dispatch
         now sends the due-bucket asset (`resolved.rs`, deadline =
         `min(expiry_slot, resolved_slot + max(max_accrual_dt, h_max,
         max_bankrupt_close_lifetime, 1))`, lapsed **Fresh** buckets only).
         `EngineNonProgress` = idle. Resolved tag 5 routes through
         `handle_close_resolved`, so account 0 = portfolio owner (signer only
         inside `force_close_delay_slots`) and slots 3–6 carry the payout
         (owner ATA, vault, vault authority, token program).
         `CrankRoute::Resolved` builds that layout. Inside the window,
         third-party portfolios are skipped as idle.
      2. Recovery crank bumps `position_epoch` (`b40ffbd8`).
         - The production crank path takes no binding (tag 5 has no epoch).
           Owner instructions re-read the binding from fresh bytes before
           sending.
         - Observed in the (since separated) `live_shaped` load test: a
           Recovery-asset force-close bumps the epoch, the stale binding is
           rejected, and the fresh one is accepted. Not exercised by PR #1's
           own tests.
         - Not reached: the market-wide Recovery crank path itself.
         `SetMatcherConfig` now also binds `position_epoch` +
         `asset_generation_frontier` (cranker doesn't send it).
      3. No layout/plan change (above).
      4. Exact funding (engine #227, only in `68859b94`): sub-atom F lag =
         F-settled; `work::leg_f_settled` keeps such legs out of pending work.
      5. Provider surplus withdraw (engine #223 / prog #454) rides the existing
         `WithdrawBackingBucket` tag in Resolved mode when signed. Not a crank;
         the cranker never calls it; nothing new to decode.
      6. Live OI symmetry: nothing in cranker code/tests asserts it.

      **Fees (since round 2):** off by default. The default bootstrap has no
      maintenance fee and zero cranker shares, and cranks attach no reward
      portfolio. `CRANKER_ENABLE_FEE_SHARES=1` opts in (50% shares);
      `CRANKER_WITHDRAWAL_TEST=1` also requires it.

      **Hardening:** per-account quarantine (same error × 3 → backoff 2, 4, 8 …
      capped at 64 cycles, JSON event line); per-cycle wall/CU/tx budget with
      liquidatable-first ordering and per-class rotation; Progress / Idle /
      Failed reported separately in every cycle summary. Unit test
      `scheduler::persistent_error_is_quarantined_and_others_progress` injects
      a persistent `EngineCounterUnderflow` on one account for 20 cycles: the
      5 others progress every cycle; the stuck one is tried < 10 times.

      **Dry-run (2026-10-28):** `FORCE_SBF_REBUILD=1 ./scripts/m1_bringup.sh`
      green; `CRANKER_CYCLES=3 cargo run --release --manifest-path
      cranker/Cargo.toml` green (3 × idle). Preview script green (3 × idle).

      **CU re-baseline (program CU, tx total in parens):**

      | Path | Old pin | Step A | Step B preview |
      |---|---|---|---|
      | No-op `PermissionlessCrank` | 17,975 (18,299) | **18,565 (18,889)** | 18,820 (19,144) |
      | No-op, 3 hints (live_shaped) | — | 41,695 (42,019) | 42,179 (42,503) |
| Worst landed crank (live_shaped) | — | **409,225** | 402,701 |
| Worst reverted crank (live_shaped) | — | 261,535 | 319,008 |

      Upstream reference: prog #456 pins max-shape paths at ~566k
      (`v16_cu`, 566_314 after `febc3731`).

      **Load test `live_shaped`** (HISTORICAL: moved out of PR #1 to branch `claude/live-shaped-scenario` on 2026-10-28; the results below are kept as findings, not as PR #1 acceptance) (`scripts/live_shaped.sh`, or
      `CRANKER_SCENARIO=live_shaped`). Seed `0x5eed1a7e20261028`, 40 traders
      plus 3 makers, 3 AuthMark assets, 200 cycles, all driven by the real
      `cycle::run_one_cycle`. Covered in the run: liquidations, lapsed
      buckets, cross-asset netting, nonzero funding on all assets, asset 2
      Recovery (shutdown and force-close), Resolve, every exit, and
      CloseSlab. **Not reached:** an Impaired bucket, and market-wide
      Recovery.

      | Assertion | Step A (`4db11a8c`), 782 s | Preview (`68859b94`), 805 s |
      |---|---|---|
      | a. no account unrefreshable past the quarantine window | **FAIL**: 69 events, longest failure run 81 cycles against a bound of 67 (F1 + F1b) | **FAIL**: 72 events, all `EngineStale`, run 79 against a bound of 67 (F1 only) |
      | b. every resolved claimant paid | **FAIL**: 1 of 44 (t28, equity 137,486) unpaid; it was wedged by F1 | PASS (43 owners paid) |
      | c. vault holds only dust after exits | **FAIL**: 13,482,914 left against 8,000,528 allowed (lapsed principal + dust); ≈5.48M unexplained (F2) | **FAIL**: 9,127,703 left against 9,000,528 allowed + 48,674 insurance that couldn't be withdrawn; ≈78.5k unexplained; CloseSlab rejected `LockActive` |
      | d. position epoch re-read after Recovery | PASS: epoch 1 → 2; ClosePortfolio with the stale epoch rejected `EngineStale`; re-read accepted | PASS |
      | e. coverage | Impaired bucket missing | Impaired bucket missing |

      **Re-run after the PR #1 review fixes (Step A, 200 cycles).** Changes
      since the table above: assertions tightened, keys seeded, and a
      keeper-only Resolved cohort added (check f). Results:
      - a PASS (longest failure run 35 cycles; bound 67);
      - b PASS: no senior-capital shortfall;
      - c FAIL: 1,543,750 atoms unexplained, and the market account was not
        closed;
      - d PASS;
      - e FAIL: no Impaired bucket;
      - f: 7 of 8 cohort members closed through the keeper's Resolved route,
        each paid at least its capital. The eighth was already terminal at
        resolve, a check bug that is now fixed and re-passed at 40 cycles.

      These numbers move from run to run with validator slot timing. The
      action-plan digest pins the inputs.

      Cranker totals (Step A): 2,079 txs; 634 progressed, 660 idle, 163
      failed; 381 resolved hints sent. The failures sit on F1-wedged
      accounts. Quarantine kept every other account progressing; that part
      is what the cranker controls, and it held.

      ### Found in load test (F1 posted upstream as #243 by the owner; the rest private)

      Full write-ups, native replays and upstream cross-references are in
      [`cranker/scenario-findings.md`](cranker/scenario-findings.md).

      **Posting upstream (owner decision, 2026-10-28): F1 only.** Posted as
      [aeyakovenko/percolator#243](https://github.com/aeyakovenko/percolator/issues/243)
      with a LiteSVM repro on both pins. F1b, F2 and the rest stay private.

      - **F1: a lapsed bucket that no account references wedges Live refresh
        and liquidation (`EngineStale` 0x13).**
        - **Trigger.** A Fresh bucket goes past `expiry_slot` and no account
          references it. A refresh that would attribute a winner's new
          positive PnL to that domain hits `validate_source_domain_ledger_current`,
          which returns Stale. Every crank, liquidation and force-close on
          that account then reverts until Resolve.
        - **Why Live can't recover.** Only the Resolved `CloseResolved` arm
          scans buckets at the asset level, so nothing in Live expires one.
        - **Repro** (one asset is enough):
          1. Open a long and a short.
          2. `TopUpBackingBucket{domain: 1, expiry: now+20}`.
          3. Wait past expiry, then push the mark +10%.
          4. Crank the long: `0x13` every slot.
        - **Replay.** Expiring that one bucket first makes the refresh pass,
          and the account then shows `certified_liq_deficit = 3782`. So a
          liquidatable account can't be liquidated.
        - **Status.** Still present on `68859b94`. It belongs to the
          engine #102/#173/#179/#196 and prog #204 family, but those landed
          only account-scoped expiry. No open issue covers the
          unreferenced-domain case.
      - **F1b: `EngineLockActive` on the same accounts.** Gone on
        `68859b94`; consistent with engine #216 (merged 2026-10-27).
      - **F2: unexplained residue after every exit; CloseSlab burns it.**
        - **Step A:** ≈5.48M atoms unexplained. Contributors:
          - Resolved `WithdrawBackingBucket` on a never-lapsed domain
            released 1 atom out of 1,000,000.
          - `ClaimResolvedPayoutTopup` was rejected `EngineLockActive` 290
            times.
        - **Preview:** ≈78.5k unexplained, and CloseSlab was refused.
        - **Related open upstream items:** engine #206/#209/#210, prog
          #451/#454.
        - No minimal repro yet.


---

**M2 completion notes (2026-07-07):**
The cranker binary lives at [`./cranker/`](cranker/). Full docs (what's
real, what's stubbed, how to read a run) in
[`cranker/README.md`](cranker/README.md).

Empirical results from a 5-cycle run against a fresh
`solana-test-validator`:

```
cycle  outcome      tgtΔ  rwdΔ   pre_hash          post_hash
0      Ok(mut)      4     0      affb2381e37c959b  abeb2b4b6a92b1df
1      Ok(no-mut)   0     0      abeb2b4b6a92b1df  abeb2b4b6a92b1df
2      Ok(no-mut)   0     0      abeb2b4b6a92b1df  abeb2b4b6a92b1df
3      Ok(no-mut)   0     0      abeb2b4b6a92b1df  abeb2b4b6a92b1df
4      Ok(no-mut)   0     0      abeb2b4b6a92b1df  abeb2b4b6a92b1df
```

* Cycle 0 mutates 4 bytes on the target portfolio — engine synthesizes
  observation from committed state (per the 2026-06-26 upstream
  relaxation) and dispatches a progress step. Verifies M2.b.
* Cycles 1–4 all `Ok(no-mut)` with identical `pre_hash == post_hash` —
  the wrapper is swallowing engine `NonProgress` into a successful tx
  with no state change (`Err(V16Error::NonProgress) if !observations.is_empty() && close_q == 0 => None`,
  `repo-prog/src/v16_program.rs:10626`). Verifies M2.f's three-way
  classifier empirically.
* `reward_diff = 0` throughout is honest: no liquidation happened, so
  `retained_fee = 0` and `retained_fee * cranker_share_bps / 10_000 = 0`.
  The reward-portfolio append path is exercised end-to-end (wrapper
  accepts the appended writable portfolio because
  `liquidation_cranker_fee_share_bps = 5000` was set at bootstrap);
  the null reward is the correct signal for a market with no
  liquidations, not a bug.

Findings the M2 build surfaced (fold into open questions / watcher
targets):

- **Reward-portfolio append is gated by `liquidation_cranker_fee_share_bps
  != 0`.** With the default (bps=0), the wrapper does NOT split the
  last account off as reward — instead treats it as an unaccounted-for
  oracle tail account and rejects `InvalidInstruction (0x9)`. So the
  M1-verified "reward portfolio is optional, omit to leave in insurance"
  language in the README applies ONLY when the config bps is nonzero.
  If bps == 0 you MUST NOT append; if bps > 0 you MAY append. This is
  a strictly narrower rule than the README suggests — worth surfacing
  to the watcher as an assumption to check for regression.
- **Reward-portfolio owner MUST equal the tx signer** (`Unauthorized 0x8`
  otherwise). `expect_signer(owner)` + `reward_owner != owner.key.to_bytes()`
  at `repo-prog/src/v16_program.rs:10473,10485`. Implication for
  production cranker: the operator keypair is BOTH the tx payer/signer
  AND the reward-portfolio owner. Cannot split roles trivially.
- **Discovery must be market-scoped locally.** `getProgramAccounts`
  memcmp'd on `[MAGIC | VERSION | KIND_PORTFOLIO]` returns portfolios
  from every market the program has ever seeded on the ledger — including
  leftovers from prior bringups. Filtering by target market via each
  candidate's provenance `market_group_id` (offset `HEADER_LEN..+32`)
  is required, or every cross-market submission wastes tx fees on
  `EngineProvenanceMismatch (0x10)`.
- **`Ok(no-mut)` is what the wrapper looks like in the current
  no-observation NonProgress case.** For M2.f purposes this means:
  the response classifier that keys off tx status + state-diff is
  correct. The wrapper does not surface `EngineNonProgress (0x16)`
  as a distinct code in the normal path — that variant only leaks
  through when `observations.is_empty() || close_q != 0`, both of
  which the cranker avoids by construction. **Q5(i)** is now partially
  answered: a keeper CAN'T rely on the tx status alone to distinguish
  no-op from success in the swallowed case; it MUST diff state. Wrapper `acd5c0c → 62c8abf`,
      engine `83387267 → c8aab33` (the wrapper's own engine pin;
      master `68986f7` is 4 proof-only commits ahead but the wrapper
      doesn't track that far). Public surface confirmed unchanged
      end-to-end via bringup. Both new hard-reject guards
      (`6869e58`/`8067a13`) run pre-flight on the no-op refresh path
      without failing, so the M1 client already threads them; M2.b
      and M2.f will be written against current guard semantics
      (no false-green risk).

### R1. Public reference release (new repo): plain keeper only. **DONE 2026-10-28: published as `Augustas11/drip` v0.1.0.**
- [x] Scope (owner decision): keeper core, decoding, tx builders, quarantine
      and budget, docs, CI. The operator stack stays private: staging,
      bots, matcher, backing manager, playbook, devnet and hosting scripts,
      the monitor, and the load test. `export_public.sh` never copies it
      (allowlist manifest); the denylist names it for the leak helper.
- [x] Plan, name options and sync proposal: `docs/PUBLIC_REPO_PLAN.md`.
- [x] Allowlist export plus a simple leak helper (owner decision: the
      helper is advisory, not a security boundary). The private denylist
      (`scripts/public_denylist.txt`) is never exported.
- [x] Owner reads the exported tree before the first push (required).
- [x] Same CI file in both repos (layout-neutral via `scripts/env.sh`).
- [x] Owner decisions: name `drip`, Apache-2.0, GitHub private
      vulnerability reporting. The keeper crate and binary are renamed
      `drip` in this repo.
- [x] Published: https://github.com/Augustas11/drip, one initial commit
      `ae8845f` (the export of private `050b0b2`); CI green there,
      including e2e by dispatch; tag `v0.1.0`.
- Sync model: this repo stays the code home; drip is updated only by
      re-running the export.

### R2. Local staging market as an operator rehearsal (private). Reference / dry-run only; not a paid live service; M3 NO-GO.
- [x] `staging/` crate: one SOL-PERP market (SOL/USD on AuthMark, test USDC
      collateral); price pusher (swappable `mock_sol_usd` source), LP
      matcher (the only LP, 10 SOL cap), backing manager (roll before
      expiry), insurance and the real keeper cycle as bots; once per run
      SOL goes Live -> Recovery (SHUTDOWN) -> Resolved -> relisted; JSON
      summary and an exit code (`docs/STAGING.md`).
- [x] `scripts/staging_local.sh` (own validator on 8799, run dir
      `.staging-runs/`); 10-minute smoke run passes.
- [ ] Owner's 6-hour unattended run exits 0.
- [x] `docs/OPERATOR_PLAYBOOK.md` (roles, SOL-PERP parameters and reasoning,
      bots, runbook, operator income).
- [x] `scripts/staging_devnet_deploy.sh` (SOL-PERP, test USDC mint, Pyth
      SOL/USD feed id as a parameter; prints steps, `--yes` to act, refuses
      mainnet), `scripts/staging_usdc_faucet.sh`, and
      `deploy/systemd/percolator-staging@.service`. Nothing deployed; neither
      devnet script has been run.
- [ ] Later option: Pyth SOL/USD read in the pusher on devnet; a direct Pyth
      (Hybrid) profile.

### M3. Pre-deployment readiness (only relevant once devnet deployment appears)
- [ ] Real keeper portfolio funded on the target chain.
- [ ] Oracle account subscriptions wired (not stubbed).
- [ ] Monitoring: `NonProgress` rate, fee-capture rate, observation freshness.
- [ ] Disaster paths: keeper-portfolio drained, hostile matcher, oracle stale.

---

## Baked-in assumptions (the watcher will flag wrapper-side changes that
## invalidate these)

Fill these in as your prototype hardens. Each line is the assumption the
watcher should re-check against the wrapper diff each run.

### Instruction shape
- **Instruction variant name (UPDATED 2026-09-11):** `PermissionlessCrank
  { now_slot, observations: Vec<CrankObservationHint> }` — **`close_q`
  deleted** (wrapper `970ddc5` / engine `b97e174`; re-verified on
  `d5e2ec6f`). Tag=5 wire:
  `[tag=5(1) | now_slot(8) | n(1) | (asset_index(2) oracle_accounts(1))*n]`.
  Our literals no longer carry `close_q`. Deposit / Withdraw /
  ClosePortfolio now bind `(portfolio_id, expected_sequence[,
  position_epoch])`; fee-policy / ResolveMarket / CloseSlab bind
  `(policy_sequence, authority_epoch)` / `asset_generation_frontier`.
- **`now_slot` is non-authoritative** — wrapper authenticates from the
  Clock sysvar; caller-supplied value is ignored. Test
  `v16_bpf_permissionless_crank_uses_authenticated_clock_slot_not_caller_slot`
  is the proof. **Cranker bots should still send a sensible value** (the
  test asserts the wrapper rejects spoofs but doesn't require a specific
  format).
- **Observation cap:** `CRANK_OBSERVATION_DECODE_MAX = 16` — unchanged.
- **✅ RESOLVED assumption invalidator (PR #92 MERGED 2026-07-10, drift check 2026-07-13).**
  What we flagged as PENDING on 2026-07-06 has landed. Engine commits
  `b97e174` (PR #92) + `2cbca3e` (PR #89) + `876fa9f` (PR #94), wrapper
  commits `970ddc5` (PR #206) + `e502653` (PR #191) + `392a376` (PR #184).
  **The `close_q` field is deleted, `LiquidationRequestV16` collapsed
  to `{ asset_index: usize }`, `AutoCrankWorkV16` collapsed to
  `{ now_slot, observations, resolved_close_fee_rate_per_slot }`.**
  Engine computes minimum close size itself; caller submits observations
  only. Since our M1 client and M2 cranker both sent `close_q: 0`
  already, the semantic behaviour is unchanged — but the field is gone
  from the encode/decode pair, so the code needs a mechanical fix at
  re-pin time (delete the field from three call sites). Once the
  re-pin lands, this bullet can be reduced to a one-line summary and
  moved into an "instruction-shape history" appendix if desired.
- **Hint shape (CORRECTED from baseline):**
  `CrankObservationHint { asset_index: u16, oracle_accounts: u8 }`
  (`src/v16_program.rs:2394`). **The wrapper hint carries NO prices or
  funding** — it only routes which asset + how many tail oracle accounts
  belong to that hint. The wrapper itself reads price/funding from the
  appended oracle accounts (Pyth / Switchboard / Chainlink discriminated
  inside the wrapper). This is a **major correction** to the baseline
  read and reshapes the cranker design: keepers don't sign prices, they
  supply *oracle accounts* whose contents the wrapper authenticates.
- **The engine's `AutoCrankObservationV16`** still carries authenticated
  price + funding — but it's *the wrapper that constructs that struct*
  from the oracle accounts, not the keeper.

### Account ordering — **verified by M1**
- `PermissionlessCrank`: `[payer (signer) → market (writable) →
  portfolio (writable) → oracle accounts (readonly) → optional
  cranker-reward portfolio (writable, last)]`
  — confirmed from `handle_permissionless_crank` at
  `src/v16_program.rs:10615` and exercised in the M1 client.
- `SyncMaintenanceFee`: `[market (writable) → portfolio (writable) →
  optional cranker portfolio (writable)]`. **No owner signer required**
  on the portfolio (it's permissionless) — only the tx fee payer signs.
- Reward portfolio omission ⇒ fee share stays in insurance (confirmed
  by README operational runbook; not directly tested in M1).

### Fee economics
- **Liquidation fee share:** `liquidation_cranker_fee_share_bps` (u16, ≤10_000),
  set at `InitMarket`. Reward = share of retained liquidation penalty, paid by
  appending cranker portfolio as final account.
- **Maintenance fee share:** `maintenance_cranker_fee_share_bps` (u16, ≤10_000),
  same append-portfolio collection mechanism.
- **No flat per-call reward** — fees are event-triggered only.

### Engine API the wrapper depends on
- Single public crank entry point: `permissionless_auto_crank_not_atomic`
  (`percolator/src/v16.rs:11298`).
- Plan variants (pins `495a5590`, `4db11a8c`, `68859b94`; unchanged): `AutoCrankPlanV16 = NoAction |
  RefreshAccount | SettleBChunk | Liquidate | ReleaseSourceLiens |
  AdvanceClose | DeclareRecovery | FinalizeRecovery | CloseResolved`.
  Roster A1–A8 (`source_liens_releasable` = A6, `resolved_winner` = A8).
  `V16_LAYOUT_DISCRIMINATOR = 18`. `V16_SOURCE_LIEN_RELEASE_CHUNK_DOMAINS
  = 1`. Cranker must not treat firing order as a same-refresh K/F PnL
  lever (spec v16.9.1, order-canonical).
- **Out-of-order safety (CORRECTED 2026-06-26):** absent observation is
  **no longer** a universal `NonProgress` no-op. After upstream `3d250d2`
  / `f5d291e` / `8338726` (PRs #82, #83), only
  `RefreshAccount { asset_index: None }` requires a caller-supplied
  observation; all other plans (`Liquidate`, `SettleBChunk`,
  `RefreshAccount { Some(_) }`, `DeclareRecovery`, `CloseResolved`,
  `NoAction`) **dispatch from committed `asset_state`** — when no
  observation is attached the engine synthesizes an
  `AutoCrankObservationV16` via the new `obs_or_current_asset` closure
  (`repo/src/v16.rs:11328`) using the committed `effective_price` with
  `funding_rate_e9: 0`, then **mutates state**. Source of truth is the
  free fn `pub fn auto_crank_plan_requires_caller_observation(plan:
  &AutoCrankPlanV16) -> bool` (`repo/src/v16.rs:187`, re-exported in
  `repo/src/lib.rs:54`); pinned by the proofs
  `proof_v16_auto_crank_refresh_is_unique_observation_requiring_plan`
  and
  `v16_auto_crank_progress_realizable_without_observation_for_every_class`.
  The cranker MUST call this predicate; do not hand-roll the match.

### Program identity
- **Program id is a vanity placeholder** (`Perco1ator111…`); confirmed null on
  mainnet-beta and devnet 2026-06-23. Any change here = launch threshold event.

---

## Decisions to revisit each run

These get re-evaluated as upstream/wrapper signals change.

- **Is now still "prototype, don't deploy"?** Yes (last re-confirmed
  2026-07-07 against 2026-07-01 and 2026-07-06 digests). `getAccountInfo`
  = null on both mainnet-beta and devnet on each check; no release, no
  tag, engine crank contract actively mutating (DRAFT PR #92 in flight).
  Flip when devnet deployment appears OR when PR #92 merges and settles
  (the latter is an assumption-invalidator, not a launch signal).
- **Is the wrapper repo `percolator-prog` stable enough to write hardened
  client code against?** No (educational disclaimer, no release). Code
  against the public instruction surface only; treat internal types as
  unstable.
- **Open questions for which I'd open an issue (eventually, against the
  wrapper, not the library):**
    - **Q1 (mechanics).** No `msg!()` log is emitted naming the selected
      `AutoCrankPlanV16` variant or `AutoCrankOutcomeV16` — keepers must
      infer from state diffs. Is this a deliberate design choice
      (cheap-CU, anti-spam) or a gap? If forks add log emission, it
      affects the strategic-questions answer to "how do you observe
      what the engine did" (currently: read-state-after).
    - **Q2 (tooling).** `cargo build-sbf --tools-version v1.52` silently
      downgrades to v1.42 inside Agave 2.0.21 (no error, no warning).
      The README marks v1.52 as the deployable build. M3-relevant: a
      production cranker built against the v1.42 artefact may not match
      mainnet bytecode. Need to install Agave 2.x more recent (or kani
      toolchain channel) to actually exercise v1.52 — and verify the
      .so hash differs.
    - **Q3 (instruction) — CLOSABLE 2026-07-07.**
      `Instruction::UpdateLiquidationFeePolicy` uses field name
      `cranker_share_bps`; `WrapperConfigV16` uses
      `liquidation_cranker_fee_share_bps`. Watcher confirmed on the
      2026-07-01 digest (`v16_program.rs:520,2516,3093` at wrapper
      `62c8abf`) that both field names still coexist across the
      re-pin candidate diff and there is no second cranker-share
      gate. Field naming diverges but the on-chain value is the same.
      Close on the one-line M2 audit once M2.b is running against a
      real market.
    - **Q4 (oracle binding) — ANSWERED 2026-07-07.** The exact walk is
      now implemented in [`cranker/src/observation.rs`](cranker/src/observation.rs)
      and verified end-to-end. Findings, all pinned to source:
      1. **The profile struct** is `state::AssetOracleProfileV16`
         (`repo-prog/src/v16_program.rs:556`, `bytemuck::Pod`, 400
         bytes = `ASSET_ORACLE_PROFILE_LEN`). One per asset. Read by
         the wrapper via `read_oracle_profile_from_view(:4509)`, which
         slices the first 400 bytes of the asset's 512-byte wrapper
         blob out of the market slab and `bytemuck::pod_read_unaligned`s
         it.
      2. **Byte offset for asset `i`**:
         `MARKET_GROUP_OFF + MARKET_GROUP_LEN + i * MARKET_ASSET_SLOT_LEN`.
         All three constants are `pub` in
         `percolator_prog::constants`. The engine crate's
         `MarketGroupV16HeaderAccount::dynamic_asset_slot_offset::<[u8;
         ASSET_ORACLE_WRAPPER_LEN]>(i)` computes the same term minus
         `MARKET_GROUP_OFF`; the cranker's `read_asset_oracle_profile`
         `debug_assert_eq!`s the two so layout drift trips at test time.
      3. **`oracle_mode` (byte 0 of the profile) is the dispatch
         discriminant.** Values:
         `ORACLE_MODE_MANUAL = 0`,
         `ORACLE_MODE_HYBRID_AFTER_HOURS = 1`,
         `ORACLE_MODE_EWMA_MARK = 2`,
         `ORACLE_MODE_AUTH_MARK = 3`. All `pub` in the same constants
         module. For MANUAL/EWMA_MARK/AUTH_MARK the mark comes from
         committed state; the cranker MUST supply `oracle_leg_count = 0`
         and pass no tail oracle accounts.
      4. **`oracle_leg_count` (byte 1) is the tail-account budget** for
         this asset's hint. The wrapper hard-rejects
         `InvalidInstruction (0x9)` when
         `hint.oracle_accounts != oracle_leg_count`
         (`:10505-10508`). So the cranker MUST read this value and use
         it verbatim as `CrankObservationHint.oracle_accounts`.
      5. **`oracle_leg_feeds: [[u8; 32]; ORACLE_LEG_CAP]` is the
         per-leg feed identifier — with source-dependent semantics.**
         The wrapper's `read_oracle_price_e6(:3863)` dispatches on the
         account's `owner`, and each of the three reader family
         functions has DIFFERENT semantics for the 32-byte value:
         * **Pyth** (`read_pyth_price_e6:3655`): the `[u8; 32]` is a
           `feed_id`, matched against `PriceFeedMessage.feed_id` after
           deserialization (`:3679`). **The account pubkey is NOT
           stored in the profile.** The cranker must resolve
           `feed_id → live PriceUpdateV2 account pubkey` off-chain.
         * **Switchboard On-Demand** (`read_switchboard_price_e6:3765`):
           the `[u8; 32]` **IS the account pubkey** —
           `price_ai.key.to_bytes() == expected_feed_key` at `:3777`.
           No off-chain lookup needed.
         * **Chainlink Store** (`read_chainlink_price_e6:3821`): same
           as Switchboard — `[u8; 32]` **IS the account pubkey**
           (`:3830`).
      6. **Consequence for the reference cranker.** A cranker CANNOT
         fully derive HYBRID-Pyth account pubkeys from market state
         alone. It needs an out-of-band
         `PythFeedResolver: HashMap<[u8; 32], Pubkey>` (or an
         equivalent) mapping feed_ids to the current PriceUpdateV2
         account. For Switchboard/Chainlink legs the pubkey is
         byte-embedded; no resolver hop. This is the concrete
         operational-data edge the service-operator side of the hybrid
         positioning trades on — Q4's answer is also a strategic-
         positioning signal (folded into
         [cranker-strategic-questions.md](cranker-strategic-questions.md)
         Q4 as "differentiation surface").
      7. **Which source is which leg** is a deployer configuration,
         not readable from state alone. The wrapper detects it at
         submit-time from the account's `owner`. So the cranker's
         off-chain config either (a) tags each `(asset, leg)` pair
         with its source, or (b) probes with fallback (try SB-embedded
         pubkey, on failure try Pyth resolver, ...). Our M2
         implementation uses fallback: if the Pyth resolver has a hit
         for the 32-byte value, use it; otherwise treat the bytes as
         a SB/Chainlink account pubkey. Correct for SB/Chainlink,
         correct for Pyth-with-resolver, silently wrong for Pyth-
         without-resolver (the tx will hard-reject at Pyth's discriminator
         check — clean failure mode).
      8. **Pyth resolver interface (added 2026-07-07 after Q4).**
         `observation::PythFeedResolver` in the cranker supports two
         mechanisms: explicit `overrides: HashMap<[u8;32], Pubkey>`
         (operator-populated, wins on hit), and canonical PDA
         fallback via `PDA(seeds=[shard_u16_le, feed_id],
         program_id=PYTH_RECEIVER)` — matching Pyth's own
         `getPriceFeedAccountAddress` (js) helper. Shard 0 is
         mainnet's Pyth-official poster stream. Selection is via
         `PythFeedResolver::empty()` / `canonical_pda_only()` /
         `with_overrides_and_canonical_pda(map)` in code, or
         `CRANKER_PYTH_MODE=canonical` env at bin-launch. Six unit
         tests in `observation::tests` pin the contract (empty
         resolver never hits, override wins over PDA, PDA is
         deterministic and matches `pyth_price_update_v2_pda(shard,
         feed)`, different shards/feeds derive different PDAs). The
         wrapper does NOT check derivation — only
         `owner == PYTH_RECEIVER_PROGRAM_ID` and the deserialized
         `PriceFeedMessage.feed_id`. So the PDA scheme is a discovery
         convention, not an admission gate; a non-canonical account
         with the right owner+feed_id would also authenticate. Verify
         the shard convention against the Pyth version your target
         market's deployer tracks.
    - **Q5 (NonProgress observability) — REOPENED 2026-06-26 · REINFORCED 2026-07-07.**
      Three separate things changed and the question needs to be
      asked again from scratch:
      1. **The NonProgress surface SHRANK.** After upstream 3d250d2 /
         f5d291e / 8338726, only `RefreshAccount{None}` returns
         NonProgress for the no-observation case. `Liquidate`,
         `SettleBChunk`, and `RefreshAccount{Some}` now *progress*
         from committed state instead of no-op'ing. So the
         "did NonProgress fire?" detection question matters in fewer
         situations.
      2. **But the retry-budget question got HARDER**, not easier. The
         old heuristic ("if I can't tell, just retry — it's a no-op")
         is now actively unsafe for the three new progress paths (M2.f
         spells out why). The keeper needs to *correctly classify
         which plan will run* before deciding to retry, which is
         exactly what `auto_crank_plan_requires_caller_observation`
         was added upstream to support.
      3. **Wrapper-side hard-rejects (2026-07-07, digest 2026-07-01)
         added a third response shape.** `6869e58` and `8067a13` on
         wrapper `main` make `PermissionlessCrank` return a wrapper
         *error* — not `NonProgress`, not a state-mutating progress
         step — when the pending selected asset lacks a matching
         observation OR when the per-slot price movement is above
         the market's cap. Response classification is now three-way
         (Err / Ok-no-mutation / Ok-with-mutation), not two-way, and
         the wrapper's error variants are the new signal to key off.
         M2.f's retry loop was already updated for this.
      Outstanding sub-questions:
      (i) **PARTIALLY ANSWERED 2026-07-07 (M2 empirical); citation
          refreshed 2026-07-13 after PR #92 merge.** Wrapper does NOT
          surface NonProgress as a distinct tx-level status in the
          normal path — the swallow branch (simplified when `close_q`
          was removed by `970ddc5` on 2026-07-10) is now
          `Err(V16Error::NonProgress) if !observations.is_empty() => None`.
          Same semantics for us (we always send observations); the
          `close_q == 0` clause the old citation carried is gone.
          Collapses NonProgress into `Ok(no-mutation)`.
          Downstream, the cranker sees a successful tx with no state
          change. `EngineNonProgress (0x16)` only surfaces when
          `observations.is_empty() || close_q != 0` — both of which
          a correctly-built cranker avoids by construction. So Q5(i)
          answer is: **state-diff is mandatory**, tx status alone
          cannot distinguish clean no-op from mutation. That is what
          M2's `submit::CrankOutcome` classifier does.
      (ii) **ANSWERED 2026-07-07 (Q5(ii) investigation).** There is
           **no off-chain idempotency key** anywhere in the crank
           path. Grep results:
           - Wrapper `req_id` / `nonce` / `dedup` / `replay` /
             `idempot` / `last_cranked` / `crank_seq` /
             `crank_counter` / `crank_slot` — zero hits on the
             crank path. All `req_id` uses are in the matcher
             CPI anti-spoof binding (TradeCpi / BatchTradeCpi:
             `WrapperConfigV16.matcher_req_seq: u64` bumped per
             trade, echoed by the untrusted matcher, validated at
             `MatcherReturn.req_id == expected`). Wrapper↔matcher
             binding, not tx-level anti-replay.
           - Engine `nonce` / `dedup` / `replay` / `idempot` /
             `crank_seq` / `last_crank` / `request_id` — zero hits
             across the entire v16 engine.
           - `permissionless_auto_crank_not_atomic`
             (`repo/src/v16.rs:11400`) does exactly what its
             docstring says: build actionable summary from committed
             state, select plan, dispatch. No anti-replay pre-check.
           **The engine takes zero anti-replay signals.** Every
           submission is a fresh dispatch against committed state.
           **And none is needed.** Safety comes from three layers:
             1. **Solana runtime.** Rejects literal
                signature-identical replays via signature-status
                cache (`AlreadyProcessed`); blockhash expiration
                (~150 slots) bounds the replay window.
             2. **Engine state-based dispatch.**
                `select_auto_crank_plan` re-evaluates against fresh
                committed state each call. A re-submitted-with-new-
                blockhash retry against post-first-tx state
                NonProgresses naturally (or dispatches the next
                legitimate step — never a double-execute of the
                same step).
             3. **M2.f's pre-retry state re-read.** Closes the
                timing gap where a first-attempt outcome is uncertain
                — cranker checks post-state before retrying, aborts
                if the goal is already achieved.
           **Adding an idempotency key wouldn't add safety** — only
           marginally better observability (explicit reject vs
           swallowed NonProgress) and a small CU saving on the
           duplicate-detection short-circuit. Not worth the extra
           per-market or per-portfolio state. Under contention,
           what a cranker actually wants isn't a dedup key but
           priority-fee + tx-scheduling policy (same finding as
           `cranker-strategic-questions.md` Q2: race is
           landing-order, not hint-quality).
      **Implication for M2.f: no code change needed.** The retry
      rule is complete; the empirical `Ok(mut) → Ok(no-mut) →
      Ok(no-mut) → …` chain observed in the M2 run against the
      synthetic market is exactly the intended steady state.
      **Q5 CLOSED 2026-07-07.**
    - **Q6 (cranker withdrawal flow) — ANSWERED 2026-07-07.**
      Fee shares accrue as internal portfolio capital; the withdrawal
      surface is the user-facing `Instruction::Withdraw { amount }`
      (tag 4). Investigation and verification:

      **Source-read (definitive).** No role field anywhere:
      `PortfolioAccountV16Account` (`repo/src/v16.rs:15754`) has
      `provenance_header` + `owner` + capital/pnl/reserved counters —
      **no `is_cranker` / `portfolio_type` / `role` / `is_operator`
      marker.** Cranker and user portfolios are structurally
      indistinguishable. `handle_withdraw` (`repo-prog/src/v16_program.rs:5662`)
      does the identical set of checks for every portfolio:
      signer==owner, market Live, dest ATA matches owner, vault has
      atoms, portfolio provenance/owner match, engine
      `withdraw_not_atomic` (`repo/src/v16.rs:14544`) validates market
      binding, mode Live, `active_bitmap_is_empty` (flat),
      close-progress inert, non-negative PnL after settlement, and
      capital ≥ amount. Nothing keys off funding provenance —
      capital credited by a Deposit and capital credited by a
      `SyncMaintenanceFee`/`PermissionlessCrank` fee-share are the
      same field.

      **Empirical (against wrapper `62c8abf` + engine `c8aab33`).**
      Ran with `CRANKER_WITHDRAWAL_TEST=1`. `SyncMaintenanceFee`
      routed **1200 atoms** to the cranker portfolio's capital (100
      atoms/slot × ~24 slots × 50% share). `Withdraw(100)`
      succeeded first-shot. Payer ATA moved **0 → 100 atoms
      exactly**. Zero role-based checks tripped. Sigs captured in
      the run log; code path lives at
      `cranker/src/main.rs::run_q6_withdrawal_test`.

      **Non-obvious operational finding surfaced by the investigation.**
      There is **no `UpdatePortfolioOwner` / `RotatePortfolio`
      instruction** — `grep` for that + variants returned zero hits.
      Only `InitPortfolio` and `ClosePortfolio` exist. Consequence:
      **a cranker's fee-receiving key cannot be rotated in place.**
      To change the receiving key the operator must (1) Withdraw
      all capital, (2) `ClosePortfolio`, (3) `InitPortfolio` with
      the new owner, (4) update the cranker binary's config to point
      at the new portfolio pubkey. Non-zero operational cost for
      key rotation. Worth noting in strategic-questions.md if the
      operational-security surface matters for the service-operator
      side of positioning.

      **Constraints that DO apply to cranker withdrawals (in
      practice, always satisfied for a well-run cranker):**
      * Market must be Live (not Recovery/Resolved).
      * Portfolio must be flat (`active_bitmap_is_empty`) — cranker
        portfolios ARE flat by construction; no gate.
      * No active close-progress ledger — same, doesn't happen to a
        properly-managed cranker portfolio.
      * `withdraw amount ≤ capital` — obvious.
      * Withdraw is blocked while `permissionless_resolve_matured`
        triggers — inherits the market-wide gate.

      **Q6 CLOSED 2026-07-07.**

  See also: `cranker-strategic-questions.md` for full
  question-and-running-answer set.

---

## What the watcher should report each run in §9 "Personal cranker prep"

1. **Milestone unblockers:** wrapper-side changes that *enable* a milestone
   you haven't yet completed (e.g. `examples/cranker.rs` lands → M2 just got
   easier).
2. **Assumption invalidators:** wrapper-side changes that break a baked-in
   assumption above (instruction variant renamed, observation cap changed,
   fee-share field renamed, account ordering changed).
3. **Launch threshold proximity:** any of (declare_id change /
   `getAccountInfo` not null on devnet/mainnet / release tag /
   audit-related commit / announcement). One flip = escalate from M2 to M3.
4. **Open-question status:** if you've recorded any, has upstream/wrapper
   activity answered them?
5. **One sentence on whether the "decisions to revisit" answers should
   change this week.**

Keep this section ≤20 lines per digest. It's a status line, not a re-pitch.

---

## Watcher notes

_Last updated by watcher: 2026-09-11 post-M2.i (overwrites prior block; content above is user-owned)._

- **M2.i = DONE** on wrapper `d5e2ec6f` (#135) + engine pin `495a5590` (deliberate wrapper pin; master tip remains `8eb7142a`). `close_q` gone; layout 18; A6 one-domain drain; portfolio_id/sequence bindings in.
- **Dry-run green.** M1 bring-up + 3-cycle cranker succeeded. CU re-baseline: no-op PermissionlessCrank **17,975** program CU (tx total 18,299) — do not use ~42,817.
- **M3 = NO-GO.** `declare_id!("Perco1ator111…")` still placeholder; mainnet+devnet `getAccountInfo` null (as of 2026-09-11 slots ~446093639 / ~496543354). Fee-share defaults still 0 (local bootstrap 5000 bps is ABI-path only). Funding apply/settle still absent.
- **Watch next:** engine `#197`–`#200` (post-thaw LoF/DoS); wrapper pin bump if Cargo moves off `495a5590`; first non-null RPC / real program id / non-zero fee shares / funding settle symbols; any `PermissionlessCrank` wire or portfolio_id shape change.
- **Signal of the week:** reference cranker is on the thawed surface for dry-run; paid live service is still gated on deploy + economics, not on M2 completeness.
