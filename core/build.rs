use std::path::{Path, PathBuf};

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    if matches!(target_os.as_str(), "macos" | "ios" | "android" | "linux") {
        cc::Build::new()
            .file("src/heapsize_shim.c")
            .compile("heapsize_shim");
        println!("cargo:rerun-if-changed=src/heapsize_shim.c");
    }

    // 32-bit Android references __atomic_* builtins from vendored OpenSSL.
    // NDK r28 moved them into libclang_rt.builtins-<arch>-android.a, and rustc
    // links with -nodefaultlibs, so clang never adds that archive on its own.
    // Link it explicitly by locating the resource dir through the NDK clang
    // that cargokit exposes as CC_<target>.
    if target_os == "android" && matches!(target_arch.as_str(), "x86" | "arm") {
        let rt_arch = match target_arch.as_str() {
            "x86" => "i686",
            _ => "arm",
        };
        match find_android_builtins(rt_arch) {
            Some((search_dir, lib_name)) => {
                println!("cargo:rustc-link-search=native={}", search_dir.display());
                println!("cargo:rustc-link-lib=static={lib_name}");
            }
            None => {
                println!(
                    "cargo:warning=fiber_wallet_core: could not locate NDK compiler-rt \
                     builtins for {rt_arch}; the 32-bit link may fail on __atomic symbols"
                );
            }
        }
    }
}

/// Locates `libclang_rt.builtins-<rt_arch>-android.a` and returns its
/// directory. Probes, in order: ANDROID_NDK_HOME/ANDROID_NDK_ROOT, any NDK
/// installed under ANDROID_HOME/ANDROID_SDK_ROOT, then the NDK clang that
/// cargokit exposes as CC_<rust-target>.
fn find_android_builtins(rt_arch: &str) -> Option<(PathBuf, String)> {
    let lib_name = format!("libclang_rt.builtins-{rt_arch}-android.a");

    let mut roots: Vec<PathBuf> = Vec::new();
    for key in ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT"] {
        if let Ok(ndk) = std::env::var(key) {
            roots.push(PathBuf::from(ndk));
        }
    }
    for key in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(sdk) = std::env::var(key) {
            let ndks = PathBuf::from(sdk).join("ndk");
            if let Ok(entries) = std::fs::read_dir(&ndks) {
                for entry in entries.flatten() {
                    roots.push(entry.path());
                }
            }
        }
    }
    let cc_keys = [
        "CC_i686_linux_android",
        "CC_i686-linux-android",
        "CC_armv7_linux_androideabi",
        "CC_armv7-linux-androideabi",
    ];
    if let Some(clang) = cc_keys
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .filter(|value| Path::new(value).is_file())
    {
        // <ndk>/toolchains/llvm/prebuilt/<host>/bin/clang -> <...>/prebuilt/<host>
        if let Some(prebuilt) = Path::new(&clang).ancestors().nth(1) {
            roots.push(prebuilt.to_owned());
        }
    }

    for root in roots {
        // Toolchain root may be the NDK itself or its prebuilt dir.
        for base in [root.join("toolchains/llvm/prebuilt"), root.clone()] {
            let Ok(hosts) = std::fs::read_dir(&base) else {
                continue;
            };
            for host in hosts.flatten() {
                let clang_libs = host.path().join("lib/clang");
                let Ok(apis) = std::fs::read_dir(&clang_libs) else {
                    continue;
                };
                for api in apis.flatten() {
                    let candidate = api.path().join("lib/linux").join(&lib_name);
                    if candidate.is_file() {
                        return Some((
                            api.path().join("lib/linux"),
                            lib_name
                                .trim_start_matches("lib")
                                .trim_end_matches(".a")
                                .to_owned(),
                        ));
                    }
                }
            }
        }
    }
    None
}
