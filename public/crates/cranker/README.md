# percolator-cranker crate

Library and binaries for the reference keeper. See the workspace
[README](../../README.md), [RUNBOOK](../../docs/RUNBOOK.md) and
[ARCHITECTURE](../../docs/ARCHITECTURE.md).

```bash
cargo run --release -p percolator-cranker --bin cranker        # bootstrap + N cycles
cargo run --release -p percolator-cranker --bin live_shaped    # load scenario
cargo test -p percolator-cranker                               # unit tests (no validator)
```
