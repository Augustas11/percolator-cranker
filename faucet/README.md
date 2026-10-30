# Devnet test-USDC faucet
Node HTTP service: `GET /health`, `GET /claim?wallet=...` or JSON `POST /claim`.
Requires devnet, a 6-decimal classic SPL mint, and its mint-authority payer.
Claims also send SOL to wallets below the drip amount when the SOL cap and payer floor allow it, creating a missing test-USDC ATA in the same transaction.
Otherwise the claimant must create their ATA first using their own SOL; USDC claims continue without a SOL drip.
Required: `FAUCET_KEYPAIR` (path to payer JSON); never expose this file.
Optional: `FAUCET_RPC_URL`, `FAUCET_MINT`, `FAUCET_BIND` (127.0.0.1:8092), `FAUCET_STATE` (faucet-state.json).
Limits: `FAUCET_DAILY_CAP` (200/UTC day), `FAUCET_AMOUNT_USDC` (1000, integer ≤100000).
`FAUCET_SOL_LAMPORTS` (80000000 = 0.08 SOL) is the drip amount; `FAUCET_SOL_DAILY_CAP` (12) limits SOL drips per UTC day.
`FAUCET_MIN_PAYER_LAMPORTS` (3000000000) gates SOL drips only: payer must hold at least this floor plus the drip amount.
One claim/wallet/rolling 24h includes both USDC and any SOL; responses include `sol` in SOL units.
Health includes `sol_drips_today`, `sol_daily_cap`, and `sol_paused` from the last seen payer balance without RPC calls.
Run: `npm ci && npm test && npm run build && FAUCET_KEYPAIR=/path/payer.json npm start`.
Persist the state file; corrupt state refuses startup. Old state files without `sol_count` start that counter at zero.
