# Owner actions

Only the owner can do these. Work through them in order; the lead time is
how early each must start before launch. Upstream has no launch date, so
**start the long ones now**.

| # | Action | Lead time | Done when |
|---|---|---|---|
| 1 | **Engage a lawyer** with `LEGAL_QUESTIONS.md` | 3–6 weeks (finding counsel ~1–2 weeks, plus answers) | Written answers on status, geo-block, KYC, marketing |
| 2 | **Decide the operating entity** (from the lawyer's answer) | 2–8 weeks if a new entity is needed | Entity exists, or a personal start is approved |
| 3 | **Buy hardware wallets** (4) | 1 week (shipping) | Seeds on paper, stored in 2 places |
| 4 | **Create the Squads multisig** 2-of-3 (`KEYS.md`) | 1 day | Vault address recorded in the private launch config |
| 5 | **Rent servers:** 2 small VMs (primary + standby) in different regions | 1 day | SSH access; bots install from the R2 hosting recipe |
| 6 | **Sign up for a paid RPC** (mainnet, with priority or staked send) plus a second provider for failover | 1–3 days | Keys in the hosts' secret stores; quota checked |
| 7 | **Open the hedge-venue account** (if counsel says it is allowed) | 1–2 weeks (access checks) | Account funded with the $20k hedge bucket |
| 8 | **Fund the multisig:** ~$70k USDC + ~20 SOL (the hedge $20k goes to the venue) | 1–3 days (exchange withdrawals) | Balances visible in Squads |
| 9 | **Create the hot keys** on their hosts; give Ops the public keys | 1 hour | Launch config complete |
| 10 | **Approve market parameters** (`LAUNCH_RUNBOOK.md` table) | 1 day | Signed off in the config |
| 11 | **Recruit testers** (10–20, for the devnet PoC beta first) | 2–4 weeks | Names, wallets and a channel to reach them |
| 12 | **Recruit first traders and a second market maker** (optional, later) | 4–8 weeks | Intros made |
| 13 | **Prepare announcement accounts:** brand and founder posts drafted, a support channel, a status page | 1 week | Drafts ready, **not posted** |
| 14 | **Set alert routing** to your phone (monitor pages) | 1 hour | A test page received |
| 15 | **Quarterly drill:** sign a no-op multisig proposal with the backup signer | 1 hour | Done before launch, then every quarter |

**Critical path:** lawyer (1–2) → entity → fund (8). Everything else fits
inside it.
