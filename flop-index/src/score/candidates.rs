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
];

const HOUR_MS: u64 = 3_600_000;

/// SPEC with a missing endpoint held at its last up price (`crate::Holder`,
/// decision 2026-11-13). On daily pins snapshots are 24 h apart, so no hold here
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
