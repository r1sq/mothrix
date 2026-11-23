#!/bin/bash
# SPDX-License-Identifier: MPL-2.0
# Screenshots of mothrix in a real terminal (xterm, which draws Sixel), for
# checking what the text snapshot cannot show: colours and pictures.
# Runs in a docker container with a virtual screen; needs dev/server.sh
# to be up. Pictures land in target/dev/shots.
#
#   dev/xterm-shot.sh key:ctrl+k type:lunch key:Return wait:4 shot:room
#
# Steps: key:<xdotool key>, type:<text>, wait:<seconds>, shot:<name>.
# MOTHRIX_SHOT_BG (the terminal's background, default #1a1b26) and
# MOTHRIX_SHOT_SIZE (columns x rows, default 120x40) change the window.
set -eu
cd "$(dirname "$0")/.."
mkdir -p target/dev/shots
if [ "${1:-}" != --inside ]; then
  exec docker run --rm -u 0 --network host -e MOTHRIX_SHOT_BG -e MOTHRIX_SHOT_SIZE -v "$PWD:/mothrix" -w /mothrix "${MOTHRIX_SHOT_IMAGE:-debian:trixie}" \
    dev/xterm-shot.sh --inside "$(id -u)" "$@"
fi
owner=$2; shift 2
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq xterm xvfb xdotool imagemagick fonts-dejavu-core ca-certificates >/dev/null 2>&1
Xvfb :7 -screen 0 1600x1000x24 >/dev/null 2>&1 &
sleep 1
export DISPLAY=:7 LANG=C.UTF-8 LC_ALL=C.UTF-8
export XDG_CONFIG_HOME=/mothrix/target/dev/config XDG_DATA_HOME=/mothrix/target/dev/data XDG_CACHE_HOME=/mothrix/target/dev/cache
xterm -u8 -ti vt340 -xrm 'XTerm*numColorRegisters: 256' -fa 'DejaVu Sans Mono' -fs 10 \
  -geometry "${MOTHRIX_SHOT_SIZE:-120x40}+0+0" -bg "${MOTHRIX_SHOT_BG:-#1a1b26}" -fg '#c0caf5' -e /mothrix/target/debug/mothrix &
sleep 7
for step in "$@"; do
  case "$step" in
    shot:*) import -window root "/mothrix/target/dev/shots/${step#shot:}.png" ;;
    wait:*) sleep "${step#wait:}" ;;
    key:*) xdotool key --clearmodifiers "${step#key:}" ;;
    type:*) xdotool type --delay 40 "${step#type:}" ;;
  esac
done
chown -R "$owner" /mothrix/target/dev
