#!/usr/bin/env bash
# ADR 0057 §8 の browser の固定 workload（E1〜E4）を headless の Chromium で実行する。
# browser↔native の相手（examples/webrtc_peer.rs）を起動し、その URL を試験の build に渡す。
# 要るもの: wasm32-unknown-unknown の target、clang と llvm-ar（C の依存の build）、
# wasm-bindgen-cli（lockfile の wasm-bindgen と同じ版）、chromedriver（環境変数 CHROMEDRIVER）。
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo build -p kukuri-webrtc-transport --example webrtc_peer
log="$(mktemp)"
"${CARGO_TARGET_DIR:-target}/debug/examples/webrtc_peer" >"$log" 2>&1 &
peer=$!
trap 'kill "$peer" 2>/dev/null || true; rm -f "$log"' EXIT

url=""
for _ in $(seq 1 100); do
  url="$(sed -n 's/^KUKURI_WEBRTC_PEER_URL=//p' "$log")"
  [ -n "$url" ] && break
  sleep 0.1
done
if [ -z "$url" ]; then
  cat "$log"
  exit 1
fi

KUKURI_WEBRTC_PEER_URL="$url" \
  cargo test -p kukuri-webrtc-transport --target wasm32-unknown-unknown --lib
