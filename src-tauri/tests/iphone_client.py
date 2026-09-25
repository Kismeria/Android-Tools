"""Plays the phone for the iphone.rs server test: page over HTTPS, then frames over WSS."""
import base64, json, os, socket, ssl, sys, urllib.parse, urllib.request, urllib.error

url = sys.argv[1]
u = urllib.parse.urlsplit(url)
ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE

page = urllib.request.urlopen(url, context=ctx, timeout=5).read().decode()
assert "Android Tools" in page and "getUserMedia" in page, "page"
try:
    urllib.request.urlopen(f"https://{u.netloc}/?k=wrong", context=ctx, timeout=5)
    raise SystemExit("wrong token accepted")
except urllib.error.HTTPError as e:
    assert e.code == 403, e.code
print("page ok, token checked")

raw = socket.create_connection((u.hostname, u.port), timeout=5)
s = ctx.wrap_socket(raw, server_hostname=u.hostname)
key = base64.b64encode(os.urandom(16)).decode()
s.sendall((f"GET /ws?{u.query} HTTP/1.1\r\nHost: {u.netloc}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
           f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
           "User-Agent: Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X)\r\n\r\n").encode())
head = b""
while b"\r\n\r\n" not in head:
    head += s.recv(1024)
assert head.startswith(b"HTTP/1.1 101"), head
print("websocket ok")

def frame(opcode, payload):
    mask = os.urandom(4)
    n = len(payload)
    hdr = bytes([0x80 | opcode])
    if n < 126:
        hdr += bytes([0x80 | n])
    elif n < 65536:
        hdr += bytes([0x80 | 126]) + n.to_bytes(2, "big")
    else:
        hdr += bytes([0x80 | 127]) + n.to_bytes(8, "big")
    return hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload))

s.sendall(frame(1, json.dumps({"type": "state", "camera": True, "width": 1280, "height": 720, "facing": "front"}).encode()))
jpeg = b"\xff\xd8" + os.urandom(70000) + b"\xff\xd9"  # sink only checks the SOI marker
for _ in range(10):
    s.sendall(frame(2, jpeg))
s.sendall(frame(8, b"\x03\xe8"))
s.close()
print("sent 10 frames")
