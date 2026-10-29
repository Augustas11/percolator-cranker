# hedger (private)

Our LP takes the other side of every taker trade on our SOL-PERP market, so it
builds SOL inventory. The replay (`research/replay/REPORT.md`) only makes money
**hedged**. This bot reads the LP's net SOL from the chain and offsets it on
Hyperliquid's SOL perp.

- **Policy:** the replay's tight setting. When |LP + hedge| × price passes
  `threshold_usd` (1% of daily volume), it sends the whole net as one
  immediate-or-cancel order, capped by `max_order_sol` and `max_hedge_sol`, with
  a `cooldown_ms` between orders. Every knob is in `hedger.toml`.
- **Dry-run by default.** It logs the order it *would* place, priced by walking
  Hyperliquid's public order book. It sends nothing and needs no keys.
- **Safety:**
  - live needs `--live` *and* credentials;
  - the bot reads the cluster's genesis hash at startup and refuses Hyperliquid
    mainnet unless the market is on mainnet-beta, in every mode, with no override;
  - `touch` the `kill_file` to stop all orders;
  - any live IOC error halts ordering until restart because it may have filled;
  - a max-loss guard stops ordering once the residual's P&L less hedge costs
    passes `-max_loss_usd`. It stays stopped until restart;
  - it holds when the market is not Live, or when our mark and the venue mid
    differ by more than `max_mark_gap_bps`.
- **Logs:** one JSON line per tick and per order on stdout (`tick`, `order`,
  `halt`, `error`, `start`, `stop`).
- **Metrics:** `percolator_hedger_*` at `metrics_listen`, e.g. LP net SOL, hedge
  position, residual delta, hedge cost (bps), funding, guard P&L, halted.

## Why Hyperliquid

Hyperliquid has the deepest on-chain SOL perp book (about $5–7M a side in the 20
levels the replay measured), and the replay's 4.5 bps fee and slippage figures
are from it. Its API is two JSON endpoints (`/info` and `/exchange`). It has
**API wallets**, which can trade but cannot withdraw, so we don't need a
withdrawal-capable key on the bot host.

Drift is a Solana program: hedging there means a second on-chain client. Binance
needs a KYC'd account. Either can sit behind the same `Venue` trait in `src/venue.rs`
later.

Order signing follows the official `hyperliquid-python-sdk` (MIT), and a unit test
pins it to a signature that SDK produced. No SDK dependency is pulled in: the
official Rust SDK brings ethers and its own async stack, while the ~60 lines here
use crates already in our graph (msgpack aside).

## Hedge cost vs the replay

`cost-check` on 2026-10-29 (SOL ≈ $99.7, base-tier 4.5 bps fee), average of buy
and sell:

| Hedge size | Slippage bps | All-in bps | Replay assumed |
|---:|---:|---:|---:|
| $1k | 0.05 | 4.55 | 4.99 |
| $10k | 0.18 | 4.68 | 4.99 |
| $100k | 0.58 | 5.08 | 5.21 |
| $1M | 7.6 (aggregated book) | 12.1 | 7.58 |

Up to $100k, the real cost is at or a little under the replay's assumption.
Tight hedges are $10k orders, so that is the range that matters. The $1M row
comes from the aggregated book, whose 0.1 levels overstate slippage.

## Run

```bash
./scripts/fetch_upstream.sh
cd hedger
cargo test
cargo run --release -- cost-check            # fee + book slippage at $1k-$1M
cargo run --release -- --minutes 60          # dry-run against hedger.toml
```

The default dry-run uses the testnet book. `cost-check` pairs with no market
and always reads mainnet's public book, regardless of `venue.network`.

## Going live (owner)

1. **Account.** Open a Hyperliquid account from a wallet you control (the
   hardware wallet in `ops/KEYS.md` style, not the bot host). Deposit the hedge
   margin, about **$20k USDC** (`ops/CAPITAL_PLAN.md`).
2. **API wallet, trade only.** In Hyperliquid → More → API, create an API
   (agent) wallet. It can place and cancel orders but **cannot withdraw or
   transfer**. Copy its secret key once.
3. **Key file, outside the repo.**
   ```bash
   mkdir -p ~/.config/percolator && umask 077
   pbpaste > ~/.config/percolator/hl-api-wallet.key && chmod 600 ~/.config/percolator/hl-api-wallet.key
   ```
   The bot refuses a key file inside this repo or readable by others. Never
   commit it.
4. **Check the fee.** If your tier's taker fee is not 4.5 bps, set
   `venue.taker_fee_bps`.
5. **Point at mainnet.** In `hedger.toml`, set `venue.network = "mainnet"` together
   with `chain.rpc`, `chain.market` and `chain.lp_portfolio` for the mainnet-beta
   market and LP. The bot refuses otherwise. Size `policy.threshold_usd`
   to 1% of real daily volume. Leave the caps as they are until it has run for a
   week.
6. **Rehearse on testnet first** (optional): set `venue.network = "testnet"` with a
   testnet API wallet and test USDC. This is how to use `--live` with a devnet/local
   market before a mainnet market exists.
7. **Start.**
   ```bash
   HL_ACCOUNT_ADDRESS=0xYourMainAccount \
   HL_SECRET_KEY_FILE=~/.config/percolator/hl-api-wallet.key \
   cargo run --release -- --live | tee -a hedger.jsonl
   ```
   The `start` line shows `"mode":"live"` and the API wallet's address. To stop
   all orders: `touch /tmp/percolator-hedger.KILL`.
