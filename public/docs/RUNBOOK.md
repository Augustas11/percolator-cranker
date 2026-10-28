# Runbook

Everything here targets a **local `solana-test-validator`**. This keeper is
reference / dry-run / staging only.

## 1. Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust | stable, 1.80 or newer | `rustup` |
| Solana CLI (Agave) | 2.0.x (tested 2.0.21) | `sh -c "$(curl -sSfL https://release.anza.xyz/v2.0.21/install)"` |
| SBF platform-tools | **v1.52** | Side-loaded on first use by `cargo build-sbf --tools-version v1.52`. The default platform-tools of the 2.0 CLI do not build the wrapper at this pin; always pass the flag. The scripts do. |
| git, python3 | any recent | scripts |

`scripts/env.sh` puts the Solana and cargo bins on `PATH` and sets the
layout variables the scripts use. Source it in every shell.

## 2. Fetch and build the wrapper at the pinned SHA

```bash
./scripts/fetch_upstream.sh
```

This clones `percolator-prog` into `upstream/` (gitignored), checks out
`PERCOLATOR_PROG_SHA` from `scripts/pins.env`, and confirms that the
wrapper's Cargo engine pin equals ours.

- The host crates use the wrapper as a path dependency, for its instruction
  encoder and account decoders.
- The engine is a git dependency pinned by `rev` in `crates/*/Cargo.toml`.

The SBF program is built by the bring-up script below. To build it by hand:

```bash
(cd upstream/percolator-prog && cargo build-sbf --tools-version v1.52)
```

## 3. Local bring-up

```bash
FORCE_SBF_REBUILD=1 ./scripts/m1_bringup.sh
```

The script:
1. resets the ledger in `.solana-test-ledger/` and starts the validator on
   `127.0.0.1:8899`;
2. funds the default keypair `~/.config/solana/id.json`;
3. deploys the program under the build-time keypair id;
4. runs the bring-up smoke test (`crates/bringup`): market, deposit, crank,
   fee routing.

`KEEP_VALIDATOR=1` reuses a running validator. The program id is printed and
exported as `PERCOLATOR_PROGRAM_ID`.

## 4. Run the keeper

```bash
source scripts/env.sh
export PERCOLATOR_PROGRAM_ID=$(solana-keygen pubkey upstream/percolator-prog/target/deploy/percolator_prog-keypair.json)
CRANKER_CYCLES=3 cargo run --release -p percolator-cranker --bin cranker
```

The binary bootstraps its own one-asset market and runs N cycles against
it.

The load scenario uses the running validator and never resets it:

```bash
./scripts/live_shaped.sh 2>&1 | tee live-shaped-run.log
```

## 5. Configuration (environment)

| Variable | Default | Meaning |
|---|---|---|
| `PERCOLATOR_PROGRAM_ID` | — (required by the binary) | Deployed wrapper program id |
| `CRANKER_CYCLES` | 3 (scenario: 200) | Loop iterations |
| `CRANKER_CYCLE_MS` | 30000 | Per-cycle wall budget |
| `CRANKER_CYCLE_CU` | 24000000 | Per-cycle CU budget. Unknown CU is charged at 200k per tx. |
| `CRANKER_CYCLE_MAX_TX` | 64 | Per-cycle tx budget. Each drain step counts, and a quarter is reserved for quiet accounts. |
| `CRANKER_QUARANTINE_THRESHOLD` | 3 | Same-error failures in a row before quarantine |
| `CRANKER_QUARANTINE_BASE_CYCLES` | 2 | First backoff window, in cycles. It doubles on each re-quarantine. |
| `CRANKER_QUARANTINE_MAX_CYCLES` | 64 | Backoff cap |
| `CRANKER_MEASURE_CU` | 1 | `0` skips the per-tx `getTransaction` that reads CU from the logs |
| `CRANKER_PYTH_MODE` | off | `canonical` enables the shard-0 Pyth PDA fallback for Hybrid assets |
| `CRANKER_SCENARIO` | — | `live_shaped` runs the load scenario from the main binary |
| `CRANKER_SCENARIO_SEED` / `_TRADERS` / `_ASSETS` | fixed / 40 / 3 | Scenario shape |
| `CRANKER_SCENARIO_DUMP_DIR` | — | Save market and portfolio bytes whenever an account is quarantined |

**Keys.** The keeper signs with `~/.config/solana/id.json`, a local
validator key. The integration tests read `PERCOLATOR_TEST_PAYER` when it is
set. Never commit keys; `.gitignore` excludes `keys/` and `.env`.

## 6. Reading the output

Every submission prints a line with the target, plan class, outcome and
program CU. Every cycle ends with a summary:

```
cycle 12 summary: progressed=4 idle=7 failed=1 quarantined_skipped=2 deferred=0 new_quarantine=1 cu=1365077 max_tx_cu=275934
```

| Field | Meaning |
|---|---|
| `progressed` | The engine took a step (state moved). An A6 lien drain counts once per account. |
| `idle` | Nothing to do: `Ok(no-mut)`, or a revert with `EngineNonProgress` (0x16). On a Resolved market this also counts third-party accounts skipped inside the owner-signer window. |
| `failed` | Any other revert, engine error or RPC error. Only that account is affected. |
| `quarantined_skipped` | Accounts inside a backoff window. They spend no budget. |
| `deferred` | Accounts left for later because a budget ran out. Rotation reaches them first next cycle. |

Quarantine events are single JSON lines:

```json
{"event":"quarantine","account":"…","error":"EngineStale","cycle":33,"slot":3895,"plan_class":"live:pending","strike":3,"backoff_cycles":8,"until_cycle":42}
```

`plan_class` is the keeper's own label. The engine's chosen plan isn't
visible from outside the transaction.

## 7. Re-pinning when upstream moves

1. Fetch upstream and diff the wrapper and engine source between the old
   pin and the candidate. The wrapper lives in `src/v16_program.rs`; the
   engine in `src/v16.rs` and `spec.md`.
2. Confirm that the `PermissionlessCrank` wire, the `AutoCrankPlanV16` enum
   and `V16_LAYOUT_DISCRIMINATOR` are unchanged. If any of them changed,
   it's a rewrite, not a re-pin.
3. Update `scripts/pins.env`, plus the engine `rev` in both
   `crates/*/Cargo.toml`. The engine rev must equal the wrapper's own Cargo
   pin, or two `percolator` crates get linked.
4. Run `cargo update -p percolator`, then `./scripts/fetch_upstream.sh`,
   `FORCE_SBF_REBUILD=1 ./scripts/m1_bringup.sh`, a 3-cycle keeper run, the
   unit tests and the load scenario.
5. Re-measure CU and update the README tables.

## 8. Known limitations

- **Unreferenced lapsed backing wedges Live refresh** (engine side, current
  pins and the engine's newer master).
  - A Fresh backing bucket that passes its `expiry_slot` without any
    account referencing its domain is not expired by any Live instruction.
    The account-scoped lapse scan only covers referenced domains, and the
    asset-level scan runs only on Resolved markets.
  - A refresh that would attribute new positive PnL to that domain returns
    `Stale`. The account can't be refreshed or liquidated until the market
    resolves.
  - The keeper quarantines such accounts and keeps the rest progressing.
    `live_shaped` reports the failure honestly.
- **Residual value at CloseSlab.** Backing principal that lapses unused
  goes to the residual pool, and `CloseSlab` burns claim-free residue. The
  scenario checks the residue against "dust plus lapsed principal" and
  reports any excess.
- **The keeper doesn't decide the plan.** Priority is an off-chain
  estimate; the engine's selector is the only authority on what a crank
  does.
- **No priority fees** and no leader-slot awareness. Pyth accounts for
  Hybrid assets need operator-supplied mapping, or the canonical PDA mode.
- **Integration tests are gated.** They run only when `PERCOLATOR_TEST_RPC`
  and `PERCOLATOR_PROGRAM_ID` are set; `invariants` and `hard_reject` also
  need `PERCOLATOR_TEST_MARKET`.
