"""A small, bounded Chrome DevTools Protocol client for the Bridle window (WebView2 debug port).

Usage (Windows Python):  python cdp.py STEPS.json [--port 9222] [--limit 60]

STEPS.json is a list of steps, run in order:
  {"eval": "<javascript>"}      evaluate in the page and print the value
  {"shot": "C:\\path\\x.png"}    save a screenshot
  {"wait": 1.5}                 sleep (seconds)

Safety limits, all enforced: a hard watchdog that ends the process after --limit seconds whatever it is doing,
a timeout on every socket read, a cap on the size of one message, and a cap on the number of messages read.
This replaced a PowerShell client that blocked forever and leaked 32 GB (see the notes of 2026-11-03).
"""
import base64
import json
import os
import re
import socket
import struct
import sys
import threading
import time
import urllib.request

MAX_MESSAGE_BYTES = 16 * 1024 * 1024
MAX_MESSAGES = 5000
READ_TIMEOUT = 15.0  # seconds a single read may wait; `--read-timeout` raises it for slow steps


def die(message, code=2):
    print(f"cdp: {message}", flush=True)
    os._exit(code)


def read_exact(sock, count, deadline):
    chunks, remaining = [], count
    while remaining:
        left = deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError("deadline reached while reading")
        sock.settimeout(min(READ_TIMEOUT, left))
        chunk = sock.recv(min(remaining, 65536))
        if not chunk:
            raise ConnectionError("the connection was closed")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def send_text(sock, text):
    payload = text.encode("utf-8")
    header = bytearray([0x81])
    length = len(payload)
    if length < 126:
        header.append(0x80 | length)
    elif length < 65536:
        header.append(0x80 | 126)
        header += struct.pack(">H", length)
    else:
        header.append(0x80 | 127)
        header += struct.pack(">Q", length)
    mask = os.urandom(4)
    header += mask
    masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    sock.sendall(bytes(header) + masked)


def recv_message(sock, deadline):
    data, total = b"", 0
    while True:
        first, second = read_exact(sock, 2, deadline)
        opcode, length = first & 0x0F, second & 0x7F
        if length == 126:
            length = struct.unpack(">H", read_exact(sock, 2, deadline))[0]
        elif length == 127:
            length = struct.unpack(">Q", read_exact(sock, 8, deadline))[0]
        total += length
        if total > MAX_MESSAGE_BYTES:
            raise ValueError("a message is larger than the limit")
        payload = read_exact(sock, length, deadline) if length else b""
        if opcode == 0x8:
            raise ConnectionError("the page closed the connection")
        if opcode == 0x9:  # ping: answer with a pong carrying the same bytes
            sock.sendall(bytes([0x8A, 0x80 | len(payload)]) + b"\0\0\0\0" + payload)
            continue
        if opcode in (0x1, 0x0, 0x2):
            data += payload
            if first & 0x80:
                return data.decode("utf-8", "replace")


def connect(port, deadline, match=None):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=10) as response:
        targets = json.loads(response.read(1_000_000))
    page = next((t for t in targets if t.get("type") == "page" and (not match or match in t.get("url", ""))), None)
    if not page:
        die("no page target on the debug port")
    url = page["webSocketDebuggerUrl"]
    host, rest = url[len("ws://"):].split("/", 1)
    name, _, number = host.partition(":")
    sock = socket.create_connection((name, int(number or 80)), timeout=10)
    key = base64.b64encode(os.urandom(16)).decode()
    sock.sendall((f"GET /{rest} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                  f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
    reply = b""
    while b"\r\n\r\n" not in reply:
        left = deadline - time.monotonic()
        if left <= 0 or len(reply) > 65536:
            die("the handshake did not finish")
        sock.settimeout(min(READ_TIMEOUT, left))
        chunk = sock.recv(4096)
        if not chunk:
            die("the connection closed during the handshake")
        reply += chunk
    if b" 101 " not in reply.split(b"\r\n", 1)[0]:
        die("the page refused the connection")
    return sock


class Session:
    def __init__(self, sock, deadline):
        self.sock, self.deadline, self.next_id = sock, deadline, 0

    def call(self, method, params=None):
        self.next_id += 1
        wanted = self.next_id
        send_text(self.sock, json.dumps({"id": wanted, "method": method, "params": params or {}}))
        for _ in range(MAX_MESSAGES):
            message = json.loads(recv_message(self.sock, self.deadline))
            if message.get("id") == wanted:
                return message
        raise RuntimeError("too many messages without an answer")


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    args = sys.argv[1:]
    if not args:
        die(__doc__)
    global READ_TIMEOUT
    port, limit = 9222, 60.0
    if "--read-timeout" in args:
        READ_TIMEOUT = float(args[args.index("--read-timeout") + 1])
    if "--port" in args:
        port = int(args[args.index("--port") + 1])
    if "--limit" in args:
        limit = float(args[args.index("--limit") + 1])
    # The watchdog ends the process whatever the main thread is doing.
    watchdog = threading.Timer(limit + 5, lambda: die("the watchdog ended the process", 3))
    watchdog.daemon = True
    watchdog.start()
    deadline = time.monotonic() + limit
    steps = json.load(open(args[0], encoding="utf-8"))
    match = args[args.index("--match") + 1] if "--match" in args else None
    session = Session(connect(port, deadline, match), deadline)
    failures = []
    for index, step in enumerate(steps):
        if "wait" in step:
            time.sleep(min(float(step["wait"]), max(0.0, deadline - time.monotonic())))
        elif "eval" in step:
            reply = session.call("Runtime.evaluate", {"expression": step["eval"], "awaitPromise": True, "returnByValue": True})
            result = reply.get("result", {})
            if result.get("exceptionDetails"):
                text = result["exceptionDetails"].get("exception", {}).get("description") or result["exceptionDetails"].get("text")
                shown = f"EXCEPTION: {text}"
                ok = False
            else:
                value = result.get("result", {}).get("value")
                shown = value if isinstance(value, str) else json.dumps(value)
                ok = True
            # `expect` is a regular expression the output must match; a step without one only has to run.
            if ok and "expect" in step and not re.search(step["expect"], shown, re.S):
                ok = False
                shown = f"{shown}\n    expected /{step['expect']}/"
            name = step.get("name", f"step {index}")
            print(f"[{'ok' if ok else 'FAIL'}] {name}: {shown[:600]}", flush=True)
            if not ok:
                failures.append(name)
        elif "insert" in step:
            session.call("Input.insertText", {"text": step["insert"]})
            print(f"[{index}] inserted", flush=True)
        elif "press" in step:
            key = step["press"]  # {"key": "Enter", "code": "Enter", "vk": 13, "ctrl": false}
            mods = (2 if key.get("ctrl") else 0) | (8 if key.get("shift") else 0)
            base = {"key": key["key"], "code": key.get("code", key["key"]), "windowsVirtualKeyCode": key.get("vk", 0), "modifiers": mods}
            session.call("Input.dispatchKeyEvent", {**base, "type": "keyDown", "text": key.get("text", "")})
            session.call("Input.dispatchKeyEvent", {**base, "type": "keyUp"})
            print(f"[{index}] pressed {key['key']}", flush=True)
        elif "shot" in step:
            reply = session.call("Page.captureScreenshot", {"format": "png"})
            with open(step["shot"], "wb") as out:
                out.write(base64.b64decode(reply["result"]["data"]))
            print(f"[{index}] saved {step['shot']}", flush=True)
    session.sock.close()
    if failures:
        die(f"{len(failures)} check(s) failed: {', '.join(failures)}", 1)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:  # report, never hang
        die(f"{type(error).__name__}: {error}", 1)
