# Drip SOL-PERP matcher

Derived from aeyakovenko/percolator-match @ 60aac3a (Apache-2.0; see LICENSE).
Single asset, full fills only, exact oracle/mark execution price; no vAMM or batches.
The wrapper authenticates CPI via its delegate PDA; direct random signers cannot fill.
Init requires both LP owner and context signatures, wrapper-owned market/portfolio,
portfolio header/owner checks, and zero state. Reinitialization is forbidden.
Limits enforce positive fill/cap <= 1e15, age 1..9000 slots, deviation <= 500 bps.
LP inventory subtracts taker size using checked math; above-cap risk reduction is allowed.
SetParams may resync inventory (absolute value <= 1e15), rotate authority, or pause.

| Tag | Instruction | Data after tag (little endian) | Accounts in order |
| --- | --- | --- | --- |
| 0 | Call, 67 bytes total | req_id u64, asset u16, lp_id u64, price u64, size i128, 24 zero bytes | delegate signer, ctx writable |
| 2 | Init, 77 bytes | asset u16, fill u128, cap u128, age u64, deviation u16, authority pubkey | owner signer, ctx signer+writable, market, portfolio, executable wrapper |
| 4 | UpdateRef, 9 bytes | price u64 > 0 | price authority signer, ctx writable |
| 5 | SetParams, 92 bytes | fill u128, cap u128, age u64, deviation u16, authority pubkey, paused u8 (0/1), inventory i128 | owner signer, ctx writable |

Unknown tags, including 3, return InvalidInstructionData. Other rejections use Custom:
1 BAD_DATA, 2 BAD_ACCOUNT, 3 MISSING_SIGNATURE, 4 BAD_AUTHORITY,
5 ALREADY_INITIALIZED, 6 BAD_PARAMS, 7 PAUSED, 8 BAD_ASSET, 9 STALE,
10 MARK_DEVIATES, 11 FILL_TOO_LARGE, 12 INVENTORY_CAP, 13 BAD_PRICE, 14 BAD_CLOCK.

Context is program-owned, at least **352 bytes** (new accounts use exactly 352).
Offsets: 0..64 ABI-v3 MatcherReturn; 64 magic DRIPMTC1; 72 version u32=1;
76 wrapper, 108 market, 140 LP portfolio, 172 LP owner, 204 delegate,
236 price authority (each pubkey 32 bytes); 268 asset u16; 270 paused u8;
271 max_fill_abs u128; 287 max_inventory_abs u128; 303 inventory i128;
319 max_ref_age_slots u64; 327 max_mark_dev_bps u16; 329 ref_price_e6 u64;
337 ref_slot u64; 345..352 zero padding. All numbers are little endian.
Wrapper 5cb331dd portfolio owner is at 116 (16-byte header + 100-byte provenance),
with provenance owner at 80; LiteSVM asserts these on real wrapper-created portfolios.

Build: `cd matcher/program && cargo build-sbf` (Solana platform-tools v1.52+).
With this machine's older launcher: `PATH="$HOME/.cargo/bin:$PATH" cargo build-sbf --tools-version v1.52`.
Test: `cd matcher/host && cargo test`; CLI: `cargo build --bin matcher_ops`.
Tests require `repo-prog/target/deploy/percolator_prog.so` at wrapper pin 5cb331dd.
Deploy: `solana program deploy --url devnet matcher/program/target/deploy/drip_matcher.so`.
CLI: `matcher_ops <init-ctx|set-matcher-config|update-ref|set-params|show|test-fill>`.
Options use `--name value` or `MATCHER_NAME` env; RPC defaults to devnet and rejects mainnet URLs.
Writes use --payer KEYPAIR; all commands use --matcher PUBKEY; show uses --ctx PUBKEY.
Init: --ctx-keypair KEYPAIR --lp-owner KEYPAIR --wrapper PUBKEY --market PUBKEY
--portfolio PUBKEY --asset 0 --fill N --cap N --age N --deviation BPS --authority PUBKEY.
Set-config: --lp-owner KEYPAIR --wrapper PUBKEY --market PUBKEY --portfolio PUBKEY
--ctx PUBKEY --expiry-slots N. Set-params: --lp-owner KEYPAIR --ctx PUBKEY
--fill N --cap N --age N --deviation BPS --authority PUBKEY --paused 0 --inventory N.
Update-ref: --price-authority KEYPAIR --market PUBKEY --ctx PUBKEY;
optional --push-auth-mark --wrapper PUBKEY pushes the same effective price in that tx.
Test-fill: --wrapper PUBKEY --market PUBKEY --portfolio LP_PUBKEY --ctx PUBKEY
--size SIGNED_BASE_UNITS --deposit TOKEN_ATOMS (payer must own the test mint).

The pusher must add UpdateRef to its PushAuthMark transaction, signed by the same
oracle key. The matcher cannot read the market through the wrapper's CPI tail.
At this pin fills book at AuthMark; returned price only feeds the taker's limit check.
The wrapper rejects target/effective mismatch and fees below the 12 bps base fee.
SetMatcherConfig uses a 12 bps cap; TradeCpi charges the LP exactly the base fee.
Return flags are VALID only: backing-fee cap bits are zero, so the LP pays no backing fee.
Test-fill option --price-authority KEYPAIR pushes the reference (plus the mark with --push-auth-mark) just before
the trade (rehearsal while the pusher is paused); the trade itself is signed by the taker only.

Devnet (2026-10-29): program `8era4GFKLUu24TLr1H2Gjqt5csiYj4jhPwoxCXfEDvKE`, context
`BHsZrk4cck5xBUhFkE59G9ZJtDLvSZLEtAmpn3wNbDTK` for market `7p8DtcMH…`, LP portfolio
`GKJHRYBK…`, delegate `DdHruM8Y…`. Limits: fill 10 SOL, inventory 100 SOL, age 25 slots,
deviation 10 bps; price authority = the run's oracle key. Keys live in `~/.config/drip-devnet/`.
