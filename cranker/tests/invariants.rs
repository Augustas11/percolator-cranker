//! Invariant oracle smoke test.
//!
//! Loads the live market + all its portfolios and runs the conservation
//! checklist once. This is the test to run FIRST when bringing up a market:
//! if it fails, either (a) the market genuinely violates conservation (a real
//! bug — escalate), or (b) `common::PORTFOLIO_ENGINE_OFF` is wrong and the
//! decode is reading garbage. Rule out (b) before believing (a).

mod common;

#[test]
fn market_snapshot_satisfies_conservation() -> anyhow::Result<()> {
    let Some(live) = common::live_from_env() else {
        common::skip("market_snapshot_satisfies_conservation");
        return Ok(());
    };

    let (mg, portfolios) = common::assert_conservation_live(&live)?;
    eprintln!(
        "conservation OK: {} portfolios, c_tot={}, vault={}, insurance={}, pnl_pos_tot={}",
        portfolios.len(),
        mg.c_tot,
        mg.vault,
        mg.insurance,
        mg.pnl_pos_tot,
    );
    Ok(())
}
