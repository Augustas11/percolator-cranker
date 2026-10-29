//! Hedge policy: the replay's "tight" setting (research/replay/REPORT.md,
//! "Method" step 5). When |LP + hedge| x price exceeds the threshold, send the
//! whole net, capped by the max order and the max total hedge. Pure maths.

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Policy {
    /// Hedge when |residual| x price exceeds this. Tight = 1% of daily volume.
    pub threshold_usd: f64,
    /// Largest single hedge order, SOL.
    pub max_order_sol: f64,
    /// Largest total hedge position either side, SOL.
    pub max_hedge_sol: f64,
    /// No new order within this long of the last one.
    pub cooldown_ms: u64,
    /// Smaller orders are skipped (Hyperliquid's minimum is $10).
    pub min_order_usd: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Hold(&'static str),
    /// Signed SOL: positive buys on the venue, negative sells.
    Order(f64),
}

/// `step` is the venue's size increment (0.01 SOL on Hyperliquid).
pub fn decide(
    p: &Policy,
    lp_sol: f64,
    hedge_sol: f64,
    px: f64,
    step: f64,
    since_last_ms: Option<u64>,
) -> Decision {
    if !(px.is_finite() && px > 0.0 && lp_sol.is_finite() && hedge_sol.is_finite()) {
        return Decision::Hold("bad_input");
    }
    let residual = lp_sol + hedge_sol;
    if residual.abs() * px <= p.threshold_usd {
        return Decision::Hold("within_threshold");
    }
    if since_last_ms.is_some_and(|t| t < p.cooldown_ms) {
        return Decision::Hold("cooldown");
    }
    let want = -residual;
    let target = (hedge_sol + want).clamp(-p.max_hedge_sol, p.max_hedge_sol);
    let order = (target - hedge_sol).clamp(-p.max_order_sol, p.max_order_sol);
    let order = round_toward_zero(order, step);
    if order == 0.0 || order.signum() != want.signum() {
        return Decision::Hold("max_hedge");
    }
    if order.abs() * px < p.min_order_usd {
        return Decision::Hold("below_min_order");
    }
    Decision::Order(order)
}

pub fn round_toward_zero(x: f64, step: f64) -> f64 {
    // The epsilon keeps 0.29 / 0.01 = 28.999... from losing a step.
    let n = (x.abs() / step + 1e-9).floor();
    (n * step).copysign(x) + 0.0
}

/// Walk one side of the book (best level first) for `size` SOL.
/// Some((average fill price, worst level touched)), or None if the book is too thin.
pub fn walk(levels: &[(f64, f64)], size: f64) -> Option<(f64, f64)> {
    let (mut left, mut cost, mut worst) = (size, 0.0, 0.0);
    for &(px, sz) in levels {
        if left <= 0.0 {
            break;
        }
        let take = left.min(sz);
        cost += take * px;
        left -= take;
        worst = px;
    }
    (left <= 1e-12 && size > 0.0).then(|| (cost / size, worst))
}

/// All-in taker cost in bps of notional: book slippage vs mid plus the fee.
pub fn cost_bps(avg_px: f64, mid: f64, fee_bps: f64) -> f64 {
    (avg_px - mid).abs() / mid * 1e4 + fee_bps
}

/// Max-loss guard on what the hedger is answerable for: the P&L of the
/// unhedged residual (marked at the venue mid) less every hedge cost paid.
#[derive(Debug, Default)]
pub struct Guard {
    pub pnl_usd: f64,
    last: Option<(f64, f64)>,
}

impl Guard {
    /// Call each tick with the residual held since the last tick and the new mid.
    pub fn mark(&mut self, residual_sol: f64, mid: f64) {
        if let Some((res, px)) = self.last {
            self.pnl_usd += res * (mid - px);
        }
        self.last = Some((residual_sol, mid));
    }
    /// After a fill, the residual changes at once (the mid does not).
    pub fn fill(&mut self, residual_sol: f64, cost_usd: f64) {
        self.pnl_usd -= cost_usd;
        if let Some((_, px)) = self.last {
            self.last = Some((residual_sol, px));
        }
    }
    pub fn breached(&self, max_loss_usd: f64) -> bool {
        self.pnl_usd < -max_loss_usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Policy {
        Policy {
            threshold_usd: 10_000.0,
            max_order_sol: 500.0,
            max_hedge_sol: 1_000.0,
            cooldown_ms: 2_000,
            min_order_usd: 10.0,
        }
    }

    #[test]
    fn holds_inside_threshold_and_hedges_whole_net_outside() {
        // $9,990 of residual at $100: inside.
        assert_eq!(decide(&p(), 99.9, 0.0, 100.0, 0.01, None), Decision::Hold("within_threshold"));
        // Exactly at the threshold still holds (strictly greater hedges).
        assert_eq!(decide(&p(), 100.0, 0.0, 100.0, 0.01, None), Decision::Hold("within_threshold"));
        // LP long 150 SOL: sell all 150 on the venue.
        assert_eq!(decide(&p(), 150.0, 0.0, 100.0, 0.01, None), Decision::Order(-150.0));
        // LP short 150, already 20 long hedge: buy the other 130.
        assert_eq!(decide(&p(), -150.0, 20.0, 100.0, 0.01, None), Decision::Order(130.0));
        // The existing hedge counts: LP 150 long, hedge -100 short, residual 50 = $5k.
        assert_eq!(decide(&p(), 150.0, -100.0, 100.0, 0.01, None), Decision::Hold("within_threshold"));
    }

    #[test]
    fn caps_order_size_total_hedge_and_rounds_to_step() {
        assert_eq!(decide(&p(), 900.0, 0.0, 100.0, 0.01, None), Decision::Order(-500.0));
        // Hedge already -800: only 200 more allowed.
        assert_eq!(decide(&p(), 1_200.0, -800.0, 100.0, 0.01, None), Decision::Order(-200.0));
        // At the cap: nothing more.
        assert_eq!(decide(&p(), 2_000.0, -1_000.0, 100.0, 0.01, None), Decision::Hold("max_hedge"));
        // Going back toward zero is always allowed.
        assert_eq!(decide(&p(), -500.0, -1_000.0, 100.0, 0.01, None), Decision::Order(500.0));
        // 123.456 SOL rounds toward zero to 123.45.
        assert_eq!(decide(&p(), 123.456, 0.0, 100.0, 0.01, None), Decision::Order(-123.45));
        assert_eq!(round_toward_zero(0.29, 0.01), 0.29);
        assert_eq!(round_toward_zero(-0.004, 0.01), 0.0);
    }

    #[test]
    fn hedge_outside_cap_respects_order_cap_and_residual_direction() {
        let mut p = p();
        p.max_order_sol = 100.0;
        p.max_hedge_sol = 500.0;
        assert_eq!(decide(&p, 0.0, 1_000.0, 100.0, 0.01, None), Decision::Order(-100.0));
        assert_eq!(decide(&p, -1_000.0, 800.0, 100.0, 0.01, None), Decision::Hold("max_hedge"));
    }

    #[test]
    fn cooldown_min_order_and_bad_input() {
        assert_eq!(decide(&p(), 150.0, 0.0, 100.0, 0.01, Some(1_999)), Decision::Hold("cooldown"));
        assert_eq!(decide(&p(), 150.0, 0.0, 100.0, 0.01, Some(2_000)), Decision::Order(-150.0));
        let mut tiny = p();
        tiny.threshold_usd = 1.0;
        assert_eq!(decide(&tiny, 0.05, 0.0, 100.0, 0.01, None), Decision::Hold("below_min_order"));
        assert_eq!(decide(&p(), f64::NAN, 0.0, 100.0, 0.01, None), Decision::Hold("bad_input"));
        assert_eq!(decide(&p(), 150.0, 0.0, 0.0, 0.01, None), Decision::Hold("bad_input"));
    }

    #[test]
    fn book_walk_and_cost() {
        let asks = [(100.0, 10.0), (100.1, 10.0), (100.5, 100.0)];
        assert_eq!(walk(&asks, 5.0), Some((100.0, 100.0)));
        let (avg, worst) = walk(&asks, 20.0).unwrap();
        assert!((avg - 100.05).abs() < 1e-9 && worst == 100.1);
        assert_eq!(walk(&asks, 121.0), None);
        assert_eq!(walk(&asks, 0.0), None);
        // Mid 99.95, filled at 100.05: 10.005 bps slippage + 4.5 fee.
        assert!((cost_bps(100.05, 99.95, 4.5) - 14.5050).abs() < 1e-3);
    }

    #[test]
    fn guard_marks_residual_and_charges_costs() {
        let mut g = Guard::default();
        g.mark(100.0, 100.0); // first mark only sets the base
        assert_eq!(g.pnl_usd, 0.0);
        g.mark(100.0, 99.0); // long 100 residual, -$1
        assert_eq!(g.pnl_usd, -100.0);
        g.fill(0.0, 5.0); // hedged flat, paid $5
        assert_eq!(g.pnl_usd, -105.0);
        g.mark(0.0, 90.0); // flat: the drop costs nothing
        assert_eq!(g.pnl_usd, -105.0);
        assert!(g.breached(100.0) && !g.breached(105.0));
    }
}
