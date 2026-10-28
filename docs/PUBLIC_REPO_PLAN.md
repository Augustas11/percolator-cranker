# Public repo plan (R1)

Status: export and check scripts are done and green. The exported tree
builds and tests on its own, including bring-up and a dry run on a local
validator. Waiting on the owner for the decisions at the bottom.

This private repo (`percolator-cranker`) is never made public. The public
release is a **new repository** with one fresh initial commit, produced by an
allowlist export (`scripts/export_public.sh`) and gated by a content check
(`scripts/check_public_tree.sh`). The owner creates the repository; this
session prepares the tree.

## Name options (owner decides)

| Name | Rationale |
|---|---|
| `perc-keeper` | Short and functional; the README says plainly that it is an independent, unofficial keeper for the Percolator protocol. |
| `percolator-reference-keeper` | Says exactly what it is. Long, but "reference" sets expectations (no paid service). |
| `open-percolator-crank` | Leads with the open-source stance. "crank" matches upstream vocabulary (`PermissionlessCrank`). |
| `drip` | Brandable and unrelated to upstream naming, so it can't be mistaken for an official project. Needs a descriptive tagline. |
| `percolator-keeper-kit` | Leaves room for the matcher kit and monitor (R2/R3) under one name. |

Every option that contains "percolator" ships with this README line: *"An
independent, unofficial reference keeper for the Percolator protocol. Not
affiliated with the upstream authors. Not a paid service."*

**GitHub description:** "Independent reference keeper (cranker) and
liveness monitor for the Percolator perp engine on Solana. Dry-run and
staging only."

**Topics:** `solana`, `perpetuals`, `keeper`, `cranker`, `defi`, `rust`,
`risk-engine`, `liquidation`.

## Layout

```
README.md  LICENSE  CHANGELOG.md  SECURITY.md  CONTRIBUTING.md
Cargo.toml                 # workspace
crates/
  core/      # account decoding, tx builders, pins (grows into the SDK)
  cranker/   # binary: discovery, plan gate, submit, quarantine, scheduler
  monitor/   # R3
staging/     # trader bots and scenarios (R2)
scripts/     # bring-up, staging, export checks
docs/        # ARCHITECTURE, RUNBOOK, STAGING, MONITOR
.github/workflows/ci.yml
```

**v0.1.0 as built:** `crates/cranker` (the whole keeper crate: library,
`cranker` and `live_shaped` binaries, integration tests) and `crates/bringup`
(the smoke-test client). `upstream/` is fetched at the pinned SHA by
`scripts/fetch_upstream.sh` and is gitignored.

**Not split yet, on purpose:**
- `crates/core` waits until after v0.1.0. The candidates are `binding`,
  `discovery`, `observation`, `plan_gate`, `submit` and `resolved`. The
  bring-up client still keeps its own copies of a few helpers, and merging
  them is the first core-crate task.
- `crates/monitor` comes with R3, and `staging/` with R2.

## How the export works

| Piece | Where | Exported? |
|---|---|---|
| `scripts/export_public.sh <dir>` | private only | no |
| allowlist | inside `export_public.sh` (explicit `put` lines) | — |
| public-only files (README, docs, CI, workspace `Cargo.toml`, `env.sh`, `pins.env`, `fetch_upstream.sh`, crate READMEs, `.gitignore`) | `public/` overlay | yes |
| layout rewrites (path dep `../repo-prog` → `../../upstream/percolator-prog`, `@PUBLIC_NAME@`) | `export_public.sh`, each anchored and checked | — |
| `scripts/check_public_tree.sh <dir> [denylist]` | private, and exported (generic checks) | yes |
| `scripts/public_denylist.txt` | private only | no (asserted) |

The scripts read their layout from env vars with private-layout defaults
(`PERCOLATOR_PROG_DIR`, `BRINGUP_DIR`, `CRANKER_MANIFEST`), so the same
`m1_bringup.sh` and `live_shaped.sh` run in both trees. The public
`env.sh` sets the public layout.

Content gate: secrets and keypair JSON, absolute local paths, emails, URLs
outside the allowlist (the two upstream repos, toolchain installers,
registries, localhost), and the denylist. The URL rule skips `Cargo.lock`,
because its sources come from upstream manifests. A negative test with
planted violations is in the R1 PR description.

## Release checklist (after the owner creates the repo)

1. `PUBLIC_NAME=<chosen> ./scripts/export_public.sh <scratch>`. It must end
   with `== OK`.
2. In `<scratch>`, run `./scripts/fetch_upstream.sh`, `cargo build`,
   `cargo test`, the bring-up and a 3-cycle dry run. All green.
3. Add the owner-approved `LICENSE` and replace `@SECURITY_CONTACT@` in
   `SECURITY.md`.
4. `git init`, then push one commit to the new repo. Wait for CI to go
   green; dispatch the gated e2e job once.
5. Tag `v0.1.0`. The release notes cover pins, keeper behavior, quarantine
   and load-test results, CU, and the known issue.

## Export rules

- **Allowlist only.** `scripts/export_public.sh` copies named paths; nothing
  is copied by default.
- **Never exported:** `prep.md`, `ROADMAP.md`,
  `cranker-strategic-questions.md`, `prompts/`, watcher outputs (`digests/`,
  `signals.md`, `radar.md`, `contributors.md`, `watchlist.md`, `releases/`,
  `state/`), `repo/`, `repo-prog/`, `.preview/`, ledgers, keypairs, `.env`,
  and `scripts/public_denylist.txt` itself.
- **Content gate.** `scripts/check_public_tree.sh` fails on keypair JSON or
  secrets, absolute local paths, personal emails, links to this private repo,
  and any denylisted handle, org name or tool name.

## Sync model after v0.1.0 (proposal; owner decides)

The public repo becomes the **code home**. This private repo keeps ops,
strategy, `prep.md` and the watcher outputs, and consumes the public crates
as a git dependency pinned by tag. Code changes land in public first; this
repo only bumps the tag. Until the owner decides, code stays here and each
release is a fresh export (one commit per release, no shared history).

## Owner decisions needed

1. **Name**: pick one from the table above (or another).
2. **License**: Apache-2.0 proposed, to match upstream. Nothing is added
   until you approve.
3. **Security contact**: an address or form for `SECURITY.md`.
4. **Repo creation**: you create it and give this session push access.
   This session never creates repos or changes visibility.
5. **Sync model**: confirm "public becomes the code home after v0.1.0", or
   keep export-per-release.
6. **Oracle-provider names in code comments**: Pyth, Switchboard and
   Chainlink appear because the wrapper supports them. They are protocol
   dependencies, not competing tools, so they are **not** on the denylist.
   Say if you want them generic.
7. **Known-issue wording**: the public README describes the unreferenced
   lapsed-backing wedge without naming upstream PRs. Posting it upstream
   stays your call.
