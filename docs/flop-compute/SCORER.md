# FLOP mark-rule scorer

Measures the OpenRouter mark rule in [SPEC.md](SPEC.md). It changes nothing
live. Code: `flop-index/` (the rule itself, shared with the pusher, and the
`flop-score` binary).

## One command, one number

    flop-index/pins/fetch.sh 2026-11-11 <dir>        # once: download the pin
    cargo run --release --manifest-path flop-index/Cargo.toml --bin flop-score -- \
      --data <dir> --rule <name> --split tune|heldout [--seed N]

The first line is the number: `gain <g> se <s> n <days>`. Gain is the
baseline's loss minus the candidate's on the same days, so positive means the
candidate is better. Then each part alone. Exit 0 means it ran; any other
exit prints `DID NOT RUN` and why. `flop-score list` names the rules.

**Gate** ([SPEC.md](SPEC.md), "Changing the mark rule"): a candidate is
recommended only if, on `--split heldout`, gain > 2 x se, and a reviewer on
a different model does not veto it.

## What it charges

Each day, in log units (0.01 is about 1%):

- **jump**: how much each mark move differs from the move of the median
  listed price, between consecutive snapshots.
- **attack**: one anchor a day, four injected scenarios, each lasting 24 h
  and measured for 24 h after: a spoofed listing at ~1% of the median
  (0.5-2%) under a real provider name from elsewhere in the data, one
  provider's promo at ~0.1x (0.05-0.2x), half the providers down, one
  provider relisting at ~10x (5-20x). Loss is the mean |ln(attacked mark /
  clean mark)|. Victims and strengths are drawn from the seed.
- **follow**: a genuine move, every provider about 1.25-3x up or down
  (from the seed; each provider within +-10% of it, moving at its own time
  in the first 6 h); loss is the mean distance from where the mark should
  be (the clean mark times the move), from 6 h to 48 h, beyond the +-10% band (a rule that follows at once
  pays 0). It weighs as much
  as the four attacks together: a mark that doesn't move is not an index. Added 2026-11-11
  after swarm round 1: a mark capped near 2.4%/day passed the gate (+0.213)
  by ignoring everything; with follow it scores clearly negative. The
  range reaches 3x because real cuts do (a disprover showed a 24 h time
  filter lagging a 2.5x cut by a day).
- **reference**: distance from the leave-one-provider-out median.

Day loss = jump + mean of the four attacks + follow + reference, each averaged over
the models scored that day. Standard error is over days, Newey-West
corrected (attack windows span two days and rules carry state). A snapshot
is scored when it has 2+ up providers. A rule that
holds (returns no price) keeps its last mark; no mark at all costs 1.0.

The report also prints `ex-ref`: jump + mean attack + follow, paired per
day with its own standard error. It is advisory; the gate still uses the
total. The reference part is a median, so it favours median-like rules by
construction; a gain that lives only there is suspect.

**Held out:** the last 30% of scored days (`--split heldout`). Tune on
`--split tune` only. Rules replay the whole history, so the last tune day's
attack window and the first held-out jump read one day across the line.

**Daily-data caveat:** on daily pins an attack covers one snapshot, so a
time filter or a multi-day smoother clears most of the attack term almost
for free. A pass by such a rule needs confirming on recorder data, where an
attack covers 288 snapshots. The reverse holds for follow: on daily data only the
+24 h snapshot is charged, so lagging one snapshot pays the whole move; on
5-minute data it pays about 1/500 of it.

## Data

A pin is a folder with a `MANIFEST` of `sha256 loader file`. The scorer reads
only listed files and stops if any hash differs. `flop-score pin <dir>
loader:file ...` makes one and never overwrites it.

Pin 2026-11-11 (`flop-index/pins/2026-11-11/`, files not committed):

| File | Source | Used rows |
|---|---|---|
| romanshumy-price-history.csv | romanshumy/llm-prices-data @3517cbd, MIT | daily price per model per provider |
| sophie-history.csv | sophie-nguyenthuthuy/llm-pricing-history @9711a88, licence unclear: analysed, not vendored | LiteLLM per-provider price changes, replayed daily |

219 model series (one per model per source) with 2+ providers, 14,348 daily snapshots, 2024-02 to 2026-11-10
(daily from late June). Neither source has quantization or uptime: every
endpoint is "unknown" and up, so the bf16/fp16 set order is not exercised.

Spot-check, 2026-11-11, 10 pinned rows against OpenRouter's live listing: 3
match (GMI 0.45/3.2; OpenRouter's model price for qwen3.8-27b and
qwen3.6-27b), 5 differ (DeepInfra pinned 0.40/3.00, listed 0.15/1.875; also
Nebius, StreamLake, Cloudflare gateway, Alibaba), 2 are not on OpenRouter
(Groq, WandB). The LiteLLM rows lag the listings. So this data measures the
rule on realistic endpoint sets, not on OpenRouter's own history.

Recorder data (`recorder-jsonl`, 5-minute OpenRouter snapshots with
quantization and status) replaces it as it accumulates: pin a dated copy of
the day files and score the same way.

## Results, pin 2026-11-11, held out (2026-10-13 to 11-10, 29 days)

Scored against SPEC as it stood on 2026-11-11, which clamped sets of 2. Since 2026-11-12 the clamp needs 3 endpoints (SPEC, cheap-outlier clamp), so these numbers are for the old baseline until the scorer is rerun.

| Rule | gain | se | Note |
|---|---|---|---|
| spec (baseline) | 0.000 | 0.000 | itself |
| mean (no clamp, no sets) | +0.024 | 0.004 | better overall; worse on 10x relisting and jumps |
| spec-median (SPEC's `or_weighting = median`) | +0.116 | 0.003 | worse on relisting |

Seed 2 gives the same to 0.001. The baseline's own losses: jump 0.003,
spoof 0.20, promo 0.36, down 0.03, relist 0.34, reference 0.11.

So the brief's last check, "a plain mean scores clearly worse", does not
hold overall: the mean is worse only on relisting and jumps. The scorer
was not reshaped to make it hold.

The relist loss came from the clamp as it then stood. With two endpoints, one
relisting at 10x lifted the median, the half-median floor rose past the other
endpoint, and that cheap endpoint was clamped up: on SPEC's bf16 pair
(DeepInfra $0.2578, GMICloud $0.6219), GMICloud at 10x moved the mark from
$0.311 to $1.911. Since 2026-11-12 a pair is not clamped and the same
relisting gives $0.268. Test:
`the_live_bf16_pair_relisting_at_10x_no_longer_lifts_the_mark`.

## Hold candidates, recorder data so far (2026-11-13, not a gate result)

`hold-2h`, `hold-6h`, `hold-12h`: SPEC with a missing endpoint held at its
last up price (SPEC.md, "Missing endpoints"). On daily pins they equal `spec`
(snapshots are 24 h apart). On the recorder's first 539 snapshots
(2026-11-11 11:08 to 11-13, one model, `--split all`, seed 1):

| Rule | gain | se | down | jump |
|---|---|---|---|---|
| hold-2h | +0.005 | 0.007 | +0.035 | -0.0005 |
| hold-6h | +0.346 | 0.215 | +0.178 | -0.0007 |
| hold-12h | +0.366 | 0.228 | +0.188 | -0.0007 |

All with the 1 h confirm. The jump part is slightly worse for every length;
not yet explained. The "down" scenario downs endpoints at their current
price, so these gains say nothing about a provider that lists a new price
and then goes down: add that scenario before choosing.

Three days, so no rule clears 2 x se and there is no held-out split yet.
Rerun on a pin of the first full week (complete 2026-11-18) before choosing.

## Adding a candidate

Write a struct implementing `Rule` in `flop-index/src/score/candidates.rs`
and add one line to `ALL`. A rule sees every endpoint at each snapshot (down
ones too) and may keep state between snapshots (time filters, smoothing).
