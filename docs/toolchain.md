# Toolchain

Pinned workspace toolchain:

- Rust: repository default stable; Fiber v0.9.1 uses Rust 1.95.0
- NSH CLI: 0.2.0
- Xcode: 26.6
- Android: SDK 35 and NDK required for mobile builds

The NSH CLI is installed at:

```text
/Volumes/tlt/.tooling/nsh
```

Add it to the shell when needed:

```sh
export PATH="/Volumes/tlt/.tooling/nsh/bin:$PATH"
```

Network-constrained environments use:

```sh
export https_proxy=http://127.0.0.1:7897
export http_proxy=http://127.0.0.1:7897
export all_proxy=socks5://127.0.0.1:7897
```

## Android (bootstrap 2026-10-24)

The system data volume is nearly full, so the whole Android toolchain lives on
`/Volumes/tlt/.tooling` (installed by `scripts/setup-android-sdk.sh`):

- JDK 17 (Temurin): `/Volumes/tlt/.tooling/jdk-17/Contents/Home`
- Android SDK: `/Volumes/tlt/.tooling/android-sdk`
  - platform-tools 37, platforms;android-35, build-tools;35.0.0
  - NDK 27.0.12077973
- Gradle caches: `/Volumes/tlt/.tooling/gradle-home` (`GRADLE_USER_HOME`)

Xcode DerivedData is also redirected: `~/Library/Developer/Xcode/DerivedData`
is a symlink to `/Volumes/tlt/.tooling/XcodeDerivedData` (the
`IDEDerivedDataFolderLocation` default is ignored by plain `xcodebuild`).

## Android shell builds

`scripts/build-android.sh <app-dir> [nsh args…]` wraps `nsh` with the SDK/JDK
environment, the proxy, and the `RANLIB_/AR_` overrides the vendored OpenSSL
build needs (NDK r28 ships no `<triple>-ranlib`; without the override
`make install_dev` fails with "aarch64-linux-android-ranlib: command not
found"). Its CLI packaging step occasionally dies with
`zipalign failed: …unaligned.apk as zip archive` on this volume — the script
retries after clearing the staging directory.

Two app-level quirks of the nsh 0.2.0 Android host:

- The generated manifest only declares INTERNET and there is no hook for
  more permissions; `scripts/patch_apk_camera.sh <apk>` re-links the
  manifest with CAMERA added (the app APK carries no local resources, so the
  relink needs only android.jar), zipaligns and re-signs with the debug key
  so `adb install -r` updates in place. Grant with
  `adb shell pm grant dev.fiberwallet.nsh.android android.permission.CAMERA`.
- An APK file name with spaces breaks the CLI's zipalign invocation, so
  `nsh.json` "name" stays space-free (`FiberWallet`).
