import json, os, socket, sys

path, marker = sys.argv[1:]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(path)
os.chmod(path, 0o600)
s.listen(1)
open(marker, "w").close()
conn, _ = s.accept()
data = b""
while b"\n" not in data:
    chunk = conn.recv(1 << 20)
    if not chunk:
        break
    data += chunk
conn.sendall((json.dumps({"v": 3, "ok": True, "pid": os.getpid()}) + "\n").encode())
