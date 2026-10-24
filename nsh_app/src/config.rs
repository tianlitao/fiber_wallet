//! App configuration persisted to `config.json`: theme, accent, and proxy.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// How the app picks light or dark appearance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "跟随系统",
            ThemeMode::Light => "浅色",
            ThemeMode::Dark => "深色",
        }
    }

    pub const ALL: [ThemeMode; 3] = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];
}

/// The app's accent colors, Nervos/Fiber-flavored (see `theme.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Accent {
    /// CKB mint green — the Nervos ecosystem's signature tone.
    #[default]
    CkbMint,
    /// Fiber teal (also Longbridge's current brand token).
    FiberTeal,
    /// Aurora blue.
    Aurora,
    /// Violet.
    Violet,
    /// Amber.
    Amber,
}

impl Accent {
    pub fn label(self) -> &'static str {
        match self {
            Accent::CkbMint => "CKB 薄荷",
            Accent::FiberTeal => "Fiber 青",
            Accent::Aurora => "极光蓝",
            Accent::Violet => "星紫",
            Accent::Amber => "琥珀",
        }
    }

    pub const ALL: [Accent; 5] = [
        Accent::CkbMint,
        Accent::FiberTeal,
        Accent::Aurora,
        Accent::Violet,
        Accent::Amber,
    ];
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub theme_mode: ThemeMode,
    pub accent: Accent,
    pub proxy_enabled: bool,
    pub proxy_host: String,
    pub proxy_port: u16,
}

static CONFIG: OnceLock<AppConfig> = OnceLock::new();

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join("Library/Application Support/FiberWalletNSH/config.json")
}

/// Loads `config.json` once at startup; the result is what
/// [`get`] returns for the rest of the process.
pub fn load() -> AppConfig {
    let loaded = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|text| {
            serde_json::from_str::<AppConfig>(&text)
                .map_err(|error| eprintln!("config.json invalid, using defaults: {error}"))
                .ok()
        })
        .unwrap_or_default();
    let _ = CONFIG.set(loaded.clone());
    apply_proxy_env(&loaded);
    loaded
}

/// The config loaded at startup. Settings edits update the UI signal and call
/// [`save`]; this stays the process-wide baseline.
pub fn get() -> &'static AppConfig {
    CONFIG.get_or_init(load)
}

pub fn save(config: &AppConfig) {
    if let Some(dir) = config_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(config) {
        Ok(text) => {
            if let Err(error) = std::fs::write(config_path(), text) {
                eprintln!("failed to write config.json: {error}");
            }
        }
        Err(error) => eprintln!("failed to encode config: {error}"),
    }
}

/// Applies the configured HTTP(S) proxy to this process's environment. The
/// CKB RPC clients (reqwest-based) pick these up when they are built, so a
/// node restart makes the proxy effective; P2P connections are never proxied.
pub fn apply_proxy_env(config: &AppConfig) {
    let keys = ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"];
    if config.proxy_enabled && !config.proxy_host.trim().is_empty() && config.proxy_port > 0 {
        let url = format!("http://{}:{}", config.proxy_host.trim(), config.proxy_port);
        // Called once at startup before threads spawn, and from the settings
        // thread afterwards; no other thread reads the environment
        // concurrently in either case.
        for key in keys {
            #[allow(unused_unsafe)]
            unsafe {
                std::env::set_var(key, &url)
            };
        }
        tracing::info!(proxy = %url, "applied proxy to process environment");
    } else {
        for key in keys {
            #[allow(unused_unsafe)]
            unsafe {
                std::env::remove_var(key)
            };
        }
    }
}
