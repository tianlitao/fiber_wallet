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
use fiber_wallet_core::api::wallet::{
    self, WalletChannel, WalletInvoice, WalletNodeInfo, WalletPayment,
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
    pub last_invoice: Option<WalletInvoice>,
    pub invoice_busy: bool,
    pub invoice_error: Option<String>,
    pub send_busy: bool,
    pub send_result: Option<String>,
    pub send_error: Option<String>,
    pub data_busy: bool,
    pub data_error: Option<String>,
}

pub struct WalletBridge {
    state: Mutex<UiState>,
    runtime: tokio::runtime::Runtime,
    data_dir: PathBuf,
    secret_password: String,
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
        let secret_password = load_or_create_password(&data_dir);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build wallet runtime");

        Self {
            state: Mutex::new(UiState::default()),
            runtime,
            data_dir,
            secret_password,
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
            let snapshot = node::init_local_node(
                NodeNetwork::Testnet,
                self.data_dir.to_string_lossy().into_owned(),
                self.secret_password.clone(),
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
            let (info, channels, payments) = self.runtime.block_on(async {
                tokio::join!(
                    wallet::wallet_node_info(),
                    wallet::wallet_list_channels(),
                    wallet::wallet_list_payments(50),
                )
            });
            self.update(|state| {
                match (info, channels, payments) {
                    (Ok(info), Ok(channels), Ok(payments)) => {
                        state.node_info = Some(info);
                        state.channels = channels;
                        state.payments = payments;
                        state.data_error = None;
                    }
                    (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
                        state.data_error = Some(error);
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

/// Development stand-in for the M2 Keychain integration: `FIBER_SECRET_KEY_PASSWORD`
/// wins, else a random hex password persisted 0600 next to the data directory.
fn load_or_create_password(data_dir: &Path) -> String {
    if let Some(password) = std::env::var("FIBER_SECRET_KEY_PASSWORD")
        .ok()
        .map(|password| password.trim().to_owned())
        .filter(|password| !password.is_empty())
    {
        return password;
    }
    let path = data_dir.join(".key-password");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    let password: String = (0..32)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();
    use std::io::Write as _;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
        {
            let _ = file.write_all(password.as_bytes());
        }
    }
    #[cfg(not(unix))]
    if let Ok(mut file) = std::fs::File::create(&path) {
        let _ = file.write_all(password.as_bytes());
    }
    password
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
