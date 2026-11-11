# FLOP compute perp: contract spec (devnet demo)

A perpetual future on the dollar price of AI inference. Devnet only, test
money, not audited. How the index is built in detail: [INDEX.md](INDEX.md).

## Unit

**USD per 1M tokens of the benchmark model**, times 1e6 on chain. One
contract (`size_q` = 1e6) is 1M benchmark tokens.

- **Benchmark:** Qwen3.8-27B at 52 GFLOP per token (FLOP's model registry,
  `gflopsPerToken`). 52 GF/token x 1M tokens = 52e6 G_n.
- **Mix:** 15 input tokens per output token (`flop_blend_in_out`), OpenRouter's
  measured mix. A blended price is `(15 x input + output) / 16`.
- **Other models:** `$ per 1M tokens of model M = mark x gflopsPerToken_M / 52`.
- **Pure compute:** `$ per exaFLOP = mark / 0.052`.

## Price sources

| Leg | Source | Refresh | Stale when |
|---|---|---|---|
| Mark (`openrouter`) | OpenRouter endpoints list for qwen/qwen3.8-27b: median of each endpoint's 15:1 blend | 5 min | no successful fetch for 30 min |
| FLOP side (check; mark under `flop_standin`) | FLOP Network spot settlements, all models, 6 h window, GN-weighted median of escrow / aggregateGn | 60 s | < 20 prints, < 3 miners, < 3 buyers, or newest print > 30 min old |

**Two-endpoint caveat.** On 2026-11-11 only two endpoints serve bf16
(DeepInfra $0.2578, GMICloud $0.6219), so the median is their average, $0.44
(owner decision: keep it). Either provider repricing moves the mark, by half
its own change; one of them leaving drops the mark to the fp16 tier or to all
endpoints.

**Fallbacks, in order, for the OpenRouter leg:** bf16 endpoints if at least
2; else fp16 if at least 2; else bf16 and fp16 together if at least 2; else
every endpoint. There is no fallback to a last-good value past 30 minutes, to
FLOP's nominal gateway prices, or to a guess. A stale index pushes nothing and
the on-chain mark ages.

`flop_mark_source = flop_standin` switches the mark to
`FLOP side x 52e6 x FLOP/USD`, with FLOP/USD a configured stand-in until a live
FLOP/USD feed exists. The bots log the FLOP-implied FLOP/USD
(`OpenRouter $ / (FLOP per G_n x 52e6)`) on every change either way.

## Manipulation limits (FLOP side)

- No buyer and no miner carries more than 50% of the window's weight
  (`flop_max_agent_share`, `flop_max_miner_share`). A larger one is scaled down
  to the cap, not dropped, so a flood can at most tie everyone else.
- At least 3 distinct buyers and 3 distinct miners, or stale.
- Self-trades, forwards and unsettled rows are excluded.

## Checking the pusher

Every landed push writes one JSON line to the bots log
(`"event":"flop_push"`): market, slot, transaction signature, mark, source
time, and the inputs it was made from (for the OpenRouter leg, every endpoint
fetched with its prices, the blend, the quantization tiers). The line is
signed by the oracle authority key, the same key the market accepts marks
from. To check a log:

    staging verify-push-log bots.log <oracle authority pubkey>

It checks each signature and signer, and recomputes each OpenRouter mark from
its logged endpoints. Any mismatch fails. Compare the pubkey with the asset's
oracle authority on chain, and a line's `sig` with the transaction on chain.

## Risk settings (engine master)

| Setting | Value | Why |
|---|---|---|
| Initial margin | 50% (2x) | Thin, gappy index; wider than the 20% floor asked for |
| Maintenance margin | 25% | |
| `max_price_move_bps_per_slot` | 20 | The mark walks to a new value; a 40% jump (a provider leaving the median) takes ~200 slots, ~90 s |
| `max_accrual_dt_slots` | 64 | Loss budget 20 x 64 = 12.8% plus funding < 25% maintenance, as the engine's solvency check wants |
| `max_abs_funding_e9_per_slot` | 10,000 | Engine maximum |

**Funding.** Engine master has no funding instruction. Each accrual it charges
a premium of `(mark - index) / index` per slot, capped at the max above, where
mark is the last pushed mark and index is the engine's effective price walking
toward it. Here both come from the same oracle push, so funding is near zero
except while the effective price is catching up. There is no separate index
feed to peg to at this pin.

**Reduce-only.** Engine master has a per-asset drain-only switch
(`UpdateAssetLifecycle`, action 1, signed by the market authority): no new
risk, closes still work. It is **one-way**: there is no path from drain-only
back to active. So the bots do not flip it automatically on staleness. Stale
for any time: no pushes, the mark ages, opens are refused once the mark is
older than ~32 minutes. Stale for 2 hours: the operator decides whether to
drain. Stale for ~24 hours: anyone can resolve the market (permissionless
stale resolve).

**Open-interest caps.** None configurable on engine master beyond the engine
constants. The bots' own LP inventory limit (`max_lp_inventory_usd`) is the cap.

## Market maker (devnet demo)

The bots' LP trades against a test trader every few minutes, as a minimal
market maker:

- Small: 100 x 1M tokens a fill (~$44), at most $100 a fill and $500 of LP
  inventory.
- Pulls quotes when stale: no opens or closes while the mark is older than
  ~32 minutes.
- Kill switch: while `/run-dir/KILL` exists, no trades at all.
- Logs one `"event":"mm_fill"` JSON line per fill (size, mark, quote, LP
  inventory).

Gap: at this engine pin a co-signed fill books at the oracle mark, and the
quote price only sizes the fee. A real +-1% two-sided quote that outside
takers can hit needs a matcher that prices off the mark, which engine master
and our matcher do not have. The quote stays at mark +-10 bps.

## Rollover

If the benchmark changes (a new model or a new FLOP registry value), the unit
changes by `gflopsPerToken_new / 52`. Open positions are not converted: the
old market is resolved and a new market lists in the new unit, with the
conversion `new mark = old mark x gflopsPerToken_new / 52` published at the
switch.

## What this is not

- Not a product. Devnet only; no real money; no mainnet market.
- Not audited, and not an offer to trade anything.
- Not FLOP Network's or OpenRouter's product; neither endorses it. They are
  price sources read through their public endpoints.
- Not a price for any one provider: the mark is a median across endpoints.
