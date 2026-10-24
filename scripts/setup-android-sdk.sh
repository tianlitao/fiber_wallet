#!/bin/sh
# One-time Android toolchain bootstrap for this machine.
#
# Everything installs under /Volumes/tlt/.tooling: the system data volume has
# only a few GB free, so nothing Android-related may land in $HOME or /opt.
# Redirect GRADLE_USER_HOME when building (see Makefile android targets).
set -eu

TOOLING=/Volumes/tlt/.tooling
SDK="$TOOLING/android-sdk"
JDK_DIR="$TOOLING/jdk-17"
CMDLINE_TOOLS_VERSION=11076708

mkdir -p "$TOOLING"

if [ ! -x "$JDK_DIR/Contents/Home/bin/java" ] && [ ! -x "$JDK_DIR/bin/java" ]; then
  echo "== downloading Temurin JDK 17 (aarch64 macOS)"
  curl -L --fail --retry 3 -o "$TOOLING/jdk17.tar.gz" \
    "https://api.adoptium.net/v3/binary/latest/17/ga/mac/aarch64/jdk/hotspot/normal/eclipse?project=jdk"
  tar -xzf "$TOOLING/jdk17.tar.gz" -C "$TOOLING"
  extracted=$(find "$TOOLING" -maxdepth 1 -type d -name 'jdk-17*')
  mv "$extracted" "$JDK_DIR"
  rm -f "$TOOLING/jdk17.tar.gz"
fi
echo "== jdk ready"

if [ ! -x "$SDK/cmdline-tools/latest/bin/sdkmanager" ]; then
  echo "== downloading Android cmdline-tools"
  mkdir -p "$SDK/cmdline-tools"
  curl -L --fail --retry 3 -o "$TOOLING/cmdline-tools.zip" \
    "https://dl.google.com/android/repository/commandlinetools-mac-${CMDLINE_TOOLS_VERSION}_latest.zip"
  unzip -q "$TOOLING/cmdline-tools.zip" -d "$SDK/cmdline-tools"
  mv "$SDK/cmdline-tools/cmdline-tools" "$SDK/cmdline-tools/latest"
  rm -f "$TOOLING/cmdline-tools.zip"
fi
echo "== cmdline-tools ready"

# macOS JDK tarballs extract to a Contents/Home layout.
if [ -x "$JDK_DIR/Contents/Home/bin/java" ]; then
  JAVA_HOME="$JDK_DIR/Contents/Home"
else
  JAVA_HOME="$JDK_DIR"
fi
export JAVA_HOME
export ANDROID_HOME="$SDK"

echo "== accepting licenses"
yes | "$SDK/cmdline-tools/latest/bin/sdkmanager" --licenses >/dev/null 2>&1 || true

echo "== installing platform-tools, android-35, build-tools"
"$SDK/cmdline-tools/latest/bin/sdkmanager" "platform-tools" "platforms;android-35" "build-tools;35.0.0"

echo "== android sdk base ready: $SDK"
"$SDK/cmdline-tools/latest/bin/sdkmanager" --list_installed
