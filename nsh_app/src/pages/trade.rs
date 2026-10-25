//! 交易: real Fiber native-CKB keysend, invoice receiving/paying, and channels.
//!
//! Cross-asset CKB/rUSDT exchange is intentionally unavailable: this wallet
//! has no AMM, order book, or conversion provider, and Fiber v0.9.1 payments
//! require one asset across the route.

use fiber_wallet_core::api::node::NodePhase;
use fiber_wallet_core::api::wallet::{
    ChannelCloseMode, ChannelCloseStatus, OnchainTxState, WalletChannel, WalletPayment,
    WalletPaymentQuote, WalletPaymentStatus,
};
use nsh_core::ui::prelude::*;

use crate::bridge::{
    ChannelCloseAttempt, OnchainPhase, OnchainState, PaymentPhase, PaymentState, WalletBridge,
    shannons_to_ckb,
};
use crate::channel_close::{self, CloseActionKind};
use crate::theme::{self, type_scale as ts};
use crate::widgets::*;

pub fn trade_page(cx: Cx) -> View {
    view()
        .gap_5()
        .child(page_header(
            cx,
            "交易",
            "Fiber 同资产转账、发票收付款与通道",
        ))
        .child(segmented(
            cx,
            &["Fiber 转账", "链上 CKB", "收款", "付款", "通道"],
            cx.trade_tab,
        ))
        .child(dynamic(move || match cx.trade_tab.get() {
            0 => Some(view().child(transfer_view(cx))),
            1 => Some(view().child(onchain_view(cx))),
            2 => Some(view().child(receive_view(cx))),
            3 => Some(view().child(send_view(cx))),
            _ => Some(view().child(channels_view(cx))),
        }))
}

// ------------------------------------------------------ same-asset transfer ----

fn transfer_view(cx: Cx) -> View {
    let target = cx.transfer_target;
    let amount = cx.transfer_amount;
    let max_fee = cx.transfer_max_fee;

    // A dry-run quote is bound to the exact target/amount/fee cap used to
    // create it. Revalidate the form against bridge updates so a quote cannot
    // stay confirmable under different visible inputs.
    {
        let ui_signal = cx.ui;
        effect(move || {
            let state = ui_signal.get().transfer;
            if state.phase == PaymentPhase::Quoted {
                WalletBridge::global().invalidate_transfer_quote(
                    &target.get(),
                    &amount.get(),
                    &max_fee.get(),
                );
            }
        });
    }

    view()
        .gap_4()
        .child(
            card(cx)
                .p_6()
                .gap_4()
                .max_w(px(620.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            view()
                                .gap_1()
                                .child(
                                    text("Fiber 同资产转账")
                                        .text_size(px(ts::LG))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(move || pal_fn(&cx).fg),
                                )
                                .child(
                                    text("CKB → CKB · keysend · Fiber v0.9.1")
                                        .text_size(px(ts::XS))
                                        .text_color(move || pal_fn(&cx).fg_muted),
                                ),
                        )
                        .child(
                            view()
                                .px_2()
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .bg(move || pal_fn(&cx).accent_soft)
                                .child(
                                    text("真实 Fiber 路由")
                                        .text_size(px(ts::XS))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(move || pal_fn(&cx).accent),
                                ),
                        ),
                )
                .child(labeled_input(
                    cx,
                    "目标节点 pubkey（33 字节压缩公钥）",
                    target,
                    "02… 或 03…",
                    false,
                ))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_3()
                        .child(
                            labeled_input(cx, "支付金额 (CKB)", amount, "例如 1.5", false)
                                .flex_1(),
                        )
                        .child(
                            labeled_input(cx, "最大路由费 (CKB)", max_fee, "例如 0.01", false)
                                .w(px(200.0)),
                        ),
                )
                .child(transfer_status(cx))
                .child(transfer_actions(cx, target, amount, max_fee)),
        )
        .child(cross_asset_unavailable(cx))
        .child(
            text("这是同一资产 CKB 的 Fiber keysend，不是 CKB↔rUSDT 兑换。dry-run 只检查路由与费用，不发送 HTLC；确认后才发送真实支付。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

fn transfer_actions(
    cx: Cx,
    target: Signal<String>,
    amount: Signal<String>,
    max_fee: Signal<String>,
) -> AnyElement {
    dynamic(move || {
        let state = cx.ui_state().transfer;
        match state.phase {
            PaymentPhase::Idle => Some(
                primary_button(
                    cx,
                    "Dry-run 获取真实报价",
                    move || {
                        cx.ui_state().snapshot.map(|snapshot| snapshot.phase)
                            == Some(NodePhase::Running)
                            && !target.get().trim().is_empty()
                            && !amount.get().trim().is_empty()
                            && !max_fee.get().trim().is_empty()
                    },
                    move || {
                        WalletBridge::global().quote_transfer(
                            target.get(),
                            amount.get(),
                            max_fee.get(),
                        )
                    },
                ),
            ),
            PaymentPhase::Quoted => Some(
                view()
                    .gap_3()
                    .child(
                        card(cx)
                            .p_4()
                            .gap_2()
                            .border_color(move || pal_fn(&cx).warn)
                            .child(section_label(cx, "确认发送真实支付"))
                            .child(
                                text("确认会使用上方 dry-run 绑定的 request 发送真实 keysend。报价不是付款，也不会自动发送。")
                                    .text_size(px(ts::SM))
                                    .text_color(move || pal_fn(&cx).warn),
                            )
                            .child(
                                text("确认后将锁定 target、金额、最大路由费和 60 秒超时；表单之后的变化不会再影响本次发送。")
                                    .text_size(px(ts::XS))
                                    .text_color(move || pal_fn(&cx).fg_muted),
                            ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .gap_2()
                            .child(outline_button(
                                cx,
                                "取消报价",
                                || true,
                                || WalletBridge::global().clear_transfer(),
                            ))
                            .child(primary_button(
                                cx,
                                "确认并发送真实支付",
                                || true,
                                || WalletBridge::global().confirm_transfer(),
                            )),
                    ),
            ),
            PaymentPhase::Quoting | PaymentPhase::Submitting => None,
            PaymentPhase::Success | PaymentPhase::Failed | PaymentPhase::Unknown => Some(
                view()
                    .flex_row()
                    .gap_2()
                    .child(primary_button(
                        cx,
                        "重新 Dry-run",
                        || true,
                        move || {
                            WalletBridge::global().clear_transfer();
                            WalletBridge::global().quote_transfer(
                                target.get(),
                                amount.get(),
                                max_fee.get(),
                            );
                        },
                    ))
                    .child(outline_button(
                        cx,
                        "清除结果",
                        || true,
                        || WalletBridge::global().clear_transfer(),
                    )),
            ),
        }
    })
}

fn transfer_status(cx: Cx) -> AnyElement {
    dynamic(move || {
        let state = cx.ui_state().transfer;
        match state.phase {
            PaymentPhase::Idle => None,
            PaymentPhase::Quoting => Some(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(spinner(cx))
                    .child(
                        text("正在通过 Fiber dry-run 检查同资产路由并计算费用…")
                            .text_size(px(ts::SM))
                            .text_color(move || pal_fn(&cx).fg_muted),
                    ),
            ),
            PaymentPhase::Quoted => state.quote.map(|quote| quote_card(cx, &quote)),
            PaymentPhase::Submitting => Some(submitting_card(cx)),
            PaymentPhase::Success => state
                .payment
                .map(|payment| success_card(cx, &payment, state.quote.as_ref())),
            PaymentPhase::Failed => Some(failure_card(cx, &state)),
            PaymentPhase::Unknown => Some(unknown_card(cx, &state)),
        }
    })
}

fn quote_card(cx: Cx, quote: &WalletPaymentQuote) -> View {
    let amount = shannons_to_ckb(&quote.amount_shannons);
    let fee = shannons_to_ckb(&quote.fee_shannons);
    let target = quote
        .request
        .target_pubkey
        .as_deref()
        .map(|target| theme::short_id(target, 16))
        .unwrap_or_else(|| "未返回".to_owned());
    let max_fee = quote
        .request
        .max_fee_amount
        .map(|value| format!("{} CKB", shannons_to_ckb(&value.to_string())))
        .unwrap_or_else(|| "未设置".to_owned());
    let quote_id = theme::short_id(&quote.quote_id, 16);
    let created_at = crate::bridge::timestamp(quote.created_at_ms);

    view()
        .gap_2()
        .p_3()
        .rounded(px(10.0))
        .bg(move || pal_fn(&cx).bg)
        .border_1()
        .border_color(move || pal_fn(&cx).accent.opacity(0.35))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text("路由可用 · 已取得 dry-run 报价")
                        .text_size(px(ts::SM))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || pal_fn(&cx).ok),
                )
                .child(
                    text("尚未发送")
                        .text_size(px(ts::XS))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || pal_fn(&cx).warn),
                ),
        )
        .child(detail_kv(cx, "目标节点", move || target.clone()))
        .child(detail_kv(cx, "支付金额", move || {
            format!("{amount} CKB")
        }))
        .child(detail_kv(cx, "预计路由费", move || {
            format!("{fee} CKB")
        }))
        .child(detail_kv(cx, "最大路由费", move || max_fee.clone()))
        .child(detail_kv(cx, "quote id", move || quote_id.clone()))
        .child(detail_kv(cx, "报价时间", move || created_at.clone()))
}

fn submitting_card(cx: Cx) -> View {
    view()
        .gap_2()
        .p_3()
        .rounded(px(10.0))
        .bg(move || pal_fn(&cx).bg)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(spinner(cx))
                .child(
                    text("正在发送真实 keysend，并等待 Fiber 报告 Success/Failed…")
                        .text_size(px(ts::SM))
                        .text_color(move || pal_fn(&cx).fg_muted),
                ),
        )
        .child(dynamic(move || {
            cx.ui_state().transfer.payment.map(|payment| {
                text(format!(
                    "Fiber {} · {} · 当前费用 {} CKB",
                    payment.status,
                    theme::short_id(&payment.payment_hash, 12),
                    shannons_to_ckb(&payment.fee_shannons)
                ))
                .text_size(px(ts::XS))
                .font_family(theme::mono_family)
                .text_color(move || pal_fn(&cx).fg_muted)
            })
        }))
}

fn success_card(cx: Cx, payment: &WalletPayment, quote: Option<&WalletPaymentQuote>) -> View {
    let target = quote
        .and_then(|quote| quote.request.target_pubkey.as_deref())
        .map(|target| theme::short_id(target, 16))
        .unwrap_or_else(|| "目标节点".to_owned());
    let amount = quote
        .map(|quote| format!("{} CKB", shannons_to_ckb(&quote.amount_shannons)))
        .unwrap_or_else(|| "金额见支付记录".to_owned());
    let hash = payment.payment_hash.clone();
    let fee = shannons_to_ckb(&payment.fee_shannons);

    view()
        .gap_2()
        .p_3()
        .rounded(px(10.0))
        .bg(move || pal_fn(&cx).ok.opacity(0.10))
        .child(
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
                    text("Fiber 报告支付成功")
                        .text_size(px(ts::SM))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || pal_fn(&cx).ok),
                ),
        )
        .child(
            text(format!(
                "{amount} → {target}\n实际费用 {fee} CKB\npayment hash {hash}"
            ))
            .text_size(px(ts::XS))
            .font_family(theme::mono_family)
            .text_color(move || pal_fn(&cx).fg_muted),
        )
}

fn failure_card(cx: Cx, state: &PaymentState) -> View {
    let paid = state.payment.is_some();
    let title = if paid {
        "Fiber 报告支付失败"
    } else {
        "报价或提交未完成"
    };
    let detail = state.error.clone().unwrap_or_else(|| {
        if paid {
            "Fiber 未返回失败详情".to_owned()
        } else {
            "未创建支付记录；调整参数后重新 dry-run。".to_owned()
        }
    });
    let payment_line = state.payment.as_ref().map(|payment| {
        format!(
            "payment hash {} · Fiber {}",
            payment.payment_hash, payment.status
        )
    });

    view()
        .gap_2()
        .p_3()
        .rounded(px(10.0))
        .bg(move || pal_fn(&cx).danger.opacity(0.10))
        .child(
            text(title)
                .text_size(px(ts::SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || pal_fn(&cx).danger),
        )
        .child(
            text(detail)
                .text_size(px(ts::SM))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .when_some(payment_line, |card, line| {
            card.child(
                text(line)
                    .text_size(px(ts::XS))
                    .font_family(theme::mono_family)
                    .text_color(move || pal_fn(&cx).fg_muted),
            )
        })
        .child(
            text(if paid {
                "Fiber 已给出终态 Failed；这不是超时或未知状态。"
            } else {
                "Fiber 未确认一笔真实支付失败；这里只表示本次报价或发送未完成。"
            })
            .text_size(px(ts::XS))
            .text_color(move || pal_fn(&cx).fg_muted),
        )
}

fn unknown_card(cx: Cx, state: &PaymentState) -> View {
    let detail = state
        .error
        .clone()
        .unwrap_or_else(|| "未取得 Fiber 终态。".to_owned());
    let payment_line = state.payment.as_ref().map(|payment| {
        format!(
            "payment hash {} · 当前 Fiber {}",
            payment.payment_hash, payment.status
        )
    });

    view()
        .gap_2()
        .p_3()
        .rounded(px(10.0))
        .bg(move || pal_fn(&cx).warn.opacity(0.12))
        .child(
            text("未确认：可能已发送，也可能仍在结算")
                .text_size(px(ts::SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || pal_fn(&cx).warn),
        )
        .child(
            text(detail)
                .text_size(px(ts::SM))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .when_some(payment_line, |card, line| {
            card.child(
                text(line)
                    .text_size(px(ts::XS))
                    .font_family(theme::mono_family)
                    .text_color(move || pal_fn(&cx).fg_muted),
            )
        })
        .child(
            text("这不是失败证明，也不是成功。请刷新支付记录或稍后继续核对；不要仅凭此页面重复发送。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

fn cross_asset_unavailable(cx: Cx) -> View {
    card(cx)
        .p_6()
        .gap_3()
        .max_w(px(620.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text("CKB ↔ rUSDT 跨资产兑换")
                        .text_size(px(ts::LG))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || pal_fn(&cx).fg),
                )
                .child(
                    view()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(move || pal_fn(&cx).danger.opacity(0.12))
                        .child(
                            text("不可用")
                                .text_size(px(ts::XS))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || pal_fn(&cx).danger),
                        ),
                ),
        )
        .child(
            text("当前没有接入 AMM、订单簿或兑换 provider。Fiber v0.9.1 的普通单笔支付要求整条路由使用同一资产，因此不能靠 keysend 完成 CKB↔rUSDT 兑换。")
                .text_size(px(ts::SM))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(
            text("只有外部适配器能够构建、签名并提交真实兑换交易时才可启用；本页不会展示模拟汇率或伪造成交。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

// ------------------------------------------------------- on-chain CKB ----

fn onchain_view(cx: Cx) -> View {
    let address = cx.onchain_address;
    let amount = cx.onchain_amount;
    let max_fee = cx.onchain_max_fee;
    let confirming = cx.onchain_confirming;
    let validation_error = cx.onchain_validation_error;
    let submit_address = address;
    let submit_amount = amount;
    let submit_fee = max_fee;
    let form_error = validation_error;

    view()
        .gap_4()
        .child(
            card(cx)
                .p_6()
                .gap_4()
                .max_w(px(620.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            view()
                                .gap_1()
                                .child(
                                    text("链上 CKB 转账")
                                        .text_size(px(ts::LG))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(move || pal_fn(&cx).fg),
                                )
                                .child(
                                    text("CKB Testnet · 原生 cell 转账 · 非 Fiber 通道支付")
                                        .text_size(px(ts::XS))
                                        .text_color(move || pal_fn(&cx).fg_muted),
                                ),
                        )
                        .child(
                            view()
                                .px_2()
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .bg(move || pal_fn(&cx).warn.opacity(0.15))
                                .child(
                                    text("仅 Testnet")
                                        .text_size(px(ts::XS))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(move || pal_fn(&cx).warn),
                                ),
                        ),
                )
                .child(labeled_input(
                    cx,
                    "接收地址（ckt1…）",
                    address,
                    "ckt1…",
                    false,
                ))
                .child(scan_button(cx, crate::scan::ScanTarget::OnchainAddress))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_3()
                        .child(
                            labeled_input(cx, "转账金额 (CKB)", amount, "例如 10", false)
                                .flex_1(),
                        )
                        .child(
                            labeled_input(
                                cx,
                                "手续费上限 (CKB)",
                                max_fee,
                                "例如 0.001",
                                false,
                            )
                            .w(px(200.0)),
                        ),
                )
                .child(dynamic(move || {
                    form_error
                        .get()
                        .map(|message| view().child(error_text(cx, message)))
                }))
                .child(onchain_confirmation(
                    cx,
                    confirming,
                    submit_address,
                    submit_amount,
                    submit_fee,
                    form_error,
                ))
                .child(onchain_result(cx)),
        )
        .child(
            text("签名与广播使用本地 Fiber funding key；RPC 固定为 CKB Testnet。手续费在构建时按 1000 shannons/KW 估算，并在签名后、广播前再次校验上限。广播成功只代表节点接受了交易，不等于已上链确认。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

fn onchain_confirmation(
    cx: Cx,
    confirming: Signal<bool>,
    address: Signal<String>,
    amount: Signal<String>,
    max_fee: Signal<String>,
    validation_error: Signal<Option<String>>,
) -> AnyElement {
    dynamic(move || {
        let state = cx.ui_state().onchain;
        if state.phase == OnchainPhase::Confirming {
            let target = address.get();
            let amount_value = amount.get();
            let fee_value = max_fee.get();
            return Some(
                card(cx)
                    .p_4()
                    .gap_2()
                    .border_color(move || pal_fn(&cx).warn)
                    .child(section_label(cx, "二次确认"))
                    .child(
                        text(format!(
                            "请确认以下参数后再签名广播：\n接收地址：{target}\n金额：{amount_value} CKB\n手续费上限：{fee_value} CKB\n网络：CKB Testnet"
                        ))
                        .text_size(px(ts::SM))
                        .font_family(theme::mono_family)
                        .text_color(move || pal_fn(&cx).fg),
                    )
                    .child(
                        text("链上交易不可撤销。广播结果不确定时不得直接重试；先按交易哈希查询状态。")
                            .text_size(px(ts::XS))
                            .text_color(move || pal_fn(&cx).danger),
                    )
                    .child(
                        view()
                            .flex_row()
                            .gap_2()
                            .child(outline_button(
                                cx,
                                "取消",
                                || true,
                                move || {
                                    WalletBridge::global().cancel_onchain_transfer();
                                    confirming.set(false);
                                },
                            ))
                            .child(primary_button(
                                cx,
                                "确认并签名广播",
                                || true,
                                move || {
                                    WalletBridge::global().confirm_onchain_transfer(
                                        target.clone(),
                                        amount_value.clone(),
                                        fee_value.clone(),
                                    );
                                    confirming.set(false);
                                },
                            )),
                    ),
            );
        }

        if matches!(
            state.phase,
            OnchainPhase::Broadcasting
                | OnchainPhase::Broadcast
                | OnchainPhase::Uncertain
                | OnchainPhase::Committed
                | OnchainPhase::Rejected
                | OnchainPhase::Unknown
        ) {
            return None;
        }

        let retrying = state.phase == OnchainPhase::Failed;
        let node_online =
            cx.ui_state().snapshot.map(|snapshot| snapshot.phase) == Some(NodePhase::Running);
        Some(
            view()
                .gap_2()
                .child(primary_button(
                    cx,
                    if retrying {
                        "重新检查并进入二次确认"
                    } else {
                        "检查并进入二次确认"
                    },
                    move || {
                        node_online
                            && !address.get().trim().is_empty()
                            && !amount.get().trim().is_empty()
                            && !max_fee.get().trim().is_empty()
                    },
                    move || {
                        validation_error.set(None);
                        WalletBridge::global().prepare_onchain_transfer(
                            address.get(),
                            amount.get(),
                            max_fee.get(),
                        );
                        let state = WalletBridge::global().state().onchain;
                        if state.phase != OnchainPhase::Confirming {
                            validation_error.set(state.error);
                        }
                    },
                ))
                .when(!node_online, |view| {
                    view.child(
                        text("节点离线。链上转账必须启动本地 Fiber 节点以解密签名密钥。")
                            .text_size(px(ts::SM))
                            .text_color(move || pal_fn(&cx).danger),
                    )
                }),
        )
    })
}

fn onchain_result(cx: Cx) -> AnyElement {
    dynamic(move || {
        let state = cx.ui_state().onchain;
        match state.phase {
            OnchainPhase::Idle | OnchainPhase::Confirming => None,
            OnchainPhase::Broadcasting => Some(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(spinner(cx))
                    .child(
                        text("正在构建、签名并广播 CKB Testnet 交易…")
                            .text_size(px(ts::SM))
                            .text_color(move || pal_fn(&cx).fg_muted),
                    ),
            ),
            OnchainPhase::Broadcast
            | OnchainPhase::Uncertain
            | OnchainPhase::Committed
            | OnchainPhase::Rejected
            | OnchainPhase::Unknown => Some(onchain_status_card(cx, &state)),
            OnchainPhase::Failed => Some(
                view()
                    .gap_2()
                    .p_3()
                    .rounded(px(10.0))
                    .bg(move || pal_fn(&cx).danger.opacity(0.10))
                    .child(
                        text("链上转账未提交")
                            .text_size(px(ts::SM))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(move || pal_fn(&cx).danger),
                    )
                    .child(
                        text(
                            state
                                .error
                                .unwrap_or_else(|| "构建、签名或广播失败。".to_owned()),
                        )
                        .text_size(px(ts::SM))
                        .text_color(move || pal_fn(&cx).fg_muted),
                    ),
            ),
        }
    })
}

fn onchain_status_card(cx: Cx, state: &OnchainState) -> View {
    let tx_hash = state.query_hash.clone().or_else(|| {
        state
            .transfer
            .as_ref()
            .map(|transfer| transfer.tx_hash.clone())
    });
    let Some(tx_hash) = tx_hash else {
        return view();
    };
    let (title, color, detail) = match state.phase {
        OnchainPhase::Broadcast => {
            let status = state.status.as_ref().map(|status| status.state);
            if state.broadcast_accepted || status.is_some() {
                (
                    "广播已接受 · 等待确认",
                    pal_fn(&cx).warn,
                    status
                        .map(OnchainTxState::label)
                        .unwrap_or("RPC 已接受交易哈希，尚未取得链上状态")
                        .to_owned(),
                )
            } else {
                (
                    "广播结果待核实",
                    pal_fn(&cx).warn,
                    state
                        .error
                        .clone()
                        .unwrap_or_else(|| "RPC 未明确接受广播结果；不要直接重试。".to_owned()),
                )
            }
        }
        OnchainPhase::Uncertain => (
            "广播结果不确定",
            pal_fn(&cx).warn,
            state
                .error
                .clone()
                .unwrap_or_else(|| "RPC 未返回确定的广播结果；不能直接重试。".to_owned()),
        ),
        OnchainPhase::Committed => (
            "已上链确认",
            pal_fn(&cx).ok,
            state
                .status
                .as_ref()
                .map(|status| {
                    format!(
                        "committed · block {}",
                        status
                            .block_number
                            .map(|number| number.to_string())
                            .unwrap_or_else(|| "未知".to_owned())
                    )
                })
                .unwrap_or_else(|| "RPC 报告 committed".to_owned()),
        ),
        OnchainPhase::Rejected => (
            "交易被拒绝",
            pal_fn(&cx).danger,
            state
                .status
                .as_ref()
                .and_then(|status| status.reason.clone())
                .or_else(|| state.error.clone())
                .unwrap_or_else(|| "CKB RPC 报告 rejected".to_owned()),
        ),
        OnchainPhase::Unknown => (
            "确认状态未知",
            pal_fn(&cx).warn,
            state
                .error
                .clone()
                .unwrap_or_else(|| "尚未取得终态；不要据此认定失败或成功。".to_owned()),
        ),
        OnchainPhase::Idle
        | OnchainPhase::Confirming
        | OnchainPhase::Broadcasting
        | OnchainPhase::Failed => {
            unreachable!("only terminal or broadcast states are rendered here")
        }
    };
    let last_checked = state
        .last_checked_at_ms
        .map(crate::bridge::timestamp)
        .unwrap_or_else(|| "-".to_owned());
    let can_refresh = matches!(
        state.phase,
        OnchainPhase::Broadcast | OnchainPhase::Uncertain | OnchainPhase::Unknown
    );
    let can_clear = !matches!(
        state.phase,
        OnchainPhase::Confirming
            | OnchainPhase::Broadcasting
            | OnchainPhase::Uncertain
            | OnchainPhase::Unknown
    );
    let failed = state.phase == OnchainPhase::Rejected;

    view()
        .gap_2()
        .p_3()
        .rounded(px(10.0))
        .bg(move || {
            if failed {
                pal_fn(&cx).danger.opacity(0.10)
            } else {
                color.opacity(0.10)
            }
        })
        .child(
            text(title)
                .text_size(px(ts::SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color),
        )
        .child(
            text(detail)
                .text_size(px(ts::SM))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    text(tx_hash.clone())
                        .text_size(px(ts::XS))
                        .font_family(theme::mono_family)
                        .text_color(move || pal_fn(&cx).fg_muted)
                        .flex_1(),
                )
                .child(copy_chip(cx, move || tx_hash.clone())),
        )
        .child(detail_kv(cx, "最后查询", move || last_checked.clone()))
        .when(can_refresh, |card| {
            card.child(primary_button(
                cx,
                "查询链上状态",
                || true,
                || WalletBridge::global().refresh_onchain_status(),
            ))
        })
        .when(can_clear, |card| {
            card.child(outline_button(
                cx,
                "清除结果",
                || true,
                || WalletBridge::global().clear_onchain(),
            ))
        })
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
                    if !items
                        .iter()
                        .any(|item| item.payment_hash == invoice.payment_hash)
                    {
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
                .child(labeled_input(
                    cx,
                    "备注（可选）",
                    description,
                    "给付款人看",
                    false,
                ))
                .child(primary_button(
                    cx,
                    "创建发票",
                    move || !cx.ui_state().invoice_busy,
                    move || {
                        WalletBridge::global()
                            .create_invoice(submit_amount.get(), submit_description.get())
                    },
                ))
                .child(dynamic(move || {
                    cx.ui_state()
                        .invoice_busy
                        .then(|| view().child(spinner(cx)))
                }))
                .child(dynamic(move || {
                    cx.ui_state()
                        .invoice_error
                        .clone()
                        .map(|message| view().child(error_text(cx, message)))
                }))
                .child(dynamic(move || {
                    cx.ui_state().last_invoice.clone().map(|invoice| {
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
                                text(format!(
                                    "payment hash {} · {} CKB",
                                    invoice.payment_hash,
                                    shannons_to_ckb(&invoice.amount)
                                ))
                                .text_size(px(ts::XS))
                                .text_color(move || pal_fn(&cx).fg_muted),
                            )
                    })
                }))
                // The QR canvas must not live inside a re-running region (it
                // is rebuilt with the region and never paints). This reactive
                // `when` creates it once an invoice exists; its paint closure
                // re-reads the invoice on changes.
                .child(when(
                    move || cx.ui_state().last_invoice.is_some(),
                    move || {
                        view()
                            .items_center()
                            .gap_1()
                            .child(qr_canvas(
                                cx,
                                move || {
                                    cx.ui_state()
                                        .last_invoice
                                        .clone()
                                        .map(|invoice| invoice.invoice)
                                        .unwrap_or_default()
                                },
                                180.0,
                            ))
                            .child(
                                text("扫描二维码向本节点支付此发票")
                                    .text_size(px(ts::XS))
                                    .text_color(move || pal_fn(&cx).fg_muted),
                            )
                    },
                )),
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
                                    invoice
                                        .description
                                        .clone()
                                        .unwrap_or_else(|| "-".to_owned()),
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
// ------------------------------------------------------------- channels ----

/// 通道: connected peers, open-channel form, channel list.
fn channels_view(cx: Cx) -> View {
    let peer_input = cx.channel_peer;
    let address_input = cx.channel_address;
    let amount_input = cx.channel_amount;
    let public = cx.channel_public;
    let close_target = signal::<Option<String>>(None);
    let close_mode = signal(0_usize);
    let close_fee = signal(String::new());
    let submit_peer = peer_input;
    let submit_address = address_input;
    let submit_amount = amount_input;
    let submit_public = public;

    view()
        .gap_4()
        .child(
            card(cx)
                .p_6()
                .gap_4()
                .max_w(px(620.0))
                .child(section_label(cx, "开新通道"))
                .child(labeled_input(
                    cx,
                    "对端节点 ID（33 字节 hex pubkey）",
                    peer_input,
                    "02…",
                    false,
                ))
                .child(labeled_input(
                    cx,
                    "对端地址（可选，未连接时自动 connect）",
                    address_input,
                    "/ip4/x.x.x.x/tcp/8228",
                    false,
                ))
                .child(labeled_input(cx, "通道金额 (CKB)", amount_input, "例如 100", false))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            text("公开通道（可转发路由）")
                                .text_size(px(ts::SM))
                                .text_color(move || pal_fn(&cx).fg),
                        )
                        .child(toggle(cx, public, |value| {
                            tracing::info!(public = value, "channel visibility toggled");
                        })),
                )
                .child(primary_button(
                    cx,
                    "开通道",
                    move || !cx.ui_state().open_channel_busy,
                    move || {
                        WalletBridge::global().open_channel(
                            submit_peer.get(),
                            submit_amount.get(),
                            submit_public.get(),
                            Some(submit_address.get()),
                        )
                    },
                ))
                .child(dynamic(move || {
                    cx.ui_state().open_channel_busy.then(|| view().child(spinner(cx)))
                }))
                .child(dynamic(move || {
                    cx.ui_state()
                        .open_channel_error
                        .clone()
                        .map(|message| view().child(error_text(cx, message)))
                })),
        )
        .child(channel_close_confirmation(
            cx,
            close_target,
            close_mode,
            close_fee,
        ))
        .child(
            card(cx)
                .p_6()
                .gap_3()
                .child(section_label(cx, "已连接节点"))
                .child(dynamic(move || {
                    let state = cx.ui_state();
                    if state.peers.is_empty() {
                        Some(
                            view().py_2().child(
                                text(if state.snapshot.map(|s| s.phase)
                                    == Some(NodePhase::Running)
                                {
                                    "暂无已连接节点。"
                                } else {
                                    "节点离线，启动后显示。"
                                })
                                .text_size(px(ts::SM))
                                .text_color(move || pal_fn(&cx).fg_muted),
                            ),
                        )
                    } else {
                        Some(view().gap_2().children(
                            state.peers.iter().cloned().map(|peer| {
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .pb_2()
                                    .child(
                                        view().flex_1().child(
                                            text(format!(
                                                "{}\n{}",
                                                theme::short_id(&peer.pubkey, 16),
                                                peer.address
                                            ))
                                            .text_size(px(ts::XS))
                                            .font_family(theme::mono_family)
                                            .text_color(move || pal_fn(&cx).fg_muted),
                                        ),
                                    )
                                    .child(
                                        text("选用")
                                            .text_size(px(ts::XS))
                                            .px_2()
                                            .py_1()
                                            .rounded(px(6.0))
                                            .bg(move || pal_fn(&cx).card_alt)
                                            .text_color(move || pal_fn(&cx).fg)
                                            .cursor_pointer()
                                            .active(|style| style.opacity(0.6))
                                            .on_press(move || {
                                                cx.channel_peer.set(peer.pubkey.clone())
                                            }),
                                    )
                            })
                        ))
                    }
                })),
        )
        .child(
            card(cx)
                .p_6()
                .gap_3()
                .child(section_label(cx, "通道列表"))
                .child(dynamic(move || {
                    let state = cx.ui_state();
                    if state.channels.is_empty() {
                        Some(
                            view().py_2().child(
                                text(if state.snapshot.map(|s| s.phase)
                                    == Some(NodePhase::Running)
                                {
                                    "暂无通道。开通道需要 funding address 有余额。"
                                } else {
                                    "节点离线，启动后显示。"
                                })
                                .text_size(px(ts::SM))
                                .text_color(move || pal_fn(&cx).fg_muted),
                            ),
                        )
                    } else {
                        let running =
                            state.snapshot.map(|snapshot| snapshot.phase) == Some(NodePhase::Running);
                        let any_closing = state.closing_channel_id.is_some();
                        let attempts = state.channel_close_attempts.clone();
                        Some(view().gap_2().children(state.channels.iter().cloned().map(
                            |channel| {
                                let key = channel_close::canonical_channel_key(&channel.channel_id);
                                let attempt = attempts
                                    .iter()
                                    .find(|attempt| attempt.normalized_channel_id == key)
                                    .cloned();
                                channel_row(
                                    cx,
                                    channel,
                                    attempt,
                                    close_target,
                                    close_mode,
                                    running,
                                    any_closing,
                                )
                            },
                        )))
                    }
                })),
        )
        .child(
            text("开通道流程：先给 funding address 充测试网 CKB → 对端保持连接 → 提交后等待链上确认，通道变为 Ready 即可收付款。关闭成功后通道余额会回到默认 funding 地址；链上确认仍可能继续结算中。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

/// The FNN channel state as a colored badge: ready = green, closing/settling
/// = amber, closed = muted, failures = red.
fn channel_state_chip(cx: Cx, state: &str) -> View {
    let state = state.to_owned();
    status_chip(
        cx,
        {
            let state = state.clone();
            move || state.clone()
        },
        move || {
            let colors = pal_fn(&cx);
            if state.contains("Failed") || state.contains("Error") {
                colors.danger
            } else if state.contains("Closing")
                || state.contains("Shutdown")
                || state.contains("Waiting")
            {
                colors.warn
            } else if state == "ChannelReady" {
                colors.ok
            } else if state == "Closed" {
                colors.fg_muted
            } else {
                colors.accent
            }
        },
    )
}

fn close_mode_from_index(index: usize) -> ChannelCloseMode {
    if index == 1 {
        ChannelCloseMode::Force
    } else {
        ChannelCloseMode::Cooperative
    }
}

fn close_mode_index(mode: ChannelCloseMode) -> usize {
    usize::from(mode == ChannelCloseMode::Force)
}

fn close_mode_label(mode: ChannelCloseMode) -> &'static str {
    match mode {
        ChannelCloseMode::Cooperative => "协调关闭",
        ChannelCloseMode::Force => "强制关闭",
    }
}

fn channel_close_confirmation(
    cx: Cx,
    target: Signal<Option<String>>,
    mode: Signal<usize>,
    fee: Signal<String>,
) -> View {
    view().child(dynamic(move || {
        let requested_id = target.get()?;
        let key = channel_close::canonical_channel_key(&requested_id);
        let state = cx.ui_state();
        let channel = state
            .channels
            .iter()
            .find(|channel| channel_close::canonical_channel_key(&channel.channel_id) == key)?
            .clone();
        let running = state.snapshot.map(|snapshot| snapshot.phase) == Some(NodePhase::Running);
        let busy = state.closing_channel_id.is_some();
        let selected = close_mode_from_index(mode.get());

        Some(
            card(cx)
                .p_5()
                .gap_3()
                .border_2()
                .border_color(move || pal_fn(&cx).warn)
                .child(section_label(cx, "确认关闭方式"))
                .child(
                    text(format!(
                        "{} · {} CKB local · {} CKB remote",
                        channel.channel_id,
                        shannons_to_ckb(&channel.local_balance),
                        shannons_to_ckb(&channel.remote_balance)
                    ))
                    .text_size(px(ts::SM))
                    .font_family(theme::mono_family)
                    .text_color(move || pal_fn(&cx).fg),
                )
                .when_some(channel.failure_detail.clone(), |card, detail| {
                    card.child(
                        text(format!("失败原因：{detail}"))
                            .text_size(px(ts::XS))
                            .text_color(move || pal_fn(&cx).danger),
                    )
                })
                .when_some(
                    channel.shutdown_transaction_hash.clone(),
                    |card, tx_hash| {
                        card.child(
                            text(format!(
                                "关闭交易：{tx_hash}\n等待链上确认，可在 CKB 浏览器查询。"
                            ))
                            .text_size(px(ts::XS))
                            .font_family(theme::mono_family)
                            .text_color(move || pal_fn(&cx).fg_muted),
                        )
                    },
                )
                .child(segmented(cx, &["协调关闭", "强制关闭"], mode))
                .child(
                    text(channel_close::mode_description(selected))
                        .text_size(px(ts::XS))
                        .text_color(move || {
                            let colors = pal_fn(&cx);
                            if close_mode_from_index(mode.get()) == ChannelCloseMode::Force {
                                colors.danger
                            } else {
                                colors.fg_muted
                            }
                        }),
                )
                .child(dynamic(move || {
                    (close_mode_from_index(mode.get()) == ChannelCloseMode::Cooperative).then(
                        || {
                            view()
                                .gap_1()
                                .child(
                                    labeled_input(
                                        cx,
                                        "关闭费率（可选，shannons/KW）",
                                        fee,
                                        "留空使用通道 commitment 费率",
                                        false,
                                    )
                                    .w(px(280.0)),
                                )
                                .child(
                                    text("费率低于通道 commitment 费率时 FNN 会拒绝。")
                                        .text_size(px(ts::XS))
                                        .text_color(move || pal_fn(&cx).fg_muted),
                                )
                        },
                    )
                }))
                .child(
                    view()
                        .flex_row()
                        .gap_2()
                        .child(outline_button(
                            cx,
                            "取消",
                            || true,
                            move || target.set(None),
                        ))
                        .children([
                            confirm_close_button(cx, "确认协调关闭", target, mode, fee, true),
                            confirm_close_button(cx, "确认强制关闭", target, mode, fee, false),
                        ]),
                )
                .when(!running, |card| {
                    card.child(
                        text("节点离线，无法关闭通道。")
                            .text_size(px(ts::SM))
                            .text_color(move || pal_fn(&cx).danger),
                    )
                })
                .when(busy, |card| {
                    card.child(
                        text("另一个关闭请求正在处理中，请等待完成。")
                            .text_size(px(ts::SM))
                            .text_color(move || pal_fn(&cx).warn),
                    )
                }),
        )
    }))
}

/// Renders only the button for the currently selected close mode, so the
/// final action always names the destructive method explicitly.
fn confirm_close_button(
    cx: Cx,
    label: &'static str,
    target: Signal<Option<String>>,
    mode: Signal<usize>,
    fee: Signal<String>,
    cooperative: bool,
) -> View {
    let selected =
        move || (close_mode_from_index(mode.get()) == ChannelCloseMode::Cooperative) == cooperative;
    view().when(selected(), |button| {
        button.child(primary_button(
            cx,
            label,
            move || {
                let state = cx.ui_state();
                selected()
                    && state.snapshot.map(|snapshot| snapshot.phase) == Some(NodePhase::Running)
                    && state.closing_channel_id.is_none()
            },
            move || {
                let Some(channel_id) = target.get() else {
                    return;
                };
                let selected_mode = close_mode_from_index(mode.get());
                if selected_mode == ChannelCloseMode::Cooperative
                    && !matches!(
                        cx.ui_state()
                            .channels
                            .iter()
                            .find(|channel| {
                                channel_close::canonical_channel_key(&channel.channel_id)
                                    == channel_close::canonical_channel_key(&channel_id)
                            })
                            .map(|channel| channel.state.as_str()),
                        Some("ChannelReady")
                    )
                {
                    toast("此通道当前只能强制关闭。".to_owned(), ToastDuration::Short);
                    return;
                }
                let fee_rate = match channel_close::parse_fee_rate(selected_mode, &fee.get()) {
                    Ok(rate) => rate,
                    Err(error) => {
                        toast(error, ToastDuration::Short);
                        return;
                    }
                };
                WalletBridge::global().close_channel(channel_id, selected_mode, fee_rate);
                target.set(None);
                fee.set(String::new());
            },
        ))
    })
}

fn channel_row(
    cx: Cx,
    channel: WalletChannel,
    attempt: Option<ChannelCloseAttempt>,
    close_target: Signal<Option<String>>,
    close_mode: Signal<usize>,
    running: bool,
    any_closing: bool,
) -> View {
    let action = channel_close::close_action_kind(&channel.state, channel.state_flags);
    let action_id = channel.channel_id.clone();
    let header = view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            view()
                .gap_1()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            text(format!(
                                "{} CKB local · {} CKB remote",
                                shannons_to_ckb(&channel.local_balance),
                                shannons_to_ckb(&channel.remote_balance),
                            ))
                            .text_size(px(ts::SM))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(move || pal_fn(&cx).fg),
                        )
                        .child(channel_state_chip(cx, &channel.state)),
                )
                .child(
                    text(format!(
                        "{} · {} · {}",
                        if channel.is_inbound {
                            "inbound"
                        } else {
                            "outbound"
                        },
                        channel.channel_id,
                        channel.peer_id
                    ))
                    .text_size(px(ts::XS))
                    .font_family(theme::mono_family)
                    .text_color(move || pal_fn(&cx).fg_muted),
                )
                .when_some(channel.failure_detail.clone(), |row, detail| {
                    row.child(
                        text(format!("失败原因：{detail}"))
                            .text_size(px(ts::XS))
                            .text_color(move || pal_fn(&cx).danger),
                    )
                })
                .when_some(channel.shutdown_transaction_hash.clone(), |row, tx_hash| {
                    row.child(
                        text(format!(
                            "关闭交易：{tx_hash}\n等待链上确认，可在 CKB 浏览器查询。"
                        ))
                        .text_size(px(ts::XS))
                        .font_family(theme::mono_family)
                        .text_color(move || pal_fn(&cx).fg_muted),
                    )
                }),
        )
        .when_some(action, |row, action| {
            let enabled = running && !any_closing;
            row.child(
                text(match action {
                    CloseActionKind::ChooseMode => "关闭通道",
                    CloseActionKind::ForceOnly => "强制关闭",
                })
                .text_size(px(ts::XS))
                .px_2()
                .py_1()
                .rounded(px(6.0))
                .bg(move || {
                    let colors = pal_fn(&cx);
                    if enabled {
                        colors.card_alt
                    } else {
                        colors.card
                    }
                })
                .text_color(move || {
                    let colors = pal_fn(&cx);
                    if !enabled {
                        colors.fg_muted
                    } else if action == CloseActionKind::ForceOnly {
                        colors.danger
                    } else {
                        colors.fg
                    }
                })
                .cursor_pointer()
                .active(|style| style.opacity(0.6))
                .on_press(move || {
                    if !enabled {
                        return;
                    }
                    close_target.set(Some(action_id.clone()));
                    close_mode.set(match action {
                        CloseActionKind::ChooseMode => {
                            close_mode_index(ChannelCloseMode::Cooperative)
                        }
                        CloseActionKind::ForceOnly => close_mode_index(ChannelCloseMode::Force),
                    });
                }),
            )
        });

    let status_row = attempt.map(|attempt| {
        let status = attempt.status;
        let mode = close_mode_label(attempt.mode);
        view()
            .gap_1()
            .child(
                text(format!("{mode} · {}", channel_close::status_label(status)))
                    .text_size(px(ts::XS))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(move || {
                        let colors = pal_fn(&cx);
                        match status {
                            ChannelCloseStatus::Closed => colors.ok,
                            ChannelCloseStatus::Failed
                            | ChannelCloseStatus::FailedWithDetail
                            | ChannelCloseStatus::OpeningFailed => colors.danger,
                            ChannelCloseStatus::InProgress
                            | ChannelCloseStatus::WaitingOnchainSettlement
                            | ChannelCloseStatus::Submitted => colors.warn,
                            ChannelCloseStatus::Unknown => colors.fg_muted,
                        }
                    }),
            )
            .when_some(attempt.error, |row, error| {
                row.child(
                    text(error)
                        .text_size(px(ts::XS))
                        .text_color(move || pal_fn(&cx).danger),
                )
            })
    });

    view()
        .gap_2()
        .p_3()
        .rounded(px(10.0))
        .bg(move || pal_fn(&cx).bg)
        .child(header)
        .child(status_row)
}

fn send_view(cx: Cx) -> View {
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
                .child(scan_button(cx, crate::scan::ScanTarget::Invoice))
                .child(primary_button(
                    cx,
                    "发送支付",
                    move || !cx.ui_state().send_busy && !submit_invoice.get().trim().is_empty(),
                    move || WalletBridge::global().send_payment(submit_invoice.get()),
                ))
                .child(dynamic(move || {
                    cx.ui_state().send_busy.then(|| view().child(spinner(cx)))
                }))
                .child(dynamic(move || {
                    cx.ui_state()
                        .send_result
                        .clone()
                        .map(|message| view().child(text(message).text_size(px(ts::SM))))
                }))
                .child(dynamic(move || {
                    cx.ui_state()
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
                    let state = cx.ui_state();
                    if state.payments.is_empty() {
                        Some(
                            view().py_3().child(
                                text("暂无支付。")
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
                                    .map(|payment| payment_row(cx, payment)),
                            ),
                        )
                    }
                })),
        )
}

/// 扫一扫: opens the platform camera scanner and writes the decoded text
/// into the target input. Renders nothing where no scanner is installed
/// (desktop hosts), so the shared page keeps its layout.
fn scan_button(cx: Cx, target: crate::scan::ScanTarget) -> View {
    if !crate::scan::available() {
        return view();
    }
    let on_result = move |text: String| match target {
        crate::scan::ScanTarget::Invoice => cx.pay_invoice.set(text),
        crate::scan::ScanTarget::OnchainAddress => cx.onchain_address.set(text),
    };
    outline_button(cx, "扫一扫", crate::scan::available, move || {
        crate::scan::request(target, std::boxed::Box::new(on_result));
    })
}

/// One payment record row, shared with the home page.
pub fn payment_row(cx: Cx, payment: &WalletPayment) -> View {
    let status = payment.status;
    let failed_error = payment.failed_error.clone();
    let hash = theme::short_id(&payment.payment_hash, 10);
    let fee = payment.fee_shannons.clone();
    let created = crate::bridge::timestamp(payment.created_at_ms);
    let updated = crate::bridge::timestamp(payment.last_updated_at_ms);
    view()
        .flex_row()
        .items_start()
        .gap_2()
        .pb_2()
        .child(status_chip(
            cx,
            move || {
                match status {
                    WalletPaymentStatus::Success => "成功",
                    WalletPaymentStatus::Failed => "失败",
                    WalletPaymentStatus::Inflight => "进行中",
                    WalletPaymentStatus::Created => "已创建",
                }
                .to_owned()
            },
            move || {
                let colors = pal_fn(&cx);
                match status {
                    WalletPaymentStatus::Success => colors.ok,
                    WalletPaymentStatus::Failed => colors.danger,
                    WalletPaymentStatus::Created | WalletPaymentStatus::Inflight => colors.warn,
                }
            },
        ))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text(hash)
                        .text_size(px(ts::XS))
                        .font_family(theme::mono_family)
                        .text_color(move || pal_fn(&cx).fg),
                )
                .child(
                    text(format!(
                        "费用 {fee} shannons · 创建 {created} · 更新 {updated}"
                    ))
                    .text_size(px(ts::XS))
                    .font_family(theme::mono_family)
                    .text_color(move || pal_fn(&cx).fg_muted),
                )
                .when_some(failed_error.clone(), |row, error| {
                    row.child(
                        text(error)
                            .text_size(px(ts::XS))
                            .font_family(theme::mono_family)
                            .text_color(move || pal_fn(&cx).danger),
                    )
                }),
        )
}
