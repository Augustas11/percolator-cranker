//! Stateful: the median of `median.rs`, but a provider that goes down (or
//! leaves the listing) keeps counting at its last up price for 30 minutes.

#![no_std]

use flop_rule::{export_rule, median, Quote, MAX_QUOTES};

const HOLD_MS: u64 = 30 * 60 * 1000;
const SLOTS: usize = 256;

/// Per provider: (name hash, last up price, when last seen up). 6 KiB.
struct State {
    n: usize,
    seen: [(u64, f64, u64); SLOTS],
}

const INITIAL: State = State { n: 0, seen: [(0, 0.0, 0); SLOTS] };

fn step(s: &mut State, at_ms: u64, quotes: &[Quote]) -> Option<f64> {
    // This step's price per up provider: its cheapest up endpoint.
    let mut ids = [0u64; MAX_QUOTES];
    let mut px = [0f64; MAX_QUOTES];
    let mut n = 0;
    for q in quotes.iter().filter(|q| q.is_up()) {
        let b = q.blend();
        match ids[..n].iter().position(|&p| p == q.provider) {
            Some(i) if b < px[i] => px[i] = b,
            Some(_) => {}
            None => {
                ids[n] = q.provider;
                px[n] = b;
                n += 1;
            }
        }
    }
    // Remember them.
    for i in 0..n {
        match s.seen[..s.n].iter().position(|e| e.0 == ids[i]) {
            Some(j) => s.seen[j] = (ids[i], px[i], at_ms),
            None if s.n < SLOTS => {
                s.seen[s.n] = (ids[i], px[i], at_ms);
                s.n += 1;
            }
            None => {}
        }
    }
    // Forget anything missing for longer than the hold (or stamped in the future).
    let mut k = 0;
    for j in 0..s.n {
        let e = s.seen[j];
        if e.2 <= at_ms && at_ms - e.2 <= HOLD_MS {
            s.seen[k] = e;
            k += 1;
        }
    }
    s.n = k;
    // Up providers at their price, plus held ones at their last up price.
    let up = n;
    for j in 0..s.n {
        let e = s.seen[j];
        if !ids[..up].contains(&e.0) && n < MAX_QUOTES {
            px[n] = e.1;
            n += 1;
        }
    }
    if n < 2 {
        return None;
    }
    median(&mut px[..n])
}

export_rule!(State = INITIAL, step);
