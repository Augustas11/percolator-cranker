# live_shaped scenario findings

Pins: wrapper `percolator-prog` 5cb331dd, engine `percolator` 4db11a8c.
Nothing here was filed upstream.

## F1. A lapsed, never-liened backing bucket wedges refresh and liquidation in Live

**Symptom.** In Live mode, `PermissionlessCrank` on some portfolios reverts
with `EngineStale` (`0x13`) on every attempt, for as long as the market stays
Live. The cranker quarantines them with growing backoff; they are never
refreshed and never liquidated, even when they are under maintenance margin.
They come back only after `ResolveMarket` (the resolved-mode backing hint
expires the bucket).

**Trigger.** A backing bucket that is `Fresh`, reaches `expiry_slot`, and is
not referenced by any portfolio's source domains. Nothing in Live normalizes
it to `Expired`:

* the account-level lapse scan (`expire_first_lapsed_source_backing_for_account_not_atomic`,
  `repo/src/v16.rs` @4db11a8c ~9554) only looks at domains the account already
  references;
* the asset-level scan (`first_lapsed_source_backing_for_asset_at_slot`, ~9524)
  is reached only from the Resolved `CloseResolved` plan (~15321);
* `TopUpBackingBucket` rejects `expiry_slot <= now`, `WithdrawBackingBucket`
  requires `now < expiry`, `SyncBackingDomainLedger` only syncs the ledger.

Then any refresh that settles K/F so that the account gains positive PnL
sourced from that domain (a long winner → the asset's short domain `2a+1`; a
short winner → `2a`) reaches `account_source_realizable_support` (~10876) →
`validate_source_domain_ledger_current` (~9202), which returns `Stale` for a
`Fresh` bucket with `expiry_slot <= current_slot`. The refresh reverts, so
the claim is never recorded, so the next attempt fails the same way.

`Liquidate` goes through the same refresh, so a wedged account that is under
maintenance margin cannot be liquidated either.

**Minimal sequence** (one asset is enough):

1. `InitMarket`, `ConfigureAuthMark(asset 0, 1_000_000)`.
2. Two portfolios with deposits; `TradeNoCpi` makes A long and B short on asset 0.
3. `TopUpBackingBucket{domain: 1, amount: 1_000_000, expiry_slot: now + 20}`
   (backing authority = marketauth). Do not trade or crank anything that would
   lien it.
4. Wait past `expiry_slot`. `PushAuthMark(asset 0, mark +10%)`, keep pushing.
5. `PermissionlessCrank(A, hints [0])` → `custom program error: 0x13`, every
   slot, while bucket 1 stays `Fresh` with `expiry_slot < current_slot`.

**Observed in the scenario** (seed `0x5eed1a7e20261028`, 10 traders, 60
cycles; market `EqXtf6qAB3a5KYNG83g2PBnjDapZVP6SbpHaKUBbBuJp`, program
`EFPsj9tBY53XKynzS244xmbzGaZXqRmmFtoAFvDcqxfb`, local validator):

* buckets 1 and 2 topped up with `expiry_slot = 3088`, still `Fresh` at slot
  3260 and later;
* `o9Zqw3izfGq9aBiwPSf59YubH6hFWf9WVNenHjVC6Vo` (long asset 0) and
  `2YpNKhm2vrdNfGScR5HJM2mWFhsdDDCxPx7pvVk7z4uR`,
  `4UftLFKsxY2L134V4BsZQ9mMh3VtUjJX5HZTFSmvsDGo` (short asset 1): crank →
  `0x13` at slot 3260, quarantined at cycles 33 (strike 3) and later; the
  default run's report lists every such event.

**Default 200-cycle run** (40 traders, 3 assets, market
`HcuMznaPcx4FLXC3BGciAZLqhFoKqcEEVv1wy4YhHqEZ`): lapsed-but-Fresh buckets seen
in Live for domains 0, 1, 2, 3, 5 (e.g. domain 1 from slot 3961 to 4888);
69 quarantine events, 46 of them `EngineStale` on `live:pending` accounts; one
account stayed in a failure run for 81 consecutive cycles (bound 67);
`ForceCloseAbandonedAsset` on a wedged recovery-asset leg answered `0x13`
124 times.

**Native replay** of the bytes captured at the failing slot (market +
portfolio, wrapper `processor::process_instruction` with a stubbed `Clock`):

| step on the captured state | result |
|---|---|
| wrapper `PermissionlessCrank` | `Custom(19)` (`EngineStale`) |
| engine `permissionless_auto_crank_not_atomic` | `Err(Stale)` |
| engine `full_account_refresh_not_atomic` | `Err(Stale)` |
| `expire_source_backing_bucket_not_atomic(1)` then refresh (long asset 0) | `Ok`, cert `certified_liq_deficit = 3782` |
| `expire_source_backing_bucket_not_atomic(2)` then refresh (shorts asset 1) | `Ok` |
| expiring an unrelated lapsed domain first | still `Err(Stale)` |

So the only missing step is the bucket's own expiry. The `o9Zq…` row also
shows the wedged account is liquidatable.

**Suggested direction (not a patch):** let the Live refresh expire a lapsed
bucket for the asset/side it is about to attribute PnL to (the same
asset-level scan the Resolved path uses), or reject a Fresh-past-expiry
domain only after trying to normalize it.

### F1b (unconfirmed variant): `EngineLockActive` on the same accounts

The default run also quarantined 23 `live:pending` accounts with
`EngineLockActive` (`0x15`). Replaying two of them
(`Dy23NZrAAZQH3PRVDwNe3MnqUJXY8z9aL6XCvSNhsBgF`,
`9bzmbFw83cuSwdyG1Q8so63FDJ9CxxV2mti24LH8RJB2`, both with a short leg on
asset 2) gives wrapper `Custom(21)`, and engine auto-crank → `Ok(RefreshAccount)`
once the lapsed domain 5 (asset 2 short side) is expired first. Those bytes
were captured after the cycle, not at the failing slot, and the LockActive
site was not pinned down. Treat this as "probably F1 by another route".

## Observation (likely by design): lapsed backing principal is burned at CloseSlab

Unliened principal of a lapsed bucket moves to the residual pool. In the
scenario nobody holds a claim on that residual by the end, and `CloseSlab`'s
terminal step burns the claim-free retired residue (`burn_tokens_signed`,
wrapper `handle_close_slab`). In the 10-trader run the vault held 9,098,424
atoms after every owner exit and every authority withdrawal, and in the
default run 13,482,914; in both cases CloseSlab burned the balance (nothing
was swept to the market authority). Roughly 8M of that
is backing principal the backing authority topped up and let lapse. Flagged
only so the economics are deliberate; the scenario counts it as CloseSlab
success.
