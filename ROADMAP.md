# Roadmap: from reference cranker to product

Written 2026-10-28. Positioning is unchanged: **open-source reference
cranker + service operator** (see `prep.md`). This file orders the work. It
records facts as of 2026-10-28, and items marked *to verify* are not
established yet.

## Situation (2026-10-29)

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
- **Upstream does not build frontends or run markets.** It ships the engine
  and wrapper; operating, interfaces and keepers are left to others.

## Done (2026-10-28)

- **Re-pin to the launch fixes** (PR #1): wrapper `5cb331dd`, engine
  `4db11a8c`; Resolved-mode hint, recovery epoch re-read, funding dust,
  per-account quarantine, cycle budget, fair scheduling, idle vs failed.
- **Upstream bug report:** F1 (unreferenced lapsed backing wedges Live
  refresh and liquidation) filed as percolator #243 with a LiteSVM repro.
  Nothing else posted.
- **Item 1 shipped:** `drip` v0.1.0 at `Augustas11/drip` (plain keeper,
  Apache-2.0). The load test and findings stay private, on
  `claude/live-shaped-scenario`.
- **Item 2 (R2):** SOL-PERP staging market, 6-hour unattended run with zero
  failures and the full lifecycle (PR #7).
- **Item 3 (R3):** liveness and solvency monitor, both injected faults caught
  (PR #8).
- **Ops half of the Day-0 kit** (PR #9) and **tech half** (PR #11): one
  config, one-command launch (fresh launch key, then handover to the
  multisig), timed drill ~27–33 s locally.
- **Money case** (PR #10, `research/replay/REPORT.md`): 90 days of real SOL
  flow through our matcher. **5 bps loses** to latency arbitrage. **12 bps,
  hedged, with the posted mark within ~400 ms of the market, makes money**:
  break-even ~$133k/day with small tickets, ~$550k/day with whole large
  tickets. Measured adverse selection 2.2–2.4 bps. Jupiter/JLP keeps
  17–23 bps.
- **Devnet:** unmodified upstream wrapper at `5cb331dd` deployed as
  `GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS` (2026-10-29). Upstream's own
  deployments are ~1,700 commits behind.
- **Upstream inventory:** six repos (`percolator`, `-prog`, `-cli`, `-match`,
  `-stress-test`, `-meta`). `percolator-meta` (token + DAO market controller)
  is undeployed and parked; it cannot take a market without its owner's
  signature. We keep maintenance fee 0 and listing closed to stay compatible.

## In flight (money-critical, from the replay)

The replay turned two engineering numbers into the business: **fee ≥ 12 bps**
(set in the launch config) and **mark lag ≤ ~400 ms**. The local drill
measured 525 ms. These tracks follow from it:

1. **Fast pusher on real Pyth.** Replace the mock price with Pyth SOL/USD
   (Hermes streaming), push every slot with priority fees, measure mark lag
   continuously on devnet against mainnet Pyth. Matcher keeps refusing fills
   while the mark lags.
2. **Order-size cap and tight OI caps.** Large whole tickets cut the edge
   (break-even ~$133k → ~$550k/day); the LP co-signs fills, so refuse
   oversized ones.
3. **Hedging bot.** The profit assumes a hedged LP. Keep net SOL near zero on
   a liquid external venue. Owner: open the venue account (~$20k margin per
   the capital plan).
4. **TS client for the frontend.** Fresh v16 client (~4–6 days) so the
   frontend trades on devnet. Small retail tickets are the profitable flow
   profile, so the frontend is the volume plan.
5. **Devnet live 24/7** with `drip`, the bots and the monitor; publish crank
   and uptime numbers only (bot volume is not traction).
6. **Re-run the replay on real Pyth data** once the devnet pusher has logged
   enough of it (the current run uses Binance spot as the oracle stand-in).

## Immediate (next 2–4 weeks)

### 1. Ship the first public cranker for the current engine and wrapper — DONE

Shipped as `drip` v0.1.0 (2026-10-28). Load-test results stay private, so
CI on drip runs build, tests and the dry run, not the scenario. Original
plan, for the record:

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
cranker and scripted traders. **Start with one market: SOL-PERP** (SOL/USD
priced, test USDC collateral), funding on, conservative leverage, tight OI
caps, our matcher as the only LP. SOL is an AuthMark asset whose price
pusher signs marks from Pyth SOL/USD (a seeded mock locally). SOL goes once
through Live → Recovery → Resolved → relisted. Add a second asset only after
that lifecycle has passed.
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

### PoC beta: run SOL-PERP perps ourselves (after 2 and 3)

**Goal:** a proof of concept that we can actually run perps on the
Percolator engine and operate them, validated with data. It is not a demo
for others.

**What it is:** the SOL-PERP market from item 2 on devnet, running for
weeks, with `drip` cranking, the operator bots (price pusher, matcher,
backing manager, insurance), the monitor from item 3, and a **minimal
trading frontend** so invited humans trade next to the bots (deposit,
open, close, withdraw) with test USDC from a faucet.

**Pass/fail, fixed before launch:**
1. **Liveness:** no account stuck past the quarantine window, except known
   upstream bugs (e.g. #243), each logged with a repro.
2. **Solvency:** unowned collateral stays bounded; every resolved claimant
   is paid.
3. **Lifecycle:** SOL goes listing → Live → Recovery → Resolved → relisted
   under real trading.
4. **Operations:** the pusher, matcher and backing manager run unattended;
   every incident is handled from the playbook.
5. **Economics:** measured operator income (LP spread, backing fees,
   cranker share) against cost (insurance, backing capital, SOL).

**Phases:**
1. Devnet SOL-PERP with bots only (items 2 and 3 on devnet).
2. Add the trading frontend and invited testers.
3. Decision: if 1–5 pass, item 4 (mainnet) goes on the table through its
   gates; if not, we have the data and repros to say why.

**Guardrails:** devnet and test tokens only, invite-only testers, a clear
"demo, no real funds" label, and no real-money path before the legal
review in item 4.

### Cross-cutting decision: posting upstream

**Decided for F1 only** (percolator #243, 2026-10-28). Other findings stay
private in `cranker/scenario-findings.md` unless the owner decides to post
them.

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

1, 2, 3 and the Day-0 kit are done. Now, in parallel: fast Pyth pusher +
size caps (on devnet), hedging bot, TS client → **PoC beta** phase 1 (devnet
24/7, bots) → phase 2 (frontend + invited testers). Mainnet SOL-PERP only on
the official program ID, with fee ≥ 12 bps, measured mark lag ≤ ~400 ms and
the hedge live. 4b only when other operators exist.

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
