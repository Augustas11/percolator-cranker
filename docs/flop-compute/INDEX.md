# FLOP compute perp: index v0 (devnet demo)

A perpetual future on the price of AI inference compute, as settled on FLOP
Network. It runs on Solana devnet in Percolator, the market lives on our devnet
wrapper, and its mark comes from FLOP's on-chain settlements. FLOP is only the
price source: it has no smart contracts, so the market can't live there.

**This is a devnet demo. It is not a product, takes no real money, and is not
audited.** See "What this is not" below.

## The index

**Input.** Every realized compute-channel settlement FLOP reports
(`flop_list_settlements` on FLOP's read-only MCP server), with:

- `marketKind` = `spot` (forwards are left out);
- `settlementKind` = `settled`;
- `selfTrade` = false;
- positive `aggregateGn` (compute delivered) and positive `escrow`.

**Value.** The GN-weighted median of the buyer's price, `escrow / aggregateGn`
(FLOP per G_n), over those settlements in the last 6 hours. Sort the prints by price and take the lowest price at
which half of the window's GN is covered. A median ignores one odd print;
weighting by GN means a large job counts more than a tiny one.

**Units.** USD per 1M tokens of Qwen3.8-27B, times 1e6 on chain (owner
decision, 2026-11-11). What buyers and miners hedge is the dollar price of
compute; a FLOP-denominated index would mostly mirror the token price upside
down, because miners reprice to stay competitive in dollars.

`mark = (FLOP per G_n) x 52e6 G_n per 1M tokens (52 GF/token) x FLOP/USD`.

There is no FLOP/USD market until mainnet, so FLOP/USD is a configured
**stand-in**, derived from config rather than typed: the rate at which the
anchor 0.003 FLOP per G_n prices 1M tokens at OpenRouter's qwen/qwen3.8-27b
price (DeepInfra BF16, matching FLOP's weights) blended at
`flop_blend_in_out` = 15 input tokens per output token. That is OpenRouter's
measured mix (State of AI, arXiv 2601.10088). The blend is (15 x $0.15 +
$1.875) / 16 = $0.2578 per 1M, so FLOP/USD = 0.0000016527 and the opening
mark is about $0.26 (owner decisions, 2026-11-11). Change the blend or the
reference prices in config and the stand-in follows. At mainnet the stand-in is replaced by a live FLOP/USD feed.

**All models.** The index uses every model's settlements, priced per G_n;
there is no model filter. Qwen3.8-27B is only the benchmark unit (like quoting
oil in Brent): it has had 15 settlements ever, so a feed filtered to it would
be permanently stale.

**Conversion table.** The bots log a table (`flop table: ...` in `bots.log`)
whenever the mark changes, computed from the live mark and FLOP's model
registry (`gflopsPerToken` from `flop_get_model_pricing`, refreshed hourly):

| Quote | Formula | At a $0.2578 mark |
|---|---|---|
| Model M, $ per 1M tokens | mark x gflopsPerToken_M / 52 | |
| Qwen3.8-27B (52 GF/token) | mark | $0.2578 |
| Qwen3-8B (17 GF/token) | mark x 17 / 52 | $0.0843 |
| Qwen2.5-0.5B (2 GF/token) | mark x 2 / 52 | $0.0099 |
| Pure compute, $ per exaFLOP (1e9 G_n) | mark / 0.052 | $4.96 |

The last column is an example; the log line always uses the live mark.

**Refresh.** The pusher refetches every 60 s and pushes on a move or a
heartbeat, like the SOL-PERP pusher.

## Staleness

The index is **stale**, and the pusher sends nothing, when:

- the 6-hour window holds fewer than 20 prints, or fewer than 3 distinct miners; or
- the newest print is more than 30 minutes old; or
- FLOP's endpoint fails or returns something we can't parse.

When the index is stale, the on-chain mark just ages. There is no fallback:
we never substitute FLOP's nominal gateway prices (`api.flop.network/v1/models`,
tagged `devnet-nominal`), a last-good value, or a guess. Opening a trade
requires a mark whose newest print is under ~32 minutes old. That check uses
the print's time, not when FLOP was last read, so if FLOP's endpoint breaks,
opens can continue at the last mark for up to ~32 minutes. If the index stays
stale for about 24 hours (216,000 slots), anyone can resolve the market under
the wrapper's permissionless stale-resolve rule.

Our own replay work on SOL-PERP showed that stalls are where a perp loses
money, so the stale rule matters more than the price formula.

## The data today, honestly

On 2026-11-10 the latest 300 spot settlements (about 7.5 hours) had:

- about 35 settlements an hour, so the index is fresh;
- only 2 distinct prices, 0.002970 and 0.002881 FLOP per Gflop;
- one buyer address on 280 of the 300 (buyer HHI 0.88), across 62 miners.

So today's index tracks one buyer's posted price, not a market-clearing price
for compute. FLOP's own reference index (FRCR on the explorer) doesn't publish
for this reason: it has volume and concentration floors, and they fail. Index
v0 has **no buyer-concentration floor**, by the owner's choice, so the demo
has a mark to trade against. Expect the mark to sit flat. When a second real
buyer shows up, the same index starts to mean something, and adding a floor
(for example a cap on any one buyer's share of GN) becomes the obvious v1.

We won't use our own FLOP miner to make the index look liquid. FLOP flags
only buyer-equals-miner settlements as self-trades, so if our miner sells to a
third-party buyer those prints would count; if that starts, its address gets
filtered out explicitly.

Two more caveats come from FLOP's own repos:

- The index uses the buyer's price, `escrow / aggregateGn`, not
  `pricePerGflop`, which is what the miner was paid (`minerPaid / aggregateGn`,
  about 1% less). A reviewer on `flop-labs/yellowpaper` #208 argues
  that today's per-settlement figures reflect reservation terms and an
  administratively set registry price, not a negotiated price.
- FLOP Labs' decision record, as quoted in yellowpaper #208 and #223, says
  derivatives should settle on an external venue against a published
  benchmark, "never on a thin index." This demo is exactly a thin index. That's
  fine for showing the plumbing, but it is not what FLOP Labs says a real
  compute derivative should settle on.

## The market

`launch/flop-compute.toml`, launched with the same steps and bots as SOL-PERP
(`scripts/launch_market.sh`):

- AuthMark: our pusher holds the oracle authority and pushes the index. That
  key can set any mark, which is acceptable only for a devnet demo.
- Collateral is our devnet test mint, and PnL is in USD per 1M tokens. The
  FLOP/USD leg is a stand-in, so the mark moves only with FLOP per G_n.
- 2x leverage (50% initial, 25% maintenance), because the index is thin and
  gappy. The mark may move 20 bps per slot, so a ~3% jump between FLOP's few
  price levels is followed in about 15 slots (~6 s).
- The permissionless asset-init fee stays 0, as on SOL-PERP.
- Trades: the bots' off-chain matcher co-signs bilateral trades between the
  run's LP and test-trader wallets. There is no hedge venue for compute and no
  on-chain matcher.
- The keeper (drip) cranks it like any other Percolator market.

## Running it

```bash
scripts/launch_market.sh launch/flop-compute.toml devnet   # run dir launch-runs/flop-compute-devnet
LAUNCH_RUN_DIR=launch-runs/flop-compute-devnet scripts/launch_market.sh stop devnet
```

The monitor's alert thresholds (`monitor/alerts.yml`) are SOL-PERP's: a mark
lag over 400 ms and price gaps over 10 s. On this market both fire all the
time, so its monitor is not wired to alerting.

## What this is not

- Not on mainnet, not with real money, not a service anyone should rely on.
- Not a market-clearing compute price yet. It is one buyer's price until
  FLOP has more demand.
- Not trustless. One oracle key sets the mark, and Percolator has no official
  mainnet program and no audit.
- Not FLOP's FRCR index. Their methodology (`frcr-methodology-v0.md`) isn't
  published; this is our own, simpler definition.
- Not a change to SOL-PERP, which stays our only production target.
