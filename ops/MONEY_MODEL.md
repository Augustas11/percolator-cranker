# Money model: SOL-PERP on the official Percolator program

**Bottom line (default assumptions):**
- Break-even is about **$330k/day** of taker volume.
- The model makes about **$6k/month at $1M/day** and **$90k/month at $10M/day**.
- It loses about **$2k/month at $100k/day**.
- Most of the income lands in **insurance**, which we can withdraw while Live
  only when the whole market is healthy. Plan the cash flow around that.

Run it: `python3 ops/money_model.py` (any `--flag` overrides an assumption).

Citations are to source at the pins in `scripts/pins.env`:
- `P` = wrapper `src/v16_program.rs` @ `5cb331dd`;
- `E` = engine `src/v16.rs` @ `4db11a8c`;
- `L` = engine `src/lib.rs` @ `4db11a8c`.

Most mechanisms are also mapped in `docs/OPERATOR_PLAYBOOK.md` on branch
`claude/r2-operator-docs`. Re-check every line number after a re-pin.

## Setup assumed

- One market, one asset (asset 0 = SOL-PERP), USDC collateral, AuthMark
  oracle pushed by our pusher from Pyth.
- We hold every role: `marketauth`, insurance, backing provider, the **only
  LP**, and the keeper.
- Volume = daily taker notional. Every taker fill is against our LP.

## Four facts that drive the money

1. **There is no LP spread.** Fills execute at the engine's effective mark.
   The `exec_price` is used only to size the fee (`P 8076-8083`). Under
   AuthMark the accepted price is the effective price (`P 15237-15256`).
   TradeCpi takes the same path (`P 9454-9471`). So "spread" can only be
   charged as a fee.
2. **The trade fee hits both sides.** The rate is
   `max(caller fee_bps, trade_fee_base_bps)` (`P 15285`). The engine charges
   that rate to the long *and* the short (`E 17657-17660`). The charge moves
   capital into `insurance` (`E 18194-18230`). It is skipped if the payer's
   PnL is negative and capped at its capital (`E 18198-18202`). Our LP pays
   the same fee as the taker. That is a transfer from our LP capital into our
   insurance budget, not income.
3. **Fees land in insurance budgets, not in a wallet.** Trade fees are
   credited to the asset's long/short budgets (`P 6595-6630`). There is no
   redirect on asset 0 (`P 6605-6606`), and no mark-externality carve-out
   under AuthMark (`P 15298-15304`). Maintenance fees go to asset 0's budget
   (`P 6790-6814`).
4. **Live withdrawal is gated.** `WithdrawInsuranceAsset` while Live needs
   the `insurance_operator` signature (`P 10936-10950`) and passes
   `live_domain_withdraw_health_or_shutdown_view` (`P 6007-6043`). The
   market must:
   - not be stale-matured;
   - have `negative_pnl_account_count == 0` market-wide;
   - have no threshold stress, no loss-stale and no Recovery;
   - have no pending loss barrier and no exposed mark lag on the asset.

   The amount is capped by unspent budget, by `insurance − reserved` and by
   the vault (`P 6545-6561`). Each withdrawal consumes an authority epoch
   (playbook: `P 11050-11053`).

## Revenue lines

| Line | Mechanism (source) | Who gets it | Withdrawable while Live? | Model default |
|---|---|---|---|---|
| Trade fee, taker side | fact 2 above | insurance budget, asset 0 | only through the gate (fact 4) | 5 bps × volume |
| Trade fee, LP side | same; our own money | insurance budget | same | not income (a transfer) |
| Liquidation fee | `clamp(ceil(notional·bps/1e4), min_abs, cap)` (playbook: `E 20852-20890`). The keeper share is `liquidation_cranker_fee_share_bps` of the retained fee, credited as **capital** to the cranker's portfolio. The rest goes to budgets (`P 14275-14320`). The default share is 0 (`P 7713`) | keeper portfolio + insurance | **keeper share: yes**, via normal `Withdraw` (`P 7896`). The rest: gated | 1% of volume liquidated *to verify* × 50 bps |
| Maintenance fee | `maintenance_fee_per_slot` per portfolio per slot; Live only (`P 6816-6830`). Cranker share via `maintenance_cranker_fee_share_bps` (`P 11630-11690`; default 0 at `P 7714`) | asset 0 budget + keeper | keeper share yes; rest gated | 1 atom/slot ≈ $0.22/day per portfolio; 20 portfolios per $100k/day *to verify* |
| Backing fee | Charged to an account whose counterparty-backed lien grows (`P 14987-15037`). The amount is `fee_bps` of the lien delta, split `insurance_share_bps` to insurance and the rest to provider earnings (`E 47-74`, `E 10165-10232`). The payer must have PnL ≥ 0 (`E 10183`) | provider earnings + insurance | provider earnings: `WithdrawBackingBucketEarnings`, under the same health gate (playbook: `P 10761-10790`) | 5% of volume becomes new backed lien *to verify* × 10 bps |
| Funding | A premium of the pushed mark over the lagged effective price, capped by `max_abs_funding_e9_per_slot` (`P 5714-5732`, `P 15502-15537`). It is **not** OI-imbalance funding | LP vs takers, zero-sum | n/a | 0 (≈0 in steady state) *to verify* |
| Market-init fee | only when others append assets to *our* market (playbook: `P 12447-12458`) | asset 0 budget | gated | 0 (off at launch) |
| Portfolio rent | closed portfolios' rent moves to the market account and returns to `marketauth` at `CloseSlab` (playbook: `P 14818-14838`, `P 11330-11342`) | marketauth | at close only | ignored |

**What we cannot withdraw:**
- trader capital or PnL;
- unbudgeted insurance: the mark-externality part (not applicable under
  AuthMark), non-reclaimable liquidation fees (`P 14312-14314`), and budget
  already spent on bankruptcies;
- lapsed backing principal;
- whatever is left unbudgeted at `CloseSlab`, which is burned (playbook:
  `E 11363-11372`, `P 11221-11268`).

After resolution, budgets go to `insurance_authority` with no gate (playbook:
`P 10991-11003`).

## Cost lines

| Line | Model default | Note |
|---|---|---|
| LP adverse selection | 2 bps × volume *to verify* | Takers who see Pyth before our push, or who trade during mark lag, win against us. Mitigation: the LP co-signs every `TradeNoCpi` (`P 8709-8715`), so the matcher refuses fills while target ≠ effective. |
| LP hedge | 30% of volume hedged × 4.5 bps *to verify* | Net inventory is hedged on an external perp venue, at its taker fee. |
| Insurance capital | in capital cost | Seed. Fees add to it, but stay locked behind the gate. |
| Backing capital | in capital cost | Winners can use PnL only up to realizable backing (engine `spec.md`), so backing is a launch requirement, not an add-on. Expiry forfeits unliened principal (playbook: `E 3468-3497`); roll before expiry. |
| LP inventory capital | in capital cost | Margin plus a buffer for SOL moves. See `CAPITAL_PLAN.md`. |
| Capital opportunity cost | $90k × 5% APY | The money could sit in a USDC yield instead. |
| SOL tx + priority fees | 0.5 SOL/day × $150 *to verify* | Cranker plus pusher. The staging budget was ~1 SOL/day worst case for the cranker (`docs/STAGING.md` on the R2 branch). On mainnet, priority fees dominate; measure them. |
| SOL rent | one-time, ~0.5 SOL | Market (1 asset) 0.022 SOL, each portfolio 0.067 SOL, ATAs 0.002 SOL. No program deploy: the official program is upstream's. |
| RPC | $300/month *to verify* | A paid plan with staked/priority send. |
| Servers | $100/month | Two small VMs (primary + standby). |
| Legal | one-time, outside the daily model | See `LEGAL_QUESTIONS.md`. |

## Output (defaults; `python3 ops/money_model.py`)

| USD per day | $100k/d | $1M/d | $10M/d |
|---|---:|---:|---:|
| Trade fees (taker side) | 50 | 500 | 5,000 |
| Liquidation fees | 5 | 50 | 500 |
| Backing fees | 5 | 50 | 500 |
| Maintenance fees | 4 | 43 | 432 |
| LP adverse selection | −20 | −200 | −2,000 |
| LP hedge fees | −14 | −135 | −1,350 |
| Fixed (SOL, RPC, servers, capital) | −100 | −100 | −100 |
| **Net per day** | **−70** | **208** | **2,982** |
| **Net per 30 days** | **−2,090** | **6,232** | **89,446** |
| Into insurance (both fee sides) | 100 | 1,000 | 10,000 |

- Net variable margin: **2.65 bps** of volume.
- Break-even: **$326k/day**.
- The $10M column assumes capital scales with volume (see `CAPITAL_PLAN.md`
  top-up rules). The opportunity cost of that extra capital is small next to
  the fees.

## What moves the answer most

1. **Fee rate.** Each bp is $100/day per $1M. Takers see fee only (no
   spread), so 5 bps all-in is competitive with large venues' ~4.5 bps taker
   fee plus their spread. *To verify against live competitors on the day.*
2. **Adverse selection.** If it is 5 bps instead of 2, the margin goes
   negative. This is the number the PoC beta must measure.
3. **Gate uptime.** If `negative_pnl_account_count` is rarely 0, fee income
   is P&L but not cash until Resolved. Meanwhile our LP keeps paying its fee
   side into insurance, draining LP capital by `fee × volume` per day (≈$500
   per day at $1M). *To verify:* the share of slots where the gate is open, in
   the staging run.
4. **SOL fees.** These are the largest fixed cost. Priority-fee policy
   decides break-even at low volume.

## To verify before committing capital

- Liquidation share, backed-lien share and portfolios per volume: measure in
  R2 staging.
- Adverse selection and hedge ratio: measure in the PoC beta, with real
  testers.
- How often the Live insurance gate is open: R2 staging, via the monitor.
- Mainnet SOL per day for the cranker and pusher at our cadence.
- Whether the official program's deployment changes any default.
- Competitor fee levels on launch day.
