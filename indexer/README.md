# Private v16 indexer

A synchronous, read-only Solana poller for the devnet SOL-PERP market. It decodes
landed top-level wrapper instructions, keeps SQLite WAL history and portfolio
snapshots, and serves frontend history/positions plus honest keeper statistics.
It never submits transactions. Executed NoCpi fill count is reported; CPI requests
are retained separately. Volume/notional is not reported.

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
`--db indexer/indexer.sqlite`; do not commit database files. Root `/indexer.sqlite*`
and indexer-local SQLite files are ignored. Legacy databases with `exec_price` or
marks without `observation_sequence` are refused: preserve the old database and use a new `--db` path with `backfill` to
rebuild corrected market filtering, crank outcomes, and mark history. No automatic
migration or deletion is performed.

A devnet backfill of about 230 transactions takes about 15 minutes on the public
RPC. Use a paid RPC via `INDEXER_RPC_URL` for faster backfills. Errors replace every
occurrence of the configured RPC URL with `<rpc>`, including retry and exit errors.

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
| `--leaderboard-exclude` | `INDEXER_LEADERBOARD_EXCLUDE` | Six devnet matcher/bot portfolio and owner pubkeys listed below |
| `--poll-secs` | `INDEXER_POLL_SECS` | `10` |
| `--rpc-delay-ms` | `INDEXER_RPC_DELAY_MS` | `250` |
| `--cycle-secs` | `INDEXER_CYCLE_SECS` | `3` |
| `--uptime-slack-cycles` | `INDEXER_UPTIME_SLACK_CYCLES` | `10` |

`--leaderboard-exclude` accepts comma-separated validated portfolio or owner pubkeys.
An explicitly empty value (including `INDEXER_LEADERBOARD_EXCLUDE=`) excludes nothing.
The defaults are:

```text
6wH2d4CARhEWeGC2deBDb9pVT1a7Tgd48VUiFmkTNAHY
TVA9xtfvX8uiFAqDHCpeD4HzwT6CYb5iwD6J16cba46
GUvzajrrfWpizSGj38iCC7TfB7rbbWxTgCszPAh1VRyh
Fxi2q4iFrhPpfemCzkCK2haseAuoh6GvK7h7zwmaNZeu
CUJ86gVNfnTowKt3k7QVbCa589hpUtZhiypEnt9Ben8B
ANNQ2dksYNTWJG33jo5okUPb7AqpjUu6gcMN62bWdJc7
```

The API is local-only. `INDEXER_BIND` / `--bind` must be a numeric loopback socket
address (`127.0.0.0/8` or `::1`, e.g. `[::1]:8090`). Put a reverse proxy in front
for any non-local use, with request body limits and timeouts. Requests with a
positive Content-Length are rejected with 413, and Transfer-Encoding or malformed
Content-Length with 400, before URL parsing (best effort). tiny_http 0.12 drains
unread bodies, so these checks do not make direct public exposure safe.

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

`GET /stats` — success rate is `ok / (ok + failed)`, excluding idle and reverted. Idle still
counts as keeper activity. Uptime starts at the first ok/idle crank, subtracts
`max(0, gap - slack * cycle)` for each gap including the tail to the pass time, and excludes
unknown block times. An empty or zero-duration window has a null percentage.
`as_of` and `uptime.window_end` are the last complete ingest pass time. Stats are
computed once per completed pass and stored in `meta.stats_json`; requests read
that cached JSON without history scans. Before the first pass the empty shape is
returned, with window_end 0. Open interest includes per-asset lifecycle and
effective engine OI.

```json
{"market":"7p8DtcMHSjdCsM9xWvBwF6TebGBSL18wKvfM7BS1zyHq","program":"GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS","as_of_slot":null,"as_of":null,"cranks":{"total":0,"ok":0,"idle":0,"failed":0,"reverted":0,"success_rate":null,"first_at":null,"last_at":null},"uptime":{"cycle_secs":3,"window_start":null,"window_end":0,"covered_secs":null,"missed_secs":null,"uptime_pct":null,"longest_gap_secs":null,"gaps_over_slack":null},"liquidations_observed":0,"liquidations_since_slot":null,"open_interest":{},"depositors":0,"portfolios":0,"fills":0,"mode":null,"materialized_portfolio_count":null,"insurance":null,"vault":null}
```

`GET /leaderboard` — up to 100 wallets, sorted by PnL descending then wallet ascending,
with 1-based ranks. Computed each ingest pass and cached; includes closed portfolios with zero equity.
Aggregates latest snapshots; current equity is
capital plus engine PnL, net deposits are successful deposits minus withdrawals,
and PnL is equity minus net deposits. Amounts are collateral atoms as decimal
strings. Trades count all stored fill rows (executed and requested) per portfolio;
open positions count nonzero basis legs. Configured owners/portfolios are excluded.
`as_of_slot` is the last completed ingest slot. No query parameters are accepted.
The response note explains that open positions are marked at the last crank.

`GET /fills?portfolio=5XWHkzJhpp1hFjMrTwbARWqCJGGuRmrrDDqHW9EmYbzN&limit=100`
— newest first by `(slot, signature, ix_index, leg)` descending; `limit` is 1..500,
default 100. Pass the returned opaque `next_before` as `before` to resume,
including inside one slot or transaction. The exclusive cursor format is
`<slot>.<signature>.<ix_index>.<leg>`; malformed/noncanonical cursors are rejected.
`before_slot` is no longer supported.

Each row has signature, ix/leg, slot/time, both portfolio keys/IDs, asset/market ID,
raw taker-signed `size_q`, `reported_price` (NoCpi instruction price, otherwise
null), `limit_price` (CPI), `requested_fee_bps`, `mark_price`, `status`, and `side`
from the queried portfolio's view (buy means it went longer). NoCpi rows are
`executed`; TradeCpi/BatchTradeCpi rows are `requested`, because successful handlers
can execute zero size. `/stats.fills` counts only `executed` rows. CPI uses the
configured base fee and NoCpi may derive a dynamic fee, so requested fees are not
actual charged fees. The old `exec_price`, `fee_bps`, and `price_known` fields are
removed.

`marks(signature, ix_index, slot, asset_index, price, observation_sequence)` stores
successful tag 63 PushAuthMark instructions, uniquely by signature/ix. `mark_price` is queried from
the latest push for this database's market and the fill's asset_index at an
earlier slot, or the same transaction with a lower ix_index; it is null if none
exists. Other transactions in the same slot have no reliable relative order and
are excluded. Qualifying marks in the same slot are ordered by the monotonic
`observation_sequence`, stored as decimal text to preserve full-width u64 values.
Every fill includes:

```json
{"price_note":"estimate: execution uses the engine mark; mark_price is the last pushed mark before this fill"}
```

```json
{"fills":[],"next_before":null}
```

`GET /positions?owner=EdcNDZxPK6NPSQW4Yo8Di1veA9PRpVo9cr4Zgq99tSXo`
— latest changed snapshot per currently existing portfolio, with capital, pnl and active legs.
`as_of_slot` is the last completed ingest slot (also for empty results); each portfolio
has `last_changed_slot`, the slot of its stored snapshot:

```json
{"owner":"EdcNDZxPK6NPSQW4Yo8Di1veA9PRpVo9cr4Zgq99tSXo","as_of_slot":null,"portfolios":[]}
```

Bad/missing pubkeys or query parameters return 400 with `{"error":"..."}`;
unknown routes return 404. Historical snapshots remain after an account closes,
but `/positions` joins `current_portfolios`, which is replaced from valid finalized
reads in the same transaction as each pass's snapshots. The keys read are the union
of confirmed discovery (filtered by market/layout) and existing current portfolios,
so a confirmed close cannot remove a portfolio before it closes at finalized.

## Coverage and limits

- Finalized commitment is approximately 13 seconds behind tip. Signature paging
  follows the program, but **only transactions referencing this market are stored**;
  cursors in `meta` also cover skipped unrelated transactions. Top-level program
  instructions with known handler layouts must have this market at their handler's
  market account index. Unknown layouts retain the market-anywhere rule but create
  only generic instruction rows; inner/CPI invocations are ignored.
- Oldest-page cursors and rows commit atomically; tip cursors advance only after
  the full gap is committed. Replaying rows is idempotent. RPC 429/transport failures
  back off from 0.5 to 30 seconds, and every getTransaction attempt has the configured
  delay. Null/unavailable transactions leave the page pending instead of silently
  advancing past missing history. RPC history retention bounds available backfill.
- Failed transactions roll back all effects. Error codes belong only to the
  failing instruction. Percolator error names are assigned only when that instruction
  targets this program; foreign-program errors retain their code/raw JSON with a
  null name. Crank Custom(22) at that crank is idle; a different failure
  at that crank is failed. Other cranks in a failed transaction are `reverted`,
  excluded from both the success-rate denominator and uptime liveness. Crank stats
  include every signer: cranking is permissionless. Preflight-rejected cranks never land, so the
  success rate covers **landed transactions only**, not all attempted submissions.
- Fill prices are estimates. The wrapper uses the engine effective price when
  the profile does not update the mark from trades (this AuthMark market), and
  otherwise clamps the reported price. The last pushed mark is an estimate, not
  proof of the engine execution price. CPI size and limit are requests; matcher
  execution/return data is not decoded.
- Snapshots are saved only when owner, capital, pnl or active-leg JSON changes. Discovery
  uses drip's confirmed key/layout filtering plus current portfolio keys, then
  account data is re-read finalized. Only valid portfolios for this market remain current.
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
  not prove causality. An undecodable trade on a portfolio can look like a
  liquidation to this heuristic. Catch-up signature requests use minContextSlot
  at least as high as every captured snapshot before running exclusions; an RPC
  rejection fails the pass without writing liquidation rows. It never invents
  historical liquidation counts from backfill.
  The generic instruction table has one extra portfolio column to exclude closes.
  `portfolios` counts distinct snapshotted keys; the market's current count is
  reported separately as `materialized_portfolio_count`.

- `rustls-webpki 0.101.7` via Solana 1.18 has advisory RUSTSEC-2026-0099.
  This is a repo-wide dependency pin and is not changed here.
- CI gates the private indexer test step on `indexer/Cargo.toml` being present;
  public exports omit this crate.

Offline verification (after dependencies have been fetched once):

```sh
source scripts/env.sh
export CARGO_NET_OFFLINE=true
cargo test --release --manifest-path indexer/Cargo.toml
cargo clippy --release --manifest-path indexer/Cargo.toml -- -D warnings
cargo fmt --manifest-path indexer/Cargo.toml --check
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
