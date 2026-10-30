# Devnet test-USDC faucet
Node HTTP service: `GET /health`, `GET /claim?wallet=...` or JSON `POST /claim`.
Requires devnet, a 6-decimal classic SPL mint, and its mint-authority payer.
The claimant must create their test-USDC ATA first using their own SOL.
Required: `FAUCET_KEYPAIR` (path to payer JSON); never expose this file.
Optional: `FAUCET_RPC_URL`, `FAUCET_MINT`, `FAUCET_BIND` (127.0.0.1:8092), `FAUCET_STATE` (faucet-state.json).
Limits: `FAUCET_DAILY_CAP` (200/UTC day), `FAUCET_AMOUNT_USDC` (1000, integer ≤100000).
`FAUCET_MIN_PAYER_LAMPORTS` defaults to 3000000000; one claim/wallet/rolling 24h.
Run: `npm ci && npm test && npm run build && FAUCET_KEYPAIR=/path/payer.json npm start`.
Persist the state file; corrupt state refuses startup. No token accounts are created.
