# Compute price feed (flop-feed)

Read-only JSON for the site's `/compute` page, built from the pusher's push
log and the OpenRouter recorder.

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

- `bots.log`: the bots' log. Only lines with `"event":"flop_push"` are used;
  everything else, unparsable lines, lines over 1 MiB, lines from another
  oracle and a half-written last line are skipped.
- `or-prices dir`: the recorder's `YYYY-MM-DD.jsonl` files
  (`staging record-or`). Missing days or a missing directory mean no outage
  data, not an error.
- `--oracle`: the FLOP market's oracle authority pubkey (required).
- `out dir`: gets `latest.json` and `history.json`. Each is written to a temp
  file in the same directory (named with the process id), synced, and
  renamed, so a reader never sees half a file.

The first pass streams the whole log once, keeping only each push's time,
mark and transaction signature. `--loop` then reads only bytes added since
the last complete line, and re-reads only recorder files whose size changed.
A log that got shorter or is a new file (rotated) is read again from the
start; pushes already read are kept, and a push seen twice (same `sig`)
counts once. One run exits
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
| `high`, `low` | Highest and lowest mark over every push in the window (null if none) |
| `points` | `[landed_ms, mark_usd]` pairs, oldest first, at most 600. The mark is a step, so these are the first and last push and the pushes on both sides of every change. Over 600, each equal time slice keeps its lowest and highest push. |
| `changes[]` | Each push whose mark differs from the one before: `t_ms`, `from`, `to`, `sig`; the newest 500 |
| `outages[]` | Per bf16/fp16 seller from the recorder: `provider`, `quantization`, `from_ms`, `to_ms` (null while still down). Down means status not 0 (a missing status counts as 0, as in the pusher), unpriced, or missing from a snapshot after it was first listed. |
| `recorder_gaps[]` | Recorder silences over 15 minutes: `from_ms`, `to_ms` (null if still silent) |

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
