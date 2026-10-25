//! Bridges the wallet core's async API to the NSH engine.
//!
//! The core's futures need a tokio context (Fiber actors spawn on tokio), so
//! the bridge owns a runtime on dedicated threads and mirrors results into a
//! shared [`UiState`]. The UI copies that state into its signals on a timer.
//! Lifecycle behavior: optimistic phase transitions, serialized start/stop,
//! an event log of phase/detail changes, and 10 s data polling while running.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::dapp_protocol::{self, PayParams, ProviderMethod, RpcError, WalletAction, error_code};
use crate::vault;
use fiber_wallet_core::api::node::{self, NodeNetwork, NodePhase, NodeSnapshot};
use fiber_wallet_core::api::wallet::{
    self, ChannelCloseMode, ChannelCloseRequest, ChannelCloseStatus, OnchainTransferError,
    OnchainTxState, WalletChannel, WalletInvoice, WalletNodeInfo, WalletOnchainTransfer,
    WalletOnchainTxStatus, WalletOpenedChannel, WalletPayment, WalletPaymentQuote,
    WalletPaymentStatus, WalletPeer, WalletSendPaymentRequest, WalletSendPaymentResult,
};

const PAYMENT_POLL_INTERVAL: Duration = Duration::from_secs(1);
const PAYMENT_POLL_ERROR_LIMIT: u8 = 3;
const TRANSFER_TIMEOUT_SECONDS: u64 = 60;
/// The core waits for the request timeout; this bounded recovery poll runs
/// only when the send unexpectedly returns a nonterminal payment.
const TRANSFER_POLL_MAX_SECONDS: u64 = TRANSFER_TIMEOUT_SECONDS + 15;
const ONCHAIN_STATUS_POLL_INTERVAL: Duration = Duration::from_secs(3);
const ONCHAIN_STATUS_MAX_POLLS: usize = 20;
const CHANNEL_CLOSE_MAX_POLLS: usize = 30;
const CHANNEL_CLOSE_ERROR_LIMIT: u8 = 5;
const PEER_READY_MAX_ATTEMPTS: usize = 10;
const PEER_READY_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// UI phase for payments initiated from the unified payment API.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaymentPhase {
    #[default]
    Idle,
    Quoting,
    Quoted,
    Submitting,
    Success,
    Failed,
    /// Fiber accepted the payment but did not report Success/Failed before
    /// polling timed out or repeated lookup errors made the state unprovable.
    Unknown,
}

/// The dry-run quote is paired with the exact validated request that produced
/// it. Confirmation sends this request, never mutable form fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaymentState {
    pub phase: PaymentPhase,
    pub quote: Option<WalletPaymentQuote>,
    pub payment: Option<WalletPayment>,
    pub error: Option<String>,
}

/// Lifecycle of one CKB testnet on-chain broadcast.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnchainPhase {
    #[default]
    Idle,
    Confirming,
    Broadcasting,
    /// The RPC accepted the transaction hash. This is not confirmation.
    Broadcast,
    /// The broadcast RPC failed after the signed hash existed. The hash is
    /// retained and must be queried; retrying without checking may double-pay.
    Uncertain,
    Committed,
    Rejected,
    /// Canonical status could not be established within the bounded poll.
    Unknown,
    /// Build, signing, or broadcast failed without an uncertain hash. This is
    /// terminal for the attempt and keeps the error visible while allowing a
    /// fresh form submission.
    Failed,
}

/// On-chain form result and the exact normalized transaction being tracked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OnchainState {
    pub phase: OnchainPhase,
    pub transfer: Option<WalletOnchainTransfer>,
    pub query_hash: Option<String>,
    pub status: Option<WalletOnchainTxStatus>,
    pub error: Option<String>,
    pub last_checked_at_ms: Option<u64>,
    /// True only when the broadcast RPC explicitly accepted the transaction.
    pub broadcast_accepted: bool,
}

/// Everything the wallet page renders, cloned from the bridge on each tick.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct UiState {
    pub snapshot: Option<NodeSnapshot>,
    pub busy: bool,
    pub events: Vec<String>,
    /// Newest lines of the app's tracing output (oldest first), copied from
    /// `logs::global()` by the root poller. Empty until the binary installs
    /// the capture layer.
    pub log_tail: Vec<String>,
    /// One-shot user-facing result of the last completed operation; the root
    /// shows it as a toast keyed on `seq`.
    pub flash: Option<Flash>,
    pub node_info: Option<WalletNodeInfo>,
    pub channels: Vec<WalletChannel>,
    pub payments: Vec<WalletPayment>,
    pub peers: Vec<WalletPeer>,
    pub last_invoice: Option<WalletInvoice>,
    pub invoice_busy: bool,
    pub invoice_error: Option<String>,
    pub send_busy: bool,
    pub send_result: Option<String>,
    pub send_error: Option<String>,
    /// Keysend quote/send workflow. Invoice payments update `send_*` only.
    pub transfer: PaymentState,
    /// Native CKB testnet transfer and its independently queried chain status.
    pub onchain: OnchainState,
    pub data_busy: bool,
    pub data_error: Option<String>,
    pub open_channel_busy: bool,
    pub open_channel_error: Option<String>,
    /// The one channel whose close RPC is currently executing.
    pub closing_channel_id: Option<String>,
    /// Close attempts retained after the RPC thread returns so the row can
    /// continue showing `ShuttingDown` until FNN reports `Closed`.
    pub channel_close_attempts: Vec<ChannelCloseAttempt>,
    pub backup_busy: bool,
    pub backup_message: Option<String>,
    pub backup_error: Option<String>,
    pub password_busy: bool,
    pub password_message: Option<String>,
    pub password_error: Option<String>,
}

/// A one-shot user-facing outcome of a bridge operation (成功/失败提示).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashKind {
    Ok,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flash {
    /// Strictly increasing; the root toasts only sequences it has not seen.
    pub seq: u64,
    pub kind: FlashKind,
    pub message: String,
}

/// One close attempt, persisted only in memory for the lifetime of this app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelCloseAttempt {
    pub channel_id: String,
    /// Normalized form used to match FNN channel rows.
    pub normalized_channel_id: String,
    pub mode: ChannelCloseMode,
    pub status: ChannelCloseStatus,
    pub state: Option<String>,
    pub state_flags: Option<u32>,
    pub shutdown_transaction_hash: Option<String>,
    pub latest_commitment_transaction_hash: Option<String>,
    pub failure_detail: Option<String>,
    /// Consecutive list-channel query failures while awaiting closure.
    pub poll_errors: u8,
    /// Last UI-facing error. Present only when `status == Failed`.
    pub error: Option<String>,
}

impl ChannelCloseAttempt {
    fn new(channel_id: &str, normalized_channel_id: String, mode: ChannelCloseMode) -> Self {
        Self {
            channel_id: channel_id.to_owned(),
            normalized_channel_id,
            mode,
            status: ChannelCloseStatus::Submitted,
            state: None,
            state_flags: None,
            shutdown_transaction_hash: None,
            latest_commitment_transaction_hash: None,
            failure_detail: None,
            poll_errors: 0,
            error: None,
        }
    }

    fn reconcile(&mut self, channel: Option<&WalletChannel>) {
        if self.status.is_terminal() {
            return;
        }
        self.poll_errors = 0;
        self.state = channel.map(|channel| channel.state.clone());
        self.state_flags = channel.and_then(|channel| channel.state_flags);
        self.shutdown_transaction_hash =
            channel.and_then(|channel| channel.shutdown_transaction_hash.clone());
        self.latest_commitment_transaction_hash =
            channel.and_then(|channel| channel.latest_commitment_transaction_hash.clone());
        self.failure_detail = channel.and_then(|channel| channel.failure_detail.clone());
        self.status = wallet::channel_close_status(
            self.state.as_deref(),
            self.state_flags,
            self.shutdown_transaction_hash.as_deref(),
        );
    }

    fn observe_close_result(&mut self, result: &wallet::WalletChannelCloseResult) {
        self.state = result.state.clone();
        self.state_flags = result.state_flags;
        self.shutdown_transaction_hash = result.shutdown_transaction_hash.clone();
        self.latest_commitment_transaction_hash = result.latest_commitment_transaction_hash.clone();
        self.failure_detail = result.failure_detail.clone();
        self.status = wallet::channel_close_status(
            self.state.as_deref(),
            self.state_flags,
            self.shutdown_transaction_hash.as_deref(),
        );
    }

    fn record_poll_error(&mut self, error: &str) -> ChannelCloseStatus {
        self.poll_errors = self.poll_errors.saturating_add(1);
        self.error = Some(format!(
            "读取关闭状态失败（连续 {} 次）：{error}",
            self.poll_errors
        ));
        if self.poll_errors >= CHANNEL_CLOSE_ERROR_LIMIT {
            self.status = ChannelCloseStatus::Unknown;
        }
        self.status
    }
}

pub struct WalletBridge {
    state: Mutex<UiState>,
    runtime: tokio::runtime::Runtime,
    data_dir: PathBuf,
    secret_password: Mutex<String>,
    pub vault_kind: vault::VaultKind,
    lifecycle_busy: AtomicBool,
    refresh_busy: AtomicBool,
    onchain_generation: AtomicU64,
    flash_seq: AtomicU64,
}

static BRIDGE: OnceLock<WalletBridge> = OnceLock::new();
static POLLER_STARTED: OnceLock<()> = OnceLock::new();

impl WalletBridge {
    pub fn global() -> &'static WalletBridge {
        let bridge = BRIDGE.get_or_init(Self::new);
        POLLER_STARTED.get_or_init(|| {
            bridge.spawn_data_poller();
        });
        bridge
    }

    fn new() -> Self {
        let data_dir = wallet_data_directory();
        if let Err(error) = std::fs::create_dir_all(&data_dir) {
            eprintln!("failed to create data directory: {error}");
        }
        let (secret_password, vault_kind) = vault::load_or_create(&data_dir);
        tracing::info!(vault = ?vault_kind, "keystore password loaded");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build wallet runtime");

        Self {
            state: Mutex::new(UiState::default()),
            runtime,
            data_dir,
            secret_password: Mutex::new(secret_password),
            vault_kind,
            lifecycle_busy: AtomicBool::new(false),
            refresh_busy: AtomicBool::new(false),
            onchain_generation: AtomicU64::new(0),
            flash_seq: AtomicU64::new(0),
        }
    }

    pub fn state(&self) -> UiState {
        self.state.lock().expect("ui state poisoned").clone()
    }

    fn update(&self, apply: impl FnOnce(&mut UiState)) {
        let mut state = self.state.lock().expect("ui state poisoned");
        apply(&mut state);
    }

    /// Records a one-shot user-facing outcome; the root shows it as a toast.
    /// Every button-triggered operation should flash its result so the user
    /// never has to guess whether the click did anything.
    pub fn flash(&self, kind: FlashKind, message: String) {
        let seq = self.flash_seq.fetch_add(1, Ordering::Relaxed) + 1;
        self.update(|state| {
            state.flash = Some(Flash { seq, kind, message });
        });
    }

    /// Logs one event per phase/detail transition, keeping the last 20.
    fn set_snapshot(&self, next: NodeSnapshot) {
        self.update(|state| {
            let changed = state.snapshot.as_ref().is_none_or(|previous| {
                previous.phase != next.phase || previous.detail != next.detail
            });
            if changed {
                state
                    .events
                    .insert(0, format!("{}: {}", phase_name(next.phase), next.detail));
                state.events.truncate(20);
            }
            state.snapshot = Some(next);
        });
    }

    /// Initializes the node, then auto-starts it.
    pub fn initialize_and_start(&'static self) {
        if self.lifecycle_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.update(|state| state.busy = true);
        self.guarded("initialize", move || {
            tracing::info!(data_dir = %self.data_dir.display(), "initializing local node");
            let password = self
                .secret_password
                .lock()
                .expect("password poisoned")
                .clone();
            let snapshot = node::init_local_node(
                NodeNetwork::Testnet,
                self.data_dir.to_string_lossy().into_owned(),
                password,
            );
            self.set_snapshot(snapshot);
            self.start_inner();
            self.finish_lifecycle();
        });
    }

    pub fn start(&'static self) {
        if self.lifecycle_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.update(|state| state.busy = true);
        self.guarded("start", move || {
            self.start_inner();
            self.finish_lifecycle();
        });
    }

    fn start_inner(&'static self) {
        if let Some(current) = self.state().snapshot {
            if matches!(current.phase, NodePhase::Starting | NodePhase::Running) {
                return;
            }
            tracing::info!("starting local Fiber node");
            self.set_snapshot(NodeSnapshot {
                phase: NodePhase::Starting,
                detail: "Opening wallet storage and starting local Fiber node".to_owned(),
                node_id: None,
                ..current
            });
        }

        match self.runtime.block_on(node::start_local_node()) {
            Ok(snapshot) => {
                if let Some(node_id) = &snapshot.node_id {
                    tracing::info!(node_id, "local Fiber node is online");
                }
                self.set_snapshot(snapshot);
                self.flash(FlashKind::Ok, "节点已上线".to_owned());
            }
            Err(error) => {
                tracing::error!(error = %error, "local Fiber node failed to start");
                self.fail(error.clone());
                self.flash(FlashKind::Error, format!("节点启动失败：{error}"));
            }
        }
    }

    /// Marks the machine failed; falls back to a synthetic snapshot when the
    /// node was never initialized.
    fn fail(&self, error: String) {
        match node::mark_local_node_failed(error) {
            Ok(snapshot) => self.set_snapshot(snapshot),
            Err(message) => self.set_snapshot(NodeSnapshot {
                phase: NodePhase::Failed,
                network: NodeNetwork::Testnet,
                detail: message,
                node_id: None,
                backend: "unavailable".to_owned(),
            }),
        }
    }

    pub fn stop(&'static self) {
        if self.lifecycle_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.update(|state| state.busy = true);
        self.guarded("stop", move || {
            if let Some(current) = self.state().snapshot.filter(|current| {
                !matches!(current.phase, NodePhase::Stopping | NodePhase::Stopped)
            }) {
                tracing::info!("stopping local Fiber node");
                self.set_snapshot(NodeSnapshot {
                    phase: NodePhase::Stopping,
                    detail: "Draining work and closing wallet storage".to_owned(),
                    ..current
                });
                match self.runtime.block_on(node::stop_local_node()) {
                    Ok(snapshot) => {
                        self.set_snapshot(snapshot);
                        self.flash(FlashKind::Ok, "节点已停止".to_owned());
                    }
                    Err(error) => {
                        self.fail(error.clone());
                        self.flash(FlashKind::Error, format!("节点停止失败：{error}"));
                    }
                }
                // Keep hashes and phases needed to recover unconfirmed
                // on-chain transactions and Fiber payments. A stopped local
                // node does not make their external outcome disappear.
                self.update(|state| {
                    state.last_invoice = None;
                    state.send_result = None;
                    if state.transfer.payment.is_none()
                        && !matches!(
                            state.transfer.phase,
                            PaymentPhase::Quoted | PaymentPhase::Submitting
                        )
                    {
                        state.transfer = PaymentState::default();
                    }
                    if state.onchain.query_hash.is_none() {
                        state.onchain = OnchainState::default();
                    }
                });
            }
            self.finish_lifecycle();
        });
    }

    fn finish_lifecycle(&self) {
        self.update(|state| state.busy = false);
        self.lifecycle_busy.store(false, Ordering::SeqCst);
    }

    /// Stops then starts the node in place (settings apply, proxy changes).
    pub fn restart(&'static self) {
        if self.lifecycle_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.update(|state| state.busy = true);
        self.guarded("restart", move || {
            tracing::info!("restarting local Fiber node");
            if let Some(current) = self
                .state()
                .snapshot
                .filter(|current| matches!(current.phase, NodePhase::Running | NodePhase::Starting))
            {
                self.set_snapshot(NodeSnapshot {
                    phase: NodePhase::Stopping,
                    detail: "Draining work and closing wallet storage".to_owned(),
                    ..current
                });
                if let Err(error) = self.runtime.block_on(node::stop_local_node()) {
                    tracing::warn!(error = %error, "node stop during restart failed; starting anyway");
                }
            }
            self.start_inner();
            self.finish_lifecycle();
        });
    }

    /// Refreshes node info, channels, and payments; fails soft, keeping the
    /// last good data. One refresh at a time.
    pub fn refresh(&'static self) {
        if self.refresh_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let running = self
            .state()
            .snapshot
            .is_some_and(|snapshot| snapshot.phase == NodePhase::Running);
        if !running {
            self.refresh_busy.store(false, Ordering::SeqCst);
            return;
        }
        self.update(|state| state.data_busy = true);
        self.guarded("refresh", move || {
            let (info, channels, payments, peers) = self.runtime.block_on(async {
                tokio::join!(
                    wallet::wallet_node_info(),
                    wallet::wallet_list_channels(),
                    wallet::wallet_list_payments(50),
                    wallet::wallet_list_peers(),
                )
            });
            self.update(|state| {
                match (info, channels, payments, peers) {
                    (Ok(info), Ok(channels), Ok(payments), Ok(peers)) => {
                        state.node_info = Some(info);
                        state.channels = channels;
                        state.payments = payments;
                        state.peers = peers;
                        state.data_error = None;
                    }
                    (info, channels, payments, peers) => {
                        let error = info
                            .err()
                            .or(channels.err())
                            .or(payments.err())
                            .or(peers.err());
                        if let Some(error) = error {
                            state.data_error = Some(error);
                        }
                    }
                }
                state.data_busy = false;
            });
            self.refresh_busy.store(false, Ordering::SeqCst);
        });
    }

    /// Polls wallet data every 10 s while the node is running. A panicking
    /// iteration is logged and the loop continues.
    fn spawn_data_poller(&'static self) {
        std::thread::spawn(move || {
            loop {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    std::thread::sleep(Duration::from_secs(1));
                    let running = self
                        .state()
                        .snapshot
                        .is_some_and(|snapshot| snapshot.phase == NodePhase::Running);
                    if running {
                        self.refresh();
                        std::thread::sleep(Duration::from_secs(9));
                    }
                }));
                if result.is_err() {
                    tracing::error!("data poller iteration panicked; continuing");
                }
            }
        });
    }

    /// Runs `run` on a background thread; a panic in the wallet core becomes
    /// a Failed snapshot instead of a silently dead operation.
    fn guarded(&'static self, op: &'static str, run: impl FnOnce() + Send + 'static) {
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
            if let Err(panic) = result {
                let message = panic_message(&panic);
                tracing::error!(op, message = %message, "wallet operation panicked");
                self.fail(format!("内部错误（{op}）：{message}"));
            }
        });
    }

    pub fn create_invoice(&'static self, amount_ckb: String, description: String) {
        self.update(|state| {
            state.invoice_busy = true;
            state.invoice_error = None;
        });
        self.guarded("create_invoice", move || {
            let result =
                wallet::ckb_to_shannons(amount_ckb.trim().to_owned()).and_then(|shannons| {
                    let shannons: u64 = shannons
                        .parse()
                        .map_err(|error| format!("invalid CKB amount: {error}"))?;
                    self.runtime
                        .block_on(wallet::wallet_new_invoice(shannons, description.clone()))
                });
            self.update(|state| {
                state.invoice_busy = false;
                match &result {
                    Ok(invoice) => state.last_invoice = Some(invoice.clone()),
                    Err(error) => state.invoice_error = Some(error.clone()),
                }
            });
            match &result {
                Ok(invoice) => self.flash(
                    FlashKind::Ok,
                    format!("发票已创建 · {} CKB", shannons_to_ckb(&invoice.amount)),
                ),
                Err(error) => self.flash(FlashKind::Error, format!("创建发票失败：{error}")),
            }
        });
    }

    pub fn send_payment(&'static self, invoice: String) {
        self.send_payment_request(WalletSendPaymentRequest::for_invoice(invoice));
    }

    pub fn send_payment_request(&'static self, request: WalletSendPaymentRequest) {
        let request = match request.into_validated() {
            Ok(request) => request,
            Err(error) => {
                self.update(|state| {
                    state.send_busy = false;
                    state.send_result = None;
                    state.send_error = Some(error.clone());
                });
                self.flash(FlashKind::Error, format!("支付参数无效：{error}"));
                return;
            }
        };
        let request = WalletSendPaymentRequest {
            dry_run: false,
            ..request
        };
        self.update(|state| {
            state.send_busy = true;
            state.send_error = None;
            state.send_result = None;
        });
        self.guarded("send_payment", move || {
            match self
                .runtime
                .block_on(wallet::wallet_send_payment_request(request))
            {
                Ok(WalletSendPaymentResult::Quote { quote }) => {
                    self.update(|state| {
                        state.send_result = Some(format!(
                            "报价：金额 {} shannons，预计路由费 {} shannons；未发送支付。",
                            quote.amount_shannons, quote.fee_shannons
                        ));
                        state.send_busy = false;
                    });
                    self.flash(
                        FlashKind::Warn,
                        format!(
                            "dry-run 报价已取得（{} shannons，费用 {}）；这是报价，不是支付",
                            quote.amount_shannons, quote.fee_shannons
                        ),
                    );
                }
                Ok(WalletSendPaymentResult::Submitted {
                    payment,
                    terminal_error,
                }) => {
                    let message = format!(
                        "Payment {}: hash {}, fee {} shannons.",
                        payment.status, payment.payment_hash, payment.fee_shannons
                    );
                    let payment_status = payment.status;
                    let payment_hash = payment.payment_hash.clone();
                    let failed_detail = terminal_error.clone().or_else(|| {
                        (payment.status == WalletPaymentStatus::Failed)
                            .then(|| payment.failed_error.clone())
                            .flatten()
                    });
                    self.update(|state| {
                        state.send_result = Some(message);
                        state.send_error = failed_detail.clone();
                        state.send_busy = false;
                    });
                    match payment_status {
                        WalletPaymentStatus::Success => {
                            self.flash(FlashKind::Ok, format!("支付成功 · hash {payment_hash}"))
                        }
                        WalletPaymentStatus::Failed => self.flash(
                            FlashKind::Error,
                            format!(
                                "支付失败 · hash {payment_hash}{}",
                                failed_detail
                                    .as_deref()
                                    .map(|detail| format!("：{detail}"))
                                    .unwrap_or_default()
                            ),
                        ),
                        _ => self.flash(
                            FlashKind::Warn,
                            format!("支付已提交，未取得终态 · hash {payment_hash}"),
                        ),
                    }
                    self.refresh();
                }
                Err(error) => {
                    self.update(|state| {
                        state.send_error = Some(error.clone());
                        state.send_busy = false;
                    });
                    self.flash(FlashKind::Error, format!("发送支付失败：{error}"));
                }
            }
        });
    }

    /// Executes a strictly parsed dApp action against the same wallet core as
    /// the local UI. Approval and origin/session checks happen in the host
    /// before this method is called.
    pub fn execute_dapp(
        &'static self,
        method: ProviderMethod,
        action: WalletAction,
        request_id: &str,
    ) -> String {
        let result = self.execute_dapp_inner(action);
        let response = match result {
            Ok(value) => dapp_protocol::response_ok(request_id, value),
            Err(error) => {
                tracing::warn!(
                    method = method.canonical(),
                    code = error.code,
                    message = %error.message,
                    "dApp wallet action failed"
                );
                dapp_protocol::response_error(request_id, error)
            }
        };
        dapp_protocol::encode_response(&response)
    }

    fn execute_dapp_inner(
        &'static self,
        action: WalletAction,
    ) -> Result<serde_json::Value, RpcError> {
        match action {
            WalletAction::ReadNodeInfo => {
                let info = self.run_dapp(wallet::wallet_node_info())?;
                Ok(dapp_protocol::node_info_result(&info))
            }
            WalletAction::ReadBalance => {
                let info = self.run_dapp(wallet::wallet_node_info())?;
                Ok(dapp_protocol::balance_result(
                    self.network_name(),
                    &info.funding_balance_shannons,
                    &info.channel_balance_shannons,
                ))
            }
            WalletAction::ReadChannels => {
                let channels = self.run_dapp(wallet::wallet_list_channels())?;
                Ok(dapp_protocol::channels_result(&channels))
            }
            WalletAction::ReadPeers => {
                let peers = self.run_dapp(wallet::wallet_list_peers())?;
                Ok(dapp_protocol::peers_result(&peers))
            }
            WalletAction::CreateInvoice(params) => {
                let invoice = self.run_dapp(wallet::wallet_new_invoice(
                    params.amount_shannons,
                    params.description,
                ))?;
                Ok(dapp_protocol::invoice_result(&invoice))
            }
            WalletAction::SendPayment(params) => self.send_payment_action(params, false),
            WalletAction::DryRunPayment(params) => self.send_payment_action(params, true),
            WalletAction::OpenChannel(params) => {
                let peers = self.run_dapp(wallet::wallet_list_peers())?;
                if !peers
                    .iter()
                    .any(|peer| peer.pubkey.eq_ignore_ascii_case(&params.peer_pubkey))
                {
                    return Err(RpcError::new(
                        error_code::UNAVAILABLE,
                        "the requested peer is not connected; connect it in the wallet before opening a channel",
                    )
                    .with_data(serde_json::json!({
                        "peerPubkey": params.peer_pubkey,
                        "connectedPeers": peers.iter().map(|peer| &peer.pubkey).collect::<Vec<_>>(),
                    })));
                }
                let channel = self.run_dapp(wallet::wallet_open_channel(
                    params.peer_pubkey,
                    params.funding_amount_shannons,
                    params.public_channel,
                ))?;
                Ok(dapp_protocol::open_channel_result(&channel))
            }
            WalletAction::CloseChannel(params) => {
                let mode = if params.force {
                    ChannelCloseMode::Force
                } else {
                    ChannelCloseMode::Cooperative
                };
                let result = self.run_dapp(wallet::wallet_close_channel(ChannelCloseRequest {
                    channel_id: params.channel_id,
                    mode,
                    fee_rate: None,
                }))?;
                self.refresh();
                Ok(dapp_protocol::close_channel_result(&result))
            }
            WalletAction::GetPayment(params) => {
                let payment = self.run_dapp(wallet::wallet_get_payment(params.payment_hash))?;
                Ok(dapp_protocol::payment_record_result(&payment))
            }
            WalletAction::Connect(_) => Err(RpcError::new(
                error_code::INVALID_REQUEST,
                "fiber_connect is handled by the host approval flow",
            )),
        }
    }

    fn run_dapp<T>(
        &'static self,
        future: impl std::future::Future<Output = Result<T, String>>,
    ) -> Result<T, RpcError> {
        let running = self
            .state()
            .snapshot
            .is_some_and(|snapshot| snapshot.phase == NodePhase::Running);
        if !running {
            return Err(RpcError::new(
                error_code::UNAVAILABLE,
                "the wallet node is not running",
            ));
        }
        self.runtime
            .block_on(future)
            .map_err(|message| dapp_core_error(&message))
    }

    fn network_name(&self) -> &'static str {
        match self.state().snapshot.map(|snapshot| snapshot.network) {
            Some(NodeNetwork::Mainnet) => "Mainnet",
            _ => "Testnet",
        }
    }

    fn send_payment_action(
        &'static self,
        params: PayParams,
        dry_run: bool,
    ) -> Result<serde_json::Value, RpcError> {
        let keysend = params.target_pubkey.is_some();
        let timeout_seconds = if dry_run { None } else { Some(120) };
        let request = WalletSendPaymentRequest {
            invoice: params.invoice,
            target_pubkey: params.target_pubkey,
            amount: params.amount_shannons.map(u128::from),
            max_fee_amount: params.max_fee_shannons.map(u128::from),
            timeout_seconds,
            dry_run,
            keysend,
        }
        .into_validated()
        .map_err(|message| RpcError::new(error_code::INVALID_PARAMS, message))?;
        let result = self.run_dapp(wallet::wallet_send_payment_request(request))?;
        self.refresh();
        Ok(dapp_protocol::payment_result(&result))
    }

    /// Runs a keysend dry run for the current form fields. A quote is not a
    /// payment and never sends an HTLC.
    pub fn quote_transfer(
        &'static self,
        target_pubkey: String,
        amount_ckb: String,
        max_fee_ckb: String,
    ) {
        let request = match build_transfer_request(&target_pubkey, &amount_ckb, &max_fee_ckb, true)
        {
            Ok(request) => request,
            Err(error) => {
                self.update(|state| {
                    state.transfer = PaymentState {
                        phase: PaymentPhase::Failed,
                        error: Some(error),
                        ..PaymentState::default()
                    };
                });
                return;
            }
        };
        self.update(|state| {
            state.transfer = PaymentState {
                phase: PaymentPhase::Quoting,
                ..PaymentState::default()
            };
        });
        self.guarded("quote_transfer", move || {
            let result = self
                .runtime
                .block_on(wallet::wallet_send_payment_request(request));
            match result {
                Ok(WalletSendPaymentResult::Quote { quote }) => {
                    let amount = quote.amount_shannons.clone();
                    let fee = quote.fee_shannons.clone();
                    self.update(|state| {
                        state.transfer.phase = PaymentPhase::Quoted;
                        state.transfer.quote = Some(quote);
                        state.transfer.error = None;
                    });
                    self.flash(
                        FlashKind::Ok,
                        format!(
                            "dry-run 报价已取得（{} shannons，路由费 ≤ {} shannons），确认后才会发送",
                            amount, fee
                        ),
                    );
                }
                Ok(WalletSendPaymentResult::Submitted { .. }) => {
                    self.update(|state| {
                        state.transfer.phase = PaymentPhase::Failed;
                        state.transfer.error =
                            Some("dry-run unexpectedly created a payment session".to_owned());
                    });
                    self.flash(
                        FlashKind::Error,
                        "内部错误：dry-run 意外创建了支付会话".to_owned(),
                    );
                }
                Err(error) => {
                    self.update(|state| {
                        state.transfer.phase = PaymentPhase::Failed;
                        state.transfer.error = Some(format!("dry-run 报价失败：{error}"));
                    });
                    self.flash(FlashKind::Error, format!("dry-run 报价失败：{error}"));
                }
            }
        });
    }

    /// Sends the exact request returned by the last successful dry run. The
    /// send is followed by terminal polling; a timeout remains `Unknown`.
    pub fn confirm_transfer(&'static self) {
        let request = {
            let state = self.state();
            if state.transfer.phase != PaymentPhase::Quoted {
                return;
            }
            let Some(request) = state
                .transfer
                .quote
                .as_ref()
                .map(|quote| quote.request.clone())
            else {
                return;
            };
            WalletSendPaymentRequest {
                dry_run: false,
                ..request
            }
        };
        self.update(|state| {
            state.transfer.phase = PaymentPhase::Submitting;
            state.transfer.payment = None;
            state.transfer.error = None;
        });
        self.guarded("confirm_transfer", move || {
            let payment_hash = request_payment_hash(&request);
            let result = self.runtime.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(TRANSFER_TIMEOUT_SECONDS + 1),
                    wallet::wallet_send_payment_request(request),
                )
                .await
            });
            match result {
                Ok(Ok(WalletSendPaymentResult::Submitted {
                    payment,
                    terminal_error,
                })) => {
                    if let Some(error) = terminal_error {
                        self.mark_payment_unknown(
                            payment,
                            format!(
                                "Fiber 已接受付款，但终态查询失败：{error}。这不是失败证明，请按 payment hash 继续核对。"
                            ),
                        );
                    } else {
                        self.finish_payment(payment);
                    }
                }
                Ok(Ok(WalletSendPaymentResult::Quote { .. })) => {
                    self.update(|state| {
                        state.transfer.phase = PaymentPhase::Failed;
                        state.transfer.error =
                            Some("真实发送意外返回 dry-run 报价；未发送".to_owned());
                    });
                }
                Ok(Err(error)) => {
                    if let Some(payment_hash) = payment_hash {
                        self.recover_payment(payment_hash, error);
                    } else {
                        self.fail_after_send(error);
                    }
                }
                Err(_) => {
                    if let Some(payment_hash) = payment_hash {
                        self.recover_payment(
                            payment_hash,
                            format!(
                                "{} 秒内未完成发送/终态等待",
                                TRANSFER_TIMEOUT_SECONDS + 1
                            ),
                        );
                    } else {
                        self.mark_send_result_unknown(format!(
                            "{} 秒内未收到 Fiber 发送结果；不能确认支付是否已创建或是否已发送。请刷新支付记录核对。",
                            TRANSFER_TIMEOUT_SECONDS + 1
                        ));
                    }
                }
            }
        });
    }

    /// A failed send RPC can still have persisted a session. Resolve it by the
    /// deterministic keysend hash when possible; otherwise show an explicit
    /// indeterminate state rather than declaring that no payment happened.
    fn recover_payment(&'static self, payment_hash: String, cause: String) {
        match self
            .runtime
            .block_on(wallet::wallet_poll_payment_terminal(
                payment_hash.clone(),
                5,
            ))
        {
            Ok(payment) => self.set_terminal_payment(payment),
            Err(error) => self.mark_payment_unknown(
                WalletPayment {
                    payment_hash,
                    status: WalletPaymentStatus::Created,
                    failed_error: None,
                    fee_shannons: "0".to_owned(),
                    created_at_ms: 0,
                    last_updated_at_ms: 0,
                },
                format!(
                    "发送流程未确认终态：{cause}。随后查询也未取得终态：{error}。这不是失败证明，请刷新支付记录核对。"
                ),
            ),
        }
    }

    fn finish_payment(&'static self, mut payment: WalletPayment) {
        if payment.status.is_terminal() {
            self.set_terminal_payment(payment);
            return;
        }

        let payment_hash = payment.payment_hash.clone();
        let started = std::time::Instant::now();
        let deadline = Duration::from_secs(TRANSFER_POLL_MAX_SECONDS);
        let mut poll_errors = 0_u8;
        loop {
            self.update(|state| {
                state.transfer.phase = PaymentPhase::Submitting;
                state.transfer.payment = Some(payment.clone());
            });

            let remaining = deadline.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                self.mark_payment_unknown(
                    payment,
                    format!(
                        "{} 秒内未取得 Success/Failed；支付可能仍在结算，这不是失败。",
                        TRANSFER_POLL_MAX_SECONDS
                    ),
                );
                return;
            }
            std::thread::sleep(PAYMENT_POLL_INTERVAL.min(remaining));

            match self
                .runtime
                .block_on(wallet::wallet_get_payment(payment_hash.clone()))
            {
                Ok(next) => {
                    poll_errors = 0;
                    payment = next;
                    if payment.status.is_terminal() {
                        self.set_terminal_payment(payment);
                        return;
                    }
                }
                Err(error) => {
                    poll_errors += 1;
                    if poll_errors >= PAYMENT_POLL_ERROR_LIMIT {
                        self.mark_payment_unknown(
                            payment,
                            format!(
                                "无法查询支付终态（连续 {poll_errors} 次失败）：{error}；不是失败证明。"
                            ),
                        );
                        return;
                    }
                }
            }
        }
    }

    fn set_terminal_payment(&'static self, payment: WalletPayment) {
        let phase = if payment.status == WalletPaymentStatus::Success {
            PaymentPhase::Success
        } else {
            PaymentPhase::Failed
        };
        let error = (phase == PaymentPhase::Failed)
            .then(|| payment.failed_error.clone())
            .flatten();
        let payment_hash = payment.payment_hash.clone();
        self.update(|state| {
            state.transfer.phase = phase;
            state.transfer.error = error.clone();
            state.transfer.payment = Some(payment);
        });
        match phase {
            PaymentPhase::Success => self.flash(
                FlashKind::Ok,
                format!("Fiber 转账成功 · hash {payment_hash}"),
            ),
            _ => self.flash(
                FlashKind::Error,
                format!(
                    "Fiber 转账失败 · hash {payment_hash}{}",
                    error
                        .as_deref()
                        .map(|detail| format!("：{detail}"))
                        .unwrap_or_default()
                ),
            ),
        }
        self.refresh();
    }

    fn mark_payment_unknown(&'static self, payment: WalletPayment, error: String) {
        self.update(|state| {
            state.transfer.phase = PaymentPhase::Unknown;
            state.transfer.error = Some(error.clone());
            state.transfer.payment = Some(payment);
        });
        self.flash(
            FlashKind::Warn,
            format!("转账未确认终态，请按 payment hash 核对：{error}"),
        );
        self.refresh();
    }

    fn fail_after_send(&'static self, error: String) {
        self.update(|state| {
            state.transfer.phase = PaymentPhase::Failed;
            state.transfer.error = Some(format!("提交支付失败：{error}"));
        });
        self.flash(FlashKind::Error, format!("提交支付失败：{error}"));
    }

    fn mark_send_result_unknown(&'static self, error: String) {
        self.update(|state| {
            state.transfer.phase = PaymentPhase::Unknown;
            state.transfer.error = Some(error.clone());
        });
        self.flash(FlashKind::Warn, format!("转账结果未确认：{error}"));
        self.refresh();
    }

    pub fn clear_transfer(&'static self) {
        self.update(|state| state.transfer = PaymentState::default());
    }

    /// Validates the CKB Testnet form and enters the explicit confirmation
    /// layer. Nothing is signed or broadcast until [`Self::confirm_onchain_transfer`].
    pub fn prepare_onchain_transfer(
        &'static self,
        target_address: String,
        amount_ckb: String,
        max_fee_ckb: String,
    ) {
        if !matches!(
            self.state().onchain.phase,
            OnchainPhase::Idle | OnchainPhase::Failed
        ) {
            return;
        }
        let outcome =
            validate_onchain_form(&target_address, &amount_ckb, &max_fee_ckb).and_then(|_| {
                wallet::parse_testnet_address(target_address.trim())
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            });
        match outcome {
            Ok(()) => self.update(|state| {
                state.onchain = OnchainState {
                    phase: OnchainPhase::Confirming,
                    error: None,
                    ..OnchainState::default()
                };
            }),
            Err(error) => {
                self.update(|state| {
                    state.onchain.phase = OnchainPhase::Idle;
                    state.onchain.error = Some(error.clone());
                });
                self.flash(FlashKind::Error, format!("转账参数未通过校验：{error}"));
            }
        }
    }

    pub fn cancel_onchain_transfer(&'static self) {
        if self.state().onchain.phase != OnchainPhase::Confirming {
            return;
        }
        self.update(|state| {
            state.onchain = OnchainState::default();
        });
    }

    /// Signs and broadcasts the exact values the user confirmed.
    ///
    /// The form signals are intentionally not read here. This prevents a quick
    /// edit between confirmation and click from changing the submitted amount.
    pub fn confirm_onchain_transfer(
        &'static self,
        target_address: String,
        amount_ckb: String,
        max_fee_ckb: String,
    ) {
        if self.state().onchain.phase != OnchainPhase::Confirming {
            return;
        }
        let (amount, max_fee) =
            match validate_onchain_form(&target_address, &amount_ckb, &max_fee_ckb) {
                Ok(values) => values,
                Err(error) => {
                    self.update(|state| {
                        state.onchain.phase = OnchainPhase::Failed;
                        state.onchain.error = Some(error.clone());
                    });
                    self.flash(FlashKind::Error, format!("转账参数未通过校验：{error}"));
                    return;
                }
            };
        let target = target_address.trim().to_owned();
        self.update(|state| {
            state.onchain = OnchainState {
                phase: OnchainPhase::Broadcasting,
                error: None,
                ..OnchainState::default()
            };
        });

        self.guarded("onchain_transfer", move || {
            let result = self
                .runtime
                .block_on(wallet::wallet_onchain_transfer_result(
                    target.clone(),
                    format_ckb(amount),
                    format_ckb(max_fee),
                ));
            match result {
                Ok(transfer) => {
                    let tx_hash = transfer.tx_hash.clone();
                    self.start_onchain_status(&tx_hash, Some(transfer));
                }
                Err(error) => {
                    if let Some(tx_hash) = error.uncertain_tx_hash() {
                        let tx_hash = tx_hash.to_owned();
                        let message = error.to_string();
                        self.update(|state| {
                            state.onchain = OnchainState {
                                phase: OnchainPhase::Uncertain,
                                query_hash: Some(tx_hash.clone()),
                                error: Some(message.clone()),
                                ..OnchainState::default()
                            };
                        });
                        self.flash(
                            FlashKind::Warn,
                            format!("广播结果不确定：{message}；请按哈希查询，不要直接重试"),
                        );
                        self.poll_onchain_status(tx_hash);
                    } else {
                        self.fail_onchain(error);
                    }
                }
            }
        });
    }

    /// Queries the chain RPC directly. This works after the local node stops,
    /// because transaction status has no dependency on Fiber state.
    pub fn refresh_onchain_status(&'static self) {
        let Some(tx_hash) = self.state().onchain.query_hash else {
            return;
        };
        self.poll_onchain_status(tx_hash);
    }

    pub fn clear_onchain(&'static self) {
        let phase = self.state().onchain.phase;
        if matches!(
            phase,
            OnchainPhase::Confirming
                | OnchainPhase::Broadcasting
                | OnchainPhase::Uncertain
                | OnchainPhase::Unknown
        ) {
            return;
        }
        self.update(|state| state.onchain = OnchainState::default());
    }

    fn start_onchain_status(&'static self, tx_hash: &str, transfer: Option<WalletOnchainTransfer>) {
        self.update(|state| {
            state.onchain = OnchainState {
                phase: OnchainPhase::Broadcast,
                transfer,
                query_hash: Some(tx_hash.to_owned()),
                error: None,
                broadcast_accepted: true,
                ..OnchainState::default()
            };
        });
        self.flash(
            FlashKind::Ok,
            format!("交易已广播 · {tx_hash}；广播成功≠已确认，请等待链上状态"),
        );
        self.poll_onchain_status(tx_hash.to_owned());
    }

    fn poll_onchain_status(&'static self, tx_hash: String) {
        let tx_hash = tx_hash.trim().to_ascii_lowercase();
        let generation = self.onchain_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.guarded("onchain_status", move || {
            let mut last_status = None;
            let mut last_error = None;
            for poll in 0..ONCHAIN_STATUS_MAX_POLLS {
                if self.onchain_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                match self
                    .runtime
                    .block_on(wallet::wallet_onchain_transfer_status(tx_hash.clone()))
                {
                    Ok(status) => {
                        if self.onchain_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        last_error = None;
                        last_status = Some(status.clone());
                        self.update(|state| {
                            if state.onchain.query_hash.as_deref() != Some(tx_hash.as_str()) {
                                return;
                            }
                            state.onchain.status = Some(status.clone());
                            state.onchain.last_checked_at_ms =
                                Some(chrono::Utc::now().timestamp_millis() as u64);
                            state.onchain.phase = match status.state {
                                OnchainTxState::Committed => OnchainPhase::Committed,
                                OnchainTxState::Rejected => OnchainPhase::Rejected,
                                OnchainTxState::Pending | OnchainTxState::Proposed => {
                                    OnchainPhase::Broadcast
                                }
                                OnchainTxState::Unknown => OnchainPhase::Unknown,
                            };
                        });
                        if matches!(
                            status.state,
                            OnchainTxState::Committed | OnchainTxState::Rejected
                        ) {
                            match status.state {
                                OnchainTxState::Committed => self.flash(
                                    FlashKind::Ok,
                                    format!("链上交易已确认 · {tx_hash}"),
                                ),
                                OnchainTxState::Rejected => self.flash(
                                    FlashKind::Error,
                                    format!("链上交易被拒绝 · {tx_hash}"),
                                ),
                                _ => {}
                            }
                            self.refresh();
                            return;
                        }
                    }
                    Err(error) => {
                        if self.onchain_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        last_error = Some(error);
                    }
                }
                if poll + 1 < ONCHAIN_STATUS_MAX_POLLS {
                    std::thread::sleep(ONCHAIN_STATUS_POLL_INTERVAL);
                }
            }

            let message = match (last_status.as_ref(), last_error) {
                (Some(status), _) => {
                    let accepted = self.state().onchain.broadcast_accepted;
                    if accepted {
                        format!(
                            "广播已接受，但 60 秒内未确认；最后状态为 {}。请稍后刷新，不能把广播当成已确认。",
                            status.state.label()
                        )
                    } else {
                        format!(
                            "交易哈希已保留，但无法证明广播 RPC 已接受；最后状态为 {}。请按哈希核对，不要直接重试。",
                            status.state.label()
                        )
                    }
                }
                (None, Some(error)) => format!(
                    "广播哈希已保留，但查询链上状态失败：{error}。请稍后刷新，不要重复支付。"
                ),
                (None, None) => "未取得链上状态；请稍后刷新。".to_owned(),
            };
            self.update(|state| {
                if state.onchain.query_hash.as_deref() != Some(tx_hash.as_str())
                    || self.onchain_generation.load(Ordering::SeqCst) != generation
                {
                    return;
                }
                if state.onchain.phase == OnchainPhase::Uncertain {
                    state.onchain.error = Some(message);
                } else {
                    state.onchain.phase = OnchainPhase::Unknown;
                    state.onchain.error = Some(message);
                }
            });
        });
    }

    fn fail_onchain(&self, error: OnchainTransferError) {
        self.update(|state| {
            state.onchain.phase = OnchainPhase::Failed;
            state.onchain.error = Some(error.to_string());
        });
        self.flash(FlashKind::Error, format!("链上转账未提交：{error}"));
    }

    /// Drops a quote when a form field changes after that quote was created.
    ///
    /// `quote_transfer` clears the previous quote before starting a new dry
    /// run. This check keeps a stale quote from remaining confirmable while a
    /// fresh quote is still in flight.
    pub fn invalidate_transfer_quote(
        &'static self,
        target_pubkey: &str,
        amount_ckb: &str,
        max_fee_ckb: &str,
    ) {
        let current = (
            target_pubkey.trim().to_owned(),
            amount_ckb.trim().to_owned(),
            max_fee_ckb.trim().to_owned(),
        );
        let quote_fields = self
            .state()
            .transfer
            .quote
            .and_then(|quote| transfer_request_fields(&quote.request));
        if quote_fields
            .as_ref()
            .is_some_and(|quote_fields| quote_fields != &current)
        {
            self.clear_transfer();
        }
    }

    /// Refuses an operation that needs the node stopped, with a UI-facing
    /// message.
    fn require_stopped(&self, action: &str) -> Result<(), String> {
        match self.state().snapshot {
            None => Ok(()),
            Some(snapshot) => match snapshot.phase {
                NodePhase::Stopped | NodePhase::Failed => Ok(()),
                other => Err(format!(
                    "请先停止节点再{action}（当前状态：{}）",
                    phase_name(other)
                )),
            },
        }
    }

    /// Connects (optionally) and opens a channel to a peer, then refreshes.
    /// `peer` is the 33-byte hex pubkey; pass `connect_address` when the peer
    /// is not connected yet.
    pub fn open_channel(
        &'static self,
        peer: String,
        funding_amount_ckb: String,
        public_channel: bool,
        connect_address: Option<String>,
    ) {
        self.update(|state| {
            state.open_channel_busy = true;
            state.open_channel_error = None;
        });
        self.guarded("open_channel", move || {
            let result = (|| -> Result<WalletOpenedChannel, String> {
                let peer_id = peer.trim().to_ascii_lowercase();
                if let Some(address) = connect_address.filter(|a| !a.trim().is_empty()) {
                    self.runtime
                        .block_on(wallet::wallet_connect_peer(address.trim().to_owned()))?;
                }
                let shannons = wallet::ckb_to_shannons(funding_amount_ckb.trim().to_owned())?;
                let shannons: u64 = shannons
                    .parse()
                    .map_err(|error| format!("CKB 金额超出范围: {error}"))?;
                if shannons == 0 {
                    return Err("开通道金额必须大于 0".to_owned());
                }
                self.wait_for_connected_peer(&peer_id)?;
                self.runtime.block_on(wallet::wallet_open_channel(
                    peer_id,
                    shannons,
                    public_channel,
                ))
            })();
            self.update(|state| {
                state.open_channel_busy = false;
                match &result {
                    Ok(_) => {}
                    Err(error) => state.open_channel_error = Some(error.clone()),
                }
            });
            match &result {
                Ok(channel) => self.flash(
                    FlashKind::Ok,
                    format!(
                        "开通道请求已提交 · 临时 id {}；等待 funding 交易上链确认",
                        channel.temporary_channel_id
                    ),
                ),
                Err(error) => self.flash(FlashKind::Error, format!("开通道失败：{error}")),
            }
            if result.is_ok() {
                self.refresh();
            }
        });
    }

    /// `connect_peer` only starts the dial. Opening before the peer appears
    /// in `list_peers` fails with a misleading "not connected" error.
    fn wait_for_connected_peer(&self, peer_id: &str) -> Result<(), String> {
        let mut last_error = None;
        for attempt in 0..PEER_READY_MAX_ATTEMPTS {
            match self.runtime.block_on(wallet::wallet_list_peers()) {
                Ok(peers) => {
                    if peers
                        .iter()
                        .any(|peer| peer.pubkey.eq_ignore_ascii_case(peer_id))
                    {
                        return Ok(());
                    }
                    last_error = None;
                }
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < PEER_READY_MAX_ATTEMPTS {
                std::thread::sleep(PEER_READY_POLL_INTERVAL);
            }
        }
        Err(match last_error {
            Some(error) => {
                format!("无法确认对端已连接：{error}。通道未提交。")
            }
            None => format!(
                "对端 {peer_id} 在等待 {} 秒后仍未出现在已连接 peer 列表，通道未提交。",
                PEER_READY_MAX_ATTEMPTS / 2
            ),
        })
    }

    /// Requests a close, then keeps polling that channel until FNN reports a
    /// terminal state. The node-wide refresh runs on success so balances,
    /// channels, and all rows reflect the updated node state.
    pub fn close_channel(
        &'static self,
        channel_id: String,
        mode: ChannelCloseMode,
        fee_rate: Option<u64>,
    ) {
        let request = ChannelCloseRequest {
            channel_id: channel_id.trim().to_owned(),
            mode,
            fee_rate,
        };
        self.start_channel_close(request);
    }

    /// Starts a close from an already constructed core DTO. The dApp provider
    /// can reuse this path without translating the request into widget args.
    pub fn close_channel_request(&'static self, request: ChannelCloseRequest) {
        self.start_channel_close(request);
    }

    fn start_channel_close(&'static self, request: ChannelCloseRequest) {
        let key = crate::channel_close::canonical_channel_key(&request.channel_id);
        let mode = request.mode;
        let mut state = self.state.lock().expect("ui state poisoned");
        if state.closing_channel_id.is_some() {
            return;
        }
        if let Err(error) = wallet::validate_channel_close_request(&request) {
            state
                .channel_close_attempts
                .retain(|attempt| attempt.normalized_channel_id != key);
            let mut attempt = ChannelCloseAttempt::new(&request.channel_id, key, mode);
            attempt.status = ChannelCloseStatus::Failed;
            attempt.error = Some(error);
            state.channel_close_attempts.insert(0, attempt);
            return;
        }
        state.closing_channel_id = Some(key.clone());
        state
            .channel_close_attempts
            .retain(|attempt| attempt.normalized_channel_id != key);
        state.channel_close_attempts.insert(
            0,
            ChannelCloseAttempt::new(&request.channel_id, key.clone(), mode),
        );
        drop(state);

        self.guarded("close_channel", move || {
            let result = self.runtime.block_on(wallet::wallet_close_channel(request));
            match result {
                Ok(result) => {
                    self.update(|state| {
                        if let Some(attempt) = state
                            .channel_close_attempts
                            .iter_mut()
                            .find(|attempt| attempt.normalized_channel_id == key)
                        {
                            attempt.observe_close_result(&result);
                        }
                    });
                    self.flash(
                        FlashKind::Ok,
                        format!(
                            "关闭请求已提交（{}），等待链上结算；状态在通道列表跟踪",
                            if mode == ChannelCloseMode::Force {
                                "强制关闭"
                            } else {
                                "协调关闭"
                            }
                        ),
                    );
                    self.refresh();
                    self.poll_channel_close(key, mode);
                }
                Err(error) => self.fail_channel_close(&key, error),
            }
        });
    }

    /// Reads channel state until `Closed` or a failed request. The close RPC
    /// has already returned, so `closing_channel_id` protects only this polling
    /// phase from duplicate submissions in the UI.
    fn poll_channel_close(&'static self, key: String, mode: ChannelCloseMode) {
        const POLL_INTERVAL: Duration = Duration::from_secs(2);
        self.guarded("poll_channel_close", move || {
            for _ in 0..CHANNEL_CLOSE_MAX_POLLS {
                let result = self
                    .runtime
                    .block_on(wallet::wallet_list_channels());
                let close_status = match result {
                    Ok(channels) => {
                        let channel = channels.iter().find(|channel| {
                            crate::channel_close::canonical_channel_key(&channel.channel_id) == key
                        });
                        let mut status = ChannelCloseStatus::Unknown;
                        self.update(|ui| {
                            if let Some(attempt) = ui
                                .channel_close_attempts
                                .iter_mut()
                                .find(|attempt| attempt.normalized_channel_id == key)
                            {
                                attempt.reconcile(channel);
                                attempt.error = None;
                                status = attempt.status;
                            }
                        });
                        status
                    }
                    Err(error) => {
                        let mut status = ChannelCloseStatus::Unknown;
                        self.update(|ui| {
                            if let Some(attempt) = ui
                                .channel_close_attempts
                                .iter_mut()
                                .find(|attempt| attempt.normalized_channel_id == key)
                            {
                                status = attempt.record_poll_error(&error);
                            }
                        });
                        status
                    }
                };

                match close_status {
                    ChannelCloseStatus::Closed
                    | ChannelCloseStatus::OpeningFailed
                    | ChannelCloseStatus::Failed
                    | ChannelCloseStatus::FailedWithDetail => {
                        self.update(|ui| ui.closing_channel_id = None);
                        match close_status {
                            ChannelCloseStatus::Closed => self.flash(
                                FlashKind::Ok,
                                "通道已关闭（链上结算完成）".to_owned(),
                            ),
                            _ => self.flash(
                                FlashKind::Error,
                                "通道关闭失败，详情见通道列表".to_owned(),
                            ),
                        }
                        self.refresh();
                        return;
                    }
                    ChannelCloseStatus::InProgress
                    | ChannelCloseStatus::WaitingOnchainSettlement
                    | ChannelCloseStatus::Submitted
                    | ChannelCloseStatus::Unknown => {}
                }
                if close_status != ChannelCloseStatus::Unknown {
                    std::thread::sleep(POLL_INTERVAL);
                }
            }

            self.update(|ui| {
                ui.closing_channel_id = None;
                if let Some(attempt) = ui
                    .channel_close_attempts
                    .iter_mut()
                    .find(|attempt| attempt.normalized_channel_id == key)
                {
                    let last_state = attempt.state.as_deref().unwrap_or("未知");
                    attempt.status = ChannelCloseStatus::Unknown;
                    attempt.error = Some(format!(
                        "{mode:?} 关闭请求已提交，但 60 秒内未确认完成（最后状态：{last_state}）。请在通道列表继续核对，不能据此判断失败或成功。"
                    ));
                }
            });
            self.flash(
                FlashKind::Warn,
                "关闭请求已提交但 60 秒内未确认完成，请在通道列表继续核对".to_owned(),
            );
            self.refresh();
        });
    }

    fn fail_channel_close(&self, key: &str, error: String) {
        self.update(|state| {
            state.closing_channel_id = None;
            if let Some(attempt) = state
                .channel_close_attempts
                .iter_mut()
                .find(|attempt| attempt.normalized_channel_id == key)
            {
                attempt.status = ChannelCloseStatus::Failed;
                attempt.error = Some(error.clone());
            }
        });
        self.flash(FlashKind::Error, format!("通道关闭失败：{error}"));
    }

    /// Re-encrypts the keystore files under a new password and updates the
    /// vault. Requires the node to be stopped.
    pub fn change_password(&'static self, new_password: String) {
        self.update(|state| {
            state.password_busy = true;
            state.password_error = None;
            state.password_message = None;
        });
        self.guarded("change_password", move || {
            let outcome: Result<(), String> = (|| {
                self.require_stopped("轮换密钥加密密码")?;
                let old = self
                    .secret_password
                    .lock()
                    .expect("password poisoned")
                    .clone();
                fiber_wallet_core::keystore::change_keystore_password(
                    &self.data_dir,
                    &old,
                    new_password.trim(),
                )?;
                vault::store(new_password.trim())
                    .map_err(|error| format!("新密码写入钥匙串失败: {error}"))?;
                *self.secret_password.lock().expect("password poisoned") =
                    new_password.trim().to_owned();
                Ok(())
            })();
            self.update(|state| {
                state.password_busy = false;
                match &outcome {
                    Ok(()) => state.password_message = Some("密钥加密密码已轮换".to_owned()),
                    Err(error) => state.password_error = Some(error.clone()),
                }
            });
            match &outcome {
                Ok(()) => self.flash(FlashKind::Ok, "密钥加密密码已轮换".to_owned()),
                Err(error) => self.flash(FlashKind::Error, format!("轮换密码失败：{error}")),
            }
        });
    }

    /// Exports the wallet state (keystore + channel store) encrypted with
    /// `password`. Requires the node to be stopped.
    pub fn export_state(&'static self, out_path: String, password: String) {
        self.update(|state| {
            state.backup_busy = true;
            state.backup_error = None;
            state.backup_message = None;
        });
        self.guarded("export_state", move || {
            let outcome = (|| {
                self.require_stopped("导出钱包状态")?;
                let node_id = self.state().snapshot.and_then(|snapshot| snapshot.node_id);
                fiber_wallet_core::backup::export_wallet_state(
                    &self.data_dir,
                    Path::new(out_path.trim()),
                    &password,
                    node_id.as_deref(),
                )
            })();
            self.update(|state| {
                state.backup_busy = false;
                match &outcome {
                    Ok(manifest) => {
                        state.backup_message = Some(format!(
                            "已导出（{}）到 {}",
                            manifest.node_id.as_deref().unwrap_or("未初始化节点"),
                            out_path.trim()
                        ));
                    }
                    Err(error) => state.backup_error = Some(error.clone()),
                }
            });
            match &outcome {
                Ok(_) => self.flash(
                    FlashKind::Ok,
                    format!("钱包状态已导出到 {}", out_path.trim()),
                ),
                Err(error) => self.flash(FlashKind::Error, format!("导出失败：{error}")),
            }
        });
    }

    /// Imports a wallet state archive, replacing the current one; restarts
    /// the node afterwards. Requires the node to be stopped.
    pub fn import_state(&'static self, archive_path: String, password: String) {
        self.update(|state| {
            state.backup_busy = true;
            state.backup_error = None;
            state.backup_message = None;
        });
        self.guarded("import_state", move || {
            let outcome = (|| {
                self.require_stopped("导入钱包状态")?;
                fiber_wallet_core::backup::import_wallet_state(
                    &self.data_dir,
                    Path::new(archive_path.trim()),
                    &password,
                )
            })();
            self.update(|state| {
                state.backup_busy = false;
                match &outcome {
                    Ok(_) => {
                        state.backup_message =
                            Some("导入完成，已备份原状态，节点重启中…".to_owned());
                        state.node_info = None;
                        state.channels.clear();
                        state.payments.clear();
                        state.peers.clear();
                    }
                    Err(error) => state.backup_error = Some(error.clone()),
                }
            });
            if outcome.is_ok() {
                self.flash(FlashKind::Ok, "钱包状态导入完成，节点重启中…".to_owned());
                self.restart();
            } else if let Err(error) = &outcome {
                self.flash(FlashKind::Error, format!("导入失败：{error}"));
            }
        });
    }
}

/// This shell's wallet data directory, distinct from other hosts' so
/// `wallet.lock` never contends.
fn wallet_data_directory() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join("Library/Application Support/FiberWalletNSH/fiber-wallet")
}

/// The data directory for display in settings.
pub fn data_directory() -> String {
    wallet_data_directory().to_string_lossy().into_owned()
}

/// Lowercase phase name for the event log.
pub fn phase_name(phase: NodePhase) -> &'static str {
    match phase {
        NodePhase::Stopped => "stopped",
        NodePhase::Starting => "starting",
        NodePhase::Running => "running",
        NodePhase::Stopping => "stopping",
        NodePhase::Failed => "failed",
    }
}

/// The message of a caught panic, for logging.
fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = panic.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = panic.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_owned()
    }
}

pub fn shannons_to_ckb(shannons: &str) -> String {
    fiber_wallet_core::api::wallet::shannons_to_ckb(shannons)
}

fn format_ckb(shannons: u64) -> String {
    shannons_to_ckb(&shannons.to_string())
}

/// Builds the native-CKB keysend request shared by dry-run and real send.
///
/// CKB input is parsed as fixed-point shannons rather than through `f64`, so
/// the displayed amount and the signed payment cannot differ by rounding.
fn build_transfer_request(
    target_pubkey: &str,
    amount_ckb: &str,
    max_fee_ckb: &str,
    dry_run: bool,
) -> Result<WalletSendPaymentRequest, String> {
    let target = target_pubkey.trim();
    if target.is_empty() {
        return Err("目标节点 pubkey 不能为空".to_owned());
    }
    let amount = parse_ckb_shannons(amount_ckb)?;
    if amount == 0 {
        return Err("支付金额必须大于 0 CKB".to_owned());
    }
    let max_fee = parse_ckb_shannons(max_fee_ckb)?;
    WalletSendPaymentRequest::for_keysend(target.to_owned(), amount)
        .with_max_fee_amount(max_fee)
        .with_timeout_seconds(TRANSFER_TIMEOUT_SECONDS)
        .with_dry_run(dry_run)
        .into_validated()
        .map_err(|error| format!("支付参数无效: {error}"))
}

fn parse_ckb_shannons(value: &str) -> Result<u128, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("金额不能为空".to_owned());
    }
    if value.starts_with('-') || value.starts_with('+') {
        return Err(format!("金额格式无效: {value}"));
    }
    let mut parts = value.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || (!fraction.is_empty() && !fraction.bytes().all(|byte| byte.is_ascii_digit()))
        || fraction.len() > 8
    {
        return Err(format!("CKB 金额最多 8 位小数且不能为负: {value}"));
    }
    let whole: u128 = whole
        .parse()
        .map_err(|_| format!("CKB 金额超出范围: {value}"))?;
    let fraction = format!("{fraction:0<8}");
    let fraction: u128 = fraction
        .parse()
        .map_err(|_| format!("CKB 小数格式无效: {value}"))?;
    whole
        .checked_mul(100_000_000)
        .and_then(|whole| whole.checked_add(fraction))
        .ok_or_else(|| format!("CKB 金额超出范围: {value}"))
}

/// Fast, non-authoritative form check. The core repeats all checks against
/// the fixed testnet RPC and the actual signed transaction.
fn validate_onchain_form(
    target_address: &str,
    amount_ckb: &str,
    max_fee_ckb: &str,
) -> Result<(u64, u64), String> {
    if target_address.trim().is_empty() {
        return Err("接收地址不能为空".to_owned());
    }
    let quantity = |value: &str| {
        let shannons = parse_ckb_shannons(value)?;
        u64::try_from(shannons).map_err(|_| "CKB 金额超出 u64 shannons 范围".to_owned())
    };
    let amount = quantity(amount_ckb)?;
    if amount == 0 {
        return Err("转账金额必须大于 0 CKB".to_owned());
    }
    let max_fee = quantity(max_fee_ckb)?;
    wallet::validate_onchain_fee_limit(max_fee).map_err(|error| error.to_string())?;
    Ok((amount, max_fee))
}

/// Keysend without an explicit payment hash gets a random hash inside Fiber,
/// so it cannot be recovered after a caller-side timeout. Invoice payments
/// carry their own hash and can always be looked up.
fn request_payment_hash(request: &WalletSendPaymentRequest) -> Option<String> {
    request.known_payment_hash()
}

#[cfg(test)]
mod onchain_tests {
    use super::*;

    #[test]
    fn validates_exact_testnet_transfer_form() {
        let address = "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqf7v2xsyj0p8szesqrwqapvvygpc8hzg9sku954v";
        assert_eq!(
            validate_onchain_form(address, "1.5", "0.001").unwrap(),
            (150_000_000, 100_000)
        );
        assert!(validate_onchain_form(address, "0", "0.001").is_err());
        assert!(validate_onchain_form(address, "1", "0").is_err());
        assert!(validate_onchain_form(address, "1.000000001", "0.001").is_err());
        assert!(validate_onchain_form("", "1", "0.001").is_err());
    }

    #[test]
    fn formats_shannons_without_float_rounding() {
        assert_eq!(format_ckb(150_000_001), "1.50000001");
        assert_eq!(format_ckb(100_000), "0.001");
    }
}

fn transfer_request_fields(request: &WalletSendPaymentRequest) -> Option<(String, String, String)> {
    if !request.keysend {
        return None;
    }
    Some((
        request.target_pubkey.clone()?,
        shannons_to_ckb(&request.amount?.to_string()),
        shannons_to_ckb(&request.max_fee_amount?.to_string()),
    ))
}

#[cfg(test)]
mod payment_tests {
    use super::*;

    const VALID_PUBKEY: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    #[test]
    fn parses_exact_ckb_shannons() {
        assert_eq!(parse_ckb_shannons("1").unwrap(), 100_000_000);
        assert_eq!(parse_ckb_shannons("0.00000001").unwrap(), 1);
        assert_eq!(parse_ckb_shannons("12.3").unwrap(), 1_230_000_000);
        assert!(parse_ckb_shannons("1.000000001").is_err());
        assert!(parse_ckb_shannons("-1").is_err());
        assert!(parse_ckb_shannons("1e6").is_err());
        assert!(parse_ckb_shannons("").is_err());
    }

    #[test]
    fn transfer_request_binds_dry_run_and_exact_amount() {
        let quote = build_transfer_request(VALID_PUBKEY, "1.5", "0.01", true).unwrap();
        assert!(quote.dry_run);
        assert!(quote.keysend);
        assert_eq!(quote.amount, Some(150_000_000));
        assert_eq!(quote.max_fee_amount, Some(1_000_000));
        assert_eq!(quote.timeout_seconds, Some(TRANSFER_TIMEOUT_SECONDS));

        let send = WalletSendPaymentRequest {
            dry_run: false,
            ..quote
        };
        assert!(!send.dry_run);
    }

    #[test]
    fn rejects_zero_amount_and_bad_target() {
        assert!(
            build_transfer_request(VALID_PUBKEY, "0", "0", true)
                .unwrap_err()
                .contains("大于 0")
        );
        assert!(build_transfer_request("02abcd", "1", "0", true).is_err());
    }
}

pub fn timestamp(ms: u64) -> String {
    if ms == 0 {
        return "-".to_owned();
    }
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|utc| {
            use chrono::{Datelike, Local, Timelike};
            let local = utc.with_timezone(&Local);
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                local.year(),
                local.month(),
                local.day(),
                local.hour(),
                local.minute()
            )
        })
        .unwrap_or_else(|| "-".to_owned())
}

fn dapp_core_error(message: &str) -> RpcError {
    let lower = message.to_ascii_lowercase();
    let unavailable = [
        "not running",
        "wallet operations require the native node backend",
        "node is not",
        "node not",
        "no route",
        "route not found",
        "insufficient",
        "not enough",
        "no channel",
        "channel not ready",
        "peer is not connected",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let code = if unavailable {
        error_code::UNAVAILABLE
    } else {
        error_code::INTERNAL_ERROR
    };
    RpcError::new(code, message.to_owned())
}
