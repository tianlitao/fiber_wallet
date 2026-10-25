#!/bin/sh
# Builds and runs the Android shell through `nsh run --target android` with
# the machine's toolchain environment (scripts/setup-android-sdk.sh installs
# everything under /Volumes/tlt/.tooling).
#
# Why the extra environment:
# - the proxy: crates.io and the Gradle/SDK endpoints stall without it here;
# - RANLIB_/AR_: openssl-sys' vendored build derives `<triple>-ranlib` from
#   the compiler prefix, and NDK r28 ships only the generic `llvm-ranlib`
#   (`/bin/sh: aarch64-linux-android-ranlib: command not found` mid-make).
#
# The packaged APK only declares INTERNET (the nsh CLI generates the
# manifest from a template with no permission hook), so after `nsh run` or
# `nsh package` installs an app, run scripts/patch_apk_camera.sh and
# reinstall for the scanner's CAMERA permission — see that script.
#
# Usage: scripts/build-android.sh [app-dir] [nsh args...]
#   scripts/build-android.sh nsh_app_android run --target android
set -eu

TOOLING="${NSH_TOOLING:-/Volumes/tlt/.tooling}"
SDK="$TOOLING/android-sdk"
NDK_BIN=$(ls -d "$SDK"/ndk/*/toolchains/llvm/prebuilt/*/bin | sort -V | tail -1)

export ANDROID_HOME="$SDK"
export ANDROID_SDK_ROOT="$SDK"
export JAVA_HOME="$TOOLING/jdk-17/Contents/Home"
export PATH="$TOOLING/nsh/bin:$SDK/platform-tools:$PATH"
export https_proxy="${https_proxy:-http://127.0.0.1:7897}"
export http_proxy="${http_proxy:-http://127.0.0.1:7897}"
export all_proxy="${all_proxy:-socks5://127.0.0.1:7897}"
# openssl-sys' vendored build derives `<triple>-ranlib` from the compiler
# prefix; NDK r28 ships only the generic llvm-ar/llvm-ranlib, so every ABI
# triple gets pointed at them.
for triple in aarch64-linux-android x86_64-linux-android i686-linux-android armv7-linux-androideabi; do
    export "RANLIB_${triple//-/_}=$NDK_BIN/llvm-ranlib"
    export "AR_${triple//-/_}=$NDK_BIN/llvm-ar"
done

APP="${1:-nsh_app_android}"
shift 2>/dev/null || true
cd "$APP"

# The CLI's packaging step occasionally fails on this external volume with
# `zipalign failed: Unable to open …unaligned.apk as zip archive`; clearing
# the staging directory and retrying has been reliable.
attempt=0
until nsh "$@"; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 3 ]; then
        echo "build-android.sh: giving up after $attempt attempts" >&2
        exit 1
    fi
    echo "build-android.sh: retrying (attempt $attempt) after a failed nsh invocation" >&2
    sleep 2
done
