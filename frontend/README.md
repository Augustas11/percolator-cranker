# SOL-PERP PoC frontend

A small React trading page backed by a deterministic mock MarketClient.
Mock data only; no wallet library or network requests.

From `frontend/`, run:

- `npm ci` — install the pinned dependencies.
- `npm run dev` — start the local development server.
- `npm run build` — typecheck and build for production.
- `npm test` — run the UI smoke test with jsdom.

Connect the placeholder wallet, deposit test USDC, and open or close a position.
The mock updates prices every second and supports one SOL-PERP leg.
Mock data only; real chain calls come later through the SDK (ROADMAP item 5).
