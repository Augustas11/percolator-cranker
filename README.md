# percolator-cranker

Local prototype work against [aeyakovenko/percolator](https://github.com/aeyakovenko/percolator) (the risk-engine library) and [aeyakovenko/percolator-prog](https://github.com/aeyakovenko/percolator-prog) (the Solana wrapper). Private working repo for a **reference cranker + service operator** positioning — nothing here is deployed on any chain.

## What lives here

- [`cranker/`](cranker/) — M2 minimal permissionless cranker. Discovery, plan-classification gate, three-way response classifier, mirror-custody bookkeeping, real `AssetOracleProfileV16` walk + Pyth PDA resolver. See [`cranker/README.md`](cranker/README.md).
- [`client/`](client/) — M1 bring-up client. End-to-end deploy → init market → deposit → `PermissionlessCrank` round-trip against `solana-test-validator`.
- [`scripts/m1_bringup.sh`](scripts/m1_bringup.sh) — idempotent local-validator bring-up. Resets ledger, redeploys the wrapper, runs the M1 client.
- [`prep.md`](prep.md) — milestones (M1 done, M2 done, M3 blocked on devnet deployment), baked-in assumptions (with the byte-offset citations against upstream source), open questions Q1/Q2, and a running drift log tied to specific upstream commits.
- [`cranker-strategic-questions.md`](cranker-strategic-questions.md) — running answers to fee-capture / latency-sensitivity / operational-complexity / differentiation-surface questions, informed by what M1/M2 empirically observed.
- `digests/`, `signals.md`, `radar.md`, `contributors.md`, `watchlist.md`, `releases/`, `state/` — outputs from the sibling percolator-watch process that seeds prep.md and flags upstream drift. Kept alongside for context.

## Not tracked here (`.gitignore`)

- `repo/` and `repo-prog/` — upstream clones; not ours to redistribute. Pinned by rev in the two `Cargo.toml` files. Re-clone them locally alongside this repo when working on it.
- `target/`, `.solana-test-ledger/` — build/test scratch.

## Status

- **M1**: end-to-end green. `PermissionlessCrank` round-trip lands at ~42,700 CU on a fresh no-op refresh.
- **M2**: 5+ consecutive cycles green. Discovery → plan gate → submit → three-way classify → reconciliation.
- **M3**: no-go. Program id `Perco1ator111…` still returns `null` on both mainnet-beta and devnet as of the last drift check (see `prep.md` header for the date).
- **Outstanding**: one mechanical re-pin (drop `close_q` from three call sites, bump wrapper + engine pins) — see the M2.i entry in `prep.md`.

## Attribution

The upstream projects [`percolator`](https://github.com/aeyakovenko/percolator) and [`percolator-prog`](https://github.com/aeyakovenko/percolator-prog) are © aeyakovenko under their own licenses. This repo consumes them as git dependencies at pinned revisions; nothing from those trees is redistributed here.
