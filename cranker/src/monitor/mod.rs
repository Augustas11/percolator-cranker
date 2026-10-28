//! Read-only liveness and solvency monitor (bin `monitor`).
//!
//! * [`snapshot`]: reads one market and its portfolios over RPC, decodes them
//!   with the wrapper's decoders, and derives the monitored quantities.
//! * [`status`]: the keeper status file (`CRANKER_STATUS_FILE`), written by
//!   the keeper after every cycle and read by the monitor.
//! * [`metrics`]: Prometheus text exposition.
//! * [`http`]: a tiny blocking HTTP server for `/metrics`, `/snapshot.json`
//!   and the dashboard at `/`.
//! * [`json`]: minimal JSON reader/writer (no new dependencies).
//!
//! The monitor never signs or sends a transaction. Metric meanings and
//! sources: `docs/MONITOR.md`.

pub mod http;
pub mod json;
pub mod metrics;
pub mod snapshot;
pub mod status;
