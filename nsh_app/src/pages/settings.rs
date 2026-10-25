//! 设置: theme (mode + accent), proxy, node info, about.

use nsh_core::ui::prelude::*;

use fiber_wallet_core::api::node::NodePhase;

use crate::bridge::WalletBridge;
use crate::config::{self, Accent, AppConfig, ThemeMode};
use crate::theme::type_scale as ts;
use crate::widgets::*;

pub fn settings_page(cx: Cx) -> View {
    view()
        .gap_5()
        .max_w(px(720.0))
        .child(page_header(cx, "设置", "外观、网络、安全与备份"))
        .child(appearance_card(cx))
        .child(proxy_card(cx))
        .child(security_card(cx))
        .child(backup_card(cx))
        .child(node_card(cx))
        .child(about_card(cx))
}

fn settings_group(cx: Cx, title: &'static str) -> View {
    card(cx).p_6().gap_4().child(
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
                            move || !cx.ui_state().busy,
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

/// 安全: keystore password vault kind + rotation (node must be stopped).
fn security_card(cx: Cx) -> View {
    let ui = WalletBridge::global();
    let vault_label = ui.vault_kind.label();
    let env_override = ui.vault_kind == crate::vault::VaultKind::Env;

    view().child(
        settings_group(cx, "安全")
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        view()
                            .gap_1()
                            .child(
                                text("密钥加密密码存储")
                                    .text_size(px(ts::SM))
                                    .text_color(move || pal_fn(&cx).fg),
                            )
                            .child(
                                text("加密数据目录中的 CKB 私钥与节点身份密钥")
                                    .text_size(px(ts::XS))
                                    .text_color(move || pal_fn(&cx).fg_muted),
                            ),
                    )
                    .child(
                        text(vault_label)
                            .text_size(px(ts::SM))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(move || pal_fn(&cx).accent),
                    ),
            )
            .child(divider(cx))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        view()
                            .gap_1()
                            .child(
                                text("轮换密钥加密密码")
                                    .text_size(px(ts::SM))
                                    .text_color(move || pal_fn(&cx).fg),
                            )
                            .child(
                                text("生成新随机密码并重加密密钥文件；需先停止节点")
                                    .text_size(px(ts::XS))
                                    .text_color(move || pal_fn(&cx).fg_muted),
                            ),
                    )
                    .child(outline_button(
                        cx,
                        "轮换",
                        move || !cx.ui_state().password_busy && !env_override,
                        move || {
                            let new_password: String = (0..32)
                                .map(|_| format!("{:02x}", rand::random::<u8>()))
                                .collect();
                            WalletBridge::global().change_password(new_password);
                        },
                    )),
            )
            .when(env_override, |card| {
                card.child(
                    text("当前密码来自 FIBER_SECRET_KEY_PASSWORD 环境变量：轮换会被下次启动覆盖，请直接修改环境变量。")
                        .text_size(px(ts::XS))
                        .text_color(move || pal_fn(&cx).warn),
                )
            })
            .child(dynamic(move || {
                cx.ui_state().password_busy.then(|| view().child(spinner(cx)))
            }))
            .child(dynamic(move || {
                let state = cx.ui_state();
                state
                    .password_message
                    .clone()
                    .map(|message| view().child(ok_text(cx, message)))
                    .or_else(|| {
                        state
                            .password_error
                            .clone()
                            .map(|message| view().child(error_text(cx, message)))
                    })
            })),
    )
}

/// 导出 / 导入: whole wallet state (keystore + channel store) as a
/// password-encrypted archive. Node must be stopped for both.
fn backup_card(cx: Cx) -> View {
    let export_path = signal(default_export_path());
    let export_password = signal(String::new());
    let export_confirm = signal(String::new());
    let import_path = signal(String::new());
    let import_password = signal(String::new());
    let export_path_submit = export_path;
    let export_password_submit = export_password;
    let export_confirm_submit = export_confirm;
    let import_path_submit = import_path;
    let import_password_submit = import_password;

    view().child(
        settings_group(cx, "导出 / 导入")
            .child(
                text("导出 = 数据目录（密钥 + 通道状态）打包并用密码加密；导入 = 替换当前数据并自动重启节点。两种操作都要求节点已停止。")
                    .text_size(px(ts::XS))
                    .text_color(move || pal_fn(&cx).fg_muted),
            )
            .child(
                view()
                    .flex_row()
                    .items_start()
                    .gap_3()
                    .child(
                        labeled_input(cx, "导出到", export_path, "~/Downloads/…", false).flex_1(),
                    )
                    .child(labeled_input(cx, "导出密码 (≥8位)", export_password, "••••••••", false).w(px(170.0)))
                    .child(labeled_input(cx, "确认密码", export_confirm, "••••••••", false).w(px(170.0))),
            )
            .child(
                view()
                    .flex_row()
                    .gap_3()
                    .child(
                        primary_button(
                            cx,
                            "导出钱包状态",
                            move || {
                                let state = cx.ui_state();
                                !state.backup_busy
                                    && state.snapshot.as_ref().map(|s| s.phase)
                                        != Some(NodePhase::Running)
                            },
                            move || {
                                let password = export_password_submit.get();
                                if password.chars().count() < 8
                                    || password != export_confirm_submit.get()
                                {
                                    toast("导出密码需 ≥8 位且两次输入一致", ToastDuration::Short);
                                    return;
                                }
                                WalletBridge::global().export_state(
                                    export_path_submit.get(),
                                    password,
                                );
                            },
                        ),
                    )
                    .child(
                        outline_button(cx, "在 Finder 中显示导出目录", || true, move || {
                            let downloads = std::env::var("HOME")
                                .map(|home| format!("{home}/Downloads"))
                                .unwrap_or_else(|_| ".".to_owned());
                            #[cfg(target_os = "macos")]
                            {
                                let _ = std::process::Command::new("open").arg(&downloads).spawn();
                            }
                            let _ = downloads;
                        }),
                    ),
            )
            .child(divider(cx))
            .child(
                view()
                    .flex_row()
                    .items_start()
                    .gap_3()
                    .child(
                        labeled_input(cx, "导入文件 (.fwexport)", import_path, "选择或粘贴路径", false)
                            .flex_1(),
                    )
                    .child(
                        labeled_input(cx, "导入密码", import_password, "••••••••", false)
                            .w(px(170.0)),
                    ),
            )
            .child(
                text("导入会覆盖当前钱包状态，并把原状态备份到数据目录 import-backup-<时间>/。导入前请确认导出方实例已永久停止——同一通道状态在两处运行会触发对端撤销惩罚。")
                    .text_size(px(ts::XS))
                    .text_color(move || pal_fn(&cx).warn),
            )
            .child(
                outline_button(
                    cx,
                    "导入钱包状态",
                    move || {
                        let state = cx.ui_state();
                        !state.backup_busy
                            && state.snapshot.as_ref().map(|s| s.phase) != Some(NodePhase::Running)
                    },
                    move || {
                        let path = import_path_submit.get();
                        if path.trim().is_empty() {
                            toast("请先填写导入文件路径", ToastDuration::Short);
                            return;
                        }
                        WalletBridge::global().import_state(path, import_password_submit.get());
                    },
                ),
            )
            .child(dynamic(move || {
                cx.ui_state().backup_busy.then(|| view().child(spinner(cx)))
            }))
            .child(dynamic(move || {
                let state = cx.ui_state();
                state
                    .backup_message
                    .clone()
                    .map(|message| view().child(ok_text(cx, message)))
                    .or_else(|| {
                        state
                            .backup_error
                            .clone()
                            .map(|message| view().child(error_text(cx, message)))
                    })
            })),
    )
}

fn ok_text(cx: Cx, message: String) -> View {
    view().child(
        text(message)
            .text_size(px(ts::SM))
            .text_color(move || pal_fn(&cx).ok),
    )
}

/// 网络 selection: 测试网 is the only selectable value today. 主网 is shown
/// for transparency but not clickable — clicking it explains why.
fn network_selector(cx: Cx) -> View {
    view()
        .gap_1()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .children(crate::config::Network::ALL.map(|network| network_chip(cx, network))),
        )
        .child(
            text("主网暂未开放：钱包当前仅支持测试网；切换网络需要重启节点并使用独立数据目录。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

fn network_chip(cx: Cx, network: crate::config::Network) -> View {
    let selected = move || cx.config.get().network == network;
    let available = network == crate::config::Network::Testnet;
    view()
        .flex_row()
        .items_center()
        .gap_1()
        .px_3()
        .py(px(6.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(move || {
            let colors = pal_fn(&cx);
            if selected() {
                colors.accent
            } else {
                colors.border
            }
        })
        .bg(move || {
            let colors = pal_fn(&cx);
            if selected() {
                colors.accent_soft
            } else {
                colors.card
            }
        })
        .child(
            text(network.label())
                .text_size(px(ts::SM))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    let colors = pal_fn(&cx);
                    if selected() {
                        colors.accent
                    } else {
                        colors.fg_muted
                    }
                }),
        )
        .when(available, |chip| chip.cursor_pointer())
        .when(!available, |chip| {
            chip.child(
                text("未开放")
                    .text_size(px(ts::XS))
                    .px_1()
                    .py(px(1.0))
                    .rounded(px(4.0))
                    .bg(move || pal_fn(&cx).card_alt)
                    .text_color(move || pal_fn(&cx).fg_muted),
            )
            .cursor_pointer()
            .on_press(move || {
                toast("主网暂未开放，当前仅支持测试网", ToastDuration::Short);
            })
        })
}

fn default_export_path() -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    match std::env::var("HOME") {
        Ok(home) => format!("{home}/Downloads/fiber-wallet-export-{stamp}.fwexport"),
        Err(_) => format!("fiber-wallet-export-{stamp}.fwexport"),
    }
}

fn node_card(cx: Cx) -> View {
    view().child(
        settings_group(cx, "节点")
            .child(network_selector(cx))
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
                    cx.ui_state()
                        .node_info
                        .map(|info| info.version)
                        .unwrap_or_else(|| "—".to_owned())
                }
            }))
            .child(detail_row(cx, "Node ID", {
                move || {
                    cx.ui_state()
                        .node_info
                        .map(|info| info.node_id)
                        .or_else(|| {
                            cx.ui_state()
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
