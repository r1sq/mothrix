import json, sys, urllib.request, urllib.parse
HS = "http://127.0.0.1:16167"
seed = json.load(open(sys.argv[1]))
tok = seed["users"]["tester"]
def call(method, path, body=None):
    req = urllib.request.Request(HS + path, method=method, data=None if body is None else json.dumps(body).encode())
    req.add_header("Authorization", "Bearer " + tok); req.add_header("Content-Type", "application/json")
    return json.loads(urllib.request.urlopen(req, timeout=20).read() or b"{}")
def create(name, space=False):
    body = {"name": name, "preset": "private_chat"}
    if space: body["creation_content"] = {"type": "m.space"}
    return call("POST", "/_matrix/client/v3/createRoom", body)["room_id"]
def child(space, room):
    call("PUT", f"/_matrix/client/v3/rooms/{urllib.parse.quote(space)}/state/m.space.child/{urllib.parse.quote(room)}", {"via": ["hs.test"]})
school = create("School", True); sub = create("Class 7b", True)
maths = create("Maths"); hall = create("Assembly Hall")
child(school, seed["room_id"]); child(school, hall); child(school, sub); child(sub, maths)
print("ok")
