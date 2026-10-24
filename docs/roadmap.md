# Roadmap

Status markers updated 2026-10-24. See
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
- Expose node identity, peers, channels, invoices, and payments. 🔶 node
  identity exposed; peers/channels/invoices/payments APIs land with the first
  funded testnet flow.
- Add restart/crash recovery tests. ✅ (`make integration-macos`,
  `make recovery-smoke`)
- Add a testnet smoke test. 🔶 startup exercises CKB testnet RPC + P2P;
  invoice/payment needs a funded channel.

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

## M3 — Web edge node

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
