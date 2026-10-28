# Operator playbook

**Reference / dry-run only; not a paid live service; M3 NO-GO.** For the
operator of a Percolator market: whoever holds its authority keys. It covers
what the staging rehearsal (`docs/STAGING.md`) runs: one SOL-PERP market
(SOL in USD, test USDC collateral, AuthMark oracle). Mainnet is out of scope.

Citations: `P` is the wrapper `percolator-prog` `src/v16_program.rs`, `E` is
the engine `percolator` `src/v16.rs`, at the pins in `scripts/pins.env`
(`5cb331dd` / `4db11a8c`). Re-check them when a pin moves.

Asset `i` has two backing/insurance **domains**: `2i` (long) and `2i+1`
(short). Every privileged instruction carries the asset's `authority_epoch`
and a strictly increasing per-lane sequence (`observation_sequence`,
`intent_id`); read both from the market account just before signing.

## 1. Roles and keys

At `InitMarket` the signer becomes `marketauth` and holds every role on every
initial asset (`P 2013-2030`). Staging keeps it that way: one key signs all of
them.

| Role | Can do | Custody once split |
|---|---|---|
| `marketauth` | `InitMarket`, fee policies, `ConfigurePermissionlessResolve`, `ResolveMarket`, `CloseSlab`, asset lifecycle | multisig |
| `oracle_authority` | `ConfigureAuthMark` (flat market only, `P 6850-6874`), `PushAuthMark` (`P 13516-13605`) | hot, on the pusher host |
| `backing_bucket_authority` | `TopUpBackingBucket`, `WithdrawBackingBucket`, `WithdrawBackingBucketEarnings`; it is their beneficiary | warm |
| `insurance_authority` | `TopUpInsurance` (`P 10028-10170`), backing fee policy, base trade fee (asset 0) | cold or multisig |
| `insurance_operator` | Live `WithdrawInsuranceAsset` (tag 57), paid to itself | warm |
| keeper | permissionless crank; needs only a fee payer | hot |

To split roles off: `UpdateAssetAuthority` for each role first, then
`UpdateAuthority` for `marketauth` **last**. `UpdateAuthority` rewrites every
asset-0 role still equal to the old `marketauth` (`P 11978-11992`).

## 2. SOL-PERP parameters

Set at `InitMarket` (`staging/src/main.rs`, `open_market`). Prices are USD
e6 per SOL; amounts are test-USDC atoms (6 decimals); sizes are 1e-6 SOL.
The margin values passed the engine's config check on a live run (it rejects
a bad config with `InvalidAccountData`).

| Field | Value | Why |
|---|---|---|
| `max_portfolio_assets` | 1 | One asset, SOL. |
| `initial_price` | 150_000_000 ($150) | Near the SOL price; must equal the first AuthMark. |
| `initial_margin_bps` | **2_000** (20%, 5x) | Conservative: half the leverage of the tested 10x set. |
| `maintenance_margin_bps` | **1_000** (10%) | Liquidation only after about a 10% adverse move from a max-leverage entry; SOL moves of that size in hours are routine, so 5x is the ceiling we rehearse. |
| `min_nonzero_mm_req` / `min_nonzero_im_req` | 1_000 / 2_000 atoms | Floors of 1 / 2 fail the engine's solvency envelope once there is a liquidation fee. |
| `max_price_move_bps_per_slot` | 5 | The effective price follows the pushed mark at most 5 bps a slot (about 12 bps a second), so one bad push cannot jump the price. |
| `max_accrual_dt_slots` / `min_funding_lifetime_slots` | 64 / 64 | Keeper cycles are seconds apart; 64 slots covers a gap without forcing catch-up steps. Lifetime must be at least `dt`. |
| `max_abs_funding_e9_per_slot` | 10_000 | Funding stays on and bounded. |
| `liquidation_fee_bps` / `cap` / `min_abs` | 50 / 1e10 / 0 | 0.5%, standard; it is part of the envelope. |
| `max_trading_fee_bps` / `trade_fee_base_bps` | 100 / **0** | Fees off by default; a 1% ceiling if turned on. |
| `maintenance_fee_per_slot` | **0** | Off. |
| `max_account_b_settlement_chunks`, `max_bankrupt_close_chunks` | 4 / 4 | Bounded close work per call. |
| `max_bankrupt_close_lifetime_slots` | 1_500 | About 10 min; a slow keeper must not trip market-wide Recovery. |
| `h_min` / `h_max` | 0 / 150 | Harmless at this pin. |
| `ConfigurePermissionlessResolve` | stale 9_000 (~1 h), force-close delay 20 | SHUTDOWN needs a non-zero delay; the stale window is long against a pusher that pushes every tick. |

**Open-interest cap.** The wrapper has **no configurable OI cap** at this pin;
the only bound is the global `MAX_OI_SIDE_Q` of 1e14 per side (1e8 SOL). The
cap is ours: our matcher is the only LP, and it never holds more than one
10 SOL clip, so open interest is at most 10 SOL a side (about $1.5k). No
other LP portfolio is created.

## 3. The bots

| Role | Bot (staging) | Cadence | Must never |
|---|---|---|---|
| Price pusher | `PushAuthMark` of `mock_sol_usd` (swap for Pyth SOL/USD on devnet; not built) | heartbeat (25 slots) or deviation (10 bps), and before each opening trade | go quiet: with permissionless resolve set, a stale mark lets anyone resolve the market (`P 4810-4814`) |
| LP matcher | LP and trader co-sign `TradeNoCpi` at mark ± spread, alternating sides | every tick | leave the book exposed during a backing roll |
| Backing manager | keeps domains 0 and 1 funded; rolls before expiry | every tick, acts when due | let a `Fresh` bucket pass expiry with unliened principal (F1) |
| Insurance | small `TopUpInsurance` | every 30 ticks | — |
| Keeper | `drip::cycle::run_one_cycle` | every tick | — |
| Lifecycle | SHUTDOWN, `ResolveMarket`, relist as a new market | once per run | — |

Note on the matcher: in `TradeNoCpi` the engine fills at the asset's
effective price; `exec_price` is used only as the fee notional (wrapper
comment F-TRADENOCPI-FEE, `P 8076-8083`). So the spread is not LP income on
this path.

## 4. Runbook

**Start.** `./scripts/staging_local.sh` (6 h). Watch `staging.log`: one line
per tick, a JSON summary every 100 ticks.

**Healthy.** Every role `ok` or `-`; `keeper=pN/iM` with no failures; the
summary's `fail` counts stay near zero; `insurance` grows by the top-ups.

**Judge a run.** Exit code 0 and a final summary line with `"summary":"final"`.
Exit 1 means one role failed `STAGING_MAX_CONSEC_FAIL` times in a row or more
than 5% of the time; the last tick lines name the error.

**Incidents.**

| Symptom | Likely cause | Action |
|---|---|---|
| `push=FAIL(EngineStale)` | `now_slot` behind the market's slot | Retry with a fresh slot; check the RPC is not lagging. |
| `push=FAIL(...)` repeatedly | pusher key, epoch or sequence wrong | Re-read `authority_epoch` and `oracle_observation`; a stale mark is the top risk. |
| `trade=FAIL(...)` | asset not accrued, or margin | The matcher cranks both sides first; persistent failures mean a stuck account. |
| `backing=FAIL(EngineLockActive)` | Live withdraw gate: negative-PnL account, exposed mark lag, or the domain would drop below full backing of its open PnL claims (`P 6007-6043`, `E` `withdraw_fresh_counterparty_backing_not_atomic`) | Keep the book flat; withdraw only principal above the open claims (the bot does both). |
| `backing=FAIL(... F1 shape)` | an unreferenced `Fresh` bucket passed expiry with principal | Upstream F1 (#243): refresh and liquidation can wedge. Live code cannot clear it; plan a resolution. |
| `lifecycle=FAIL(...)` | SHUTDOWN, resolve or relist rejected; or portfolios still open 100 ticks after resolve | Read the error; check the keeper's resolved-route cranks. |
| `keeper=FAIL(...)` | a crank failed or stayed unconfirmed | The keeper quarantines repeat failures; read the error name. |

**Backing roll, by hand.** A `Fresh` bucket only accepts top-ups at the same
expiry (`E 3400-3406`), so a roll is: before `expiry_slot`, withdraw the
unliened principal that is not backing open positive-PnL claims
(`WithdrawBackingBucket`, needs `now < expiry`, `P 10653-10658`); the bucket
goes `Empty` when nothing is left (`E 3455-3464`); then `TopUpBackingBucket`
with a new expiry, restating the current backing fee policy
(`P 10470-10485`). A remainder that backs claims lapses at expiry and is
forfeited, but the claim holders reference the domain, so the account-level
scan expires it (no F1 wedge).

**Lifecycle (rehearsed once per run).** SHUTDOWN (`UpdateAssetLifecycle`
action 3, needs a non-zero force-close delay) moves SOL to asset RECOVERY.
`ResolveMarket` (market authority, Live mode only) resolves the market. After
`force_close_delay_slots` the keeper closes every portfolio through the
resolved route and pays the owners. A resolved market never returns to Live:
relisting is a new market account. Draining the old one (backing, insurance,
`CloseSlab`) is not rehearsed.

## 5. Operator income

All protocol fees land in the engine's `insurance` balance. Each domain has a
budget; only budgeted insurance can leave during Live.

| Source | Charged | Goes to | Default |
|---|---|---|---|
| Trade fee | `max(caller fee_bps, trade_fee_base_bps)` of the fee notional, charged to **each** side from capital (`P 15285`, `E 17546-17548`) | insurance, budgeted to the asset's domains; AuthMark has no unbudgeted mark-externality part (`P 15298-15315`) | **off**: base 0, staging matcher passes 0 (`STAGING_TRADE_FEE_BPS` opts in) |
| Liquidation fee | `clamp(ceil(notional * bps / 1e4), min_abs, cap)` on a liquidation | keeper share first (`liquidation_cranker_fee_share_bps`), the rest to the domains (`P 14293-14320`) | fee 50 bps in staging; keeper share **0** (upstream default) |
| Maintenance fee | `maintenance_fee_per_slot * elapsed` per portfolio | keeper share, then asset 0's budgets (`P 6790-6814`) | **off** (0) |
| Backing fee | `fee_bps` of a new counterparty-backed lien on a risk-increasing fill (`E 47-74`) | `insurance_share_bps` to insurance, the rest to the bucket's `utilization_fee_earnings` (the backing provider) | **off** (fee policy 0) |
| Insurance top-ups | the operator's own principal | budgeted insurance | staging tops up $10 of test USDC per 30 ticks |

**Keeper income** is the cranker share of liquidation and maintenance fees,
credited as capital into the keeper's own reward portfolio, withdrawn with a
normal `Withdraw` (`P 14301-14303`). Both shares are 0 by default, and the
`drip` keeper attaches no reward portfolio unless `CRANKER_ENABLE_FEE_SHARES=1`.
There is no bounty path.

**Taking income out.**

- Live: `WithdrawInsuranceAsset` (tag 57), signed by the asset's
  `insurance_operator`, paid to itself. Only while the whole market is healthy
  (no negative-PnL account, no stress, loss-stale or Recovery), and only up
  to the domain's unspent, unreserved budget. Each one consumes an authority
  epoch (`P 6007-6043`, `P 10936-10957`, `P 11050-11053`).
- Backing provider: `WithdrawBackingBucketEarnings` up to the bucket's
  earnings, under the same Live health gate (`P 10715-10827`).
- Resolved: the remaining insurance goes to `insurance_authority` once no
  portfolios remain (`P 10991-11003`). Unbudgeted value left at `CloseSlab`
  is burned, and lapsed backing principal is forfeited (`E 3468-3497`).

The operator cannot touch trader capital or PnL.

## Not covered here

Multi-asset listing (ACTIVATE / RETIRE; asset 0 cannot be retired), the
market-wide Recovery mode (entered only by the engine), `ForceCloseAbandonedAsset`,
`CloseSlab` teardown, and Hybrid (direct Pyth) oracles: staging does not
exercise them.
