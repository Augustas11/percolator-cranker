# Contributing

- **Before sending a change:** run `cargo build` and `cargo test` from the
  workspace root. If you touched the crank path, also run
  `./scripts/m1_bringup.sh` and a short `live_shaped` run
  (`CRANKER_CYCLES=50 ./scripts/live_shaped.sh`).
- **Keep the keeper's rules upstream-derived.** Use the engine predicate
  and the wrapper's decoders; don't hand-roll a match on plan variants or
  byte offsets.
- **Fee, bounty and mainnet paths are out of scope.**
- **No keys, mnemonics or RPC tokens in commits.**
- **License:** to be decided by the maintainer before the first tagged
  release. Contributions are accepted under the license the repository
  carries at that time.
