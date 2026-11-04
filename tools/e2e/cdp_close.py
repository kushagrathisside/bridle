"""Closes the browser that owns a debug port (Browser.close). Usage: python cdp_close.py PORT"""
import json, sys, time, urllib.request, socket, base64, os, threading
sys.path.insert(0, os.path.dirname(__file__))
import cdp
threading.Timer(20, lambda: os._exit(3)).start()
port = int(sys.argv[1])
with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version", timeout=5) as r:
    url = json.loads(r.read(100000))["webSocketDebuggerUrl"]
host, rest = url[len("ws://"):].split("/", 1)
name, _, number = host.partition(":")
sock = socket.create_connection((name, int(number)), timeout=5)
key = base64.b64encode(os.urandom(16)).decode()
sock.sendall((f"GET /{rest} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
sock.settimeout(5); sock.recv(4096)
cdp.send_text(sock, json.dumps({"id": 1, "method": "Browser.close"}))
print("Browser.close sent")
