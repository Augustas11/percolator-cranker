use super::rules::{Mean, Spec};
use super::*;
use crate::{combine, spec_quote, OrEndpoint};
use data::Snapshot;

const DAY0: u64 = 20_400; // 2025-11-08, any day works

/// An endpoint whose 15:1 blend is `b` USD per 1M tokens.
fn ep(provider: &str, b: f64) -> OrEndpoint {
    OrEndpoint { provider: provider.into(), quantization: "bf16".into(), prompt: b * 1e-6, completion: b * 1e-6, status: 0 }
}

/// Today's bf16 pair plus two more, constant for `days` daily snapshots.
fn flat(days: u64) -> Series {
    let eps = vec![ep("DeepInfra", 0.2578), ep("GMICloud", 0.6219), ep("C", 0.30), ep("D", 0.45)];
    Series { group: "test:flat".into(), snaps: (0..days).map(|d| Snapshot { at_ms: (DAY0 + d) * DAY_MS, endpoints: eps.clone() }).collect() }
}

fn spec_mark(eps: &[OrEndpoint]) -> f64 {
    spec_quote(eps, 0).unwrap().usd_e6 as f64 / 1e6
}

/// A deterministic synthetic panel: `n` models, 3 to 6 providers each,
/// prices on a slow random walk, now and then a provider leaving for a day.
fn panel(n: usize, days: u64) -> Vec<Series> {
    let mut r = 7u64;
    let mut u = || {
        r = splitmix(r);
        (r >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..n)
        .map(|g| {
            let k = 3 + (u() * 4.0) as usize;
            let mut p: Vec<f64> = (0..k).map(|_| 0.1 + u() * 2.0).collect();
            let snaps = (0..days)
                .map(|d| {
                    for x in p.iter_mut() {
                        *x *= (0.03 * (u() - 0.5)).exp();
                    }
                    let endpoints = p.iter().enumerate().filter(|_| u() > 0.05).map(|(i, b)| ep(&format!("p{i}"), *b)).collect();
                    Snapshot { at_ms: (DAY0 + d) * DAY_MS, endpoints }
                })
                .collect();
            Series { group: format!("test:{g}"), snaps }
        })
        .collect()
}

#[test]
fn the_baseline_scores_zero_against_itself() {
    let s = panel(8, 60);
    for split in [Split::Tune, Split::Heldout, Split::All] {
        let r = score(&Spec, &Spec, &s, 1, split).unwrap();
        assert_eq!((r.total.gain, r.total.se), (0.0, 0.0), "{split:?}");
        assert_eq!((r.ex_reference.gain, r.ex_reference.se), (0.0, 0.0), "{split:?}");
        assert_eq!(r.base, r.cand);
        assert!(r.base.total() > 0.0, "the baseline has a loss of its own");
    }
}

#[test]
fn a_plain_mean_loses_on_relisting() {
    // Measured, not wished for: on the pinned data the plain mean scores
    // better overall (SCORER.md, results). It is worse where a mean must be:
    // a provider relisting at 10x. (Under the 2-point clamp, before
    // 2026-11-12, it also lost on jumps; with no clamp under 3 endpoints the
    // jump difference on this panel is within noise, so it is not asserted.)
    let s = panel(8, 60);
    for split in [Split::Tune, Split::Heldout] {
        let r = score(&Mean, &Spec, &s, 1, split).unwrap();
        assert!(r.scenario[3].gain < -2.0 * r.scenario[3].se && r.scenario[3].gain < -0.3, "{split:?} relist {:?}", r.scenario[3]);
    }
}

#[test]
fn same_data_and_seed_give_the_same_number() {
    let s = panel(6, 40);
    let a = score(&Mean, &Spec, &s, 3, Split::All).unwrap().total;
    let b = score(&Mean, &Spec, &s, 3, Split::All).unwrap().total;
    assert_eq!(a, b);
}

#[test]
fn the_last_30_percent_of_days_are_held_out() {
    let days: Vec<u64> = (0..10).collect();
    assert_eq!(split_days(&days, Split::Tune), &days[..7]);
    assert_eq!(split_days(&days, Split::Heldout), &days[7..]);
    assert_eq!(split_days(&days, Split::All), &days[..]);
    let s = panel(3, 40);
    let t = score(&Spec, &Spec, &s, 1, Split::Tune).unwrap();
    let h = score(&Spec, &Spec, &s, 1, Split::Heldout).unwrap();
    assert!(t.last_day < h.first_day);
    assert_eq!(h.last_day, DAY0 + 39);
    assert_eq!(h.total.n, 12);
}

#[test]
fn too_little_data_does_not_score() {
    assert!(score(&Spec, &Spec, &[], 1, Split::All).is_err());
    assert!(score(&Spec, &Spec, &[flat(2)], 1, Split::Heldout).is_err());
}

/// The baseline's attack loss on day 0 of `flat(3)`: the attack hits the
/// day-0 snapshot, day 1 is clean again, so the loss is half the displacement.
fn attack_case(k: usize) -> (f64, f64) {
    let s = flat(3);
    let clean = spec_mark(&s.snaps[0].endpoints);
    let at = plan(&s.snaps[0].endpoints, attack_rng(1, &s.group, DAY0, k), k, &[], &BTreeSet::new());
    let hit = attacked(&at, &s.snaps[0].endpoints);
    let want = match spec_quote(&hit, 0) {
        Ok(q) => (q.usd_e6 as f64 / 1e6 / clean).ln().abs() / 2.0,
        Err(_) => 0.0, // stale: the mark holds
    };
    let got = day_losses(&Spec, &[s], 1).unwrap()[&DAY0].attack[k];
    assert!((got - want).abs() < 1e-12, "{}: got {got} want {want}", SCENARIOS[k]);
    (got, clean)
}

#[test]
fn a_spoofed_cheap_listing_moves_the_baseline_about_30_percent() {
    let (loss, clean) = attack_case(0);
    assert!((clean - 0.327852).abs() < 1e-6, "{clean}");
    // A listing at ~1% of the median is clamped to half the new median
    // ($0.15) and still takes the most weight: $0.328 to $0.227.
    assert!((loss - (0.327852f64 / 0.2267).ln() / 2.0).abs() < 2e-3, "{loss}");
}

#[test]
fn a_promo_moves_the_baseline_most() {
    let (loss, _) = attack_case(1);
    // SPEC known limit: a promo is clamped, not dropped, and moves the mark.
    assert!(loss > 0.05, "{loss}");
}

#[test]
fn half_the_providers_down_moves_the_baseline() {
    let (loss, _) = attack_case(2);
    // Two of four left: the mark is theirs alone.
    assert!(loss > 0.0, "{loss}");
}

#[test]
fn a_relisting_at_10x_barely_moves_the_baseline() {
    let (loss, _) = attack_case(3);
    assert!(loss > 0.0 && loss < 0.05, "{loss}");
}

#[test]
fn with_two_providers_one_down_holds_the_mark() {
    let mut s = flat(3);
    for sn in &mut s.snaps {
        sn.endpoints.truncate(2);
    }
    // Stale (one up endpoint): the baseline holds, the attack costs nothing.
    // (Day 0 has no earlier mark to hold, so day 1.)
    assert_eq!(day_losses(&Spec, &[s], 1).unwrap()[&(DAY0 + 1)].attack[2], 0.0);
}

#[test]
fn the_reference_is_the_leave_one_provider_out_median() {
    let eps = [ep("a", 1.0), ep("b", 2.0), ep("c", 4.0), ep("d", 10.0)];
    // Without a: med(2,4,10)=4; b: 4; c: 2; d: 2. Median of those: 3.
    assert_eq!(reference(&eps), Some(3.0));
    assert_eq!(reference(&eps[..2]), Some(1.5));
}

#[test]
fn a_jump_the_median_explains_costs_nothing() {
    // Every price doubles on day 1: the mark doubles with the median.
    let mut s = flat(2);
    for e in &mut s.snaps[1].endpoints {
        e.prompt *= 2.0;
        e.completion *= 2.0;
    }
    let l = day_losses(&Spec, &[s], 1).unwrap();
    assert!(l[&(DAY0 + 1)].jump < 1e-5, "{}", l[&(DAY0 + 1)].jump);
    assert_eq!(l[&DAY0].jump, 0.0, "no earlier snapshot");
}

#[test]
fn the_live_bf16_pair_relisting_at_10x_no_longer_lifts_the_mark() {
    // SPEC's 2026-11-11 pair. Under the 2-point clamp (before 2026-11-12)
    // GMICloud relisting at 10x moved the median to $3.24, the floor to
    // $1.62, and clamped DeepInfra *up* to it: mark $1.911, 6x. A pair is
    // now too few for the clamp, so the relisting moves the mark the other
    // way, to DeepInfra's side: $0.268 (GMICloud loses weight).
    let pair = [ep("DeepInfra", 0.2578), ep("GMICloud", 0.6219)];
    assert!((spec_mark(&pair) - 0.311).abs() < 1e-3);
    let at = Attack { k: 3, victims: vec!["GMICloud".into()], quant: "bf16".into(), spoof_name: String::new(), mult: 10.0 };
    let hit = attacked(&at, &pair);
    let m = spec_mark(&hit);
    let raw = combine(&[0.2578, 6.219], "inverse_square").unwrap();
    assert!((raw - 0.268).abs() < 1e-3, "{raw}");
    assert!((m - raw).abs() < 2e-3, "{m}");
}

#[test]
fn the_spoof_wears_a_real_name_and_strengths_vary_with_the_seed() {
    let eps = flat(1).snaps[0].endpoints.clone();
    let pool = vec!["DeepInfra".to_string(), "Nebius".into()];
    let none = BTreeSet::new();
    let a = plan(&eps, attack_rng(1, "g", DAY0, 0), 0, &pool, &none);
    assert_eq!(a.spoof_name, "Nebius", "never a provider already listed");
    let later: BTreeSet<&str> = ["Nebius"].into();
    let n = plan(&eps, attack_rng(1, "g", DAY0, 0), 0, &pool, &later).spoof_name;
    assert!(n != "Nebius" && n.starts_with('p'), "nor one the series lists later: {n}");
    // Its input:output shape is a real listing's, not 1:1.
    let mut real = eps.clone();
    for e in &mut real {
        e.completion *= 6.0;
    }
    let sp = attacked(&a, &real).pop().unwrap();
    assert!((sp.completion / sp.prompt - 6.0).abs() < 1e-9, "{sp:?}");
    let m: BTreeSet<u64> = (0..20).map(|s| plan(&eps, attack_rng(s, "g", DAY0, 3), 3, &pool, &none).mult.to_bits()).collect();
    assert!(m.len() > 10);
    for s in 0..50 {
        let x = plan(&eps, attack_rng(s, "g", DAY0, 1), 1, &pool, &none).mult;
        assert!((0.05..=0.2).contains(&x), "{x}");
    }
}

#[test]
fn the_standard_error_counts_correlated_days() {
    // Alternating blocks of four: positively correlated at short lags, so
    // the corrected se is above the plain one.
    let d: Vec<f64> = (0..40).map(|i| if (i / 4) % 2 == 0 { 1.0 } else { -1.0 }).collect();
    let plain = (d.iter().map(|x| x * x).sum::<f64>() / 39.0 / 40.0).sqrt();
    assert!(paired(&d).se > plain, "{} vs {plain}", paired(&d).se);
    assert_eq!(paired(&[0.5; 10]).se, 0.0);
}

/// Holds its first mark forever.
#[derive(Clone, Default)]
struct Frozen(Option<f64>);
impl Rule for Frozen {
    fn step(&mut self, at_ms: u64, eps: &[OrEndpoint]) -> Option<f64> {
        if self.0.is_none() {
            self.0 = Spec.step(at_ms, eps);
        }
        self.0
    }
    fn clone_box(&self) -> Box<dyn Rule> {
        Box::new(self.clone())
    }
}

#[test]
fn a_frozen_mark_pays_for_not_following_real_moves() {
    let s = panel(8, 60);
    let r = score(&Frozen::default(), &Spec, &s, 1, Split::Heldout).unwrap();
    // The baseline follows within the jitter band: it pays nothing.
    assert_eq!(r.base.follow, 0.0);
    // A frozen mark pays the whole move less the band, >= ln 1.25 - 0.1.
    assert!(r.cand.follow > 1.25f64.ln() - FOLLOW_JITTER, "{}", r.cand.follow);
    assert!(r.total.gain < -2.0 * r.total.se, "{:?}", r.total);
    // ex-ref is the total less the reference part, day by day.
    assert!((r.ex_reference.gain - (r.total.gain - r.reference.gain)).abs() < 1e-12);
    assert!((r.ex_reference.gain - (r.jump.gain + r.attack.gain + r.follow.gain)).abs() < 1e-12);
}

#[test]
fn on_hourly_data_an_instant_follower_pays_nothing_and_a_lagging_one_pays() {
    // Hourly snapshots, so the stagger and the 6 h start matter.
    let mut s = flat(1);
    let eps = s.snaps[0].endpoints.clone();
    s.snaps = (0..24 * 6).map(|h| Snapshot { at_ms: DAY0 * DAY_MS + h * 3_600_000, endpoints: eps.clone() }).collect();
    let l = day_losses(&Spec, std::slice::from_ref(&s), 1).unwrap();
    assert!(l.values().all(|d| d.follow == 0.0));
    // Holds each new mark for 12 h before following.
    #[derive(Clone, Default)]
    struct Lag12(Option<(u64, f64)>);
    impl Rule for Lag12 {
        fn step(&mut self, at: u64, eps: &[OrEndpoint]) -> Option<f64> {
            let m = Spec.step(at, eps)?;
            match self.0 {
                Some((t, held)) if at < t + 12 * 3_600_000 => Some(held),
                _ => {
                    self.0 = Some((at, m));
                    Some(m)
                }
            }
        }
        fn clone_box(&self) -> Box<dyn Rule> {
            Box::new(self.clone())
        }
    }
    let lag = day_losses(&Lag12::default(), &[s], 1).unwrap();
    assert!(lag.values().any(|d| d.follow > 0.01), "{:?}", lag.values().map(|d| d.follow).collect::<Vec<_>>());
}
