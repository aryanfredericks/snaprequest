#!/usr/bin/env python3
"""Smoke test of a running SnapRequest server (e.g. the Docker image).

Checks /healthz, executes a request through the REST API and through the WebSocket endpoint
(live trace events, then the result), and reads it back from the history.

Usage: scripts/docker-smoke.py [http://127.0.0.1:7878]
Only the Python standard library is used (a minimal WebSocket client is included).
"""
import base64
import json
import os
import socket
import struct
import sys
import urllib.request

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:7878"
HOST, PORT = BASE.removeprefix("http://").split(":")
PORT = int(PORT)


def api(method, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, method=method,
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read() or b"null")


def ws_jobs(message):
    """Sends one job over a WebSocket and returns every message until its result."""
    sock = socket.create_connection((HOST, PORT), timeout=30)
    key = base64.b64encode(os.urandom(16)).decode()
    sock.sendall((f"GET /api/v1/ws HTTP/1.1\r\nHost: {HOST}:{PORT}\r\nUpgrade: websocket\r\n"
                  f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
    head = b""
    while b"\r\n\r\n" not in head:
        head += sock.recv(1)
    assert head.startswith(b"HTTP/1.1 101"), head
    payload = json.dumps(message).encode()
    mask = os.urandom(4)
    header = bytes([0x81])
    header += bytes([0x80 | len(payload)]) if len(payload) < 126 else bytes([0x80 | 126]) + struct.pack(">H", len(payload))
    sock.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))

    def read(n):
        out = b""
        while len(out) < n:
            chunk = sock.recv(n - len(out))
            if not chunk:
                raise EOFError("connection closed")
            out += chunk
        return out

    messages = []
    while True:
        b0, b1 = read(2)
        length = b1 & 0x7F
        if length == 126:
            length = struct.unpack(">H", read(2))[0]
        elif length == 127:
            length = struct.unpack(">Q", read(8))[0]
        data = read(length)
        if b0 & 0x0F != 1:  # only text frames carry messages
            continue
        msg = json.loads(data)
        messages.append(msg)
        if msg["type"] in ("result", "error"):
            sock.close()
            return messages


def main():
    assert api("GET", "/healthz") == {"status": "ok"}
    version = api("GET", "/api/v1/version")
    print(f"server {version['version']} is up")

    # The server itself, as seen from inside its container (SNAPREQ_SMOKE_TARGET overrides).
    target = os.environ.get("SNAPREQ_SMOKE_TARGET", "http://127.0.0.1:7878/healthz")
    rest = api("POST", "/api/v1/requests/execute", {"request": {"method": "GET", "url": target}})
    assert rest["response"]["status"] == 200, rest
    print(f"REST execute: {rest['response']['status']} (history {rest['history_id']})")

    messages = ws_jobs({"type": "execute", "id": 1, "request": {"method": "GET", "url": target}})
    traces = [m for m in messages if m["type"] == "trace"]
    result = messages[-1]
    assert result["type"] == "result", result
    assert result["result"]["response"]["status"] == 200, result
    assert len(traces) >= 5, traces
    print(f"WebSocket execute: {len(traces)} live trace events, then status {result['result']['response']['status']}")

    entry = api("GET", f"/api/v1/history/{result['result']['history_id']}")
    assert entry["status"] == 200, entry
    blocked = api("POST", "/api/v1/requests/execute",
                  {"request": {"method": "GET", "url": "http://169.254.169.254/latest/meta-data/"}})
    assert "blocked by the outbound request policy" in blocked["error"]["message"], blocked
    print("cloud metadata endpoint blocked:", blocked["error"]["message"])
    print("smoke test passed")


if __name__ == "__main__":
    main()
