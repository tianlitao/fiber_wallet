# Roadmap

Status markers updated 2026-10-25. See
[implementation-status.md](implementation-status.md) for verification details.

## M0 — vertical slice and contract — **done**

Goal: prove the host/Rust app-lifecycle boundary.

- The NSH shell builds on macOS.
- Rust core has a tested node lifecycle state machine.
- The shell can start and stop the simulated local node through the bridge.
- The shell automatically stops the node when the app is backgrounded.
- UI clearly distinguishes online, offline, and error states.
- Architecture and toolchain are documented.

## M1 — Fiber native backend on macOS — **done**

- Pin Fiber v0.9.1. ✅ (`vendor/fiber`)
- Implement `FnnBackend` with SQLite and a dedicated data directory. ✅
- Start/stop the node in-process on macOS. ✅
- Expose node identity, peers, channels, invoices, and payments. ✅
  (`wallet_list_peers`/`wallet_connect_peer`, `wallet_list_channels`,
  `wallet_new_invoice`, `wallet_list_payments`/`wallet_get_payment`)
- Add restart/crash recovery tests. ✅ (`make integration-macos`,
  `make recovery-smoke`)
- Add a testnet smoke test. 🔶 startup exercises CKB testnet RPC + P2P;
  funded invoice/payment/close still needs faucet CKB (see status doc).

## M2 — mobile lifecycle and secure storage — **mostly done**

- iOS arm64 build and real-device test. 🔶 simulator build + integration test
  pass; device signing not executed.
- Android NDK build and real-device test. 🔶 arm64 build + emulator
  integration test pass; no device attached.
- Keychain/Keystore adapters. ❌ dev password fallback still in place.
- Background stop and foreground restart. ✅ (bridge lifecycle handling,
  exercised by unit tests).
- Database lock and unclean-shutdown recovery. ✅ flock + recovery smoke.
- QR scan through a native plugin or platform channel. ❌

## M2.5 — in-app dApp provider — **done**

- dApp registry (`dapps.json`, builtin protocol-selftest page, user-added
  https/localhost entries). ✅
- Native WebView host with nonce-bound `window.fiberWallet` injection,
  origin-bound sessions, navigation whitelist with revocation. ✅
- Provider surface: `fiber_connect`, `fiber_getNodeInfo/getBalance/
  listChannels/listPeers`, `fiber_createInvoice`, `fiber_sendPayment` +
  `fiber_dryRunPayment` (invoice/keysend), `fiber_openChannel`,
  `fiber_closeChannel`, `fiber_getPayment`. ✅
- Default-deny approval dialog per write, approval expiry, serialized
  executor; dApps never touch keys/Keychain/arbitrary RPC. ✅

## M3 — Web edge node — **deferred**

Deferred by product decision ("web 端可以先不实现"). The wallet-side web
story shipped instead is the dApp provider in 发现 (MetaMask-style
`window.fiberWallet` over a sandboxed native WebView); a browser-hosted edge
node remains future work.

- `fiber-js` in a browser host/worker.
- IndexedDB persistence.
- Multi-tab leader election.
- COOP/COEP deployment configuration.
- Explicit offline/edge-node UI.

## M4 — release hardening

- Database migration/backup/restore flows.
- Crash-safe payment idempotency.
- Observability and support diagnostics.
- Packaging, signing, notarization, store review.
- Security audit before mainnet funds.
