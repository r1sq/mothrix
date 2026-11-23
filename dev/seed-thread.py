import json, sys, time, urllib.request, urllib.parse
HS = "http://127.0.0.1:16167"
seed = json.load(open(sys.argv[1]))
room = seed["room_id"]
def send(user, content, kind="m.room.message"):
    token = seed["users"][user]
    if isinstance(token, dict): token = token["access_token"]
    url = f"{HS}/_matrix/client/v3/rooms/{urllib.parse.quote(room)}/send/{kind}/{time.time_ns()}"
    req = urllib.request.Request(url, method="PUT", data=json.dumps(content).encode())
    req.add_header("Authorization", "Bearer " + token); req.add_header("Content-Type", "application/json")
    return json.loads(urllib.request.urlopen(req, timeout=20).read())["event_id"]
root = send("amy", {"msgtype": "m.text", "body": "Thread root: where do we eat on Friday?"})
last = root
for user, text in [("ben", "The noodle place"), ("cleo", "Too far. Pizza around the corner?"), ("ben", "Fine, pizza it is")]:
    last = send(user, {"msgtype": "m.text", "body": text, "m.relates_to": {"rel_type": "m.thread", "event_id": root, "is_falling_back": True, "m.in_reply_to": {"event_id": last}}})
send("dan", {"msgtype": "m.text", "body": "Unrelated: anyone seen my umbrella?"})
print(root)
