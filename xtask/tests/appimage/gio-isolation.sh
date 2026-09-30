#!/usr/bin/env bash
set -euo pipefail
# GUI・ネットワーク・ユーザーprofileを使わず、実際の初期化処理と同梱GIOを検証する。
appdir=$(realpath "${1:?usage: gio-isolation.sh path/to/kukuri.AppDir}")
test_source=$(cd "$(dirname "$0")" && pwd)
test_dir=$(mktemp -d /tmp/kukuri-gio-test.XXXXXX)
trap 'rm -rf -- "$test_dir"' EXIT
fixture="$test_dir/AppDir with spaces"
mkdir -p "$fixture" "$test_dir/host-modules"
ln -s "$appdir/usr" "$fixture/usr"
# shellcheck disable=SC2046
cc "$test_source/gio-probe.c" -o "$fixture/AppRun.wrapped" $(pkg-config --cflags --libs gio-2.0)
# shellcheck disable=SC2046
cc -shared -fPIC "$test_source/incompatible-gio.c" -o "$test_dir/host-modules/libincompatible.so" $(pkg-config --cflags --libs gio-2.0)
# shellcheck disable=SC2046
cc -DRUST_PROBE -c "$test_source/gio-probe.c" -o "$test_dir/gio-probe.o" $(pkg-config --cflags gio-2.0)
rustc --edition=2024 "$test_source/env-probe.rs" -o "$fixture/env-probe" \
    -C "link-arg=$test_dir/gio-probe.o" -l gio-2.0 -l gobject-2.0 -l glib-2.0

# 修正前に問題を起こすfixtureであることも確認し、検査の空振りを防ぐ。
env LD_LIBRARY_PATH="$appdir/usr/lib" GIO_MODULE_DIR="$test_dir/host-modules" \
    GIO_EXTRA_MODULES="$appdir/usr/lib/gio/modules" \
    "$fixture/AppRun.wrapped" >"$test_dir/before.out" 2>"$test_dir/before.err" || true
grep -q 'undefined symbol: kukuri_fixture_missing_gio_symbol' "$test_dir/before.err"

env APPDIR="$fixture" LD_LIBRARY_PATH="$appdir/usr/lib" \
    GIO_MODULE_DIR="$test_dir/host-modules" GIO_EXTRA_MODULES="$test_dir/host-modules" \
    "$fixture/env-probe" >"$test_dir/after.out" 2>"$test_dir/after.err"
cat "$test_dir/after.out"
if [ -s "$test_dir/after.err" ]; then
    cat "$test_dir/after.err" >&2
    exit 1
fi
grep -q '^local=1 tls=1$' "$test_dir/after.out"

# 通常起動では既存環境を変更しないため、同じ不整合fixtureは引き続き失敗する。
env -u APPDIR LD_LIBRARY_PATH="$appdir/usr/lib" GIO_MODULE_DIR="$test_dir/host-modules" \
    GIO_EXTRA_MODULES="$appdir/usr/lib/gio/modules" \
    "$fixture/env-probe" >"$test_dir/native.out" 2>"$test_dir/native.err" || true
grep -q 'undefined symbol: kukuri_fixture_missing_gio_symbol' "$test_dir/native.err"

# 同梱TLS欠落時にホストへ黙ってfallbackせず、GIOを起動する前に拒否する。
status=0
env APPDIR="$test_dir/missing" "$fixture/env-probe" 2>"$test_dir/missing.err" || status=$?
test "$status" -eq 2
grep -q 'AppImage' "$test_dir/missing.err"
printf 'GIO isolation: incompatible module rejected before load; bundled TLS and local files available\n'
