#!/bin/sh
# Compiles the scanner's Java glue (nsh_app_android/bridge/java) into the dex
# blob the Rust scanner embeds with include_bytes!. The nsh Android host has
# no app-level Java extension point, so the scanner ships its own tiny dex
# and loads it at runtime through an InMemoryDexClassLoader.
#
# Requires the bootstrap toolchain: scripts/setup-android-sdk.sh (JDK 17 and
# build-tools 35 under /Volumes/tlt/.tooling).
set -eu

REPO=$(cd "$(dirname "$0")/.." && pwd)
BRIDGE="$REPO/nsh_app_android/bridge"
OUT="$BRIDGE/scanner_bridge.dex"

TOOLING="${NSH_TOOLING:-/Volumes/tlt/.tooling}"
JAVAC="$TOOLING/jdk-17/Contents/Home/bin/javac"
ANDROID_HOME="${ANDROID_HOME:-$TOOLING/android-sdk}"
PLATFORM="$ANDROID_HOME/platforms/android-35/android.jar"
D8="$ANDROID_HOME/build-tools/35.0.0/d8"

[ -x "$JAVAC" ] || { echo "javac not found at $JAVAC (run scripts/setup-android-sdk.sh)" >&2; exit 1; }
[ -f "$PLATFORM" ] || { echo "android.jar not found at $PLATFORM" >&2; exit 1; }
[ -x "$D8" ] || { echo "d8 not found at $D8" >&2; exit 1; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

"$JAVAC" --release 11 -classpath "$PLATFORM" -d "$WORK/classes" \
    "$BRIDGE/java/dev/fiberwallet/nsh/ScannerBridge.java"

mkdir -p "$WORK/dex"
"$D8" --release --min-api 28 --lib "$PLATFORM" \
    --output "$WORK/dex" "$WORK/classes/dev/fiberwallet/nsh/ScannerBridge.class"

mkdir -p "$BRIDGE"
cp "$WORK/dex/classes.dex" "$OUT"
echo "wrote $OUT ($(wc -c < "$OUT") bytes)"
