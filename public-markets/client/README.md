# Bring-up client

A smoke test against a deployed wrapper: it creates a mint and market, makes a
deposit, sends one `PermissionlessCrank` and a `SyncMaintenanceFee` with a
keeper portfolio appended, and prints the market layout facts the keeper
relies on.

`ts_fixtures` writes the byte fixtures the TypeScript client in
`frontend/src/chain` is tested against:

```sh
cargo run --manifest-path client/Cargo.toml --bin ts_fixtures -- frontend/src/chain/fixtures
```
