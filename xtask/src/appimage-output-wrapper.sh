#!/bin/sh
# kukuri AppImage output plugin wrapper（#1222）。`cargo xtask desktop-package` が Tauri の tools directory へ
# `linuxdeploy-plugin-appimage.AppImage` として配置する。linuxdeploy は依存の収集と GTK plugin の後に
# この output plugin を呼ぶので、ホスト提供の lib を AppDir から除いてから本来の plugin へ渡す。
set -eu

real="$(dirname "$0")/kukuri-appimage-output.real.AppImage"
appdir=""
previous=""
for arg in "$@"; do
    case "$arg" in
        --appdir=*) appdir=${arg#--appdir=} ;;
    esac
    if [ "$previous" = "--appdir" ]; then
        appdir=$arg
    fi
    previous=$arg
done

# kukuri の package 以外（同じ cache を使う他の Tauri project）と、plugin 情報の問合せでは何も変えない。
if [ -n "${KUKURI_APPIMAGE_HOST_LIBRARIES:-}" ] && [ -n "$appdir" ]; then
    if [ ! -d "$appdir/usr/lib" ]; then
        echo "kukuri AppImage output wrapper: $appdir/usr/lib がありません" >&2
        exit 1
    fi
    old_ifs=$IFS
    IFS=:
    for stem in $KUKURI_APPIMAGE_HOST_LIBRARIES; do
        [ -n "$stem" ] || continue
        find "$appdir" \( -type f -o -type l \) -name "$stem*" -exec rm -f -- {} +
    done
    IFS=$old_ifs
fi

exec "$real" "$@"
