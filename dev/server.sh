#!/bin/bash
# SPDX-License-Identifier: MPL-2.0
# A throwaway Matrix homeserver for trying mothrix against: starts it in
# docker, fills it with people, a room, polls and messages, and signs
# "tester" in as mothrix's default profile. Everything it writes goes to
# target/dev, away from your real mothrix data.
#
#   dev/server.sh          start fresh (stops a running one first)
#   dev/server.sh stop     stop it
#   . dev/env.sh           point mothrix in this shell at the test data
#
# MOTHRIX_DEV_BIND=<this machine's LAN address> dev/server.sh: reachable on the LAN too
# (for the iOS simulator on the Mac); otherwise only from this machine.
# Everyone on that network can then sign in as the people below: their
# passwords are in this script and in the docs. New accounts take the
# registration token, which is made anew at every start. Only on a
# network you trust, with nothing in the rooms that matters, and
# "dev/server.sh stop" afterwards.
#
# MOTHRIX_DEV_IMAGE=<image> takes another server image than the one below.
set -eu
cd "$(dirname "$0")/.."
name=mothrix-dev-hs
docker rm -f "$name" >/dev/null 2>&1 || true
[ "${1:-}" = stop ] && exit 0

# "latest" is whatever the registry has on the day it is pulled. To run the
# same server every time, name a digest here (docker image inspect
# --format '{{index .RepoDigests 0}}' on an image that was tried).
image=${MOTHRIX_DEV_IMAGE:-forgejo.ellis.link/continuwuation/continuwuity:latest}
# The token new accounts need: only this run's seed knows it.
seedtoken=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
settings=""
# The server reads either prefix, depending on its version.
for prefix in CONTINUWUITY CONDUWUIT; do
  settings="$settings -e ${prefix}_SERVER_NAME=hs.test -e ${prefix}_DATABASE_PATH=/var/lib/continuwuity"
  settings="$settings -e ${prefix}_PORT=6167 -e ${prefix}_ADDRESS=0.0.0.0 -e ${prefix}_ALLOW_REGISTRATION=true"
  settings="$settings -e ${prefix}_REGISTRATION_TOKEN=$seedtoken -e ${prefix}_ALLOW_FEDERATION=false -e ${prefix}_LOG=warn"
done
# shellcheck disable=SC2086
docker run -d --name "$name" $settings --tmpfs /var/lib/continuwuity -p 127.0.0.1:16167:6167 ${MOTHRIX_DEV_BIND:+-p "$MOTHRIX_DEV_BIND:16167:6167"} "$image" >/dev/null
sleep 7
# The first account needs the token the server prints at its first start.
token=$(docker logs "$name" 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -o 'registration token [A-Za-z0-9]*' | awk '{print $3}' | head -1)

rm -rf target/dev
mkdir -p target/dev
python3 -I dev/seed.py "$seedtoken,$token" target/dev/seed.json > target/dev/seed.log 2>&1 || { tail -5 target/dev/seed.log; exit 1; }

. dev/env.sh
binary=target/debug/mothrix
[ -x "$binary" ] || cargo build
MOTHRIX_PASSWORD=pw-tester-12345 "$binary" login http://127.0.0.1:16167 --user tester | tail -1
echo "Ready. People: tester, amy, ben, cleo, dan, eve (password: pw-<name>-12345)."
echo "Now: . dev/env.sh && target/debug/mothrix"
if [ -n "${MOTHRIX_DEV_BIND:-}" ]; then
  echo "Listening on $MOTHRIX_DEV_BIND:16167 too: anyone there can sign in with these passwords. Stop it with: dev/server.sh stop"
fi
