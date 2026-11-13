//! The mark-rule scorer (docs/flop-compute/SCORER.md).
//!
//! Replays a rule over pinned snapshots and charges it, per day, four
//! losses in log units (|ln(a/b)|, so 0.01 is about 1%):
//!
//! - jump: each move of the mark not explained by the move of the median
//!   listed price, |Δln mark − Δln median|, between consecutive snapshots;
//! - attack: mean displacement |ln(attacked mark / clean mark)| while an
//!   injected scenario runs (24 h) and for 24 h after, one anchor a day:
//!   a spoofed cheap listing (~1% of the median), a promo (~0.1x), half the
//!   providers down, a provider relisting (~10x);
//! - follow: lag behind a genuine move (every provider about 1.25x to 3x,
//!   up or down, each within +-10% and at its own time in the first 6 h),
//!   charged from 6 h to 48 h beyond the +-10% band. Added after a near-frozen
//!   mark passed the gate by ignoring everything, attacks included;
//! - reference: distance from the leave-one-provider-out median.
//!
//! A day's loss is jump + mean(attack) + follow + reference. The score of a candidate
//! is the paired gain, baseline loss minus candidate loss on the same days
//! (positive: the candidate is better), with its standard error over days.

pub mod candidates;
pub mod data;
pub mod rules;
pub mod wasm;

use anyhow::{bail, Context, Result};
use data::{Series, DAY_MS};
use rules::{blend, up, Rule};
use std::collections::{BTreeMap, BTreeSet};

pub const SCENARIOS: [&str; 4] = ["spoof", "promo", "down", "relist"];
/// Each attack lasts this long...
pub const ATTACK_MS: u64 = DAY_MS;
/// ...and is measured this long past its end, for lingering effects.
pub const AFTER_MS: u64 = DAY_MS;
/// A genuine move's size range (log-uniform, up or down). Real cuts reach
/// past 2x (DeepInfra's qwen3.8-27b listing went $0.56 to $0.26 blended)...
pub const FOLLOW: (f64, f64) = (1.25, 3.0);
/// ...each provider within +-10% (log) of it...
pub const FOLLOW_JITTER: f64 = 0.1;
/// ...moving at its own time within the first 6 h.
pub const FOLLOW_STAGGER_MS: u64 = 6 * 3_600_000;
/// Loss charged for a term where the rule has no mark (never priced yet).
pub const PENALTY: f64 = 1.0;
/// The last 30% of days are held out: never used for tuning.
pub const HELD_OUT_FRAC: f64 = 0.3;
/// Per scenario, the range its strength is drawn from (log-uniform): spoof
/// price as a fraction of the median (~1%), promo multiplier (~0.1x),
/// unused for down, relisting multiplier (~10x).
pub const MULT: [(f64, f64); 4] = [(0.005, 0.02), (0.05, 0.2), (1.0, 1.0), (5.0, 20.0)];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Split {
    Tune,
    Heldout,
    All,
}

impl std::str::FromStr for Split {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "tune" => Split::Tune,
            "heldout" => Split::Heldout,
            "all" => Split::All,
            _ => bail!("split is tune, heldout or all, not {s:?}"),
        })
    }
}

/// One rule's loss on one day, each part averaged over the series scored
/// that day (a part no series had that day is 0).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DayLoss {
    pub jump: f64,
    pub attack: [f64; 4],
    /// Lag behind a genuine market move (all providers move, each by its
    /// own amount and at its own time).
    pub follow: f64,
    pub reference: f64,
}

impl DayLoss {
    pub fn attack_mean(&self) -> f64 {
        self.attack.iter().sum::<f64>() / 4.0
    }
    pub fn ex_reference(&self) -> f64 {
        self.total() - self.reference
    }
    pub fn total(&self) -> f64 {
        self.jump + self.attack_mean() + self.follow + self.reference
    }
}

#[derive(Default)]
struct Acc {
    jump: (f64, usize),
    reference: (f64, usize),
    attack: [(f64, usize); 4],
    follow: (f64, usize),
}

fn add(a: &mut (f64, usize), x: f64) {
    a.0 += x;
    a.1 += 1;
}

fn avg(a: (f64, usize)) -> f64 {
    if a.1 == 0 { 0.0 } else { a.0 / a.1 as f64 }
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// Median of the leave-one-provider-out medians of the up blends.
fn reference(eps: &[crate::OrEndpoint]) -> Option<f64> {
    let provs: BTreeSet<&str> = up(eps).map(|e| e.provider.as_str()).collect();
    if provs.len() < 2 {
        return median(up(eps).map(blend).collect());
    }
    median(provs.iter().filter_map(|p| median(up(eps).filter(|e| e.provider != *p).map(blend).collect())).collect())
}

fn dist(a: Option<f64>, b: Option<f64>) -> f64 {
    match (a, b) {
        (Some(a), Some(b)) => (a / b).ln().abs(),
        (None, None) => 0.0,
        _ => PENALTY,
    }
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn attack_rng(seed: u64, group: &str, day: u64, k: usize) -> u64 {
    splitmix(seed ^ splitmix(fnv(group) ^ splitmix(day ^ splitmix(k as u64))))
}

/// One injected attack: who it hits and how hard.
#[derive(Clone, Debug)]
struct Attack {
    k: usize,
    /// One provider for spoof/promo/relist (the spoof copies its
    /// quantization), half the providers (rounded down) for down.
    victims: Vec<String>,
    quant: String,
    /// The spoof lists under a real provider name from elsewhere in the
    /// data, priced in the median endpoint's input:output shape, so a rule
    /// can't spot it by name or shape.
    spoof_name: String,
    /// Spoof: fraction of the median. Promo / relist: price multiplier.
    /// Drawn log-uniformly from the seed so a rule can't key on one value.
    mult: f64,
}

fn plan(eps: &[crate::OrEndpoint], rng: u64, k: usize, pool: &[String], listed: &BTreeSet<&str>) -> Attack {
    let mut provs: Vec<String> = up(eps).map(|e| e.provider.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let mut r = rng;
    for i in (1..provs.len()).rev() {
        r = splitmix(r);
        provs.swap(i, (r % (i as u64 + 1)) as usize);
    }
    let take = if SCENARIOS[k] == "down" { provs.len() / 2 } else { 1 };
    provs.truncate(take);
    let quant = up(eps).find(|e| provs.first() == Some(&e.provider)).map(|e| e.quantization.clone()).unwrap_or_default();
    let free: Vec<&String> = pool.iter().filter(|p| !listed.contains(p.as_str()) && !eps.iter().any(|e| e.provider == **p)).collect();
    r = splitmix(r);
    let spoof_name = if free.is_empty() { format!("p{:08x}", r >> 32) } else { free[(r % free.len() as u64) as usize].clone() };
    r = splitmix(r);
    let u = (r >> 11) as f64 / (1u64 << 53) as f64;
    let (lo, hi) = MULT[k];
    let mult = (lo.ln() + u * (hi.ln() - lo.ln())).exp();
    Attack { k, victims: provs, quant, spoof_name, mult }
}

/// The endpoints as an attack shows them.
fn attacked(at: &Attack, eps: &[crate::OrEndpoint]) -> Vec<crate::OrEndpoint> {
    let mut v = eps.to_vec();
    let hit = |e: &crate::OrEndpoint| at.victims.iter().any(|p| *p == e.provider);
    match SCENARIOS[at.k] {
        "spoof" => {
            // Priced like the median endpoint (same input:output shape),
            // scaled so its blend is `mult` x the median.
            let mut by: Vec<&crate::OrEndpoint> = up(eps).collect();
            by.sort_by(|a, b| blend(a).total_cmp(&blend(b)));
            if let (Some(med), Some(shape)) = (median(up(eps).map(blend).collect()), by.get(by.len() / 2)) {
                let f = at.mult * med / blend(shape);
                v.push(crate::OrEndpoint { provider: at.spoof_name.clone(), quantization: at.quant.clone(), prompt: shape.prompt * f, completion: shape.completion * f, status: 0 });
            }
        }
        "promo" | "relist" => {
            for e in v.iter_mut().filter(|e| hit(e)) {
                e.prompt *= at.mult;
                e.completion *= at.mult;
            }
        }
        "down" => {
            for e in v.iter_mut().filter(|e| hit(e)) {
                e.status = 1;
            }
        }
        _ => unreachable!(),
    }
    v
}

fn providers(eps: &[crate::OrEndpoint]) -> usize {
    up(eps).map(|e| e.provider.as_str()).collect::<BTreeSet<_>>().len()
}

fn run_series(proto: &dyn Rule, s: &Series, seed: u64, pool: &[String], out: &mut BTreeMap<u64, Acc>) {
    let snaps = &s.snaps;
    let scored: Vec<bool> = snaps.iter().map(|sn| providers(&sn.endpoints) >= 2).collect();
    // Anchors: the first scored snapshot of each day.
    let mut anchors = BTreeMap::new();
    for (i, sn) in snaps.iter().enumerate() {
        if scored[i] {
            anchors.entry(sn.at_ms / DAY_MS).or_insert(i);
        }
    }
    let anchor_at: BTreeSet<usize> = anchors.values().copied().collect();
    // Every provider this series ever lists: a spoof never borrows one.
    let listed: BTreeSet<&str> = snaps.iter().flat_map(|sn| sn.endpoints.iter().map(|e| e.provider.as_str())).collect();
    // Clean pass: effective marks (a hold keeps the last one), and the rule's
    // state just before each anchor.
    let mut rule = proto.clone_box();
    let mut last = None;
    let mut marks = Vec::with_capacity(snaps.len());
    let mut saved: BTreeMap<usize, (Box<dyn Rule>, Option<f64>)> = BTreeMap::new();
    for (i, sn) in snaps.iter().enumerate() {
        if anchor_at.contains(&i) {
            saved.insert(i, (rule.clone_box(), last));
        }
        if let Some(m) = rule.step(sn.at_ms, &sn.endpoints).filter(|m| m.is_finite() && *m > 0.0) {
            last = Some(m);
        }
        marks.push(last);
    }
    // Jump and reference on every scored snapshot.
    let mut prev: Option<(Option<f64>, f64)> = None;
    for (i, sn) in snaps.iter().enumerate() {
        if !scored[i] {
            continue;
        }
        let acc = out.entry(sn.at_ms / DAY_MS).or_default();
        let med = median(up(&sn.endpoints).map(blend).collect()).expect("scored has 2+ up endpoints");
        add(&mut acc.reference, dist(marks[i], reference(&sn.endpoints)));
        if let Some((pm, pmed)) = prev {
            let j = match (marks[i], pm) {
                (Some(m), Some(pm)) => ((m / pm).ln() - (med / pmed).ln()).abs(),
                _ => PENALTY,
            };
            add(&mut acc.jump, j);
        }
        prev = Some((marks[i], med));
    }
    // Attacks from each anchor.
    for (day, a) in anchors {
        let start = snaps[a].at_ms;
        for k in 0..4 {
            let at = plan(&snaps[a].endpoints, attack_rng(seed, &s.group, day, k), k, pool, &listed);
            let (st, l) = &saved[&a];
            let (mut r, mut last) = (st.clone_box(), *l);
            let mut d = (0.0, 0);
            for (i, sn) in snaps.iter().enumerate().skip(a) {
                if sn.at_ms >= start + ATTACK_MS + AFTER_MS {
                    break;
                }
                let eps = if sn.at_ms < start + ATTACK_MS { attacked(&at, &sn.endpoints) } else { sn.endpoints.clone() };
                if let Some(m) = r.step(sn.at_ms, &eps).filter(|m| m.is_finite() && *m > 0.0) {
                    last = Some(m);
                }
                add(&mut d, dist(last, marks[i]));
            }
            add(&mut out.entry(day).or_default().attack[k], avg(d));
        }
        // A genuine move: every provider moves by about m (1.25x to 3x, up or
        // down, from the seed; each provider its own +-10% around it) at its
        // own time in the first FOLLOW_STAGGER_MS, and stays. The rule should
        // follow: loss is the distance from m x the clean mark, charged once
        // every provider has moved, to the end of the window. Jitter and
        // stagger keep the move from being recognisable as synthetic.
        let r0 = splitmix(attack_rng(seed, &s.group, day, 4));
        let u = (r0 >> 11) as f64 / (1u64 << 53) as f64;
        let lm = (FOLLOW.0.ln() + u * (FOLLOW.1.ln() - FOLLOW.0.ln())) * if r0 & 1 == 0 { 1.0 } else { -1.0 };
        let m = lm.exp();
        // Per provider (by name, so it holds across snapshots): its factor
        // and when it moves.
        let per = |p: &str| {
            let h = splitmix(r0 ^ fnv(p));
            let u = (h >> 11) as f64 / (1u64 << 53) as f64;
            let at = start + splitmix(h) % (FOLLOW_STAGGER_MS + 1);
            (m * (FOLLOW_JITTER * (2.0 * u - 1.0)).exp(), at)
        };
        let (st, l) = &saved[&a];
        let (mut r, mut last) = (st.clone_box(), *l);
        let mut d = (0.0, 0);
        for (i, sn) in snaps.iter().enumerate().skip(a) {
            if sn.at_ms >= start + ATTACK_MS + AFTER_MS {
                break;
            }
            let eps: Vec<crate::OrEndpoint> = sn
                .endpoints
                .iter()
                .map(|e| {
                    let (f, at) = per(&e.provider);
                    let f = if sn.at_ms >= at { f } else { 1.0 };
                    crate::OrEndpoint { prompt: e.prompt * f, completion: e.completion * f, ..e.clone() }
                })
                .collect();
            if let Some(x) = r.step(sn.at_ms, &eps).filter(|x| x.is_finite() && *x > 0.0) {
                last = Some(x);
            }
            if sn.at_ms >= start + FOLLOW_STAGGER_MS {
                // The jitter band is free: following is charged, the mix of
                // providers' slightly different moves is not.
                add(&mut d, (dist(last, marks[i].map(|c| c * m)) - FOLLOW_JITTER).max(0.0));
            }
        }
        add(&mut out.entry(day).or_default().follow, avg(d));
    }
}

/// A rule's loss per day (day = UTC day number), or an error if the rule
/// was disqualified while it ran.
pub fn day_losses(rule: &dyn Rule, series: &[Series], seed: u64) -> Result<BTreeMap<u64, DayLoss>> {
    let pool: Vec<String> = series.iter().flat_map(|s| s.snaps.iter().flat_map(|sn| sn.endpoints.iter().map(|e| e.provider.clone()))).collect::<BTreeSet<_>>().into_iter().collect();
    let mut acc = BTreeMap::new();
    for s in series {
        run_series(rule, s, seed, &pool, &mut acc);
    }
    if let Some(why) = rule.disqualified() {
        bail!("disqualified: {why}");
    }
    Ok(acc
        .into_iter()
        .map(|(d, a)| (d, DayLoss { jump: avg(a.jump), follow: avg(a.follow), reference: avg(a.reference), attack: [avg(a.attack[0]), avg(a.attack[1]), avg(a.attack[2]), avg(a.attack[3])] }))
        .collect())
}

/// Mean of paired differences and its standard error.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stat {
    pub gain: f64,
    pub se: f64,
    pub n: usize,
}

/// Standard error with Newey-West (Bartlett) correction: days are not
/// independent (an attack window spans two days, rules carry state), so
/// autocovariances up to lag max(2, floor(4 (n/100)^(2/9))) are counted.
pub fn paired(d: &[f64]) -> Stat {
    let n = d.len();
    let mean = d.iter().sum::<f64>() / n as f64;
    let gamma = |l: usize| d.iter().zip(&d[l..]).map(|(a, b)| (a - mean) * (b - mean)).sum::<f64>() / n as f64;
    let lags = ((4.0 * (n as f64 / 100.0).powf(2.0 / 9.0)).floor() as usize).max(2).min(n.saturating_sub(1));
    let mut var = gamma(0);
    for l in 1..=lags {
        var += 2.0 * (1.0 - l as f64 / (lags + 1) as f64) * gamma(l);
    }
    Stat { gain: mean, se: (var.max(0.0) / n as f64).sqrt(), n }
}

pub struct Report {
    pub total: Stat,
    pub jump: Stat,
    pub attack: Stat,
    pub scenario: [Stat; 4],
    pub follow: Stat,
    pub reference: Stat,
    /// The total without the reference part. The reference is a median,
    /// so it favours median-like rules by construction; a candidate whose
    /// gain lives only there is suspect.
    pub ex_reference: Stat,
    /// Mean daily loss of the baseline and the candidate on the split.
    pub base: DayLoss,
    pub cand: DayLoss,
    pub first_day: u64,
    pub last_day: u64,
}

impl Report {
    /// The promotion gate (SPEC): gain above twice its standard error.
    pub fn passes(&self) -> bool {
        self.total.gain > 2.0 * self.total.se
    }
}

/// The days in a split: the last 30% of all days scored are held out.
pub fn split_days(days: &[u64], split: Split) -> &[u64] {
    let tune = ((days.len() as f64) * (1.0 - HELD_OUT_FRAC)).floor() as usize;
    match split {
        Split::Tune => &days[..tune],
        Split::Heldout => &days[tune..],
        Split::All => days,
    }
}

pub fn score(cand: &dyn Rule, base: &dyn Rule, series: &[Series], seed: u64, split: Split) -> Result<Report> {
    if series.is_empty() {
        bail!("no series with 2+ providers in the data");
    }
    let b = day_losses(base, series, seed).map_err(|e| anyhow::anyhow!("{e:#} (baseline)"))?;
    let c = day_losses(cand, series, seed).map_err(|e| anyhow::anyhow!("{e:#} (candidate)"))?;
    let all: Vec<u64> = b.keys().copied().collect();
    let days = split_days(&all, split);
    if days.len() < 2 {
        bail!("{split:?} split has {} scored day(s), need 2", days.len());
    }
    let pick = |f: &dyn Fn(&DayLoss) -> f64| -> Stat {
        let d: Vec<f64> = days.iter().map(|d| f(&b[d]) - f(&c[d])).collect();
        paired(&d)
    };
    let mean = |m: &BTreeMap<u64, DayLoss>| {
        let n = days.len() as f64;
        let mut x = DayLoss::default();
        for d in days {
            let l = &m[d];
            x.jump += l.jump / n;
            x.reference += l.reference / n;
            x.follow += l.follow / n;
            for k in 0..4 {
                x.attack[k] += l.attack[k] / n;
            }
        }
        x
    };
    Ok(Report {
        total: pick(&|l| l.total()),
        jump: pick(&|l| l.jump),
        attack: pick(&|l| l.attack_mean()),
        scenario: [pick(&|l| l.attack[0]), pick(&|l| l.attack[1]), pick(&|l| l.attack[2]), pick(&|l| l.attack[3])],
        follow: pick(&|l| l.follow),
        ex_reference: pick(&|l| l.ex_reference()),
        reference: pick(&|l| l.reference),
        base: mean(&b),
        cand: mean(&c),
        first_day: *days.first().context("no days")?,
        last_day: *days.last().context("no days")?,
    })
}

#[cfg(test)]
mod tests;
