# Fiber Wallet

Local-first Fiber wallet on CKB. The UI is being implemented with
[NSH](https://nsh.dev/) (Rust-native UI); the wallet and node logic lives in a
framework-agnostic Rust core in `core/`.

## Product boundary

A wallet instance starts its own local Fiber node when the app is running and
stops the node when the app leaves the foreground. It does not promise 24/7
availability, background receiving, or shared channel state across devices.

Each installation is an independent wallet/node instance with its own data
directory and keys.

## Layout

```text
core/            Rust wallet core (framework-agnostic crate)
  src/node/      Embedded Fiber node lifecycle (FNN v0.9.1 + SQLite + watchtower)
  src/types.rs   Node/wallet DTOs shared by hosts
  src/api/       Compatibility paths the NSH shell was wired against
  src/bin/       Recovery smoke binary
vendor/fiber/    Pinned nervosnetwork/fiber v0.9.1 (see FIBER_WALLET_PATCH.md)
nsh_app/         The NSH macOS shell (UI host); depends on ../core
nsh_app_ios/     The NSH iOS shell: bottom-tab UI over the same bridge, Keychain-backed wallet password, AVFoundation QR scanner
nsh_app_android/ The NSH Android shell: bottom-tab UI over the same bridge, Keystore-backed wallet password, Camera2 QR scanner
docs/            Architecture, fiber integration, toolchain, status
scripts/         Recovery smoke driver, Android toolchain bootstrap and build/permission helpers
```

## The core crate

`fiber_wallet_core` exposes:

- `NodeMachine` — embed one local Fiber node per process: `new(network,
  data_dir, secret_password)`, `start()`, `stop()`, `snapshot()`,
  `mark_failed()`. Start/stop are idempotent; restarts inside one process are
  supported (Fiber's process-global chain/contract initializers are skipped
  after the first successful start).
- Wallet operations on a running node: `wallet_node_info()`,
  `wallet_list_peers()`/`wallet_connect_peer()`, `wallet_open_channel()`,
  `wallet_list_channels()`, `wallet_close_channel()`, `wallet_new_invoice()`,
  `wallet_list_payments()`, `wallet_send_payment()`, `wallet_get_payment()`.
- Payments: `wallet_send_payment_request()` takes a `WalletSendPaymentRequest`
  (invoice or keysend with target pubkey/amount/max fee/timeout; `dry_run`
  returns a quote without creating a payment record) and waits for Fiber's
  terminal `Success`/`Failed`; `wallet_poll_payment_terminal()` re-awaits an
  already-submitted payment. UDT invoices are rejected — native CKB only.
- Channel closing uses `ChannelCloseRequest` / `ChannelCloseMode`:
  `Cooperative` derives the wallet's default funding script as FNN's
  `close_script`; `Force` broadcasts the latest commitment transaction and
  deliberately omits `close_script`/`fee_rate`. The RPC result is a submission,
  not proof of final closure; poll `wallet_list_channels()` until the channel
  reports `Closed`.
- On-chain CKB transfers (`wallet_onchain_transfer()` +
  `wallet_onchain_transfer_status()`): testnet-only, funding-lock cells,
  fee cap re-checked after signing; broadcast accepted ≠ confirmed.
- Safety rails: an exclusive flock on `<data-dir>/wallet.lock` (one node per
  data directory), password re-encryption migration from the legacy
  development password, and stable node identity across unclean exits.
- `recovery_smoke` — hard-exit recovery verification (`make recovery-smoke`).

Two NSH shells live beside it: `nsh_app` (macOS, icon sidebar) and
`nsh_app_ios` (iOS, bottom tabs). Both reuse the bridge, theme, widgets, and
pages from `nsh_app`; the iOS shell additionally loads the wallet password
from the iOS Keychain and exports it through `FIBER_SECRET_KEY_PASSWORD`
before the bridge initializes. Run the iOS shell in the simulator with:

```sh
cd nsh_app_ios && nsh run --target ios --device "iPhone 17 Pro"
```

The macOS shell's 发现 page hosts dApps as a MetaMask-style provider: dApps
connect through an injected, nonce-bound `window.fiberWallet` and may read
node/balance/channel/peer state after an approved connection; every write
(invoice, payment, dry-run, channel open/close) opens a fresh default-deny
approval dialog. dApps never see key material or arbitrary RPC. See
`nsh_app/src/dapp_host.rs`, `dapp_protocol.rs`, and `dapp_registry.rs`.

Hosts provide the secret password from platform secure storage; this crate
never stores secrets itself. iOS reads the Keychain (`nsh_app_ios/src/keychain.rs`)
and Android the hardware-backed Keystore (`nsh_app_android/src/keystore.rs`);
both export it through `FIBER_SECRET_KEY_PASSWORD` before the bridge
initializes. 交易页's 扫一扫 opens the platform camera scanner through the
shared `scan` hook (`nsh_app/src/scan.rs`): AVFoundation on iOS, Camera2 NDK
plus a runtime-loaded dex bridge on Android (`nsh_app_android/src/scanner.rs`).

## Android shell

Run the Android shell on the emulator (boots it, builds both ABIs, installs
and launches):

```sh
scripts/build-android.sh nsh_app_android run --target android
```

The nsh CLI generates the app manifest from a template that only declares
INTERNET, so the scanner's CAMERA permission is added to the packaged APK by
`scripts/patch_apk_camera.sh` (re-links the manifest, re-signs with the
Android debug key); reinstall the patched APK and `pm grant` the permission
before exercising 扫一扫. See the script headers for the details.

`src/api/` re-exports the same items under the module paths the NSH shell was
originally wired against (`api::node::init_local_node`, `api::wallet::*`); it
is a thin alias of `types` plus the process-global node accessors.

## Commands

```sh
make check           # fmt + tests + clippy for core/
make recovery-smoke  # unclean-exit recovery verification
```

## References

- [docs/architecture.md](docs/architecture.md) — product/runtime architecture
- [docs/fiber-integration.md](docs/fiber-integration.md) — how FNN is embedded,
  including the restart-semantics discoveries
- [docs/toolchain.md](docs/toolchain.md) — pinned toolchain and machine notes
- [docs/implementation-status.md](docs/implementation-status.md) — verified
  work to date
- [docs/roadmap.md](docs/roadmap.md)
