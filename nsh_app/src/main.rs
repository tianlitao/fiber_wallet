//! Runs the Fiber wallet NSH app directly on the machine's native backend.
//!
//! Boots `tracing` (RUST_LOG controls the filter, default `info`) before the
//! engine starts so the embedded Fiber node's logs stream to the terminal.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> impl std::process::Termination {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    // Loads config.json (theme, accent, proxy) and applies the proxy env
    // before any RPC client is built.
    let config = nsh_app_fiber_wallet_nsh::config::load();
    tracing::info!(
        mode = ?config.theme_mode,
        accent = ?config.accent,
        proxy_enabled = config.proxy_enabled,
        "config loaded"
    );

    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<nsh_app_fiber_wallet_nsh::App>()),
        platform::AppOptions {
            title: "Fiber Wallet (NSH)".to_owned(),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            app_id: "dev.fiberwallet.nsh".to_owned(),
            size: (1240, 820),
            ..Default::default()
        },
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!(
    "Run this binary on macOS, Windows or Linux; use the TSX entry with nsh run for iOS and Android."
);
