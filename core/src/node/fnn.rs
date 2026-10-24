use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ckb_chain_spec::ChainSpec;
use ckb_jsonrpc_types::JsonBytes;
use ckb_resource::Resource;
use ckb_sdk::rpc::ckb_indexer::{Order, ScriptType, SearchKey};
use ckb_sdk::{Address, AddressPayload, NetworkType};
use fiber_json_types::invoice::Currency as InvoiceCurrency;
use fnn::actors::RootActor;
use fnn::ckb::client::{CkbChainClient, CkbRpcClient};
use fnn::ckb::contracts::ContractsContextError;
use fnn::ckb::contracts::TypeIDResolver;
use fnn::ckb::{contracts::try_init_contracts_context, CkbChainActor};
use fnn::fiber::graph::NetworkGraph;
use fnn::fiber::network::{get_chain_hash, init_chain_hash, NetworkActorMessage};
use fnn::fiber::types::pubkey_from_tentacle;
use fnn::fiber::types::Hash256;
use fnn::rpc::channel::{ChannelRpcServerImpl, ListChannelsParams};
use fnn::rpc::info::InfoRpcServerImpl;
use fnn::rpc::invoice::{InvoiceRpcServerImpl, NewInvoiceParams};
use fnn::rpc::payment::{
    GetPaymentCommandParams, ListPaymentsParams, PaymentRpcServerImpl, SendPaymentCommandParams,
};
use fnn::store::actor::{StoreActor, StoreActorInitializationParameter};
use fnn::store::open_store_with_migration;
use fnn::utils::encrypt_decrypt_file::{decrypt_from_file, encrypt_to_file};
use fnn::watchtower::{
    WatchtowerActor, WatchtowerMessage, DEFAULT_WATCHTOWER_CHECK_INTERVAL_SECONDS,
};
use fnn::{start_network, Config, NetworkServiceEvent};
use fs2::FileExt;
use ractor::{Actor, ActorRef};
use secp256k1::SECP256K1;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, error, info};

use crate::types::{NodeNetwork, NodePhase, NodeSnapshot};
use crate::types::{WalletChannel, WalletInvoice, WalletNodeInfo, WalletPayment};

const TESTNET_CONFIG: &str = include_str!("../../assets/fiber-testnet.yml");
const FIBER_VERSION: &str = "v0.9.1";

/// Marks that Fiber's process-global contracts context is populated. Fiber
/// refuses to initialize it twice, so node restarts inside the same process
/// must skip the work; a real failure keeps this unset and stays retryable.
static CONTRACTS_CONTEXT_READY: OnceLock<()> = OnceLock::new();

struct RunningNode {
    node_id: String,
    root_actor: ActorRef<String>,
    token: CancellationToken,
    tracker: TaskTracker,
    store: fnn::store::Store,
    network_actor: ActorRef<NetworkActorMessage>,
    fiber_config: fnn::FiberConfig,
    ckb_config: fnn::ckb::CkbConfig,
    /// Exclusive lock on the data directory. Held for the whole node lifetime;
    /// dropping it releases the lock.
    _data_dir_lock: File,
}

impl RunningNode {
    async fn stop(self) -> Result<(), String> {
        info!("stopping embedded Fiber node");
        self.token.cancel();
        let stop_result = self
            .root_actor
            .stop_and_wait(Some("Fiber Wallet requested shutdown".to_owned()), None)
            .await
            .map_err(|err| format!("failed to stop Fiber root actor: {err}"));

        self.tracker.close();
        self.tracker.wait().await;
        debug!("embedded Fiber tasks stopped");
        stop_result
    }
}

/// Native local node backed by Fiber Network Node v0.9.1.
pub struct NodeMachine {
    state: Mutex<NodeSnapshot>,
    operation: tokio::sync::Mutex<()>,
    runtime: Mutex<Option<RunningNode>>,
    data_dir: PathBuf,
    secret_password: String,
}

impl NodeMachine {
    pub fn new(network: NodeNetwork, data_dir: String, secret_password: String) -> Self {
        Self {
            state: Mutex::new(NodeSnapshot {
                phase: NodePhase::Stopped,
                network,
                detail: "Local Fiber node is stopped".to_owned(),
                node_id: None,
                backend: format!("fnn {FIBER_VERSION} (sqlite)"),
            }),
            operation: tokio::sync::Mutex::new(()),
            runtime: Mutex::new(None),
            data_dir: PathBuf::from(data_dir),
            secret_password,
        }
    }

    pub fn snapshot(&self) -> NodeSnapshot {
        self.state.lock().expect("node state poisoned").clone()
    }

    pub async fn start(&self) -> Result<NodeSnapshot, String> {
        let _operation = self.operation.lock().await;
        {
            let mut state = self.state.lock().expect("node state poisoned");
            if matches!(state.phase, NodePhase::Starting | NodePhase::Running) {
                return Ok(state.clone());
            }
            if state.network != NodeNetwork::Testnet {
                let message = "M1 only supports Fiber testnet";
                state.phase = NodePhase::Failed;
                state.detail = message.to_owned();
                state.node_id = None;
                return Err(message.to_owned());
            }
            state.phase = NodePhase::Starting;
            state.detail = "Opening SQLite store and starting Fiber actors".to_owned();
            state.node_id = None;
        }

        match start_runtime(&self.data_dir, &self.secret_password).await {
            Ok(running) => {
                let node_id = running.node_id.clone();
                *self.runtime.lock().expect("runtime poisoned") = Some(running);
                let mut state = self.state.lock().expect("node state poisoned");
                state.phase = NodePhase::Running;
                state.detail = "Fiber node is running while the app is active".to_owned();
                state.node_id = Some(node_id);
                Ok(state.clone())
            }
            Err(error) => Ok(self.set_failed(error)),
        }
    }

    pub async fn stop(&self) -> Result<NodeSnapshot, String> {
        let _operation = self.operation.lock().await;
        {
            let mut state = self.state.lock().expect("node state poisoned");
            if matches!(state.phase, NodePhase::Stopping | NodePhase::Stopped) {
                return Ok(state.clone());
            }
            state.phase = NodePhase::Stopping;
            state.detail = "Stopping Fiber actors and flushing SQLite".to_owned();
        }

        let running = self.runtime.lock().expect("runtime poisoned").take();
        if let Some(running) = running {
            if let Err(error) = running.stop().await {
                return Ok(self.set_failed(error));
            }
        }

        let mut state = self.state.lock().expect("node state poisoned");
        state.phase = NodePhase::Stopped;
        state.detail = "Local Fiber node is offline".to_owned();
        state.node_id = None;
        Ok(state.clone())
    }

    pub fn mark_failed(&self, message: String) -> NodeSnapshot {
        self.set_failed(message)
    }

    fn set_failed(&self, message: String) -> NodeSnapshot {
        error!("embedded Fiber node failed: {message}");
        let mut state = self.state.lock().expect("node state poisoned");
        state.phase = NodePhase::Failed;
        state.detail = message;
        state.node_id = None;
        state.clone()
    }
}

async fn start_runtime(data_dir: &Path, secret_password: &str) -> Result<RunningNode, String> {
    std::fs::create_dir_all(data_dir)
        .map_err(|err| format!("failed to create data directory: {err}"))?;

    let data_dir_lock = acquire_data_dir_lock(data_dir)?;

    // Fiber reads this env var whenever it loads or generates a key file, so
    // it must be set before any key access below.
    std::env::set_var("FIBER_SECRET_KEY_PASSWORD", secret_password);

    // Installs from the M1 development build encrypted their key files with a
    // hardcoded password. Re-encrypt them under the wallet password so the
    // upgrade is transparent; anything else fails loudly instead of silently
    // regenerating keys (which would lose channel state).
    migrate_secret_file(&data_dir.join("sk"), secret_password)?;
    migrate_secret_file(&data_dir.join("key"), secret_password)?;
    ensure_ckb_secret_key(data_dir, secret_password.as_bytes())?;

    let mut config = Config::parse_from_str(
        TESTNET_CONFIG,
        Some(data_dir.to_string_lossy().into_owned()),
    );
    let fiber_config = config
        .fiber
        .take()
        .ok_or_else(|| "Fiber service missing from embedded config".to_owned())?;
    let ckb_config = config
        .ckb
        .take()
        .ok_or_else(|| "CKB service missing from embedded config".to_owned())?;

    let node_public_key = fiber_config.public_key();
    let node_id = hex::encode(pubkey_from_tentacle(node_public_key.clone()).serialize());

    let store = open_store_with_migration(
        fiber_config.store_path(),
        Box::new(|_| true),
        Box::new(|progress| {
            info!(
                "Fiber store migration {}/{}: {}",
                progress.current_step, progress.total_steps, progress.message
            );
        }),
    )?;

    let tracker = TaskTracker::new();
    let token = CancellationToken::new();
    let root_actor = RootActor::start(tracker.clone(), token.clone()).await;

    let chain = fiber_config.chain.as_str();
    let chain_spec = ChainSpec::load_from(&match chain {
        "mainnet" => Resource::bundled("specs/mainnet.toml".to_owned()),
        "testnet" => Resource::bundled("specs/testnet.toml".to_owned()),
        path => {
            return Err(format!(
                "unsupported embedded chain '{path}'; only mainnet/testnet are bundled"
            ))
        }
    })
    .map_err(|err| format!("failed to load chain spec: {err}"))?;
    let genesis_block = chain_spec
        .build_genesis()
        .map_err(|err| format!("failed to build CKB genesis: {err}"))?;

    // Fiber keeps the chain hash and contract scripts in process-global cells
    // that panic or error on re-initialization. Node restarts inside the same
    // process must skip that work; switching chains would require a new process.
    let chain_hash: Hash256 = genesis_block.hash().into();
    if get_chain_hash().as_ref() == [0u8; 32] {
        init_chain_hash(chain_hash);
    }

    if CONTRACTS_CONTEXT_READY.get().is_none() {
        let type_id_resolver = TypeIDResolver::new(ckb_config.rpc_url.clone());
        match try_init_contracts_context(
            genesis_block,
            fiber_config.scripts.clone(),
            ckb_config.udt_whitelist.clone().unwrap_or_default(),
            Some(type_id_resolver),
        )
        .await
        {
            Ok(()) | Err(ContractsContextError::ContextAlreadyInitialized) => {
                let _ = CONTRACTS_CONTEXT_READY.set(());
            }
            Err(err) => return Err(format!("failed to initialize CKB contracts: {err}")),
        }
    }

    let ckb_chain_actor = Actor::spawn_linked(
        Some("ckb".to_owned()),
        CkbChainActor {},
        ckb_config.clone(),
        root_actor.get_cell(),
    )
    .await
    .map_err(|err| format!("failed to start CKB actor: {err}"))?
    .0;

    let (event_sender, mut event_receiver) = mpsc::channel::<NetworkServiceEvent>(4000);
    let network_graph = Arc::new(RwLock::new(NetworkGraph::new(
        store.clone(),
        pubkey_from_tentacle(node_public_key),
        fiber_config.announce_private_addr(),
    )));
    let default_shutdown_script = ckb_config
        .get_default_funding_lock_script()
        .map_err(|err| format!("failed to derive CKB funding lock: {err}"))?;

    let backup_path = fiber_config.base_dir().join("backups");
    let ckb_key_path = ckb_config.base_dir().join("key");
    let fiber_key_path = fiber_config.base_dir().join("sk");
    let store_actor = Actor::spawn_linked(
        Some("store_actor".to_owned()),
        StoreActor {
            _phantom: std::marker::PhantomData,
        },
        StoreActorInitializationParameter {
            store: store.clone(),
            backup_path,
            ckb_key_path,
            fiber_key_path,
            backup_interval_hours: 24,
        },
        root_actor.get_cell(),
    )
    .await
    .map_err(|err| format!("failed to start store actor: {err}"))?
    .0;

    let chain_client = CkbRpcClient::new(&ckb_config);
    let network_actor: ActorRef<NetworkActorMessage> = start_network(
        fiber_config.clone(),
        chain_client,
        ckb_chain_actor,
        event_sender,
        tracker.clone(),
        root_actor.get_cell(),
        store.clone(),
        Some(store_actor),
        network_graph,
        default_shutdown_script,
    )
    .await;

    let watchtower_actor = Actor::spawn_linked(
        Some("watchtower".to_owned()),
        WatchtowerActor::new(store.clone()),
        ckb_config.clone(),
        root_actor.get_cell(),
    )
    .await
    .map_err(|err| format!("failed to start watchtower actor: {err}"))?
    .0;

    watchtower_actor.send_interval(
        Duration::from_secs(
            fiber_config
                .watchtower_check_interval_seconds
                .unwrap_or(DEFAULT_WATCHTOWER_CHECK_INTERVAL_SECONDS),
        ),
        || WatchtowerMessage::PeriodicCheck,
    );

    let event_token = token.clone();
    let watchtower_for_events = watchtower_actor.clone();
    tracker.spawn(async move {
        loop {
            tokio::select! {
                _ = event_token.cancelled() => {
                    debug!("Fiber event forwarder stopped");
                    break;
                }
                event = event_receiver.recv() => {
                    match event {
                        Some(event) => forward_event_to_actor(event, &watchtower_for_events),
                        None => break,
                    }
                }
            }
        }
    });

    // Keep the network actor alive through the root actor supervision tree. The
    // binding is retained on the running node so wallet operations can issue
    // commands to it after startup returns.
    let _ = &network_actor;
    info!("embedded Fiber node started: {node_id}");

    Ok(RunningNode {
        node_id,
        root_actor,
        token,
        tracker,
        store,
        network_actor,
        fiber_config,
        ckb_config: ckb_config.clone(),
        _data_dir_lock: data_dir_lock,
    })
}

/// One wallet instance at a time may open the Fiber store under a data
/// directory. Two live Fiber nodes on the same SQLite database would corrupt
/// channel state, so the second instance fails fast instead of racing it.
fn acquire_data_dir_lock(data_dir: &Path) -> Result<File, String> {
    let path = data_dir.join("wallet.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|err| format!("failed to open lock file {}: {err}", path.display()))?;

    file.try_lock_exclusive().map_err(|_| {
        format!(
            "wallet data directory is already in use by another instance: {}",
            path.display()
        )
    })?;
    Ok(file)
}

/// M1 installs encrypted their key files with this hardcoded password; it only
/// exists so `migrate_secret_file` can upgrade them to the wallet password.
const LEGACY_DEV_PASSWORD: &str = "fiber-wallet-m1-development-only";

/// Re-encrypts a password-protected key file under the wallet password if it
/// was written by an M1 build. Fiber's node identity file (`sk`) is stored
/// unencrypted, so anything outside the encrypted-file format is left alone.
/// A file in encrypted format that decrypts under neither password is a hard
/// error: regenerating it would silently drop the CKB wallet key.
fn migrate_secret_file(path: &Path, secret_password: &str) -> Result<(), String> {
    const VERSION: u8 = 0;
    const HEADER_LEN: usize = 1 + 16 + 12; // version byte + scrypt salt + nonce

    if !path.exists() {
        return Ok(());
    }
    let bytes = std::fs::read(path)
        .map_err(|err| format!("failed to read secret file {}: {err}", path.display()))?;
    if bytes.len() < HEADER_LEN + 16 || bytes[0] != VERSION {
        // Not a password-protected file (raw node key or foreign format).
        return Ok(());
    }
    if decrypt_from_file(path, secret_password.as_bytes()).is_ok() {
        return Ok(());
    }
    let plaintext = decrypt_from_file(path, LEGACY_DEV_PASSWORD.as_bytes()).map_err(|_| {
        format!(
            "secret file {} cannot be decrypted with the wallet password or the \
             legacy development password",
            path.display()
        )
    })?;
    encrypt_to_file(path, &plaintext, secret_password.as_bytes())
        .map_err(|err| format!("failed to re-encrypt secret file {}: {err}", path.display()))
}

fn ensure_ckb_secret_key(data_dir: &Path, password: &[u8]) -> Result<(), String> {
    let key_path = data_dir.join("key");
    if key_path.exists() {
        return Ok(());
    }

    let mut generator = rand::thread_rng();
    let (secret_key, _) = SECP256K1.generate_keypair(&mut generator);
    encrypt_to_file(&key_path, &secret_key.secret_bytes(), password)
        .map_err(|err| format!("failed to write CKB key: {err}"))
}

fn forward_event_to_actor(event: NetworkServiceEvent, watchtower: &ActorRef<WatchtowerMessage>) {
    match event {
        NetworkServiceEvent::RemoteTxComplete(
            _peer_id,
            channel_id,
            funding_udt_type_script,
            local_settlement_key,
            remote_settlement_key,
            local_funding_pubkey,
            remote_funding_pubkey,
            remote_settlement_data,
        ) => {
            let _ = watchtower.send_message(WatchtowerMessage::CreateChannel(
                channel_id,
                funding_udt_type_script,
                local_settlement_key,
                remote_settlement_key,
                local_funding_pubkey,
                remote_funding_pubkey,
                remote_settlement_data,
            ));
        }
        NetworkServiceEvent::ChannelClosed(_, channel_id, _)
        | NetworkServiceEvent::ChannelAbandon(channel_id) => {
            let _ = watchtower.send_message(WatchtowerMessage::RemoveChannel(channel_id));
        }
        NetworkServiceEvent::RevokeAndAckReceived(
            _peer_id,
            channel_id,
            revocation_data,
            settlement_data,
        ) => {
            let _ = watchtower.send_message(WatchtowerMessage::UpdateRevocation(
                channel_id,
                revocation_data,
                settlement_data,
            ));
        }
        NetworkServiceEvent::RemoteCommitmentSigned(
            _peer_id,
            channel_id,
            _commitment_tx,
            settlement_data,
        ) => {
            let _ = watchtower.send_message(WatchtowerMessage::UpdateLocalSettlement(
                channel_id,
                settlement_data,
            ));
        }
        NetworkServiceEvent::LocalCommitmentSigned(channel_id, settlement_data) => {
            let _ = watchtower.send_message(WatchtowerMessage::UpdatePendingRemoteSettlement(
                channel_id,
                settlement_data,
            ));
        }
        NetworkServiceEvent::PreimageCreated(payment_hash, preimage) => {
            let _ =
                watchtower.send_message(WatchtowerMessage::CreatePreimage(payment_hash, preimage));
        }
        NetworkServiceEvent::PreimageRemoved(payment_hash) => {
            let _ = watchtower.send_message(WatchtowerMessage::RemovePreimage(payment_hash));
        }
        _ => {}
    }
}

/// Wallet operations on the running node. Each call clones the actor handles
/// out of the runtime slot before awaiting: the std mutex guard must never be
/// held across an await point.
impl NodeMachine {
    fn running_parts(
        &self,
    ) -> Result<
        (
            ActorRef<NetworkActorMessage>,
            fnn::store::Store,
            fnn::FiberConfig,
            fnn::ckb::CkbConfig,
        ),
        String,
    > {
        let guard = self.runtime.lock().expect("runtime poisoned");
        let running = guard
            .as_ref()
            .ok_or_else(|| "local node is not running".to_owned())?;
        Ok((
            running.network_actor.clone(),
            running.store.clone(),
            running.fiber_config.clone(),
            running.ckb_config.clone(),
        ))
    }

    pub async fn wallet_node_info(&self) -> Result<WalletNodeInfo, String> {
        let (network_actor, _store, _fiber_config, ckb_config) = self.running_parts()?;
        let info = InfoRpcServerImpl::new(network_actor, ckb_config.clone());
        let result = info
            .node_info()
            .await
            .map_err(|err| err.message().to_owned())?;

        let funding_script = ckb_config
            .get_default_funding_lock_script()
            .map_err(|err| format!("failed to derive funding lock script: {err}"))?;
        let funding_address = Address::new(
            NetworkType::Testnet,
            AddressPayload::from(funding_script.clone()),
            true,
        )
        .to_string();
        let funding_balance = fetch_funding_balance(&ckb_config, funding_script).await?;
        let channel_balance: u128 = self
            .wallet_list_channels()
            .await?
            .iter()
            .filter_map(|channel| channel.local_balance.parse::<u128>().ok())
            .sum();

        Ok(WalletNodeInfo {
            node_id: result.pubkey.to_string(),
            node_name: result.node_name.map(|name| name.to_string()),
            addresses: result.addresses,
            version: format!("fnn {} ({})", result.version, result.commit_hash),
            funding_address,
            funding_balance_shannons: funding_balance.to_string(),
            channel_balance_shannons: channel_balance.to_string(),
        })
    }

    pub async fn wallet_list_channels(&self) -> Result<Vec<WalletChannel>, String> {
        let (network_actor, store, _fiber_config, _ckb_config) = self.running_parts()?;
        let result = ChannelRpcServerImpl::new(network_actor, store)
            .list_channels(ListChannelsParams {
                pubkey: None,
                include_closed: None,
                only_pending: None,
            })
            .await
            .map_err(|err| err.message().to_owned())?;

        Ok(result
            .channels
            .into_iter()
            .map(|channel| WalletChannel {
                channel_id: hex::encode(channel.channel_id.0),
                peer_id: channel.pubkey.to_string(),
                state: json_variant_name(&channel.state),
                is_inbound: channel.is_acceptor,
                local_balance: channel.local_balance.to_string(),
                remote_balance: channel.remote_balance.to_string(),
            })
            .collect())
    }

    pub async fn wallet_new_invoice(
        &self,
        amount_shannons: u64,
        description: String,
    ) -> Result<WalletInvoice, String> {
        let (network_actor, store, fiber_config, _ckb_config) = self.running_parts()?;
        let description = (!description.is_empty()).then_some(description);
        let result = InvoiceRpcServerImpl::new(store, Some(network_actor), Some(fiber_config))
            .new_invoice(NewInvoiceParams {
                amount: u128::from(amount_shannons),
                description: description.clone(),
                // The embedded node runs on CKB testnet; invoices must use the
                // testnet currency or Fiber rejects them.
                currency: InvoiceCurrency::Fibt,
                ..Default::default()
            })
            .await
            .map_err(|err| err.message().to_owned())?;

        Ok(WalletInvoice {
            invoice: result.invoice_address,
            payment_hash: hex::encode(result.invoice.data.payment_hash.0),
            amount: result
                .invoice
                .amount
                .unwrap_or(u128::from(amount_shannons))
                .to_string(),
            description,
        })
    }

    pub async fn wallet_list_payments(&self, limit: u32) -> Result<Vec<WalletPayment>, String> {
        let (network_actor, store, _fiber_config, _ckb_config) = self.running_parts()?;
        let result = PaymentRpcServerImpl::new(network_actor, store)
            .list_payments(ListPaymentsParams {
                status: None,
                limit: Some(u64::from(limit)),
                after: None,
            })
            .await
            .map_err(|err| err.message().to_owned())?;
        Ok(result.payments.into_iter().map(payment_to_wallet).collect())
    }

    pub async fn wallet_get_payment(&self, payment_hash: String) -> Result<WalletPayment, String> {
        let (network_actor, store, _fiber_config, _ckb_config) = self.running_parts()?;
        let hash = parse_payment_hash(&payment_hash)?;
        let payment = PaymentRpcServerImpl::new(network_actor, store)
            .get_payment(GetPaymentCommandParams {
                // The json-types Hash256 wraps a public [u8; 32]; channel.rs
                // re-exports the same type the payment params use.
                payment_hash: fnn::rpc::channel::Hash256(hash),
            })
            .await
            .map_err(|err| err.message().to_owned())?;
        Ok(payment_to_wallet(payment))
    }

    pub async fn wallet_send_payment(&self, invoice: String) -> Result<WalletPayment, String> {
        let (network_actor, store, _fiber_config, _ckb_config) = self.running_parts()?;
        let payment = PaymentRpcServerImpl::new(network_actor, store)
            .send_payment(SendPaymentCommandParams {
                invoice: Some(invoice),
                ..empty_send_payment_params()
            })
            .await
            .map_err(|err| err.message().to_owned())?;
        Ok(payment_to_wallet(payment))
    }
}

fn empty_send_payment_params() -> SendPaymentCommandParams {
    SendPaymentCommandParams {
        target_pubkey: None,
        amount: None,
        payment_hash: None,
        final_tlc_expiry_delta: None,
        tlc_expiry_limit: None,
        invoice: None,
        timeout: None,
        max_fee_amount: None,
        max_fee_rate: None,
        max_parts: None,
        trampoline_hops: None,
        keysend: None,
        udt_type_script: None,
        allow_self_payment: None,
        custom_records: None,
        hop_hints: None,
        dry_run: None,
    }
}

fn payment_to_wallet(payment: fnn::rpc::payment::GetPaymentCommandResult) -> WalletPayment {
    WalletPayment {
        payment_hash: hex::encode(payment.payment_hash.0),
        status: json_variant_name(&payment.status),
        failed_error: payment.failed_error,
        created_at_ms: payment.created_at,
        last_updated_at_ms: payment.last_updated_at,
    }
}

fn parse_payment_hash(value: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(value.trim())
        .map_err(|err| format!("invalid payment hash '{value}': {err}"))?;
    bytes
        .try_into()
        .map_err(|_| format!("payment hash must be 32 bytes: {value}"))
}

fn json_variant_name<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(serde_json::Value::Object(map)) => map
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "unknown".to_owned()),
        Ok(other) => other.to_string(),
        Err(_) => "unknown".to_owned(),
    }
}

/// Sums the unspent cell capacity under the funding lock via the CKB indexer.
async fn fetch_funding_balance(
    ckb_config: &fnn::ckb::CkbConfig,
    funding_lock_script: ckb_types::packed::Script,
) -> Result<u128, String> {
    let client = CkbRpcClient::new(ckb_config);
    let search_key = SearchKey {
        script: ckb_jsonrpc_types::Script::from(funding_lock_script),
        script_type: ScriptType::Lock,
        script_search_mode: None,
        filter: None,
        with_data: None,
        group_by_transaction: None,
    };

    let mut after: Option<JsonBytes> = None;
    let mut total: u128 = 0;
    loop {
        let page = client
            .get_cells(search_key.clone(), Order::Asc, 200, after)
            .await
            .map_err(|err| format!("failed to query funding cells: {err}"))?;
        let empty = page.objects.is_empty();
        for cell in &page.objects {
            let unspent = cell
                .output_data
                .as_ref()
                .map(|data| data.as_bytes().is_empty())
                .unwrap_or(true);
            if unspent {
                total += u128::from(cell.output.capacity.value());
            }
        }
        if empty {
            return Ok(total);
        }
        after = Some(page.last_cursor);
    }
}
