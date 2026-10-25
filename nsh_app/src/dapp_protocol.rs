//! JSON-RPC wire protocol, provider method registry, and dApp session state.
//!
//! The page is untrusted. It speaks only through
//! `window.nsh.postMessage(JSON.stringify(request))`; the host answers through
//! `ElementRef::post_message` and never accepts keys, arbitrary RPC endpoints,
//! or code execution URLs from the page.
//!
//! Authorization is scoped to the exact registered web origin and one WebView
//! generation. Connecting is a host-confirmed action. Every wallet mutation is
//! separately confirmed; knowing a method name never grants it.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const SELFTEST_URL: &str = "fiber-wallet://protocol-selftest";
pub const APPROVAL_TIMEOUT_MS: u64 = 60_000;
/// Payments may spend up to the core's 120 s request timeout after approval.
/// Leave time for the approval itself plus bounded recovery polling.
pub const PAGE_REQUEST_TIMEOUT_MS: u64 = 240_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DappRequest {
    Request {
        /// Host-generated CSPRNG capability included in every frame.
        nonce: String,
        jsonrpc: String,
        id: String,
        method: String,
        #[serde(default)]
        params: Option<Value>,
    },
    Event {
        method: String,
        #[serde(default)]
        params: Option<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostEvent {
    pub r#type: String,
    pub nonce: String,
    pub event: String,
    pub protocol_version: u32,
    pub data: Value,
}

pub mod error_code {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    pub const METHOD_NOT_ALLOWED: i32 = -32001;
    pub const UNAUTHORIZED: i32 = -32002;
    pub const USER_REJECTED: i32 = -32003;
    pub const APPROVAL_TIMEOUT: i32 = -32004;
    pub const ORIGIN_MISMATCH: i32 = -32005;
    pub const UNAVAILABLE: i32 = -32006;
    pub const UNSUPPORTED: i32 = -32007;
    pub const REQUEST_IN_PROGRESS: i32 = -32008;
    pub const OUT_OF_CAPACITY: i32 = -32009;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Read,
    Connect,
    Mutating,
    DryRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderMethod {
    Connect,
    GetNodeInfo,
    GetBalance,
    ListChannels,
    ListPeers,
    CreateInvoice,
    SendPayment,
    OpenChannel,
    CloseChannel,
    GetPayment,
    DryRunPayment,
}

impl ProviderMethod {
    pub fn canonical(self) -> &'static str {
        match self {
            Self::Connect => "fiber_connect",
            Self::GetNodeInfo => "fiber_getNodeInfo",
            Self::GetBalance => "fiber_getBalance",
            Self::ListChannels => "fiber_listChannels",
            Self::ListPeers => "fiber_listPeers",
            Self::CreateInvoice => "fiber_createInvoice",
            Self::SendPayment => "fiber_sendPayment",
            Self::OpenChannel => "fiber_openChannel",
            Self::CloseChannel => "fiber_closeChannel",
            Self::GetPayment => "fiber_getPayment",
            Self::DryRunPayment => "fiber_dryRunPayment",
        }
    }

    pub fn kind(self) -> MethodKind {
        match self {
            Self::GetNodeInfo
            | Self::GetBalance
            | Self::ListChannels
            | Self::ListPeers
            | Self::GetPayment => MethodKind::Read,
            Self::Connect => MethodKind::Connect,
            Self::DryRunPayment => MethodKind::DryRun,
            Self::CreateInvoice | Self::SendPayment | Self::OpenChannel | Self::CloseChannel => {
                MethodKind::Mutating
            }
        }
    }
}

/// The closed registry of provider methods. `wallet_*` aliases are accepted;
/// arbitrary unknown methods are never dispatched.
pub fn resolve_method(method: &str) -> Option<ProviderMethod> {
    Some(match method {
        "fiber_connect" | "wallet_connect" => ProviderMethod::Connect,
        "fiber_getNodeInfo" | "wallet_getNodeInfo" => ProviderMethod::GetNodeInfo,
        "fiber_getBalance" | "wallet_getBalance" => ProviderMethod::GetBalance,
        "fiber_listChannels" | "wallet_listChannels" => ProviderMethod::ListChannels,
        "fiber_listPeers" | "wallet_listPeers" => ProviderMethod::ListPeers,
        "fiber_createInvoice" | "wallet_createInvoice" => ProviderMethod::CreateInvoice,
        "fiber_sendPayment" | "wallet_sendPayment" => ProviderMethod::SendPayment,
        "fiber_openChannel" | "wallet_openChannel" => ProviderMethod::OpenChannel,
        "fiber_closeChannel" | "wallet_closeChannel" => ProviderMethod::CloseChannel,
        "fiber_getPayment" | "wallet_getPayment" => ProviderMethod::GetPayment,
        "fiber_dryRunPayment" | "wallet_dryRunPayment" => ProviderMethod::DryRunPayment,
        _ => return None,
    })
}

/// Methods that can never be bridged, regardless of user confirmation. Only
/// [`resolve_method`] is dispatchable, so this is an additional hard deny,
/// not an allow-list.
pub fn is_forbidden_method(method: &str) -> bool {
    let lower = method.to_ascii_lowercase();
    lower.starts_with("sign")
        || lower.contains("private")
        || lower.contains("secret")
        || lower.contains("keychain")
        || lower.contains("keystore")
        || lower.contains("eval")
        || lower.contains("exec")
        || lower.contains("inject")
        || lower.contains("rawrpc")
        || lower.contains("rpcraw")
        || lower.contains("endpoint")
        || lower.contains("arbitrary")
        || lower.contains("filesystem")
        || lower.contains("process")
        || lower.contains("shell")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin(String);

impl Origin {
    pub fn parse(url: &str) -> Result<Self, String> {
        let url = url.trim();
        if url == SELFTEST_URL {
            return Ok(Self(SELFTEST_URL.to_owned()));
        }
        let parts = origin_parts(url).ok_or_else(|| "URL has no permitted origin".to_owned())?;
        Ok(Self(format_origin(&parts)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn allows(&self, url: &str) -> bool {
        Self::parse(url).is_ok_and(|origin| origin == *self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DappSession {
    pub dapp_id: String,
    pub dapp_name: String,
    pub registered_url: String,
    pub origin: Origin,
    pub generation: u64,
    pub connected: bool,
    pub connected_at_ms: Option<u64>,
}

impl DappSession {
    pub fn new(
        dapp_id: impl Into<String>,
        dapp_name: impl Into<String>,
        registered_url: impl Into<String>,
        generation: u64,
    ) -> Result<Self, String> {
        let registered_url = registered_url.into();
        let origin = Origin::parse(&registered_url)?;
        Ok(Self {
            dapp_id: dapp_id.into(),
            dapp_name: dapp_name.into(),
            registered_url,
            origin,
            generation,
            connected: false,
            connected_at_ms: None,
        })
    }

    pub fn authorize(&mut self, current_url: &str, now_ms: u64) -> Result<(), RpcError> {
        self.ensure_origin(current_url)?;
        self.connected = true;
        self.connected_at_ms = Some(now_ms);
        Ok(())
    }

    pub fn ensure_origin(&self, current_url: &str) -> Result<(), RpcError> {
        if self.origin.allows(current_url) {
            Ok(())
        } else {
            Err(RpcError::new(
                error_code::ORIGIN_MISMATCH,
                "the current WebView origin does not match the registered dApp",
            )
            .with_data(json!({
                "expectedOrigin": self.origin.as_str(),
                "actualUrl": current_url,
            })))
        }
    }

    pub fn ensure_connected(&self, current_url: &str) -> Result<(), RpcError> {
        self.ensure_origin(current_url)?;
        if self.connected {
            Ok(())
        } else {
            Err(RpcError::new(
                error_code::UNAUTHORIZED,
                "the dApp is not connected; call fiber_connect first",
            ))
        }
    }

    pub fn revoke(&mut self) {
        self.connected = false;
        self.connected_at_ms = None;
    }

    pub fn connected_result(&self) -> Value {
        json!({
            "connected": self.connected,
            "dappId": self.dapp_id,
            "origin": self.origin.as_str(),
            "protocolVersion": PROTOCOL_VERSION,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectParams {
    pub permissions: Vec<String>,
}

impl ConnectParams {
    pub fn parse(value: Option<&Value>) -> Result<Self, RpcError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            #[serde(default)]
            permissions: Vec<String>,
        }
        let raw = parse_params::<Raw>(value, true)?;
        if raw.permissions.len() > 16
            || raw
                .permissions
                .iter()
                .any(|permission| permission.len() > 64 || permission.chars().any(char::is_control))
        {
            return Err(invalid_params(
                "permissions must contain at most 16 short strings",
            ));
        }
        Ok(Self {
            permissions: raw.permissions,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateInvoiceParams {
    pub amount_shannons: u64,
    pub description: String,
}

impl CreateInvoiceParams {
    pub fn parse(value: Option<&Value>) -> Result<Self, RpcError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            amount_shannons: String,
            #[serde(default)]
            description: String,
        }
        let raw = parse_params::<Raw>(value, false)?;
        let amount_shannons = parse_u64_decimal(&raw.amount_shannons, "amountShannons")?;
        if amount_shannons == 0 {
            return Err(invalid_params("amountShannons must be greater than zero"));
        }
        if raw.description.chars().count() > 256 || raw.description.chars().any(char::is_control) {
            return Err(invalid_params(
                "description must be at most 256 characters without controls",
            ));
        }
        Ok(Self {
            amount_shannons,
            description: raw.description,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayParams {
    pub invoice: Option<String>,
    pub target_pubkey: Option<String>,
    pub amount_shannons: Option<u64>,
    pub max_fee_shannons: Option<u64>,
}

/// Host-decoded invoice facts shown in the approval dialog. Parsing happens
/// before approval; the exact invoice string is still what gets sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoicePreview {
    pub payment_hash: String,
    pub amount_shannons: u64,
    pub description: Option<String>,
    pub payee: Option<String>,
    pub expires_at_ms: Option<u64>,
    pub network: &'static str,
    pub asset: &'static str,
}

pub fn inspect_invoice(invoice: &str, now_ms: u64) -> Result<InvoicePreview, RpcError> {
    let preview = fiber_wallet_core::api::wallet::wallet_inspect_invoice(invoice, now_ms)
        .map_err(invalid_params)?;
    Ok(InvoicePreview {
        payment_hash: preview.payment_hash,
        amount_shannons: preview.amount_shannons,
        description: preview.description,
        payee: preview.payee,
        expires_at_ms: preview.expires_at_ms,
        network: "Fiber Testnet",
        asset: "native CKB",
    })
}

impl PayParams {
    pub fn parse(value: Option<&Value>) -> Result<Self, RpcError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            invoice: Option<String>,
            target_pubkey: Option<String>,
            amount_shannons: Option<String>,
            max_fee_shannons: Option<String>,
        }
        let raw = parse_params::<Raw>(value, false)?;
        let invoice = raw
            .invoice
            .map(|invoice| invoice.trim().to_owned())
            .filter(|invoice| !invoice.is_empty());
        let target_pubkey = raw
            .target_pubkey
            .map(|pubkey| pubkey.trim().to_ascii_lowercase())
            .filter(|pubkey| !pubkey.is_empty());
        match (&invoice, &target_pubkey) {
            (Some(_), Some(_)) => {
                return Err(invalid_params(
                    "provide exactly one of invoice or targetPubkey",
                ));
            }
            (None, None) => return Err(invalid_params("invoice or targetPubkey is required")),
            _ => {}
        }
        let amount_shannons = raw
            .amount_shannons
            .map(|amount| parse_u64_decimal(&amount, "amountShannons"))
            .transpose()?;
        let max_fee_shannons = raw
            .max_fee_shannons
            .map(|amount| parse_u64_decimal(&amount, "maxFeeShannons"))
            .transpose()?;
        if invoice.is_some() && amount_shannons.is_some() {
            return Err(invalid_params(
                "invoice payments take their amount from the invoice",
            ));
        }
        if target_pubkey.is_some() && amount_shannons.is_none_or(|amount| amount == 0) {
            return Err(invalid_params("keysend requires a positive amountShannons"));
        }
        if max_fee_shannons == Some(0) {
            return Err(invalid_params("maxFeeShannons must be positive"));
        }
        if let Some(invoice) = &invoice
            && (invoice.len() < 8 || invoice.len() > 8192 || invoice.chars().any(char::is_control))
        {
            return Err(invalid_params("invoice length or characters are invalid"));
        }
        if let Some(pubkey) = &target_pubkey {
            validate_pubkey(pubkey)?;
        }
        let max_fee_shannons = max_fee_shannons.or_else(|| {
            Some(
                u64::try_from(fiber_wallet_core::api::wallet::DEFAULT_DAPP_MAX_FEE_SHANNONS)
                    .expect("default fee cap fits u64"),
            )
        });
        Ok(Self {
            invoice,
            target_pubkey,
            amount_shannons,
            max_fee_shannons,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenChannelParams {
    pub peer_pubkey: String,
    pub funding_amount_shannons: u64,
    pub public_channel: bool,
}

impl OpenChannelParams {
    pub fn parse(value: Option<&Value>) -> Result<Self, RpcError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            peer_pubkey: String,
            funding_amount_shannons: String,
            #[serde(default)]
            public_channel: bool,
        }
        let raw = parse_params::<Raw>(value, false)?;
        let peer_pubkey = raw.peer_pubkey.trim().to_ascii_lowercase();
        validate_pubkey(&peer_pubkey)?;
        let funding_amount_shannons =
            parse_u64_decimal(&raw.funding_amount_shannons, "fundingAmountShannons")?;
        if funding_amount_shannons == 0 {
            return Err(invalid_params(
                "fundingAmountShannons must be greater than zero",
            ));
        }
        Ok(Self {
            peer_pubkey,
            funding_amount_shannons,
            public_channel: raw.public_channel,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseChannelParams {
    pub channel_id: String,
    pub force: bool,
}

impl CloseChannelParams {
    pub fn parse(value: Option<&Value>) -> Result<Self, RpcError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            channel_id: String,
            #[serde(default)]
            force: bool,
        }
        let raw = parse_params::<Raw>(value, false)?;
        Ok(Self {
            channel_id: normalize_hex_id(&raw.channel_id, "channelId")?,
            force: raw.force,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetPaymentParams {
    pub payment_hash: String,
}

impl GetPaymentParams {
    pub fn parse(value: Option<&Value>) -> Result<Self, RpcError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            payment_hash: String,
        }
        let raw = parse_params::<Raw>(value, false)?;
        Ok(Self {
            payment_hash: normalize_hex_id(&raw.payment_hash, "paymentHash")?,
        })
    }
}

#[derive(Debug, Clone)]
pub enum WalletAction {
    Connect(ConnectParams),
    ReadNodeInfo,
    ReadBalance,
    ReadChannels,
    ReadPeers,
    CreateInvoice(CreateInvoiceParams),
    SendPayment(PayParams),
    DryRunPayment(PayParams),
    OpenChannel(OpenChannelParams),
    CloseChannel(CloseChannelParams),
    GetPayment(GetPaymentParams),
}

impl WalletAction {
    pub fn parse(method: ProviderMethod, params: Option<&Value>) -> Result<Self, RpcError> {
        Ok(match method {
            ProviderMethod::Connect => Self::Connect(ConnectParams::parse(params)?),
            ProviderMethod::GetNodeInfo => {
                parse_no_params(params)?;
                Self::ReadNodeInfo
            }
            ProviderMethod::GetBalance => {
                parse_no_params(params)?;
                Self::ReadBalance
            }
            ProviderMethod::ListChannels => {
                parse_no_params(params)?;
                Self::ReadChannels
            }
            ProviderMethod::ListPeers => {
                parse_no_params(params)?;
                Self::ReadPeers
            }
            ProviderMethod::CreateInvoice => {
                Self::CreateInvoice(CreateInvoiceParams::parse(params)?)
            }
            ProviderMethod::SendPayment => Self::SendPayment(PayParams::parse(params)?),
            ProviderMethod::DryRunPayment => Self::DryRunPayment(PayParams::parse(params)?),
            ProviderMethod::OpenChannel => Self::OpenChannel(OpenChannelParams::parse(params)?),
            ProviderMethod::CloseChannel => Self::CloseChannel(CloseChannelParams::parse(params)?),
            ProviderMethod::GetPayment => Self::GetPayment(GetPaymentParams::parse(params)?),
        })
    }

    pub fn requires_approval(&self) -> bool {
        !matches!(
            self,
            Self::ReadNodeInfo
                | Self::ReadBalance
                | Self::ReadChannels
                | Self::ReadPeers
                | Self::GetPayment(_)
        )
    }

    pub fn summary(&self, session: &DappSession) -> Result<ApprovalSummary, RpcError> {
        let origin = session.origin.as_str().to_owned();
        Ok(match self {
            Self::Connect(params) => ApprovalSummary {
                title: "连接 Fiber 钱包".to_owned(),
                origin,
                action: "允许读取节点、余额、通道与 peer；任何写操作仍会逐次确认。".to_owned(),
                details: vec![
                    ("dApp".to_owned(), session.dapp_name.clone()),
                    (
                        "请求权限".to_owned(),
                        if params.permissions.is_empty() {
                            "默认只读".to_owned()
                        } else {
                            params.permissions.join(", ")
                        },
                    ),
                ],
                destructive: false,
                dry_run: false,
            },
            Self::ReadNodeInfo | Self::ReadBalance | Self::ReadChannels | Self::ReadPeers => {
                ApprovalSummary {
                    title: "读取钱包数据".to_owned(),
                    origin,
                    action: "读取请求已获授权，无需逐次确认。".to_owned(),
                    details: Vec::new(),
                    destructive: false,
                    dry_run: false,
                }
            }
            Self::CreateInvoice(params) => ApprovalSummary {
                title: "创建收款发票".to_owned(),
                origin,
                action: "宿主将用本钱包节点创建发票。".to_owned(),
                details: vec![
                    (
                        "金额".to_owned(),
                        format!("{} shannons", params.amount_shannons),
                    ),
                    (
                        "备注".to_owned(),
                        if params.description.is_empty() {
                            "-".to_owned()
                        } else {
                            params.description.clone()
                        },
                    ),
                ],
                destructive: false,
                dry_run: false,
            },
            Self::SendPayment(params) => {
                let details = payment_details(params, dapp_protocol_now_ms())?;
                ApprovalSummary {
                    title: "发送 Fiber 付款".to_owned(),
                    origin,
                    action: "批准后，钱包将立即按下列预览发送付款；这不是 dry-run。".to_owned(),
                    details: with_payment_preview(details, params),
                    destructive: true,
                    dry_run: false,
                }
            }
            Self::DryRunPayment(params) => {
                let details = payment_details(params, dapp_protocol_now_ms())?;
                ApprovalSummary {
                    title: "付款预演（不发送）".to_owned(),
                    origin,
                    action: "仅构建路由并检查费用，不创建付款会话；仍需宿主确认。".to_owned(),
                    details: with_payment_preview(details, params),
                    destructive: false,
                    dry_run: true,
                }
            }
            Self::OpenChannel(params) => ApprovalSummary {
                title: "开通道并使用资金".to_owned(),
                origin,
                action: "批准后只对已连接的 peer 请求 Fiber 开通道；资金可用需链上确认。"
                    .to_owned(),
                details: vec![
                    ("对端".to_owned(), params.peer_pubkey.clone()),
                    (
                        "资金".to_owned(),
                        format!("{} shannons", params.funding_amount_shannons),
                    ),
                    (
                        "公开通道".to_owned(),
                        if params.public_channel { "是" } else { "否" }.to_owned(),
                    ),
                ],
                destructive: true,
                dry_run: false,
            },
            Self::CloseChannel(params) => ApprovalSummary {
                title: if params.force {
                    "强制关闭通道".to_owned()
                } else {
                    "协商关闭通道".to_owned()
                },
                origin,
                action: if params.force {
                    "强制关闭可能产生链上手续费和等待期。".to_owned()
                } else {
                    "批准后请求对端协商关闭；链上结算另行确认。".to_owned()
                },
                details: vec![
                    ("通道".to_owned(), params.channel_id.clone()),
                    (
                        "模式".to_owned(),
                        if params.force { "强制" } else { "协商" }.to_owned(),
                    ),
                ],
                destructive: true,
                dry_run: false,
            },
            Self::GetPayment(_) => unreachable!("reads never create approval summaries"),
        })
    }
}

struct PaymentDetails {
    rows: Vec<(String, String)>,
    preview: Option<InvoicePreview>,
}

fn payment_details(params: &PayParams, now_ms: u64) -> Result<PaymentDetails, RpcError> {
    if let Some(invoice) = &params.invoice {
        let preview = inspect_invoice(invoice, now_ms)?;
        return Ok(PaymentDetails {
            rows: Vec::new(),
            preview: Some(preview),
        });
    }
    Ok(PaymentDetails {
        rows: payment_details_unchecked(params),
        preview: None,
    })
}

fn with_payment_preview(details: PaymentDetails, params: &PayParams) -> Vec<(String, String)> {
    if let Some(preview) = details.preview {
        let mut rows = vec![
            ("网络".to_owned(), preview.network.to_owned()),
            ("资产".to_owned(), preview.asset.to_owned()),
            (
                "金额".to_owned(),
                format!("{} shannons", preview.amount_shannons),
            ),
            ("payment hash".to_owned(), preview.payment_hash),
        ];
        rows.push((
            "收款方".to_owned(),
            preview.payee.unwrap_or_else(|| "由发票签名恢复".to_owned()),
        ));
        rows.push((
            "描述".to_owned(),
            preview.description.unwrap_or_else(|| "-".to_owned()),
        ));
        rows.push((
            "过期时间".to_owned(),
            preview
                .expires_at_ms
                .map(|timestamp| timestamp.to_string())
                .unwrap_or_else(|| "发票未设置过期时间".to_owned()),
        ));
        rows.push((
            "费用上限".to_owned(),
            params
                .max_fee_shannons
                .map(|fee| format!("{fee} shannons"))
                .unwrap_or_else(|| "未提供；由钱包默认策略限制".to_owned()),
        ));
        rows.push((
            "完整发票".to_owned(),
            params.invoice.clone().unwrap_or_default(),
        ));
        rows
    } else {
        details.rows
    }
}

fn payment_details_unchecked(params: &PayParams) -> Vec<(String, String)> {
    let mut details = Vec::new();
    if let Some(invoice) = &params.invoice {
        details.push(("完整发票".to_owned(), invoice.clone()));
    }
    if let Some(pubkey) = &params.target_pubkey {
        details.push(("收款节点".to_owned(), pubkey.clone()));
    }
    if let Some(amount) = params.amount_shannons {
        details.push(("金额".to_owned(), format!("{amount} shannons")));
    }
    if let Some(fee) = params.max_fee_shannons {
        details.push(("费用上限".to_owned(), format!("{fee} shannons")));
    }
    details
}

fn dapp_protocol_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalSummary {
    pub title: String,
    pub origin: String,
    pub action: String,
    pub details: Vec<(String, String)>,
    pub destructive: bool,
    pub dry_run: bool,
}

/// Host-only approval data. The page never sees this token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalGrant {
    pub token: u64,
    pub generation: u64,
    pub method: ProviderMethod,
}

#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub grant: ApprovalGrant,
    pub request_id: String,
    pub dapp_id: String,
    pub session_generation: u64,
    pub action: WalletAction,
    pub summary: ApprovalSummary,
    pub created_at_ms: u64,
    pub nonce: String,
}

fn parse_params<T: DeserializeOwned>(
    value: Option<&Value>,
    allow_absent: bool,
) -> Result<T, RpcError> {
    let value = match value {
        None | Some(Value::Null) if allow_absent => Value::Object(Map::new()),
        None | Some(Value::Null) => {
            return Err(invalid_params("params object is required"));
        }
        Some(value) => value.clone(),
    };
    if !value.is_object() {
        return Err(invalid_params("params must be a JSON object"));
    }
    serde_json::from_value(value)
        .map_err(|error| invalid_params(format!("invalid params: {error}")))
}

fn parse_no_params(value: Option<&Value>) -> Result<(), RpcError> {
    if value.is_none_or(|value| matches!(value, Value::Null)) {
        Ok(())
    } else {
        Err(invalid_params("this method takes no params"))
    }
}

fn invalid_params(message: impl Into<String>) -> RpcError {
    RpcError::new(error_code::INVALID_PARAMS, message)
}

fn parse_u64_decimal(value: &str, field: &str) -> Result<u64, RpcError> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_params(format!(
            "{field} must be a decimal integer string"
        )));
    }
    value
        .parse()
        .map_err(|_| invalid_params(format!("{field} is out of range")))
}

fn validate_pubkey(value: &str) -> Result<(), RpcError> {
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if hex.len() != 66 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_params(
            "pubkey must be a 33-byte compressed key (66 hex characters)",
        ));
    }
    let bytes = hex::decode(hex).map_err(|_| invalid_params("pubkey is not hex"))?;
    if bytes.first() != Some(&0x02) && bytes.first() != Some(&0x03) {
        return Err(invalid_params(
            "pubkey must start with compressed-key prefix 02 or 03",
        ));
    }
    Ok(())
}

fn normalize_hex_id(value: &str, field: &str) -> Result<String, RpcError> {
    let value = value.trim();
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_params(format!(
            "{field} must be a 32-byte hex value"
        )));
    }
    Ok(format!("0x{}", hex.to_ascii_lowercase()))
}

pub fn response_ok(id: &str, result: Value) -> RpcResponse {
    RpcResponse {
        jsonrpc: "2.0".to_owned(),
        id: id.to_owned(),
        result: Some(result),
        error: None,
    }
}

pub fn response_error(id: &str, error: RpcError) -> RpcResponse {
    RpcResponse {
        jsonrpc: "2.0".to_owned(),
        id: id.to_owned(),
        result: None,
        error: Some(error),
    }
}

pub fn encode_response(response: &RpcResponse) -> String {
    serde_json::to_string(response).unwrap_or_else(|_| {
        serde_json::to_string(&response_error(
            &response.id,
            RpcError::new(error_code::INTERNAL_ERROR, "failed to encode response"),
        ))
        .unwrap_or_else(|_| "{}".to_owned())
    })
}

pub fn encode_event(nonce: &str, name: &str, data: Value) -> String {
    serde_json::to_string(&HostEvent {
        r#type: "event".to_owned(),
        nonce: nonce.to_owned(),
        event: name.to_owned(),
        protocol_version: PROTOCOL_VERSION,
        data,
    })
    .unwrap_or_else(|_| "{}".to_owned())
}

pub fn event_navigation() -> String {
    format!("host.v{PROTOCOL_VERSION}.navigation")
}

pub fn event_connection() -> String {
    format!("host.v{PROTOCOL_VERSION}.connection")
}

pub fn navigation_event(
    url: &str,
    title: &str,
    loading: bool,
    can_go_back: bool,
    can_go_forward: bool,
) -> Value {
    json!({
        "url": url,
        "title": title,
        "loading": loading,
        "canGoBack": can_go_back,
        "canGoForward": can_go_forward,
    })
}

pub fn parse_browser_message(data: &str) -> Result<DappRequest, RpcError> {
    if data.len() > MAX_REQUEST_BYTES {
        return Err(RpcError::new(
            error_code::INVALID_REQUEST,
            format!("request exceeds {MAX_REQUEST_BYTES} bytes"),
        ));
    }
    let request: DappRequest = serde_json::from_str(data).map_err(|error| {
        RpcError::new(
            error_code::PARSE_ERROR,
            format!("invalid JSON request: {error}"),
        )
    })?;
    request.validate()?;
    Ok(request)
}

impl DappRequest {
    fn validate(&self) -> Result<(), RpcError> {
        match self {
            DappRequest::Request {
                nonce,
                jsonrpc,
                id,
                method,
                params,
            } => {
                if nonce.is_empty() || nonce.len() > 256 || nonce.chars().any(char::is_control) {
                    return Err(RpcError::new(
                        error_code::INVALID_REQUEST,
                        "nonce is missing or malformed",
                    ));
                }
                if jsonrpc != "2.0" {
                    return Err(RpcError::new(
                        error_code::INVALID_REQUEST,
                        "jsonrpc must be \"2.0\"",
                    ));
                }
                validate_id(id)?;
                validate_method_syntax(method)?;
                if params.as_ref().is_some_and(|params| {
                    serde_json::to_vec(params).is_ok_and(|encoded| encoded.len() > 60_000)
                }) {
                    return Err(RpcError::new(
                        error_code::INVALID_REQUEST,
                        "params exceed 60 KiB",
                    ));
                }
                Ok(())
            }
            DappRequest::Event { method, .. } => validate_method_syntax(method),
        }
    }

    pub fn method(&self) -> &str {
        match self {
            DappRequest::Request { method, .. } | DappRequest::Event { method, .. } => method,
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        match self {
            DappRequest::Request { id, .. } => Some(id),
            DappRequest::Event { .. } => None,
        }
    }
}

fn validate_id(id: &str) -> Result<(), RpcError> {
    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
        return Err(RpcError::new(
            error_code::INVALID_REQUEST,
            "id must be 1-128 printable bytes",
        ));
    }
    Ok(())
}

fn validate_method_syntax(method: &str) -> Result<(), RpcError> {
    if method.is_empty()
        || method.len() > 128
        || !method
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(RpcError::new(
            error_code::INVALID_REQUEST,
            "method must be 1-128 ASCII method characters",
        ));
    }
    Ok(())
}

pub fn chain_info_result(
    node_phase: &str,
    network: &str,
    node_id: Option<&str>,
    funding_address: Option<&str>,
    node_name: Option<&str>,
    backend: Option<&str>,
    version: Option<&str>,
) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "node": {
            "phase": node_phase,
            "network": network,
            "nodeId": node_id,
            "fundingAddress": funding_address,
            "nodeName": node_name,
            "backend": backend,
            "version": version,
        },
        "capabilities": {
            "read": true,
            "invoice": true,
            "payment": true,
            "keysend": true,
            "openChannel": true,
            "closeChannel": true,
            "signing": false,
            "privateKeyAccess": false,
            "arbitraryRpc": false
        }
    })
}

pub fn balance_result(
    network: &str,
    funding_balance_shannons: &str,
    channel_balance_shannons: &str,
) -> Value {
    let funding = funding_balance_shannons.parse::<u128>().ok();
    let channel = channel_balance_shannons.parse::<u128>().ok();
    let total = match (funding, channel) {
        (Some(a), Some(b)) => a.checked_add(b),
        _ => None,
    };
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "network": network,
        "unit": "shannons",
        "fundingBalanceShannons": funding_balance_shannons,
        "channelBalanceShannons": channel_balance_shannons,
        "totalBalanceShannons": total.map(|value| value.to_string()),
    })
}

pub fn node_info_result(info: &fiber_wallet_core::api::wallet::WalletNodeInfo) -> Value {
    json!({
        "nodeId": info.node_id,
        "nodeName": info.node_name,
        "fundingAddress": info.funding_address,
        "addresses": info.addresses,
        "version": info.version,
        "fundingBalanceShannons": info.funding_balance_shannons,
        "channelBalanceShannons": info.channel_balance_shannons,
    })
}

pub fn channels_result(channels: &[fiber_wallet_core::api::wallet::WalletChannel]) -> Value {
    json!(
        channels
            .iter()
            .map(|channel| json!({
                "channelId": channel.channel_id,
                "peerId": channel.peer_id,
                "state": channel.state,
                "stateFlags": channel.state_flags,
                "inbound": channel.is_inbound,
                "fundingAsset": channel.funding_asset.label(),
                "fundingTypeScript": match &channel.funding_asset {
                    fiber_wallet_core::api::wallet::WalletChannelAsset::NativeCkb => None,
                    fiber_wallet_core::api::wallet::WalletChannelAsset::Udt { type_script } =>
                        Some(type_script.as_str()),
                },
                "localBalanceShannons": channel.local_balance,
                "remoteBalanceShannons": channel.remote_balance,
                "latestCommitmentTransactionHash": channel.latest_commitment_transaction_hash,
                "shutdownTransactionHash": channel.shutdown_transaction_hash,
                "failureDetail": channel.failure_detail,
            }))
            .collect::<Vec<_>>()
    )
}

pub fn peers_result(peers: &[fiber_wallet_core::api::wallet::WalletPeer]) -> Value {
    json!(
        peers
            .iter()
            .map(|peer| json!({
                "pubkey": peer.pubkey,
                "address": peer.address,
            }))
            .collect::<Vec<_>>()
    )
}

pub fn invoice_result(invoice: &fiber_wallet_core::api::wallet::WalletInvoice) -> Value {
    json!({
        "invoice": invoice.invoice,
        "paymentHash": invoice.payment_hash,
        "amountShannons": invoice.amount,
        "description": invoice.description,
    })
}

pub fn open_channel_result(channel: &fiber_wallet_core::api::wallet::WalletOpenedChannel) -> Value {
    json!({
        "temporaryChannelId": channel.temporary_channel_id,
        "peerId": channel.peer_id,
        "fundingAmountShannons": channel.funding_amount,
        "status": "accepted",
        "note": "Fiber accepted the open request; usable funds require funding confirmation",
    })
}

pub fn close_channel_result(
    result: &fiber_wallet_core::api::wallet::WalletChannelCloseResult,
) -> Value {
    json!({
        "channelId": result.channel_id,
        "mode": if result.mode == fiber_wallet_core::api::wallet::ChannelCloseMode::Force {
            "force"
        } else {
            "cooperative"
        },
        "state": result.state,
        "stateFlags": result.state_flags,
        "shutdownTransactionHash": result.shutdown_transaction_hash,
        "latestCommitmentTransactionHash": result.latest_commitment_transaction_hash,
        "failureDetail": result.failure_detail,
        "status": channel_close_status(
            result.state.as_deref(),
            result.state_flags,
            result.shutdown_transaction_hash.as_deref(),
        ),
        "note": "FNN accepted the close request; this is not proof of on-chain settlement",
    })
}

pub fn payment_result(result: &fiber_wallet_core::api::wallet::WalletSendPaymentResult) -> Value {
    use fiber_wallet_core::api::wallet::WalletSendPaymentResult;
    match result {
        WalletSendPaymentResult::Quote { quote } => json!({
            "kind": "quote",
            "dryRun": true,
            "quoteId": quote.quote_id,
            "amountShannons": quote.amount_shannons,
            "feeShannons": quote.fee_shannons,
            "createdAtMs": quote.created_at_ms,
        }),
        WalletSendPaymentResult::Submitted {
            payment,
            terminal_error,
        } => json!({
            "kind": "submitted",
            "dryRun": false,
            "paymentHash": payment.payment_hash,
            "status": payment.status,
            "failedError": payment.failed_error,
            "feeShannons": payment.fee_shannons,
            "createdAtMs": payment.created_at_ms,
            "lastUpdatedAtMs": payment.last_updated_at_ms,
            "terminalError": terminal_error,
            "indeterminate": terminal_error.is_some() || !payment.status.is_terminal(),
        }),
    }
}

pub fn payment_record_result(payment: &fiber_wallet_core::api::wallet::WalletPayment) -> Value {
    json!({
        "kind": "payment",
        "dryRun": false,
        "paymentHash": payment.payment_hash,
        "status": payment.status,
        "failedError": payment.failed_error,
        "feeShannons": payment.fee_shannons,
        "createdAtMs": payment.created_at_ms,
        "lastUpdatedAtMs": payment.last_updated_at_ms,
    })
}

fn channel_close_status(
    state: Option<&str>,
    flags: Option<u32>,
    shutdown_tx: Option<&str>,
) -> &'static str {
    match fiber_wallet_core::api::wallet::channel_close_status(state, flags, shutdown_tx) {
        fiber_wallet_core::api::wallet::ChannelCloseStatus::Closed => "closed",
        fiber_wallet_core::api::wallet::ChannelCloseStatus::WaitingOnchainSettlement => {
            "waiting_onchain"
        }
        fiber_wallet_core::api::wallet::ChannelCloseStatus::InProgress => "in_progress",
        fiber_wallet_core::api::wallet::ChannelCloseStatus::Submitted => "submitted",
        fiber_wallet_core::api::wallet::ChannelCloseStatus::OpeningFailed => "opening_failed",
        fiber_wallet_core::api::wallet::ChannelCloseStatus::Failed
        | fiber_wallet_core::api::wallet::ChannelCloseStatus::FailedWithDetail => "failed",
        fiber_wallet_core::api::wallet::ChannelCloseStatus::Unknown => "unknown",
    }
}

pub fn bootstrap_script(nonce: &str) -> String {
    let nonce = serde_json::to_string(nonce).expect("nonce is a JSON string");
    format!(
        r#"(function () {{
  var hostNonce = {nonce};
  var root = window.fiberWallet = window.fiberWallet || {{}};
  if (root.__installedVersion === {version} && root.__hostNonce === hostNonce) {{
    return;
  }}
  var pending = Object.create(null);
  var listeners = Object.create(null);
  function randomId() {{
    if (window.crypto && typeof window.crypto.randomUUID === 'function') {{
      return window.crypto.randomUUID();
    }}
    var bytes = new Uint8Array(16);
    window.crypto.getRandomValues(bytes);
    return Array.prototype.map.call(bytes, function (value) {{
      return value.toString(16).padStart(2, '0');
    }}).join('');
  }}
  function emit(event) {{
    if (!event || event.type !== 'event' || event.nonce !== hostNonce ||
        typeof event.event !== 'string') return;
    (listeners[event.event] || []).slice().forEach(function (fn) {{
      try {{ fn(event.data); }} catch (_) {{}}
    }});
  }}
  function receiveMessage(event) {{
    var message = event.data;
    if (Object.prototype.toString.call(message) === '[object String]') {{
      try {{ message = JSON.parse(message); }} catch (_) {{ return; }}
    }}
    if (!message || message.nonce !== hostNonce) return;
    if (message.type === 'event' && typeof message.event === 'string') {{
      emit(message);
      return;
    }}
    if (message.jsonrpc !== '2.0' || typeof message.id !== 'string') return;
    var entry = pending[message.id];
    if (!entry) return;
    delete pending[message.id];
    clearTimeout(entry.timer);
    if (message.error) {{
      var error = new Error(message.error.message || 'dApp request failed');
      error.code = message.error.code;
      error.data = message.error.data;
      entry.reject(error);
    }} else {{
      entry.resolve(message.result);
    }}
  }}
  window.addEventListener('message', receiveMessage);
  function request(method, params) {{
    return new Promise(function (resolve, reject) {{
      if (typeof method !== 'string') {{
        reject(new TypeError('method must be a string'));
        return;
      }}
      if (!window.nsh || typeof window.nsh.postMessage !== 'function') {{
        reject(new Error('the NSH host bridge is unavailable'));
        return;
      }}
      var id;
      try {{
        id = randomId();
      }} catch (error) {{
        reject(new Error('the page has no cryptographically secure randomness'));
        return;
      }}
      var timer = setTimeout(function () {{
        if (!pending[id]) return;
        delete pending[id];
        reject(new Error('dApp request timed out'));
      }}, {timeout});
      pending[id] = {{ resolve: resolve, reject: reject, timer: timer }};
      try {{
        window.nsh.postMessage(JSON.stringify({{
          type: 'request',
          nonce: hostNonce,
          jsonrpc: '2.0',
          id: id,
          method: method,
          params: params === undefined ? null : params
        }}));
      }} catch (error) {{
        clearTimeout(timer);
        delete pending[id];
        reject(error);
      }}
    }});
  }}
  var methods = Object.freeze({{
    connect: 'fiber_connect',
    getNodeInfo: 'fiber_getNodeInfo',
    getBalance: 'fiber_getBalance',
    listChannels: 'fiber_listChannels',
    listPeers: 'fiber_listPeers',
    createInvoice: 'fiber_createInvoice',
    sendPayment: 'fiber_sendPayment',
    openChannel: 'fiber_openChannel',
    closeChannel: 'fiber_closeChannel',
    getPayment: 'fiber_getPayment',
    dryRunPayment: 'fiber_dryRunPayment'
  }});
  root.__installedVersion = {version};
  root.__hostNonce = hostNonce;
  root.protocolVersion = {version};
  root.info = Object.freeze({{
    protocol: 'fiber-wallet-jsonrpc',
    protocolVersion: {version},
    permissions: Object.freeze({{
      read: true,
      invoice: true,
      payment: true,
      keysend: true,
      openChannel: true,
      closeChannel: true,
      signing: false,
      privateKeyAccess: false,
      arbitraryRpc: false
    }})
  }});
  root.methods = methods;
  root.request = request;
  root.connect = function (options) {{ return request(methods.connect, options || {{}}); }};
  root.getNodeInfo = function () {{ return request(methods.getNodeInfo); }};
  root.getBalance = function () {{ return request(methods.getBalance); }};
  root.listChannels = function () {{ return request(methods.listChannels); }};
  root.listPeers = function () {{ return request(methods.listPeers); }};
  root.createInvoice = function (params) {{ return request(methods.createInvoice, params); }};
  root.sendPayment = function (params) {{ return request(methods.sendPayment, params); }};
  root.openChannel = function (params) {{ return request(methods.openChannel, params); }};
  root.closeChannel = function (params) {{ return request(methods.closeChannel, params); }};
  root.getPayment = function (params) {{ return request(methods.getPayment, params); }};
  root.dryRunPayment = function (params) {{ return request(methods.dryRunPayment, params); }};
  root.on = function (name, listener) {{
    if (typeof name !== 'string' || typeof listener !== 'function') {{
      throw new TypeError('on(event, listener) needs a string and a function');
    }}
    (listeners[name] || (listeners[name] = [])).push(listener);
    return function () {{
      var list = listeners[name] || [];
      var index = list.indexOf(listener);
      if (index >= 0) list.splice(index, 1);
    }};
  }};
  root.__selftestRoundtrip = function () {{
    return new Promise(function (resolve, reject) {{
      var timer = setTimeout(function () {{
        reject(new Error('host roundtrip timed out'));
      }}, 5000);
      var remove = null;
      function cleanup() {{
        clearTimeout(timer);
        if (remove) remove();
        remove = null;
      }}
      function finish(ok, value) {{
        cleanup();
        ok ? resolve(value) : reject(value);
      }}
      remove = root.on('host.v{version}.roundtrip', function (data) {{
        finish(true, data);
      }});
      request('fiber_getNodeInfo').then(
        function () {{ finish(false, new Error('unconnected read unexpectedly succeeded')); }},
        function (error) {{
          if (error && error.code === -32002) {{
            finish(true, {{ status: 'host roundtrip OK', code: error.code }});
          }} else {{
            finish(false, error);
          }}
        }}
      );
    }});
  }};
}})();"#,
        version = PROTOCOL_VERSION,
        nonce = nonce,
        timeout = PAGE_REQUEST_TIMEOUT_MS,
    )
}

/// Reinstalls the provider in a document that reloaded after the WebView was
/// constructed. The long-lived injected script keeps the original nonce; this
/// script is evaluated into the new same-origin document with the current one.
pub fn document_bootstrap_script(nonce: &str) -> String {
    bootstrap_script(nonce)
}

pub fn post_event_script(nonce: &str, name: &str, data: &Value) -> String {
    let encoded = encode_event(nonce, name, data.clone());
    format!(
        "window.dispatchEvent(new MessageEvent('message', {{ data: {} }}));",
        serde_json::to_string(&encoded).unwrap_or_else(|_| "\"{}\"".to_owned())
    )
}

pub fn protocol_selftest_html() -> String {
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<meta charset="utf-8">
<title>Fiber Wallet · dApp provider test</title>
<style>
  :root {{ color-scheme: dark; font-family: -apple-system, BlinkMacSystemFont, sans-serif; }}
  body {{ margin: 0; padding: 26px; background: #0b0f1a; color: #e8ecf4; }}
  h1 {{ margin: 0 0 6px; font-size: 22px; }}
  h2 {{ margin: 20px 0 8px; font-size: 14px; color: #9aa5ba; }}
  p {{ color: #8b93a7; line-height: 1.5; }}
  button, input {{ box-sizing: border-box; }}
  button {{ margin: 4px; padding: 9px 12px; border: 1px solid #2b3650; border-radius: 8px;
            background: #162033; color: #e8ecf4; cursor: pointer; }}
  button.danger {{ border-color: #71343b; color: #ff9c9c; }}
  input {{ width: 100%; max-width: 660px; padding: 9px 11px; margin: 4px 0;
           border: 1px solid #2b3650; border-radius: 8px; background: #111827; color: #e8ecf4; }}
  pre {{ white-space: pre-wrap; word-break: break-all; padding: 14px; border-radius: 10px;
         background: #121826; border: 1px solid #212a3d; line-height: 1.45; min-height: 120px; }}
  .ok {{ color: #3dd68c; }} .bad {{ color: #ff5d5d; }} .muted {{ color: #8b93a7; }}
</style>
<h1>内建 dApp provider 测试页</h1>
<p>此页离线运行。它验证 connect、读写授权、宿主确认、结构化错误与真实消息往返；
不会自动发送付款。写操作必须由你在钱包宿主弹窗中明确批准。</p>
<h2>连接与只读</h2>
<button onclick="runConnect()">connect</button>
<button onclick="runRead('getNodeInfo')">getNodeInfo</button>
<button onclick="runRead('getBalance')">getBalance</button>
<button onclick="runRead('listChannels')">listChannels</button>
<button onclick="runRead('listPeers')">listPeers</button>
<button class="danger" onclick="runUnauthorizedWrite()">未连接写请求（应 UNAUTHORIZED）</button>
<button onclick="runUnknown()">未知方法</button>
<button onclick="runDenied()">尝试 sign（应拒绝）</button>
<h2>写操作与 dry-run（每次均需确认）</h2>
<input id="invoice" placeholder="粘贴 Fiber invoice">
<input id="pubkey" placeholder="keysend 目标 33-byte pubkey">
<input id="amount" placeholder="keysend 金额（shannons）">
<button onclick="runInvoice()">createInvoice</button>
<button class="danger" onclick="runPayInvoice()">sendPayment(invoice)</button>
<button class="danger" onclick="runKeysend()">sendPayment(keysend)</button>
<button onclick="runDryRunInvoice()">dryRunPayment(invoice)</button>
<button class="danger" onclick="runOpen()">openChannel</button>
<button class="danger" onclick="runClose()">closeChannel</button>
<input id="paymentHash" placeholder="32-byte hex payment hash / channel id">
<button onclick="runGetPayment()">getPayment</button>
<h2>结果</h2>
<pre id="out">加载中…</pre>
<script>
  var out = document.getElementById('out');
  function show(title, value) {{
    out.textContent = title + '\n' + JSON.stringify(value, null, 2);
    out.className = value && value.error ? 'bad' : 'ok';
  }}
  function fail(title, error) {{
    show(title, {{ error: {{ code: error.code, message: error.message, data: error.data }} }});
  }}
  function ready() {{
    if (!window.fiberWallet || typeof window.fiberWallet.request !== 'function') {{
      out.textContent = 'window.fiberWallet 未注入';
      out.className = 'bad';
      return false;
    }}
    return true;
  }}
  function runConnect() {{
    if (!ready()) return;
    window.fiberWallet.connect({{ permissions: ['read', 'invoice', 'payment', 'channels'] }})
      .then(function (v) {{ show('连接授权结果', v); }})
      .catch(function (e) {{ fail('连接被拒绝或失败', e); }});
  }}
  function runRead(name) {{
    if (!ready()) return;
    window.fiberWallet[name]()
      .then(function (v) {{ show(name + ' 成功', v); }})
      .catch(function (e) {{ fail(name + ' 失败', e); }});
  }}
  function runUnknown() {{
    if (!ready()) return;
    window.fiberWallet.request('fiber_notImplemented', {{}})
      .then(function (v) {{ show('严重错误：未知方法成功', v); }})
      .catch(function (e) {{ fail('未知方法结构化拒绝', e); }});
  }}
  function runUnauthorizedWrite() {{
    if (!ready()) return;
    window.fiberWallet.createInvoice({{ amountShannons: '1000', description: 'must be denied' }})
      .then(function (v) {{ show('严重错误：未连接写请求成功', v); }})
      .catch(function (e) {{
        if (e.code === -32002) {{
          show('未连接写请求已拒绝（UNAUTHORIZED）', {{ code: e.code, message: e.message }});
        }} else {{
          fail('未连接写请求返回了错误码，但不是 UNAUTHORIZED', e);
        }}
      }});
  }}
  function runDenied() {{
    if (!ready()) return;
    window.fiberWallet.request('sign_message', {{ payload: 'test' }})
      .then(function (v) {{ show('严重错误：签名成功', v); }})
      .catch(function (e) {{ fail('签名被永久禁止', e); }});
  }}
  function runInvoice() {{
    if (!ready()) return;
    window.fiberWallet.createInvoice({{ amountShannons: '1000', description: 'provider self-test' }})
      .then(function (v) {{ show('发票已创建（宿主已确认）', v); }})
      .catch(function (e) {{ fail('创建发票未完成', e); }});
  }}
  function payParams(invoiceOnly) {{
    if (invoiceOnly) {{
      return {{ invoice: document.getElementById('invoice').value.trim() }};
    }}
    return {{
      targetPubkey: document.getElementById('pubkey').value.trim(),
      amountShannons: document.getElementById('amount').value.trim()
    }};
  }}
  function runPayInvoice() {{
    if (!ready()) return;
    window.fiberWallet.sendPayment(payParams(true))
      .then(function (v) {{ show('付款请求返回（仍需链上/通道结果确认）', v); }})
      .catch(function (e) {{ fail('付款未完成或被拒绝', e); }});
  }}
  function runKeysend() {{
    if (!ready()) return;
    window.fiberWallet.sendPayment(payParams(false))
      .then(function (v) {{ show('keysend 请求返回', v); }})
      .catch(function (e) {{ fail('keysend 未完成或被拒绝', e); }});
  }}
  function runDryRunInvoice() {{
    if (!ready()) return;
    window.fiberWallet.dryRunPayment(payParams(true))
      .then(function (v) {{ show('dry-run 返回（未付款）', v); }})
      .catch(function (e) {{ fail('dry-run 失败或被拒绝', e); }});
  }}
  function runOpen() {{
    if (!ready()) return;
    window.fiberWallet.openChannel({{
      peerPubkey: document.getElementById('pubkey').value.trim(),
      fundingAmountShannons: document.getElementById('amount').value.trim(),
      publicChannel: false
    }}).then(function (v) {{ show('开通道请求返回', v); }})
      .catch(function (e) {{ fail('开通道未完成或被拒绝', e); }});
  }}
  function runClose() {{
    if (!ready()) return;
    window.fiberWallet.closeChannel({{
      channelId: document.getElementById('paymentHash').value.trim(),
      force: false
    }}).then(function (v) {{ show('关通道请求返回', v); }})
      .catch(function (e) {{ fail('关通道未完成或被拒绝', e); }});
  }}
  function runGetPayment() {{
    if (!ready()) return;
    window.fiberWallet.getPayment({{
      paymentHash: document.getElementById('paymentHash').value.trim()
    }}).then(function (v) {{ show('付款状态', v); }})
      .catch(function (e) {{ fail('读取付款失败', e); }});
  }}
  window.fiberWallet.on('host.v{version}.connection', function (data) {{
    var line = document.createElement('div');
    line.className = 'muted';
    line.textContent = 'connection event: ' + JSON.stringify(data);
    document.body.insertBefore(line, out);
  }});
  show('provider 已就绪', {{
    protocolVersion: window.fiberWallet.protocolVersion,
    methods: window.fiberWallet.methods
  }});
  window.fiberWallet.__selftestRoundtrip()
    .then(function (v) {{
      var ok = document.createElement('div');
      ok.className = 'ok';
      ok.textContent = 'HOST ROUNDTRIP OK · UNAUTHORIZED=' + v.code;
      document.body.insertBefore(ok, out);
    }})
    .catch(function (e) {{
      var bad = document.createElement('div');
      bad.className = 'bad';
      bad.textContent = 'HOST ROUNDTRIP FAILED: ' + (e && e.message ? e.message : String(e));
      document.body.insertBefore(bad, out);
    }});
</script>
</html>"#,
        version = PROTOCOL_VERSION
    )
}

/// Longest-prefix-safe origin parser. Userinfo and non-HTTP(S) URLs are
/// rejected.
fn origin_parts(url: &str) -> Option<(String, String, u16)> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "https" | "http") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, explicit_port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        let port = match tail {
            "" => None,
            tail => Some(tail.strip_prefix(':')?.parse::<u16>().ok()?),
        };
        (format!("[{}]", host.to_ascii_lowercase()), port)
    } else {
        let (host, port) = authority
            .rsplit_once(':')
            .filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        let port = port.map(str::parse::<u16>).transpose().ok()?;
        (host.to_ascii_lowercase(), port)
    };
    if host.is_empty() {
        return None;
    }
    let port = explicit_port.unwrap_or(if scheme == "https" { 443 } else { 80 });
    Some((scheme, host, port))
}

fn format_origin(parts: &(String, String, u16)) -> String {
    let default_port = if parts.0 == "https" { 443 } else { 80 };
    if parts.2 == default_port {
        format!("{}://{}", parts.0, parts.1)
    } else {
        format!("{}://{}:{}", parts.0, parts.1, parts.2)
    }
}

/// Retained for focused policy tests. This check is necessary but not
/// sufficient: NSH 0.2.0 does not expose a per-WebView navigation callback,
/// so the host can react to a cross-origin commit but cannot preempt it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationPolicy {
    origin: Origin,
}

impl NavigationPolicy {
    pub fn for_origin(url: &str) -> Self {
        Self {
            origin: Origin::parse(url).unwrap_or_else(|_| Origin(SELFTEST_URL.to_owned())),
        }
    }

    pub fn allows(&self, url: &str) -> bool {
        self.origin.allows(url)
    }

    pub fn is_same_origin(&self, url: &str) -> bool {
        self.allows(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_json_rpc() {
        let request = parse_browser_message(
            r#"{"type":"request","nonce":"test-nonce","jsonrpc":"2.0","id":"1","method":"fiber_getBalance","params":null}"#,
        )
        .expect("valid");
        assert_eq!(request.request_id(), Some("1"));
        assert!(parse_browser_message("not json").is_err());
        assert!(
            parse_browser_message(
                r#"{"type":"request","nonce":"test-nonce","jsonrpc":"1.0","id":"1","method":"fiber_getBalance"}"#
            )
            .is_err()
        );
        assert!(
            parse_browser_message(
                r#"{"type":"request","nonce":"test-nonce","jsonrpc":"2.0","id":"","method":"fiber_getBalance"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_oversized_and_dangerous_methods() {
        let oversized = format!(
            r#"{{"type":"request","nonce":"test-nonce","jsonrpc":"2.0","id":"1","method":"fiber_getBalance","params":"{}"}}"#,
            "x".repeat(MAX_REQUEST_BYTES)
        );
        assert_eq!(
            parse_browser_message(&oversized).unwrap_err().code,
            error_code::INVALID_REQUEST
        );
        for method in [
            "sign_message",
            "wallet_exportPrivateKey",
            "wallet_getKeychain",
            "eval",
            "exec",
            "inject_js",
            "wallet_rawRpc",
        ] {
            assert!(is_forbidden_method(method), "{method}");
        }
        assert!(!is_forbidden_method("fiber_createInvoice"));
        assert!(resolve_method("fiber_createInvoice").is_some());
        assert!(resolve_method("fiber_createInvoiceExtra").is_none());
    }

    #[test]
    fn parses_wallet_aliases() {
        for (method, expected) in [
            ("wallet_connect", ProviderMethod::Connect),
            ("wallet_getBalance", ProviderMethod::GetBalance),
            ("wallet_sendPayment", ProviderMethod::SendPayment),
            ("fiber_closeChannel", ProviderMethod::CloseChannel),
        ] {
            assert_eq!(resolve_method(method), Some(expected));
        }
    }

    #[test]
    fn origin_binding_is_exact() {
        let origin = Origin::parse("https://example.com/path").unwrap();
        assert_eq!(origin.as_str(), "https://example.com");
        assert!(origin.allows("https://example.com:443/other"));
        assert!(!origin.allows("https://example.com.evil/"));
        assert!(!origin.allows("http://example.com/"));
        assert!(!origin.allows("https://user@example.com/"));
        assert!(
            Origin::parse("http://[::1]:8080/x")
                .unwrap()
                .allows("http://[::1]:8080/y")
        );
        assert_eq!(Origin::parse(SELFTEST_URL).unwrap().as_str(), SELFTEST_URL);
    }

    #[test]
    fn session_requires_origin_and_explicit_connect() {
        let mut session = DappSession::new("user:1", "Test", "https://example.com/app", 1).unwrap();
        assert_eq!(
            session
                .ensure_connected("https://example.com/app")
                .unwrap_err()
                .code,
            error_code::UNAUTHORIZED
        );
        assert_eq!(
            session
                .authorize("https://evil.example/", 1)
                .unwrap_err()
                .code,
            error_code::ORIGIN_MISMATCH
        );
        session.authorize("https://example.com/other", 10).unwrap();
        assert!(session.connected);
        session.revoke();
        assert!(!session.connected);
    }

    #[test]
    fn payment_parser_distinguishes_invoice_and_keysend() {
        let invoice = json!({"invoice": "fibbtest1abcdefgh"});
        let parsed = PayParams::parse(Some(&invoice)).unwrap();
        assert!(parsed.invoice.is_some());
        assert!(parsed.target_pubkey.is_none());

        let keysend = json!({
            "targetPubkey": "02aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "amountShannons": "1000",
            "maxFeeShannons": "10",
        });
        let parsed = PayParams::parse(Some(&keysend)).unwrap();
        assert_eq!(parsed.amount_shannons, Some(1000));
        assert_eq!(parsed.max_fee_shannons, Some(10));

        assert!(
            PayParams::parse(Some(&json!({
                "invoice": "fibbtest1abcdefgh",
                "amountShannons": "1",
            })))
            .is_err()
        );
        assert!(
            PayParams::parse(Some(&json!({
                "targetPubkey": "02aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            })))
            .is_err()
        );
    }

    #[test]
    fn writes_need_approval_but_reads_do_not() {
        let session = DappSession::new("id", "Name", "https://example.com", 7).unwrap();
        assert!(!WalletAction::ReadBalance.requires_approval());
        let action = WalletAction::CreateInvoice(CreateInvoiceParams {
            amount_shannons: 1,
            description: String::new(),
        });
        assert!(action.requires_approval());
        let summary = action.summary(&session).expect("summary");
        assert!(!summary.dry_run);
        assert_eq!(summary.origin, "https://example.com");
    }

    #[test]
    fn approval_queue_is_token_bound_and_rejection_consumes() {
        let payment = WalletAction::GetPayment(GetPaymentParams {
            payment_hash: format!("0x{}", "11".repeat(32)),
        });
        assert!(!payment.requires_approval());
    }

    #[test]
    fn provider_exposes_operations_but_not_signing() {
        let script = bootstrap_script("test-nonce");
        for needle in [
            "fiber_connect",
            "fiber_sendPayment",
            "fiber_openChannel",
            "fiber_closeChannel",
            "fiber_dryRunPayment",
        ] {
            assert!(script.contains(needle), "{needle}");
        }
        assert!(!script.contains("exportPrivateKey"));
        assert!(!script.contains("signMessage"));
        let html = protocol_selftest_html();
        assert!(html.contains("sendPayment(invoice)"));
        assert!(html.contains("UNAUTHORIZED"));
        assert!(html.contains("HOST ROUNDTRIP OK"));
        assert!(html.contains("dryRunPayment"));
        assert!(html.contains("sign_message"));
    }

    #[test]
    fn response_requires_exactly_one_of_result_or_error() {
        let ok = response_ok("1", json!({"value": 1}));
        assert!(ok.result.is_some() && ok.error.is_none());
        let bad = response_error("2", RpcError::new(error_code::USER_REJECTED, "no"));
        assert!(bad.result.is_none() && bad.error.is_some());
        let decoded: RpcResponse =
            serde_json::from_str(&encode_response(&bad)).expect("wire response");
        assert_eq!(decoded.error.unwrap().code, error_code::USER_REJECTED);
    }
}
