//! The secret password that encrypts the wallet's key files: macOS Keychain
//! (KeychainVault) with a 0600 file fallback, plus `FIBER_SECRET_KEY_PASSWORD`
//! for explicit overrides. Replaces the old always-file dev fallback.

use std::path::{Path, PathBuf};

/// Where the keystore password lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultKind {
    /// macOS Keychain (Security.framework via the keyring crate).
    Keychain,
    /// Explicit `FIBER_SECRET_KEY_PASSWORD` env override.
    Env,
    /// 0600 file next to the data directory (fallback).
    File,
}

impl VaultKind {
    pub fn label(self) -> &'static str {
        match self {
            VaultKind::Keychain => "macOS 钥匙串",
            VaultKind::Env => "环境变量 FIBER_SECRET_KEY_PASSWORD",
            VaultKind::File => "数据目录文件（开发回退）",
        }
    }
}

const SERVICE: &str = "dev.fiberwallet.nsh";
const ACCOUNT: &str = "ckb-key-password";
const FILE_NAME: &str = ".key-password";

/// Loads the keystore password, creating one if needed. Priority:
/// `FIBER_SECRET_KEY_PASSWORD` env → Keychain → generate + Keychain →
/// generate + file (when the Keychain is unavailable).
pub fn load_or_create(data_dir: &Path) -> (String, VaultKind) {
    if let Some(password) = std::env::var("FIBER_SECRET_KEY_PASSWORD").ok().map(|password| password.trim().to_owned()).filter(|password| !password.is_empty()) {
        return (password, VaultKind::Env);
    }

    match keyring::Entry::new(SERVICE, ACCOUNT) {
        Ok(entry) => match entry.get_password() {
            Ok(existing) if !existing.trim().is_empty() => {
                return (existing.trim().to_owned(), VaultKind::Keychain);
            }
            Ok(_) => {}
            Err(keyring::Error::NoEntry) => {}
            Err(error) => {
                tracing::warn!(error = %error, "keychain read failed; falling back to file");
            }
        },
        Err(error) => {
            tracing::warn!(error = %error, "keychain entry unavailable; falling back to file");
        }
    }

    let (password, in_keychain) = adopt_file_password(data_dir);
    if in_keychain {
        return (password, VaultKind::Keychain);
    }
    (password, VaultKind::File)
}

/// First Keychain adoption: the data dir may already hold keys encrypted
/// under a file-fallback password. Runs after the Keychain read missed and
/// before generating a fresh password — adopting the existing one keeps the
/// key files decryptable. Returns the password and whether it now lives in
/// the Keychain (true: the file was removed) or stayed in the file.
fn adopt_file_password(data_dir: &Path) -> (String, bool) {
    if let Some(existing) = read_file(data_dir) {
        if store(&existing).is_ok() {
            let _ = std::fs::remove_file(file_path(data_dir));
            tracing::info!("adopted file keystore password into the keychain");
            return (existing, true);
        }
        return (existing, false);
    }
    let fresh = generate();
    (write_file(data_dir, &fresh), false)
}

/// Persists a new keystore password: Keychain first, file fallback.
pub fn store(password: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, ACCOUNT).map_err(|error| error.to_string())?;
    entry
        .set_password(password)
        .map_err(|error| error.to_string())
}

fn generate() -> String {
    (0..32)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect()
}

fn file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

fn read_file(data_dir: &Path) -> Option<String> {
    let existing = std::fs::read_to_string(file_path(data_dir)).ok()?;
    let trimmed = existing.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn write_file(data_dir: &Path, password: &str) -> String {
    use std::io::Write as _;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(file_path(data_dir))
        {
            let _ = file.write_all(password.as_bytes());
        }
    }
    #[cfg(not(unix))]
    if let Ok(mut file) = std::fs::File::create(file_path(data_dir)) {
        let _ = file.write_all(password.as_bytes());
    }
    password.to_owned()
}

/// File fallback used when the Keychain write fails; mirrors the old
/// dev-only behavior.
pub fn file_fallback(data_dir: &Path) -> String {
    if let Some(existing) = read_file(data_dir) {
        return existing;
    }
    write_file(data_dir, &generate())
}
