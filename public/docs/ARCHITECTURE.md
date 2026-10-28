# Architecture

```
crates/
  cranker/            library + binaries
    src/
      discovery.rs    getProgramAccounts (header memcmp) → market-scoped candidates
      binding.rs      portfolio_id / sequence / position_epoch bindings, layout check
      observation.rs  per-asset oracle-profile walk (Manual/Hybrid/EWMA/AuthMark), Pyth resolver
      plan_gate.rs    delegates to the engine's auto_crank_plan_requires_caller_observation
      resolved.rs     Resolved-market backing hint (lapsed Fresh bucket, earliest deadline)
      work.rs         off-chain priority estimate; sub-atom funding remainder = settled
      scheduler.rs    ordering, rotation, quiet reserve, per-cycle budget, outcome tally
      quarantine.rs   same-error streaks → capped exponential backoff, JSON events
      submit.rs       tx build (Live / Resolved account routes), send, classify, CU from logs
      cycle.rs        one discover → hint → schedule → submit cycle (shared by everything)
      setup.rs        one-asset market bootstrap for the binary and tests
      main.rs         binary `cranker`
    tests/            validator-gated integration tests (conservation, rollback, lifecycle)
  bringup/            local smoke test: init market → deposit → crank → fee routing
```

## The crank cycle

1. **Discover.** All `KIND_PORTFOLIO` accounts of the program. Keep the ones
   that are in the target market, initialized, on layout 18, and not
   excluded.
2. **Read the market once.** Use `state::read_market` to get the mode,
   backing buckets, assets and config.
3. **Plan per candidate.**
   - **Live / Recovery:** one hint per active leg asset, each with the
     oracle accounts its profile needs.
   - **Resolved:** at most one hint `{asset, oracle_accounts: 0}` for the
     earliest-due lapsed Fresh bucket. The wrapper reads Resolved accounts
     as `[owner, market, portfolio, owner ATA, vault, vault authority,
     token program]`. The owner has to sign only inside
     `force_close_delay_slots`; there the keeper skips third-party accounts
     as idle.
4. **Order.** Sort by priority (liquidatable, pending, quiet), then rotate
   each class by the cycle number. A quarter of the tx budget is reserved
   for quiet accounts.
5. **Submit and classify.** One scheduler attempt can send several
   transactions, because an A6 source-lien release moves one domain per
   call. Draining continues only while the target account changes. Every
   transaction counts against the budget. If an RPC error comes after a
   landed step, that progress still counts.
6. **Quarantine.** A failure feeds the same-error streak. Progress or idle
   clears it. After one strike, the next same-error failure re-quarantines
   with a doubled window.

## Why the keeper never hand-rolls engine rules

The plan truth table, the layout and the instruction encoding all come from
upstream (`percolator::auto_crank_plan_requires_caller_observation` and
`percolator_prog::{ix, state}`). The keeper's own heuristics, such as
priority and plan class, only order work and label logs. They never decide
that an account is done.
