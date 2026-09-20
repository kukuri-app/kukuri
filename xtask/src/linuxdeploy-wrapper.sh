#!/bin/sh
# kukuri linuxdeploy wrapper（#1222）。`cargo xtask desktop-package` が Tauri の tools directory へ配置する。
# Tauri bundler は linuxdeploy へ --exclude-library を渡せず、GTK plugin が内部で呼ぶ linuxdeploy にも
# 除外が届かない。そこで AppImage の出力だけを後段へ分け、その前にホスト提供の lib を AppDir から除く。
set -eu

real="$(dirname "$0")/linuxdeploy-x86_64.real.AppImage"
# kukuri の package 以外（同じ cache を使う他の Tauri project）では何も変えない。
if [ -z "${KUKURI_APPIMAGE_HOST_LIBRARIES:-}" ]; then
    exec "$real" "$@"
fi

appdir=""
previous=""
for arg in "$@"; do
    if [ "$previous" = "--appdir" ]; then
        appdir=$arg
    fi
    previous=$arg
done

# "$@" から `--output <format>` だけを外し、前段の引数にする。
output=""
remaining=$#
while [ "$remaining" -gt 0 ]; do
    arg=$1
    shift
    remaining=$((remaining - 1))
    if [ "$arg" = "--output" ] && [ "$remaining" -gt 0 ]; then
        output=$1
        shift
        remaining=$((remaining - 1))
        continue
    fi
    set -- "$@" "$arg"
done

if [ -z "$output" ]; then
    exec "$real" "$@"
fi
if [ -z "$appdir" ] || [ ! -d "$appdir/usr/lib" ]; then
    echo "kukuri linuxdeploy wrapper: --appdir が見つかりません" >&2
    exit 1
fi

"$real" "$@"

set -- --appimage-extract-and-run --appdir "$appdir"
old_ifs=$IFS
IFS=:
for stem in $KUKURI_APPIMAGE_HOST_LIBRARIES; do
    [ -n "$stem" ] || continue
    find "$appdir" \( -type f -o -type l \) -name "$stem*" -exec rm -f -- {} +
    set -- "$@" --exclude-library "$stem*"
done
IFS=$old_ifs

exec "$real" "$@" --output "$output"
