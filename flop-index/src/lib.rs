//! The FLOP compute perp's OpenRouter mark rule (docs/flop-compute/SPEC.md),
//! shared by the pusher (staging/src/pusher/flop.rs) and the scorer
//! (`flop-score`, docs/flop-compute/SCORER.md). Pure: no I/O, no config.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub mod score;

/// The SPEC rule's parameters. The pusher reads its own from
/// launch/flop-compute.toml; a staging test checks they equal these.
pub struct Params {
    pub blend: f64,
    pub quantizations: &'static [&'static str],
    pub min_endpoints: usize,
    pub weighting: &'static str,
    pub min_frac_of_median: f64,
    /// The clamp applies only to a set with at least this many up endpoints.
    pub clamp_min_set: usize,
    /// An endpoint that goes down or leaves the listing counts at its last up
    /// price for this long (`Holder`). 0 = off.
    pub hold_missing_s: u64,
}

/// The live rule (owner decisions 2026-11-11): 15:1 blend, bf16 then fp16,
/// at least 2 up endpoints, 1/price² weights, clamp at half the median; the
/// clamp needs at least 3 endpoints in the set (owner, 2026-11-12).
pub const SPEC: Params = Params {
    blend: 15.0,
    quantizations: &["bf16", "fp16"],
    min_endpoints: 2,
    weighting: "inverse_square",
    min_frac_of_median: 0.5,
    clamp_min_set: 3,
    // Off until the hold length is chosen on recorder data (owner, 2026-11-13:
    // rule approved; score 2 h, 6 h and 12 h on the first full week).
    hold_missing_s: 0,
};

/// One OpenRouter endpoint for the benchmark model, prices in USD per token.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrEndpoint {
    pub provider: String,
    pub quantization: String,
    pub prompt: f64,
    pub completion: f64,
    /// OpenRouter's endpoint status: 0 is up. Only up endpoints count.
    #[serde(default)]
    pub status: i64,
}

/// The pusher's filter for a usable price (status aside).
fn priced(e: &OrEndpoint) -> bool {
    e.prompt.is_finite() && e.completion.is_finite() && e.prompt >= 0.0 && e.completion >= 0.0 && e.prompt + e.completion > 0.0
}

/// An endpoint counted at its last up price while it is down or unlisted.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Held {
    pub provider: String,
    pub quantization: String,
    /// When it was last seen up, at the price now counted.
    pub last_up_ms: u64,
}

/// Holds a missing endpoint at its last up price (owner, 2026-11-13). An
/// endpoint that is down, unpriced or gone from the listing keeps counting,
/// as up and at the price it last had while up, until it has been missing
/// longer than `hold_ms`; then it is dropped as before. One that comes back
/// counts at its new price at once. So a short outage, or a provider stepping
/// out of the listing, does not move the mark; a real exit moves it `hold_ms`
/// late. Endpoints are told apart by provider, quantization and position
/// among equals. State is in memory: after a restart nothing is held until
/// an endpoint has been seen up again.
#[derive(Clone, Debug, Default)]
pub struct Holder {
    hold_ms: u64,
    last_up: std::collections::BTreeMap<(String, String, usize), (OrEndpoint, u64)>,
}

impl Holder {
    pub fn new(hold_ms: u64) -> Self {
        Self { hold_ms, last_up: Default::default() }
    }

    /// The listing to price at `at_ms`: `endpoints` with each missing one
    /// inside its hold replaced by (or, if unlisted, added as) its last up
    /// row. Also returns which were held. `hold_ms` 0 returns the input.
    pub fn apply(&mut self, at_ms: u64, endpoints: &[OrEndpoint]) -> (Vec<OrEndpoint>, Vec<Held>) {
        if self.hold_ms == 0 {
            return (endpoints.to_vec(), Vec::new());
        }
        let hold_ms = self.hold_ms;
        // Older than the hold: forgotten. A clock that steps back holds.
        self.last_up.retain(|_, (_, t)| at_ms.saturating_sub(*t) <= hold_ms);
        let mut nth = std::collections::BTreeMap::<(String, String), usize>::new();
        let mut listed = std::collections::BTreeSet::new();
        let (mut out, mut held) = (Vec::with_capacity(endpoints.len()), Vec::new());
        let note = |e: &OrEndpoint, t: u64| Held { provider: e.provider.clone(), quantization: e.quantization.clone(), last_up_ms: t };
        for e in endpoints {
            let n = nth.entry((e.provider.clone(), e.quantization.clone())).or_default();
            let key = (e.provider.clone(), e.quantization.clone(), *n);
            *n += 1;
            listed.insert(key.clone());
            if e.status == 0 && priced(e) {
                self.last_up.insert(key, (e.clone(), at_ms));
                out.push(e.clone());
            } else if let Some((old, t)) = self.last_up.get(&key) {
                held.push(note(old, *t));
                out.push(old.clone());
            } else {
                out.push(e.clone());
            }
        }
        for (key, (old, t)) in &self.last_up {
            if !listed.contains(key) {
                held.push(note(old, *t));
                out.push(old.clone());
            }
        }
        (out, held)
    }
}

/// One endpoint in the priced set: blended USD per 1M tokens as listed, and
/// as priced after the cheap-outlier clamp (equal unless clamped).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Used {
    pub provider: String,
    pub quantization: String,
    pub listed: f64,
    pub priced: f64,
}

/// How the cheap-outlier guard treats a price under the floor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Guard {
    /// Count it as the floor, in a set of at least `min_set` endpoints;
    /// smaller sets are not clamped. Live rule: `min_set` 3. Lines pushed
    /// before 2026-11-12 clamped any set (`min_set` 0).
    Clamp { min_set: usize },
    /// Drop it, and hold if too few remain. Only to verify lines pushed
    /// before the clamp (they carry no "guard" field).
    LegacyDrop,
}

/// The live USD leg: the combined blended price and the endpoints behind it.
#[derive(Clone, Debug, PartialEq)]
pub struct OrQuote {
    pub usd_e6: u64,
    pub at_ms: u64,
    /// Each endpoint used, with its listed and clamped price.
    pub used: Vec<Used>,
    /// Which set was used: a quantization tier, "preferred" (all tiers) or "all".
    pub set: String,
    /// The cheap-outlier guard on this set: "on", "off (n<3)" (set too small
    /// for a median floor) or "off" (disabled).
    pub clamp: String,
    /// Every priced endpoint fetched, so the push log can be recomputed. A
    /// held endpoint (`held`) is here as its last up row.
    pub endpoints: Vec<OrEndpoint>,
    /// Endpoints counted at their last up price (`Holder`); the caller that
    /// ran the holder fills it in.
    pub held: Vec<Held>,
}

/// The SPEC rule on one endpoint list, as the pusher runs it with the
/// shipped config.
pub fn spec_quote(endpoints: &[OrEndpoint], at_ms: u64) -> Result<OrQuote> {
    let qs: Vec<String> = SPEC.quantizations.iter().map(|q| q.to_string()).collect();
    or_quote_with(endpoints, at_ms, SPEC.blend, &qs, SPEC.min_endpoints, SPEC.weighting, SPEC.min_frac_of_median, Guard::Clamp { min_set: SPEC.clamp_min_set })
}

/// Combine blended prices (USD per 1M tokens, each > 0).
/// `inverse_square`: weights 1/b^2, as OpenRouter's default routing spreads
/// load, so the mark sits where buyers' money goes: sum(1/b) / sum(1/b^2).
/// `median`: the plain median (mean of the middle two for an even count).
pub fn combine(prices: &[f64], weighting: &str) -> Result<f64> {
    if prices.is_empty() || prices.iter().any(|b| !(b.is_finite() && *b > 0.0)) {
        bail!("openrouter: no positive prices to combine");
    }
    match weighting {
        "inverse_square" => {
            let (num, den) = prices.iter().fold((0.0, 0.0), |(n, d), b| (n + 1.0 / b, d + 1.0 / (b * b)));
            Ok(num / den)
        }
        "median" => {
            let mut v = prices.to_vec();
            v.sort_by(f64::total_cmp);
            let n = v.len();
            Ok(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
        }
        other => bail!("or_weighting must be inverse_square or median, not {other:?}"),
    }
}

/// Pick endpoints: the first quantization tier with at least `min_endpoints`;
/// else all listed tiers together if that reaches it; else every endpoint.
/// Each endpoint's price is its input:output blend; `weighting` combines them.
#[allow(clippy::too_many_arguments)]
pub fn or_quote_with(
    endpoints: &[OrEndpoint], at_ms: u64, blend: f64, quantizations: &[String], min_endpoints: usize, weighting: &str,
    min_frac_of_median: f64, guard: Guard,
) -> Result<OrQuote> {
    let valid: Vec<&OrEndpoint> = endpoints.iter().filter(|e| priced(e)).filter(|e| e.status == 0).collect();
    let tier = |qs: &[String]| valid.iter().copied().filter(|e| qs.iter().any(|q| q == &e.quantization)).collect::<Vec<_>>();
    // Sets in order: each preferred quantization alone, then the preferred
    // ones together, then every up endpoint. The first with `min_endpoints`
    // up endpoints prices the push (owner, 2026-11-11: fall through rather
    // than hold); none: hold (stale). Cheap prices never empty a set: they
    // are clamped, below.
    let mut sets: Vec<(Vec<&OrEndpoint>, String)> = quantizations.iter().map(|q| (tier(std::slice::from_ref(q)), q.clone())).collect();
    if quantizations.len() > 1 {
        sets.push((tier(quantizations), "preferred".to_owned()));
    }
    // Lines pushed before the clamp priced from "all" with any endpoint.
    let all_min = if guard == Guard::LegacyDrop { 1 } else { min_endpoints };
    let mut why = Vec::new();
    let mut picked = None;
    for (set, name) in sets {
        if set.len() >= min_endpoints {
            picked = Some((set, name));
            break;
        }
        why.push(format!("{name}: {} endpoint(s)", set.len()));
    }
    if picked.is_none() && valid.len() >= all_min.max(1) {
        picked = Some((valid.clone(), "all".to_owned()));
    }
    let Some((set, mut name)) = picked else {
        bail!("openrouter: no set has {min_endpoints} up endpoints ({}; all: {})", why.join("; "), valid.len());
    };
    let b = blend;
    let mut used: Vec<Used> = set
        .iter()
        .map(|e| {
            let usd = (b * e.prompt + e.completion) / (b + 1.0) * 1e6;
            Used { provider: e.provider.clone(), quantization: e.quantization.clone(), listed: usd, priced: usd }
        })
        .collect();
    used.sort_by(|x, y| x.listed.total_cmp(&y.listed));
    // Cheap-outlier guard: 1/b² weights hand a very cheap listing (a
    // mispricing, a promo) nearly all the weight, so a price under
    // `min_frac_of_median` of the set's median counts as that floor (owner,
    // 2026-11-11: clamp, don't drop). Dropping made the mark jump when the
    // cheapest provider crossed the floor; clamping keeps it continuous and
    // never lets a cheaper listing raise it.
    //
    // A set under `min_set` endpoints is not clamped (owner, 2026-11-12): with
    // two points the half-median floor is a quarter of their sum, so it lifted
    // the cheaper legitimate endpoint when an outage left two (DeepInfra
    // $0.2578 priced at $0.3198, mark +23%). Too few points to call one an
    // outlier.
    let mut clamp = if min_frac_of_median > 0.0 { "on".to_owned() } else { "off".to_owned() };
    if let Guard::Clamp { min_set } = guard {
        if min_frac_of_median > 0.0 && used.len() < min_set {
            clamp = format!("off (n<{min_set})");
        }
    }
    if clamp == "on" {
        let floor = min_frac_of_median * combine(&used.iter().map(|u| u.listed).collect::<Vec<_>>(), "median")?;
        match guard {
            Guard::Clamp { .. } => {
                let mut n = 0;
                for u in used.iter_mut().filter(|u| u.listed < floor) {
                    u.priced = floor;
                    n += 1;
                }
                if n > 0 {
                    name = format!("{name}-clamped");
                }
            }
            Guard::LegacyDrop => {
                let before = used.len();
                used.retain(|u| u.listed >= floor);
                if used.len() < before {
                    if used.len() < min_endpoints {
                        bail!("openrouter: {} endpoint(s) under {floor:.4} dropped as outliers, {} left, need {min_endpoints}", before - used.len(), used.len());
                    }
                    name = format!("{name}-trimmed");
                }
            }
        }
    }
    let prices: Vec<f64> = used.iter().map(|u| u.priced).collect();
    let price = combine(&prices, weighting)?;
    let usd_e6 = (price * 1e6).round();
    if !(usd_e6 >= 1.0 && usd_e6 < u64::MAX as f64) {
        bail!("openrouter: {weighting} {price} USD per 1M tokens out of range");
    }
    // Every priced endpoint, down ones included, so the log can be recomputed.
    let endpoints = endpoints.iter().filter(|e| priced(e)).cloned().collect();
    Ok(OrQuote { usd_e6: usd_e6 as u64, at_ms, used, set: name, clamp, endpoints, held: Vec::new() })
}

#[cfg(test)]
mod tests {
    #[test]
    fn spec_is_the_shipped_config() {
        // The pusher reads its rule from this file; the scorer scores SPEC.
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../launch/flop-compute.toml")).unwrap();
        let v: toml::Value = text.parse().unwrap();
        let o = &v["oracle"];
        let s = &super::SPEC;
        assert_eq!(o["flop_blend_in_out"].as_integer().map(|x| x as f64).or(o["flop_blend_in_out"].as_float()), Some(s.blend));
        let qs: Vec<&str> = o["or_quantizations"].as_array().unwrap().iter().map(|q| q.as_str().unwrap()).collect();
        assert_eq!(qs, s.quantizations);
        assert_eq!(o["or_min_endpoints"].as_integer(), Some(s.min_endpoints as i64));
        assert_eq!(o["or_weighting"].as_str(), Some(s.weighting));
        assert_eq!(o["or_min_frac_of_median"].as_float(), Some(s.min_frac_of_median));
        assert_eq!(o["or_clamp_min_endpoints"].as_integer(), Some(s.clamp_min_set as i64));
        assert_eq!(o["or_hold_missing_s"].as_integer(), Some(s.hold_missing_s as i64));
    }

    use super::{spec_quote, Holder, OrEndpoint};

    fn ep(provider: &str, usd_in: f64, usd_out: f64, status: i64) -> OrEndpoint {
        OrEndpoint { provider: provider.into(), quantization: "bf16".into(), prompt: usd_in / 1e6, completion: usd_out / 1e6, status }
    }
    const H: u64 = 3_600_000;
    fn mark(h: &mut Holder, at: u64, eps: &[OrEndpoint]) -> Option<u64> {
        spec_quote(&h.apply(at, eps).0, at).ok().map(|q| q.usd_e6)
    }
    // The live bf16 pair and a third, so the outage leaves a set of two.
    fn trio(gmi_status: i64) -> Vec<OrEndpoint> {
        vec![ep("DeepInfra", 0.15, 1.875, 0), ep("GMICloud", 0.45, 3.2, gmi_status), ep("Novita", 0.30, 2.4, 0)]
    }

    #[test]
    fn a_short_outage_does_not_move_the_mark() {
        let mut h = Holder::new(6 * H);
        let up = mark(&mut h, 0, &trio(0)).unwrap();
        // Without the hold the outage moves it.
        assert_ne!(spec_quote(&trio(-2), 0).unwrap().usd_e6, up);
        for t in [H / 12, H, 6 * H] {
            assert_eq!(mark(&mut h, t, &trio(-2)), Some(up), "held at {t}");
        }
        let (_, held) = h.apply(6 * H, &trio(-2));
        assert_eq!(held.len(), 1);
        assert_eq!((held[0].provider.as_str(), held[0].last_up_ms), ("GMICloud", 0));
    }

    #[test]
    fn a_long_outage_drops_the_endpoint_after_the_hold() {
        let mut h = Holder::new(6 * H);
        let up = mark(&mut h, 0, &trio(0)).unwrap();
        let gone = spec_quote(&trio(-2), 0).unwrap().usd_e6;
        assert_eq!(mark(&mut h, 6 * H, &trio(-2)), Some(up));
        assert_eq!(mark(&mut h, 6 * H + 1, &trio(-2)), Some(gone));
        assert!(h.apply(7 * H, &trio(-2)).1.is_empty());
    }

    #[test]
    fn an_unlisted_endpoint_is_held_like_a_down_one() {
        let mut h = Holder::new(2 * H);
        let up = mark(&mut h, 0, &trio(0)).unwrap();
        let without: Vec<OrEndpoint> = trio(0).into_iter().filter(|e| e.provider != "GMICloud").collect();
        assert_eq!(mark(&mut h, H, &without), Some(up));
        assert_ne!(mark(&mut h, 3 * H, &without), Some(up));
    }

    #[test]
    fn a_returning_endpoint_counts_at_its_new_price_at_once() {
        let mut h = Holder::new(6 * H);
        mark(&mut h, 0, &trio(0));
        mark(&mut h, H, &trio(-2));
        let mut back = trio(0);
        back[1] = ep("GMICloud", 0.30, 2.0, 0);
        assert_eq!(mark(&mut h, 2 * H, &back), Some(spec_quote(&back, 0).unwrap().usd_e6));
        // And that new price is what a later outage holds.
        assert_eq!(mark(&mut h, 3 * H, &trio(-2)), Some(spec_quote(&back, 0).unwrap().usd_e6));
    }

    #[test]
    fn a_price_changed_while_down_is_not_used() {
        // A down endpoint's listed price is not a price anyone can buy at.
        let mut h = Holder::new(6 * H);
        let up = mark(&mut h, 0, &trio(0)).unwrap();
        let mut down = trio(-2);
        down[1] = ep("GMICloud", 0.001, 0.001, -2);
        assert_eq!(mark(&mut h, H, &down), Some(up));
    }

    #[test]
    fn nothing_is_held_before_it_was_seen_up_and_zero_is_off() {
        let mut h = Holder::new(6 * H);
        assert_eq!(mark(&mut h, 0, &trio(-2)), Some(spec_quote(&trio(-2), 0).unwrap().usd_e6));
        let mut off = Holder::new(0);
        mark(&mut off, 0, &trio(0));
        assert_eq!(mark(&mut off, 1, &trio(-2)), Some(spec_quote(&trio(-2), 0).unwrap().usd_e6));
    }

    #[test]
    fn two_endpoints_of_one_provider_and_tier_are_held_apart() {
        let mut h = Holder::new(6 * H);
        let two = |s: i64| vec![ep("A", 0.1, 1.0, 0), ep("A", 0.4, 3.0, s), ep("B", 0.2, 2.0, 0)];
        let up = mark(&mut h, 0, &two(0)).unwrap();
        assert_eq!(mark(&mut h, H, &two(-2)), Some(up));
    }
}
