//! Headless tests of the wallet shell (docs: "Building with Rust" → Testing).
//!
//! `FIBER_WALLET_TEST=1` runs the shell without the bridge, the embedded
//! node, or desktop commands.
//!
//! KNOWN ISSUE (upstream): nsh-testing 0.2.0's headless driver intermittently
//! never finishes the engine's first commit — even a root of one static
//! `text` hangs `Headless::start` on some runs (`State::commit` waits on a
//! semaphore forever), and the same binary passes on the next run. The real
//! windowed app is unaffected. The tests are kept for when that is fixed;
//! run them with `cargo test -- --include-ignored` and re-run on a hang.

use nsh_core::ui::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

#[component]
fn Probe() -> impl IntoElement {
    view().flex_1().child(text("probe ok"))
}

#[test]
fn probe_engine_mounts() {
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<Probe>())).expect("headless engine starts");
    app.wait_for("probe ok").expect("probe mounts");
}

#[test]
#[ignore = "flaky: nsh-testing 0.2.0 headless first-commit race (see module docs)"]
fn wallet_shell_mounts() {
    unsafe {
        std::env::set_var("FIBER_WALLET_TEST", "1");
    }
    let mut app = Headless::start(EngineConfig::new(
        Root::of::<nsh_app_fiber_wallet_nsh::App>(),
    ))
    .expect("headless engine starts");
    app.wait_for("Fiber Wallet").expect("the title bar mounts");
    app.wait_for("首页").expect("the sidebar mounts");
    app.wait_for("总资产").expect("the home page mounts");
}
