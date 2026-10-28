# Keys

**No key in this repo, ever.** This file is the layout and the procedure.
The owner creates every key on their own devices. Nobody else (and no
agent) generates, receives or stores private keys.

The role powers are from the R2 playbook section (a) at the pin. They are
summarised here.

## Layout

| Key | Holds role(s) | Custody | Limits |
|---|---|---|---|
| **Squads multisig vault** (2-of-3) | `marketauth`, asset 0 `insurance_authority`, the USDC/SOL reserve | Owner's 3 signers: hardware wallet A, hardware wallet B, backup device in a safe | Resolves and closes the market and sets fee policy; `marketauth` cannot be burned (playbook: `P 11970`) |
| **Asset admin** (cold) | asset 0 `asset_admin` | Hardware wallet C, offline | Re-keys roles only. It cannot take a funded role without the incumbent (playbook: `P 12002-12035`). Burn it once roles are final (playbook: `P 12081-12085`) |
| **Insurance operator** (warm) | asset 0 `insurance_operator` | Separate hardware wallet, or an air-gapped signer | Signs Live insurance withdrawals, to its own token account only (`P 10936-10957`). Forward to the multisig the same day |
| **Backing authority** (warm) | asset 0 `backing_bucket_authority` | Hardware wallet, used at each roll | Holds the backing principal. It must act before every expiry |
| **Oracle pusher** (hot) | asset 0 `oracle_authority` | Pusher host only | **It is the price under AuthMark.** Damage per slot is bounded by `max_price_move_bps_per_slot`. Replacing it once there is exposure needs its own consent (playbook: `P 12027-12030`); see the incident below |
| **LP / matcher** (hot) | LP portfolio owner; co-signs every fill | Matcher host only | Can withdraw LP capital, so LP capital stays capped (`CAPITAL_PLAN.md`) |
| **Keeper** (hot) | fee payer + reward portfolio owner | Keeper host only | ≤ 3 days of SOL; rewards swept weekly |
| **Bot fee payers** (hot) | SOL for tx fees | each bot host | ≤ 3 days of SOL |

Rules:
- One key, one job. No hot key holds a role that controls value beyond its
  own bucket.
- Hot keys live only on their host (file mode 0600 or the host's secret
  store), never in git, CI, chat or notes.
- Every key's **public** key goes into the private launch config (outside
  this repo) and into the monitor's expected-roles check.

## Creation (owner only)

1. Buy or reserve 4 hardware wallets (A, B, C, plus one for the insurance
   operator and backing authority). Write each seed phrase on paper; store
   them in two places.
2. Create the Squads v4 multisig, 2-of-3 (A, B, backup). Record the vault
   address.
3. Create the hot keys **on their hosts** with `solana-keygen new` (pusher,
   matcher/LP, keeper). Copy out public keys only.
4. **Decide how `marketauth` reaches the multisig** (*to verify* in R2
   staging, on devnet):
   - Option 1: run `InitMarket` as a Squads vault transaction, so the vault
     is `marketauth` from the start (`P 7702`: the signer becomes
     `marketauth`).
   - Option 2: a bootstrap hot key inits the market and splits roles, then
     `UpdateAuthority` moves `marketauth` to the vault **last**. The new key
     must co-sign (playbook: `P 14418-14435`); confirm that a Squads vault
     can co-sign that transaction.
5. Fund the multisig (see `OWNER_ACTIONS.md`), then hot wallets from it
   with ≤ 3 days of SOL each.
6. Fill in the public keys in the launch config. `scripts/launch_market.sh`
   reads them; it never reads a private key it did not get from the owner's
   signer at run time.

**Split order matters.** `UpdateAuthority` also rewrites every asset 0 role
that still equals the old `marketauth` (playbook: `P 11978-11992`). Move
oracle, operator, backing, insurance and admin with `UpdateAssetAuthority`
**first**, and rotate `marketauth` **last**.

## Rotation (routine)

- **Hot keys:** every 90 days, and whenever a host is rebuilt.
  - `UpdateAssetAuthority` (tag 65); the new key co-signs.
  - Read the authority epoch just before signing; a mismatch fails
    `EngineStale` (playbook: `P 1817-1826`).
- **Multisig signers:** rotate inside Squads (no on-chain market change).
- After every rotation:
  - update the monitor's expected-roles list;
  - confirm one successful push, fill and crank;
  - log the change.

## Incidents

| Incident | Do now | Then |
|---|---|---|
| **Pusher key suspected leaked** | Set the matcher to **stop quoting** (no new risk). Page the owner. | Push from the real key to keep the mark honest. If the market has exposure, a rotation needs the old key's consent, which an attacker won't give. So **resolve the market** with the multisig (`ResolveMarket`, playbook: `P 11928-11954`), and relist with a new key. *To verify* the exact damage bound per slot for our params. |
| **LP key leaked** | Withdraw LP capital to the multisig if you still can, and stop the matcher. | New LP key, new LP portfolio; old portfolio closed. Loss ≤ LP bucket. |
| **Keeper key leaked** | Sweep its rewards; stop the keeper. | New keeper key. The crank is permissionless, so others can crank meanwhile. |
| **Insurance operator leaked** | It can pull Live insurance only through the gate, to its own account. Rotate it at once with the asset admin (it controls value, so the incumbent must consent; if not, resolve). | Review every withdrawal since the last known-good. |
| **Multisig signer lost** | 2-of-3 still works. | Replace the signer in Squads within 7 days. |
| **Two multisig signers lost** | Market keeps running (bots are hot keys), but no policy change, resolve or close is possible. | This is why the backup signer lives in a safe. Test recovery quarterly. |
