import json, sys, time, urllib.request, urllib.parse
HS="http://127.0.0.1:16167"; seed=json.load(open(sys.argv[1])); room=seed["room_id"]; q=urllib.parse.quote
def call(user, method, path, body=None):
    req=urllib.request.Request(HS+path, method=method, data=json.dumps(body or {}).encode())
    req.add_header("Authorization","Bearer "+seed["users"][user]); req.add_header("Content-Type","application/json")
    return json.loads(urllib.request.urlopen(req, timeout=20).read() or b"{}")
last=call("amy","GET",f"/_matrix/client/v3/rooms/{q(room)}/messages?dir=b&limit=1")["chunk"][0]["event_id"]
for u in ("amy","ben","cleo"):
    call(u,"POST",f"/_matrix/client/v3/rooms/{q(room)}/receipt/m.read/{q(last)}")
time.sleep(float(sys.argv[2]))
call("amy","PUT",f"/_matrix/client/v3/rooms/{q(room)}/typing/{q('@amy:hs.test')}",{"typing":True,"timeout":20000})
call("dan","PUT",f"/_matrix/client/v3/rooms/{q(room)}/typing/{q('@dan:hs.test')}",{"typing":True,"timeout":20000})
