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
- **follow**: a genuine move, every provider 1.25-2x up or down (from the
  seed) from the anchor on; loss is the mean distance from where the mark
  should be (the clean mark times the move) over 48 h. Added 2026-11-11
  after swarm round 1: a mark capped near 2.4%/day passed the gate (+0.213)
  by ignoring everything; with follow it scores -0.206.
- **reference**: distance from the leave-one-provider-out median.

Day loss = jump + mean of the four attacks + follow + reference, each averaged over
the models scored that day. Standard error is over days, Newey-West
corrected (attack windows span two days and rules carry state). A snapshot
is scored when it has 2+ up providers. A rule that
holds (returns no price) keeps its last mark; no mark at all costs 1.0.

**Held out:** the last 30% of scored days (`--split heldout`). Tune on
`--split tune` only. Rules replay the whole history, so the last tune day's
attack window and the first held-out jump read one day across the line.

**Daily-data caveat:** on daily pins an attack covers one snapshot, so a
time filter or a multi-day smoother clears most of the attack term almost
for free. A pass by such a rule needs confirming on recorder data, where an
attack covers 288 snapshots.

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

The relist loss comes from the clamp. With two endpoints, one relisting at
10x lifts the median, the half-median floor rises past the other endpoint,
and that cheap endpoint is clamped up. On SPEC's bf16 pair (DeepInfra
$0.2578, GMICloud $0.6219), GMICloud at 10x moves the mark from $0.311 to
$1.911. Without the clamp it would go to $0.268. Test:
`the_live_bf16_pair_relisting_at_10x_lifts_the_mark_6x`.

## Adding a candidate

Write a struct implementing `Rule` in `flop-index/src/score/candidates.rs`
and add one line to `ALL`. A rule sees every endpoint at each snapshot (down
ones too) and may keep state between snapshots (time filters, smoothing).
