//! 首页: balances, quick actions, recent activity.

use nsh_core::ui::prelude::*;

use crate::theme::{self, palette, type_scale as ts};
use crate::widgets::*;

fn sum_shannons(values: &[String]) -> String {
    values
        .iter()
        .filter_map(|value| value.parse::<u128>().ok())
        .sum::<u128>()
        .to_string()
}

pub fn home_page(cx: Cx) -> View {
    view()
        .gap_5()
        .child(page_header(
            cx,
            "首页",
            "本地优先的 Fiber 钱包 · 节点随应用运行",
        ))
        .child(hero_balance(cx))
        .child(stat_row(cx))
        .child(quick_actions(cx))
        .child(activity_row(cx))
        .child(
            text("节点仅在本应用前台运行；退出应用即停止。收款需要一条有本地余额的通道。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

fn hero_balance(cx: Cx) -> View {
    view()
        .gap_2()
        .p_6()
        .rounded(px(14.0))
        .bg(move || pal_fn(&cx).card)
        .border_1()
        .border_color(move || pal_fn(&cx).border)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    view()
                        .size(px(40.0))
                        .items_center()
                        .justify_center()
                        .rounded(px(12.0))
                        .bg_linear(deg(135.0))
                        .gradient_from(move || pal_fn(&cx).gradient_from)
                        .gradient_to(move || pal_fn(&cx).gradient_to)
                        .child(text("🐋").text_size(px(20.0))),
                )
                .child(
                    text("总资产")
                        .text_size(px(ts::SM))
                        .text_color(move || pal_fn(&cx).fg_muted),
                ),
        )
        .child({
            // Desktop: balance left, QR right. A phone stacks them: at phone
            // width the hero number would wrap mid-digit beside the 104 pt QR.
            #[cfg(target_os = "ios")]
            let metrics = view().gap_5();
            #[cfg(not(target_os = "ios"))]
            let metrics = view().flex_row().items_center().gap_5();
            metrics
                // Texts refresh with the wallet state…
                .child(dynamic(move || {
                    let state = cx.ui_state();
                    let colors = palette(&cx.config.get());
                    let has_info = state.node_info.is_some();
                    let total = has_info.then(|| {
                        let info = state.node_info.as_ref().expect("checked");
                        sum_shannons(&[
                            info.funding_balance_shannons.clone(),
                            info.channel_balance_shannons.clone(),
                        ])
                    });
                    let mut hero = view().gap_1().min_w(px(0.0)).child(
                        text(match &total {
                            Some(shannons) => {
                                format!("{} CKB", theme::ckb_display(shannons))
                            }
                            None => "— CKB".to_owned(),
                        })
                        .text_size(px(ts::HERO))
                        .font_weight(FontWeight::BOLD)
                        .font_family(theme::mono_family)
                        .text_color(colors.fg),
                    );
                    if !cfg!(target_os = "ios") {
                        // Only the desktop row needs the grow; in a stacked
                        // column it would be meaningless.
                        hero = hero.flex_1();
                    }
                    hero = hero.child(
                        view().flex_row().items_center().gap_2().child(
                            text(match state.node_info.as_ref() {
                                Some(info) => theme::short_id(&info.funding_address, 10),
                                None => "启动节点后加载收款地址".to_owned(),
                            })
                            .text_size(px(ts::SM))
                            .font_family(theme::mono_family)
                            .text_color(colors.fg_muted),
                        ),
                    );
                    hero.when_some(
                        state
                            .node_info
                            .as_ref()
                            .map(|info| info.funding_address.clone()),
                        |hero, address| {
                            hero.child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(copy_chip(cx, move || address.clone())),
                            )
                        },
                    )
                }))
                // …but the canvas itself must live outside every re-running
                // region: a canvas rebuilt with its region never paints. The
                // reactive `when` keeps one instance while the node info is
                // up; its paint closure re-reads the address on changes.
                .child(when(
                    move || cx.ui_state().node_info.is_some(),
                    move || {
                        view()
                            .items_center()
                            .gap_1()
                            .child(qr_canvas(
                                cx,
                                move || {
                                    cx.ui_state()
                                        .node_info
                                        .map(|info| info.funding_address.clone())
                                        .unwrap_or_default()
                                },
                                104.0,
                            ))
                            .child(
                                text("收款地址")
                                    .text_size(px(ts::XS))
                                    .text_color(move || pal_fn(&cx).fg_muted),
                            )
                    },
                ))
        })
}

fn stat_card(cx: Cx, label: &'static str, value: impl Fn() -> String + 'static) -> View {
    view()
        .flex_1()
        .gap_1()
        .p_4()
        .rounded(px(12.0))
        .bg(move || pal_fn(&cx).card)
        .border_1()
        .border_color(move || pal_fn(&cx).border)
        .child(
            text(label)
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(
            text(value)
                .text_size(px(ts::XL))
                .font_weight(FontWeight::SEMIBOLD)
                .font_family(theme::mono_family)
                .text_color(move || pal_fn(&cx).fg),
        )
}

fn stat_row(cx: Cx) -> View {
    view()
        .flex_row()
        .gap_4()
        .child(stat_card(cx, "链上余额", {
            move || {
                cx.ui_state()
                    .node_info
                    .map(|info| {
                        format!("{} CKB", theme::ckb_display(&info.funding_balance_shannons))
                    })
                    .unwrap_or_else(|| "—".to_owned())
            }
        }))
        .child(stat_card(cx, "通道余额", {
            move || {
                cx.ui_state()
                    .node_info
                    .map(|info| {
                        format!("{} CKB", theme::ckb_display(&info.channel_balance_shannons))
                    })
                    .unwrap_or_else(|| "—".to_owned())
            }
        }))
        .child(stat_card(cx, "通道数", {
            move || {
                let state = cx.ui_state();
                if state.snapshot.map(|s| s.phase) == Some(NodePhase::Running) {
                    format!("{}", state.channels.len())
                } else {
                    "—".to_owned()
                }
            }
        }))
}

fn quick_actions(cx: Cx) -> View {
    let cx_home = cx;
    let cx_pay = cx;
    let cx_transfer = cx;
    view()
        .flex_row()
        .gap_3()
        .child(
            primary_button(
                cx,
                "收款",
                || true,
                move || {
                    cx_home.nav.set(Nav::Trade);
                    cx_home.trade_tab.set(2);
                },
            )
            .flex_1(),
        )
        .child(
            primary_button(
                cx,
                "付款",
                || true,
                move || {
                    cx_pay.nav.set(Nav::Trade);
                    cx_pay.trade_tab.set(3);
                },
            )
            .flex_1(),
        )
        .child(
            outline_button(
                cx,
                "链上转账",
                || true,
                move || {
                    cx_transfer.nav.set(Nav::Trade);
                    cx_transfer.trade_tab.set(1);
                },
            )
            .flex_1(),
        )
}

fn activity_row(cx: Cx) -> View {
    // Desktop: the two cards share one row. A phone stacks them — two
    // half-width cards squeeze the log console (and its monospace lines)
    // past the screen edge.
    #[cfg(target_os = "ios")]
    let row = view().gap_4();
    #[cfg(not(target_os = "ios"))]
    let row = view().flex_row().items_start().gap_4();
    row.child(recent_payments_card(cx)).child(node_log_card(cx))
}

fn recent_payments_card(cx: Cx) -> View {
    let card = card(cx).overflow_hidden().p_5().gap_3();
    #[cfg(not(target_os = "ios"))]
    // flex_1 (`1 1 0`) shares the desktop row exactly; long content pushed
    // the row past the window edge, and flex_grow alone never shrinks.
    let card = card.flex_1().min_w(px(0.0));
    card.child(section_label(cx, "最近支付"))
        .child(dynamic(move || {
            let state = cx.ui_state();
            if state.payments.is_empty() {
                Some(
                    view().py_3().child(
                        text(
                            if state.snapshot.map(|s| s.phase) == Some(NodePhase::Running) {
                                "暂无支付记录。"
                            } else {
                                "节点离线，启动后显示。"
                            },
                        )
                        .text_size(px(ts::SM))
                        .text_color(move || pal_fn(&cx).fg_muted),
                    ),
                )
            } else {
                Some(
                    view().gap_2().children(
                        state
                            .payments
                            .iter()
                            .take(5)
                            .map(|payment| crate::pages::trade::payment_row(cx, payment)),
                    ),
                )
            }
        }))
}

fn node_log_card(cx: Cx) -> View {
    let card = card(cx).overflow_hidden().p_5().gap_3();
    #[cfg(not(target_os = "ios"))]
    let card = card.flex_1().min_w(px(0.0));
    card.child(section_label(cx, "节点事件 · 实时日志"))
        .child(dynamic(move || {
            let state = cx.ui_state();
            // The native binary installs a log capture layer; until
            // its first lines arrive (or on hosts without it) fall
            // back to the phase-transition events.
            let mut lines: Vec<String> = if state.log_tail.is_empty() {
                state.events.iter().take(6).cloned().collect()
            } else {
                let start = state.log_tail.len().saturating_sub(9);
                state.log_tail[start..].to_vec()
            };
            // Newest first: new lines always appear at the top of the
            // card instead of pushing the view down.
            lines.reverse();
            Some(
                view()
                    .gap_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .p_3()
                    .rounded(px(8.0))
                    .bg(move || pal_fn(&cx).bg)
                    .border_1()
                    .border_color(move || pal_fn(&cx).border)
                    .children(lines.into_iter().map(|line| {
                        let colors = pal_fn(&cx);
                        let color = if line.contains(" ERROR ") {
                            colors.danger
                        } else if line.contains(" WARN ") {
                            colors.warn
                        } else {
                            colors.fg_muted
                        };
                        view().child(
                            text(line)
                                .text_size(px(ts::XS))
                                .font_family(theme::mono_family)
                                .text_color(color),
                        )
                    })),
            )
        }))
}

use fiber_wallet_core::api::node::NodePhase;
