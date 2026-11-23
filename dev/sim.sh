#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# dev/sim.sh: copy the committed tree to the Mac (~/mothrix there), build
# the app for the simulator, install and start it there. Settings in
# dev/local.env: MOTHRIX_SIM (the simulator's UDID), MOTHRIX_SIM_CONFIG
# (Debug, the default, or Release), MOTHRIX_BUNDLE_ID.
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=/dev/null
[ -f dev/local.env ] && . dev/local.env
sim=${MOTHRIX_SIM:?set MOTHRIX_SIM in dev/local.env}
config=${MOTHRIX_SIM_CONFIG:-Debug}
bundle=${MOTHRIX_BUNDLE_ID:-de.tmrk.mothrix}
git archive --format=tar HEAD | dev/mac.sh 'mkdir -p ~/mothrix && tar -x -C ~/mothrix'
dev/mac.sh "xcrun simctl boot $sim 2>/dev/null; cd ~/mothrix
  CARGO_PROFILE=dev ONLY_SIMULATOR=1 caffeinate -i ios/build-rust.sh > ~/build-rust.log 2>&1 || { tail -30 ~/build-rust.log; exit 1; }
  cd ios && xcodegen -q
  caffeinate -i xcodebuild build -project Mothrix.xcodeproj -scheme Mothrix -configuration $config \
    -destination 'platform=iOS Simulator,id=$sim' -derivedDataPath ~/dd \
    CODE_SIGN_STYLE=Manual CODE_SIGN_IDENTITY=- DEVELOPMENT_TEAM= PROVISIONING_PROFILE_SPECIFIER= \
    > ~/xcodebuild.log 2>&1 || { grep -E 'error:' ~/xcodebuild.log | head -30; exit 1; }
  xcrun simctl terminate $sim $bundle >/dev/null 2>&1 || true
  xcrun simctl install $sim ~/dd/Build/Products/$config-iphonesimulator/Mothrix.app && xcrun simctl launch $sim $bundle"
