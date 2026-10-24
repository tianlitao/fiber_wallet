# Implementation status

Date: 2026-10-24

## Verified

`core/` is a framework-agnostic Rust crate verified with `make check` and
`make recovery-smoke`:

- Embedded Fiber node lifecycle (`NodeMachine`): FNN v0.9.1 (vendored) +
  SQLite + watchtower, idempotent start/stop, in-process restarts.
- Wallet operations: node info (funding address + balances), channels,
  invoice creation, payments (send/list/get).
- Restart safety: Fiber's process-global chain-hash and contracts-context
  initializers are skipped on the second start; in-process chain switching is
  rejected.
- Safety rails: exclusive data-directory flock; migration that re-encrypts
  key files from the legacy development password to the wallet password
  (raw/unrecognized files are left alone; undecryptable encrypted files fail
  loudly); unclean-exit recovery smoke.
- The core was exercised end to end on macOS, the iOS simulator, and the
  Android emulator with the real FNN (macOS integration test:
  start/stop/restart with a real node id and `fnn v0.9.1 (sqlite)`; Android
  required explicit NDK compiler-rt builtins for 32-bit ABIs, see
  docs/toolchain.md).
- Recovery smoke: node starts, hard-exits, restarts with the same identity.
- `nsh_app/` (macOS shell) is wired to `core/` with the node lifecycle UI,
  wallet screens (home/trade/discover/settings), QR rendering, theming, and
  proxy settings; `nsh_app_ios/` runs the same bridge on the iOS simulator.
- Known machine facts: system volume is nearly full — all toolchains live
  under `/Volumes/tlt/.tooling`; Xcode DerivedData is symlinked there; the
  Gradle JVM needs proxy properties (see docs/toolchain.md).

## Not yet done

- Keychain/Keystore adapters for the NSH host (dev password fallback still
  in place).
- Funded testnet payment flow (needs faucet CKB; invoice creation works
  without funds).
- Windows/Linux builds; iOS/Android host integration.
- M4 release hardening (migrations, observability, packaging, audit).
