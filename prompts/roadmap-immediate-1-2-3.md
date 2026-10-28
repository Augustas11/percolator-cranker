# Session prompt: execute ROADMAP.md immediate deliverables 1, 2, 3

Give this whole file to the session **"Cranker re-pin and upstream changes"**
as its next instruction. That session continues: it finishes the re-pin, then
takes over execution of `ROADMAP.md`, starting with deliverables 1, 2 and 3.

## Mission

Turn the re-pinned cranker into three things:
1. **the first public reference cranker** for the current Percolator engine
   and wrapper, published as a **new, clean public repo**. This repo,
   `percolator-cranker`, stays private as the working, ops and strategy repo;
2. **a staging market run as an operator rehearsal**: the owner is the
   first market operator, so every operator role (keeper, price pusher, LP
   matcher, backing, insurance, keys) runs as a bot or a written procedure.
   It runs unattended, locally now and on devnet when the owner deploys it;
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

## Deliverable 1: public reference release in a NEW repo

**This repo (`percolator-cranker`) is never made public.** It keeps its full
history, `prep.md`, watcher outputs and strategy docs as they are; there is
nothing to scrub or delete here. The public release is a **new repository**
with fresh history, a proper name, a clean layout and a README written for
outside readers. The owner creates it; you prepare everything it needs.

1. **Propose name and positioning** (owner decides).
   - Give 3–5 name options with a one-line rationale each.
   - Avoid names that read as official upstream projects. If "percolator"
     appears in the name, the README must state clearly that this is an
     independent, unofficial keeper for the Percolator protocol.
   - Propose the GitHub description and topics.
2. **Design the public layout.** Write it into `docs/PUBLIC_REPO_PLAN.md`
   here. The default proposal:
   ```
   README.md  LICENSE  CHANGELOG.md  SECURITY.md  CONTRIBUTING.md
   Cargo.toml                 # workspace
   crates/
     core/      # account decoding, tx builders, pins (later grows into the SDK)
     cranker/   # binary: discovery, plan gate, submit, quarantine
     monitor/   # Deliverable 3
   staging/     # trader bots and scenarios (Deliverable 2)
   scripts/     # bring-up, staging, export checks
   docs/        # ARCHITECTURE, RUNBOOK, STAGING, MONITOR
   .github/workflows/ci.yml
   ```
   - Split out the shared decoding and transaction code from today's
     `client/` and `cranker/` into `crates/core` where it is clean to do so.
     Don't do a large refactor before the first release.
3. **Build an allowlist export, not a copy.**
   - Write `scripts/export_public.sh <target-dir>`. It copies **only
     allowlisted paths** (code, scripts, product docs) into a clean tree and
     applies the public layout.
   - It must never copy:
     - `prep.md`, `ROADMAP.md`, `cranker-strategic-questions.md`, `prompts/`;
     - watcher outputs (`digests/`, `signals.md`, `radar.md`,
       `contributors.md`, `watchlist.md`, `releases/`, `state/`);
     - `repo/`, `repo-prog/`, ledgers, keypairs, `.env` files.
   - Add `scripts/check_public_tree.sh <dir>`. It fails on any of these:
     - secrets or keypair JSON;
     - absolute local paths (e.g. `/Users/...`);
     - personal emails;
     - links to this private repo;
     - any third-party handle, org name or competing tool name;
     - anything that isn't the upstream dependencies
       (`aeyakovenko/percolator`, `aeyakovenko/percolator-prog`) or this
       project.

     Keep the denylist in a file that is **not** exported.
   - Product docs and code comments must stand on their own. No references
     to watcher digests, strategy notes or third parties.
4. **Packaging** (written as source files that the export places into the
   public tree):
   - **`LICENSE`:** propose Apache-2.0 to match upstream, but **ask the
     owner before adding a license**.
   - **`README.md`:**
     - what this is, and the "independent reference keeper; not a paid
       service; not affiliated with upstream" status;
     - a quickstart;
     - the pins table and the CU baselines;
     - links to the docs.
   - **`docs/RUNBOOK.md`:**
     - prerequisites (Solana toolchain, the platform-tools v1.52 side-load);
     - building the wrapper at the pinned SHA;
     - local bring-up;
     - running the cranker;
     - configuration (env vars, keys, intervals, quarantine N and backoff);
     - reading the cycle summary (progress / idle / failed);
     - re-pinning when upstream moves;
     - known limitations.
   - **The rest:** `CHANGELOG.md`, `SECURITY.md` (private disclosure
     contact from the owner), `CONTRIBUTING.md` and `docs/ARCHITECTURE.md`.
5. **CI** for the public repo, in `.github/workflows/ci.yml`. On each push it
   runs:
   - build;
   - unit tests;
   - an SBF build of the pinned wrapper;
   - the bring-up;
   - a shortened `live_shaped` run (e.g. 50 cycles).

   Cache the toolchain. If the SBF toolchain can't run in CI, gate that job
   and say so in the README instead of faking green. Prove it by running the
   exported tree's tests locally from the export directory.
6. **Release handoff**
   - Run the export into a scratch directory, run the check script, and
     build and test the exported tree on its own.
   - Report the result to the owner along with the name options.
   - After the owner creates the new repo and gives this session push
     access, push the export as a **single initial commit**, then tag
     `v0.1.0` once CI is green there. The release notes cover:
     - pins;
     - what the cranker handles;
     - quarantine and load-test results;
     - CU.
   - **Don't create the repository yourself.** Don't change visibility of
     any repo.
7. **Going forward** (propose; owner decides). By default the public repo
   becomes the code home after `v0.1.0`, and this repo keeps ops, strategy,
   `prep.md` and the watcher outputs. Write down the sync direction in
   `docs/PUBLIC_REPO_PLAN.md` so code doesn't drift between the two.

## Deliverable 2: staging market = operator rehearsal

> **Update 2026-10-28 (owner):** R2 is a single **SOL-PERP** market (SOL/USD
> priced, test USDC collateral, SOL as an AuthMark asset priced by our
> pusher from Pyth SOL/USD, a seeded mock locally). Where this section says
> 3+ assets or Hybrid assets, read one asset (SOL). This market becomes the
> PoC beta in `ROADMAP.md`. Keep it simple: smallest change that meets the
> criteria.

There are no market operators yet; **the owner is the first operator**. Run
the staging market exactly as a real market: the owner's (staging) keys hold
`marketauth`, every asset admin, oracle authority, insurance operator and
backing authority, and every role gets a bot or a written procedure.

1. **Local first.** Extend the `live_shaped` scenario into a long-running
   staging mode (`scripts/staging_local.sh`):
   - a local validator;
   - the wrapper deployed with a staging program keypair;
   - one asset (SOL-PERP), about 40 trader bots and one maker;
   - a deterministic but evolving price path: random walk with shocks that
     cause liquidations, lapsed/impaired buckets and reversals;
   - nonzero funding;
   - SOL scheduled once through Recovery → Resolved and then relisted.

   It must run unattended for at least 6 hours without operator action.
   Record the result in `prep.md`.
2. **Operator roles** (new modules under `staging/` or `crates/`):
   - **Price pusher.** For AuthMark assets, a bot that reads Pyth (devnet or
     a local mock) and signs `PushAuthMark` with sequence handling, a
     staleness alarm and a max-move guard. SOL is AuthMark; no Hybrid
     asset for now.
   - **LP matcher.** A minimal matcher program (oracle ± spread, ABI v3,
     modelled on the wrapper's own test fixture semantics but written fresh),
     configured with `SetMatcherConfig` on an operator LP portfolio. Traders
     use `TradeCpi` against it. This becomes the seed of the matcher kit.
   - **Backing manager.** It tops up and rolls counterparty backing before
     expiry and withdraws resolved surplus. Document how backing providers
     are compensated (backing fee on risk-increasing fills, split with
     insurance), with source citations.
   - **Insurance and keys.** Seed insurance and document the withdrawal
     gates. Write `docs/OPERATOR_PLAYBOOK.md` covering:
     - key layout (multisig for `marketauth`, cold asset admins);
     - market parameters (margins, fees, fee shares, OI caps,
       `force_close_delay_slots`, backing horizon), with the reasoning;
     - listing and shutting down an asset;
     - the incident runbook (oracle stale, stuck account, Recovery);
     - **where operator income actually comes from**, with source citations.
       Fees accrue to insurance, and live withdrawal is gated, so state
       plainly what an operator can and cannot take out.
3. **Trader bots**
   - A small binary or module, `staging/traders`, driving deposits, trades,
     withdrawals and closes through the same client code the cranker uses.
   - Configurable via TOML.
   - No hardcoded keys; generate them into an ignored directory.
4. **Devnet readiness (scripts only)**
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
5. **Findings.** Anything that looks like an engine or wrapper bug goes into
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
- **Owner decisions are needed for:** the license, the public repo's name
  and creation, the code-home and sync model, devnet deploy, and posting
  upstream. Batch these questions; don't block other work on them.
- **Records:** update `prep.md` milestones (add M2.j re-pin, R1 public
  release, R2 staging, R3 monitor) and CU baselines as they change.

## Acceptance criteria

- [x] R1: name options and `docs/PUBLIC_REPO_PLAN.md`; `export_public.sh` and
      `check_public_tree.sh` passing; the exported tree builds and tests on
      its own; LICENSE (owner-approved), README, RUNBOOK, CHANGELOG, SECURITY
      and CI ready; pushed as a single commit to the owner-created repo, and
      `v0.1.0` tagged there (created only after CI
      is green).
- [x] Step 0 re-pin done (PR #1); R1 shipped as drip v0.1.0 (2026-10-28).
- [ ] R2 (SOL-PERP): local staging runs 6+ hours unattended with the operator roles
      (price pusher, LP matcher, backing manager, insurance) running as bots;
      `OPERATOR_PLAYBOOK.md` written, including operator income; devnet scripts, STAGING
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
