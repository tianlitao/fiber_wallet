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
        .child(page_header(cx, "首页", "本地优先的 Fiber 钱包 · 节点随应用运行"))
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
    let ui = crate::bridge::WalletBridge::global();
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
        .child(dynamic(move || {
            let state = ui.state();
            let colors = palette(&cx.config.get());
            let has_info = state.node_info.is_some();
            let total = has_info.then(|| {
                let info = state.node_info.as_ref().expect("checked");
                sum_shannons(&[
                    info.funding_balance_shannons.clone(),
                    info.channel_balance_shannons.clone(),
                ])
            });
            let funding_address = state
                .node_info
                .as_ref()
                .map(|info| info.funding_address.clone());
            let mut hero = view().flex_1().gap_1().child(
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
            let mut address_row = view().flex_row().items_center().gap_2().child(
                text(match state.node_info.as_ref() {
                    Some(info) => theme::short_id(&info.funding_address, 10),
                    None => "启动节点后加载收款地址".to_owned(),
                })
                .text_size(px(ts::SM))
                .font_family(theme::mono_family)
                .text_color(colors.fg_muted),
            );
            address_row = address_row.when_some(
                state.node_info.as_ref().map(|info| info.funding_address.clone()),
                |row, address| row.child(copy_chip(cx, move || address.clone())),
            );
            hero = hero.child(address_row);

            // 收款地址二维码 on the right of the hero once the node is up.
            view()
                .flex_row()
                .items_center()
                .gap_5()
                .child(hero)
                .child(match &funding_address {
                    Some(address) => view()
                        .items_center()
                        .gap_1()
                        .child(qr_canvas(cx, {
                            let address = address.clone();
                            move || address.clone()
                        }, 104.0))
                        .child(
                            text("收款地址")
                                .text_size(px(ts::XS))
                                .text_color(colors.fg_muted),
                        ),
                    None => view(),
                })
        }))
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
    let ui = crate::bridge::WalletBridge::global();
    view()
        .flex_row()
        .gap_4()
        .child(stat_card(cx, "链上余额", {
            move || {
                ui.state()
                    .node_info
                    .map(|info| format!("{} CKB", theme::ckb_display(&info.funding_balance_shannons)))
                    .unwrap_or_else(|| "—".to_owned())
            }
        }))
        .child(stat_card(cx, "通道余额", {
            move || {
                ui.state()
                    .node_info
                    .map(|info| format!("{} CKB", theme::ckb_display(&info.channel_balance_shannons)))
                    .unwrap_or_else(|| "—".to_owned())
            }
        }))
        .child(stat_card(cx, "通道数", {
            move || {
                let state = ui.state();
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
    let cx_swap = cx;
    view()
        .flex_row()
        .gap_3()
        .child(
            primary_button(cx, "收款", || true, move || {
                cx_home.nav.set(Nav::Trade);
                cx_home.trade_tab.set(1);
            })
            .flex_1(),
        )
        .child(
            primary_button(cx, "付款", || true, move || {
                cx_pay.nav.set(Nav::Trade);
                cx_pay.trade_tab.set(2);
            })
            .flex_1(),
        )
        .child(
            outline_button(cx, "去 Swap", || true, move || {
                cx_swap.nav.set(Nav::Trade);
                cx_swap.trade_tab.set(0);
            })
            .flex_1(),
        )
}

fn activity_row(cx: Cx) -> View {
    let ui = crate::bridge::WalletBridge::global();
    let ui_events = ui;
    view()
        .flex_row()
        .items_start()
        .gap_4()
        .child(
            card(cx)
                .flex_grow(1.2)
                .p_5()
                .gap_3()
                .child(section_label(cx, "最近支付"))
                .child(dynamic(move || {
                    let state = ui_events.state();
                    if state.payments.is_empty() {
                        Some(
                            view().py_3().child(
                                text(if state.snapshot.map(|s| s.phase) == Some(NodePhase::Running) {
                                    "暂无支付记录。"
                                } else {
                                    "节点离线，启动后显示。"
                                })
                                .text_size(px(ts::SM))
                                .text_color(move || pal_fn(&cx).fg_muted),
                            ),
                        )
                    } else {
                        Some(view().gap_2().children(
                            state
                                .payments
                                .iter()
                                .take(5)
                                .map(|payment| crate::pages::trade::payment_row(cx, payment)),
                        ))
                    }
                })),
        )
        .child(
            card(cx)
                .flex_grow(1.0)
                .p_5()
                .gap_3()
                .child(section_label(cx, "节点事件"))
                .child(dynamic(move || {
                    let state = ui.state();
                    if state.events.is_empty() {
                        Some(
                            view().py_3().child(
                                text("暂无事件。")
                                    .text_size(px(ts::SM))
                                    .text_color(move || pal_fn(&cx).fg_muted),
                            ),
                        )
                    } else {
                        Some(view().gap_2().children(
                            state
                                .events
                                .iter()
                                .take(5)
                                .map(|event| {
                                    text(event.clone())
                                        .text_size(px(ts::XS))
                                        .font_family(theme::mono_family)
                                        .text_color(move || pal_fn(&cx).fg_muted)
                                })
                                .map(|element| view().child(element)),
                        ))
                    }
                })),
        )
}

use fiber_wallet_core::api::node::NodePhase;
