//! flop-feed: write the /compute page's JSON from the push log, the
//! OpenRouter recorder and (optionally) the wide collector. See
//! docs/flop-compute/FEED.md.
//!
//!   flop-feed <bots.log> <or-prices dir> <out dir> --oracle <pubkey> [--wide <dir>] [--loop <seconds>]

use anyhow::{bail, Context, Result};
use flop_index::feed::{write_atomic, Feed};
use flop_index::wide::Wide;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: flop-feed <bots.log> <or-prices dir> <out dir> --oracle <pubkey> [--wide <dir>] [--loop <seconds>]";

fn once(feed: &mut Feed, wide: &mut Option<(Wide, PathBuf)>, log: &Path, or_dir: &Path, out: &Path) -> Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    // The market file does not wait for a push line, nor the push files for it.
    let market = wide.as_mut().map(|(w, dir)| -> Result<()> {
        w.update(dir, now)?;
        write_atomic(out, "market.json", &w.market_json(now)?)
    });
    let (latest, history) = feed.build(log, or_dir, now)?;
    write_atomic(out, "history.json", &history)?;
    write_atomic(out, "latest.json", &latest)?;
    market.unwrap_or(Ok(()))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [log, or_dir, out] = match args.get(..3) {
        Some([a, b, c]) if !a.starts_with("--") && !b.starts_with("--") && !c.starts_with("--") => [a, b, c].map(PathBuf::from),
        _ => bail!(USAGE),
    };
    let (mut oracle, mut wide_dir, mut every) = (None, None, None);
    let mut rest = args[3..].iter();
    while let Some(flag) = rest.next() {
        let val = rest.next().context(USAGE)?;
        match flag.as_str() {
            "--oracle" => oracle = Some(val.clone()),
            "--wide" => wide_dir = Some(PathBuf::from(val)),
            "--loop" => every = Some(val.parse::<u64>().context(USAGE)?),
            _ => bail!(USAGE),
        }
    }
    let oracle = oracle.context(USAGE)?;
    // A base58 Solana pubkey: 32 to 44 characters, no 0, O, I or l.
    if !(32..=44).contains(&oracle.len()) || !oracle.chars().all(|c| c.is_ascii_alphanumeric() && !"0OIl".contains(c)) {
        bail!("--oracle must be the oracle authority's base58 pubkey");
    }
    let mut feed = Feed::new(&oracle);
    let mut wide = wide_dir.map(|d| (Wide::default(), d));
    let Some(every) = every else { return once(&mut feed, &mut wide, &log, &or_dir, &out) };
    if every == 0 {
        bail!("--loop needs at least 1 second");
    }
    loop {
        if let Err(e) = once(&mut feed, &mut wide, &log, &or_dir, &out) {
            eprintln!("flop-feed: {e:#}");
        }
        std::thread::sleep(Duration::from_secs(every));
    }
}
