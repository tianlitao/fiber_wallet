# Fiber Wallet vendor notes

Upstream: nervosnetwork/fiber
Tag: v0.9.1
Vendored: 2026-10-23

## Local patches

1. `crates/fiber-lib/src/config.rs`
   - Make the string-based `Config::parse_from_str` parser available to native
     embedded hosts. Upstream 0.9.1 gates it behind `target_arch = "wasm32"`,
     although the implementation is target-neutral.

Keep this file and the patch minimal. Rebase against the next pinned Fiber
release instead of drifting the vendored source.
