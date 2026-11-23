# SPDX-License-Identifier: MPL-2.0
# Messages for screenshots, after dev/server.sh and the other seeds: in the
# Lunch Club, Cleo posts the moth as a picture with a caption, Ben a code
# block, Amy a reply; people react to them. Amy also starts a direct chat.
#
#   python3 -I dev/seed-showcase.py target/dev/seed.json
import json, sys, urllib.request, urllib.parse, uuid

seed = json.load(open(sys.argv[1]))
room = seed["room_id"]
q = urllib.parse.quote
base = "http://127.0.0.1:16167"


def call(method, path, body, token, raw=None, kind="application/json"):
    data = raw if raw is not None else (None if body is None else json.dumps(body).encode())
    request = urllib.request.Request(base + path, method=method, data=data)
    request.add_header("Authorization", "Bearer " + token)
    request.add_header("Content-Type", kind)
    return json.loads(urllib.request.urlopen(request, timeout=30).read())


def send(who, content, kind="m.room.message"):
    path = f"/_matrix/client/v3/rooms/{q(room)}/send/{kind}/{uuid.uuid4().hex}"
    return call("PUT", path, content, seed["users"][who])["event_id"]


def react(who, event, key):
    send(who, {"m.relates_to": {"rel_type": "m.annotation", "event_id": event, "key": key}}, "m.reaction")


picture = open("assets/mothrix-logo-512.png", "rb").read()
uploaded = call("POST", "/_matrix/media/v3/upload?filename=moth.png", None, seed["users"]["cleo"],
                raw=picture, kind="image/png")["content_uri"]
moth = send("cleo", {
    "msgtype": "m.image", "body": "Look who was at the window this morning 🦋",
    "filename": "moth.png", "url": uploaded,
    "info": {"mimetype": "image/png", "w": 512, "h": 512, "size": len(picture)},
})
for who, key in [("amy", "😍"), ("ben", "😍"), ("dan", "🌙")]:
    react(who, moth, key)

code = ("Here is the fix for the lunch bot:\n\n```rust\nfn main() {\n"
        "    let moth = Moth::new();\n    moth.fly_to(\"the light\");\n}\n```")
fix = send("ben", {"msgtype": "m.text", "body": code})
react("cleo", fix, "🚀")
react("tester", fix, "👍")

send("amy", {
    "msgtype": "m.text", "body": "> <@ben:hs.test> Here is the fix for the lunch bot\n\nShip it! 🍕",
    "m.relates_to": {"m.in_reply_to": {"event_id": fix}},
})

# A direct chat with Amy, with something new in it.
tester = seed["users"]["tester"]
direct = call("POST", "/_matrix/client/v3/createRoom", {
    "is_direct": True, "preset": "trusted_private_chat", "invite": ["@tester:hs.test"],
}, seed["users"]["amy"])["room_id"]
call("POST", f"/_matrix/client/v3/join/{q(direct)}", {}, tester)
call("PUT", f"/_matrix/client/v3/user/{q('@tester:hs.test')}/account_data/m.direct",
     {"@amy:hs.test": [direct]}, tester)
call("PUT", f"/_matrix/client/v3/rooms/{q(direct)}/send/m.room.message/{uuid.uuid4().hex}",
     {"msgtype": "m.text", "body": "Moth watching tonight? 🌙 I'll bring the lamp."}, seed["users"]["amy"])

print("ready")
