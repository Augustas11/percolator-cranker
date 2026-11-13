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
  (`{"at_ms", "model", "hf", "downloads_30d", "likes"}`); missing or odd
  ones mean no aliases or no demand data, not an error. Only directories
  named `<org>__<name>` are models. Adds `market.json`, and keeps the index state
  in `<out dir>/index-state.json`.
- `--rebase` (with `--wide`): ignore any saved index state and start a new
  base. Only for a deliberate re-base; the level then means something new.
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

With `--wide`, the feed loads `index-state.json` from the out dir at start
and continues from it. If there is none it creates a base; if it is there
but cannot be read or has bad values, the feed refuses to start (no silent
re-base) until it is fixed or `--rebase` is passed. The state is written
atomically each run, before `market.json`.

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
| `generated_ms` | When the feed wrote this file |
| `window` | `from_ms` (oldest snapshot held), `to_ms` (now) |
| `index.level` | The index now (100 at the base) |
| `index.base_ms` | Time of the base snapshot |
| `index.change_24h_pct`, `index.change_7d_pct` | Change vs the level 24 h / 7 days ago; null without data that old |
| `index.n_models` | Models in the index now |
| `index.n_quotes`, `index.n_quotes_up` | Sellers listed in each model's latest fetch, all and up |
| `index.n_providers` | Distinct providers in those listings |
| `index.series` | `[t_ms, level]`, at most 500; both sides of every change |
| `models[]` | Per model, most up sellers first: `id` (shortest id of its alias group), `aliases` (the other ids), `name`, `family`, `sellers_total`, `sellers_up`, `median`, `p25`, `p75`, `min`, `max`, `thin`, `cheapest` (`provider`, `quantization`, `price`; null if no usable quote), `change_24h_pct`, `demand` (below), `sellers_added_7d`, `series` (`[t_ms, median]`, at most 200), `quotes[]` (`provider`, `quantization`, `price` or null, `up`, `uptime_24h_pct`), cheapest first, unpriced next, down last |
| `models[].demand` | From the newest demand line of the model (or an alias): `hf_downloads_30d`, `hf_likes`, `as_of_ms`, and `hf_downloads_change_7d_pct` against the newest line at least 6 days older (null without one). All null without demand data. |
| `models[].sellers_added_7d` | Quotes (provider, quantization) listed now that were not listed in the snapshot 7 days ago; null without data that old. Sellers adding a model is a supply-side read of demand. |
| `demand_rank` | Model ids by `hf_downloads_30d`, highest first (models without it left out) |
| `providers[]` | `name`, `models`, `quotes` (in the latest listings), `uptime_24h_pct` (over all its quotes), `cheapest_in` (models where it is the cheapest up seller); most models first |
| `changes[]` | Price changes over the last 7 days, newest first, at most 200: `at_ms`, `model`, `provider`, `quantization`, `from`, `to` |
| `dynamic[]` | Quotes that change price too often to list: `model`, `provider`, `quantization`, `min`, `max` (over 7 days), `changes_24h`; most changes first |

## Broad index method

- **Price.** Each quote's price is (15 × prompt + completion) / 16 × 1e6.
  A quote is usable if it is up (status 0 or missing) and both prices are
  finite, not negative, and not both 0.
- **Aliases.** Ids are grouped by Hugging Face id from `hf-map.tsv` (else
  the id itself), lowercased and without a trailing date (`-0731`,
  `-20261017`), so dated snapshots such as `deepseek/deepseek-v4-flash` and
  `deepseek/deepseek-v4-flash-0731` are one model, named by its shortest id.
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
  The hold is per listing too; the median then takes one price per
  (provider, quantization), the lowest. The hold feeds the index and each
  model's `series`; the card's `median`,
  `p25`..`max` and `quotes` show what is listed now.
- **Model median.** At each fetch of a model, the median of its usable
  quotes (with held ones) (mean of the middle two for an even count). Percentiles interpolate
  linearly between the sorted prices. With fewer than 3 usable quotes the
  last median is carried forward, and `thin` is true while that is the case
  now. On the card, a thin model shows the carried median (or, if it never
  had 3, the median of what it has); `p25`..`max` are over its current
  usable quotes. A model with no usable quote and no median yet is left out.
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
  point per hour). Each run steps only through snapshots after `at_ms`.
- **Level.** Jevons over the models in the index: the geometric mean of
  (median / base median) × 100. A model that joins gets base = median /
  (level / 100), so the level does not move. When a model leaves (24 h
  without 3 usable quotes), the remaining bases are scaled by one common
  factor so the level at their last medians is unchanged. So after a gap
  (feed down for hours), the first snapshot's level comes from the saved
  bases and moves only by real price changes. A group whose listed aliases
  change (an alias appears or goes quiet) leaves and joins again in the same
  step, so its own move at that step is not counted.
- **Method changes.** At start, each basket model's median at the saved
  step is recomputed from the data; if it differs from the saved one (the
  method changed), that model's base is scaled by the same ratio, so the
  level does not jump.
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

1. In `deploy/.env`: `FLOP_ORACLE_PUBKEY` (the FLOP market's oracle
   authority), and if not the defaults `FLOP_RUN_DIR` (`/opt/drip/flop-devnet`,
   holding `bots.log`), `OR_PRICES_DIR` (`/opt/drip/or-prices`),
   `OR_PRICES_WIDE_DIR` (`/opt/drip/or-prices-wide`, mounted read-only),
   `COMPUTE_FEED_DIR` (`/opt/drip/compute-feed`, owned by `LAUNCH_UID`).
2. `docker compose -f deploy/docker-compose.yml --profile compute-feed up -d --build compute-feed indexer-proxy`
   builds `deploy/Dockerfile.flop-feed` (flop-index only) and restarts the
   proxy with the read-only feed mount.
3. `deploy/indexer-nginx.conf` serves the files at `:8091/compute-feed/`
   (GET only, `Cache-Control: no-store`).
4. `frontend/vercel.json` rewrites `/compute-feed/*` to that route; the next
   Vercel deploy picks it up.

The service mounts only `bots.log`, not its directory (which holds keys).
A bind-mounted file keeps its inode, so rotation by rename is not followed
inside the container; `copytruncate` rotation is.
