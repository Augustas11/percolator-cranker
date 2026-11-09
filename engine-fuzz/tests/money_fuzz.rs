//! Random money fuzzer: proptest action sequences over a bounded domain, checked
//! against the shadow ledger at the end of every run.
//!
//! Env:
//! - `FUZZ_CASES` (default 64): proptest cases for `money_fuzz`, and scenarios for
//!   the survey.
//! - `FUZZ_STRICT=1`: the known upstream bug classes (invariants 2-4, see
//!   `Invariant::is_known_bug_class`) fail too. Off in CI, on in run.sh.
//! - `FUZZ_INVARIANT=<label>[,<label>]`: in strict mode, only these invariants fail
//!   (so proptest shrinks to a minimal repro of that class). Non-bug-class
//!   invariants always fail regardless.
//! - `FUZZ_SURVEY=1`: run `money_survey`, which runs `FUZZ_CASES` scenarios without
//!   stopping and prints a histogram of violated invariants.

use engine_fuzz::*;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, TestRunner};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
struct Scenario {
    setup: Setup,
    actions: Vec<Action>,
}

/// (initial margin, maintenance margin, price cap) profiles that validate.
const PROFILES: [(u64, u64, u64); 4] = [
    (1_000, 500, 400),
    (2_000, 1_000, 500),
    (5_000, 2_500, 2_000),
    (10_000, 10_000, 10_000),
];

fn setup_strategy() -> impl Strategy<Value = Setup> {
    (
        3usize..=5,
        0usize..PROFILES.len(),
        prop_oneof![Just(0u128), 1_000u128..=200_000],
        prop_oneof![Just(0u128), 1_000u128..=200_000],
    )
        .prop_map(|(accounts, profile, p0, p1)| {
            let (im, mm, cap) = PROFILES[profile];
            Setup {
                accounts,
                initial_margin_bps: im,
                maintenance_margin_bps: mm,
                price_cap_bps: cap,
                start_price: 10_000,
                provider: [p0, p1],
                h_max: 10,
                expiry: 1_000_000,
            }
        })
}

fn action_strategy() -> impl Strategy<Value = Action> {
    let acct = 0usize..5;
    prop_oneof![
        3 => (acct.clone(), 1_000u128..=500_000).prop_map(|(acct, amount)| Action::Deposit { acct, amount }),
        8 => (acct.clone(), acct.clone(), prop_oneof![1u64..=10, 1u64..=100])
            .prop_map(|(buyer, seller, units)| Action::Trade { buyer, seller, units }),
        6 => prop_oneof![-50i32..=50, -10_000i32..=10_000].prop_map(|bps| Action::Price { bps }),
        2 => (any::<bool>(), 1u8..=6).prop_map(|(up, slots)| Action::Jump { up, slots }),
        1 => Just(Action::Advance),
        5 => acct.clone().prop_map(|acct| Action::Crank { acct }),
        2 => acct.clone().prop_map(|acct| Action::CrankOnce { acct }),
        1 => acct.clone().prop_map(|acct| Action::Refresh { acct }),
        2 => acct.clone().prop_map(|acct| Action::Liquidate { acct }),
        2 => acct.clone().prop_map(|acct| Action::Convert { acct }),
        2 => (acct.clone(), 1u128..=200_000).prop_map(|(acct, amount)| Action::Withdraw { acct, amount }),
        1 => (0usize..2).prop_map(|domain| Action::ProviderWithdraw { domain }),
    ]
}

/// Straight into the window after a bankruptcy: positions, a price gap, then only
/// cranks and trades in that same slot, so accounts can act before their loss
/// share settles (the #253 shape). Random action lists almost never get here.
fn bankruptcy_window_strategy() -> impl Strategy<Value = Scenario> {
    let acct = 0usize..5;
    let opening = (acct.clone(), acct.clone(), 1u64..=100)
        .prop_map(|(buyer, seller, units)| Action::Trade { buyer, seller, units });
    let in_window = prop_oneof![
        1 => acct.clone().prop_map(|acct| Action::CrankOnce { acct }),
        1 => acct.clone().prop_map(|acct| Action::Crank { acct }),
        4 => (acct.clone(), acct.clone(), 1u64..=100)
            .prop_map(|(buyer, seller, units)| Action::Trade { buyer, seller, units }),
    ];
    (
        setup_strategy(),
        prop::collection::vec(10_000u128..=2_000_000, 5),
        prop::collection::vec(opening, 2..8),
        (any::<bool>(), 1u8..=6),
        Just((0usize..5).collect::<Vec<_>>()).prop_shuffle(),
        prop::collection::vec(in_window, 1..12),
    )
        .prop_map(|(setup, initial, opening, (up, slots), refresh_order, window)| {
            let mut actions: Vec<Action> = initial
                .into_iter()
                .take(setup.accounts)
                .enumerate()
                .map(|(acct, amount)| Action::Deposit { acct, amount })
                .collect();
            actions.extend(opening);
            actions.push(Action::Jump { up, slots });
            // Refresh everyone (certificates current), then crank the worst account until
            // its liquidation books the loss. Nobody else has settled it yet.
            actions.extend(refresh_order.into_iter().map(|acct| Action::CrankOnce { acct }));
            actions.extend([Action::CrankWorst; 3]);
            actions.extend(window);
            Scenario { setup, actions }
        })
}

fn scenario_strategy() -> impl Strategy<Value = Scenario> {
    prop_oneof![3 => random_scenario_strategy(), 1 => bankruptcy_window_strategy()]
}

fn random_scenario_strategy() -> impl Strategy<Value = Scenario> {
    (
        setup_strategy(),
        prop::collection::vec(10_000u128..=1_000_000, 5),
        prop::collection::vec(action_strategy(), 1..60),
    )
        .prop_map(|(setup, initial, mut actions)| {
            // Every account starts funded so trades have counterparties.
            let mut all: Vec<Action> = initial
                .into_iter()
                .take(setup.accounts)
                .enumerate()
                .map(|(acct, amount)| Action::Deposit { acct, amount })
                .collect();
            all.append(&mut actions);
            Scenario { setup, actions: all }
        })
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).map(|v| v == "1").unwrap_or(false)
}

fn cases() -> u32 {
    std::env::var("FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64)
}

fn targeted() -> Option<Vec<Invariant>> {
    let v = std::env::var("FUZZ_INVARIANT").ok()?;
    Some(
        v.split(',')
            .filter(|s| !s.is_empty())
            .map(|s| Invariant::from_label(s.trim()).unwrap_or_else(|| panic!("unknown invariant {s}")))
            .collect(),
    )
}

fn is_fatal(inv: Invariant, strict: bool, targets: &Option<Vec<Invariant>>) -> bool {
    if !inv.is_known_bug_class() {
        return true;
    }
    if !strict {
        return false;
    }
    match targets {
        Some(t) => t.contains(&inv),
        None => true,
    }
}

fn config(cases: u32) -> Config {
    Config {
        cases,
        failure_persistence: None,
        max_shrink_iters: 4_096,
        ..Config::default()
    }
}

#[test]
fn money_fuzz() {
    let strict = env_flag("FUZZ_STRICT");
    let targets = targeted();
    let mut runner = TestRunner::new(config(cases()));
    let result = runner.run(&scenario_strategy(), |scn| {
        let r = run(scn.setup, &scn.actions);
        let v = r.check();
        let fatal: Vec<Violation> = v
            .iter()
            .filter(|x| is_fatal(x.invariant, strict, &targets))
            .cloned()
            .collect();
        if !fatal.is_empty() {
            return Err(TestCaseError::fail(r.explain(&fatal)));
        }
        if r.unexplained_transfer(&v) {
            return Err(TestCaseError::fail(format!(
                "money moved between winners with no known cause (new bug?)\n{}",
                r.explain(&v)
            )));
        }
        Ok(())
    });
    if let Err(e) = result {
        panic!(
            "money fuzz found a violation (strict={strict}, targets={targets:?}).\nMinimal (shrunk) case:\n{e}"
        );
    }
}

/// Histogram of violated invariants over many random scenarios (no shrinking).
#[test]
fn money_survey() {
    if !env_flag("FUZZ_SURVEY") {
        return;
    }
    let strict = env_flag("FUZZ_STRICT");
    let n = cases();
    let mut runner = TestRunner::new(config(n));
    let strategy = scenario_strategy();
    let mut hist: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut first: BTreeMap<&'static str, String> = BTreeMap::new();
    let (mut defaults, mut recoveries, mut liquidated) = (0usize, 0usize, 0usize);
    let mut always_fatal = Vec::new();
    let mut all_bad = Vec::new();
    let mut provider_after_default = 0usize;
    let mut resized = 0usize;
    let mut unexplained = 0usize;
    for _ in 0..n {
        let scn = strategy.new_tree(&mut runner).unwrap().current();
        let r = run(scn.setup, &scn.actions);
        if r.had_default() {
            defaults += 1;
        }
        if r.recovery {
            recoveries += 1;
        }
        if r.trace.iter().any(|t| t.contains("Liquidate") || t.contains("liquidate acct")) {
            liquidated += 1;
        }
        let v = r.check();
        let mut labels: Vec<&'static str> = v.iter().map(|x| x.invariant.label()).collect();
        labels.sort();
        labels.dedup();
        for l in &labels {
            *hist.entry(l).or_default() += 1;
            first.entry(l).or_insert_with(|| {
                v.iter()
                    .filter(|x| x.invariant.label() == *l)
                    .map(|x| x.detail.clone())
                    .collect::<Vec<_>>()
                    .join(" | ")
            });
        }
        if r.resized_before_loss_settled {
            resized += 1;
        }
        if r.unexplained_transfer(&v) {
            unexplained += 1;
            always_fatal.push(format!("money moved between winners, no known cause\n{}", r.explain(&v)));
        }
        if r.had_default() && labels.contains(&Invariant::ProviderPrincipal.label()) {
            provider_after_default += 1;
        }
        if v.iter().any(|x| !x.invariant.is_known_bug_class()) {
            always_fatal.push(r.explain(&v));
        }
        if !v.is_empty() {
            all_bad.push(r.explain(&v));
        }
    }
    println!("survey: {n} scenarios (strict={strict}); with default {defaults}, with liquidation {liquidated}, recovery {recoveries}");
    for inv in ALL_INVARIANTS {
        let c = hist.get(inv.label()).copied().unwrap_or(0);
        println!(
            "  {:<28} {:>6} runs{}",
            inv.label(),
            c,
            if inv.is_known_bug_class() { "  (known bug class)" } else { "" }
        );
    }
    println!("  of which inv4 after a default   {provider_after_default:>6} runs");
    println!("  resized before loss settled {resized:>6} runs");
    println!("  winner transfers, no known cause {unexplained:>4} runs");
    for (l, d) in &first {
        println!("  first {l}: {d}");
    }
    // Debug aid: write every violating scenario (full report) to a file.
    if let Ok(path) = std::env::var("FUZZ_SURVEY_DUMP") {
        std::fs::write(&path, all_bad.join("\n=====\n")).unwrap();
    }
    if let Some(first_bad) = always_fatal.first() {
        panic!(
            "{} scenarios violated an always-on invariant; first:\n{first_bad}",
            always_fatal.len()
        );
    }
}
