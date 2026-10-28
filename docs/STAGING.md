# Staging

The staging market is an **operator rehearsal**:

- The owner's staging keys hold every authority (`marketauth`, the asset
  admins, oracle, insurance authority and operator, backing).
- Every role has either a bot or a written procedure in
  `docs/OPERATOR_PLAYBOOK.md`.
- It is not a product, and there is no mainnet step. Mainnet is out of
  scope for this repo.

Source citations use the playbook's tags: `P` is the wrapper, `E` is the
engine, at the pins in `scripts/pins.env`.

## 1. Local vs devnet

| | Local | Devnet |
|---|---|---|
| Cluster | `solana-test-validator` on `127.0.0.1:8899` | `-u devnet` |
| Entry point | `scripts/staging_local.sh` (in progress, separate change) | `scripts/staging_devnet_deploy.sh` (this doc) |
| Program id | build-time keypair `target/deploy/percolator_prog-keypair.json` | the owner's own program keypair |
| Upgrade authority | default local keypair | a dedicated owner key, not `marketauth` |
| SOL | airdrop freely | faucet-limited. Budget it (section 7) |
| Oracle | the local script decides | **AuthMark** with our own pusher (section 4) |
| Reset | wipe the ledger | resolve, then exits, then `CloseSlab` (section 6) |

Run everything locally first. The devnet run should change nothing but the
cluster and the keys.

## 2. Program keypair and upgrade authority

- **The program id is whatever key you deploy under.** The wrapper's
  `declare_id!` (`P 34`) is not checked at runtime: the entrypoint passes
  the runtime `program_id` through (`P 16893-16901`). Keepers find the
  program through `PERCOLATOR_PROGRAM_ID`.
- **The program keypair** matters only for the first deploy, where it fixes
  the address. Keep it offline afterwards. It cannot upgrade anything.
- **The upgrade authority** is the key that can replace the code. Use a key
  that is **not** `marketauth` and not any bot key. The staging script reads
  it from `UPGRADE_AUTHORITY_KEYPAIR` and passes `--upgrade-authority`.
  - Rehearse an upgrade once: deploy the same `.so` again. The script
    detects identical bytes and skips.
  - Do **not** pass `--final` on staging. It cannot be undone.
- **Max length.** `solana program deploy` defaults `--max-len` to the size of
  the first deploy (Solana CLI 2.0.21 help text). If an upgrade grows the
  program, run `solana program extend` first. The script exposes
  `PROGRAM_MAX_LEN`.

## 3. Market init

Order of operations. Each step names its signer.

| # | Step | Signer | Source |
|---|---|---|---|
| 1 | Create the staging SPL mint, 6 decimals. Keep its mint authority as a separate owner key | owner | `spl-token create-token` |
| 2 | Create the market account at `market_account_len_for_capacity(n)` bytes, owned by the program | payer + market keypair | `P 1688-1697`, `P 2593-2595` |
| 3 | Create the vault: the ATA of PDA `["vault", market]` for the mint | payer | `P 16161-16184` |
| 4 | `InitMarket` (tag 0) with the staging values from playbook section (b) | `marketauth` | `P 7639-7754` |
| 5 | `ConfigureAuthMark` (tag 62) for every asset, initial mark = `initial_price`. Do it **before any deposit**, because reconfiguration needs a flat market | oracle authority (`marketauth` until rotated) | `P 13407-13513`, `P 6850-6874` |
| 6 | `ConfigurePermissionlessResolve` (tag 38): 9_000 / 9_000 | `marketauth` | `P 13131-13175` |
| 7 | Fee policies: tags 37, 49 (`marketauth`); tag 55 (asset-0 insurance authority); tag 51 per domain (insurance authority) | as listed | playbook (b) |
| 8 | Split roles: `UpdateAssetAuthority` per asset (oracle, then operator, then backing, then insurance, then admin), and `UpdateAuthority` **last** | current holder + new key | `P 11957-12107`. Order: playbook (a) |
| 9 | `TopUpInsurance` (tag 9) seed, and `TopUpBackingBucket` (tag 24) with an `expiry_slot` | insurance authority / backing authority | `P 10028-10170`, `P 10423-10573` |
| 10 | Start the bots | — | section 5 |

At these pins, no CLI encodes steps 2 and 4 to 9 for a remote cluster:

- The bring-up client and the keeper's setup hard-code `127.0.0.1:8899` and
  `~/.config/solana/id.json`.
- `scripts/staging_devnet_deploy.sh` prints the exact command lines it would
  need, and marks the gap.
- The InitMarket CLI itself belongs to the staging code change, not to this
  doc.

## 4. Devnet oracle choice: AuthMark with our own pusher

The wrapper supports four oracle modes (`P 92-95`):

- Manual;
- Hybrid, which reads Pyth, Switchboard or Chainlink accounts
  (`P 5129-5168`);
- EwmaMark;
- AuthMark.

**Staging uses AuthMark**, with a pusher bot that holds only the
`oracle_authority` key. The reasons:

1. **It rehearses a real role.** Liveness under AuthMark refreshes only on
   `PushAuthMark` from `oracle_authority` (`P 13516-13605`, `P 13589`). The
   oracle role gets a bot, a hot key and a stale alarm, which is the point
   of the rehearsal.
2. **No third-party liveness dependency.**
   - Hybrid refreshes liveness only when a crank reads an *advanced*
     external price (`P 15476-15481`).
   - The account must be a Pyth `PriceUpdateV2` owned by the hard-coded
     receiver program id, with full verification (`P 4719-4723`, `P 4910`,
     `P 5139-5146`).
   - With permissionless resolve configured, a stalled devnet feed lets
     **anyone** resolve the market (`P 4810-4814`, `P 13178-13200`).
   - We do not control devnet feed freshness.
3. **Clean income accounting.** AuthMark trade fees have no mark-externality
   part, so the whole fee is budgeted to the asset (`P 15298-15304`). Under
   EwmaMark or a soft-stale Hybrid, part of each fee stays unbudgeted and
   is burned at close (`P 6632-6686`).
4. **It is a one-way door per market.** Oracle reconfiguration needs a flat
   market (`P 6850-6874`), so the choice is made at step 5 and kept.

**Cost of the choice:** AuthMark is trusted. The pusher *is* the price,
bounded only by `max_price_move_bps_per_slot` on the effective walk. Hybrid
is the mode to study before any non-rehearsal market. Rehearse it locally
against the mock price program, not on devnet.

**Pusher cadence:**

- Push at least every `stale_slots / 10` (900 slots, about 6 min, at the
  staging value).
- Alert at 25% and 50% of the window (playbook d).
- Each push carries a strictly increasing `observation_sequence`
  (`P 13565-13570`).
- Each push also carries the asset's current `authority_epoch`
  (`P 13559`).

## 5. Bots and funding

| Bot | Binary | Keys it holds | SOL budget (devnet) | Tokens |
|---|---|---|---|---|
| cranker | `cranker` (this repo) | a keeper fee payer plus its reward portfolio owner | ~1 SOL/day at worst: 64 tx/cycle × 5_000 lamports × 2_880 cycles/day. Lower `CRANKER_CYCLE_MAX_TX` / raise `CRANKER_CYCLE_MS` to cut it | none |
| oracle pusher | separate change | `oracle_authority` only | ~0.05 SOL/day at one push per 10 s | none |
| traders | `staging_traders` (separate change) | N throwaway trader keys | rent 0.0674 SOL per portfolio + fees | minted staging tokens |
| monitor | `monitor` (separate change) | none (read-only RPC) | 0 | none |
| backing roller | procedure, not yet a bot | `backing_bucket_authority` | a few tx per expiry | provider principal |
| insurance withdraw | procedure | `insurance_operator` | a few tx | — |

Funding rules:

- **Fund each key separately**, from the owner's devnet wallet. No bot ever
  holds `marketauth`, the mint authority or the upgrade authority.
- **Tokens come from the staging mint.** The owner mints to traders,
  insurance and backing; mint authority stays on the owner key.
- **Keep keypair files `0600`**, outside the repo, and mounted read-only
  into containers (`deploy/`). `.gitignore` already excludes `keys/` and
  `.env`.

## 6. Teardown

1. **Resolve.** `ResolveMarket` (tag 19, `marketauth`), or let the stale
   window lapse and call `ResolveStalePermissionless` (tag 39). The second
   path rehearses the stale runbook (`P 11928-11954`, `P 13178-13200`).
2. **Close users.** Crank until every portfolio closes through the resolved
   close plan; holders claim with `ClaimResolvedPayoutTopup` where needed
   (`P 13609-13797`). Closed portfolios' rent moves to the market account
   (`P 14818-14838`).
3. **Pay out insurance.** `WithdrawInsuranceAsset` (tag 57) per asset. In
   Resolved it pays `insurance_authority`, and anyone may submit it once no
   portfolios remain (`P 10991-11003`).
4. **Pay out backing.**
   - `WithdrawBackingBucketEarnings` (tag 52).
   - `WithdrawBackingBucket` (tag 50) while `now < expiry_slot`. Expired
     principal is forfeited (`P 10653-10658`).
5. **`CloseSlab`** (tag 13, `marketauth`). Repeat until the account becomes
   a closed-market tombstone.
   - It **waits** while any Fresh backing is unexpired (`E 966-988`).
   - It refuses while provider earnings or insurance budgets remain
     (`E 11966-11970`, `E 11363-11372`).
   - **CloseSlab burns claim-free residue.** All accounted vault value left
     at close is retired and burned through the token program
     (`E 11363-11372`, `P 11221-11268`). For a native SOL mint it goes to
     asset 0's insurance authority instead (`P 11234-11258`).
   - Only token balance beyond engine accounting is swept to `marketauth`
     (`P 11270-11279`).
   - The market account's lamports, including the closed portfolios' rent,
     are refunded to `marketauth` (`P 11330-11342`).
6. **Optional: reclaim the program rent.** `solana program close <id>`
   (upgrade authority) reclaims about 8.9 SOL. The id can never be reused.

## 7. Cost estimates

Lengths are computed with the wrapper's own
`market_account_len_for_capacity` / `portfolio_account_len_for_market_slots`
at the pin (`P 1688-1706`). Rent is from `solana rent <bytes>` (default rent:
6_960 lamports/byte including the 128-byte overhead).

| Account | Bytes | Rent-exempt SOL |
|---|---|---|
| market, capacity 1 | 3_003 | 0.0218 |
| market, capacity 2 | 4_816 | 0.0344 |
| market, capacity 3 (staging) | 6_629 | 0.0470 |
| market, capacity 4 | 8_442 | 0.0596 |
| market, capacity 8 | 15_694 | 0.1101 |
| market, capacity 14 | 26_572 | 0.1858 |
| market, capacity 32 | 59_206 | 0.4130 |
| market, capacity 64 | 117_222 | 0.8168 |
| market, capacity 128 | 233_254 | 1.6243 |
| portfolio (any market size; fixed length) | 9_563 | 0.0674 |
| backing-domain ledger (optional) | 240 | 0.0026 |
| insurance ledger (optional) | 176 | 0.0021 |
| SPL token account / ATA | 165 | 0.0020 |
| program data, `.so` of 1_272_160 bytes + 45-byte header, `--max-len` 1x | 1_272_205 | 8.8554 |
| program data at 1.5x headroom | 1_908_285 | 13.2826 |

Notes on the table:

- Each extra asset slot adds 1_813 bytes, about 0.0126 SOL.
- The program size is from a local SBF build at the pin
  (`cargo build-sbf --tools-version v1.52`). Re-measure after any re-pin.
- A deploy also needs a temporary buffer of the same size. It is refunded on
  success, so hold about 2x the program rent while deploying.

**Rough devnet budget for the rehearsal:**

| Item | SOL |
|---|---|
| deploy (1x, plus buffer while deploying) | ~18 |
| market (3 assets) and vault | ~0.05 |
| 40 traders × portfolio, plus fees | ~3 |
| cranker for a week at the default budget | ~7 |
| everything else | under 1 |

Total: about 29 SOL. About 9 SOL comes back at teardown (program close). The
portfolio rent, about 2.7 SOL, comes back through `CloseSlab`.
