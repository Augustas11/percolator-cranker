# Compute price feed (flop-feed)

Read-only JSON for the site's `/compute` page, built from the pusher's push
log, the OpenRouter recorder and, optionally, the wide collector (about 20
open models, every seller).

**Trust.** The feed trusts the log file. It keeps only lines whose `oracle`
is the `--oracle` pubkey and ignores lines landed more than 5 minutes in the
future, but it does not check signatures (that needs ed25519 and base58 deps
this Solana-free crate avoids). To check the log, run
`staging verify-push-log bots.log <oracle pubkey>` (SPEC.md, "Checking the
pusher").

## Run

    cargo build --release --manifest-path flop-index/Cargo.toml --bin flop-feed
    flop-feed <bots.log> <or-prices dir> <out dir> --oracle <pubkey>            # once
    flop-feed <bots.log> <or-prices dir> <out dir> --oracle <pubkey> --loop 60  # every 60 s
    flop-feed <bots.log> <or-prices dir> <out dir> --oracle <pubkey> --wide <dir> --loop 60

- `bots.log`: the bots' log. Only lines with `"event":"flop_push"` are used;
  everything else, unparsable lines, lines over 1 MiB, lines from another
  oracle and a half-written last line are skipped.
- `or-prices dir`: the recorder's `YYYY-MM-DD.jsonl` files
  (`staging record-or`). Missing days or a missing directory mean no outage
  data, not an error.
- `--oracle`: the FLOP market's oracle authority pubkey (required).
- `--wide`: the wide collector's directory (optional): one subdirectory per
  model (`qwen__qwen3.8-27b` for `qwen/qwen3.8-27b`) holding
  `YYYY-MM-DD.jsonl` files, one line per fetch:
  `{"at_ms", "model", "rows": [{"provider", "tag", "quantization", "prompt", "completion", "status", "uptime_30m", "context"}]}`
  (prices in USD per token). Optional in the same directory: `hf-map.tsv`
  (`<openrouter id>\t<hugging face id>\t<slug>`) and `demand/YYYY-MM-DD.jsonl`
  (`{"at_ms", "model", "hf", "downloads_30d", "likes", "params", "config"}`,
  `config` from the model's `config.json`: `hidden_size`, `num_hidden_layers`,
  `vocab_size`, `experts_per_tok`, `experts`, `shared_experts` and others,
  any null); missing or odd
  ones mean no aliases or no demand data, not an error. Only directories
  named `<org>__<name>` are models. Adds `market.json`, and keeps the index state
  in `<out dir>/index-state.json`.
- `--rebase` (with `--wide`): ignore any saved index state and start a new
  base. Only for a deliberate re-base; the level then means something new.
- `--compute <file>` (with `--wide`, optional): one entry per line, `#`
  comments: `<openrouter id> <G_n per token>` (FLOP's registry value) or
  `<openrouter id> active_params <count>` (a mixture-of-experts model's
  active parameters). Added to (or overriding) the built-in entries
  `qwen/qwen3.8-27b 52` and `qwen/qwen3-8b 17` (SPEC.md, INDEX.md) and
  FLOP's DeepSeek-V3 anchor (below). Read once at start; a bad line stops
  the feed.
- `--flop-gn-price <FLOP per G_n>` (with `--wide`, optional): FLOP's price
  of one G_n, for `flop.implied_flop_usd`.
- `out dir`: gets `latest.json`, `history.json` and, with `--wide`,
  `market.json`. Each is written to a temp
  file in the same directory (named with the process id), synced, and
  renamed, so a reader never sees half a file.

The first pass streams the whole log once, keeping only each push's time,
mark and transaction signature. `--loop` then reads only bytes added since
the last complete line, and re-reads only recorder files whose size changed.
A log that got shorter or is a new file (rotated) is read again from the
start; pushes already read are kept, and a push seen twice (same `sig`)
counts once. One run exits
non-zero if the log has no push yet; `--loop` prints the error and retries.
`market.json` is written even when the log has no push yet, and the push
files even when the wide directory is empty.

With `--wide`, the feed loads `index-state.json` from the out dir and
continues from it. If there is none it creates a base. If it is there but
cannot be read or has bad values, the feed never re-bases silently: it
writes no `market.json`, logs the error every run and tries the file again
next run, while `latest.json` and `history.json` keep updating. `--rebase`
starts a fresh base instead. The state is written atomically each run,
before `market.json`.

One feed per out dir: the feed locks `<out dir>/.flop-feed.lock` for its
lifetime, and a second one exits with an error naming the lock.

The feed holds at most 30 days of wide data. Day files are
re-read only when their size changed. Day files more than 8 days old keep
one snapshot per model per hour.

## latest.json

From the newest push line.

| Field | Meaning |
|---|---|
| `v` | Format version, 1 |
| `generated_ms` | When the feed wrote this file |
| `mark_usd` | The mark: USD per 1M tokens of Qwen3.8-27B (`mark_e6` / 1e6) |
| `landed_ms` | When the push transaction landed |
| `source_ms` | When the OpenRouter list behind it was fetched |
| `sig`, `slot` | Solana devnet transaction of the push |
| `market`, `oracle` | Market and oracle authority from the line |
| `source` | `openrouter` (or `flop_standin`) |
| `set`, `clamp` | Which set priced the mark and the clamp state, as logged |
| `blend_in_out` | Input:output token mix (15) |
| `pricing_set[]` | Each endpoint used: `provider`, `quantization`, `listed`, `priced` (USD per 1M, blended), `up` |
| `sellers[]` | Every priced endpoint in the push, cheapest first: `provider`, `quantization`, `blended` (USD per 1M at `blend_in_out`), `up` (OpenRouter status 0), `in_set`, `tier` (quantization is a pricing tier, bf16/fp16) |
| `flop_side` | `flop_per_gn`, `prints`, `buyers`, `miners`, `gateway_share` (share of the window's G_n from FLOP's gateway agent), `stale` (reason, or null). Fields the line does not carry are null. |

## history.json

The last 7 days.

| Field | Meaning |
|---|---|
| `v`, `generated_ms` | As above |
| `from_ms`, `to_ms` | The window |
| `high`, `low` | Highest and lowest mark over every push in the window (null if none) |
| `points` | `[landed_ms, mark_usd]` pairs, oldest first, at most 600. The mark is a step, so these are the first and last push and the pushes on both sides of every change. Over 600, each equal time slice keeps its lowest and highest push. |
| `changes[]` | Each push whose mark differs from the one before: `t_ms`, `from`, `to`, `sig`; the newest 500 |
| `outages[]` | Per bf16/fp16 seller from the recorder: `provider`, `quantization`, `from_ms`, `to_ms` (null while still down). Down means status not 0 (a missing status counts as 0, as in the pusher), unpriced, or missing from a snapshot after it was first listed. |
| `recorder_gaps[]` | Recorder silences over 15 minutes: `from_ms`, `to_ms` (null if still silent) |

## market.json

The broad index, from the wide collector. Prices are USD per 1M tokens at a
15:1 input:output blend.

| Field | Meaning |
|---|---|
| `generated_ms` | When the feed wrote this file (not how fresh the data is) |
| `data_ms` | The newest snapshot read (null if none). The page marks itself stale from this. |
| `window` | `from_ms` (oldest snapshot held), `to_ms` (= `data_ms`); both null without data |
| `index.level` | The index now (100 at the base); null before a base exists |
| `index.base_ms` | Time of the base snapshot; null before a base exists |
| `index.change_24h_pct`, `index.change_7d_pct` | Change vs the level 24 h / 7 days ago; null without data that old |
| `index.n_models` | Models in the index now |
| `index.n_quotes`, `index.n_quotes_up` | Sellers listed in each model's latest fetch, all and up |
| `index.n_providers` | Distinct providers in those listings |
| `index.series` | `[t_ms, level]`, at most 500; both sides of every change |
| `models[]` | Per model, most up sellers first: `id` (shortest id of its alias group), `aliases` (the other ids), `name`, `family`, `data_ms` (its newest snapshot), `stale` (that is over 30 minutes old: no quote is reported up), `sellers_total`, `sellers_up`, `median`, `p25`, `p75` (over the index input's set, below), `min`, `max` (over quotes up now), `thin`, `quotes_held` (providers in the median's set held at their last up price), `cheapest` (`provider`, `quantization`, `price`; null if no quote is up and priced), `change_24h_pct` (same set as `median`), `demand` (below), `sellers_added_7d`, `series` (`[t_ms, median]`, at most 200), `quotes[]` (`provider`, `quantization`, `price` or null, `up`, `uptime_24h_pct`), cheapest first, unpriced next, down last |
| `models[].demand` | From the newest demand line of the model (or an alias): `hf_downloads_30d`, `hf_likes`, `as_of_ms`, and `hf_downloads_change_7d_pct` against the newest line at least 6 days older (null without one). All null without demand data. |
| `models[].compute` | `params` (newest known count from the demand lines, or null), `active_params` and `active_params_source` (`table`, `id`, `config_estimate`, or null), `gn_per_token`, `basis`: `flop_registry`, `2x_active_params`, `2x_active_params_est`, `2x_params_dense`, or null when unknown (then every G_n field of the model is null). `source` repeats `basis` and is deprecated: read `basis`; `source` will be removed once the page does. |
| `models[].usd_per_bn_gn` | `median` in USD per billion G_n; `usd_per_bn_gn_min`, `usd_per_bn_gn_max` likewise from `min`, `max` |
| `models[].mix` | `chat_3_1`, `avg_15_1`, `agent_150_1`: USD per 1M tokens at 3:1, 15:1, 150:1 input:output (`avg_15_1` = `median`); null with no set |
| `models[].out_in_ratio` | Median completion ÷ prompt price over the median's set; null if none has a prompt price above 0 |
| `models[].routed` | The routed price (below): `price` (null when no seller is up, or the model is `stale`), `sellers` (providers with a share above 0), `top[]` (up to 5 by share: `provider`, `quantization`, `price`, `share`), `change_1h_pct`, `change_24h_pct` (null without data that old), `uptime_fallback`, `capped`, `series` (`[t_ms, price]`, at most 300, both sides of every change) |
| `models[].routed_usd_per_bn_gn` | `routed.price` in USD per billion G_n; null when either is unknown |
| `models[].sellers_added_7d` | Quotes (provider, quantization) listed now that were not listed in the snapshot 7 days ago; null without data that old. Sellers adding a model is a supply-side read of demand. |
| `models` (empty) | With no wide data yet the file is still written, with `models: []`, null `data_ms`, `level` and `base_ms` |
| `compute_price` | `median_usd_per_bn_gn`, `p25`, `p75` of `usd_per_bn_gn` over models that are neither `thin` nor `stale` and have a finite one, and their count `n_models`, of which `n_estimated` use a config estimate; `n_unknown` counts the models neither `thin` nor `stale` left out for lack of a G_n figure; null with none |
| `mixes` | `chat_3_1`, `avg_15_1`, `agent_150_1`: the median of the models' `mix` (models neither `thin` nor `stale`) |
| `out_in_ratio_median` | The median of the models' `out_in_ratio` (models neither `thin` nor `stale`) |
| `routed` | The benchmark's routed price: `benchmark` (`qwen/qwen3.8-27b`), `price`, `usd_per_bn_gn`, `series` (as per model) |
| `flop` | `gn_price_flop` (from `--flop-gn-price`, or null), `basis_model` (`qwen/qwen3.8-27b`), `implied_flop_usd` (from the median; null without both, or while the basis model is `thin` or `stale`), `implied_flop_usd_routed` (`routed.usd_per_bn_gn` / 1e9 / `gn_price_flop`), `parity`: `market_usd_per_bn_gn` (= `routed.usd_per_bn_gn`), `flop_rate_flop_per_gn` (= `gn_price_flop`), `implied_flop_usd`, `implied_flop_usd_routed`, `as_of_ms` (the benchmark's newest snapshot), so FLOP's rate and the market's sit side by side |
| `demand_rank` | Model ids by `hf_downloads_30d`, highest first (models without it left out) |
| `providers[]` | `name`, `models`, `quotes` (in the latest listings), `uptime_24h_pct` (over all its quotes), `cheapest_in` (models where it is the cheapest up seller); most models first |
| `changes[]` | Price changes over the last 7 days, newest first, at most 200: `at_ms`, `model`, `provider`, `quantization`, `from`, `to` |
| `dynamic[]` | Quotes that change price too often to list: `model`, `provider`, `quantization`, `min`, `max` (over 7 days), `changes_24h`; most changes first |

## Broad index method

- **Price.** Each quote's price is (15 × prompt + completion) / 16 × 1e6.
  A quote is usable if it is up (status 0 or missing) and both prices are
  finite, not negative, and not both 0.
- **Aliases.** An id `X-<4, 6 or 8 digits>` is a dated snapshot of `X`, and
  one model with it, only if `X` is itself a model here and their Hugging
  Face ids (`hf-map.tsv`, case ignored) are equal or the dated one's is
  `X`'s plus the same suffix. So `deepseek/deepseek-v4-flash-0731`
  (`DeepSeek-V4-Flash-0731`) joins `deepseek/deepseek-v4-flash`
  (`DeepSeek-V4-Flash`), while `mistral-small-2501` and `-2603` (no
  `mistral-small`) or `Qwen3-235B-A22B-2507` (its own HF repo,
  `...-Instruct-2507`) stay separate releases. The model is named by `X`.
  Each fetch of a member gives the group a snapshot: every member's latest
  listing (from the last 24 h) merged. Everywhere, quotes are one per
  (provider, quantization): the usable one with the lowest price, else an up
  one. A provider listing several regional endpoints counts once. Totals
  count after this.
- **Hold.** A listing that goes down, loses its price or drops out of the
  listing keeps counting in its model's median, at the last price it was up
  at, for 6 hours after it was last seen up. If it comes back, it counts at
  its new price at once; if not, it leaves the median after 6 hours. Why:
  on the first day of data every one of the 36 model moves behind the 10
  largest 5-minute index moves was a seller flapping down or up, so the
  median jumped to a neighbouring price with no price change. Unlike the
  perp's hold there is no confirm period: most flaps here last 5 to 30
  minutes, which a 1-hour confirm would not cover, and one quote held in a
  median of many cannot move a reference index the way it could a mark.
  The hold is per listing too.
- **Model median.** At each fetch of a model, its set is one price per
  distinct provider: the lowest of that provider's usable and held listings
  across precisions, regions and aliases (`quotes` still lists every
  listing). The median is over that set (mean of the middle two for an even
  count); percentiles interpolate linearly. With fewer than 3 providers the
  last median is carried forward, and `thin` is true while that is the case
  now. The card's `median`, `p25`, `p75` and `change_24h_pct` use this same
  set, so they match the index input and each other; `quotes_held` says how
  many of its providers are held. `min`, `max` and `cheapest` are over
  quotes up now. A model that never had a median shows the median of what
  it has; with nothing, it is left out.
- **Stale.** A model whose newest snapshot is over 30 minutes old is
  `stale`: none of its quotes is reported up (`sellers_up` 0, `cheapest`
  null). The page uses the top-level `data_ms` for the whole file.
- **Snapshots.** The collector fetches the models one after another every 5
  minutes. Fetches less than 60 s apart, with no model twice, are one
  snapshot; its time is the last fetch in it.
- **Basket.** Every model directory. A model is in the index while it has a
  median and had at least 3 usable quotes in the last 24 h.
- **Base.** Set once, when no `index-state.json` exists: the first
  snapshot where every model priced within the first 24 h of the data has a
  median. The index is 100 there. The base never moves after that: not on a
  restart, not when data ages out of the 30 days held. Only `--rebase`
  starts a new one.
- **State.** `index-state.json` holds `base_ms`, each basket model's base
  median (after chain-link scaling) and last median, the last level and its
  snapshot time (`at_ms`), and the level series (30 days; past 8 days, one
  point per hour), and the method version. Each run steps only through
  snapshots after `at_ms` that are at least 2 minutes old, so a cycle still
  being written is complete. A snapshot arriving later for a time already
  stepped never changes a stepped level, and a model in the index keeps its
  last input until it has a newer snapshot.
- **Level.** Jevons over the models in the index: the geometric mean of
  (median / base median) × 100. A model that joins gets base = median /
  (level / 100), so the level does not move. When a model leaves (24 h
  without 3 usable quotes), the remaining bases are scaled by one common
  factor so the level at their last medians is unchanged. So after a gap
  (feed down for hours), the first snapshot's level comes from the saved
  bases and moves only by real price changes. A group whose listed aliases
  change (an alias appears or goes quiet) leaves and joins again in the same
  step, so its own move at that step is not counted.
- **Step cap.** Per step, a model's index input moves by at most 25% from
  its last input (its median ratio is clamped to 0.8–1.25); a bigger move
  is reached over several steps (2 → 10 takes 8 steps). Worst case: one
  absurd quote moves a model's median by at most one rank (with three
  sellers 1, 2, 3, a fourth at 1000 moves it from 2 to 2.5), and one model
  moves the level by at most 1.25^(1/n) per step: 25% with n = 1, 11.8%
  with n = 2, 0.41% with the 54 models in it on 2026-11-13.
- **Method changes.** When the saved state was written by another method
  version, once at start each basket model's input at the saved step is
  recomputed from the data and its base scaled by the same ratio, so the
  level does not jump. Never otherwise.
- **Uptime.** `uptime_24h_pct` is the share of the model's fetches in the
  last 24 h where the quote was listed and up. A fetch that did not list it
  counts as down; times the model was not fetched do not count.
- **Changes.** Price history is kept per listing as fetched: (OpenRouter
  model id, provider, endpoint tag), before alias merging. A listing's price
  changed if it differs from that listing's last price by more than 0.1%.
  Its first appearance is never a change, and a listing back from down at
  the same price is not one. Two alias listings of one provider at
  different prices are two prices, not a change: the merged model shows the
  lower, and which one is lower or up does not emit a change. A quote that changed in more than 20% of its priced
  fetches over 7 days, with at least 24 priced fetches, is a dynamic pricer: it goes in `dynamic`, not
  `changes`.
- **Names.** `name` is the id after `/`, hyphens as spaces, version dots
  kept, words capitalized (size tokens such as `27b` upper-cased). `family`
  comes from the org: qwen Qwen, deepseek DeepSeek, moonshotai Kimi, z-ai
  GLM, minimax MiniMax, openai GPT-OSS, meta-llama Llama, google Gemma,
  nvidia Nemotron, mistralai Mistral; any other org is its own family.
- **Demand.** `hf_downloads_30d` and `hf_likes` come from Hugging Face.
  They measure interest in the open weights (people downloading the model
  to run it themselves), not paid API usage on OpenRouter or anywhere
  else.
- **Compute units.** FLOP meters work in G_n. Its spec (yellowpaper R4.2)
  counts active parameters (mixture-of-experts aware) plus an attention term
  and the embedding, with published anchors Llama-3-8B 16, Llama-3-70B 140
  and DeepSeek-V3 74 G_n per token; the exact formula is not published
  ([flop-labs/yellowpaper#18](https://github.com/flop-labs/yellowpaper/issues/18),
  [#55](https://github.com/flop-labs/yellowpaper/issues/55)). So where FLOP
  gives no figure, ours is 2 × active parameters / 1e9: a lower bound that
  leaves out the attention term, which grows with context length. It puts
  listed prices on one scale; it is not a serving cost. `compute.basis`
  says which rule each model uses, first match wins:
  1. `flop_registry`: FLOP's value from `--compute` or built in. The
     DeepSeek-V3 anchor (74) applies to `deepseek/deepseek-chat-v3.1`,
     `deepseek/deepseek-v3.2` and `deepseek/deepseek-r1-0528` only when
     their Hugging Face parameter count is within 1% of DeepSeek-V3's 685B.
  2. `2x_active_params`: active parameters from `--compute`
     (`active_params_source: table`), else from an id carrying `-a<N>b`
     (`id`; `qwen3.5-397b-a17b` → 17e9, so 34 G_n per token).
  3. `2x_active_params_est`: active parameters estimated from the
     Hugging Face config when it gives `experts` and `experts_per_tok`
     (`config_estimate`, below).
  4. `2x_params_dense`: 2 × the Hugging Face parameter count, only up to
     100B weights and only when the config shows no experts (a model with
     `experts` in its config is never dense).
  5. null: anything else, such as a model over 100B weights with no active
     count and no expert counts in its config. Such a model is almost
     certainly sparse, so we do not guess; it is left out of
     `compute_price` and counted in `n_unknown`. On 2026-11-13: DeepSeek
     V4.1 Flash, GLM 5.3 Flash, Kimi K2.5/K2.6/K2.7-code/K3/K3.1-pro,
     MiniMax M3 and M3.1 Flash, Nemotron 3.5 Super VL, Inkling 2 Small, Muse
     Spark, Llama 3.1 8B (no parameter count), and Llama 4 Scout and Mistral
     Small 2603 (both thin).
  - **Active-parameter estimate.** Attention and embedding weights always
    run: fixed = layers × 4 × hidden² + vocab × hidden (vocab 150,000 if
    unknown). The rest, total − fixed, is expert weights, of which
    (per token + shared) / (experts + shared) run (shared 0 if unknown).
    Active = fixed + (total − fixed) × that share. No estimate (so the next
    rule applies) when the config cannot be right: more experts per token
    than experts, or fixed weights at or over the total.
    Without layers or hidden size: total × the share. It ignores dense MLP
    layers, norms and biases, and hybrid (Mamba) layers. Checked on
    2026-11-13 against every model with a stated active count and expert
    counts in its config:

    | Model | Stated active | Estimated | Ratio |
    |---|---|---|---|
    | Qwen3-235B-A22B-2507 | 22B (id) | 21.19B | 0.963 |
    | Qwen3-30B-A3B-2507 | 3B (id) | 2.95B | 0.985 |
    | Qwen3.8-2.4T-A95B | 95B (id) | 73.99B | 0.779 |
    | Beam 1.0 501B-A23B | 23B (id) | 21.70B | 0.943 |
    | Nemotron 3 Ultra 550B-A55B | 55B (id) | 25.13B | 0.457 |
    | DeepSeek V3.2 (V3 class) | 37B (FLOP's 74 G_n anchor) | 36.99B | 1.000 |
    | gpt-oss-120b | 5.1B (OpenAI) | 5.37B | 1.053 |

    Median absolute error 5.3%; it runs low on the largest models, and
    Nemotron 3 Ultra (no layer count in its config, so total × share) is
    54% low. Those two have the id count, which wins; the estimate is used
    only where nothing better exists. gpt-oss-20b (32 experts, 4 per
    token): 3.82B active, so 7.64 G_n per token instead of 41.8 as dense.
  - `usd_per_bn_gn` = median (USD per 1M tokens) / (G_n per token × 1e6) × 1e9.
    Qwen3.8-27B on 2026-11-13 (registry, 52): $0.296875 / 52e6 × 1e9 =
    $5.709 per billion G_n. DeepSeek V3.2 (685,355,329,792 params, anchor
    74): $0.278125 / 74e6 × 1e9 = $3.758. Qwen3.5-397B-A17B (34):
    $0.7609375 / 34e6 × 1e9 = $22.38. Qwen3.5-9B (dense, 9,653,104,368
    params, so 19.306): $0.103125 / 19.306e6 × 1e9 = $5.342.
  - `mix` at r:1 = (r × prompt + completion) / (r + 1) × 1e6 for each
    provider in the median's set (its price is the one picked at 15:1), then
    the median. The middle Qwen3.5-9B quote, $0.10 in and $0.15 out per 1M:
    3:1 (3 × 0.10 + 0.15) / 4 = $0.1125, 15:1 $0.103125, 150:1
    (150 × 0.10 + 0.15) / 151 = $0.1003. Its `out_in_ratio` is 0.15 / 0.10 = 1.5.
  - `flop.implied_flop_usd` = the basis model's `usd_per_bn_gn` / 1e9 /
    `gn_price_flop`: USD per G_n over FLOP per G_n, so USD per FLOP. With
    0.00291 FLOP per G_n: 5.709 / 1e9 / 0.00291 = $0.000001962.
  - Totals (`compute_price`, `mixes`, `out_in_ratio_median`) leave out
    `thin` and `stale` models and any value that is not finite. A parameter
    count of 0 is unknown, and a G_n per token that is not above 0 is none.
- **Routed price.** The price a request pays right now if it is spread
  across the sellers that are up, the way OpenRouter's default routing
  spreads traffic: cheaper sellers get more, in proportion to 1 ÷ price².
  Over the sellers in the model's latest snapshot that are up (status 0 or
  missing) with a usable 15:1 price, one per provider (its lowest; aliases
  merged as for the median), each gets weight uptime_30m / 100 ÷ price²,
  and the price is Σ weight × price / Σ weight (= Σ(u/p) / Σ(u/p²)). A
  seller with no `uptime_30m` (listed, no recent traffic) gets weight 0; if
  no up seller has any uptime weight, each gets 1 ÷ price² and
  `uptime_fallback` is true. There is no hold: a seller that is down is not
  counted, since this is the price paid now. **Cap:** no provider carries
  more than 50% of a model's weight; the excess goes to the others in
  proportion to their weights (repeated until none is over), so one very
  cheap seller cannot be the whole price, and `capped` says it acted. With
  one seller the cap cannot act; with two it makes the price their plain
  average. Worked: sellers at $2, $2 and $4, all at 100% uptime: weights
  1/4, 1/4, 1/16, price (2/4 + 2/4 + 4/16) / (9/16) = $2.222. This is not
  the model `median` (list prices, one per provider, sellers that went down
  held for 6 hours, unweighted) and not the perp's settlement mark
  (`latest.json`: the pusher's price set for Qwen3.8-27B under its own
  rules). On 2026-11-13 Qwen3.8-27B routed at $0.1931 per 1M (20 sellers,
  largest share 19%, so the cap did not act) against a median of $0.2969
  and a perp mark of $0.3036.
- **Robustness.** Unparsable or torn lines, a line without `rows`, rows
  without a provider, and missing days are skipped. An empty `rows` is a
  fetch that listed nobody.

## The page

`frontend/src/pages/Compute.tsx` fetches `/compute-feed/latest.json` and
`/compute-feed/history.json` (override with `VITE_COMPUTE_FEED_URL`; `none`
turns it off) every 60 s. It shows a stale badge when `landed_ms` is over 30
minutes before `generated_ms`, when `generated_ms` is over 30 minutes behind
the browser's clock (the feed stopped), or when `landed_ms` is more than 5
minutes after `generated_ms`. Without `VITE_MARKET_CLIENT=devnet` it uses the
mock in `frontend/src/compute/mock.ts`.

## Deploy (not done yet)

0. Once, create the out dir for the service user (compose cannot chown a
   bind mount): `install -d -o "$LAUNCH_UID" -g "$LAUNCH_GID" /opt/drip/compute-feed`.
1. In `deploy/.env`: `FLOP_ORACLE_PUBKEY` (the FLOP market's oracle
   authority; compose refuses to start the service without it), and if not the defaults `FLOP_RUN_DIR` (`/opt/drip/flop-devnet`,
   holding `bots.log`), `OR_PRICES_DIR` (`/opt/drip/or-prices`),
   `OR_PRICES_WIDE_DIR` (`/opt/drip/or-prices-wide`, mounted read-only),
   `COMPUTE_FEED_DIR` (`/opt/drip/compute-feed`, owned by `LAUNCH_UID`).
2. `docker compose -f deploy/docker-compose.yml --profile compute-feed up -d --build compute-feed indexer-proxy`
   builds `deploy/Dockerfile.flop-feed` (flop-index only) and restarts the
   proxy with the read-only feed mount.
3. `deploy/indexer-nginx.conf` serves the files at `:8091/compute-feed/`
   (GET only, `Cache-Control: no-store`); `index-state.json` and dotfiles
   (temp files, the lock) answer 404.
4. `frontend/vercel.json` rewrites `/compute-feed/*` to that route; the next
   Vercel deploy picks it up.

The service mounts only `bots.log`, not its directory (which holds keys).
A bind-mounted file keeps its inode, so rotation by rename is not followed
inside the container; `copytruncate` rotation is.
