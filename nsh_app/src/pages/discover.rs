//! 发现: dApp entry points, reserved for M3+.

use nsh_core::ui::prelude::*;

use crate::theme::type_scale as ts;
use crate::widgets::*;

const DAPPS: [(&str, &str, &str); 6] = [
    ("🌐", "CKB 浏览器", "链上交易、单元格与脚本"),
    ("🔌", "Fiber 探针", "节点与通道网络拓扑"),
    ("🌉", "RGB++ 桥", "Bitcoin 账本资产跨链"),
    ("🖼", "NFT 市场", "CoTA / Spore 资产交易"),
    ("⛏", "CKB DAO", "质押存取与收益查看"),
    ("🛣", "支付路由器", "多跳通道管理与流动性"),
];

pub fn discover_page(cx: Cx) -> View {
    view()
        .gap_5()
        .child(page_header(cx, "发现", "dApp 入口 · 预留"))
        .child(
            text("dApp 框架在 M3+ 开放：经签名的 TSX 模块将在此安全下发，由宿主逐项授权能力。")
                .text_size(px(ts::SM))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(
            view()
                .flex_wrap()
                .gap_4()
                .children(DAPPS.iter().map(|(glyph, name, description)| {
                    dapp_card(cx, glyph, name, description)
                })),
        )
}

fn dapp_card(cx: Cx, glyph: &'static str, name: &'static str, description: &'static str) -> View {
    view()
        .w(px(240.0))
        .gap_3()
        .p_5()
        .rounded(px(12.0))
        .bg(move || pal_fn(&cx).card)
        .border_1()
        .border_color(move || pal_fn(&cx).border)
        .opacity(0.85)
        .cursor_pointer()
        .active(|style| style.opacity(0.6))
        .on_press(move || toast(format!("{name} 即将上线（M3+ 开放 dApp 框架）"), ToastDuration::Short))
        .child(
            view()
                .flex_row()
                .items_start()
                .justify_between()
                .child(
                    view()
                        .size(px(44.0))
                        .items_center()
                        .justify_center()
                        .rounded(px(12.0))
                        .bg(move || pal_fn(&cx).card_alt)
                        .child(text(glyph).text_size(px(22.0))),
                )
                .child(
                    view()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(move || pal_fn(&cx).card_alt)
                        .child(
                            text("即将上线")
                                .text_size(px(ts::XS))
                                .text_color(move || pal_fn(&cx).fg_muted),
                        ),
                ),
        )
        .child(
            text(name)
                .text_size(px(ts::MD))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || pal_fn(&cx).fg),
        )
        .child(
            text(description)
                .text_size(px(ts::SM))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}
