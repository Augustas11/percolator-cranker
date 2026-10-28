# Launch runbook: SOL-PERP on the official program

**Goal:** under 1 hour from the official program ID being published to the
market being Live.

**Roles:**
- **Owner** signs with the multisig and hardware keys, and decides go/no-go.
- **Ops** runs the scripts and bots, and watches the monitor. Ops may be the
  owner, with an agent assisting.

**Clock:**
- **T-1h** is the moment the program ID is seen (watcher alert). That starts
  the 1-hour window.
- **T-0** is the moment the market opens to external traders.
- T-7d and T-1d are *standing readiness* checks. Upstream gives no date, so
  re-run them every week and every day while we wait.

## T-7d (weekly while waiting)

| # | Who | Check |
|---|---|---|
| 1 | Owner | Everything in `OWNER_ACTIONS.md` up to "fund the multisig" is done |
| 2 | Ops | The full launch passes on a devnet rehearsal against our own deployment of the pinned wrapper: `scripts/launch_market.sh` end to end, timed (R2 drill) |
| 3 | Ops | Pins match upstream `main`; if not, re-pin and re-run the rehearsal |
| 4 | Ops | Monitor alerts fire in staging: stale oracle, backing near expiry, stuck account, SOL low |
| 5 | Ops | Market params file reviewed: margins, fees, stale window, cranker shares (see below) |
| 6 | Owner | Legal answers in hand for the access policy (`LEGAL_QUESTIONS.md`) |

## T-1d (daily while waiting)

| # | Who | Check |
|---|---|---|
| 1 | Ops | Bots build at the pin; hosts up; RPC quota fine |
| 2 | Ops | Hot wallets hold ≥ 1 day of SOL; multisig holds the launch USDC and SOL |
| 3 | Owner | Hardware wallets charged and to hand; Squads signers reachable |
| 4 | Ops | Pyth SOL/USD feed healthy; pusher dry-run prints sane marks |
| 5 | Owner | Announcement drafts ready (not posted) |

## T-1h: program ID published

| Min | Who | Step |
|---|---|---|
| 0 | Ops | Confirm the ID is upstream's official one (`declare_id!` on `main`, release tag or upstream announcement) |
| 0–10 | Ops | Pin to the release commit; build our crates against it; check the verified-build hash matches the deployed program (*to verify*: the tool upstream uses) |
| 10–15 | Ops | Diff the release against our pin: instruction tags, account layouts, `InitMarket` validation. **Any change to fees, gates or layout = no-go** until reviewed |
| 15 | Owner | **Go/no-go #1** (criteria below) |
| 15–35 | Ops + Owner | Run `scripts/launch_market.sh` against mainnet with the official program ID (the tech kit; built in parallel, see its `--help`). It does, in order: create the market account → `InitMarket` → AuthMark config → policies → role split → insurance seed → backing top-up → LP portfolio → start keeper, pusher, matcher (reduce-only, closed to others) |
| 35–45 | Ops | Smoke: one owner-tester round-trip ($100 open, close, withdraw); the keeper cranks; a push lands; the monitor is green |
| 45–55 | Owner | **Go/no-go #2** |
| 55–60 | Ops | Matcher to open mode at launch limits |

## T-0: Live

- Owner posts the announcement (if legal said yes).
- Invite testers first, then traders.
- Ops watches the monitor continuously for the first hour.

## T+1h

| Who | Check |
|---|---|
| Ops | No stuck account; pushes on cadence; crank success rate; SOL burn rate against the model |
| Ops | First fills: fee lands in the insurance budget as expected (`MONEY_MODEL.md`, facts 2–3) |
| Owner | Confirm the role holders on-chain match `KEYS.md` (monitor expected-roles check) |

## T+24h

| Who | Check |
|---|---|
| Ops | P&L snapshot against the model: volume, fees, LP inventory P&L, SOL spent |
| Ops | Gate uptime: share of slots where Live insurance withdrawal was allowed |
| Ops | Backing expiry is far enough out; next roll scheduled |
| Owner | Decide: keep the limits, raise them (rule 7 in `CAPITAL_PLAN.md`), or pause |

## Go/no-go criteria

All must be true. Any "no" stops the launch.

1. The program ID is upstream's official one, and the deployed binary
   matches the release.
2. The release has no change to fee routing, withdrawal gates or account
   layout against our reviewed pin, or the change has been reviewed.
3. The launch config's role public keys match `KEYS.md`; the multisig signs.
4. Pyth SOL/USD is live; the pusher's dry-run mark is within 0.1% of Pyth.
5. The monitor is green on the new market; alerts reach the owner's phone.
6. Capital is in place (`CAPITAL_PLAN.md`); hot wallets are funded.
7. The legal access policy is in force (geo-block, terms) if counsel
   required it.
8. #2 only: the smoke round-trip succeeded with exact balances.

## Market parameters (starting point; final values in the launch config)

| Setting | Launch | Why |
|---|---|---|
| Margins | IM 2,000 / MM 1,000 bps (*to verify* against the engine check) | 5x max; tighter than staging |
| `trade_fee_base_bps` | 5 | `MONEY_MODEL.md` |
| `max_trading_fee_bps` | 100 | The ceiling; it caps every fee policy |
| Liquidation fee | 50 bps | Playbook staging value |
| Liquidation cranker share | 5,000 | Half goes to the keeper as withdrawable capital (`P 14293-14320`) |
| Maintenance cranker share | 2,000 | Playbook staging value |
| Maintenance fee | 1 atom/slot | ≈ $0.22/day per portfolio |
| Backing fee | 10 bps, insurance share 2,000 | Playbook staging value |
| Permissionless resolve | ~1 h stale window and force-close delay | 0 disables SHUTDOWN (playbook: `P 12628`) |

## Pause and rollback

- **Soft pause (seconds):** switch the matcher to **reduce-only**. We are the
  only LP, so no new risk can open; users can still close against us. Use it
  for any doubt.
- **Hard pause:** stop the matcher entirely. Users cannot close either, so
  use it for minutes only (for example, a suspected oracle problem).
- **Rollback before T-0** (no external funds): resolve with the multisig
  (`ResolveMarket`), then close out and `CloseSlab` (playbook section d,
  "Resolution and exits"). Reclaim insurance and backing to their
  beneficiaries.
- **Rollback after T-0:** there is no undo. Soft pause, announce, then
  `ResolveMarket`. Users exit through the resolved close and claim path. Only
  then `CloseSlab`.

## Incident runbook

### Oracle stale

- **Page** at 25% and 50% of `stale_slots` without a successful push.
- At 25%:
  - restart the pusher;
  - fail over to the standby host and the second RPC.
- **Push now.** An asset 0 push is accepted while nobody has resolved, and it
  resets the clock (playbook: `P 5948-5957`, `P 13589`).
- If the window matures:
  - **anyone** can call `ResolveStalePermissionless` (playbook:
    `P 13178-13200`);
  - Live withdrawals and policy changes refuse with `OracleStale`;
  - soft pause, then push if we can, otherwise accept resolution and run the
    exit steps.
- **If Pyth itself is down:** do not push a guessed price. Soft pause and let
  the window decide.

### Stuck accounts (percolator #243 and similar)

- **Signal:** the keeper's quarantine list is non-empty, or the monitor
  shows a stale account.
- **Triage by error** (playbook section d):
  - `OracleStale`: push.
  - `EngineStale` on refresh or liquidate: check backing buckets first.
  - `AssetGenerationMismatch`: rebuild the intent.
  - `EngineLockActive`: a gate is closed.
- **#243 (unreferenced lapsed backing wedges Live refresh and
  liquidation):**
  - **Prevent it:** withdraw unliened principal **before** `expiry_slot`, and
    never let a bucket lapse unused (`CAPITAL_PLAN.md` rule 5). The monitor
    pages on `Fresh` with `expiry_slot ≤ now`.
  - **If it happens:** quarantine keeps the keeper healthy, and the soft
    pause stops new risk. It clears only at market resolution, so plan an
    orderly `ResolveMarket`.
- Log every stuck account with a repro, as private findings. Posting
  upstream is an owner decision.

### Recovery (market-wide)

- The crank declares it permissionlessly when bounded progress is
  impossible (playbook: `E 4316-4325`, `E 14595-14614`). **It cannot be
  undone.**
- Do:
  - soft pause;
  - keep the keeper running (each crank forfeits a released leg or resolves,
    playbook: `E 15097-15122`);
  - tell users;
  - follow the exit steps to `CloseSlab`.
- Maintenance fees stop in Recovery.
- Afterwards, relist (a new market) only after a written cause and fix.

### Bot or host down

- **Keeper:**
  - the crank is permissionless, so others may crank;
  - restart, or fail over to the standby within 15 min;
  - while it is down, liquidations lag and the cranker share goes to others.
- **Matcher:** with no LP there is no trading, which is a de facto pause.
  Restart.
- **Pusher:** see "Oracle stale".
