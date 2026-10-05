#!/usr/bin/env bash
# browser の試験を headless の Chromium で実行する。browser↔native の相手（<package> の example <example>）を
# 起動し、その URL を試験の build に渡す（`KUKURI_PEER_URL`。src/signaling_fixture.rs）。
# 使い方: browser_peer_test.sh <package> <example> [cargo の追加引数...]
# 要るもの: wasm32-unknown-unknown の target、clang と llvm-ar（C の依存の build）、
# wasm-bindgen-cli（lockfile の wasm-bindgen と同じ版）、chromedriver（環境変数 CHROMEDRIVER）。
set -euo pipefail
cd "$(dirname "$0")/../.."
package="$1"
example="$2"
shift 2

cargo build -p "$package" --example "$example" "$@"
log="$(mktemp)"
"${CARGO_TARGET_DIR:-target}/debug/examples/$example" >"$log" 2>&1 &
peer=$!
trap 'kill "$peer" 2>/dev/null || true; rm -f "$log"' EXIT

url=""
for _ in $(seq 1 100); do
  url="$(sed -n 's/^KUKURI_PEER_URL=//p' "$log")"
  [ -n "$url" ] && break
  sleep 0.1
done
if [ -z "$url" ]; then
  cat "$log"
  exit 1
fi

# wasm-bindgen-test-runner は試験全体を既定 20 秒で打ち切る。各試験は自前の期限を持つので、全体の上限は
# 件数分の余裕をとる。
KUKURI_PEER_URL="$url" WASM_BINDGEN_TEST_TIMEOUT="${WASM_BINDGEN_TEST_TIMEOUT:-180}" \
  cargo test -p "$package" --target wasm32-unknown-unknown --lib "$@"
