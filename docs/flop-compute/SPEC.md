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
| Mark (`openrouter`) | OpenRouter endpoints list for qwen/qwen3.8-27b: each up endpoint's 15:1 blend, weighted 1/price² | 5 min | no successful fetch for 30 min |
| FLOP side (check; mark under `flop_standin`) | FLOP Network spot settlements, all models, 6 h window, GN-weighted median of escrow / aggregateGn | 60 s | < 20 prints, < 3 miners, < 3 buyers, or newest print > 30 min old |

**How endpoints combine** (`or_weighting = inverse_square`, decision
2026-11-11). Each up endpoint (OpenRouter status 0) has a blended price
`b = (15 x prompt + completion) / 16` per 1M tokens and weight `w = 1 / b²`;
the mark is `sum(w x b) / sum(w)`. That mirrors OpenRouter's default routing,
which spreads requests over stable providers by inverse square price. List
prices carry no volume, so a plain median gives a little-used expensive
provider the same say as the cheap one buyers actually route to; the weighting
puts the mark where buyers' money goes, like an oil benchmark priced where the
marginal barrel trades rather than the average of advertised offers. On
2026-11-11 the bf16 pair (DeepInfra $0.2578, GMICloud $0.6219) gives $0.311.
One endpoint alone is its own price; a new expensive endpoint barely moves the
mark (a $5 one would add ~3%). `or_weighting = median` keeps the old method.

**Cheap-outlier clamp** (decision 2026-11-11: clamp, don't drop).
Under 1/price² weights a very cheap listing (a mispricing, a promo, a
near-zero completion price) would take almost all the weight. So before
weighting, any endpoint priced under half the set's median
(`or_min_frac_of_median` = 0.5) counts as half the median. Nothing is dropped
for being cheap. An earlier rule dropped such endpoints; with two bf16
endpoints that made the mark jump ~3x ($0.24 to $0.73) when DeepInfra's cut
went from 21% to 22%. With the clamp the mark moves smoothly with every price.

The clamp needs at least 3 up endpoints in the set (`or_clamp_min_endpoints`
= 3; decision 2026-11-12). Two points can't name an outlier: their
half-median floor is a quarter of their sum, so it lifted the cheaper real
price. On 2026-11-12 GMICloud bf16 went down, the feed fell through to
bf16+fp16 = DeepInfra $0.2578 and Cerebras $1.0212, the floor ($0.3198)
priced DeepInfra above its listing, and the mark went $0.3112 to $0.3824
(+23%) from an outage. A set of 2 now prices plain 1/price² over the listings
($0.3035 there). The cost: a 2-endpoint set has no promo guard, so a promo in
it moves the mark to near its own price. Each push logs `clamp: on` or
`clamp: off (n<3)` in `flop status` and the signed push log (`clamp`, with
`clamp_min_endpoints`). `clamp: on` means the guard checked the set; a set
name ending `-clamped` means it lifted a price. The clamp turning off logs
`flop alert: clamp off (n<3) ...: no promo guard on this set`, so a promo
in a 2-endpoint set is noticed by that alert. `staging verify-push-log`
refuses a log whose rule changes: from the first line with
`clamp_min_endpoints` on, every OpenRouter line must declare exactly the
shipped rule (guard, clamp minimum, floor fraction, weighting, minimum
endpoints, quantizations, blend). Earlier lines verify under the rule they
declare. The cutover is the first new-style line in the log, not a time: a
log holding only old-style lines verifies under the old rule.

With 3 or more endpoints an outage can still move the floor: the median moves
when one goes down, and a price near the floor can be lifted to it (test
`with_three_or_more_an_outage_can_still_move_the_floor`: $0.29 to $0.30).
That lift is bounded by half the median and is the price of any median floor.

Known limit (decision 2026-11-11: keep this rule for now). Promos still
move the mark. One $0.01 promo next to today's bf16 pair makes the median
$0.2578 and the floor $0.1289, which 1/price² gives about 77% of the weight:
the mark goes $0.311 to about $0.170. Two promos take it to about $0.076 (the
median itself is then cheap). A floor anchored to the last mark doesn't help:
a clamped promo still carries 1/floor² weight and the floor follows the mark
down, so the mark reaches the promo price within a few pushes. Planned fix if
promos appear: a time filter, under which a listing priced under 0.5x the last
mark counts only after it has stayed there for 24 h. Until then, every change
in which endpoints are clamped logs a `flop alert: clamped ...` line (and
`flop alert: no endpoint clamped` when it clears), so the first promo is
noticed.

**Missing endpoints** (decision 2026-11-13: rule approved, off until
its length is chosen). An endpoint that goes down, loses its price or leaves
the listing keeps counting, as up and at the price it last had while up, for
`or_hold_missing_s`; after that it is dropped as before. One that comes back
counts at its new price at once, and a price shown while down is ignored.
Only a price the endpoint has been seen up at for `or_hold_confirm_s` (1 h)
is held: otherwise a provider could list any price for one fetch, go down,
and have it counted for the whole hold without selling at it. An endpoint
that goes down sooner is dropped, as with no hold. A provider with two
endpoints in one quantization is not held (its rows can't be told apart
between fetches). Why:
in the recorder's first 44 h (2026-11-11 to 11-13) no bf16 or fp16 price
changed, but GMICloud's bf16 endpoint was down 44% of the time in 15 spells
(median 30 min, longest 6 h 15 min). Each spell moved the pricing set from the
bf16 pair ($0.3112) to DeepInfra with Cerebras fp16 ($0.3035), a 2.5% step
either way, 29 times, with no price change behind it. Cost of the rule: a real
exit reaches the mark one hold late, and a provider that comes back for one
fetch in each hold keeps its confirmed price counted while mostly down. Limits: the hold is kept in memory, so
after a pusher restart nothing is held until an endpoint is seen up again; a
push line logs `hold_missing_s`, the held endpoints (`held`) and the listing
as priced, so the mark recomputes from the line, but the line alone does not
prove the endpoint was up when `held` says. The scorer's "down" scenario takes
endpoints down at their current price, the case the hold is built for; it
has no "new price, then down" scenario yet, and one must be added and scored
before the hold is turned on. A change in what is held logs
`flop alert: held at last up price: ...`. The shipped value is 0 (off): 2 h,
6 h and 12 h are scored as `hold-2h`, `hold-6h` and `hold-12h`
([SCORER.md](SCORER.md)) on the recorder's first full week, and the length is
set then. Code: `flop_index::Holder`.

Inverse-square weighting has one known exception to "a cheaper provider never
raises the mark": a cut by an endpoint priced over 2x the mark raises it a
little, because the cheaper expensive endpoint wins weight. Example, bf16 at
$0.2578, $0.6219 and $3.00: the set's median is $0.6219, so the $0.2578 one is
clamped to $0.3109, and cutting the $3.00 one to $0.80 moves the mark $0.3955
to $0.4192 (+6%). Without the clamp the same cut is $0.328 to $0.351 (+7%). The
test `the_spec_example_cut_raises_the_mark_a_little` pins these numbers. Cuts
by endpoints under 2x the mark, which today is both bf16 endpoints, never
raise it.

**Sets, in order** (decision 2026-11-11: fall through, don't hold).
The bots try bf16 endpoints alone, then fp16 alone, then bf16 and fp16
together, then every up endpoint, and price from the first set with at least
2 up endpoints (then clamp, if it has 3 or more, and weight inside it). So the next set is used only
when endpoints are down or missing, never because one got cheap. The mark
holds (stale) only when every set has fewer than 2. A push from bf16+fp16 or
from every endpoint may blend quantizations. Each push logs its set in
`bots.log` (`flop status`) and in the signed push log (`set`, with
`-clamped` when the clamp applied), plus each used endpoint's listed and
clamped price (`used`). The verifier checks the set and those prices against
the logged endpoints. Lines pushed before the clamp have no `guard` field and
verify under the old drop rule; lines pushed before 2026-11-12 have no
`clamp_min_endpoints` and verify under the clamp on any set. There is no fallback to a last-good value past 30 minutes, to FLOP's
nominal gateway prices, or to a guess. A stale index pushes nothing and the
on-chain mark ages.

`flop_mark_source = flop_standin` switches the mark to
`FLOP side x 52e6 x FLOP/USD`, with FLOP/USD a configured stand-in until a live
FLOP/USD feed exists. The bots log the FLOP-implied FLOP/USD
(`OpenRouter $ / (FLOP per G_n x 52e6)`) on every change either way.

Open question, G_n per token. Our own paid gateway job (2026-11-11, block
3,450,756: Qwen2.5-0.5B, 75 tokens) settled 74 G_n, about 1 per token, though
the registry lists 2 GF/token and the gateway reserved at 2. That is one
sample, so no constant changes. If it holds, settled G_n per token runs about
half the registry figure, and the FLOP-implied FLOP/USD above (which assumes
52e6 G_n per 1M Qwen3.8-27B tokens) may be off by about 2x.

FLOP receipts on devnet carry `flopUsd: "1"`, `flopUsdSource:
"devnet-nominal"`. That is FLOP's devnet placeholder, not a price, and is
never used as an FX source.

## Changing the mark rule

The rule above is scored by `flop-score` ([SCORER.md](SCORER.md)). A
candidate rule is recommended only if, on the held-out days
(`--split heldout`, the last 30% of scored days, never used for tuning), its
paired gain over the current rule is above twice its standard error, and an
independent review has not vetoed it. A recommendation comes with
the numbers. It does not change the live rule; that takes a reviewed change to this spec.

## Manipulation limits (FLOP side)

- No buyer and no miner carries more than 45% of the window's weight
  (`flop_max_agent_share`, `flop_max_miner_share`). A larger one is scaled down
  to the cap, not dropped, so a flood stays a minority and can't set the
  median, priced high or low. The cap must be under 50%: at exactly half a
  low-priced flood would tie everyone else and the lower median would take
  its price.
- At least 3 distinct buyers and 3 distinct miners, or stale.
- Buyers are counted per on-chain agent. Gateway API keys are off-chain
  allowances, so every gateway customer settles under one agent
  (`flop_gateway_agent`, 5GsXb3TmNGixuEsbtixqHnqWr9UQJ2M8bkqBQWteNekgF98S).
  The buyer count and the 45% cap see it as one buyer however many customers
  sit behind it. That is a main reason this leg is stale. The logic is
  unchanged. The status line and each signed push line (`flop_side.window`;
  pushes happen in openrouter mode even when this leg is stale) log the
  window's prints, on-chain buyers and the gateway agent's share of G_n.
  `window` is null before the first good FLOP read.
- The caps are per address. One party running several buyer or miner
  addresses is not caught.
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
its logged endpoints. Any mismatch fails. The oracle pubkey is required:
each line names its signer, so without it a self-signed forgery would pass.
What it proves: each logged mark matches the endpoints logged with it, signed
by that key. It does not detect dropped or reordered lines, and does not
check the mark against the transaction on chain. Compare the pubkey with the asset's
oracle authority on chain, and a line's `sig` with the transaction on chain.

## Risk settings (engine master)

| Setting | Value | Why |
|---|---|---|
| Initial margin | 50% (2x) | Thin, gappy index; wider than the 20% floor asked for |
| Maintenance margin | 25% | |
| `max_price_move_bps_per_slot` | 20 | The mark walks to a new value; a 40% jump (a provider leaving the set) takes ~200 slots, ~90 s |
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
older than 30 minutes. Stale for 2 hours: the operator decides whether to
drain. Stale for ~24 hours: anyone can resolve the market (permissionless
stale resolve).

**Open-interest caps.** None configurable on engine master beyond the engine
constants. The bots' own LP inventory limit (`max_lp_inventory_usd`) is the cap.

## Market maker (devnet demo)

The bots' LP trades against a test trader every few minutes, as a minimal
market maker:

- Small: 100 x 1M tokens a fill (~$31), at most $100 a fill and $500 of LP
  inventory.
- Pulls quotes when stale: no opens or closes while the mark is older than
  30 minutes.
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

## Long-term direction (nothing built yet)

Move weight to FLOP's own settled prices as independent buyers appear there.
Once the perp has its own order flow, the index anchors funding and the
perp's bids and asks set the price.

## What this is not

- Not a product. Devnet only; no real money; no mainnet market.
- Not audited, and not an offer to trade anything.
- Not FLOP Network's or OpenRouter's product; neither endorses it. They are
  price sources read through their public endpoints.
- Not a price for any one provider: the mark is a weighted average across endpoints.
