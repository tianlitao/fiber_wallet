# Architecture

## Decision summary

The NSH host owns presentation and platform UI. Rust owns wallet/domain logic
and the local Fiber node supervisor; the host calls the Rust core directly.
Fiber is embedded on native platforms and represented by a WASM edge node on Web.

This deliberately separates:

1. **UI lifecycle** — NSH host and native platform lifecycle.
2. **Node lifecycle** — Rust supervisor with an explicit state machine.
3. **Wallet state** — Rust domain model and persistence.
4. **Transport** — direct Rust calls, not localhost HTTP, on native platforms.

## Runtime model

### Desktop

- App process owns one node supervisor.
- FNN uses the app's data directory and SQLite.
- Node starts after the window is ready.
- Closing the app performs an ordered shutdown.
- Optional tray mode can keep the process alive; that is a later product mode.

### iOS and Android

- Node starts only while the app is active.
- Entering background triggers an ordered stop.
- Returning to foreground starts a new node instance against the same database.
- A stale lock or unclean shutdown must be recoverable from SQLite/WAL.
- No guarantee of receiving while suspended or killed.

### Web

- The browser cannot host the native FNN P2P stack.
- Web uses `fiber-js`/`fiber-wasm` as an edge node while the page is open.
- The Web adapter is separate from the native FNN adapter.
- The UI must label this mode as an edge node, not a public full node.

## Components

```text
nsh_app (macOS) / nsh_app_ios (iOS)
  WalletBridge (tokio runtime, UiState)
  fiber_wallet_core::api

rust/fiber_wallet_core
  NodeSupervisor
  NodeBackend
    FnnBackend       (M1)
    WasmBackend      (M2, Web bridge)
    SimulatedBackend (M0)
  WalletStore       (M1)
  SecretStore       (platform adapters, M2)
```

## Node state machine

```text
Stopped --start--> Starting --ready--> Running
   ^                   |                 |
   |                   v                 v
   +--------------- Failed <---------- Stopping
                                      |
                                      +--> Stopped
```

Rules:

- `start` is idempotent while `Starting` or `Running`.
- `stop` is idempotent while `Stopping` or `Stopped`.
- A database lock is acquired before `Starting`.
- A node is not considered able to receive until `Running`.
- Failed shutdown must be visible to the user; it must not be silently retried.

## Startup sequence

1. Validate config and data directory.
2. Acquire single-instance database lock.
3. Open SQLite and run migrations.
4. Initialize Fiber/CKB contract context.
5. Start the node supervisor and backend actors.
6. Wait for readiness and network sync threshold.
7. Publish `Running` plus node identity.

## Shutdown sequence

1. Reject new payments.
2. Allow in-flight operations a bounded drain period.
3. Stop network and watchtower actors.
4. Flush the store and close SQLite.
5. Release the lock.
6. Publish `Stopped`.

The mobile background window is short. The implementation must prefer a
consistent database over waiting indefinitely for an in-flight payment.

## Identity and storage

Each installation has independent:

- CKB wallet key material
- Fiber node key material
- SQLite database
- channel state
- backups

Do not copy a live Fiber database between devices. Multi-device state sharing is
out of scope and would require a hosted LSP or remote-signing protocol.

## Security boundaries

- `nsh/storage`, SharedPreferences, and plain JSON are forbidden for secrets.
- iOS uses Keychain; Android uses Keystore; desktop uses the OS keychain where available.
- The database is encrypted where the platform allows it; secrets are never written to logs.
- RPC listeners bind to loopback only if they exist at all. The native app should
  call an in-process Rust API instead of exposing a local HTTP port.
- Payment retries are idempotent by payment hash. A UI timeout is not proof of failure.

## Fiber integration

Pin a known Fiber release and do not follow `develop` in production. The first
native backend targets Fiber v0.9.1 with the `sqlite` feature. The Fiber adapter
must be isolated in `fiber-node-host`, with no Fiber types leaking into the
host UI.

The upstream `fiber-bin` startup logic is reference material, not a mobile API.
M1 will extract a stable `start/stop/status/events` wrapper and reuse the same
actor/store initialization as the reference node.

## What is intentionally not decided in M0

- Exact remote-signing design
- Hosted LSP integration
- Push-notification delivery
- Multi-device account model
- Mainnet release policy
