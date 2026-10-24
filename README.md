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
nsh_app_ios/     The NSH iOS shell: bottom-tab UI over the same bridge, Keychain-backed wallet password
docs/            Architecture, fiber integration, toolchain, status
scripts/         Recovery smoke driver, Android toolchain bootstrap
```

## The core crate

`fiber_wallet_core` exposes:

- `NodeMachine` — embed one local Fiber node per process: `new(network,
  data_dir, secret_password)`, `start()`, `stop()`, `snapshot()`,
  `mark_failed()`. Start/stop are idempotent; restarts inside one process are
  supported (Fiber's process-global chain/contract initializers are skipped
  after the first successful start).
- Wallet operations on a running node: `wallet_node_info()`,
  `wallet_list_channels()`, `wallet_new_invoice()`, `wallet_list_payments()`,
  `wallet_send_payment()`, `wallet_get_payment()`.
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

Hosts provide the secret password from platform secure storage; this crate
never stores secrets itself.

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
