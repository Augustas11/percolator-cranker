#!/usr/bin/env python3
"""Extract measured pusher latency, retaining segment boundaries for cadence replay."""
import datetime as dt
import json
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
COMPONENTS = ('src_age_ms', 'send_ms', 'confirm_ms')
SOURCES = {
    'pyth_public': 'Pyth free price / public devnet RPC; mac segments 0..23',
    'median_public': 'Exchange median / public RPC; mac 24..26, pearl 0..31,34',
    'median_fast': 'Exchange median / paid RPC / every move; pearl 32,33,35',
    'move_or_heartbeat': 'Exchange median / paid RPC / 10 bps or 60 slots; pearl 36..41',
}


def pairs(rows):
    """Gap and later-row latency; never join runs or retain outage gaps."""
    gap = np.diff(rows['at_ms'])
    same = np.diff(rows['segment']) == 0
    keep = same & (gap > 0) & (gap <= 120_000)
    return gap[keep], np.asarray(rows['ms'])[1:][keep]


def extract(paths):
    regimes = {name: dict(source=source, ms=[], at_ms=[], segment=[], latency_rows=[],
                          **{key: [] for key in COMPONENTS}) for name, source in SOURCES.items()}
    for file_i, (path, expected) in enumerate(zip(paths, (27, 42))):
        segment = -1
        with open(os.path.expanduser(path)) as f:
            for line in f:
                if line.startswith('staging SOL-PERP'):
                    segment += 1
                if not line.startswith('{"event":"mark_lag"'):
                    continue
                if segment < 0:
                    raise ValueError(f'{path}: mark_lag before first segment')
                row = json.loads(line)
                if file_i == 0:
                    name = 'pyth_public' if segment < 24 else 'median_public'
                else:
                    name = ('move_or_heartbeat' if segment >= 36 else
                            'median_fast' if segment in (32, 33, 35) else 'median_public')
                out = regimes[name]
                if all(key in row for key in COMPONENTS):
                    out['latency_rows'].append(len(out['ms']))
                    for key in COMPONENTS:
                        out[key].append(int(row[key]))
                out['ms'].append(int(row['ms']))
                out['at_ms'].append(int(row['at_ms']))
                out['segment'].append(segment + file_i * 27)
        if segment + 1 != expected:
            raise ValueError(f'{path}: expected {expected} segments, found {segment + 1}')
    return regimes


def stats(values, percentiles=(50, 95)):
    return '/'.join(f'{v:.0f}' for v in np.percentile(values, percentiles)) if len(values) else '—'


def main():
    if len(sys.argv) != 3:
        raise SystemExit(f'Usage: {sys.argv[0]} mac.log pearl.log')
    regimes = extract(sys.argv[1:])
    target = os.path.join(HERE, 'inputs', 'mark_lag_regimes.json')
    with open(target, 'w') as f:
        json.dump(regimes, f, separators=(',', ':'))
        f.write('\n')
    print('| Regime | n | First UTC | Last UTC | Lag p50/p95/p99/max ms | Gap p50/p95 ms | Source age p50/p95 ms | Send p50/p95 ms | Confirm p50/p95 ms |')
    print('|---|---:|---|---|---:|---:|---:|---:|---:|')
    for name, rows in regimes.items():
        times = [dt.datetime.fromtimestamp(t / 1000, dt.timezone.utc).isoformat() for t in (min(rows['at_ms']), max(rows['at_ms']))]
        # Report all within-segment gaps; only the schedule drops outages.
        gap = np.diff(rows['at_ms'])
        gap = gap[np.diff(rows['segment']) == 0]
        print(f"| {name} | {len(rows['ms'])} | {' | '.join(times)} | {stats(rows['ms'], (50,95,99,100))} | {stats(gap)} | "
              + ' | '.join(stats(rows[key]) for key in COMPONENTS) + ' |')


if __name__ == '__main__':
    main()
