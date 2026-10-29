# Private v16 indexer

A synchronous, read-only Solana poller for the devnet SOL-PERP market. It decodes
landed top-level wrapper instructions, keeps SQLite WAL history and portfolio
snapshots, and serves frontend history/positions plus honest keeper statistics.
It never submits transactions. Fill count is reported; volume/notional is not.

From the repository root:

```sh
source scripts/env.sh && ./scripts/fetch_upstream.sh  # once
cargo run --release --manifest-path indexer/Cargo.toml -- backfill
cargo run --release --manifest-path indexer/Cargo.toml -- run
# HTTP only, using an existing database (no RPC):
cargo run --release --manifest-path indexer/Cargo.toml -- serve
```

`backfill` resumes historical pages, catches up, snapshots state, prints `/stats`
JSON and exits. `run` does the same initially, then polls every 10 seconds with
HTTP on a separate thread. `serve` initializes schema if needed and serves HTTP.
The default database is relative to **cwd**. Use a private writable directory or
`--db indexer/indexer.sqlite`; do not commit database files.

## Configuration

Flags override environment variables. All flags take a separate value.

| Flag | Environment | Default |
| --- | --- | --- |
| `--rpc-url` | `INDEXER_RPC_URL` | `https://api.devnet.solana.com` |
| `--program-id` | `INDEXER_PROGRAM_ID` | `GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS` |
| `--market` | `INDEXER_MARKET` | `HyMKxfct1S9aFtq21GG3FUYUViQkmpZdk5pwGRtAz48C` |
| `--db` | `INDEXER_DB` | `indexer.sqlite` |
| `--bind` | `INDEXER_BIND` | `127.0.0.1:8090` |
| `--cors-origins` | `INDEXER_CORS_ORIGINS` | `http://localhost:5173,http://127.0.0.1:5173` |
| `--poll-secs` | `INDEXER_POLL_SECS` | `10` |
| `--rpc-delay-ms` | `INDEXER_RPC_DELAY_MS` | `250` |
| `--cycle-secs` | `INDEXER_CYCLE_SECS` | `3` |
| `--uptime-slack-cycles` | `INDEXER_UPTIME_SLACK_CYCLES` | `10` |

CORS echoes an exactly allowed Origin and `Vary: Origin`, including error
responses; no wildcard is supported. OPTIONS is accepted only for allowed
origins. All other methods except GET return 405. No files are served.
Each database is bound to its program and market. Keep a separate database per
network; RPC providers can change within that network. SQLite stores amounts and
full-width IDs/prices as decimal text; API IDs/prices are JSON integers (clients
should use a lossless JSON parser when handling values above JavaScript's safe
integer range). Each GET opens a read-only SQLite connection.

## Endpoints

Examples illustrate an empty database before its first successful ingest.

`GET /health` — healthy only if the last complete pass is within `3 * poll-secs`:

```json
{"ok":false,"last_ingest_at":null,"last_ingest_slot":null,"backfill_complete":false,"lag_secs":null}
```

`GET /stats` — success rate is `ok / (ok + failed)`, excluding idle. Idle still
counts as keeper activity. Uptime starts at the first ok/idle crank, subtracts
`max(0, gap - slack * cycle)` for each gap including the tail to now, and excludes
unknown block times. An empty or zero-duration window has a null percentage.
`as_of` is the last complete ingest time, while the uptime window ends at request
time. Open interest includes per-asset lifecycle and effective engine OI.

```json
{"market":"7p8DtcMHSjdCsM9xWvBwF6TebGBSL18wKvfM7BS1zyHq","program":"GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS","as_of_slot":null,"as_of":null,"cranks":{"total":0,"ok":0,"idle":0,"failed":0,"success_rate":null,"first_at":null,"last_at":null},"uptime":{"cycle_secs":3,"window_start":null,"window_end":1793243324,"covered_secs":null,"missed_secs":null,"uptime_pct":null,"longest_gap_secs":null,"gaps_over_slack":null},"liquidations_observed":0,"liquidations_since_slot":null,"open_interest":{},"depositors":0,"portfolios":0,"fills":0,"mode":null,"materialized_portfolio_count":null,"insurance":null,"vault":null}
```

`GET /fills?portfolio=5XWHkzJhpp1hFjMrTwbARWqCJGGuRmrrDDqHW9EmYbzN&limit=100`
— newest first; `limit` is 1..500, default 100, optional exclusive
`before_slot`. Each row has signature, ix/leg, slot/time, both portfolio keys/IDs,
asset/market ID, raw taker-signed `size_q`, exec/limit price, fee, price-known flag,
and `side` from the queried portfolio's view (buy means it went longer).

```json
{"fills":[],"next_before_slot":null}
```

The slot-only cursor cannot resume inside a slot. If a page cuts through multiple
fills at its boundary slot, remaining same-slot fills are skipped by the next
`before_slot` query. Use a larger limit (up to 500); a composite cursor is a future
API extension for complete pagination of unusually busy slots.

`GET /positions?owner=EdcNDZxPK6NPSQW4Yo8Di1veA9PRpVo9cr4Zgq99tSXo`
— latest changed snapshot per portfolio, with capital, pnl and active legs:

```json
{"owner":"EdcNDZxPK6NPSQW4Yo8Di1veA9PRpVo9cr4Zgq99tSXo","as_of_slot":null,"portfolios":[]}
```

Bad/missing pubkeys or query parameters return 400 with `{"error":"..."}`;
unknown routes return 404. Historical snapshots remain after an account closes.

## Coverage and limits

- Finalized commitment is approximately 13 seconds behind tip. Signature paging
  follows the program, but **only transactions referencing this market are stored**;
  cursors in `meta` also cover skipped unrelated transactions. Top-level program
  instructions must reference the market; inner/CPI invocations are ignored.
- Oldest-page cursors and rows commit atomically; tip cursors advance only after
  the full gap is committed. Replaying rows is idempotent. RPC 429/transport failures
  back off from 0.5 to 30 seconds, and every getTransaction attempt has the configured
  delay. Null/unavailable transactions leave the page pending instead of silently
  advancing past missing history. RPC history retention bounds available backfill.
- Failed transactions mark every wrapper instruction failed atomically. Error codes
  belong only to the failing instruction. Crank Custom(22) at that crank is idle;
  a different failure is failed. Preflight-rejected cranks never land, so the
  success rate covers **landed transactions only**, not all attempted submissions.
- TradeCpi/BatchTradeCpi exec price is unknown (`null`, `price_known=false`); the
  stored limit price and signed size come from the request. Matcher return data
  contains actual execution price (and may adjust size); decoding it is a follow-up.
- Snapshots are saved only when capital, pnl or active-leg JSON changes. Discovery
  uses drip's confirmed key/layout filtering, then account data is re-read finalized.
  Market and portfolio batches may have different context slots; each snapshot has
  its own actual slot. Positions report `basis_pos_q` only: the engine's pure
  `V16Core::effective_abs_quantity_for_leg` is `pub(crate)` at this pin
  (`src/v16.rs:2035`), so it cannot be reused here. These are stored basis positions,
  not a claim about current effective position after K/F or ADL.
- No liquidation event/counter exists at this pin. `liquidations_observed` is a
  **forward-only heuristic** starting at `liquidations_since_slot`, the first
  snapshot slot: a decrease in total absolute basis quantity, a successful targeted
  crank in `(previous.slot, new.slot]`, and no successful fill, withdrawal or close
  on that portfolio in that interval. It can miss intermediate changes and does
  not prove causality. It never invents historical liquidation counts from backfill.
  The generic instruction table has one extra portfolio column to exclude closes.
  `portfolios` counts distinct snapshotted keys; the market's current count is
  reported separately as `materialized_portfolio_count`.

Offline verification (after dependencies have been fetched once):

```sh
source scripts/env.sh
cargo test --offline --manifest-path indexer/Cargo.toml
cargo build --release --manifest-path indexer/Cargo.toml
cargo test --release --offline --manifest-path indexer/Cargo.toml
cargo clippy --manifest-path indexer/Cargo.toml --all-targets --no-deps -- -D warnings
cargo fmt --manifest-path indexer/Cargo.toml --check
python3 scripts/test_check_public_tree.py
```

## Prior art

- [aeyakovenko/percolator-cli](https://github.com/aeyakovenko/percolator-cli):
  TypeScript v16 parsing/trade scripts; no indexer identified in the reviewed CLI.
- [aeyakovenko/percolator-prog](https://github.com/aeyakovenko/percolator-prog):
  `ix::Instruction::decode`/`encode` and `state::read_market`/`read_portfolio` are
  reused directly, with the wrapper pin enforced by drip's build script.
- This repository's `drip::discovery` and monitor's market/header reads are reused.
- [dcccrypto/percolator-launch](https://github.com/dcccrypto/percolator-launch)
  (Apache-2.0): patterns considered include excluding liquidation markers from
  trade aggregates and scoping queries by network. Its account layouts describe
  its fork; **no code was copied**.
- [mizuki0x](https://github.com/mizuki0x): nothing indexer-related identified in
  this slice's prior-art review.
