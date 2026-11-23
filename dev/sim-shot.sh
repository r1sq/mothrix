#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# dev/sim-shot.sh NAME: a screenshot of the simulator (MOTHRIX_SIM in
# dev/local.env) as target/dev/sim/NAME.png, and NAME-small.png in points
# (402 wide), the size AXe's coordinates use:
# `dev/mac.sh axe tap -x 70 -y 246 --udid …`.
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=/dev/null
[ -f dev/local.env ] && . dev/local.env
sim=${MOTHRIX_SIM:?set MOTHRIX_SIM in dev/local.env}
mkdir -p target/dev/sim
dev/mac.sh "xcrun simctl io $sim screenshot /tmp/mothrix-shot.png >/dev/null 2>&1 && cat /tmp/mothrix-shot.png" > "target/dev/sim/$1.png"
convert "target/dev/sim/$1.png" -resize 402x "target/dev/sim/$1-small.png"
echo "target/dev/sim/$1-small.png"
