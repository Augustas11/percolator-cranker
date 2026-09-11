# Personal Cranker Prep — Working State

The watcher uses this file to size each digest's "Personal cranker prep" section
against your actual prep state. Edit freely; the watcher reads but does not
overwrite. (The watcher may append a `## Watcher notes` block at the bottom each
run with what changed wrapper-side relative to this state.)

Last reviewed by you: **2026-06-23 — positioning locked: open-source reference cranker + service operator** · **M1 prototype completed same day** · **2026-06-26 — re-pinned M1 against wrapper `acd5c0c` + engine `83387267`, M2 plan revised for the new no-observation-progress surface.** · **2026-07-07 — absorbed 2026-07-01 + 2026-07-06 digests, re-pinned to wrapper `62c8abf` + engine `c8aab33`, M2 cranker built in `./cranker/` and validated end-to-end, Q4 answered (real `AssetOracleProfileV16` walk + Pyth PDA resolver added), Q5 CLOSED, Q6 CLOSED (cranker withdrawal identical to user withdrawal — 1200-atom fee accrual + 100-atom Withdraw verified end-to-end; no portfolio-owner-rotation instruction exists → operator key rotation costs a Close+Init cycle). Only Q1 (no `msg!` for plan variant) + Q2 (v1.52 tooling) remain open; both are low-priority observability/tooling issues.** · **2026-07-13 — drift inspection against upstream `master`/`main`: DRAFT PR #92 MERGED as `b97e174`/`970ddc5` on 2026-07-10, deleting `close_q` from `PermissionlessCrank`. Our client + cranker code now fails to compile against `origin/main`. Wrapper up 6 commits (`62c8abf → 186e9c6`), engine up 6 commits (`c8aab33 → 143e68c`), all inspected. Prep.md assumption block updated to reflect the merged shape; re-pin + code fix deferred pending user go/no-go.**

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

- [ ] **M2.i — Re-pin required (drift inspection 2026-07-13).**
      Wrapper at `origin/main` is `186e9c6` (6 commits ahead of our
      `62c8abf`); engine at `origin/master` is `143e68c` (6 commits
      ahead of `c8aab33`). Full window inspected against the
      assumption block.

      **Actions required before this checkbox can be ticked:**
      1. **Remove `close_q: 0` from three call sites** —
         `client/src/main.rs`, `cranker/src/submit.rs`,
         `cranker/src/setup.rs`. Mechanical delete of the field from
         `Instruction::PermissionlessCrank { ... }` literals; the
         wrapper's decoder no longer accepts a `close_q` slot.
      2. **Bump wrapper path-dep** (`./repo-prog`) to `186e9c6` and
         engine `Cargo.toml` rev in `client/` and `cranker/` to
         `143e68c` (matches wrapper's own engine pin at `origin/main`).
      3. **Re-apply the `Cargo.lock` v4→v3 patch** to
         `./repo-prog/Cargo.lock` so `cargo build-sbf --tools-version
         v1.52` reads it. Same one-line `sed` as the last re-pin,
         since main is still format-v4.
      4. **Rebuild SBF + redeploy** to test-validator; re-run
         `scripts/m1_bringup.sh` and `cargo run --release` in
         `cranker/` to confirm CU is still in the 42_817-ish range
         (the two new proof-only engine commits shouldn't shift it
         meaningfully) and the mirror-custody chain still shows the
         `Ok(mut) → Ok(no-mut) → …` shape.

      **New engine invariants inspected (no code impact, logged so
      the next drift check knows we already considered them):**
      - `7388645` "Invalidate health certificates when oracle
        targets change" (PR #107) — engine advances `oracle_epoch`
        when a raw target moves; cached health certs become stale.
        Our cranker uses committed state, doesn't cache health
        certs. No code impact.
      - `75283b7` "Block fresh risk during side recovery" (PR #108)
        — new `EngineLockActive`-shaped hard-reject path during
        side recovery. Same `Custom(N)` code, new trigger. The
        response-classifier's error-name table doesn't need
        updating.
      - `143e68c` "Prevent same-trade OI masking" (PR #109) —
        trader-facing risk fix. Not cranker-facing.
      - `876fa9f` + wrapper `e502653` "Terminal spent-domain
        cleanup / restart" (PR #94 / #191) — enable `CloseResolved`
        + `RestartAssetOracle` variants in more edge cases. Our
        plan-classification gate delegates to the upstream
        predicate → still correct.
      - `2cbca3e` + wrapper `392a376` "Make resolved bound
        refinement internal" (PR #89 / #184) — narrows public API
        surface. Nothing we depend on.

      **Unchanged (verified 2026-07-13):**
      - `CrankObservationHint { asset_index: u16, oracle_accounts: u8 }`.
      - `CRANK_OBSERVATION_DECODE_MAX = 16`.
      - `liquidation_cranker_fee_share_bps` / `maintenance_cranker_fee_share_bps`.
      - Account ordering.
      - `declare_id!("Perco1ator111…")` — placeholder. **M3 no-go.**
      - No new `msg!()` for `AutoCrankPlanV16` — **Q1 still open**.
      - No tags, no releases.
      - `auto_crank_plan_requires_caller_observation` predicate —
        unchanged; `plan_gate.rs` still correct.

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
- **Instruction variant name (UPDATED 2026-07-13):** `PermissionlessCrank
  { now_slot, observations: Vec<CrankObservationHint> }` — **`close_q`
  field DELETED** by upstream `970ddc5` "Remove keeper-controlled
  liquidation sizing" (PR #206, merged 2026-07-10). Engine-side removal
  in `b97e174` "Fix liquidation min-fee chunking" (PR #92). Tag=5
  wire format shrank by 16 bytes:
  `[tag=5(1) | now_slot(8) | n(1) | (asset_index(2) oracle_accounts(1))*n]`.
  The engine now computes the minimum close size and derives the fee
  from config; cranker submits only observations + fee-rate hint.
  **Impact on our code:** every `Instruction::PermissionlessCrank {
  close_q: 0, ... }` literal in the tree (client/src/main.rs,
  cranker/src/submit.rs, cranker/src/setup.rs at time of drift check)
  fails to compile against `origin/main`. Fix at re-pin time is
  mechanical (remove the `close_q` field from the literal).
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
- Plan variants: `AutoCrankPlanV16 = NoAction | RefreshAccount | SettleBChunk |
  Liquidate | DeclareRecovery | CloseResolved` (`src/v16.rs:4443`).
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

_Last updated by watcher: 2026-08-18 (overwrites prior block; content above is user-owned)._

- **M3 = NO-GO, still pre-deployment.** `getAccountInfo(Perco1ator1111…)` →
  `null` on BOTH mainnet-beta (slot 439966332) and devnet (slot 484953943).
  `declare_id!` unchanged placeholder. No release/tag/chain-target/economics.
- **Engine `master` frozen @ `143e68c` for a 5th window** — zero landed
  changes since 2026-08-06. No cranker-enabling knob/phase/entry-point landed.
- **No cranker instruction-shape drift.** `PermissionlessCrank { now_slot,
  observations }`, `close_q`-absent, `CRANK_OBSERVATION_DECODE_MAX=16`,
  account ordering — all unchanged. Existing re-pin plan still valid.
- **⚠ New indexer/distributor obligation (wrapper `main` +4 → `36fa587e`):**
  commit `567c76c9` adds `portfolio_id` incarnation field. **Portfolio
  account layout grew +8 bytes** (`PORTFOLIO_ACCOUNT_LEN`), and any counter
  snapshot must now bind `(market, portfolio pubkey, portfolio_id)` — a
  close+recreate at the same pubkey gets a new ID. Fold both into the indexer
  design at next re-pin. Also `c8cf4de8` EWMA-clamp-to-collected-fees (#198,
  fee accounting, operator-relevant, not a cranker-ix change).
- **Signal of the week:** the deployment surface (`percolator-prog`) is where
  all drift now originates while the library sits still — watch `main`, not
  `master`, for the next assumption break.
