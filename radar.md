# Radar — what to watch next (as of 2026-09-11, post-M2.i)

- **M2.i is done** on wrapper `d5e2ec6f` + engine `495a5590`. Next cranker work is
  product hardening / stuck-state coverage — not another full re-pin unless pins move.
- **Engine `#197`–`#200`.** Still open post-`#195`. If wrapper Cargo pin advances,
  re-check wire + portfolio_id + plan surface before merging.
- **Launch lights still off:** placeholder `declare_id!`, null mainnet/devnet RPC,
  fee shares default 0, no funding apply/settle, no tags.
- **CU baseline:** no-op PermissionlessCrank ≈ **17,975** program CU on this pin.
- **Operator path:** keep dry-run only until a real program id + non-null RPC.
