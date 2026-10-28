#!/usr/bin/env python3
"""SOL-PERP operator money model. Stdlib only.

    python3 ops/money_model.py                 # default table
    python3 ops/money_model.py --fee-bps 7     # override any assumption

Every default is an assumption; the ones marked TO VERIFY in ops/MONEY_MODEL.md
must be replaced with measured numbers (R2 staging, then live) before money
is committed. Volumes are daily taker notional in USD; we are the only LP, so
we are the counterparty to every taker fill.
"""
import argparse

SLOTS_PER_DAY = 216_000  # 0.4 s slots
ATOM = 1e-6              # USDC has 6 decimals


def args():
    p = argparse.ArgumentParser()
    a = p.add_argument
    # revenue
    a("--fee-bps", type=float, default=5.0, help="trade fee, charged to EACH side")
    a("--liq-share", type=float, default=0.01, help="liquidated notional / volume (TO VERIFY)")
    a("--liq-fee-bps", type=float, default=50.0, help="liquidation_fee_bps")
    a("--backed-share", type=float, default=0.05, help="new backed lien / volume (TO VERIFY)")
    a("--backing-fee-bps", type=float, default=10.0)
    a("--maint-atoms-per-slot", type=float, default=1.0)
    a("--portfolios-per-100k", type=float, default=20.0, help="open portfolios per $100k/day (TO VERIFY)")
    # LP costs
    a("--adverse-bps", type=float, default=2.0, help="LP adverse selection per $ of volume (TO VERIFY)")
    a("--hedged-share", type=float, default=0.3, help="share of volume we hedge off-venue (TO VERIFY)")
    a("--hedge-bps", type=float, default=4.5, help="taker fee on the hedge venue (TO VERIFY)")
    # fixed costs
    a("--sol-usd", type=float, default=150.0, help="TO VERIFY on the day")
    a("--sol-per-day", type=float, default=0.5, help="cranker + pusher tx and priority fees (TO VERIFY)")
    a("--rpc-usd-month", type=float, default=300.0)
    a("--hosting-usd-month", type=float, default=100.0)
    a("--capital-usd", type=float, default=90_000.0, help="see ops/CAPITAL_PLAN.md")
    a("--capital-apy", type=float, default=0.05, help="opportunity cost of idle USDC")
    a("--volumes", type=str, default="100000,1000000,10000000")
    return p.parse_args()


def lines(o, v):
    """Daily P&L lines (USD) at taker volume v."""
    portfolios = o.portfolios_per_100k * v / 100_000
    rev = {
        # Taker side only. The LP side pays the same fee into insurance we
        # control, so it is a transfer from LP capital to insurance, not income.
        "trade fees (taker side)": v * o.fee_bps / 1e4,
        "liquidation fees": v * o.liq_share * o.liq_fee_bps / 1e4,
        "backing fees": v * o.backed_share * o.backing_fee_bps / 1e4,
        "maintenance fees": portfolios * o.maint_atoms_per_slot * SLOTS_PER_DAY * ATOM,
    }
    var = {
        "LP adverse selection": v * o.adverse_bps / 1e4,
        "LP hedge fees": v * o.hedged_share * o.hedge_bps / 1e4,
    }
    fixed = fixed_costs(o)
    return rev, var, fixed


def fixed_costs(o):
    return {
        "SOL tx + priority fees": o.sol_per_day * o.sol_usd,
        "RPC": o.rpc_usd_month * 12 / 365,
        "servers": o.hosting_usd_month * 12 / 365,
        "capital opportunity cost": o.capital_usd * o.capital_apy / 365,
    }


def net_bps(o):
    """Net variable margin per $ of volume, excluding maintenance fees."""
    return (o.fee_bps + o.liq_share * o.liq_fee_bps + o.backed_share * o.backing_fee_bps
            - o.adverse_bps - o.hedged_share * o.hedge_bps) / 1e4


def breakeven(o):
    fixed = sum(fixed_costs(o).values())
    per_dollar = net_bps(o) + o.portfolios_per_100k * o.maint_atoms_per_slot * SLOTS_PER_DAY * ATOM / 100_000
    return float("inf") if per_dollar <= 0 else fixed / per_dollar


def main():
    o = args()
    vols = [float(x) for x in o.volumes.split(",")]
    hdr = f"{'USD per day':32}" + "".join(f"{'$' + format(v, ',.0f') + '/d':>16}" for v in vols)
    print(hdr)
    print("-" * len(hdr))
    rows = [lines(o, v) for v in vols]
    for section, idx, sign in (("revenue", 0, 1), ("variable cost", 1, -1), ("fixed cost", 2, -1)):
        print(section)
        for k in rows[0][idx]:
            print(f"  {k:30}" + "".join(f"{sign * r[idx][k]:>16,.0f}" for r in rows))
    totals = [sum(r[0].values()) - sum(r[1].values()) - sum(r[2].values()) for r in rows]
    print("-" * len(hdr))
    print(f"{'net P&L per day':32}" + "".join(f"{t:>16,.0f}" for t in totals))
    print(f"{'net P&L per 30 days':32}" + "".join(f"{30 * t:>16,.0f}" for t in totals))
    # Cash view: what lands in insurance (gated) vs what is withdrawable normally.
    ins = [2 * v * o.fee_bps / 1e4 for v in vols]
    print(f"{'into insurance, both fee sides':32}" + "".join(f"{x:>16,.0f}" for x in ins))
    print()
    print(f"net variable margin: {net_bps(o) * 1e4:.2f} bps of volume")
    print(f"break-even taker volume: ${breakeven(o):,.0f}/day")


if __name__ == "__main__":
    main()
