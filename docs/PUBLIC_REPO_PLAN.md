# Public repo plan (R1)

Status: **released.** https://github.com/Augustas11/drip is public, with
private vulnerability reporting on. Its one initial commit `ae8845f` is the
export of private `050b0b2`. CI was green there, including e2e by
dispatch, and `v0.1.0` is tagged
(https://github.com/Augustas11/drip/releases/tag/v0.1.0). This repo stays
the code home: drip is updated only by re-running the export.

This private repo (`percolator-cranker`) is never made public. The public
release is a **new repository** with one fresh initial commit, produced by an
allowlist export (`scripts/export_public.sh`). The owner creates the
repository; this session prepares the tree.

**What protects the export (owner decision, 2026-10-28):**
1. **The allowlist export is the control.** Only the paths named in
   `scripts/public_manifest.txt` are copied, from the committed HEAD.
   Anything not listed never leaves.
2. **`scripts/check_public_tree.sh` is a simple helper**, not a security
   boundary. It greps the exported text files for obvious mistakes (see
   below) and exits non-zero on any hit.
3. **The owner reads the exported tree before the first push.** This step
   is required; see the release checklist.

## Owner decisions (2026-10-28)

- **Name:** `drip`, as `Augustas11/drip`. The README opens with: *"An
  independent, unofficial reference keeper for the Percolator protocol.
  Not affiliated with the upstream authors. Not a paid service."*
- **License:** Apache-2.0. The standard text is `public/LICENSE`, listed in
  the manifest.
- **Security contact:** GitHub private vulnerability reporting, at
  `https://github.com/Augustas11/drip/security/advisories/new`. The owner
  enables "Private vulnerability reporting" in the repo settings when
  creating the repo.

## Name options (decided: `drip`)

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

`scripts/export_public.sh` never copies any of those paths (they are not
in the manifest), and the private denylist names them so the helper flags a
reference that slips into an exported file.

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
`drip` binary, integration tests) and `crates/bringup`
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
| `scripts/check_public_tree.sh [--draft] [--allow-email ADDR] <dir> [denylist]`: the leak helper (plain `grep`) | private, and exported (generic checks) | yes |
| `scripts/public_denylist.txt` | private only | no (asserted) |
| `scripts/test_check_public_tree.py`: exporter tests (allowlist, destinations, rollback) and one basic test per helper pattern | private only | no |

**Export.**
- The destination must not exist.
- The tree is built in a fresh temp dir next to it. Only after every check
  passes is the destination created (an atomic `mkdir`, which fails if
  anything is there) and the entries moved in. A failed export, including
  one that fails while moving entries, leaves nothing behind and never
  touches existing files.
- Destinations are unique, case-insensitively.
- HEAD is resolved once. Exactly the manifest's files are read from that
  commit (`git cat-file`), never from the working tree.
- Every file committed at that HEAD under `cranker/src`, `cranker/tests`,
  `client/src` and `public/` must be listed in the manifest. A new file goes
  public only on purpose.
- Destinations are normalized, and must not escape the tree, land in
  `target/` or `upstream/`, or use dot components other than `.github` and
  `.gitignore`.
- Without `SECURITY_CONTACT` the export is a DRAFT and says so.

**Leak helper.** A plain `grep` over the exported tree, text files only
(binaries are skipped). It flags:
- private-key shapes: a 64-number array, a PEM private-key header, and
  keypair / `id.json` / `.env` file names;
- `/Users/…` and `/home/…` paths;
- email addresses, except the approved security contact in `SECURITY.md`
  and the reserved documentation domains (`example.*`, `.invalid`, `.test`);
- every denylist string (owner handles, the private repo name and URL,
  private doc names, the operator stack), in contents and file names;
- unreplaced placeholders (unless it is a draft export).

It is advisory: it catches obvious mistakes and is not built to resist
deliberate evasion. The allowlist decides what is copied, and the owner's
read is the final check.

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
  exists, so it does nothing in the public repo. It runs the exporter and
  helper tests, then the export and the helper with the private denylist,
  then a `--locked` build and test of the exported workspace.
- **e2e** (pull requests, manual dispatch): SBF build with platform-tools
  v1.52, validator bring-up, a 3-cycle dry run with fail-on-error, and the
  validator integration tests. Nothing is non-gating. (The `live_shaped`
  load test is not part of the public release; it is maintained
  separately.)
- **Caches:** cargo registry and target dirs, the Solana CLI and
  platform-tools, and the wrapper SBF target.

## Release checklist (after the owner creates the repo)

Done for v0.1.0 on 2026-10-28. Later releases repeat steps 2 to 6 on the
existing repo.

1. The owner creates `Augustas11/drip` (empty) and, in Settings →
   Security, enables **Private vulnerability reporting**. `SECURITY.md`
   points there.
2. From a clean checkout of `main`: `PUBLIC_NAME=drip SECURITY_CONTACT=https://github.com/Augustas11/drip/security/advisories/new ./scripts/export_public.sh <new-dir>`.
   It must end with `exported to <new-dir>` and must not say DRAFT.
3. In `<new-dir>`, run `./scripts/fetch_upstream.sh`, `cargo build`,
   `cargo test`, the bring-up and a 3-cycle dry run. All green.
4. **Required: the owner reads the whole exported tree** before anything is
   pushed. To produce it for review (from a clean checkout of `main` in
   this repo):

   ```bash
   PUBLIC_NAME=drip SECURITY_CONTACT=https://github.com/Augustas11/drip/security/advisories/new ./scripts/export_public.sh ../public-review
   ```

   Then read every file in `../public-review` (`find ../public-review -type
   f | sort` lists them; about 42 files). Look for anything private: names,
   paths, notes, links, numbers. Only push that exact tree.
5. `git init`, then push one commit to the new repo. Wait for CI to go
   green; dispatch the gated e2e job once.
6. Tag `v0.1.0`. The release notes cover pins, keeper behavior,
   quarantine, CU and the known issue (upstream #243). **Never** include
   load-test results or findings: they stay private (see Scope). Release
   notes are written by hand, so they don't pass through the export. Gate
   them first: `mkdir -p <tmp>/notes && cp RELEASE_NOTES.md <tmp>/notes/ &&
   ./scripts/check_public_tree.sh <tmp>/notes scripts/public_denylist.txt`,
   and read them yourself.

## Export rules

- **Manifest only.** `scripts/export_public.sh` copies exactly the
  committed files in `scripts/public_manifest.txt`; nothing is copied by
  default.
- **Never exported:** `prep.md`, `ROADMAP.md`,
  `cranker-strategic-questions.md`, `prompts/`, watcher outputs (`digests/`,
  `signals.md`, `radar.md`, `contributors.md`, `watchlist.md`, `releases/`,
  `state/`), `repo/`, `repo-prog/`, `.preview/`, ledgers, keypairs, `.env`,
  and `scripts/public_denylist.txt` itself.
- **Leak helper (advisory).** `scripts/check_public_tree.sh` flags keypair
  shapes, home paths, emails, and any denylisted handle, repo name or tool
  name. The owner's read before the first push is required.

## Sync model (owner decision, 2026-10-28)

This private repo stays the **code home**. drip is updated only by
re-running the export, as one new commit per release; there is no shared
history.

## Owner decisions still open

Name, license, security contact, repo creation and the sync model are
decided (see above).

1. **Oracle-provider names in code comments**: Pyth, Switchboard and
   Chainlink appear because the wrapper supports them. They are protocol
   dependencies, not competing tools, so they are **not** on the denylist.
   Say if you want them generic.
2. **Known-issue wording**: the public README describes the unreferenced
   lapsed-backing wedge without naming upstream PRs. Posting it upstream
   stays your call.
