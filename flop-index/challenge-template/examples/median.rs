//! Median of all up sellers, one price per provider (its cheapest up
//! endpoint). No state. Holds with fewer than 2 providers up.

#![no_std]

use flop_rule::{export_rule, median, Quote, MAX_QUOTES};

fn step(_: &mut (), _at_ms: u64, quotes: &[Quote]) -> Option<f64> {
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
    if n < 2 {
        return None;
    }
    median(&mut px[..n])
}

export_rule!(() = (), step);
