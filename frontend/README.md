# SOL-PERP PoC frontend

From `frontend/`, run `npm ci`, then `npm run dev` for the deterministic mock,
or `VITE_MARKET_CLIENT=devnet npm run dev` for devnet with Phantom or Solflare.
Copy `.env.example` to `.env.local` to override the RPC and account addresses.

Fund your wallet with devnet SOL using `solana airdrop 2 <wallet> --url devnet`
or https://faucet.solana.com. For test USDC, ask the mint owner to run
`./scripts/staging_usdc_faucet.sh <wallet> <amount> --yes` from the repository
root (`LAUNCH_RUN_DIR` points it at the devnet launch run); there is no web faucet for this mint. Connect and deposit to create a portfolio.

Trades are `TradeCpi` against our LP's devnet matcher program, which is the
default in `src/chain/config.ts`. The tester wallet is the only signer. Fills
need the mark pusher running; when it is down, the matcher refuses with custom
error 9 (stale) and the engine with `EngineLockActive`.
Entry price and unrealized PnL display as “—” because the engine stores no entry
price. Fills show only this client's landed transactions and the mark at send time.

`npm test` runs tests; `npm run build` typechecks and builds for production.

`src/chain/` is the client for upstream Percolator v16 (wrapper `5cb331dd`). Its
tests match byte for byte the fixtures that the Rust `client/` crate emits from the
pinned program crate. Regenerate them from the repository root with
`cargo run --manifest-path client/Cargo.toml --bin ts_fixtures -- frontend/src/chain/fixtures`.
Refresh the `.bin` account dumps first with
`solana account -u devnet <key> --output json-compact`.

`npm run roundtrip` runs a real devnet round trip. It creates a portfolio,
deposits, opens, closes and withdraws, and prints the signatures. It needs a
funded throwaway wallet (`TESTER_KEYPAIR`). Its trades take the same `TradeCpi`
path as the browser.
Test tokens only, no real funds.
