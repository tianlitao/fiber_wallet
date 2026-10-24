//! Shared UI widgets, themed through the app's reactive [`Cx`].

use nsh_core::ui::prelude::*;

use crate::bridge::UiState;
use crate::config::AppConfig;
use crate::theme::{self, palette, type_scale as ts};

/// Shared reactive app state handed to every page.
#[derive(Clone, Copy)]
pub struct Cx {
    pub config: Signal<AppConfig>,
    pub nav: Signal<Nav>,
    pub trade_tab: Signal<usize>,
    /// Mirror of the bridge state, updated a few times a second by the root.
    pub ui: Signal<UiState>,
    /// Invoices created this session (newest first).
    pub invoice_history: Signal<Vec<fiber_wallet_core::api::wallet::WalletInvoice>>,
    /// Cross-page inputs, so they survive tab and page switches.
    pub swap_amount: Signal<String>,
    pub receive_amount: Signal<String>,
    pub receive_description: Signal<String>,
    pub pay_invoice: Signal<String>,
    pub channel_peer: Signal<String>,
    pub channel_address: Signal<String>,
    pub channel_amount: Signal<String>,
    pub channel_public: Signal<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Home,
    Trade,
    Discover,
    Settings,
}

impl Nav {
    pub const ALL: [Nav; 4] = [Nav::Home, Nav::Trade, Nav::Discover, Nav::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Nav::Home => "首页",
            Nav::Trade => "交易",
            Nav::Discover => "发现",
            Nav::Settings => "设置",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Nav::Home => "⌂",
            Nav::Trade => "⇌",
            Nav::Discover => "◈",
            Nav::Settings => "⚙",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Nav::Home => "首页",
            Nav::Trade => "交易 · Swap",
            Nav::Discover => "发现",
            Nav::Settings => "设置",
        }
    }
}

#[allow(dead_code)]
pub fn pal(cx: &Cx) -> Palette {
    palette(&cx.config.get())
}

/// A `Palette` for style closures: reads the config signal (tracked).
pub fn pal_fn(cx: &Cx) -> Palette {
    let config = cx.config;
    palette(&config.get())
}

pub fn card(cx: Cx) -> View {
    view()
        .rounded(px(12.0))
        .bg(move || pal_fn(&cx).card)
        .border_1()
        .border_color(move || pal_fn(&cx).border)
}

pub fn section_label(cx: Cx, label: &'static str) -> View {
    view().child(
        text(label)
            .text_size(px(ts::XS))
            .font_weight(FontWeight::SEMIBOLD)
            .letter_spacing(px(0.8))
            .text_color(move || pal_fn(&cx).fg_muted),
    )
}

pub fn page_header(cx: Cx, title: &'static str, subtitle: &'static str) -> View {
    view()
        .flex_row()
        .items_end()
        .justify_between()
        .child(
            view()
                .gap_1()
                .child(
                    text(title)
                        .text_size(px(ts::TITLE))
                        .font_weight(FontWeight::BOLD)
                        .text_color(move || pal_fn(&cx).fg),
                )
                .child(
                    text(subtitle)
                        .text_size(px(ts::SM))
                        .text_color(move || pal_fn(&cx).fg_muted),
                ),
        )
        .child(right_status(cx))
}

/// The node status pill shown at the top of every page.
pub fn right_status(cx: Cx) -> View {
    let ui = crate::bridge::WalletBridge::global();
    view().child(dynamic(move || {
        let state = ui.state();
        let colors = palette(&cx.config.get());
        let phase = state.snapshot.as_ref().map(|snapshot| snapshot.phase);
        let (label, glyph) = match phase {
            Some(NodePhase::Running) => ("在线", "●"),
            Some(NodePhase::Starting) | Some(NodePhase::Stopping) => ("切换中", "◌"),
            Some(NodePhase::Failed) => ("故障", "✕"),
            _ => ("离线", "○"),
        };
        let color = match phase {
            Some(NodePhase::Running) => colors.ok,
            Some(NodePhase::Failed) => colors.danger,
            Some(NodePhase::Starting) | Some(NodePhase::Stopping) => colors.warn,
            _ => colors.fg_muted,
        };
        Some(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(6.0))
                .rounded(px(999.0))
                .bg(colors.card_alt)
                .border_1()
                .border_color(colors.border)
                .child(
                    text(glyph)
                        .text_size(px(ts::XS))
                        .text_color(color)
                        .when(
                            matches!(
                                phase,
                                Some(NodePhase::Starting) | Some(NodePhase::Stopping)
                            ),
                            |dot| dot.animate_spin(),
                        ),
                )
                .child(
                    text(format!("{label} · 测试网"))
                        .text_size(px(ts::XS))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.fg),
                ),
        )
    }))
}

pub fn primary_button(
    cx: Cx,
    label: &'static str,
    enabled: impl Fn() -> bool + Clone + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    let enabled_bg = enabled.clone();
    let enabled_fg = enabled.clone();
    view()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .h(px(38.0))
        .px_4()
        .rounded(px(10.0))
        .bg(move || {
            let colors = pal_fn(&cx);
            if enabled_bg() {
                colors.accent
            } else {
                colors.card_alt
            }
        })
        .active(|style| style.opacity(0.8))
        .cursor_pointer()
        .on_press(move || {
            if enabled() {
                on_press();
            }
        })
        .child(
            text(label)
                .text_size(px(ts::MD))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || {
                    let colors = pal_fn(&cx);
                    if enabled_fg() {
                        colors.accent_fg
                    } else {
                        colors.fg_muted
                    }
                }),
        )
}

pub fn outline_button(
    cx: Cx,
    label: &'static str,
    enabled: impl Fn() -> bool + Clone + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    let enabled_fg = enabled.clone();
    view()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .h(px(38.0))
        .px_4()
        .rounded(px(10.0))
        .border_1()
        .border_color(move || pal_fn(&cx).border)
        .active(|style| style.opacity(0.75))
        .cursor_pointer()
        .on_press(move || {
            if enabled() {
                on_press();
            }
        })
        .child(
            text(label)
                .text_size(px(ts::MD))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    let colors = pal_fn(&cx);
                    if enabled_fg() {
                        colors.fg
                    } else {
                        colors.fg_muted
                    }
                }),
        )
}

pub fn toggle(cx: Cx, value: Signal<bool>, on_change: impl Fn(bool) + 'static) -> View {
    let track = value;
    let knob = value;
    view()
        .w(px(44.0))
        .h(px(26.0))
        .flex_row()
        .items_center()
        .p(px(2.0))
        .rounded(px(13.0))
        .bg(move || {
            let colors = pal_fn(&cx);
            if track.get() {
                colors.accent
            } else {
                colors.card_alt
            }
        })
        .border_1()
        .border_color(move || pal_fn(&cx).border)
        .transition_colors()
        .duration(150.0)
        .cursor_pointer()
        .on_press(move || {
            let next = !value.get();
            value.set(next);
            on_change(next);
        })
        .child(
            view()
                .size(px(20.0))
                .rounded(px(10.0))
                .bg(rgb(0xffffff))
                .shadow_sm()
                .translate_x(move || px(if knob.get() { 20.0 } else { 0.0 }))
                .transition_all()
                .duration(150.0),
        )
}

pub fn segmented(
    cx: Cx,
    options: &[&'static str],
    active: Signal<usize>,
) -> View {
    view()
        .flex_row()
        .p(px(3.0))
        .gap(px(2.0))
        .rounded(px(10.0))
        .bg(move || pal_fn(&cx).card_alt)
        .children(options.iter().enumerate().map(|(index, label)| {
            let selected = active;
            view()
                .px_3()
                .py(px(5.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .bg(move || {
                    let colors = pal_fn(&cx);
                    if selected.get() == index {
                        colors.card
                    } else {
                        transparent_black()
                    }
                })
                .on_press(move || active.set(index))
                .child(
                    text(*label)
                        .text_size(px(ts::SM))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(move || {
                            let colors = pal_fn(&cx);
                            if selected.get() == index {
                                colors.fg
                            } else {
                                colors.fg_muted
                            }
                        }),
                )
        }))
}

pub fn labeled_input(
    cx: Cx,
    label: &'static str,
    value: Signal<String>,
    placeholder: &'static str,
    multiline: bool,
) -> View {
    let focused = signal(false);
    view()
        .gap_1()
        .child(
            text(label)
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(
            input()
                .value(move || value.get())
                .on_change(move |text: &str| value.set(text.to_owned()))
                .placeholder(placeholder)
                .multiline(multiline)
                .text_size(px(ts::MD))
                .text_color(move || pal_fn(&cx).fg)
                .h(px(if multiline { 64.0 } else { 38.0 }))
                .px_3()
                .rounded(px(10.0))
                .bg(move || pal_fn(&cx).bg)
                .border_1()
                .border_color(move || {
                    let colors = pal_fn(&cx);
                    if focused.get() {
                        colors.accent
                    } else {
                        colors.border
                    }
                })
                .on_focus(move || focused.set(true))
                .on_blur(move || focused.set(false)),
        )
}

/// A small copy chip that writes to the clipboard and flashes ✓.
pub fn copy_chip(cx: Cx, get_text: impl Fn() -> String + 'static) -> View {
    let copied = signal(false);
    let copied_style = copied;
    view()
        .px_2()
        .py(px(2.0))
        .rounded(px(6.0))
        .bg(move || pal_fn(&cx).card_alt)
        .cursor_pointer()
        .active(|style| style.opacity(0.6))
        .on_press(move || {
            nsh_core::ui::ambient::set_clipboard(get_text());
            copied.set(true);
            spawn(async move {
                timer(std::time::Duration::from_millis(1200)).await;
                copied.set(false);
            });
        })
        .child(
            text(move || {
                if copied_style.get() {
                    "已复制"
                } else {
                    "复制"
                }
            })
            .text_size(px(ts::XS))
            .text_color(move || {
                let colors = pal_fn(&cx);
                if copied_style.get() {
                    colors.ok
                } else {
                    colors.fg_muted
                }
            }),
        )
}

pub fn error_text(cx: Cx, message: String) -> View {
    view().child(
        text(message)
            .text_size(px(ts::SM))
            .text_color(move || pal_fn(&cx).danger),
    )
}

pub fn divider(cx: Cx) -> View {
    view().h(px(1.0)).w_full().bg(move || pal_fn(&cx).border)
}

pub fn detail_row(
    cx: Cx,
    label: &'static str,
    value: impl Fn() -> String + 'static,
) -> View {
    view()
        .flex_row()
        .items_start()
        .gap_3()
        .py_2()
        .child(
            text(label)
                .text_size(px(ts::SM))
                .w(px(110.0))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(
            text(value)
                .text_size(px(ts::SM))
                .flex_1()
                .font_family(theme::mono_family),
        )
}

pub fn spinner(cx: Cx) -> View {
    view()
        .size(px(16.0))
        .items_center()
        .justify_center()
        .animate_spin()
        .child(
            text("◌")
                .text_size(px(ts::MD))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

use crate::theme::Palette;
use fiber_wallet_core::api::node::NodePhase;

// ----------------------------------------------------------------- qr ----

/// A QR code drawn on a canvas: re-recorded when the data or the theme
/// changes. The canvas has a two-module quiet zone on the card color.
pub fn qr_canvas(cx: Cx, data: impl Fn() -> String + 'static, size: f32) -> View {
    view().child(canvas(move || {
        let text = data();
        let colors = pal_fn(&cx);
        let mut painter = Painter::new()
            .fill_color(colors.card)
            .fill_rect(0.0_f32, 0.0_f32, size, size);
        if let Ok(qr) = fast_qr::QRBuilder::new(text).ecl(fast_qr::ECL::Q).build() {
            let modules = qr.size as f32;
            let quiet = 2.0_f32;
            let cell = size / (modules + quiet * 2.0);
            painter = painter.fill_color(colors.fg);
            for y in 0..qr.size {
                for x in 0..qr.size {
                    if qr[y][x].value() {
                        let left = (x as f32 + quiet) * cell;
                        let top = (y as f32 + quiet) * cell;
                        painter = painter.fill_rect(left, top, cell, cell);
                    }
                }
            }
        }
        painter
    }))
}
