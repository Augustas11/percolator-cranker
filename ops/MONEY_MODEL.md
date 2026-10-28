# Money model: SOL-PERP on the official Percolator program

**Bottom line (default assumptions):**

| | We are the only LP | Outside LP + rebate |
|---|---:|---:|
| Break-even | **$330k/day** | **$240k/day** |
| Net per month at $100k/day | −$2k | −$1.6k |
| Net per month at $1M/day | +$6k | +$9k |
| Net per month at $10M/day | +$89k | +$112k |

- Most of the income lands in **insurance**, which we can withdraw while Live
  only when the whole market is healthy. Plan the cash flow around that.
- **Upstream says "not audited, do not use with real funds"** (both
  READMEs). No real capital goes in until upstream ships an official,
  non-bounty program ID and the audit status is clear (`LAUNCH_RUNBOOK.md`,
  gate 0).

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

   (The gate is skipped only for a shut-down, matured, empty asset ≥ 1,
   `P 6013`; that never applies to asset 0.)

   The amount is capped by unspent budget, by `insurance − reserved` and by
   the vault (`P 6545-6561`). Each withdrawal consumes an authority epoch
   (playbook: `P 11050-11053`).

## Revenue lines

| Line | Mechanism (source) | Who gets it | Withdrawable while Live? | Model default |
|---|---|---|---|---|
| Trade fee, taker side | fact 2 above | insurance budget, asset 0 | only through the gate (fact 4) | 5 bps × volume |
| Trade fee, LP side | same; our own money | insurance budget | same | not income (a transfer) |
| Liquidation fee | `clamp(ceil(notional·bps/1e4), min_abs, cap)` (playbook: `E 20852-20884`). The keeper share is `liquidation_cranker_fee_share_bps` of the retained fee, credited as **capital** to the cranker's portfolio, only when the fee is reclaimable and the crank passes a cranker portfolio (`P 14280-14284`). The rest goes to budgets (`P 14275-14320`). The default share is 0 (`P 7713`) | keeper portfolio + insurance | **keeper share: yes**, via normal `Withdraw` (`P 7896`). The rest: gated | 1% of volume liquidated *to verify* × 50 bps |
| Maintenance fee | `maintenance_fee_per_slot` per portfolio per slot; Live only (`P 6816-6830`). Cranker share via `maintenance_cranker_fee_share_bps` (`P 11630-11690`; default 0 at `P 7714`) | asset 0 budget + keeper | keeper share yes; rest gated | 1 atom/slot ≈ $0.22/day per portfolio; 20 portfolios per $100k/day *to verify* |
| Backing fee | Charged to an account whose counterparty-backed lien grows (`P 14987-15037`). The amount is `fee_bps` of the lien delta, split `insurance_share_bps` to insurance and the rest to provider earnings (`E 47-74`, `E 10165-10232`). The payer must have PnL ≥ 0 (`E 10183`) | provider earnings + insurance | provider earnings: `WithdrawBackingBucketEarnings`, under the same health gate (playbook: `P 10761-10790`) | 5% of volume becomes new backed lien *to verify* × 10 bps |
| Funding | A premium of the pushed mark over the lagged effective price, capped by `max_abs_funding_e9_per_slot` (`P 5714-5732`, `P 15502-15537`). It is **not** OI-imbalance funding | LP vs takers, zero-sum | n/a | 0 (≈0 in steady state) *to verify* |
| Listing fee (permissionless asset create) | `marketauth` sets it with `UpdateMarketInitFeePolicy` (tag 59). Fee 0 means only the authority may list (`P 12447-12458`). It doubles every 32 slots of asset index (`P 5543-5557`). The whole fee is deposited to asset 0 insurance (`P 12588`, `P 12610`; wrapper README "Assets 1..N") | asset 0 insurance budget | only through the gate (fact 4), and the gate is **market-wide**, so the listed asset's traders can hold it shut | 0: listing is closed on Day 0. Example: 2 listings/month at $1k each is ≈ $66/day |
| Portfolio rent | closed portfolios' rent moves to the market account and returns to `marketauth` at `CloseSlab` (playbook: `P 14818-14838`, `P 11330-11342`) | marketauth | at close only | ignored |

**What we cannot withdraw:**
- trader capital or PnL;
- unbudgeted insurance: the mark-externality part (not applicable under
  AuthMark), non-reclaimable liquidation fees (`P 14312-14314`), and budget
  already spent on bankruptcies;
- lapsed backing principal;
- whatever is left unbudgeted at `CloseSlab`, which is burned (`P 11262-11268`; playbook:
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

### Scenario A: we are the only LP

| USD per day | $100k/d | $1M/d | $10M/d |
|---|---:|---:|---:|
| Trade fees (taker side) | 50 | 500 | 5,000 |
| Liquidation fees | 5 | 50 | 500 |
| Backing fees | 5 | 50 | 500 |
| Maintenance fees | 4 | 43 | 432 |
| LP adverse selection + hedge | −33 | −335 | −3,350 |
| Fixed (SOL, RPC, servers, $90k capital) | −100 | −100 | −100 |
| **Net per day** | **−70** | **208** | **2,982** |
| **Net per 30 days** | **−2,090** | **6,232** | **89,446** |
| Into insurance (both fee sides) | 100 | 1,000 | 10,000 |

- Net variable margin: **2.65 bps** of volume.
- Break-even: **$326k/day**.
- The $10M column assumes capital scales with volume (see `CAPITAL_PLAN.md`
  top-up rules). The opportunity cost of that extra capital is small next to
  the fees.

### Scenario B: an outside professional LP, paid a rebate

Assume professional market makers eventually take the LP role. At this pin
an LP earns **no spread** (fact 1). It also pays its own fee side into *our*
insurance (fact 2). So the only way to pay an outside LP is an off-chain
rebate. The rebate must:
- return its fee side (5 bps);
- cover its adverse selection and hedge (1.6 bps *to verify*; better than
  ours);
- leave it a margin (1 bp *to verify*).

That is **7.6 bps, about 76% of fee income**. We drop the LP risk and $55k
of LP and hedge capital.

| USD per day | $100k/d | $1M/d | $10M/d |
|---|---:|---:|---:|
| Trade fees (both sides, outside money) | 100 | 1,000 | 10,000 |
| Liquidation + backing + maintenance | 14 | 143 | 1,432 |
| Rebate to outside LP | −76 | −760 | −7,600 |
| Fixed (SOL, RPC, servers, $35k capital) | −93 | −93 | −93 |
| **Net per day** | **−55** | **290** | **3,739** |
| **Net per 30 days** | **−1,639** | **8,708** | **112,172** |

- Break-even: **$243k/day**.
- **The cash-flow catch.** The rebate is paid in cash, but the fee income
  sits in insurance behind the Live gate. Hold a cash float to cover the
  rebates between withdrawal windows, or settle rebates only at withdrawal.
- **What lasts.** The trade fee mostly flows to whoever carries the
  inventory risk. What stays with us is:
  - the operator role (liquidation, backing and maintenance fees, the
    keeper share);
  - the order flow our frontend brings, which is what an outside LP pays
    the rebate for.

  Start as the only LP (scenario A) to prove the market. Move to B once a
  professional LP wants the flow.

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
- Outside-LP costs and required margin: from real conversations with market
  makers.

## Upstream status (gate on all of this)

- Both upstream READMEs say the code is an educational research project, is
  not audited, and should not be used with real funds:
  - wrapper `README.md` lines 3–5 at `5cb331dd`;
  - engine `README.md` lines 3–4 at `4db11a8c`.
- Upstream's mainnet deployments so far are bounty programs. The CLI repo
  has `scripts/deploy-bounty5-v16.ts` (R2 reuse survey). The re-pin session
  reports rounds 2, 4 and 5 at 1.5–5 SOL each, rewarded by draining. Those
  round sizes are *to verify*.
- Treat every number above as a plan for **after** an official, non-bounty
  program ID with a clear audit status.
