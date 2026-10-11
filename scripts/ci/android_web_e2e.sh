#!/usr/bin/env bash
# 実ブラウザの E2E の scenario を、Android の emulator の Chrome で 1 つ回す（#1220 AC-5c）。
# android-emulator-runner の script は 1 行ずつ別の shell で動くので、手順はこの file にまとめる。
set -euo pipefail
scenario="$1"
[ "${CI:-}" = true ] || { echo 'Android Web E2E requires the isolated CI emulator'; exit 1; }
trap 'status=$?; if [ "$status" -ne 0 ]; then adb logcat -d -b crash -t 150 || true; adb shell dumpsys meminfo | head -60 || true; fi; exit "$status"' EXIT

adb wait-for-device
echo 'SDK TalkBack package diagnostics'
adb shell pm list packages -u | grep 'talkback' || true
adb shell pm path com.google.android.marvin.talkback || true
adb shell dumpsys package com.google.android.marvin.talkback | grep -E 'codePath=|versionName=|primaryCpuAbi=|User 0:|flags=' || true
if [[ "$scenario" =~ ^pwa-(input|ime|accessibility)$ ]]; then
  density=$(adb shell wm density | sed -n 's/.*density: \([0-9]*\).*/\1/p' | tail -1)
  adb shell wm size "$((320 * density / 160))x$((720 * density / 160))"
  adb shell settings put system font_scale 2.0
fi
if [ "$scenario" = pwa-accessibility ]; then
  # UiAutomationの通常dumpはTalkBackを抑制するため、focusだけを非抑制のinstrumentationで読む。
  focus_dir=$(mktemp -d)
  platform="$ANDROID_HOME/platforms/android-35/android.jar"
  [ -f "$platform" ] || sdkmanager 'platforms;android-35'
  build_tools=$(find "$ANDROID_HOME/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)
  javac -source 8 -target 8 -cp "$platform" -d "$focus_dir" apps/desktop/tests/web-e2e/android-focus/FocusProbe.java
  "$build_tools/d8" --lib "$platform" --output "$focus_dir" "$focus_dir/app/kukuri/focusprobe/FocusProbe.class"
  "$build_tools/aapt" package -f -M apps/desktop/tests/web-e2e/android-focus/AndroidManifest.xml -I "$platform" -F "$focus_dir/probe.apk"
  (cd "$focus_dir" && "$build_tools/aapt" add probe.apk classes.dex)
  keytool -genkeypair -keystore "$focus_dir/probe.keystore" -storepass android -keypass android -alias probe -keyalg RSA -dname 'CN=kukuri test' -validity 3650
  "$build_tools/apksigner" sign --ks "$focus_dir/probe.keystore" --ks-pass pass:android --key-pass pass:android --ks-key-alias probe "$focus_dir/probe.apk"
  adb install "$focus_dir/probe.apk"
fi
# 現行stableの検証も隔離CIで自動準備する。通常のlaneは既存imageのChromeを使う。
apk_url=$(jq -r '.inputs.android_chrome_apk_url // ""' "${GITHUB_EVENT_PATH:-/dev/null}")
if [ -n "$apk_url" ]; then
  echo "::add-mask::$apk_url"
  apk_dir=$(mktemp -d)
  tools=$(find "$ANDROID_HOME/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)
  curl -sfL "$apk_url" -o "$apk_dir/chrome.apk"
  # 155.0.8059.39 x86_64はSDKでGoogle署名を照合済みの固定artifact（#1767）。
  # URLを取得し直しても同一の署名済みbytesだけを導入する。
  echo '600fa0642ff7141a6ca8e3386bfcace6074d43c6ed7ae1de15ec9622375462c6  '"$apk_dir/chrome.apk" | sha256sum -c -
  "$tools/apksigner" verify --verbose --print-certs "$apk_dir/chrome.apk"
  badging=$("$tools/aapt" dump badging "$apk_dir/chrome.apk")
  grep -q "^package: name='com.android.chrome' " <<< "$badging"
  incoming=$(sed -n "s/^package: .*versionName='\([^']*\)'.*/\1/p" <<< "$badging")
  # desktopのChrome for Testingではなく、Android Stableの現在の配布を照合する。
  curl -sf "https://versionhistory.googleapis.com/v1/chrome/platforms/android/channels/stable/versions/$incoming/releases" |
    jq -e --arg version "$incoming" '.releases | any(.version == $version and .serving.endTime == null and .fraction > 0)' >/dev/null ||
    { echo 'Chrome APK is not an actively served Android Stable release'; exit 1; }
  adb install -r "$apk_dir/chrome.apk"
  echo "Verified Google Chrome APK $incoming; sha256 $(sha256sum "$apk_dir/chrome.apk" | cut -d' ' -f1)"
fi
# Chrome の初回の画面（利用規約・同期の案内）を出さない。
adb shell am set-debug-app --persistent com.android.chrome
adb shell "echo '_ --disable-fre --no-default-browser-check --no-first-run' > /data/local/tmp/chrome-command-line"
# factory launcherのANRがChromeのOS操作を遮ったため、試験前に停止する。
# ホーム画面の検証ではlauncherを新しく起動する（専用CI emulatorのみ）。
adb shell am force-stop com.google.android.apps.nexuslauncher

# emulator の Chrome の版に合う chromedriver（Chrome for Testing は 115 から。それより前は旧い配布先）。
version=$(adb shell dumpsys package com.android.chrome | sed -n 's/.*versionName=\([0-9.]*\).*/\1/p' | head -1)
major=${version%%.*}
if [ "$major" -ge 115 ]; then
  url=$(curl -sf https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json |
    jq -r --arg major "$major." '[.versions[] | select(.version | startswith($major)) | .downloads.chromedriver[]? | select(.platform == "linux64") | .url] | last')
else
  url="https://chromedriver.storage.googleapis.com/$(curl -sf "https://chromedriver.storage.googleapis.com/LATEST_RELEASE_$major")/chromedriver_linux64.zip"
fi
dir=$(mktemp -d)
curl -sfL "$url" -o "$dir/chromedriver.zip"
unzip -q "$dir/chromedriver.zip" -d "$dir"
driver=$(find "$dir" -name chromedriver -type f | head -1)
chmod +x "$driver"
echo "emulator Chrome $version, chromedriver $url"

KUKURI_WEB_E2E_BROWSER=android ANDROID_CHROMEDRIVER="$driver" CHROMEDRIVER="$CHROMEWEBDRIVER/chromedriver" \
  cargo xtask-lite web-e2e --no-build "$scenario"
