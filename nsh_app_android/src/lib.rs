//! The app's Rust UI on Android: a phone-shaped shell over the same wallet
//! core and bridge the macOS (`nsh_app`) and iOS (`nsh_app_ios`) shells use —
//! a status header, the four shared pages (首页 / 交易 / 发现 / 设置) in a
//! scroll area, and a bottom tab bar instead of the desktop icon sidebar.
//!
//! Platform bits mirror the iOS shell: the wallet password lives in the
//! Android Keystore (`keystore.rs`) and 交易页's 扫一扫 opens the Camera2
//! scanner (`scanner.rs`) through the shared `scan` hook.

#[cfg(target_os = "android")]
pub mod keystore;
// Platform-free so the decoder runs in host tests (`qr.rs`).
pub mod qr;
#[cfg(target_os = "android")]
pub mod scanner;

use std::sync::OnceLock;
use std::time::Duration;

use fiber_wallet_core::api::node::NodePhase;
use nsh_app_fiber_wallet_nsh as shared;
use nsh_base::safe_area_view;
use nsh_core::ui::prelude::*;

use shared::bridge;
use shared::theme;
use shared::widgets::{Cx, Nav, pal_fn};

#[native_module("fiber-wallet-android/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// The application's root component.
#[component]
pub fn App() -> impl IntoElement {
    // The Android runner installs only the engine's `log` logger; without
    // this the bridge's tracing output (and 首页 → 节点事件) would be empty.
    // Runs before the Keystore and the bridge so their startup lines are
    // captured too.
    install_tracing();
    tracing::warn!("android shell: App() entered");
    // The camera scanner backs 交易页's 扫一扫 buttons (shared `scan` hook).
    #[cfg(target_os = "android")]
    scanner::install();
    // Before config and the bridge initialize: points the wallet data
    // directory and config.json at the app's files directory and exports the
    // Keystore password through FIBER_SECRET_KEY_PASSWORD, which the bridge
    // reads before its file fallback. (The nsh tooling also builds this
    // crate on the host to describe components; the module is Android-only.)
    #[cfg(target_os = "android")]
    keystore::install_keystore_password();
    // Loads config.json (theme, accent, proxy, network) and applies the
    // proxy env before any RPC client is built, as the macOS binary does in
    // `main`. This must be `load`, not `get`: `get` is `get_or_init(load)`,
    // and `load` itself writes the same OnceLock — the first `get` caller
    // would wait on its own initialization forever.
    let config = shared::config::load();
    tracing::info!(
        mode = ?config.theme_mode,
        accent = ?config.accent,
        proxy_enabled = config.proxy_enabled,
        "config loaded"
    );

    let cx = Cx {
        config: signal(shared::config::get().clone()),
        nav: signal(Nav::Home),
        trade_tab: signal(0_usize),
        ui: signal(bridge::WalletBridge::global().state()),
        invoice_history: signal(Vec::new()),
        transfer_target: signal(String::new()),
        transfer_amount: signal(String::new()),
        transfer_max_fee: signal(String::from("0.01")),
        onchain_address: signal(String::new()),
        onchain_amount: signal(String::new()),
        onchain_max_fee: signal(String::from("0.001")),
        onchain_confirming: signal(false),
        onchain_validation_error: signal(None),
        receive_amount: signal(String::new()),
        receive_description: signal(String::new()),
        pay_invoice: signal(String::new()),
        channel_peer: signal(String::new()),
        channel_address: signal(String::new()),
        channel_amount: signal(String::new()),
        channel_public: signal(true),
        dapp_open: signal(None),
        dapp_navigation: signal(None),
        dapp_loading: signal(false),
        dapp_error: signal(None),
        dapp_outbox: signal(Vec::new()),
        dapp_generation: signal(0_u64),
    };

    // Persist config edits (theme, accent, proxy) as they happen.
    effect(move || {
        let snapshot: shared::config::AppConfig = cx.config.get();
        shared::config::save(&snapshot);
    });

    // Surface every bridge operation result as a toast, keyed on a strictly
    // increasing sequence so one flash is announced exactly once.
    let seen_flash = signal(0_u64);
    effect(move || {
        if let Some(flash) = cx.ui.get().flash
            && flash.seq > seen_flash.get()
        {
            seen_flash.set(flash.seq);
            toast(flash.message, ToastDuration::Long);
        }
    });

    // Copy bridge state into the UI's signal a few times a second; the
    // bridge is the source of truth. The log tail rides along so 首页's
    // 节点事件 card renders a rolling console of the tracing output.
    let bridge = bridge::WalletBridge::global();
    let ui = cx.ui;
    let poller = spawn(async move {
        loop {
            timer(Duration::from_millis(400)).await;
            let mut state = bridge.state();
            state.log_tail = shared::logs::global().tail(80);
            ui.set(state);
        }
    });
    cleanup(move || poller.cancel());
    bridge.initialize_and_start();

    view()
        .flex_1()
        .bg(move || theme::palette(&cx.config.get()).bg)
        .text_color(move || theme::palette(&cx.config.get()).fg)
        .child(status_header(cx))
        .child(
            safe_area_view()
                .flex_1()
                .child(
                    scroll()
                        .flex_1()
                        .child(
                            view()
                                .w_full()
                                .p_4()
                                .child(dynamic(move || match cx.nav.get() {
                                    Nav::Home => {
                                        Some(view().child(shared::pages::home::home_page(cx)))
                                    }
                                    Nav::Trade => {
                                        Some(view().child(shared::pages::trade::trade_page(cx)))
                                    }
                                    Nav::Discover => Some(
                                        view().child(shared::pages::discover::discover_page(cx)),
                                    ),
                                    Nav::Settings => Some(
                                        view().child(shared::pages::settings::settings_page(cx)),
                                    ),
                                })),
                        ),
                ),
        )
        .child(tab_bar(cx))
}

/// A slim header: wallet title on the left, the live node phase on the right
/// (with a retry action when the node failed).
fn status_header(cx: Cx) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .pt_3()
        .pb_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    view()
                        .size(px(28.0))
                        .items_center()
                        .justify_center()
                        .rounded(px(8.0))
                        .bg_linear(deg(135.0))
                        .gradient_from(move || pal_fn(&cx).gradient_from)
                        .gradient_to(move || pal_fn(&cx).gradient_to)
                        .child(text("🐋").text_size(px(14.0))),
                )
                .child(
                    text("Fiber Wallet")
                        .text_size(px(theme::type_scale::LG))
                        .font_weight(FontWeight::SEMIBOLD),
                ),
        )
        .child(dynamic(move || {
            let ui = bridge::WalletBridge::global().state();
            let Some(snapshot) = ui.snapshot else {
                return view().child(text("…").text_size(px(theme::type_scale::SM)));
            };

            let (label, color_key): (&str, fn(&theme::Palette) -> Hsla) = match snapshot.phase {
                NodePhase::Running => ("节点在线", |p| p.accent),
                NodePhase::Starting | NodePhase::Stopping => ("切换中", |p| p.fg_muted),
                NodePhase::Stopped => ("离线", |p| p.fg_muted),
                NodePhase::Failed => ("失败", |p| p.danger),
            };
            let colors = pal_fn(&cx);
            let pill = view()
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded(px(999.0))
                .bg(colors.card)
                .child(
                    view()
                        .size(px(7.0))
                        .rounded(px(999.0))
                        .bg(color_key(&colors)),
                )
                .child(
                    text(label)
                        .text_size(px(theme::type_scale::SM))
                        .text_color(colors.fg_muted),
                );

            if snapshot.phase == NodePhase::Failed {
                pill.child(
                    text("重试")
                        .text_size(px(theme::type_scale::SM))
                        .text_color(pal_fn(&cx).accent)
                        .on_press(move || bridge::WalletBridge::global().start()),
                )
            } else {
                pill
            }
        }))
}

/// Installs the tracing stack the macOS binary sets up in `main`: the same
/// env filter (default `info`) over a logcat layer and the in-memory tail
/// that 首页 → 节点事件 renders. The engine's own `log` logger already owns
/// the global `log` slot, so tracing events reach logcat through this fmt
/// layer's writer instead of a tracing-log bridge.
fn install_tracing() {
    use tracing_subscriber::prelude::*;

    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let log_tail = shared::logs::global().clone();
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(false)
                    .with_ansi(false)
                    .with_writer(logcat_writer()),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(false)
                    .with_ansi(false)
                    .with_writer(log_tail),
            );
        // Another subscriber may already be installed (a future runner or a
        // host integration); keep theirs rather than panicking.
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

/// The logcat writer source. The engine's own `log` logger already owns the
/// global `log` slot, so tracing events reach logcat through this fmt
/// layer's writer instead of a tracing-log bridge.
#[cfg(target_os = "android")]
fn logcat_writer() -> LogcatMakeWriter {
    LogcatMakeWriter
}

/// The describe build on the host has no logcat; events go nowhere.
#[cfg(not(target_os = "android"))]
fn logcat_writer() -> impl for<'a> tracing_subscriber::fmt::MakeWriter<'a> {
    use std::io::sink;
    || sink()
}

/// Forwards complete lines to Android's log (visible with
/// `adb logcat -s nsh`), buffering the fmt layer's partial writes. The
/// writer lives for one event, so a plain line buffer is enough.
#[cfg(target_os = "android")]
struct LogcatWriter {
    line: Vec<u8>,
}

#[cfg(target_os = "android")]
impl std::io::Write for LogcatWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.line.extend_from_slice(buf);
        while let Some(end) = self.line.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.line.drain(..=end).collect();
            logcat_line(&line[..line.len() - 1]);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "android")]
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogcatMakeWriter {
    type Writer = LogcatWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogcatWriter { line: Vec::new() }
    }
}

#[cfg(target_os = "android")]
struct LogcatMakeWriter;

#[cfg(target_os = "android")]
fn logcat_line(line: &[u8]) {
    use std::ffi::{c_char, c_int, CString};

    // SAFETY: __android_log_write copies both NUL-terminated strings.
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    const INFO: c_int = 4;
    let text = CString::new(String::from_utf8_lossy(line).replace('\0', " "))
        .unwrap_or_default();
    // SAFETY: as above.
    unsafe {
        __android_log_write(INFO, c"nsh".as_ptr(), text.as_ptr());
    }
}

/// The bottom tab bar: icon above label, one entry per page.
fn tab_bar(cx: Cx) -> View {
    view()
        .flex_row()
        .items_center()
        .w_full()
        .px_2()
        .py_2()
        .gap_1()
        .border_t(px(1.0))
        .border_t_color(move || pal_fn(&cx).border)
        .bg(move || theme::palette(&cx.config.get()).bg)
        .children(
            [Nav::Home, Nav::Trade, Nav::Discover, Nav::Settings].map(|item| tab_item(cx, item)),
        )
}

fn tab_item(cx: Cx, item: Nav) -> View {
    view()
        .flex_1()
        .flex_col()
        .items_center()
        .gap(px(2.0))
        .py_1()
        .rounded(px(10.0))
        .cursor_pointer()
        .on_press(move || cx.nav.set(item))
        .child(
            text(item.glyph())
                .text_size(px(theme::type_scale::XL))
                .text_color(move || {
                    let colors = pal_fn(&cx);
                    if cx.nav.get() == item {
                        colors.accent
                    } else {
                        colors.fg_muted
                    }
                }),
        )
        .child(
            text(item.label())
                .text_size(px(theme::type_scale::XS))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    let colors = pal_fn(&cx);
                    if cx.nav.get() == item {
                        colors.accent
                    } else {
                        colors.fg_muted
                    }
                }),
        )
}
