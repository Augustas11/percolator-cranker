# Roadmap: from reference cranker to product

Written 2026-10-28 from the percolator-watch digest `digests/2026-10-28.md` and
the follow-up analysis. Positioning is unchanged: **open-source reference
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
- **Live-shaped test deployments exist outside this repo.** Commoneffort runs
  two v16 test deployments on a forked engine (~40 accounts each) and calls
  `PermissionlessCrank` against them. Within 30 minutes they found an account
  no crank could refresh (engine `5f2a7151`, percolator #226). It is unknown
  whether they run a cranker service, and no public repo of theirs has been
  found.
- **A competing public keeper exists, but it is stale.** `kamiyoai/percli`
  (the same org as upstream contributor mizuki0x) ships a `keeper` command,
  but on the pre-v16 engine API and its own program, with `master` last
  changed 2026-04-13. A v16 port would make it a direct competitor.
- **No public cranker for the current engine and wrapper is known.** The
  reference spot is open.

## In flight

- **Re-pin to the launch fixes:** `percolator-watch/prompts/cranker-repin-launch-fixes.md`
  (session "Cranker re-pin and upstream changes"). Covers the new pins,
  Resolved-mode asset hint, recovery position epoch, exact funding,
  per-account quarantine, and a `live_shaped` local load scenario.

## Immediate (next 2–4 weeks)

### 1. Ship the first public cranker for the current engine and wrapper

Release it as a tagged version with a runbook, quarantine logic and
load-test results.
- **Why:** being first, documented and current is the cheapest moat. It
  lasts until someone ports `percli keeper` to v16.
- **Done when:**
  - tag `v0.1.0` exists on the re-pinned surface;
  - the README, `docs/RUNBOOK.md`, `CHANGELOG.md` and the license are in
    place;
  - CI runs build, tests and the `live_shaped` scenario;
  - a publish checklist is ready.
- **Owner decision:** flipping the repo to public, and separating the
  watcher outputs (`digests/`, `signals.md`, …) from the published tree.

### 2. A persistent staging market

Run your own copy of the wrapper (own program ID) on devnet, 24/7, with the
cranker and scripted traders: several assets, funding on, and at least one
asset driven through Recovery → Resolved.
- **Why:** it reproduces what Commoneffort learned under load, gives you
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
  - unowned collateral (`vault − c_tot − insurance − pnl_pos_tot`, the
    metric Commoneffort tracked by hand; map it to field names, *to verify*);
  - cranks by outcome (progress, idle, failed), and CU used.
- **Why:** every market operator needs this at launch, and it is the first
  thing you can sell beyond cranking.
- **Done when:**
  - Prometheus `/metrics` and a JSON snapshot exist;
  - a static dashboard page and alert thresholds exist;
  - it runs against both the local scenario and the staging market.

### Cross-cutting decision: posting upstream

The watcher stays read-only. For the cranker work, the visible path to
influence is posting bug reports with reproductions upstream, as Commoneffort
and mizuki0x did. **Owner decision.** Until it's made, bugs found are written
up in `prep.md` under "Found in load test" and nothing is posted.

## At launch

### 4. Crank-as-a-service for market operators

Asset listing is becoming permissionless (prog #447), so there will be many
markets that each need reliable cranking. Sell uptime guarantees and
monitoring, not the crank itself. Pricing depends on the keeper-reward
decision and the fee shares each market sets.

**Gate:** the keeper-reward policy decision and non-zero fee-share
defaults, or operators willing to set them.

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
listing markets. This is the gap `percli` leaves by staying on the old engine.

### 8. Own venue or frontend

Highest upside, but also the most capital, regulatory exposure and
competition. Hold until launch and real flow.

## Order

1 → 2 → 3 now, then 5. Decide 4 and 6 when crank economics and provider
compensation are known.

## Risks

- Crank pay could settle near zero.
- Upstream could ship an official keeper.
- kamiyoai could port `percli keeper` to v16 first.
- Upstream API churn could make each release cost a re-pin. Mitigate by
  pinning to the wrapper's `main` and keeping the re-pin path scripted.

## Signals that change this plan (watched by percolator-watch)

- prog #456 merge;
- `declare_id!` change or a first release tag;
- the keeper-reward decision;
- fee-share defaults;
- how backing providers are paid;
- any v16 activity in `kamiyoai/percli`;
- a public Commoneffort cranker or fork.
