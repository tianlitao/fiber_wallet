//! The app's Rust UI on iOS: a phone-shaped shell over the same wallet core
//! and bridge the macOS shell (`nsh_app`) uses — a status header, the four
//! shared pages (首页 / 交易 / 发现 / 设置) in a scroll area, and a bottom
//! tab bar instead of the desktop icon sidebar.

#[cfg(target_os = "ios")]
pub mod keychain;
#[cfg(target_os = "ios")]
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

#[native_module("fiber-wallet-ios/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// The application's root component.
#[component]
pub fn App() -> impl IntoElement {
    // The iOS runner installs only a `log` logger; without this the bridge's
    // tracing output (and 首页 → 节点事件) would be empty. Runs before the
    // Keychain and the bridge so their startup lines are captured too.
    install_tracing();
    tracing::warn!("ios shell: App() entered");
    // The camera scanner backs 交易页's 扫一扫 buttons (shared `scan` hook).
    #[cfg(target_os = "ios")]
    scanner::install();
    // Loads config.json (theme, accent, proxy) and applies the proxy env
    // before any RPC client is built, as the macOS binary does in `main`.
    // This must be `load`, not `get`: `get` is `get_or_init(load)`, and
    // `load` itself writes the same OnceLock — the first `get` caller
    // would wait on its own initialization forever.
    let config = shared::config::load();
    tracing::info!(
        mode = ?config.theme_mode,
        accent = ?config.accent,
        proxy_enabled = config.proxy_enabled,
        "config loaded"
    );
    // Before the bridge initializes: the password comes from the Keychain and
    // is exported through FIBER_SECRET_KEY_PASSWORD, which the bridge reads
    // before its file fallback. (The nsh tooling also builds this crate on
    // the host to describe components; the module is iOS-only.)
    #[cfg(target_os = "ios")]
    keychain::install_keychain_password();
    tracing::warn!("ios shell: keychain done");

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
        .child(
            // One safe-area shell: the header clears the status bar (time /
            // notch) at the top, the tab bar clears the home indicator at the
            // bottom, and the scroll content sits between them.
            safe_area_view()
                .flex_1()
                .child(status_header(cx))
                .child(
                    scroll().flex_1().child(
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
                )
                .child(tab_bar(cx)),
        )
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
            // The reactive mirror, not a raw `bridge.state()` read: a plain
            // snapshot never re-renders, so the phase pill would freeze.
            let ui = cx.ui_state();
            let Some(snapshot) = ui.snapshot else {
                return view().child(text("…").text_size(px(theme::type_scale::SM)));
            };

            let (label, color_key): (&str, fn(&theme::Palette) -> Hsla) = match snapshot.phase {
                NodePhase::Running => ("节点在线", |p| p.ok),
                NodePhase::Starting => ("启动中", |p| p.warn),
                NodePhase::Stopping => ("停止中", |p| p.warn),
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
/// env filter (default `info`) over a stdout layer and the in-memory tail
/// that 首页 → 节点事件 renders. The iOS runner only configures the `log`
/// crate, so the shell must install the subscriber itself, before the bridge
/// or the Keychain log anything.
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
                    .with_ansi(false),
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
        .active(|style| style.opacity(0.45))
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
