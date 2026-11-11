//! Shadow-ledger money harness for the Percolator v16 engine.
//!
//! The harness drives one market (one asset, two source domains) through public
//! engine calls, the way the upstream tests and the wrapper program do, and keeps
//! an independent ledger of what every account is owed:
//!
//! ```text
//! fair_i = deposits_i - withdrawals_i + sum(position_i * delta_price)
//!          - bankruptcy share_i + forgiven_i
//! ```
//!
//! Fees and funding are off, so that value is exact up to a small, explicit rounding
//! tolerance. Positions and prices are tracked here; nothing is read back from
//! engine-derived PnL. The only engine values the shadow consumes are decisions the
//! engine makes on its own (how much a liquidation closes) and the token flows it
//! reports (payouts, provider withdrawals, insurance, retired atoms, vault).
//!
//! At the end of every run the market is resolved, every account is closed, the
//! provider withdraws what it can, the terminal slab advances, and [`Report::check`]
//! evaluates the money invariants.

use percolator::{
    AutoCrankObservationV16, AutoCrankPlanV16, AutoCrankWorkV16, BackingBucketStatusV16,
    EngineAssetSlotV16Account, LiquidationRequestV16, Market, MarketGroupV16HeaderAccount,
    MarketGroupV16ViewMut, MarketModeV16, PortfolioAccountV16Account, PortfolioV16View,
    PortfolioV16ViewMut, ProvenanceHeaderV16, ProvenanceHeaderV16Account,
    ResolvedCloseOutcomeV16, SideV16, TerminalSlabOutcomeV16, TradeRequestV16, V16Config,
    V16Error, BOUND_SCALE, POS_SCALE,
};
use std::fmt;

pub use percolator;

const PS: i128 = POS_SCALE as i128;

/// Rounding tolerance, in atoms, charged once per operation that can introduce a
/// floor: the base settlement of an account, each ADL scaling or bankruptcy share it
/// receives, and each price move while it holds a fractional (post-ADL) position.
/// Whole-unit positions with integer prices are exact, so most runs check at 2 atoms.
pub const TOL_ATOMS_PER_OP: u128 = 2;

/// A winner off by more than this, with the total right, counts as money moved
/// between traders. Rounding alone reached 111 atoms in our surveys; the #253
/// replay moves 21,391.
pub const TRANSFER_ALARM_ATOMS: u128 = 1_000;

/// Market parameters for one run.
#[derive(Clone, Copy, Debug)]
pub struct Setup {
    pub accounts: usize,
    pub initial_margin_bps: u64,
    pub maintenance_margin_bps: u64,
    pub price_cap_bps: u64,
    pub start_price: u64,
    /// Provider fresh counterparty backing deposited at start: [domain 0, domain 1].
    /// Domain 0 is asset-0 Long (long losers pay in, backs short gains); domain 1 is
    /// asset-0 Short (backs long gains).
    pub provider: [u128; 2],
    pub h_max: u64,
    pub expiry: u64,
}

impl Setup {
    /// Leveraged market: 10% IM, 5% MM, 40 bps/slot cap (the #250 setup).
    pub fn leveraged(accounts: usize, provider: [u128; 2]) -> Self {
        Self {
            accounts,
            initial_margin_bps: 1_000,
            maintenance_margin_bps: 500,
            price_cap_bps: 40,
            start_price: 10_000,
            provider,
            h_max: 10,
            expiry: 1_000_000,
        }
    }

    /// Engine defaults from `public_user_fund_with_market_slots` (100% margin).
    pub fn defaults(accounts: usize, provider: [u128; 2]) -> Self {
        Self {
            accounts,
            initial_margin_bps: 10_000,
            maintenance_margin_bps: 10_000,
            price_cap_bps: 10_000,
            start_price: 100,
            provider,
            h_max: 10,
            expiry: 1_000_000,
        }
    }

    pub fn config(&self) -> V16Config {
        let mut cfg = V16Config::public_user_fund_with_market_slots(1, 1, 0, self.h_max);
        cfg.initial_margin_bps = self.initial_margin_bps;
        cfg.maintenance_margin_bps = self.maintenance_margin_bps;
        cfg.max_price_move_bps_per_slot = self.price_cap_bps;
        cfg.max_accrual_dt_slots = 1;
        // Fees and funding stay at the constructor's zero values.
        cfg
    }
}

/// One step of a scenario. Account indices are taken modulo the account count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Deposit { acct: usize, amount: u128 },
    /// `buyer` buys `units` whole units from `seller` at the current mark.
    Trade { buyer: usize, seller: usize, units: u64 },
    /// Advance one slot and move the mark by `bps` (clamped to the cap).
    Price { bps: i32 },
    /// `slots` consecutive full-cap moves in one direction, one slot each, with no
    /// crank in between (how losers outrun their margin and go bankrupt).
    Jump { up: bool, slots: u8 },
    /// Advance one slot and set the mark to `price` (the engine rejects moves past the cap).
    PriceTo { price: u64 },
    /// Advance one slot at the same mark.
    Advance,
    /// Permissionless auto crank to its fixpoint.
    Crank { acct: usize },
    /// One permissionless auto-crank step.
    CrankOnce { acct: usize },
    /// One auto-crank step on the open account with the lowest shadow equity.
    CrankWorst,
    /// Full account refresh.
    Refresh { acct: usize },
    Liquidate { acct: usize },
    Convert { acct: usize },
    Withdraw { acct: usize, amount: u128 },
    /// Provider withdraws fresh backing not needed by current claims in `domain`.
    ProviderWithdraw { domain: usize },
}

/// Invariant names. `is_known_bug_class` marks the ones the three open upstream
/// issues (#250, #252, #253) violate; the rest must always hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Invariant {
    /// 1. Token conservation across payouts, provider, insurance, retired, vault.
    Conservation,
    /// Per step: vault covers capital + insurance + provider earnings + fresh backing.
    StepSolvency,
    /// Per step: engine effective positions match the shadow position model.
    PositionModel,
    /// Terminal: every account closes, provider withdraws, slab reaches ReadyToClose.
    TerminalLiveness,
    /// Terminal liveness failure whose stuck accounts all hold source face in a domain
    /// with zero backing and no fresh bucket to expire (the face can never realize or
    /// be demoted to junior). Seen only at the pin, as a consequence of the #457
    /// case-2 reversal leak; does not reproduce on upstream master.
    StuckUnbackedClaim,
    /// 2. No default: every payout equals its shadow fair value.
    NoDefaultPayout,
    /// 2. No default: nothing retired, burned, stuck or left in insurance.
    NoDefaultRetired,
    /// 3. Default: total loss socialized onto winners equals the shadow shortfall.
    LossShareTotal,
    /// 3. Default: each winner bears its exposure-proportional share.
    LossSharePerWinner,
    /// 4. No default: provider recovers its full principal.
    ProviderPrincipal,
}

impl Invariant {
    pub fn is_known_bug_class(self) -> bool {
        matches!(
            self,
            Invariant::StuckUnbackedClaim
                | Invariant::NoDefaultPayout
                | Invariant::NoDefaultRetired
                | Invariant::LossShareTotal
                | Invariant::LossSharePerWinner
                | Invariant::ProviderPrincipal
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Invariant::Conservation => "inv1_conservation",
            Invariant::StepSolvency => "step_solvency",
            Invariant::PositionModel => "position_model",
            Invariant::TerminalLiveness => "terminal_liveness",
            Invariant::StuckUnbackedClaim => "terminal_stuck_unbacked_claim",
            Invariant::NoDefaultPayout => "inv2_no_default_payout",
            Invariant::NoDefaultRetired => "inv2_no_default_retired",
            Invariant::LossShareTotal => "inv3_loss_share_total",
            Invariant::LossSharePerWinner => "inv3_loss_share_per_winner",
            Invariant::ProviderPrincipal => "inv4_provider_principal",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        ALL_INVARIANTS.iter().copied().find(|i| i.label() == s)
    }
}

pub const ALL_INVARIANTS: [Invariant; 10] = [
    Invariant::Conservation,
    Invariant::StepSolvency,
    Invariant::PositionModel,
    Invariant::TerminalLiveness,
    Invariant::StuckUnbackedClaim,
    Invariant::NoDefaultPayout,
    Invariant::NoDefaultRetired,
    Invariant::LossShareTotal,
    Invariant::LossSharePerWinner,
    Invariant::ProviderPrincipal,
];

#[derive(Clone, Debug)]
pub struct Violation {
    pub invariant: Invariant,
    pub detail: String,
}

/// A bankruptcy booking the shadow ledger recorded.
#[derive(Clone, Debug)]
pub struct DefaultEvent {
    pub step: usize,
    pub victim: usize,
    /// Shadow shortfall: the victim's negative equity at liquidation.
    pub shortfall: u128,
    /// What the engine says it booked (B + explicit loss).
    pub engine_booked: u128,
    /// (account, exposure in raw q, expected share in atoms).
    pub shares: Vec<(usize, u128, u128)>,
    /// True when the account was still underwater at resolution (no live liquidation
    /// booked it). The resolved-mode split is not modelled per winner, so only the
    /// total is checked for these.
    pub terminal: bool,
}

#[derive(Clone, Debug, Default)]
struct Shadow {
    price: u64,
    dep: Vec<u128>,
    wd: Vec<u128>,
    pos: Vec<i128>,
    /// Mark-to-market PnL in atoms * POS_SCALE.
    mtm_raw: Vec<i128>,
    b_share: Vec<u128>,
    forgiven: Vec<u128>,
    tol_ops: Vec<u128>,
    fractional: Vec<bool>,
    defaults: Vec<DefaultEvent>,
    provider_principal: u128,
    provider_out: u128,
    liquidations: usize,
}

impl Shadow {
    fn equity(&self, i: usize) -> i128 {
        self.dep[i] as i128 - self.wd[i] as i128 + self.mtm_raw[i].div_euclid(PS)
            - self.b_share[i] as i128
            + self.forgiven[i] as i128
    }

    /// Expected total paid out to account i over its life (withdrawals + payout).
    fn fair_out(&self, i: usize) -> i128 {
        self.dep[i] as i128 + self.mtm_raw[i].div_euclid(PS) - self.b_share[i] as i128
            + self.forgiven[i] as i128
    }

    fn fair_out_before_losses(&self, i: usize) -> i128 {
        self.dep[i] as i128 + self.mtm_raw[i].div_euclid(PS) + self.forgiven[i] as i128
    }

    fn tol(&self, i: usize) -> u128 {
        TOL_ATOMS_PER_OP * (1 + self.tol_ops[i])
    }
}

/// Outcome of one live action.
#[derive(Clone, Debug)]
pub enum StepResult {
    Applied,
    Rejected(V16Error),
    Skipped(&'static str),
}

pub struct World {
    pub setup: Setup,
    header: MarketGroupV16HeaderAccount,
    markets: Vec<Market<u64>>,
    accts: Vec<PortfolioAccountV16Account>,
    slot: u64,
    shadow: Shadow,
    /// Human-readable trace of every action and its result.
    pub trace: Vec<String>,
    step_violations: Vec<Violation>,
    pub recovery: bool,
    /// Resolved close payout per account (closed by a direct close or by a crank).
    closed_payout: Vec<Option<u128>>,
    provider_out_domain: [u128; 2],
    resized_before_loss_settled: bool,
}

type Snap = (
    MarketGroupV16HeaderAccount,
    Vec<Market<u64>>,
    Vec<PortfolioAccountV16Account>,
);

fn account_fixture(seed: u8) -> PortfolioAccountV16Account {
    let header = ProvenanceHeaderV16Account::from_runtime(&ProvenanceHeaderV16::new(
        [1; 32], [seed; 32], [3; 32],
    ));
    let mut account = PortfolioAccountV16Account::default();
    account.init_empty_in_place(header).unwrap();
    account
}

impl World {
    pub fn new(setup: Setup) -> Self {
        let cfg = setup.config();
        let mut header = MarketGroupV16HeaderAccount::new_dynamic([1; 32], cfg, 1, 0)
            .expect("market config must validate");
        let mut markets = vec![Market::new(0u64, EngineAssetSlotV16Account::default())];
        header
            .activate_empty_asset_slot_not_atomic(0, &mut markets[0].engine, setup.start_price, 1)
            .expect("asset activation");
        let accts = (0..setup.accounts)
            .map(|i| account_fixture(10 + i as u8))
            .collect();
        let n = setup.accounts;
        let mut w = World {
            setup,
            header,
            markets,
            accts,
            slot: 1,
            shadow: Shadow {
                price: setup.start_price,
                dep: vec![0; n],
                wd: vec![0; n],
                pos: vec![0; n],
                mtm_raw: vec![0; n],
                b_share: vec![0; n],
                forgiven: vec![0; n],
                tol_ops: vec![0; n],
                fractional: vec![false; n],
                ..Shadow::default()
            },
            trace: Vec::new(),
            step_violations: Vec::new(),
            recovery: false,
            closed_payout: vec![None; n],
            provider_out_domain: [0; 2],
            resized_before_loss_settled: false,
        };
        for d in 0..2 {
            let amount = setup.provider[d];
            if amount > 0 {
                let mut m = MarketGroupV16ViewMut::new(&mut w.header, &mut w.markets);
                m.deposit_fresh_counterparty_backing_not_atomic(d, amount, setup.expiry)
                    .expect("provider deposit");
                w.shadow.provider_principal += amount;
            }
        }
        w.trace.push(format!("setup {:?}", setup));
        w
    }

    pub fn n(&self) -> usize {
        self.setup.accounts
    }

    pub fn price(&self) -> u64 {
        self.shadow.price
    }

    pub fn slot(&self) -> u64 {
        self.slot
    }

    pub fn defaults(&self) -> &[DefaultEvent] {
        &self.shadow.defaults
    }

    pub fn shadow_position_units(&self, i: usize) -> f64 {
        self.shadow.pos[i] as f64 / PS as f64
    }

    pub fn shadow_equity(&self, i: usize) -> i128 {
        self.shadow.equity(i)
    }

    fn snap(&self) -> Snap {
        (self.header, self.markets.clone(), self.accts.clone())
    }

    fn restore(&mut self, s: Snap) {
        self.header = s.0;
        self.markets = s.1;
        self.accts = s.2;
    }

    fn mode(&self) -> MarketModeV16 {
        match self.header.mode {
            0 => MarketModeV16::Live,
            1 => MarketModeV16::Resolved,
            _ => MarketModeV16::Recovery,
        }
    }

    // ----- engine reads (token flows and engine-chosen quantities only) -----

    /// Engine effective signed position of account i in raw q.
    pub fn engine_position(&self, i: usize) -> i128 {
        let asset = self.markets[0].engine.asset.try_to_runtime().unwrap();
        for leg in self.accts[i].legs.iter() {
            let Ok(leg) = leg.try_to_runtime() else { continue };
            if !leg.active || leg.asset_index != 0 {
                continue;
            }
            let (a, epoch) = match leg.side {
                SideV16::Long => (asset.a_long, asset.epoch_long),
                SideV16::Short => (asset.a_short, asset.epoch_short),
            };
            if leg.epoch_snap != epoch || leg.a_basis == 0 {
                return 0;
            }
            let num = leg.basis_pos_q.unsigned_abs() * a;
            let eff = num.div_ceil(leg.a_basis) as i128;
            return match leg.side {
                SideV16::Long => eff,
                SideV16::Short => -eff,
            };
        }
        0
    }

    /// Account i has a booked loss it has not settled yet.
    fn loss_unsettled(&self, i: usize) -> bool {
        let Ok(asset) = self.markets[0].engine.asset.try_to_runtime() else {
            return false;
        };
        let leg = self.accts[i]
            .legs
            .iter()
            .filter_map(|l| l.try_to_runtime().ok())
            .find(|l| l.active && l.asset_index == 0);
        leg.is_some_and(|l| {
            let (b, epoch) = match l.side {
                SideV16::Long => (asset.b_long_num, asset.epoch_long),
                SideV16::Short => (asset.b_short_num, asset.epoch_short),
            };
            l.b_epoch_snap == epoch && l.b_snap != b
        })
    }

    fn bucket_fresh_atoms(&self, d: usize) -> u128 {
        let e = &self.markets[0].engine;
        let b = if d == 0 { e.backing_long } else { e.backing_short };
        let b = b.try_to_runtime().unwrap();
        if b.status == BackingBucketStatusV16::Fresh {
            b.fresh_unliened_backing_num / BOUND_SCALE
        } else {
            0
        }
    }

    fn domain_claims_atoms_ceil(&self, d: usize) -> u128 {
        let e = &self.markets[0].engine;
        let s = if d == 0 { e.source_credit_long } else { e.source_credit_short };
        let s = s.try_to_runtime().unwrap();
        s.positive_claim_bound_num.div_ceil(BOUND_SCALE)
    }

    /// (claims, fresh reserved backing, liened backing, credit rate) of a domain.
    pub fn domain_state(&self, d: usize) -> (u128, u128, u128, u128) {
        let e = &self.markets[0].engine;
        let s = if d == 0 { e.source_credit_long } else { e.source_credit_short };
        let s = s.try_to_runtime().unwrap();
        (
            s.positive_claim_bound_num / BOUND_SCALE,
            s.fresh_reserved_backing_num / BOUND_SCALE,
            s.valid_liened_backing_num / BOUND_SCALE,
            s.credit_rate_num,
        )
    }

    pub fn account_capital_pnl(&self, i: usize) -> (u128, i128) {
        (self.accts[i].capital.get(), self.accts[i].pnl.get())
    }

    // ----- shadow bookkeeping -----

    fn shadow_price_move(&mut self, new_price: u64) {
        let dp = new_price as i128 - self.shadow.price as i128;
        for i in 0..self.n() {
            self.shadow.mtm_raw[i] += self.shadow.pos[i] * dp;
            if self.shadow.fractional[i] && self.shadow.pos[i] != 0 && dp != 0 {
                self.shadow.tol_ops[i] += 1;
            }
        }
        self.shadow.price = new_price;
    }

    /// Record a liquidation of `victim` that closed `closed_q` raw q.
    fn shadow_liquidation(&mut self, step: usize, victim: usize, closed_q: u128, booked: u128) {
        self.shadow.liquidations += 1;
        let side = self.shadow.pos[victim].signum();
        if side == 0 || closed_q == 0 {
            self.trace
                .push(format!("  shadow: liquidation of flat/zero (closed_q={closed_q})"));
            return;
        }
        let eq = self.shadow.equity(victim);
        let opp: Vec<usize> = (0..self.n())
            .filter(|&j| j != victim && self.shadow.pos[j].signum() == -side)
            .collect();
        let total: u128 = opp.iter().map(|&j| self.shadow.pos[j].unsigned_abs()).sum();
        if eq < 0 {
            let shortfall = eq.unsigned_abs();
            self.shadow.forgiven[victim] += shortfall;
            let mut shares = Vec::new();
            for &j in &opp {
                let w = self.shadow.pos[j].unsigned_abs();
                let share = if total == 0 { 0 } else { shortfall * w / total };
                self.shadow.b_share[j] += share;
                self.shadow.tol_ops[j] += 1;
                shares.push((j, w, share));
            }
            self.trace.push(format!(
                "  shadow: DEFAULT victim={victim} shortfall={shortfall} engine_booked={booked} shares={shares:?}"
            ));
            self.shadow.defaults.push(DefaultEvent {
                step,
                victim,
                shortfall,
                engine_booked: booked,
                shares,
                terminal: false,
            });
        } else if booked > 0 {
            self.trace.push(format!(
                "  shadow: engine booked {booked} but shadow equity is {eq} (no default)"
            ));
        }
        // Close the victim and ADL the opposite side pro rata.
        let closed = (closed_q as i128).min(self.shadow.pos[victim].abs());
        self.shadow.pos[victim] -= side * closed;
        if total > 0 {
            let keep = total.saturating_sub(closed as u128);
            for &j in &opp {
                let p = self.shadow.pos[j];
                let scaled = (p.unsigned_abs() * keep / total) as i128;
                if scaled * total as i128 != p.abs() * keep as i128 {
                    self.shadow.fractional[j] = true;
                }
                self.shadow.pos[j] = p.signum() * scaled;
                self.shadow.tol_ops[j] += 1;
            }
        }
        if self.shadow.pos[victim] % PS != 0 {
            self.shadow.fractional[victim] = true;
        }
        self.shadow.tol_ops[victim] += 1;
    }

    // ----- per-step checks -----

    fn step_checks(&mut self, step: usize) {
        let m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
        if let Err(e) = m.validate_shape() {
            self.step_violations.push(Violation {
                invariant: Invariant::StepSolvency,
                detail: format!("step {step}: validate_shape failed: {e:?}"),
            });
        }
        let h = &m.header;
        let senior = h.c_tot.get()
            + h.insurance.get()
            + h.backing_provider_earnings_total.get()
            + h.source_fresh_backing_total_num.get() / BOUND_SCALE;
        if h.vault.get() < senior {
            self.step_violations.push(Violation {
                invariant: Invariant::StepSolvency,
                detail: format!(
                    "step {step}: vault {} < c_tot {} + insurance {} + earnings {} + fresh backing {}",
                    h.vault.get(),
                    h.c_tot.get(),
                    h.insurance.get(),
                    h.backing_provider_earnings_total.get(),
                    h.source_fresh_backing_total_num.get() / BOUND_SCALE
                ),
            });
        }
        if self.mode() == MarketModeV16::Live {
            for i in 0..self.n() {
                let e = self.engine_position(i);
                let s = self.shadow.pos[i];
                // Engine rounds ADL'd quantities up per leg; allow a few raw q per ADL op.
                let slack = 2 * (1 + self.shadow.tol_ops[i]) as i128;
                if (e - s).abs() > slack {
                    self.step_violations.push(Violation {
                        invariant: Invariant::PositionModel,
                        detail: format!(
                            "step {step}: account {i} engine position {e} raw q != shadow {s} (slack {slack})"
                        ),
                    });
                }
            }
        }
    }

    // ----- live actions -----

    fn obs(&self) -> [AutoCrankObservationV16; 1] {
        [AutoCrankObservationV16 {
            asset_index: 0,
            effective_price: self.shadow.price,
            funding_rate_e9: 0,
        }]
    }

    /// One auto-crank step on account i, rolled back on error. Returns the plan.
    fn crank_step(&mut self, step: usize, i: usize) -> Result<AutoCrankPlanV16, V16Error> {
        let snap = self.snap();
        let before_q = self.engine_position(i);
        let obs = self.obs();
        let res = {
            let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
            let mut a = PortfolioV16ViewMut::new(&mut self.accts[i]);
            m.permissionless_auto_crank_not_atomic(
                &mut a,
                AutoCrankWorkV16 {
                    now_slot: self.slot,
                    observations: &obs,
                    resolved_close_fee_rate_per_slot: 0,
                },
            )
        };
        match res {
            Ok(r) => {
                if let AutoCrankPlanV16::Liquidate { .. } = r.selected {
                    let after_q = self.engine_position(i);
                    let closed = before_q.abs().saturating_sub(after_q.abs()) as u128;
                    let booked = match r.outcome {
                        percolator::AutoCrankOutcomeV16::Progressed(
                            percolator::PermissionlessProgressOutcomeV16::ResidualBooked(b),
                        ) => b.booked_loss + b.explicit_loss,
                        _ => 0,
                    };
                    self.trace.push(format!(
                        "  crank acct {i}: Liquidate closed_q={closed} outcome={:?}",
                        r.outcome
                    ));
                    self.shadow_liquidation(step, i, closed, booked);
                }
                if let percolator::AutoCrankOutcomeV16::ResolvedClose(
                    ResolvedCloseOutcomeV16::Closed { payout },
                ) = r.outcome
                {
                    // A crank can close an account and later pay a receipt top-up
                    // through another close; every payout counts.
                    self.closed_payout[i] = Some(self.closed_payout[i].unwrap_or(0) + payout);
                    self.trace
                        .push(format!("  crank acct {i}: resolved close, payout {payout}"));
                }
                if self.mode() == MarketModeV16::Recovery {
                    self.recovery = true;
                }
                Ok(r.selected)
            }
            Err(e) => {
                self.restore(snap);
                Err(e)
            }
        }
    }

    fn crank_fixpoint(&mut self, step: usize, i: usize) -> StepResult {
        let mut plans = Vec::new();
        for _ in 0..32 {
            match self.crank_step(step, i) {
                Ok(AutoCrankPlanV16::NoAction) => {
                    self.trace.push(format!("  crank acct {i}: {plans:?}"));
                    return StepResult::Applied;
                }
                Ok(p) => plans.push(p),
                Err(e) => {
                    self.trace
                        .push(format!("  crank acct {i}: {plans:?} then error {e:?}"));
                    return if plans.is_empty() {
                        StepResult::Rejected(e)
                    } else {
                        StepResult::Applied
                    };
                }
            }
        }
        self.trace
            .push(format!("  crank acct {i}: no fixpoint after 32 steps {plans:?}"));
        StepResult::Applied
    }

    fn accrue(&mut self, new_price: u64) -> Result<(), V16Error> {
        let slot = self.slot + 1;
        let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
        m.set_asset_raw_oracle_target_not_atomic(0, new_price)?;
        m.accrue_asset_to_not_atomic(0, slot, new_price, 0, true)?;
        Ok(())
    }

    fn liquidate(&mut self, step: usize, i: usize) -> StepResult {
        let snap = self.snap();
        let res = {
            let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
            let mut a = PortfolioV16ViewMut::new(&mut self.accts[i]);
            m.liquidate_account_not_atomic(&mut a, LiquidationRequestV16 { asset_index: 0 })
        };
        match res {
            Ok(o) => {
                self.trace.push(format!("  liquidate acct {i}: {o:?}"));
                self.shadow_liquidation(step, i, o.closed_q, o.residual_booked + o.explicit_loss);
                if self.mode() == MarketModeV16::Recovery {
                    self.recovery = true;
                }
                StepResult::Applied
            }
            Err(e) => {
                self.restore(snap);
                if self.mode() == MarketModeV16::Recovery {
                    self.recovery = true;
                }
                StepResult::Rejected(e)
            }
        }
    }

    /// Apply one live action. Every engine error rolls the whole instruction back,
    /// as the SVM would.
    pub fn apply(&mut self, step: usize, action: Action) -> StepResult {
        if self.mode() != MarketModeV16::Live {
            self.trace.push(format!("#{step} {action:?} -> skipped (not live)"));
            return StepResult::Skipped("market not live");
        }
        let result = self.apply_inner(step, action);
        if self.mode() == MarketModeV16::Recovery {
            self.recovery = true;
        }
        self.trace.push(format!("#{step} {action:?} -> {result:?}"));
        self.step_checks(step);
        result
    }

    fn apply_inner(&mut self, step: usize, action: Action) -> StepResult {
        let n = self.n();
        let result = match action {
            Action::Deposit { acct, amount } => {
                let i = acct % n;
                let snap = self.snap();
                let r = {
                    let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                    let mut a = PortfolioV16ViewMut::new(&mut self.accts[i]);
                    m.deposit_not_atomic(&mut a, amount)
                };
                match r {
                    Ok(()) => {
                        self.shadow.dep[i] += amount;
                        StepResult::Applied
                    }
                    Err(e) => {
                        self.restore(snap);
                        StepResult::Rejected(e)
                    }
                }
            }
            Action::Withdraw { acct, amount } => {
                let i = acct % n;
                let snap = self.snap();
                let r = {
                    let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                    let mut a = PortfolioV16ViewMut::new(&mut self.accts[i]);
                    m.withdraw_not_atomic(&mut a, amount)
                };
                match r {
                    Ok(()) => {
                        self.shadow.wd[i] += amount;
                        StepResult::Applied
                    }
                    Err(e) => {
                        self.restore(snap);
                        StepResult::Rejected(e)
                    }
                }
            }
            Action::Trade {
                buyer,
                seller,
                units,
            } => {
                let (b, s) = (buyer % n, seller % n);
                if b == s || units == 0 {
                    StepResult::Skipped("self trade / zero")
                } else {
                    let snap = self.snap();
                    let price = self.shadow.price;
                    let q = units as i128 * PS;
                    let (pos_b, pos_s) = (self.shadow.pos[b], self.shadow.pos[s]);
                    let same_side = |pos: i128, next: i128| pos != 0 && next != 0 && pos.signum() == next.signum();
                    let skips_loss = (same_side(pos_b, pos_b + q) && self.loss_unsettled(b))
                        || (same_side(pos_s, pos_s - q) && self.loss_unsettled(s));
                    let r = {
                        let mut m =
                            MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                        let (lo, hi) = self.accts.split_at_mut(b.max(s));
                        let (bb, ss) = if b < s {
                            (&mut lo[b], &mut hi[0])
                        } else {
                            (&mut hi[0], &mut lo[s])
                        };
                        let mut ba = PortfolioV16ViewMut::new(bb);
                        let mut sa = PortfolioV16ViewMut::new(ss);
                        m.execute_trade_with_fee_loss_stale_scoped_not_atomic(
                            &mut ba,
                            &mut sa,
                            TradeRequestV16 {
                                asset_index: 0,
                                size_q: units as i128 * PS,
                                exec_price: price,
                                fee_bps: 0,
                            },
                        )
                    };
                    match r {
                        Ok(_) => {
                            self.resized_before_loss_settled |= skips_loss;
                            self.shadow.pos[b] += q;
                            self.shadow.pos[s] -= q;
                            StepResult::Applied
                        }
                        Err(e) => {
                            self.restore(snap);
                            StepResult::Rejected(e)
                        }
                    }
                }
            }
            Action::Price { bps } => {
                let cap = self.setup.price_cap_bps as i64;
                let bps = (bps as i64).clamp(-cap, cap);
                let old = self.shadow.price as i64;
                // Round the step toward zero so a clamped move never exceeds the cap.
                let step = old * bps.abs() / 10_000;
                let new = (old + bps.signum() * step).max(1) as u64;
                let snap = self.snap();
                match self.accrue(new) {
                    Ok(()) => {
                        self.slot += 1;
                        self.shadow_price_move(new);
                        self.trace.push(format!("  price {old} -> {new} @slot {}", self.slot));
                        StepResult::Applied
                    }
                    Err(e) => {
                        self.restore(snap);
                        StepResult::Rejected(e)
                    }
                }
            }
            Action::Jump { up, slots } => {
                let cap = self.setup.price_cap_bps as i32;
                let mut moved = 0;
                let mut last = StepResult::Skipped("zero slots");
                for _ in 0..slots {
                    match self.apply_inner(step, Action::Price { bps: if up { cap } else { -cap } }) {
                        StepResult::Applied => {
                            moved += 1;
                            last = StepResult::Applied;
                        }
                        other => {
                            last = other;
                            break;
                        }
                    }
                }
                if moved > 0 {
                    StepResult::Applied
                } else {
                    last
                }
            }
            Action::PriceTo { price } => {
                let old = self.shadow.price;
                let snap = self.snap();
                match self.accrue(price.max(1)) {
                    Ok(()) => {
                        self.slot += 1;
                        self.shadow_price_move(price.max(1));
                        self.trace.push(format!("  price {old} -> {price} @slot {}", self.slot));
                        StepResult::Applied
                    }
                    Err(e) => {
                        self.restore(snap);
                        StepResult::Rejected(e)
                    }
                }
            }
            Action::Advance => {
                let snap = self.snap();
                match self.accrue(self.shadow.price) {
                    Ok(()) => {
                        self.slot += 1;
                        StepResult::Applied
                    }
                    Err(e) => {
                        self.restore(snap);
                        StepResult::Rejected(e)
                    }
                }
            }
            Action::Crank { acct } => self.crank_fixpoint(step, acct % n),
            Action::CrankOnce { acct } => match self.crank_step(step, acct % n) {
                Ok(p) => {
                    self.trace.push(format!("  crank-once acct {}: {p:?}", acct % n));
                    StepResult::Applied
                }
                Err(e) => StepResult::Rejected(e),
            },
            Action::CrankWorst => {
                let Some(i) = (0..n).filter(|&i| self.shadow.pos[i] != 0).min_by_key(|&i| self.shadow.equity(i)) else {
                    return StepResult::Skipped("no open account");
                };
                match self.crank_step(step, i) {
                    Ok(p) => {
                        self.trace.push(format!("  crank-worst acct {i}: {p:?}"));
                        StepResult::Applied
                    }
                    Err(e) => StepResult::Rejected(e),
                }
            }
            Action::Refresh { acct } => {
                let i = acct % n;
                let snap = self.snap();
                let r = {
                    let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                    let mut a = PortfolioV16ViewMut::new(&mut self.accts[i]);
                    m.full_account_refresh_not_atomic(&mut a)
                };
                match r {
                    Ok(_) => StepResult::Applied,
                    Err(e) => {
                        self.restore(snap);
                        StepResult::Rejected(e)
                    }
                }
            }
            Action::Liquidate { acct } => self.liquidate(step, acct % n),
            Action::Convert { acct } => {
                let i = acct % n;
                let snap = self.snap();
                let r = {
                    let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                    let mut a = PortfolioV16ViewMut::new(&mut self.accts[i]);
                    m.convert_released_pnl_to_capital_not_atomic(&mut a)
                };
                match r {
                    Ok(_) => StepResult::Applied,
                    Err(e) => {
                        self.restore(snap);
                        StepResult::Rejected(e)
                    }
                }
            }
            Action::ProviderWithdraw { domain } if self.setup.provider[domain % 2] == 0 => {
                StepResult::Skipped("provider has no bucket in this domain")
            }
            Action::ProviderWithdraw { domain } => {
                let d = domain % 2;
                let free = self
                    .bucket_fresh_atoms(d)
                    .saturating_sub(self.domain_claims_atoms_ceil(d));
                if free == 0 {
                    StepResult::Skipped("no free backing")
                } else {
                    let snap = self.snap();
                    let r = {
                        let mut m =
                            MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                        m.withdraw_fresh_counterparty_backing_not_atomic(d, free)
                    };
                    match r {
                        Ok(()) => {
                            self.shadow.provider_out += free;
                            self.provider_out_domain[d] += free;
                            self.trace.push(format!("  provider withdrew {free} from domain {d}"));
                            StepResult::Applied
                        }
                        Err(e) => {
                            self.restore(snap);
                            StepResult::Rejected(e)
                        }
                    }
                }
            }
        };
        result
    }

    // ----- terminal phase -----

    /// Resolve, close every account, let the provider withdraw, advance the slab.
    pub fn finish(mut self, actions: &[Action]) -> Report {
        let mut liveness: Vec<String> = Vec::new();
        let mut stuck_unbacked: Vec<String> = Vec::new();
        let step0 = actions.len();
        // Bring every account current and sweep liquidations so nobody resolves underwater.
        if self.mode() == MarketModeV16::Live {
            for round in 0..8 {
                let before = self.shadow.liquidations;
                for i in 0..self.n() {
                    self.crank_fixpoint(step0 + round, i);
                }
                for i in 0..self.n() {
                    if self.engine_position(i) != 0 && self.mode() == MarketModeV16::Live {
                        let _ = self.liquidate(step0 + round, i);
                    }
                }
                if self.shadow.liquidations == before || self.mode() != MarketModeV16::Live {
                    break;
                }
            }
            for i in 0..self.n() {
                self.crank_fixpoint(step0 + 8, i);
            }
            self.step_checks(step0 + 8);
        }
        if self.mode() == MarketModeV16::Recovery {
            self.recovery = true;
        }
        // Anyone the sweep could not liquidate and who is still underwater defaults at
        // resolution.
        for i in 0..self.n() {
            let eq = self.shadow.equity(i);
            if eq < 0 {
                let shortfall = eq.unsigned_abs();
                self.shadow.forgiven[i] += shortfall;
                let dbg = self.debug_account(i);
                self.trace.push(format!(
                    "  shadow: TERMINAL DEFAULT acct {i} shortfall {shortfall} ({dbg})"
                ));
                self.shadow.defaults.push(DefaultEvent {
                    step: step0 + 9,
                    victim: i,
                    shortfall,
                    engine_booked: 0,
                    shares: Vec::new(),
                    terminal: true,
                });
            }
        }
        if self.mode() != MarketModeV16::Resolved {
            let resolve_slot = self.slot + 1;
            let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
            if let Err(e) = m.resolve_market_not_atomic(resolve_slot) {
                liveness.push(format!("resolve_market failed: {e:?}"));
            }
            self.slot = resolve_slot;
            self.trace.push(format!("resolve @slot {resolve_slot}"));
        } else {
            self.trace.push("already resolved (recovery)".into());
        }

        let n = self.n();
        let mut last_err: Vec<Option<V16Error>> = vec![None; n];
        let mut topups = vec![0u128; n];
        let provider_live_out = self.shadow.provider_out;
        let mut provider_terminal = 0u128;
        // Phase A: close everyone who can close now; the provider takes back backing
        // that no longer stands behind a claim.
        self.close_rounds(&mut last_err);
        self.topup_sweep(&mut topups);
        provider_terminal += self.provider_sweep(false, &mut liveness);
        self.close_rounds(&mut last_err);
        self.topup_sweep(&mut topups);
        // Phase B: accounts whose claims wait on backing expiry (zero-rate source face
        // becomes junior only once its bucket lapses). Let the clock pass expiry.
        if self.closed_payout.iter().any(Option::is_none) {
            let slot = self.setup.expiry + 1;
            self.trace.push(format!(
                "terminal: {} account(s) still open; advancing to slot {slot} and expiring backing",
                self.closed_payout.iter().filter(|p| p.is_none()).count()
            ));
            {
                let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                let _ = m.advance_resolved_slot_not_atomic(slot);
            }
            for d in 0..2 {
                let snap = self.snap();
                let r = {
                    let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                    m.expire_source_backing_bucket_not_atomic(d, slot)
                };
                match r {
                    Ok(()) => self.trace.push(format!("terminal: expired domain {d}")),
                    Err(e) => {
                        self.restore(snap);
                        self.trace.push(format!("terminal: expire domain {d}: {e:?}"));
                    }
                }
            }
            self.close_rounds(&mut last_err);
            self.topup_sweep(&mut topups);
        }
        for i in 0..n {
            if self.closed_payout[i].is_none() {
                let unbacked = self.unbacked_source_domains(i);
                if unbacked.is_empty() {
                    liveness.push(format!(
                        "account {i} did not close (last error {:?})",
                        last_err[i]
                    ));
                } else {
                    stuck_unbacked.push(format!(
                        "account {i} did not close (last error {:?}); source face in domain(s) {unbacked:?} has zero backing and no fresh bucket",
                        last_err[i]
                    ));
                }
            }
        }
        provider_terminal += self.provider_sweep(true, &mut liveness);
        self.topup_sweep(&mut topups);
        let mut retired = None;
        let mut start = 0usize;
        let terminal_slot = self.setup.expiry + 1;
        for _ in 0..32 {
            let r = {
                let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                m.advance_terminal_slab_not_atomic(terminal_slot, start)
            };
            match r {
                Ok(TerminalSlabOutcomeV16::ReadyToClose { retired: r }) => {
                    retired = Some(r);
                    break;
                }
                Ok(TerminalSlabOutcomeV16::ScanProgress { next_asset_index }) => {
                    start = next_asset_index
                }
                Ok(TerminalSlabOutcomeV16::BackingExpired { domain }) => {
                    self.trace
                        .push(format!("terminal: backing expired in domain {domain}"));
                    self.topup_sweep(&mut topups);
                    start = 0;
                }
                Ok(other) => {
                    self.trace.push(format!("terminal: {other:?}"));
                    start = 0;
                }
                Err(e) => {
                    liveness.push(format!("advance_terminal_slab failed: {e:?}"));
                    break;
                }
            }
        }
        if retired.is_none() && !liveness.iter().any(|l| l.contains("terminal_slab")) {
            liveness.push("advance_terminal_slab never reached ReadyToClose".into());
        }
        for i in 0..n {
            if topups[i] > 0 {
                self.trace.push(format!("terminal: account {i} topup {}", topups[i]));
            }
        }
        let accounts = (0..n)
            .map(|i| AccountReport {
                deposits: self.shadow.dep[i],
                withdrawals: self.shadow.wd[i],
                payout: self.closed_payout[i].unwrap_or(0) + topups[i],
                fair: self.shadow.fair_out(i),
                fair_before_losses: self.shadow.fair_out_before_losses(i),
                b_share: self.shadow.b_share[i],
                forgiven: self.shadow.forgiven[i],
                tol: self.shadow.tol(i),
            })
            .collect();
        Report {
            actions: actions.to_vec(),
            accounts,
            provider_principal: self.shadow.provider_principal,
            provider_out: provider_live_out + provider_terminal,
            insurance_end: self.header.insurance.get(),
            retired: retired.unwrap_or(0),
            vault_end: self.header.vault.get(),
            defaults: self.shadow.defaults.clone(),
            recovery: self.recovery,
            liveness,
            stuck_unbacked,
            step_violations: self.step_violations,
            trace: self.trace,
            resized_before_loss_settled: self.resized_before_loss_settled,
        }
    }

    fn close_rounds(&mut self, last_err: &mut [Option<V16Error>]) {
        for _ in 0..64 {
            let mut progressed = false;
            for i in 0..self.n() {
                if self.closed_payout[i].is_some() {
                    continue;
                }
                let snap = self.snap();
                let r = {
                    let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                    let mut a = PortfolioV16ViewMut::new(&mut self.accts[i]);
                    m.close_resolved_account_not_atomic(&mut a, 0)
                };
                match r {
                    Ok(ResolvedCloseOutcomeV16::Closed { payout: p }) => {
                        self.closed_payout[i] = Some(p);
                        progressed = true;
                        self.trace.push(format!("terminal: account {i} closed, payout {p}"));
                    }
                    Ok(ResolvedCloseOutcomeV16::ProgressOnly) => progressed = true,
                    Err(e) => {
                        self.restore(snap);
                        last_err[i] = Some(e);
                    }
                }
            }
            if self.closed_payout.iter().all(Option::is_some) || !progressed {
                break;
            }
        }
    }

    /// Provider withdraws from each funded domain: only backing not standing behind a
    /// claim (`all == false`), or everything still fresh (`all == true`).
    fn provider_sweep(&mut self, all: bool, liveness: &mut Vec<String>) -> u128 {
        let mut got = 0;
        for d in 0..2 {
            // Only a domain the provider funded has an owner who can withdraw; loser-paid
            // backing left in an unfunded domain expires into the junior residual.
            if self.setup.provider[d] == 0 {
                continue;
            }
            let fresh = self.bucket_fresh_atoms(d);
            let mut amount = if all {
                fresh
            } else {
                fresh.saturating_sub(self.domain_claims_atoms_ceil(d))
            };
            // Engine #249: in Resolved mode a provider may only take the surplus above the
            // reserve that still pays underpaid winners (the junior payout pool). That
            // reserve is forfeited to the junior residual at expiry, by design; the
            // fairness invariants judge whether the provider bore more than it should.
            if self.mode() == MarketModeV16::Resolved {
                let surplus = {
                    let m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                    m.resolved_counterparty_backing_surplus(d)
                };
                let surplus = match surplus {
                    Ok(s) => s,
                    // LockActive: the junior pool is not settled yet, so nothing is
                    // withdrawable now; the end-of-run provider checks still judge the
                    // final recovery. Any other error is unexpected.
                    Err(V16Error::LockActive) => {
                        self.trace.push(format!("terminal: domain {d} resolved surplus locked"));
                        0
                    }
                    Err(e) => {
                        liveness.push(format!("resolved surplus read failed in domain {d}: {e:?}"));
                        0
                    }
                };
                if surplus < amount {
                    let reserve = amount - surplus;
                    self.trace.push(format!(
                        "terminal: domain {d} resolved surplus {surplus} < requested {amount}; reserve {reserve} kept for junior payouts"
                    ));
                    amount = surplus;
                }
            }
            if amount == 0 {
                continue;
            }
            let snap = self.snap();
            let r = {
                let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                m.withdraw_fresh_counterparty_backing_not_atomic(d, amount)
            };
            match r {
                Ok(()) => {
                    got += amount;
                    self.provider_out_domain[d] += amount;
                    self.trace
                        .push(format!("terminal: provider withdrew {amount} from domain {d}"));
                }
                Err(e) => {
                    self.restore(snap);
                    self.trace.push(format!(
                        "terminal: provider could not withdraw {amount} from domain {d}: {e:?}"
                    ));
                    // Backing above the provider's own principal is loser-paid money the
                    // fairness invariants account for; only principal being stuck is a
                    // liveness failure.
                    let owed = self.setup.provider[d].saturating_sub(self.provider_out_domain[d]);
                    if all && owed > 0 {
                        liveness.push(format!(
                            "provider could not withdraw {amount} (principal still owed {owed}) from domain {d}: {e:?}"
                        ));
                    }
                }
            }
        }
        got
    }

    /// Source domains where account i holds face but the domain has zero available
    /// backing and no fresh bucket that could ever expire.
    fn unbacked_source_domains(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        for src in self.accts[i].source_domains.iter() {
            if src.source_claim_market_id.get() == 0 || src.source_claim_bound_num.get() == 0 {
                continue;
            }
            let d = src.domain.get() as usize;
            if d > 1 {
                continue;
            }
            let e = &self.markets[0].engine;
            let bucket = if d == 0 { e.backing_long } else { e.backing_short };
            let bucket = bucket.try_to_runtime().unwrap();
            let (_, fresh, liened, rate) = self.domain_state(d);
            if bucket.status != BackingBucketStatusV16::Fresh && rate == 0 && fresh <= liened {
                out.push(d);
            }
        }
        out
    }

    fn topup_sweep(&mut self, topups: &mut [u128]) {
        for i in 0..self.n() {
            let receipt = self.accts[i].resolved_payout_receipt.try_to_runtime();
            let Ok(receipt) = receipt else { continue };
            if !receipt.present || receipt.finalized {
                continue;
            }
            let snap = self.snap();
            let r = {
                let mut m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
                let mut a = PortfolioV16ViewMut::new(&mut self.accts[i]);
                m.claim_resolved_payout_topup_not_atomic(&mut a)
            };
            match r {
                Ok(p) => topups[i] += p,
                Err(_) => self.restore(snap),
            }
        }
    }

    /// Raw engine state, for debugging tools only.
    pub fn engine_parts(
        &mut self,
    ) -> (
        &mut MarketGroupV16HeaderAccount,
        &mut Vec<Market<u64>>,
        &mut Vec<PortfolioAccountV16Account>,
    ) {
        (&mut self.header, &mut self.markets, &mut self.accts)
    }

    /// Engine view of an account (for debugging replays).
    pub fn debug_account(&mut self, i: usize) -> String {
        let m = MarketGroupV16ViewMut::new(&mut self.header, &mut self.markets);
        let ok = PortfolioV16View::new(&self.accts[i]).validate_with_market(&m.as_view());
        format!(
            "acct {i}: capital {} pnl {} engine_pos {} shadow_pos {} shadow_equity {} valid {:?}",
            self.accts[i].capital.get(),
            self.accts[i].pnl.get(),
            self.engine_position(i),
            self.shadow.pos[i],
            self.shadow.equity(i),
            ok
        )
    }
}

#[derive(Clone, Debug)]
pub struct AccountReport {
    pub deposits: u128,
    pub withdrawals: u128,
    /// Resolved close payout plus any later top-up.
    pub payout: u128,
    /// Shadow fair total out (withdrawals + payout), after bankruptcy shares.
    pub fair: i128,
    pub fair_before_losses: i128,
    pub b_share: u128,
    pub forgiven: u128,
    pub tol: u128,
}

impl AccountReport {
    pub fn out(&self) -> u128 {
        self.withdrawals + self.payout
    }
}

/// Everything a run produced, with the terms each invariant needs.
#[derive(Clone, Debug)]
pub struct Report {
    pub actions: Vec<Action>,
    pub accounts: Vec<AccountReport>,
    pub provider_principal: u128,
    pub provider_out: u128,
    pub insurance_end: u128,
    pub retired: u128,
    pub vault_end: u128,
    pub defaults: Vec<DefaultEvent>,
    pub recovery: bool,
    pub liveness: Vec<String>,
    /// Stuck accounts matching the `StuckUnbackedClaim` signature.
    pub stuck_unbacked: Vec<String>,
    pub step_violations: Vec<Violation>,
    pub trace: Vec<String>,
    /// A same-side resize ran with a booked loss unsettled (#253).
    pub resized_before_loss_settled: bool,
}

impl Report {
    pub fn total_tol(&self) -> u128 {
        self.accounts.iter().map(|a| a.tol).sum()
    }

    pub fn had_default(&self) -> bool {
        !self.defaults.is_empty()
    }

    /// Evaluate every invariant. Each violation names its terms.
    pub fn check(&self) -> Vec<Violation> {
        let mut v = self.step_violations.clone();
        let tol = self.total_tol();
        let deposits: u128 = self.accounts.iter().map(|a| a.deposits).sum();
        let outs: u128 = self.accounts.iter().map(|a| a.out()).sum();
        let lhs = outs + self.provider_out + self.insurance_end + self.retired + self.vault_end;
        let rhs = deposits + self.provider_principal;
        if lhs != rhs {
            v.push(Violation {
                invariant: Invariant::Conservation,
                detail: format!(
                    "payouts+withdrawals {outs} + provider {} + insurance {} + retired {} + vault_end {} = {lhs} != deposits {deposits} + provider principal {} = {rhs}",
                    self.provider_out, self.insurance_end, self.retired, self.vault_end,
                    self.provider_principal
                ),
            });
        }
        // Provider/slab failures that follow from stuck accounts belong to the same class.
        let stuck_accounts = self.liveness.iter().any(|l| l.contains("did not close"));
        if !self.stuck_unbacked.is_empty() && !stuck_accounts {
            let mut detail = self.stuck_unbacked.join(" | ");
            for l in &self.liveness {
                detail.push_str(&format!(" | then: {l}"));
            }
            v.push(Violation {
                invariant: Invariant::StuckUnbackedClaim,
                detail,
            });
        } else {
            for l in self.liveness.iter().chain(&self.stuck_unbacked) {
                v.push(Violation {
                    invariant: Invariant::TerminalLiveness,
                    detail: l.clone(),
                });
            }
        }
        if self.recovery {
            // Recovery mode is outside the shadow model: only conservation and
            // liveness are checked.
            return v;
        }
        if !self.had_default() {
            for (i, a) in self.accounts.iter().enumerate() {
                let diff = a.out() as i128 - a.fair;
                if diff.unsigned_abs() > a.tol {
                    v.push(Violation {
                        invariant: Invariant::NoDefaultPayout,
                        detail: format!(
                            "account {i}: paid {} (withdrawals {} + payout {}) != fair {} (deposits {} + mtm); diff {diff:+}, tol {}",
                            a.out(), a.withdrawals, a.payout, a.fair, a.deposits, a.tol
                        ),
                    });
                }
            }
            let stuck = self.retired + self.insurance_end + self.vault_end;
            if stuck > tol {
                v.push(Violation {
                    invariant: Invariant::NoDefaultRetired,
                    detail: format!(
                        "retired {} + insurance {} + vault_end {} = {stuck} > tol {tol}",
                        self.retired, self.insurance_end, self.vault_end
                    ),
                });
            }
            if self.provider_out + tol < self.provider_principal {
                v.push(Violation {
                    invariant: Invariant::ProviderPrincipal,
                    detail: format!(
                        "provider recovered {} of principal {} (short {}), tol {tol}",
                        self.provider_out,
                        self.provider_principal,
                        self.provider_principal - self.provider_out
                    ),
                });
            }
        } else {
            let shortfall: u128 = self.defaults.iter().map(|d| d.shortfall).sum();
            let expected_shares: u128 = self.accounts.iter().map(|a| a.b_share).sum();
            // What everyone lost relative to mark-to-market value (victims' fair value
            // already includes the forgiven shortfall, so only winners contribute).
            let socialized: i128 = self
                .accounts
                .iter()
                .map(|a| a.fair_before_losses - a.out() as i128)
                .sum();
            if (socialized - shortfall as i128).unsigned_abs() > tol {
                v.push(Violation {
                    invariant: Invariant::LossShareTotal,
                    detail: format!(
                        "shadow shortfall {shortfall} (expected B shares {expected_shares}) but accounts lost {socialized} vs mark-to-market; provider {} of {}, retired {}, insurance {}, vault_end {}; tol {tol}",
                        self.provider_out, self.provider_principal, self.retired,
                        self.insurance_end, self.vault_end
                    ),
                });
            }
            // A default's shortfall is borne by winners through B, never by the provider:
            // with no fees its fair recovery is still its full principal. Terminal
            // defaults (resolved-mode split) are outside the model, as for inv3 below.
            let terminal = self.defaults.iter().any(|d| d.terminal);
            // Terminal defaults: since engine #249 the resolved reserve may pay winners
            // left short, but never more than the total shortfall.
            if terminal && self.provider_out + shortfall + tol < self.provider_principal {
                v.push(Violation {
                    invariant: Invariant::ProviderPrincipal,
                    detail: format!(
                        "terminal default: provider recovered {} of principal {}, more than the shortfall {shortfall} withheld, tol {tol}",
                        self.provider_out, self.provider_principal
                    ),
                });
            }
            if !terminal && self.provider_out + tol < self.provider_principal {
                v.push(Violation {
                    invariant: Invariant::ProviderPrincipal,
                    detail: format!(
                        "after default: provider recovered {} of principal {} (short {}), shadow shortfall {shortfall}, tol {tol}",
                        self.provider_out,
                        self.provider_principal,
                        self.provider_principal - self.provider_out
                    ),
                });
            }
            if !terminal {
                for (i, a) in self.accounts.iter().enumerate() {
                    let diff = a.out() as i128 - a.fair;
                    if diff.unsigned_abs() > a.tol {
                        v.push(Violation {
                            invariant: Invariant::LossSharePerWinner,
                            detail: format!(
                                "account {i}: paid {} != fair {} (mtm value {} - expected share {}); bore {} instead; diff {diff:+}, tol {}",
                                a.out(), a.fair, a.fair_before_losses, a.b_share,
                                a.fair_before_losses - a.out() as i128, a.tol
                            ),
                        });
                    }
                }
            }
        }
        v
    }

    /// Money moved between traders (one winner paid for another), and the one known
    /// way to do that (#253) did not happen in this run.
    pub fn unexplained_transfer(&self, violations: &[Violation]) -> bool {
        let has = |inv| violations.iter().any(|x| x.invariant == inv);
        let floor = TRANSFER_ALARM_ATOMS as i128;
        // A leak out of the trader pool only leaves traders short, so with the
        // total wrong, only someone paid over their fair value is a transfer.
        let moved = self.accounts.iter().any(|a| {
            let diff = a.out() as i128 - a.fair;
            if has(Invariant::LossShareTotal) { diff > floor } else { diff.abs() > floor }
        });
        has(Invariant::LossSharePerWinner) && moved && !self.resized_before_loss_settled
    }

    /// Full failure text: invariant, terms, action sequence, trace.
    pub fn explain(&self, violations: &[Violation]) -> String {
        let mut s = String::new();
        for x in violations {
            s.push_str(&format!("VIOLATED {}: {}\n", x.invariant.label(), x.detail));
        }
        s.push_str(&format!("{self}"));
        s
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "--- terms ---")?;
        for (i, a) in self.accounts.iter().enumerate() {
            writeln!(
                f,
                "account {i}: deposits {} withdrawals {} payout {} out {} fair {} (pre-loss {}, b_share {}, forgiven {}) tol {}",
                a.deposits, a.withdrawals, a.payout, a.out(), a.fair, a.fair_before_losses,
                a.b_share, a.forgiven, a.tol
            )?;
        }
        writeln!(
            f,
            "provider principal {} recovered {}; insurance {}; retired {}; vault_end {}; defaults {}; recovery {}",
            self.provider_principal, self.provider_out, self.insurance_end, self.retired,
            self.vault_end, self.defaults.len(), self.recovery
        )?;
        for d in &self.defaults {
            writeln!(f, "default: {d:?}")?;
        }
        writeln!(f, "--- actions ---")?;
        writeln!(f, "{:?}", self.actions)?;
        writeln!(f, "--- trace ---")?;
        for t in &self.trace {
            writeln!(f, "{t}")?;
        }
        Ok(())
    }
}

/// Run a full scenario: setup, actions, terminal phase.
pub fn run(setup: Setup, actions: &[Action]) -> Report {
    let mut w = World::new(setup);
    for (step, &a) in actions.iter().enumerate() {
        w.apply(step, a);
    }
    w.finish(actions)
}

/// `PriceTo` steps from `from` to `to`, each within `cap_bps` of the previous mark.
pub fn price_path(from: u64, to: u64, cap_bps: u64) -> Vec<Action> {
    let mut out = Vec::new();
    let mut p = from;
    while p != to {
        let max_step = (p as u128 * cap_bps as u128 / 10_000) as u64;
        assert!(max_step > 0, "price {p} cannot move under a {cap_bps} bps cap");
        p = if to > p { (p + max_step).min(to) } else { p.saturating_sub(max_step).max(to) };
        out.push(Action::PriceTo { price: p });
    }
    out
}
