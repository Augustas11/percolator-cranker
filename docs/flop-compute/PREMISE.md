# FLOP compute perp (devnet demo): premise

**Premise.** We can run a Percolator perp on FLOP's on-chain compute price, on
devnet, where the mark comes only from real FLOP settlements and stops when they
stop. It counts as working if, over a 24-hour run, the pusher pushes only from
non-self-trade spot settlements, holds at least once when the data goes stale,
and one bilateral trade opens, gets cranked and closes. It fails if FLOP devnet
can't give us a price that reflects more than one buyer, because then the market
tracks one party's posted price, not compute.

**Challenge (2026-11-10, from FLOP devnet data).** The data is fresh but not a
market. Of the latest 300 spot settlements (about 7.5 hours, ~35 an hour):

- price has 2 distinct values, 0.002970 and 0.002881 FLOP per Gflop;
- one agent (buyer) made 280 of 300 (agent HHI 0.88), over 62 miners;
- 2 models.

So the "price" is one buyer's fixed offer. FLOP's own FRCR index doesn't publish
for the same reason: its concentration floor fails. An honest index with a
buyer-concentration floor would be stale today, and the market would never get a
mark. Without that floor, it gets a flat mark that tells us nothing about compute.

What still holds: the plumbing (FLOP reader, stale gate, AuthMark market, crank)
can be built and tested now, and it starts meaning something when a second real
buyer shows up.
