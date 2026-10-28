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
| `percolator-keeper-sdk` | Leaves room for the SDK (decoding and tx builders) that is the only planned addition. |

Every option that contains "percolator" ships with this README line: *"An
independent, unofficial reference keeper for the Percolator protocol. Not
affiliated with the upstream authors. Not a paid service."*

**GitHub description:** "Independent reference keeper (cranker) for the
Percolator perp engine on Solana. Dry-run and local-validator only."

**Topics:** `solana`, `perpetuals`, `keeper`, `cranker`, `defi`, `rust`,
`risk-engine`, `liquidation`.

## Scope (owner decision, 2026-10-28)

The public repo is the **plain keeper only**, and later the SDK. The
operator stack stays private:
- staging market, trader bots, matcher, backing manager;
- operator playbook, devnet deploy scripts, hosting recipes;
- the liveness/solvency monitor (may be sold later);
- the load-test scenario and its findings.

`scripts/export_public.sh` never copies any of those paths, and
`scripts/check_public_tree.sh` fails if one appears (path rules plus the
private denylist).

## Layout

```
README.md  LICENSE  CHANGELOG.md  SECURITY.md  CONTRIBUTING.md
Cargo.toml                 # workspace
crates/
  cranker/   # keeper: discovery, plan gate, submit, quarantine, scheduler
  bringup/   # local smoke test
  core/      # later: account decoding, tx builders, pins (grows into the SDK)
scripts/     # bring-up, fetch, export checks
docs/        # ARCHITECTURE, RUNBOOK
.github/workflows/ci.yml
```

**v0.1.0 as built:** `crates/cranker` (the whole keeper crate: library,
`cranker` binary, integration tests) and `crates/bringup`
(the smoke-test client). `upstream/` is fetched at the pinned SHA by
`scripts/fetch_upstream.sh` and is gitignored.

**Not split yet, on purpose:**
- `crates/core` waits until after v0.1.0. The candidates are `binding`,
  `discovery`, `observation`, `plan_gate`, `submit` and `resolved`. The
  bring-up client still keeps its own copies of a few helpers, and merging
  them is the first core-crate task.
- No monitor, staging or operator crates: they stay private (see Scope).

## How the export works

| Piece | Where | Exported? |
|---|---|---|
| `scripts/export_public.sh <new-dir>` → `export_public.py` | private only | no |
| `scripts/public_manifest.txt`: the exact file list, `<private path> <public path>` | private only | no |
| public-only files (README, docs, workspace `Cargo.toml` and lock, public-layout `env.sh`, crate READMEs, `.gitignore`) | `public/` overlay, listed in the manifest | yes |
| shared with this repo (identical at each export; private CI re-runs the export on every change): `.github/workflows/ci.yml`, `scripts/fetch_upstream.sh`, `scripts/pins.env`, `m1_bringup.sh` | private tree, listed in the manifest | yes |
| layout rewrites (path dep `../repo-prog` → `../../upstream/percolator-prog`), `@PUBLIC_NAME@`, `@SECURITY_CONTACT@` | `export_public.py`, each anchored and checked | — |
| `scripts/check_public_tree.sh [--draft] [--allow-email ADDR] <dir> [denylist]` → `check_public_tree.py` | private, and exported (generic checks) | yes |
| `scripts/public_denylist.txt` | private only | no (asserted) |
| `scripts/test_check_public_tree.py`: regression probes for every gate bypass found in audits | private only | no |

**Export.**
- The destination must not exist.
- The tree is built in a fresh temp dir next to it, and renamed into place
  only after every check passes. A failed export leaves nothing behind and
  never touches existing files.
- HEAD is resolved once. Exactly the manifest's files are read from that
  commit (`git cat-file`), never from the working tree.
- Every file committed at that HEAD under `cranker/src`, `cranker/tests`,
  `client/src` and `public/` must be listed in the manifest. A new file goes
  public only on purpose.
- Destinations are normalized, and must not escape the tree, land in
  `target/` or `upstream/`, or use dot components other than `.github` and
  `.gitignore`.
- Without `SECURITY_CONTACT` the export is a DRAFT and says so.

**Content gate.** It fails closed:
- **File checks:** walk errors, bad names, dot paths and symlinks fail, and
  any file that isn't strict UTF-8 without NULs fails. The whole file is
  decoded, so late binary or UTF-16 content can't hide.
- **Secrets:** secret-like file names, 64-byte key arrays, and private-key,
  seed-phrase and mnemonic markers.
- **Locations:** absolute local paths and emails. The one exception is the
  approved security contact, and only in `SECURITY.md`.
- **Links:** URLs (parsed, with an allowlisted host and path) and any
  `github.com/<owner>/<repo>` that isn't one of the two upstream repos.
- **Private material:** operator-stack paths, unreplaced placeholders, and
  the denylist, checked in both contents and file names.
- **Test fixtures:** a line marked `public-check: allow-test-fixture` is
  exempt from the email and URL rules only. It must be in `crates/*/tests/`
  and use loopback or documentation-placeholder hosts.
- **Cargo.lock** is exempt from the URL rule only; its sources come from
  upstream manifests.

The scripts read their layout from env vars with private-layout defaults
(`PERCOLATOR_PROG_DIR`, `BRINGUP_DIR`, `CRANKER_MANIFEST`), so the same
`m1_bringup.sh` runs in both trees. The public `env.sh` sets the public
layout.

## CI

Both repos run the **same** `.github/workflows/ci.yml`, copied by the
export. It is layout-neutral: each tree's `scripts/env.sh` names the
wrapper checkout, the bring-up crate and the keeper manifest.

- **host** (push to `main`, pull requests, dispatch): fetch the pinned
  wrapper, build, run unit tests.
- **public-export:** runs its steps only where `scripts/export_public.py`
  exists, so it does nothing in the public repo. It runs the gate's
  regression probes, then the export and its gate with the private
  denylist, then a `--locked` build and test of the exported workspace.
- **e2e** (pull requests, manual dispatch): SBF build with platform-tools
  v1.52, validator bring-up, a 3-cycle dry run with fail-on-error, and the
  validator integration tests. Nothing is non-gating. (The `live_shaped`
  load test is not part of the public release; it is maintained
  separately.)
- **Caches:** cargo registry and target dirs, the Solana CLI and
  platform-tools, and the wrapper SBF target.

## Release checklist (after the owner creates the repo)

1. `PUBLIC_NAME=<chosen> SECURITY_CONTACT=<approved> ./scripts/export_public.sh <new-dir>`.
   It must end with `exported to <new-dir>` and must not say DRAFT.
2. In `<scratch>`, run `./scripts/fetch_upstream.sh`, `cargo build`,
   `cargo test`, the bring-up and a 3-cycle dry run. All green.
3. Add the owner-approved `LICENSE` (add it to `public/` and the manifest,
   so it goes through the gate).
4. `git init`, then push one commit to the new repo. Wait for CI to go
   green; dispatch the gated e2e job once.
5. Tag `v0.1.0`. The release notes cover pins, keeper behavior,
   quarantine, CU and the known issue (upstream #243). **Never** include
   load-test results or findings: they stay private (see Scope). Release
   notes are written by hand, so they don't pass through the export. Gate
   them first: `mkdir -p <tmp>/notes && cp RELEASE_NOTES.md <tmp>/notes/ &&
   ./scripts/check_public_tree.sh <tmp>/notes scripts/public_denylist.txt`.

## Export rules

- **Manifest only.** `scripts/export_public.sh` copies exactly the
  committed files in `scripts/public_manifest.txt`; nothing is copied by
  default.
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
