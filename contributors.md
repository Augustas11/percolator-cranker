# External Contributor Roster

Append-only roster of non-aeyakovenko contributors. Early contributors are the
likely future cranker / wrapper builders. Handle · first-seen · activity · area.

Seeded 2026-06-19 (cold start). `master` is solo-committer (aeyakovenko);
externals appear via PR only.

---

## dcccrypto ("Dark Cobra")
- **First seen:** 2026-06-19 (cold-start baseline; PR open since 2026-06-17)
- **Activity:** PR [#79](https://github.com/aeyakovenko/percolator/pull/79)
  ("v16: fix a vacuous fee-last-slot assertion, found by mutation-testing the
  suite", branch `kani/fix-vacuous-fee-last-slot-assert`) — **CLOSED UNMERGED
  2026-06-22.** Notable dynamic: aeyakovenko landed the equivalent fix himself in
  `57dbdd7` ("De-vacuum source-lien fee-clock assertion (mutation-sensitivity)").
  The mutation-testing *finding* was adopted; the PR was not merged.
- **Area:** Formal verification / mutation-testing the proof suite. Kani-focused
  (branch namespace `kani/`).
- **Cranker relevance:** proof-bench member; leading indicator per watch
  priority 5. Finding-adopted-PR-closed pattern worth watching — externals
  surface findings, maintainer reimplements solo on `master`.

## Ashutosh0x ("Ashutosh Kumar Singh")
- **First seen:** 2026-06-19 (cold-start baseline; PR open since 2026-06-16)
- **Activity:** OPEN PR [#77](https://github.com/aeyakovenko/percolator/pull/77)
  — "Machine-checked division axiom proofs via Z3 integer theory" (branch
  `division-axiom-z3-proofs`). Not yet merged.
- **Area:** Formal verification / Z3 integer theory; division-axiom proofs
  underpinning core risk math.
- **Cranker relevance:** proof-bench member; leading indicator per watch
  priority 5.

## julian-osec ("Julian K")
- **First seen:** 2026-07-22 (PR opened 2026-07-18).
- **Activity:** OPEN PR [#137](https://github.com/aeyakovenko/percolator/pull/137)
  — "Extend fuzing harness with more ixs" (`tests/v16_fuzzing.rs`, +1590/−61,
  **not draft**). Extends the proptest fuzz harness with more instructions, value
  ranges, slot advancing, and `V16_FUZZ_DIAGNOSTICS=1` logging; end goal is a
  drive-to-close run (close accounts + market, verifying invariants along the way).
- **Area:** Fuzz / property testing — **first external to touch the fuzz surface**
  (prior externals were proof/Kani/Z3 only). Exercises the auto-crank lifecycle
  continuation path (advance → crank → close).
- **Cranker relevance:** HIGH as a leading indicator — a security-minded external
  stress-testing the crank *lifecycle* (not just proofs). Handle suggests possible
  OtterSec (`osec`) affiliation — **unconfirmed** (no company on profile). Likely
  future cranker/wrapper fork-builder per the community-bench thesis.

---

## Roster updates

- **2026-07-01** — No new external contributors this window
  (`83387267..68986f7`). All 9 master commits + all 3 merged PRs (#84/#85/#86)
  authored by `aeyakovenko` via `[codex]`. #77 (Ashutosh0x) still open but
  **STALE since 2026-06-16**. #79 (dcccrypto) remains closed-unmerged. Master
  stays solo-committer; externals PR-only and quiet.
- **2026-07-14** — No new external contributors (`68986f7..143e68c`). All 6
  master commits + all 6 merged PRs (#89/#92/#94/#107/#108/#109) authored by
  `aeyakovenko` via `[codex]`. **Ashutosh0x #77 REVIVED** — updated 2026-07-13
  after 24 days dormant; still open, still unmerged (the only open external PR).
  dcccrypto still closed-unmerged, no new activity. Master remains
  solo-committer.

- **2026-07-22** — **NEW external contributor `julian-osec`** (PR #137, fuzz
  harness; see section above). Engine `master` unmoved (`143e68c`); no external
  commit reached `master`. #77 (Ashutosh0x) still open/unmerged; dcccrypto
  unchanged (closed). Master stays solo-committer.
