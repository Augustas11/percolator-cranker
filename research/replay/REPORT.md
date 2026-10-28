# SOL-PERP replay: does our market make money?

**Verdict.** At the money model's **5 bps fee it does not**. Real flow plus a latency arbitrageur
loses money at every pusher lag we can actually reach: −0.9 bps of volume at 400 ms and −10.5 bps at 1 s,
hedged. **It does make money at 12 bps or more, hedged, if the pusher keeps lag at 1 s or less.**
- At 12 bps, 1 s lag, tight hedge and $1M/day: the edge is **+7.8 bps**, about **+$675/day after fixed
  costs**. 97% of days are positive, the worst day is −$2.6k and the max drawdown is $3k.
- **Break-even is $110k–$132k/day** at 12 bps, against **$437k/day** in the model.
- 8 bps is marginal: +4.2 bps at 400 ms, and break-even at 1 s.
- **Measured adverse selection is 2.2–2.4 bps** for real flow at 400 ms–2 s lag, so the model's 2 bps
  was about right.
- **The model missed the latency arb.** It costs another 1–34 bps, depending on fee and lag.

**The risks:**
- **Lag and fee together are the business.** Fees under 8 bps with lag of 1 s or more lose money
  quickly, and the stale-mark arb compounds the loss.
- **Volume is the unknown.** These results hold flow fixed while the fee changes, and nothing here
  shows we can attract $1M/day.
- **Profit is locked.** Fees land in insurance behind the Live gate.

For comparison, Jupiter/JLP keeps about 17–23 bps. Our ~8–9 bps hedged edge at 12 bps is roughly half
that, with no directional bet.

All numbers come from 90 days of real flow (2026-07-30 to 2026-10-27, which includes the Aug 19–28
volatility stretch), scaled to our volume.

## Headline: toxic flow (real flow + latency arb), $1M/day

Hedged (tight, net ≤ 1% of daily volume) is the **edge**. Unhedged adds the LP's directional P&L.
SOL rose 37% in the window and the capped LP sat long, so the unhedged column is luck, not edge.

| Fee | Lag | Hedged edge bps | Net/day | SD/day | Max DD | Break-even/day | Unhedged bps | Unhedged SD/day |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 5 | 400 ms | −0.88 | −$190 | $577 | $20k | none | −0.60 | $2,353 |
| 5 | 1 s | −10.53 | −$1,155 | $1,759 | $104k | none | −7.27 | $2,732 |
| 8 | 400 ms | 4.20 | $318 | $368 | $3k | $244k | 4.47 | $2,140 |
| 8 | 1 s | 0.46 | −$56 | $719 | $16k | $2.5M | 0.87 | $1,982 |
| **12** | **400 ms** | **9.27** | **$826** | $445 | $0.5k | **$110k** | 11.23 | $2,194 |
| **12** | **1 s** | **7.77** | **$675** | $531 | $3k | **$132k** | 9.26 | $1,984 |
| 15 | 400 ms | 12.51 | $1,149 | $592 | $0 | $82k | 14.76 | $2,288 |
| 15 | 1 s | 11.70 | $1,068 | $587 | $0.9k | $87k | 12.15 | $2,300 |
| 20 | 400 ms | 17.57 | $1,655 | $788 | $0 | $58k | 19.98 | $2,479 |
| 20 | 1 s | 17.24 | $1,622 | $803 | $0 | $59k | 18.34 | $2,560 |

- **Net/day** is after the model's fixed costs ($101.85/day).
- **Break-even** is trading edge against fixed costs. With the model's other lines added (liquidation,
  backing and maintenance fees, minus bad debt: +0.68 bps, all unverified), it is about 5–10% lower.
  See the grids.
- **Fees of 15–20 bps look best only because flow is held fixed.** Real takers would leave (see
  "Limits"). We don't recommend them on this evidence.
- **Lag 0 is not reachable.** Our oracle stand-in is the Binance spot print, and Pyth itself trails it.
  So 400 ms is the best case and 1 s is a realistic pusher.

## Best settings

| | Fee | Lag | Hedge | Why |
|---|---:|---|---|---|
| **Recommended start** | **12 bps** | ≤ 1 s | tight (net ≤ 1% of daily volume) | +7.8 to +9.3 bps edge, break-even ≈ $110–130k/day, max DD ≤ $3k at $1M/day. 12 bps per trade is close to Jupiter's 6 bps base plus borrow plus impact for a one-day hold |
| Fallback if 12 bps kills volume | 8 bps | **≤ 400 ms** | tight | +4.2 bps; needs a fast pusher. Break-even ≈ $244k/day |
| Do not run | ≤ 5 bps | any reachable lag | any | Negative edge under the arb overlay |

## Adverse selection (measured, replaces the model's 2 bps)

This is the LP markout on real flow, against the oracle at each horizon. The units are bps of
volume, and a positive number means the LP loses.

| Lag | at fill | 1 s | 10 s | 60 s | 5 min |
|---:|---:|---:|---:|---:|---:|
| 0 | 0.00 | 1.32 | 1.48 | 1.62 | 1.43 |
| 400 ms | 0.63 | 1.95 | 2.11 | **2.24** | 2.05 |
| 1 s | 0.67 | 1.99 | 2.15 | **2.28** | 2.10 |
| 2 s | 0.75 | 2.08 | 2.23 | **2.37** | 2.18 |

- **Cross-check.** With the arb nearly absent (20 bps, 400 ms, hedged), the replay's realised
  inventory loss is 2.27 bps, the same as the 60 s markout.
- **By trade size:**
  - trades under $10k, 14% of volume: 0.1–1.0 bps;
  - $10k–$100k: 1.4–2.0 bps;
  - over $100k: 2.5–4.8 bps.

  So retail-sized flow is close to harmless (see the appendix).
- **The latency arb is the larger cost.** It is loss to the LP at fill, before the inventory limit, in
  bps of base volume:

  | | 400 ms | 1 s | 2 s |
  |---|---:|---:|---:|
  | 5 bps | 5.6 | 17.9 | 46.1 |
  | 8 bps | 2.8 | 8.8 | 22.8 |
  | 12 bps | 1.2 | 3.9 | 10.3 |
  | 20 bps | 0.4 | 1.2 | 3.3 |

  The arb also pays our fee, so its net cost is about half of these numbers. A higher fee is the
  cheapest defence: the arb needs |oracle − mark| > fee + its own cost.
- **Hedge.** Hyperliquid taker fee 4.5 bps plus 0.5–0.7 bps of book slippage at $1k–$100k. Tight
  hedging costs 0.2–1.9 bps of volume when the fee is ≥ 8 bps.
- **Funding** on the hedge leg: about +0.1 bps (Hyperliquid hourly rates). On our venue it is about 0;
  see `ops/MONEY_MODEL.md`.

## Worst days (12 bps, 1 s, tight hedge, $1M/day)

| Day | Net | Fees | Inventory P&L | Hedge cost | SOL open→close | Intraday range | Ann. vol |
|---|---:|---:|---:|---:|---:|---:|---:|
| 2026-08-22 | −$2,561 | $4,833 | −$6,656 | $665 | +0.1% | 13.5% | 177% |
| 2026-08-23 | −$310 | $2,104 | −$2,156 | $184 | +1.7% | 5.7% | 91% |
| 2026-08-25 | −$103 | $3,198 | −$2,902 | $322 | −2.4% | 6.9% | 93% |
| 2026-08-01 | +$120 | $634 | −$356 | $54 | −1.3% | 3.7% | 32% |
| 2026-08-16 | +$122 | $414 | −$180 | $21 | −1.0% | 2.0% | 24% |

**Why:**
- The bad days are the volatility spikes. Aug 22 had a 13.5% range at 177% annualised vol. The mark is
  stale for longer, so the arb fires on most slots and the hedge lags behind it.
- Fees cover most of it, and only three days lose money.
- **At 5 bps the same days are 5× worse** (−$13.1k on Aug 22), because the arb clears a 5 + 2 bps bar
  far more often.
- Max drawdown is $3k at 12 bps and $104k at 5 bps (1 s, tight).

## Benchmark: Jupiter Perps / JLP

The source is the Chaos Labs weekly risk reports: 67 weeks, Aug 2024 – Mar 2026, about $315B of volume.
The parent session gathered them; see "Sources".

| | bps of volume | Notes |
|---|---:|---|
| JLP pool + protocol take, 67 weeks | ~24 (≈19.5 to JLP) | 18 bps fees plus ~6 bps of trader losses |
| JLP, Q3 2026 / Q2 2026 | 20.7 / 26.5 (17.3 / 22.9 to JLP) | |
| JLP trader-loss part, by half-year | −12.7 to +1.2 | Directional: OI is ~90% long and JLP holds the coins |
| **Us, 12 bps, 1 s, hedged** | **+7.8** | No directional bet; trading edge only |
| Us, 8 bps, 400 ms, hedged | +4.2 | |
| Us, 5 bps (model default), 1 s, hedged | −10.5 | |

- **Jupiter charges more than our grid.** It takes 6 bps on open and 6 on close, plus a price-impact
  fee (about 1 bp per $375k, plus a one-minute imbalance charge, capped at 50 bps), plus ~14%/yr
  borrow on SOL longs. A one-day SOL long pays about 6 + 3.8 bps per trade, before impact.
- **Our 12 bps all-in per trade is in Jupiter's range.** Our 5 bps is below it, and the arb punishes that.
- **JLP's extra edge comes from holding the coins against long-skewed flow.** We treat that as risk,
  not edge. Our unhedged column shows the same effect: +0.3 to +2.4 bps over hedged in a +37% SOL
  window, with 3–5× the daily SD.

## Size-based fee: possible, but it does nothing at our scale

- **It is possible at the pin.** `TradeNoCpi` needs both signatures, the taker's and our LP's
  (`P 8709-8715` at `5cb331dd`). The fee is `max(caller fee_bps, trade_fee_base_bps)`, and it is
  refused above `max_trading_fee_bps` (`P 15285-15289`).
- So our co-signing matcher can set `fee_bps` per trade by size. It is whole bps only. There is no
  program-level size curve, and under AuthMark there is no impact or externality fee (`P 15298-15304`).
- **At our volumes it changes nothing.** At $10M/day the largest scaled trade in 90 days was $93k, and
  the median daily maximum was $21k. A step of 1 bp per $375k never triggers, so the fee stays at the
  base. It only matters at Jupiter-like size, or against an arb who sizes up. Our arb cap already
  limits that.

## Method

`replay.py`, deterministic, with no randomness:

1. **Flow.** Every Binance USDⓈ-M SOLUSDT aggTrade (taker side from `is_buyer_maker`) is a taker fill
   against our LP.
   - Sizes are scaled by one constant per target volume: $100k, $1M and $10M a day ÷ the mean daily
     Binance notional of $1.78B. Busy days stay busy.
   - Each fill is at `mark = oracle(t − lag)`.
   - We book only the taker fee as income. The LP-side fee goes to our own insurance.
2. **Oracle.** The last Binance spot SOLUSDT print at or before `t`, with no look-ahead.
3. **Inventory limit.** The matcher fills only the part of a trade that keeps |LP| ≤ 10% of daily
   volume ($100k at $1M/day, from `ops/CAPITAL_PLAN.md`). It filled 98.6% of base flow.
4. **Toxic overlay.** Every 400 ms slot, if |oracle(t)/mark − 1| > fee + 2 bps, an arbitrageur trades
   0.5% of daily volume at our mark, in the profitable direction. The inventory limit applies to it too.
   This is an upper bound: it fires whenever it profits, slot after slot, for the whole window.
5. **Hedge.** When |LP + hedge| × oracle(t) > threshold, the whole net is hedged on Hyperliquid at
   `oracle(t + 500 ms)` plus taker fee plus book slippage.
   - Thresholds: none, tight (1% of daily volume) and loose (5%).
   - Hourly Hyperliquid funding is applied to the hedge leg.
6. **P&L.** Equity = cash + (LP + hedge) × oracle, marked at each UTC day end.
   - "Inventory P&L" = daily P&L − fees + hedge cost − funding. It includes the stale-mark arb loss.
   - Margin is per $ of **base** (non-arb) volume. Break-even solves V × margin(V) = fixed cost, with
     margin interpolated across the three volume tiers.
7. **Markouts** (the adverse-selection table) use future oracle prices only to measure. No decision
   uses them.

## Inputs

| Input | Value | Measured or assumption |
|---|---|---|
| Taker flow | Binance USDⓈ-M SOLUSDT aggTrades, 2026-07-30 to 2026-10-27, SHA-256 checked against Binance's `.CHECKSUM` files | measured (public) |
| Oracle | Binance spot SOLUSDT aggTrades, same window | **assumption: stand-in for Pyth SOL/USD.** Pyth Benchmarks and Hermes history return 401 without a key |
| Pusher lag | 0 / 400 ms / 1 s / 2 s, constant | assumption (grid) |
| Hedge venue fee | 4.5 bps, Hyperliquid base tier | published schedule |
| Hedge slippage | book walk, Hyperliquid SOL l2Book snapshot (`inputs/hl_l2book_SOL.json`, mid 102.82, ~$5–7M per side over 20 levels); beyond the book, 2× the deepest level | measured once; **assumed constant** |
| Hedge latency | 500 ms | assumption |
| Hedge funding | Hyperliquid SOL hourly funding, same window | measured |
| Inventory limit | 10% of daily volume | from `ops/CAPITAL_PLAN.md` ($100k at $1M/day), scaled |
| Arb cost / size / rate | 2 bps / 0.5% of daily volume / one per 400 ms slot | assumption |
| Fixed cost | $101.85/day (SOL tx, RPC, servers, capital cost) | `ops/money_model.py` defaults (unverified there) |
| Other lines | +0.68 bps (liquidation, backing, maintenance, minus 0.5 bps bad debt) | `ops/money_model.py` defaults (unverified there) |
| Jupiter / JLP | see "Benchmark" | external reports |

## Limits

- **Flow realism, part 1: wrong mix of traders.** Binance flow is the most informed, HFT-heavy SOL
  flow there is. Its markout also includes its own price impact, which our tiny venue would not cause.
  The 2.2–2.4 bps is likely an **upper** estimate for organic flow. Retail-sized trades show 0.1–1.0 bps.
- **Flow realism, part 2: no reaction to the fee.** Flow does not respond to the fee. Real takers would
  trade less at 12 bps than at 5 bps: Jupiter's SOL volume fell 21–28% when its imbalance fee arrived.
  So the 15–20 bps rows are not achievable as shown. Fee elasticity is unmeasured; the PoC beta has to
  measure it.
- **Toxic-flow assumptions.** The arb's 2 bps cost, $5k size at $1M/day and one-per-slot rate are
  guesses. Doubling the size roughly doubles the arb rows.
  - The matcher's real defence is not modelled: the LP co-signs `TradeNoCpi` and refuses while
    target ≠ effective (`ops/MONEY_MODEL.md`). With it, the arb rows shrink, possibly to near zero.
  - So the true cost lies between the "base" grid and the "toxic" grid. Both are in the appendix.
- **Oracle stand-in.** Pyth aggregates many venues, and Binance spot is one input. Any Pyth-specific
  lag or jitter is missing, and lag is measured against the Binance print. So "400 ms" means 400 ms
  behind Binance spot, not behind Pyth.
- **No leverage, liquidations or bad debt.** Taker accounts are not simulated. Bad debt and the
  liquidation and backing lines stay as the model's unverified assumptions. A gap move bigger than the
  LP's 15% buffer is not tested.
- **Hedge.** The book is one snapshot, and fills take 500 ms with no queue risk. Perp–spot basis and
  USDT/USD are ignored.
- **Cash.** These are accrual results. Fee income sits in insurance behind the market-wide Live gate
  (`ops/MONEY_MODEL.md`), while hedge fees and losses are paid in cash.
- **One window.** 90 days, with SOL up 37% and one sharp stretch in late August. A crash window may look
  different for the unhedged LP. The hedged results depend much less on direction.
- **Not covered:** the Jupiter benchmark is cited, not re-derived. Volume we can attract is not measured.

## Reproduce

```
python3 research/replay/fetch.py     # ~200 MB into research/replay/data/ (git-ignored), checksummed
python3 research/replay/replay.py    # ~2.5 min; prints the appendix below
```

Only numpy and the standard library are needed. No keys are needed and nothing is paid.
`REPLAY_DATA=<dir>` runs on a subset of days.

## Sources

- Binance public data: https://data.binance.vision/ (futures/um and spot daily aggTrades).
- Hyperliquid info API (`l2Book`, `fundingHistory`): https://api.hyperliquid.xyz/info.
- Tried and unavailable:
  - Drift historical-data S3 (empty bucket);
  - Hyperliquid S3 archive (requester-pays);
  - Pyth Benchmarks and Hermes history (401).
- Jupiter / JLP:
  - https://discuss.jup.ag/ (Risk category);
  - https://docs.jup.ag/user-docs/trade/perps/fees.md;
  - https://pineanalytics.substack.com/p/jupiter-quarterly-report-q3-2026;
  - DefiLlama fees API.
- Program citations (`P`) are to wrapper `src/v16_program.rs` @ `5cb331dd` (`scripts/pins.env`).

## Appendix: full output of `replay.py`

Window: 2026-07-30 to 2026-10-27 (90 days). Mean Binance SOLUSDT-perp taker notional $1,778.1M/day.
Hedge book snapshot: mid 102.82, 20-level depth bid $6.9M / ask $4.8M.
Fixed cost 101.85 USD/day; model's other lines net +0.682 bps.

### Hedge slippage (Hyperliquid book walk, avg of buy and sell)

| Hedge size | Slippage bps | + taker fee |
|---:|---:|---:|
| $1k | 0.49 | 4.99 |
| $10k | 0.49 | 4.99 |
| $100k | 0.71 | 5.21 |
| $1.0M | 3.08 | 7.58 |
| $10.0M | 21.18 | 25.68 |

### Volatility (oracle, per day)

Median annualised 1-min vol 53%; SOL 73.69 -> 101.35.

| Top-vol day | Ann. vol | Intraday range | Open -> close |
|---|---:|---:|---:|
| 2026-08-22 | 177% | 13.5% | +0.1% |
| 2026-08-25 | 93% | 6.9% | -2.4% |
| 2026-08-27 | 91% | 9.3% | +6.9% |
| 2026-08-21 | 91% | 8.4% | +6.9% |
| 2026-08-23 | 91% | 5.7% | +1.7% |

### Adverse selection, base flow (LP markout vs mark, bps of volume; positive = LP loses)

| Lag | 0s | 1s | 10s | 60s | 300s |
|---:|---:|---:|---:|---:|---:|
| 0 ms | -0.00 | 1.32 | 1.48 | 1.62 | 1.43 |
| 400 ms | 0.63 | 1.95 | 2.11 | 2.24 | 2.05 |
| 1000 ms | 0.67 | 1.99 | 2.15 | 2.28 | 2.10 |
| 2000 ms | 0.75 | 2.08 | 2.23 | 2.37 | 2.18 |

By raw Binance trade size (lag 0 and 1 s):

| Lag | Trade size | Share of volume | 0s | 1s | 10s | 60s | 300s |
|---|---|---:|---:|---:|---:|---:|---:|
| 0 ms | $0–$1k | 2% | -0.00 | 0.03 | 0.08 | 0.12 | 0.16 |
| 0 ms | $1k–$10k | 12% | -0.00 | 0.39 | 0.55 | 0.64 | 0.46 |
| 0 ms | $10k–$100k | 58% | -0.00 | 1.01 | 1.22 | 1.36 | 1.12 |
| 0 ms | $100k–$1.0M | 27% | -0.00 | 2.33 | 2.39 | 2.53 | 2.35 |
| 0 ms | $1.0M–+ | 1% | -0.00 | 3.98 | 3.91 | 4.52 | 5.86 |
| 1000 ms | $0–$1k | 2% | 0.07 | 0.11 | 0.15 | 0.20 | 0.24 |
| 1000 ms | $1k–$10k | 12% | 0.38 | 0.78 | 0.94 | 1.03 | 0.84 |
| 1000 ms | $10k–$100k | 58% | 0.62 | 1.64 | 1.85 | 1.98 | 1.74 |
| 1000 ms | $100k–$1.0M | 27% | 0.95 | 3.28 | 3.33 | 3.48 | 3.29 |
| 1000 ms | $1.0M–+ | 1% | 0.28 | 4.26 | 4.20 | 4.80 | 6.14 |

Inventory limit 10% of daily volume: base flow filled 98.6% of offered notional.

### Toxic overlay: latency arb (per slot, cap 0.5% of daily volume per trade)

| Fee | Lag | Arb trades/day | Arb volume, % of base | Arb loss to LP at 0s before the limit, bps of base | net of its fee | Filled, all flow (limit) |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 3 | 400 ms | 280 | 140.1% | 10.60 | 6.40 | 94% |
| 3 | 1000 ms | 937 | 468.4% | 35.06 | 21.01 | 91% |
| 3 | 2000 ms | 2,387 | 1193.3% | 89.72 | 53.92 | 88% |
| 5 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 5 | 400 ms | 108 | 53.8% | 5.60 | 2.91 | 97% |
| 5 | 1000 ms | 345 | 172.7% | 17.93 | 9.29 | 93% |
| 5 | 2000 ms | 886 | 443.0% | 46.11 | 23.96 | 89% |
| 8 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 8 | 400 ms | 40 | 20.1% | 2.82 | 1.21 | 98% |
| 8 | 1000 ms | 124 | 62.2% | 8.84 | 3.87 | 95% |
| 8 | 2000 ms | 318 | 159.0% | 22.75 | 10.03 | 90% |
| 12 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 12 | 400 ms | 12 | 6.0% | 1.17 | 0.45 | 98% |
| 12 | 1000 ms | 40 | 19.8% | 3.91 | 1.54 | 96% |
| 12 | 2000 ms | 104 | 52.0% | 10.33 | 4.08 | 93% |
| 15 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 15 | 400 ms | 5 | 2.6% | 0.66 | 0.27 | 98% |
| 15 | 1000 ms | 19 | 9.3% | 2.31 | 0.91 | 97% |
| 15 | 2000 ms | 51 | 25.7% | 6.30 | 2.45 | 95% |
| 20 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 20 | 400 ms | 2 | 1.1% | 0.36 | 0.15 | 98% |
| 20 | 1000 ms | 7 | 3.7% | 1.24 | 0.51 | 98% |
| 20 | 2000 ms | 20 | 9.9% | 3.30 | 1.32 | 97% |

### Grid, base flow: trading margin bps of base volume (fees − adverse − hedge + funding), and break-even

| Fee | Lag | Hedge | margin @$100k | @$1M | @$10M | Net/day @$1M | Max DD @$1M | Break-even (trading) | (+ model lines) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|
| 3 | 0 | none | 4.15 | 4.15 | 4.15 | 313 | $14k | $245k | $211k |
| 3 | 0 | tight | 1.54 | 1.54 | 1.53 | 52 | $2k | $665k | $460k |
| 3 | 0 | loose | 2.87 | 2.87 | 2.86 | 185 | $3k | $357k | $288k |
| 3 | 400 | none | 3.57 | 3.57 | 3.57 | 255 | $14k | $287k | $240k |
| 3 | 400 | tight | 0.96 | 0.96 | 0.95 | -6 | $4k | $1.1M | $624k |
| 3 | 400 | loose | 2.29 | 2.29 | 2.28 | 127 | $3k | $447k | $343k |
| 3 | 1000 | none | 3.54 | 3.54 | 3.54 | 252 | $14k | $288k | $243k |
| 3 | 1000 | tight | 0.93 | 0.93 | 0.92 | -9 | $4k | $1.1M | $635k |
| 3 | 1000 | loose | 2.26 | 2.26 | 2.25 | 124 | $3k | $452k | $347k |
| 3 | 2000 | none | 3.47 | 3.47 | 3.47 | 246 | $15k | $293k | $245k |
| 3 | 2000 | tight | 0.86 | 0.86 | 0.85 | -16 | $4k | $1.2M | $661k |
| 3 | 2000 | loose | 2.19 | 2.19 | 2.19 | 117 | $4k | $465k | $355k |
| 5 | 0 | none | 6.12 | 6.12 | 6.12 | 511 | $11k | $167k | $150k |
| 5 | 0 | tight | 3.51 | 3.51 | 3.50 | 249 | $1k | $290k | $244k |
| 5 | 0 | loose | 4.84 | 4.84 | 4.84 | 382 | $3k | $211k | $185k |
| 5 | 400 | none | 5.55 | 5.55 | 5.55 | 453 | $11k | $184k | $164k |
| 5 | 400 | tight | 2.93 | 2.93 | 2.92 | 191 | $2k | $349k | $282k |
| 5 | 400 | loose | 4.26 | 4.26 | 4.26 | 324 | $3k | $240k | $207k |
| 5 | 1000 | none | 5.51 | 5.51 | 5.51 | 449 | $11k | $185k | $165k |
| 5 | 1000 | tight | 2.90 | 2.90 | 2.89 | 188 | $2k | $353k | $285k |
| 5 | 1000 | loose | 4.23 | 4.23 | 4.23 | 321 | $3k | $241k | $208k |
| 5 | 2000 | none | 5.45 | 5.45 | 5.45 | 443 | $11k | $187k | $167k |
| 5 | 2000 | tight | 2.83 | 2.83 | 2.83 | 182 | $2k | $361k | $290k |
| 5 | 2000 | loose | 4.17 | 4.17 | 4.16 | 315 | $3k | $245k | $210k |
| 8 | 0 | none | 9.09 | 9.09 | 9.09 | 807 | $8k | $112k | $105k |
| 8 | 0 | tight | 6.47 | 6.47 | 6.46 | 545 | $245 | $158k | $143k |
| 8 | 0 | loose | 7.80 | 7.80 | 7.80 | 678 | $2k | $131k | $120k |
| 8 | 400 | none | 8.51 | 8.51 | 8.51 | 749 | $8k | $120k | $111k |
| 8 | 400 | tight | 5.89 | 5.89 | 5.88 | 487 | $742 | $174k | $156k |
| 8 | 400 | loose | 7.22 | 7.22 | 7.22 | 621 | $2k | $141k | $130k |
| 8 | 1000 | none | 8.47 | 8.47 | 8.47 | 746 | $8k | $120k | $112k |
| 8 | 1000 | tight | 5.86 | 5.86 | 5.85 | 484 | $748 | $175k | $156k |
| 8 | 1000 | loose | 7.19 | 7.19 | 7.19 | 617 | $2k | $142k | $130k |
| 8 | 2000 | none | 8.41 | 8.41 | 8.41 | 739 | $8k | $122k | $112k |
| 8 | 2000 | tight | 5.80 | 5.80 | 5.79 | 478 | $801 | $176k | $158k |
| 8 | 2000 | loose | 7.13 | 7.13 | 7.12 | 611 | $2k | $144k | $131k |
| 12 | 0 | none | 13.03 | 13.03 | 13.03 | 1,201 | $6k | $79k | $75k |
| 12 | 0 | tight | 10.42 | 10.42 | 10.41 | 940 | $0 | $98k | $92k |
| 12 | 0 | loose | 11.75 | 11.75 | 11.75 | 1,073 | $1k | $87k | $82k |
| 12 | 400 | none | 12.45 | 12.45 | 12.45 | 1,144 | $6k | $82k | $78k |
| 12 | 400 | tight | 9.84 | 9.84 | 9.83 | 882 | $0 | $104k | $97k |
| 12 | 400 | loose | 11.17 | 11.17 | 11.17 | 1,015 | $2k | $91k | $86k |
| 12 | 1000 | none | 12.42 | 12.42 | 12.42 | 1,140 | $6k | $82k | $78k |
| 12 | 1000 | tight | 9.81 | 9.81 | 9.80 | 879 | $0 | $104k | $97k |
| 12 | 1000 | loose | 11.14 | 11.14 | 11.13 | 1,012 | $2k | $92k | $87k |
| 12 | 2000 | none | 12.36 | 12.36 | 12.36 | 1,134 | $6k | $83k | $79k |
| 12 | 2000 | tight | 9.74 | 9.74 | 9.73 | 872 | $0 | $105k | $98k |
| 12 | 2000 | loose | 11.07 | 11.07 | 11.07 | 1,006 | $2k | $92k | $87k |
| 15 | 0 | none | 15.99 | 15.99 | 15.99 | 1,497 | $5k | $64k | $61k |
| 15 | 0 | tight | 13.38 | 13.38 | 13.37 | 1,236 | $0 | $76k | $72k |
| 15 | 0 | loose | 14.71 | 14.71 | 14.71 | 1,369 | $940 | $70k | $66k |
| 15 | 400 | none | 15.41 | 15.41 | 15.41 | 1,440 | $5k | $66k | $63k |
| 15 | 400 | tight | 12.80 | 12.80 | 12.79 | 1,178 | $0 | $80k | $76k |
| 15 | 400 | loose | 14.13 | 14.13 | 14.13 | 1,311 | $1k | $72k | $69k |
| 15 | 1000 | none | 15.38 | 15.38 | 15.38 | 1,436 | $5k | $66k | $63k |
| 15 | 1000 | tight | 12.77 | 12.77 | 12.76 | 1,175 | $0 | $80k | $76k |
| 15 | 1000 | loose | 14.10 | 14.10 | 14.10 | 1,308 | $1k | $72k | $69k |
| 15 | 2000 | none | 15.32 | 15.32 | 15.32 | 1,430 | $5k | $67k | $64k |
| 15 | 2000 | tight | 12.70 | 12.70 | 12.70 | 1,169 | $0 | $80k | $76k |
| 15 | 2000 | loose | 14.03 | 14.03 | 14.03 | 1,302 | $1k | $73k | $70k |
| 20 | 0 | none | 20.93 | 20.93 | 20.93 | 1,991 | $3k | $49k | $47k |
| 20 | 0 | tight | 18.31 | 18.31 | 18.31 | 1,730 | $0 | $56k | $54k |
| 20 | 0 | loose | 19.65 | 19.65 | 19.64 | 1,863 | $161 | $52k | $50k |
| 20 | 400 | none | 20.35 | 20.35 | 20.35 | 1,933 | $3k | $50k | $49k |
| 20 | 400 | tight | 17.74 | 17.74 | 17.73 | 1,672 | $0 | $58k | $56k |
| 20 | 400 | loose | 19.07 | 19.07 | 19.06 | 1,805 | $320 | $54k | $52k |
| 20 | 1000 | none | 20.32 | 20.32 | 20.32 | 1,930 | $3k | $50k | $49k |
| 20 | 1000 | tight | 17.70 | 17.70 | 17.70 | 1,669 | $0 | $58k | $56k |
| 20 | 1000 | loose | 19.03 | 19.03 | 19.03 | 1,802 | $336 | $54k | $52k |
| 20 | 2000 | none | 20.25 | 20.25 | 20.25 | 1,923 | $4k | $50k | $49k |
| 20 | 2000 | tight | 17.64 | 17.64 | 17.63 | 1,662 | $0 | $58k | $56k |
| 20 | 2000 | loose | 18.97 | 18.97 | 18.96 | 1,795 | $376 | $54k | $52k |

### Grid, toxic flow: trading margin bps of base volume (fees − adverse − hedge + funding), and break-even

| Fee | Lag | Hedge | margin @$100k | @$1M | @$10M | Net/day @$1M | Max DD @$1M | Break-even (trading) | (+ model lines) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|
| 3 | 0 | none | 4.15 | 4.15 | 4.15 | 313 | $14k | $245k | $211k |
| 3 | 0 | tight | 1.54 | 1.54 | 1.53 | 52 | $2k | $665k | $460k |
| 3 | 0 | loose | 2.87 | 2.87 | 2.86 | 185 | $3k | $357k | $288k |
| 3 | 400 | none | -7.23 | -7.23 | -7.23 | -825 | $86k | none | none |
| 3 | 400 | tight | -7.83 | -7.83 | -8.02 | -885 | $80k | none | none |
| 3 | 400 | loose | -6.14 | -6.14 | -6.42 | -716 | $67k | none | none |
| 3 | 1000 | none | -22.59 | -22.59 | -22.59 | -2,361 | $218k | none | none |
| 3 | 1000 | tight | -31.21 | -31.21 | -32.01 | -3,222 | $290k | none | none |
| 3 | 1000 | loose | -21.48 | -21.48 | -22.62 | -2,250 | $203k | none | none |
| 3 | 2000 | none | -48.18 | -48.18 | -48.18 | -4,919 | $446k | none | none |
| 3 | 2000 | tight | -85.43 | -85.43 | -87.85 | -8,645 | $778k | none | none |
| 3 | 2000 | loose | -57.82 | -57.82 | -61.99 | -5,884 | $530k | none | none |
| 5 | 0 | none | 6.12 | 6.12 | 6.12 | 511 | $11k | $167k | $150k |
| 5 | 0 | tight | 3.51 | 3.51 | 3.50 | 249 | $1k | $290k | $244k |
| 5 | 0 | loose | 4.84 | 4.84 | 4.84 | 382 | $3k | $211k | $185k |
| 5 | 400 | none | -0.60 | -0.60 | -0.60 | -162 | $37k | none | $12.4M |
| 5 | 400 | tight | -0.88 | -0.88 | -0.96 | -190 | $20k | none | none |
| 5 | 400 | loose | -1.01 | -1.01 | -1.13 | -203 | $24k | none | none |
| 5 | 1000 | none | -7.27 | -7.27 | -7.27 | -829 | $85k | none | none |
| 5 | 1000 | tight | -10.53 | -10.53 | -10.84 | -1,155 | $104k | none | none |
| 5 | 1000 | loose | -7.42 | -7.42 | -7.85 | -844 | $80k | none | none |
| 5 | 2000 | none | -20.44 | -20.44 | -20.44 | -2,146 | $199k | none | none |
| 5 | 2000 | tight | -32.52 | -32.52 | -33.44 | -3,354 | $302k | none | none |
| 5 | 2000 | loose | -22.32 | -22.33 | -23.84 | -2,335 | $211k | none | none |
| 8 | 0 | none | 9.09 | 9.09 | 9.09 | 807 | $8k | $112k | $105k |
| 8 | 0 | tight | 6.47 | 6.47 | 6.46 | 545 | $245 | $158k | $143k |
| 8 | 0 | loose | 7.80 | 7.80 | 7.80 | 678 | $2k | $131k | $120k |
| 8 | 400 | none | 4.47 | 4.47 | 4.47 | 346 | $18k | $228k | $198k |
| 8 | 400 | tight | 4.20 | 4.20 | 4.16 | 318 | $3k | $244k | $209k |
| 8 | 400 | loose | 4.70 | 4.70 | 4.63 | 368 | $2k | $218k | $189k |
| 8 | 1000 | none | 0.87 | 0.87 | 0.87 | -15 | $28k | $1.2M | $661k |
| 8 | 1000 | tight | 0.46 | 0.46 | 0.34 | -56 | $16k | $2.5M | $891k |
| 8 | 1000 | loose | 1.57 | 1.56 | 1.38 | 55 | $13k | $653k | $454k |
| 8 | 2000 | none | -4.32 | -4.32 | -4.32 | -534 | $61k | none | none |
| 8 | 2000 | tight | -7.87 | -7.87 | -8.21 | -889 | $82k | none | none |
| 8 | 2000 | loose | -5.11 | -5.11 | -5.63 | -612 | $63k | none | none |
| 12 | 0 | none | 13.03 | 13.03 | 13.03 | 1,201 | $6k | $79k | $75k |
| 12 | 0 | tight | 10.42 | 10.42 | 10.41 | 940 | $0 | $98k | $92k |
| 12 | 0 | loose | 11.75 | 11.75 | 11.75 | 1,073 | $1k | $87k | $82k |
| 12 | 400 | none | 11.23 | 11.23 | 11.23 | 1,021 | $7k | $91k | $86k |
| 12 | 400 | tight | 9.27 | 9.27 | 9.26 | 826 | $465 | $110k | $102k |
| 12 | 400 | loose | 9.77 | 9.77 | 9.73 | 875 | $3k | $105k | $98k |
| 12 | 1000 | none | 9.26 | 9.26 | 9.26 | 824 | $7k | $110k | $103k |
| 12 | 1000 | tight | 7.77 | 7.77 | 7.72 | 675 | $3k | $132k | $121k |
| 12 | 1000 | loose | 8.15 | 8.15 | 8.08 | 713 | $3k | $125k | $115k |
| 12 | 2000 | none | 4.75 | 4.75 | 4.75 | 373 | $13k | $215k | $188k |
| 12 | 2000 | tight | 4.67 | 4.67 | 4.56 | 365 | $9k | $219k | $191k |
| 12 | 2000 | loose | 5.44 | 5.44 | 5.23 | 442 | $9k | $187k | $167k |
| 15 | 0 | none | 15.99 | 15.99 | 15.99 | 1,497 | $5k | $64k | $61k |
| 15 | 0 | tight | 13.38 | 13.38 | 13.37 | 1,236 | $0 | $76k | $72k |
| 15 | 0 | loose | 14.71 | 14.71 | 14.71 | 1,369 | $940 | $70k | $66k |
| 15 | 400 | none | 14.76 | 14.76 | 14.76 | 1,374 | $6k | $69k | $66k |
| 15 | 400 | tight | 12.51 | 12.51 | 12.50 | 1,149 | $0 | $82k | $78k |
| 15 | 400 | loose | 13.19 | 13.19 | 13.17 | 1,217 | $2k | $78k | $74k |
| 15 | 1000 | none | 12.15 | 12.15 | 12.15 | 1,113 | $11k | $84k | $79k |
| 15 | 1000 | tight | 11.70 | 11.70 | 11.67 | 1,068 | $872 | $87k | $83k |
| 15 | 1000 | loose | 11.13 | 11.13 | 11.07 | 1,011 | $7k | $92k | $87k |
| 15 | 2000 | none | 12.35 | 12.35 | 12.35 | 1,133 | $8k | $83k | $79k |
| 15 | 2000 | tight | 10.10 | 10.10 | 10.04 | 908 | $4k | $101k | $95k |
| 15 | 2000 | loose | 9.93 | 9.93 | 9.80 | 891 | $10k | $103k | $96k |
| 20 | 0 | none | 20.93 | 20.93 | 20.93 | 1,991 | $3k | $49k | $47k |
| 20 | 0 | tight | 18.31 | 18.31 | 18.31 | 1,730 | $0 | $56k | $54k |
| 20 | 0 | loose | 19.65 | 19.65 | 19.64 | 1,863 | $161 | $52k | $50k |
| 20 | 400 | none | 19.98 | 19.98 | 19.98 | 1,896 | $3k | $51k | $50k |
| 20 | 400 | tight | 17.57 | 17.57 | 17.55 | 1,655 | $0 | $58k | $56k |
| 20 | 400 | loose | 18.54 | 18.54 | 18.52 | 1,752 | $3k | $55k | $53k |
| 20 | 1000 | none | 18.34 | 18.34 | 18.34 | 1,732 | $10k | $56k | $54k |
| 20 | 1000 | tight | 17.24 | 17.24 | 17.23 | 1,622 | $0 | $59k | $57k |
| 20 | 1000 | loose | 18.06 | 18.06 | 18.04 | 1,704 | $0 | $57k | $55k |
| 20 | 2000 | none | 18.02 | 18.02 | 18.02 | 1,700 | $12k | $57k | $55k |
| 20 | 2000 | tight | 16.38 | 16.38 | 16.36 | 1,537 | $951 | $62k | $60k |
| 20 | 2000 | loose | 17.53 | 17.53 | 17.49 | 1,651 | $2k | $58k | $56k |

### Headline: toxic flow, realistic lags, $1M/day. Hedged (tight) = edge; unhedged adds directional P&L

| Fee | Lag | Hedged margin bps | Inventory bps | Hedge cost bps | Funding bps | Net/day | SD/day | Max DD | Break-even | Unhedged margin bps | Unhedged net/day | Unhedged SD/day | Unhedged max DD |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 3 | 400 | -7.83 | -11.66 | 2.98 | 0.00 | -885 | 1,180 | $80k | none | -7.23 | -825 | 2,531 | $86k |
| 3 | 1000 | -31.21 | -33.36 | 13.35 | -0.01 | -3,222 | 3,893 | $290k | none | -22.59 | -2,361 | 4,066 | $218k |
| 5 | 400 | -0.88 | -7.14 | 1.23 | 0.04 | -190 | 577 | $20k | none | -0.60 | -162 | 2,353 | $37k |
| 5 | 1000 | -10.53 | -18.09 | 5.11 | 0.00 | -1,155 | 1,759 | $104k | none | -7.27 | -829 | 2,732 | $85k |
| 8 | 400 | 4.20 | -4.69 | 0.59 | 0.07 | 318 | 368 | $3k | $244k | 4.47 | 346 | 2,140 | $18k |
| 8 | 1000 | 0.46 | -9.94 | 1.92 | 0.04 | -56 | 719 | $16k | $2.5M | 0.87 | -15 | 1,982 | $28k |
| 12 | 400 | 9.27 | -3.02 | 0.33 | 0.11 | 826 | 445 | $465 | $110k | 11.23 | 1,021 | 2,194 | $7k |
| 12 | 1000 | 7.77 | -5.43 | 0.75 | 0.08 | 675 | 531 | $3k | $132k | 9.26 | 824 | 1,984 | $7k |
| 15 | 400 | 12.51 | -2.53 | 0.25 | 0.12 | 1,149 | 592 | $0 | $82k | 14.76 | 1,374 | 2,288 | $6k |
| 15 | 1000 | 11.70 | -3.92 | 0.43 | 0.10 | 1,068 | 587 | $872 | $87k | 12.15 | 1,113 | 2,300 | $11k |
| 20 | 400 | 17.57 | -2.27 | 0.21 | 0.13 | 1,655 | 788 | $0 | $58k | 19.98 | 1,896 | 2,479 | $3k |
| 20 | 1000 | 17.24 | -2.93 | 0.27 | 0.11 | 1,622 | 803 | $0 | $59k | 18.34 | 1,732 | 2,560 | $10k |

### Setting 12 bps / 1000 ms / tight hedge, toxic, $1M/day

Net/day mean 675, SD 531, max drawdown $3k, days positive 97%.

| Inventory | p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|
| net exposure, LP + hedge | $3k | $8k | $9k | $10k |
| LP on-chain position | $69k | $99k | $100k | $100k |

Five worst days:

| Day | Net | Fees | Inventory MTM | Hedge cost | SOL open→close | Range | Ann. vol |
|---|---:|---:|---:|---:|---:|---:|---:|
| 2026-08-22 | -2,561 | 4,833 | -6,656 | 665 | +0.1% | 13.5% | 177% |
| 2026-08-23 | -310 | 2,104 | -2,156 | 184 | +1.7% | 5.7% | 91% |
| 2026-08-25 | -103 | 3,198 | -2,902 | 322 | -2.4% | 6.9% | 93% |
| 2026-08-01 | 120 | 634 | -356 | 54 | -1.3% | 3.7% | 32% |
| 2026-08-16 | 122 | 414 | -180 | 21 | -1.0% | 2.0% | 24% |

### Setting 5 bps / 1000 ms / tight hedge, toxic, $1M/day

Net/day mean -1,155, SD 1,759, max drawdown $104k, days positive 8%.

| Inventory | p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|
| net exposure, LP + hedge | $3k | $8k | $10k | $10k |
| LP on-chain position | $64k | $100k | $100k | $100k |

Five worst days:

| Day | Net | Fees | Inventory MTM | Hedge cost | SOL open→close | Range | Ann. vol |
|---|---:|---:|---:|---:|---:|---:|---:|
| 2026-08-22 | -13,108 | 7,108 | -16,345 | 3,810 | +0.1% | 13.5% | 177% |
| 2026-08-25 | -6,102 | 4,529 | -8,387 | 2,154 | -2.4% | 6.9% | 93% |
| 2026-08-23 | -4,558 | 3,172 | -6,070 | 1,573 | +1.7% | 5.7% | 91% |
| 2026-08-27 | -3,975 | 3,731 | -5,840 | 1,758 | +6.9% | 9.3% | 91% |
| 2026-08-28 | -3,790 | 3,691 | -5,744 | 1,644 | -4.6% | 7.3% | 85% |
