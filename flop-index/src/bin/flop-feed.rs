//! flop-feed: write the /compute page's JSON from the push log and the
//! OpenRouter recorder. See docs/flop-compute/FEED.md.
//!
//!   flop-feed <bots.log> <or-prices dir> <out dir> --oracle <pubkey> [--loop <seconds>]

use anyhow::{bail, Context, Result};
use flop_index::feed::{write_atomic, Feed};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: flop-feed <bots.log> <or-prices dir> <out dir> --oracle <pubkey> [--loop <seconds>]";

fn once(feed: &mut Feed, log: &Path, or_dir: &Path, out: &Path) -> Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let (latest, history) = feed.build(log, or_dir, now)?;
    write_atomic(out, "history.json", &history)?;
    write_atomic(out, "latest.json", &latest)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (paths, oracle, every) = match args.as_slice() {
        [a, b, c, o, key] if o == "--oracle" => ([a, b, c], key, None),
        [a, b, c, o, key, l, secs] if o == "--oracle" && l == "--loop" => ([a, b, c], key, Some(secs.parse::<u64>().context(USAGE)?)),
        _ => bail!(USAGE),
    };
    // A base58 Solana pubkey: 32 to 44 characters, no 0, O, I or l.
    if !(32..=44).contains(&oracle.len()) || !oracle.chars().all(|c| c.is_ascii_alphanumeric() && !"0OIl".contains(c)) {
        bail!("--oracle must be the oracle authority's base58 pubkey");
    }
    let [log, or_dir, out] = paths.map(PathBuf::from);
    let mut feed = Feed::new(oracle);
    let Some(every) = every else { return once(&mut feed, &log, &or_dir, &out) };
    if every == 0 {
        bail!("--loop needs at least 1 second");
    }
    loop {
        if let Err(e) = once(&mut feed, &log, &or_dir, &out) {
            eprintln!("flop-feed: {e:#}");
        }
        std::thread::sleep(Duration::from_secs(every));
    }
}
