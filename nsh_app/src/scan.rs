//! QR scan hook: pages can request a camera scan without depending on a
//! concrete scanner. The iOS shell registers an AVFoundation scanner at
//! startup; desktop hosts register nothing, and the scan buttons hide
//! themselves ([`available`]).

use std::sync::{Arc, Mutex, OnceLock};

/// What the scanned text is for. The scanner may tailor its hint; the
/// callback routes the result to the requesting input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanTarget {
    /// A Fiber invoice to pay (交易 → 付款).
    Invoice,
    /// A CKB testnet address to transfer to (交易 → 链上 CKB).
    OnchainAddress,
}

/// Runs with the decoded text when a scan succeeds. Never runs when the
/// user cancels, denies camera access, or no camera is present.
pub type ScanCallback = Box<dyn FnOnce(String) + Send + Sync>;

type ScanHandler = Arc<dyn Fn(ScanTarget, ScanCallback) + Send + Sync>;

static HANDLER: OnceLock<Mutex<Option<ScanHandler>>> = OnceLock::new();

fn handler_slot() -> &'static Mutex<Option<ScanHandler>> {
    HANDLER.get_or_init(|| Mutex::new(None))
}

/// Installs the platform scanner (replacing any earlier one).
pub fn register(handler: impl Fn(ScanTarget, ScanCallback) + Send + Sync + 'static) {
    *handler_slot().lock().expect("scan handler poisoned") = Some(Arc::new(handler));
}

/// Whether a scanner is installed; scan buttons only show when true.
pub fn available() -> bool {
    handler_slot()
        .lock()
        .expect("scan handler poisoned")
        .is_some()
}

/// Opens the scanner; `on_result` receives the decoded text. Returns false
/// (and never calls `on_result`) when no scanner is installed.
pub fn request(target: ScanTarget, on_result: ScanCallback) -> bool {
    let current = handler_slot()
        .lock()
        .expect("scan handler poisoned")
        .clone();
    match current {
        Some(scan) => {
            scan(target, on_result);
            true
        }
        None => false,
    }
}
