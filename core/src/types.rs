//! Framework-agnostic types shared by the node backends and future hosts
//! (NSH app, recovery smoke, tests).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Default payment timeout when the caller does not provide one.
pub const DEFAULT_PAYMENT_TIMEOUT_SECONDS: u64 = 60;
/// Upper bound accepted by the wallet API. Fiber itself has no comparable
/// cancellation guarantee, so keeping this bounded also bounds UI polling.
pub const MAX_PAYMENT_TIMEOUT_SECONDS: u64 = 24 * 60 * 60;
/// Fiber rejects native payments at or above `u64::MAX` shannons.
pub const MAX_NATIVE_PAYMENT_AMOUNT_SHANNONS: u128 = u64::MAX as u128 - 1;
/// A fee budget beyond the native amount range is never meaningful here.
pub const MAX_PAYMENT_FEE_SHANNONS: u128 = u64::MAX as u128 - 1;
/// Default host-enforced cap when a dApp omits `maxFeeShannons`.
pub const DEFAULT_DAPP_MAX_FEE_SHANNONS: u128 = 100_000_000;
/// Bound obviously hostile input before Fiber parses the invoice. The Fiber
/// invoice payload itself is capped at 16 KiB, so this leaves ample headroom
/// for bech32m framing.
pub const MAX_INVOICE_LENGTH: usize = 32 * 1024;

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
    /// Sum of local balances across open native-CKB channels, in shannons.
    ///
    /// This deliberately excludes UDT channels. Their units are not
    /// interchangeable with CKB and must be reported separately.
    pub channel_balance_shannons: String,
}

/// A chain transaction that has been accepted for broadcast. This does not
/// mean the transaction is confirmed; use [`WalletOnchainTxStatus`] for that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletOnchainTransfer {
    /// Normalized `0x`-prefixed transaction hash returned by the node.
    pub tx_hash: String,
    /// Normalized testnet address that received the output.
    pub target_address: String,
    pub amount_shannons: u64,
    /// Actual fee when it can be calculated before broadcast.
    pub fee_shannons: Option<u64>,
}

/// Canonical CKB transaction status shown to wallet users.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnchainTxState {
    Pending,
    Proposed,
    Committed,
    Rejected,
    Unknown,
}

impl OnchainTxState {
    /// Only a canonical-chain commit is a confirmed transfer.
    pub fn is_committed(self) -> bool {
        self == Self::Committed
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "已广播 / 待打包",
            Self::Proposed => "已提案 / 待链上提交",
            Self::Committed => "已上链确认",
            Self::Rejected => "已被拒绝",
            Self::Unknown => "节点尚未看到该交易",
        }
    }
}

/// The RPC's current view of an on-chain transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletOnchainTxStatus {
    pub tx_hash: String,
    pub state: OnchainTxState,
    pub block_number: Option<u64>,
    pub block_hash: Option<String>,
    pub reason: Option<String>,
}

/// Structured failures for the native CKB transfer path. No variant carries
/// key material, and the node's RPC/builder errors are kept out of key access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnchainTransferError {
    InvalidAddress {
        detail: String,
    },
    WrongNetwork {
        expected: String,
        actual: String,
    },
    InvalidAmount {
        detail: String,
    },
    AmountTooSmall {
        amount_shannons: u64,
        minimum_shannons: u64,
    },
    InvalidFeeLimit {
        detail: String,
    },
    InsufficientBalance {
        detail: String,
    },
    FeeLimitExceeded {
        fee_shannons: u64,
        max_fee_shannons: u64,
    },
    Rpc {
        operation: String,
        detail: String,
    },
    Build {
        detail: String,
    },
    /// The broadcast result is uncertain, so the expected hash is retained for
    /// status checks instead of claiming the transaction was not sent.
    Broadcast {
        tx_hash: String,
        detail: String,
    },
    InvalidTxHash {
        detail: String,
    },
    NodeNotRunning,
    Unavailable {
        detail: String,
    },
}

impl fmt::Display for OnchainTransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress { detail } => write!(f, "目标地址无效：{detail}"),
            Self::WrongNetwork { expected, actual } => {
                write!(f, "目标地址网络错误：需要 {expected}，实际为 {actual}")
            }
            Self::InvalidAmount { detail } => write!(f, "CKB 金额无效：{detail}"),
            Self::AmountTooSmall {
                amount_shannons,
                minimum_shannons,
            } => write!(
                f,
                "转账金额低于目标 cell 最低占用容量：{amount_shannons} < {minimum_shannons} shannons"
            ),
            Self::InvalidFeeLimit { detail } => write!(f, "手续费上限无效：{detail}"),
            Self::InsufficientBalance { detail } => write!(f, "Funding lock 余额不足：{detail}"),
            Self::FeeLimitExceeded {
                fee_shannons,
                max_fee_shannons,
            } => write!(
                f,
                "实际手续费超过上限：{fee_shannons} > {max_fee_shannons} shannons"
            ),
            Self::Rpc { operation, detail } => write!(f, "CKB RPC 错误（{operation}）：{detail}"),
            Self::Build { detail } => write!(f, "CKB 交易构建失败：{detail}"),
            Self::Broadcast { tx_hash, detail } => write!(
                f,
                "交易广播结果不确定（交易哈希 {tx_hash}）：{detail}。请查询链上状态确认"
            ),
            Self::InvalidTxHash { detail } => write!(f, "交易哈希无效：{detail}"),
            Self::NodeNotRunning => write!(f, "本地节点未运行，无法发起链上转账"),
            Self::Unavailable { detail } => write!(f, "链上转账不可用：{detail}"),
        }
    }
}

impl std::error::Error for OnchainTransferError {}

impl OnchainTransferError {
    /// Stable, UI-friendly category. Keep this mapping exhaustive so new
    /// variants cannot silently fall into a generic error.
    pub fn category(&self) -> &'static str {
        match self {
            Self::InvalidAddress { .. } => "invalid_address",
            Self::WrongNetwork { .. } => "wrong_network",
            Self::InvalidAmount { .. } | Self::AmountTooSmall { .. } => "invalid_amount",
            Self::InvalidFeeLimit { .. } | Self::FeeLimitExceeded { .. } => "invalid_fee",
            Self::InsufficientBalance { .. } => "insufficient_balance",
            Self::Rpc { .. } => "rpc_error",
            Self::Build { .. } => "build_error",
            Self::Broadcast { .. } => "broadcast_uncertain",
            Self::InvalidTxHash { .. } => "invalid_tx_hash",
            Self::NodeNotRunning => "node_not_running",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// The hash that may have reached the network. Only a broadcast error
    /// carries one; every other failure happened before bytes were sent.
    pub fn uncertain_tx_hash(&self) -> Option<&str> {
        match self {
            Self::Broadcast { tx_hash, .. } => Some(tx_hash),
            _ => None,
        }
    }
}

/// One channel of the local node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletChannel {
    pub channel_id: String,
    pub peer_id: String,
    /// FNN variant name, for example `ChannelReady` or `Closed`.
    pub state: String,
    /// Raw bit flags for the state variant, when FNN exposes flags.
    pub state_flags: Option<u32>,
    pub is_inbound: bool,
    /// Shannons, decimal string.
    pub local_balance: String,
    /// Shannons, decimal string.
    pub remote_balance: String,
    /// Asset funded by this channel. `None` means native CKB.
    pub funding_asset: WalletChannelAsset,
    /// The latest commitment transaction hash, when available.
    pub latest_commitment_transaction_hash: Option<String>,
    /// Hash of the closing transaction, when FNN has produced one.
    pub shutdown_transaction_hash: Option<String>,
    /// Why opening or closing failed, if FNN recorded a reason.
    pub failure_detail: Option<String>,
}

/// Explicit channel asset model. Amounts from different assets are never
/// summed or compared in the wallet core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalletChannelAsset {
    NativeCkb,
    /// Hex-encoded molecule bytes of the channel's UDT type script.
    Udt {
        type_script: String,
    },
}

impl WalletChannelAsset {
    pub fn is_native_ckb(&self) -> bool {
        matches!(self, Self::NativeCkb)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::NativeCkb => "CKB",
            Self::Udt { .. } => "UDT",
        }
    }
}

/// How a channel close should be attempted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChannelCloseMode {
    /// Ask the peer to cooperatively produce and sign a closing transaction.
    Cooperative,
    /// Broadcast the latest commitment transaction without peer cooperation.
    Force,
}

/// Parameters for closing one channel through the local FNN node.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChannelCloseRequest {
    /// 32-byte channel id as hex, with or without a `0x` prefix.
    pub channel_id: String,
    pub mode: ChannelCloseMode,
    /// Optional closing fee rate in shannons per 1000 weight units.
    ///
    /// `None` lets FNN use the channel's commitment fee rate. A fee rate is
    /// invalid for force closes because FNN ignores it and rejects requests
    /// that set both `force` and `fee_rate`.
    pub fee_rate: Option<u64>,
}

impl ChannelCloseRequest {
    /// A cooperative close using the wallet's default shutdown/funding script.
    pub fn cooperative(channel_id: impl Into<String>) -> Self {
        Self {
            channel_id: channel_id.into(),
            mode: ChannelCloseMode::Cooperative,
            fee_rate: None,
        }
    }

    /// A force close. FNN uses the channel's original default shutdown script.
    pub fn force(channel_id: impl Into<String>) -> Self {
        Self {
            channel_id: channel_id.into(),
            mode: ChannelCloseMode::Force,
            fee_rate: None,
        }
    }
}

/// The immediate result of an accepted channel-close RPC.
///
/// FNN accepts a close before the closing transaction is confirmed. `state`
/// is the state read back from FNN immediately after the RPC; callers must
/// poll the channel (or wait for the next refresh) to observe final closure.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WalletChannelCloseResult {
    pub channel_id: String,
    pub mode: ChannelCloseMode,
    /// FNN channel state, for example `ShuttingDown` or `Closed`.
    pub state: Option<String>,
    /// Raw flags for the returned state variant.
    pub state_flags: Option<u32>,
    pub shutdown_transaction_hash: Option<String>,
    pub latest_commitment_transaction_hash: Option<String>,
    pub failure_detail: Option<String>,
}

/// UI-safe lifecycle classification for a close request/result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelCloseStatus {
    /// FNN accepted the request, but no state was returned yet.
    Submitted,
    /// The cooperative handshake is still in progress.
    InProgress,
    /// The closing transaction was broadcast and is waiting for on-chain
    /// settlement.
    WaitingOnchainSettlement,
    /// The on-chain closing transaction is confirmed and the channel is
    /// fully closed/settled.
    Closed,
    /// The opening attempt was abandoned or its funding was aborted. This is
    /// not a successfully closed, funded channel.
    OpeningFailed,
    /// A local/remote close attempt failed.
    Failed,
    /// The channel is stuck in an intermediate state with a recorded failure.
    FailedWithDetail,
    /// A state this wallet does not understand.
    Unknown,
}

impl ChannelCloseStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Closed | Self::OpeningFailed | Self::Failed | Self::FailedWithDetail
        )
    }
}

/// Parses a 32-byte channel id, accepting an optional `0x` prefix.
pub fn parse_channel_id(channel_id: &str) -> Result<[u8; 32], String> {
    let value = channel_id.trim();
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if value.len() != 64 {
        return Err(format!(
            "channel id must be 32 bytes (64 hex characters), got {} characters",
            value.len()
        ));
    }
    let bytes = hex::decode(value).map_err(|error| format!("invalid channel id hex: {error}"))?;
    bytes
        .try_into()
        .map_err(|_| "channel id must be exactly 32 bytes".to_owned())
}

/// Validates close parameters before they reach FNN.
///
/// Channel-specific fee limits still have to be enforced by FNN. This function
/// catches malformed ids and parameter combinations that are unconditionally
/// invalid for the selected close mode.
pub fn validate_channel_close_request(request: &ChannelCloseRequest) -> Result<[u8; 32], String> {
    let channel_id = parse_channel_id(&request.channel_id)?;
    if let Some(fee_rate) = request.fee_rate {
        if fee_rate == 0 {
            return Err("closing fee rate must be greater than zero".to_owned());
        }
        if request.mode == ChannelCloseMode::Force {
            return Err(
                "fee rate cannot be set for a force close; FNN ignores it and rejects ambiguous requests"
                    .to_owned(),
            );
        }
    }
    Ok(channel_id)
}

/// Maps an FNN channel state to a close lifecycle suitable for display.
/// Whether a channel in this state is a candidate for a force close.
///
/// FNN permits force close from `ChannelReady`, from `ShuttingDown`, and from
/// `AwaitingChannelReady` once our ready message was sent and a latest
/// commitment transaction exists.
pub fn can_force_close(state: Option<&str>, flags: Option<u32>) -> bool {
    match state.map(str::trim).filter(|state| !state.is_empty()) {
        Some("ChannelReady") | Some("ShuttingDown") => true,
        Some("AwaitingChannelReady") => flags.is_some_and(|bits| bits & 1 != 0),
        _ => false,
    }
}

pub fn channel_close_status(
    state: Option<&str>,
    flags: Option<u32>,
    shutdown_transaction_hash: Option<&str>,
) -> ChannelCloseStatus {
    match state.map(str::trim).filter(|state| !state.is_empty()) {
        None => ChannelCloseStatus::Submitted,
        Some("NegotiatingFunding")
        | Some("CollaboratingFundingTx")
        | Some("SigningCommitment")
        | Some("AwaitingTxSignatures")
        | Some("AwaitingChannelReady")
        | Some("Stale") => ChannelCloseStatus::InProgress,
        Some("ChannelReady") => {
            if shutdown_transaction_hash.is_some() {
                // A close request may have advanced FNN's persisted state
                // between the close RPC and this list read.
                ChannelCloseStatus::InProgress
            } else {
                ChannelCloseStatus::Unknown
            }
        }
        Some("ShuttingDown") => ChannelCloseStatus::InProgress,
        Some("Closed") => match flags {
            Some(flags) if flags & (1 << 5) != 0 => ChannelCloseStatus::WaitingOnchainSettlement,
            Some(flags) if flags & (1 << 1) != 0 => ChannelCloseStatus::WaitingOnchainSettlement,
            Some(flags) if flags & (1 << 4) != 0 => ChannelCloseStatus::WaitingOnchainSettlement,
            Some(flags) if flags & ((1 << 2) | (1 << 3)) != 0 => ChannelCloseStatus::OpeningFailed,
            _ => ChannelCloseStatus::Closed,
        },
        Some("Failed") => ChannelCloseStatus::Failed,
        Some(_) => ChannelCloseStatus::Unknown,
    }
}

/// The payment state machine defined by Fiber:
/// `Created -> Inflight -> Success | Failed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalletPaymentStatus {
    Created,
    Inflight,
    Success,
    Failed,
}

/// Host-safe facts decoded from a Fiber invoice before a user approves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletInvoicePreview {
    pub payment_hash: String,
    pub amount_shannons: u64,
    pub description: Option<String>,
    pub payee: Option<String>,
    pub created_at_ms: u64,
    pub expires_at_ms: Option<u64>,
    pub network: String,
    pub asset: String,
}

impl WalletPaymentStatus {
    /// Only these states are terminal for a real payment session.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Success | Self::Failed)
    }

    pub fn is_success(self) -> bool {
        self == Self::Success
    }

    pub fn is_failed(self) -> bool {
        self == Self::Failed
    }
}

impl fmt::Display for WalletPaymentStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => formatter.write_str("Created"),
            Self::Inflight => formatter.write_str("Inflight"),
            Self::Success => formatter.write_str("Success"),
            Self::Failed => formatter.write_str("Failed"),
        }
    }
}

/// One payment record (inbound or outbound).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletPayment {
    pub payment_hash: String,
    pub status: WalletPaymentStatus,
    pub failed_error: Option<String>,
    /// Routing fee in shannons, decimal string.
    pub fee_shannons: String,
    pub created_at_ms: u64,
    pub last_updated_at_ms: u64,
}

impl WalletPayment {
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }
}

/// Native-CKB payment request. This API deliberately has no UDT/asset field:
/// paying a UDT invoice or performing a CKB/rUSDT exchange is out of scope and
/// must go through a real, separately reviewed adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletSendPaymentRequest {
    /// A signed Fiber invoice. The invoice fixes the payee, amount and asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invoice: Option<String>,
    /// Compressed secp256k1 pubkey (33 bytes, hex). Keysend only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_pubkey: Option<String>,
    /// Amount in shannons. Keysend only; invoices carry their own amount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<u128>,
    /// Maximum total routing fee in shannons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fee_amount: Option<u128>,
    /// Fiber payment timeout in seconds. Defaults to 60 seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Route and fee check only. A dry run never creates a payment record.
    #[serde(default)]
    pub dry_run: bool,
    /// Selects keysend. It must be false for invoice payments.
    #[serde(default)]
    pub keysend: bool,
}

impl WalletSendPaymentRequest {
    pub fn for_invoice(invoice: String) -> Self {
        Self {
            invoice: Some(invoice),
            ..Self::default()
        }
    }

    pub fn for_keysend(target_pubkey: String, amount: u128) -> Self {
        Self {
            target_pubkey: Some(target_pubkey),
            amount: Some(amount),
            keysend: true,
            ..Self::default()
        }
    }

    pub fn with_max_fee_amount(mut self, max_fee_amount: u128) -> Self {
        self.max_fee_amount = Some(max_fee_amount);
        self
    }

    pub fn with_timeout_seconds(mut self, timeout_seconds: u64) -> Self {
        self.timeout_seconds = Some(timeout_seconds);
        self
    }

    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    /// Structural validation shared by native and simulated backends.
    ///
    /// Fiber still performs authoritative invoice signature, expiry, currency
    /// and route validation. The native backend additionally rejects UDT
    /// invoices so this API cannot silently cross assets.
    pub fn validate(&self) -> Result<(), String> {
        match (&self.invoice, self.keysend) {
            (Some(_), true) => {
                return Err("invoice and keysend are mutually exclusive".to_owned());
            }
            (None, false) => {
                return Err(
                    "a payment request must contain an invoice or enable keysend".to_owned(),
                );
            }
            (None, true) => {
                let target = self
                    .target_pubkey
                    .as_deref()
                    .ok_or_else(|| "keysend requires target_pubkey".to_owned())?;
                validate_compressed_pubkey(target)?;
                let amount = self
                    .amount
                    .ok_or_else(|| "keysend requires amount".to_owned())?;
                validate_native_amount(amount)?;
            }
            (Some(invoice), false) => {
                let invoice = invoice.trim();
                if invoice.is_empty() {
                    return Err("invoice must not be empty".to_owned());
                }
                if invoice.len() > MAX_INVOICE_LENGTH {
                    return Err(format!(
                        "invoice exceeds the {MAX_INVOICE_LENGTH}-byte limit"
                    ));
                }
                if self.target_pubkey.is_some() || self.amount.is_some() {
                    return Err(
                        "invoice payments must not override target_pubkey or amount".to_owned()
                    );
                }
            }
        }

        if let Some(max_fee_amount) = self.max_fee_amount {
            if max_fee_amount > MAX_PAYMENT_FEE_SHANNONS {
                return Err(format!(
                    "max_fee_amount exceeds {MAX_PAYMENT_FEE_SHANNONS} shannons"
                ));
            }
        }
        if let Some(timeout) = self.timeout_seconds {
            if timeout == 0 {
                return Err("timeout_seconds must be greater than 0".to_owned());
            }
            if timeout > MAX_PAYMENT_TIMEOUT_SECONDS {
                return Err(format!(
                    "timeout_seconds exceeds {MAX_PAYMENT_TIMEOUT_SECONDS}"
                ));
            }
        }
        Ok(())
    }

    /// Validates and canonicalizes superficial fields before calling Fiber.
    pub fn into_validated(self) -> Result<Self, String> {
        self.validate()?;
        let mut request = self;
        request.invoice = request.invoice.map(|invoice| invoice.trim().to_owned());
        request.target_pubkey = request
            .target_pubkey
            .map(|pubkey| canonical_pubkey_hex(&pubkey));
        Ok(request)
    }

    pub fn effective_timeout_seconds(&self) -> u64 {
        self.timeout_seconds
            .unwrap_or(DEFAULT_PAYMENT_TIMEOUT_SECONDS)
    }

    /// Payment hash fixed by this request, when it is present without running
    /// another node. Keysend without an explicit hash gets one inside Fiber,
    /// so it is intentionally not recoverable here.
    pub fn known_payment_hash(&self) -> Option<String> {
        self.invoice.as_deref().and_then(|invoice| {
            invoice
                .parse::<fnn::invoice::CkbInvoice>()
                .ok()
                .map(|invoice| hex::encode(invoice.payment_hash().as_ref()))
        })
    }
}

/// A route/fee simulation result. It is intentionally not a
/// [`WalletPayment`] and is never pollable: Fiber does not persist dry runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletPaymentQuote {
    /// Fiber's dry-run correlation hash. For keysend this hash is ephemeral
    /// and has no stored payment session.
    pub quote_id: String,
    /// Amount in shannons, decimal string.
    pub amount_shannons: String,
    /// Estimated routing fee in shannons, decimal string.
    pub fee_shannons: String,
    /// Canonical request the quote was produced for. Sending must use exactly
    /// this request, not fields read back from mutable UI inputs.
    pub request: WalletSendPaymentRequest,
    pub created_at_ms: u64,
}

/// Distinguishes a dry-run quote from a real payment submission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WalletSendPaymentResult {
    Quote {
        quote: WalletPaymentQuote,
    },
    Submitted {
        /// The last payment record observed. The hash remains valid even when
        /// terminal polling failed.
        payment: WalletPayment,
        /// A polling/query error after FNN accepted the payment. This is not
        /// a send failure and the record must remain pollable by hash.
        terminal_error: Option<String>,
    },
}

impl WalletSendPaymentResult {
    pub fn is_quote(&self) -> bool {
        matches!(self, Self::Quote { .. })
    }
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

/// Format a shannons value as an exact decimal CKB string.
pub fn shannons_to_ckb(shannons: &str) -> String {
    match shannons.parse::<u128>() {
        Ok(value) => format_shannons(value),
        Err(_) => shannons.to_owned(),
    }
}

fn format_shannons(value: u128) -> String {
    let whole = value / 100_000_000;
    let fraction = format!("{:08}", value % 100_000_000);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{fraction}")
    }
}

/// Parse a decimal CKB amount without floating-point rounding. The function
/// accepts at most eight fractional digits and rejects signs, exponents,
/// empty components, and values that overflow `u64` shannons.
pub fn parse_ckb_amount(ckb: &str) -> Result<u64, OnchainTransferError> {
    let input = ckb.trim();
    if input.is_empty() {
        return Err(OnchainTransferError::InvalidAmount {
            detail: "金额为空".to_owned(),
        });
    }
    if input.starts_with('-') {
        return Err(OnchainTransferError::InvalidAmount {
            detail: "金额不能为负数".to_owned(),
        });
    }
    if input.contains(['e', 'E']) {
        return Err(OnchainTransferError::InvalidAmount {
            detail: "不支持科学计数法".to_owned(),
        });
    }
    if input.starts_with('+') {
        return Err(OnchainTransferError::InvalidAmount {
            detail: "金额不能以加号开头".to_owned(),
        });
    }

    let mut parts = input.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some() {
        return Err(OnchainTransferError::InvalidAmount {
            detail: "金额只能包含一个小数点".to_owned(),
        });
    }
    if whole.is_empty() && fraction.is_none_or(str::is_empty) {
        return Err(OnchainTransferError::InvalidAmount {
            detail: "金额必须包含数字".to_owned(),
        });
    }
    if !whole.is_empty() && !whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(OnchainTransferError::InvalidAmount {
            detail: "金额只能包含十进制数字".to_owned(),
        });
    }
    let fraction = fraction.unwrap_or_default();
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(OnchainTransferError::InvalidAmount {
            detail: "小数部分只能包含十进制数字".to_owned(),
        });
    }
    if fraction.len() > 8 {
        return Err(OnchainTransferError::InvalidAmount {
            detail: "CKB 金额最多支持 8 位小数".to_owned(),
        });
    }

    let whole = if whole.is_empty() { "0" } else { whole };
    let whole = whole
        .parse::<u64>()
        .map_err(|_| OnchainTransferError::InvalidAmount {
            detail: "金额超出 u64 范围".to_owned(),
        })?;
    let whole_shannons =
        whole
            .checked_mul(100_000_000)
            .ok_or_else(|| OnchainTransferError::InvalidAmount {
                detail: "金额超出 u64 shannons 范围".to_owned(),
            })?;
    let mut padded_fraction = fraction.to_owned();
    padded_fraction.extend(std::iter::repeat_n('0', 8 - fraction.len()));
    let fractional_shannons = if padded_fraction.is_empty() {
        0
    } else {
        padded_fraction
            .parse::<u64>()
            .map_err(|_| OnchainTransferError::InvalidAmount {
                detail: "小数部分超出范围".to_owned(),
            })?
    };
    whole_shannons
        .checked_add(fractional_shannons)
        .ok_or_else(|| OnchainTransferError::InvalidAmount {
            detail: "金额超出 u64 shannons 范围".to_owned(),
        })
}

/// Format helper: CKB to shannons. Kept as a string for the compatibility API.
pub fn ckb_to_shannons(ckb: String) -> Result<String, String> {
    parse_ckb_amount(&ckb)
        .map(|value| value.to_string())
        .map_err(|error| error.to_string())
}

/// Normalize and validate a 32-byte on-chain transaction hash.
pub fn parse_onchain_tx_hash(input: &str) -> Result<String, OnchainTransferError> {
    let input = input.trim();
    let hex = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
        .unwrap_or(input);
    if hex.len() != 64 {
        return Err(OnchainTransferError::InvalidTxHash {
            detail: "交易哈希必须是 32 字节（64 个 hex 字符）".to_owned(),
        });
    }
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(OnchainTransferError::InvalidTxHash {
            detail: "交易哈希不是合法 hex".to_owned(),
        });
    }
    Ok(format!("0x{}", hex.to_ascii_lowercase()))
}

fn minimum_onchain_fee_limit_from_env() -> Result<u64, OnchainTransferError> {
    let raw = std::env::var("CKB_ONCHAIN_MIN_FEE_SHANNONS").unwrap_or_else(|_| "1000".to_owned());
    let value =
        raw.trim()
            .parse::<u64>()
            .map_err(|error| OnchainTransferError::InvalidFeeLimit {
                detail: format!("CKB_ONCHAIN_MIN_FEE_SHANNONS 不是合法 u64: {error}"),
            })?;
    if value == 0 {
        return Err(OnchainTransferError::InvalidFeeLimit {
            detail: "CKB_ONCHAIN_MIN_FEE_SHANNONS 必须大于 0".to_owned(),
        });
    }
    Ok(value)
}

/// Validate the user's fee cap against both a positive floor and the
/// process-level hard ceiling. The hard ceiling is deliberately not a UI
/// constant: operators can lower it without rebuilding the app.
pub fn validate_onchain_fee_limit(max_fee_shannons: u64) -> Result<(), OnchainTransferError> {
    let hard_limit = std::env::var("CKB_ONCHAIN_MAX_FEE_SHANNONS")
        .ok()
        .map(|raw| {
            raw.trim()
                .parse::<u64>()
                .map_err(|error| OnchainTransferError::InvalidFeeLimit {
                    detail: format!("CKB_ONCHAIN_MAX_FEE_SHANNONS 不是合法 u64: {error}"),
                })
        })
        .transpose()?
        .unwrap_or(10_000_000);
    if max_fee_shannons == 0 {
        return Err(OnchainTransferError::InvalidFeeLimit {
            detail: "手续费上限必须大于 0".to_owned(),
        });
    }
    if max_fee_shannons > hard_limit {
        return Err(OnchainTransferError::InvalidFeeLimit {
            detail: format!("手续费上限超过硬上限 {hard_limit} shannons"),
        });
    }
    if max_fee_shannons < minimum_onchain_fee_limit_from_env()? {
        return Err(OnchainTransferError::InvalidFeeLimit {
            detail: "手续费上限过低，交易可能无法被打包".to_owned(),
        });
    }
    Ok(())
}

fn validate_native_amount(amount: u128) -> Result<(), String> {
    if amount == 0 {
        return Err("amount must be greater than 0".to_owned());
    }
    if amount > MAX_NATIVE_PAYMENT_AMOUNT_SHANNONS {
        return Err(format!(
            "amount exceeds the native CKB limit of {MAX_NATIVE_PAYMENT_AMOUNT_SHANNONS} shannons"
        ));
    }
    Ok(())
}

fn canonical_pubkey_hex(pubkey: &str) -> String {
    pubkey
        .trim()
        .strip_prefix("0x")
        .or_else(|| pubkey.trim().strip_prefix("0X"))
        .unwrap_or_else(|| pubkey.trim())
        .to_ascii_lowercase()
}

fn validate_compressed_pubkey(pubkey: &str) -> Result<(), String> {
    let canonical = canonical_pubkey_hex(pubkey);
    let bytes = hex::decode(&canonical)
        .map_err(|error| format!("target_pubkey is not valid hex: {error}"))?;
    if bytes.len() != 33 {
        return Err(format!(
            "target_pubkey must be 33 bytes (compressed), got {}",
            bytes.len()
        ));
    }
    secp256k1::PublicKey::from_slice(&bytes)
        .map_err(|error| format!("target_pubkey is not a valid secp256k1 key: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_PUBKEY: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    #[test]
    fn converts_amounts() {
        assert_eq!(ckb_to_shannons("1".to_owned()).unwrap(), "100000000");
        assert_eq!(ckb_to_shannons("0.00000001".to_owned()).unwrap(), "1");
        assert_eq!(
            ckb_to_shannons("184467440737.09551615".to_owned()).unwrap(),
            u64::MAX.to_string()
        );
        assert_eq!(shannons_to_ckb("100000000"), "1");
        assert_eq!(shannons_to_ckb("100000001"), "1.00000001");
        assert!(ckb_to_shannons("-1".to_owned()).is_err());
        assert!(ckb_to_shannons("abc".to_owned()).is_err());
    }

    #[test]
    fn parses_strict_decimal_amounts() {
        assert_eq!(parse_ckb_amount("1").unwrap(), 100_000_000);
        assert_eq!(parse_ckb_amount(" 1.5 ").unwrap(), 150_000_000);
        assert_eq!(parse_ckb_amount(".5").unwrap(), 50_000_000);
        assert_eq!(parse_ckb_amount("1.").unwrap(), 100_000_000);
        assert_eq!(parse_ckb_amount("0").unwrap(), 0);

        assert!(parse_ckb_amount("").is_err());
        assert!(parse_ckb_amount("-0.1").is_err());
        assert!(parse_ckb_amount("+1").is_err());
        assert!(parse_ckb_amount("1e-8").is_err());
        assert!(parse_ckb_amount("1.000000001").is_err());
        assert!(parse_ckb_amount("1.2.3").is_err());
        assert!(parse_ckb_amount("184467440737.09551616").is_err());
        assert!(parse_ckb_amount("184467440738").is_err());
    }

    #[test]
    fn validates_onchain_hashes_and_fee_caps() {
        let hash = "AA".repeat(32);
        assert_eq!(
            parse_onchain_tx_hash(&hash).unwrap(),
            format!("0x{}", "aa".repeat(32))
        );
        assert!(parse_onchain_tx_hash("0x1234").is_err());
        assert!(parse_onchain_tx_hash(&"zz".repeat(32)).is_err());

        assert!(validate_onchain_fee_limit(0).is_err());
        assert!(validate_onchain_fee_limit(1000).is_ok());
        assert!(validate_onchain_fee_limit(10_000_001).is_err());
    }

    #[test]
    fn invoice_and_keysend_are_mutually_exclusive() {
        let request = WalletSendPaymentRequest {
            invoice: Some("fibt1000invoice".to_owned()),
            keysend: true,
            target_pubkey: Some(VALID_PUBKEY.to_owned()),
            amount: Some(1),
            ..WalletSendPaymentRequest::default()
        };

        assert_eq!(
            request.validate().unwrap_err(),
            "invoice and keysend are mutually exclusive"
        );
    }

    #[test]
    fn keysend_requires_a_valid_pubkey_and_positive_amount() {
        let missing_target = WalletSendPaymentRequest {
            keysend: true,
            amount: Some(1),
            ..WalletSendPaymentRequest::default()
        };
        assert_eq!(
            missing_target.validate().unwrap_err(),
            "keysend requires target_pubkey"
        );

        let missing_amount =
            WalletSendPaymentRequest::for_keysend(VALID_PUBKEY.to_owned(), 0).with_dry_run(true);
        assert_eq!(
            missing_amount.validate().unwrap_err(),
            "amount must be greater than 0"
        );

        let bad_hex = WalletSendPaymentRequest::for_keysend("zz".repeat(33), 1);
        assert!(bad_hex.validate().unwrap_err().contains("not valid hex"));

        let bad_length = WalletSendPaymentRequest::for_keysend(VALID_PUBKEY[2..].to_owned(), 1);
        assert!(bad_length
            .validate()
            .unwrap_err()
            .contains("must be 33 bytes"));

        let invalid_key =
            WalletSendPaymentRequest::for_keysend(format!("02{}", "00".repeat(32)), 1);
        assert!(invalid_key
            .validate()
            .unwrap_err()
            .contains("not a valid secp256k1 key"));
    }

    #[test]
    fn invoice_cannot_override_payee_or_amount() {
        let request = WalletSendPaymentRequest {
            invoice: Some("  fibt1000invoice  ".to_owned()),
            amount: Some(10),
            ..WalletSendPaymentRequest::default()
        };
        assert!(request
            .validate()
            .unwrap_err()
            .contains("must not override"));

        let normalized = WalletSendPaymentRequest::for_invoice("  fibt1000invoice  ".to_owned())
            .into_validated()
            .unwrap();
        assert_eq!(normalized.invoice.as_deref(), Some("fibt1000invoice"));
    }

    #[test]
    fn enforces_amount_fee_and_timeout_bounds() {
        let max_amount =
            WalletSendPaymentRequest::for_keysend(VALID_PUBKEY.to_owned(), u64::MAX as u128);
        assert!(max_amount
            .validate()
            .unwrap_err()
            .contains("native CKB limit"));

        let max_fee = WalletSendPaymentRequest::for_keysend(VALID_PUBKEY.to_owned(), 1)
            .with_max_fee_amount(u64::MAX as u128);
        assert!(max_fee.validate().unwrap_err().contains("max_fee_amount"));

        let zero_timeout = WalletSendPaymentRequest::for_keysend(VALID_PUBKEY.to_owned(), 1)
            .with_timeout_seconds(0);
        assert!(zero_timeout
            .validate()
            .unwrap_err()
            .contains("greater than 0"));

        let long_timeout = WalletSendPaymentRequest::for_keysend(VALID_PUBKEY.to_owned(), 1)
            .with_timeout_seconds(MAX_PAYMENT_TIMEOUT_SECONDS + 1);
        assert!(long_timeout.validate().unwrap_err().contains("exceeds"));

        let valid = WalletSendPaymentRequest::for_keysend(
            format!("0X{}", VALID_PUBKEY.to_ascii_uppercase()),
            1,
        )
        .with_max_fee_amount(0)
        .with_timeout_seconds(MAX_PAYMENT_TIMEOUT_SECONDS)
        .into_validated()
        .unwrap();
        assert_eq!(valid.target_pubkey.as_deref(), Some(VALID_PUBKEY));
        assert_eq!(
            valid.effective_timeout_seconds(),
            MAX_PAYMENT_TIMEOUT_SECONDS
        );
    }

    #[test]
    fn payment_status_is_terminal_only_after_success_or_failure() {
        assert!(!WalletPaymentStatus::Created.is_terminal());
        assert!(!WalletPaymentStatus::Inflight.is_terminal());
        assert!(WalletPaymentStatus::Success.is_terminal());
        assert!(WalletPaymentStatus::Failed.is_terminal());

        let payment = WalletPayment {
            payment_hash: "0x01".repeat(32),
            status: WalletPaymentStatus::Inflight,
            failed_error: None,
            fee_shannons: "0".to_owned(),
            created_at_ms: 1,
            last_updated_at_ms: 2,
        };
        assert!(!payment.is_terminal());
    }

    #[test]
    fn dry_run_result_is_serialized_as_a_quote() {
        let result = WalletSendPaymentResult::Quote {
            quote: WalletPaymentQuote {
                quote_id: "0x01".repeat(32),
                amount_shannons: "1000".to_owned(),
                fee_shannons: "5".to_owned(),
                request: WalletSendPaymentRequest::for_keysend(VALID_PUBKEY.to_owned(), 1_000)
                    .with_max_fee_amount(5)
                    .with_dry_run(true),
                created_at_ms: 42,
            },
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["kind"], "quote");
        assert_eq!(json["quote"]["fee_shannons"], "5");
        assert!(json.get("payment").is_none());
    }

    #[test]
    fn request_round_trips_through_serde() {
        let request = WalletSendPaymentRequest::for_keysend(VALID_PUBKEY.to_owned(), 42)
            .with_dry_run(true)
            .with_max_fee_amount(7)
            .with_timeout_seconds(30);
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: WalletSendPaymentRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn invalid_invoice_has_no_recovery_hash() {
        assert_eq!(
            WalletSendPaymentRequest::for_invoice("not-an-invoice".to_owned()).known_payment_hash(),
            None
        );
        assert_eq!(
            WalletSendPaymentRequest::for_keysend(VALID_PUBKEY.to_owned(), 1).known_payment_hash(),
            None
        );
    }

    #[test]
    fn parses_and_validates_channel_close_requests() {
        let channel_id = "ab".repeat(32);
        let request = ChannelCloseRequest::cooperative(format!("0x{channel_id}"));
        assert_eq!(
            validate_channel_close_request(&request).unwrap(),
            [0xab; 32]
        );

        let mut with_fee = request.clone();
        with_fee.fee_rate = Some(1_000);
        assert!(validate_channel_close_request(&with_fee).is_ok());

        let force = ChannelCloseRequest::force(channel_id);
        assert!(validate_channel_close_request(&force).is_ok());

        let mut force_with_fee = force.clone();
        force_with_fee.fee_rate = Some(1_000);
        assert!(validate_channel_close_request(&force_with_fee).is_err());

        let mut zero_fee = request;
        zero_fee.fee_rate = Some(0);
        assert!(validate_channel_close_request(&zero_fee).is_err());
        assert!(parse_channel_id("not-a-channel").is_err());
        assert!(parse_channel_id(&"ab".repeat(31)).is_err());
    }

    #[test]
    fn classifies_channel_close_states() {
        assert_eq!(
            channel_close_status(None, None, None),
            ChannelCloseStatus::Submitted
        );
        assert_eq!(
            channel_close_status(Some("ShuttingDown"), Some(1), None),
            ChannelCloseStatus::InProgress
        );
        assert_eq!(
            channel_close_status(Some("Closed"), Some(1), None),
            ChannelCloseStatus::Closed
        );
        assert_eq!(
            channel_close_status(Some("Closed"), Some(1 << 5), Some("0xtx")),
            ChannelCloseStatus::WaitingOnchainSettlement
        );
        assert_eq!(
            channel_close_status(Some("Closed"), Some(1 << 3), None),
            ChannelCloseStatus::OpeningFailed
        );
        assert_eq!(
            channel_close_status(Some("Failed"), None, None),
            ChannelCloseStatus::Failed
        );
        assert_eq!(
            channel_close_status(Some("future-state"), None, None),
            ChannelCloseStatus::Unknown
        );
        assert!(ChannelCloseStatus::Closed.is_terminal());
        assert!(ChannelCloseStatus::OpeningFailed.is_terminal());
        assert!(!ChannelCloseStatus::WaitingOnchainSettlement.is_terminal());
        assert!(!ChannelCloseStatus::InProgress.is_terminal());
        assert!(can_force_close(Some("ChannelReady"), None));
        assert!(can_force_close(Some("AwaitingChannelReady"), Some(1)));
        assert!(!can_force_close(Some("AwaitingChannelReady"), Some(2)));
        assert!(!can_force_close(Some("NegotiatingFunding"), Some(1)));
    }
}
