//! Rules the scorer can run: the SPEC baseline, a deliberately bad one, and
//! candidates (candidates.rs).

use crate::{spec_quote, OrEndpoint, SPEC};

/// A mark rule replayed over one series, oldest snapshot first. `step` gets
/// every endpoint listed at `at_ms` (down ones too, `status != 0`) and
/// returns USD per 1M benchmark tokens, or None to hold (stale). A rule may
/// keep state between steps (a time filter, a smoother); the scorer clones
/// it to replay attacks from the same point.
pub trait Rule {
    fn step(&mut self, at_ms: u64, endpoints: &[OrEndpoint]) -> Option<f64>;
    fn clone_box(&self) -> Box<dyn Rule>;
    /// Why the rule is out of the running, if it is (a WebAssembly rule
    /// that broke a limit). The scorer then returns an error, not losses.
    fn disqualified(&self) -> Option<String> {
        None
    }
}

/// USD per 1M tokens at the SPEC's 15:1 input:output blend.
pub fn blend(e: &OrEndpoint) -> f64 {
    (SPEC.blend * e.prompt + e.completion) / (SPEC.blend + 1.0) * 1e6
}

/// Up endpoints with usable prices (the pusher's filter).
pub fn up(endpoints: &[OrEndpoint]) -> impl Iterator<Item = &OrEndpoint> {
    endpoints.iter().filter(|e| e.status == 0 && e.prompt.is_finite() && e.completion.is_finite() && e.prompt >= 0.0 && e.completion >= 0.0 && e.prompt + e.completion > 0.0)
}

/// The live rule, the same code the pusher runs (crate::spec_quote).
#[derive(Clone)]
pub struct Spec;
impl Rule for Spec {
    fn step(&mut self, at_ms: u64, endpoints: &[OrEndpoint]) -> Option<f64> {
        spec_quote(endpoints, at_ms).ok().map(|q| q.usd_e6 as f64 / 1e6)
    }
    fn clone_box(&self) -> Box<dyn Rule> {
        Box::new(self.clone())
    }
}

/// SPEC's documented alternative (`or_weighting = median`): same sets and
/// clamp, plain median. A reference point, not a candidate.
#[derive(Clone)]
pub struct SpecMedian;
impl Rule for SpecMedian {
    fn step(&mut self, at_ms: u64, endpoints: &[OrEndpoint]) -> Option<f64> {
        let qs: Vec<String> = SPEC.quantizations.iter().map(|q| q.to_string()).collect();
        crate::or_quote_with(endpoints, at_ms, SPEC.blend, &qs, SPEC.min_endpoints, "median", SPEC.min_frac_of_median, crate::Guard::Clamp { min_set: SPEC.clamp_min_set })
            .ok()
            .map(|q| q.usd_e6 as f64 / 1e6)
    }
    fn clone_box(&self) -> Box<dyn Rule> {
        Box::new(self.clone())
    }
}

/// Deliberately bad: plain mean of every up endpoint, no clamp, no sets.
#[derive(Clone)]
pub struct Mean;
impl Rule for Mean {
    fn step(&mut self, _: u64, endpoints: &[OrEndpoint]) -> Option<f64> {
        let b: Vec<f64> = up(endpoints).map(blend).collect();
        (!b.is_empty()).then(|| b.iter().sum::<f64>() / b.len() as f64)
    }
    fn clone_box(&self) -> Box<dyn Rule> {
        Box::new(self.clone())
    }
}

/// Every rule by name: `spec`, `spec-median`, `mean`, then candidates::ALL.
pub fn names() -> Vec<&'static str> {
    let mut v = vec!["spec", "spec-median", "mean"];
    v.extend(super::candidates::ALL.iter().map(|(n, _)| *n));
    v
}

pub fn by_name(name: &str) -> Option<Box<dyn Rule>> {
    match name {
        "spec" => Some(Box::new(Spec)),
        "spec-median" => Some(Box::new(SpecMedian)),
        "mean" => Some(Box::new(Mean)),
        _ => super::candidates::ALL.iter().find(|(n, _)| *n == name).map(|(_, f)| f()),
    }
}
