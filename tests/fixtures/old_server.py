"""Stands in for an imux server that does not know the version check: it accepts a connection,
reads what the client sends first, and hangs up without a word (what a real older server does when
a frame has a tag it cannot decode). Usage: old_server.py SOCKET_PATH [CONNECTIONS]."""
import os
import socket
import sys

path = sys.argv[1]
count = int(sys.argv[2]) if len(sys.argv) > 2 else 1
if os.path.exists(path):
    os.unlink(path)
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
os.chmod(path, 0o600)
server.listen(8)
print("ready", flush=True)
for _ in range(count):
    conn, _ = server.accept()
    conn.settimeout(1.0)
    try:
        conn.recv(4096)
    except OSError:
        pass
    conn.close()
server.close()
