# Contributing

- **Price rules.** Open an issue with the "Propose a price rule" template.
  Add your rule to `flop-index/src/score/candidates.rs` (one struct, one line
  in `ALL`) and include the `flop-score` output for `--split tune` and
  `--split heldout`. A rule is recommended only if, on the held-out days,
  gain > 2 x se ([SCORER.md](docs/flop-compute/SCORER.md)). The live rule
  changes only through a reviewed change to
  [SPEC.md](docs/flop-compute/SPEC.md) and the code that implements it.
- **Bad prices.** Use the "Report a bad price" template: the time (UTC), the
  price shown, the price you expected and where you saw it.
- **Before sending a change:** run the tests of what you touched
  (`cargo test --release --locked --manifest-path <crate>/Cargo.toml`, or
  `npm ci && npm test && npm run build` in a Node package). CI runs them all.
- **No keys, mnemonics or RPC tokens in commits.** Keypair files, `.env`
  files and run directories are ignored; keep them that way.
- **Licence:** Apache-2.0 (see `LICENSE`). Contributions are accepted under
  the same licence.
