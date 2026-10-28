# Session prompt: execute ROADMAP.md immediate deliverables 1, 2, 3

Give this whole file to the session **"Cranker re-pin and upstream changes"**
as its next instruction. That session continues: it finishes the re-pin, then
takes over execution of `ROADMAP.md`, starting with deliverables 1, 2 and 3.

## Mission

Turn the re-pinned cranker into three things:
1. **the first public reference cranker** for the current Percolator engine
   and wrapper (prepared for release; the owner flips visibility);
2. **a staging market** that runs unattended, locally now and on devnet
   when the owner deploys it;
3. **a liveness and solvency monitor** built on the cranker's own reads.

**Bottom line (non-negotiable):**
> Everything stays **reference / dry-run / staging**. No mainnet, no paid
> service, no fee claiming. M3 stays NO-GO until upstream publishes a program
> ID and the owner says go.

## Read first

- `ROADMAP.md` (this repo): the why and the ordering. Don't re-argue it.
- `prep.md`: pins, milestones, baked-in assumptions, drift log. Keep it
  current. Leave `## Watcher notes` for the watcher.
- The re-pin brief you are already executing.
- `cranker/README.md`, `scripts/m1_bringup.sh`,
  `cranker-strategic-questions.md`.

## Step 0: finish or confirm the re-pin (gate for everything below)

Before starting deliverable work, confirm that the re-pin brief's acceptance
criteria are met:
- pins recorded in `prep.md`;
- bring-up plus multi-cycle dry-run green;
- Resolved-mode asset hint working;
- recovery position epoch re-read;
- quarantine working;
- the `live_shaped` scenario running 200 cycles.

If anything is missing, finish it first. If prog #456 merged since you
started, do the pin bump to the wrapper's new `main` now. If it hasn't
merged, stay on wrapper `main` (`5cb331dd`, engine `4db11a8c`) with the
optional `68859b94` preview behind a flag.

Re-check upstream at the start of each work block (`git fetch`; engine
`origin/master`, wrapper `origin/main`). If the `PermissionlessCrank` wire, the
plan enum or the layout version changed, stop and do a mini re-pin before
continuing.

## Deliverable 1: public reference release (prepare; owner publishes)

1. **Packaging**
   - Add a `LICENSE`. Propose Apache-2.0 to match upstream, but **ask the
     owner before committing a license**.
   - Add `CHANGELOG.md`, and `docs/RUNBOOK.md` covering:
     - prerequisites (Solana toolchain, platform-tools v1.52 side-load from
       `prep.md`);
     - building the wrapper at the pinned SHA;
     - local bring-up;
     - running the cranker;
     - configuration (env vars, keys, intervals, quarantine N and backoff);
     - reading the cycle summary (progress / idle / failed);
     - re-pinning when upstream moves;
     - known limitations.
   - Rewrite the top of `README.md` for an outside reader:
     - what this is, and the "reference / not a paid service" status;
     - a quickstart;
     - the pins table;
     - the CU baselines.
2. **CI**
   - Add a GitHub Actions workflow that runs, on each push:
     - build;
     - unit tests;
     - an SBF build of the pinned wrapper;
     - the bring-up;
     - a shortened `live_shaped` run (e.g. 50 cycles).
   - Cache the toolchain.
   - If the SBF toolchain can't run in CI, gate that job and say so in the
     README instead of faking green.
3. **Separate watcher outputs from the product.** The repo will be made
   public. It must not mention other projects or contributors beyond the
   upstream dependencies.
   - `digests/`, `signals.md`, `radar.md`, `contributors.md`, `watchlist.md`,
     `releases/` and `state/` are watcher context, not product, and name
     third parties.
   - Write `docs/PUBLISH_CHECKLIST.md` proposing their removal from the
     published tree.
   - **Git history also contains them.** Removing files leaves them in
     history, so the checklist must propose publishing from a fresh history
     (a new repo or a squashed orphan branch). That is an owner decision;
     never rewrite or force-push `main` yourself.
   - Grep the tree for third-party handles, org names and competitor tools
     before release, and keep new docs, commits and PR text free of them.
   - The checklist should also cover: secrets scan, no keypairs in the tree,
     license, and the strategy docs to keep private or move
     (`cranker-strategic-questions.md`, the strategy section of `prep.md`,
     `ROADMAP.md`).
   - **Don't delete or move them yourself** without the owner's OK.
4. **Release**
   - Prepare a `v0.1.0` tag on the re-pinned surface, with release notes
     covering pins, what it handles, quarantine and load-test results, and CU.
   - Create the tag only after CI is green.
   - **Don't change repo visibility.** That is the owner's call.

## Deliverable 2: staging market

1. **Local first.** Extend the `live_shaped` scenario into a long-running
   staging mode (`scripts/staging_local.sh`):
   - a local validator;
   - the wrapper deployed with a staging program keypair;
   - 3+ assets, about 40 trader bots and one maker per asset;
   - a deterministic but evolving price path: random walk with shocks that
     cause liquidations, lapsed/impaired buckets and reversals;
   - nonzero funding;
   - one asset scheduled through Recovery → Resolved and then relisted.

   It must run unattended for at least 6 hours without operator action.
   Record the result in `prep.md`.
2. **Trader bots**
   - A small binary or module, `staging/traders`, driving deposits, trades,
     withdrawals and closes through the same client code the cranker uses.
   - Configurable via TOML.
   - No hardcoded keys; generate them into an ignored directory.
3. **Devnet readiness (scripts only)**
   - `scripts/staging_devnet_deploy.sh` and `docs/STAGING.md`:
     - program keypair and upgrade authority handling;
     - market init;
     - the oracle choice for devnet (Pyth devnet feeds, or the wrapper's
       Hybrid/authority oracle; document which and why);
     - funding the bots;
     - teardown.
   - Add a Dockerfile, a systemd unit or a docker-compose file for running the
     cranker, the traders and the monitor 24/7.
   - **Don't deploy to devnet yourself**, and never ask for or commit private
     keys. The owner runs the deploy script.
4. **Findings.** Anything that looks like an engine or wrapper bug goes into
   `prep.md` → "Found in load test", with a minimal repro, preferably a
   LiteSVM test. **Don't post upstream.** That's an owner decision listed in
   `ROADMAP.md`.

## Deliverable 3: liveness and solvency monitor

1. **Where it lives:** a `monitor` crate or module reusing the cranker's
   discovery and decoding. It should run alongside the cranker or on its own,
   read-only.
2. **Metrics:** each one carries labels for market and asset where relevant.
   - accounts total, and out of date (stale by the engine's own definition;
     cite the field);
   - quarantined, unrefreshable accounts, with the last error code;
   - crank outcomes per cycle (progress, idle/NonProgress, failed), and CU
     per crank (p50 and max);
   - backing buckets by status (Fresh, Impaired, Expired), and buckets
     expiring within N slots;
   - asset modes (Live, DrainOnly, Recovery, Resolved);
   - resolved claimants unpaid, and the atoms owed;
   - **unowned collateral** `vault − c_tot − insurance − pnl_pos_tot`.
     - Derive the exact field names from the engine source at the pin.
     - Document the formula in `docs/MONITOR.md` with source citations.
     - If it can't be computed exactly from public state, say so rather than
       approximating silently.
   - funding: last accrual slot per asset, and the F remainder below one atom
     (this is not pending work).
3. **Outputs:**
   - Prometheus `/metrics`;
   - a `--json` snapshot;
   - a single static HTML dashboard page (no build step) that reads the
     JSON;
   - alert rules (Prometheus rules file) with defaults:
     - any account unrefreshable for over 10 minutes;
     - unowned collateral drifting more than X per hour (configurable);
     - a bucket expiring with liens outstanding;
     - no successful crank for over N slots.
4. **Validation:**
   - Run it against the `live_shaped` scenario and the local staging market.
   - Show the dashboard catching at least one injected fault: a
     forced-failing account and a stopped cranker.

## Working rules

- **Upstream** `aeyakovenko/percolator` and `aeyakovenko/percolator-prog`
  are READ-ONLY. No PRs, issues or comments.
- **Branches and PRs:** one branch and one PR per deliverable on
  `Augustas11/percolator-cranker`. Keep PRs reviewable, and keep CI green
  before asking for review.
- **Secrets:** no keys, mnemonics or RPC tokens in commits.
- **Feature gates:** fee, bounty and mainnet paths stay feature-gated
  **off**.
- **Owner decisions are needed for:** the license, repo visibility, deleting
  or moving watcher files and strategy docs, devnet deploy, and posting
  upstream. Batch these questions; don't block other work on them.
- **Records:** update `prep.md` milestones (add M2.j re-pin, R1 public
  release, R2 staging, R3 monitor) and CU baselines as they change.

## Acceptance criteria

- [ ] Step 0 re-pin criteria met and recorded.
- [ ] R1: LICENSE (owner-approved), README, RUNBOOK, CHANGELOG, CI green,
      PUBLISH_CHECKLIST, and a `v0.1.0` tag prepared (created only after CI
      is green).
- [ ] R2: local staging runs 6+ hours unattended; devnet scripts, STAGING
      doc and hosting recipe exist; nothing deployed by you.
- [ ] R3: monitor exports metrics, JSON, dashboard and alert rules;
      unowned-collateral formula documented with citations; two injected
      faults detected.
- [ ] `prep.md` and the README status still say **reference / dry-run; not a
      paid live service; M3 NO-GO**.

## Report format

Lead with the following, then link the PRs:
- pins (unchanged or moved);
- R1, R2 and R3 status, one line each;
- owner decisions needed;
- anything found in load testing;
- the CU delta.
