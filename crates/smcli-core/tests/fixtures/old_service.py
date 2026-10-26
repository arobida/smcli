import json, os, socket, sys

path, log, ready = sys.argv[1:]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(path)
os.chmod(path, 0o600)
s.listen(8)
open(ready, "w").close()
while True:
    conn, _ = s.accept()
    data = b""
    while b"\n" not in data:
        chunk = conn.recv(1 << 20)
        if not chunk:
            break
        data += chunk
    req = json.loads(data.decode() or "{}")
    with open(log, "a") as fh:
        fh.write(f"{req.get('v')} {req.get('op')}\n")
    if req.get("v") == 1 and req.get("op") == "shutdown":
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass
        conn.sendall((json.dumps({"v": 1, "ok": True, "pid": os.getpid()}) + "\n").encode())
        break
    conn.sendall(b'{"v":1,"ok":false,"error":{"code":"version","message":"Protocol version 1 required."}}\n')
    conn.close()
