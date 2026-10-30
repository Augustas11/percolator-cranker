# Lifecycle drill: devnet, 2026-10-30

**Reference / dry-run only; not a paid live service; M3 NO-GO.**

**Result: PASS.** A separate SOL-PERP drill market went listing → Live →
Recovery → Resolved → relisted on devnet. Every account ended in a terminal
state, and the relisted market was wound down the same way. The run took
8 min 46 s from start to finish and cost 0.0226 SOL net.

This covers PoC criterion 3 in `ROADMAP.md`, with the limits listed under
"What this does not prove".

## How it ran

- **Program:** `GaXu3DXK…HKSHS`, the same program as the live market.
- **Live market:** `HyMK…z48C` and its keys were not touched. The Pearl VPS
  was not touched.
- **Config:** `config.pearl.toml` from the live run (price source
  `exchanges`, a public SOL/USD median), with two changes:
  - `matcher_ctxs = []`, so the drill never refreshed the live market's
    on-chain matcher;
  - its own monitor port.
- **Mint:** a fresh test mint per market.
- **Payer:** a dedicated drill payer, `BB2Ly2xk…tT1` (`~/.config/drip-devnet/drill-payer.json`).
  It was funded with 1.2 SOL by the main session.
- **Run dir:** `~/.config/drip-devnet/runs/drill-20261030/`
  (`listing/`, `relist/`). All keys are kept there.
- **Command:**
  ```
  DRILL_PAYER=~/.config/drip-devnet/drill-payer.json scripts/lifecycle_drill.sh drill.toml devnet <drill dir>
  ```
- **What the command does:**
  1. Lists with the normal Day-0 path, `scripts/launch_market.sh`: steps
     1–8, ending in a health check that needs a landed fill.
  2. Stops the bots.
  3. Runs `staging lifecycle`, which:
     - shuts down asset 0 (Recovery);
     - resolves the market;
     - lets the keeper pay every portfolio out to its owner;
     - closes both portfolios;
     - withdraws the backing principal and the insurance;
     - runs `CloseSlab`;
     - sweeps the refunded rent back to the payer.
  4. Lists a fresh market and winds that one down too.
- **Sends:** every lifecycle transaction is simulated first and sent only if
  the simulation is clean.

## Phases

Times are UTC on 2026-10-30.

**Listing market** `BbkaN7Bsk2AK4GJQ2DBbSPUpGMkysHrChQyqJMiDmdDN`:

| Phase | Pass | Slot | Time | Tx |
|---|---|---|---|---|
| Listing (steps 1–6) | yes | | 08:01:48 → 08:03:35 | see `listing/bots.log` |
| Live: health, fill landed | yes | | 08:05:20 | |
| Recovery (SHUTDOWN) | yes | 516709261 | 08:05:23 | `41r82K98…1aNiKzwB` |
| Resolved | yes | 516709311 | 08:05:34 | `2nQ1Xcsh…EJU7jjt5` |
| Paid out: keeper, both portfolios | yes | 516709364 | 08:05:47 | keeper cranks |
| LP portfolio closed | yes | 516709369 | 08:05:48 | `5N32kRrQ…nfWcTQLS` |
| Trader portfolio closed | yes | 516709415 | 08:05:59 | `2oHMmrru…Mk4NwBw5` |
| Backing withdrawn, 2 × $10k | yes | 516709421 / 428 | 08:06:01 | `5AXnqSoy…WmV6XqZC`, `UFQ6RTWy…vrkMNS2a` |
| Insurance withdrawn, $14.66 | yes | 516709476 | 08:06:14 | `66SgYNAQ…47VSEWgC` |
| Market closed (CloseSlab) | yes | 516709483 | 08:06:15 | `4V3tmbEc…FpLvavRJ` |
| Rent swept to payer, 0.1151 SOL | yes | 516709490 | 08:06:17 | `2mPF1ZsM…LPw7yyKt` |

**Relisted market** `FgBaWtweGVADazrCtjWVR89BcyDQiC2i7MoTDNVGeUgW`:

| Phase | Pass | Slot | Time | Tx |
|---|---|---|---|---|
| Relisted: steps 1–8, fill landed | yes | | 08:06:28 → 08:09:15 | see `relist/bots.log` |
| Recovery | yes | 516710280 | 08:09:26 | `2LqBJ9Wx…ws4mHgqK` |
| Resolved | yes | 516710284 | 08:09:27 | `533XE3zw…tFTTXuPh` |
| Paid out | yes | 516710385 | 08:09:52 | keeper cranks |
| Portfolios closed | yes | 516710391 / 439 | 08:09:53 / 08:10:04 | `63XjY6Bn…1bJxaYc2`, `5KyC5qnn…Ffxfj3Xf` |
| Backing withdrawn | yes | 516710445 / 452 | 08:10:06 | `5xhsYbkW…2qzxQicg`, `2jwEabbk…iAdEVd7f` |
| Insurance withdrawn, $12.32 | yes | 516710505 | 08:10:20 | `3KnVXAtK…8PRhqD2d` |
| Market closed | yes | 516710512 | 08:10:22 | `2QdCHX2m…oCFzv378` |
| Rent swept, 0.1151 SOL | yes | 516710559 | 08:10:33 | `o193bjF6…jnqExf9w` |

Full signatures are in each run dir's `lifecycle.jsonl`. Step timings are in
`timings.txt`.

## Final account states

For both markets:

- **Market:** closed to a 16-byte tombstone. It keeps 0.00073 SOL rent;
  everything above that was refunded.
- **Vault:** closed.
- **LP and trader portfolios:** closed (accounts gone). Their rent went to
  the market and came back through `CloseSlab`.
- **LP and trader:** paid out in full to their token accounts. Each holds
  about $29,997–29,999 of the $30,000 deposit; the difference is trading
  PnL and fees.
- **Backing and insurance:** fully withdrawn to the role keys' token
  accounts (test USDC).
- **Market authority key:** emptied; its lamports were swept to the payer.
- **Role keys:** all hold 0 SOL.

## SOL

- **Before:** 1.2000 SOL.
- **After:** 1.1774 SOL.
- **Net:** 0.0226 SOL. The payer keeps the rest.

Where the 0.0226 SOL went:

- **Fees:** about 0.0011 SOL, over 110 transactions.
- **Rent still locked:** 0.0215 SOL, made up of:
  - 12 token accounts at 0.0015 SOL each: admin, LP, trader, backing,
    insurance and market-authority, per market. These can be reclaimed by
    burning the test tokens and closing the accounts; the drill does not
    do that yet.
  - 2 test mints at 0.0011 SOL each. SPL mints can't be closed.
  - 2 market tombstones at 0.0007 SOL each. The wrapper keeps these by
    design.

## What broke

- **Locally, before devnet:** three wind-down steps were missing and were
  added. Without them `CloseSlab` refuses with `EngineLockActive`:
  - **Backing:** the backing buckets must be withdrawn first, by the backing
    authority, once the market is Resolved and empty.
  - **Insurance:** the insurance domain budget must be withdrawn first, by
    the insurance authority.
  - **Trade cadence:** the bots' first trade can come before the first price
    push and is then refused. With the 300 s cadence the next try missed
    the health window, so the drill sets `STAGING_TRADE_EVERY_SECS=30`.
- **On devnet:**
  - **One failed transaction:** a bot crank during the relisted market's
    Live phase, `5VAMKgNW…jajFPX9w`, failed with `EngineNonProgress`
    (fee 0.000019 SOL). It came from the existing bots, not the lifecycle
    code: the pusher's accrual crank sends without preflight. Every
    lifecycle transaction landed clean.
  - **Mark lag** at health was p50 1.0–1.9 s against the 400 ms target.
    Same cause as the live market: a devnet price source.

## What this does not prove

- **SHUTDOWN timing:** SHUTDOWN ran seconds after the bots stopped, not
  while trades were landing. Each Live phase had real fills; the moment of
  shutdown did not.
- **F1 (#243):** no backing roll happened. The tenor is 1 day and each Live
  phase lasted about 4 minutes, so the F1 wedge was not exercised.
- **Stale oracle:** a market left with a stale oracle for over an hour needs
  the permissionless stale-resolve route. The drill does not implement it;
  SHUTDOWN would fail simulation and stop.

## Rerun

```
DRILL_PAYER=<payer.json> scripts/lifecycle_drill.sh <config> devnet <new drill dir>
```

The config must have `matcher_ctxs = []`; the script refuses otherwise. The
payer needs at least 1 SOL at each listing (the `init-market` check). An
interrupted run resumes: the lifecycle step works from chain state, and the
script skips a listing whose wind-down has started.
