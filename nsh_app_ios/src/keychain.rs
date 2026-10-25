//! iOS Keychain backing for the wallet's secret password.
//!
//! The shared bridge reads `FIBER_SECRET_KEY_PASSWORD` first and falls back to
//! a file next to the data directory. On iOS we set that variable from the
//! Keychain before the bridge initializes, so the wallet password lives in the
//! system Keychain (protected by the device passcode) instead of a file. The
//! app sandbox already isolates the data directory; this keeps the key
//! encryption password out of it entirely.

use tracing::info;

const SERVICE: &str = "dev.fiberwallet.nsh.ios";
const ACCOUNT: &str = "fiber-wallet-key-password";

/// Loads the wallet password from the Keychain, creating one on first run,
/// and exports it as `FIBER_SECRET_KEY_PASSWORD` for the bridge.
///
/// Must run before the first `bridge::WalletBridge::global()` call. Failures
/// fall back to the bridge's file-based development path rather than blocking
/// startup; the error is logged for diagnosis.
pub fn install_keychain_password() {
    if std::env::var_os("FIBER_SECRET_KEY_PASSWORD").is_some_and(|value| !value.is_empty()) {
        return;
    }

    match load_or_create() {
        Ok(password) => {
            // Single-threaded engine startup reads this before any other
            // consumer; the process-wide env slot is ours to set. set_var is
            // unsafe under edition 2024 because of multithreaded readers —
            // this runs before the engine spawns its threads.
            // SAFETY: called from App() before the engine starts other threads.
            unsafe { std::env::set_var("FIBER_SECRET_KEY_PASSWORD", &password) };
            info!("wallet password loaded from the iOS Keychain");
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Keychain password unavailable; falling back to the file-based development password"
            );
        }
    }
}

fn load_or_create() -> Result<String, String> {
    match security_framework::passwords::get_generic_password(SERVICE, ACCOUNT) {
        Ok(password) => match String::from_utf8(password) {
            Ok(text) if !text.is_empty() => Ok(text),
            _ => create_and_store(),
        },
        // errSecItemNotFound: no password stored yet.
        Err(error) if error.code() == -25300 => create_and_store(),
        Err(error) => Err(format!("Keychain lookup failed: {error}")),
    }
}

fn create_and_store() -> Result<String, String> {
    let password: String = (0..32).map(|_| format!("{:02x}", rand_byte())).collect();
    security_framework::passwords::set_generic_password(SERVICE, ACCOUNT, password.as_bytes())
        .map_err(|error| format!("Keychain write failed: {error}"))?;
    Ok(password)
}

fn rand_byte() -> u8 {
    // Called only during first-run password generation; a simple counter-based
    // fallback is unnecessary — the security framework's own RNG is not
    // exposed, so use the OS random device.
    use std::fs::File;
    use std::io::Read as _;
    let mut byte = [0u8; 1];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut byte))
        .expect("/dev/urandom must be readable on iOS");
    byte[0]
}
