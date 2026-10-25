#!/bin/sh
# Adds the CAMERA permission to a packaged nsh Android APK.
#
# The nsh CLI generates the app manifest itself from a template that only
# declares INTERNET; there is no hook to add permissions (checked against the
# 0.2.0 CLI: no nsh.json key, no manifest merge from the embed AARs, and
# aapt2 is resolved directly from the SDK so a PATH shim is bypassed). The
# scanner needs android.permission.CAMERA declared, so this script rebuilds
# the manifest: it rewrites the exact manifest the CLI generated plus CAMERA,
# re-links it with aapt2 (the app APK carries no local resources, so the link
# needs only the platform android.jar), swaps the compiled AXML +
# resources.arsc into the APK, re-aligns and re-signs with the standard
# Android debug key so `adb install -r` updates the nsh-installed app.
#
# Usage: scripts/patch_apk_camera.sh <app.apk> [output.apk]
# Requires ANDROID_HOME with build-tools (aapt2, zipalign, apksigner).
set -eu

APK="$1"
OUT="${2:-${APK%.apk}.camera.apk}"

TOOLING="${NSH_TOOLING:-/Volumes/tlt/.tooling}"
ANDROID_HOME="${ANDROID_HOME:-$TOOLING/android-sdk}"
BT="$ANDROID_HOME/build-tools"
AAPT2=$(ls -d "$BT"/*/aapt2 | sort -V | tail -1)
ZIPALIGN=$(ls -d "$BT"/*/zipalign | sort -V | tail -1)
APKSIGNER=$(ls -d "$BT"/*/apksigner | sort -V | tail -1)
PLATFORM="$ANDROID_HOME/platforms/android-35/android.jar"
KEYSTORE="${ANDROID_DEBUG_KEYSTORE:-$HOME/.android/debug.keystore}"

[ -f "$APK" ] || { echo "APK not found: $APK" >&2; exit 1; }
[ -f "$PLATFORM" ] || { echo "android.jar not found: $PLATFORM" >&2; exit 1; }
[ -f "$KEYSTORE" ] || {
  "$TOOLING/jdk-17/Contents/Home/bin/keytool" -genkeypair -keystore "$KEYSTORE" \
      -storepass android -alias androiddebugkey -keyalg RSA -validity 10000 \
      -dname "CN=Android Debug,O=Android,C=US"
}

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# The app's identity comes from the packaged manifest; the rest is the CLI's
# fixed template (AppActivity, RUST meta-data, framework theme).
PKG=$("$AAPT2" dump badging "$APK" | sed -n "s/^package: name='\([^']*\)'.*/\1/p")
LABEL=$("$AAPT2" dump badging "$APK" | sed -n "s/^application-label:'\([^']*\)'.*/\1/p")
VC=$("$AAPT2" dump badging "$APK" | sed -n "s/^package:.*versionCode='\([^']*\)'.*/\1/p")
VN=$("$AAPT2" dump badging "$APK" | sed -n "s/^package:.*versionName='\([^']*\)'.*/\1/p")
[ -n "$PKG" ] || { echo "cannot read the package id from $APK" >&2; exit 1; }

cat > "$WORK/AndroidManifest.xml" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="$PKG">
    <uses-permission android:name="android.permission.INTERNET" />
    <uses-permission android:name="android.permission.CAMERA" />
    <application android:label="$LABEL" android:usesCleartextTraffic="true"
        android:extractNativeLibs="false" android:enableOnBackInvokedCallback="true"
        android:supportsRtl="true" android:theme="@android:style/Theme.Material.Light.NoActionBar">
        <meta-data android:name="com.longbridge.nsh.RUST" android:value="true" />
        <activity android:name="com.longbridge.nsh.AppActivity" android:exported="true"
            android:windowSoftInputMode="adjustResize"
            android:configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboard|keyboardHidden|navigation|uiMode|density|fontScale|locale|layoutDirection">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
EOF

"$AAPT2" link -o "$WORK/base.apk" --manifest "$WORK/AndroidManifest.xml" -I "$PLATFORM" \
    --min-sdk-version 28 --target-sdk-version 35 \
    --version-code "${VC:-1}" --version-name "${VN:-1.0.0}"

# Start from the packaged APK untouched, swap in the two rebuilt entries, then
# realign (entry offsets shift) and re-sign with the same debug key the CLI
# uses, so `adb install -r` keeps updating the same installation.
cp "$APK" "$WORK/patched.apk"
unzip -o -q "$WORK/base.apk" AndroidManifest.xml resources.arsc -d "$WORK/swapped"
(cd "$WORK/swapped" && zip -q "$WORK/patched.apk" AndroidManifest.xml)
# resources.arsc must stay stored (uncompressed): Android 11+ rejects APKs
# that compress it.
(cd "$WORK/swapped" && zip -q -0 "$WORK/patched.apk" resources.arsc)
"$ZIPALIGN" -f -p 4 "$WORK/patched.apk" "$WORK/aligned.apk"

"$APKSIGNER" sign --ks "$KEYSTORE" --ks-pass pass:android \
    --ks-key-alias androiddebugkey --key-pass pass:android \
    --out "$OUT" "$WORK/aligned.apk"

"$AAPT2" dump badging "$OUT" | grep -E "^uses-permission" || true
echo "wrote $OUT (camera permission added)"
