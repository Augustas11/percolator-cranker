# Keeper (`drip` crate)

Each cycle the keeper discovers every portfolio of a market, decodes it with
the wrapper's own decoders, and sends `PermissionlessCrank` transactions
within a wall-clock, compute-unit and transaction budget. Liquidatable
accounts, accounts with pending work and quiet ones share the budget 2:1:1,
and one failing account never blocks the queue.

```sh
./scripts/fetch_upstream.sh                                   # once, from the repository root
cargo run --release --manifest-path cranker/Cargo.toml --bin drip
cargo test --release --manifest-path cranker/Cargo.toml       # validator tests self-skip
```

The validator integration tests run with `PERCOLATOR_TEST_RPC` and
`PERCOLATOR_PROGRAM_ID` set (`cargo test -- --include-ignored`).
`build.rs` refuses to build unless `upstream/percolator-prog` is at the commit
pinned in `scripts/pins.env`.
