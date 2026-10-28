//! Build-time wrapper pin check.
//!
//! The wrapper (`percolator-prog`) is a path dependency, so Cargo cannot pin
//! it by rev. This script finds `scripts/pins.env` (walking up from the
//! crate) and the wrapper checkout (`PERCOLATOR_PROG_DIR`, else the
//! `percolator-prog` path in this crate's Cargo.toml), then fails the build
//! if `git rev-parse HEAD` there differs from `PERCOLATOR_PROG_SHA`.
//! `PERCOLATOR_SKIP_PIN_CHECK=1` downgrades a mismatch to a warning (for a
//! deliberate pre-merge preview build).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PERCOLATOR_PROG_DIR");
    println!("cargo:rerun-if-env-changed=PERCOLATOR_SKIP_PIN_CHECK");
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let Some(pins) = find_up(&manifest_dir, "scripts/pins.env") else {
        println!("cargo:warning=scripts/pins.env not found; wrapper pin not checked");
        return;
    };
    println!("cargo:rerun-if-changed={}", pins.display());
    let want = read_pin(&pins, "PERCOLATOR_PROG_SHA").expect("PERCOLATOR_PROG_SHA in pins.env");
    let prog = std::env::var("PERCOLATOR_PROG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join(prog_path_from_manifest(&manifest_dir)));
    let head = prog.join(".git");
    if head.is_dir() {
        println!("cargo:rerun-if-changed={}", head.join("HEAD").display());
    }
    let got = Command::new("git")
        .arg("-C")
        .arg(&prog)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let skip = std::env::var("PERCOLATOR_SKIP_PIN_CHECK").as_deref() == Ok("1");
    match got {
        Some(sha) if sha == want => {}
        other => {
            let msg = format!(
                "wrapper checkout {} is at {:?}, scripts/pins.env pins {want}. Run ./scripts/fetch_upstream.sh \
                 (or set PERCOLATOR_SKIP_PIN_CHECK=1 for a deliberate preview build)",
                prog.display(),
                other.as_deref().unwrap_or("<not a git checkout>")
            );
            if skip {
                println!("cargo:warning={msg}");
            } else {
                panic!("{msg}");
            }
        }
    }
}

fn find_up(start: &Path, rel: &str) -> Option<PathBuf> {
    start.ancestors().map(|d| d.join(rel)).find(|p| p.is_file())
}

fn read_pin(path: &Path, key: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()?.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

fn prog_path_from_manifest(manifest_dir: &Path) -> String {
    let toml = std::fs::read_to_string(manifest_dir.join("Cargo.toml")).unwrap_or_default();
    toml.lines()
        .find(|l| l.trim_start().starts_with("percolator-prog"))
        .and_then(|l| l.split("path = \"").nth(1))
        .and_then(|r| r.split('"').next())
        .unwrap_or("../repo-prog")
        .to_string()
}
