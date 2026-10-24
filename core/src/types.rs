//! Framework-agnostic types shared by the node backends and future hosts
//! (NSH app, recovery smoke, tests).

/// The execution environment shown by the wallet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeNetwork {
    Testnet,
    Mainnet,
}

/// The lifecycle state of the embedded local Fiber node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodePhase {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

/// A snapshot describing the node lifecycle for a host UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSnapshot {
    pub phase: NodePhase,
    pub network: NodeNetwork,
    pub detail: String,
    pub node_id: Option<String>,
    pub backend: String,
}

/// Node identity and on-chain funding info for the wallet home screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletNodeInfo {
    pub node_id: String,
    pub node_name: Option<String>,
    pub addresses: Vec<String>,
    pub version: String,
    pub funding_address: String,
    /// On-chain balance of the funding address, in shannons (decimal string).
    pub funding_balance_shannons: String,
    /// Sum of local balances across open channels, in shannons.
    pub channel_balance_shannons: String,
}

/// One channel of the local node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletChannel {
    pub channel_id: String,
    pub peer_id: String,
    pub state: String,
    pub is_inbound: bool,
    /// Shannons, decimal string.
    pub local_balance: String,
    /// Shannons, decimal string.
    pub remote_balance: String,
}

/// One payment record (inbound or outbound).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletPayment {
    pub payment_hash: String,
    pub status: String,
    pub failed_error: Option<String>,
    pub created_at_ms: u64,
    pub last_updated_at_ms: u64,
}

/// A freshly created payment request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletInvoice {
    /// The encoded invoice (share this string).
    pub invoice: String,
    pub payment_hash: String,
    /// Shannons, decimal string.
    pub amount: String,
    pub description: Option<String>,
}

/// One peer connected to the local node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletPeer {
    /// 33-byte compressed identity pubkey, hex.
    pub pubkey: String,
    /// The multiaddr this peer is connected on.
    pub address: String,
}

/// The temporary channel created by an open-channel request. It appears in
/// the channel list (with its permanent id) once funding completes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletOpenedChannel {
    pub temporary_channel_id: String,
    pub peer_id: String,
    /// Shannons, decimal string.
    pub funding_amount: String,
}

/// Manifest embedded in a wallet state export (`backup.rs`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WalletBackupManifest {
    pub format: u32,
    pub network: String,
    pub node_id: Option<String>,
    pub created_at_ms: u64,
}

/// Format helper: shannons to whole CKB.
pub fn shannons_to_ckb(shannons: &str) -> String {
    match shannons.parse::<u128>() {
        Ok(value) => (value / 100_000_000).to_string(),
        Err(_) => shannons.to_owned(),
    }
}

/// Format helper: whole CKB to shannons.
pub fn ckb_to_shannons(ckb: String) -> Result<String, String> {
    let value: f64 = ckb
        .trim()
        .parse()
        .map_err(|_| format!("invalid CKB amount: {ckb}"))?;
    if value < 0.0 {
        return Err(format!("negative CKB amount: {ckb}"));
    }
    let shannons = value * 100_000_000.0;
    if shannons.fract() != 0.0 || shannons > u64::MAX as f64 {
        return Err(format!("CKB amount out of range: {ckb}"));
    }
    Ok((shannons as u64).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_amounts() {
        assert_eq!(ckb_to_shannons("1".to_owned()).unwrap(), "100000000");
        assert_eq!(shannons_to_ckb("100000000"), "1");
        assert!(ckb_to_shannons("-1".to_owned()).is_err());
        assert!(ckb_to_shannons("abc".to_owned()).is_err());
    }
}
