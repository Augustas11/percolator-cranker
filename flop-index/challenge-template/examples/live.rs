//! The live rule (docs/flop-compute/SPEC.md, `flop_index::spec_quote`)
//! re-implemented over the ABI: 15:1 blend; the first set with 2+ up
//! endpoints of bf16, fp16, bf16+fp16, then everything up; a price under
//! half the set's median counts as that floor in a set of 3+; 1/price²
//! weights; rounded to 1e-6 USD. Gives the same marks as the scorer's
//! `spec` rule. It runs without the endpoint hold, as `spec` does.

#![no_std]

use flop_rule::{export_rule, median, name, round, Quote, MAX_QUOTES};

const TIERS: [u64; 2] = [name("bf16"), name("fp16")];
const MIN_ENDPOINTS: usize = 2;
const CLAMP_MIN_SET: usize = 3;
const MIN_FRAC_OF_MEDIAN: f64 = 0.5;

fn step(_: &mut (), _at_ms: u64, quotes: &[Quote]) -> Option<f64> {
    let up = |q: &&Quote| q.is_up();
    let mut b = [0f64; MAX_QUOTES];
    // Blends of the up endpoints whose quantization passes `keep`.
    let mut pick = |keep: &dyn Fn(u64) -> bool| -> usize {
        let mut n = 0;
        for q in quotes.iter().filter(up).filter(|q| keep(q.quantization)) {
            b[n] = q.blend();
            n += 1;
        }
        n
    };
    let mut n = pick(&|q| q == TIERS[0]);
    if n < MIN_ENDPOINTS {
        n = pick(&|q| q == TIERS[1]);
    }
    if n < MIN_ENDPOINTS {
        n = pick(&|q| TIERS.contains(&q));
    }
    if n < MIN_ENDPOINTS {
        n = pick(&|_| true);
    }
    if n < MIN_ENDPOINTS {
        return None;
    }
    let set = &mut b[..n];
    set.sort_unstable_by(f64::total_cmp);
    if n >= CLAMP_MIN_SET {
        let mut sorted = [0f64; MAX_QUOTES];
        sorted[..n].copy_from_slice(set);
        let floor = MIN_FRAC_OF_MEDIAN * median(&mut sorted[..n])?;
        for x in set.iter_mut().filter(|x| **x < floor) {
            *x = floor;
        }
    }
    let (num, den) = set.iter().fold((0.0, 0.0), |(n, d), b| (n + 1.0 / b, d + 1.0 / (b * b)));
    let usd_e6 = round(num / den * 1e6);
    if !(usd_e6 >= 1.0 && usd_e6 < u64::MAX as f64) {
        return None;
    }
    Some(usd_e6 / 1e6)
}

export_rule!(() = (), step);
