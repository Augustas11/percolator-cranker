# Compute price feed (flop-feed)

Read-only JSON for the site's `/compute` page, built from the pusher's push
log and the OpenRouter recorder. It does not check signatures; to check a
push line use `staging verify-push-log` (SPEC.md, "Checking the pusher").

## Run

    cargo build --release --manifest-path flop-index/Cargo.toml --bin flop-feed
    flop-feed <bots.log> <or-prices dir> <out dir>                # once
    flop-feed <bots.log> <or-prices dir> <out dir> --loop 60      # every 60 s

- `bots.log`: the bots' log. Only lines with `"event":"flop_push"` are used;
  everything else, unparsable lines and a half-written last line are skipped.
- `or-prices dir`: the recorder's `YYYY-MM-DD.jsonl` files
  (`staging record-or`). Missing days or a missing directory mean no outage
  data, not an error.
- `out dir`: gets `latest.json` and `history.json`. Each is written to a temp
  file in the same directory and renamed, so a reader never sees half a file.

The first pass streams the whole log once, keeping only each push's time,
mark and transaction signature. `--loop` then reads only bytes added since
the last complete line, and re-reads only recorder files whose size changed.
A log that got shorter (rotated) is read again from the start. One run exits
non-zero if the log has no push yet; `--loop` prints the error and retries.

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
| `points` | `[landed_ms, mark_usd]` pairs, oldest first, at most 600 (the last push of each equal time slice) |
| `changes[]` | Each push whose mark differs from the one before: `t_ms`, `from`, `to`, `sig`; the newest 500 |
| `outages[]` | Per bf16/fp16 seller from the recorder: `provider`, `quantization`, `from_ms`, `to_ms` (null while still down). Down means not up (status not 0 or unpriced) or missing from a snapshot after it was first listed. |
| `recorder_gaps[]` | Recorder silences over 15 minutes: `from_ms`, `to_ms` (null if still silent) |

## The page

`frontend/src/pages/Compute.tsx` fetches `/compute-feed/latest.json` and
`/compute-feed/history.json` (override with `VITE_COMPUTE_FEED_URL`; `none`
turns it off) every 60 s, and marks the price stale when `landed_ms` is over
30 minutes old. Without `VITE_MARKET_CLIENT=devnet` it uses the mock in
`frontend/src/compute/mock.ts`.
