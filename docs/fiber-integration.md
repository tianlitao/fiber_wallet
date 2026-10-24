# Fiber native integration plan

## Pinned target

M1 targets Fiber **v0.9.1** and Rust 1.95+ with:

```toml
features = ["sqlite", "watchtower"]
```

Do not track Fiber `develop` in a wallet build. Pin a tag or commit and update
through an explicit migration/release process.

## Why SQLite

- Mobile-friendly embedded database.
- Avoids RocksDB's mobile build complexity.
- Fiber v0.9.1 already exposes the `sqlite` feature.
- The store can be snapshotted for backup and recovery tests.

## Integration shape

```text
NSH shell (nsh_app / nsh_app_ios)
  └─ fiber_wallet_core::api
       └─ NodeMachine
            └─ FnnLocalBackend
                 ├─ Fiber actors
                 ├─ SQLite store
                 ├─ CKB RPC client
                 └─ watchtower
```

`fnn` types must not appear in `api/`. The bridge surface stays wallet-owned and
small: start, stop, status, peers, channels, invoices, payments, events.

## M1 tasks

1. Vendor Fiber v0.9.1 as a pinned dependency. **done**
2. Add a `fnn-sqlite` feature path that compiles on macOS. **done**
3. Extract the native startup sequence from fiber's `fiber-bin/src/main.rs`. **done**
4. Reuse the lifecycle from `fiber-wasm/src/lib.rs` where it is platform-neutral. **n/a** — the native startup path is self-contained in `node/fnn.rs`.
5. Open the store under the app support directory. **done**
6. Acquire a process-level lock before startup. **done** — exclusive `flock` on `<data-dir>/wallet.lock`, held by `RunningNode` (see `acquire_data_dir_lock`).
7. Expose node identity and readiness only after actors are initialized. **done**
8. Implement ordered stop and store flush. **done**
9. Add a testnet smoke test. **partial** — the integration test brings up the full stack against testnet: CKB RPC (contracts context resolution), SQLite store, P2P listener. Invoice/payment flows need a funded testnet channel and stay open.
10. Test an unclean exit followed by restart. **done** — `make recovery-smoke` (`scripts/recovery_smoke.sh` + `src/bin/fnn_recovery_smoke.rs`): hard-exit without stopping, then restart on the same data directory and verify recovery and stable node identity.

### Restart semantics discovered in M1

Fiber v0.9.1 keeps two process-globals that refuse re-initialization:

- `fnn::fiber::network::init_chain_hash` panics on the second call.
- `fnn::ckb::contracts::try_init_contracts_context` returns `ContextAlreadyInitialized`.

An embedded node that starts, stops, and starts again inside one process must
skip both after the first successful start. `node/fnn.rs` guards them with
`get_chain_hash()` and a `CONTRACTS_CONTEXT_READY` flag; switching chains
within one process is rejected instead.

## Mobile follow-up

- Compile the same backend with `aarch64-apple-ios`.
- Compile with `aarch64-linux-android` using the Android NDK.
- Add Keychain/Keystore key providers.
- Stop the node on iOS `background` and Android `onStop`.
- Verify SQLite WAL recovery and database locking after force-kill.

## Web follow-up

Do not attempt to compile the native FNN stack for Web. Implement a separate
`fiber-js`/`fiber-wasm` adapter behind the same wallet API. The
browser adapter is an edge node and is available only while the page is open.
