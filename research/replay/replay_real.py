#!/usr/bin/env python3
"""Money replay with measured landing schedules and move/heartbeat pusher rules."""
import datetime as dt
import glob
import json
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import replay as R
from lag_regimes import pairs

MAX_REF_AGE_MS = 80 * 400


class MarkPath:
    def __init__(self, landing=None, source=None, lag=None):
        self.lag = lag
        if lag is None:
            order = np.argsort(landing, kind='stable')
            self.L, self.S = np.asarray(landing)[order], np.asarray(source)[order]
            if not len(self.L) or np.any(self.S > self.L):
                raise ValueError('empty path or source after landing')

    def source(self, t):
        if self.lag is not None:
            return np.asarray(t) - self.lag
        i = np.searchsorted(self.L, t, side='right') - 1
        if np.any(i < 0):
            raise ValueError('mark queried before first landing')
        return self.S[i]

    def mark(self, t):
        if self.lag is not None:
            return R.px(np.asarray(t) - self.lag)
        # Oracle prints have integer-ms timestamps: floor preserves lookup
        # semantics and avoids casting the entire oracle for fractional queries.
        return R.px(np.floor(self.source(t)).astype(np.int64))

    def mark_age(self, t):
        return np.asarray(t) - self.source(t)

    def landing_age(self, t):
        if self.lag is not None:
            return np.zeros_like(np.asarray(t))
        i = np.searchsorted(self.L, t, side='right') - 1
        if np.any(i < 0):
            raise ValueError('mark queried before first landing')
        return np.asarray(t) - self.L[i]


def schedule(rows, start, end, k=1.0):
    gaps, lags = pairs(rows)
    if not len(gaps):
        raise ValueError('no usable cadence samples')
    count = int(np.ceil((end - start) / gaps.sum()))
    landing = start + np.cumsum(np.tile(gaps, count))
    latency = np.tile(lags, count)
    keep = landing < end
    return MarkPath(landing[keep], landing[keep] - k * latency[keep])


def rule(rows, start, end, move_bps=10, heartbeat_ms=24_000, poll_ms=400, k=1.0):
    age, send, confirm = [np.asarray(rows[key]) * k for key in ('src_age_ms', 'send_ms', 'confirm_ms')]
    if not len(age):
        raise ValueError('rule needs component latency samples')
    landing, source = [], []
    d, last_price, last_landing, sample = start, None, start, 0
    while d < end:
        i = sample % len(age)
        # Peek this sample until a push fires. Search bounded numpy chunks, never
        # iterate through all polls in Python. Heartbeat bounds the search too.
        stop = min(end, max(d + poll_ms, last_landing + heartbeat_ms + poll_ms))
        while d < stop:
            times = np.arange(d, min(stop, d + 256 * poll_ms), poll_ms)
            view = R.px(np.floor(times - age[i]).astype(np.int64))
            hit = (np.ones(len(times), dtype=bool) if last_price is None else
                   (np.abs(view / last_price - 1) >= move_bps / 1e4) |
                   (times - last_landing >= heartbeat_ms))
            indices = np.flatnonzero(hit)
            if len(indices):
                j = int(indices[0])
                decision = times[j]
                landed = decision + send[i] + confirm[i]
                landing.append(landed)
                source.append(decision - age[i])
                last_price, last_landing = view[j], landed
                sample += 1
                # First poll at or after the in-flight transaction lands; even
                # zero latency cannot produce two pushes at the same poll.
                n = max(int((decision - start) // poll_ms) + 1,
                        int(np.ceil((landed - start) / poll_ms)))
                d = start + n * poll_ms
                break
            d = times[-1] + poll_ms
    path = MarkPath(landing, source)
    keep = path.L < end
    path.L, path.S = path.L[keep], path.S[keep]
    return path


def specs():
    whole = [(offset, v, R.ARB_CAP_FRAC) for v in (500_000, 1_000_000, 2_000_000)
             for offset in range(3)]
    replay_set = os.environ.get('REPLAY_SET', 'default')
    if replay_set == 'postfix':
        rows = [(f'post_fix move={move} hb={slots}', 'rule', 'post_fix',
                 {'move_bps': move, 'heartbeat_ms': heartbeat}, (12, 15),
                 whole if move == 10 and slots == 60 else [])
                for move in (10, 5) for slots, heartbeat in ((60, 26_000), (30, 13_000))]
        rows += [(f'post_fix k={k:g}', 'rule', 'post_fix',
                  {'move_bps': 10, 'heartbeat_ms': 26_000, 'k': k}, (12, 15), [])
                 for k in (0.5, 1.5, 2, 3)]
        rows += [('post_fix logged', 'schedule', 'post_fix', {}, (12, 15), [])]
        return rows
    if replay_set != 'default':
        raise ValueError(f'unknown REPLAY_SET: {replay_set}')
    rows = [('const 400 ms', 'const', None, {'lag': 400}, (12,), []),
            ('const 1 s', 'const', None, {'lag': 1000}, (12,), [])]
    rows += [(name, 'schedule', name, {}, (12,), []) for name in ('pyth_public', 'median_public')]
    rows += [('median_fast', 'schedule', 'median_fast', {}, (12, 8, 15), whole)]
    rows += [('move_or_heartbeat', 'rule', 'move_or_heartbeat', {}, (12, 8, 15),
              whole + [(offset, 1_000_000, 4 * R.ARB_CAP_FRAC) for offset in range(3)])]
    rows += [('move_or_heartbeat logged', 'schedule', 'move_or_heartbeat', {}, (12,), [])]  # logged timing, stalls included
    rows += [(f'{name} k={k:g}', kind, name, {'k': k}, (12,), [])
             for name, kind in (('median_fast', 'schedule'), ('move_or_heartbeat', 'rule')) for k in (0.5, 2)]
    rows += [('rule move=5 bps', 'rule', 'move_or_heartbeat', {'move_bps': 5}, (12,), []),
             ('rule heartbeat=10 s', 'rule', 'move_or_heartbeat', {'heartbeat_ms': 10_000}, (12,), []),
             ('rule move=1 bps', 'rule', 'move_or_heartbeat', {'move_bps': 1}, (12,), [])]
    return rows


def day_start(day):
    return int(dt.datetime.fromisoformat(day).replace(tzinfo=dt.timezone.utc).timestamp() * 1000)


def main():
    stale_refusal = os.environ.get('REPLAY_NO_STALE_REFUSAL') != '1'
    days = sorted(os.path.basename(f)[5:15] for f in glob.glob(os.path.join(R.DATA, 'perp-*.npz')))
    if not days:
        raise ValueError('no perp days')
    with open(os.path.join(R.DATA, 'hl_funding_SOL.json')) as f:
        funding_all = [(r['time'], r['rate']) for r in json.load(f)]
    with open(os.path.join(HERE, 'inputs', 'mark_lag_regimes.json')) as f:
        regimes = json.load(f)
    raw_vol = []
    for day in days:
        with np.load(os.path.join(R.DATA, f'perp-{day}.npz')) as z:
            raw_vol.append(float(np.abs(z['q'] * z['p']).sum()))
    mean_raw, n = float(np.mean(raw_vol)), len(days)
    start, end = day_start(days[0]) - 3_600_000, day_start(days[-1]) + R.DAY_MS
    print(f'Window: {days[0]} to {days[-1]} ({n} days); mean raw volume {R.fmt_usd(mean_raw)}/day.')
    print(f'Fixed cost ${R.FIXED_USD_DAY:.2f}/day; model lines {R.OTHER_BPS:+.3f} bps. '
          'All settings use toxic flow; ages are unweighted over fillable real-flow fills. '
          'Volume scaling uses all raw trades, so refusals lose volume and margin bps remains per dollar of target volume V. '
          f'Stale-mark refusal is {"enabled" if stale_refusal else "disabled"} (landing age > {MAX_REF_AGE_MS} ms). '
          'Arb statistics are before the inventory limit.\n')
    age_rows, mo_rows, arb_rows, main_rows, whole_rows = [], [], [], [], []
    for name, kind, regime, kwargs, fees, whole in specs():
        path = (MarkPath(**kwargs) if kind == 'const' else
                schedule(regimes[regime], start, end, **kwargs) if kind == 'schedule' else
                rule(regimes[regime], start, end, **kwargs))
        if kind == 'rule':
            lands = path.L[(path.L >= start + 3_600_000) & (path.L < end)]
            gap = np.percentile(np.diff(lands), [50, 95])
            print(f'Rule validation {name}: {len(lands)/n:,.1f} pushes/day; '
                  f'landing gaps p50/p95 {gap[0]:.0f}/{gap[1]:.0f} ms.\n', flush=True)
        books = {(fee, hz, v): R.Book() for fee in fees for hz in ('tight', 'none') for v in R.VOLS_USD}
        positions = {fee: 0.0 for fee in fees}
        arb = {fee: [0, 0.0] for fee in fees}
        ages, error_sum, count = [], 0.0, 0
        refused_notional, refused_ticks = 0.0, 0
        mo = np.zeros(len(R.MARKOUT_S))
        # Whole tickets retain their own positions and books across midnight.
        wb = [R.Book() for _ in whole]
        wp, filled = [0.0] * len(whole), np.zeros((len(whole), 2))
        idx0 = 0
        for day in days:
            day0 = day_start(day)
            with np.load(os.path.join(R.DATA, f'perp-{day}.npz')) as z:
                t, q, praw = z['t'], z['q'], z['p']
            if not (np.all(np.isfinite(q)) and np.all(np.isfinite(praw)) and
                    np.all(praw > 0) and np.all(np.diff(t) >= 0)):
                raise ValueError(f'bad cached flow {day}')
            # Keep thinning anchored to original trades, including refused fills.
            gidx = idx0 + np.arange(len(t))
            idx0 += len(t)
            ticks = day0 + np.arange(0, R.DAY_MS, R.ARB_TICK_MS)
            if stale_refusal:
                fillable = path.landing_age(t) <= MAX_REF_AGE_MS
                refused_notional += float(np.abs(q[~fillable] * praw[~fillable]).sum())
                t, q, praw, gidx = t[fillable], q[fillable], praw[fillable], gidx[fillable]
                fillable_ticks = path.landing_age(ticks) <= MAX_REF_AGE_MS
                refused_ticks += int((~fillable_ticks).sum())
                ticks = ticks[fillable_ticks]
            funding = [(ft, rate) for ft, rate in funding_all if day0 <= ft < day0 + R.DAY_MS]
            p, m = R.px(t), path.mark(t)
            ages.append(path.mark_age(t))
            error_sum += float(np.abs(m / p - 1).sum())
            count += len(t)
            mo += R.markouts(t, -q, m) / mean_raw
            pt, mt = R.px(ticks), path.mark(ticks)
            dev = pt / mt - 1
            for fee in fees:
                trig = np.flatnonzero(np.abs(dev) > (fee + R.ARB_COST_BPS) / 1e4)
                at, am, ap = ticks[trig], mt[trig], pt[trig]
                aq = -np.sign(dev[trig]) / am
                arb[fee][0] += len(trig)
                arb[fee][1] -= R.markouts(at, aq, am)[0]
                tt = np.concatenate([t, at])
                order = np.argsort(tt, kind='stable')
                mm, pn = np.concatenate([m, am])[order], np.concatenate([p, ap])[order]
                qq, positions[fee] = R.cap_fills(
                    np.concatenate([-q, aq * R.ARB_CAP_FRAC * mean_raw])[order],
                    pn, positions[fee], R.LP_CAP_FRAC * mean_raw)
                for v in R.VOLS_USD:
                    for hz in ('tight', 'none'):
                        cap = None if R.HEDGES[hz] is None else R.HEDGES[hz] * v
                        books[(fee, hz, v)].run_day(day0, tt[order], qq * (v / mean_raw), mm, pn, fee, cap, funding)
                if fee == 12 and whole:
                    for i, (offset, v, afrac) in enumerate(whole):
                        every = int(round(mean_raw / v))
                        if every < 1:
                            raise ValueError('whole-ticket target exceeds raw daily volume')
                        sel = (gidx - offset * every // 3) % every == 0
                        wt = np.concatenate([t[sel], at])
                        order_w = np.argsort(wt, kind='stable')
                        wm = np.concatenate([m[sel], am])[order_w]
                        wn = np.concatenate([p[sel], ap])[order_w]
                        offered = np.concatenate([-q[sel], aq * afrac * v])[order_w]
                        wq, wp[i] = R.cap_fills(offered, wn, wp[i], R.LP_CAP_FRAC * v)
                        filled[i] += [float(np.abs(wq * wn).sum()), float(np.abs(offered * wn).sum())]
                        wb[i].run_day(day0, wt[order_w], wq, wm, wn, fee, R.HEDGES['tight'] * v, funding)
            print(f'{day} {name} done', file=sys.stderr, flush=True)
        all_age = np.concatenate(ages)
        age_rows.append([name, *[f'{v:.0f}' for v in np.percentile(all_age, [50, 95, 99])],
                         f'{all_age.mean():.0f}', *[f'{np.mean(all_age > age):.2%}' for age in (10_000, 20_000)],
                         f'{error_sum/count*1e4:.3f}',
                         f'{refused_notional/sum(raw_vol):.2%}'])
        mo_rows.append([name, *[f'{-v/n*1e4:.2f}' for v in mo]])
        for fee in fees:
            trades, loss = arb[fee]
            share = trades / n * R.ARB_CAP_FRAC
            loss_bps = loss * R.ARB_CAP_FRAC / n * 1e4
            arb_rows.append([name, fee, f'{trades/n:,.1f}', f'{refused_ticks/n:,.1f}', f'{share:.1%}', f'{loss_bps:.2f}', f'{loss_bps-fee*share:.2f}'])
            row = [name, fee]
            for hz in ('tight', 'none'):
                stats = [R.summary(books[(fee, hz, v)], fee, v, 'toxic', n) for v in R.VOLS_USD]
                margins = [s['trade_bps'] for s in stats]
                s = stats[R.VOLS_USD.index(1_000_000)]
                row += [*[f'{v:.2f}' for v in margins], f"{s['net_day']:,.0f}", f"{s['sd']:,.0f}", R.fmt_usd(s['dd'])]
                row += [R.fmt_usd(be) if be is not None else 'none'
                        for be in (R.breakeven(margins), R.breakeven(margins, R.OTHER_BPS))]
            main_rows.append(row)
        for (offset, v, afrac), book, fill in zip(whole, wb, filled):
            s = R.summary(book, 12, v, 'toxic', n)
            label = name + (' arb x4' if afrac != R.ARB_CAP_FRAC else '')
            whole_rows.append([label, f'{offset}/3 N', R.fmt_usd(v), f"{s['trade_bps']:.2f}",
                               f"{s['net_day']:,.0f}", f'{fill[0]/fill[1]:.1%}' if fill[1] else '—'])
    table('Effective mark at real-flow fills', ['Path', 'Age p50 ms', 'p95 ms', 'p99 ms', 'Mean ms', 'Age >10 s share', 'Age >20 s share', 'Mean absolute mark error bps', 'Refused (stale) share of real-flow notional'], age_rows)
    table('Adverse selection, real flow (positive = LP loses; bps of base volume)', ['Path', *[f'{h}s' for h in R.MARKOUT_S]], mo_rows)
    table('Latency arb before inventory limit', ['Path', 'Fee', 'Trades/day', 'Arb ticks refused/day', 'Volume % of base', 'LP loss at 0s bps', 'Net of arb fee bps'], arb_rows)
    cols = ['margin @$100k', '@$1M', '@$10M', 'Net/day @$1M', 'SD/day', 'Max DD', 'Break-even', '+ model lines']
    table('Toxic flow: tight hedge and unhedged (USD unless bps)', ['Path', 'Fee'] + [f'{hz}: {c}' for hz in ('tight', 'none') for c in cols], main_rows)
    table('Whole tickets, fee 12 bps, tight hedge', ['Path', 'Offset', 'Volume/day', 'Edge bps', 'Net/day', 'Filled share'], whole_rows)


def table(title, headers, rows):
    print(f'\n### {title}\n')
    print('| ' + ' | '.join(headers) + ' |')
    print('|' + '---|' * len(headers))
    for row in rows:
        print('| ' + ' | '.join(map(str, row)) + ' |')


if __name__ == '__main__':
    main()
