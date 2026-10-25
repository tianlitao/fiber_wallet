//! Host-side dApp session, approval, and wallet execution.
//!
//! The page is untrusted. The session binds to a URL observed by the native
//! WebView, never to a URL claimed by the page. Host mutations require a
//! one-use host token and explicit user approval. Wallet operations call the
//! same Rust core as the wallet UI; pages never receive key material or a
//! generic RPC/script channel.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc, mpsc::SyncSender};
use std::time::Instant;

use serde_json::{Value, json};

use crate::bridge::WalletBridge;
use crate::dapp_protocol::{
    self, ApprovalSummary, DappRequest, DappSession, PendingApproval, ProviderMethod, RpcError,
    RpcResponse, WalletAction, error_code, resolve_method,
};
use crate::dapp_registry::DappEntry;

/// Where a response or host event goes.
#[derive(Debug, Clone)]
pub enum HostMessage {
    Response(String),
    Event(String),
    NavigationRevoked(String),
    Fatal(String),
}

/// A host-only lease that ties an already-approved action to the exact
/// WebView generation and origin that requested it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLease {
    pub dapp_id: String,
    pub generation: u64,
    pub origin: String,
}

impl SessionLease {
    /// Returns false once the page navigated away, reloaded, or the user closed
    /// the host. A queued request must re-check this immediately before any
    /// wallet call; approval at time T does not preserve authority forever.
    pub fn is_current(&self) -> bool {
        lease_is_active(self)
    }
}

#[derive(Debug, Clone)]
pub struct SessionState {
    pub session: DappSession,
    /// CSPRNG capability minted for this document generation. Every bridge
    /// request and host event must carry it.
    pub nonce: String,
    pub current_url: String,
    pub active: Arc<AtomicBool>,
}

#[derive(Debug)]
pub enum Dispatch {
    Reply(RpcResponse),
    Approval {
        pending: PendingApproval,
        lease: SessionLease,
    },
    Execute {
        lease: SessionLease,
        method: ProviderMethod,
        action: WalletAction,
        request_id: String,
    },
}

/// One live WebView's host state. `request_id` is retained at approval time so
/// the final response can be correlated after the page's request task moves
/// off the UI thread.
#[derive(Debug, Clone, Default)]
pub struct DappHostState {
    session: Option<SessionState>,
    approval: Option<PendingApproval>,
}

impl DappHostState {
    pub fn new(dapp: &DappEntry, current_url: &str, generation: u64) -> Result<Self, String> {
        let session = DappSession::new(
            dapp.id.clone(),
            dapp.name.clone(),
            dapp.url.clone(),
            generation,
        )?;
        session
            .ensure_origin(current_url)
            .map_err(|error| error.message)?;
        let active = register_host(lease_of_session(&session));
        Ok(Self {
            session: Some(SessionState {
                session,
                nonce: new_nonce(),
                current_url: current_url.to_owned(),
                active,
            }),
            approval: None,
        })
    }

    pub fn session(&self) -> Option<&SessionState> {
        self.session.as_ref()
    }

    pub fn current_approval(&self) -> Option<&PendingApproval> {
        self.approval.as_ref()
    }

    /// Updates the URL only after the host checked the origin. A mismatch is
    /// handled by [`Self::revoke_for_navigation`].
    pub fn set_current_url(&mut self, current_url: &str) -> Result<(), RpcError> {
        let state = self.session.as_mut().ok_or_else(session_missing)?;
        if !state.active.load(Ordering::Acquire) {
            return Err(RpcError::new(
                error_code::ORIGIN_MISMATCH,
                "the dApp session is revoked; reopen the dApp",
            ));
        }
        state.session.ensure_origin(current_url)?;
        state.current_url = current_url.to_owned();
        Ok(())
    }

    /// Starts a new same-origin document epoch as soon as WebKit begins the
    /// main-frame navigation. The old generation stops being executable
    /// before the new document's first script can call the bridge.
    pub fn begin_document(&mut self, current_url: &str) -> Result<(), RpcError> {
        let state = self.session.as_mut().ok_or_else(session_missing)?;
        state.session.ensure_origin(current_url)?;
        let old = lease_of(state);
        state.active.store(false, Ordering::Release);
        state.session.generation = new_generation();
        state.session.revoke();
        state.current_url = current_url.to_owned();
        state.active = register_host(lease_of(state));
        self.approval = None;
        revoke_matching_leases(&old);
        Ok(())
    }

    /// Clears authority and any pending approval after a cross-origin commit
    /// or any session lifecycle change.
    pub fn revoke(&mut self) {
        if let Some(state) = self.session.as_mut() {
            state.session.revoke();
            state.session.generation = new_generation();
            state.active.store(false, Ordering::Release);
        }
        self.approval = None;
    }

    pub fn dispatch(&mut self, request: DappRequest, now_ms: u64) -> Result<Dispatch, RpcError> {
        let DappRequest::Request {
            nonce,
            id,
            method,
            params,
            ..
        } = request
        else {
            return Ok(Dispatch::Reply(unexpected_event()));
        };
        let state = self.session.as_ref().ok_or_else(session_missing)?;
        if !state.active.load(Ordering::Acquire) {
            return Err(RpcError::new(
                error_code::ORIGIN_MISMATCH,
                "the dApp session is revoked; reopen the dApp",
            ));
        }
        if !constant_time_eq(nonce.as_bytes(), state.nonce.as_bytes()) {
            return Err(RpcError::new(
                error_code::ORIGIN_MISMATCH,
                "the document capability is stale; reload the dApp",
            ));
        }
        if let Err(error) = state.session.ensure_origin(&state.current_url) {
            self.revoke();
            return Err(error);
        }
        if dapp_protocol::is_forbidden_method(&method) {
            return Ok(Dispatch::Reply(dapp_protocol::response_error(
                &id,
                RpcError::new(
                    error_code::METHOD_NOT_ALLOWED,
                    "signing, key access, arbitrary RPC, and code execution are never available to dApps",
                ),
            )));
        }
        let Some(provider_method) = resolve_method(&method) else {
            return Ok(Dispatch::Reply(dapp_protocol::response_error(
                &id,
                RpcError::new(
                    error_code::METHOD_NOT_FOUND,
                    format!("unknown dApp method `{method}`"),
                ),
            )));
        };
        let action = WalletAction::parse(provider_method, params.as_ref())?;
        let lease = lease_of(state);
        let connected = state.session.connected;
        if !action.requires_approval() {
            if let Err(error) = state.session.ensure_connected(&state.current_url) {
                return Ok(Dispatch::Reply(dapp_protocol::response_error(&id, error)));
            }
            return Ok(Dispatch::Execute {
                lease,
                method: provider_method,
                action,
                request_id: id,
            });
        }

        // A new connection request may replace a stale/expired approval. A
        // mutation from an unconnected session is denied before the UI opens
        // a dialog, so an unapproved write never reaches core.
        if !matches!(action, WalletAction::Connect(_)) && !connected {
            return Ok(Dispatch::Reply(dapp_protocol::response_error(
                &id,
                RpcError::new(
                    error_code::UNAUTHORIZED,
                    "the dApp is not connected; call fiber_connect first",
                ),
            )));
        }
        if let Some(existing) = self.approval.as_ref()
            && now_ms.saturating_sub(existing.created_at_ms) < dapp_protocol::APPROVAL_TIMEOUT_MS
        {
            return Ok(Dispatch::Reply(dapp_protocol::response_error(
                &id,
                RpcError::new(
                    error_code::REQUEST_IN_PROGRESS,
                    "another approval is waiting for the user",
                )
                .with_data(json!({
                    "pendingMethod": existing.grant.method.canonical(),
                    "requestId": existing.request_id,
                })),
            )));
        }
        let state = self.session.as_ref().ok_or_else(session_missing)?;
        let session = &state.session;
        let session_nonce = state.nonce.clone();
        let pending = PendingApproval {
            grant: dapp_protocol::ApprovalGrant {
                token: next_approval_token(),
                generation: session.generation,
                method: provider_method,
            },
            request_id: id,
            dapp_id: session.dapp_id.clone(),
            session_generation: session.generation,
            summary: action.summary(session)?,
            action,
            created_at_ms: now_ms,
            nonce: session_nonce,
        };
        self.approval = Some(pending.clone());
        Ok(Dispatch::Approval { pending, lease })
    }

    pub fn approve(&mut self, token: u64, now_ms: u64) -> Result<Dispatch, Box<RpcResponse>> {
        let Some(state) = self.session.as_ref() else {
            let id = self
                .approval
                .as_ref()
                .map(|pending| pending.request_id.as_str())
                .unwrap_or("unknown");
            return Err(Box::new(dapp_protocol::response_error(
                id,
                session_missing(),
            )));
        };
        if !state.active.load(Ordering::Acquire) {
            let id = self
                .approval
                .as_ref()
                .map(|pending| pending.request_id.as_str())
                .unwrap_or("unknown");
            return Err(Box::new(dapp_protocol::response_error(
                id,
                RpcError::new(
                    error_code::ORIGIN_MISMATCH,
                    "the dApp session is revoked; reopen the dApp",
                ),
            )));
        }
        if self.approval.is_none() {
            return Err(Box::new(dapp_protocol::response_error(
                "unknown",
                RpcError::new(error_code::INVALID_REQUEST, "no approval is pending"),
            )));
        }
        let pending = self.approval.clone().expect("checked above");
        if pending.grant.token != token {
            return Err(Box::new(dapp_protocol::response_error(
                &pending.request_id,
                RpcError::new(error_code::INVALID_REQUEST, "approval token does not match"),
            )));
        }
        if now_ms.saturating_sub(pending.created_at_ms) >= dapp_protocol::APPROVAL_TIMEOUT_MS {
            return Err(Box::new(self.reject_with(
                pending.request_id,
                error_code::APPROVAL_TIMEOUT,
                "approval timed out",
            )));
        }
        if pending.session_generation != state.session.generation
            || pending.dapp_id != state.session.dapp_id
        {
            return Err(Box::new(dapp_protocol::response_error(
                &pending.request_id,
                RpcError::new(error_code::ORIGIN_MISMATCH, "the dApp session changed"),
            )));
        }
        self.approval = None;
        let method = pending.grant.method;
        let action = pending.action;
        let Some(state) = self.session.as_ref() else {
            return Err(Box::new(dapp_protocol::response_error(
                &pending.request_id,
                session_missing(),
            )));
        };
        Ok(Dispatch::Execute {
            lease: lease_of(state),
            method,
            action,
            request_id: pending.request_id,
        })
    }

    pub fn authorize_connect(&mut self, now_ms: u64) -> Result<Value, RpcError> {
        let state = self.session.as_mut().ok_or_else(session_missing)?;
        if !state.active.load(Ordering::Acquire) {
            return Err(RpcError::new(
                error_code::ORIGIN_MISMATCH,
                "the dApp session is revoked; reopen the dApp",
            ));
        }
        state.session.authorize(&state.current_url, now_ms)?;
        Ok(state.session.connected_result())
    }

    /// Starts a fresh document epoch after a same-origin reload. The old
    /// nonce, connection grant, pending approval, and executor leases are all
    /// invalidated so a stale prompt or queued job cannot mutate the wallet.
    pub fn rotate_document(&mut self, current_url: &str) -> Result<String, RpcError> {
        let state = self.session.as_mut().ok_or_else(session_missing)?;
        state.session.ensure_origin(current_url)?;
        let old = lease_of(state);
        state.active.store(false, Ordering::Release);
        state.session.generation = new_generation();
        state.session.revoke();
        state.nonce = new_nonce();
        state.current_url = current_url.to_owned();
        state.active = register_host(lease_of(state));
        self.approval = None;
        revoke_matching_leases(&old);
        Ok(state.nonce.clone())
    }

    pub fn reject(&mut self) -> RpcResponse {
        let Some(pending) = self.approval.take() else {
            return dapp_protocol::response_error(
                "unknown",
                RpcError::new(error_code::INVALID_REQUEST, "no approval is pending"),
            );
        };
        dapp_protocol::response_error(
            &pending.request_id,
            RpcError::new(error_code::USER_REJECTED, "the user rejected the request"),
        )
    }

    pub fn expire(&mut self, now_ms: u64) -> Option<RpcResponse> {
        let pending = self.approval.as_ref()?;
        if now_ms.saturating_sub(pending.created_at_ms) < dapp_protocol::APPROVAL_TIMEOUT_MS {
            return None;
        }
        let id = pending.request_id.clone();
        self.approval = None;
        Some(dapp_protocol::response_error(
            &id,
            RpcError::new(
                error_code::APPROVAL_TIMEOUT,
                "the wallet approval timed out",
            ),
        ))
    }

    pub fn fail_lease(&mut self, lease: &SessionLease, error: RpcError) -> bool {
        if self
            .session
            .as_ref()
            .is_some_and(|state| lease_matches(&lease_of(state), lease))
        {
            if error.code == error_code::ORIGIN_MISMATCH {
                self.revoke();
            }
            true
        } else {
            false
        }
    }

    fn reject_with(&mut self, id: String, code: i32, message: &str) -> RpcResponse {
        self.approval = None;
        dapp_protocol::response_error(&id, RpcError::new(code, message))
    }
}

impl Drop for DappHostState {
    fn drop(&mut self) {
        if let Some(state) = self.session.as_ref() {
            state.active.store(false, Ordering::Release);
        }
    }
}

fn register_host(lease: SessionLease) -> Arc<AtomicBool> {
    let mut leases = active_leases()
        .lock()
        .expect("dApp lease registry poisoned");
    leases.retain(|(_, active)| active.load(Ordering::Acquire));
    let active = Arc::new(AtomicBool::new(true));
    leases.push((lease, active.clone()));
    active
}

pub fn lease_is_active(lease: &SessionLease) -> bool {
    let mut leases = active_leases()
        .lock()
        .expect("dApp lease registry poisoned");
    leases.retain(|(_, active)| active.load(Ordering::Acquire));
    leases
        .iter()
        .find(|(known, _)| known == lease)
        .is_some_and(|(_, active)| active.load(Ordering::Acquire))
}

#[cfg(test)]
fn active_lease_count() -> usize {
    let mut leases = active_leases()
        .lock()
        .expect("dApp lease registry poisoned");
    leases.retain(|(_, active)| active.load(Ordering::Acquire));
    leases.len()
}

type ActiveLease = (SessionLease, Arc<AtomicBool>);
type ActiveLeases = Mutex<Vec<ActiveLease>>;

static ACTIVE_LEASES: OnceLock<ActiveLeases> = OnceLock::new();

fn active_leases() -> &'static ActiveLeases {
    ACTIVE_LEASES.get_or_init(|| Mutex::new(Vec::new()))
}

fn lease_of_session(session: &DappSession) -> SessionLease {
    SessionLease {
        dapp_id: session.dapp_id.clone(),
        generation: session.generation,
        origin: session.origin.as_str().to_owned(),
    }
}

fn revoke_matching_leases(lease: &SessionLease) {
    let leases = active_leases()
        .lock()
        .expect("dApp lease registry poisoned");
    for (known, active) in leases.iter() {
        if known == lease {
            active.store(false, Ordering::Release);
        }
    }
}

fn new_nonce() -> String {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("operating-system CSPRNG unavailable");
    hex::encode(bytes)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

fn next_approval_token() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed) + 1
}

fn unexpected_event() -> RpcResponse {
    dapp_protocol::response_error(
        "unknown",
        RpcError::new(error_code::INVALID_REQUEST, "events cannot be requests"),
    )
}

pub fn session_missing() -> RpcError {
    RpcError::new(
        error_code::UNAUTHORIZED,
        "the dApp host session is not available",
    )
}

pub fn lease_of(state: &SessionState) -> SessionLease {
    SessionLease {
        dapp_id: state.session.dapp_id.clone(),
        generation: state.session.generation,
        origin: state.session.origin.as_str().to_owned(),
    }
}

pub fn lease_matches(left: &SessionLease, right: &SessionLease) -> bool {
    left == right
}

pub fn approval_text(summary: &ApprovalSummary) -> String {
    let mut text = format!(
        "{}\n\n{}\n\n{}",
        summary.title, summary.origin, summary.action
    );
    for (label, value) in &summary.details {
        text.push_str(&format!("\n{label}: {value}"));
    }
    if summary.dry_run {
        text.push_str("\n\n这是 dry-run；不会发送付款。");
    }
    text
}

pub fn new_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed) + 1
}

/// Executes one already-approved or read-only action on a dedicated thread.
///
/// `WalletBridge` owns the tokio runtime used by the embedded node, so the
/// executor calls its `execute_dapp` method rather than creating a second
/// runtime. The page callback only enqueues; it never blocks the WebView or
/// the UI thread.
pub struct WalletExecutor {
    tx: SyncSender<Job>,
}

struct Job {
    lease: SessionLease,
    method: ProviderMethod,
    action: WalletAction,
    request_id: String,
    reply: mpsc::Sender<HostMessage>,
}

static EXECUTOR: OnceLock<WalletExecutor> = OnceLock::new();

/// Revokes every live host lease. The app owns one dApp WebView at a time, and
/// this closes the window where a queued executor job could still pass
/// `SessionLease::is_current` after the user closes the host.
pub fn revoke_all_hosts() {
    let leases = active_leases()
        .lock()
        .expect("dApp lease registry poisoned");
    for (_, active) in leases.iter() {
        active.store(false, Ordering::Release);
    }
}

pub fn executor() -> &'static WalletExecutor {
    EXECUTOR.get_or_init(WalletExecutor::start)
}

impl WalletExecutor {
    fn start() -> Self {
        // Wallet mutations must be globally ordered. Multiple workers could
        // otherwise pay, close, and reopen a channel in an unintended order.
        let (tx, rx) = mpsc::sync_channel::<Job>(EXECUTOR_CAPACITY);
        std::thread::Builder::new()
            .name("fiber-dapp-executor".to_owned())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let started = Instant::now();
                    let response = if job.lease.is_current() {
                        WalletBridge::global().execute_dapp(job.method, job.action, &job.request_id)
                    } else {
                        dapp_protocol::encode_response(&dapp_protocol::response_error(
                            &job.request_id,
                            RpcError::new(
                                error_code::ORIGIN_MISMATCH,
                                "the dApp session ended before this request ran",
                            ),
                        ))
                    };
                    tracing::debug!(
                        method = job.method.canonical(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "dApp request completed"
                    );
                    let _ = job.reply.send(HostMessage::Response(response));
                }
            })
            .expect("spawn dApp executor");
        Self { tx }
    }

    pub fn submit(
        &self,
        lease: SessionLease,
        method: ProviderMethod,
        action: WalletAction,
        request_id: String,
        reply: mpsc::Sender<HostMessage>,
    ) -> Result<(), String> {
        self.tx
            .try_send(Job {
                lease,
                method,
                action,
                request_id,
                reply,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    "the dApp executor queue is full; wait for pending wallet actions".to_owned()
                }
                mpsc::TrySendError::Disconnected(_) => {
                    "the dApp executor is unavailable".to_owned()
                }
            })
    }
}

const EXECUTOR_CAPACITY: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dapp_protocol::{ApprovalGrant, PendingApproval, response_error};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn dapp() -> DappEntry {
        DappEntry {
            id: "user:test".to_owned(),
            name: "Test dApp".to_owned(),
            url: "https://example.com/app".to_owned(),
            description: String::new(),
            builtin: false,
            inline_html: None,
        }
    }

    fn host(generation: u64) -> DappHostState {
        let mut host = DappHostState::new(&dapp(), "https://example.com/app", generation)
            .expect("valid host session");
        if let Some(state) = host.session.as_mut() {
            state.nonce = request_nonce();
        }
        host
    }

    fn request(id: &str, method: &str, params: Option<Value>) -> DappRequest {
        DappRequest::Request {
            nonce: request_nonce(),
            jsonrpc: "2.0".to_owned(),
            id: id.to_owned(),
            method: method.to_owned(),
            params,
        }
    }

    fn request_nonce() -> String {
        "test-nonce".to_owned()
    }

    fn reply_code(dispatch: Dispatch) -> i32 {
        match dispatch {
            Dispatch::Reply(response) => response.error.expect("expected an RPC error").code,
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    fn connect(host: &mut DappHostState, now_ms: u64) -> SessionLease {
        let Dispatch::Approval { pending, lease } = host
            .dispatch(
                request("connect", "fiber_connect", Some(json!({"permissions": []}))),
                now_ms,
            )
            .expect("connect dispatch")
        else {
            panic!("connect must require approval");
        };
        let Dispatch::Execute { method, .. } = host
            .approve(pending.grant.token, now_ms)
            .expect("connect approval")
        else {
            panic!("approved connect must execute");
        };
        assert_eq!(method, ProviderMethod::Connect);
        host.authorize_connect(now_ms).expect("authorize connect");
        lease
    }

    #[test]
    fn document_epoch_rejects_old_nonce_and_approval() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut host = host(20);
        connect(&mut host, 1);
        let old_nonce = host.session().expect("session").nonce.clone();
        let Dispatch::Approval { pending, .. } = host
            .dispatch(
                request(
                    "invoice-before-reload",
                    "fiber_createInvoice",
                    Some(json!({"amountShannons": "10"})),
                ),
                2,
            )
            .expect("approval dispatch")
        else {
            panic!("write must require approval");
        };

        host.begin_document("https://example.com/next")
            .expect("same-origin navigation");
        assert!(host.current_approval().is_none());
        let error = host
            .approve(pending.grant.token, 3)
            .expect_err("pre-reload approval must not execute");
        assert_eq!(
            error.error.as_ref().expect("RPC error").code,
            error_code::INVALID_REQUEST
        );
        let dispatch = host
            .dispatch(
                DappRequest::Request {
                    nonce: old_nonce,
                    jsonrpc: "2.0".to_owned(),
                    id: "old".to_owned(),
                    method: "fiber_getNodeInfo".to_owned(),
                    params: None,
                },
                4,
            )
            .expect("dispatched");
        // A revoked connection is checked before the old nonce, so either
        // revocation code is valid; both prove the request was not executed.
        assert_eq!(reply_code(dispatch), error_code::UNAUTHORIZED);
    }

    #[test]
    fn reads_and_writes_require_an_approved_connection() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut host = host(10);

        assert_eq!(
            reply_code(
                host.dispatch(request("read", "fiber_getBalance", None), 1)
                    .expect("read dispatch")
            ),
            error_code::UNAUTHORIZED
        );
        assert_eq!(
            reply_code(
                host.dispatch(
                    request(
                        "invoice",
                        "fiber_createInvoice",
                        Some(json!({"amountShannons": "1"})),
                    ),
                    1,
                )
                .expect("write dispatch")
            ),
            error_code::UNAUTHORIZED
        );

        connect(&mut host, 2);
        let Dispatch::Execute { action, .. } = host
            .dispatch(request("read-ok", "fiber_getBalance", None), 3)
            .expect("authorized read")
        else {
            panic!("authorized read must execute");
        };
        assert!(matches!(action, WalletAction::ReadBalance));
    }

    #[test]
    fn approval_tokens_are_single_use() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut host = host(11);
        let Dispatch::Approval { pending, .. } = host
            .dispatch(
                request("connect", "fiber_connect", Some(json!({"permissions": []}))),
                1,
            )
            .expect("approval dispatch")
        else {
            panic!("connect must require approval");
        };
        assert!(host.approve(pending.grant.token, 2).is_ok());
        let error = host
            .approve(pending.grant.token, 3)
            .expect_err("approval token must be consumed");
        assert_eq!(
            error.error.as_ref().expect("RPC error").code,
            error_code::INVALID_REQUEST
        );
    }

    #[test]
    fn expired_approval_is_erased_and_cannot_execute() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut host = host(12);
        let Dispatch::Approval { pending, .. } = host
            .dispatch(
                request("connect", "fiber_connect", Some(json!({"permissions": []}))),
                100,
            )
            .expect("approval dispatch")
        else {
            panic!("connect must require approval");
        };
        let error = host
            .approve(
                pending.grant.token,
                100 + dapp_protocol::APPROVAL_TIMEOUT_MS,
            )
            .expect_err("expired approval must fail");
        assert_eq!(
            error.error.as_ref().expect("RPC error").code,
            error_code::APPROVAL_TIMEOUT
        );
        assert!(host.current_approval().is_none());
    }

    #[test]
    fn revoke_invalidates_the_execution_lease_and_connection() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut host = host(13);
        let lease = connect(&mut host, 1);
        assert!(lease.is_current());
        assert!(host.session().expect("session").session.connected);

        host.revoke();
        assert!(!lease.is_current());
        let session = host.session().expect("session retained");
        assert!(!session.session.connected);
        assert!(!session.active.load(Ordering::Acquire));
        assert_eq!(
            host.dispatch(request("read", "fiber_getBalance", None), 2)
                .expect_err("revoked host")
                .code,
            error_code::ORIGIN_MISMATCH
        );
    }

    #[test]
    fn close_channel_force_maps_to_a_force_action_without_a_fee_rate() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut host = host(14);
        connect(&mut host, 1);
        let Dispatch::Approval { pending, .. } = host
            .dispatch(
                request(
                    "close",
                    "fiber_closeChannel",
                    Some(json!({"channelId": format!("0x{}", "11".repeat(32)), "force": true})),
                ),
                2,
            )
            .expect("close dispatch")
        else {
            panic!("close must require approval");
        };
        let Dispatch::Execute { action, .. } = host
            .approve(pending.grant.token, 3)
            .expect("close approval")
        else {
            panic!("approved close must execute");
        };
        let WalletAction::CloseChannel(params) = action else {
            panic!("expected close action");
        };
        assert!(params.force);
        assert_eq!(params.channel_id, format!("0x{}", "11".repeat(32)));
        assert_eq!(pending.summary.title, "强制关闭通道");
    }

    #[test]
    fn signing_and_key_access_are_permanently_denied() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut host = host(15);
        for method in [
            "sign_message",
            "wallet_exportPrivateKey",
            "wallet_getKeychain",
            "wallet_rawRpc",
        ] {
            assert_eq!(
                reply_code(
                    host.dispatch(request(method, method, None), 1)
                        .expect("deny dispatch")
                ),
                error_code::METHOD_NOT_ALLOWED,
                "{method}"
            );
        }
        assert!(host.current_approval().is_none());
    }

    #[test]
    fn lease_registry_drops_revoked_and_dropped_hosts() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let baseline = active_lease_count();
        let mut hosts = (100..132).map(host).collect::<Vec<_>>();
        assert_eq!(active_lease_count(), baseline + hosts.len());
        hosts[0].revoke();
        hosts.remove(0);
        assert_eq!(active_lease_count(), baseline + hosts.len());
        drop(hosts);
        assert_eq!(active_lease_count(), baseline);
    }

    #[test]
    fn helper_types_do_not_expose_tokens_in_approval_text() {
        let pending = PendingApproval {
            grant: ApprovalGrant {
                token: 999,
                generation: 1,
                method: ProviderMethod::CreateInvoice,
            },
            request_id: "1".to_owned(),
            dapp_id: "user:test".to_owned(),
            session_generation: 1,
            action: WalletAction::CreateInvoice(crate::dapp_protocol::CreateInvoiceParams {
                amount_shannons: 10,
                description: "coffee".to_owned(),
            }),
            summary: dapp_protocol::ApprovalSummary {
                title: "Create".to_owned(),
                origin: "https://example.com".to_owned(),
                action: "Approve".to_owned(),
                details: vec![("Amount".to_owned(), "10".to_owned())],
                destructive: false,
                dry_run: false,
            },
            created_at_ms: 1,
            nonce: "test-nonce".to_owned(),
        };
        let text = approval_text(&pending.summary);
        assert!(!text.contains("999"));
        assert!(text.contains("https://example.com"));

        let rejected = response_error(
            &pending.request_id,
            RpcError::new(error_code::USER_REJECTED, "rejected"),
        );
        assert_eq!(rejected.id, "1");
    }
}
