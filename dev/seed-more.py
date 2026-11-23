import json, sys, urllib.request, urllib.parse
seed=json.load(open(sys.argv[1])); room=seed["room_id"]; q=urllib.parse.quote
def call(method, path, body, tok):
    r=urllib.request.Request("http://127.0.0.1:16167"+path, method=method, data=None if body is None else json.dumps(body).encode()); r.add_header("Authorization","Bearer "+tok); r.add_header("Content-Type","application/json")
    return json.loads(urllib.request.urlopen(r, timeout=20).read())
t=seed["users"]["tester"]
msgs=call("GET",f"/_matrix/client/v3/rooms/{q(room)}/messages?dir=f&limit=100",None,t)["chunk"]
poll=next(e for e in msgs if e["type"]=="org.matrix.msc3381.poll.start")
answers=poll["content"]["org.matrix.msc3381.poll.start"]["answers"]
sushi=next(a["id"] for a in answers if "Sushi" in json.dumps(a))
# Eve answers the older-format poll the way the IServ messenger does.
call("PUT",f"/_matrix/client/v3/rooms/{q(room)}/send/m.poll.response/iserv1", {"m.poll.response":{"answers":[sushi]},"m.relates_to":{"event_id":poll["event_id"],"rel_type":"m.reference"}}, seed["users"]["eve"])
md="Use `cargo run` to start, it is **really** fast and *quite* small. Not ~~slow~~ at all; snake_case_name and 2 * 3 * 4 stay as typed."
call("PUT",f"/_matrix/client/v3/rooms/{q(room)}/send/m.room.message/md1", {"msgtype":"m.text","body":md}, seed["users"]["ben"])
for n in ["Dev Chat","Family"]:
    call("POST","/_matrix/client/v3/createRoom", {"name": n}, t)
print("ready")
