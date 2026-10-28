# percolator-cranker

Local prototype work against [aeyakovenko/percolator](https://github.com/aeyakovenko/percolator) (the risk-engine library) and [aeyakovenko/percolator-prog](https://github.com/aeyakovenko/percolator-prog) (the Solana wrapper). Private working repo for a **reference cranker + service operator** positioning — nothing here is deployed on any chain.

## What lives here

- [`cranker/`](cranker/) — M2 minimal permissionless cranker. Discovery, plan-classification gate, three-way response classifier, mirror-custody bookkeeping, real `AssetOracleProfileV16` walk + Pyth PDA resolver. See [`cranker/README.md`](cranker/README.md).
- [`client/`](client/) — M1 bring-up client. End-to-end deploy → init market → deposit → `PermissionlessCrank` round-trip against `solana-test-validator`.
- [`scripts/m1_bringup.sh`](scripts/m1_bringup.sh) — idempotent local-validator bring-up. Resets ledger, redeploys the wrapper, runs the M1 client.
- [`scripts/preview_launch_fixes.sh`](scripts/preview_launch_fixes.sh) — Step B **pre-merge preview**: wrapper prog #456 head + engine `68859b94` via path patch, isolated under `.preview/`.
- [`prep.md`](prep.md) — milestones (M1 done, M2 done, M3 blocked on devnet deployment), baked-in assumptions (with the byte-offset citations against upstream source), open questions Q1/Q2, and a running drift log tied to specific upstream commits.
- [`cranker-strategic-questions.md`](cranker-strategic-questions.md) — running answers to fee-capture / latency-sensitivity / operational-complexity / differentiation-surface questions, informed by what M1/M2 empirically observed.
- `digests/`, `signals.md`, `radar.md`, `contributors.md`, `watchlist.md`, `releases/`, `state/` — outputs from the sibling percolator-watch process that seeds prep.md and flags upstream drift. Kept alongside for context.

## Not tracked here (`.gitignore`)

- `repo/` and `repo-prog/` — upstream clones; not ours to redistribute. Pinned by rev in the two `Cargo.toml` files. Re-clone them locally alongside this repo when working on it.
- `target/`, `.solana-test-ledger/` — build/test scratch.

## Status

**Reference / dry-run only; not a paid live service.** All fee, bounty and mainnet paths stay off: nothing here claims fees, and the local bootstrap's 5000 bps cranker share only exercises the ABI path.

- **Pins (2026-10-28)**: wrapper `5cb331dd` (`origin/main`) + engine `4db11a8c` (the wrapper's Cargo pin). Upstream re-pin prog #456 is still a draft, so the engine launch-fix batch `68859b94` runs only as a **pre-merge preview** (`scripts/preview_launch_fixes.sh`, path-patched, never the default). Layout 18; `AutoCrankPlanV16` unchanged.
- **M1**: end-to-end green on the new pin. A fresh no-op `PermissionlessCrank` consumes **18,565 program CU** (tx total 18,889); it was 17,975 on the old pin. Preview pin: 18,820 (19,144).
- **M2**: loop green. It now covers the resolved-mode backing hint, `EngineNonProgress` counted as idle, per-account failure quarantine with capped backoff, and a per-cycle wall/CU/tx budget that puts liquidatable accounts first. See [`cranker/README.md`](cranker/README.md).
- **Load test**: `CRANKER_SCENARIO=live_shaped` (see [`cranker/README.md`](cranker/README.md) and `prep.md`).
- **M3**: NO-GO. `declare_id!` is still `Perco1ator111…`, there's no release tag, and fee shares default to 0.

## Attribution

The upstream projects [`percolator`](https://github.com/aeyakovenko/percolator) and [`percolator-prog`](https://github.com/aeyakovenko/percolator-prog) are © aeyakovenko under their own licenses. This repo consumes them as git dependencies at pinned revisions; nothing from those trees is redistributed here.
