# SOL-PERP replay on real devnet pusher data

> **Update 2026-10-30:** the stalls were fixed in PR #52. See [REPORT-post-fix.md](REPORT-post-fix.md): +4.8 bps at 12 bps, break-even ~$210k/day.

**Answer: not as the pusher runs today.** At 12 bps the LP loses about 44 bps per $ of volume hedged, or 24 bps
unhedged, at any volume. The cause is not the lag. Our measured lag at landing is fine: p50 1.0 s, p95 2.1 s.
The cause is the **stalls**. The pusher often goes quiet for 25–120 s because the exchange quorum fails, and a
latency arb trades against the stale mark until the matcher's 32 s stale cutoff stops it.

**If the stalls are fixed**, the current pusher (push on a 10 bps move or a 60-slot heartbeat) makes a thin
**+3.4 bps hedged (+3.5 unhedged)**. That is $236/day at $1M/day, with break-even at **about $300k/day**
($250k counting the model's other lines). It needs many small tickets: with whole Binance-size tickets it
loses 0.8–2.6 bps.

| 12 bps fee, toxic flow (real flow + latency arb), $1M/day | Hedged bps | Unhedged bps | Break-even/day |
|---|---:|---:|---:|
| Old report, constant 400 ms lag | +9.5 | +11.1 | $108k |
| Old report, constant 1 s lag | +7.7 | +9.1 | $133k |
| **Current pusher with the stalls we logged** | **−43.7** | **−24.0** | none |
| **Current pusher, no stalls** | **+3.4** | **+3.5** | **~$300k** |
| …same, lag halved (p50 ~0.5 s) | +7.1 | +9.6 | $145k |
| …same, lag doubled (p50 ~2 s) | −4.1 | −1.9 | none |
| …same, push on a 5 bps move instead of 10 | +5.9 | +6.5 | $174k |
| …same, fee 15 bps | +9.7 | +11.2 | $105k |
| …same, whole Binance-size tickets | −0.8 to −2.6 | | none tested ($0.5–2M) |
| Every-move pusher on paid RPC, with its logged stalls | −15.7 | −9.1 | none |
| Same pusher with no stalls (simulated) | +6.0 | +6.7 | $170k |
| Public RPC (Pyth or median), ~10–15 s lag | −60 to −87 | −29 to −50 | none |

**What this means:**
- **Fix the stalls first.** In the current-pusher log there are 2,362 "exchange quorum: N usable venues,
  need 2" failures against 598 pushes. A venue counts as stale after 2 s with no trade, and Kraken and
  Binance.US often go 2 s without a SOL trade. Until this is fixed, the market loses money whatever the fee.
- **Lag p50 decides the margin.** Each halving or doubling of the landing lag moves the edge by about ±4 bps,
  and 2 s at p50 is already a loss.
- **The old "12 bps with lag ≤ 400 ms" rule still holds**, but our real setup is weaker than the old 1 s
  case. The mark is up to 24 s old between heartbeats, held in place only by the 10 bps move trigger
  (effective mark age at fills: p50 12 s, p95 24 s).
- **Unhedged** has about the same mean, because SOL drifted up and the LP sat long. But the daily swing is
  2.5× larger (SD $2.2k vs $0.9k), so the mean is luck, not edge. With the hedge parked, expect that swing.
- **Not proven:** real ticket sizes, a real arb's size, and whether the median tracks the market like
  Binance spot does. The arb model is the same as the old report's (0.5% of daily volume per slot, hits
  any deviation above fee + 2 bps).

---

## Method

**Lag data (`lag_regimes.py`).** It parses the 6,306 `mark_lag` events in the Pearl log and the earlier Mac log
(`ms` = source price time to push landed = source age + send + confirm) and splits them by run segment into
four regimes. Measured (full table in the appendix):

| Regime | When (UTC) | Pushes | Lag p50 / p95 | Gap between pushes p50 / p95 |
|---|---|---:|---:|---:|
| Free Pyth, public RPC (Mac) | 10-29 03:52–09:49 | 1,504 | 15.3 s / 23.6 s | 11.6 s / 23.6 s |
| Exchange median, public RPC | 10-29 09:49 – 10-30 03:50 | 3,379 | 10.3 s / 20.4 s | 13.2 s / 43.0 s |
| Exchange median, paid RPC, every move | 10-30 02:31–03:57 | 825 | 0.72 s / 1.25 s | 2.1 s / 12.5 s |
| Exchange median, 10 bps move or 60-slot heartbeat (current) | 10-30 03:57–07:20 | 598 | 1.04 s / 2.06 s | 18.2 s / 38.0 s |

The logs carry no timestamped mark price, so the mark path is rebuilt from the lag data. The prices come
from the old replay's Binance-spot oracle stand-in.

**Replay (`replay_real.py`).** It reuses `replay.py` unchanged: same 90 days (2026-07-30 to 10-27), same
Binance flow scaled to volume, same latency-arb overlay, inventory cap, hedge and costs. Only the mark
changes. Instead of `oracle(t − constant lag)`, the mark is the last landed push:
- **Logged timing** (fixed-cadence regimes, and the "with stalls" row): the logged sequence of
  (gap between pushes, lag) is tiled over the 90 days. Gaps over 120 s (restarts) are dropped.
- **Simulated pusher** (the "no stalls" rows): this is `staging/src/pusher.rs`'s rule. It polls every 400 ms
  and pushes on a ≥10 bps move or a 24 s heartbeat, one push in flight at a time. Each push's source age,
  send and confirm times are drawn in order from the logged samples. It never stalls, so it is the
  optimistic bound; the logged timing is the pessimistic one.
- **Matcher stale cutoff:** a fill is refused when the mark landed more than 80 slots (32 s) ago, which is
  the on-chain matcher's `STALE` check at the devnet setting. Refused real flow is lost volume. Without the
  cutoff, the stalled cases are worse (current pusher with stalls: −51 bps).
- **Lag sensitivity** scales every lag sample by 0.5 or 2 (cadence unchanged).
- The two constant-lag rows reproduce the old REPORT.md exactly (+9.47 and +7.71 bps), as a check.

**Limits.** The current-pusher sample is 3.4 hours. The simulated pusher pushes about every 25 s against
18 s in the log, because the 3-venue median moves more often than Binance spot, so the no-stall case may be
slightly optimistic. The other limits are the same as REPORT.md: flow held fixed as the fee changes, profit
locked in insurance, and the hedge modelled at Hyperliquid base fees.

**Reproduce:**

```bash
python3 research/replay/fetch.py
python3 research/replay/lag_regimes.py ~/drip-data/mac-devnet2-bots.log ~/drip-data/pearl/bots.log
python3 research/replay/replay_real.py
```

The first command fetches the data. The second is optional, because its output
`inputs/mark_lag_regimes.json` is committed. The third takes about 5 minutes. Set
`REPLAY_NO_STALE_REFUSAL=1` to turn off the cutoff. The full output is in
[REPORT-real-data-appendix.md](REPORT-real-data-appendix.md).
