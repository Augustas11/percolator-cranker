//! Deterministic replays of the three open upstream issues, driven through the same
//! harness and invariant checker as the fuzzer, plus control cases from the issues
//! that must pass (no false positives).
//!
//! At the production pin each bug replay must show its invariant VIOLATED. When
//! upstream fixes an issue and we re-pin, flip that test's assertion to
//! `assert_clean` and rename it `upstream_NNN_..._is_fixed`.

use engine_fuzz::*;
use Action::*;

fn violated(r: &Report) -> Vec<Invariant> {
    let mut v: Vec<Invariant> = r.check().iter().map(|x| x.invariant).collect();
    v.sort();
    v.dedup();
    v
}

fn assert_clean(r: &Report) {
    let v = r.check();
    assert!(v.is_empty(), "expected no violations\n{}", r.explain(&v));
}

/// The replay must violate exactly the bug-class invariants listed, nothing else.
fn assert_violates(r: &Report, expected: &[Invariant]) {
    let got = violated(r);
    let v = r.check();
    assert_eq!(got, expected, "unexpected invariant set\n{}", r.explain(&v));
    for inv in &got {
        assert!(inv.is_known_bug_class(), "{} is not a known bug class", inv.label());
    }
    println!("{}", r.explain(&v));
}

// ---------------------------------------------------------------------------
// #250 case A: a lien keeps its face in the rate denominator. Engine prices are
// x100 so a 40 bps/slot cap can move them (10_000 -> 11_000 is the issue's 100 -> 110,
// and every amount is x100).
// Accounts: 0 = W1, 1 = W2, 2 = L.
// ---------------------------------------------------------------------------

fn case_250(w2_pyramids: bool) -> Report {
    let s = Setup::leveraged(3, [0, 0]);
    let mut acts = vec![
        Deposit { acct: 0, amount: 20_000 },
        Deposit { acct: 1, amount: 20_000 },
        Deposit { acct: 2, amount: 100_000 },
        Trade { buyer: 0, seller: 2, units: 10 },
        Trade { buyer: 1, seller: 2, units: 10 },
    ];
    acts.extend(price_path(10_000, 11_000, 40));
    acts.extend([Crank { acct: 0 }, Crank { acct: 1 }, Crank { acct: 2 }]);
    if w2_pyramids {
        // W2 adds 15 units using 7_500 of its 10_000 gain as initial margin (the lien).
        acts.push(Trade { buyer: 1, seller: 2, units: 15 });
    }
    // W1 goes flat and converts.
    acts.extend([Trade { buyer: 2, seller: 0, units: 10 }, Convert { acct: 0 }]);
    // W2 goes flat, its lien is released by the auto crank, it converts.
    let w2_units = if w2_pyramids { 25 } else { 10 };
    acts.extend([
        Trade { buyer: 2, seller: 1, units: w2_units },
        Crank { acct: 1 },
        Convert { acct: 1 },
    ]);
    run(s, &acts)
}

/// Fixed upstream by #260 (c7ae2256): the lien leaves the rate denominator.
#[test]
fn upstream_250_lien_rate_dilution_is_fixed() {
    let r = case_250(true);
    // W1 is owed 30_000 (20_000 + 10_000 gain) and is now paid in full; nothing is retired.
    assert_clean(&r);
    assert_eq!(r.accounts[0].out(), 30_000);
    assert_eq!(r.accounts[0].fair, 30_000);
    assert_eq!(r.retired, 0);
}

#[test]
fn upstream_250_baseline_without_lien_passes() {
    let r = case_250(false);
    assert_clean(&r);
    assert_eq!(r.accounts[0].out(), 30_000);
}

// ---------------------------------------------------------------------------
// #252: a K/F loss met by the account's own rate-1 face in the loss domain leaks
// the domain's backing to the junior pool; the provider recovers less.
// Accounts: 0 = A, 1 = B, 2 = C. Provider: 100_000 in domain 0.
// ---------------------------------------------------------------------------

fn case_252(reversal: bool) -> Report {
    let mut s = Setup::defaults(3, [100_000, 0]);
    s.price_cap_bps = 500;
    let mut acts = vec![
        Deposit { acct: 0, amount: 1_000_000 },
        Deposit { acct: 1, amount: 1_000_000 },
        Deposit { acct: 2, amount: 1_000_000 },
        Trade { buyer: 1, seller: 0, units: 1000 },
        PriceTo { price: 96 },
        Crank { acct: 0 },
        Crank { acct: 1 },
        Crank { acct: 2 },
    ];
    if reversal {
        // A buys 2_000 from C: A flips to long 1_000, C is short 2_000.
        acts.push(Trade { buyer: 0, seller: 2, units: 2000 });
    } else {
        // Control: A just closes and B adds the long instead.
        acts.push(Trade { buyer: 0, seller: 2, units: 1000 });
        acts.push(Trade { buyer: 1, seller: 2, units: 1000 });
    }
    acts.extend([
        PriceTo { price: 92 },
        Crank { acct: 0 },
        Crank { acct: 1 },
        Crank { acct: 2 },
        ProviderWithdraw { domain: 0 },
    ]);
    run(s, &acts)
}

/// Fixed upstream by #260 (6666a2ab): the K/F loss cancels against the account's own face.
#[test]
fn upstream_252_provider_leak_is_fixed() {
    let r = case_252(true);
    // The provider recovers its principal; nothing is retired.
    assert_clean(&r);
    assert_eq!(r.retired, 0);
    assert_eq!(r.provider_out, 100_000);
    let outs: Vec<u128> = r.accounts.iter().map(|a| a.out()).collect();
    assert_eq!(outs, vec![1_000_000, 992_000, 1_008_000]);
}

#[test]
fn upstream_252_control_passes() {
    let r = case_252(false);
    assert_clean(&r);
    assert_eq!(r.provider_out, 100_000);
    assert_eq!(r.retired, 0);
}

// ---------------------------------------------------------------------------
// #253: a same-side resize after a bankruptcy booking skips B settlement and
// pushes the winner's B share onto other winners. Engine defaults (100% margin).
// Accounts: 0 = L1 (attacker), 1 = L2 (honest), 2 = S (bankrupt), 3 = S2.
// ---------------------------------------------------------------------------

fn case_253(settle_b_first: bool) -> Report {
    let s = Setup::defaults(4, [0, 0]);
    let mut acts = vec![
        Deposit { acct: 0, amount: 1_000_000 },
        Deposit { acct: 1, amount: 1_000_000 },
        Deposit { acct: 2, amount: 100_000 },
        Deposit { acct: 3, amount: 10_000_000 },
        Trade { buyer: 0, seller: 2, units: 1000 },
        Trade { buyer: 1, seller: 3, units: 1000 },
        PriceTo { price: 200 },
        PriceTo { price: 300 },
        // All in one slot from here.
        CrankOnce { acct: 3 },
        CrankOnce { acct: 1 },
        CrankOnce { acct: 2 }, // S refresh: capital reserved as backing
        CrankOnce { acct: 0 }, // L1 refresh: cert current
        CrankOnce { acct: 2 }, // S refresh
        CrankOnce { acct: 2 }, // S liquidated: 100_000 residual booked into B_long
    ];
    if settle_b_first {
        // Control: L1 settles its B share before resizing.
        acts.push(Crank { acct: 0 });
    }
    // L1 sells (effective 500 - 1) units to the reducing short S2.
    acts.push(Trade { buyer: 3, seller: 0, units: 499 });
    acts.push(Crank { acct: 0 });
    run(s, &acts)
}

/// Fixed at engine b22bef7d (#253): both winners net +150_000 even when L1 resizes
/// before settling its B share. At 4db11a8c L1 netted +171_391 and L2 +128_608.
#[test]
fn upstream_253_b_skip_resize_is_fixed() {
    let r = case_253(false);
    assert_clean(&r);
    assert_eq!(r.accounts[0].out(), 1_150_000);
    assert_eq!(r.accounts[1].out(), 1_150_000);
    assert_eq!(r.accounts[0].fair, 1_150_000);
    assert_eq!(r.accounts[1].fair, 1_150_000);
}

/// The #253 run with its pre-fix payouts (L1 +21_391 over fair, L2 21_392 under)
/// and violation put back, so the alarm logic keeps a fixed fixture.
fn transfer_fixture() -> (Report, Vec<Violation>) {
    let mut r = case_253(false);
    r.accounts[0].fair -= 21_391;
    r.accounts[1].fair += 21_392;
    r.resized_before_loss_settled = true;
    let v = vec![Violation { invariant: Invariant::LossSharePerWinner, detail: String::new() }];
    (r, v)
}

/// A winner-to-winner transfer is silent only with a known cause label; without it
/// the alarm fires: this is how a new cause of winners paying for each other shows up.
#[test]
fn transfer_alarm_fires_when_no_known_cause() {
    let (mut r, v) = transfer_fixture();
    assert!(!r.unexplained_transfer(&v), "resize-before-settle is a known cause");
    r.resized_before_loss_settled = false;
    assert!(r.unexplained_transfer(&v));
}

/// With the total wrong, a shortfall alone is a leak; only an overpayment is a transfer.
#[test]
fn transfer_alarm_with_a_wrong_total_needs_an_overpayment() {
    let (mut r, mut v) = transfer_fixture();
    r.resized_before_loss_settled = false;
    v.push(Violation { invariant: Invariant::LossShareTotal, detail: String::new() });
    assert!(r.unexplained_transfer(&v), "account 0 is overpaid");
    r.accounts[0].fair = r.accounts[0].out() as i128;
    assert!(!r.unexplained_transfer(&v), "only a shortfall remains");
}

#[test]
fn upstream_253_control_b_settled_first_passes() {
    let r = case_253(true);
    assert_clean(&r);
    assert_eq!(r.defaults.len(), 1);
    assert_eq!(r.defaults[0].shortfall, 100_000);
}

// ---------------------------------------------------------------------------
// BY DESIGN, not a bug (verified against spec.md §2.2, the v16.9.1 header and
// "Distribution characterization"): the engine never nets value across source
// domains and a reserve is irreversible. Kept because it shows where our stricter
// "fair payout" and "provider keeps principal" checks go beyond the engine's
// promise; see README. Related upstream: #175.
// A loser pays into
// its loss domain before the winner has settled any claim there, then the price
// reverses before the winner settles. The loser's payment is stranded in the loss
// domain with no claim against it, while its later gain claim sits in the other
// domain backed only by the counterparty's smaller loss.
// - Provider funds domain 0 only: the provider sweeps the stranded payment and the
//   trader is paid short by the same amount.
// - Provider funds domain 1 only: the provider covers the gain, the stranded
//   payment expires and is retired, so the provider recovers less.
// Present at the pin and at upstream master 37cb14ee. Clean with no provider, with
// both domains funded, or when the winner is cranked before the reversal.
// Accounts: 0 = A (long), 1 = B (short), 2 = unused.
// ---------------------------------------------------------------------------

fn case_stale_winner_reversal(provider: [u128; 2], crank_winner_first: bool) -> Report {
    let mut s = Setup::leveraged(3, provider);
    s.price_cap_bps = 400;
    let mut acts = vec![
        Deposit { acct: 0, amount: 100_000 },
        Deposit { acct: 1, amount: 100_000 },
        Trade { buyer: 0, seller: 1, units: 10 },
        Price { bps: -20 },
        Crank { acct: 0 }, // A pays 200 into domain 0; B has not settled its gain
    ];
    if crank_winner_first {
        acts.push(Crank { acct: 1 });
    }
    acts.push(Price { bps: 40 }); // reversal: A is now up 190 net, B down 190
    run(s, &acts)
}

/// Fixed at engine b22bef7d: A is paid its fair 100_190. At 4db11a8c the provider
/// took 50_200 and A got 99_990.
#[test]
fn design_stale_winner_reversal_trader_gain_is_fixed() {
    let r = case_stale_winner_reversal([50_000, 0], false);
    assert_clean(&r);
    assert_eq!(r.accounts[0].out(), 100_190);
}

#[test]
fn design_stale_winner_reversal_provider_loses_principal() {
    let r = case_stale_winner_reversal([0, 50_000], false);
    assert_violates(&r, &[Invariant::NoDefaultRetired, Invariant::ProviderPrincipal]);
    assert_eq!(r.accounts[0].out(), 100_190);
    assert_eq!(r.provider_out, 49_800);
    assert_eq!(r.retired, 200);
}

#[test]
fn design_stale_winner_reversal_controls_pass() {
    assert_clean(&case_stale_winner_reversal([50_000, 0], true));
    // ([0, 50_000], true) is not a control at the pin: there B's later loss is met by
    // its own domain-0 face via support-and-burn (#457 case 2, fixed upstream by #226).
    assert_clean(&case_stale_winner_reversal([0, 0], false));
    assert_clean(&case_stale_winner_reversal([50_000, 50_000], false));
}

// ---------------------------------------------------------------------------
// REAL BUG (found by the random fuzzer; new trigger for upstream #172 site 2,
// with a solvent winner). Breaks spec.md v16.9.1 ~1355-1372: settlement order must
// not change an account's terminal value. Root cause: the B loss share is settled
// into pnl before K/F, so the K/F gain takes the negative-pnl branch of
// apply_signed_kf_delta_to_pnl, where support for a domain with no claims yet
// (positive_claim_bound_num == 0) is 0 and the whole gain is burned. A bankruptcy
// residual booked while no winner on the loss-bearing side has settled its gain yet.
// The first winner to settle takes its B share but its K/F gain never appears, and
// the bankrupt loser's paid-in capital is orphaned in the gain domain (expired and
// retired at the end). Winners are paid far less than gain minus their loss share.
// If any winner refreshes before the liquidation, every winner is paid correctly.
// Present at the pin and at upstream master 37cb14ee.
// Accounts: 0 = L (short, goes bankrupt), 1 = W (long winner).
// ---------------------------------------------------------------------------

fn case_bankruptcy_before_winner_refresh(refresh_winner_first: bool) -> Report {
    let mut s = Setup::leveraged(3, [0, 0]);
    s.price_cap_bps = 400;
    let mut acts = vec![
        Deposit { acct: 0, amount: 10_000 },
        Deposit { acct: 1, amount: 100_000 },
        Trade { buyer: 1, seller: 0, units: 9 },
        Jump { up: true, slots: 4 }, // +4 slots at the 4% cap: L is underwater
    ];
    if refresh_winner_first {
        acts.push(Refresh { acct: 1 });
    }
    acts.extend([Crank { acct: 0 }, Crank { acct: 1 }]); // L liquidated, then W settles
    run(s, &acts)
}

/// Fixed at engine b22bef7d: W is paid its fair 110_000. At 4db11a8c W got 94_728
/// and 15_272 was retired.
#[test]
fn new_bankruptcy_before_winner_refresh_is_fixed() {
    let r = case_bankruptcy_before_winner_refresh(false);
    assert_clean(&r);
    assert_eq!(r.accounts[1].out(), 110_000);
}

#[test]
fn new_bankruptcy_after_winner_refresh_control_passes() {
    let r = case_bankruptcy_before_winner_refresh(true);
    assert_clean(&r);
    assert_eq!(r.defaults.len(), 1);
}

// ---------------------------------------------------------------------------
// Upstream #212 case (b), engine level: a provider backs the short winner's domain
// and the long loser goes bankrupt. Fair: provider recovers its principal, the
// winner gets its deposit plus the loser's. Accounts: 0 = long loser, 1 = short winner.
// ---------------------------------------------------------------------------

fn case_212_provider(loser_first: bool) -> Report {
    let mut s = Setup::leveraged(3, [78_464, 0]);
    s.initial_margin_bps = 2_000;
    s.maintenance_margin_bps = 1_000;
    s.price_cap_bps = 500;
    let mut acts = vec![
        Deposit { acct: 0, amount: 200_000 },
        Deposit { acct: 1, amount: 1_000_000 },
        Trade { buyer: 0, seller: 1, units: 100 },
        Jump { up: false, slots: 6 },
    ];
    acts.extend(if loser_first {
        [Crank { acct: 0 }, Crank { acct: 1 }]
    } else {
        [Crank { acct: 1 }, Crank { acct: 0 }]
    });
    run(s, &acts)
}

/// At 4db11a8c the provider swept the loser's payment and the winner was paid short
/// (935_300; engine #172 site 2). At engine b22bef7d the winner was paid in full but the
/// provider recovered only 13_764 of 78_464 (inv4), as on #212's head (96f70b43).
/// Fixed on our engine fork b7751935 (#172 SettleB settles K/F first): the provider
/// recovers its full principal whichever account is cranked first.
#[test]
fn upstream_212_provider_case_is_fixed() {
    let r = case_212_provider(true);
    assert_clean(&r);
    assert_eq!(r.accounts[1].out(), 1_200_000);
    assert_eq!(r.provider_out, 78_464);
    assert_eq!(r.defaults.len(), 1);
}

#[test]
fn upstream_212_provider_case_control_passes() {
    let r = case_212_provider(false);
    assert_clean(&r);
    assert_eq!(r.accounts[1].out(), 1_200_000);
    assert_eq!(r.provider_out, 78_464);
    assert_eq!(r.defaults.len(), 1);
}
