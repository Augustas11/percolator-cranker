# Capital plan: conservative SOL-PERP launch

**Total: about $90k USDC and about 20 SOL**, all held in the Squads
multisig until deployed. Numbers are a starting point; shrink or grow them
with the scale rules below.

## Risk limits that size the capital

There is **no configurable OI cap** in the program. The only bound is the
global `MAX_OI_SIDE_Q = 1e14` per side (`L 31`). Because we are the only LP
and co-sign every fill (`P 8709-8715`), **our matcher is the OI cap**:

| Limit (enforced by our matcher) | Launch value |
|---|---|
| LP net inventory | ≤ $100k notional |
| Per-taker position | ≤ $10k notional |
| Per-fill size | ≤ $5k notional |
| Quote only when the pushed target equals the effective mark | yes (cuts adverse selection) |
| Reduce-only mode | switch for pause (see `LAUNCH_RUNBOOK.md`) |

Margin parameters are fixed at `InitMarket` and validated by the engine
(playbook section b). For mainnet, start stricter than staging, for example
initial 2,000 bps and maintenance 1,000 bps (5x). *To verify* that this
passes `validate_public_user_fund` before launch.

## USDC

| Bucket | Amount | Sits in | Why this size |
|---|---:|---|---|
| LP inventory (LP portfolio capital) | $35k | LP portfolio on-chain; owner key is hot | Covers 5x margin on $100k inventory ($20k) plus a 15% SOL move before the hedge catches up |
| Hedge venue collateral | $20k | external perp venue account (owner-held) | Hedges net LP inventory off-venue; *to verify* venue and its access rules (see `LEGAL_QUESTIONS.md`) |
| Insurance seed | $10k | market insurance (asset 0) via `TopUpInsurance` (playbook: `P 10028-10170`) | ~10% of max LP inventory; absorbs bankruptcy gaps |
| Backing | $15k | backing bucket, split across the long and short domains | Winners can use PnL only up to realizable backing; ~15% of max OI *to verify* from staging |
| Reserve | $10k | Squads vault, untouched | Top-ups and incidents |
| **Total** | **$90k** | | |

**Lean start option:** halve everything ($45k) with a $50k LP inventory cap.
It breaks even at the same volume per dollar of capital but stops growing
sooner.

## SOL

| Item | SOL |
|---|---:|
| Market account (1 asset) | 0.022 |
| Operator portfolios (LP, keeper, test trader) × 0.067 | 0.2 |
| Token accounts, ledgers | 0.05 |
| Bot tx + priority fees, 30 days at ~0.5 SOL/day *to verify* | 15 |
| Buffer | 5 |
| **Total** | **~20** |

The rent figures are from the R2 `docs/STAGING.md` table at the pin. No
program deploy is needed (the program is upstream's).

**Hot wallets hold at most 3 days of fees each.** The rest stays in the
multisig.

## Where it sits

| Holder | Holds | Can move it |
|---|---|---|
| Squads multisig vault | reserve USDC, SOL float, insurance authority | M-of-N owner signers |
| LP owner key (hot, matcher host) | LP portfolio capital | yes, so cap it at the LP bucket |
| `insurance_operator` key (warm) | nothing at rest; receives Live insurance withdrawals | forwards to the multisig the same day |
| `backing_bucket_authority` key (warm) | backing principal while in the bucket | withdraws or rolls |
| Keeper reward portfolio (hot) | cranker fee share | sweeps weekly to the multisig |
| Bot fee payers (hot) | ≤ 3 days of SOL | refilled from the multisig |

## Top-up and sweep rules

1. **LP capital < 70% of target:** top up from the reserve. If the reserve
   is below $5k, the matcher goes reduce-only until the owner refills.
2. **LP fee drain.** Our LP pays its fee side into insurance (`MONEY_MODEL.md`,
   fact 2). Expect LP capital to fall by `fee × volume` per day. Refill from
   insurance withdrawals.
3. **Insurance sweep.** When the Live gate is open (monitor shows
   `negative_pnl_account_count == 0`, no stress, no Recovery) and the budget
   is above the $10k seed: the `insurance_operator` withdraws the excess and
   forwards it to the multisig. Re-read the authority epoch after each
   withdrawal (playbook: `P 11050-11053`).
4. **Insurance below seed** (spent on bankruptcies): the multisig tops it up
   before the matcher raises any limit.
5. **Backing roll.** Before `expiry_slot − roll_lead`: withdraw unliened
   principal, then top up with a new expiry (playbook section d). Never let a
   bucket lapse unused (percolator #243).
6. **SOL.** Refill a bot when it is below 1 day of fees. Page when it is
   below 12 hours.
7. **Scale up** only after 7 days with no incident and net P&L ≥ 0: double
   the LP inventory cap and scale all buckets with it. Scale down by half
   after any loss event above $5k.
