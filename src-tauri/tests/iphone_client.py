"""Plays the phone for the iphone.rs server test: page over HTTPS, a poll that receives the
start command, then frames over one keep-alive connection."""
import http.client, json, os, ssl, sys, urllib.parse

url = sys.argv[1]
u = urllib.parse.urlsplit(url)
ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE

conn = http.client.HTTPSConnection(u.hostname, u.port, context=ctx, timeout=15)

def get(path, method="GET", body=None, headers=None):
    conn.request(method, path, body=body, headers=headers or {})
    r = conn.getresponse()
    return r.status, r.read()

status, page = get(f"/?{u.query}")
assert status == 200 and b"Android Tools" in page and b"getUserMedia" in page, "page"
status, _ = get("/?k=wrong")
assert status == 403, status
conn.close()  # the 403 closes the connection
print("page ok, token checked")

q = f"{u.query}&sid=phone1"
ua = {"User-Agent": "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X)"}
status, body = get(f"/poll?{q}&hello=1", headers=ua)
assert status == 200, status
cmds = json.loads(body)
assert cmds and cmds[0]["cmd"] == "start", cmds
print("poll ok:", cmds)

state = json.dumps({"type": "state", "camera": True, "width": 1280, "height": 720, "facing": "back"})
status, _ = get(f"/state?{q}", "POST", state, {"Content-Type": "application/json"})
assert status == 200, status

jpeg = b"\xff\xd8" + os.urandom(70000) + b"\xff\xd9"  # the sink only checks the SOI marker
for _ in range(10):
    status, _ = get(f"/frame?{q}", "POST", jpeg, {"Content-Type": "image/jpeg"})
    assert status == 200, status
print("sent 10 frames on one connection")

# A second page that is not a fresh load cannot take over the connected phone.
status, _ = get(f"/poll?{u.query}&sid=phone2", headers=ua)
assert status == 409, status
print("second page refused")
