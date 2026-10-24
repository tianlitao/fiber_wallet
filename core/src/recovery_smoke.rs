//! Manual recovery smoke for the embedded Fiber node.
//!
//! Not part of `cargo test`: the native node backend is cfg-gated out of test
//! builds. A driver script runs this binary twice against one data directory:
//!
//! 1. `crash` — start the node, then exit without stopping it. Destructors do
//!    not run, so SQLite keeps its WAL and the data-dir lock is released only
//!    by the OS.
//! 2. `recover` — start against the same directory. Success proves the store
//!    reopens after an unclean exit, the stale lock does not block startup,
//!    and the node identity is stable across restarts.

use crate::node::NodeMachine;
use crate::types::{NodeNetwork, NodePhase};

/// Runs one smoke phase. Returns the process exit code.
pub fn run(data_dir: String, phase: RecoverySmokePhase) -> i32 {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async move {
        const SMOKE_PASSWORD: &str = "fiber-wallet-recovery-smoke";

        let node = NodeMachine::new(NodeNetwork::Testnet, data_dir, SMOKE_PASSWORD.to_owned());

        let running = match node.start().await {
            Ok(snapshot) if snapshot.phase == NodePhase::Running => snapshot,
            Ok(snapshot) => {
                eprintln!("node did not reach Running: {snapshot:?}");
                return 1;
            }
            Err(error) => {
                eprintln!("node start failed: {error}");
                return 1;
            }
        };

        let node_id = running.node_id.clone().expect("running node id");
        // stderr: the crash phase hard-exits, which would drop buffered stdout.
        eprintln!("phase {phase:?}: node running, node_id={node_id}");

        if phase == RecoverySmokePhase::Crash {
            eprintln!("simulating unclean exit (no stop, no SQLite close)");
            std::process::exit(9);
        }

        let stopped = match node.stop().await {
            Ok(snapshot) if snapshot.phase == NodePhase::Stopped => snapshot,
            Ok(snapshot) => {
                eprintln!("node did not reach Stopped: {snapshot:?}");
                return 1;
            }
            Err(error) => {
                eprintln!("node stop failed: {error}");
                return 1;
            }
        };
        println!("phase recover: clean stop ok ({})", stopped.detail);
        0
    })
}

/// The two phases driven by `scripts/recovery_smoke.sh`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoverySmokePhase {
    Crash,
    Recover,
}

impl RecoverySmokePhase {
    /// Parses the phase CLI argument.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "crash" => Some(Self::Crash),
            "recover" => Some(Self::Recover),
            _ => None,
        }
    }
}
