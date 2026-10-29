# Prior art (checked 2026-10-28, read-only)

**Upstream** (`aeyakovenko/percolator`, `percolator-prog`, Apache-2.0): Rust only. No TS
client, IDL, SDK or UI. The only client-side reference is `examples/dump_layout.rs`,
which is the source for the real client's account layouts later on. Contributors are
`mizuki0x`, plus bot accounts. None of mizuki0x's public repos is a Percolator client or UI.

**Percolator ecosystem:**
- `dcccrypto/percolator-launch` (Apache-2.0, active): a Next.js + pnpm monorepo with a full
  trading UI, keeper and indexer. It runs on dcccrypto's own wrapper fork (v12/v17 lineage),
  not our v16 pin. Its size goes against this skeleton's keep-it-small rule.
  - **Used as a pattern:** separating the SDK from the UI, a health-ratio display, and a
    devnet-only label.
- `dcccrypto/percolator-sdk` (Apache-2.0, TS): encodes v12.19 wire format, so it is not
  usable against v16. It is the first reference for roadmap item 5 (port its structure, not
  its layouts).
- `dcccrypto/percolator-mobile`: React Native. Not applicable.
- `psyto/percolator-apps`, `co-numina/slab-terminal`: no licence, so we read them for ideas
  and copy nothing.

**General perps UIs:**
- Rejected `mango-v4-ui` (AGPL-3.0: we copy nothing) and the Drift protocol (archived).
- **Later, not now:**
  - `anza-xyz/wallet-adapter` (Apache-2.0) is the standard for the real Connect button.
  - `tradingview/lightweight-charts` (Apache-2.0) if we ever need a chart.

**Real client (`src/chain/`, 2026-10-29):** written fresh against upstream
`percolator-prog` `5cb331dd`. `dcccrypto/percolator-sdk` was the reference for its
shape: little-endian helpers, vault and matcher PDA helpers. No code was copied, so
no Apache-2.0 header is needed. The offsets and fixtures come from the pinned Rust
crate (`client/src/bin/ts_fixtures.rs`, the same `offset_of!` source as upstream
`examples/dump_layout.rs`). Upstream's `dump_layout.rs` does not compile at
`5cb331dd`: it names the removed field
`_reserved_insurance_withdraw_deposit_remaining`. No wallet-adapter library is
used: Phantom and Solflare are reached through their injected providers.

**What this skeleton does:**
- Plain React with a typed `MarketClient` and a mock behind it.
- The data shapes use v16 engine terms: portfolio, leg, mark, funding, health.
- No code was copied from any repo above.
