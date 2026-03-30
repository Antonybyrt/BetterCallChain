/// P2P networking layer for BetterCallChain.
///
/// - [`protocol`]: wire message enum (JSON + length-delimited framing).
/// - [`server`]: TCP listener that spawns per-peer tasks.
/// - [`handler`]: per-peer read/write select loop and message dispatch.
pub mod handler;
pub mod protocol;
pub mod server;
