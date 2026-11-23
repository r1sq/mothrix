import json, sys, time, urllib.request, urllib.error
HS = "http://127.0.0.1:16167"
REG_TOKENS = sys.argv[1].split(",")
def call(method, path, body=None, token=None):
    req = urllib.request.Request(HS + path, method=method, data=None if body is None else json.dumps(body).encode())
    req.add_header("Content-Type", "application/json")
    if token: req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(req, timeout=20) as r: return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")
def register(user, password):
    for token in REG_TOKENS:
        try:
            return register_with(user, password, token)
        except SystemExit as e:
            last = e
    raise last
def register_with(user, password, REG_TOKEN):
    body = {"username": user, "password": password, "initial_device_display_name": "seed"}
    code, res = call("POST", "/_matrix/client/v3/register", body)
    for _ in range(4):
        if code == 200: return res
        session = res.get("session")
        stages = [s for f in res.get("flows", []) for s in f["stages"]]
        done = res.get("completed", [])
        todo = [s for s in stages if s not in done]
        stage = "m.login.registration_token" if "m.login.registration_token" in todo else "m.login.dummy"
        auth = {"type": stage, "session": session}
        if stage == "m.login.registration_token": auth["token"] = REG_TOKEN
        code, res = call("POST", "/_matrix/client/v3/register", {**body, "auth": auth})
    raise SystemExit(f"register {user} failed: {code} {res}")
users = {}
for name, display in [("tester", "Tester"), ("amy", "Amy Winter"), ("ben", "Ben Okafor"), ("cleo", "Cleo Marchetti"), ("dan", "Dan Sørensen"), ("eve", "Eve Tanaka")]:
    r = register(name, "pw-" + name + "-12345")
    users[name] = r
    call("PUT", f"/_matrix/client/v3/profile/{r['user_id']}/displayname", {"displayname": display}, r["access_token"])
    print("registered", r["user_id"])
amy = users["amy"]["access_token"]
code, room = call("POST", "/_matrix/client/v3/createRoom", {"name": "Lunch Club", "preset": "public_chat", "room_alias_name": "lunch"}, amy)
room_id = room["room_id"]; print("room", room_id)
for name in users:
    if name != "amy":
        print("join", name, call("POST", f"/_matrix/client/v3/join/{room_id}", {}, users[name]["access_token"])[0])
txn = [0]
def send(user, etype, content):
    txn[0] += 1
    code, res = call("PUT", f"/_matrix/client/v3/rooms/{room_id}/send/{etype}/seed{txn[0]}", content, users[user]["access_token"])
    assert code == 200, (code, res)
    time.sleep(0.15)
    return res["event_id"]
def text(user, body): return send(user, "m.room.message", {"msgtype": "m.text", "body": body})
def poll(user, question, answers, kind="disclosed", max_sel=1):
    return send(user, "org.matrix.msc3381.poll.start", {
        "org.matrix.msc1767.text": question,
        "org.matrix.msc3381.poll.start": {"question": {"org.matrix.msc1767.text": question}, "kind": "org.matrix.msc3381.poll." + kind,
            "max_selections": max_sel, "answers": [{"id": f"a{i}", "org.matrix.msc1767.text": a} for i, a in enumerate(answers)]}})
def vote(user, poll_id, ids):
    return send(user, "org.matrix.msc3381.poll.response", {"m.relates_to": {"rel_type": "m.reference", "event_id": poll_id},
        "org.matrix.msc3381.poll.response": {"answers": ids}})
def end(user, poll_id):
    return send(user, "org.matrix.msc3381.poll.end", {"m.relates_to": {"rel_type": "m.reference", "event_id": poll_id},
        "org.matrix.msc1767.text": "The poll has ended.", "org.matrix.msc3381.poll.end": {}})
text("amy", "Hi all, quick poll about Friday lunch 👇")
p1 = poll("amy", "Where should we go for lunch on Friday?", ["Pizza place around the corner", "Sushi", "Tacos", "Just bring your own"])
vote("ben", p1, ["a0"]); vote("cleo", p1, ["a1"]); vote("dan", p1, ["a0"]); vote("eve", p1, ["a2"]); vote("amy", p1, ["a0"])
vote("cleo", p1, ["a0"])  # changed her mind
text("ben", "Pizza it is, apparently")
p2 = poll("ben", "Which days work for the team offsite? (pick up to 3)", ["Monday", "Tuesday", "Wednesday", "Thursday"], max_sel=3)
vote("amy", p2, ["a0", "a2"]); vote("cleo", p2, ["a2", "a3"]); vote("dan", p2, ["a2"]); vote("eve", p2, ["a1", "a2", "a3"])
p3 = poll("cleo", "Secret ballot: who brings cake next week?", ["Amy", "Ben", "Dan"], kind="undisclosed")
vote("amy", p3, ["a1"]); vote("ben", p3, ["a2"]); vote("dan", p3, ["a1"])
p4 = poll("dan", "Coffee or tea?", ["Coffee", "Tea"])
vote("amy", p4, ["a0"]); vote("ben", p4, ["a0"]); vote("cleo", p4, ["a1"]); vote("eve", p4, ["a0"])
end("dan", p4)
json.dump({"room_id": room_id, "users": {k: v["access_token"] for k, v in users.items()}, "polls": [p1, p2, p3, p4]}, open(sys.argv[2], "w"))
print("seeded")
