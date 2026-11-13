---
name: Propose a price rule
about: A candidate mark rule for the compute price, with its scorer numbers
title: "Price rule: <name>"
labels: price-rule
---

**Rule name** (as in `flop-score list`):

**What it does**, in a few lines (which endpoints count, how they combine,
any state it keeps between snapshots):

**Code**: a branch or PR adding it to `flop-index/src/score/candidates.rs`.

**Scorer output** (paste the full output, both splits, same seed):

```
flop-score --data <pin dir> --rule <name> --split tune
flop-score --data <pin dir> --rule <name> --split heldout
```

**Pin used** (e.g. `2026-11-11`, or a recorder pin you made with `flop-score pin`):

**Where it is worse** than the current rule (the per-part numbers):
