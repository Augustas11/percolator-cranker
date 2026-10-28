#!/usr/bin/env python3
"""Replay real SOL-PERP taker flow through our matcher and print the report tables.

    python3 research/replay/fetch.py      # once: public data into research/replay/data/
    python3 research/replay/replay.py     # tables to stdout (markdown)

Our matcher, as ops/MONEY_MODEL.md reads the program: every taker fill is at our posted
oracle mark, no spread, fee f on each side. Our LP is the counterparty to every fill. The
LP-side fee goes to insurance we control, so only the taker fee is income.

Per fill:  mark = oracle(t - lag),  LP takes the opposite of the taker at that mark.
Decisions (arb trigger, hedge trigger) use only oracle(<= t). Future prices are used only
to MEASURE markouts and for the hedge fill at t + HEDGE_LATENCY_MS.

Deterministic; nothing is random. Volumes are daily taker notional in USD.
"""
import datetime as dt
import glob
import json
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.environ.get("REPLAY_DATA", os.path.join(HERE, "data"))  # override for a sub-window

# --- grid ---
FEES_BPS = [3, 5, 8, 12, 15, 20]
LAGS_MS = [0, 400, 1000, 2000]
HEDGES = {"none": None, "tight": 0.01, "loose": 0.05}  # |net exposure| threshold, fraction of daily volume
VOLS_USD = [100_000, 1_000_000, 10_000_000]
MARKOUT_S = [0, 1, 10, 60, 300]

# --- assumptions (each named in REPORT.md) ---
HEDGE_FEE_BPS = 4.5        # Hyperliquid base-tier taker fee (published schedule)
HEDGE_LATENCY_MS = 500     # hedge fills at oracle(t + 500 ms) plus book slippage
ARB_COST_BPS = 2.0         # latency arb's own round-trip cost beyond our fee
ARB_CAP_FRAC = 0.005       # max arb trade notional, fraction of daily volume
ARB_TICK_MS = 400          # arb can hit us at most once per slot
LP_CAP_FRAC = 0.10         # matcher refuses fills past |LP| = 10% of daily volume ($100k at $1M/day, ops/CAPITAL_PLAN.md)
# fixed and other lines: ops/money_model.py defaults
FIXED_USD_DAY = 0.5 * 150 + (300 + 100) * 12 / 365 + 100_000 * 0.05 / 365
OTHER_BPS = 0.01 * 50 * 0.5 + 0.05 * 10 + 20 * 216_000 * 1e-6 / 1e5 * 1e4 - 0.5  # liq + backing + maint - bad debt
DAY_MS = 86_400_000


def load_spot():
    fs = sorted(glob.glob(os.path.join(DATA, "spot-*.npz")))
    t, p = [], []
    for f in fs:
        d = np.load(f)
        t.append(d["t"])
        p.append(d["p"])
    t, p = np.concatenate(t), np.concatenate(p)
    o = np.argsort(t, kind="stable")
    return t[o], p[o]


ST, SP = load_spot()


def px(ts):
    """Last oracle print at or before ts (no look-ahead). Refuses queries before the first print."""
    i = np.searchsorted(ST, ts, side="right") - 1
    if np.any(i < 0):
        raise ValueError("oracle queried before its first print (fetch.py loads a warm-up day)")
    return SP[i]


def covered(ts):
    """1 where oracle data extends to ts, else 0: markouts past the end of the data are dropped."""
    return (np.asarray(ts) <= ST[-1]).astype(float)


def slip_table():
    """Average of buy and sell cost vs mid, bps, walking the committed Hyperliquid book."""
    with open(os.path.join(HERE, "inputs", "hl_l2book_SOL.json")) as f:
        bids, asks = json.load(f)["levels"]
    for lv in bids + asks:
        if not all(np.isfinite(float(lv[k])) and float(lv[k]) > 0 for k in ("px", "sz")):
            raise ValueError(f"bad book level {lv!r}")
    mid = (float(bids[0]["px"]) + float(asks[0]["px"])) / 2

    def walk(levels, usd):
        left, cost = usd, 0.0
        for lv in levels:
            p, s = float(lv["px"]), float(lv["sz"])
            take = min(left, p * s)
            cost += take * abs(p / mid - 1)
            left -= take
            if left <= 0:
                return cost / usd * 1e4
        last = abs(float(levels[-1]["px"]) / mid - 1)  # beyond snapshot depth: 2x the deepest level
        return (cost + left * 2 * last) / usd * 1e4

    sizes = np.logspace(2, 8, 61)
    bps = np.array([(walk(asks, s) + walk(bids, s)) / 2 for s in sizes])
    depth = sum(float(l["px"]) * float(l["sz"]) for l in bids), sum(float(l["px"]) * float(l["sz"]) for l in asks)
    return sizes, bps, mid, depth


SLIP_X, SLIP_Y, BOOK_MID, BOOK_DEPTH = slip_table()


def slip_bps(usd):
    return np.interp(np.log(np.maximum(usd, 1.0)), np.log(SLIP_X), SLIP_Y)


def cap_fills(q, p, pos, cap_usd):
    """Matcher inventory limit: fill only the part of each trade that keeps |LP| <= cap at oracle(t).
    Trades that reduce |LP| always fill. Sequential by nature; q, pos in SOL."""
    out = []
    for qi, pi in zip(q.tolist(), p.tolist()):
        c = cap_usd / pi
        new = pos + qi
        if qi > 0 and new > c:
            new = max(pos, c)
        elif qi < 0 and new < -c:
            new = min(pos, -c)
        out.append(new - pos)
        pos = new
    return np.array(out), pos


def hedge_points(lp_path, h0, p_now, cap):
    """Indices j where |LP + hedge| * oracle(t_j) > cap; hedge back to zero after fill j."""
    pts, h, i, n = [], h0, 0, len(lp_path)
    if cap is None:
        return pts, h
    while i < n:
        w = min(n, i + 20_000)
        k = np.flatnonzero(np.abs(lp_path[i:w] + h) * p_now[i:w] > cap)
        if k.size == 0:
            i = w
            continue
        j = i + int(k[0])
        dq = -(lp_path[j] + h)
        pts.append((j, dq))
        h += dq
        i = j + 1
    return pts, h


class Book:
    """State and daily ledger of one (flow, fee, lag, hedge, volume) setting."""

    def __init__(self):
        self.lp = self.h = self.cash = self.eq = 0.0
        self.hc = 0.0         # committed hedge: executed + pending orders (the trigger sees this)
        self.pending = []     # (exec_time_ms, dq) orders submitted but not yet filled
        self.days = []        # per-day dicts
        self.expo = []        # |LP + hedge| USD, sampled each minute
        self.lpabs = []       # |LP| USD (on-chain inventory), sampled each minute
        self.hedge_usd = 0.0
        self.expo_max = 0.0   # |LP + filled hedge| USD, max over every fill

    def run_day(self, day0, t, qlp, m, p_now, fee_bps, cap, funding):
        n_lp0, h0 = self.lp, self.h
        day_end = day0 + DAY_MS
        notional = np.abs(qlp) * m
        fees = fee_bps / 1e4 * notional.sum()   # taker side only
        cash = self.cash - float(np.dot(qlp, m)) + fees
        lp_path = n_lp0 + np.cumsum(qlp)
        pts, self.hc = hedge_points(lp_path, self.hc, p_now, cap)
        # a hedge counts only once it fills at t + latency; orders still open at midnight carry over
        orders = sorted(self.pending + [(int(t[j]) + HEDGE_LATENCY_MS, dq) for j, dq in pts])
        self.pending = [o for o in orders if o[0] >= day_end]
        hcost = hvol = 0.0
        ht, hh = [day0 - 1], [h0]
        for te, dq in (o for o in orders if o[0] < day_end):
            pe = float(px(te))
            usd = abs(dq) * pe
            cash -= dq * pe
            c = usd * (HEDGE_FEE_BPS + float(slip_bps(usd))) / 1e4
            cash -= c
            hcost += c
            hvol += usd
            ht.append(te)
            hh.append(hh[-1] + dq)
        ht, hh = np.array(ht), np.array(hh)
        h = float(hh[-1])
        fund = 0.0
        for ft, rate in funding:   # hedge venue funding, longs pay positive rates
            hf = hh[np.searchsorted(ht, ft, side="right") - 1]
            fund -= hf * float(px(ft)) * rate
        cash += fund
        lp = float(lp_path[-1]) if len(lp_path) else n_lp0
        p_eod = float(px(day0 + DAY_MS - 1))
        eq = cash + (lp + h) * p_eod
        s = day0 + np.arange(60_000, DAY_MS + 1, 60_000)
        fi = np.searchsorted(t, s, side="right") - 1
        lps = np.where(fi >= 0, lp_path[np.maximum(fi, 0)] if len(lp_path) else n_lp0, n_lp0)
        hs = hh[np.searchsorted(ht, s, side="right") - 1]
        ps = px(s)
        self.expo.append((np.abs(lps + hs) * ps).astype(np.float32))
        if len(lp_path):  # event-level peak: exposure only jumps at our fills
            hf = hh[np.searchsorted(ht, t, side="right") - 1]
            self.expo_max = max(self.expo_max, float((np.abs(lp_path + hf) * p_now).max()))
        self.lpabs.append((np.abs(lps) * ps).astype(np.float32))
        pnl = eq - self.eq
        self.days.append({"pnl": pnl, "fees": fees, "hedge_cost": hcost, "funding": fund,
                          "inventory": pnl - fees + hcost - fund, "hedge_usd": hvol,
                          "volume": float(notional.sum()), "hedges": len(ht) - 1})
        self.lp, self.h, self.cash, self.eq = lp, h, cash, eq


def markouts(t, qlp, m):
    """LP gain per horizon: sum qlp * (oracle(t+H) - mark). Negative = adverse selection."""
    return np.array([float(np.dot(qlp * covered(t + 1000 * h), px(t + 1000 * h) - m)) for h in MARKOUT_S])


def main():
    days = sorted(os.path.basename(f)[5:15] for f in glob.glob(os.path.join(DATA, "perp-*.npz")))
    with open(os.path.join(DATA, "hl_funding_SOL.json")) as f:
        fund_all = [(r["time"], r["rate"]) for r in json.load(f)]
    # mean Binance daily notional -> one size scale per target volume
    raw_vol = {}
    for d in days:
        z = np.load(os.path.join(DATA, f"perp-{d}.npz"))
        raw_vol[d] = float(np.abs(z["q"] * z["p"]).sum())
    mean_raw = np.mean(list(raw_vol.values()))

    books = {}
    for flow in ("base", "toxic"):
        for fee in (FEES_BPS if flow == "toxic" else [None]):
            for lag in LAGS_MS:
                for hz in HEDGES:
                    for v in VOLS_USD:
                        books[(flow, fee, lag, hz, v)] = Book()
    mk_base = {lag: np.zeros(len(MARKOUT_S)) for lag in LAGS_MS}
    mk_arb = {(f, lag): np.zeros(len(MARKOUT_S)) for f in FEES_BPS for lag in LAGS_MS}
    arb_vol = {(f, lag): 0.0 for f in FEES_BPS for lag in LAGS_MS}
    size_edges = [0, 1e3, 1e4, 1e5, 1e6, np.inf]   # raw Binance trade notional
    mk_size = {lag: np.zeros((len(size_edges) - 1, len(MARKOUT_S) + 1)) for lag in (0, 1000)}
    day_info = []
    cap_raw = LP_CAP_FRAC * mean_raw          # fills are capped at unit scale; the cap scales with volume
    pos_base = 0.0
    pos_tox = {(f, lag): 0.0 for f in FEES_BPS for lag in LAGS_MS}
    fill = {"base": [0.0, 0.0], **{(f, lag): [0.0, 0.0] for f in FEES_BPS for lag in LAGS_MS}}  # filled, offered USD

    for d in days:
        day0 = int(dt.datetime.fromisoformat(d).replace(tzinfo=dt.timezone.utc).timestamp() * 1000)
        z = np.load(os.path.join(DATA, f"perp-{d}.npz"))
        t, q, praw = z["t"], z["q"], z["p"]
        if not (np.all(np.isfinite(q)) and np.all(np.isfinite(praw)) and np.all(praw > 0) and np.all(np.diff(t) >= 0)):
            raise ValueError(f"bad cached flow {d}")
        p_now = px(t)
        funding = [(ft, r) for ft, r in fund_all if day0 <= ft < day0 + DAY_MS]
        s = px(day0 + np.arange(0, DAY_MS + 1, 60_000))
        r1 = np.diff(np.log(s))
        day_info.append({"day": d, "open": float(s[0]), "close": float(s[-1]),
                         "range_pct": float((s.max() / s.min() - 1) * 100),
                         "vol_ann_pct": float(r1.std() * np.sqrt(1440 * 365) * 100),
                         "raw_volume": raw_vol[d]})
        ticks = day0 + np.arange(0, DAY_MS, ARB_TICK_MS)
        qb, pos_base = cap_fills(-q, p_now, pos_base, cap_raw)   # LP side, after the inventory limit
        fill["base"][0] += float(np.abs(qb * p_now).sum())
        fill["base"][1] += float(np.abs(q * p_now).sum())
        p_tick = px(ticks)
        for lag in LAGS_MS:
            m = px(t - lag)
            qlp_raw = -q  # LP takes the other side, unscaled SOL
            mk_base[lag] += markouts(t, qlp_raw, m) / mean_raw
            if lag in mk_size:
                usd = np.abs(q) * praw
                b = np.digitize(usd, size_edges) - 1
                mo = np.stack([qlp_raw * covered(t + 1000 * h) * (px(t + 1000 * h) - m) for h in MARKOUT_S], axis=1)
                for k in range(len(size_edges) - 1):
                    sel = b == k
                    mk_size[lag][k, :-1] += mo[sel].sum(axis=0)
                    mk_size[lag][k, -1] += float((np.abs(q[sel]) * m[sel]).sum())
            m_tick = px(ticks - lag)
            dev = p_tick / m_tick - 1
            for v in VOLS_USD:
                k = v / mean_raw
                for hz, frac in HEDGES.items():
                    cap = None if frac is None else frac * v
                    books[("base", None, lag, hz, v)].run_day(day0, t, qb * k, m, p_now, 0.0, cap, funding)
            for fee in FEES_BPS:
                trig = np.flatnonzero(np.abs(dev) > (fee + ARB_COST_BPS) / 1e4)
                at, am = ticks[trig], m_tick[trig]
                aq_unit = -np.sign(dev[trig]) / am   # LP side, per $1 of arb notional
                mk_arb[(fee, lag)] += markouts(at, aq_unit, am)  # per $1 cap; scale later
                arb_vol[(fee, lag)] += len(trig)
                tt = np.concatenate([t, at])
                o = np.argsort(tt, kind="stable")
                mm = np.concatenate([m, am])[o]
                pn = np.concatenate([p_now, p_tick[trig]])[o]
                qq, pos_tox[(fee, lag)] = cap_fills(np.concatenate([-q, aq_unit * ARB_CAP_FRAC * mean_raw])[o],
                                                    pn, pos_tox[(fee, lag)], cap_raw)
                fill[(fee, lag)][0] += float(np.abs(qq * pn).sum())
                fill[(fee, lag)][1] += float(np.abs(q * p_now).sum()) + len(trig) * ARB_CAP_FRAC * mean_raw
                for v in VOLS_USD:
                    k = v / mean_raw
                    for hz, frac in HEDGES.items():
                        cap = None if frac is None else frac * v
                        books[("toxic", fee, lag, hz, v)].run_day(day0, tt[o], qq * k, mm, pn, fee, cap, funding)
        print(f"{d} done", file=sys.stderr, flush=True)

    report(days, day_info, books, mk_base, mk_arb, arb_vol, mk_size, size_edges, mean_raw, len(days), fill)
    P = print
    P("\n### Sensitivity: toxic flow, tight hedge (whole tickets: every N-th trade, N = mean notional / volume, offsets 0, N/3, 2N/3)\n")
    P("| Variant | Offset | Volume/day | Fee | Lag | Edge bps | Net/day | SD/day | Max DD | Filled |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for (name, fee, lag, k, afrac, phase, v), sm, fr in sensitivity(days, mean_raw, fund_all):
        off = "" if k is None else f"{k}/3 N"
        P(f"| {name} | {off} | {fmt_usd(v)} | {fee} | {lag} ms | {sm['trade_bps']:.2f} | {sm['net_day']:,.0f} | {sm['sd']:,.0f} | {fmt_usd(sm['dd'])} | {fr:.0%} |")


def sensitivity(days, mean_raw, fund_all):
    """Toxic flow, tight hedge, with inputs changed per row (review rounds 1-2).
    thin: whole Binance tickets, every N-th trade (N = mean notional / V) starting at a fixed offset, no size scaling.
    arb: ticket size as a fraction of daily volume, and the slot phase of the arb clock. V: daily volume."""
    M = 1_000_000
    x4 = 4 * ARB_CAP_FRAC
    rows = [("reference (as headline)", 12, 1000, None, ARB_CAP_FRAC, 0, M)]
    rows += [("arb ticket x4", f, lag, None, x4, 0, M) for f in (8, 12) for lag in (400, 1000)]
    rows += [("arb slot phase +200 ms", 12, 1000, None, ARB_CAP_FRAC, 200, M)]
    rows += [("whole tickets", f, lag, k, ARB_CAP_FRAC, 0, M) for f, lag in ((12, 1000), (8, 400), (12, 400)) for k in range(3)]
    rows += [("whole tickets + arb ticket x4", 12, lag, k, x4, 0, M) for lag in (400, 1000) for k in range(3)]
    rows += [("whole tickets", 12, 1000, k, ARB_CAP_FRAC, 0, v) for v in (500_000, 2_000_000, 4_000_000) for k in range(3)]
    books = [Book() for _ in rows]
    pos = [0.0] * len(rows)
    filled = [[0.0, 0.0] for _ in rows]
    idx0 = 0  # running trade index so thinning is continuous across days
    for d in days:
        day0 = int(dt.datetime.fromisoformat(d).replace(tzinfo=dt.timezone.utc).timestamp() * 1000)
        z = np.load(os.path.join(DATA, f"perp-{d}.npz"))
        t_all, q_all = z["t"], z["q"]
        funding = [(ft, r) for ft, r in fund_all if day0 <= ft < day0 + DAY_MS]
        gidx = idx0 + np.arange(len(t_all))
        idx0 += len(t_all)
        for i, (_, fee, lag, k, afrac, phase, V) in enumerate(rows):
            if k is None:
                t, qlp = t_all, -q_all * (V / mean_raw)
            else:
                N = int(round(mean_raw / V))
                sel = (gidx - k * N // 3) % N == 0   # offsets 0, N/3, 2N/3
                t, qlp = t_all[sel], -q_all[sel]
            ticks = day0 + phase + np.arange(0, DAY_MS - phase, ARB_TICK_MS)
            m_tick, p_tick = px(ticks - lag), px(ticks)
            dev = p_tick / m_tick - 1
            trig = np.flatnonzero(np.abs(dev) > (fee + ARB_COST_BPS) / 1e4)
            tt = np.concatenate([t, ticks[trig]])
            o = np.argsort(tt, kind="stable")
            mm = np.concatenate([px(t - lag), m_tick[trig]])[o]
            pn = np.concatenate([px(t), p_tick[trig]])[o]
            offered = np.concatenate([qlp, -np.sign(dev[trig]) * afrac * V / m_tick[trig]])[o]
            qq, pos[i] = cap_fills(offered, pn, pos[i], LP_CAP_FRAC * V)
            filled[i][0] += float(np.abs(qq * pn).sum())
            filled[i][1] += float(np.abs(offered * pn).sum())
            books[i].run_day(day0, tt[o], qq, mm, pn, fee, HEDGES["tight"] * V, funding)
    return [(r, summary(b, r[1], r[6], "toxic", len(days)), f[0] / f[1]) for r, b, f in zip(rows, books, filled)]


def fmt_usd(x):
    return f"${x / 1e6:,.1f}M" if abs(x) >= 1e6 else f"${x / 1e3:,.0f}k" if abs(x) >= 1e3 else f"${x:,.0f}"


def summary(b, fee, v, flow, n):
    """Daily stats for one setting; base flow gets its fee income added here (path does not depend on it)."""
    D = b.days
    fees = np.array([x["fees"] for x in D]) + (fee / 1e4 * np.array([x["volume"] for x in D]) if flow == "base" else 0)
    pnl = np.array([x["pnl"] for x in D]) + (fees - np.array([x["fees"] for x in D]))
    net = pnl - FIXED_USD_DAY
    cum = np.cumsum(net)
    dd = float((np.maximum.accumulate(np.concatenate([[0], cum])) - np.concatenate([[0], cum])).max())
    return {"trade_bps": pnl.sum() / (v * n) * 1e4, "net_day": net.mean(), "dd": dd, "worst": net.min(),
            "sd": net.std(), "fees": fees, "pnl": pnl, "net": net,
            "hedge_bps": sum(x["hedge_cost"] for x in D) / (v * n) * 1e4,
            "hedge_usd_day": sum(x["hedge_usd"] for x in D) / n,
            "fund_bps": sum(x["funding"] for x in D) / (v * n) * 1e4,
            "inv_bps": sum(x["inventory"] for x in D) / (v * n) * 1e4,
            "expo": np.concatenate(b.expo), "lpabs": np.concatenate(b.lpabs), "expo_max": b.expo_max}


def breakeven(bps_by_v, other_bps=0.0):
    """Volume where V * margin(V) = fixed; margin interpolated in log V, held flat past the ends."""
    xs = np.log(VOLS_USD)
    for V in np.logspace(4, 9, 2001):
        mg = np.interp(np.log(V), xs, bps_by_v) + other_bps
        if V * mg / 1e4 >= FIXED_USD_DAY:
            return V
    return None


def report(days, info, books, mk_base, mk_arb, arb_vol, mk_size, edges, mean_raw, n, fill):
    P = print
    P(f"Window: {days[0]} to {days[-1]} ({n} days). Mean Binance SOLUSDT-perp taker notional {fmt_usd(mean_raw)}/day.")
    P(f"Hedge book snapshot: mid {BOOK_MID:.2f}, 20-level depth bid {fmt_usd(BOOK_DEPTH[0])} / ask {fmt_usd(BOOK_DEPTH[1])}.")
    P(f"Fixed cost {FIXED_USD_DAY:.2f} USD/day; model's other lines net {OTHER_BPS:+.3f} bps.\n")

    P("### Hedge slippage (Hyperliquid book walk, avg of buy and sell)\n")
    P("| Hedge size | Slippage bps | + taker fee |\n|---:|---:|---:|")
    for s in (1e3, 1e4, 1e5, 1e6, 1e7):
        P(f"| {fmt_usd(s)} | {slip_bps(s):.2f} | {slip_bps(s) + HEDGE_FEE_BPS:.2f} |")

    P("\n### Volatility (oracle, per day)\n")
    vols = sorted(info, key=lambda x: -x["vol_ann_pct"])
    P(f"Median annualised 1-min vol {np.median([x['vol_ann_pct'] for x in info]):.0f}%; "
      f"SOL {info[0]['open']:.2f} -> {info[-1]['close']:.2f}.\n")
    P("| Top-vol day | Ann. vol | Intraday range | Open -> close |\n|---|---:|---:|---:|")
    for x in vols[:5]:
        P(f"| {x['day']} | {x['vol_ann_pct']:.0f}% | {x['range_pct']:.1f}% | {(x['close'] / x['open'] - 1) * 100:+.1f}% |")

    P("\n### Adverse selection, base flow (LP markout vs mark, bps of volume; positive = LP loses)\n")
    P("| Lag | " + " | ".join(f"{h}s" for h in MARKOUT_S) + " |\n|---:|" + "---:|" * len(MARKOUT_S))
    for lag in LAGS_MS:
        P(f"| {lag} ms | " + " | ".join(f"{-x / n * 1e4:.2f}" for x in mk_base[lag]) + " |")
    P("\nBy raw Binance trade size (lag 0 and 1 s):\n")
    P("| Lag | Trade size | Share of volume | " + " | ".join(f"{h}s" for h in MARKOUT_S) + " |\n|---|---|---:|" + "---:|" * len(MARKOUT_S))
    for lag, M in mk_size.items():
        tot = M[:, -1].sum()
        for k in range(len(edges) - 1):
            hi = "+" if edges[k + 1] == np.inf else fmt_usd(edges[k + 1])
            P(f"| {lag} ms | {fmt_usd(edges[k])}–{hi} | {M[k, -1] / tot:.0%} | "
              + " | ".join(f"{-x / M[k, -1] * 1e4:.2f}" for x in M[k, :-1]) + " |")

    P(f"\nInventory limit {LP_CAP_FRAC:.0%} of daily volume: base flow filled {fill['base'][0] / fill['base'][1]:.1%} of offered notional.")
    P("\n### Toxic overlay: latency arb (per slot, cap 0.5% of daily volume per trade)\n")
    P("| Fee | Lag | Arb trades/day | Arb volume, % of base | Arb loss to LP at 0s before the limit, bps of base | net of its fee | Filled, all flow (limit) |\n|---:|---:|---:|---:|---:|---:|---:|")
    for fee in FEES_BPS:
        for lag in LAGS_MS:
            cnt = arb_vol[(fee, lag)] / n
            share = cnt * ARB_CAP_FRAC
            loss0 = -mk_arb[(fee, lag)][0] * ARB_CAP_FRAC / n * 1e4   # per $1 base volume
            P(f"| {fee} | {lag} ms | {cnt:,.0f} | {share:.1%} | {loss0:.2f} | {loss0 - fee * share:.2f} | {fill[(fee, lag)][0] / fill[(fee, lag)][1]:.0%} |")

    S = {}
    for (flow, fee, lag, hz, v), b in books.items():
        for f in (FEES_BPS if flow == "base" else [fee]):
            S[(flow, f, lag, hz, v)] = summary(b, f, v, flow, n)

    for flow in ("base", "toxic"):
        P(f"\n### Grid, {flow} flow: trading margin bps of base volume (fees − adverse − hedge + funding), and break-even\n")
        P("| Fee | Lag | Hedge | margin @$100k | @$1M | @$10M | Net/day @$1M | Max DD @$1M | Break-even (trading) | (+ model lines) |")
        P("|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|")
        for fee in FEES_BPS:
            for lag in LAGS_MS:
                for hz in HEDGES:
                    m = [S[(flow, fee, lag, hz, v)]["trade_bps"] for v in VOLS_USD]
                    s1 = S[(flow, fee, lag, hz, 1_000_000)]
                    be, be2 = breakeven(m), breakeven(m, OTHER_BPS)
                    P(f"| {fee} | {lag} | {hz} | {m[0]:.2f} | {m[1]:.2f} | {m[2]:.2f} | {s1['net_day']:,.0f} | {fmt_usd(s1['dd'])} | "
                      f"{fmt_usd(be) if be else 'none'} | {fmt_usd(be2) if be2 else 'none'} |")

    P("\n### Headline: toxic flow, realistic lags, $1M/day. Hedged (tight) = edge; unhedged adds directional P&L\n")
    P("| Fee | Lag | Hedged margin bps | Inventory bps | Hedge cost bps | Funding bps | Net/day | SD/day | Max DD | Break-even "
      "| Unhedged margin bps | Unhedged net/day | Unhedged SD/day | Unhedged max DD |")
    P("|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for fee in FEES_BPS:
        for lag in (400, 1000):
            s, u = S[("toxic", fee, lag, "tight", 1_000_000)], S[("toxic", fee, lag, "none", 1_000_000)]
            be = breakeven([S[("toxic", fee, lag, "tight", vv)]["trade_bps"] for vv in VOLS_USD])
            P(f"| {fee} | {lag} | {s['trade_bps']:.2f} | {s['inv_bps']:.2f} | {s['hedge_bps']:.2f} | {s['fund_bps']:.2f} | "
              f"{s['net_day']:,.0f} | {s['sd']:,.0f} | {fmt_usd(s['dd'])} | {fmt_usd(be) if be else 'none'} | "
              f"{u['trade_bps']:.2f} | {u['net_day']:,.0f} | {u['sd']:,.0f} | {fmt_usd(u['dd'])} |")

    best, head = ("toxic", 12, 1000, "tight", 1_000_000), ("toxic", 5, 1000, "tight", 1_000_000)
    for key in dict.fromkeys([best, head]):
        s = S[key]
        P(f"\n### Setting {key[1]} bps / {key[2]} ms / {key[3]} hedge, toxic, $1M/day\n")
        P(f"Net/day mean {s['net_day']:,.0f}, SD {s['sd']:,.0f}, max drawdown {fmt_usd(s['dd'])}, "
          f"days positive {np.mean(s['net'] > 0):.0%}. Peak net exposure over every fill {fmt_usd(s['expo_max'])}.\n")
        P("| Inventory, sampled each minute | p50 | p95 | p99 | max |\n|---|---:|---:|---:|---:|")
        for name, arr in (("net exposure, LP + hedge", s["expo"]), ("LP on-chain position", s["lpabs"])):
            P(f"| {name} | " + " | ".join(fmt_usd(float(np.percentile(arr, q))) for q in (50, 95, 99, 100)) + " |")
        b = books[key]
        P("\nFive worst days:\n")
        P("| Day | Net | Fees | Inventory MTM | Hedge cost | SOL open→close | Range | Ann. vol |\n|---|---:|---:|---:|---:|---:|---:|---:|")
        for i in np.argsort(s["net"])[:5]:
            x, di = b.days[i], info[i]
            P(f"| {di['day']} | {s['net'][i]:,.0f} | {x['fees']:,.0f} | {x['inventory']:,.0f} | {x['hedge_cost']:,.0f} | "
              f"{(di['close'] / di['open'] - 1) * 100:+.1f}% | {di['range_pct']:.1f}% | {di['vol_ann_pct']:.0f}% |")


if __name__ == "__main__":
    main()
