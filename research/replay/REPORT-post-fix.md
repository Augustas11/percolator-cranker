# SOL-PERP replay after the pusher stall fix (PR #52)

**Answer: yes, it makes money now, but thinly at 12 bps.** With the post-fix push timing (10 bps move trigger,
60-slot heartbeat) the LP makes **+4.8 bps per $ hedged** ($380/day at $1M/day). Break-even is
**~$210k/day**, against −44 bps before the fix. At **15 bps** it is **+10.4 bps**, with break-even at
**~$100k/day**.

**The stalls are gone.** Over 58 min of post-fix logs there are 139 pushes, landing lag p50 0.72 s and p95 1.25 s,
and one gap over 32 s (35.6 s).

**The heartbeat now decides how old the mark is.** At a fill the mark is **13 s old at p50 and 26 s at p95**.
61% of fills see a mark older than 10 s, and 25% one older than 20 s. Only the 10 bps move trigger keeps it
near the market.

| Toxic flow (real flow + latency arb), $1M/day | 12 bps hedged | 12 bps unhedged | Break-even (12, hedged) | 15 bps hedged | Break-even (15) |
|---|---:|---:|---:|---:|---:|
| **Now: 10 bps trigger, 60-slot heartbeat** | **+4.8** | +7.5 | **$211k** | **+10.4** | **$98k** |
| 10 bps trigger, 30-slot heartbeat | +5.6 | +5.9 | $181k | +10.8 | $95k |
| 5 bps trigger, 60-slot heartbeat | +6.6 | +7.2 | $155k | +11.0 | $93k |
| 5 bps trigger, 30-slot heartbeat | +6.8 | +6.7 | $151k | +10.9 | $94k |
| Before the fix, with its stalls (REPORT-real-data.md) | −43.7 | −24.0 | none | | |
| Old report, constant 1 s lag | +7.7 | +9.1 | $133k | | |

**Effective mark age at our fills** (now = 10 bps trigger, 60-slot heartbeat):

| | p50 | p95 | older than 10 s | older than 20 s | average price error |
|---|---:|---:|---:|---:|---:|
| Now | 12.9 s | 26.2 s | 61% | 25% | 2.9 bps |
| 30-slot heartbeat | 7.4 s | 14.0 s | 32% | 0.2% | 2.5 bps |
| 5 bps trigger + 30-slot heartbeat | 6.7 s | 13.9 s | 28% | 0.1% | 2.1 bps |

**What landing lag do we need?** With the trigger and heartbeat held as now, and every lag sample scaled:

| Landing lag p50 | 12 bps hedged | Break-even | 15 bps hedged | Break-even |
|---:|---:|---:|---:|---:|
| 0.36 s | +7.5 | $136k | +11.6 | $88k |
| **0.72 s (now)** | **+4.8** | **$211k** | **+10.4** | **$98k** |
| 1.08 s | +1.7 | $589k | +8.9 | $115k |
| 1.44 s | −0.4 | none | +8.0 | $127k |
| 2.16 s | −6.8 | none | +4.8 | $215k |

- **0.72 s is enough to make money at 12 bps, but not comfortably.** A slip to about 1.1 s takes the
  break-even to $590k/day, and about 1.4 s loses. For a comfortable margin (about +7 bps, break-even about
  $150k) at 12 bps, we need **lag p50 ≤ ~0.4 s**, or a **5 bps push trigger** (+6.6 at today's lag).
- **15 bps is the robust setting.** It stays at +4.8 bps or better up to a 2 s lag p50.
- **Cheapest win: a 5 bps trigger.** It adds about 800 pushes a day (3,400 → 4,200) for +1.8 bps at 12 bps.
  A 30-slot heartbeat costs about 3,000 more pushes for +0.8.
- **Unhedged** looks better at 12 bps (+7.5), but that is SOL's drift up while the LP sat long. Its daily
  swing is 3× the hedged one (SD $2.2k vs $0.7k).
- **Whole Binance-size tickets** are still about zero at 12 bps / now: −1.3 to +0.8 bps at $0.5–2M/day.
  Our real ticket mix remains the biggest unknown.

---

## Method

This is the same replay as [REPORT-real-data.md](REPORT-real-data.md): 90 days of Binance flow, the
latency-arb overlay, the matcher's 32 s stale cutoff, and hedge costs. The one new input is the measured
post-fix latency.

- **Data:** the `mark_lag` events in `~/drip-data/pearl/bots-after-52.log` after the first
  `price_stall_stats` line: 139 pushes, 2026-10-30 10:14–11:12 UTC. Landing lag is p50/p95 0.72/1.25 s:
  source age 0.11/0.19 s, send 0.05/0.08 s, confirm 0.55/1.08 s. The gap between pushes is p50/p95
  26.0/26.5 s, so 60 slots is about 26 s.
- **Model:** a simulated pusher (`staging/src/pusher.rs` rule). It polls every 400 ms and pushes on a move
  ≥ trigger, or once the heartbeat has elapsed (26 s for 60 slots, 13 s for 30). Source age, send and
  confirm are drawn in order from the post-fix samples. The simulated gaps (p50 26.8 s) match the log
  (26.0 s).
- **Lag sensitivity:** every latency sample is scaled by 0.5, 1.5, 2 or 3.
- **Not used for the verdict: tiling the logged timing** (−51.5 bps at 12). The logged hour was quiet, so
  almost every push was a heartbeat. Tiling those gaps over 90 days ignores the move trigger, which a
  pusher with no stalls would fire in volatile hours. Before the fix that tiling captured real stalls; now
  there are none to capture.
- **Limits:** the post-fix sample is 58 min. The arb model and the fixed-flow assumption are unchanged
  from the earlier reports.

**Reproduce:**

```bash
python3 research/replay/lag_regimes.py ~/drip-data/mac-devnet2-bots.log ~/drip-data/pearl/bots.log ~/drip-data/pearl/bots-after-52.log
REPLAY_SET=postfix python3 research/replay/replay_real.py
```

The second command takes about 5 minutes. The full output is in
[REPORT-post-fix-appendix.md](REPORT-post-fix-appendix.md).
