//! 交易: Swap (simulated router until M3+), invoice receiving, invoice paying.

use std::time::Duration;

use fiber_wallet_core::api::wallet::WalletPayment;
use nsh_core::ui::prelude::*;

use crate::bridge::{WalletBridge, shannons_to_ckb};
use crate::theme::{self, type_scale as ts};
use crate::widgets::*;

/// Simulated mid-market rate until a real Fiber router exists (M3+).
const MOCK_RATE: f64 = 0.118_7;

pub fn trade_page(cx: Cx) -> View {
    view()
        .gap_5()
        .child(page_header(cx, "交易", "Swap 与发票收付款"))
        .child(segmented(cx, &["Swap", "收款", "付款"], cx.trade_tab))
        .child(dynamic(move || match cx.trade_tab.get() {
            0 => Some(view().child(swap_view(cx))),
            1 => Some(view().child(receive_view(cx))),
            _ => Some(view().child(send_view(cx))),
        }))
}

// ---------------------------------------------------------------- swap ----

#[derive(Clone, Copy, PartialEq)]
enum SwapState {
    Idle,
    Routing,
    Settling,
    Done,
}

fn swap_view(cx: Cx) -> View {
    let direction = signal(false); // false: CKB -> rUSDT
    let submitting = signal(SwapState::Idle);
    let error = signal::<Option<String>>(None);
    let amount = cx.swap_amount;
    // Simulated mid-market rate, jittered every few seconds to feel live
    // until a real Fiber router quotes in M3+. Cancelled when the Swap view
    // goes away (tab switch or app shutdown).
    let rate = signal(MOCK_RATE);
    {
        let ticker = spawn(async move {
            loop {
                timer(Duration::from_millis(5000)).await;
                let drift = (rand::random::<f64>() - 0.5) * 0.008;
                rate.set(MOCK_RATE * (1.0 + drift));
            }
        });
        cleanup(move || ticker.cancel());
    }

    let submit_state = submitting;
    let submit_error = error;
    let submit_amount = amount;
    let submit_direction = direction;

    view()
        .gap_4()
        .child(
            card(cx)
                .p_6()
                .gap_4()
                .max_w(px(520.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            text("Swap")
                                .text_size(px(ts::LG))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || pal_fn(&cx).fg),
                        )
                        .child(
                            view()
                                .px_2()
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .bg(move || pal_fn(&cx).warn.opacity(0.15))
                                .child(
                                    text("模拟报价 · M3 接入真实路由")
                                        .text_size(px(ts::XS))
                                        .text_color(move || pal_fn(&cx).warn),
                                ),
                        ),
                )
                .child(swap_amount_box(cx, amount, "卖出", "CKB", true, direction))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .child(
                            text("⇅")
                                .text_size(px(ts::LG))
                                .text_color(move || pal_fn(&cx).accent)
                                .cursor_pointer()
                                .on_press(move || direction.update(|flip| *flip = !*flip)),
                        )
                        .child(
                            text(move || {
                                format!("1 CKB ≈ {:.4} rUSDT", rate.get())
                            })
                            .text_size(px(ts::XS))
                            .font_family(theme::mono_family)
                            .text_color(move || pal_fn(&cx).fg_muted),
                        ),
                )
                .child(swap_amount_box(cx, amount, "买入", "rUSDT · 测试", false, direction))
                .child(rate_details(cx, amount, direction, rate))
                .child(dynamic(move || {
                    match submit_state.get() {
                        SwapState::Routing | SwapState::Settling => Some(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(spinner(cx))
                                .child(
                                    text(if submit_state.get() == SwapState::Routing {
                                        "正在寻找通道路由…"
                                    } else {
                                        "正在通过 Fiber 通道结算…"
                                    })
                                    .text_size(px(ts::SM))
                                    .text_color(move || pal_fn(&cx).fg_muted),
                                ),
                        ),
                        SwapState::Done => Some(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    text("✓")
                                        .text_size(px(ts::MD))
                                        .text_color(move || pal_fn(&cx).ok),
                                )
                                .child(
                                    text("Swap 完成（模拟）。真实结算将出现在支付记录中。")
                                        .text_size(px(ts::SM))
                                        .text_color(move || pal_fn(&cx).ok),
                                ),
                        ),
                        _ => None,
                    }
                }))
                .child(dynamic(move || {
                    submit_error
                        .get()
                        .map(|message| view().child(error_text(cx, message)))
                }))
                .child(primary_button(
                    cx,
                    "Swap",
                    move || {
                        let amount: f64 = submit_amount.get().trim().parse().unwrap_or(0.0);
                        submit_state.get() == SwapState::Idle && amount > 0.0
                    },
                    move || {
                        submit_error.set(None);
                        submit_state.set(SwapState::Routing);
                        let state = submit_state;
                        spawn(async move {
                            timer(Duration::from_millis(900)).await;
                            state.set(SwapState::Settling);
                            timer(Duration::from_millis(900)).await;
                            state.set(SwapState::Done);
                            toast("Swap 已完成（模拟结算）", ToastDuration::Short);
                            timer(Duration::from_millis(2500)).await;
                            state.set(SwapState::Idle);
                        });
                        let _ = submit_direction;
                    },
                )),
        )
        .child(
            text("Swap 走 Fiber 通道的多跳路由，结算在通道内即时完成。当前报价为模拟数据，真实路由在 M3+ 接入。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

fn swap_amount_box(
    cx: Cx,
    amount: Signal<String>,
    side: &'static str,
    token: &'static str,
    _is_input: bool,
    _direction: Signal<bool>,
) -> View {
    view()
        .gap_2()
        .p_4()
        .rounded(px(12.0))
        .bg(move || pal_fn(&cx).bg)
        .border_1()
        .border_color(move || pal_fn(&cx).border)
        .child(
            text(side)
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    input()
                        .value(move || amount.get())
                        .on_change(move |text: &str| amount.set(text.to_owned()))
                        .placeholder("0.00")
                        .text_size(px(ts::TITLE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || pal_fn(&cx).fg)
                        .flex_1()
                        .bg(rgba(0x00000000)),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py(px(4.0))
                        .rounded(px(8.0))
                        .bg(move || pal_fn(&cx).card_alt)
                        .child(
                            text(match token {
                                "CKB" => "🟢",
                                _ => "💵",
                            })
                            .text_size(px(ts::SM)),
                        )
                        .child(
                            text(token)
                                .text_size(px(ts::SM))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || pal_fn(&cx).fg),
                        ),
                ),
        )
}

fn rate_details(cx: Cx, amount: Signal<String>, direction: Signal<bool>, rate: Signal<f64>) -> View {
    view()
        .gap_2()
        .p_3()
        .rounded(px(10.0))
        .bg(move || pal_fn(&cx).bg)
        .child(detail_kv(cx, "汇率", {
            move || {
                if direction.get() {
                    format!("1 rUSDT ≈ {:.4} CKB", 1.0 / rate.get())
                } else {
                    format!("1 CKB ≈ {:.4} rUSDT", rate.get())
                }
            }
        }))
        .child(detail_kv(cx, "预计到手", {
            move || {
                let value: f64 = amount.get().trim().parse().unwrap_or(0.0);
                if direction.get() {
                    format!("{:.4} CKB", value / rate.get())
                } else {
                    format!("{:.4} rUSDT", value * rate.get())
                }
            }
        }))
        .child(detail_kv(cx, "路由费", || "0.3% · 通道内结算".to_owned()))
        .child(detail_kv(cx, "路由", || "直连通道 · 模拟".to_owned()))
}

fn detail_kv(cx: Cx, key: &'static str, value: impl Fn() -> String + 'static) -> View {
    view()
        .flex_row()
        .justify_between()
        .child(
            text(key)
                .text_size(px(ts::SM))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(
            text(value)
                .text_size(px(ts::SM))
                .font_family(theme::mono_family)
                .text_color(move || pal_fn(&cx).fg),
        )
}

// ------------------------------------------------------------- receive ----

fn receive_view(cx: Cx) -> View {
    let ui = crate::bridge::WalletBridge::global();
    let amount = cx.receive_amount;
    let description = cx.receive_description;
    let submit_amount = amount;
    let submit_description = description;

    // Track invoices created this session, newest first (node restarts drop
    // last_invoice, the history keeps them around).
    {
        let history = cx.invoice_history;
        let ui_signal = cx.ui;
        effect(move || {
            let fresh = ui_signal.get().last_invoice.clone();
            if let Some(invoice) = fresh {
                history.update(|items| {
                    if !items.iter().any(|item| item.payment_hash == invoice.payment_hash) {
                        items.insert(0, invoice);
                        items.truncate(10);
                    }
                });
            }
        });
    }

    view()
        .gap_4()
        .child(
            card(cx)
                .p_6()
                .gap_4()
                .max_w(px(520.0))
                .child(section_label(cx, "创建发票"))
                .child(labeled_input(cx, "金额 (CKB)", amount, "例如 1.5", false))
                .child(labeled_input(cx, "备注（可选）", description, "给付款人看", false))
                .child(primary_button(
                    cx,
                    "创建发票",
                    move || !ui.state().invoice_busy,
                    move || {
                        WalletBridge::global().create_invoice(
                            submit_amount.get(),
                            submit_description.get(),
                        )
                    },
                ))
                .child(dynamic(move || {
                    ui.state().invoice_busy.then(|| view().child(spinner(cx)))
                }))
                .child(dynamic(move || {
                    ui.state()
                        .invoice_error
                        .clone()
                        .map(|message| view().child(error_text(cx, message)))
                }))
                .child(dynamic(move || {
                    ui.state().last_invoice.clone().map(|invoice| {
                        view()
                            .gap_2()
                            .child(section_label(cx, "发票"))
                            .child(
                                view()
                                    .flex_row()
                                    .items_start()
                                    .gap_2()
                                    .p_3()
                                    .rounded(px(10.0))
                                    .bg(move || pal_fn(&cx).bg)
                                    .child(
                                        text(invoice.invoice.clone())
                                            .text_size(px(ts::XS))
                                            .font_family(theme::mono_family)
                                            .flex_1(),
                                    )
                                    .child(copy_chip(cx, {
                                        let invoice = invoice.invoice.clone();
                                        move || invoice.clone()
                                    })),
                            )
                            .child(
                                view()
                                    .items_center()
                                    .child(qr_canvas(cx, {
                                        let invoice = invoice.invoice.clone();
                                        move || invoice.clone()
                                    }, 180.0)),
                            )
                            .child(
                                text("扫描二维码向本节点支付此发票")
                                    .text_size(px(ts::XS))
                                    .text_color(move || pal_fn(&cx).fg_muted),
                            )
                            .child(
                                text(format!(
                                    "payment hash {} · {} CKB",
                                    invoice.payment_hash,
                                    shannons_to_ckb(&invoice.amount)
                                ))
                                .text_size(px(ts::XS))
                                .text_color(move || pal_fn(&cx).fg_muted),
                            )
                    })
                })),
        )
        .child(dynamic(move || {
            let history = cx.invoice_history.get();
            (!history.is_empty()).then(|| {
                view()
                    .gap_2()
                    .max_w(px(520.0))
                    .child(section_label(cx, "本次会话的发票"))
                    .children(history.iter().take(5).map(|invoice| {
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .py_1()
                            .child(
                                text(format!(
                                    "{} · {} CKB · {}",
                                    theme::short_id(&invoice.payment_hash, 8),
                                    shannons_to_ckb(&invoice.amount),
                                    invoice.description.clone().unwrap_or_else(|| "-".to_owned()),
                                ))
                                .text_size(px(ts::XS))
                                .font_family(theme::mono_family)
                                .text_color(move || pal_fn(&cx).fg_muted)
                                .flex_1(),
                            )
                            .child(copy_chip(cx, {
                                let invoice = invoice.invoice.clone();
                                move || invoice.clone()
                            }))
                    }))
            })
        }))
        .child(
            text("收款需要一条有本地余额的通道；先充值，再与 peers 建立通道。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

// ---------------------------------------------------------------- send ----

fn send_view(cx: Cx) -> View {
    let ui = WalletBridge::global();
    let invoice = cx.pay_invoice;
    let submit_invoice = invoice;

    view()
        .gap_4()
        .child(
            card(cx)
                .p_6()
                .gap_4()
                .max_w(px(560.0))
                .child(section_label(cx, "支付发票"))
                .child(labeled_input(
                    cx,
                    "粘贴发票",
                    invoice,
                    "fiber invoice 字符串",
                    true,
                ))
                .child(primary_button(
                    cx,
                    "发送支付",
                    move || !ui.state().send_busy && !submit_invoice.get().trim().is_empty(),
                    move || WalletBridge::global().send_payment(submit_invoice.get()),
                ))
                .child(dynamic(move || {
                    ui.state().send_busy.then(|| view().child(spinner(cx)))
                }))
                .child(dynamic(move || {
                    ui.state()
                        .send_result
                        .clone()
                        .map(|message| view().child(text(message).text_size(px(ts::SM))))
                }))
                .child(dynamic(move || {
                    ui.state()
                        .send_error
                        .clone()
                        .map(|message| view().child(error_text(cx, message)))
                })),
        )
        .child(
            card(cx)
                .p_6()
                .gap_3()
                .child(section_label(cx, "支付记录"))
                .child(dynamic(move || {
                    let state = ui.state();
                    if state.payments.is_empty() {
                        Some(
                            view().py_3().child(
                                text("暂无支付。")
                                    .text_size(px(ts::SM))
                                    .text_color(move || pal_fn(&cx).fg_muted),
                            ),
                        )
                    } else {
                        Some(view().gap_2().children(
                            state
                                .payments
                                .iter()
                                .map(|payment| payment_row(cx, payment)),
                        ))
                    }
                })),
        )
}

/// One payment record row, shared with the home page.
pub fn payment_row(cx: Cx, payment: &WalletPayment) -> View {
    let status_glyph = payment.status.clone();
    let status_color = payment.status.clone();
    view()
        .flex_row()
        .items_start()
        .gap_2()
        .pb_2()
        .child(
            text(move || match status_glyph.to_ascii_lowercase().as_str() {
                "success" => "✓",
                "failed" => "✕",
                _ => "⏳",
            })
            .text_size(px(ts::SM))
            .text_color(move || {
                let colors = pal_fn(&cx);
                match status_color.to_ascii_lowercase().as_str() {
                    "success" => colors.ok,
                    "failed" => colors.danger,
                    _ => colors.warn,
                }
            }),
        )
        .child(
            view().flex_1().child(
                text(format!(
                    "{}\n{}\n创建 {} · 更新 {}{}",
                    payment.status,
                    theme::short_id(&payment.payment_hash, 10),
                    crate::bridge::timestamp(payment.created_at_ms),
                    crate::bridge::timestamp(payment.last_updated_at_ms),
                    payment
                        .failed_error
                        .as_ref()
                        .map(|error| format!("\n{error}"))
                        .unwrap_or_default(),
                ))
                .text_size(px(ts::XS))
                .font_family(theme::mono_family)
                .text_color(move || pal_fn(&cx).fg_muted),
            ),
        )
}
