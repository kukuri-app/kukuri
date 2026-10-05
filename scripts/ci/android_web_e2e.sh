#!/usr/bin/env bash
# 実ブラウザの E2E の scenario を、Android の emulator の Chrome で 1 つ回す（#1220 AC-5c）。
# android-emulator-runner の script は 1 行ずつ別の shell で動くので、手順はこの file にまとめる。
set -euo pipefail
scenario="$1"

adb wait-for-device
# Chrome の初回の画面（利用規約・同期の案内）を出さない。
adb shell am set-debug-app --persistent com.android.chrome
adb shell "echo '_ --disable-fre --no-default-browser-check --no-first-run' > /data/local/tmp/chrome-command-line"
# 画面の keyboard を出さない（出ると表示の範囲が縮み、chromedriver の押下の位置がずれて button に当たらない）。chromedriver は
# 文字を IME を通さずに送る。
for ime in $(adb shell ime list -s); do adb shell ime disable "$ime"; done

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
