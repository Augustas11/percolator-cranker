//! Candidate mark rules. To add one: write a struct implementing
//! `rules::Rule` below and add one line to `ALL`. Nothing else changes.
//! Score it with `flop-score --data <pin> --rule <name>`.

#[allow(unused_imports)]
use super::rules::{blend, up, Rule};
#[allow(unused_imports)]
use crate::{combine, or_quote_with, spec_quote, Guard, Holder, OrEndpoint, SPEC};

/// (name, constructor). Names are unique, lowercase, no spaces.
pub const ALL: &[(&str, fn() -> Box<dyn Rule>)] = &[
    ("hold-2h", || Box::new(Hold(Holder::new(2 * HOUR_MS, SPEC.hold_confirm_s * 1000)))),
    ("hold-6h", || Box::new(Hold(Holder::new(6 * HOUR_MS, SPEC.hold_confirm_s * 1000)))),
    ("hold-12h", || Box::new(Hold(Holder::new(12 * HOUR_MS, SPEC.hold_confirm_s * 1000)))),
    ("all-median", || Box::new(AllSellers { holder: Holder::new(0, 0), weighting: "median", fallback: false })),
    ("all-median-hold6h", || Box::new(AllSellers { holder: Holder::new(6 * HOUR_MS, SPEC.hold_confirm_s * 1000), weighting: "median", fallback: false })),
    ("wide", || Box::new(AllSellers { holder: Holder::new(6 * HOUR_MS, SPEC.hold_confirm_s * 1000), weighting: "median", fallback: true })),
    ("all-invsq-hold6h", || Box::new(AllSellers { holder: Holder::new(6 * HOUR_MS, SPEC.hold_confirm_s * 1000), weighting: "inverse_square", fallback: false })),
];

/// Every up seller of the model, any precision, one price per provider (its
/// lowest), combined by `weighting`; with an optional hold for sellers that
/// go offline. Scratch candidate for widening the settlement price.
#[derive(Clone)]
pub struct AllSellers { holder: Holder, weighting: &'static str, fallback: bool }
impl Rule for AllSellers {
    fn step(&mut self, at_ms: u64, endpoints: &[OrEndpoint]) -> Option<f64> {
        let (eps, _) = self.holder.apply(at_ms, endpoints);
        let mut best = std::collections::BTreeMap::<String, f64>::new();
        for e in up(&eps) {
            let b = blend(e);
            let x = best.entry(e.provider.clone()).or_insert(b);
            if b < *x { *x = b; }
        }
        if best.len() < 3 {
            // Too few sellers for a median: `fallback` uses the live rule, else hold.
            return if self.fallback { crate::spec_quote(&eps, at_ms).ok().map(|q| q.usd_e6 as f64 / 1e6) } else { None };
        }
        let mut v: Vec<f64> = best.into_values().collect();
        if self.weighting != "median" {
            // the live clamp: under half the median counts as half the median
            let mut s = v.clone(); s.sort_by(f64::total_cmp);
            let floor = 0.5 * combine(&s, "median").ok()?;
            for x in v.iter_mut() { if *x < floor { *x = floor; } }
        }
        combine(&v, self.weighting).ok()
    }
    fn clone_box(&self) -> Box<dyn Rule> { Box::new(self.clone()) }
}

const HOUR_MS: u64 = 3_600_000;

/// SPEC with a missing endpoint held at its last up price (`crate::Holder`,
/// owner 2026-11-13). On daily pins snapshots are 24 h apart, so no hold here
/// bridges two of them and these score exactly as `spec`; they differ only on
/// recorder data.
#[derive(Clone)]
pub struct Hold(Holder);
impl Rule for Hold {
    fn step(&mut self, at_ms: u64, endpoints: &[OrEndpoint]) -> Option<f64> {
        let (eps, _) = self.0.apply(at_ms, endpoints);
        spec_quote(&eps, at_ms).ok().map(|q| q.usd_e6 as f64 / 1e6)
    }
    fn clone_box(&self) -> Box<dyn Rule> {
        Box::new(self.clone())
    }
}
