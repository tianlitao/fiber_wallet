//! `fiber_wallet_core` — the local-first Fiber wallet core.
//!
//! Framework-agnostic Rust library: hosts (the planned NSH app, CLI tools,
//! smoke binaries) embed the node lifecycle machine and call the wallet
//! operations directly. UI bindings belong to the host, not to this crate.

pub mod api;
pub mod backup;
pub mod keystore;
pub mod node;
#[cfg(not(target_arch = "wasm32"))]
pub mod onchain;
mod payment;
pub mod recovery_smoke;
pub mod types;

pub use node::NodeMachine;
pub use types::{NodeNetwork, NodePhase, NodeSnapshot};
