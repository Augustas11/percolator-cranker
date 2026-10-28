# Questions for a lawyer

These are questions only. Nothing here is legal advice. Bring this list,
`MONEY_MODEL.md` (how we earn) and `KEYS.md` (what we control) to the first
meeting.

**Facts to give counsel:**
- We would create and run one perpetual-futures market (SOL/USD, USDC
  collateral, leverage up to 5x) on a public Solana program that we did not
  write and do not control.
- We hold the market's admin keys and set its fees.
- We publish the price (from Pyth), and we are the only liquidity provider,
  so we take the other side of every trade.
- We also run the liquidation keeper. Fees accrue to an on-chain insurance
  fund that we can withdraw under conditions.

## Status and jurisdiction

1. Given those facts, what are we in each relevant jurisdiction: an
   exchange, a broker, a dealer, a market maker, a swap dealer, a software
   provider, or something else?
2. Does being the **only counterparty** to every trade change that, compared
   with only listing the market?
3. Does **publishing the price** that settles the contracts create extra
   duties or liability (benchmark rules, manipulation risk)?
4. Which entity should operate this, and where? Is a personal operation
   acceptable for a small, invite-only start?
5. Which jurisdictions' laws apply when the program is on a public chain and
   users are anonymous wallets?

## Access and geo-blocking

6. Which countries or regions must we block (for example the US, the UK,
   OFAC-sanctioned countries)? Is a frontend geo-block enough when the
   on-chain market is open to any wallet?
7. Do we need to stop listed wallets (sanctions screening) from trading
   against our LP? Our LP co-signs every fill, so we can refuse them. Does
   that ability create a duty?
8. Are terms of use and a self-certification ("I am not a US person")
   adequate, and what must they say?
9. Is an invite-only beta with real funds treated differently from a public
   launch?

## KYC and AML

10. Do we need KYC on traders, on testers, or on neither, given that we are
    the counterparty?
11. What AML obligations apply to the insurance withdrawals and LP flows
    that we receive?
12. Do we need to register as a money services business, or the equivalent,
    anywhere?

## Marketing and communications

13. What may we say in announcements and on X? For example, may we mention
    leverage, returns or "earn" language, and what risk wording is
    required?
14. Can we pay or reward testers and early traders (fee rebates, points)
    without creating a security or a promotion problem?
15. Do posts by the founder account count as offering the product?

## Liability and operations

16. What is our exposure if the upstream program has a bug and users lose
    funds?
17. What is our exposure if our price pusher posts a wrong price, or our key
    is stolen?
18. What disclosures do we owe about our dual role as LP and operator?
19. How should we treat income from fees and insurance withdrawals for tax,
    and in which entity?
20. Do we need insurance (cover), and what kind?
21. What records must we keep, and for how long?
22. If a regulator contacts us, what is the plan, and can we shut the market
    down (resolve it) on request?

## Hedging

23. Can we hedge our LP inventory on an external perp venue, given that
    venue's own access rules for our jurisdiction?
