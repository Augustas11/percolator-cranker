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
}

/// The live rule (owner decisions 2026-11-11): 15:1 blend, bf16 then fp16,
/// at least 2 up endpoints, 1/price² weights, clamp at half the median.
pub const SPEC: Params = Params { blend: 15.0, quantizations: &["bf16", "fp16"], min_endpoints: 2, weighting: "inverse_square", min_frac_of_median: 0.5 };

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
    /// Count it as the floor. Live rule.
    Clamp,
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
    /// Every priced endpoint fetched, so the push log can be recomputed.
    pub endpoints: Vec<OrEndpoint>,
}

/// The SPEC rule on one endpoint list, as the pusher runs it with the
/// shipped config.
pub fn spec_quote(endpoints: &[OrEndpoint], at_ms: u64) -> Result<OrQuote> {
    let qs: Vec<String> = SPEC.quantizations.iter().map(|q| q.to_string()).collect();
    or_quote_with(endpoints, at_ms, SPEC.blend, &qs, SPEC.min_endpoints, SPEC.weighting, SPEC.min_frac_of_median, Guard::Clamp)
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
    let priced = |e: &&OrEndpoint| e.prompt.is_finite() && e.completion.is_finite() && e.prompt >= 0.0 && e.completion >= 0.0 && e.prompt + e.completion > 0.0;
    let valid: Vec<&OrEndpoint> = endpoints.iter().filter(priced).filter(|e| e.status == 0).collect();
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
    if min_frac_of_median > 0.0 {
        let floor = min_frac_of_median * combine(&used.iter().map(|u| u.listed).collect::<Vec<_>>(), "median")?;
        match guard {
            Guard::Clamp => {
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
    let endpoints = endpoints.iter().filter(priced).cloned().collect();
    Ok(OrQuote { usd_e6: usd_e6 as u64, at_ms, used, set: name, endpoints })
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
    }
}
