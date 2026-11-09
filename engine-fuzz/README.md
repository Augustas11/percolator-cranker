# engine-fuzz

A money fuzzer for the Percolator engine we run in production. It plays random
trading sessions against the engine and checks that everyone gets back what they
are owed.

## What it checks

Each run uses one asset, 3 to 5 traders and an optional backing provider (fees and
funding off). A simple shadow ledger tracks what each trader is owed:
deposits - withdrawals + position x price change. Random actions follow (deposits,
trades, price moves, cranks, liquidations, conversions, withdrawals, provider
withdrawals), then the market is resolved, every account is closed and the slab is
wound down. At the end it checks:

1. **Conservation**: everything paid out plus what is left equals everything paid in.
2. **No-default fairness**: if nobody went bankrupt, each trader gets exactly their
   fair amount, the provider gets its full principal, and nothing is retired.
3. **Fair loss sharing**: if someone went bankrupt, winners lose exactly the shortfall
   in total, split by their exposure at that moment.
4. **Provider principal**: the provider recovers its full principal, with or without a
   default (a default's shortfall is borne by winners, not the provider).

It also checks solvency and positions after every step and that the market can
always be closed. Rounding slack is 2 atoms per operation.

Tests:

- `tests/replays.rs`: fixed replays of upstream issues #250, #252 and #253, plus the #212 provider case and
  two new cases the fuzzer found. Each must show its bug; each has a control that must
  pass. When upstream fixes one and we re-pin, flip its assertion.
- `tests/money_fuzz.rs`: the random fuzzer (proptest).

## How to run

```bash
# CI smoke run: replays + 64 random cases. Known bug classes do not fail.
cargo test --release --manifest-path engine-fuzz/Cargo.toml

# More cases
FUZZ_CASES=1000 cargo test --release --manifest-path engine-fuzz/Cargo.toml

# Strict run against the production pin, or against upstream master
engine-fuzz/run.sh pin 2000
engine-fuzz/run.sh master 2000
```

`run.sh` runs the replays, a survey (how many runs break each invariant), then one
strict fuzz per known bug class so each gets its own shrunk repro. `master` is
exported from `../repo` (ref `origin/master`; override with `MASTER_REF` or
`MASTER_SRC`) and patched in without touching `Cargo.toml` or `Cargo.lock`.

Env flags: `FUZZ_CASES` (default 64), `FUZZ_STRICT=1` (known bug classes fail),
`FUZZ_INVARIANT=<label>,...` (in strict mode, fail only on these), `FUZZ_SURVEY=1`.

## Reading a failure

A failure prints one `VIOLATED <invariant>: ...` line per break with every term,
then per-account numbers (deposits, withdrawals, payout, fair amount, loss share),
provider principal vs recovered, retired atoms, the minimal action list (shrunk by
proptest) and a step-by-step trace of what the engine did. Paste the action list
into a replay test to reproduce it.

Invariant labels: `inv1_conservation`, `step_solvency`, `position_model`,
`terminal_liveness` always fail. `inv2_no_default_payout`, `inv2_no_default_retired`,
`inv3_loss_share_total`, `inv3_loss_share_per_winner`, `inv4_provider_principal`,
`terminal_stuck_unbacked_claim` are known upstream bug classes and only fail with
`FUZZ_STRICT=1`.

One check always fails, even outside strict mode, and it ignores `FUZZ_INVARIANT`:
a winner's share is off by more than 1,000 atoms in a way that moved money between
traders (total right, or someone paid over fair value), and no known cause happened
in that run. That means money moved between traders in a new way. The only known cause so far is #253
(a same-side resize before the loss settles). When upstream fixes a cause and we
re-pin, or a new one is understood, update `Report::unexplained_transfer`. A quarter of the
random scenarios go straight into the slot after a bankruptcy, since plain random
action lists never reach that state. Money leaving traders for the provider or
retired funds (`inv3_loss_share_total`) is not covered by this alarm, and one #253
resize silences it for the rest of that run.

`inv2` and `inv4` are stricter than the engine promises. The engine never nets
value across source domains (spec §2.2), so when the price reverses before a
winner settles, a trader or the provider can come out a little off mark-to-market
by design (`design_stale_winner_reversal_*`). Before reporting an `inv2`/`inv4`
hit upstream, check it isn't that pattern.
