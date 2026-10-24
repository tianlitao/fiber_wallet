//! 设置: theme (mode + accent), proxy, node info, about.

use nsh_core::ui::prelude::*;

use crate::bridge::WalletBridge;
use crate::config::{self, Accent, AppConfig, ThemeMode};
use crate::theme::type_scale as ts;
use crate::widgets::*;

pub fn settings_page(cx: Cx) -> View {
    view()
        .gap_5()
        .max_w(px(720.0))
        .child(page_header(cx, "设置", "外观、网络与节点"))
        .child(appearance_card(cx))
        .child(proxy_card(cx))
        .child(node_card(cx))
        .child(about_card(cx))
}

fn settings_group(cx: Cx, title: &'static str) -> View {
    card(cx)
        .p_6()
        .gap_4()
        .child(
            text(title)
                .text_size(px(ts::MD))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || pal_fn(&cx).fg),
        )
}

fn appearance_card(cx: Cx) -> View {
    view().child(
        settings_group(cx, "外观")
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        text("主题模式")
                            .text_size(px(ts::SM))
                            .text_color(move || pal_fn(&cx).fg),
                    )
                    .child(segmented(
                        cx,
                        &[
                            ThemeMode::System.label(),
                            ThemeMode::Light.label(),
                            ThemeMode::Dark.label(),
                        ],
                        mode_index(cx),
                    )),
            )
            .child(divider(cx))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        text("主题色")
                            .text_size(px(ts::SM))
                            .text_color(move || pal_fn(&cx).fg),
                    )
                    .child(
                        view()
                            .flex_row()
                            .gap_2()
                            .children(Accent::ALL.iter().map(|accent| swatch(cx, *accent))),
                    ),
            ),
    )
}

/// Maps the config's theme mode onto the segmented control index, and writes
/// selection back.
fn mode_index(cx: Cx) -> Signal<usize> {
    let index = signal(mode_to_index(&cx.config.get().theme_mode));
    let cx_write = cx;
    effect(move || {
        let selected = index.get();
        let mode = index_to_mode(selected);
        if cx_write.config.get().theme_mode != mode {
            update_config(cx_write, |config| config.theme_mode = mode);
        }
    });
    index
}

fn mode_to_index(mode: &ThemeMode) -> usize {
    match mode {
        ThemeMode::System => 0,
        ThemeMode::Light => 1,
        ThemeMode::Dark => 2,
    }
}

fn index_to_mode(index: usize) -> ThemeMode {
    match index {
        0 => ThemeMode::System,
        1 => ThemeMode::Light,
        _ => ThemeMode::Dark,
    }
}

fn swatch(cx: Cx, accent: Accent) -> View {
    view()
        .size(px(28.0))
        .rounded(px(14.0))
        .bg(crate::theme::palette(&AppConfig {
            accent,
            ..cx.config.get().clone()
        })
        .accent)
        .border_2()
        .border_color(move || {
            let colors = pal_fn(&cx);
            if cx.config.get().accent == accent {
                colors.fg
            } else {
                transparent_black()
            }
        })
        .cursor_pointer()
        .active(|style| style.opacity(0.8))
        .on_press(move || {
            update_config(cx, |config| config.accent = accent);
        })
}

fn proxy_card(cx: Cx) -> View {
    let host = signal(cx.config.get().proxy_host.clone());
    let port = signal(if cx.config.get().proxy_port > 0 {
        cx.config.get().proxy_port.to_string()
    } else {
        String::new()
    });
    let enabled = signal(cx.config.get().proxy_enabled);
    let ui = WalletBridge::global();

    view().child(
        settings_group(cx, "代理")
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        view()
                            .gap_1()
                            .child(
                                text("启用 HTTP(S) 代理")
                                    .text_size(px(ts::SM))
                                    .text_color(move || pal_fn(&cx).fg),
                            )
                            .child(
                                text("作用于 CKB RPC 等 HTTP 请求；P2P 连接不走代理")
                                    .text_size(px(ts::XS))
                                    .text_color(move || pal_fn(&cx).fg_muted),
                            ),
                    )
                    .child(toggle(cx, enabled, |enabled| {
                        tracing::info!(enabled, "proxy toggled");
                    })),
            )
            .child(
                view()
                    .flex_row()
                    .gap_3()
                    .child(labeled_input(cx, "地址", host, "127.0.0.1", false).flex_1())
                    .child(labeled_input(cx, "端口", port, "7897", false).w(px(110.0))),
            )
            .child(
                view()
                    .flex_row()
                    .gap_3()
                    .child(
                        primary_button(cx, "保存并应用", || true, move || {
                            let port_value: u16 = port.get().trim().parse().unwrap_or(0);
                            update_config(cx, |config| {
                                config.proxy_enabled = enabled.get();
                                config.proxy_host = host.get().trim().to_owned();
                                config.proxy_port = port_value;
                            });
                            toast("代理设置已保存并写入环境变量", ToastDuration::Short);
                        }),
                    )
                    .child(
                        outline_button(
                            cx,
                            "重启节点生效",
                            move || !ui.state().busy,
                            move || {
                                WalletBridge::global().restart();
                                toast("节点重启中，稍候自动恢复在线", ToastDuration::Short);
                            },
                        ),
                    ),
            )
            .child(
                text("保存即写入环境变量并持久化到 config.json；正在运行的 RPC 客户端与节点需要重启后读取新代理。")
                    .text_size(px(ts::XS))
                    .text_color(move || pal_fn(&cx).fg_muted),
            ),
    )
}

fn node_card(cx: Cx) -> View {
    let ui = WalletBridge::global();
    view().child(
        settings_group(cx, "节点")
            .child(detail_row(cx, "网络", || "CKB Fiber 测试网".to_owned()))
            .child(detail_row(cx, "数据目录", crate::bridge::data_directory))
            .child(
                outline_button(cx, "在 Finder 中打开", || true, move || {
                    let path = crate::bridge::data_directory();
                    #[cfg(target_os = "macos")]
                    {
                        let _ = std::process::Command::new("open").arg(&path).spawn();
                    }
                    #[cfg(not(target_os = "macos"))]
                    tracing::info!(path, "open data directory");
                    toast("已在 Finder 中打开数据目录", ToastDuration::Short);
                }),
            )
            .child(detail_row(cx, "节点版本", {
                move || {
                    ui.state()
                        .node_info
                        .map(|info| info.version)
                        .unwrap_or_else(|| "—".to_owned())
                }
            }))
            .child(detail_row(cx, "Node ID", {
                move || {
                    ui.state()
                        .node_info
                        .map(|info| info.node_id)
                        .or_else(|| {
                            ui.state()
                                .snapshot
                                .and_then(|snapshot| snapshot.node_id)
                        })
                        .unwrap_or_else(|| "—".to_owned())
                }
            }))
            .child(
                text("每次安装都是独立的钱包与节点实例；数据目录独占锁定，同一时刻只有一个实例能打开。")
                    .text_size(px(ts::XS))
                    .text_color(move || pal_fn(&cx).fg_muted),
            ),
    )
}

fn about_card(cx: Cx) -> View {
    view().child(
        settings_group(cx, "关于")
            .child(detail_row(cx, "应用", || "Fiber Wallet (NSH) v0.2.0".to_owned()))
            .child(detail_row(
                cx,
                "引擎",
                || "Native Shell 0.2.0 · AppKit".to_owned(),
            ))
            .child(detail_row(
                cx,
                "核心",
                || "fiber_wallet_core · FNN v0.9.1 (sqlite)".to_owned(),
            ))
            .child(
                text("本应用为 M1/M2 阶段的开发预览：私钥加密口令使用开发回退方案，M2 完成前请勿存入大额资产。")
                    .text_size(px(ts::XS))
                    .text_color(move || pal_fn(&cx).warn),
            ),
    )
}

/// Applies a config edit: persists to config.json and refreshes the
/// process-wide proxy environment.
fn update_config(cx: Cx, apply: impl FnOnce(&mut AppConfig)) {
    cx.config.update(|config| apply(config));
    let snapshot = cx.config.get();
    config::save(&snapshot);
    config::apply_proxy_env(&snapshot);
}
