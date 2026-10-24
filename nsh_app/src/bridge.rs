//! Bridges the wallet core's async API to the NSH engine.
//!
//! The core's futures need a tokio context (Fiber actors spawn on tokio), so
//! the bridge owns a runtime on dedicated threads and mirrors results into a
//! shared [`UiState`]. The UI copies that state into its signals on a timer.
//! Lifecycle behavior: optimistic phase transitions, serialized start/stop,
//! an event log of phase/detail changes, and 10 s data polling while running.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use fiber_wallet_core::api::node::{self, NodeNetwork, NodePhase, NodeSnapshot};
use crate::vault;
use fiber_wallet_core::api::wallet::{
    self, WalletChannel, WalletInvoice, WalletNodeInfo, WalletOpenedChannel, WalletPayment,
    WalletPeer,
};

/// Everything the wallet page renders, cloned from the bridge on each tick.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct UiState {
    pub snapshot: Option<NodeSnapshot>,
    pub busy: bool,
    pub events: Vec<String>,
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
    pub data_busy: bool,
    pub data_error: Option<String>,
    pub open_channel_busy: bool,
    pub open_channel_error: Option<String>,
    pub backup_busy: bool,
    pub backup_message: Option<String>,
    pub backup_error: Option<String>,
    pub password_busy: bool,
    pub password_message: Option<String>,
    pub password_error: Option<String>,
}

pub struct WalletBridge {
    state: Mutex<UiState>,
    runtime: tokio::runtime::Runtime,
    data_dir: PathBuf,
    secret_password: Mutex<String>,
    pub vault_kind: vault::VaultKind,
    lifecycle_busy: AtomicBool,
    refresh_busy: AtomicBool,
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
        }
    }

    pub fn state(&self) -> UiState {
        self.state.lock().expect("ui state poisoned").clone()
    }

    fn update(&self, apply: impl FnOnce(&mut UiState)) {
        let mut state = self.state.lock().expect("ui state poisoned");
        apply(&mut state);
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
            let password = self.secret_password.lock().expect("password poisoned").clone();
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
            }
            Err(error) => {
                tracing::error!(error = %error, "local Fiber node failed to start");
                self.fail(error);
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
            if let Some(current) = self
                .state()
                .snapshot
                .filter(|current| !matches!(current.phase, NodePhase::Stopping | NodePhase::Stopped))
            {
                tracing::info!("stopping local Fiber node");
                self.set_snapshot(NodeSnapshot {
                    phase: NodePhase::Stopping,
                    detail: "Draining work and closing wallet storage".to_owned(),
                    ..current
                });
                match self.runtime.block_on(node::stop_local_node()) {
                    Ok(snapshot) => self.set_snapshot(snapshot),
                    Err(error) => self.fail(error),
                }
                // Drop transient payment state once the node is down.
                self.update(|state| {
                    state.last_invoice = None;
                    state.send_result = None;
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
        std::thread::spawn(move || loop {
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
            let result = wallet::ckb_to_shannons(amount_ckb.trim().to_owned()).and_then(|shannons| {
                let shannons: u64 = shannons
                    .parse()
                    .map_err(|error| format!("invalid CKB amount: {error}"))?;
                self.runtime
                    .block_on(wallet::wallet_new_invoice(shannons, description.clone()))
            });
            self.update(|state| {
                state.invoice_busy = false;
                match result {
                    Ok(invoice) => state.last_invoice = Some(invoice),
                    Err(error) => state.invoice_error = Some(error),
                }
            });
        });
    }

    pub fn send_payment(&'static self, invoice: String) {
        self.update(|state| {
            state.send_busy = true;
            state.send_error = None;
            state.send_result = None;
        });
        self.guarded("send_payment", move || {
            match self.runtime.block_on(wallet::wallet_send_payment(invoice)) {
                Ok(payment) => {
                    let message = format!(
                        "Payment {}: hash {}.",
                        payment.status, payment.payment_hash
                    );
                    self.update(|state| {
                        state.send_result = Some(message);
                        state.send_busy = false;
                    });
                    self.refresh();
                }
                Err(error) => self.update(|state| {
                    state.send_error = Some(error);
                    state.send_busy = false;
                }),
            }
        });
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
                if let Some(address) = connect_address.filter(|a| !a.trim().is_empty()) {
                    self.runtime
                        .block_on(wallet::wallet_connect_peer(address.trim().to_owned()))?;
                }
                let shannons = wallet::ckb_to_shannons(funding_amount_ckb.trim().to_owned())?;
                let shannons: u64 = shannons
                    .parse()
                    .map_err(|error| format!("CKB 金额超出范围: {error}"))?;
                self.runtime.block_on(wallet::wallet_open_channel(
                    peer.trim().to_owned(),
                    shannons,
                    public_channel,
                ))
            })();
            self.update(|state| {
                state.open_channel_busy = false;
                match result {
                    Ok(_) => {}
                    Err(error) => state.open_channel_error = Some(error),
                }
            });
            if self.state().open_channel_error.is_none() {
                self.refresh();
            }
        });
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
            let outcome = (|| {
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
                match outcome {
                    Ok(()) => state.password_message = Some("密钥加密密码已轮换".to_owned()),
                    Err(error) => state.password_error = Some(error),
                }
            });
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
                match outcome {
                    Ok(manifest) => {
                        state.backup_message = Some(format!(
                            "已导出（{}）到 {}",
                            manifest.node_id.as_deref().unwrap_or("未初始化节点"),
                            out_path.trim()
                        ));
                    }
                    Err(error) => state.backup_error = Some(error),
                }
            });
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
                self.restart();
            }
        });
    }
}

/// This shell's wallet data directory, distinct from other hosts' so
/// `wallet.lock` never contends.
fn wallet_data_directory() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home)
        .join("Library/Application Support/FiberWalletNSH/fiber-wallet")
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
    let value: u128 = shannons.parse().unwrap_or(0);
    let whole = value / 100_000_000;
    let fraction = format!("{:08}", value % 100_000_000);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{fraction}")
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
