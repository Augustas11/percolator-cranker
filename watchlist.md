# Open-PR Watchlist

OPEN PRs on `aeyakovenko/percolator` that, if merged, would matter per the watch
priority list. Updated in place; entries removed when merged/closed.

Last refreshed: 2026-08-18. Engine `master` unmoved @ `143e68c` for a **5th
window** — nothing merged, no release, no tag. New this window: DRAFT **#174**
(`…crank-20260808`, DoS residual-progress fix; "crank" in branch name only).
All movement is wrapper-side (`percolator-prog` `main` +4 commits, portfolio
incarnation IDs). No new external contributor.

| PR | Title | Author | Last update | Why it matters (≤15 words) |
|----|-------|--------|-------------|----------------------------|
| [#174](https://github.com/aeyakovenko/percolator/pull/174) | [REAL DoS] Preserve flat trade residual progress | aeyakovenko (codex) | 2026-08-08 | DoS liveness on flat/neg-trade cranks; no new knob, `src/v16.rs +1658` |
| [#173](https://github.com/aeyakovenko/percolator/pull/173) | 🔔 [PARTIAL DoS] Make lapsed live source backing publicly progress | aeyakovenko (codex) | 2026-08-06 | **NEW crank outcome `SourceBackingExpired`**; permissionless one-domain-per-call expiry |
| [#165](https://github.com/aeyakovenko/percolator/pull/165) | [BLOCKER DoS] Resolve terminal recovery through auto-crank | aeyakovenko (codex) | 2026-07-29 | Auto-crank liveness at terminal recovery; no new crank surface |
| [#164](https://github.com/aeyakovenko/percolator/pull/164) | [BLOCKER LoF] Carry fractional oracle caps across max-dt cranks | aeyakovenko (codex) | 2026-07-29 | Oracle cap carry across max-dt crank; crank cadence correctness |
| [#161](https://github.com/aeyakovenko/percolator/pull/161) | [BLOCKER DoS] Bound resolved close work per call | aeyakovenko (codex) | 2026-07-27 | Bounds per-call CU on resolved close; operator crank budget |
| [#160](https://github.com/aeyakovenko/percolator/pull/160) | [BLOCKER DoS] Bound max-source PnL conversion | aeyakovenko (codex) | 2026-07-27 | Bounds max-source PnL conversion CU; operator crank budget |
| [#148](https://github.com/aeyakovenko/percolator/pull/148) | [REAL LoF] Preserve bankruptcy loss domains across leg exits | aeyakovenko (codex) | 2026-07-22 | 🔔 Re-declares `V16_LAYOUT_DISCRIMINATOR = 18` (consensus w/ #129) |
| [#147](https://github.com/aeyakovenko/percolator/pull/147) | [REAL LoF/DoS] Credit post-snapshot expired backing to payouts | aeyakovenko (codex) | 2026-07-22 | Terminal payout correctness; source-backing realization |
| [#146](https://github.com/aeyakovenko/percolator/pull/146) | [HARDENING] Retire terminal non-reclaimable fees safely | aeyakovenko (codex) | 2026-07-22 | Fee-sink cleanup at terminal; capacity/DoS |
| [#145](https://github.com/aeyakovenko/percolator/pull/145) | [REAL LoF] Keep terminal source payouts monotonic | aeyakovenko (codex) | 2026-07-22 | ⚠️ **spec property flip**: realizable-limited → payout-monotone (P4 if lands) |
| [#144](https://github.com/aeyakovenko/percolator/pull/144) | [REAL LoF] Retire B losses from their source domain | aeyakovenko (codex) | 2026-07-22 | Loss-domain locality; touches `v16_fuzzing.rs` |
| [#142](https://github.com/aeyakovenko/percolator/pull/142) | [REAL DoS] Reserve source capacity for admitted positions | aeyakovenko (codex) | 2026-07-19 | Source-capacity reservation; touches `v16_fuzzing.rs` |
| [#141](https://github.com/aeyakovenko/percolator/pull/141) | [REAL DoS] Reset unilateral exact-OI ADL residues | aeyakovenko (codex) | 2026-07-19 | ADL residue reset; auto-crank state liveness |
| [#140](https://github.com/aeyakovenko/percolator/pull/140) | [REAL LoF] Keep source-claim conversion domain-local | aeyakovenko (codex) | 2026-07-19 | Domain-local source-claim conversion; correctness |
| [#139](https://github.com/aeyakovenko/percolator/pull/139) | [REAL DoS] Reset exact-OI ADL trade residues | aeyakovenko (codex) | 2026-07-19 | ADL trade-residue reset; crank liveness |
| [#138](https://github.com/aeyakovenko/percolator/pull/138) | Price oracle target lag through final margin | aeyakovenko (codex) | 2026-07-19 | Oracle target-lag into final margin; price/liq correctness |
| [#137](https://github.com/aeyakovenko/percolator/pull/137) | Extend fuzzing harness with more ixs | **julian-osec** | 2026-07-18 | 🔔 **1f + 1st external fuzz PR**: drive-to-close crank-lifecycle walk |
| [#136](https://github.com/aeyakovenko/percolator/pull/136) | [HARDENING] Allow a Recovery market to wind down to Resolved (#236) | aeyakovenko (codex) | 2026-07-18 | Recovery→Resolved wind-down; close-crank liveness |
| [#134](https://github.com/aeyakovenko/percolator/pull/134) | [REAL LoF] Prevent ADL basis reissue across split trades | aeyakovenko (codex) | 2026-07-19 | ADL basis integrity across split trades; crank correctness |
| [#133](https://github.com/aeyakovenko/percolator/pull/133) | Preserve accrual slots without observations | aeyakovenko (codex) | 2026-07-17 | 🔔 Stale crank can't suppress honest Hybrid observation (landing-order race) |
| [#129](https://github.com/aeyakovenko/percolator/pull/129) | Make K/F settlement invariant to crank cadence | aeyakovenko (codex) | 2026-07-17 | 🔔 1c funding: settlement cadence-invariant; bumps layout discriminator 17→18 |
| [#128](https://github.com/aeyakovenko/percolator/pull/128) | Prevent zero-delta oracle crank pinning | aeyakovenko (codex) | 2026-07-17 | 🔔 Anti-grief: NonProgress before mutation stops slot_last pinning |
| [#127](https://github.com/aeyakovenko/percolator/pull/127) | Skip Recovery legs in auto-crank selection | aeyakovenko (codex) | 2026-07-17 | 🔔 1d: NEW `pub fn build_auto_crank_plan` for wrappers; supersedes #98 |
| [#126](https://github.com/aeyakovenko/percolator/pull/126) | [REAL DoS] Keep recovery-asset closes local | aeyakovenko (codex) | 2026-07-15 | Recovery-asset close must not force global recovery |
| [#125](https://github.com/aeyakovenko/percolator/pull/125) | Keep discharged social-loss assets reusable | aeyakovenko (codex) | 2026-07-15 | Market-slot reuse after social-loss discharge; capacity DoS |
| [#124](https://github.com/aeyakovenko/percolator/pull/124) | Keep funded flat assets reusable | aeyakovenko (codex) | 2026-07-17 | 1c funding: stale F indices block asset restart forever |
| [#123](https://github.com/aeyakovenko/percolator/pull/123) | Keep price-moved empty assets reusable | aeyakovenko (codex) | 2026-07-15 | Market-slot reuse after price move on empty asset |
| [#120](https://github.com/aeyakovenko/percolator/pull/120) | [REAL DoS] Scope bankruptcy locks to affected assets | aeyakovenko (codex) | 2026-07-13 | Crank-loop liveness; stops cross-asset bankruptcy lock stalling unrelated work |
| [#119](https://github.com/aeyakovenko/percolator/pull/119) | [REAL DoS] Keep fragmented risk reductions live below IM | aeyakovenko (codex) | 2026-07-13 | Keeps sub-IM reductions crankable; prevents fragmented-recovery stall |
| [#118](https://github.com/aeyakovenko/percolator/pull/118) | [REAL DoS] Reset both zero-OI sides after unilateral close | aeyakovenko (codex) | 2026-07-13 | Bilateral zero-OI reset; auto-crank state-machine liveness |
| [#117](https://github.com/aeyakovenko/percolator/pull/117) | [REAL DoS] Quarantine reset carry without blocking progress | aeyakovenko (codex) | 2026-07-13 | Reset-remainder carry must not block crank progress |
| [#116](https://github.com/aeyakovenko/percolator/pull/116) | [REAL DoS] Keep social-loss dust from blocking owner exit | aeyakovenko (codex) | 2026-07-13 | Dust must not strand owner exit under auto-crank |
| [#115](https://github.com/aeyakovenko/percolator/pull/115) | [REAL DoS] Allow resolved close after ADL scaling | aeyakovenko (codex) | 2026-07-13 | Resolved close path liveness after ADL; crank progress |
| [#114](https://github.com/aeyakovenko/percolator/pull/114) | [REAL DoS] Commit recovery-required auto-crank progress | aeyakovenko (codex) | 2026-07-13 | Recovery-required legs must commit auto-crank progress |
| [#113](https://github.com/aeyakovenko/percolator/pull/113) | Bound max-source lien allocation CU | aeyakovenko (codex) | 2026-07-13 | Bounds crank CU on max-source lien allocation |
| [#112](https://github.com/aeyakovenko/percolator/pull/112) | [REAL DoS] Reconcile expired source liens during live refresh | aeyakovenko (codex) | 2026-07-13 | Expired-lien reconcile in Live auto-crank refresh |
| [#111](https://github.com/aeyakovenko/percolator/pull/111) | [REAL DoS] Unwind source liens when positive PnL reverses | aeyakovenko (codex) | 2026-07-13 | Source-lien unwind on PnL reversal; crank correctness |
| [#110](https://github.com/aeyakovenko/percolator/pull/110) | [REAL DoS] Bound max-source refresh and liquidation | aeyakovenko (codex) | 2026-07-13 | Bounds crank CU on max-source refresh + liquidation (force-pushed this window) |
| [#106](https://github.com/aeyakovenko/percolator/pull/106) | [REAL DoS] Bound resolved active-leg teardown | aeyakovenko (codex) | 2026-07-13 | Bounds crank CU on resolved active-leg teardown |
| [#104](https://github.com/aeyakovenko/percolator/pull/104) | [REAL DoS] Bound resolved source-domain close work | aeyakovenko (codex) | 2026-07-13 | Bounds crank CU on resolved source-domain close |
| [#102](https://github.com/aeyakovenko/percolator/pull/102) | [REAL DoS] Expire lapsed source backing during Live auto-crank | aeyakovenko (codex) | 2026-07-13 | Lapsed-backing expiry inside Live auto-crank |
| [#101](https://github.com/aeyakovenko/percolator/pull/101) | [REAL DoS] Unstick abandoned ADL residues during Recovery | aeyakovenko (codex) | 2026-07-13 | Unsticks abandoned ADL residues; Recovery crank liveness |
| [#100](https://github.com/aeyakovenko/percolator/pull/100) | [REAL DoS] Keep partially ADL-reduced positions exit-live | aeyakovenko (codex) | 2026-07-10 | Caps liq/rebalance by matched OI; auto-crank cleans dead legs |
| [#99](https://github.com/aeyakovenko/percolator/pull/99) | [REAL DoS] Make prior-reset auto-crank obligations progress | aeyakovenko (codex) | 2026-07-10 | Crank progresses stranded `ResetPending` legs owner-independently |
| [#98](https://github.com/aeyakovenko/percolator/pull/98) | [REAL DoS] Skip Recovery legs in auto-crank selection | aeyakovenko (codex) | 2026-07-10 | **Superseded by #127** (same title, adds `build_auto_crank_plan`) |
| [#87](https://github.com/aeyakovenko/percolator/pull/87) | [COVERAGE] Prove resolved source realization no-ops | aeyakovenko (codex) | 2026-07-13 | Close-crank liveness proof; low direct cranker relevance |
| [#77](https://github.com/aeyakovenko/percolator/pull/77) | [PROOF] Machine-check division axioms with Z3 integer theory | Ashutosh0x | 2026-07-13 | Only open **external** PR; Z3 division-axiom proofs underpin core risk math |

**Notes:**
- **🔔 New this window:** **#137 (julian-osec)** — first external *fuzz*-harness PR;
  drives a full close-out invariant walk (advance → crank → close accounts +
  market). Signal-type 1f + external contributor. Watch for merge / follow-ons.
  **#148** re-declares `V16_LAYOUT_DISCRIMINATOR = 18` → the 17→18 bump is now
  consensus across ≥2 DRAFT PRs. **#145** flips a spec property (realizable-limited
  → payout-monotone); P4 if it reaches master.
- **🔔 Prior highest-value open items (unchanged, still DRAFT):** **#127** (new
  `pub fn build_auto_crank_plan` — first additive crank surface, signals.md 1d) and
  **#129** (funding K/F cadence-invariance + discriminator 17→18, signals.md 1c).
  Watch for either leaving DRAFT — #127 is an M2 crank-selection simplifier, #129
  forces a discriminator/ABI bump.
- **The `[REAL DoS]` / market-slot-reuse wave keeps growing, ZERO merged.** The
  ~27 open PRs (#98–#134) all harden the *existing*
  `permissionless_auto_crank_not_atomic` path — CU bounds, liveness, market-slot
  reuse, anti-grief pinning — plus #127's one additive helper. Engine `master`
  has not moved since 2026-07-10; this is a large batch waiting to land. **Watch
  for the merge wave.**
- **#77 (Ashutosh0x)** — only open external PR; still unmerged. A merge would be
  the first non-aeyakovenko commit on `master`.
- **The `[REAL DoS]`/`[REAL LoF]` wave now ~40 open PRs (#87–#148), ZERO merged
  across two windows.** All harden the *existing* `permissionless_auto_crank_not_
  atomic` path — CU bounds, liveness, market-slot reuse, monotonic payouts,
  anti-grief — plus #127's one additive helper. A large landing is overdue.
- **#77 (Ashutosh0x)** — only proof external PR, still unmerged. **#137
  (julian-osec)** — new external fuzz PR, unmerged. Either merging = first
  non-aeyakovenko commit on `master`.
- **Resolved since last run (2026-07-18):** none — no PR merged or closed;
  `master` unmoved.
