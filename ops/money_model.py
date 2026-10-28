#!/usr/bin/env python3
"""SOL-PERP operator money model. Stdlib only.

    python3 ops/money_model.py                 # both LP scenarios, default assumptions
    python3 ops/money_model.py --fee-bps 7     # override any assumption

Every default is an assumption; the ones marked TO VERIFY in ops/MONEY_MODEL.md
must be replaced with measured numbers (R2 staging, then live) before money
is committed. Volumes are daily taker notional in USD.

Scenarios:
  own     we are the only LP, the counterparty to every taker fill.
  outside a professional LP takes the flow; we keep the operator role and
          pay it an off-chain share of the fee income.
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
    a("--listing-fee-usd", type=float, default=0.0, help="permissionless asset-create fee; 0 = listing closed")
    a("--listings-per-month", type=float, default=0.0)
    # own-LP costs
    a("--adverse-bps", type=float, default=2.0, help="LP adverse selection per $ of volume (TO VERIFY)")
    a("--hedged-share", type=float, default=0.3, help="share of volume we hedge off-venue (TO VERIFY)")
    a("--hedge-bps", type=float, default=4.5, help="taker fee on the hedge venue (TO VERIFY)")
    # outside-LP deal
    a("--mm-cost-bps", type=float, default=1.6, help="outside LP adverse + hedge cost, bps of volume (TO VERIFY)")
    a("--mm-margin-bps", type=float, default=1.0, help="profit an outside LP needs, bps of volume (TO VERIFY)")
    # fixed costs
    a("--sol-usd", type=float, default=150.0, help="TO VERIFY on the day")
    a("--sol-per-day", type=float, default=0.5, help="cranker + pusher tx and priority fees (TO VERIFY)")
    a("--rpc-usd-month", type=float, default=300.0)
    a("--hosting-usd-month", type=float, default=100.0)
    a("--capital-usd", type=float, default=90_000.0, help="own-LP capital, see ops/CAPITAL_PLAN.md")
    a("--capital-usd-outside", type=float, default=35_000.0, help="capital without LP and hedge buckets")
    a("--capital-apy", type=float, default=0.05, help="opportunity cost of idle USDC")
    a("--volumes", type=str, default="100000,1000000,10000000")
    return p.parse_args()


def maint_per_dollar(o):
    return o.portfolios_per_100k * o.maint_atoms_per_slot * SLOTS_PER_DAY * ATOM / 100_000


def variable(o, mode):
    """(revenue, cost) per $ of taker volume."""
    other = (o.liq_share * o.liq_fee_bps + o.backed_share * o.backing_fee_bps) / 1e4
    if mode == "own":
        # Taker side only: our LP pays the same fee into insurance we control,
        # a transfer from LP capital to insurance, not income.
        fees = o.fee_bps / 1e4
        cost = (o.adverse_bps + o.hedged_share * o.hedge_bps) / 1e4
    else:
        # Both sides are outside money. The LP also pays its fee side, so its
        # off-chain rebate must return that, cover its costs and pay a margin.
        fees = 2 * o.fee_bps / 1e4
        cost = min(lp_rebate_bps(o), 2 * o.fee_bps) / 1e4
    return fees, other, cost


def lp_rebate_bps(o):
    return o.fee_bps + o.mm_cost_bps + o.mm_margin_bps


def fixed_costs(o, mode):
    cap = o.capital_usd if mode == "own" else o.capital_usd_outside
    return {
        "SOL tx + priority fees": o.sol_per_day * o.sol_usd,
        "RPC": o.rpc_usd_month * 12 / 365,
        "servers": o.hosting_usd_month * 12 / 365,
        "capital opportunity cost": cap * o.capital_apy / 365,
    }


def lines(o, mode, v):
    fees, _, cost = variable(o, mode)
    rev = {
        "trade fees (into insurance)": v * fees,
        "liquidation fees": v * o.liq_share * o.liq_fee_bps / 1e4,
        "backing fees": v * o.backed_share * o.backing_fee_bps / 1e4,
        "maintenance fees": v * maint_per_dollar(o),
        "listing fees": o.listing_fee_usd * o.listings_per_month * 12 / 365,
    }
    var = {"LP adverse + hedge" if mode == "own" else "rebate to outside LP": v * cost}
    return rev, var, fixed_costs(o, mode)


def breakeven(o, mode):
    fees, other, cost = variable(o, mode)
    fixed = sum(fixed_costs(o, mode).values()) - o.listing_fee_usd * o.listings_per_month * 12 / 365
    per_dollar = fees + other - cost + maint_per_dollar(o)
    return float("inf") if per_dollar <= 0 else max(fixed, 0) / per_dollar


def table(o, mode, vols):
    title = {"own": "Scenario A: we are the only LP", "outside": "Scenario B: outside LP + fee share"}[mode]
    hdr = f"{'USD per day':32}" + "".join(f"{'$' + format(v, ',.0f') + '/d':>16}" for v in vols)
    print(title)
    print(hdr)
    print("-" * len(hdr))
    rows = [lines(o, mode, v) for v in vols]
    for section, idx, sign in (("revenue", 0, 1), ("variable cost", 1, -1), ("fixed cost", 2, -1)):
        print(section)
        for k in rows[0][idx]:
            print(f"  {k:30}" + "".join(f"{sign * r[idx][k]:>16,.0f}" for r in rows))
    totals = [sum(r[0].values()) - sum(r[1].values()) - sum(r[2].values()) for r in rows]
    print("-" * len(hdr))
    print(f"{'net P&L per day':32}" + "".join(f"{t:>16,.0f}" for t in totals))
    print(f"{'net P&L per 30 days':32}" + "".join(f"{30 * t:>16,.0f}" for t in totals))
    print(f"{'into insurance, both fee sides':32}" + "".join(f"{2 * v * o.fee_bps / 1e4:>16,.0f}" for v in vols))
    fees, other, cost = variable(o, mode)
    print(f"net variable margin: {(fees + other - cost) * 1e4:.2f} bps of volume (+ maintenance)")
    if mode == "outside":
        print(f"rebate to outside LP: {lp_rebate_bps(o):.2f} bps of volume "
              f"({lp_rebate_bps(o) / (2 * o.fee_bps):.0%} of fee income)")
    print(f"break-even taker volume: ${breakeven(o, mode):,.0f}/day")
    print()


def main():
    o = args()
    vols = [float(x) for x in o.volumes.split(",")]
    for mode in ("own", "outside"):
        table(o, mode, vols)


if __name__ == "__main__":
    main()
