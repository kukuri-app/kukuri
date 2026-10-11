#!/usr/bin/env bash
# 実ブラウザの E2E の scenario を、Android の emulator の Chrome で 1 つ回す（#1220 AC-5c）。
# android-emulator-runner の script は 1 行ずつ別の shell で動くので、手順はこの file にまとめる。
set -euo pipefail
scenario="$1"

adb wait-for-device
# 現行stableの検証も隔離CIで自動準備する。通常のlaneは既存imageのChromeを使う。
apk_url=$(jq -r '.inputs.android_chrome_apk_url // ""' "${GITHUB_EVENT_PATH:-/dev/null}")
if [ -n "$apk_url" ]; then
  [ "${CI:-}" = true ] || { echo 'Chrome update requires the isolated CI emulator'; exit 1; }
  echo "::add-mask::$apk_url"
  apk_dir=$(mktemp -d)
  tools=$(find "$ANDROID_HOME/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)
  installed=$(adb shell pm path com.android.chrome | head -1 | sed 's/^package://;s/\r$//')
  adb pull "$installed" "$apk_dir/reference.apk" >/dev/null
  curl -sfL "$apk_url" -o "$apk_dir/chrome.apk"
  signer() { "$tools/apksigner" verify --print-certs "$1" | sed -n 's/^Signer #1 certificate SHA-256 digest: //p'; }
  reference_signer=$(signer "$apk_dir/reference.apk")
  echo "SDK Chrome signer: $reference_signer"
  [ -n "$reference_signer" ] && [ "$(signer "$apk_dir/chrome.apk")" = "$reference_signer" ] || {
    echo 'Chrome APK signer does not match the preinstalled Google Chrome'; exit 1;
  }
  badging=$("$tools/aapt" dump badging "$apk_dir/chrome.apk")
  grep -q "^package: name='com.android.chrome' " <<< "$badging"
  stable=$(curl -sf https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions.json | jq -r '.channels.Stable.version')
  incoming=$(sed -n "s/^package: .*versionName='\([^']*\)'.*/\1/p" <<< "$badging")
  [ "${incoming%%.*}" = "${stable%%.*}" ] || { echo 'Chrome APK is not the current stable milestone'; exit 1; }
  adb install -r "$apk_dir/chrome.apk"
  echo "Verified Google Chrome APK $incoming; sha256 $(sha256sum "$apk_dir/chrome.apk" | cut -d' ' -f1)"
fi
# Chrome の初回の画面（利用規約・同期の案内）を出さない。
adb shell am set-debug-app --persistent com.android.chrome
adb shell "echo '_ --disable-fre --no-default-browser-check --no-first-run' > /data/local/tmp/chrome-command-line"

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
