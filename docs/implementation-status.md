# Implementation status

Date: 2026-10-25

## Verified

`core/` is a framework-agnostic Rust crate verified with `make check` (fmt +
tests + clippy `-D warnings`) and `make recovery-smoke`:

- Embedded Fiber node lifecycle (`NodeMachine`): FNN v0.9.1 (vendored) +
  SQLite + watchtower, idempotent start/stop, in-process restarts.
- Wallet operations on a running node: node info (funding address +
  balances), peers (list/connect), channel open, cooperative/force channel
  close, invoice creation, payments (send/list/get), same-asset keysend with
  dry-run quotes, and terminal-state polling.
- Channel close uses `ChannelCloseRequest` / `ChannelCloseMode`: the RPC
  result is a submission, not proof of final closure; hosts poll
  `wallet_list_channels()` until the channel reports `Closed`
  (`nsh_app/src/channel_close.rs`).
- Payments: `WalletSendPaymentRequest` covers invoice and keysend (target
  pubkey + amount + max fee + timeout), `dry_run` returns a
  `WalletSendPaymentResult::Quote` without creating a payment record, real
  sends wait for Fiber's terminal `Success`/`Failed` inside the request
  timeout, and `wallet_poll_payment_terminal` re-awaits a submitted payment.
  UDT invoices are rejected (native CKB only).
- On-chain CKB transfers (`core/src/onchain.rs`): builds, signs, and
  broadcasts a testnet transaction from the wallet's funding lock via
  ckb-sdk; testnet-only address validation, fee cap re-checked after signing,
  broadcast accepted ≠ confirmed (`wallet_onchain_transfer` +
  `wallet_onchain_transfer_status`).
- Restart safety: Fiber's process-global chain-hash and contracts-context
  initializers are skipped on the second start; in-process chain switching is
  rejected.
- Safety rails: exclusive data-directory flock; password re-encryption
  migration; unclean-exit recovery smoke.
- `nsh_app/` (macOS shell) is wired to `core/`: node lifecycle UI, 首页 /
  交易 / 发现 / 设置 pages, QR rendering, theming, proxy settings.
  - 交易: real Fiber same-asset keysend transfer (dry-run quote → explicit
    confirm → real send → terminal state), on-chain CKB transfer with a
    two-step confirmation and post-broadcast status polling, invoice receive
    (QR + session history), invoice pay, channel open/close UI. Cross-asset
    CKB↔rUSDT exchange is shown as explicitly unavailable (no AMM/order-book
    provider; no simulated rates anywhere).
  - 发现: dApp provider host. Registry (`dapps.json` + builtin protocol
    selftest page), native WKWebView with a nonce-bound injected
    `window.fiberWallet` provider, origin-bound sessions, navigation
    whitelist with revocation, default-deny approval dialogs for every write
    (connect/invoice/payment/dry-run/open/close channel) with expiry, and a
    serialized executor. dApps never see keys, the Keychain, arbitrary RPC,
    or host-side script execution.
  - 设置: theme (mode + 5 accents), HTTP(S) proxy, keystore password change
    (`vault.rs`), backup export/import.
- `nsh_app_ios/` shares the bridge/pages and compiles for the iOS simulator;
  the wallet password comes from the iOS Keychain.
- The core was exercised end to end on macOS, the iOS simulator, and the
  Android emulator with the real FNN (macOS integration test:
  start/stop/restart with a real node id and `fnn v0.9.1 (sqlite)`; Android
  required explicit NDK compiler-rt builtins for 32-bit ABIs, see
  docs/toolchain.md).
- macOS smoke: the debug shell (`cargo run`) launches, mounts the UI, loads
  config, and stays up with no crash. Node-state rendering is reactive:
  every page component reads the root's 400 ms `cx.ui` mirror signal (via
  `Cx::ui_state()`), so the top-right status pill and the sidebar line track
  启动中/在线/停止中/故障 live (an earlier build rendered the pill from an
  untracked Mutex snapshot, freezing it on stale phases; fixed 2026-10-25).
  首页 → 节点事件 renders a rolling console of the app's actual tracing
  output (a second `fmt` layer captures into an in-memory ring buffer that
  rides the same 400 ms mirror; ANSI codes stripped, level-colored, newest
  line on top, wrapped inside a shrinkable card; falls back to phase events
  on hosts without the capture layer), and every button-triggered operation
  flashes a one-shot success/failure toast (开通道/关通道/发票/支付/dry-run/
  链上转账/节点启停/密码轮换/导出导入).
  QR canvases (首页 hero address, 收款 invoice) live outside any re-running
  region: they are created once by a reactive `when` on data presence and
  repaint from their own reactive paint closure — a canvas built inside a
  re-running `dynamic` is destroyed before it paints and renders blank.
- Known machine facts: system volume is nearly full — all toolchains live
  under `/Volumes/tlt/.tooling`; Xcode DerivedData is symlinked there; the
  Gradle JVM needs proxy properties (see docs/toolchain.md).

## Not yet done

- Funded testnet end-to-end runs from this workspace (faucet CKB): opening a
  funded channel, a real keysend over it, and a real funded close. All the
  code paths and UI exist; they need a funded testnet address to exercise.
- M3 Web edge node: deferred by product decision (the in-app dApp provider
  above is the shipped web-story for now).
- QR scan via camera (desktop QR display works).
- Windows/Linux builds; iOS/Android host integration beyond the simulator
  and emulator.
- M4 release hardening (migrations, observability, packaging, audit).
