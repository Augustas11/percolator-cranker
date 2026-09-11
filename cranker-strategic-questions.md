# Cranker strategic questions — running answers from prototyping

This file is **observed mechanics + first-order numbers** from the M1
prototype. No editorialising on which positioning is right. Each section
maps to a question in the original mission brief.

Status legend:
- ✅ answered with code/data
- ◑ partial answer (needs M2 or production-shaped run to firm up)
- ◻ open

Updated **2026-06-23** at end of M1 spike.

---

## Q1. What's the realistic per-event fee a cranker captures?

**Answer (mechanics, ✅):** fees are split by **basis points of the
already-charged fee**, not a flat per-call reward. Two fee classes:

| Field (`InitMarket` config)                     | Type    | Bound    | Trigger                                                |
|-------------------------------------------------|---------|----------|--------------------------------------------------------|
| `liquidation_cranker_fee_share_bps`             | u16     | ≤ 10_000 | liquidation event — share of retained liquidation penalty |
| `maintenance_cranker_fee_share_bps`             | u16     | ≤ 10_000 | per-portfolio `SyncMaintenanceFee` call                |

Both are set in the wrapper config and re-settable post-init by
`marketauth` via `UpdateLiquidationFeePolicy` / `UpdateMaintenanceFeePolicy`
(tag exercised in M1 client, line invoking `UpdateMaintenanceFeePolicy`).

The cranker collects by **appending its writable Percolator portfolio
account as the LAST account** in the instruction. If omitted, the
configured share stays in insurance — fee is forfeited, not refunded.

**Per-event size estimate, ✅ for maintenance, ◑ for liquidation.**

- **Maintenance:** `fee_atoms_charged_to_user = maintenance_fee_per_slot * elapsed_slots`,
  capped by the user's remaining capital. With 50% cranker share, cranker
  gets `fee_atoms_charged * 5000 / 10000`. For a "$0.50/day anti-dust"
  fee at Solana's ~432_000 slots/day, `maintenance_fee_per_slot ≈
  500_000 / 432_000 ≈ 1.16` atoms/slot in a 6-dp collateral. A daily sync
  of one portfolio yields ~250_000 atoms (~$0.25) to the cranker at
  50% share. **Latency-insensitive** — fees accrue per slot whether the
  cranker calls or not, until cap. Cranker just races to be the one who
  collects when settling.
- **Liquidation:** harder — depends on `liquidation_penalty * liquidation_fee_bps
  * liquidation_cranker_fee_share_bps`. Not measured in M1 (would require
  synthesising a liquidatable position). M2 deliverable.

**Order-of-magnitude headline:** a per-portfolio maintenance crank at the
default-looking "anti-dust" rate is sub-dollar per call. A liquidation
crank is event-sized: the cranker's slice of a meaningful liquidation
penalty. Real revenue requires either lots of portfolios or actual
liquidations, not maintenance dust.

## Q2. How latency-sensitive is winning a crank race?

**Answer (◑):** It depends on the action, and the structure of the answer
is now visible.

- **`PermissionlessCrank` (the auto-crank):** the wrapper's
  `handle_permissionless_crank` reads the **on-chain Clock sysvar** for
  the effective slot, *not* the caller-supplied `now_slot`. This is
  proven by `v16_bpf_permissionless_crank_uses_authenticated_clock_slot_not_caller_slot`.
  The engine then dispatches whichever bounded primitive
  `select_auto_crank_plan` picks based on **current account state**. If
  two cranker tx's land same-slot:
  - Both see the same Clock-derived `now_slot`.
  - The **first to land** mutates state; the second sees post-state and
    likely returns `NonProgress` (no-op) — exactly the "absent
    observation = no-op" invariant the engine guarantees.
  - The first-landing cranker is rewarded; the second pays tx fees for
    a no-op. No hint-quality tiebreak — selection happens **inside the
    engine**, hints are only routing pointers.
- **`SyncMaintenanceFee`:** same shape — `now_slot` parameter, but fees
  accrue per-slot until the portfolio is touched. First-to-touch wins
  the accumulated fee bucket. Slow cranker collects only what's accrued
  since the last touch (might be zero just after a peer cranked).

**Implication:** the race is **landing-order, not hint-quality**. There
is no protocol-level tiebreak that rewards "better" observations. You
win by being first, period. That means:

- **Operational craftsmanship matters more than algorithmic quality.**
  RPC choice, geographic location, priority fees, leader-slot prediction.
- **A naïve cranker can be competitive** for any single event — the
  engine doesn't penalise lazy hint construction. Race is on
  submission infrastructure.

Open: same-slot multiple-tx ordering on Solana is leader-dependent
(priority-fee-based tiebreak). Worth measuring under congestion.

## Q3. What's the operational complexity floor?

**Answer (M1-scoped, ✅):** the minimum-viable cranker is **very small**.
The M1 bring-up client `client/src/main.rs` is ~470 lines including a
complete deploy+init flow, and the actual cranker call is *3 instructions*:

```
PermissionlessCrank {
    now_slot: <ignored — authenticated from Clock>,
    close_q: 0,
    observations: vec![CrankObservationHint { asset_index, oracle_accounts: 0 }],
}
```

plus account list `[payer, market, portfolio, ...oracle accounts...,
cranker_reward_portfolio?]`.

A purely-reactive cranker that polls portfolios and submits
`PermissionlessCrank` until `NonProgress` is **probably ≤ 200 lines of
Rust**. M2 will firm this number with a runnable bin.

**What a competitively reliable cranker would add:**

- **Portfolio discovery** — `getProgramAccounts` filter to enumerate all
  Percolator portfolios, then heuristic ranking (which look closest to
  liquidatable, which have stalest oracle, which are accruing the
  largest maintenance balance).
- **Oracle plumbing** — the wrapper accepts Pyth, Switchboard, Chainlink
  accounts as tail observations; a real cranker needs to know which
  oracle account a given asset is configured with and include it in the
  tx tail. **Not just hints** — actual oracle accounts. The
  `CrankObservationHint` only carries `{ asset_index, oracle_accounts }`;
  the wrapper reads price/funding from the appended oracle accounts.
  See **assumption correction in `prep.md`** — earlier baseline reads
  thought the hint carried price/funding.
- **Tx-shape optimization** — observation cap is `CRANK_OBSERVATION_DECODE_MAX = 16`,
  so a cranker batches up to 16 asset hints per tx. Each hint may
  reference 1+ oracle accounts; account count balloons fast. Per-asset
  CU is bounded but each oracle account costs tx-size.
- **Race / priority-fee logic** — to actually win events under
  competition.
- **State reconciliation** — when `NonProgress` happens, *don't* burn
  fees retrying. Wait for state change. Indexer-driven.
- **Monitoring** — last-successful-crank slot, NonProgress rate, fee
  capture per cycle.

Order-of-magnitude: **~200 LoC for reactive prototype, low thousands for
production-grade** (most of it state-reconciliation + monitoring).

## Q4. Where would a reference cranker have to make non-obvious choices?

**Answer (refined 2026-07-07, ✅ for choice 1, ◑ for the rest):**

1. **Oracle account binding — the biggest differentiation surface.**
   Now firmed up via the `observation.rs` walk (see prep.md Q4 for the
   pinned source references). Key facts:
   - Per-asset `AssetOracleProfileV16.oracle_mode` (byte 0) picks
     between MANUAL / HYBRID_AFTER_HOURS / EWMA_MARK / AUTH_MARK.
     Only HYBRID needs external oracle accounts; the other three read
     the mark from committed state.
   - For HYBRID, `oracle_leg_feeds: [[u8; 32]; 3]` is the per-leg
     identifier, but its meaning depends on the source. Switchboard
     and Chainlink store the account **pubkey** directly. **Pyth
     stores a `feed_id`, not the pubkey — the cranker must resolve
     `feed_id → live PriceUpdateV2 account` off-chain.**
   - The wrapper does not tell the cranker which source each leg
     uses — it detects at submit-time from `account.owner`. So the
     cranker's off-chain config either tags `(asset, leg) → source`
     or does a fallback probe.
   - **This is the operational-data edge.** A published reference
     cranker without a Pyth resolver can crank MANUAL/EWMA/AUTH_MARK
     markets and Switchboard/Chainlink-HYBRID markets. It CANNOT crank
     Pyth-HYBRID markets without an out-of-band feed→pubkey table.
     That table is where forks either diverge (each maintains their
     own) or converge (someone publishes a canonical one and everyone
     depends on it). Your service-operator side wins on Pyth-resolver
     completeness; forks compete on getting theirs into sync fast.

2. **Hint selection budget.** With observation cap 16 and many assets,
   *which* 16 to include per cycle? Worst-known-stale ordering is the
   safe default, but stale and *currently liquidatable* may diverge.
   Without protocol-level reward for hint quality (see Q2), the
   reference impl probably just uses simple staleness ranking and lets
   forks innovate.

3. **NonProgress backoff strategy.** Engine guarantees `NonProgress`
   on a stale tx = clean no-op. But what's the *poll interval* before
   retry? Too tight → wasted fees, too loose → losing races. This is
   pure operational tuning — perfect place for a reference impl to
   document an opinion and let operators override.

4. **Cranker portfolio capital management.** Fee shares accrue as
   internal account capital, *not* as withdrawable lamports. The
   cranker needs its own `Withdraw` flow (gated by market Live +
   flat portfolio + capital ≥ amount — verified end-to-end in the
   M2 spot-check, prep.md Q6) to actually realise PnL. The reference
   impl has to encode the withdrawal cadence — daily? on threshold?
   — and that choice is observable to forks. **Operational cost of
   key rotation (Q6 finding, 2026-07-07):** there is no
   `UpdatePortfolioOwner` instruction, so a cranker's fee-receiving
   key cannot rotate in place. Changing the receiving key means
   drain-then-close-then-init-then-repoint. For a service operator
   this is a non-trivial hot-key-compromise recovery cost; for the
   reference cranker it's a documented behaviour that forks must
   acknowledge. Worth factoring into any operational-security
   design.

5. **What to do on `NonProgress` for a specific hint.** Did the asset
   become healthy in the interim, or is the hint genuinely stale? The
   reference impl needs an answer; the simplest one ("drop the hint,
   re-poll next cycle") is also the dumbest; operators will want a
   `last_seen_stuck_at` retry-budget. Easy place to differentiate.

6. **Co-bundle with other senders.** Solana's tx pipeline rewards
   bundling; a reference impl shipping a stand-alone tx submitter
   forgoes that. A more sophisticated fork might integrate with
   Jito-bundle relays.

## Open mechanical questions surfaced during M1

These should also land in `prep.md` "Open questions":

- **The wrapper emits no `msg!()` logs identifying the selected
  `AutoCrankPlanV16` variant.** Cranker bots cannot scrape logs for
  "which action ran"; they must infer from state diffs. (Verified: M1
  PermissionlessCrank tx logs show only generic "Program invoke /
  success" — see `solana confirm -v`.)
- **No `WithdrawCrankerEarnings` or equivalent.** Earnings accrue
  internally; withdrawal goes through the same `Withdraw` (tag 4)
  instruction used by users. The cranker portfolio is just a normal
  portfolio for withdrawal purposes — important for cranker-economic
  modelling.
- **Liquidation cranker share field name.** The wrapper config field
  named in the readme is `liquidation_cranker_fee_share_bps`, but the
  `UpdateLiquidationFeePolicy` arg is named `cranker_share_bps` — they
  may be the same value with different display names. M2 should confirm
  there's no second cranker-share field that also gates rewards.

---

## TL;DR for the strategic decision

- Per-event capture is **bps-of-fee**, not flat. Maintenance fees are
  predictable dust; liquidation is event-sized.
- Wins go to **infrastructure-fastest**, not algorithmically-smartest.
- Minimum-viable cranker is **~200 LoC Rust**; production hardening
  is mostly *operational*, not codey.
- The reference-impl differentiation surface lives in (a) oracle
  discovery, (b) hint-selection policy, (c) NonProgress retry policy,
  (d) earnings-withdrawal cadence. None of those are protocol-fixed.
