#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# dev/mac.sh 'command': run a command over SSH on the Mac that builds the
# iOS app (Xcode, Rust, XcodeGen; see docs/ios.md). Which Mac, and how to
# reach it, is in dev/local.env (not in git; start from
# dev/local.env.example):
#   MOTHRIX_MAC        user@host to SSH to
#   MOTHRIX_MAC_SSH    more ssh options ("-4 -i ~/.ssh/key -o IdentitiesOnly=yes")
#   MOTHRIX_MAC_SETUP  shell run there first (Xcode, cargo on PATH)
# What comes in on stdin goes to the command.
set -euo pipefail
here=$(dirname "$0")
# shellcheck source=/dev/null
[ -f "$here/local.env" ] && . "$here/local.env"
: "${MOTHRIX_MAC:?set MOTHRIX_MAC (user@host) in dev/local.env}"
# shellcheck disable=SC2086
exec ssh ${MOTHRIX_MAC_SSH:-} -o BatchMode=yes "$MOTHRIX_MAC" \
  "${MOTHRIX_MAC_SETUP:-} export PATH=\$HOME/.cargo/bin:\$PATH; $*"
