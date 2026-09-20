#!/usr/bin/env bash
set -euo pipefail
# 新しいMesaを持つFedoraのcontainerで、AppImageのWebKitWebProcessがEGLを初期化できるかを確認する（#1222）。
# headlessのweston＋Xwayland＋software renderingを使い、GPU・ホストのdesktop・ユーザーprofile・外部通信は使わない
# （未同意のprofileなのでアプリはnetworkを開始しない。container内のpackage取得は行う）。
# 実GPU・実desktopでの表示確認の代替ではない。
#
# usage: egl-startup-fedora.sh path/to/kukuri.AppImage [output-dir]
# 成功条件: EGL_BAD_PARAMETERが出ず、起動25秒後にWebKitWebProcessが生存している。
# output-dirにはアプリのlogとwindowの画像（window.png）を残す。
appimage=$(realpath "${1:?usage: egl-startup-fedora.sh path/to/kukuri.AppImage [output-dir]}")
output=$(realpath -m "${2:-test-results/kukuri/appimage-egl-startup}")
image=${KUKURI_EGL_TEST_IMAGE:-fedora:latest}
mkdir -p "$output"
cp "$appimage" "$output/under-test.AppImage"
chmod +x "$output/under-test.AppImage"

docker run --rm -i -v "$output:/work" "$image" bash -s <<'CONTAINER'
set -uo pipefail
dnf install -y -q weston xorg-x11-server-Xwayland mesa-dri-drivers mesa-libEGL mesa-libGL mesa-libgbm \
    gtk3 dbus-daemon dbus-x11 procps-ng fontconfig libsoup3 gstreamer1-plugins-base libxslt lcms2 \
    libwebp woff2 harfbuzz-icu enchant2 hyphen libsecret libmanette libseccomp bubblewrap xdg-dbus-proxy \
    libayatana-appindicator-gtk3 desktop-file-utils xdg-utils xwd xwininfo ImageMagick >/dev/null 2>&1
. /etc/os-release
echo "os=$PRETTY_NAME"
rpm -q mesa-libEGL libwayland-client libxkbcommon | tr '\n' ' '
echo
cd /work
rm -rf squashfs-root
./under-test.AppImage --appimage-extract >/dev/null 2>&1
export XDG_RUNTIME_DIR=/tmp/xdg HOME=/tmp/home LIBGL_ALWAYS_SOFTWARE=1
mkdir -p "$XDG_RUNTIME_DIR" "$HOME" /tmp/.X11-unix
chmod 700 "$XDG_RUNTIME_DIR"
chmod 1777 /tmp/.X11-unix
weston --backend=headless --xwayland --socket=wl-test --idle-time=0 --width=1400 --height=900 >/work/weston.log 2>&1 &
for _ in $(seq 1 30); do [ -S "$XDG_RUNTIME_DIR/wl-test" ] && break; sleep 0.5; done
export WAYLAND_DISPLAY=wl-test DISPLAY=:0
(cd squashfs-root && timeout 45 dbus-run-session -- ./AppRun >/work/app.log 2>&1) &
sleep 25
alive=$(pgrep -c -f WebKitWebProcess)
window=$(xwininfo -root -tree 2>/dev/null | grep '"kukuri":' | head -1 | awk '{print $1}')
if [ -n "$window" ]; then
    xwd -id "$window" -out /work/window.xwd && magick /work/window.xwd /work/window.png && rm -f /work/window.xwd
fi
wait %2 2>/dev/null
rm -rf squashfs-root
echo "WebKitWebProcess alive: $alive"
if grep -q 'EGL_BAD_PARAMETER' /work/app.log; then
    grep 'EGL' /work/app.log
    exit 1
fi
[ "$alive" -ge 1 ]
CONTAINER
