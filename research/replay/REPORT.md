# SOL-PERP replay: does our market make money?

**Verdict.** At the money model's **5 bps fee it does not**. Once a latency arbitrageur is added, real
flow loses money at every pusher lag we can reach: hedged, −1.0 bps of volume at 400 ms and −10.6 bps
at 1 s.

**At 12 bps, hedged, with lag of 1 s or less, it makes money, but the size of the edge depends on what
our flow looks like:**

| Flow shape | Edge at 12 bps / 1 s | Net at $1M/day | Break-even |
|---|---:|---:|---:|
| Many small tickets (Binance flow scaled down) | **+7.7 bps** | +$669/day | ~$133k/day |
| Fewer whole Binance-size tickets | **+2.2–2.6 bps** | +$115–160/day | ~$400–470k/day |

- **Small tickets:** 97% of days are positive, the worst day is −$2.8k and the max drawdown is $3k.
- **Whole tickets:** the inventory cap refuses about a quarter of the volume, and large tickets carry more
  adverse selection.
- **8 bps is marginal.** It is +4.1 bps at 400 ms, and about zero at 1 s or with whole tickets.

**Adverse selection:**
- **Measured on real flow: 2.2–2.4 bps** at 400 ms–2 s lag, so the model's 2 bps was about right.
- **The model missed the latency arb.** At fees of 5 bps or more and 400 ms–1 s lag, it costs another
  0.4–18 bps gross, about half that net of the fee it pays. At 1 s, a 4× larger arb ticket cuts the
  12 bps edge to +2.6 bps.

**The risks:**
- **Lag and fee together are the business.**
- **Volume is unproven.** These results hold flow fixed as the fee rises.
- **The flow shape is unknown.** Our real ticket mix decides whether the edge is closer to 2.5 or
  8 bps.
- **Profit is locked.** Fees land in insurance behind the Live gate.

**Against Jupiter.** Jupiter/JLP keeps about 17–23 bps. Our hedged 2.5–9 bps at 12 bps is a fraction of
that, and it carries no directional bet.

**What would change this verdict:** measuring two numbers in the PoC beta, our real ticket-size mix and
the matcher's refusal rate against stale marks.

All results come from 90 days of real flow, 2026-07-30 to 2026-10-27, which includes the Aug 19–28
volatility stretch, scaled to our volume.

## Headline: toxic flow (real flow + latency arb), $1M/day, flow scaled down

Hedged (tight, net ≤ 1% of daily volume) is the **edge**. Unhedged adds the LP's directional P&L.
SOL rose 37% in the window and the capped LP sat long, so the unhedged column is luck, not edge.

| Fee | Lag | Hedged edge bps | Net/day | SD/day | Max DD | Break-even/day | Unhedged bps | Unhedged SD/day |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 5 | 400 ms | −1.01 | −$202 | $582 | $20k | none | −0.69 | $2,365 |
| 5 | 1 s | −10.63 | −$1,164 | $1,790 | $105k | none | −7.38 | $2,710 |
| 8 | 400 ms | 4.06 | $304 | $363 | $2k | $251k | 4.57 | $2,130 |
| 8 | 1 s | 0.52 | −$49 | $732 | $16k | $2.1M | 0.79 | $1,992 |
| **12** | **400 ms** | **9.47** | **$845** | $479 | $0.9k | **$108k** | 11.08 | $2,202 |
| **12** | **1 s** | **7.71** | **$669** | $546 | $3k | **$133k** | 9.14 | $1,986 |
| 15 | 400 ms | 12.41 | $1,139 | $580 | $0 | $82k | 14.50 | $2,288 |
| 15 | 1 s | 11.62 | $1,060 | $594 | $1.0k | $88k | 12.11 | $2,312 |
| 20 | 400 ms | 17.73 | $1,671 | $805 | $0 | $58k | 19.94 | $2,485 |
| 20 | 1 s | 17.25 | $1,623 | $809 | $0 | $59k | 18.27 | $2,539 |

- **Net/day** is after the model's fixed costs ($101.85/day).
- **Break-even** is trading edge against fixed costs. The model's other lines (liquidation, backing and
  maintenance fees, minus bad debt: +0.68 bps, all unverified) lower it:
  - by 6–8% at 12 bps;
  - by 14% at 8 bps / 400 ms;
  - by 60% at 8 bps / 1 s, where the edge is thin.

  See the grids.
- **Fees of 15–20 bps look best only because flow is held fixed.** Takers would leave (see "Limits"). We
  don't recommend them on this evidence.
- **Lag 0 is not reachable.** Our oracle stand-in is the Binance spot print, and Pyth itself trails it.
  So 400 ms is the best case and 1 s is a realistic pusher.
- **The hedge fills 500 ms after it is sent.** So net exposure briefly exceeds the 1% threshold, to a
  maximum of about 2%.

## Sensitivity (toxic, tight hedge, $1M/day)

This was added at review: one input changes per row. Full rows are in the appendix.

| Variant | 12 bps / 1 s | 8 bps / 400 ms |
|---|---:|---:|
| Reference: flow scaled down, arb ticket 0.5% of daily volume | +7.71 | +4.06 |
| Arb ticket ×4 (2%) | +2.61 | −1.59 |
| Arb ticket ×4, at 12 bps / 400 ms | +7.35 | |
| Arb slot clock shifted 200 ms | +7.79 | |
| Whole Binance tickets, every 1,778th trade, three offsets | +2.17 to +2.61 | −0.08 to +0.11 |

- **Whole tickets** keep real sizes and timing. The volume is about the same, but there are about 100
  lumpy trades a day. The $100k inventory cap fills only 74–79% of the volume, and large tickets have
  2.7–4.8 bps of adverse selection.
- **Real flow on our venue is somewhere between the two:** small-venue retail tickets are smaller than
  Binance's average.
- **The arb at 12 bps is robust at 400 ms but not at 1 s.** A larger arb ticket at 1 s lag takes most of
  the edge.

## Best settings

| | Fee | Lag | Hedge | Why |
|---|---:|---|---|---|
| **Recommended start** | **12 bps** | **≤ 400 ms if possible**, ≤ 1 s at most | tight (net ≤ 1% of daily volume) | +7.4 to +9.5 bps at 400 ms in both arb-size variants; +2.2 to +7.7 bps across flow shapes at 1 s. 12 bps per trade is close to Jupiter's 6 bps base plus borrow plus impact for a one-day hold |
| Not on its own | 8 bps | ≤ 400 ms | tight | +4.1 bps with small tickets; ~0 with whole tickets or a 4× arb |
| Do not run | ≤ 5 bps | any reachable lag | any | Negative edge under the arb overlay |

## Adverse selection (measured, replaces the model's 2 bps)

This is the LP markout on real flow against the oracle at each horizon. The units are bps of
volume, and a positive number means the LP loses.

| Lag | at fill | 1 s | 10 s | 60 s | 5 min |
|---:|---:|---:|---:|---:|---:|
| 0 | 0.00 | 1.40 | 1.56 | 1.69 | 1.50 |
| 400 ms | 0.55 | 1.95 | 2.11 | **2.24** | 2.05 |
| 1 s | 0.59 | 1.99 | 2.15 | **2.28** | 2.10 |
| 2 s | 0.67 | 2.08 | 2.23 | **2.37** | 2.18 |

- **Cross-check.** With the arb nearly absent (20 bps, 400 ms, hedged), the replay's realised inventory
  loss is 2.12 bps, close to the 60 s markout.
- **By raw trade size, at 60 s (lag 0 to 1 s):**
  - under $10k, 14% of volume: 0.1–1.0 bps;
  - $10k–$100k: 1.4–2.0 bps;
  - over $100k: 2.7–4.8 bps.

  Retail-sized flow is close to harmless (see the appendix).
- **The latency arb is the larger cost.** It is loss to the LP at fill, before the inventory limit, in
  bps of base volume:

  | | 400 ms | 1 s | 2 s |
  |---|---:|---:|---:|
  | 5 bps | 5.6 | 17.9 | 46.1 |
  | 8 bps | 2.8 | 8.9 | 22.8 |
  | 12 bps | 1.2 | 3.9 | 10.3 |
  | 20 bps | 0.4 | 1.3 | 3.3 |

  The arb also pays our fee, so its net cost is about half of these numbers. A higher fee is the
  cheapest defence: the arb needs |oracle − mark| > fee + its own cost.
- **Hedge.** Hyperliquid taker fee 4.5 bps plus 0.5–0.7 bps of book slippage at $1k–$100k. Tight
  hedging costs 0.2–1.9 bps of volume when the fee is ≥ 8 bps.
- **Funding** on the hedge leg: about +0.1 bps (Hyperliquid hourly rates). On our venue it is about 0;
  see `ops/MONEY_MODEL.md`.

## Worst days (12 bps, 1 s, tight hedge, $1M/day)

| Day | Net | Fees | Inventory P&L | Hedge cost | SOL open→close | Intraday range | Ann. vol |
|---|---:|---:|---:|---:|---:|---:|---:|
| 2026-08-22 | −$2,813 | $4,895 | −$6,941 | $691 | +0.1% | 13.5% | 177% |
| 2026-08-23 | −$249 | $2,098 | −$2,094 | $179 | +1.7% | 5.7% | 91% |
| 2026-08-25 | −$184 | $3,186 | −$2,972 | $322 | −2.4% | 6.9% | 93% |
| 2026-08-01 | +$119 | $634 | −$356 | $54 | −1.3% | 3.7% | 32% |
| 2026-08-08 | +$127 | $700 | −$414 | $49 | +3.2% | 4.2% | 29% |

**Why:**
- The bad days are the volatility spikes. Aug 22 had a 13.5% range at 177% annualised vol. The mark is
  stale for longer, so the arb fires on most slots and the hedge lags behind it.
- Fees cover most of it, and only three days lose money.
- **At 5 bps the same days are about 5× worse** (−$13.4k on Aug 22), because the arb clears a 5 + 2 bps
  bar far more often.
- Max drawdown is $3k at 12 bps and $105k at 5 bps (1 s, tight).

## Benchmark: Jupiter Perps / JLP

The source is the Chaos Labs weekly risk reports: 67 weeks, Aug 2024 – Mar 2026, about $315B of volume.
The parent session gathered them; see "Sources".

| | bps of volume | Notes |
|---|---:|---|
| JLP pool + protocol take, 67 weeks | ~24 (≈19.5 to JLP) | 18 bps fees plus ~6 bps of trader losses |
| JLP, Q3 2026 / Q2 2026 | 20.7 / 26.5 (17.3 / 22.9 to JLP) | |
| JLP trader-loss part, by half-year | −12.7 to +1.2 | Directional: OI is ~90% long and JLP holds the coins |
| **Us, 12 bps, ≤ 1 s, hedged** | **+2.2 to +9.5** | Depends on flow shape and lag; no directional bet |
| Us, 8 bps, 400 ms, hedged | −0.1 to +4.1 | |
| Us, 5 bps (model default), 1 s, hedged | −10.6 | |

- **Jupiter charges more than our grid.** It takes 6 bps on open and 6 on close, plus a price-impact
  fee (about 1 bp per $375k, plus a one-minute imbalance charge, capped at 50 bps), plus ~14%/yr
  borrow on SOL longs. A one-day SOL long pays about 6 + 3.8 bps per trade, before impact.
- **Our 12 bps all-in per trade is in Jupiter's range.** Our 5 bps is below it, and the arb punishes that.
- **JLP's extra edge comes from holding the coins against long-skewed flow.** We treat that as risk,
  not edge. Our unhedged column shows the same effect: +0.3 to +3.3 bps over hedged in a +37% SOL
  window, with 1.5–6× the daily SD.

## Size-based fee: possible, but it does nothing with scaled tickets

- **It is possible at the pin.** `TradeNoCpi` needs both signatures, the taker's and our LP's
  (`P 8709-8715` at `5cb331dd`). The fee is `max(caller fee_bps, trade_fee_base_bps)`, and it is
  refused above `max_trading_fee_bps` (`P 15285-15289`).
- So our co-signing matcher can set `fee_bps` per trade by size. It is whole bps only. There is no
  program-level size curve, and under AuthMark there is no impact or externality fee (`P 15298-15304`).
- **With scaled tickets it changes nothing.** At $10M/day the largest scaled trade in 90 days was $93k,
  so a step of 1 bp per $375k never triggers.
- **With whole Binance tickets it would bite.** Those reach $16M, and above $100k they carry 2.7–4.8
  bps of adverse selection. Against that flow, a size fee or a per-trade size limit is a cheap guard
  worth measuring in the beta.

## Method

`replay.py`, deterministic, with no randomness:

1. **Flow.** Every Binance USDⓈ-M SOLUSDT aggTrade (taker side from `is_buyer_maker`) is a taker fill
   against our LP.
   - Sizes are scaled by one constant per target volume: $100k, $1M and $10M a day ÷ the mean daily
     Binance notional of $1.78B. Busy days stay busy. The sensitivity table instead keeps every
     1,778th whole ticket.
   - Each fill is at `mark = oracle(t − lag)`.
   - We book only the taker fee as income. The LP-side fee goes to our own insurance.
2. **Oracle.** The last Binance spot SOLUSDT print at or before `t`, with no look-ahead.
   - Spot microsecond timestamps are rounded **up** to the millisecond.
   - A warm-up day precedes the window, and queries before the first print are refused.
   - Markouts that would reach past the end of the data are dropped.
3. **Inventory limit.** The matcher fills only the part of a trade that keeps |LP| ≤ 10% of daily
   volume ($100k at $1M/day, from `ops/CAPITAL_PLAN.md`). It filled 98.6% of base flow.
4. **Toxic overlay, a stress scenario rather than a bound.** Every 400 ms slot, if
   |oracle(t)/mark − 1| > fee + 2 bps, an arbitrageur trades 0.5% of daily volume at our mark, in the
   profitable direction. The inventory limit applies to it too.
   - A smarter arb with bigger tickets extracts more: see the 4× row.
   - The matcher's stale-mark refusal would extract less (see "Limits").
5. **Hedge.** When |LP + committed hedge| × oracle(t) > threshold, the whole net is sent to
   Hyperliquid.
   - It **fills at `oracle(t + 500 ms)`** plus taker fee plus book slippage. Its position, cash and
     funding start at the fill.
   - Orders still open at midnight carry to the next day.
   - Thresholds: none, tight (1% of daily volume) and loose (5%).
   - Hourly Hyperliquid funding is applied to the filled hedge.
6. **P&L.** Equity = cash + (LP + filled hedge) × oracle, marked at each UTC day end.
   - "Inventory P&L" = daily P&L − fees + hedge cost − funding. It includes the stale-mark arb loss.
   - Margin is per $ of **base** (non-arb) volume. Break-even solves V × margin(V) = fixed cost, with
     margin interpolated across the three volume tiers.
7. **Markouts** (the adverse-selection table) use future oracle prices only to measure. No decision
   uses them.

## Inputs

| Input | Value | Measured or assumption |
|---|---|---|
| Taker flow | Binance USDⓈ-M SOLUSDT aggTrades, 2026-07-30 to 2026-10-27, SHA-256 checked against Binance's `.CHECKSUM` files, every row validated | measured (public) |
| Flow shape at our volume | scaled tickets (headline) or every N-th whole ticket (sensitivity) | **assumption: the main open question** |
| Oracle | Binance spot SOLUSDT aggTrades, same window plus one warm-up day | **assumption: stand-in for Pyth SOL/USD.** Pyth Benchmarks and Hermes history return 401 without a key |
| Pusher lag | 0 / 400 ms / 1 s / 2 s, constant | assumption (grid) |
| Hedge venue fee | 4.5 bps, Hyperliquid base tier | published schedule |
| Hedge slippage | book walk, Hyperliquid SOL l2Book snapshot (`inputs/hl_l2book_SOL.json`, mid 102.82, ~$5–7M per side over 20 levels); beyond the book, 2× the deepest level | measured once; **assumed constant** |
| Hedge latency | 500 ms | assumption |
| Hedge funding | Hyperliquid SOL hourly funding, same window | measured |
| Inventory limit | 10% of daily volume | from `ops/CAPITAL_PLAN.md` ($100k at $1M/day), scaled |
| Arb cost / size / rate | 2 bps / 0.5% of daily volume (and 2%) / one per 400 ms slot | assumption |
| Fixed cost | $101.85/day (SOL tx, RPC, servers, capital cost) | `ops/money_model.py` defaults (unverified there) |
| Other lines | +0.68 bps (liquidation, backing, maintenance, minus 0.5 bps bad debt) | `ops/money_model.py` defaults (unverified there) |
| Jupiter / JLP | see "Benchmark" | external reports |

## Limits

- **Flow shape.** This is the biggest open question: whole-ticket and scaled flow differ by about 5 bps
  of edge.
  - Binance flow is the most informed, HFT-heavy SOL flow there is, and its markout includes its own
    price impact, which our tiny venue would not cause. So 2.2–2.4 bps is likely an **upper** estimate
    for organic flow. Retail-sized trades show 0.1–1.0 bps.
  - But whole Binance tickets show that lumpy flow against a $100k cap can cost most of the edge.
  - Our real ticket mix is unmeasured; the PoC beta has to measure it.
- **Fee response.** Flow does not respond to the fee in this replay. Real takers would trade less at
  12 bps than at 5 bps: Jupiter's SOL volume fell 21–28% when its imbalance fee arrived. So the 15–20
  bps rows are not achievable as shown. Fee elasticity is unmeasured.
- **Toxic flow is a stress scenario, not a bound.**
  - The arb's 2 bps cost, ticket size and one-per-slot rate are guesses. A 4× ticket is shown, and a
    better-timed or larger arb could take more.
  - The matcher's real defence is not modelled: the LP co-signs `TradeNoCpi` and can refuse fills
    while target ≠ effective (`ops/MONEY_MODEL.md`). That would cut the arb's take.
  - The base-flow grid, with no arb, is in the appendix for comparison.
- **Oracle stand-in.** Pyth aggregates many venues, and Binance spot is one input. Any Pyth-specific
  lag or jitter is missing, and lag is measured against the Binance print. So "400 ms" means 400 ms
  behind Binance spot.
- **No leverage, liquidations or bad debt.** Taker accounts are not simulated. Bad debt and the
  liquidation and backing lines stay as the model's unverified assumptions. A gap move bigger than the
  LP's 15% buffer is not tested.
- **Hedge.** The book is one snapshot, and a fixed 500 ms latency carries no queue risk. Perp–spot basis
  and USDT/USD are ignored.
- **Cash.** These are accrual results. Fee income sits in insurance behind the market-wide Live gate
  (`ops/MONEY_MODEL.md`), while hedge fees and losses are paid in cash.
- **One window.** 90 days, with SOL up 37% and one sharp stretch in late August. A crash window may look
  different for the unhedged LP. The hedged results depend much less on direction.
- **Not covered:** the Jupiter benchmark is cited, not re-derived. Volume we can attract is not measured.

## Reproduce

```
python3 research/replay/fetch.py     # ~200 MB into research/replay/data/ (git-ignored), checksummed
python3 research/replay/replay.py    # ~4 min; prints the appendix below
```

Only numpy and the standard library are needed. No keys are needed and nothing is paid.
`REPLAY_DATA=<dir>` runs on a subset of days; include the spot file for the day before the first one.

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

Median annualised 1-min vol 53%; SOL 73.68 -> 101.35.

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
| 0 ms | -0.00 | 1.40 | 1.56 | 1.69 | 1.50 |
| 400 ms | 0.55 | 1.95 | 2.11 | 2.24 | 2.05 |
| 1000 ms | 0.59 | 1.99 | 2.15 | 2.28 | 2.10 |
| 2000 ms | 0.67 | 2.08 | 2.23 | 2.37 | 2.18 |

By raw Binance trade size (lag 0 and 1 s):

| Lag | Trade size | Share of volume | 0s | 1s | 10s | 60s | 300s |
|---|---|---:|---:|---:|---:|---:|---:|
| 0 ms | $0–$1k | 2% | -0.00 | 0.03 | 0.08 | 0.12 | 0.16 |
| 0 ms | $1k–$10k | 12% | -0.00 | 0.41 | 0.57 | 0.65 | 0.47 |
| 0 ms | $10k–$100k | 58% | -0.00 | 1.07 | 1.28 | 1.41 | 1.18 |
| 0 ms | $100k–$1.0M | 27% | -0.00 | 2.48 | 2.53 | 2.68 | 2.49 |
| 0 ms | $1.0M–+ | 1% | -0.00 | 4.13 | 4.07 | 4.67 | 6.01 |
| 1000 ms | $0–$1k | 2% | 0.07 | 0.11 | 0.15 | 0.19 | 0.24 |
| 1000 ms | $1k–$10k | 12% | 0.37 | 0.78 | 0.94 | 1.03 | 0.84 |
| 1000 ms | $10k–$100k | 58% | 0.57 | 1.64 | 1.85 | 1.98 | 1.74 |
| 1000 ms | $100k–$1.0M | 27% | 0.80 | 3.28 | 3.33 | 3.48 | 3.29 |
| 1000 ms | $1.0M–+ | 1% | 0.13 | 4.26 | 4.19 | 4.80 | 6.14 |

Inventory limit 10% of daily volume: base flow filled 98.6% of offered notional.

### Toxic overlay: latency arb (per slot, cap 0.5% of daily volume per trade)

| Fee | Lag | Arb trades/day | Arb volume, % of base | Arb loss to LP at 0s before the limit, bps of base | net of its fee | Filled, all flow (limit) |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 3 | 400 ms | 280 | 140.0% | 10.60 | 6.40 | 94% |
| 3 | 1000 ms | 937 | 468.4% | 35.06 | 21.01 | 91% |
| 3 | 2000 ms | 2,386 | 1193.2% | 89.71 | 53.92 | 88% |
| 5 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 5 | 400 ms | 108 | 53.8% | 5.60 | 2.91 | 97% |
| 5 | 1000 ms | 345 | 172.6% | 17.92 | 9.29 | 93% |
| 5 | 2000 ms | 886 | 442.9% | 46.11 | 23.96 | 89% |
| 8 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 8 | 400 ms | 40 | 20.1% | 2.82 | 1.21 | 98% |
| 8 | 1000 ms | 124 | 62.2% | 8.85 | 3.87 | 95% |
| 8 | 2000 ms | 318 | 159.0% | 22.76 | 10.04 | 90% |
| 12 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 12 | 400 ms | 12 | 6.0% | 1.17 | 0.46 | 98% |
| 12 | 1000 ms | 40 | 19.8% | 3.92 | 1.54 | 96% |
| 12 | 2000 ms | 104 | 52.0% | 10.32 | 4.08 | 93% |
| 15 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 15 | 400 ms | 5 | 2.6% | 0.67 | 0.27 | 98% |
| 15 | 1000 ms | 19 | 9.3% | 2.31 | 0.92 | 97% |
| 15 | 2000 ms | 51 | 25.7% | 6.31 | 2.45 | 95% |
| 20 | 0 ms | 0 | 0.0% | -0.00 | -0.00 | 99% |
| 20 | 400 ms | 2 | 1.1% | 0.37 | 0.15 | 99% |
| 20 | 1000 ms | 7 | 3.7% | 1.25 | 0.51 | 98% |
| 20 | 2000 ms | 20 | 9.9% | 3.30 | 1.33 | 97% |

### Grid, base flow: trading margin bps of base volume (fees − adverse − hedge + funding), and break-even

| Fee | Lag | Hedge | margin @$100k | @$1M | @$10M | Net/day @$1M | Max DD @$1M | Break-even (trading) | (+ model lines) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|
| 3 | 0 | none | 4.08 | 4.08 | 4.08 | 306 | $14k | $250k | $215k |
| 3 | 0 | tight | 1.46 | 1.46 | 1.46 | 45 | $2k | $696k | $476k |
| 3 | 0 | loose | 2.80 | 2.80 | 2.79 | 178 | $3k | $365k | $293k |
| 3 | 400 | none | 3.57 | 3.57 | 3.57 | 255 | $14k | $287k | $240k |
| 3 | 400 | tight | 0.96 | 0.96 | 0.95 | -6 | $4k | $1.1M | $624k |
| 3 | 400 | loose | 2.29 | 2.29 | 2.29 | 127 | $3k | $447k | $343k |
| 3 | 1000 | none | 3.54 | 3.54 | 3.54 | 252 | $14k | $288k | $241k |
| 3 | 1000 | tight | 0.93 | 0.93 | 0.92 | -9 | $4k | $1.1M | $635k |
| 3 | 1000 | loose | 2.26 | 2.26 | 2.25 | 124 | $3k | $452k | $347k |
| 3 | 2000 | none | 3.47 | 3.47 | 3.47 | 246 | $15k | $293k | $245k |
| 3 | 2000 | tight | 0.86 | 0.86 | 0.85 | -16 | $4k | $1.2M | $661k |
| 3 | 2000 | loose | 2.19 | 2.19 | 2.19 | 117 | $4k | $465k | $355k |
| 5 | 0 | none | 6.05 | 6.05 | 6.05 | 503 | $11k | $169k | $151k |
| 5 | 0 | tight | 3.44 | 3.44 | 3.43 | 242 | $1k | $297k | $248k |
| 5 | 0 | loose | 4.77 | 4.77 | 4.77 | 375 | $3k | $214k | $187k |
| 5 | 400 | none | 5.55 | 5.55 | 5.55 | 453 | $11k | $184k | $164k |
| 5 | 400 | tight | 2.93 | 2.93 | 2.92 | 191 | $2k | $349k | $283k |
| 5 | 400 | loose | 4.26 | 4.26 | 4.26 | 325 | $3k | $240k | $207k |
| 5 | 1000 | none | 5.51 | 5.51 | 5.51 | 449 | $11k | $185k | $165k |
| 5 | 1000 | tight | 2.90 | 2.90 | 2.89 | 188 | $2k | $353k | $285k |
| 5 | 1000 | loose | 4.23 | 4.23 | 4.23 | 321 | $3k | $241k | $208k |
| 5 | 2000 | none | 5.45 | 5.45 | 5.45 | 443 | $11k | $187k | $167k |
| 5 | 2000 | tight | 2.83 | 2.83 | 2.83 | 182 | $2k | $361k | $290k |
| 5 | 2000 | loose | 4.17 | 4.17 | 4.16 | 315 | $3k | $245k | $210k |
| 8 | 0 | none | 9.01 | 9.01 | 9.01 | 799 | $8k | $114k | $105k |
| 8 | 0 | tight | 6.40 | 6.40 | 6.39 | 538 | $245 | $159k | $145k |
| 8 | 0 | loose | 7.73 | 7.73 | 7.73 | 671 | $2k | $132k | $122k |
| 8 | 400 | none | 8.51 | 8.51 | 8.51 | 749 | $8k | $120k | $111k |
| 8 | 400 | tight | 5.89 | 5.89 | 5.88 | 487 | $713 | $174k | $156k |
| 8 | 400 | loose | 7.22 | 7.22 | 7.22 | 621 | $2k | $141k | $129k |
| 8 | 1000 | none | 8.47 | 8.47 | 8.47 | 746 | $8k | $120k | $112k |
| 8 | 1000 | tight | 5.86 | 5.86 | 5.85 | 484 | $721 | $175k | $156k |
| 8 | 1000 | loose | 7.19 | 7.19 | 7.19 | 617 | $2k | $142k | $130k |
| 8 | 2000 | none | 8.41 | 8.41 | 8.41 | 739 | $8k | $122k | $112k |
| 8 | 2000 | tight | 5.79 | 5.79 | 5.79 | 478 | $772 | $176k | $158k |
| 8 | 2000 | loose | 7.13 | 7.13 | 7.12 | 611 | $2k | $144k | $131k |
| 12 | 0 | none | 12.96 | 12.96 | 12.96 | 1,194 | $6k | $79k | $75k |
| 12 | 0 | tight | 10.35 | 10.35 | 10.34 | 933 | $0 | $99k | $93k |
| 12 | 0 | loose | 11.68 | 11.68 | 11.67 | 1,066 | $1k | $88k | $83k |
| 12 | 400 | none | 12.45 | 12.45 | 12.45 | 1,144 | $6k | $82k | $78k |
| 12 | 400 | tight | 9.84 | 9.84 | 9.83 | 882 | $0 | $104k | $97k |
| 12 | 400 | loose | 11.17 | 11.17 | 11.17 | 1,015 | $2k | $91k | $86k |
| 12 | 1000 | none | 12.42 | 12.42 | 12.42 | 1,140 | $6k | $82k | $78k |
| 12 | 1000 | tight | 9.81 | 9.81 | 9.80 | 879 | $0 | $104k | $97k |
| 12 | 1000 | loose | 11.14 | 11.14 | 11.13 | 1,012 | $2k | $92k | $87k |
| 12 | 2000 | none | 12.36 | 12.36 | 12.36 | 1,134 | $6k | $83k | $79k |
| 12 | 2000 | tight | 9.74 | 9.74 | 9.73 | 872 | $0 | $105k | $98k |
| 12 | 2000 | loose | 11.07 | 11.07 | 11.07 | 1,006 | $2k | $92k | $87k |
| 15 | 0 | none | 15.92 | 15.92 | 15.92 | 1,490 | $5k | $64k | $62k |
| 15 | 0 | tight | 13.31 | 13.31 | 13.30 | 1,229 | $0 | $77k | $73k |
| 15 | 0 | loose | 14.64 | 14.64 | 14.63 | 1,362 | $948 | $70k | $67k |
| 15 | 400 | none | 15.41 | 15.41 | 15.41 | 1,440 | $5k | $66k | $63k |
| 15 | 400 | tight | 12.80 | 12.80 | 12.79 | 1,178 | $0 | $80k | $76k |
| 15 | 400 | loose | 14.13 | 14.13 | 14.13 | 1,311 | $1k | $72k | $69k |
| 15 | 1000 | none | 15.38 | 15.38 | 15.38 | 1,436 | $5k | $66k | $63k |
| 15 | 1000 | tight | 12.77 | 12.77 | 12.76 | 1,175 | $0 | $80k | $76k |
| 15 | 1000 | loose | 14.10 | 14.10 | 14.10 | 1,308 | $1k | $72k | $69k |
| 15 | 2000 | none | 15.32 | 15.32 | 15.32 | 1,430 | $5k | $67k | $64k |
| 15 | 2000 | tight | 12.70 | 12.70 | 12.69 | 1,168 | $0 | $80k | $76k |
| 15 | 2000 | loose | 14.04 | 14.04 | 14.03 | 1,302 | $1k | $73k | $70k |
| 20 | 0 | none | 20.86 | 20.86 | 20.86 | 1,984 | $3k | $49k | $47k |
| 20 | 0 | tight | 18.24 | 18.24 | 18.23 | 1,722 | $0 | $56k | $54k |
| 20 | 0 | loose | 19.57 | 19.57 | 19.57 | 1,856 | $169 | $52k | $50k |
| 20 | 400 | none | 20.35 | 20.35 | 20.35 | 1,933 | $3k | $50k | $49k |
| 20 | 400 | tight | 17.73 | 17.73 | 17.73 | 1,672 | $0 | $58k | $56k |
| 20 | 400 | loose | 19.07 | 19.07 | 19.06 | 1,805 | $320 | $54k | $52k |
| 20 | 1000 | none | 20.32 | 20.32 | 20.32 | 1,930 | $3k | $50k | $49k |
| 20 | 1000 | tight | 17.70 | 17.70 | 17.69 | 1,668 | $0 | $58k | $56k |
| 20 | 1000 | loose | 19.03 | 19.03 | 19.03 | 1,802 | $337 | $54k | $52k |
| 20 | 2000 | none | 20.25 | 20.25 | 20.25 | 1,923 | $4k | $50k | $49k |
| 20 | 2000 | tight | 17.64 | 17.64 | 17.63 | 1,662 | $0 | $58k | $56k |
| 20 | 2000 | loose | 18.97 | 18.97 | 18.96 | 1,795 | $377 | $54k | $52k |

### Grid, toxic flow: trading margin bps of base volume (fees − adverse − hedge + funding), and break-even

| Fee | Lag | Hedge | margin @$100k | @$1M | @$10M | Net/day @$1M | Max DD @$1M | Break-even (trading) | (+ model lines) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|
| 3 | 0 | none | 4.08 | 4.08 | 4.08 | 306 | $14k | $250k | $215k |
| 3 | 0 | tight | 1.46 | 1.46 | 1.46 | 45 | $2k | $696k | $476k |
| 3 | 0 | loose | 2.80 | 2.80 | 2.79 | 178 | $3k | $365k | $293k |
| 3 | 400 | none | -7.41 | -7.41 | -7.41 | -843 | $88k | none | none |
| 3 | 400 | tight | -7.75 | -7.75 | -7.94 | -877 | $79k | none | none |
| 3 | 400 | loose | -6.92 | -6.93 | -7.21 | -794 | $73k | none | none |
| 3 | 1000 | none | -22.71 | -22.71 | -22.71 | -2,373 | $219k | none | none |
| 3 | 1000 | tight | -31.12 | -31.12 | -31.92 | -3,214 | $289k | none | none |
| 3 | 1000 | loose | -21.29 | -21.30 | -22.41 | -2,231 | $201k | none | none |
| 3 | 2000 | none | -48.26 | -48.26 | -48.26 | -4,928 | $446k | none | none |
| 3 | 2000 | tight | -85.38 | -85.38 | -87.80 | -8,640 | $778k | none | none |
| 3 | 2000 | loose | -57.47 | -57.48 | -61.61 | -5,849 | $526k | none | none |
| 5 | 0 | none | 6.05 | 6.05 | 6.05 | 503 | $11k | $169k | $151k |
| 5 | 0 | tight | 3.44 | 3.44 | 3.43 | 242 | $1k | $297k | $248k |
| 5 | 0 | loose | 4.77 | 4.77 | 4.77 | 375 | $3k | $214k | $187k |
| 5 | 400 | none | -0.69 | -0.69 | -0.69 | -171 | $38k | none | none |
| 5 | 400 | tight | -1.01 | -1.01 | -1.08 | -202 | $20k | none | none |
| 5 | 400 | loose | -0.98 | -0.98 | -1.10 | -199 | $24k | none | none |
| 5 | 1000 | none | -7.38 | -7.38 | -7.38 | -840 | $86k | none | none |
| 5 | 1000 | tight | -10.63 | -10.63 | -10.94 | -1,164 | $105k | none | none |
| 5 | 1000 | loose | -6.21 | -6.21 | -6.63 | -723 | $71k | none | none |
| 5 | 2000 | none | -20.44 | -20.44 | -20.44 | -2,146 | $199k | none | none |
| 5 | 2000 | tight | -32.53 | -32.53 | -33.45 | -3,355 | $302k | none | none |
| 5 | 2000 | loose | -21.37 | -21.37 | -22.78 | -2,239 | $203k | none | none |
| 8 | 0 | none | 9.01 | 9.01 | 9.01 | 799 | $8k | $114k | $105k |
| 8 | 0 | tight | 6.40 | 6.40 | 6.39 | 538 | $245 | $159k | $145k |
| 8 | 0 | loose | 7.73 | 7.73 | 7.73 | 671 | $2k | $132k | $122k |
| 8 | 400 | none | 4.57 | 4.57 | 4.57 | 355 | $18k | $224k | $195k |
| 8 | 400 | tight | 4.06 | 4.06 | 4.02 | 304 | $2k | $251k | $215k |
| 8 | 400 | loose | 4.21 | 4.21 | 4.15 | 319 | $2k | $243k | $209k |
| 8 | 1000 | none | 0.79 | 0.79 | 0.79 | -23 | $29k | $1.3M | $696k |
| 8 | 1000 | tight | 0.52 | 0.52 | 0.40 | -49 | $16k | $2.1M | $846k |
| 8 | 1000 | loose | 1.47 | 1.47 | 1.29 | 45 | $15k | $696k | $476k |
| 8 | 2000 | none | -4.32 | -4.32 | -4.32 | -534 | $61k | none | none |
| 8 | 2000 | tight | -7.87 | -7.87 | -8.20 | -889 | $82k | none | none |
| 8 | 2000 | loose | -4.42 | -4.42 | -4.95 | -544 | $54k | none | none |
| 12 | 0 | none | 12.96 | 12.96 | 12.96 | 1,194 | $6k | $79k | $75k |
| 12 | 0 | tight | 10.35 | 10.35 | 10.34 | 933 | $0 | $99k | $93k |
| 12 | 0 | loose | 11.68 | 11.68 | 11.67 | 1,066 | $1k | $88k | $83k |
| 12 | 400 | none | 11.08 | 11.08 | 11.08 | 1,007 | $7k | $92k | $87k |
| 12 | 400 | tight | 9.47 | 9.47 | 9.45 | 845 | $933 | $108k | $101k |
| 12 | 400 | loose | 9.38 | 9.38 | 9.35 | 836 | $4k | $109k | $102k |
| 12 | 1000 | none | 9.14 | 9.14 | 9.14 | 812 | $7k | $112k | $104k |
| 12 | 1000 | tight | 7.71 | 7.71 | 7.66 | 669 | $3k | $133k | $122k |
| 12 | 1000 | loose | 8.77 | 8.77 | 8.68 | 775 | $2k | $117k | $108k |
| 12 | 2000 | none | 4.76 | 4.76 | 4.76 | 374 | $12k | $215k | $187k |
| 12 | 2000 | tight | 4.69 | 4.69 | 4.58 | 367 | $9k | $218k | $191k |
| 12 | 2000 | loose | 4.86 | 4.86 | 4.67 | 384 | $13k | $210k | $184k |
| 15 | 0 | none | 15.92 | 15.92 | 15.92 | 1,490 | $5k | $64k | $62k |
| 15 | 0 | tight | 13.31 | 13.31 | 13.30 | 1,229 | $0 | $77k | $73k |
| 15 | 0 | loose | 14.64 | 14.64 | 14.63 | 1,362 | $948 | $70k | $67k |
| 15 | 400 | none | 14.50 | 14.50 | 14.50 | 1,348 | $6k | $70k | $67k |
| 15 | 400 | tight | 12.41 | 12.41 | 12.39 | 1,139 | $0 | $82k | $78k |
| 15 | 400 | loose | 13.09 | 13.09 | 13.07 | 1,208 | $5k | $78k | $74k |
| 15 | 1000 | none | 12.11 | 12.11 | 12.11 | 1,109 | $12k | $84k | $80k |
| 15 | 1000 | tight | 11.62 | 11.62 | 11.59 | 1,060 | $995 | $88k | $83k |
| 15 | 1000 | loose | 12.07 | 12.07 | 12.03 | 1,105 | $4k | $85k | $80k |
| 15 | 2000 | none | 12.21 | 12.21 | 12.21 | 1,119 | $9k | $84k | $79k |
| 15 | 2000 | tight | 10.25 | 10.25 | 10.19 | 923 | $3k | $99k | $93k |
| 15 | 2000 | loose | 9.71 | 9.71 | 9.58 | 869 | $10k | $105k | $98k |
| 20 | 0 | none | 20.86 | 20.86 | 20.86 | 1,984 | $3k | $49k | $47k |
| 20 | 0 | tight | 18.24 | 18.24 | 18.23 | 1,722 | $0 | $56k | $54k |
| 20 | 0 | loose | 19.57 | 19.57 | 19.57 | 1,856 | $169 | $52k | $50k |
| 20 | 400 | none | 19.94 | 19.94 | 19.94 | 1,892 | $4k | $51k | $50k |
| 20 | 400 | tight | 17.73 | 17.73 | 17.72 | 1,671 | $0 | $58k | $56k |
| 20 | 400 | loose | 18.83 | 18.83 | 18.82 | 1,781 | $3k | $54k | $52k |
| 20 | 1000 | none | 18.27 | 18.27 | 18.27 | 1,725 | $10k | $56k | $54k |
| 20 | 1000 | tight | 17.25 | 17.25 | 17.23 | 1,623 | $0 | $59k | $57k |
| 20 | 1000 | loose | 17.60 | 17.60 | 17.57 | 1,658 | $0 | $58k | $56k |
| 20 | 2000 | none | 17.97 | 17.97 | 17.97 | 1,695 | $12k | $57k | $55k |
| 20 | 2000 | tight | 16.36 | 16.36 | 16.33 | 1,534 | $1k | $62k | $60k |
| 20 | 2000 | loose | 17.59 | 17.59 | 17.54 | 1,657 | $2k | $58k | $56k |

### Headline: toxic flow, realistic lags, $1M/day. Hedged (tight) = edge; unhedged adds directional P&L

| Fee | Lag | Hedged margin bps | Inventory bps | Hedge cost bps | Funding bps | Net/day | SD/day | Max DD | Break-even | Unhedged margin bps | Unhedged net/day | Unhedged SD/day | Unhedged max DD |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 3 | 400 | -7.75 | -11.58 | 2.98 | 0.00 | -877 | 1,167 | $79k | none | -7.41 | -843 | 2,553 | $88k |
| 3 | 1000 | -31.12 | -33.34 | 13.28 | -0.01 | -3,214 | 3,891 | $289k | none | -22.71 | -2,373 | 4,050 | $219k |
| 5 | 400 | -1.01 | -7.25 | 1.24 | 0.04 | -202 | 582 | $20k | none | -0.69 | -171 | 2,365 | $38k |
| 5 | 1000 | -10.63 | -18.19 | 5.10 | 0.00 | -1,164 | 1,790 | $105k | none | -7.38 | -840 | 2,710 | $86k |
| 8 | 400 | 4.06 | -4.81 | 0.61 | 0.07 | 304 | 363 | $2k | $251k | 4.57 | 355 | 2,130 | $18k |
| 8 | 1000 | 0.52 | -9.86 | 1.94 | 0.04 | -49 | 732 | $16k | $2.1M | 0.79 | -23 | 1,992 | $29k |
| 12 | 400 | 9.47 | -2.83 | 0.32 | 0.11 | 845 | 479 | $933 | $108k | 11.08 | 1,007 | 2,202 | $7k |
| 12 | 1000 | 7.71 | -5.49 | 0.75 | 0.07 | 669 | 546 | $3k | $133k | 9.14 | 812 | 1,986 | $7k |
| 15 | 400 | 12.41 | -2.63 | 0.26 | 0.12 | 1,139 | 580 | $0 | $82k | 14.50 | 1,348 | 2,288 | $6k |
| 15 | 1000 | 11.62 | -4.00 | 0.43 | 0.10 | 1,060 | 594 | $995 | $88k | 12.11 | 1,109 | 2,312 | $12k |
| 20 | 400 | 17.73 | -2.12 | 0.21 | 0.13 | 1,671 | 805 | $0 | $58k | 19.94 | 1,892 | 2,485 | $4k |
| 20 | 1000 | 17.25 | -2.93 | 0.27 | 0.11 | 1,623 | 809 | $0 | $59k | 18.27 | 1,725 | 2,539 | $10k |

### Setting 12 bps / 1000 ms / tight hedge, toxic, $1M/day

Net/day mean 669, SD 546, max drawdown $3k, days positive 97%.

| Inventory | p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|
| net exposure, LP + hedge | $3k | $8k | $9k | $17k |
| LP on-chain position | $69k | $99k | $100k | $100k |

Five worst days:

| Day | Net | Fees | Inventory MTM | Hedge cost | SOL open→close | Range | Ann. vol |
|---|---:|---:|---:|---:|---:|---:|---:|
| 2026-08-22 | -2,813 | 4,895 | -6,941 | 691 | +0.1% | 13.5% | 177% |
| 2026-08-23 | -249 | 2,098 | -2,094 | 179 | +1.7% | 5.7% | 91% |
| 2026-08-25 | -184 | 3,186 | -2,972 | 322 | -2.4% | 6.9% | 93% |
| 2026-08-01 | 119 | 634 | -356 | 54 | -1.3% | 3.7% | 32% |
| 2026-08-08 | 127 | 700 | -414 | 49 | +3.2% | 4.2% | 29% |

### Setting 5 bps / 1000 ms / tight hedge, toxic, $1M/day

Net/day mean -1,164, SD 1,790, max drawdown $105k, days positive 8%.

| Inventory | p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|
| net exposure, LP + hedge | $3k | $9k | $10k | $20k |
| LP on-chain position | $64k | $100k | $100k | $100k |

Five worst days:

| Day | Net | Fees | Inventory MTM | Hedge cost | SOL open→close | Range | Ann. vol |
|---|---:|---:|---:|---:|---:|---:|---:|
| 2026-08-22 | -13,436 | 7,097 | -16,629 | 3,843 | +0.1% | 13.5% | 177% |
| 2026-08-25 | -6,131 | 4,567 | -8,324 | 2,283 | -2.4% | 6.9% | 93% |
| 2026-08-23 | -4,861 | 3,172 | -6,316 | 1,628 | +1.7% | 5.7% | 91% |
| 2026-08-27 | -3,928 | 3,729 | -5,807 | 1,743 | +6.9% | 9.3% | 91% |
| 2026-08-28 | -3,861 | 3,683 | -5,825 | 1,625 | -4.6% | 7.3% | 85% |

### Sensitivity: toxic flow, tight hedge, $1M/day, one input changed per row

| Variant | Fee | Lag | Edge bps | Net/day | SD/day | Max DD | Filled |
|---|---:|---:|---:|---:|---:|---:|---:|
| reference (as headline) | 12 | 1000 ms | 7.71 | 669 | 546 | $3k | 96% |
| arb ticket x4 (2% of volume) | 8 | 400 ms | -1.59 | -261 | 1,075 | $31k | 92% |
| arb ticket x4 (2% of volume) | 8 | 1000 ms | -15.39 | -1,641 | 2,966 | $149k | 83% |
| arb ticket x4 (2% of volume) | 12 | 400 ms | 7.35 | 633 | 646 | $5k | 95% |
| arb ticket x4 (2% of volume) | 12 | 1000 ms | 2.61 | 159 | 1,342 | $16k | 86% |
| arb slot phase +200 ms | 12 | 1000 ms | 7.79 | 678 | 502 | $3k | 96% |
| whole tickets, every 1,778th from offset 0 | 12 | 1000 ms | 2.61 | 159 | 372 | $3k | 74% |
| whole tickets, every 1,778th from offset 592 | 12 | 1000 ms | 2.17 | 115 | 442 | $4k | 75% |
| whole tickets, every 1,778th from offset 1,185 | 12 | 1000 ms | 2.37 | 135 | 440 | $5k | 77% |
| whole tickets, every 1,778th from offset 0 | 8 | 400 ms | -0.06 | -107 | 314 | $11k | 76% |
| whole tickets, every 1,778th from offset 592 | 8 | 400 ms | -0.08 | -110 | 339 | $10k | 77% |
| whole tickets, every 1,778th from offset 1,185 | 8 | 400 ms | 0.11 | -91 | 304 | $10k | 79% |
