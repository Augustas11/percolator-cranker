# Radar — what to watch next week (as of 2026-08-18)

- **Engine `master` frozen @ `143e68c` for a 5th straight window.** The whole
  codex bug-hunt loop (`scripts/loop.md` now in the wrapper repo) runs as
  DRAFT PRs against master; watch for the first *merge* of any of them —
  especially anything that lands a new `cfg_*_chunks` knob or crank outcome
  enum, which would be the first real cranker signal in weeks.
- **Wrapper (`percolator-prog`) is where the action is.** `main` moved +4 this
  window (portfolio incarnation IDs, EWMA fee clamp). Re-check the wrapper
  `main` each run — assumption drift now originates here, not the library.
- **Portfolio incarnation-ID ripple:** watch for follow-up wrapper commits that
  thread `portfolio_id` into crystallization/claim paths — that hardens the
  indexer/distributor snapshot contract the operator role depends on.
- **`scripts/loop.md` bug classes** explicitly name "keeper/cranker cannot
  drive an account toward health/settlement/close" as a first-class LoF/DoS
  target — the cranker liveness surface is being actively fuzzed. Watch which
  of these fixes graduate from DRAFT to merged.
- **Still-unfired launch triggers:** first release tag, non-null program-id
  RPC, any crank-economics doc. All null this run. RPC re-check every run.
