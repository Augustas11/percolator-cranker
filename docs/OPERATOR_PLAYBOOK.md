# Operator playbook

For the **market operator** of a Percolator market: the person who holds the
market's authority keys. This repo is a reference keeper for reference,
dry-run and staging use. It is not a paid service and it does not cover
mainnet operation.

Every claim below cites source at the pinned SHAs (`scripts/pins.env`):

| Tag | Source | Pin |
|---|---|---|
| `P` | `percolator-prog` `src/v16_program.rs` (the Solana wrapper) | `5cb331dd` |
| `E` | `percolator` `src/v16.rs` (the risk engine) | `4db11a8c` |
| `L` | `percolator` `src/lib.rs` (engine constants) | `4db11a8c` |

`P 7639-7754` means wrapper lines 7639 to 7754. After
`./scripts/fetch_upstream.sh` the wrapper is in `upstream/percolator-prog`.
The engine source is the cargo git checkout of the pinned rev. If a pin
moves, re-check every citation.

Terms used throughout:

- **asset**: one perp market slot inside the market account. Asset `i` has
  two insurance/backing **domains**, `2i` (long) and `2i+1` (short)
  (`P 6494-6512`).
- **authority epoch**: a per-asset counter. Every privileged instruction
  carries the value it expects, and every authority handoff increments it
  (`P 652-655`, `P 1817-1834`).

---

## (a) Key layout

### Roles

| Role | Scope | Stored in | Set at |
|---|---|---|---|
| `marketauth` | whole market | `WrapperConfigV16.marketauth` (`P 533-539`) | `InitMarket` signer (`P 7702`) |
| `asset_admin` | one asset | `AssetOracleProfileV16.asset_admin` (`P 625-629`) | the activator; asset 0 gets `marketauth` (`P 1668`) |
| `insurance_authority` | one asset | profile (`P 610`) | activation args; asset 0 gets `marketauth` (`P 1650-1656`) |
| `insurance_operator` | one asset | profile (`P 611`) | same |
| `backing_bucket_authority` | one asset | profile (`P 612`) | same |
| `oracle_authority` | one asset | profile (`P 613`) | same |

At `InitMarket`, every configured asset slot gets the same bootstrap
profile. `marketauth` therefore holds every role on every initial asset
(`P 2013-2030`, `P 2582-2602`). The activator of a later asset becomes its
`asset_admin` (`P 12565`, `P 12606`, `P 12787`), and supplies the four
domain authorities. None of the four may be zero (`P 1967-1973`,
`P 12536`, `P 12752`).

### What each role can do

**`marketauth`**:

- Market-wide policy, bound to asset 0's epoch (`P 12940-12963`, enum at
  `P 12892-12937`):
  - `UpdateLiquidationFeePolicy` (tag 37): cranker share of liquidation fees;
  - `UpdateMaintenanceFeePolicy` (49): cranker share of maintenance fees;
  - `UpdateFeeRedirectPolicy` (58): share of non-asset-0 fees sent to asset 0;
  - `UpdateMarketInitFeePolicy` (59): permissionless activation fee.
- `ConfigurePermissionlessResolve` (38): the stale window and the
  force-close delay (`P 13131-13175`).
- `ResolveMarket` (19) (`P 11928-11954`) and `CloseSlab` (13)
  (`P 11099-11343`).
- Asset lifecycle: fee-free ACTIVATE, DRAIN_ONLY, RETIRE (`P 12713-12871`)
  and SHUTDOWN (`P 12627-12712`, which `asset_admin` may also do).
- Base-unit mints: `UpdateBaseUnitMints` (60) and `SwapSecondaryForPrimary`
  (61) (`P 12110-12250`).
- Rotating itself: `UpdateAuthority` (32) (`P 11957-11999`).
- Draining a *shut-down, matured, empty* asset `i >= 1`: it may withdraw
  that asset's insurance (`P 10977-10986`) and backing on the backing
  authority's behalf (`P 10625-10646`). The tokens still go only to the
  beneficiary's token account (`P 10946-10957`, `P 10398-10414`).

**`asset_admin`** (cold, per asset):

- Rotates any of its asset's roles, and itself (`P 12038-12107`). It is
  the only role that may be burned to zero (`P 12081-12085`).
- It cannot take over a role that already controls value: insurance budget,
  backing, or (for the oracle) live exposure. That rotation needs the
  incumbent's signature (`P 12002-12035`, `P 12090-12095`).
- SHUTDOWN of its own asset (`P 12651-12663`).
- `RestartAssetOracle` (69) of an empty asset in RECOVERY (`P 12308-12384`).

**`insurance_authority`** (per asset):

- `TopUpInsurance` (9) / `TopUpInsuranceDomain` (56) (`P 10028-10170`).
- `UpdateBackingFeePolicy` (51) for its asset's domains (`P 12967-13090`).
- Asset 0's `insurance_authority` also sets `UpdateTradeFeePolicy` (55),
  the market-wide base trade fee (`P 13093-13128`), and runs
  `SyncInsuranceLedger` (`P 10863-10892`).
- Beneficiary of **Resolved** insurance withdrawals (`P 10946-10950`,
  `P 10991-11003`).

**`insurance_operator`** (per asset): signs **Live**
`WithdrawInsuranceAsset` (57) and is its beneficiary (`P 10936-10950`,
`P 10977-10990`).

**`backing_bucket_authority`** (per asset):

- `TopUpBackingBucket` (24) (`P 10423-10573`).
- `WithdrawBackingBucket` (50) (`P 10577-10712`).
- `WithdrawBackingBucketEarnings` (52) (`P 10715-10827`).
- Beneficiary of all three.

**`oracle_authority`** (per asset):

- `ConfigureHybridOracle` (34), `ConfigureEwmaMark` (35) and
  `ConfigureAuthMark` (62). These are allowed only while the asset is Active
  and the whole market is flat (`P 13267-13272`, `P 13450-13452`, gate at
  `P 6850-6874`).
- `PushEwmaMark` (36) / `PushAuthMark` (63) (`P 13516-13605`).
- It owns no tokens. Once the asset has exposure, its consent is needed to
  replace it (`P 12027-12030`).

**Permissionless:**

- crank, `ResolveStalePermissionless` (39), `ForceCloseAbandonedAsset` (64);
- activation of a new asset when a market-init fee is set
  (`P 12447-12458`).

### Recommended custody

| Role | Custody | Why (source) |
|---|---|---|
| `marketauth` | multisig | Resolves and closes the market, sets fee shares and the stale window (above). Rotation needs the new key to co-sign (`P 14418-14435`), so a multisig PDA cannot be set by mistake. It can't be burned (`P 11970`, `allow_burn = false`). |
| `asset_admin` | cold, one per asset | Only re-keys roles; it is fenced off from funded roles (`P 12090-12095`). It can be burned once roles are final (`P 12081-12085`). |
| `insurance_authority` | cold or multisig | Sets trade and backing fee policy (`P 13109`, `P 12996`) and receives Resolved insurance. |
| `insurance_operator` | warm | It can move Live insurance out, so it is the key a thief wants. Live withdrawals are gated (section e). |
| `backing_bucket_authority` | warm | Holds the backing principal. Needed before every expiry to roll or withdraw (section d). |
| `oracle_authority` | **hot**, on the pusher host | Its power is limited to marks for its own asset (above). AuthMark makes it the market's price, though, so watch it. |

**Order matters when you split roles off.** `UpdateAuthority` also rewrites
every asset-0 role that still equals the *old* `marketauth` (`P 11978-11992`).
Move asset 0's oracle, insurance, backing and admin roles to their own keys
with `UpdateAssetAuthority` **first**. Rotate `marketauth` to the multisig
**last**. Otherwise the hot oracle role lands on the multisig.

### Rotation and `authority_epoch`

- `UpdateAuthority { authority_epoch, new_pubkey }` (tag 32):
  - the current `marketauth` signs and the new key co-signs;
  - it advances **asset 0's** epoch (`P 11993`).
- `UpdateAssetAuthority { asset_index, market_id, authority_epoch, kind, new_pubkey }` (tag 65):
  - `kind`: 0 admin, 1 insurance, 2 operator, 3 backing, 4 oracle
    (`P 5847-5851`);
  - the signer is the asset admin or the incumbent of that role;
  - the new key co-signs, unless this burns the admin;
  - it advances **that asset's** epoch (`P 12096`).
- Every privileged instruction passes the epoch it expects. A mismatch is
  `EngineStale` (`P 1817-1826`). Read the epoch from the market account just
  before you sign.
- **A Live insurance withdrawal also consumes an epoch** (`P 11050-11053`).
  The insurance tooling must re-read it after every withdrawal.
- `market_id` binds the asset generation. A retired then re-activated slot
  gets a new id, so old intents fail with `AssetGenerationMismatch`
  (`P 6876-6893`).

---

## (b) Market parameters

### `InitMarket` (tag 0)

The signer becomes `marketauth`. The account must already exist, be owned
by the program, and be at least `market_account_len_for_capacity(n)` bytes
(`P 2582-2602`).

Validation happens in two places:

- the wrapper (`P 7672-7678`);
- the engine's shape check (`E 4824-4884`) plus the exact solvency envelope
  (`E 4645-4822`).

A config that fails the engine check comes back as `InvalidAccountData`.

The recommended values below pass the engine check at the pin. They were
run through `V16Config::validate_public_user_fund` with `n = 3`. The "tested"
column is the set the load scenario uses
(`crates/cranker/src/scenario/live_shaped.rs`, `init_market_params`).
Staging widens accrual and close lifetime for devnet latency.

| Field | Meaning and limits | Tested | **Staging** | Reasoning |
|---|---|---|---|---|
| `max_portfolio_assets` | Active legs per portfolio; also the number of assets live at init (`E 4396-4403`). 1 to 14 (`P 110`, `P 7673-7674`) | 3 | **3** | Two to three assets exercise cross-margin and one SHUTDOWN without bloating CU. |
| `h_min`, `h_max` | `h_max > 0`, `h_min <= h_max` (`E 4832`), `h_max <= BOUND_SCALE` (`P 7675`). At this pin `h_max` only feeds the backing freshness horizon (`E 9320-9324`); `h_min` is validated only | 0 / 10 | **0 / 150** | Harmless. Keep `h_min = 0`. |
| `initial_price` | `1..=MAX_ORACLE_PRICE` (1e12) (`P 7697`, `L 18`) | 1_000_000 | **1_000_000** | Price in engine units. Must match the first oracle mark. |
| `min_nonzero_mm_req` / `min_nonzero_im_req` | atom floors, `0 < mm < im` (`E 4835`) | 1_000 / 2_000 | **1_000 / 2_000** | Floors of 1 / 2 fail the envelope once there is a liquidation fee. |
| `maintenance_margin_bps` / `initial_margin_bps` | `mm <= im <= 10_000` (`E 4838-4839`) | 500 / 1_000 | **500 / 1_000** | 10x initial leverage, liquidation on about a 5% adverse move. |
| `max_trading_fee_bps` | `<= 10_000` (`E 4840`). Caps the caller fee and every fee policy (`P 7672`, `P 13110`, `P 13013`, `P 15286-15289`) | 100 | **100** | 1% ceiling. |
| `trade_fee_base_bps` | `<= max_trading_fee_bps` (`P 7672`). The fee floor: charged fee = `max(caller, base)` (`P 15285`) | 0 | **5** | Non-zero so staging income is visible. |
| `liquidation_fee_bps` / `liquidation_fee_cap` / `min_liquidation_abs` | `fee = clamp(ceil(notional * bps / 1e4), min_abs, cap)` (`E 20852-20890`). `min_abs <= cap` (`E 4842`). Part of the envelope | 50 / 1e10 / 0 | **50 / 1e10 / 0** | 0.5% liquidation fee. |
| `max_price_move_bps_per_slot` | Cap on the effective-price walk per slot. `> 0` (`E 4847`). Envelope term `move * dt` (`E 4646-4648`) | 20 | **5** | Combined with `dt` below, the per-accrual budget stays at 320 bps (< 500 mm). |
| `max_accrual_dt_slots` | Slots one accrual can cover. `> 0` (`E 4844`). Envelope term | 16 | **64** | Devnet crank cycles are seconds apart (about 2.5 slots/s), so 16 forces many catch-up steps. |
| `max_abs_funding_e9_per_slot` | `<= 10_000` (`E 4846`). Envelope term | 10_000 | **10_000** | Funding stays nonzero so it gets exercised. |
| `min_funding_lifetime_slots` | `>= max_accrual_dt_slots` (`E 4845`) | 16 | **64** | Tied to `dt`. |
| `max_account_b_settlement_chunks`, `max_bankrupt_close_chunks` | `> 0` (`E 4848-4849`) | 4 / 4 | **4 / 4** | Bounded close progress per call. |
| `max_bankrupt_close_lifetime_slots` | `> 0` (`E 4850`). An expired close routes to Recovery (engine `spec.md` item 23) | 20 | **1_500** | 20 slots is 8 s on devnet. A slow keeper would trip market-wide Recovery. 1_500 is about 10 min. |
| `public_b_chunk_atoms` | `> 0` (`E 4852`) | `MAX_VAULT_TVL` | **`MAX_VAULT_TVL`** | No artificial chunking. |
| `maintenance_fee_per_slot` | Atoms per slot per portfolio. `<= MAX_PROTOCOL_FEE_ABS` (`P 7676`) | 0 | **1** | 1 atom/slot is about 216k atoms/day per portfolio. Small, but visible. |

Engine settings an operator **cannot** change at this pin, because
`public_user_fund` fixes them (`E 4405-4449`):

- the backing utilization curve (base 0, kink 8_000, slopes 0);
- `backing_freshness_buckets = 1` (`E 28`).

**No configurable OI cap exists.** The only bound is the global
`MAX_OI_SIDE_Q = 1e14` per side (`L 31`), checked after every trade
(`P 14707-14724`).

### Policies set after init

| Policy | Instruction / signer | Bounds | Staging value | Why |
|---|---|---|---|---|
| Liquidation cranker share | tag 37 / marketauth | `<= 10_000` (`P 12900-12905`) | **5_000** | Pays the keeper; the rest goes to insurance (`P 14293-14320`). |
| Maintenance cranker share | tag 49 / marketauth | `<= 10_000` | **2_000** | Rewards `SyncMaintenanceFee` callers (`P 11630-11690`). |
| Fee redirect to asset 0 | tag 58 / marketauth | `<= 10_000` (`P 6595-6618`) | **0** | Keep per-asset income separate while rehearsing. |
| Market-init fee | tag 59 / marketauth | fits u64. Doubles every 32 slots of index (`P 5543-5557`) | **0** (permissionless activation off) | Turn it on only to rehearse permissionless listing. |
| Base trade fee | tag 55 / asset-0 insurance_authority | `<= max_trading_fee_bps`, `<= 10_000` (`P 13110-13115`) | **5** | As above. |
| Backing fee | tag 51 / insurance_authority of that asset | `fee <= min(max_trading_fee_bps, 10_000)`; `share <= 10_000`; `share == 0` when fee is 0 (`P 13009-13017`). It cannot change while the bucket holds value (`P 5504-5520`, `P 13047-13056`) | **fee 10, share 2_000** | Providers keep 80%, insurance keeps 20% (section f). |
| Permissionless resolve | tag 38 / marketauth | `stale_slots` in `1..=6_480_000` and `force_close_delay_slots` in `1..=10_000_000` (`P 104-105`, `P 13145-13151`). Rejected if already stale (`P 13161-13163`) | **9_000 / 9_000** (about 1 h each) | 0 disables both permissionless resolve and SHUTDOWN (`P 12628`). Set it, so the shutdown runbook can be rehearsed. |

Each policy carries a strictly increasing `policy_sequence` per lane
(`P 1809-1815`, `P 6199-6290`). Keep a counter per lane, or read it from the
market account.

### Oracle modes

There are four modes (`P 92-95`):

- **Manual** (0) is the default at init. Nothing refreshes its liveness, so
  with permissionless resolve set, a Manual market goes stale on schedule
  (`P 4810-4814`, `P 14118-14126`).
- **Hybrid** (1) reads Pyth, Switchboard or Chainlink accounts
  (`P 5129-5168`).
- **EwmaMark** (2) and **AuthMark** (3) are pushed by `oracle_authority`.

Choose the mode before any trading. Reconfiguration needs a flat market
(`P 6850-6874`). Staging uses AuthMark (`docs/STAGING.md`).

---

## (c) Asset listing and shutdown

`UpdateAssetLifecycle` (tag 40) actions (`P 5853-5856`):

| Action | Who | Effect |
|---|---|---|
| ACTIVATE (0), append at `asset_index == configured` | marketauth (fee-free), or anyone paying the init fee | Grows the account and activates a new slot. The fee is `market_init_fee * 2^(index/32)` and is capped by the caller's `max_init_fee` (`P 12447-12458`). It is deposited into **asset 0's** insurance budget (`P 12608-12611`, `P 6514-6531`). The caller must pre-fund lamports, because the program reallocs but transfers nothing (`P 12477-12480`). |
| ACTIVATE, reuse a RETIRED slot | marketauth, or anyone paying the fee when free slots exist | New `market_id`; control sequences reset (`P 12519-12575`, `P 12751-12795`). |
| DRAIN_ONLY (1) | marketauth | Positions can only be reduced (`P 12800-12807`). |
| SHUTDOWN (3) | marketauth or the asset's asset_admin. Needs `force_close_delay_slots != 0` and a market that is not stale (`P 12627-12712`) | Accrues committed funding, freezes the mark at the current effective price, and moves the asset to RECOVERY (`P 12667-12699`). |
| RETIRE (2) | marketauth; not asset 0 (`P 12808-12811`) | Needs an empty asset (`E 19690-19725`). Expires its lapsed backing and frees the slot (`P 12826-12845`). |

Shutdown sequence for asset `i >= 1`:

1. **SHUTDOWN.** This records the shutdown slot as `last_good_oracle_slot`
   (`P 12693`).
2. **Wait `force_close_delay_slots`.** Owners close positions themselves
   in the meantime.
3. **`ForceCloseAbandonedAsset` (tag 64).** This is permissionless after the
   delay. It closes `close_q` between two opposite accounts at the frozen
   mark (`P 9043-9100`).
4. **Drain.** Once the asset is empty and matured, marketauth may drain its
   insurance and backing to their beneficiaries (`P 6013-6015`,
   `P 10977-10986`).
5. **RETIRE** the slot, or **`RestartAssetOracle`** (asset_admin) to relist
   it under a fresh `market_id`. Restart keeps the remaining insurance
   budgets (`E 19791`, `P 12308-12384`).

---

## (d) Incident runbook

### Oracle stale

- **Clock.** `permissionless_stale_matured` is true once
  `now - last_good_oracle_slot >= permissionless_resolve_stale_slots`
  (`P 4810-4814`). The market-wide clock is asset 0's liveness, mirrored
  into the config (`P 6311-6365`, `P 14118-14126`).
- **What refreshes it:**
  - AuthMark/EwmaMark: a push (`P 13589`);
  - Hybrid: a crank that reads an advanced external price (`P 15476-15481`);
  - Manual: nothing.
- **Effects once matured:**
  - Live withdrawals, policy and lifecycle changes, top-ups and liquidating
    cranks return `OracleStale` (call sites of
    `reject_permissionless_resolve_matured_*`, `P 5921-5946`;
    `P 13890-13896`);
  - non-asset-0 oracle updates are refused (`P 5948-5957`);
  - **anyone** may call `ResolveStalePermissionless` (tag 39) and resolve
    the market (`P 13178-13200`).
- **Recovery while nobody has resolved.** An asset-0 push is still accepted
  (the block applies only to `asset_index != 0`, `P 5953`) and resets the
  clock. Push immediately.
- **Page well inside the window.** Alert at 25% and 50% of `stale_slots`
  without a successful push.

### Stuck account

The keeper quarantines an account that keeps failing and keeps the rest
progressing (`docs/RUNBOOK.md`, section 6). Triage by error:

| Error | Likely cause | Action |
|---|---|---|
| `OracleStale` | see above | Push the mark. |
| `EngineStale` on a refresh/liquidate path | lapsed backing (next section) or a stale `now_slot` | Check the backing buckets. |
| `AssetGenerationMismatch` | the slot was re-listed | Rebuild the intent with the new `market_id`. |
| `EngineLockActive` | a gate is closed (withdraw health, lifecycle) | Read the gate list in section (e). |

### Known engine-side liveness issue: unreferenced lapsed backing

**What happens:**

1. A backing bucket stays `Fresh` after `expiry_slot`, and no account's
   source domains reference it.
2. Live code only expires buckets reached through an account's own source
   domains (`E 9486-9565`, `E 14731-14733`).
3. The asset-level scan runs on Resolved paths (`E 15321-15330`,
   `E 11880-11934`) and on RETIRE of an empty asset (`E 19704-19705`).
4. Any path that needs that domain's realizable support or a new lien calls
   `validate_source_domain_ledger_current`. That returns `Stale` for a Fresh
   bucket past expiry (`E 9202-9210`; callers at `E 10817`, `E 10876`,
   `E 10916`, `E 11691`, `E 12240`, `E 12287`).
5. An account whose refresh would attribute positive PnL to that domain then
   cannot be refreshed or liquidated. Asset 0 cannot be retired
   (`P 12809`), and RETIRE needs an empty asset, so in practice this clears
   at **market resolution**.

**Mitigations, each checked against source:**

1. **Never let backing lapse unused.** Before `expiry_slot`, withdraw all
   fresh unliened principal with `WithdrawBackingBucket`.
   - Allowed only while `now < expiry_slot` (`P 10653-10658`, `P 5497-5502`).
   - Allowed only while the domain stays fully backed (`E 9419-9421`).
   - A bucket with no liened, consumed or impaired amounts goes to `Empty`
     and its expiry resets (`E 3455-3464`).
2. **Rolling means withdraw, then top up.** A `Fresh` bucket only accepts
   top-ups with the *same* `expiry_slot` (`E 3400-3406`), so there is no
   in-place extension. Once Empty, a top-up with a new expiry restarts it
   (`E 3401-3403`).
3. **If principal is liened** (`valid_liened_backing_num > 0`), it cannot all
   be withdrawn (`E 3447-3453`). But the lien means an account references
   the domain, and the account-scoped scan will expire it (`E 9554-9565`).
   Here the risk is forfeiture, not the wedge.
4. **Monitor alerts** (the monitor binary, built separately). Read each
   asset's `backing_long` / `backing_short` status and `expiry_slot` from
   the market account:
   - **warn**: `Fresh` and `expiry_slot - current_slot < roll_lead_slots`;
   - **page**: `Fresh` and `expiry_slot <= current_slot` while the market is
     Live. This is the wedge precondition.
5. **If it happens anyway:** quarantine keeps the keeper healthy. Plan an
   orderly resolution (`ResolveMarket`); terminal settlement then expires
   the bucket (`E 11925-11934`).

### Recovery

- **Per asset (lifecycle RECOVERY):** reached by SHUTDOWN. Follow the
  shutdown sequence in section (c).
- **Market-wide (mode `Recovery`):**
  - The crank declares it permissionlessly when bounded progress is
    impossible. The reasons are listed at `E 4316-4325` and the declaration
    is at `E 14595-14614`.
  - In Recovery, each crank either forfeits a released recovery leg or
    resolves the market (`E 15097-15122`). `ForfeitRecoveryLeg` (tag 43) is
    the owner route (`P 11500-11538`).
  - The operator cannot "un-recover". Plan for resolution.
  - Maintenance fees stop accruing in Recovery (`E 18901-18903`).

### Resolution and exits

1. **Resolve.** Use `ResolveMarket` (marketauth, `P 11928-11954`), or
   tag 39 once stale.
2. **Close users out.** Users exit through the crank's `CloseResolved` plan
   and `ClaimResolvedPayoutTopup` (`P 13609-13797`).
3. **Pay out insurance and backing.** Once there are no portfolios and
   `c_tot == 0`, anyone may deliver insurance and backing to their recorded
   beneficiaries. Resolved routes don't need the authority's signature
   (`P 10379-10383`, `P 10936-10940`).
4. **`CloseSlab`** (`P 11099-11343`). Requirements:
   - Resolved mode, no portfolios, `c_tot == 0` (`P 11133-11139`);
   - no unexpired Fresh backing, so the scan **waits** for expiry
     (`E 966-988`);
   - zero provider earnings and fresh backing (`E 11966-11970`);
   - zero remaining insurance budgets (`E 11363-11372`).
   It runs in steps, one per call, until it reports `ReadyToClose`.

---

## (e) Where operator income actually comes from

All protocol fees land in the engine's `insurance` balance. Each fee has a
per-domain **budget** next to it. Only budgeted insurance can be withdrawn
during Live, and only through the gates below.

| Source | How it is charged | Where it accrues |
|---|---|---|
| Trade fee | `max(caller fee_bps, trade_fee_base_bps)` of notional, charged to **each** side (`P 15285`, `E 17657-17660`), capital to insurance (`E 18194-18230`) | That asset's long/short budgets (`P 6620-6630`, `P 8192-8201`), minus the fee redirect (`P 6595-6618`), minus the **mark-externality part**, which stays in insurance unbudgeted (`P 6632-6686`; comment at `P 6648-6650`). AuthMark and fresh Hybrid have no externality part (`P 15298-15315`). |
| Liquidation fee | engine formula (`E 20852-20890`); the increase in insurance is the retained fee (`P 14275-14279`) | Cranker share first (`P 14293-14305`), the rest to the asset's budgets (`P 14315-14320`). **Unbudgeted** when the liquidation price was trade-driven (`P 14312-14314`, `P 15228-15235`). |
| Maintenance fee | `maintenance_fee_per_slot * elapsed` per portfolio (`P 6816-6830`, `E 18894`) | Cranker share, then **asset 0's** budgets only (`P 6790-6814`, `P 11630-11690`). |
| Backing fee | `fee_bps` of the new counterparty-backed lien on a risk-increasing fill (`P 14987-15037`, `E 47-74`) | `insurance_share_bps` to the insurance budget, the rest to the provider's bucket earnings (`E 10165-10232`). |
| Market-init fee | permissionless activation (`P 12447-12458`) | Asset 0's budgets (`P 12608-12611`). |
| Fee redirect | `fee_redirect_to_market_0_bps` of fees on assets `>= 1` (`P 6595-6618`) | Asset 0's budgets. |
| Insurance top-ups | `TopUpInsurance*` (`P 10028-10170`) | Your own principal, budgeted (`E 11177-11204`). |
| Portfolio rent | `ClosePortfolio` moves the portfolio's lamports into the market account (`P 14818-14838`, `P 10001`) | Refunded to `marketauth` at `CloseSlab` (`P 11330-11342`). About 0.067 SOL per closed portfolio (`docs/STAGING.md`). |

**What the operator can withdraw:**

- **Live, `WithdrawInsuranceAsset` (tag 57).** Signer: the asset's
  `insurance_operator`, paid to its own token account (`P 10936-10957`).
  Gates, from source:
  1. The market is not stale-matured (`P 6016`).
  2. Market-wide: `negative_pnl_account_count == 0`, no threshold stress,
     no loss-stale, no Recovery (`P 6017-6037`, `P 5522-5534`).
  3. The domain has no pending loss barrier, and the asset is neither
     loss-stale nor showing an exposed target/effective lag
     (`P 6018-6022`, `P 6038-6041`).
  4. `amount <= min(sum over both domains of (budget - spent - reserved),
     insurance - source_reserved_total, vault)`
     (`P 6545-6561`, `E 11427-11441`, `E 11449-11454`).
  5. The expected authority epoch matches, and is consumed
     (`P 10988`, `P 11050-11053`).
  - Exception: for a shut-down, matured, empty asset `i >= 1`, marketauth
    may sign gates 1 to 3 away (`P 6013-6015`, `P 10977-10986`).
- **Resolved:** the same instruction pays the asset's
  `insurance_authority`. It needs no portfolios and `c_tot == 0`. Anyone
  may submit it (`P 10991-11003`). It first re-credits claim-free residual
  to the asset (`P 11001-11003`).
- **Backing earnings** (only if you are the backing authority):
  `WithdrawBackingBucketEarnings` (tag 52) up to the bucket's
  `utilization_fee_earnings`. It runs under the same Live health gate
  (`P 10761-10790`).
- **Cranker rewards:** credited as *capital* into the keeper's own portfolio
  (`P 14301-14303`). It leaves through a normal `Withdraw`.

**What the operator cannot take out:**

- **Trader capital and PnL.** Nothing in the authority set debits them.
- **Unbudgeted insurance:** the mark-externality fees, non-reclaimable
  liquidation fees, budget already spent covering bankruptcies, and reserved
  credit (`E 11427-11441`). At `CloseSlab` the engine retires *all*
  accounted vault value left after budgets are zero, and the wrapper
  **burns** it (`E 11363-11372`, `P 11221-11268`). With a native SOL mint it
  goes to asset 0's insurance authority instead (`P 11203-11208`,
  `P 11234-11258`).
- **Lapsed backing principal.** Expiry zeroes the fresh principal into
  claim-free residual (`E 3468-3497`), which is burned at `CloseSlab`.
- **Provider earnings**, unless you are the backing authority. They are
  never redirected.
- **The only `CloseSlab` sweep to `marketauth`** is token-account balance
  *in excess of* engine accounting, such as direct transfers
  (`P 11222-11225`, `P 11270-11279`), plus the market account's lamports
  (`P 11330-11342`).

**Plain summary.**

- Fees accrue to insurance.
- During Live, insurance comes out only through the `insurance_operator`,
  and only while the whole market is healthy.
- Never more than the domain's unspent, unreserved budget.
- Each withdrawal burns an authority epoch.
- After resolution, the remainder goes to `insurance_authority`.
- Whatever is not budgeted is burned at close.

---

## (f) Backing providers

**Compensation:**

- **The fee.** A provider earns the backing fee on risk-increasing fills
  whose counterparty credit is backed by its domain:
  `fee = ceil(lien_delta * fee_bps / 1e4)`, split
  `insurance = floor(fee * share / 1e4)` and `provider = fee - insurance`
  (`E 47-74`).
- **Where the provider share goes.** It is credited to
  `utilization_fee_earnings`, and only while the bucket is Fresh and
  unexpired. Otherwise the trade fails with `LockActive` (`E 10207-10220`).
  The insurance share is budgeted to the same domain (`E 10222-10232`).
- **Fee cap.** Takers bound the fee with `backing_fee_cap_bps`
  (`P 15017-15023`).
- **Who sets the fee.** The asset's `insurance_authority`, via
  `UpdateBackingFeePolicy`. The fee cannot move while the bucket holds any
  value (`P 5504-5520`).
- **Top-ups pin the policy.** Every top-up must restate the current
  `(fee_bps, insurance_share_bps)` or it is rejected (`P 10470-10485`). A
  provider never deposits under terms it did not sign.

**Risks to providers:**

- **Lapse forfeiture.**
  - After `expiry_slot` the principal can no longer be withdrawn
    (`P 10653-10658`).
  - Expiry zeroes fresh unliened principal; it is not refunded
    (`E 3468-3497`).
  - Roll or withdraw before expiry, as described in section (d).
- **Impairment.**
  - Liened backing that expires becomes `Impaired` (`E 3485-3497`).
  - Backing consumed to cover a counterparty loss becomes a provider
    receivable, not principal (`E 3395-3398`, `E 3407-3412`;
    consumption at `E 11678`).
  - `SyncBackingDomainLedger` records realized loss in the optional ledger
    account (`P 674-717`, `P 10830-10861`).
- **Withdraw health gate.** Live withdrawals of principal and earnings stop
  whenever any account market-wide has negative PnL, or under stress,
  loss-stale or Recovery (`P 6007-6043`). A provider can be locked in
  during exactly the episodes it is paid for.
- **Resolved exit.**
  - Principal and earnings can be withdrawn only after every portfolio is
    closed and `c_tot == 0` (`P 10625-10633`, `P 10761-10769`).
  - Principal still needs `now < expiry_slot` (`P 10653-10658`). A bucket
    that expires during wind-down is forfeited.
  - `CloseSlab` waits for unexpired buckets, then expires them
    (`E 966-988`).
- **Resolved surplus withdraw.** A way for providers to withdraw resolved
  surplus is not in the pinned wrapper or engine. It exists only in
  upstream's unmerged change stack, so do not count on it at these pins.

---

## Not verified here

- That SHUTDOWN, then `ForceCloseAbandonedAsset`, then RETIRE can empty an
  asset whose accounts are wedged by unreferenced lapsed backing. The
  force-close path's own refresh requirements were not traced.
- The rent-exemption requirement on append-activation realloc
  (pre-funding). It follows from Solana's runtime rent check, not from
  wrapper code.
