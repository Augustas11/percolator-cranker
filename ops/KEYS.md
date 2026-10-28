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
| **Asset admin** (cold) | asset 0 `asset_admin` | Hardware wallet C, offline | Re-keys roles. It cannot take a funded role without the incumbent (`P 12070-12095`). It can also SHUTDOWN its asset and `RestartAssetOracle` an empty asset in Recovery (playbook section a). **Keep it, don't burn it**: burning (playbook: `P 12081-12085`) gives up those recovery levers |
| **Insurance operator** (warm) | asset 0 `insurance_operator` | Separate hardware wallet, or an air-gapped signer | Signs Live insurance withdrawals, to its own token account only (`P 10936-10957`). Forward to the multisig the same day |
| **Backing authority** (warm) | asset 0 `backing_bucket_authority` | Hardware wallet, used at each roll | Holds the backing principal. It must act before every expiry |
| **Oracle pusher** (hot) | asset 0 `oracle_authority` | Pusher host only | **It is the price under AuthMark.** `max_price_move_bps_per_slot` limits how fast the *effective* price can walk toward a pushed target. It does **not** check the push against Pyth, and it is no dollar-loss bound: losses depend on exposure and on how fast we react. Replacing it once there is exposure needs its own signature (`P 12070-12095`); see the incident below |
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
   reads public keys only. Hardware and multisig signers return
   **signatures or signed transactions**, never key material. No seed or
   private key is ever exported into a script, an environment variable, a
   log or an agent's context. Hot keys stay in files on their own host,
   read only by their bot.

**Split order matters.** `UpdateAuthority` also rewrites every asset 0 role
that still equals the old `marketauth` (playbook: `P 11978-11992`). Move
oracle, operator, backing, insurance and admin with `UpdateAssetAuthority`
**first**, and rotate `marketauth` **last**.

## Rotation (routine)

Every 90 days, and whenever a host is rebuilt. The procedure depends on
the key type:

- **Market roles** (oracle, insurance operator, backing, insurance, admin):
  - `UpdateAssetAuthority` (tag 65), signed by the incumbent (or the
    admin, for an unfunded role); the new key co-signs (`P 12070-12095`,
    `P 14418-14435`);
  - read the authority epoch just before signing; a mismatch fails with
    `EngineStale` (playbook: `P 1817-1826`).
- **Portfolio owners** (LP, keeper): ownership does not rotate; tag 65
  doesn't cover portfolios. Open a new portfolio under the new key, move
  quoting or cranking to it, close the old portfolio's positions, withdraw,
  then close the old portfolio.
- **Fee payers:** move the SOL to the new key and change the bot config.
- **Multisig signers:** rotate inside Squads (no on-chain market change).
- After every rotation:
  - update the monitor's expected-roles list;
  - confirm one successful push, fill and crank;
  - log the change.

## Incidents

| Incident | Do now | Then |
|---|---|---|
| **Pusher key leaked, we still hold it** | Set the matcher to **stop quoting**; page the owner. | **Rotate at once:** tag 65, signed by the old key from a trusted machine, with the new key co-signing (`P 12070-12095`). This is a race: whoever rotates first holds the role. Push honest marks from the new key. |
| **Pusher key lost, or the attacker rotated first** | `DRAIN_ONLY` on asset 0 (multisig) to stop new risk; stop quoting. | **Resolve** with the multisig (`ResolveMarket`, `P 11928-11954`). It refuses with `EngineStale` until committed mark and funding accrual is cranked through (`P 11852-11866`), so keep the keeper cranking, then retry. Resolution does **not** undo prices already committed. Relist under a new key. Rehearse this in staging. |
| **LP key leaked** | Stop the matcher. Close the LP's positions if you still can: `Withdraw` needs a flat portfolio, no active close and PnL ≥ 0 (`E 18954-18972`). Then `Withdraw` to the **LP owner's own** token account (`P 7931-7938`), and forward it to the multisig in the same transaction if possible. | New LP key, new LP portfolio. Loss ≤ the LP bucket. If positions cannot be closed, the capital comes out through the resolved exit. |
| **Keeper key leaked** | Sweep its rewards; stop the keeper. | New keeper key. The crank is permissionless, so others can crank meanwhile. |
| **Insurance operator leaked** | It can pull Live insurance only through the gate, to its own account. **Rotate at once**, signed by the incumbent key itself (the admin alone cannot move a funded role, `P 12088-12094`). If the attacker rotated first, resolve. | Review every withdrawal since the last known-good. |
| **Multisig signer lost** | 2-of-3 still works. | Replace the signer in Squads within 7 days. |
| **Two multisig signers lost** | Market keeps running (bots are hot keys), but no policy change, resolve or close is possible. | This is why the backup signer lives in a safe. Test recovery quarterly. |
