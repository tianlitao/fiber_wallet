use std::sync::OnceLock;

use crate::node::NodeMachine;
pub use crate::types::{NodeNetwork, NodePhase, NodeSnapshot};

static NODE: OnceLock<NodeMachine> = OnceLock::new();

pub(crate) fn node() -> Result<&'static NodeMachine, String> {
    NODE.get()
        .ok_or_else(|| "local node has not been initialized".to_owned())
}

/// Initializes the single local wallet node for this process. The secret
/// password comes from the host's platform secure storage and encrypts every
/// Fiber key file under the data directory.
pub fn init_local_node(
    network: NodeNetwork,
    data_dir: String,
    secret_password: String,
) -> NodeSnapshot {
    NODE.get_or_init(|| NodeMachine::new(network, data_dir, secret_password))
        .snapshot()
}

/// Returns the latest lifecycle snapshot without blocking.
pub fn local_node_snapshot() -> Result<NodeSnapshot, String> {
    node().map(NodeMachine::snapshot)
}

/// Starts the local node. Calling start while starting/running is a no-op.
pub async fn start_local_node() -> Result<NodeSnapshot, String> {
    node()?.start().await
}

/// Stops the local node. Calling stop while stopping/stopped is a no-op.
pub async fn stop_local_node() -> Result<NodeSnapshot, String> {
    node()?.stop().await
}

/// Marks an unrecoverable backend failure for display in the UI.
pub fn mark_local_node_failed(message: String) -> Result<NodeSnapshot, String> {
    node().map(|node| node.mark_failed(message))
}
