//! Discover: dApp catalog and one persistent native WebView host.
//!
//! Pages are untrusted. A session is bound to the registry origin and to
//! the URL the native WebView reports. Reads require an approved connection;
//! every invoice, payment, dry-run, and channel operation opens a fresh host
//! dialog and is denied by default. Pages never receive key material,
//! arbitrary RPC access, arbitrary URLs, or host-side script execution.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nsh_base::alert_dialog;
use nsh_core::ui::prelude::*;
use serde_json::Value;

use crate::dapp_host::{
    DappHostState, Dispatch, HostMessage, SessionLease, approval_text, executor, new_generation,
};
use crate::dapp_protocol::{self, DappRequest, ProviderMethod, RpcError, WalletAction, error_code};
use crate::dapp_registry::{DappEntry, DappRegistry, SELFTEST_URL, default_registry_path};
use crate::theme::type_scale as ts;
use crate::widgets::*;

static REGISTRY: Mutex<Option<DappRegistry>> = Mutex::new(None);

fn with_registry<R>(apply: impl FnOnce(&mut DappRegistry) -> R) -> R {
    let mut registry = REGISTRY.lock().expect("dApp registry poisoned");
    let loaded = registry.get_or_insert_with(|| DappRegistry::load(default_registry_path()));
    apply(loaded)
}

fn find_dapp(id: &str) -> Option<DappEntry> {
    with_registry(|registry| registry.get(id).cloned())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn open_dapp(cx: Cx, id: &str) {
    let Some(dapp) = find_dapp(id) else {
        toast("dApp 不在注册表中", ToastDuration::Short);
        return;
    };
    cx.dapp_error.set(None);
    cx.dapp_loading.set(true);
    cx.dapp_navigation.set(None);
    cx.dapp_outbox.set(Vec::new());
    cx.dapp_generation
        .set(cx.dapp_generation.get().wrapping_add(1));
    cx.dapp_open.set(Some(dapp.id));
}

fn close_dapp(cx: Cx) {
    crate::dapp_host::revoke_all_hosts();
    cx.dapp_generation
        .set(cx.dapp_generation.get().wrapping_add(1));
    cx.dapp_open.set(None);
    cx.dapp_navigation.set(None);
    cx.dapp_loading.set(false);
    cx.dapp_error.set(None);
    cx.dapp_outbox.set(Vec::new());
}

fn refresh_dapp(cx: Cx, page: &ElementRef, host: Stored<DappHostState>) {
    host.with(DappHostState::revoke);
    crate::dapp_host::revoke_all_hosts();
    cx.dapp_generation
        .set(cx.dapp_generation.get().wrapping_add(1));
    cx.dapp_navigation.set(None);
    cx.dapp_loading.set(true);
    page.stop_loading();
}

fn enqueue_outbox(cx: Cx, message: String) {
    if message.is_empty() {
        return;
    }
    cx.dapp_outbox.update(|queue| queue.push(message));
}

pub fn discover_page(cx: Cx) -> View {
    view()
        .flex_1()
        .gap_5()
        .child(page_header(cx, "发现", "dApp provider · Fiber 钱包能力"))
        .child(dynamic(move || {
            // Rebuild the WebView when the session generation changes.
            let _generation = cx.dapp_generation.get();
            if cx.dapp_open.get().is_some() {
                dapp_host(cx)
            } else {
                dapp_catalog(cx)
            }
        }))
}

fn dapp_catalog(cx: Cx) -> View {
    let registry = with_registry(|registry| registry.dapps().to_vec());
    let warning = with_registry(|registry| registry.warning().map(str::to_owned));

    view()
        .gap_5()
        .child(
            view()
                .gap_2()
                .child(
                    text("钱包能力由宿主执行")
                        .text_size(px(ts::MD))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || pal_fn(&cx).fg),
                )
                .child(
                    text("dApp 通过 window.fiberWallet 请求 connect，并在授权后读取节点、余额、通道与 peer。建发票、付款、dry-run、开/关通道每次都弹出宿主确认，默认拒绝。私钥、Keychain、任意 RPC 与代码执行永不暴露。")
                        .text_size(px(ts::SM))
                        .text_color(move || pal_fn(&cx).fg_muted),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(section_label(cx, "已安装 dApp"))
                .child(
                    text(format!(
                        "注册表 schema v{}",
                        crate::dapp_registry::SCHEMA_VERSION
                    ))
                    .text_size(px(ts::XS))
                    .text_color(move || pal_fn(&cx).fg_muted),
                ),
        )
        .when(warning.is_some(), |container| {
            container.child(
                view()
                    .p_3()
                    .rounded(px(10.0))
                    .bg(move || pal_fn(&cx).card_alt)
                    .child(
                        text(warning.clone().unwrap_or_default())
                            .text_size(px(ts::SM))
                            .text_color(move || pal_fn(&cx).warn),
                    ),
            )
        })
        .child(
            view()
                .flex_wrap()
                .gap_4()
                .children(registry.iter().map(|dapp| dapp_card(cx, dapp.clone()))),
        )
        .child(add_dapp_form(cx))
}

fn dapp_card(cx: Cx, dapp: DappEntry) -> View {
    let id = dapp.id.clone();
    let remove_id = dapp.id.clone();
    let builtin = dapp.builtin;
    let glyph = if dapp.url == SELFTEST_URL {
        "◉"
    } else {
        "◈"
    };
    let host = host_label(&dapp.url);

    view()
        .w(px(300.0))
        .gap_3()
        .p_5()
        .rounded(px(12.0))
        .bg(move || pal_fn(&cx).card)
        .border_1()
        .border_color(move || pal_fn(&cx).border)
        .cursor_pointer()
        .on_press(move || open_dapp(cx, &id))
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
                            text(if builtin { "内置" } else { "自添加" })
                                .text_size(px(ts::XS))
                                .text_color(move || pal_fn(&cx).fg_muted),
                        ),
                ),
        )
        .child(
            text(dapp.name)
                .text_size(px(ts::MD))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || pal_fn(&cx).fg),
        )
        .child(
            text(dapp.description)
                .text_size(px(ts::SM))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(
            text(host)
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .when(!builtin, |container| {
            container.child(
                view().py_1().child(
                    text("移除")
                        .text_size(px(ts::XS))
                        .text_color(move || pal_fn(&cx).danger)
                        .cursor_pointer()
                        .on_press(move |_: &()| {
                            match with_registry(|registry| registry.remove_user(&remove_id)) {
                                Ok(()) => toast("dApp 已移除", ToastDuration::Short),
                                Err(error) => toast(error, ToastDuration::Long),
                            }
                        }),
                ),
            )
        })
}

fn host_label(url: &str) -> String {
    if url == SELFTEST_URL {
        return "宿主内置页 · 无网络".to_owned();
    }
    url.split_once("://")
        .map(|(_, rest)| rest.split('/').next().unwrap_or(rest).to_owned())
        .unwrap_or_else(|| url.to_owned())
}

fn add_dapp_form(cx: Cx) -> View {
    let name = signal(String::new());
    let url = signal(String::new());
    let description = signal(String::new());
    view()
        .p_6()
        .gap_4()
        .rounded(px(12.0))
        .bg(move || pal_fn(&cx).card)
        .border_1()
        .border_color(move || pal_fn(&cx).border)
        .child(
            text("添加 dApp")
                .text_size(px(ts::MD))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || pal_fn(&cx).fg),
        )
        .child(
            text("注册表保存在独立 dapps.json。仅允许 https；本机开发允许 http://localhost、http://127.0.0.1 或 http://[::1]。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
        .child(
            view()
                .flex_row()
                .gap_3()
                .child(labeled_input(cx, "名称", name, "例如 My dApp", false).flex_1())
                .child(labeled_input(cx, "URL", url, "https://example.com", false).flex_1()),
        )
        .child(labeled_input(cx, "描述", description, "这个 dApp 做什么", true))
        .child(
            view()
                .flex_row()
                .gap_3()
                .child(outline_button(
                    cx,
                    "打开注册表目录",
                    || true,
                    || {
                        let path = default_registry_path();
                        let dir = path.parent().unwrap_or(&path);
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("open").arg(dir).spawn();
                        #[cfg(not(target_os = "macos"))]
                        let _ = dir;
                        toast(
                            format!("注册表：{}", path.display()),
                            ToastDuration::Long,
                        );
                    },
                ))
                .child(primary_button(cx, "添加并打开", || true, move || {
                    let result = with_registry(|registry| {
                        registry
                            .add_user(&name.get(), &url.get(), &description.get())
                            .map(|entry| entry.id.clone())
                    });
                    match result {
                        Ok(id) => {
                            name.set(String::new());
                            url.set(String::new());
                            description.set(String::new());
                            open_dapp(cx, &id);
                        }
                        Err(error) => toast(error, ToastDuration::Long),
                    }
                })),
        )
}

fn dapp_host(cx: Cx) -> View {
    let Some(id) = cx.dapp_open.get() else {
        return view();
    };
    let Some(dapp) = find_dapp(&id) else {
        return view()
            .gap_4()
            .child(error_panel(
                cx,
                "这个 dApp 已不存在",
                "可能已从注册表移除。",
            ))
            .child(outline_button(
                cx,
                "返回发现",
                || true,
                move || close_dapp(cx),
            ));
    };

    let host = match DappHostState::new(&dapp, &dapp.url, new_generation()) {
        Ok(state) => stored(state),
        Err(error) => {
            return error_panel(
                cx,
                "无法建立 dApp 会话",
                &format!("{error}；请返回并重新打开。"),
            );
        }
    };
    let page_ref = element_ref();
    let close_ref = page_ref;
    let (outbox_tx, outbox_rx) = std::sync::mpsc::channel::<HostMessage>();
    let outbox_rx = std::sync::Arc::new(Mutex::new(outbox_rx));
    start_outbox_collector(cx, page_ref, outbox_rx.clone());

    let source_html = dapp.inline_html.clone();
    let source_url = dapp.url.clone();
    let nonce = host
        .with(|state| state.session().map(|session| session.nonce.clone()))
        .unwrap_or_default();
    let bootstrap = dapp_protocol::bootstrap_script(&nonce);
    let page_nonce = nonce.clone();
    let page = if let Some(html) = source_html {
        web_view_html(html, Some(&source_url))
    } else {
        web_view(source_url.clone())
    }
    .element_ref(&page_ref)
    .java_script_enabled(true)
    .injected_java_script_before_content(bootstrap)
    .on_load_start({
        let page = page_ref;
        move |nav: &WebNavigation| {
            cx.dapp_loading.set(true);
            cx.dapp_navigation.set(Some(DappNavigation {
                url: nav.url.clone(),
                title: nav.title.clone(),
                can_go_back: nav.can_go_back,
                can_go_forward: nav.can_go_forward,
            }));
            handle_navigation(cx, host, &page, &nav.url);
            let current_nonce =
                host.with(|state| state.session().map(|session| session.nonce.clone()));
            if let Some(current_nonce) = current_nonce
                && current_nonce != page_nonce
            {
                page.inject_java_script(dapp_protocol::document_bootstrap_script(&current_nonce));
            }
            if !host.with(|state| {
                state.session().is_some_and(|session| {
                    session.active.load(std::sync::atomic::Ordering::Acquire)
                })
            }) {
                page.stop_loading();
            }
        }
    })
    .on_load({
        move |nav: &WebNavigation| {
            cx.dapp_loading.set(false);
            cx.dapp_navigation.set(Some(DappNavigation {
                url: nav.url.clone(),
                title: nav.title.clone(),
                can_go_back: nav.can_go_back,
                can_go_forward: nav.can_go_forward,
            }));
            // The bootstrap injected into this new document carries the nonce
            // that existed when the WebView was built. Rotating it here would
            // invalidate that document immediately.
            let loaded_nonce =
                host.with(|state| state.session().map(|session| session.nonce.clone()));
            let origin = host.with(|state| {
                state
                    .session()
                    .map(|session| session.session.origin.as_str().to_owned())
            });
            if let (Some(loaded_nonce), Some(origin)) = (loaded_nonce, origin) {
                enqueue_outbox(
                    cx,
                    dapp_protocol::encode_event(
                        &loaded_nonce,
                        &dapp_protocol::event_connection(),
                        serde_json::json!({
                            "connected": host.with(|state| {
                                state.session().is_some_and(|s| s.session.connected)
                            }),
                            "origin": origin,
                            "url": nav.url,
                            "protocolVersion": dapp_protocol::PROTOCOL_VERSION,
                        }),
                    ),
                );
            }
        }
    })
    .on_navigation_state_change({
        move |nav: &WebNavigation| {
            cx.dapp_loading.set(nav.loading);
            cx.dapp_navigation.set(Some(DappNavigation {
                url: nav.url.clone(),
                title: nav.title.clone(),
                can_go_back: nav.can_go_back,
                can_go_forward: nav.can_go_forward,
            }));
        }
    })
    .on_error(move |message: &str| {
        cx.dapp_loading.set(false);
        cx.dapp_error.set(Some(message.to_owned()));
    })
    .on_message({
        let page = page_ref;
        let tx = outbox_tx;
        move |raw: &str| {
            handle_page_message(cx, host, &page, raw, tx.clone());
        }
    });

    let delivery_page = page_ref;
    let delivery_cx = cx;
    effect(move || {
        let mut queued = Vec::new();
        delivery_cx.dapp_outbox.update(|queue| {
            queued = std::mem::take(queue);
        });
        for message in queued {
            delivery_page.post_message(message.clone());
        }
    });

    view()
        .flex_1()
        .gap_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .child(outline_button(
                    cx,
                    "返回",
                    || true,
                    move || {
                        close_ref.stop_loading();
                        close_dapp(cx);
                    },
                ))
                .child(
                    view()
                        .flex_1()
                        .child(
                            text(dapp.name.clone())
                                .text_size(px(ts::MD))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || pal_fn(&cx).fg),
                        )
                        .child(
                            text(dapp.url.clone())
                                .text_size(px(ts::XS))
                                .text_color(move || pal_fn(&cx).fg_muted),
                        ),
                )
                .child(outline_button(
                    cx,
                    "后退",
                    || true,
                    move || close_ref.go_back(),
                ))
                .child(outline_button(
                    cx,
                    "刷新",
                    || true,
                    move || refresh_dapp(cx, &close_ref, host),
                ))
                .child(outline_button(
                    cx,
                    "关闭",
                    || true,
                    move || {
                        close_ref.stop_loading();
                        close_dapp(cx);
                    },
                )),
        )
        .child(
            view()
                .flex_1()
                .min_h(px(520.0))
                .rounded(px(12.0))
                .overflow_hidden()
                .border_1()
                .border_color(move || pal_fn(&cx).border)
                .bg(move || pal_fn(&cx).card)
                .child(page.flex_1()),
        )
        .when(cx.dapp_error.get().is_some(), |container| {
            container.child(error_panel(
                cx,
                "dApp 宿主错误",
                &cx.dapp_error.get().unwrap_or_default(),
            ))
        })
        .child(
            text("请求经过宿主审批；dApp 页面自身仍可发起网络请求。")
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

fn start_outbox_collector(
    cx: Cx,
    page: ElementRef,
    rx: std::sync::Arc<Mutex<std::sync::mpsc::Receiver<HostMessage>>>,
) {
    let task = spawn(async move {
        loop {
            let message = {
                let rx = rx.lock().expect("dApp outbox poisoned");
                rx.recv_timeout(Duration::from_millis(100))
            };
            match message {
                Ok(HostMessage::Response(message)) | Ok(HostMessage::Event(message)) => {
                    enqueue_outbox(cx, message);
                }
                Ok(HostMessage::NavigationRevoked(message)) => {
                    enqueue_outbox(cx, message);
                    page.stop_loading();
                }
                Ok(HostMessage::Fatal(message)) => {
                    cx.dapp_error.set(Some(message));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    cleanup(move || task.cancel());
}

fn handle_navigation(cx: Cx, host: Stored<DappHostState>, page: &ElementRef, url: &str) {
    let mut revoked = false;
    host.with(|state| {
        let same_origin = state
            .session()
            .is_some_and(|session| session.session.origin.allows(url));
        if !same_origin {
            state.revoke();
            revoked = true;
            return;
        }
        if state.begin_document(url).is_err() {
            state.revoke();
            revoked = true;
        }
    });
    if revoked {
        page.stop_loading();
        cx.dapp_error.set(Some(format!(
            "已撤销会话并停止加载：导航到非注册 origin {url}"
        )));
        cx.dapp_outbox.set(Vec::new());
    }
}

fn handle_page_message(
    cx: Cx,
    host: Stored<DappHostState>,
    page: &ElementRef,
    raw: &str,
    tx: std::sync::mpsc::Sender<HostMessage>,
) {
    let request = match dapp_protocol::parse_browser_message(raw) {
        Ok(request) => request,
        Err(error) => {
            let id = raw_id(raw).unwrap_or_else(|| "invalid".to_owned());
            enqueue_outbox(
                cx,
                dapp_protocol::encode_response(&dapp_protocol::response_error(&id, error)),
            );
            return;
        }
    };
    if matches!(request, DappRequest::Event { .. }) {
        return;
    }
    let Some(current_url) = cx.dapp_navigation.get().map(|navigation| navigation.url) else {
        let id = request.request_id().unwrap_or("invalid");
        enqueue_outbox(
            cx,
            dapp_protocol::encode_response(&dapp_protocol::response_error(
                id,
                RpcError::new(
                    error_code::UNAVAILABLE,
                    "the native page URL is not known yet",
                ),
            )),
        );
        return;
    };
    if let Err(error) = host.with(|state| state.set_current_url(&current_url)) {
        host.with(DappHostState::revoke);
        page.stop_loading();
        let id = request.request_id().unwrap_or("invalid");
        enqueue_outbox(
            cx,
            dapp_protocol::encode_response(&dapp_protocol::response_error(id, error)),
        );
        return;
    }
    let dispatch = host.with(|state| state.dispatch(request, now_ms()));
    match dispatch {
        Ok(Dispatch::Reply(response)) => {
            enqueue_outbox(cx, dapp_protocol::encode_response(&response))
        }
        Ok(Dispatch::Execute {
            lease,
            method,
            action,
            request_id,
        }) => submit_execution(lease, method, action, request_id, tx),
        Ok(Dispatch::Approval { pending, lease }) => {
            prompt_approval(cx, host, pending, lease, tx);
        }
        Err(error) => {
            let id = request_id_for_error(raw);
            enqueue_outbox(
                cx,
                dapp_protocol::encode_response(&dapp_protocol::response_error(&id, error)),
            );
        }
    }
}

fn request_id_for_error(raw: &str) -> String {
    raw_id(raw).unwrap_or_else(|| "invalid".to_owned())
}

fn prompt_approval(
    cx: Cx,
    host: Stored<DappHostState>,
    pending: dapp_protocol::PendingApproval,
    _lease: SessionLease,
    tx: std::sync::mpsc::Sender<HostMessage>,
) {
    let token = pending.grant.token;
    let text = approval_text(&pending.summary);
    let dialog = alert_dialog()
        .title(pending.summary.title.clone())
        .description(text)
        .cancel("拒绝");
    let dialog = if pending.summary.destructive {
        dialog.destructive("批准并执行")
    } else {
        dialog.action("批准")
    };
    let result = dialog.show();
    spawn(async move {
        let timer = set_timeout(Duration::from_millis(dapp_protocol::APPROVAL_TIMEOUT_MS), {
            let host = host;
            let cx = cx;
            move || {
                let expired = host.with(|state| state.expire(now_ms()));
                if let Some(response) = expired {
                    enqueue_outbox(cx, dapp_protocol::encode_response(&response));
                }
            }
        });
        let approved = matches!(result.await, Ok(Some(answer)) if answer != "拒绝");
        timer.cancel();
        let outcome = host.with(|state| {
            if approved {
                state.approve(token, now_ms())
            } else {
                Err(Box::new(state.reject()))
            }
        });
        match outcome {
            Ok(Dispatch::Execute {
                lease,
                method,
                action,
                request_id,
            }) => {
                if method == ProviderMethod::Connect {
                    match host.with(|state| state.authorize_connect(now_ms())) {
                        Ok(value) => {
                            enqueue_outbox(
                                cx,
                                dapp_protocol::encode_response(&dapp_protocol::response_ok(
                                    &request_id,
                                    value,
                                )),
                            );
                            if let Some(session) = host.with(|state| state.session().cloned()) {
                                enqueue_outbox(
                                    cx,
                                    dapp_protocol::encode_event(
                                        &pending.nonce,
                                        &dapp_protocol::event_connection(),
                                        serde_json::json!({
                                            "connected": true,
                                            "dappId": session.session.dapp_id,
                                            "origin": session.session.origin.as_str(),
                                            "protocolVersion": dapp_protocol::PROTOCOL_VERSION,
                                        }),
                                    ),
                                );
                            }
                        }
                        Err(error) => enqueue_outbox(
                            cx,
                            dapp_protocol::encode_response(&dapp_protocol::response_error(
                                &request_id,
                                error,
                            )),
                        ),
                    }
                } else {
                    submit_execution(lease, method, action, request_id, tx);
                }
            }
            Ok(Dispatch::Reply(response)) => {
                enqueue_outbox(cx, dapp_protocol::encode_response(&response))
            }
            Ok(Dispatch::Approval { .. }) => enqueue_outbox(
                cx,
                dapp_protocol::encode_response(&dapp_protocol::response_error(
                    &pending.request_id,
                    RpcError::new(
                        error_code::REQUEST_IN_PROGRESS,
                        "unexpected nested approval",
                    ),
                )),
            ),
            Err(response) => enqueue_outbox(cx, dapp_protocol::encode_response(&response)),
        }
    });
}

fn submit_execution(
    lease: SessionLease,
    method: ProviderMethod,
    action: WalletAction,
    request_id: String,
    tx: std::sync::mpsc::Sender<HostMessage>,
) {
    if let Err(error) = executor().submit(lease, method, action, request_id.clone(), tx.clone()) {
        tracing::error!(%error, "failed to submit dApp execution");
        let response = dapp_protocol::response_error(
            &request_id,
            RpcError::new(error_code::UNAVAILABLE, error),
        );
        let _ = tx.send(HostMessage::Response(dapp_protocol::encode_response(
            &response,
        )));
    }
}

fn error_panel(cx: Cx, title: &'static str, message: &str) -> View {
    view()
        .gap_1()
        .p_4()
        .rounded(px(10.0))
        .bg(move || pal_fn(&cx).card_alt)
        .border_1()
        .border_color(move || pal_fn(&cx).danger)
        .child(
            text(title)
                .text_size(px(ts::SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || pal_fn(&cx).danger),
        )
        .child(
            text(message.to_owned())
                .text_size(px(ts::XS))
                .text_color(move || pal_fn(&cx).fg_muted),
        )
}

fn raw_id(raw: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(raw).ok()?;
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_ids_are_recovered_only_when_safe() {
        assert_eq!(
            raw_id(r#"{"id":"abc","method":"x"}"#).as_deref(),
            Some("abc")
        );
        assert!(raw_id(r#"{"id":"\n"}"#).is_none());
        assert!(raw_id("not json").is_none());
    }
}
