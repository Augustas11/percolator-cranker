# Roadmap: from reference cranker to product

Written 2026-10-28. Positioning is unchanged: **open-source reference
cranker + service operator** (see `prep.md`). This file orders the work. It
records facts as of 2026-10-28, and items marked *to verify* are not
established yet.

## Situation (2026-10-28)

- **Upstream is preparing to launch.** The ~20 PRs merged into the engine
  `master` on 2026-10-27 (`68859b94`) are what the wrapper re-pin PR
  (prog #456, draft) calls the "engine launch fixes."
- **Nothing is live yet.** `declare_id!` is still `Perco1ator111…`, there is no
  release tag, and the cranker fee shares
  (`liquidation_cranker_fee_share_bps`, `maintenance_cranker_fee_share_bps`)
  default to 0.
- **Crank pay is undecided.** The keeper-reward rule during Hybrid catch-up is
  an open policy question (prog #456 leaves 5 tests red on purpose).
- **Live-shaped load finds real liveness bugs.** On v16 test deployments with
  ~40 accounts, `PermissionlessCrank` hit an account that no crank could
  refresh within 30 minutes (engine `5f2a7151`, percolator #226). A cranker
  must assume more such cases exist.
- **No public cranker for the current engine and wrapper is known.** The
  reference spot is open.

## In flight

- **Re-pin to the launch fixes.** Covers the new pins,
  Resolved-mode asset hint, recovery position epoch, exact funding,
  per-account quarantine, and a `live_shaped` local load scenario.

## Immediate (next 2–4 weeks)

### 1. Ship the first public cranker for the current engine and wrapper

Release it as a tagged version with a runbook, quarantine logic and
load-test results.
- **Why:** being first, documented and current is the cheapest moat.
- **Done when:**
  - tag `v0.1.0` exists on the re-pinned surface;
  - the README, `docs/RUNBOOK.md`, `CHANGELOG.md` and the license are in
    place;
  - CI runs build, tests and the `live_shaped` scenario;
  - it is published as a **new, clean public repo** (fresh history, proper
    name and layout) built by an allowlist export. This repo stays private
    as the working, ops and strategy repo.
- **Owner decisions:** the public repo's name, creating it, the license, and
  which repo is the code home after `v0.1.0`.

### 2. A persistent staging market = operator rehearsal

No market operators exist yet, so **we are the first operator**. The staging
market is run exactly as we would run a real one: our keys hold every market
role, and every role has a bot or a written procedure.

Run your own copy of the wrapper (own program ID) on devnet, 24/7, with the
cranker and scripted traders: several assets, funding on, and at least one
asset driven through Recovery → Resolved.
- **Why:** it reproduces live-shaped failure modes under load, gives
  operating data, and serves as a live demo for market operators.
- **Done when:**
  - the local-validator version runs unattended;
  - devnet deploy scripts, trader bots and a hosting recipe
    (Docker/systemd) exist;
  - the devnet deploy itself has been done by the owner, with the owner's
    keys and SOL.

### 3. A liveness and solvency monitor

A monitor built from the state the cranker already reads.
- **Metrics:**
  - out-of-date accounts;
  - accounts no crank can refresh (from the quarantine list);
  - backing buckets near expiry;
  - unpaid resolved claimants;
  - unowned collateral (`vault − c_tot − insurance − pnl_pos_tot`;
    map it to field names, *to verify*);
  - cranks by outcome (progress, idle, failed), and CU used.
- **Why:** every market operator needs this at launch, and it is the first
  thing you can sell beyond cranking.
- **Done when:**
  - Prometheus `/metrics` and a JSON snapshot exist;
  - a static dashboard page and alert thresholds exist;
  - it runs against both the local scenario and the staging market.

### Cross-cutting decision: posting upstream

The visible path to influence is posting bug reports with reproductions
upstream. **Owner decision.** Until it's made, bugs found are written
up in `prep.md` under "Found in load test" and nothing is posted.

## At launch: we are the operator

There are no market operators today. Plan as the first one, and offer the
stack to others only once they exist.

### 4. Run our own market

The wrapper README's deployment flow has the signer of `InitMarket` become
`marketauth`. On the published program, anyone appears able to open a
market; *to verify* against the deployed program.

Being the operator means filling every role ourselves:

| Role | Our tool |
|---|---|
| `marketauth` and asset admins | key procedure (multisig and cold keys) |
| Oracle authority for AuthMark assets | **price pusher** bot |
| Insurance seed | capital plus `insurance_operator` procedure |
| LP and matcher | matcher kit (oracle ± spread template) plus LP capital |
| Backing provider | backing manager bot |
| Keeper | cranker |
| Watching | monitor |

**Economics, to verify before committing capital:**
- Trading fees and asset-creation fees go into insurance; a share is
  redirected to asset-0 insurance.
- Live insurance withdrawal is gated: the market must be flat or healthy, or
  resolved.
- So operator income may come mainly from:
  - being the LP (spread and fees);
  - backing fees;
  - fee shares paid to our own cranker;

  rather than from a withdrawable operator fee.

**Gates:**
- an official program ID, or a decision to deploy our own instance (we then
  carry its security);
- the audit status;
- **legal review** of operating a perps market (jurisdiction, access
  restrictions);
- a capital budget for insurance, LP inventory and backing;
- key custody for `marketauth`.

**Start small:** one or two assets, tight OI caps, a conservative leverage
setting, and our own LP as the only liquidity.

### 4b. Offer the operator stack to others (later)

Once other operators appear (permissionless listing, prog #447), package the
cranker, price pusher and monitor as a service. The pricing depends on the
keeper-reward decision and fee shares.

## Bigger scope (new products)

### 5. SDK and indexer for v16

It would decode layout-18 accounts and events, track portfolio state, and
serve an API for frontends, wallets and analytics. No public one exists, and
the cranker already has most of the decoding. **Likely the best-value, least
risky product.**

### 6. Backing-provider manager or vault

Counterparty backing has expiries and must be rolled; resolved surplus can be
withdrawn (#223). Automating that, or pooling it, is real work.
**Gate:** *to verify* how backing providers are compensated in the engine.

### 7. v16 market-setting simulator

It would cover margins, OI caps, backing horizon and funding, for people
listing markets. No such tool exists for the v16 engine.

### 8. Own venue or frontend

Highest upside, but also the most capital, regulatory exposure and
competition. Hold until launch and real flow.

## Order

1 → 2 (operator rehearsal on devnet, including the price pusher and a
basic matcher) → 3 now. Then the matcher kit and SDK (5), which our own market
needs anyway. Decide 4 (mainnet market) when its gates clear; 4b only when
other operators exist.

## Risks

- Crank pay could settle near zero.
- Upstream could ship an official keeper.
- Another team could publish a v16 keeper first.
- Upstream API churn could make each release cost a re-pin. Mitigate by
  pinning to the wrapper's `main` and keeping the re-pin path scripted.

## Signals that change this plan

- prog #456 merge;
- `declare_id!` change or a first release tag;
- the keeper-reward decision;
- fee-share defaults;
- how backing providers are paid;
- another public v16 cranker appearing.
