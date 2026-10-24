//! Compatibility surface for the NSH shell: wallet operations against the
//! node initialized via [`super::node::init_local_node`]. Types are
//! re-exported from [`crate::types`]; new hosts should use that module
//! directly.

use crate::node::NodeMachine;

pub use crate::types::{
    ckb_to_shannons, shannons_to_ckb, WalletChannel, WalletInvoice, WalletNodeInfo,
    WalletOpenedChannel, WalletPayment, WalletPeer,
};

fn node() -> Result<&'static NodeMachine, String> {
    crate::api::node::node()
}

/// Node identity, funding address, and balances. Requires a running node.
pub async fn wallet_node_info() -> Result<WalletNodeInfo, String> {
    node()?.wallet_node_info().await
}

/// Channels of the local node. Requires a running node.
pub async fn wallet_list_channels() -> Result<Vec<WalletChannel>, String> {
    node()?.wallet_list_channels().await
}

/// Peers currently connected to the local node. Requires a running node.
pub async fn wallet_list_peers() -> Result<Vec<WalletPeer>, String> {
    node()?.wallet_list_peers().await
}

/// Connects to a peer by multiaddr (and remembers it). Requires a running
/// node; opening a channel needs the peer connected first.
pub async fn wallet_connect_peer(address: String) -> Result<(), String> {
    node()?.wallet_connect_peer(address).await
}

/// Opens a channel to a connected peer. Returns the temporary channel id;
/// the channel becomes usable once its funding transaction confirms.
pub async fn wallet_open_channel(
    peer_pubkey: String,
    funding_amount_shannons: u64,
    public_channel: bool,
) -> Result<WalletOpenedChannel, String> {
    node()?
        .wallet_open_channel(peer_pubkey, funding_amount_shannons, public_channel)
        .await
}

/// Creates a testnet invoice for `amount_shannons` of native CKB.
pub async fn wallet_new_invoice(
    amount_shannons: u64,
    description: String,
) -> Result<WalletInvoice, String> {
    node()?
        .wallet_new_invoice(amount_shannons, description)
        .await
}

/// Recent payments, newest first. Requires a running node.
pub async fn wallet_list_payments(limit: u32) -> Result<Vec<WalletPayment>, String> {
    node()?.wallet_list_payments(limit).await
}

/// Pays an encoded invoice. Returns the payment session; poll
/// [`wallet_list_payments`] or [`wallet_get_payment`] for the final status.
pub async fn wallet_send_payment(invoice: String) -> Result<WalletPayment, String> {
    node()?.wallet_send_payment(invoice).await
}

/// Looks up one payment by hash. Requires a running node.
pub async fn wallet_get_payment(payment_hash: String) -> Result<WalletPayment, String> {
    node()?.wallet_get_payment(payment_hash).await
}
