use std::path::PathBuf;

use fiber_wallet_core::recovery_smoke::RecoverySmokePhase;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: fnn_recovery_smoke <data-dir> <crash|recover>");
        std::process::exit(2);
    }

    let Some(phase) = RecoverySmokePhase::parse(&args[2]) else {
        eprintln!("unknown phase '{}', expected crash|recover", args[2]);
        std::process::exit(2);
    };

    let data_dir = PathBuf::from(&args[1]);
    let code =
        fiber_wallet_core::recovery_smoke::run(data_dir.to_string_lossy().into_owned(), phase);
    std::process::exit(code);
}
