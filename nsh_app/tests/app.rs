//! Headless tests of the wallet shell (docs: "Building with Rust" → Testing).
//!
//! The root is the real app: it starts the embedded Fiber node on background
//! threads when the bridge initializes, so keep assertions to what the shell
//! mounts rather than node lifecycle.

use nsh_engine::{EngineConfig, Root};
use nsh_core::ui::prelude::*;
use nsh_testing::Headless;

#[component]
fn Probe() -> impl IntoElement {
    view()
        .flex_1()
        .child(text("probe ok"))
}

#[test]
fn probe_engine_mounts() {
    let mut app = Headless::start(EngineConfig::new(Root::of::<Probe>()))
        .expect("headless engine starts");
    app.wait_for("probe ok").expect("probe mounts");
}

#[test]
fn wallet_shell_mounts() {
    // Set before any thread spawns; test binaries are single-threaded here.
    unsafe {
        std::env::set_var("FIBER_WALLET_TEST", "1");
    }
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<nsh_app_fiber_wallet_nsh::App>()))
            .expect("headless engine starts");
    app.wait_for("Fiber Wallet")
        .expect("the title bar mounts");
    app.wait_for("首页").expect("the sidebar mounts");
    app.wait_for("总资产").expect("the home page mounts");
}
