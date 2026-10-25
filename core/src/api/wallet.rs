//! Compatibility surface for the NSH shell: wallet operations against the
//! node initialized via [`super::node::init_local_node`]. Types are
//! re-exported from [`crate::types`]; new hosts should use that module
//! directly.

use crate::node::NodeMachine;

#[cfg(not(target_arch = "wasm32"))]
pub use crate::onchain::{parse_testnet_address, parse_tx_hash};
pub use crate::types::{
    can_force_close, channel_close_status, ckb_to_shannons, parse_channel_id, parse_ckb_amount,
    shannons_to_ckb, validate_channel_close_request, validate_onchain_fee_limit, ChannelCloseMode,
    ChannelCloseRequest, ChannelCloseStatus, OnchainTransferError, OnchainTxState, WalletChannel,
    WalletChannelAsset, WalletChannelCloseResult, WalletInvoice, WalletInvoicePreview,
    WalletNodeInfo, WalletOnchainTransfer, WalletOnchainTxStatus, WalletOpenedChannel,
    WalletPayment, WalletPaymentQuote, WalletPaymentStatus, WalletPeer, WalletSendPaymentRequest,
    WalletSendPaymentResult, DEFAULT_DAPP_MAX_FEE_SHANNONS,
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

/// Requests a cooperative or force close and returns FNN's immediate result.
/// Poll [`wallet_list_channels`] until the channel reports `Closed`.
pub async fn wallet_close_channel(
    request: ChannelCloseRequest,
) -> Result<WalletChannelCloseResult, String> {
    node()?.wallet_close_channel(request).await
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

/// Decodes a signed Fiber invoice without contacting the node or moving
/// funds. The host uses this before showing an approval dialog.
pub fn wallet_inspect_invoice(invoice: &str, now_ms: u64) -> Result<WalletInvoicePreview, String> {
    let parsed = invoice
        .parse::<fnn::invoice::CkbInvoice>()
        .map_err(|error| format!("invalid Fiber invoice: {error}"))?;
    if parsed.currency != fnn::invoice::Currency::Fibt {
        return Err("invoice is not for the embedded Fiber testnet".to_owned());
    }
    if parsed.udt_type_script().is_some() {
        return Err(
            "UDT invoices are not supported; this provider pays native CKB only".to_owned(),
        );
    }
    let amount = parsed
        .amount()
        .ok_or_else(|| "invoice has no amount; refusing an unbounded payment".to_owned())?;
    let amount_shannons =
        u64::try_from(amount).map_err(|_| "invoice amount exceeds u64 shannons".to_owned())?;
    if amount_shannons == 0 {
        return Err("invoice amount must be greater than zero".to_owned());
    }
    let created_at_ms = u64::try_from(parsed.data.timestamp)
        .map_err(|_| "invoice timestamp is out of range".to_owned())?;
    let expires_at_ms = parsed.expiry_time().and_then(|duration| {
        u64::try_from(duration.as_millis())
            .ok()
            .and_then(|duration_ms| created_at_ms.checked_add(duration_ms))
    });
    if expires_at_ms.is_some_and(|expires_at| now_ms >= expires_at) {
        return Err("invoice has expired".to_owned());
    }
    let payee = parsed.payee_pub_key().map(ToString::to_string).or_else(|| {
        parsed
            .recover_payee_pub_key()
            .ok()
            .map(|key| key.to_string())
    });
    Ok(WalletInvoicePreview {
        payment_hash: hex::encode(parsed.payment_hash().as_ref()),
        amount_shannons,
        description: parsed.description().cloned(),
        payee,
        created_at_ms,
        expires_at_ms,
        network: "Fiber Testnet".to_owned(),
        asset: "native CKB".to_owned(),
    })
}

/// Sends a native-CKB invoice or keysend payment, or obtains a dry-run quote.
///
/// Real payments wait for Fiber's terminal `Success`/`Failed` state within the
/// request timeout. Dry runs return [`WalletSendPaymentResult::Quote`] and do
/// not create a payment record. UDT invoices are rejected.
pub async fn wallet_send_payment_request(
    request: WalletSendPaymentRequest,
) -> Result<WalletSendPaymentResult, String> {
    node()?.wallet_send_payment_request(request).await
}

/// Waits for an existing payment to become `Success` or `Failed`.
///
/// A timeout does not mean the payment failed. Use [`wallet_get_payment`] to
/// keep checking after this returns an error.
pub async fn wallet_poll_payment_terminal(
    payment_hash: String,
    timeout_seconds: u64,
) -> Result<WalletPayment, String> {
    if timeout_seconds == 0 || timeout_seconds > crate::types::MAX_PAYMENT_TIMEOUT_SECONDS {
        return Err(format!(
            "timeout_seconds must be between 1 and {}",
            crate::types::MAX_PAYMENT_TIMEOUT_SECONDS
        ));
    }
    node()?
        .wallet_poll_payment_terminal(
            payment_hash,
            None,
            std::time::Duration::from_secs(timeout_seconds),
        )
        .await
}

/// Looks up one payment by hash. Requires a running node.
pub async fn wallet_get_payment(payment_hash: String) -> Result<WalletPayment, String> {
    node()?.wallet_get_payment(payment_hash).await
}

/// Broadcasts a native CKB transfer from the wallet's funding lock to a
/// testnet address. A successful result means "accepted for broadcast", not
/// "confirmed"; poll [`wallet_onchain_transfer_status`] with the hash.
#[cfg(not(target_arch = "wasm32"))]
pub async fn wallet_onchain_transfer(
    target_address: String,
    amount_ckb: String,
    max_fee_ckb: String,
) -> Result<WalletOnchainTransfer, String> {
    wallet_onchain_transfer_result(target_address, amount_ckb, max_fee_ckb)
        .await
        .map_err(|error| error.to_string())
}

/// Typed variant of [`wallet_onchain_transfer`].
///
/// A [`OnchainTransferError::Broadcast`] retains the hash whose broadcast
/// outcome is uncertain; hosts must query the status before retrying.
#[cfg(not(target_arch = "wasm32"))]
pub async fn wallet_onchain_transfer_result(
    target_address: String,
    amount_ckb: String,
    max_fee_ckb: String,
) -> Result<WalletOnchainTransfer, OnchainTransferError> {
    let amount_shannons = parse_ckb_amount(&amount_ckb)?;
    let max_fee_shannons = parse_ckb_amount(&max_fee_ckb)?;
    validate_onchain_fee_limit(max_fee_shannons)?;
    let node = node().map_err(|detail| OnchainTransferError::Unavailable { detail })?;
    node.wallet_onchain_transfer(target_address, amount_shannons, max_fee_shannons)
        .await
}

/// Queries the canonical pending/proposed/committed/rejected state of a
/// previously broadcast testnet transaction without requiring the local node.
pub async fn wallet_onchain_transfer_status(
    tx_hash: String,
) -> Result<WalletOnchainTxStatus, String> {
    crate::onchain::transaction_status(&tx_hash, crate::onchain::TESTNET_CKB_RPC_URL)
        .await
        .map_err(|error| error.to_string())
}
