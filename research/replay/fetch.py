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

Each day is reduced to two compact .npz files (time ms, price, signed qty) and the
zip is deleted, so the cache is ~2 MB/day.
"""
import argparse
import csv
import datetime as dt
import hashlib
import io
import json
import os
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


def get(url, body=None):
    req = urllib.request.Request(url, data=None if body is None else json.dumps(body).encode(),
                                 headers={"content-type": "application/json"} if body else {})
    with urllib.request.urlopen(req, timeout=120) as r:
        return r.read()


def rows(zbytes):
    """aggTrades CSV -> (time_ms, price, taker_signed_qty). Header optional (spot has none)."""
    with zipfile.ZipFile(io.BytesIO(zbytes)) as z:
        names = z.namelist()
        if len(names) != 1 or not names[0].endswith(".csv") or "/" in names[0]:
            raise ValueError(f"unexpected zip contents {names}")
        text = io.TextIOWrapper(z.open(names[0]), encoding="ascii")
        t, p, q = [], [], []
        for r in csv.reader(text):
            if not r or not r[0].isdigit():
                continue  # header
            # cols: id, price, qty, first_id, last_id, time, is_buyer_maker[, best_match]
            ts = int(r[5])
            if ts > 10**14:  # spot switched to microseconds in 2025
                ts //= 1000
            t.append(ts)
            p.append(float(r[1]))
            q.append(-float(r[2]) if r[6].lower() == "true" else float(r[2]))  # buyer maker => taker sold
    t, p, q = np.array(t, dtype=np.int64), np.array(p), np.array(q)
    o = np.argsort(t, kind="stable")
    return t[o], p[o], q[o]


def fetch_day(kind, day):
    out = os.path.join(DATA, f"{kind}-{day}.npz")
    if os.path.exists(out):
        return
    url = SERIES[kind].format(d=day)
    want = get(url + ".CHECKSUM").decode().split()[0]
    z = get(url)
    have = hashlib.sha256(z).hexdigest()
    if have != want:
        raise SystemExit(f"checksum mismatch {url}: {have} != {want}")
    t, p, q = rows(z)
    np.savez_compressed(out, t=t, p=p, q=q)
    print(f"{kind} {day}: {len(t)} trades", flush=True)


def fetch_funding(start, end):
    out, cur = [], start
    while cur < end:
        batch = json.loads(get(HL, {"type": "fundingHistory", "coin": "SOL", "startTime": cur, "endTime": end}))
        if not batch:
            break
        out += [{"time": b["time"], "rate": float(b["fundingRate"])} for b in batch]
        nxt = batch[-1]["time"] + 1
        if nxt <= cur:
            break
        cur = nxt
    with open(os.path.join(DATA, "hl_funding_SOL.json"), "w") as f:
        json.dump(out, f)
    print(f"funding: {len(out)} hourly rates")


def main():
    a = argparse.ArgumentParser()
    a.add_argument("--start", default="2026-07-30")
    a.add_argument("--days", type=int, default=90)
    a.add_argument("--book", action="store_true", help="also refresh inputs/hl_l2book_SOL.json")
    o = a.parse_args()
    os.makedirs(DATA, exist_ok=True)
    d0 = dt.date.fromisoformat(o.start)
    days = [(d0 + dt.timedelta(i)).isoformat() for i in range(o.days)]
    for day in days:
        for kind in SERIES:
            fetch_day(kind, day)
    ms = lambda d: int(dt.datetime.fromisoformat(d).replace(tzinfo=dt.timezone.utc).timestamp() * 1000)
    fetch_funding(ms(days[0]), ms(days[-1]) + 86_400_000)
    if o.book:
        os.makedirs(INPUTS, exist_ok=True)
        book = json.loads(get(HL, {"type": "l2Book", "coin": "SOL"}))
        with open(os.path.join(INPUTS, "hl_l2book_SOL.json"), "w") as f:
            json.dump(book, f, indent=0)
        print("book snapshot saved")


if __name__ == "__main__":
    sys.exit(main())
