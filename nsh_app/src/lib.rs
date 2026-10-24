//! The app's Rust UI: a Longbridge-style shell — icon sidebar, four pages
//! (首页 / 交易 / 发现 / 设置), the Nervos/Fiber-flavored theme from
//! `theme.rs`, and the real wallet core from `bridge.rs`.

use std::time::Duration;

pub mod bridge;
pub mod config;
pub mod pages;
pub mod theme;
pub mod widgets;

use nsh_base::safe_area_view;
use nsh_core::desktop::TitleBarStyle;
#[cfg(not(target_os = "ios"))]
use nsh_core::ui::desktop::{set_title_bar, title_bar};
use nsh_core::ui::prelude::*;

use crate::config::AppConfig;
use crate::theme::type_scale as ts;
use crate::widgets::{Cx, Nav, pal_fn};

#[native_module("fiber-wallet-nsh/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// The application's root component.
#[component]
pub fn App() -> impl IntoElement {
    // Headless tests set this: no bridge, no node, just the shell.
    let test_mode = std::env::var("FIBER_WALLET_TEST").is_ok();
    let cx = Cx {
        config: signal(config::get().clone()),
        nav: signal(Nav::Home),
        trade_tab: signal(0_usize),
        ui: signal(if test_mode {
            bridge::UiState::default()
        } else {
            bridge::WalletBridge::global().state()
        }),
        invoice_history: signal(Vec::new()),
        swap_amount: signal(String::new()),
        receive_amount: signal(String::new()),
        receive_description: signal(String::new()),
        pay_invoice: signal(String::new()),
    };

    // Persist config edits (theme, accent, proxy) as they happen.
    effect(move || {
        let snapshot: AppConfig = cx.config.get();
        config::save(&snapshot);
    });

    // Copy bridge state into the UI's signal a few times a second; the
    // bridge is the source of truth.
    // The loop is cancelled when the root goes away, so the engine can shut
    // down (headless tests join it).
    if !test_mode {
        let bridge = bridge::WalletBridge::global();
        let poller = spawn(async move {
            loop {
                timer(Duration::from_millis(400)).await;
                cx.ui.set(bridge.state());
            }
        });
        cleanup(move || poller.cancel());
        bridge.initialize_and_start();
    }

    view()
        .flex_1()
        .bg(move || theme::palette(&cx.config.get()).bg)
        .text_color(move || theme::palette(&cx.config.get()).fg)
        .child(app_title_bar(cx))
        .child(
            view()
                .flex_1()
                .flex_row()
                .child(sidebar(cx))
                .child(
                    safe_area_view().flex_1().child(
                        scroll().flex_1().child(
                            view()
                                .w_full()
                                .items_center()
                                .child(
                                    view()
                                        .w_full()
                                        .max_w(px(1080.0))
                                        .p_6()
                                        .child(dynamic(move || match cx.nav.get() {
                                            Nav::Home => {
                                                Some(view().child(pages::home::home_page(cx)))
                                            }
                                            Nav::Trade => {
                                                Some(view().child(pages::trade::trade_page(cx)))
                                            }
                                            Nav::Discover => {
                                                Some(view().child(pages::discover::discover_page(cx)))
                                            }
                                            Nav::Settings => {
                                                Some(view().child(pages::settings::settings_page(cx)))
                                            }
                                        })),
                                ),
                        ),
                    ),
                ),
        )
}

/// The app's own title bar (desktop only): the system one is hidden, this one
/// drags the window and leaves the traffic lights their space on the left.
#[cfg(not(target_os = "ios"))]
fn app_title_bar(cx: Cx) -> View {
    // The headless test engine has no window: skip the desktop commands and
    // the title-bar insets read entirely.
    let test_mode = std::env::var("FIBER_WALLET_TEST").is_ok();
    if test_mode {
        return view();
    }
    set_title_bar(TitleBarStyle::Hidden, Some(px(44.0)));
    view()
        .h(px(44.0))
        .flex_row()
        .items_center()
        .gap_2()
        .window_drag(true)
        .pl(move || title_bar().left + px(16.0))
        .pr(move || title_bar().right + px(16.0))
        .bg(move || theme::palette(&cx.config.get()).bg)
        .child(text("🐋").text_size(px(13.0)))
        .child(
            text("Fiber Wallet")
                .text_size(px(ts::SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || pal_fn(&cx).fg),
        )
        .child(
            text("v0.2.0 · NSH")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(view().flex_1())
}

/// iOS renders no window chrome: the shell provides its own header.
#[cfg(target_os = "ios")]
fn app_title_bar(_cx: Cx) -> View {
    view()
}

fn sidebar(cx: Cx) -> View {
    let ui = bridge::WalletBridge::global();
    view()
        .w(px(208.0))
        .h_full()
        .gap_2()
        .p_4()
        .border_e(px(1.0))
        .border_e_color(move || pal_fn(&cx).border)
        .bg(move || theme::palette(&cx.config.get()).bg)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .pb_3()
                .child(
                    view()
                        .size(px(30.0))
                        .items_center()
                        .justify_center()
                        .rounded(px(9.0))
                        .bg_linear(deg(135.0))
                        .gradient_from(move || pal_fn(&cx).gradient_from)
                        .gradient_to(move || pal_fn(&cx).gradient_to)
                        .child(text("🐋").text_size(px(15.0))),
                )
                .child(
                    view()
                        .child(
                            text("Fiber Wallet")
                                .text_size(px(ts::MD))
                                .font_weight(FontWeight::BOLD)
                                .text_color(move || pal_fn(&cx).fg),
                        )
                        .child(
                            text("NSH shell")
                                .text_size(px(ts::XS))
                                .text_color(move || pal_fn(&cx).fg_muted),
                        ),
                ),
        )
        .children(Nav::ALL.iter().map(|item| nav_item(cx, *item)))
        .child(
            view()
                .flex_1(),
        )
        .child(
            view()
                .gap_1()
                .px_2()
                .py_3()
                .child(node_line(cx, ui))
                .child(
                    text("CKB Fiber 测试网")
                        .text_size(px(ts::XS))
                        .text_color(move || pal_fn(&cx).fg_muted),
                ),
        )
}

fn nav_item(cx: Cx, item: Nav) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .px_3()
        .py_2()
        .rounded(px(10.0))
        .bg(move || {
            let colors = pal_fn(&cx);
            if cx.nav.get() == item {
                colors.accent_soft
            } else {
                transparent_black()
            }
        })
        .cursor_pointer()
        .hover(move |style| {
            let colors = pal_fn(&cx);
            style.bg(if cx.nav.get() == item {
                colors.accent_soft
            } else {
                colors.card
            })
        })
        .on_press(move || cx.nav.set(item))
        .child(
            text(item.glyph())
                .text_size(px(ts::LG))
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
                .text_size(px(ts::MD))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    let colors = pal_fn(&cx);
                    if cx.nav.get() == item {
                        colors.fg
                    } else {
                        colors.fg_muted
                    }
                }),
        )
}

fn node_line(cx: Cx, ui: &'static bridge::WalletBridge) -> View {
    view().child(dynamic(move || {
        let state = ui.state();
        let colors = theme::palette(&cx.config.get());
        let (glyph, label, color) = match state.snapshot.map(|snapshot| snapshot.phase) {
            Some(NodePhase::Running) => ("●", "节点在线", colors.ok),
            Some(NodePhase::Starting) | Some(NodePhase::Stopping) => ("◌", "节点切换中", colors.warn),
            Some(NodePhase::Failed) => ("✕", "节点故障", colors.danger),
            _ => ("○", "节点离线", colors.fg_muted),
        };
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(text(glyph).text_size(px(ts::XS)).text_color(color))
            .child(
                text(label)
                    .text_size(px(ts::XS))
                    .text_color(colors.fg_muted),
            )
    }))
}

use fiber_wallet_core::api::node::NodePhase;
