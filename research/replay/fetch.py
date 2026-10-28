#!/usr/bin/env python3
"""Fetch the public inputs for the SOL-PERP replay into research/replay/data/ (git-ignored).

    python3 research/replay/fetch.py [--start 2026-07-30] [--days 90]

No keys, nothing paid. Sources:
  * taker flow  Binance USD-M futures SOLUSDT aggTrades, daily zips (data.binance.vision),
                each checked against its published SHA-256 .CHECKSUM file.
  * oracle      Binance spot SOLUSDT aggTrades, same source, same check. Stand-in for
                Pyth SOL/USD: Pyth Benchmarks and Hermes history now return 401.
  * hedge       Hyperliquid info API: SOL hourly funding history for the window, and
                (only with --book) a fresh l2Book snapshot. The committed snapshot in
                inputs/ is what the report used.

Spot timestamps (microseconds) are rounded UP to the millisecond so no print is seen early.
One extra spot day before the window warms up the oracle. Each day is reduced to two compact .npz files (time ms, price, signed qty) and the
zip is deleted, so the cache is ~2 MB/day.
"""
import argparse
import csv
import datetime as dt
import hashlib
import io
import json
import math
import os
import tempfile
import sys
import urllib.request
import zipfile

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "data")
INPUTS = os.path.join(HERE, "inputs")
BINANCE = "https://data.binance.vision/data"
SERIES = {"perp": f"{BINANCE}/futures/um/daily/aggTrades/SOLUSDT/SOLUSDT-aggTrades-{{d}}.zip",
          "spot": f"{BINANCE}/spot/daily/aggTrades/SOLUSDT/SOLUSDT-aggTrades-{{d}}.zip"}
HL = "https://api.hyperliquid.xyz/info"
MAX_DOWNLOAD = 64 << 20    # a daily zip is ~1-5 MB
MAX_CSV = 1 << 30          # uncompressed CSV
MAX_ROWS = 20_000_000
MAX_FUNDING_PAGES = 100


def get(url, body=None, limit=MAX_DOWNLOAD):
    req = urllib.request.Request(url, data=None if body is None else json.dumps(body).encode(),
                                 headers={"content-type": "application/json"} if body else {})
    with urllib.request.urlopen(req, timeout=120) as r:
        data = r.read(limit + 1)
    if len(data) > limit:
        raise ValueError(f"response over {limit} bytes: {url}")
    return data


def finite_pos(x):
    v = float(x)
    if not (math.isfinite(v) and v > 0):
        raise ValueError(f"bad number {x!r}")
    return v


def rows(zbytes, day0):
    """aggTrades CSV -> (time_ms, price, taker_signed_qty). Every row is validated; one bad row fails the day."""
    with zipfile.ZipFile(io.BytesIO(zbytes)) as z:
        info = z.infolist()
        if len(info) != 1 or not info[0].filename.endswith(".csv") or "/" in info[0].filename:
            raise ValueError(f"unexpected zip contents {[i.filename for i in info]}")
        if info[0].file_size > MAX_CSV:
            raise ValueError("CSV too large")
        text = io.TextIOWrapper(z.open(info[0]), encoding="ascii")
        t, p, q = [], [], []
        for n, r in enumerate(csv.reader(text)):
            if n == 0 and r and r[0] == "agg_trade_id":
                continue  # futures files have a header; spot files do not
            if n > MAX_ROWS:
                raise ValueError("too many rows")
            # cols: id, price, qty, first_id, last_id, time, is_buyer_maker[, best_match]
            ts = int(r[5])
            if ts > 10**14:  # spot switched to microseconds in 2025: round UP, never early
                ts = -(-ts // 1000)
            if not day0 <= ts <= day0 + 86_400_000:
                raise ValueError(f"timestamp {ts} outside the day")
            maker = r[6].lower()
            if maker not in ("true", "false"):
                raise ValueError(f"bad is_buyer_maker {r[6]!r}")
            qty = finite_pos(r[2])
            t.append(ts)
            p.append(finite_pos(r[1]))
            q.append(-qty if maker == "true" else qty)  # buyer is maker => taker sold
    t, p, q = np.array(t, dtype=np.int64), np.array(p), np.array(q)
    o = np.argsort(t, kind="stable")
    return t[o], p[o], q[o]


def atomic_write(out, write):
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(out), suffix=".tmp")
    try:
        with os.fdopen(fd, "wb") as f:
            write(f)
        os.replace(tmp, out)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def fetch_day(kind, day):
    out = os.path.join(DATA, f"{kind}-{day}.npz")
    if os.path.exists(out):
        return
    url = SERIES[kind].format(d=day)
    want = get(url + ".CHECKSUM", limit=4096).decode().split()[0]
    z = get(url)
    have = hashlib.sha256(z).hexdigest()
    if have != want:
        raise SystemExit(f"checksum mismatch {url}: {have} != {want}")
    t, p, q = rows(z, ms(day))
    atomic_write(out, lambda f: np.savez_compressed(f, t=t, p=p, q=q))
    print(f"{kind} {day}: {len(t)} trades", flush=True)


def fetch_funding(start, end):
    out, cur = [], start
    for _ in range(MAX_FUNDING_PAGES):
        if cur >= end:
            break
        batch = json.loads(get(HL, {"type": "fundingHistory", "coin": "SOL", "startTime": cur, "endTime": end}))
        if not isinstance(batch, list) or not batch:
            break
        for b in batch:
            rate = float(b["fundingRate"])
            if not (math.isfinite(rate) and abs(rate) < 0.01 and start <= int(b["time"]) <= end):
                raise ValueError(f"bad funding row {b!r}")
            out.append({"time": int(b["time"]), "rate": rate})
        nxt = int(batch[-1]["time"]) + 1
        if nxt <= cur:
            break
        cur = nxt
    atomic_write(os.path.join(DATA, "hl_funding_SOL.json"), lambda f: f.write(json.dumps(out).encode()))
    print(f"funding: {len(out)} hourly rates")


def ms(day):
    return int(dt.datetime.fromisoformat(day).replace(tzinfo=dt.timezone.utc).timestamp() * 1000)


def main():
    a = argparse.ArgumentParser()
    a.add_argument("--start", default="2026-07-30")
    a.add_argument("--days", type=int, default=90)
    a.add_argument("--book", action="store_true", help="also refresh inputs/hl_l2book_SOL.json")
    o = a.parse_args()
    os.makedirs(DATA, exist_ok=True)
    d0 = dt.date.fromisoformat(o.start)
    days = [(d0 + dt.timedelta(i)).isoformat() for i in range(o.days)]
    fetch_day("spot", (d0 - dt.timedelta(1)).isoformat())  # oracle warm-up: no fill before the first print
    for day in days:
        for kind in SERIES:
            fetch_day(kind, day)
    fetch_funding(ms(days[0]), ms(days[-1]) + 86_400_000)
    if o.book:
        os.makedirs(INPUTS, exist_ok=True)
        book = get(HL, {"type": "l2Book", "coin": "SOL"})
        json.loads(book)  # parse check; replay.py validates the levels
        atomic_write(os.path.join(INPUTS, "hl_l2book_SOL.json"), lambda f: f.write(book))
        print("book snapshot saved")


if __name__ == "__main__":
    sys.exit(main())
