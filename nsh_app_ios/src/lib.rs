//! The app's Rust UI on iOS: a phone-shaped shell over the same wallet core
//! and bridge the macOS shell (`nsh_app`) uses — a status header, the four
//! shared pages (首页 / 交易 / 发现 / 设置) in a scroll area, and a bottom
//! tab bar instead of the desktop icon sidebar.

pub mod keychain;

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
    // Before the bridge initializes: the password comes from the Keychain and
    // is exported through FIBER_SECRET_KEY_PASSWORD, which the bridge reads
    // before its file fallback.
    tracing::warn!("ios shell: App() entered");
    keychain::install_keychain_password();
    tracing::warn!("ios shell: keychain done");

    let cx = Cx {
        config: signal(shared::config::get().clone()),
        nav: signal(Nav::Home),
        trade_tab: signal(0_usize),
        ui: signal(bridge::WalletBridge::global().state()),
        invoice_history: signal(Vec::new()),
        swap_amount: signal(String::new()),
        receive_amount: signal(String::new()),
        receive_description: signal(String::new()),
        pay_invoice: signal(String::new()),
    };

    // Persist config edits (theme, accent, proxy) as they happen.
    effect(move || {
        let snapshot: shared::config::AppConfig = cx.config.get();
        shared::config::save(&snapshot);
    });

    // Copy bridge state into the UI's signal a few times a second; the
    // bridge is the source of truth.
    let bridge = bridge::WalletBridge::global();
    let ui = cx.ui;
    spawn(async move {
        loop {
            timer(Duration::from_millis(400)).await;
            ui.set(bridge.state());
        }
    });
    tracing::warn!("ios shell: bridge initialized, building view");
    bridge.initialize_and_start();

    view()
        .flex_1()
        .bg(move || theme::palette(&cx.config.get()).bg)
        .text_color(move || theme::palette(&cx.config.get()).fg)
        .child(status_header(cx))
        .child(
            safe_area_view().flex_1().child(
                scroll().flex_1().child(
                    view()
                        .w_full()
                        .p_4()
                        .child(dynamic(move || match cx.nav.get() {
                            Nav::Home => Some(view().child(shared::pages::home::home_page(cx))),
                            Nav::Trade => {
                                Some(view().child(shared::pages::trade::trade_page(cx)))
                            }
                            Nav::Discover => {
                                Some(view().child(shared::pages::discover::discover_page(cx)))
                            }
                            Nav::Settings => {
                                Some(view().child(shared::pages::settings::settings_page(cx)))
                            }
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
        .child(
            dynamic(move || {
                let ui = bridge::WalletBridge::global().state();
                let Some(snapshot) = ui.snapshot else {
                    return view().child(text("…").text_size(px(theme::type_scale::SM)));
                };

                let (label, color_key): (&str, fn(&theme::Palette) -> Hsla) = match snapshot.phase
                {
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
            }),
        )
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
        .children([Nav::Home, Nav::Trade, Nav::Discover, Nav::Settings].map(|item| tab_item(cx, item)))
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
