#!/bin/bash
# SPDX-License-Identifier: MPL-2.0
# Screenshots of mothrix in kitty (sharp pictures through kitty's graphics,
# colour emoji), for the README and for checking what the text snapshot
# cannot show. Runs in a docker container with a virtual screen and
# software OpenGL; needs dev/server.sh to be up. Pictures land in
# target/dev/shots, cut to the window.
#
#   dev/kitty-shot.sh key:ctrl+k type:lunch key:Return wait:5 shot:room
#
# Steps: key:<xdotool key>, type:<text>, wait:<seconds>, shot:<name>.
# MOTHRIX_SHOT_BG (the terminal's background, default the mothrix
# theme's #140a1e) and MOTHRIX_SHOT_SIZE (pixels, default 1300x800).
set -eu
cd "$(dirname "$0")/.."
mkdir -p target/dev/shots
if [ "${1:-}" != --inside ]; then
  exec docker run --rm -u 0 --network host -e MOTHRIX_SHOT_BG -e MOTHRIX_SHOT_SIZE \
    -v "$PWD:/mothrix" -w /mothrix "${MOTHRIX_SHOT_IMAGE:-debian:trixie}" \
    dev/kitty-shot.sh --inside "$(id -u)" "$@"
fi
owner=$2; shift 2
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq kitty xvfb xdotool imagemagick fonts-dejavu-core fonts-noto-color-emoji \
  ca-certificates libgl1-mesa-dri >/dev/null 2>&1
Xvfb :7 -screen 0 1920x1200x24 >/dev/null 2>&1 &
sleep 1
export DISPLAY=:7 LANG=C.UTF-8 LC_ALL=C.UTF-8 LIBGL_ALWAYS_SOFTWARE=1
export XDG_CONFIG_HOME=/mothrix/target/dev/config XDG_DATA_HOME=/mothrix/target/dev/data XDG_CACHE_HOME=/mothrix/target/dev/cache
size=${MOTHRIX_SHOT_SIZE:-1300x800}
kitty --config NONE -o font_family="DejaVu Sans Mono" -o font_size=11 \
  -o background="${MOTHRIX_SHOT_BG:-#140a1e}" -o foreground='#efdcb8' \
  -o initial_window_width="${size%x*}" -o initial_window_height="${size#*x}" -o remember_window_size=no \
  -o window_padding_width=6 -o linux_display_server=x11 /mothrix/target/debug/mothrix >/dev/null 2>&1 &
sleep 9
for step in "$@"; do
  case "$step" in
    shot:*)
      import -window root "/tmp/shot.png"
      convert /tmp/shot.png -crop "$size+0+0" +repage "/mothrix/target/dev/shots/${step#shot:}.png" ;;
    wait:*) sleep "${step#wait:}" ;;
    key:*) xdotool key --clearmodifiers "${step#key:}" ;;
    type:*) xdotool type --delay 40 "${step#type:}" ;;
  esac
done
chown -R "$owner" /mothrix/target/dev
