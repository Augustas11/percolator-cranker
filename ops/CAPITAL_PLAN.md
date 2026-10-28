# Capital plan: conservative SOL-PERP launch

**Total: about $100k USDC and about 20 SOL**, all held in the Squads
multisig until deployed. Numbers are a starting point; shrink or grow them
with the scale rules below.

## Risk limits that size the capital

There is **no configurable OI cap** in the program. The only bound is the
global `MAX_OI_SIDE_Q = 1e14` per side (`L 31`, checked at
`P 14707-14724`).

**Our matcher limits only the liquidity we provide.** Any two portfolio
owners can trade directly with each other: `TradeNoCpi` needs just their two
signatures (`P 8709-8715`), and opening a portfolio needs only its owner
(`P 7761-7775`). On Day 0 we are the only LP, so in practice most flow goes
through us. That is a practical limit, not a protocol one.

| Limit | Enforced by | Launch value |
|---|---|---|
| LP net inventory | our matcher | ≤ $100k notional |
| **Gross OI per side** (all accounts) | monitor alert; our matcher stops opening risk above it | ≤ $250k notional |
| Per-taker position | our matcher | ≤ $10k notional |
| Per-fill size | our matcher | ≤ $5k notional |
| Quote only when the pushed target equals the effective mark | our matcher | yes (cuts adverse selection) |
| Market-wide brake | `DRAIN_ONLY` on asset 0 (multisig, see `LAUNCH_RUNBOOK.md`) | blocks every risk-increasing trade, from anyone |

Gross OI matters because alternating long and short takers can grow OI on
both sides while our net inventory stays near zero. Backing and insurance
are sized against **gross OI**, not against our inventory.

Margin parameters are fixed at `InitMarket` and validated by the engine
(playbook section b). For mainnet, start stricter than staging: for example
initial 2,000 bps and maintenance 1,000 bps (5x). *To verify* that this
passes `validate_public_user_fund` before launch.

## USDC

| Bucket | Amount | Sits in | Why this size |
|---|---:|---|---|
| LP inventory (LP portfolio capital) | $35k | LP portfolio on-chain; owner key is hot | 5x margin on $100k of inventory ($20k), plus a 15% SOL move before the hedge catches up |
| Hedge venue collateral | $20k | external perp venue account (owner-held) | Hedges net LP inventory off-venue; *to verify* the venue and its access rules (`LEGAL_QUESTIONS.md`) |
| Insurance seed | $10k | market insurance (asset 0), via `TopUpInsurance` (playbook: `P 10028-10170`) | 4% of max gross OI per side; absorbs bankruptcy gaps |
| Backing | $15k | backing bucket, split across the long and short domains | 6% of max gross OI per side, *to verify* from staging. Winners can use PnL only up to realizable backing |
| Cash reserve | $20k | Squads vault, untouched | Covers 14 shut-gate days at $1M/day ($7k of LP fee drain; `money_model.py --lock-days 14`), plus top-ups and incidents |
| **Total** | **$100k** | | |

**Lean start option:** halve everything ($50k) and halve the limits. The
fixed costs don't halve, so break-even drops only slightly, to about
$407k/day (`--capital-usd 50000`). Lean saves capital, not volume.

## SOL

| Item | SOL |
|---|---:|
| Market account (1 asset) | 0.022 |
| Operator portfolios (LP, keeper, test trader) × 0.067 | 0.2 |
| Token accounts, ledgers | 0.05 |
| Bot tx + priority fees, 30 days at ~0.5 SOL/day *to verify* | 15 |
| Buffer | 5 |
| **Total** | **~20** |

- The rent figures are from the R2 `docs/STAGING.md` table at the pin.
- No program deploy is needed: the program is upstream's.
- **Hot wallets hold at most 3 days of fees each.** The rest stays in the
  multisig.

## Where it sits

| Holder | Holds | Can move it |
|---|---|---|
| Squads multisig vault | cash reserve, SOL float, insurance authority | M-of-N owner signers |
| LP owner key (hot, matcher host) | LP portfolio capital | only when the LP is **flat**: `Withdraw` refuses any open leg (`E 18954-18955`). LP capital is locked while we hold inventory, so cap it at the LP bucket |
| `insurance_operator` key (warm) | nothing at rest; receives Live insurance withdrawals | forwards to the multisig the same day |
| `backing_bucket_authority` key (warm) | backing principal while in the bucket | withdraws or rolls |
| Keeper reward portfolio (hot) | cranker fee share | sweeps weekly to the multisig |
| Bot fee payers (hot) | ≤ 3 days of SOL | refilled from the multisig |

## Top-up and sweep rules

1. **LP capital < 70% of target:** top up from the reserve. If the reserve
   is below $10k, the matcher goes reduce-only until the owner refills.
2. **LP fee drain.** Our LP pays its fee side into insurance
   (`MONEY_MODEL.md`, fact 2), so LP capital falls by `fee × volume` per day.
   Refill it from the **cash reserve**. Repay the reserve from insurance only
   when the gate opens. Quoting limits follow available cash, not accrued
   profit.
3. **Insurance sweep.** When the Live gate is open (the monitor shows
   `negative_pnl_account_count == 0`, no stress, no Recovery) and the budget
   is above the $10k seed: the `insurance_operator` withdraws the excess and
   forwards it to the multisig. Re-read the authority epoch after each
   withdrawal (playbook: `P 11050-11053`).
4. **Insurance below seed** (spent on bankruptcies): the multisig tops it up
   before the matcher raises any limit.
5. **Backing roll. It can fail, so start early.** A Fresh bucket only takes
   top-ups at its *existing* expiry (`E 3400-3406`). A new expiry needs the
   bucket to be Empty (or Expired) first, which needs:
   - all principal withdrawn;
   - no valid liens (`E 3447-3464`);
   - the Live health gate open (`P 10625-10635`);
   - the domain still fully backed afterwards (`E 9394-9420`).

   Rules:
   - Start the roll at `expiry_slot − 3 days`.
   - If liens remain at `expiry − 1 day`, put the matcher on reduce-only for
     that side, so liens can unwind.
   - If the bucket still can't be emptied by `expiry − 6 hours`: page the
     owner. The liened part becomes **Impaired** at expiry (`E 3476-3501`).
   - **Never let a bucket lapse unused** (percolator #243): withdraw all
     unliened principal before expiry, even when a roll is impossible.
   - **If the gate stays shut through expiry, the unliened principal is
     lost too.** Expiry zeroes fresh unliened principal (`E 3476-3493`), and
     the withdrawal needs the Live gate and `now < expiry`
     (`P 10625-10657`). Resolution does not rescue it: a resolved withdrawal
     also needs every portfolio closed and `c_tot == 0` first
     (`P 10627-10631`). So treat backing principal as **at risk for the
     length of a gate lock**.
   - Keep expiries **longer than the assumed lock** (`--lock-days`, 14 days)
     plus the roll lead. If the gate has been shut for most of
     `expiry − lock-days`, begin an orderly wind-down early.
6. **SOL.** Refill a bot when it is below 1 day of fees. Page when it is
   below 12 hours.
7. **Scale up** only after 7 days with no incident and net P&L ≥ 0, and
   only if the gate opened at least once in that week: double the limits and
   scale every bucket with them. Scale down by half after any loss event
   above $5k.
